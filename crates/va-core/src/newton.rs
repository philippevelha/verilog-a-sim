//! Newton–Raphson iteration driver.

use crate::CoreError;
use va_abi::ModelInstance;

/// Tunable Newton iteration controls.
#[derive(Clone, Copy, Debug)]
pub struct NewtonConfig {
    /// Maximum iterations before declaring non-convergence.
    pub max_iters: usize,
    /// Absolute residual tolerance for convergence.
    pub abstol: f64,
    /// Relative update tolerance for convergence.
    pub reltol: f64,
}

impl Default for NewtonConfig {
    fn default() -> Self {
        Self {
            max_iters: 100,
            abstol: 1e-12,
            reltol: 1e-9,
        }
    }
}

/// Solve `f(x) = 0` for the given `instances` by Newton iteration, returning the solution
/// vector of length `dim`.
///
/// # Errors
///
/// [`CoreError::NoConvergence`] if the iteration budget is exhausted, or
/// [`CoreError::Singular`] if a Jacobian factorization fails.
pub fn solve(
    _instances: &[&dyn ModelInstance],
    _dim: usize,
    _cfg: NewtonConfig,
) -> Result<Vec<f64>, CoreError> {
    todo!("T3: assemble → linsolve → update → check convergence, looped")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "T3: Newton must solve the resistor divider to the §7 DC tolerance"]
    fn solves_resistor_divider() {
        let _ = NewtonConfig::default();
    }
}
