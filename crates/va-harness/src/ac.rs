//! Drive an `.ac` small-signal sweep ([`run_ac`]/[`compare_ac`], T5) through `va-cli` and compare
//! it against golden.
//!
//! Like [`crate::tran`] and unlike [`crate::dc`], the two runs being compared can't be assumed to
//! share an independent-variable grid — but for a very different reason, and with a very
//! different fix. A transient timebase is *adaptive*: neither integrator's sample times are
//! predictable, so [`crate::tran::compare_tran`] resamples. An AC grid is fully **deterministic**
//! on both sides — it's just that the two determinations disagree at the endpoint. QSPICE, asked
//! for `.ac dec 10 1 1meg`, emits 60 points: `10^(k/10)` for `k = 0..=58`, then jumps straight to
//! `fstop`, silently dropping what would have been `10^5.9 ≈ 794.3 kHz` (confirmed empirically
//! against a real run, not assumed). `va_acnoise::ac::AcSweep::frequencies` emits the
//! mathematically clean 61 — every `10^(k/10)` from `k = 0` through `k = 60`, both endpoints
//! included.
//!
//! Rather than teach this project's own sweep to reproduce QSPICE's off-by-one, [`compare_ac`]
//! aligns the two by **frequency**: every golden frequency is looked up in the computed sweep and
//! compared there exactly. No interpolation is involved or needed — the grids genuinely coincide
//! wherever they overlap, so this compares like with like at all 60 of golden's points and simply
//! has nothing to say about the one extra point this project computes.

use crate::golden::GoldenAc;
use crate::{metrics, tol, HarnessError, Verdict};

/// The outcome of comparing an AC sweep against golden: §7's AC metric is stated as
/// "magnitude/phase error within stated band", which is genuinely **two** numbers against two
/// tolerances, not one.
///
/// Kept as a pair of [`Verdict`]s rather than collapsed into a single combined error so a failure
/// says which half failed — a magnitude that tracks golden while the phase drifts is a very
/// different bug (a wrong reactive/charge stamp) from the reverse (a wrong conductance).
#[derive(Clone, Copy, Debug)]
pub struct AcVerdict {
    /// Max relative magnitude error, against [`tol::AC_MAG_REL`].
    pub magnitude: Verdict,
    /// Max absolute phase error in radians, against [`tol::AC_PHASE_RAD`].
    pub phase: Verdict,
}

impl AcVerdict {
    /// Whether *both* halves are within their band.
    pub fn passed(&self) -> bool {
        self.magnitude.passed && self.phase.passed
    }
}

/// Solve `circuit`'s `.ac` sweep (optionally through a compiled Verilog-A `model`) and package it
/// as a [`GoldenAc`].
///
/// # Errors
///
/// [`HarnessError::Run`] if the netlist/model can't be read or parsed, the deck has no `.ac`
/// card or no AC-excited source, the DC operating point it linearizes about diverges, or the
/// complex solve is singular at some frequency.
pub fn run_ac(circuit: &str, model: Option<&str>) -> Result<GoldenAc, HarnessError> {
    let (net, compiled) =
        va_cli::load(circuit, model).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let response =
        va_cli::solve_ac(&net, &compiled).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let branch_currents = va_cli::branch_currents(&net, &compiled)
        .map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    Ok(GoldenAc::from_response(
        &net.node_order,
        &response.f,
        &response.x,
        &branch_currents,
    ))
}

/// The relative tolerance two frequency points must agree within to be considered the same point
/// of the sweep.
///
/// Both sides generate their grid by repeated multiplication in `f64`, so the same nominal
/// frequency can differ in its last bits between them; `1e-9` is far tighter than the spacing
/// between adjacent points at any realistic points-per-decade (10 points/decade are ~26% apart,
/// 1000 would still be ~0.23% apart), so this can never match the *wrong* point.
const FREQ_MATCH_REL: f64 = 1e-9;

/// Compare a freshly-computed AC sweep against its golden reference (§7's AC metric), aligning
/// the two by frequency (this module's own doc comment explains why that is necessary and why it
/// needs no interpolation).
///
/// Both halves of the metric are taken over every golden frequency and every node/branch-current
/// column, flattened into one long complex series — the same "flatten across columns and points"
/// shape [`crate::dc::compare_dc_sweep`] already uses.
///
/// # Errors
///
/// [`HarnessError::NodeOrderMismatch`] if the two don't describe the same nodes in the same
/// order; [`HarnessError::LengthMismatch`] if some golden frequency has no counterpart in the
/// computed sweep — that means the two solved genuinely different sweeps (a changed `.ac` card),
/// which is a real error rather than something to quietly compare a subset of.
pub fn compare_ac(got: &GoldenAc, golden: &GoldenAc) -> Result<AcVerdict, HarnessError> {
    if got.node_order != golden.node_order {
        return Err(HarnessError::NodeOrderMismatch {
            got: got.node_order.clone(),
            expected: golden.node_order.clone(),
        });
    }

    let mut got_flat: Vec<(f64, f64)> = Vec::new();
    let mut golden_flat: Vec<(f64, f64)> = Vec::new();
    for (freq, golden_values) in &golden.points {
        let matched = got
            .points
            .iter()
            .find(|(f, _)| (f - freq).abs() <= freq.abs().max(f64::MIN_POSITIVE) * FREQ_MATCH_REL);
        let Some((_, got_values)) = matched else {
            return Err(HarnessError::LengthMismatch {
                got: got.points.len(),
                expected: golden.points.len(),
            });
        };
        if got_values.len() != golden_values.len() {
            return Err(HarnessError::LengthMismatch {
                got: got_values.len(),
                expected: golden_values.len(),
            });
        }
        got_flat.extend(got_values);
        golden_flat.extend(golden_values);
    }

    let magnitude = metrics::max_magnitude_error(&got_flat, &golden_flat)?;
    let phase = metrics::max_phase_error(&got_flat, &golden_flat)?;
    Ok(AcVerdict {
        magnitude: Verdict::new(magnitude, tol::AC_MAG_REL),
        phase: Verdict::new(phase, tol::AC_PHASE_RAD),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_path(rel: &str) -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel
    }

    #[test]
    fn run_ac_solves_the_rc_lowpass() {
        let g = run_ac(&workspace_path("circuits/rc_ac.net"), None).expect("solve rc_ac");
        assert_eq!(g.node_order, vec!["in", "out", "I(V1)"]);
        // 1 Hz .. 1 MHz at 10 points/decade, both endpoints included.
        assert_eq!(g.points.len(), 61);

        let (f0, first) = &g.points[0];
        assert!((f0 - 1.0).abs() < 1e-9, "first frequency = {f0}");
        // The source is held at exactly its own `AC 1` excitation, and at 1 Hz (well below the
        // 159 Hz corner) the output still tracks it almost exactly.
        assert_eq!(first[0], (1.0, 0.0));
        assert!(
            (first[1].0 - 1.0).abs() < 1e-3 && first[1].1.abs() < 1e-2,
            "V(out) at 1 Hz = {:?}",
            first[1]
        );

        // At 1 MHz the RC low-pass is deep into its -20 dB/decade roll-off: |H| = 1/(wRC).
        let (f_last, last) = g.points.last().unwrap();
        assert!((f_last - 1e6).abs() < 1e-3, "last frequency = {f_last}");
        let mag = (last[1].0 * last[1].0 + last[1].1 * last[1].1).sqrt();
        let expected = 1.0 / (2.0 * std::f64::consts::PI * 1e6 * 1000.0 * 1e-6);
        assert!(
            (mag - expected).abs() < 1e-9,
            "|V(out)| at 1 MHz = {mag}, expected {expected}"
        );
    }

    #[test]
    fn compare_ac_passes_for_an_identical_reference() {
        let got = run_ac(&workspace_path("circuits/rc_ac.net"), None).expect("solve rc_ac");
        let verdict = compare_ac(&got, &got).expect("compare");
        assert!(verdict.passed());
        assert_eq!(verdict.magnitude.error, 0.0);
        assert_eq!(verdict.phase.error, 0.0);
    }

    #[test]
    fn compare_ac_tolerates_a_computed_sweep_with_extra_frequencies() {
        // The real QSPICE-vs-this-project situation (this module's own doc comment): golden's
        // grid is a subset of the computed one. Every golden frequency must still be compared,
        // and the extra computed point must not be an error.
        let got = GoldenAc {
            node_order: vec!["out".to_string()],
            points: vec![
                (1.0, vec![(1.0, 0.0)]),
                (10.0, vec![(0.5, -0.5)]),
                (100.0, vec![(0.1, -0.9)]),
            ],
        };
        let golden = GoldenAc {
            node_order: vec!["out".to_string()],
            points: vec![(1.0, vec![(1.0, 0.0)]), (100.0, vec![(0.1, -0.9)])],
        };
        let verdict = compare_ac(&got, &golden).expect("compare");
        assert!(verdict.passed());
        assert_eq!(verdict.magnitude.error, 0.0);
    }

    #[test]
    fn compare_ac_errors_when_a_golden_frequency_is_missing() {
        let got = GoldenAc {
            node_order: vec!["out".to_string()],
            points: vec![(1.0, vec![(1.0, 0.0)])],
        };
        let golden = GoldenAc {
            node_order: vec!["out".to_string()],
            points: vec![(1.0, vec![(1.0, 0.0)]), (10.0, vec![(0.5, -0.5)])],
        };
        assert!(compare_ac(&got, &golden).is_err());
    }

    #[test]
    fn compare_ac_reports_magnitude_and_phase_failures_separately() {
        let base = GoldenAc {
            node_order: vec!["out".to_string()],
            points: vec![(1.0, vec![(1.0, 0.0)])],
        };

        // Magnitude doubled, phase untouched: only the magnitude half fails.
        let mut got = base.clone();
        got.points[0].1[0] = (2.0, 0.0);
        let verdict = compare_ac(&got, &base).expect("compare");
        assert!(!verdict.magnitude.passed, "{:?}", verdict.magnitude);
        assert!(verdict.phase.passed, "{:?}", verdict.phase);
        assert!(!verdict.passed());

        // Rotated 90°, magnitude untouched: only the phase half fails.
        let mut got = base.clone();
        got.points[0].1[0] = (0.0, 1.0);
        let verdict = compare_ac(&got, &base).expect("compare");
        assert!(verdict.magnitude.passed, "{:?}", verdict.magnitude);
        assert!(!verdict.phase.passed, "{:?}", verdict.phase);
    }

    #[test]
    fn compare_ac_rejects_a_node_order_mismatch() {
        let got = GoldenAc {
            node_order: vec!["in".to_string(), "out".to_string()],
            points: vec![(1.0, vec![(1.0, 0.0), (0.5, 0.0)])],
        };
        let golden = GoldenAc {
            node_order: vec!["out".to_string(), "in".to_string()],
            points: vec![(1.0, vec![(0.5, 0.0), (1.0, 0.0)])],
        };
        assert!(compare_ac(&got, &golden).is_err());
    }

    #[test]
    fn run_ac_errors_on_a_deck_with_no_ac_card() {
        assert!(run_ac(&workspace_path("circuits/divider.net"), None).is_err());
    }
}
