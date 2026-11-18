//! T4 — transient analysis: time integration of the charge channel.
//!
//! Builds on `va-core`'s Newton/MNA by adding a companion model for the charge channel
//! ([`integrator`]) with adaptive timestep / local-truncation-error control, plus discrete
//! [`events`]. The charge stamps come from the same [`va_abi::ModelInstance::load`] call;
//! the integrator turns `Q`, `dQ/dx` into a conductance + current companion each step.

#![forbid(unsafe_code)]

pub mod events;
pub mod integrator;
pub mod noise;

use thiserror::Error;

/// Errors raised by transient analysis.
#[derive(Debug, Error)]
pub enum TransientError {
    /// The timestep was cut below the minimum without meeting the LTE bound.
    #[error("timestep underflow at t={t:e}")]
    TimestepUnderflow { t: f64 },
    /// A time step underflow whose cause is the **Newton tolerance, not the circuit** (2.58.0).
    /// Within the last `tstep` before the underflow, a step's Newton solve did not converge, yet
    /// its iterates had stopped moving: changes under 1e-6 relative and under a millionfold the
    /// absolute tolerance, with a residual of at most 1e-6. That is Newton bouncing at the limit
    /// of floating-point precision with the answer in hand. `reltol` (the run's
    /// [`integrator::TranConfig::newton_reltol`]) asks for more digits than that, and halving
    /// the step cannot help: the companion terms grow as 1/h and amplify the rounding noise.
    /// `residual` is the smallest residual infinity-norm such a solve reached.
    ///
    /// Reported rather than worked around: a looser tolerance does let the run continue, but
    /// it changes what every step is accepted at, which is the user's call to make.
    #[error(
        "timestep underflow at t={t:e}: Newton reached residual {residual:e} but not the \
         tolerance newton_reltol={reltol:e}"
    )]
    NewtonToleranceFloor { t: f64, residual: f64, reltol: f64 },
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
