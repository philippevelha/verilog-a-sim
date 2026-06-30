//! Ideal DC voltage source reference primitive.

use crate::instance::ModelInstance;
use crate::stamps::StampSink;

/// An ideal voltage source `V(p) − V(n) = value`, in the standard MNA branch-current
/// formulation.
///
/// Unlike the two-terminal compact models in this module, a voltage source introduces an
/// **extra unknown** — its branch current — at global index `branch`. The constraint row at
/// `branch` enforces the terminal voltage, and that current is injected into the `p`/`n`
/// nodes. The caller allocates `branch` as a dedicated global unknown (one per source).
///
/// # Limitations
///
/// DC only: a constant value, no time dependence. Transient excitation (`SIN`, `PULSE`, …)
/// is the transient analysis's concern; for a DC operating point the source's DC value (a
/// `SIN`'s offset) is used.
#[derive(Clone, Debug)]
pub struct VSource {
    terminals: [usize; 3], // [p, n, branch-current]
    value: f64,
}

impl VSource {
    /// A source of `value` volts from `p` to `n`, using global unknown `branch` for its
    /// current.
    pub fn new(p: usize, n: usize, branch: usize, value: f64) -> Self {
        Self {
            terminals: [p, n, branch],
            value,
        }
    }
}

impl ModelInstance for VSource {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let [p, n, b] = self.terminals;
        let vp = x.get(p).copied().unwrap_or(0.0);
        let vn = x.get(n).copied().unwrap_or(0.0);
        let ib = x.get(b).copied().unwrap_or(0.0);

        // Constraint row: V(p) − V(n) − value = 0.
        sink.residual(b, vp - vn - self.value);
        sink.jacobian(b, p, 1.0);
        sink.jacobian(b, n, -1.0);

        // The branch current flows out of p and into n.
        sink.residual(p, ib);
        sink.residual(n, -ib);
        sink.jacobian(p, b, 1.0);
        sink.jacobian(n, b, -1.0);
    }
}
