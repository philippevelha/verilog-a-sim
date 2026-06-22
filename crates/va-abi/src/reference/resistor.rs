//! Linear two-terminal resistor reference model.

use super::{stamp_conductance, voltage_across};
use crate::instance::ModelInstance;
use crate::stamps::StampSink;

/// A linear resistor `I = (V(p) - V(n)) / R` between two global unknowns.
///
/// # Limitations
///
/// Constant resistance only — no temperature coefficient, no parasitics, no `R = 0`
/// handling (a zero-ohm resistor is degenerate in MNA; insert a voltage source instead).
#[derive(Clone, Debug)]
pub struct Resistor {
    terminals: [usize; 2],
    g: f64,
}

impl Resistor {
    /// Create a resistor of `resistance` ohms between global indices `p` and `n`.
    ///
    /// `resistance` must be strictly positive; the conductance `1/R` is precomputed.
    pub fn new(p: usize, n: usize, resistance: f64) -> Self {
        debug_assert!(resistance > 0.0, "resistance must be positive");
        Self {
            terminals: [p, n],
            g: 1.0 / resistance,
        }
    }
}

impl ModelInstance for Resistor {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let [p, n] = self.terminals;
        let v = voltage_across(x, p, n);
        let i = self.g * v;
        stamp_conductance(sink, p, n, i, self.g);
    }
}
