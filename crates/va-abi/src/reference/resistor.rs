//! Linear two-terminal resistor reference model.

use super::{stamp_conductance, voltage_across};
use crate::analysis::AnalysisCtx;
use crate::instance::ModelInstance;
use crate::noise::{thermal_current_psd, NoiseSink};
use crate::stamps::StampSink;
use crate::state::ModelState;

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

    fn load(
        &self,
        x: &[f64],
        _ctx: &AnalysisCtx,
        _state: &mut ModelState,
        sink: &mut dyn StampSink,
    ) {
        let [p, n] = self.terminals;
        let v = voltage_across(x, p, n);
        let i = self.g * v;
        stamp_conductance(sink, p, n, i, self.g);
    }

    /// Johnson-Nyquist thermal noise: a `4kTG` A²/Hz white current source across the resistor,
    /// independent of the current flowing through it and of the operating point entirely (hence
    /// `x` unused). This is the one noise source in this crate that a *linear* device has.
    fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) {
        let _ = x;
        let [p, n] = self.terminals;
        sink.white_current(p, n, thermal_current_psd(self.g, ctx.temp));
    }
}
