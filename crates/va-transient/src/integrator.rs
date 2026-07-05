//! Time integration with adaptive timestep and local-truncation-error (LTE) control.
//!
//! # Status
//!
//! Fixed-timestep [`Method::BackwardEuler`] and [`Method::Trapezoidal`] are implemented;
//! [`Method::Gear`] is not (returns [`TransientError::UnsupportedMethod`], never silently
//! falls back to another method). Adaptive step sizing / LTE control (T4.2) doesn't exist yet
//! — `cfg.tstep` is used as a constant step, not yet the "maximum" its doc comment anticipates.
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
    /// Initial / maximum timestep (s).
    pub tstep: f64,
    /// Integration method.
    pub method: Method,
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
    fn trapezoidal(q_prev: &[f64], r_prev: &[f64], h: f64) -> Self {
        let coeff = 2.0 / h;
        let offset = q_prev
            .iter()
            .zip(r_prev)
            .map(|(q, r)| r - coeff * q)
            .collect();
        Companion { coeff, offset }
    }
}

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
/// charging transient). Fixed timestep only (`cfg.tstep`); no adaptive LTE control yet (T4.2).
///
/// # Errors
///
/// [`TransientError::UnsupportedMethod`] for [`Method::Gear`] (not implemented), or a
/// propagated [`TransientError::Core`] from a per-step Newton solve that fails to converge.
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
    let mut q_prev = initial.charge;
    let mut r_prev = initial.residual;

    let mut t = cfg.tstart;
    while t < cfg.tstop {
        let t_next = (t + cfg.tstep).min(cfg.tstop);
        let h = t_next - t;

        let companion = match cfg.method {
            Method::BackwardEuler => Companion::backward_euler(&q_prev, h),
            Method::Trapezoidal => Companion::trapezoidal(&q_prev, &r_prev, h),
            Method::Gear => unreachable!("handled above"),
        };

        x = newton_step(instances, dim, &x, &companion)?;
        let sink = assemble(instances, &x, dim);
        q_prev = sink.charge;
        r_prev = sink.residual;

        t = t_next;
        waveform.t.push(t);
        waveform.x.push(x.clone());
    }

    Ok(waveform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_core::CoreError;

    #[test]
    fn rc_transient() {
        // R (1 kΩ) from an ideal 5 V source (node 0) to node 1; C (1 µF) from node 1 to
        // ground. RC = 1 ms. Starting the capacitor at 0 V (not the DC operating point) gives
        // the textbook charging curve V(t) = Vs·(1 − e^(−t/RC)); dt = RC/100 keeps backward
        // Euler's first-order error small.
        let rc = 1e-3;
        let vs_val = 5.0;
        let vs = va_abi::reference::VSource::new(0, va_abi::reference::GROUND, 2, vs_val);
        let r = va_abi::reference::Resistor::new(0, 1, 1000.0);
        let c = va_abi::reference::Capacitor::new(1, va_abi::reference::GROUND, 1e-6);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];

        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 5.0 * rc,
            tstep: rc / 100.0,
            method: Method::BackwardEuler,
        };
        let x0 = vec![vs_val, 0.0, 0.0];
        let wf = run(&insts, 3, x0, cfg).expect("integrates");

        // Sample near t = RC (one time constant): analytic V1 = Vs·(1 − e^-1).
        let analytic = vs_val * (1.0 - (-1.0f64).exp());
        let idx =
            wf.t.iter()
                .position(|&t| (t - rc).abs() < 1e-9)
                .expect("has a sample at t=RC");
        let v1 = wf.x[idx][1];
        let rel_err = (v1 - analytic).abs() / analytic;
        assert!(
            rel_err < 1e-2,
            "backward Euler vs analytic RC charge: {v1} vs {analytic} (rel err {rel_err})"
        );

        // By 5 time constants it should be within 1% of the final DC value.
        let v1_final = *wf.x.last().unwrap().get(1).unwrap();
        assert!(
            (v1_final - vs_val).abs() / vs_val < 1e-2,
            "should have settled near Vs: {v1_final}"
        );
    }

    #[test]
    fn trapezoidal_is_more_accurate_than_backward_euler() {
        // Same RC circuit, coarser timestep (RC/10) — backward Euler's first-order error is
        // pronounced here; trapezoidal's second-order error should be markedly smaller.
        let rc = 1e-3;
        let vs_val = 5.0;
        let analytic_at_rc = vs_val * (1.0 - (-1.0f64).exp());

        let run_with = |method: Method| {
            let vs = va_abi::reference::VSource::new(0, va_abi::reference::GROUND, 2, vs_val);
            let r = va_abi::reference::Resistor::new(0, 1, 1000.0);
            let c = va_abi::reference::Capacitor::new(1, va_abi::reference::GROUND, 1e-6);
            let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
            let cfg = TranConfig {
                tstart: 0.0,
                tstop: rc,
                tstep: rc / 10.0,
                method,
            };
            let wf = run(&insts, 3, vec![vs_val, 0.0, 0.0], cfg).expect("integrates");
            *wf.x.last().unwrap().get(1).unwrap()
        };

        let be_err = (run_with(Method::BackwardEuler) - analytic_at_rc).abs();
        let trap_err = (run_with(Method::Trapezoidal) - analytic_at_rc).abs();
        assert!(
            trap_err < be_err,
            "trapezoidal ({trap_err}) should beat backward Euler ({be_err}) at the same dt"
        );
    }

    #[test]
    fn gear_is_not_yet_implemented() {
        let vs = va_abi::reference::VSource::new(0, va_abi::reference::GROUND, 2, 1.0);
        let r = va_abi::reference::Resistor::new(0, 1, 1000.0);
        let c = va_abi::reference::Capacitor::new(1, va_abi::reference::GROUND, 1e-6);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &c];
        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 1e-3,
            tstep: 1e-5,
            method: Method::Gear,
        };
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
        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 1.0,
            tstep: 0.1,
            method: Method::BackwardEuler,
        };
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
