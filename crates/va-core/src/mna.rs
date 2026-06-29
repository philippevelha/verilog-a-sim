//! Modified Nodal Analysis assembly.
//!
//! Builds the system Jacobian and residual by walking every [`va_abi::ModelInstance`] and
//! letting it stamp into a [`va_abi::StampSink`]. This module owns the dense system buffers
//! and the ground/reference reduction.

use va_abi::stamps::StampSink;
use va_abi::ModelInstance;

/// The assembled dense MNA system for `dim` global unknowns.
#[derive(Clone, Debug)]
pub struct System {
    dim: usize,
    /// Residual vector `f(x)`, length `dim`.
    pub residual: Vec<f64>,
    /// Jacobian `J = df/dx`, dense row-major `dim * dim`.
    pub jacobian: Vec<f64>,
}

impl System {
    /// Allocate a zeroed system of `dim` unknowns.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            residual: vec![0.0; dim],
            jacobian: vec![0.0; dim * dim],
        }
    }

    /// Number of global unknowns.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Zero the residual and Jacobian buffers between Newton iterations.
    pub fn clear(&mut self) {
        self.residual.iter_mut().for_each(|v| *v = 0.0);
        self.jacobian.iter_mut().for_each(|v| *v = 0.0);
    }
}

impl StampSink for System {
    fn residual(&mut self, row: usize, value: f64) {
        if row < self.dim {
            self.residual[row] += value;
        }
    }

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            self.jacobian[row * self.dim + col] += value;
        }
    }

    // DC ignores the charge channel; the transient companion model (T4) consumes it.
    fn charge(&mut self, _row: usize, _value: f64) {}
    fn dcharge(&mut self, _row: usize, _col: usize, _value: f64) {}
}

/// Assemble all `instances` at solution `x` into a fresh [`System`].
///
/// Allocates a zeroed [`System`] and lets every instance stamp its residual and Jacobian.
/// The charge channel is dropped here (DC); the transient companion model (T4) consumes it.
pub fn assemble(instances: &[&dyn ModelInstance], x: &[f64], dim: usize) -> System {
    let mut sys = System::new(dim);
    for inst in instances {
        inst.load(x, &mut sys);
    }
    sys
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::reference::{Resistor, GROUND};

    #[test]
    fn sink_accumulates_resistor_stamp() {
        // A 1 kΩ resistor to ground at 1 V deposits 1 mA / 1 mS.
        let r = Resistor::new(0, GROUND, 1000.0);
        let mut sys = System::new(1);
        r.load(&[1.0], &mut sys);
        assert!((sys.residual[0] - 1e-3).abs() < 1e-15);
        assert!((sys.jacobian[0] - 1e-3).abs() < 1e-18);
    }

    #[test]
    fn assemble_sums_parallel_resistors() {
        // Two 1 kΩ resistors from node 0 to ground in parallel: G_total = 2 mS, I = 2 mA at 1 V.
        let r1 = Resistor::new(0, GROUND, 1000.0);
        let r2 = Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 2] = [&r1, &r2];
        let sys = assemble(&insts, &[1.0], 1);
        assert!((sys.residual[0] - 2e-3).abs() < 1e-15);
        assert!((sys.jacobian[0] - 2e-3).abs() < 1e-18);
    }
}
