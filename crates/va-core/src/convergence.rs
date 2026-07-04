//! Convergence aids: junction limiting, damping, and `gmin` stepping.
//!
//! These keep Newton out of overflow on stiff exponential devices (diodes, BJTs). They are
//! pure numerical helpers with no model knowledge.
//!
//! # Status
//!
//! **`limit_junction` is now wired into [`crate::newton::solve`]**
//! (`NewtonConfig::limit_junctions`, default on): applied as a blanket per-unknown clamp each
//! iteration, using [`VT_300K`] and [`default_vcrit`]. The earlier blocker ("needs the
//! device's previous-iteration voltage") doesn't actually hold — the Newton loop already has
//! both `x[i]` (before the update) and `x[i] + dx[i]` (the proposed update) for every unknown,
//! with no ABI change needed. The *real* limitation is that `va-core` has no way to know
//! *which* unknowns are junction voltages specifically (the stateless
//! [`va_abi::ModelInstance`] ABI exposes no per-device `Is`/`n`), so this clamps every unknown
//! alike rather than only recognized junctions the way a real SPICE implementation would.
//! That's sound, not just convenient: a converged Newton solve is a fixed point of the
//! *unlimited* equations (same reasoning as `$limit`'s elaboration-time fold in
//! `va-frontend`) — limiting only reshapes the iteration path, never the answer it settles
//! on. The known cost is it can slow convergence on unknowns that were never exponential in
//! the first place (a purely linear resistor network's node voltages, or a branch-current
//! unknown numerically large enough to cross [`default_vcrit`]'s threshold) — disable via
//! `NewtonConfig::limit_junctions = false` if that matters more than the diode/BJT robustness.
//!
//! **`gmin_for_step` is still *not* wired in**, and not just because nobody's gotten to it:
//! gmin shunts a conductance from every circuit *node* to ground, but MNA's branch-current
//! unknowns (e.g. `va_abi::reference::VSource`'s current, whose row enforces the constraint
//! `V(p) − V(n) = value`, not a KCL sum) would be silently corrupted by the same shunt — it's
//! not a node equation to begin with. `va-core`'s `ModelInstance`/`StampSink` ABI has no
//! per-unknown "node vs. branch-constraint" tag today, so there's no way to apply gmin only
//! where it's sound. Wiring it in for real needs that tag added to Interface β — a
//! coordinated interface change (`CLAUDE.md` §6), not a same-crate fix.

/// Limit the change in a p–n junction voltage between Newton iterations (`pnjlim`-style),
/// returning the limited new voltage. Prevents the diode exponential from overflowing by
/// capping forward steps to a logarithmic increment.
///
/// `vnew` is the proposed voltage, `vold` the previous one, `vt` the thermal voltage, and
/// `vcrit` the critical voltage about which limiting pivots. Reverse and small-signal steps
/// pass through unchanged.
pub fn limit_junction(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * vt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                vold + vt * arg.ln()
            } else {
                vcrit
            }
        } else {
            vt * (vnew / vt).ln()
        }
    } else {
        vnew
    }
}

/// Physical thermal voltage `kT/q` at ~300 K, in volts.
///
/// Kept as its own copy here (rather than importing `va_abi::reference::diode::VT_300K`) so
/// `va-core`'s generic Newton loop doesn't reach into one specific reference *device* for a
/// physical constant that has nothing to do with that device in particular.
pub const VT_300K: f64 = 0.025_852;

/// Nominal saturation current used only to derive [`default_vcrit`] — representative of a
/// small silicon junction, not read from any specific device in the circuit being solved.
/// `va-core` has no way to know a real device's `Is`/`n` (see this module's doc comment), so
/// this backs a blanket threshold applied to every unknown alike, not a per-junction one.
const NOMINAL_IS: f64 = 1e-14;

/// The critical voltage [`limit_junction`] pivots on, derived the same way SPICE derives a
/// diode's `vcrit` from a saturation current: `vt * ln(vt / (sqrt(2) * is))`, using
/// [`NOMINAL_IS`] in place of a real device's `Is` (`va-core` has none to read — see the
/// module doc comment).
pub fn default_vcrit(vt: f64) -> f64 {
    vt * (vt / (std::f64::consts::SQRT_2 * NOMINAL_IS)).ln()
}

/// The `gmin` conductance to shunt across every node at `step` of a gmin-stepping ramp of
/// `total_steps` stages.
///
/// The schedule decreases geometrically from a large starting conductance toward a small
/// floor, then returns `0.0` once `step` reaches `total_steps` (the final, unshunted solve).
pub fn gmin_for_step(step: usize, total_steps: usize) -> f64 {
    /// Initial (largest) shunt conductance, in siemens.
    const GMAX: f64 = 1e-3;
    /// Final floor conductance the ramp approaches, in siemens.
    const GMIN: f64 = 1e-12;

    if total_steps == 0 || step >= total_steps {
        return 0.0;
    }
    let frac = step as f64 / total_steps as f64;
    GMAX * (GMIN / GMAX).powf(frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VT: f64 = 0.025_852;

    #[test]
    fn junction_limiting_bounds_step() {
        // A wild 5 V proposal from a 0.6 V bias is clamped to a modest forward increment.
        let limited = limit_junction(5.0, 0.6, VT, 0.6);
        assert!(limited < 1.0, "limited voltage not bounded: {limited}");
        assert!(limited > 0.6, "should still step forward: {limited}");
    }

    #[test]
    fn junction_limiting_passes_small_steps() {
        // |vnew − vold| < 2·vt: returned unchanged.
        let v = limit_junction(0.61, 0.60, VT, 0.6);
        assert_eq!(v, 0.61);
    }

    #[test]
    fn junction_limiting_passes_reverse_bias() {
        // Below vcrit: no limiting.
        let v = limit_junction(-2.0, 0.1, VT, 0.6);
        assert_eq!(v, -2.0);
    }

    #[test]
    fn default_vcrit_matches_silicon_diode_turn_on() {
        // A real silicon-diode Is ~1e-14 A, n=1 gives a vcrit around 0.6-0.75 V (the classic
        // SPICE default) — sanity-check the formula lands in that neighborhood, not just any
        // finite number.
        let vcrit = default_vcrit(VT);
        assert!(
            (0.6..0.8).contains(&vcrit),
            "vcrit out of expected range: {vcrit}"
        );
    }

    #[test]
    fn gmin_schedule_is_monotone_and_terminates() {
        let total = 10;
        let mut prev = f64::INFINITY;
        for step in 0..total {
            let g = gmin_for_step(step, total);
            assert!(
                g > 0.0 && g < prev,
                "not strictly decreasing at {step}: {g}"
            );
            prev = g;
        }
        assert_eq!(gmin_for_step(total, total), 0.0);
        assert_eq!(gmin_for_step(0, 0), 0.0);
    }
}
