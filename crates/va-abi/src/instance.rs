//! The [`ModelInstance`] trait — the unit `va-core` solves on.

use crate::stamps::StampSink;

/// The structural role of one entry in [`ModelInstance::unknowns`], distinguishing a KCL
/// node from a constraint row — needed by convergence aids (e.g. `va-core`'s `gmin` stepping)
/// that must only ever touch the former.
///
/// This is **not** about physical quantity (volts vs. amps) — it's about what kind of
/// equation the unknown's residual *row* represents, since that's what determines whether
/// shunting a conductance to ground at that row is a sound homotopy aid or a corrupted
/// constraint. Node-vs-branch is invisible from a global index alone: it depends on which
/// equation was stamped into that row, which only the instance that owns the row knows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnknownKind {
    /// A potential (node) unknown. Its residual row is a KCL current-sum — the sum of every
    /// stamped current into that node must be zero at the solution. Safe to shunt with a
    /// `gmin`-style conductance to ground.
    Node,
    /// A branch-current or other constraint-row unknown. Its residual row enforces some other
    /// equation entirely (e.g. an ideal voltage source's `V(p) − V(n) = value`), not a KCL sum.
    /// A `gmin` shunt at this row would corrupt that constraint, not aid convergence.
    Branch,
}

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

    /// The [`UnknownKind`] of `unknowns()[i]` — i.e. `i` indexes into the *position* within
    /// this instance's own `unknowns()` list, not a global index.
    ///
    /// Default: [`UnknownKind::Node`], correct for every two-terminal resistive/charge-storage
    /// device (the common case — a resistor, capacitor, or diode never introduces a row that
    /// isn't a KCL node sum). Override this only for an instance that introduces its own
    /// constraint row, the way [`crate::reference::VSource`] does for its branch current
    /// (§4/§6 additive change — added without breaking any existing implementor, exactly the
    /// "prefer a default method" guidance in `docs/bridges/interface-beta-abi.md`).
    fn unknown_kind(&self, i: usize) -> UnknownKind {
        let _ = i;
        UnknownKind::Node
    }

    /// A per-unknown absolute-tolerance override for `va-core`'s Newton convergence check
    /// (`unknowns()[i]`'s own tolerance, not indexed globally — same convention as
    /// [`Self::unknown_kind`]), sourced from a Verilog-A model's discipline/nature metadata
    /// (§ nature-metadata wiring, e.g. `nature Voltage; abstol = 1e-6; endnature`).
    ///
    /// Default `None`: no override, so `va-core` falls back to its own configured default
    /// (`va-core::newton::NewtonConfig::abstol`) — correct for every hand-written
    /// `crate::reference` model (none of them are compiled from Verilog-A source, so none has
    /// discipline metadata to report) and for a `va-codegen`-generated model whose module
    /// declared no `discipline`/`nature` preamble. Only `va-codegen`'s generated models
    /// override this, and only for their own node-kind unknowns (an auxiliary branch-current
    /// unknown has no natural per-unknown tolerance source and stays `None` too — see
    /// `va_ir::NodeDecl::abstol`'s doc comment).
    fn unknown_abstol(&self, i: usize) -> Option<f64> {
        let _ = i;
        None
    }

    /// Evaluate the model at solution vector `x` and emit its contributions into `sink`.
    ///
    /// Must emit the resistive channel (residual + Jacobian). Models with storage also emit
    /// the charge channel; DC analyses simply ignore it. `x` is indexed by global unknown
    /// index — read your terminals via the indices returned by [`Self::unknowns`].
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
}
