//! Accuracy metrics (§7): the comparison functions the harness applies against golden data.

use crate::HarnessError;

/// The relative-error floor guarding a near-zero reference point in [`max_relative_error`] —
/// small enough to never affect an ordinary circuit-scale comparison (volts, milliamps), but
/// large enough that a golden point that is *exactly* (or near) zero — e.g. a diode sweep's
/// `I(V1)` at `V1=0` — doesn't turn an otherwise-negligible absolute difference into a
/// division-by-near-zero blowup.
///
/// Widened from `1e-12` to `1e-8` (2026-07-18) once `GoldenSweep`/`GoldenDc` started carrying
/// real branch currents (§ `va_harness::golden`'s branch-current convention): `circuits/
/// diode_iv.net`'s own `I(V1)` at `V1=0.1` is `~5.7e-13` A in QSPICE's golden vs. `~4.7e-13` A
/// from this project's own solve — both effectively "off" (femtoamp-scale, dominated by
/// Newton's own residual-tolerance noise floor in both simulators, not a real model
/// disagreement), but at `1e-12` the ~`1e-13`-scale absolute difference between them blew up to
/// a ~10% "error." Likewise `circuits/mos_dc.net`'s `I(VG)` (a MOSFET gate current this Level-1
/// model has no pathway for at all) is exactly `0` from this project's own solve but QSPICE's own
/// noise floor reports `~-1.5e-14`. `1e-8` floors every `diode_iv.net` point through `V1=0.3`
/// (`|I(V1)| <~ 1e-9`, all comfortably under `1e-4` relative once floored) and leaves `V1=0.4`
/// upward — where the current is large enough to matter, `>~5e-8` A — checked against its own
/// real relative precision (worst observed: `6.6e-5` at `V1=0.6`, § `docs/validation.md`), still
/// well inside `tol::DC_REL`'s `1e-4` with room to spare.
const REL_ERROR_FLOOR: f64 = 1e-8;

/// Maximum relative error between a computed and a golden series (the DC metric).
///
/// `rel = max_i |got_i - ref_i| / max(|ref_i|, floor)`, where `floor` guards near-zero
/// reference points ([`REL_ERROR_FLOOR`]).
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the series differ in length.
pub fn max_relative_error(got: &[f64], reference: &[f64]) -> Result<f64, HarnessError> {
    max_relative_error_with_floor(got, reference, REL_ERROR_FLOOR)
}

/// [`max_relative_error`] with an explicit near-zero denominator floor instead of
/// [`REL_ERROR_FLOOR`].
///
/// Exists because that constant is calibrated for **circuit-scale** quantities — volts and
/// milliamps — where `1e-8` is "indistinguishable from zero." A quantity living on a completely
/// different scale needs its own floor, and silently reusing `1e-8` for one would not merely be
/// imprecise, it would make the comparison **vacuous**: a noise PSD of `2e-18` V²/Hz divided by
/// a `1e-8` floor yields `~1e-10` no matter how wrong the value is, so every point would pass.
/// [`max_relative_psd_error`] is the caller that needs this; the floor is a parameter rather
/// than a second constant so the choice is visible at the call site.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the series differ in length.
pub fn max_relative_error_with_floor(
    got: &[f64],
    reference: &[f64],
    floor: f64,
) -> Result<f64, HarnessError> {
    if got.len() != reference.len() {
        return Err(HarnessError::LengthMismatch {
            got: got.len(),
            expected: reference.len(),
        });
    }
    Ok(got
        .iter()
        .zip(reference)
        .map(|(&g, &r)| (g - r).abs() / r.abs().max(floor))
        .fold(0.0_f64, f64::max))
}

/// How far below a noise spectrum's own peak a point may sit before it stops being compared —
/// [`max_relative_psd_error`]'s floor, expressed *relative to the band* rather than as an
/// absolute number.
///
/// A PSD has no fixed scale (thermal noise at the output of a 1 kΩ resistor is `~1.6e-17`
/// V²/Hz; through a divider it can be many orders lower), so an absolute floor would be either
/// vacuous or crushing depending on the circuit. `1e-12` of the band's own peak is a point
/// contributing a *trillionth* of the spectrum's largest value — far below anything the
/// integrated total notices, and comfortably below where two simulators' own round-off differs.
const PSD_FLOOR_RELATIVE_TO_PEAK: f64 = 1e-12;

/// Maximum relative error between a computed and a golden noise spectrum (the noise metric).
///
/// Identical in spirit to [`max_relative_error`], but with the denominator floored relative to
/// the reference spectrum's own peak ([`PSD_FLOOR_RELATIVE_TO_PEAK`]) instead of at a fixed
/// circuit-scale constant — see [`max_relative_error_with_floor`] for why reusing the latter
/// would silently make every comparison pass.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the series differ in length.
pub fn max_relative_psd_error(got: &[f64], reference: &[f64]) -> Result<f64, HarnessError> {
    let peak = reference.iter().fold(0.0_f64, |m, &r| m.max(r.abs()));
    // An all-zero reference spectrum has no scale to floor against; fall back to comparing
    // against absolute zero, where any nonzero `got` is (correctly) an infinite relative error.
    let floor = if peak > 0.0 {
        peak * PSD_FLOOR_RELATIVE_TO_PEAK
    } else {
        f64::MIN_POSITIVE
    };
    max_relative_error_with_floor(got, reference, floor)
}

/// Maximum relative magnitude error between two complex series (half of the AC metric).
///
/// `|z|` is compared exactly the way [`max_relative_error`] compares a real quantity, including
/// its [`REL_ERROR_FLOOR`] guard on a near-zero reference.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the series differ in length.
pub fn max_magnitude_error(
    got: &[(f64, f64)],
    reference: &[(f64, f64)],
) -> Result<f64, HarnessError> {
    let magnitudes = |s: &[(f64, f64)]| -> Vec<f64> {
        s.iter()
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect()
    };
    max_relative_error(&magnitudes(got), &magnitudes(reference))
}

/// Maximum absolute phase error (radians) between two complex series — the other half of the AC
/// metric.
///
/// Two details make this different from a plain `max |∠got − ∠ref|`:
///
/// - **Wrapping.** Phase is an angle: `+179°` and `−179°` differ by 2°, not 358°. The difference
///   is wrapped into `(−π, π]` before its magnitude is taken, so a reference point sitting right
///   at the ±180° branch cut (e.g. `circuits/rc_ac.net`'s own `I(V1)`, which approaches −180° at
///   high frequency) doesn't report a ~2π error for a negligible real disagreement.
/// - **A magnitude floor.** The phase of a near-zero complex value is arbitrary — its real and
///   imaginary parts are both at the two simulators' own Newton/round-off noise floor, so their
///   ratio carries no information. Points whose *reference* magnitude is under
///   [`REL_ERROR_FLOOR`] are skipped entirely rather than contributing meaningless angles. This
///   is the same judgment [`max_relative_error`]'s own floor makes, applied to the quantity
///   phase is actually sensitive to.
///
/// Returns `0.0` if every point is floored out (nothing meaningful to disagree about).
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the series differ in length.
pub fn max_phase_error(got: &[(f64, f64)], reference: &[(f64, f64)]) -> Result<f64, HarnessError> {
    if got.len() != reference.len() {
        return Err(HarnessError::LengthMismatch {
            got: got.len(),
            expected: reference.len(),
        });
    }
    let mut worst = 0.0_f64;
    for (&(gre, gim), &(rre, rim)) in got.iter().zip(reference) {
        if (rre * rre + rim * rim).sqrt() < REL_ERROR_FLOOR {
            continue;
        }
        let mut diff = gim.atan2(gre) - rim.atan2(rre);
        diff = diff.rem_euclid(2.0 * std::f64::consts::PI);
        if diff > std::f64::consts::PI {
            diff -= 2.0 * std::f64::consts::PI;
        }
        worst = worst.max(diff.abs());
    }
    Ok(worst)
}

/// Root-mean-square error between two waveforms sharing a timebase (the transient metric).
///
/// `rms = sqrt(mean_i (got_i - ref_i)^2)` — a plain absolute RMS over already-aligned samples;
/// resampling two waveforms onto a shared timebase ([`resample_linear`]) is a separate,
/// caller-side concern this function doesn't perform.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the waveforms differ in length.
pub fn rms_error(got: &[f64], reference: &[f64]) -> Result<f64, HarnessError> {
    if got.len() != reference.len() {
        return Err(HarnessError::LengthMismatch {
            got: got.len(),
            expected: reference.len(),
        });
    }
    if got.is_empty() {
        return Ok(0.0);
    }
    let sum_sq: f64 = got
        .iter()
        .zip(reference)
        .map(|(&g, &r)| (g - r).powi(2))
        .sum();
    Ok((sum_sq / got.len() as f64).sqrt())
}

/// Linearly resample `(times, values)` onto `target_times` — the "shared-timebase resample"
/// [`rms_error`]'s own doc comment defers to. Two independent transient integrators (this
/// project's own adaptive-timestep `va-transient`, QSPICE's own) essentially never land on the
/// same time points, so comparing their waveforms point-for-point would silently diff unrelated
/// samples; this reduces both to one shared timebase (`target_times`, conventionally the golden
/// reference's own) first.
///
/// Piecewise-linear interpolation between the two bracketing samples. A `target_times` point
/// outside `times`' own covered range is clamped to `values`' first/last sample — extrapolating
/// a transient waveform past what was actually simulated isn't meaningful, and the two runs'
/// `.tran` windows are expected to already overlap (both solve the same `.tran <tstep> <tstop>`
/// card).
///
/// `times` must be sorted ascending and non-empty (guaranteed by any real integrator/QSPICE
/// output; a debug assertion catches a hand-built fixture that violates it).
pub fn resample_linear(times: &[f64], values: &[f64], target_times: &[f64]) -> Vec<f64> {
    debug_assert_eq!(times.len(), values.len());
    debug_assert!(!times.is_empty(), "resample_linear: empty source series");
    debug_assert!(
        times.windows(2).all(|w| w[0] <= w[1]),
        "times must be sorted"
    );

    target_times
        .iter()
        .map(|&t| {
            if t <= times[0] {
                return values[0];
            }
            if t >= *times.last().unwrap() {
                return *values.last().unwrap();
            }
            // First index where `times[i] >= t` — `t` falls in `(times[i-1], times[i]]`.
            let i = times.partition_point(|&ti| ti < t);
            let (t0, t1) = (times[i - 1], times[i]);
            let (v0, v1) = (values[i - 1], values[i]);
            let frac = (t - t0) / (t1 - t0);
            v0 + frac * (v1 - v0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_mismatch_is_an_error() {
        assert!(max_relative_error(&[1.0, 2.0], &[1.0]).is_err());
        assert!(rms_error(&[1.0], &[1.0, 2.0]).is_err());
    }

    #[test]
    fn max_relative_error_is_zero_for_identical_series() {
        let rel = max_relative_error(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(rel, 0.0);
    }

    #[test]
    fn max_relative_error_picks_the_worst_point() {
        // |1.1-1.0|/1.0 = 0.1; |1.9-2.0|/2.0 = 0.05 — the max is the first point's.
        let rel = max_relative_error(&[1.1, 1.9], &[1.0, 2.0]).unwrap();
        assert!((rel - 0.1).abs() < 1e-12, "rel = {rel}");
    }

    #[test]
    fn max_relative_error_floor_guards_a_near_zero_reference() {
        // A near-zero reference divides by the floor, not by zero — finite, and exactly
        // `|got|/floor`, not `NaN`/`inf`.
        let rel = max_relative_error(&[1e-13], &[0.0]).unwrap();
        assert!(rel.is_finite());
        assert!((rel - 1e-13 / REL_ERROR_FLOOR).abs() < 1e-9, "rel = {rel}");

        // A genuinely large absolute difference against a zero reference is still flagged as a
        // real divergence — the floor guards the denominator from going to zero, it doesn't
        // suppress the check entirely.
        let rel = max_relative_error(&[1e-3], &[0.0]).unwrap();
        assert!(rel > crate::tol::DC_REL, "rel = {rel}");
    }

    /// The regression this metric exists to prevent: at noise-PSD scale, the circuit-scale
    /// floor makes a *completely wrong* answer look perfect.
    #[test]
    fn psd_error_is_not_vacuous_at_noise_scale() {
        // `got` is double `reference` — a 100% error that must be reported as 1.0.
        let reference = [2e-18, 2e-18];
        let got = [4e-18, 4e-18];

        let honest = max_relative_psd_error(&got, &reference).unwrap();
        assert!((honest - 1.0).abs() < 1e-12, "psd error = {honest}");

        // The same comparison through the circuit-scale floor reports ~2e-10 — inside every
        // tolerance in `tol`, i.e. a silently passing gate. This asserts the trap is real, so
        // the specialized metric can't be "simplified" back into the general one.
        let vacuous = max_relative_error(&got, &reference).unwrap();
        assert!(
            vacuous < crate::tol::DC_REL,
            "expected the circuit-scale floor to hide this error, got {vacuous}"
        );
    }

    #[test]
    fn psd_error_floors_points_far_below_the_bands_peak() {
        // The second point sits 1e-15 of the band's peak, i.e. well under the 1e-12-of-peak
        // floor. Its raw relative disagreement is a factor of 5 (400%); floored against
        // `peak * 1e-12 = 1e-30` it becomes `4e-33 / 1e-30 = 4e-3`. The floor's job is to scale
        // an unresolvable point's error down in proportion to how far below the band it is —
        // not to zero it — so the exact floored value is what's asserted.
        let reference = [1e-18, 1e-33];
        let got = [1e-18, 5e-33];
        let err = max_relative_psd_error(&got, &reference).unwrap();
        assert!((err - 4e-3).abs() < 1e-12, "err = {err}");
        // Unfloored, that same point would have read 4.0 — three orders worse.
        let unfloored = max_relative_error_with_floor(&got, &reference, 0.0).unwrap();
        assert!((unfloored - 4.0).abs() < 1e-9, "unfloored = {unfloored}");

        // Push it further under the floor and the reported error shrinks with it, reaching
        // genuinely negligible for a point a millionth of the way there.
        let err = max_relative_psd_error(&[1e-18, 5e-39], &[1e-18, 1e-39]).unwrap();
        assert!(err < 1e-8, "err = {err}");

        // A disagreement at the peak itself is always fully reported, floor or no floor.
        let err = max_relative_psd_error(&[1.5e-18, 1e-33], &reference).unwrap();
        assert!((err - 0.5).abs() < 1e-12, "err = {err}");
    }

    #[test]
    fn max_magnitude_error_compares_moduli_not_components() {
        // Same magnitude (1), opposite components: a *magnitude* comparison sees no error at all
        // — the phase metric is what catches this pair, and does (below).
        let rel = max_magnitude_error(&[(0.0, 1.0)], &[(1.0, 0.0)]).unwrap();
        assert!(rel < 1e-12, "rel = {rel}");
        // A genuine magnitude difference is still caught.
        let rel = max_magnitude_error(&[(2.0, 0.0)], &[(1.0, 0.0)]).unwrap();
        assert!((rel - 1.0).abs() < 1e-12, "rel = {rel}");
    }

    #[test]
    fn max_phase_error_is_zero_for_identical_series() {
        let s = [(1.0, 0.0), (0.0, 1.0), (-1.0, -1.0)];
        assert_eq!(max_phase_error(&s, &s).unwrap(), 0.0);
    }

    #[test]
    fn max_phase_error_wraps_across_the_branch_cut() {
        // +179° vs -179°: 2° apart, not 358°. `circuits/rc_ac.net`'s own `I(V1)` sits right here
        // at high frequency (approaching -180°), so without wrapping a negligible disagreement
        // would report a ~2π "error".
        let a = 179.0_f64.to_radians();
        let b = (-179.0_f64).to_radians();
        let got = max_phase_error(&[(a.cos(), a.sin())], &[(b.cos(), b.sin())]).unwrap();
        assert!(
            (got - 2.0_f64.to_radians()).abs() < 1e-12,
            "got {got} rad ({}°)",
            got.to_degrees()
        );
    }

    #[test]
    fn max_phase_error_skips_points_below_the_magnitude_floor() {
        // A reference at the noise floor has an arbitrary phase — comparing it would report a
        // large, meaningless angle error (here the two are a full 90° apart).
        let got = max_phase_error(&[(0.0, 1e-15)], &[(1e-15, 0.0)]).unwrap();
        assert_eq!(got, 0.0);
        // The same 90° disagreement at a real, above-floor magnitude is genuinely reported.
        let got = max_phase_error(&[(0.0, 1.0)], &[(1.0, 0.0)]).unwrap();
        assert!(
            (got - std::f64::consts::FRAC_PI_2).abs() < 1e-12,
            "got {got}"
        );
    }

    #[test]
    fn complex_metrics_reject_a_length_mismatch() {
        assert!(max_magnitude_error(&[(1.0, 0.0)], &[]).is_err());
        assert!(max_phase_error(&[(1.0, 0.0)], &[]).is_err());
    }

    #[test]
    fn rms_error_is_zero_for_identical_waveforms() {
        let rms = rms_error(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn rms_error_matches_hand_computation() {
        // sqrt(mean((0-1)^2, (0-(-1))^2)) = sqrt((1+1)/2) = 1.0.
        let rms = rms_error(&[0.0, 0.0], &[1.0, -1.0]).unwrap();
        assert!((rms - 1.0).abs() < 1e-12, "rms = {rms}");
    }

    #[test]
    fn empty_series_have_zero_error() {
        assert_eq!(max_relative_error(&[], &[]).unwrap(), 0.0);
        assert_eq!(rms_error(&[], &[]).unwrap(), 0.0);
    }

    #[test]
    fn resample_linear_interpolates_between_bracketing_samples() {
        let times = [0.0, 1.0, 2.0];
        let values = [0.0, 10.0, 10.0];
        // Halfway between t=0 (v=0) and t=1 (v=10) -> 5.0.
        let out = resample_linear(&times, &values, &[0.5]);
        assert!((out[0] - 5.0).abs() < 1e-12, "out = {out:?}");
    }

    #[test]
    fn resample_linear_matches_exactly_at_source_samples() {
        let times = [0.0, 1.0, 2.0];
        let values = [3.0, 4.0, 5.0];
        let out = resample_linear(&times, &values, &times);
        assert_eq!(out, values);
    }

    #[test]
    fn resample_linear_clamps_outside_the_source_range() {
        let times = [1.0, 2.0];
        let values = [10.0, 20.0];
        let out = resample_linear(&times, &values, &[0.0, 3.0]);
        assert_eq!(out, vec![10.0, 20.0]);
    }
}
