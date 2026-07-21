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
    if got.len() != reference.len() {
        return Err(HarnessError::LengthMismatch {
            got: got.len(),
            expected: reference.len(),
        });
    }
    Ok(got
        .iter()
        .zip(reference)
        .map(|(&g, &r)| (g - r).abs() / r.abs().max(REL_ERROR_FLOOR))
        .fold(0.0_f64, f64::max))
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
