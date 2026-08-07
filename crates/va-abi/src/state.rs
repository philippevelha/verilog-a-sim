//! Interface β's **per-instance state channel** (§6 change, 2026-08-06): the small amount a
//! model is allowed to remember from one accepted timepoint to the next.
//!
//! # Why the model does not own this
//!
//! The obvious implementation — give the model a `RefCell<Vec<f64>>` and let it remember — is
//! wrong, and it is worth saying why, because it is the tempting one.
//! [`crate::ModelInstance::load`] takes `&self` and is **pure**: identical inputs produce
//! identical stamps. Three consumers depend on that, and a self-mutating model breaks all three:
//!
//! 1. **Newton** re-enters `load` many times per timepoint. A model that advanced its own
//!    history per call would change the equations under the solver's feet, so the iteration
//!    would chase a moving fixed point instead of converging to a fixed one.
//! 2. **The LTE controller** solves every candidate step *twice* (`va_transient`'s embedded
//!    pair) and throws rejected steps away entirely. A self-mutating model would commit history
//!    for a timepoint that never happened.
//! 3. **Finite-difference Jacobian checks** (`CLAUDE.md` §5) perturb `x` and re-evaluate. If
//!    evaluation has side effects, the check measures the side effects too.
//!
//! So the storage is **solver-owned**: the instance declares how much it needs
//! ([`crate::ModelInstance::state_len`]), the consumer allocates and slices it, and the consumer
//! alone decides when a proposal becomes history.
//!
//! # Read-old, write-new
//!
//! That is the whole trick, and it is what *preserves* the purity invariant rather than
//! weakening it. [`ModelState::get`] always reads the value committed at the **last accepted
//! timepoint**; [`ModelState::set`] always writes a **separate** proposal buffer. A model can
//! therefore never observe another iteration's proposal — not its own from a previous Newton
//! iteration, and not one from a rejected step. `load` stays a pure function of
//! `(x, ctx, committed-state)`; what changed is that it now has an output channel besides the
//! [`crate::StampSink`].
//!
//! # What a consumer must do
//!
//! Hold two buffers, `committed` and `scratch`. Before each evaluation sweep, **copy
//! `committed` into `scratch`** — this is load-bearing, not hygiene: a model whose `set` sits
//! inside an `if` may not write every slot on every path, and pre-seeding makes an unwritten
//! slot mean "unchanged" rather than "whatever a rejected candidate last wrote". Then call
//! `load` with `prev` borrowed from `committed` and `next` from `scratch`. On an **accepted**
//! timepoint, `committed = scratch`; on a rejected one, do nothing.
//!
//! DC, AC and noise never commit: they have no notion of an accepted timepoint. Their state
//! stays zero and [`crate::AnalysisCtx::is_initial_step`] stays `true`, which is what makes a
//! `slew`/`transition` settle immediately to its input in a static solve — the LRM-correct
//! steady-state answer, and the same one this project produced before the channel existed.
//!
//! # What this is not
//!
//! It is **fixed-size**. A construct needing an unbounded trajectory — `absdelay`, which must
//! produce `value(t − delay)` from however many samples the LTE controller happened to place
//! inside the delay window — cannot be built on a state vector alone; it needs an interpolated
//! history buffer, which is a separate design. See `docs/proposals/model-state.md` §1.3.
//!
//! It is also **not the Newton-iterate channel** `$limit` wants. That construct needs the
//! previous *iterate* within a single timepoint solve, never anything committed across steps —
//! a different lifetime entirely, and one whose fold is a convergence-robustness issue rather
//! than a wrong answer (§1.1 of the same document).

/// A model instance's view of its own state for one evaluation.
///
/// Constructed by the consumer, never by the model. See this module's doc comment for the
/// ownership and commit rules that make it sound.
#[derive(Debug)]
pub struct ModelState<'a> {
    /// State as committed at the last accepted timepoint. Read-only, and identical across
    /// every evaluation of the same candidate timepoint.
    prev: &'a [f64],
    /// This evaluation's proposal. Pre-seeded from `prev` by the consumer, so an unwritten slot
    /// reads as "unchanged".
    next: &'a mut [f64],
}

impl<'a> ModelState<'a> {
    /// Wrap a consumer's committed/proposal slices for one instance.
    ///
    /// `prev` and `next` must be the same length — the instance's own
    /// [`crate::ModelInstance::state_len`]. A mismatch is a consumer bug; the accessors below
    /// are bounds-checked rather than panicking, so it degrades to "no state" instead of
    /// bringing down a simulation.
    pub fn new(prev: &'a [f64], next: &'a mut [f64]) -> Self {
        ModelState { prev, next }
    }

    /// An empty state, for a stateless instance and for tests.
    ///
    /// Every reference model in [`crate::reference`] and the overwhelming majority of compiled
    /// models want exactly this: `state_len()` is `0`, so there is nothing to read or write.
    pub fn stateless() -> ModelState<'static> {
        ModelState {
            prev: &[],
            next: &mut [],
        }
    }

    /// Number of slots this instance declared.
    pub fn len(&self) -> usize {
        self.prev.len().min(self.next.len())
    }

    /// Whether this instance declared no state at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole committed slice, for an implementor that wants to read it in bulk rather than
    /// slot by slot (`va-codegen` copies it once per evaluation to seed its proposal buffer).
    ///
    /// Read-only by construction — there is deliberately no `&mut` counterpart, because
    /// mutating committed state in place is exactly what the read-old/write-new split exists to
    /// prevent.
    pub fn committed(&self) -> &[f64] {
        self.prev
    }

    /// Read slot `slot` **as committed at the last accepted timepoint**.
    ///
    /// Never reflects a [`Self::set`] made during this evaluation, or during any other
    /// evaluation of the same candidate timepoint — that is the invariant that keeps `load`
    /// pure. Out-of-range reads `0.0` rather than panicking (§5: libraries do not panic on bad
    /// input), which is also the right value for a slot a model has never written.
    pub fn get(&self, slot: usize) -> f64 {
        self.prev.get(slot).copied().unwrap_or(0.0)
    }

    /// Propose `value` for slot `slot`, to be committed only if this evaluation turns out to be
    /// the one at an accepted timepoint.
    ///
    /// Out-of-range writes are dropped rather than panicking, mirroring how
    /// [`crate::StampSink`] drops a stamp at the reference node.
    pub fn set(&mut self, slot: usize, value: f64) {
        if let Some(cell) = self.next.get_mut(slot) {
            *cell = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defining property: a `set` is invisible to a subsequent `get` in the same
    /// evaluation. Without this, a model's second Newton iteration would read its own first
    /// iteration's guess as though it were history, and the solve would chase a moving target.
    #[test]
    fn a_write_is_never_visible_to_a_read_in_the_same_evaluation() {
        let committed = [1.0, 2.0];
        let mut scratch = committed; // consumer pre-seeds from committed
        {
            let mut st = ModelState::new(&committed, &mut scratch);
            assert_eq!(st.get(0), 1.0);
            st.set(0, 99.0);
            assert_eq!(st.get(0), 1.0, "get must still read the committed value");
            assert_eq!(st.get(1), 2.0);
        }
        // The proposal landed in the consumer's scratch buffer, not in committed.
        assert_eq!(scratch, [99.0, 2.0]);
        assert_eq!(committed, [1.0, 2.0]);
    }

    /// An unwritten slot keeps its committed value, because the consumer pre-seeds `scratch`.
    /// That is what makes a `set` inside a not-taken `if` mean "unchanged" instead of
    /// resurrecting whatever a rejected candidate wrote.
    #[test]
    fn an_unwritten_slot_stays_at_its_committed_value() {
        let committed = [5.0, 6.0, 7.0];
        let mut scratch = committed;
        {
            let mut st = ModelState::new(&committed, &mut scratch);
            st.set(1, -1.0); // only slot 1 written this evaluation
        }
        assert_eq!(scratch, [5.0, -1.0, 7.0]);
    }

    /// A stateless instance is the common case and must be free of surprises: no slots, reads
    /// give zero, writes go nowhere, and nothing panics.
    #[test]
    fn a_stateless_instance_reads_zero_and_absorbs_writes() {
        let mut st = ModelState::stateless();
        assert_eq!(st.len(), 0);
        assert!(st.is_empty());
        assert_eq!(st.get(0), 0.0);
        st.set(0, 1.0);
        assert_eq!(st.get(0), 0.0);
    }

    /// Out-of-range access is bounds-checked, not a panic — `CLAUDE.md` §5.
    #[test]
    fn out_of_range_access_is_ignored_rather_than_panicking() {
        let committed = [1.0];
        let mut scratch = committed;
        {
            let mut st = ModelState::new(&committed, &mut scratch);
            assert_eq!(st.get(7), 0.0);
            st.set(7, 3.0);
        }
        assert_eq!(scratch, [1.0]);
    }
}
