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

/// How precisely a `cross` site wants its crossing resolved — Verilog-A's `time_tol` and
/// `expr_tol` (LRM §5.10.1).
///
/// They bound the error between the *true* crossing and the point at which the event triggers:
/// the event shall fire after the crossing, and while the signal is still inside the box those
/// two tolerances define. `None` means the model did not ask, and the LRM then leaves the
/// resolution to the tool.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CrossTol {
    /// Maximum time between the crossing and the firing, in seconds.
    pub time: Option<f64>,
    /// Maximum magnitude the monitored expression may still have when the event fires.
    pub expr: Option<f64>,
}

impl CrossTol {
    /// No tolerance requested — the tool chooses, which for this engine means firing at the
    /// first accepted timepoint past the crossing with no extra step control.
    pub const NONE: CrossTol = CrossTol {
        time: None,
        expr: None,
    };

    /// Whether a firing at `t_fire`, where the interpolated crossing was at `t_cross` and the
    /// monitored expression now reads `value`, satisfies what was asked for.
    ///
    /// A tolerance that was not requested is satisfied by anything. Both must hold when both
    /// are given, which is the LRM's rule.
    pub fn satisfied_by(self, t_cross: f64, t_fire: f64, value: f64) -> bool {
        self.time.is_none_or(|tt| t_fire - t_cross <= tt)
            && self.expr.is_none_or(|et| value.abs() <= et)
    }

    /// Whether either tolerance was requested at all.
    pub fn is_requested(self) -> bool {
        self.time.is_some() || self.expr.is_some()
    }
}

/// Everything a `cross`/`above` registration says about *how* to watch, beyond the value
/// itself.
///
/// Bundled into one struct rather than added as further [`EventSink::monitor`] parameters: the
/// method took `dir` at v0.9.2, `tol` at v0.9.6, and `at_initialization` would have been a
/// third widening in four versions. A spec struct absorbs the next one without touching any
/// implementor.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CrossSpec {
    /// Which edge to watch.
    pub dir: CrossDir,
    /// How precisely to resolve the crossing.
    pub tol: CrossTol,
    /// Whether this site also fires **during initialization and DC**, where a `cross` does not
    /// — what distinguishes Verilog-A's `above` from `cross` (LRM §5.10.2).
    ///
    /// The distinction is the whole reason `above` exists: a signal that is *already* past the
    /// threshold never crosses it, so a `cross` on it never fires at all. The LRM's own
    /// sample-and-hold example turns on exactly that.
    pub at_initialization: bool,
}

impl CrossSpec {
    /// A `cross(expr, dir)` with no tolerance — the plain case.
    pub fn cross(dir: CrossDir) -> Self {
        CrossSpec {
            dir,
            tol: CrossTol::NONE,
            at_initialization: false,
        }
    }

    /// An `above(expr)`: rising, and firing at initialization too.
    pub fn above() -> Self {
        CrossSpec {
            dir: CrossDir::Rising,
            tol: CrossTol::NONE,
            at_initialization: true,
        }
    }

    /// The same spec with `tol` attached.
    pub fn with_tol(self, tol: CrossTol) -> Self {
        CrossSpec { tol, ..self }
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
    ///
    /// `spec` says which edge to watch, how precisely to resolve it, and whether the site also
    /// fires at initialization ([`CrossSpec`]). It rides this call rather than sitting behind
    /// optional accessors because a consumer that ignores any of it is *silently failing to
    /// honour a request the source made* — the same argument that put the analysis context in
    /// `ModelInstance::load`'s signature instead of behind a default.
    fn monitor(&mut self, slot: usize, value: f64, spec: CrossSpec);

    /// Ask for a solve point at absolute time `t` seconds — Verilog-A's `timer`, and what a
    /// `cross` site needs to have its crossing resolved rather than merely noticed.
    ///
    /// A request, like [`crate::StampSink::bound_step`]: a consumer honours it by placing a
    /// timepoint at or near `t`, subject to its own minimum step. A `t` at or before the
    /// current time, or a non-finite one, is meaningless and is ignored rather than stalling
    /// the run.
    fn breakpoint(&mut self, t: f64);

    /// Schedule event `slot` to fire at absolute time `next` seconds — Verilog-A's `timer`
    /// (LRM §5.10.3).
    ///
    /// Differs from [`Self::breakpoint`] in carrying an **identity**: a breakpoint only asks
    /// the solver to stop somewhere, while this also says *which* of the instance's events
    /// fires when it does, so the consumer can report it through
    /// [`crate::ModelState::event_fired`] and the body can run.
    ///
    /// `next` must be **strictly after** the current evaluation time. A periodic timer is
    /// therefore re-registered at every accepted timepoint with its next occurrence, which
    /// keeps the model stateless about its own schedule: the arithmetic is a pure function of
    /// `(start, period, now)`. The consequence, stated rather than hidden: a fire time falling
    /// exactly on the run's initial timepoint is not delivered, because that point is a seed
    /// rather than a solved step.
    ///
    /// Default: forwards to [`Self::breakpoint`], so a consumer that only knows how to place
    /// timepoints still lands on the right one and merely cannot attribute the firing.
    fn timer(&mut self, slot: usize, next: f64) {
        let _ = slot;
        self.breakpoint(next);
    }
}

/// An [`EventSink`] that records into plain vectors — for tests, and for a consumer that wants
/// the registrations as data rather than streaming them.
#[derive(Clone, Debug, Default)]
pub struct RecordingEventSink {
    /// One entry per [`EventSink::monitor`] call, in call order.
    pub monitors: Vec<(usize, f64, CrossSpec)>,
    /// One entry per [`EventSink::breakpoint`] call, in call order.
    pub breakpoints: Vec<f64>,
    /// One `(slot, next_time)` per [`EventSink::timer`] call, in call order.
    pub timers: Vec<(usize, f64)>,
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
            .find(|(s, ..)| *s == slot)
            .map(|(_, v, ..)| *v)
    }
}

impl EventSink for RecordingEventSink {
    fn monitor(&mut self, slot: usize, value: f64, spec: CrossSpec) {
        self.monitors.push((slot, value, spec));
    }

    fn breakpoint(&mut self, t: f64) {
        self.breakpoints.push(t);
    }

    fn timer(&mut self, slot: usize, next: f64) {
        self.timers.push((slot, next));
    }
}

/// The consumer half of the event channel's **notification**: one flat buffer of "did slot `k`
/// of instance `i` fire at the evaluation being solved", sliced per instance.
///
/// Lives here rather than in a consumer crate because both consumers need it and must agree on
/// its shape: `va-transient` fires events across timepoints, and `va-core` fires `above` at a
/// DC operating point. Held fixed across every Newton iteration of one solve, which is what
/// keeps [`crate::ModelInstance::load`] a pure function of
/// `(x, ctx, committed state, fired events)`.
#[derive(Clone, Debug, Default)]
pub struct FiredEvents {
    /// Per-instance `[start, end)` bounds; length `instances.len() + 1`.
    offsets: Vec<usize>,
    flags: Vec<bool>,
}

impl FiredEvents {
    /// Size from each instance's declared [`crate::ModelInstance::event_count`], read once.
    pub fn new(instances: &[&dyn crate::ModelInstance]) -> Self {
        let mut offsets = Vec::with_capacity(instances.len() + 1);
        let mut total = 0usize;
        offsets.push(0);
        for inst in instances {
            total += inst.event_count();
            offsets.push(total);
        }
        FiredEvents {
            offsets,
            flags: vec![false; total],
        }
    }

    /// Instance `i`'s own slice, to hand to [`crate::ModelState::with_events`].
    pub fn slice(&self, i: usize) -> &[bool] {
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.flags[a..b],
            _ => &[],
        }
    }

    /// Mark slot `slot` of instance `i` as fired. Out-of-range is ignored rather than panicking,
    /// matching the rest of this ABI's bounds behaviour.
    pub fn set(&mut self, i: usize, slot: usize) {
        if let (Some(&a), Some(&b)) = (self.offsets.get(i), self.offsets.get(i + 1)) {
            if a + slot < b {
                self.flags[a + slot] = true;
            }
        }
    }

    /// Whether slot `slot` of instance `i` is currently marked.
    pub fn is_set(&self, i: usize, slot: usize) -> bool {
        self.slice(i).get(slot).copied().unwrap_or(false)
    }

    /// Clear every flag — an event fires *at* an evaluation, not continuously.
    pub fn clear(&mut self) {
        self.flags.fill(false);
    }

    /// Whether anything is marked.
    pub fn any(&self) -> bool {
        self.flags.iter().any(|&f| f)
    }

    /// Total slots across all instances.
    pub fn len(&self) -> usize {
        self.flags.len()
    }

    /// Whether no instance declared any event.
    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
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

    /// A timer registration keeps its slot, rather than degrading to an anonymous breakpoint —
    /// which is the whole difference between "stop here" and "this event fires here".
    #[test]
    fn a_timer_registration_carries_its_slot() {
        let mut sink = RecordingEventSink::new();
        sink.timer(2, 3e-6);
        assert_eq!(sink.timers, vec![(2, 3e-6)]);
        assert!(
            sink.breakpoints.is_empty(),
            "the recorder attributes it rather than falling back to the anonymous form"
        );
    }

    /// The default implementation forwards to `breakpoint`, so a consumer that only places
    /// timepoints still lands on the right one.
    #[test]
    fn the_default_timer_falls_back_to_a_breakpoint() {
        struct OnlyBreakpoints(Vec<f64>);
        impl EventSink for OnlyBreakpoints {
            fn monitor(&mut self, _s: usize, _v: f64, _spec: CrossSpec) {}
            fn breakpoint(&mut self, t: f64) {
                self.0.push(t);
            }
        }
        let mut sink = OnlyBreakpoints(Vec::new());
        sink.timer(0, 1.5);
        assert_eq!(sink.0, vec![1.5]);
    }

    #[test]
    fn the_recorder_keeps_call_order_and_finds_a_slot() {
        let mut sink = RecordingEventSink::new();
        sink.monitor(1, 4.0, CrossSpec::cross(CrossDir::Rising));
        sink.monitor(0, -2.0, CrossSpec::cross(CrossDir::Either));
        sink.breakpoint(1e-6);
        assert_eq!(sink.monitors.len(), 2);
        assert_eq!(sink.monitors[0].0, 1, "call order is preserved");
        assert_eq!(sink.value_of(0), Some(-2.0));
        assert_eq!(sink.value_of(2), None);
        assert_eq!(sink.breakpoints, vec![1e-6]);
    }
}
