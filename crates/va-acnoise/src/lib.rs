//! T5 — AC and noise analysis (stretch goal, §1).
//!
//! Linearizes the circuit about a DC operating point from `va-core` and solves the complex
//! system over frequency ([`ac`]), then computes output noise PSD via the adjoint method
//! ([`noise`]). Both reuse the same Jacobian the DC solve assembles.

#![forbid(unsafe_code)]

pub mod ac;
pub mod noise;

use thiserror::Error;

/// Errors raised by AC / noise analysis.
#[derive(Debug, Error)]
pub enum AcNoiseError {
    /// The DC operating point required to linearize about could not be found.
    #[error(transparent)]
    Core(#[from] va_core::CoreError),
    /// A noise analysis named an output unknown outside the system it is analyzing — the
    /// adjoint right-hand side `e_output` would be all zeros, silently reporting zero noise for
    /// every source rather than the requested output's real spectrum.
    #[error("noise output unknown {index} is outside the {dim}-unknown system")]
    InvalidOutput {
        /// The requested output's global unknown index.
        index: usize,
        /// The system dimension it had to be below.
        dim: usize,
    },
}
