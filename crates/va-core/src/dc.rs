//! DC analysis: operating point and parameter/source sweep.

use crate::newton::NewtonConfig;
use crate::CoreError;
use va_abi::ModelInstance;

/// Result of a DC operating-point solve: the converged solution vector.
#[derive(Clone, Debug)]
pub struct OperatingPoint {
    /// Node voltages / branch currents at the operating point (global unknown order).
    pub x: Vec<f64>,
}

/// Compute the DC operating point of a circuit described by `instances`.
///
/// # Errors
///
/// Propagates [`CoreError`] from the underlying Newton solve.
pub fn operating_point(
    _instances: &[&dyn ModelInstance],
    _dim: usize,
    _cfg: NewtonConfig,
) -> Result<OperatingPoint, CoreError> {
    todo!("T3: wrap Newton into a DC operating-point solve")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T3: resistor divider operating point within §7 DC tolerance vs golden"]
    fn divider_operating_point() {}
}
