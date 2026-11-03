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
    /// The stall watchdog ([`integrator::TranConfig::stall_attempts`]): `attempts` consecutive
    /// step attempts advanced time by only `advanced` seconds, less than `resolution` (the
    /// card's `tstep`, or a smaller `bound_step` a model asked for). `t` is where the run was
    /// and `h` the step it was trying.
    #[error(
        "transient stalled at t={t:e}: {attempts} step attempts advanced {advanced:e} s,          less than one step of {resolution:e} s (last step tried {h:e} s)"
    )]
    Stalled {
        t: f64,
        h: f64,
        attempts: usize,
        advanced: f64,
        resolution: f64,
    },
    /// A per-step Newton solve failed.
    #[error(transparent)]
    Core(#[from] va_core::CoreError),
    /// The requested integration method has no companion-model implementation yet.
    #[error("integration method {method:?} is not yet implemented")]
    UnsupportedMethod { method: integrator::Method },
    /// A model executed `$fatal` (LRM §9.7.3), which ends the simulation at once, whether or not
    /// the iteration it ran in would have been accepted.
    #[error("$fatal at t={t:e}: {message}")]
    Fatal { t: f64, message: String },
    /// [`integrator::run_recording`] was asked to keep an unknown the circuit does not have.
    #[error("cannot record unknown {unknown}: the circuit has {dim} unknowns")]
    RecordOutOfRange { unknown: usize, dim: usize },
}
