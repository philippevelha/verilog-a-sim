//! Convergence aids: junction limiting, damping, and `gmin` stepping.
//!
//! These keep Newton out of overflow on stiff exponential devices (diodes, BJTs). They are
//! pure numerical helpers with no model knowledge.
//!
//! # Status
//!
//! These functions are provided and unit-tested, but the v0 [`crate::newton`] loop does not
//! yet apply them automatically: junction limiting needs the device's previous-iteration
//! voltage (state the stateless [`va_abi::ModelInstance`] does not carry), and gmin stepping
//! needs a homotopy outer loop. They are the building blocks for that work and for T4.

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
