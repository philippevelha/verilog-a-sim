//! Time integration with adaptive timestep and local-truncation-error (LTE) control.

use crate::TransientError;
use va_abi::ModelInstance;

/// Integration method for the charge channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Backward Euler (first order, robust startup).
    BackwardEuler,
    /// Trapezoidal (second order).
    Trapezoidal,
    /// Gear / BDF up to the given order.
    Gear,
}

/// Transient run controls.
#[derive(Clone, Copy, Debug)]
pub struct TranConfig {
    /// Start time (s).
    pub tstart: f64,
    /// Stop time (s).
    pub tstop: f64,
    /// Initial / maximum timestep (s).
    pub tstep: f64,
    /// Integration method.
    pub method: Method,
}

/// A sampled transient waveform: aligned time and solution-vector columns.
#[derive(Clone, Debug, Default)]
pub struct Waveform {
    /// Time points (s).
    pub t: Vec<f64>,
    /// Solution vectors, one row per time point.
    pub x: Vec<Vec<f64>>,
}

/// Integrate `instances` over `[tstart, tstop]`, returning the sampled [`Waveform`].
///
/// # Errors
///
/// [`TransientError::TimestepUnderflow`] if LTE cannot be met, or a propagated
/// [`TransientError::Core`] from a failed per-step Newton solve.
pub fn run(
    _instances: &[&dyn ModelInstance],
    _dim: usize,
    _cfg: TranConfig,
) -> Result<Waveform, TransientError> {
    todo!("T4: companion-model integration loop with LTE timestep control")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T4: RC transient RMS error within §7 tolerance vs golden"]
    fn rc_transient() {}
}
