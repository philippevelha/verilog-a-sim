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
//! **There is no `freq` field**, and its absence is deliberate. Nothing this channel currently
//! serves is frequency-dependent — `analysis("ac")` asks which analysis is running, not at what
//! frequency — and `va_acnoise::ac::linearize` calls `load` exactly *once*, outside the
//! frequency loop, because `G` and `C` are frequency-independent by construction. A `freq` field
//! would therefore be meaningless at the one call site that would most obviously want it. A
//! field that is usually a lie is precisely how the DC-only folds happened in the first place.
//!
//! Frequency-domain constructs (`laplace_*`, `zi_*`, whose small-signal response genuinely does
//! vary with `ω`) need per-frequency re-linearization or a complex-valued channel — a larger
//! change that would bring its own honest `freq`. See `docs/proposals/analysis-context.md`.
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
        }
    }

    /// A transient context at absolute time `time` (s) and [`crate::noise::TEMP_NOMINAL`].
    pub const fn transient(time: f64) -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Transient,
            time,
            temp: crate::noise::TEMP_NOMINAL,
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
        }
    }

    /// A noise-analysis context at [`crate::noise::TEMP_NOMINAL`].
    pub const fn noise() -> Self {
        AnalysisCtx {
            kind: AnalysisKind::Noise,
            time: 0.0,
            temp: crate::noise::TEMP_NOMINAL,
        }
    }

    /// This context with its temperature replaced.
    pub const fn with_temp(self, temp: f64) -> Self {
        AnalysisCtx { temp, ..self }
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
