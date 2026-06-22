//! The [`ModelInstance`] trait — the unit `va-core` solves on.

use crate::stamps::StampSink;

/// A loadable model instance: a concrete device wired to specific global unknown indices.
///
/// Implementations are produced two ways and are interchangeable to `va-core`:
/// - hand-written, in [`crate::reference`];
/// - generated from Verilog-A by `va-codegen`.
pub trait ModelInstance {
    /// The global unknown indices this instance contributes to (nodes + internal unknowns).
    ///
    /// The order is the instance's own local convention; the values are positions in the
    /// global solution vector `x` passed to [`Self::load`].
    fn unknowns(&self) -> &[usize];

    /// Evaluate the model at solution vector `x` and emit its contributions into `sink`.
    ///
    /// Must emit the resistive channel (residual + Jacobian). Models with storage also emit
    /// the charge channel; DC analyses simply ignore it. `x` is indexed by global unknown
    /// index — read your terminals via the indices returned by [`Self::unknowns`].
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
}
