//! Newton–Raphson iteration driver.
//!
//! Each iteration assembles the MNA system at the current `x`, solves `J · dx = −f`, and
//! updates `x += dx` (optionally clamped by [`crate::convergence::limit_junction`] — see
//! `NewtonConfig::limit_junctions`). Convergence is declared when either the residual is below
//! `abstol` or every *applied* update component is within `reltol·|x| + abstol`. For a linear
//! circuit with limiting off this lands in two iterations; for smooth nonlinear devices Newton
//! converges quadratically near the solution.
//!
//! **The `abstol` in the per-unknown "applied update" check is per-unknown**, not always
//! `cfg.abstol`: [`solve`] builds it once via [`crate::mna::classify_abstol`], which lets any
//! [`va_abi::ModelInstance::unknown_abstol`] override its own unknown's tolerance (§
//! nature-metadata wiring — a `va-codegen`-generated model's discipline/nature metadata,
//! ultimately) — every unknown with no override still uses `cfg.abstol`. The residual-norm
//! gate (`residual_norm <= cfg.abstol`, just above) stays a single global scalar — reweighting
//! that `inf_norm` check into a per-row form is a separate design question, out of scope here.
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
    /// using [`crate::convergence::VT_NOMINAL`]/[`crate::convergence::default_vcrit`] as a
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
    // Unlike `kinds`, this always runs: every `solve_from` call's per-iteration convergence
    // check needs it, not just `shunt_gmin` (§ nature-metadata wiring's module doc comment).
    let per_abstol = mna::classify_abstol(instances, dim, cfg.abstol);

    let mut x = vec![0.0; dim];
    // `gmin_for_step(step, 0)` returns `0.0` at `step == 0`, so `gmin_steps == 0` collapses
    // this to exactly one iteration at `gmin = 0` — the original, un-homotopied solve.
    for step in 0..=cfg.gmin_steps {
        let gmin = convergence::gmin_for_step(step, cfg.gmin_steps);
        x = solve_from(x, instances, dim, cfg, gmin, &kinds, &per_abstol)?;
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
    per_abstol: &[f64],
) -> Result<Vec<f64>, CoreError> {
    let vt = convergence::VT_NOMINAL;
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
            if applied.abs() > cfg.reltol * vnew.abs() + per_abstol[i] {
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
    use va_abi::reference::diode::VT_NOMINAL;
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
    fn per_unknown_abstol_override_changes_the_convergence_decision() {
        // The exact `solves_resistor_divider` circuit, but capped to 1 Newton iteration
        // (`limit_junctions: false` for a deterministic, uncla­mped first step — the divider is
        // linear, so that one step already lands exactly on the solution; only the *declared*
        // convergence outcome is what this test is about). At the default (tight, 1e-12)
        // abstol, the first iteration's own jump (0 -> ~2V/1V/1mA) is nowhere near "small", so
        // `update_small` doesn't fire and 1 iteration isn't enough — the residual only settles
        // to ~0 on the *second* pass, exactly `reports_non_convergence`'s shape.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let cfg = NewtonConfig {
            max_iters: 1,
            limit_junctions: false,
            ..NewtonConfig::default()
        };

        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];
        assert!(
            matches!(solve(&insts, 3, cfg), Err(CoreError::NoConvergence { .. })),
            "the default tight abstol should not absorb the first iteration's jump"
        );

        // Every unknown's abstol loosened (§ nature-metadata wiring) past the size of that
        // first jump: `update_small` now holds after the very first iteration, at the exact
        // same 1-iteration budget — the override, not a wider budget, is what changes this.
        let vs_loose = crate::testutil::AbstolOverride {
            inner: &vs,
            overrides: &[(0, 10.0), (2, 10.0)], // node0, this source's own branch current
        };
        let r1_loose = crate::testutil::AbstolOverride {
            inner: &r1,
            overrides: &[(1, 10.0)], // node1, via r1's own local index for it
        };
        let insts_loose: [&dyn ModelInstance; 3] = [&vs_loose, &r1_loose, &r2];
        let x = solve(&insts_loose, 3, cfg).expect("loosened abstol converges within 1 iteration");
        assert!((x[0] - 2.0).abs() < 1e-9, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-9, "midpoint = {}", x[1]);
    }

    #[test]
    fn solves_diode_resistor_clamp() {
        // Vin = 1 V → R = 1 kΩ → diode to ground. Nonlinear: exercises the exp Jacobian.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
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
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
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
    fn gmin_stepping_converges_a_circuit_plain_newton_cannot() {
        // The demo circuit `docs/roadmap.md`'s T3.3 flagged as missing: one that genuinely
        // *needs* gmin stepping, not just tolerates it. 20 diodes in series behind a 10 Ω
        // resistor, driven at 20 V from a cold (zero) start: a real, physically sane operating
        // point exists (~0.81 V/diode, ~0.38 A), but plain Newton's log-ramp junction limiting
        // walks the chain's *internal* node voltages there one node at a time with no other
        // conductance path to keep them in check, and some node's voltage crosses into the
        // exponential's `f64` overflow range en route -- a genuine `Err(Singular)` from a
        // non-finite Jacobian entry, confirmed independent of iteration budget (still fails at
        // `max_iters: 2000`, ~13x this test's default). `gmin` stepping's early, well-
        // conditioned stages (a competing shunt conductance to ground at every node) keep the
        // whole chain in range long enough to land near the true operating point before the
        // final, unshunted stage — which then only needs a handful of iterations to finish.
        let n_diodes = 20;
        let branch = n_diodes + 1;
        let dim = branch + 1;
        let vs = VSource::new(0, GROUND, branch, 20.0);
        let r = Resistor::new(0, 1, 10.0);
        let mut diodes = Vec::new();
        for i in 1..n_diodes {
            diodes.push(Diode::new(i, i + 1, 1e-14, 1.0, VT_NOMINAL));
        }
        diodes.push(Diode::new(n_diodes, GROUND, 1e-14, 1.0, VT_NOMINAL));
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs, &r];
        insts.extend(diodes.iter().map(|d| d as &dyn ModelInstance));

        // Not just this test's default iteration budget: plain Newton stays singular even
        // given a very generous one, proving this isn't a "just needs more iterations" case.
        let cfg_no_gmin = NewtonConfig {
            max_iters: 2000,
            ..NewtonConfig::default()
        };
        assert!(
            matches!(solve(&insts, dim, cfg_no_gmin), Err(CoreError::Singular)),
            "expected plain Newton to hit overflow regardless of iteration budget"
        );

        let cfg_with_gmin = NewtonConfig {
            max_iters: 150,
            gmin_steps: 30,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, dim, cfg_with_gmin).expect("gmin stepping converges");

        // KCL: the same current flows through the resistor, the first diode, and the source
        // branch (a single series loop).
        let i_r = (x[0] - x[1]) / 10.0;
        let vd0 = x[1] - x[2];
        let i_d0 = diodes[0].current(vd0);
        assert!((i_r - i_d0).abs() < 1e-6, "KCL imbalance: {i_r} vs {i_d0}");
        assert!(
            (x[branch].abs() - i_r).abs() < 1e-6,
            "branch current mismatch: {} vs {i_r}",
            x[branch]
        );
    }

    #[test]
    fn reports_non_convergence() {
        // One iteration is not enough for a nonlinear solve from the zero guess.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
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
