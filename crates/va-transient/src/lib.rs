//! T4 — transient analysis: time integration of the charge channel.
//!
//! Builds on `va-core`'s Newton/MNA by adding a companion model for the charge channel
//! ([`integrator`]) with adaptive timestep / local-truncation-error control, plus discrete
//! [`events`]. The charge stamps come from the same [`va_abi::ModelInstance::load`] call;
//! the integrator turns `Q`, `dQ/dx` into a conductance + current companion each step.

#![forbid(unsafe_code)]

pub mod events;
pub mod integrator;

use thiserror::Error;

/// Errors raised by transient analysis.
#[derive(Debug, Error)]
pub enum TransientError {
    /// The timestep was cut below the minimum without meeting the LTE bound.
    #[error("timestep underflow at t={t:e}")]
    TimestepUnderflow { t: f64 },
    /// A per-step Newton solve failed.
    #[error(transparent)]
    Core(#[from] va_core::CoreError),
}
