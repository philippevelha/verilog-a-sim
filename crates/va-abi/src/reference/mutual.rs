//! Mutual inductance between two [`super::Inductor`]s (SPICE's `K` card).

use crate::analysis::AnalysisCtx;
use crate::instance::{ModelInstance, UnknownKind};
use crate::stamps::StampSink;
use crate::state::ModelState;

/// The coupling between two inductors, `M = k * sqrt(L1 * L2)`.
///
/// This element contributes **only the mutual terms**. Each inductor already puts its own flux
/// `L*i` on the charge channel of its own branch row; coupling adds `M*i2` to the first row and
/// `M*i1` to the second, which is exactly the off-diagonal of the flux matrix:
///
/// ```text
///   flux1 = L1*i1 + M*i2
///   flux2 = M*i1  + L2*i2
/// ```
///
/// So a `K` needs no unknown of its own and no new Interface beta machinery: it is two charge
/// contributions and two charge-Jacobian entries on rows that already exist. Splitting it this
/// way also means the two inductors stay ordinary inductors - remove the coupling and they
/// behave exactly as they did before, with nothing to unwind.
///
/// The caller supplies `m` directly rather than `k` and the two inductances, because the layer
/// that resolves the two inductors by name already knows their values.
///
/// # Limitations
///
/// Two inductors only - SPICE's `K` may couple more, and a three-winding transformer would
/// need either three of these or a genuine matrix element. No saturation or core loss: this is
/// linear coupling, the same scope [`super::Inductor`] itself states.
#[derive(Clone, Debug)]
pub struct Mutual {
    /// The two coupled inductors' branch-current rows.
    branches: [usize; 2],
    m: f64,
}

impl Mutual {
    /// Couple the inductors owning branch rows `b1` and `b2` with mutual inductance `m` henries.
    pub fn new(b1: usize, b2: usize, m: f64) -> Self {
        Self {
            branches: [b1, b2],
            m,
        }
    }

    /// `M = k * sqrt(l1 * l2)`, the coupling coefficient's usual definition.
    ///
    /// `k` is a dimensionless coupling in `[-1, 1]`; `|k| > 1` is unphysical (it would imply
    /// more flux linked than either winding produces) and is clamped rather than rejected,
    /// since this constructor has no error channel and the caller validates.
    pub fn from_coupling(b1: usize, b2: usize, l1: f64, l2: f64, k: f64) -> Self {
        Self::new(b1, b2, k.clamp(-1.0, 1.0) * (l1 * l2).sqrt())
    }
}

impl ModelInstance for Mutual {
    fn unknowns(&self) -> &[usize] {
        &self.branches
    }

    fn unknown_kind(&self, _i: usize) -> UnknownKind {
        // Both are inductors' constitutive rows, never KCL sums.
        UnknownKind::Branch
    }

    fn load(
        &self,
        x: &[f64],
        _ctx: &AnalysisCtx,
        _state: &mut ModelState,
        sink: &mut dyn StampSink,
    ) {
        let [b1, b2] = self.branches;
        let i1 = x.get(b1).copied().unwrap_or(0.0);
        let i2 = x.get(b2).copied().unwrap_or(0.0);

        // The off-diagonal flux terms. No residual contribution: coupling is purely reactive,
        // so at DC (no charge channel) a K vanishes and the two inductors stay independent
        // shorts - which is the physically right answer, not an omission.
        sink.charge(b1, self.m * i2);
        sink.dcharge(b1, b2, self.m);
        sink.charge(b2, self.m * i1);
        sink.dcharge(b2, b1, self.m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamps::DenseStamp;

    /// The stamp is the flux matrix's off-diagonal, and nothing else.
    #[test]
    fn coupling_contributes_only_off_diagonal_flux() {
        let m = Mutual::new(0, 1, 2e-3);
        let mut sink = DenseStamp::new(2);
        m.load(
            &[3.0, 5.0],
            &AnalysisCtx::dc(),
            &mut ModelState::stateless(),
            &mut sink,
        );

        assert_eq!(sink.charge[0], 2e-3 * 5.0, "row 1 links row 2's current");
        assert_eq!(sink.charge[1], 2e-3 * 3.0, "row 2 links row 1's current");
        assert_eq!(sink.dcharge[1], 2e-3, "d(flux1)/di2");
        assert_eq!(sink.dcharge[2], 2e-3, "d(flux2)/di1");
        // Symmetric, and no self-terms: those belong to the inductors themselves.
        assert_eq!(sink.dcharge[0], 0.0);
        assert_eq!(sink.dcharge[3], 0.0);
        // Purely reactive: nothing in the residual, so a DC solve sees no coupling at all.
        assert_eq!(sink.residual[0], 0.0);
        assert_eq!(sink.residual[1], 0.0);
    }

    /// `M = k*sqrt(L1*L2)`, with unphysical coupling clamped rather than propagated.
    #[test]
    fn mutual_inductance_follows_the_coupling_coefficient() {
        let m = Mutual::from_coupling(0, 1, 1e-3, 4e-3, 0.5);
        // sqrt(1e-3 * 4e-3) = 2e-3, halved by k.
        assert!((m.m - 1e-3).abs() < 1e-15, "M = {}", m.m);
        // Perfect coupling is the geometric mean itself.
        let perfect = Mutual::from_coupling(0, 1, 1e-3, 4e-3, 1.0);
        assert!((perfect.m - 2e-3).abs() < 1e-15);
        // Beyond perfect is clamped, not amplified.
        let over = Mutual::from_coupling(0, 1, 1e-3, 4e-3, 2.5);
        assert!((over.m - 2e-3).abs() < 1e-15, "M = {}", over.m);
    }
}
