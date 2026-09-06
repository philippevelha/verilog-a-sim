//! Interface β's **event-registration channel** (§6 change, 2026-09-06): how a model tells the
//! transient engine which conditions it wants solve points placed at.
//!
//! # Why this is not the stamp sink
//!
//! [`crate::StampSink::bound_step`] is already a model→engine scheduling hint, so the obvious
//! move is to add `cross` beside it. That is wrong for three reasons, and they are the reasons
//! this is a separate channel — the same argument that gave noise its own channel in T5.2.
//!
//! 1. **Cadence.** `load` runs once per *Newton iteration*, and again for every rejected
//!    timestep. An event registration is meaningful once per **accepted** timepoint: it is a
//!    statement about the trajectory, and a rejected candidate is not on the trajectory. Riding
//!    `load` would emit each registration dozens of times per step, nearly all to be discarded.
//!    `bound_step` gets away with it precisely because it is idempotent and identity-free —
//!    "no longer than `dt`", minimum-of-all, no bookkeeping.
//! 2. **Identity.** A crossing is detected by comparing *this* accepted value of an expression
//!    against *the previous* accepted value of the same expression. That needs a stable slot
//!    per monitored site, which `bound_step`'s shape has nowhere to put.
//! 3. **Purity.** `load` must stay a pure function of `(x, ctx, committed state)`. Detecting a
//!    crossing is inherently a statement about two timepoints, so it belongs where the consumer
//!    owns the history — the same read-old/write-new split [`crate::state`] describes.
//!
//! # What a consumer must do
//!
//! Poll [`crate::ModelInstance::events`] **once per accepted timepoint, after committing
//! state**, never during Newton and never for a rejected candidate. Keep the previous accepted
//! value of each `(instance, slot)` pair; a sign change between it and the current value, in the
//! requested [`CrossDir`], is a crossing. Interpolate the crossing time between the two
//! bracketing timepoints, and — if the model asked for it via [`EventSink::breakpoint`] — place
//! a solve point there.
//!
//! # What this deliberately does not do yet
//!
//! It carries **registration** only: the model says what to watch, and the consumer reports
//! what fired. It does not yet carry **notification** back into `load`, so the body of an
//! `@(cross(...))` statement still cannot be run at the firing timepoint — which is why
//! `va-frontend` continues to refuse that construct rather than accept it and mis-run it (see
//! `docs/roadmap.md`'s analog-event checklist). Notification is the next step, and it needs a
//! per-instance "these fired" input alongside `state`, not a change here.
//!
//! Nor is the interpolated crossing a genuine re-solve *at* the crossing time. That
//! simplification is the one [`crate::events`]'s consumer already documents for its own
//! watches, and it is honest for the same reason: two accepted points bracketing a crossing are
//! close together, because the same LTE control that bounds the state's error between them
//! bounds the interpolation error too.

/// Which direction of zero crossing a monitored expression cares about — Verilog-A's `cross`
/// direction argument (LRM §5.10.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CrossDir {
    /// `+1`: fires only when the expression goes from negative to positive.
    Rising,
    /// `-1`: fires only when the expression goes from positive to negative.
    Falling,
    /// `0`, and the LRM's default when the argument is omitted: fires on either direction.
    #[default]
    Either,
}

impl CrossDir {
    /// Verilog-A spells the direction as an integer; map it the LRM's way. Any value other
    /// than `+1`/`-1` reads as [`CrossDir::Either`], which is the LRM's `0`.
    pub fn from_lrm(dir: i64) -> Self {
        match dir {
            d if d > 0 => CrossDir::Rising,
            d if d < 0 => CrossDir::Falling,
            _ => CrossDir::Either,
        }
    }

    /// Whether a move from `prev` to `now` is a crossing this direction cares about.
    ///
    /// A crossing is a change of *sign*, so landing exactly on zero counts as having reached
    /// it: `prev < 0, now == 0` is a rising crossing. Treating `0` as its own third state
    /// instead would make a signal that settles exactly at the threshold fire twice, once on
    /// arrival and once on departure.
    pub fn fires(self, prev: f64, now: f64) -> bool {
        let rising = prev < 0.0 && now >= 0.0;
        let falling = prev >= 0.0 && now < 0.0;
        match self {
            CrossDir::Rising => rising,
            CrossDir::Falling => falling,
            CrossDir::Either => rising || falling,
        }
    }
}

/// The channel a model registers its transient events on.
///
/// Constructed by the consumer, never by the model — the same ownership shape as
/// [`crate::StampSink`] and [`crate::NoiseSink`].
pub trait EventSink {
    /// Report the current value of monitored expression `slot`, and the direction of crossing
    /// this instance cares about.
    ///
    /// `slot` is this instance's own index, `0..event_count()`, and must be **stable across
    /// timepoints**: the consumer pairs this value with the one it stored for the same slot at
    /// the previous accepted timepoint, so a slot that means different expressions at different
    /// times would compare unrelated numbers. A model whose monitored sites sit inside an `if`
    /// should therefore report every slot every time — reporting the expression's value is
    /// always well defined even when the branch that uses it is not taken.
    ///
    /// The value is the expression Verilog-A wrote inside `cross(...)`, already reduced to
    /// "distance from the threshold": `cross(V(out) - 2.5, +1)` reports `V(out) - 2.5`, so a
    /// crossing is a change of sign and the consumer needs no separate threshold.
    fn monitor(&mut self, slot: usize, value: f64, dir: CrossDir);

    /// Ask for a solve point at absolute time `t` seconds — Verilog-A's `timer`, and what a
    /// `cross` site needs to have its crossing resolved rather than merely noticed.
    ///
    /// A request, like [`crate::StampSink::bound_step`]: a consumer honours it by placing a
    /// timepoint at or near `t`, subject to its own minimum step. A `t` at or before the
    /// current time, or a non-finite one, is meaningless and is ignored rather than stalling
    /// the run.
    fn breakpoint(&mut self, t: f64);
}

/// An [`EventSink`] that records into plain vectors — for tests, and for a consumer that wants
/// the registrations as data rather than streaming them.
#[derive(Clone, Debug, Default)]
pub struct RecordingEventSink {
    /// One entry per [`EventSink::monitor`] call, in call order.
    pub monitors: Vec<(usize, f64, CrossDir)>,
    /// One entry per [`EventSink::breakpoint`] call, in call order.
    pub breakpoints: Vec<f64>,
}

impl RecordingEventSink {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// The reported value for `slot`, if this instance reported one.
    pub fn value_of(&self, slot: usize) -> Option<f64> {
        self.monitors
            .iter()
            .find(|(s, _, _)| *s == slot)
            .map(|(_, v, _)| *v)
    }
}

impl EventSink for RecordingEventSink {
    fn monitor(&mut self, slot: usize, value: f64, dir: CrossDir) {
        self.monitors.push((slot, value, dir));
    }

    fn breakpoint(&mut self, t: f64) {
        self.breakpoints.push(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_maps_the_lrm_spelling() {
        assert_eq!(CrossDir::from_lrm(1), CrossDir::Rising);
        assert_eq!(CrossDir::from_lrm(-1), CrossDir::Falling);
        assert_eq!(CrossDir::from_lrm(0), CrossDir::Either);
        // The LRM gives no other value a meaning; `Either` is the documented default.
        assert_eq!(CrossDir::default(), CrossDir::Either);
    }

    #[test]
    fn rising_and_falling_are_distinguished() {
        assert!(CrossDir::Rising.fires(-1.0, 1.0));
        assert!(!CrossDir::Rising.fires(1.0, -1.0));
        assert!(CrossDir::Falling.fires(1.0, -1.0));
        assert!(!CrossDir::Falling.fires(-1.0, 1.0));
        assert!(CrossDir::Either.fires(-1.0, 1.0));
        assert!(CrossDir::Either.fires(1.0, -1.0));
    }

    #[test]
    fn no_sign_change_is_not_a_crossing() {
        assert!(!CrossDir::Either.fires(1.0, 2.0));
        assert!(!CrossDir::Either.fires(-2.0, -1.0));
    }

    /// Landing exactly on zero counts as having arrived, so a signal that settles at the
    /// threshold fires once (on arrival) rather than twice (arrival and departure).
    #[test]
    fn landing_on_zero_fires_once() {
        assert!(CrossDir::Rising.fires(-1.0, 0.0));
        assert!(!CrossDir::Rising.fires(0.0, 1.0));
    }

    #[test]
    fn the_recorder_keeps_call_order_and_finds_a_slot() {
        let mut sink = RecordingEventSink::new();
        sink.monitor(1, 4.0, CrossDir::Rising);
        sink.monitor(0, -2.0, CrossDir::Either);
        sink.breakpoint(1e-6);
        assert_eq!(sink.monitors.len(), 2);
        assert_eq!(sink.monitors[0].0, 1, "call order is preserved");
        assert_eq!(sink.value_of(0), Some(-2.0));
        assert_eq!(sink.value_of(2), None);
        assert_eq!(sink.breakpoints, vec![1e-6]);
    }
}
