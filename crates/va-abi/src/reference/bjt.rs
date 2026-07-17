//! Three-terminal NPN bipolar junction transistor reference model (simplified Ebers-Moll).

use crate::instance::ModelInstance;
use crate::stamps::StampSink;

/// An NPN BJT, simplified (textbook) Ebers-Moll: `Ib = Ibe + Ibc`, `Ic = Icc - Ibc`, where
/// `Ibe = (Is/betaF)*(exp(Vbe/Vt)-1)`, `Ibc = (Is/betaR)*(exp(Vbc/Vt)-1)`, and
/// `Icc = Is*(exp(Vbe/Vt)-exp(Vbc/Vt))` — `Vbe = V(b)-V(e)`, `Vbc = V(b)-V(c)`.
///
/// # Limitations
///
/// No Early effect, no base/collector/emitter series (ohmic) resistance, no junction or
/// diffusion capacitance, no temperature dependence beyond a fixed `vt`. The reverse term
/// (`Vbc`-driven) is what keeps this model physically sane into saturation — unlike a
/// forward-active-only `Ic = betaF*Ib` model, `Ic` here self-limits as `V(c)` drops toward
/// `V(b)`, exactly the real Ebers-Moll saturation behavior, without any separate clamp.
#[derive(Clone, Debug)]
pub struct Bjt {
    terminals: [usize; 3], // [b, c, e]
    is: f64,
    beta_f: f64,
    beta_r: f64,
    vt: f64,
}

impl Bjt {
    /// Create an NPN BJT between global indices `b` (base), `c` (collector), `e` (emitter).
    ///
    /// `is` is the saturation current (A), `beta_f`/`beta_r` the forward/reverse current gains,
    /// `vt` the thermal voltage (V) — pass [`super::diode::VT_NOMINAL`] for the project's
    /// nominal simulation temperature.
    pub fn new(b: usize, c: usize, e: usize, is: f64, beta_f: f64, beta_r: f64, vt: f64) -> Self {
        debug_assert!(is > 0.0 && beta_f > 0.0 && beta_r > 0.0 && vt > 0.0);
        Self {
            terminals: [b, c, e],
            is,
            beta_f,
            beta_r,
            vt,
        }
    }

    /// Base current `Ib(Vbe, Vbc) = Ibe + Ibc`.
    pub fn ib(&self, vbe: f64, vbc: f64) -> f64 {
        let ibe = (self.is / self.beta_f) * ((vbe / self.vt).exp() - 1.0);
        let ibc = (self.is / self.beta_r) * ((vbc / self.vt).exp() - 1.0);
        ibe + ibc
    }

    /// Collector current `Ic(Vbe, Vbc) = Icc - Ibc`.
    pub fn ic(&self, vbe: f64, vbc: f64) -> f64 {
        let icc = self.is * ((vbe / self.vt).exp() - (vbc / self.vt).exp());
        let ibc = (self.is / self.beta_r) * ((vbc / self.vt).exp() - 1.0);
        icc - ibc
    }
}

impl ModelInstance for Bjt {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let [b, c, e] = self.terminals;
        let vb = x.get(b).copied().unwrap_or(0.0);
        let vc = x.get(c).copied().unwrap_or(0.0);
        let ve = x.get(e).copied().unwrap_or(0.0);
        let vbe = vb - ve;
        let vbc = vb - vc;

        let ib = self.ib(vbe, vbc);
        let ic = self.ic(vbe, vbc);
        let ie_row = -(ib + ic);

        sink.residual(b, ib);
        sink.residual(c, ic);
        sink.residual(e, ie_row);

        let gbe = (self.is / (self.beta_f * self.vt)) * (vbe / self.vt).exp();
        let gbc = (self.is / (self.beta_r * self.vt)) * (vbc / self.vt).exp();
        let gf = (self.is / self.vt) * (vbe / self.vt).exp();
        let gr = (self.is / self.vt) * (vbc / self.vt).exp();

        // dIb/d{Vbe,Vbc} and dIc/d{Vbe,Vbc}, chained through Vbe=x[b]-x[e], Vbc=x[b]-x[c] (see
        // this module's doc comment for the hand derivation; the three columns below each sum
        // to exactly zero, the structural KCL identity Ib+Ic+Ie_row=0 independent of values).
        sink.jacobian(b, b, gbe + gbc);
        sink.jacobian(b, c, -gbc);
        sink.jacobian(b, e, -gbe);

        sink.jacobian(c, b, gf - gr - gbc);
        sink.jacobian(c, c, gr + gbc);
        sink.jacobian(c, e, -gf);

        sink.jacobian(e, b, -(gbe + gf - gr));
        sink.jacobian(e, c, -gr);
        sink.jacobian(e, e, gbe + gf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::diode::VT_NOMINAL;

    fn fixture() -> Bjt {
        Bjt::new(0, 1, 2, 1e-15, 100.0, 1.0, VT_NOMINAL)
    }

    /// AD-style sanity check required by §5, applied to this hand-derived Jacobian: analytic
    /// partials vs. central difference, at a forward-active bias point.
    #[test]
    fn partials_match_finite_difference() {
        let q = fixture();
        let (vbe, vbc) = (0.65, -1.5);
        let h = 1e-6;

        let dib_dvbe = (q.ib(vbe + h, vbc) - q.ib(vbe - h, vbc)) / (2.0 * h);
        let dib_dvbc = (q.ib(vbe, vbc + h) - q.ib(vbe, vbc - h)) / (2.0 * h);
        let dic_dvbe = (q.ic(vbe + h, vbc) - q.ic(vbe - h, vbc)) / (2.0 * h);
        let dic_dvbc = (q.ic(vbe, vbc + h) - q.ic(vbe, vbc - h)) / (2.0 * h);

        let gbe = (q.is / (q.beta_f * q.vt)) * (vbe / q.vt).exp();
        let gbc = (q.is / (q.beta_r * q.vt)) * (vbc / q.vt).exp();
        let gf = (q.is / q.vt) * (vbe / q.vt).exp();
        let gr = (q.is / q.vt) * (vbc / q.vt).exp();

        let rel = |fd: f64, analytic: f64| (fd - analytic).abs() / analytic.abs().max(1e-30);
        assert!(
            rel(dib_dvbe, gbe) < 1e-5,
            "dIb/dVbe: fd={dib_dvbe} analytic={gbe}"
        );
        assert!(
            rel(dib_dvbc, gbc) < 1e-5,
            "dIb/dVbc: fd={dib_dvbc} analytic={gbc}"
        );
        assert!(
            rel(dic_dvbe, gf) < 1e-5,
            "dIc/dVbe: fd={dic_dvbe} analytic={gf}"
        );
        assert!(
            rel(dic_dvbc, -(gr + gbc)) < 1e-5,
            "dIc/dVbc: fd={dic_dvbc} analytic={}",
            -(gr + gbc)
        );
    }

    #[test]
    fn jacobian_columns_sum_to_zero() {
        // Structural KCL identity: Ib + Ic + Ie_row = 0 for any (vbe, vbc), so each Jacobian
        // column (fixed x[col], varying which row) must sum to exactly zero too. A sane
        // forward-active bias point: vb=0.65, vc=1.35 (Vbc = vb-vc = -0.7, reverse), ve=0
        // (Vbe = 0.65, forward) — not a deep-saturation point, where both junctions' exp()
        // terms would be astronomically large and floating-point cancellation alone (not a
        // derivation error) could dwarf this test's tolerance.
        use crate::stamps::DenseStamp;
        let q = fixture();
        let mut sink = DenseStamp::new(3);
        q.load(&[0.65, 1.35, 0.0], &mut sink);
        for col in 0..3 {
            let sum: f64 = (0..3).map(|row| sink.jac(row, col)).sum();
            assert!(sum.abs() < 1e-12, "column {col} sums to {sum}, expected ~0");
        }
    }

    #[test]
    fn forward_active_gives_current_gain_near_beta_f() {
        // Vbe forward-biased, Vbc reverse-biased (well into forward-active): Ic/Ib should
        // land close to beta_f (the reverse term is negligible here).
        let q = fixture();
        let (vbe, vbc) = (0.65, -3.0);
        let ib = q.ib(vbe, vbc);
        let ic = q.ic(vbe, vbc);
        assert!(ib > 0.0);
        let gain = ic / ib;
        assert!(
            (gain - q.beta_f).abs() / q.beta_f < 1e-3,
            "Ic/Ib = {gain}, expected close to beta_f = {}",
            q.beta_f
        );
    }
}
