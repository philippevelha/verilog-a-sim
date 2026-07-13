//! Accuracy metrics (§7): the comparison functions the harness applies against golden data.

use crate::HarnessError;

/// The relative-error floor guarding a near-zero reference point in [`max_relative_error`] —
/// small enough to never affect an ordinary circuit-scale comparison (volts, milliamps), but
/// large enough that a golden point that is *exactly* (or near) zero — e.g. a diode sweep's
/// `I(V1)` at `V1=0` — doesn't turn an otherwise-negligible absolute difference into a
/// division-by-near-zero blowup.
const REL_ERROR_FLOOR: f64 = 1e-12;

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
/// resampling two waveforms onto a shared timebase (§ the "shared-timebase resample" `docs/
/// validation.md` mentions) is a separate, caller-side concern this function doesn't perform.
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
}
