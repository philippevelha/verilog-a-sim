//! Interface β's **analysis context** (§6 change, 2026-08-05): what the simulator tells a model
//! about the evaluation it is being asked for.
//!
//! # Why this exists
//!
//! [`crate::ModelInstance::load`] used to take only the solution vector `x`. A model therefore
//! could not know *what was running*, which left `va-frontend` no choice but to decide at
//! elaboration — and when it was written, DC was the only analysis, so it const-folded
//! `analysis("tran")` to `false`, `$abstime` to `0.0`, and a family of transient/AC constructs
//! to their steady-state values. Those folds were correct then and became wrong the moment
//! `va-transient` and `va-acnoise` landed. This channel is what lets the decision move back to
//! where it belongs: the evaluation itself.
//!
//! It is the same argument [`crate::UnknownKind`] and [`crate::noise`] rest on, pointed the
//! other way. Those carry information only the *instance* has, upward into the solver; this
//! carries information only the *solver* has, downward into the instance.
//!
//! # What it is not
//!
//! **[`AnalysisCtx::freq`] arrived with Tier C (2026-08-07), and not before.** Tier A
//! deliberately omitted it, on the grounds that `va_acnoise::ac::linearize` called `load`
//! exactly *once*, outside the frequency loop — so a `freq` field would have been meaningless
//! at the one call site that would most obviously want it, and a field that is usually a lie is
//! precisely how the DC-only folds happened in the first place. That refusal was conditional:
//! frequency would arrive "together with the re-linearization that makes it meaningful".
//!
//! `va-acnoise` now re-linearizes **per frequency point** whenever some instance reports
//! [`crate::ModelInstance::is_frequency_dependent`], which is what makes the field honest. A
//! circuit of ordinary devices still linearizes once and pays nothing.
//!
//! Likewise this channel carries no **state**: `transition`, `slew`, `absdelay`, `$limit` and
//! `@(initial_step)` need a model to remember something between evaluations, which needs its own
//! contract (who owns the storage, and what happens on a *rejected* timestep). Those folds stay
//! wrong after this change, and `docs/token-reference.md` says so per construct.

/// Which analysis is driving an evaluation.
///
/// Deliberately coarse: this is the granularity Verilog-A's own `analysis()` function reports
/// (LRM §4.5.1), not a description of solver internals. A Newton iteration, a rejected timestep
/// and an accepted one are all `Transient` — a model has no business distinguishing them, and
/// `load` may be called any number of times per timepoint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnalysisKind {
    /// A DC operating point or DC sweep.
    Dc,
    /// A transient timepoint. [`AnalysisCtx::time`] is meaningful.
    Transient,
    /// The linearization for a small-signal AC sweep.
    Ac,
    /// The linearization for a noise analysis.
    Noise,
}

impl AnalysisKind {
    /// Whether this analysis matches one of Verilog-A's `analysis()` phase names (LRM §4.5.1).
    ///
    /// The LRM's recognized names and how they map here:
    ///
    /// | Name(s) | True during |
    /// |---|---|
    /// | `"static"`, `"dc"` | [`Self::Dc`] — a static, time-independent solve |
    /// | `"tran"` | [`Self::Transient`] |
    /// | `"ac"` | [`Self::Ac`] |
    /// | `"noise"` | [`Self::Noise`] |
    /// | `"ic"`, `"nodeset"` | never — this simulator has no initial-condition or nodeset phase |
    ///
    /// `"ic"`/`"nodeset"` returning `false` is a statement of fact rather than an
    /// approximation: those phases do not exist here at all, so no model can be inside one.
    /// Unknown names are rejected at elaboration, not silently answered `false` — a typo'd
    /// phase name would otherwise disable a branch forever with no diagnostic.
    ///
    /// This takes a `&str` rather than a bitmask on purpose. The mask encoding is Interface α's
    /// (`va_ir::ANALYSIS_PHASES`, produced by `va-frontend` at elaboration), and this crate is a
    /// leaf that cannot see it; `va-codegen` depends on both and joins the two ends. That split
    /// is what keeps the bit order defined in exactly one place.
    pub fn matches_phase(self, phase: &str) -> bool {
        match phase {
            "static" | "dc" => self == AnalysisKind::Dc,
            "tran" => self == AnalysisKind::Transient,
            "ac" => self == AnalysisKind::Ac,
            "noise" => self == AnalysisKind::Noise,
            _ => false,
        }
    }
}

/// What the simulator knows about the evaluation a model is being asked for.
///
/// Passed by reference to [`crate::ModelInstance::load`] and
/// [`crate::ModelInstance::noise`]. Cheap to construct — a caller that evaluates in a loop
/// should build one per timepoint and reuse it across Newton iterations.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnalysisCtx {
    /// Which analysis is running.
    pub kind: AnalysisKind,
    /// Absolute simulation time in seconds — Verilog-A's `$abstime`.
    ///
    /// Meaningful only when `kind` is [`AnalysisKind::Transient`]; `0.0` in every other
    /// analysis, which is the LRM-correct answer for a static solve rather than a placeholder.
    pub time: f64,
    /// Simulation temperature in kelvin — Verilog-A's `$temperature`.
    ///
    /// Folded in here rather than kept as `noise`'s own separate argument, so both entry points
    /// agree and `load` gains the temperature it never had.
    pub temp: f64,
    /// Whether this is the **first evaluation** of the analysis — Verilog-A's
    /// `@(initial_step)`, and the flag [`crate::state`]'s channel needs to know that `prev` is
    /// zero-filled rather than meaningful.
    ///
    /// `true` for the first timepoint of a transient run, and **always `true` in DC, AC and
    /// noise**: a static solve is definitionally its own initial step, having no earlier
    /// timepoint to have followed. That is also what keeps a `slew`/`transition` settling
    /// immediately to its input in a static solve — the LRM-correct steady-state answer, and
    /// the same one this project produced when those constructs were const-folded.
    pub is_initial_step: bool,
    /// The integrator's companion coefficient for the charge channel this evaluation, and the
    /// weight it puts on each `ddt` site's *previous* rate -- together enough for a model to
    /// evaluate `ddt(q)` as a **number** consistent with the discretization actually being
    /// solved:
    ///
    /// ```text
    /// dq/dt  =  ddt_coeff * (q - q_prev)
    ///           -  ddt_prev_rate_weight * dq/dt|_prev
    ///           +  ddt_prev2_weight     * (q_prev - q_prev2)
    /// ```
    ///
    /// Backward Euler is `(1/h, 0.0, 0.0)`; trapezoidal is `(2/h, 1.0, 0.0)`; Gear/BDF2 is
    /// `((1+2r)/((1+r)h), 0.0, -r^2/((1+r)h))` with `r = h_n/h_(n-1)` (§6 change, 2026-09-01 —
    /// see [`AnalysisCtx::ddt_prev2_weight`]). All are **`0.0` in DC, AC and noise**, which is not a placeholder but the right answer: a static solve and a
    /// small-signal analysis are both taken about an operating point, where every charge rate is
    /// zero by definition.
    ///
    /// # Why this is exposed rather than inferred
    ///
    /// A model cannot derive it. `h` is not `time` minus anything the model knows, the adaptive
    /// controller changes it per step, and the LTE estimator solves **the same step twice with
    /// two different methods** -- so the coefficient a model must use is a property of the solve
    /// in progress, not of the run.
    ///
    /// # What needs it, and what does not
    ///
    /// Nothing needs it to *stamp* an ordinary `ddt(q)`: that goes through the charge channel,
    /// where the consumer supplies `d/dt` and the answer stays method-independent and exact.
    /// This is for the cases the charge channel cannot express -- a `ddt` whose value is read as
    /// a number (`I(<port>)`), or one multiplied by a bias-dependent coefficient, where the
    /// product rule needs the operating-point charge *rate*.
    pub ddt_coeff: f64,
    /// The weight on a `ddt` site's previous rate -- see [`AnalysisCtx::ddt_coeff`].
    pub ddt_prev_rate_weight: f64,
    /// The weight on the charge from **two** accepted steps back (§6 change, ratified
    /// 2026-09-01; `docs/proposals/bdf2-interface-change.md`, `docs/interfaces.md`).
    ///
    /// `0.0` for every method that does not need it, which is all of them except Gear/BDF2, and
    /// `0.0` in DC/AC/noise. A defaulted field rather than a signature change, following
    /// [`crate::ModelInstance::unknown_kind`]'s precedent: nothing outside `va-abi` constructs
    /// an `AnalysisCtx` as a bare struct literal, so every existing caller keeps compiling and
    /// keeps its previous behaviour exactly.
    ///
    /// # Why a third term was necessary
    ///
    /// Backward Euler and trapezoidal share a *shape*: a one-step recursion on the previous
    /// **rate**. Trapezoidal's closed form `Q_n - Q_(n-1) = h/2*(rate_n + rate_prev)` is exactly
    /// that recursion, which is why two fields sufficed. BDF2 is a different shape — a
    /// three-point finite difference on `Q` — and cannot be recovered from one charge plus one
    /// derived rate: the previous rate was itself built under a *different* step ratio, so no
    /// algebraic identity recovers `Q_(n-2)`'s weight from `rate_(n-1)` alone.
    pub ddt_prev2_weight: f64,
    /// Small-signal frequency in **hertz** — the `ω/2π` a frequency-dependent model needs to
    /// evaluate its own transfer function.
    ///
    /// Meaningful only when `kind` is [`AnalysisKind::Ac`]; `0.0` in every other analysis, which
    /// is the honest reading for a solve that has no frequency axis rather than a placeholder.
    /// A DC operating point is `s = 0`, so a filter evaluating at `freq = 0.0` correctly reports
    /// its DC gain.
    ///
    /// Only ever non-zero when the caller re-linearizes per point (§ this module's doc comment).
    /// A model must therefore not assume distinct frequencies between evaluations: an AC run
    /// over a circuit with no frequency-dependent instance still evaluates once, at `0.0`.
    pub freq: f64,
}

impl AnalysisCtx {
    /// A DC operating-point context at [`crate::noise::TEMP_NOMINAL`].
    ///
    /// The overwhelmingly common case in tests and in any caller that has no time axis.
    pub const fn dc() -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Dc,
            time: 0.0,
            temp: crate::noise::TEMP_NOMINAL,
            is_initial_step: true,
            freq: 0.0,
            ddt_coeff: 0.0,
            ddt_prev_rate_weight: 0.0,
            ddt_prev2_weight: 0.0,
        }
    }

    /// A transient context at absolute time `time` (s) and [`crate::noise::TEMP_NOMINAL`].
    pub const fn transient(time: f64) -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Transient,
            time,
            temp: crate::noise::TEMP_NOMINAL,
            // A caller that is genuinely at the first timepoint sets this with
            // `with_initial_step`; defaulting to `false` makes the *safe* mistake, since a
            // model then reads committed state instead of re-initialising mid-run.
            is_initial_step: false,
            freq: 0.0,
            ddt_coeff: 0.0,
            ddt_prev_rate_weight: 0.0,
            ddt_prev2_weight: 0.0,
        }
    }

    /// A small-signal AC context at [`crate::noise::TEMP_NOMINAL`].
    ///
    /// Carries no frequency — see this module's doc comment for why the field does not exist.
    pub const fn ac() -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Ac,
            time: 0.0,
            temp: crate::noise::TEMP_NOMINAL,
            is_initial_step: true,
            freq: 0.0,
            ddt_coeff: 0.0,
            ddt_prev_rate_weight: 0.0,
            ddt_prev2_weight: 0.0,
        }
    }

    /// A noise-analysis context at [`crate::noise::TEMP_NOMINAL`].
    pub const fn noise() -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Noise,
            time: 0.0,
            temp: crate::noise::TEMP_NOMINAL,
            is_initial_step: true,
            freq: 0.0,
            ddt_coeff: 0.0,
            ddt_prev_rate_weight: 0.0,
            ddt_prev2_weight: 0.0,
        }
    }

    /// This context with its temperature replaced.
    pub const fn with_temp(self, temp: f64) -> Self {
        AnalysisCtx { temp, ..self }
    }

    /// A small-signal AC context at frequency `freq` (Hz).
    ///
    /// The constructor a per-frequency re-linearization uses; [`Self::ac`] is this at `0.0`,
    /// which is what a caller with no frequency-dependent instance still passes.
    pub const fn ac_at(freq: f64) -> Self {
        AnalysisCtx { freq, ..Self::ac() }
    }

    /// This context carrying the integrator's charge-channel coefficient and previous-rate
    /// weight -- see [`AnalysisCtx::ddt_coeff`]. Only a transient driver calls this.
    pub const fn with_ddt(self, ddt_coeff: f64, ddt_prev_rate_weight: f64) -> Self {
        AnalysisCtx {
            ddt_coeff,
            ddt_prev_rate_weight,
            ..self
        }
    }

    /// This context carrying the weight on the charge two accepted steps back -- see
    /// [`AnalysisCtx::ddt_prev2_weight`]. Only a Gear/BDF2 driver calls this; every other
    /// caller leaves the field at its constructor default of `0.0`, which reduces the
    /// reconstruction to exactly the two-term form that predates this field.
    pub const fn with_ddt_prev2(self, ddt_prev2_weight: f64) -> Self {
        AnalysisCtx {
            ddt_prev2_weight,
            ..self
        }
    }

    /// This context marked as (or as not) the analysis's first evaluation.
    pub const fn with_initial_step(self, is_initial_step: bool) -> Self {
        AnalysisCtx {
            is_initial_step,
            ..self
        }
    }
}

/// A DC operating-point context at nominal temperature, as a constant.
///
/// The overwhelmingly common case: every caller with no time axis, and every test that is not
/// specifically about the context, passes `&ANALYSIS_DC`. Spelled as a `const` rather than
/// written out per call site so that "this evaluation has no analysis-dependent content" reads
/// as one deliberate thing rather than four fields chosen ad hoc.
pub const ANALYSIS_DC: AnalysisCtx = AnalysisCtx::dc();

#[cfg(test)]
mod tests {
    use super::*;

    /// Each phase name selects exactly the analysis it names, and nothing else. Written out
    /// explicitly rather than looped, because the whole value of this table is that no two rows
    /// are accidentally the same.
    #[test]
    fn phase_names_map_to_exactly_one_analysis_each() {
        assert!(AnalysisKind::Dc.matches_phase("dc"));
        assert!(AnalysisKind::Dc.matches_phase("static"));
        assert!(!AnalysisKind::Dc.matches_phase("tran"));

        assert!(AnalysisKind::Transient.matches_phase("tran"));
        assert!(!AnalysisKind::Transient.matches_phase("dc"));
        // The one that was silently wrong before this channel existed: a transient run used to
        // answer `true` to "static" and `false` to "tran", both baked in at elaboration.
        assert!(!AnalysisKind::Transient.matches_phase("static"));

        assert!(AnalysisKind::Ac.matches_phase("ac"));
        assert!(AnalysisKind::Noise.matches_phase("noise"));
        assert!(!AnalysisKind::Ac.matches_phase("noise"));
    }

    /// `"ic"`/`"nodeset"` are real LRM phase names for phases this simulator does not have.
    /// False is the honest answer in every analysis — not an approximation to revisit.
    #[test]
    fn phases_this_simulator_has_no_equivalent_of_are_never_active() {
        for kind in [
            AnalysisKind::Dc,
            AnalysisKind::Transient,
            AnalysisKind::Ac,
            AnalysisKind::Noise,
        ] {
            assert!(!kind.matches_phase("ic"));
            assert!(!kind.matches_phase("nodeset"));
        }
    }

    /// Each non-DC constructor carries its own kind, a zero `$abstime`, and the nominal
    /// temperature — the AC/noise linearizations have no time axis to report.
    #[test]
    fn the_small_signal_constructors_carry_their_own_kind() {
        assert_eq!(AnalysisCtx::ac().kind, AnalysisKind::Ac);
        assert_eq!(AnalysisCtx::noise().kind, AnalysisKind::Noise);
        assert_eq!(AnalysisCtx::ac().time, 0.0);
        assert_eq!(AnalysisCtx::noise().time, 0.0);
        assert_eq!(ANALYSIS_DC, AnalysisCtx::dc());
    }

    /// The DC constructor is what every existing caller and test collapses onto, so its
    /// defaults are load-bearing: `$abstime` must read zero and the temperature must be the
    /// project's nominal, not an arbitrary one.
    #[test]
    fn the_dc_constructor_carries_zero_time_and_the_nominal_temperature() {
        let dc = AnalysisCtx::dc();
        assert_eq!(dc.kind, AnalysisKind::Dc);
        assert_eq!(dc.time, 0.0);
        assert_eq!(dc.temp, crate::noise::TEMP_NOMINAL);
        assert_eq!(AnalysisCtx::transient(2.5e-9).time, 2.5e-9);
        assert_eq!(dc.with_temp(400.0).temp, 400.0);
        assert_eq!(dc.with_temp(400.0).kind, AnalysisKind::Dc);
    }
}
