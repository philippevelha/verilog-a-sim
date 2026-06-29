//! DC analysis: operating point and parameter/source sweep.

use crate::newton::{self, NewtonConfig};
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
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
) -> Result<OperatingPoint, CoreError> {
    let x = newton::solve(instances, dim, cfg)?;
    Ok(OperatingPoint { x })
}

/// Sweep an externally-controlled quantity, solving a DC operating point at each step.
///
/// `points` are the swept values (e.g. a source voltage or a parameter). For each, `rebuild`
/// produces the instance set for that value; the operating point is solved and collected.
/// This keeps `va-core` agnostic about *what* is being swept — the caller owns the device
/// construction and just hands back fresh instances.
///
/// # Errors
///
/// Propagates the first [`CoreError`] encountered; earlier results are discarded.
pub fn sweep<'a, F>(
    points: &[f64],
    dim: usize,
    cfg: NewtonConfig,
    mut rebuild: F,
) -> Result<Vec<OperatingPoint>, CoreError>
where
    F: FnMut(f64) -> Vec<Box<dyn ModelInstance + 'a>>,
{
    let mut out = Vec::with_capacity(points.len());
    for &value in points {
        let owned = rebuild(value);
        let refs: Vec<&dyn ModelInstance> = owned.iter().map(|b| b.as_ref()).collect();
        out.push(operating_point(&refs, dim, cfg)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::VSource;
    use va_abi::reference::{Resistor, GROUND};

    #[test]
    fn divider_operating_point() {
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let op = operating_point(&insts, 3, NewtonConfig::default()).expect("converges");
        assert!((op.x[1] - 1.0).abs() < 1e-9, "midpoint = {}", op.x[1]);
    }

    #[test]
    fn sweep_divider_input() {
        // Sweep Vin; the midpoint of an equal divider tracks Vin/2.
        let points = [0.0, 1.0, 2.0, 5.0];
        let results = sweep(&points, 3, NewtonConfig::default(), |vin| {
            vec![
                Box::new(VSource::new(0, GROUND, 2, vin)),
                Box::new(Resistor::new(0, 1, 1000.0)),
                Box::new(Resistor::new(1, GROUND, 1000.0)),
            ]
        })
        .expect("all points converge");

        for (vin, op) in points.iter().zip(&results) {
            assert!(
                (op.x[1] - vin / 2.0).abs() < 1e-9,
                "vin {vin}: mid {}",
                op.x[1]
            );
        }
    }
}
