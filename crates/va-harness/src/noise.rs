//! Drive a `.noise` sweep ([`run_noise`]/[`compare_noise`], T5.2) through `va-cli` and compare
//! it against golden.
//!
//! Alignment follows [`crate::ac`] exactly — matched by frequency, not by row index, because
//! QSPICE's own `dec` grid has an off-by-one at the top end that this project's does not
//! reproduce (see that module's doc comment for the measurement). The *metric* is different
//! though: a noise spectrum is one real, non-negative, wildly-scaled quantity per frequency, so
//! it is compared with [`crate::metrics::max_relative_psd_error`] rather than the circuit-scale
//! [`crate::metrics::max_relative_error`] — reusing the latter would floor every point and pass
//! unconditionally, which that function's own doc comment spells out.

use crate::golden::GoldenNoise;
use crate::{metrics, tol, HarnessError, Verdict};

/// Solve `circuit`'s `.noise` sweep (optionally through a compiled Verilog-A `model`) and package
/// it as a [`GoldenNoise`].
///
/// # Errors
///
/// [`HarnessError::Run`] if the netlist/model can't be read or parsed, the deck has no `.noise`
/// card, its output probe names an unknown net, no device contributes noise, or a solve fails.
pub fn run_noise(circuit: &str, model: Option<&str>) -> Result<GoldenNoise, HarnessError> {
    let (net, compiled) =
        va_cli::load(circuit, model).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let output = net
        .noise
        .as_ref()
        .map(|c| c.output.clone())
        .ok_or_else(|| HarnessError::Run(format!("{circuit}: no `.noise` card")))?;
    let spectrum =
        va_cli::solve_noise(&net, &compiled).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    Ok(GoldenNoise::from_spectrum(
        &output,
        &spectrum.f,
        &spectrum.psd,
    ))
}

/// The relative tolerance two frequency points must agree within to be considered the same
/// point — same value and same reasoning as [`crate::ac`]'s own.
const FREQ_MATCH_REL: f64 = 1e-9;

/// Compare a freshly-computed noise spectrum against its golden reference (§7's noise metric),
/// aligning by frequency (this module's own doc comment explains why).
///
/// # Errors
///
/// [`HarnessError::NodeOrderMismatch`] if the two probed different output nodes — comparing
/// their spectra would silently diff unrelated quantities; [`HarnessError::LengthMismatch`] if
/// some golden frequency has no counterpart in the computed sweep.
pub fn compare_noise(got: &GoldenNoise, golden: &GoldenNoise) -> Result<Verdict, HarnessError> {
    if got.output != golden.output {
        return Err(HarnessError::NodeOrderMismatch {
            got: vec![got.output.clone()],
            expected: vec![golden.output.clone()],
        });
    }

    let mut got_psd = Vec::with_capacity(golden.points.len());
    let mut golden_psd = Vec::with_capacity(golden.points.len());
    for (freq, psd) in &golden.points {
        let matched = got
            .points
            .iter()
            .find(|(f, _)| (f - freq).abs() <= freq.abs().max(f64::MIN_POSITIVE) * FREQ_MATCH_REL);
        let Some((_, got_value)) = matched else {
            return Err(HarnessError::LengthMismatch {
                got: got.points.len(),
                expected: golden.points.len(),
            });
        };
        got_psd.push(*got_value);
        golden_psd.push(*psd);
    }

    let error = metrics::max_relative_psd_error(&got_psd, &golden_psd)?;
    Ok(Verdict::new(error, tol::NOISE_PSD_REL))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_path(rel: &str) -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel
    }

    /// The real circuit, checked against the hand-derived physics rather than only against
    /// itself: `S = (4kT/R + 2q·Id)·Z²` with `Z = R ∥ rd`, flat across the band (nothing in this
    /// deck is reactive).
    #[test]
    fn run_noise_solves_the_diode_circuit() {
        let g = run_noise(&workspace_path("circuits/diode_noise.net"), None)
            .expect("solve diode_noise");
        assert_eq!(g.output, "a");
        assert_eq!(g.points.len(), 61); // 10 Hz .. 10 MHz at 10/decade, inclusive

        // Flat: every point equals the first (no reactance to shape it).
        let first = g.points[0].1;
        for (f, psd) in &g.points {
            assert!(
                (psd / first - 1.0).abs() < 1e-12,
                "f={f}: psd = {psd}, expected flat at {first}"
            );
        }
        // ~1.99e-18 V²/Hz — the value QSPICE independently reports (§ `docs/validation.md`).
        assert!(
            (first / 1.987_7e-18 - 1.0).abs() < 1e-3,
            "psd = {first}, expected ~1.9877e-18"
        );
    }

    #[test]
    fn compare_noise_passes_for_an_identical_reference() {
        let got = run_noise(&workspace_path("circuits/diode_noise.net"), None).expect("solve");
        let verdict = compare_noise(&got, &got).expect("compare");
        assert!(verdict.passed);
        assert_eq!(verdict.error, 0.0);
    }

    /// The gate must have teeth at PSD scale — a doubled spectrum is a 100% error, not something
    /// a near-zero floor swallows (§ `metrics::max_relative_psd_error`).
    #[test]
    fn compare_noise_fails_for_a_doubled_spectrum() {
        let got = run_noise(&workspace_path("circuits/diode_noise.net"), None).expect("solve");
        let mut golden = got.clone();
        for (_, psd) in &mut golden.points {
            *psd *= 2.0;
        }
        let verdict = compare_noise(&got, &golden).expect("compare");
        assert!(!verdict.passed, "error = {}", verdict.error);
        assert!(
            (verdict.error - 0.5).abs() < 1e-9,
            "error = {}",
            verdict.error
        );
    }

    #[test]
    fn compare_noise_tolerates_a_computed_sweep_with_extra_frequencies() {
        let got = GoldenNoise {
            output: "a".to_string(),
            points: vec![(10.0, 2e-18), (100.0, 2e-18), (1000.0, 2e-18)],
        };
        let golden = GoldenNoise {
            output: "a".to_string(),
            points: vec![(10.0, 2e-18), (1000.0, 2e-18)],
        };
        assert!(compare_noise(&got, &golden).expect("compare").passed);
    }

    #[test]
    fn compare_noise_errors_when_a_golden_frequency_is_missing() {
        let got = GoldenNoise {
            output: "a".to_string(),
            points: vec![(10.0, 2e-18)],
        };
        let golden = GoldenNoise {
            output: "a".to_string(),
            points: vec![(10.0, 2e-18), (100.0, 2e-18)],
        };
        assert!(compare_noise(&got, &golden).is_err());
    }

    #[test]
    fn compare_noise_rejects_a_different_output_probe() {
        let got = GoldenNoise {
            output: "a".to_string(),
            points: vec![(10.0, 2e-18)],
        };
        let golden = GoldenNoise {
            output: "b".to_string(),
            points: vec![(10.0, 2e-18)],
        };
        assert!(compare_noise(&got, &golden).is_err());
    }

    #[test]
    fn run_noise_errors_on_a_deck_with_no_noise_card() {
        assert!(run_noise(&workspace_path("circuits/divider.net"), None).is_err());
    }

    /// A **compiled** Verilog-A model now contributes real noise sources — `white_noise()` is
    /// lowered (T5.2's frontend/codegen work), so a deck whose only noisy device is a compiled
    /// one produces a genuine spectrum instead of the error this test used to assert.
    ///
    /// The value is checkable in closed form: a diode alone across an ideal source sees the
    /// source's zero small-signal impedance, so probing the diode's own node gives... nothing
    /// useful. Hence a series resistor, making the transfer impedance `R ∥ rd` and the answer
    /// the same shot-plus-thermal sum `circuits/diode_noise.net` gates — but reached entirely
    /// through the compiled model rather than `va-abi`'s hand-written one.
    #[test]
    fn a_compiled_model_now_contributes_real_noise() {
        let deck = "* compiled diode through a series resistor\nV1 in gnd DC 0.7\n\
                    R1 in a 1000\nD1 a gnd diode\n.noise V(a) V1 dec 10 10 1meg\n.end\n";
        let dir = std::env::temp_dir().join("va_harness_noise_compiled_model_test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("compiled_only.net");
        std::fs::write(&path, deck).expect("write deck");

        let g = run_noise(
            path.to_str().unwrap(),
            Some(&workspace_path("models/diode.va")),
        )
        .expect("a compiled model's white_noise() now reaches the noise channel");

        // The same ~1.99e-18 V²/Hz `circuits/diode_noise.net` measures with the *reference*
        // diode — the compiled and hand-written models agree, which is the real claim here.
        let psd = g.points[0].1;
        assert!(
            (psd / 1.987_7e-18 - 1.0).abs() < 1e-3,
            "psd = {psd}, expected ~1.9877e-18"
        );
    }

    /// The "would be identically zero" guard is still live — it just no longer fires for a
    /// compiled model that declares noise. A circuit of genuinely noiseless devices (an ideal
    /// source and an ideal capacitor dissipate nothing and pass no carriers across a barrier)
    /// must still be an error rather than a silently-zero spectrum.
    #[test]
    fn run_noise_still_rejects_a_genuinely_noiseless_circuit() {
        let deck = "* nothing here has a noise mechanism\nV1 in gnd DC 1\n\
                    C1 in gnd 1e-6\n.noise V(in) V1 dec 10 10 1meg\n.end\n";
        let dir = std::env::temp_dir().join("va_harness_noiseless_circuit_test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("noiseless.net");
        std::fs::write(&path, deck).expect("write deck");

        let err = run_noise(path.to_str().unwrap(), None)
            .expect_err("a silently-zero spectrum must be an error");
        assert!(
            format!("{err}").contains("no device in this circuit contributes any noise"),
            "unexpected error: {err}"
        );
    }
}
