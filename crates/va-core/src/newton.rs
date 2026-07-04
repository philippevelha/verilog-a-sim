//! Newton–Raphson iteration driver.
//!
//! Each iteration assembles the MNA system at the current `x`, solves `J · dx = −f`, and
//! updates `x += dx` (optionally clamped by [`crate::convergence::limit_junction`] — see
//! `NewtonConfig::limit_junctions`). Convergence is declared when either the residual is below
//! `abstol` or every *applied* update component is within `reltol·|x| + abstol`. For a linear
//! circuit with limiting off this lands in two iterations; for smooth nonlinear devices Newton
//! converges quadratically near the solution.
//!
//! [`solve`] also drives an optional outer `gmin`-stepping homotopy (`NewtonConfig::gmin_steps`)
//! around the inner iteration: each stage re-solves with [`crate::mna::System::shunt_gmin`]
//! adding a decreasing conductance ([`crate::convergence::gmin_for_step`]) to every
//! [`va_abi::UnknownKind::Node`] row, warm-starting from the previous stage's solution, ending
//! on an unshunted (`gmin = 0`) solve of the real circuit.

use crate::{convergence, linsolve, mna, CoreError};
use va_abi::{ModelInstance, UnknownKind};

/// Tunable Newton iteration controls.
#[derive(Clone, Copy, Debug)]
pub struct NewtonConfig {
    /// Maximum iterations before declaring non-convergence.
    pub max_iters: usize,
    /// Absolute residual tolerance for convergence.
    pub abstol: f64,
    /// Relative update tolerance for convergence.
    pub reltol: f64,
    /// Clamp each iteration's proposed update with [`crate::convergence::limit_junction`],
    /// using [`crate::convergence::VT_300K`]/[`crate::convergence::default_vcrit`] as a
    /// blanket (not per-device) threshold. Keeps stiff exponential devices (diodes, BJTs) from
    /// overflowing on a cold start; the tradeoff is it can slow convergence on unknowns that
    /// were never exponential to begin with, since `va-core` has no way to tell those apart
    /// from real junction voltages (see `convergence`'s module doc comment). Default `true`.
    pub limit_junctions: bool,
    /// Number of geometric `gmin`-stepping homotopy stages to ramp through before the final,
    /// unshunted solve (see [`crate::convergence::gmin_for_step`]). `0` (the default) disables
    /// `gmin` stepping entirely — a single ordinary solve, identical to every prior release's
    /// behavior. Only ever shunts [`va_abi::UnknownKind::Node`] rows (never a branch-current
    /// constraint row — see [`crate::mna::System::shunt_gmin`]), so it's safe to enable on any
    /// circuit, including ones with ideal sources.
    pub gmin_steps: usize,
}

impl Default for NewtonConfig {
    fn default() -> Self {
        Self {
            max_iters: 100,
            abstol: 1e-12,
            reltol: 1e-9,
            limit_junctions: true,
            gmin_steps: 0,
        }
    }
}

/// Solve `f(x) = 0` for the given `instances` by Newton iteration, returning the solution
/// vector of length `dim`. The initial guess is the zero vector.
///
/// If `cfg.gmin_steps > 0`, wraps the inner iteration in a `gmin`-stepping homotopy (see this
/// module's doc comment): each stage warm-starts from the previous stage's solution, ending on
/// an unshunted solve of the real circuit. With `cfg.gmin_steps == 0` this is exactly one
/// inner solve from the zero vector — identical to every prior release's behavior.
///
/// # Errors
///
/// [`CoreError::NoConvergence`] if the iteration budget is exhausted at any stage, or
/// [`CoreError::Singular`] if a Jacobian factorization fails.
pub fn solve(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
) -> Result<Vec<f64>, CoreError> {
    if dim == 0 {
        return Ok(Vec::new());
    }

    // Only classify unknowns when gmin stepping is actually in play — `shunt_gmin` is a no-op
    // at `gmin == 0`, but building the classification is pointless work otherwise.
    let kinds = if cfg.gmin_steps > 0 {
        mna::classify_unknowns(instances, dim)
    } else {
        vec![UnknownKind::Node; dim]
    };

    let mut x = vec![0.0; dim];
    // `gmin_for_step(step, 0)` returns `0.0` at `step == 0`, so `gmin_steps == 0` collapses
    // this to exactly one iteration at `gmin = 0` — the original, un-homotopied solve.
    for step in 0..=cfg.gmin_steps {
        let gmin = convergence::gmin_for_step(step, cfg.gmin_steps);
        x = solve_from(x, instances, dim, cfg, gmin, &kinds)?;
    }
    Ok(x)
}

/// The inner Newton iteration, starting from `x0` and shunting `gmin` onto every `Node`-kind
/// row each iteration (see [`mna::System::shunt_gmin`]). [`solve`] is `gmin_steps + 1` calls to
/// this, chained by warm-starting each stage's `x0` from the previous stage's solution.
fn solve_from(
    mut x: Vec<f64>,
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    gmin: f64,
    kinds: &[UnknownKind],
) -> Result<Vec<f64>, CoreError> {
    let vt = convergence::VT_300K;
    let vcrit = convergence::default_vcrit(vt);

    let mut last_residual = f64::INFINITY;
    for _ in 0..cfg.max_iters {
        let mut sys = mna::assemble(instances, &x, dim);
        sys.shunt_gmin(&x, gmin, kinds);
        let residual_norm = inf_norm(&sys.residual);

        // Solve J · dx = −f.
        let neg_f: Vec<f64> = sys.residual.iter().map(|v| -v).collect();
        let dx = linsolve::solve_dense(&sys.jacobian, &neg_f, dim)?;

        let mut update_small = true;
        for i in 0..dim {
            let vold = x[i];
            let vnew_raw = vold + dx[i];
            let vnew = if cfg.limit_junctions {
                convergence::limit_junction(vnew_raw, vold, vt, vcrit)
            } else {
                vnew_raw
            };
            x[i] = vnew;

            let applied = vnew - vold;
            if applied.abs() > cfg.reltol * vnew.abs() + cfg.abstol {
                update_small = false;
            }
        }

        if residual_norm <= cfg.abstol || update_small {
            return Ok(x);
        }
        last_residual = residual_norm;
    }

    Err(CoreError::NoConvergence {
        iters: cfg.max_iters,
        residual: last_residual,
    })
}

/// Infinity norm (max absolute component) of a vector.
fn inf_norm(v: &[f64]) -> f64 {
    v.iter().fold(0.0_f64, |m, x| m.max(x.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::VSource;
    use va_abi::reference::diode::VT_300K;
    use va_abi::reference::{Diode, Resistor, GROUND};

    #[test]
    fn solves_resistor_divider() {
        // Vin = 2 V at node 0; R1 (node0→node1) and R2 (node1→gnd), both 1 kΩ.
        // node1 is the divider midpoint = Vin · R2/(R1+R2) = 1.0 V.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let x = solve(&insts, 3, NewtonConfig::default()).expect("converges");
        assert!((x[0] - 2.0).abs() < 1e-9, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-9, "midpoint = {}", x[1]);
        // Branch current through the source equals the divider current = 1 mA.
        assert!((x[2].abs() - 1e-3).abs() < 1e-12, "i = {}", x[2]);
    }

    #[test]
    fn solves_diode_resistor_clamp() {
        // Vin = 1 V → R = 1 kΩ → diode to ground. Nonlinear: exercises the exp Jacobian.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_300K);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let x = solve(&insts, 3, NewtonConfig::default()).expect("converges");

        // A forward-biased silicon diode sits around 0.4–0.75 V.
        let vd = x[1];
        assert!(
            (0.4..0.75).contains(&vd),
            "diode voltage out of range: {vd}"
        );
        // KCL at the diode node must balance: (Vin − Vd)/R == diode current.
        let i_r = (x[0] - vd) / 1000.0;
        let i_d = d.current(vd);
        assert!(
            (i_r - i_d).abs() < 1e-9,
            "KCL imbalance: {} vs {}",
            i_r,
            i_d
        );
    }

    #[test]
    fn gmin_stepping_does_not_corrupt_the_vsource_branch() {
        // The exact regression this is here to prevent: a naive "shunt every row" gmin
        // implementation would add a conductance to the VSource's branch-current row too,
        // corrupting its `V(p)-V(n)=value` constraint and giving a wrong answer. With
        // `classify_unknowns` tagging that row `Branch`, the divider must still solve to the
        // same exact midpoint gmin stepping on or off.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let cfg = NewtonConfig {
            gmin_steps: 8,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, 3, cfg).expect("converges");
        assert!((x[0] - 2.0).abs() < 1e-6, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-6, "midpoint = {}", x[1]);
        assert!((x[2].abs() - 1e-3).abs() < 1e-9, "i = {}", x[2]);
    }

    #[test]
    fn gmin_stepping_still_converges_the_diode_clamp() {
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_300K);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let cfg = NewtonConfig {
            gmin_steps: 8,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, 3, cfg).expect("converges");

        let vd = x[1];
        assert!(
            (0.4..0.75).contains(&vd),
            "diode voltage out of range: {vd}"
        );
        let i_r = (x[0] - vd) / 1000.0;
        let i_d = d.current(vd);
        assert!(
            (i_r - i_d).abs() < 1e-6,
            "KCL imbalance: {} vs {}",
            i_r,
            i_d
        );
    }

    #[test]
    fn reports_non_convergence() {
        // One iteration is not enough for a nonlinear solve from the zero guess.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_300K);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let cfg = NewtonConfig {
            max_iters: 1,
            ..NewtonConfig::default()
        };
        assert!(matches!(
            solve(&insts, 3, cfg),
            Err(CoreError::NoConvergence { .. })
        ));
    }
}
