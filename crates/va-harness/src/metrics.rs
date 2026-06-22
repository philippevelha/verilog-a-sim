//! Accuracy metrics (§7): the comparison functions the harness applies against golden data.

use crate::HarnessError;

/// Maximum relative error between a computed and a golden series (the DC metric).
///
/// `rel = max_i |got_i - ref_i| / max(|ref_i|, floor)`, where `floor` guards near-zero
/// reference points.
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
    todo!("T6: compute max relative error against the DC golden series")
}

/// Root-mean-square error between two waveforms sharing a timebase (the transient metric).
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
    todo!("T6: compute RMS error against the transient golden waveform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_mismatch_is_an_error() {
        assert!(max_relative_error(&[1.0, 2.0], &[1.0]).is_err());
        assert!(rms_error(&[1.0], &[1.0, 2.0]).is_err());
    }
}
