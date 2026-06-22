//! Linear two-terminal capacitor reference model.

use super::{stamp_charge, voltage_across};
use crate::instance::ModelInstance;
use crate::stamps::StampSink;

/// A linear capacitor `Q = C * (V(p) - V(n))` between two global unknowns.
///
/// The capacitor contributes only to the **charge** channel; it is an open circuit to DC
/// and is realized in transient by the integrator's companion model.
///
/// # Limitations
///
/// Constant capacitance only — no voltage dependence and no leakage conductance.
#[derive(Clone, Debug)]
pub struct Capacitor {
    terminals: [usize; 2],
    c: f64,
}

impl Capacitor {
    /// Create a capacitor of `capacitance` farads between global indices `p` and `n`.
    pub fn new(p: usize, n: usize, capacitance: f64) -> Self {
        debug_assert!(capacitance > 0.0, "capacitance must be positive");
        Self {
            terminals: [p, n],
            c: capacitance,
        }
    }
}

impl ModelInstance for Capacitor {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let [p, n] = self.terminals;
        let v = voltage_across(x, p, n);
        let q = self.c * v;
        stamp_charge(sink, p, n, q, self.c);
    }
}
