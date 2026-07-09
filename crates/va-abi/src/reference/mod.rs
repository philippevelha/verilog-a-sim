//! Hand-written reference models implementing [`crate::ModelInstance`].
//!
//! These are **real, working** models (not `todo!()`), used both as the bring-up devices for
//! `va-core` and as the oracle that AD-generated models in `va-codegen` are checked against.
//! Most are two-terminal compact models sharing this module's stamping helpers; [`Bjt`] is the
//! one three-terminal exception (its own cross-coupled stamps, no shared helper applies).

pub mod bjt;
pub mod capacitor;
pub mod diode;
pub mod resistor;
pub mod vsource;

pub use bjt::Bjt;
pub use capacitor::Capacitor;
pub use diode::Diode;
pub use resistor::Resistor;
pub use vsource::VSource;

use crate::stamps::StampSink;

/// Sentinel global index for the reference (ground) node.
///
/// [`crate::stamps::DenseStamp`] (and the real `va-core` assembler) treat any index `>= dim`
/// as ground and silently drop its contributions, which is exactly the MNA ground reduction.
pub const GROUND: usize = usize::MAX;

/// Stamp a two-terminal **resistive** contribution between terminals `(p, n)`.
///
/// `i` is the branch current flowing from `p` to `n` at the current operating point, and `g`
/// is `di/dv` where `v = x[p] - x[n]`. This expands to the canonical 2×2 conductance stamp.
pub(crate) fn stamp_conductance(sink: &mut dyn StampSink, p: usize, n: usize, i: f64, g: f64) {
    sink.residual(p, i);
    sink.residual(n, -i);
    sink.jacobian(p, p, g);
    sink.jacobian(p, n, -g);
    sink.jacobian(n, p, -g);
    sink.jacobian(n, n, g);
}

/// Stamp a two-terminal **charge** contribution between terminals `(p, n)`.
///
/// `q` is the branch charge and `c = dq/dv`. Mirrors [`stamp_conductance`] into the charge
/// channel, which the transient integrator differentiates in time via a companion model.
pub(crate) fn stamp_charge(sink: &mut dyn StampSink, p: usize, n: usize, q: f64, c: f64) {
    sink.charge(p, q);
    sink.charge(n, -q);
    sink.dcharge(p, p, c);
    sink.dcharge(p, n, -c);
    sink.dcharge(n, p, -c);
    sink.dcharge(n, n, c);
}

/// Read the voltage across terminals `(p, n)` from the solution vector, treating any
/// out-of-range terminal as the 0 V reference node.
pub(crate) fn voltage_across(x: &[f64], p: usize, n: usize) -> f64 {
    let vp = x.get(p).copied().unwrap_or(0.0);
    let vn = x.get(n).copied().unwrap_or(0.0);
    vp - vn
}
