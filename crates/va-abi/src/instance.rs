//! The [`ModelInstance`] trait — the unit `va-core` solves on.

use crate::analysis::AnalysisCtx;
use crate::noise::NoiseSink;
use crate::stamps::StampSink;
use crate::state::ModelState;

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
    ///
    /// `ctx` describes the evaluation being asked for — which analysis, at what time and
    /// temperature (§4/§6 change, 2026-08-05; see [`crate::analysis`] for why it exists and
    /// what it deliberately omits). Most models ignore it entirely: a resistor's `V/R` is the
    /// same equation in every analysis, and every model in [`crate::reference`] takes that
    /// view. It matters for a compiled Verilog-A model whose source calls `analysis()` or
    /// reads `$abstime`.
    ///
    /// Unlike the three additive revisions before it, this one **changed an existing
    /// signature** rather than adding a defaulted method. That was deliberate: a default that
    /// let an implementor keep the context-free form would leave two ways to write a model,
    /// one of which is quietly wrong in transient — and the whole point of the channel is that
    /// every implementor sees it.
    ///
    /// `load` may be called any number of times for the same `ctx` (once per Newton iteration,
    /// and again on a rejected timestep). It must be a pure function of
    /// `(x, ctx, state-as-committed)` — see [`crate::state`] for why reading committed state
    /// preserves that purity rather than weakening it, and what a consumer owes in return.
    ///
    /// `state` is this instance's own [`Self::state_len`] slots. Almost every model ignores it:
    /// a resistor's `V/R` depends on nothing but `x`. It matters for a compiled model whose
    /// source calls `transition` or `slew` — constructs whose value at this timepoint depends on
    /// what happened at the last accepted one.
    fn load(&self, x: &[f64], ctx: &AnalysisCtx, state: &mut ModelState, sink: &mut dyn StampSink);

    /// Number of `f64` state slots this instance needs carried between accepted timepoints
    /// (§6 change, 2026-08-06 — see [`crate::state`]).
    ///
    /// Default **0**: stateless, which is what every model in [`crate::reference`] and the
    /// overwhelming majority of compiled models are. A consumer sums this over its instances
    /// once, at setup, and slices one flat buffer per instance — the same "declare your size,
    /// the consumer owns the array" shape [`Self::unknowns`] already uses for the solution
    /// vector.
    ///
    /// Must be constant for the lifetime of the instance; a consumer reads it once.
    fn state_len(&self) -> usize {
        0
    }

    /// Whether this instance's **small-signal** stamps depend on the frequency in
    /// [`AnalysisCtx::freq`] (§6 change, 2026-08-07 — Tier C).
    ///
    /// Default **`false`**, which is right for every device whose `G` and `C` are constants:
    /// a resistor, a capacitor, a linearized diode. Such a device's admittance already varies
    /// with `ω` through the assembled `G + jω·C` — that is not what this asks. It asks whether
    /// `G` and `C` *themselves* move, which happens only for a rational transfer function
    /// (`laplace_*`), whose real and imaginary parts are genuinely different at each point.
    ///
    /// This is a **cost switch**, and that is why it exists rather than being assumed true.
    /// `va_acnoise::ac::run` linearizes **once** when every instance reports `false` — the
    /// behavior, and the exact numbers, that predate Tier C — and once *per frequency point*
    /// as soon as any reports `true`. Making it opt-in keeps an ordinary AC sweep free.
    ///
    /// Must be constant for the lifetime of the instance; a consumer reads it once.
    fn is_frequency_dependent(&self) -> bool {
        false
    }

    /// Emit this instance's own **noise sources** at operating point `x` into `sink` —
    /// Interface β's noise channel (§4/§6 additive change, 2026-08-01; see [`crate::noise`] for
    /// what a source means and what this channel deliberately cannot express).
    ///
    /// The temperature a thermal source needs comes from `ctx.temp`; this method took a bare
    /// `temp: f64` argument until the analysis context absorbed it (2026-08-05), so that both
    /// entry points agree on where simulation conditions live.
    ///
    /// Default: **no sources**, i.e. a noiseless element. That is physically correct for an
    /// ideal capacitor and an ideal voltage source (neither dissipates, so neither has
    /// Johnson-Nyquist noise, and neither passes carriers across a barrier, so neither has shot
    /// noise), and it is the right answer for a compiled model whose Verilog-A source declares
    /// no noise at all. A device that *does* have noise overrides this — as does every
    /// `va-codegen`-generated model whose source calls `white_noise`/`flicker_noise`/
    /// `noise_table` (T5.3/T5.6): [`crate::reference::Resistor`] (thermal),
    /// [`crate::reference::Diode`] and [`crate::reference::Bjt`] (shot).
    ///
    /// Kept a default method for the same reason [`Self::unknown_kind`] and
    /// [`Self::unknown_abstol`] are (`docs/bridges/interface-beta-abi.md` §8): every existing
    /// implementor keeps compiling untouched.
    fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) {
        let _ = (x, ctx, sink);
    }
}
