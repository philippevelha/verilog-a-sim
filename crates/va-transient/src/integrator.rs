//! Time integration with adaptive timestep and local-truncation-error (LTE) control.
//!
//! # Status
//!
//! [`Method::BackwardEuler`] and [`Method::Trapezoidal`] are implemented, both with adaptive
//! step-size control; [`Method::Gear`] is not (returns [`TransientError::UnsupportedMethod`],
//! never silently falls back to another method).
//!
//! **The LTE estimate is an embedded pair, not a rigorous divided-difference truncation-error
//! calculation.** Each accepted step computes *both* methods' result from the same starting
//! point and step size — one as the reported solution, the other purely to estimate error —
//! and uses their difference as an error proxy, the same spirit as an embedded Runge-Kutta
//! pair (e.g. RK45's 4th/5th-order combination). A textbook SPICE-style implementation would
//! instead estimate the next unused Taylor term from a divided difference of several *past*
//! accepted points, needing no second solve per step; that's the more rigorous approach and
//! remains future work, but it needs a longer history buffer this module doesn't keep yet.
//! The honest cost of the current approach: every step is ~2x a fixed-step solve. Its honest
//! benefit: it needed no new history-tracking infrastructure and is simple enough to verify by
//! direct comparison against the analytic RC solution (`tests::rc_transient_matches_analytic`).
//!
//! [`run_with_events`] wires `crate::events::EventQueue` in: breakpoints clamp the adaptive
//! step so it never overshoots a forced timepoint, and crossing watches are checked against
//! every pair of consecutive accepted points. [`run`] is a thin wrapper over it with an empty
//! queue, kept for callers that don't need either.
//!
//! **Ladder rung 6 (ring oscillator) is closed** — `va-abi::reference` gained a gain-capable
//! device (`Bjt`, a three-terminal simplified Ebers-Moll NPN), and
//! `tests::ring_oscillator_sustains_oscillation` builds a 3-stage RC-coupled common-emitter
//! BJT ring (instances constructed directly, no netlist — `va-netlist` has no 3-terminal-
//! device grammar) and runs it through exactly this module's DC (`va_core::dc::
//! operating_point`, gmin-stepping) and transient machinery. The DC solve lands on a real but
//! *unstable* equilibrium (Newton doesn't know or care that a fixed point is unstable, only
//! that its residual is zero); a small perturbation plus deliberately mismatched per-stage
//! component values (breaking the three-way symmetry a real circuit's tolerances always break)
//! is enough to diverge into genuine, growing oscillation — confirmed by watching one node's
//! voltage cross its own DC bias repeatedly with a deepening trough over time, not just guessed
//! at. **Limitation, found empirically while tuning it, not hidden:** as the oscillation grows,
//! it eventually drives a junction into strong forward bias on both sides at once, where this
//! simplified (no saturation-charge smoothing) `Bjt` model's exponential terms grow large
//! enough that the embedded-pair LTE estimator stops agreeing at any step size — the test's
//! `tstop` stays comfortably inside the well-behaved region rather than chasing that edge. No
//! golden/ngspice comparison yet (`docs/roadmap.md`'s T6.3 harness is still pending) — this
//! validates that it oscillates, not a specific frequency.

use crate::events::EventQueue;
use crate::TransientError;
use va_abi::stamps::DenseStamp;
use va_abi::ModelInstance;
use va_core::convergence;
use va_core::linsolve;

/// Integration method for the charge channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Backward Euler (first order, robust startup).
    BackwardEuler,
    /// Trapezoidal (second order).
    Trapezoidal,
    /// Gear / BDF up to the given order.
    Gear,
}

/// Transient run controls.
#[derive(Clone, Copy, Debug)]
pub struct TranConfig {
    /// Start time (s).
    pub tstart: f64,
    /// Stop time (s).
    pub tstop: f64,
    /// Maximum timestep (s) — the adaptive controller only ever shrinks below this, starting
    /// from it, never grows past it.
    pub tstep: f64,
    /// Minimum timestep (s). If the LTE controller would shrink below this without meeting
    /// tolerance, integration stops with [`TransientError::TimestepUnderflow`] rather than
    /// silently accepting an out-of-tolerance step.
    pub tstep_min: f64,
    /// Integration method.
    pub method: Method,
    /// Relative LTE tolerance (dimensionless), combined with [`Self::lte_abstol`] the same way
    /// `va-core`'s Newton `reltol`/`abstol` combine: `scale = lte_reltol·|x| + lte_abstol`.
    pub lte_reltol: f64,
    /// Absolute LTE tolerance (in the unknown's own units — volts for a node, amps for a
    /// branch current). Must be strictly positive: it is the error scale's floor when `x` is
    /// near zero, so a zero value would make the error-vs-tolerance ratio divide by zero.
    pub lte_abstol: f64,
}

/// A sampled transient waveform: aligned time and solution-vector columns.
#[derive(Clone, Debug, Default)]
pub struct Waveform {
    /// Time points (s).
    pub t: Vec<f64>,
    /// Solution vectors, one row per time point.
    pub x: Vec<Vec<f64>>,
    /// Detected crossings, in the order they occurred: `(watch_index, time)`, where
    /// `watch_index` indexes the [`EventQueue::watches`] slice passed to
    /// [`run_with_events`] (always empty for [`run`], which watches nothing).
    pub crossings: Vec<(usize, f64)>,
}

/// The companion-model contribution a discretization scheme adds to the per-iteration nodal
/// equation `residual(x) + coeff * charge(x) + offset = 0`.
///
/// Both implemented methods reduce to this same shape (only how `coeff`/`offset` are derived
/// from history differs), so the Newton loop below needs no per-method branching at all.
struct Companion {
    coeff: f64,
    offset: Vec<f64>,
}

impl Companion {
    /// `dQ/dt ≈ (Q(x) − Q_prev) / h`: `residual(x) + (Q(x) − Q_prev)/h = 0`.
    fn backward_euler(q_prev: &[f64], h: f64) -> Self {
        Companion {
            coeff: 1.0 / h,
            offset: q_prev.iter().map(|q| -q / h).collect(),
        }
    }

    /// `Q(x) − Q_prev = h/2 · (dQ/dt|_new + dQ/dt|_prev)` and `dQ/dt = −residual` (the nodal
    /// equation), rearranged to the same `residual(x) + coeff·charge(x) + offset = 0` shape:
    /// `residual(x) + (2/h)·charge(x) + (residual_prev − (2/h)·Q_prev) = 0`.
    ///
    /// **Only for rows `is_dynamic` marks true.** This "dQ/dt = −residual" identity only holds
    /// for a row some device's charge channel actually touches — a genuine state variable. A
    /// row no device ever stamps charge at (an ordinary KCL node with no capacitor, or a
    /// branch-current constraint row) is *algebraic*, not a state: it must equal `residual(x)
    /// = 0` at every solved time regardless of history, the same way backward Euler already
    /// gets right for free (`q_prev[i] = 0` there makes its own offset `0` automatically). Were
    /// this history term applied uniformly to every row, an initial condition that doesn't
    /// happen to satisfy an algebraic row's constraint exactly (a very easy mistake — e.g. a
    /// caller-supplied `x0` with a source's branch current left at a placeholder value instead
    /// of the value consistent with that source's voltage) would inject a spurious, permanent
    /// history term into that row and corrupt every step after it. Found by exactly that
    /// mistake in this module's own first test.
    fn trapezoidal(q_prev: &[f64], r_prev: &[f64], h: f64, is_dynamic: &[bool]) -> Self {
        let coeff = 2.0 / h;
        let offset = q_prev
            .iter()
            .zip(r_prev)
            .zip(is_dynamic)
            .map(|((q, r), &dynamic)| if dynamic { r - coeff * q } else { 0.0 })
            .collect();
        Companion { coeff, offset }
    }

    /// Build the companion term for `method` at step size `h` from history (`q_prev` always;
    /// `r_prev`/`is_dynamic` only matter to [`Self::trapezoidal`]).
    fn for_method(
        method: Method,
        q_prev: &[f64],
        r_prev: &[f64],
        h: f64,
        is_dynamic: &[bool],
    ) -> Self {
        match method {
            Method::BackwardEuler => Self::backward_euler(q_prev, h),
            Method::Trapezoidal => Self::trapezoidal(q_prev, r_prev, h, is_dynamic),
            Method::Gear => unreachable!("rejected in run() before any step is attempted"),
        }
    }
}

/// A row is "dynamic" (a genuine state variable, integrated via history) if some device's
/// charge channel touches it — nonzero `charge` or any nonzero `dcharge` entry in that row —
/// at the initial assembly. Computed once from `x0` and held fixed for the whole run: exact
/// for every model in this crate's reach today (a linear capacitor's `dcharge` is the constant
/// `C`, never zero; every other reference model never stamps charge at all), but not a fully
/// general answer — a hypothetical nonlinear charge model whose `dQ/dV` happens to be exactly
/// zero at `x0`'s specific bias point, but not elsewhere, would be misclassified as algebraic.
fn classify_dynamic_rows(dcharge: &[f64], charge: &[f64], dim: usize) -> Vec<bool> {
    (0..dim)
        .map(|row| charge[row] != 0.0 || (0..dim).any(|col| dcharge[row * dim + col] != 0.0))
        .collect()
}

/// The other implemented method, used purely as this step's LTE reference solution (never
/// reported as the accepted point) — see this module's doc comment on the embedded-pair
/// estimate. Only ever called with an implemented method (`run` rejects `Gear` up front).
fn reference_method(method: Method) -> Method {
    match method {
        Method::BackwardEuler => Method::Trapezoidal,
        Method::Trapezoidal => Method::BackwardEuler,
        Method::Gear => unreachable!("rejected in run() before any step is attempted"),
    }
}

/// How far outside tolerance the embedded pair's disagreement is, as a multiple of the allowed
/// budget: `max_i |x_a[i] − x_b[i]| / (lte_reltol·|x_a[i]| + lte_abstol)`. `<= 1.0` means accept.
fn lte_error_ratio(x_primary: &[f64], x_reference: &[f64], reltol: f64, abstol: f64) -> f64 {
    x_primary
        .iter()
        .zip(x_reference)
        .fold(0.0_f64, |worst, (a, b)| {
            let scale = reltol * a.abs() + abstol;
            worst.max((a - b).abs() / scale)
        })
}

/// Shrink the step when a candidate is rejected, floor at `min_h`; grow it after a
/// comfortably-within-tolerance accept, capped at `max_h`. Not a rigorous order-based
/// power-law controller (see this module's doc comment) — fixed multiplicative factors, tuned
/// to be conservative on shrink (halve) and modest on growth (50% per accepted step).
const GROWTH_FACTOR: f64 = 1.5;
const GROWTH_ERR_THRESHOLD: f64 = 0.5;
const SHRINK_FACTOR: f64 = 0.5;

/// Assemble every instance's stamps at `x` into a fresh dense sink.
fn assemble(instances: &[&dyn ModelInstance], x: &[f64], dim: usize) -> DenseStamp {
    let mut sink = DenseStamp::new(dim);
    for inst in instances {
        inst.load(x, &mut sink);
    }
    sink
}

/// Solve one implicit step's nodal equation `residual(x) + coeff·charge(x) + offset = 0` for
/// `x`, warm-started from `x_prev` (the previous step's solution — Newton's initial guess).
///
/// Structurally identical to `va-core`'s DC Newton loop (same convergence criteria, same
/// [`convergence::limit_junction`] clamp), except the assembled system is the companion-model
/// combination of the resistive and charge channels rather than the resistive channel alone.
fn newton_step(
    instances: &[&dyn ModelInstance],
    dim: usize,
    x_prev: &[f64],
    companion: &Companion,
) -> Result<Vec<f64>, TransientError> {
    const MAX_ITERS: usize = 100;
    const ABSTOL: f64 = 1e-12;
    const RELTOL: f64 = 1e-9;

    let vt = convergence::VT_300K;
    let vcrit = convergence::default_vcrit(vt);

    let mut x = x_prev.to_vec();
    let mut last_residual = f64::INFINITY;
    for _ in 0..MAX_ITERS {
        let sink = assemble(instances, &x, dim);
        let mut f = sink.residual.clone();
        let mut j = sink.jacobian.clone();
        for i in 0..dim {
            f[i] += companion.coeff * sink.charge[i] + companion.offset[i];
            for k in 0..dim {
                j[i * dim + k] += companion.coeff * sink.dcharge[i * dim + k];
            }
        }
        let residual_norm = f.iter().fold(0.0_f64, |m, v| m.max(v.abs()));

        let neg_f: Vec<f64> = f.iter().map(|v| -v).collect();
        let dx = linsolve::solve_dense(&j, &neg_f, dim)?;

        let mut update_small = true;
        for i in 0..dim {
            let vold = x[i];
            let vnew = convergence::limit_junction(vold + dx[i], vold, vt, vcrit);
            x[i] = vnew;
            let applied = vnew - vold;
            if applied.abs() > RELTOL * vnew.abs() + ABSTOL {
                update_small = false;
            }
        }

        if residual_norm <= ABSTOL || update_small {
            return Ok(x);
        }
        last_residual = residual_norm;
    }

    Err(va_core::CoreError::NoConvergence {
        iters: MAX_ITERS,
        residual: last_residual,
    }
    .into())
}

/// Integrate `instances` over `[cfg.tstart, cfg.tstop]` from initial condition `x0`, returning
/// the sampled [`Waveform`] (including the `(tstart, x0)` point itself).
///
/// Equivalent to [`run_with_events`] with an empty [`EventQueue`] (no forced breakpoints, no
/// crossing watches) — see it for the full behavior and error conditions.
pub fn run(
    instances: &[&dyn ModelInstance],
    dim: usize,
    x0: Vec<f64>,
    cfg: TranConfig,
) -> Result<Waveform, TransientError> {
    run_with_events(instances, dim, x0, cfg, &EventQueue::new())
}

/// Integrate `instances` over `[cfg.tstart, cfg.tstop]` from initial condition `x0`, forcing an
/// exact landing on every breakpoint in `events` and recording every crossing `events` watches
/// for, in addition to everything [`run`] does.
///
/// `x0` is the caller's responsibility — typically a `va_core::dc::operating_point` result, or
/// a deliberately different initial condition (e.g. a capacitor starting at 0 V to observe a
/// charging transient). Step size adapts within `[cfg.tstep_min, cfg.tstep]` to keep the
/// embedded-pair LTE estimate (this module's doc comment) within `cfg.lte_reltol`/
/// `cfg.lte_abstol`, further clamped so it never steps past the next unconsumed breakpoint.
/// Crossings are detected between consecutive *accepted* points only (see
/// [`crate::events::CrossingWatch`]'s doc comment on why interpolation, not a genuine re-solve
/// at the crossing time, is enough here).
///
/// # Errors
///
/// [`TransientError::UnsupportedMethod`] for [`Method::Gear`] (not implemented);
/// [`TransientError::TimestepUnderflow`] if the LTE controller must shrink below
/// `cfg.tstep_min` without meeting tolerance; or a propagated [`TransientError::Core`] from a
/// per-step Newton solve that fails to converge.
pub fn run_with_events(
    instances: &[&dyn ModelInstance],
    dim: usize,
    x0: Vec<f64>,
    cfg: TranConfig,
    events: &EventQueue,
) -> Result<Waveform, TransientError> {
    if cfg.method == Method::Gear {
        return Err(TransientError::UnsupportedMethod { method: cfg.method });
    }

    let mut waveform = Waveform {
        t: vec![cfg.tstart],
        x: vec![x0.clone()],
        crossings: Vec::new(),
    };
    if dim == 0 {
        return Ok(waveform);
    }

    let mut x = x0;
    let initial = assemble(instances, &x, dim);
    let is_dynamic = classify_dynamic_rows(&initial.dcharge, &initial.charge, dim);
    let mut q_prev = initial.charge;
    let mut r_prev = initial.residual;

    let mut t = cfg.tstart;
    let mut h = cfg.tstep;
    let reference = reference_method(cfg.method);

    while t < cfg.tstop {
        loop {
            let mut t_next = (t + h).min(cfg.tstop);
            if let Some(bp) = events.next_after(t) {
                t_next = t_next.min(bp);
            }
            let step_h = t_next - t;

            let primary = Companion::for_method(cfg.method, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_primary = newton_step(instances, dim, &x, &primary)?;

            let reference_companion =
                Companion::for_method(reference, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_reference = newton_step(instances, dim, &x, &reference_companion)?;

            let err_ratio =
                lte_error_ratio(&x_primary, &x_reference, cfg.lte_reltol, cfg.lte_abstol);

            if err_ratio <= 1.0 {
                let x_before = std::mem::replace(&mut x, x_primary);
                let t_before = t;

                let sink = assemble(instances, &x, dim);
                q_prev = sink.charge;
                r_prev = sink.residual;

                t = t_next;
                waveform.t.push(t);
                waveform.x.push(x.clone());

                for (watch_idx, watch) in events.watches().iter().enumerate() {
                    let before = x_before[watch.unknown] - watch.threshold;
                    let after = x[watch.unknown] - watch.threshold;
                    if before != 0.0 && (before > 0.0) != (after > 0.0) {
                        let frac = before / (before - after);
                        waveform
                            .crossings
                            .push((watch_idx, t_before + frac * (t - t_before)));
                    }
                }

                if err_ratio < GROWTH_ERR_THRESHOLD {
                    h = (h * GROWTH_FACTOR).min(cfg.tstep);
                }
                break;
            }

            let shrunk = h * SHRINK_FACTOR;
            if shrunk < cfg.tstep_min {
                return Err(TransientError::TimestepUnderflow { t });
            }
            h = shrunk;
        }
    }

    Ok(waveform)
}

/// Integrate with most devices fixed but one or more rebuilt fresh at every step *attempt*
/// (including LTE-rejection retries) — for a circuit containing a time-varying independent
/// source. `time_varying` is called with the candidate landing time `t_next` and must return
/// the device(s) valid at that instant (e.g. a `VSource` reconstructed with a freshly computed
/// value); `fixed` is everything else, unchanged for the whole run.
///
/// This exists because [`va_abi::ModelInstance::load`] deliberately has no time parameter
/// (Interface β's "no time, no frequency on the bridge" invariant —
/// `docs/bridges/interface-beta-abi.md` §7): a time-varying source's only legitimate entry
/// point is a fresh, differently-parameterized instance per step, not a stateful `load()`,
/// which would violate `ModelInstance`'s purity invariant (the same `x` must always produce
/// the same stamps). Rebuilding a plain, assertion-free constructor like `VSource::new` can't
/// fail, so `time_varying` is infallible by construction, not because errors are swallowed —
/// if a future time-varying device *can* fail to construct, this signature would need to
/// change (a `va-transient`-internal decision, not an Interface β one).
///
/// Otherwise identical to [`run_with_events`] — same LTE control, same breakpoint/crossing
/// handling, same errors. `q_prev`/`r_prev`/`is_dynamic` are computed once from the first
/// build (`time_varying(cfg.tstart)` combined with `fixed`), on the assumption that which
/// unknowns are dynamic vs. algebraic doesn't change as a time-varying source's value changes
/// — true for every device this project can build today; only a device's structure, never its
/// parameter value, determines that.
pub fn run_dynamic(
    dim: usize,
    x0: Vec<f64>,
    cfg: TranConfig,
    events: &EventQueue,
    fixed: &[&dyn ModelInstance],
    mut time_varying: impl FnMut(f64) -> Vec<Box<dyn ModelInstance>>,
) -> Result<Waveform, TransientError> {
    if cfg.method == Method::Gear {
        return Err(TransientError::UnsupportedMethod { method: cfg.method });
    }

    let mut waveform = Waveform {
        t: vec![cfg.tstart],
        x: vec![x0.clone()],
        crossings: Vec::new(),
    };
    if dim == 0 {
        return Ok(waveform);
    }

    let mut x = x0;
    let tv0 = time_varying(cfg.tstart);
    let mut refs0: Vec<&dyn ModelInstance> = fixed.to_vec();
    refs0.extend(tv0.iter().map(|b| b.as_ref()));
    let initial = assemble(&refs0, &x, dim);
    let is_dynamic = classify_dynamic_rows(&initial.dcharge, &initial.charge, dim);
    let mut q_prev = initial.charge;
    let mut r_prev = initial.residual;
    drop(refs0);
    drop(tv0);

    let mut t = cfg.tstart;
    let mut h = cfg.tstep;
    let reference = reference_method(cfg.method);

    while t < cfg.tstop {
        loop {
            let mut t_next = (t + h).min(cfg.tstop);
            if let Some(bp) = events.next_after(t) {
                t_next = t_next.min(bp);
            }
            let step_h = t_next - t;

            let tv = time_varying(t_next);
            let mut refs: Vec<&dyn ModelInstance> = fixed.to_vec();
            refs.extend(tv.iter().map(|b| b.as_ref()));

            let primary = Companion::for_method(cfg.method, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_primary = newton_step(&refs, dim, &x, &primary)?;

            let reference_companion =
                Companion::for_method(reference, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_reference = newton_step(&refs, dim, &x, &reference_companion)?;

            let err_ratio =
                lte_error_ratio(&x_primary, &x_reference, cfg.lte_reltol, cfg.lte_abstol);

            if err_ratio <= 1.0 {
                let x_before = std::mem::replace(&mut x, x_primary);
                let t_before = t;

                let sink = assemble(&refs, &x, dim);
                q_prev = sink.charge;
                r_prev = sink.residual;

                t = t_next;
                waveform.t.push(t);
                waveform.x.push(x.clone());

                for (watch_idx, watch) in events.watches().iter().enumerate() {
                    let before = x_before[watch.unknown] - watch.threshold;
                    let after = x[watch.unknown] - watch.threshold;
                    if before != 0.0 && (before > 0.0) != (after > 0.0) {
                        let frac = before / (before - after);
                        waveform
                            .crossings
                            .push((watch_idx, t_before + frac * (t - t_before)));
                    }
                }

                if err_ratio < GROWTH_ERR_THRESHOLD {
                    h = (h * GROWTH_FACTOR).min(cfg.tstep);
                }
                break;
            }

            let shrunk = h * SHRINK_FACTOR;
            if shrunk < cfg.tstep_min {
                return Err(TransientError::TimestepUnderflow { t });
            }
            h = shrunk;
        }
    }

    Ok(waveform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_core::CoreError;

    /// R (1 kΩ) from an ideal `vs_val` V source (node 0) to node 1; C (1 µF) from node 1 to
    /// ground. `RC = 1 ms`.
    fn rc_circuit(
        vs_val: f64,
    ) -> (
        va_abi::reference::VSource,
        va_abi::reference::Resistor,
        va_abi::reference::Capacitor,
    ) {
        (
            va_abi::reference::VSource::new(0, va_abi::reference::GROUND, 2, vs_val),
            va_abi::reference::Resistor::new(0, 1, 1000.0),
            va_abi::reference::Capacitor::new(1, va_abi::reference::GROUND, 1e-6),
        )
    }

    fn default_cfg(tstop: f64, tstep: f64, method: Method) -> TranConfig {
        TranConfig {
            tstart: 0.0,
            tstop,
            tstep,
            tstep_min: tstep * 1e-6,
            method,
            lte_reltol: 1e-3,
            lte_abstol: 1e-6,
        }
    }

    /// Linear interpolation of `wf.x[.][component]` at `t_query`, for comparing an adaptively
    /// sampled waveform (which won't land exactly on an arbitrary query time) against an
    /// analytic reference.
    fn interpolate(wf: &Waveform, t_query: f64, component: usize) -> f64 {
        let i =
            wf.t.windows(2)
                .position(|w| w[0] <= t_query && t_query <= w[1])
                .expect("t_query within the waveform's range");
        let (t0, t1) = (wf.t[i], wf.t[i + 1]);
        let (x0, x1) = (wf.x[i][component], wf.x[i + 1][component]);
        let frac = (t_query - t0) / (t1 - t0);
        x0 + frac * (x1 - x0)
    }

    #[test]
    fn rc_transient_matches_analytic() {
        // Charging curve V(t) = Vs·(1 − e^(−t/RC)), starting the capacitor at 0 V (not the DC
        // operating point) rather than already at steady state.
        let rc = 1e-3;
        let vs_val = 5.0;
        let (vs, r, c) = rc_circuit(vs_val);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];

        let cfg = default_cfg(5.0 * rc, rc / 10.0, Method::BackwardEuler);
        let wf = run(&insts, 3, vec![vs_val, 0.0, 0.0], cfg).expect("integrates");

        let analytic_at_rc = vs_val * (1.0 - (-1.0f64).exp());
        let v1_at_rc = interpolate(&wf, rc, 1);
        let rel_err = (v1_at_rc - analytic_at_rc).abs() / analytic_at_rc;
        assert!(
            rel_err < 1e-2,
            "adaptive backward Euler vs analytic RC charge: {v1_at_rc} vs {analytic_at_rc} \
             (rel err {rel_err})"
        );

        let v1_final = *wf.x.last().unwrap().get(1).unwrap();
        assert!(
            (v1_final - vs_val).abs() / vs_val < 1e-2,
            "should have settled near Vs: {v1_final}"
        );
    }

    #[test]
    fn adaptive_stepping_grows_the_step_as_the_transient_flattens() {
        // Early in an RC charge, V(t) is changing fastest; by several time constants in, it's
        // nearly flat. A working LTE controller should end up taking bigger accepted steps
        // late in the run than at the very start.
        let rc = 1e-3;
        let (vs, r, c) = rc_circuit(5.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];

        let cfg = default_cfg(8.0 * rc, rc / 20.0, Method::Trapezoidal);
        let wf = run(&insts, 3, vec![5.0, 0.0, 0.0], cfg).expect("integrates");

        let gaps: Vec<f64> = wf.t.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.len() >= 4, "need enough accepted steps to compare");
        let first_gap = gaps[0];
        let last_gap = *gaps.last().unwrap();
        assert!(
            last_gap > first_gap,
            "expected step growth late in the run: first={first_gap:e} last={last_gap:e}"
        );
    }

    #[test]
    fn tighter_lte_tolerance_takes_more_steps() {
        // The one thing T4.2 is actually for: the tolerance genuinely drives step count, not
        // just a config field nobody reads.
        let rc = 1e-3;
        let run_with_reltol = |reltol: f64| {
            let (vs, r, c) = rc_circuit(5.0);
            let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
            let mut cfg = default_cfg(2.0 * rc, rc / 10.0, Method::BackwardEuler);
            cfg.lte_reltol = reltol;
            cfg.lte_abstol = reltol * 1e-3;
            let wf = run(&insts, 3, vec![5.0, 0.0, 0.0], cfg).expect("integrates");
            wf.t.len()
        };

        let loose_steps = run_with_reltol(1e-1);
        let tight_steps = run_with_reltol(1e-6);
        assert!(
            tight_steps > loose_steps,
            "tighter tolerance should need more steps: tight={tight_steps} loose={loose_steps}"
        );
    }

    #[test]
    fn trapezoidal_is_more_accurate_than_backward_euler_at_the_same_schedule() {
        // NOT a step-count comparison: because both directions' accept/reject decisions come
        // from the *same* symmetric embedded-pair estimate |x_BE − x_Trap| (this module's doc
        // comment), the step schedule ends up nearly identical regardless of which method is
        // "primary" — picking one only changes which of the two solutions gets reported, not
        // how many steps it takes to get there. What genuinely differs is accuracy at that
        // shared schedule: trapezoidal's second-order reported answer should still beat
        // backward Euler's first-order one at the same point in time.
        let rc = 1e-3;
        let analytic_at_rc = 5.0 * (1.0 - (-1.0f64).exp());
        let run_with_method = |method: Method| {
            let (vs, r, c) = rc_circuit(5.0);
            let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
            let cfg = default_cfg(3.0 * rc, rc / 10.0, method);
            let wf = run(&insts, 3, vec![5.0, 0.0, 0.0], cfg).expect("integrates");
            interpolate(&wf, rc, 1)
        };

        let be_err = (run_with_method(Method::BackwardEuler) - analytic_at_rc).abs();
        let trap_err = (run_with_method(Method::Trapezoidal) - analytic_at_rc).abs();
        assert!(
            trap_err < be_err,
            "trapezoidal ({trap_err}) should beat backward Euler ({be_err}) at the same LTE \
             tolerance"
        );
    }

    #[test]
    fn timestep_underflow_is_reported() {
        // An impossibly tight tolerance forces the controller to keep halving until it hits
        // the floor without ever satisfying it — must error out, not silently accept anyway.
        let rc = 1e-3;
        let (vs, r, c) = rc_circuit(5.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let mut cfg = default_cfg(5.0 * rc, rc / 10.0, Method::BackwardEuler);
        cfg.lte_reltol = 1e-18;
        cfg.lte_abstol = 1e-18;
        cfg.tstep_min = rc / 100.0;

        assert!(matches!(
            run(&insts, 3, vec![5.0, 0.0, 0.0], cfg),
            Err(TransientError::TimestepUnderflow { .. })
        ));
    }

    #[test]
    fn gear_is_not_yet_implemented() {
        let (vs, r, c) = rc_circuit(1.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let cfg = default_cfg(1e-3, 1e-5, Method::Gear);
        assert!(matches!(
            run(&insts, 3, vec![1.0, 0.0, 0.0], cfg),
            Err(TransientError::UnsupportedMethod {
                method: Method::Gear
            })
        ));
    }

    #[test]
    fn empty_circuit_returns_only_the_initial_point() {
        let insts: [&dyn ModelInstance; 0] = [];
        let cfg = default_cfg(1.0, 0.1, Method::BackwardEuler);
        let wf = run(&insts, 0, Vec::new(), cfg).expect("trivially ok");
        assert_eq!(wf.t, vec![0.0]);
        assert_eq!(wf.x, vec![Vec::<f64>::new()]);
    }

    #[test]
    fn breakpoint_forces_an_exact_landing_time() {
        // An "awkward" time that no natural adaptive step would land on by itself.
        let rc = 1e-3;
        let (vs, r, c) = rc_circuit(5.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let cfg = default_cfg(2.0 * rc, rc / 10.0, Method::BackwardEuler);
        let awkward_t = 0.37 * rc;

        let mut events = crate::events::EventQueue::new();
        events.push_breakpoint(awkward_t);
        let wf = run_with_events(&insts, 3, vec![5.0, 0.0, 0.0], cfg, &events).expect("integrates");

        assert!(
            wf.t.iter().any(|&t| (t - awkward_t).abs() < 1e-15),
            "should land exactly on the breakpoint: {:?}",
            wf.t
        );
    }

    #[test]
    fn breakpoint_beyond_tstop_is_never_reached() {
        // A breakpoint past tstop shouldn't extend the run or otherwise change anything.
        let rc = 1e-3;
        let (vs, r, c) = rc_circuit(5.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let cfg = default_cfg(2.0 * rc, rc / 10.0, Method::BackwardEuler);

        let mut events = crate::events::EventQueue::new();
        events.push_breakpoint(100.0 * rc);
        let wf = run_with_events(&insts, 3, vec![5.0, 0.0, 0.0], cfg, &events).expect("integrates");

        assert!((*wf.t.last().unwrap() - 2.0 * rc).abs() < 1e-15);
    }

    #[test]
    fn crossing_detection_matches_analytic() {
        // V(t) = Vs·(1 − e^(−t/RC)) crosses Vs/2 at t = RC·ln(2).
        let rc = 1e-3;
        let vs_val = 5.0;
        let (vs, r, c) = rc_circuit(vs_val);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let mut cfg = default_cfg(3.0 * rc, rc / 10.0, Method::BackwardEuler);
        // Tighter than the default LTE tolerance: interpolation between two accepted points
        // can't be more accurate than the points themselves are.
        cfg.lte_reltol = 1e-5;
        cfg.lte_abstol = 1e-8;

        let mut events = crate::events::EventQueue::new();
        events.push_watch(1, vs_val / 2.0);
        let wf =
            run_with_events(&insts, 3, vec![vs_val, 0.0, 0.0], cfg, &events).expect("integrates");

        assert_eq!(wf.crossings.len(), 1, "crossings: {:?}", wf.crossings);
        let (watch_idx, t_cross) = wf.crossings[0];
        assert_eq!(watch_idx, 0);

        let analytic_t = rc * 2.0f64.ln();
        let rel_err = (t_cross - analytic_t).abs() / analytic_t;
        assert!(
            rel_err < 1e-2,
            "crossing time {t_cross} vs analytic {analytic_t} (rel err {rel_err})"
        );
    }

    #[test]
    fn no_crossing_when_threshold_never_reached() {
        let rc = 1e-3;
        let vs_val = 5.0;
        let (vs, r, c) = rc_circuit(vs_val);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let cfg = default_cfg(0.1 * rc, rc / 10.0, Method::BackwardEuler);

        let mut events = crate::events::EventQueue::new();
        events.push_watch(1, vs_val); // never reaches Vs within this short a run
        let wf =
            run_with_events(&insts, 3, vec![vs_val, 0.0, 0.0], cfg, &events).expect("integrates");

        assert!(wf.crossings.is_empty());
    }

    #[test]
    fn run_matches_run_with_events_given_an_empty_queue() {
        let rc = 1e-3;
        let (vs, r, c) = rc_circuit(5.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];

        let wf_a = run(
            &insts,
            3,
            vec![5.0, 0.0, 0.0],
            default_cfg(rc, rc / 10.0, Method::BackwardEuler),
        )
        .expect("integrates");
        let wf_b = run_with_events(
            &insts,
            3,
            vec![5.0, 0.0, 0.0],
            default_cfg(rc, rc / 10.0, Method::BackwardEuler),
            &crate::events::EventQueue::new(),
        )
        .expect("integrates");

        assert_eq!(wf_a.t, wf_b.t);
        assert_eq!(wf_a.x, wf_b.x);
    }

    #[test]
    fn run_dynamic_tracks_a_sinusoidal_source_through_a_resistive_divider() {
        // No capacitor anywhere: every row is algebraic, so V(mid) must exactly track
        // v_source(t)/2 at every accepted point regardless of method or step history --
        // isolating the time-varying-rebuild mechanism from LTE/dynamics entirely.
        let amplitude = 10.0;
        let freq = 1000.0; // 1 kHz, period = 1 ms
        let period = 1.0 / freq;
        let source_at = |t: f64| amplitude * (2.0 * std::f64::consts::PI * freq * t).sin();

        let r1 = va_abi::reference::Resistor::new(0, 1, 1000.0);
        let r2 = va_abi::reference::Resistor::new(1, va_abi::reference::GROUND, 1000.0);
        let fixed: [&dyn ModelInstance; 2] = [&r1, &r2];

        let cfg = default_cfg(2.0 * period, period / 20.0, Method::BackwardEuler);
        let wf = run_dynamic(
            3,
            vec![0.0, 0.0, 0.0],
            cfg,
            &crate::events::EventQueue::new(),
            &fixed,
            |t| {
                vec![Box::new(va_abi::reference::VSource::new(
                    0,
                    va_abi::reference::GROUND,
                    2,
                    source_at(t),
                )) as Box<dyn ModelInstance>]
            },
        )
        .expect("integrates");

        assert!(
            wf.t.len() > 10,
            "expected many accepted steps: {}",
            wf.t.len()
        );
        for (&t, x) in wf.t.iter().zip(&wf.x) {
            let expected = source_at(t);
            assert!(
                (x[0] - expected).abs() < 1e-9,
                "node0 at t={t}: {} vs source {expected}",
                x[0]
            );
            assert!(
                (x[1] - expected / 2.0).abs() < 1e-6,
                "mid at t={t}: {} vs {}",
                x[1],
                expected / 2.0
            );
        }
    }

    #[test]
    fn nonconvergence_propagates_as_a_core_error() {
        // Sanity: the `?`-propagation path from `linsolve`/Newton failures into
        // `TransientError::Core` actually compiles and matches, using the same non-convergence
        // shape `va-core`'s own tests check for.
        let err = TransientError::from(CoreError::NoConvergence {
            iters: 1,
            residual: 1.0,
        });
        assert!(matches!(
            err,
            TransientError::Core(CoreError::NoConvergence { .. })
        ));
    }

    // --- ladder rung 6: ring oscillator ---------------------------------------------------

    /// A 3-stage RC-coupled common-emitter BJT ring oscillator (LRM/roadmap "ladder rung 6" —
    /// see this module's doc comment). Global unknowns: 0 = Vcc node, 1 = its `VSource` branch
    /// current, then (base, collector) for stages 1/2/3 as (2,3), (4,5), (6,7). Each stage:
    /// emitter grounded, a single resistor `Rb` (~3.3-3.5 MΩ) biases the base from `Vcc`, a
    /// single resistor `Rc` (~19-21 kΩ) loads the collector from `Vcc`, and a coupling
    /// capacitor `Cc` (10 nF) AC-couples this stage's collector to the *next* stage's base —
    /// stage 3's collector couples back to stage 1's base, closing the ring. `Rb`/`Rc` are
    /// high-impedance by design (found empirically, not guessed): a lower-impedance,
    /// "linear-gain" bias point (kΩ-range `Rb`) converges to a real DC operating point but
    /// turned out to be small-signal *stable* around it (no oscillation on any reasonable
    /// timescale) once the coupling network's own loading is properly accounted for; a
    /// too-aggressive, deep-saturation bias point (tens-of-kΩ `Rb`) instead makes the DC solve
    /// itself numerically singular (this simplified Ebers-Moll model's exponential terms blow
    /// up when both junctions are strongly forward-biased at once). This MΩ-range point sits
    /// in between: comfortably forward-active at DC, but with enough loop gain margin to be a
    /// genuinely unstable equilibrium. Component values are deliberately mismatched
    /// stage-to-stage (not just for realism — see this test's own comment on why).
    fn ring_oscillator() -> Vec<Box<dyn ModelInstance>> {
        let vt = va_abi::reference::diode::VT_300K;
        let bjt = |b: usize, c: usize| -> va_abi::reference::Bjt {
            va_abi::reference::Bjt::new(b, c, va_abi::reference::GROUND, 1e-15, 100.0, 1.0, vt)
        };
        let vcc = 0;
        vec![
            Box::new(va_abi::reference::VSource::new(
                vcc,
                va_abi::reference::GROUND,
                1,
                5.0,
            )),
            // Stage 1: base=2, collector=3.
            Box::new(va_abi::reference::Resistor::new(vcc, 2, 3_300_000.0)),
            Box::new(va_abi::reference::Resistor::new(vcc, 3, 20_000.0)),
            Box::new(bjt(2, 3)),
            Box::new(va_abi::reference::Capacitor::new(3, 4, 10e-9)),
            // Stage 2: base=4, collector=5.
            Box::new(va_abi::reference::Resistor::new(vcc, 4, 3_400_000.0)),
            Box::new(va_abi::reference::Resistor::new(vcc, 5, 19_000.0)),
            Box::new(bjt(4, 5)),
            Box::new(va_abi::reference::Capacitor::new(5, 6, 10e-9)),
            // Stage 3: base=6, collector=7 — couples back to stage 1's base (2), closing the
            // ring.
            Box::new(va_abi::reference::Resistor::new(vcc, 6, 3_500_000.0)),
            Box::new(va_abi::reference::Resistor::new(vcc, 7, 21_000.0)),
            Box::new(bjt(6, 7)),
            Box::new(va_abi::reference::Capacitor::new(7, 2, 10e-9)),
        ]
    }

    #[test]
    fn ring_oscillator_sustains_oscillation() {
        // Closes "ladder rung 6" (this module's doc comment, `docs/roadmap.md`): the first
        // circuit in this codebase with a gain-capable device, run through the exact same DC
        // (Newton, gmin-stepping) and transient (adaptive trapezoidal) machinery every other
        // circuit here uses — no netlist file (§ scope decision 2 in the implementation plan;
        // `va-netlist` has no 3-terminal-device grammar yet), instances built directly.
        //
        // **Why mismatched component values, not identical ones**: three *identical* ring
        // stages have an exactly symmetric DC equilibrium — a real circuit's component
        // tolerances always break that symmetry (which is what actually lets thermal noise
        // kick real oscillators into motion), but a deterministic solver started from a
        // matching guess has no noise to do that job for it. Mismatching `Rb`/`Rc` stage to
        // stage means `x = 0` (and the symmetric bias point) is never an exact equilibrium of
        // *this* circuit to begin with — a genuinely tricky-for-convergence property worth
        // demonstrating, not just a simulation trick.
        //
        // **Why the assertions check growth, not many clean periods**: this is a genuinely
        // *unstable* DC operating point (found by Newton exactly like any other — Newton
        // doesn't know or care that a fixed point is unstable, only that its residual is zero),
        // so the transient's whole point is to diverge away from it. It does: stage 1's
        // collector swings from a ~15 mV trough-to-trough excursion early on to well over 400
        // mV by the second cycle (confirmed empirically while tuning this test's component
        // values). That growth is real, correct oscillator-startup behavior — but it also
        // means the swing eventually reaches this simplified Ebers-Moll model's own edge (no
        // saturation-charge smoothing — see `va_abi::reference::bjt`'s doc comment), where the
        // embedded-pair LTE estimator's two methods stop agreeing at any step size. `tstop`
        // here is chosen comfortably inside the well-behaved region (confirmed empirically),
        // not at the numerical edge — a real limitation to note, not hidden.
        let devices = ring_oscillator();
        let insts: Vec<&dyn ModelInstance> = devices.iter().map(|d| d.as_ref()).collect();
        let dim = 8;

        // DC bias first (gmin-stepping enabled — this is exactly the harder-than-a-diode-chain
        // convergence case the ring's positive-feedback loop was chosen to exercise).
        let dc_cfg = va_core::newton::NewtonConfig {
            gmin_steps: 12,
            ..va_core::newton::NewtonConfig::default()
        };
        let op = va_core::dc::operating_point(&insts, dim, dc_cfg).expect("DC bias converges");

        // Belt-and-suspenders symmetry break (this module's doc comment, scope decision 3b):
        // nudge stage 1's base a few mV off its DC bias before starting the transient.
        let mut x0 = op.x.clone();
        x0[2] += 0.005;

        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 0.2,
            tstep: 100e-6,
            tstep_min: 1e-9,
            method: Method::Trapezoidal,
            lte_reltol: 5e-2,
            lte_abstol: 2e-3,
        };
        let mut events = crate::events::EventQueue::new();
        events.push_watch(3, op.x[3]); // stage 1's collector, crossing its own DC bias voltage
        let wf = run_with_events(&insts, dim, x0, cfg, &events).expect("transient integrates");

        // At least two crossings (a genuine there-and-back alternation, not a one-off kick
        // that settles).
        assert!(
            wf.crossings.len() >= 2,
            "expected at least one full alternation (sustained oscillation), got {}: {:?}",
            wf.crossings.len(),
            wf.crossings
        );

        // Growing amplitude: the trough in the second half of the run must be deeper than the
        // trough in the first half — direct evidence this is a diverging (unstable-equilibrium)
        // oscillation, not a damped one settling back toward the DC bias.
        let mid = wf.t.len() / 2;
        let min_x3 = |range: std::ops::Range<usize>| {
            wf.x[range]
                .iter()
                .map(|x| x[3])
                .fold(f64::INFINITY, f64::min)
        };
        let first_half_min = min_x3(0..mid);
        let second_half_min = min_x3(mid..wf.t.len());
        assert!(
            second_half_min < first_half_min - 0.05,
            "expected a deeper trough later in the run (growing oscillation): first-half min \
             {first_half_min}, second-half min {second_half_min}"
        );
    }
}
