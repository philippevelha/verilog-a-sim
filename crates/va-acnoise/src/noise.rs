//! Noise analysis: per-device noise sources propagated to an output PSD via the adjoint.

use crate::ac::AcSweep;
use crate::AcNoiseError;

/// Output noise power spectral density over frequency.
#[derive(Clone, Debug, Default)]
pub struct NoiseSpectrum {
    /// Frequency points (Hz).
    pub f: Vec<f64>,
    /// Output noise PSD at each frequency (V²/Hz or A²/Hz).
    pub psd: Vec<f64>,
    /// Integrated total noise over the swept band.
    pub total: f64,
}

/// Compute the output-referred noise spectrum about DC point `x_dc` over `sweep`, with the
/// output taken at global unknown index `output`.
///
/// # Errors
///
/// Propagates [`AcNoiseError`] from the adjoint solves.
pub fn run(_x_dc: &[f64], _sweep: AcSweep, _output: usize) -> Result<NoiseSpectrum, AcNoiseError> {
    todo!("T5: adjoint noise propagation to the output PSD")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T5: resistor thermal-noise PSD matches 4kTR vs golden"]
    fn resistor_thermal_noise() {}
}
