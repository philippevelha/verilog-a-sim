//! Small-signal AC analysis: linearize about the DC point, sweep frequency.

use crate::AcNoiseError;

/// One complex value as an (real, imag) pair. Kept dependency-free; a `num-complex` type can
/// replace this if the workspace adds it.
pub type Complex = (f64, f64);

/// AC sweep specification (logarithmic decade sweep).
#[derive(Clone, Copy, Debug)]
pub struct AcSweep {
    /// Start frequency (Hz).
    pub fstart: f64,
    /// Stop frequency (Hz).
    pub fstop: f64,
    /// Points per decade.
    pub points_per_decade: usize,
}

/// The AC response: frequency points paired with the complex node-voltage vectors.
#[derive(Clone, Debug, Default)]
pub struct AcResponse {
    /// Frequency points (Hz).
    pub f: Vec<f64>,
    /// Complex solution vectors, one row per frequency.
    pub x: Vec<Vec<Complex>>,
}

/// Run an AC sweep about a precomputed DC operating point `x_dc`.
///
/// # Errors
///
/// Propagates [`AcNoiseError`] from the linear solves.
pub fn run(_x_dc: &[f64], _sweep: AcSweep) -> Result<AcResponse, AcNoiseError> {
    todo!("T5: assemble (G + jωC), solve per frequency")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T5: RC low-pass magnitude/phase within §7 AC band vs golden"]
    fn rc_lowpass_response() {}
}
