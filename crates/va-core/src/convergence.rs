//! Convergence aids: junction limiting, damping, and `gmin` stepping.
//!
//! These keep Newton out of overflow on stiff exponential devices (diodes, BJTs). They are
//! pure numerical helpers with no model knowledge.

/// Limit the change in a p-n junction voltage between Newton iterations (`pnjlim`-style),
/// returning the limited new voltage. Prevents the diode exponential from overflowing.
///
/// `vnew` is the proposed voltage, `vold` the previous one, `vt` the thermal voltage, and
/// `vcrit` the critical voltage about which limiting pivots.
pub fn limit_junction(_vnew: f64, _vold: f64, _vt: f64, _vcrit: f64) -> f64 {
    todo!("T3: implement pnjlim-style junction limiting")
}

/// The `gmin` conductance to shunt across every node this step of a gmin-stepping ramp.
pub fn gmin_for_step(_step: usize, _total_steps: usize) -> f64 {
    todo!("T3: implement gmin stepping schedule")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T3: junction limiting keeps |dV| bounded across an iteration"]
    fn junction_limiting_bounds_step() {}
}
