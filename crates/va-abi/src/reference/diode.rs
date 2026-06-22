//! Two-terminal Shockley diode reference model.

use super::{stamp_conductance, voltage_across};
use crate::instance::ModelInstance;
use crate::stamps::StampSink;

/// Default thermal voltage `kT/q` at ~300 K, in volts.
pub const VT_300K: f64 = 0.025_852;

/// A Shockley diode `I = Is * (exp(Vd / (n * Vt)) - 1)`, `Vd = V(anode) - V(cathode)`.
///
/// # Limitations
///
/// Static DC model: no junction or diffusion capacitance, no series resistance, no
/// high-injection or breakdown effects. The exponential argument is **not** limited here;
/// `va-core`'s convergence aids (junction limiting) are responsible for keeping Newton out
/// of overflow. A wrong Jacobian destroys convergence, so this `g = dI/dVd` is exact.
#[derive(Clone, Debug)]
pub struct Diode {
    terminals: [usize; 2],
    is: f64,
    nvt: f64,
}

impl Diode {
    /// Create a diode between `anode` and `cathode` global indices.
    ///
    /// `is` is the saturation current (A), `n` the ideality factor, `vt` the thermal voltage
    /// (V) — pass [`VT_300K`] for room temperature.
    pub fn new(anode: usize, cathode: usize, is: f64, n: f64, vt: f64) -> Self {
        debug_assert!(is > 0.0 && n > 0.0 && vt > 0.0);
        Self {
            terminals: [anode, cathode],
            is,
            nvt: n * vt,
        }
    }

    /// Diode current at junction voltage `vd`.
    pub fn current(&self, vd: f64) -> f64 {
        self.is * ((vd / self.nvt).exp() - 1.0)
    }

    /// Small-signal conductance `dI/dVd` at junction voltage `vd`.
    pub fn conductance(&self, vd: f64) -> f64 {
        (self.is / self.nvt) * (vd / self.nvt).exp()
    }
}

impl ModelInstance for Diode {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let [p, n] = self.terminals;
        let vd = voltage_across(x, p, n);
        let i = self.current(vd);
        let g = self.conductance(vd);
        stamp_conductance(sink, p, n, i, g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AD-style sanity check required by §5: analytic conductance vs central difference.
    #[test]
    fn conductance_matches_finite_difference() {
        let d = Diode::new(0, 1, 1e-14, 1.0, VT_300K);
        let vd = 0.6;
        let h = 1e-6;
        let fd = (d.current(vd + h) - d.current(vd - h)) / (2.0 * h);
        let analytic = d.conductance(vd);
        let rel = (fd - analytic).abs() / analytic.abs();
        assert!(rel < 1e-5, "rel error {rel} (fd={fd}, analytic={analytic})");
    }
}
