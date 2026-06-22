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
}
