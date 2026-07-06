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
//! Event handling (`crate::events`) isn't wired in either — no breakpoint forces a step yet.

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
/// `x0` is the caller's responsibility — typically a `va_core::dc::operating_point` result, or
/// a deliberately different initial condition (e.g. a capacitor starting at 0 V to observe a
/// charging transient). Step size adapts within `[cfg.tstep_min, cfg.tstep]` to keep the
/// embedded-pair LTE estimate (this module's doc comment) within `cfg.lte_reltol`/
/// `cfg.lte_abstol`.
///
/// # Errors
///
/// [`TransientError::UnsupportedMethod`] for [`Method::Gear`] (not implemented);
/// [`TransientError::TimestepUnderflow`] if the LTE controller must shrink below
/// `cfg.tstep_min` without meeting tolerance; or a propagated [`TransientError::Core`] from a
/// per-step Newton solve that fails to converge.
pub fn run(
    instances: &[&dyn ModelInstance],
    dim: usize,
    x0: Vec<f64>,
    cfg: TranConfig,
) -> Result<Waveform, TransientError> {
    if cfg.method == Method::Gear {
        return Err(TransientError::UnsupportedMethod { method: cfg.method });
    }

    let mut waveform = Waveform {
        t: vec![cfg.tstart],
        x: vec![x0.clone()],
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
            let t_next = (t + h).min(cfg.tstop);
            let step_h = t_next - t;

            let primary = Companion::for_method(cfg.method, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_primary = newton_step(instances, dim, &x, &primary)?;

            let reference_companion =
                Companion::for_method(reference, &q_prev, &r_prev, step_h, &is_dynamic);
            let x_reference = newton_step(instances, dim, &x, &reference_companion)?;

            let err_ratio =
                lte_error_ratio(&x_primary, &x_reference, cfg.lte_reltol, cfg.lte_abstol);

            if err_ratio <= 1.0 {
                x = x_primary;
                let sink = assemble(instances, &x, dim);
                q_prev = sink.charge;
                r_prev = sink.residual;

                t = t_next;
                waveform.t.push(t);
                waveform.x.push(x.clone());

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
}
