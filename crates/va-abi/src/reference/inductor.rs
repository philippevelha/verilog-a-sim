//! Linear two-terminal inductor reference model.

use crate::analysis::AnalysisCtx;
use crate::instance::{ModelInstance, UnknownKind};
use crate::stamps::StampSink;
use crate::state::ModelState;

/// A linear inductor `V(p) - V(n) = L * di/dt`, in the standard MNA branch-current
/// formulation.
///
/// Like [`super::VSource`], an inductor introduces an **extra unknown** - its branch current -
/// at global index `branch`, allocated by the caller. Its own row is not a KCL sum but the
/// element's constitutive law, written so the existing charge channel carries it:
///
/// ```text
///   -(V(p) - V(n)) + d(L*i)/dt = 0
/// ```
///
/// so the row's `residual` is `-(V(p) - V(n))` and its `charge` is the flux `L*i`. Nothing in
/// Interface beta needed to change to express this: the integrator's companion model turns
/// `charge` into `coeff*(L*i) + offset` exactly as it does a capacitor's, and the *same*
/// formulation gives the right DC answer for free - with no charge channel in a DC solve the
/// row collapses to `V(p) = V(n)`, an inductor's correct behaviour as a short circuit.
///
/// The branch current flows out of `p` and into `n`, matching [`super::VSource`]'s convention,
/// so `I(L1)` reads the same way as a source's own current.
///
/// # Limitations
///
/// Constant inductance only - no current dependence (no saturation) and no series resistance.
/// No mutual inductance: coupled inductors (SPICE's `K` card) are a different element this
/// does not model. No initial-condition support of its own; a run starts with this branch
/// current at whatever the caller's initial solution vector says, which is zero unless set.
#[derive(Clone, Debug)]
pub struct Inductor {
    terminals: [usize; 3], // [p, n, branch-current]
    l: f64,
}

impl Inductor {
    /// An inductor of `inductance` henries between global indices `p` and `n`, using global
    /// unknown `branch` for its current.
    pub fn new(p: usize, n: usize, branch: usize, inductance: f64) -> Self {
        debug_assert!(inductance > 0.0, "inductance must be positive");
        Self {
            terminals: [p, n, branch],
            l: inductance,
        }
    }
}

impl ModelInstance for Inductor {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn unknown_kind(&self, i: usize) -> UnknownKind {
        // As for `VSource`: `branch` carries this element's constitutive law, not a KCL sum,
        // so `gmin` stepping must never shunt it to ground.
        if i == 2 {
            UnknownKind::Branch
        } else {
            UnknownKind::Node
        }
    }

    fn load(
        &self,
        x: &[f64],
        _ctx: &AnalysisCtx,
        _state: &mut ModelState,
        sink: &mut dyn StampSink,
    ) {
        let [p, n, b] = self.terminals;
        let vp = x.get(p).copied().unwrap_or(0.0);
        let vn = x.get(n).copied().unwrap_or(0.0);
        let ib = x.get(b).copied().unwrap_or(0.0);

        // Constitutive row: -(V(p) - V(n)) + d(L*i)/dt = 0. The time derivative is the
        // integrator's job; this contributes the flux to the charge channel.
        sink.residual(b, -(vp - vn));
        sink.jacobian(b, p, -1.0);
        sink.jacobian(b, n, 1.0);
        sink.charge(b, self.l * ib);
        sink.dcharge(b, b, self.l);

        // The branch current flows out of p and into n.
        sink.residual(p, ib);
        sink.residual(n, -ib);
        sink.jacobian(p, b, 1.0);
        sink.jacobian(n, b, -1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamps::DenseStamp;

    /// The branch row is a constitutive law, not a KCL sum, so `gmin` must leave it alone.
    #[test]
    fn only_the_branch_current_is_a_constraint_row() {
        let l = Inductor::new(0, 1, 2, 1e-3);
        assert_eq!(l.unknown_kind(0), UnknownKind::Node);
        assert_eq!(l.unknown_kind(1), UnknownKind::Node);
        assert_eq!(l.unknown_kind(2), UnknownKind::Branch);
    }

    /// The stamp, checked entry by entry against the equations in the doc comment above:
    /// flux `L*i` on the charge channel, `-(V(p)-V(n))` in the residual, and the current
    /// injected out of `p` and into `n`.
    #[test]
    fn the_stamp_is_flux_on_the_charge_channel() {
        let henries = 2e-3;
        let l = Inductor::new(0, 1, 2, henries);
        let mut sink = DenseStamp::new(3);
        // V(p)=3, V(n)=1, i=0.5 A.
        l.load(
            &[3.0, 1.0, 0.5],
            &AnalysisCtx::dc(),
            &mut ModelState::stateless(),
            &mut sink,
        );

        assert_eq!(sink.residual[2], -(3.0 - 1.0), "constitutive row residual");
        assert_eq!(sink.charge[2], henries * 0.5, "flux on the charge channel");
        assert_eq!(sink.dcharge[2 * 3 + 2], henries, "d(flux)/di");
        assert_eq!(sink.residual[0], 0.5, "current out of p");
        assert_eq!(sink.residual[1], -0.5, "current into n");
        assert_eq!(sink.jacobian[2 * 3], -1.0);
        assert_eq!(sink.jacobian[2 * 3 + 1], 1.0);
    }

    /// A DC solve has no charge channel, so the same stamp must describe a short circuit:
    /// the branch row reduces to `V(p) = V(n)` with no dependence on the current at all.
    #[test]
    fn an_inductor_is_a_short_at_dc() {
        let l = Inductor::new(0, 1, 2, 5.0);
        let mut sink = DenseStamp::new(3);
        // Two different currents, same terminal voltages: the residual row must not move,
        // which is what makes the row a pure voltage constraint once charge is dropped.
        l.load(
            &[2.0, 2.0, 0.0],
            &AnalysisCtx::dc(),
            &mut ModelState::stateless(),
            &mut sink,
        );
        let r_zero = sink.residual[2];
        let mut sink = DenseStamp::new(3);
        l.load(
            &[2.0, 2.0, 7.0],
            &AnalysisCtx::dc(),
            &mut ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(r_zero, sink.residual[2]);
        assert_eq!(r_zero, 0.0, "equal terminal voltages satisfy the row");
    }
}
