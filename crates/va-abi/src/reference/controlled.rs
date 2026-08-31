//! Linear controlled-source reference models: a voltage-controlled current source (SPICE's
//! `G`) and a voltage-controlled voltage source (SPICE's `E`).
//!
//! Both are *linear* and *voltage*-controlled, which is what makes them expressible with no
//! new machinery: the controlling quantity is a node-voltage difference the solver already
//! carries as unknowns, so each stamps a constant-coefficient row and nothing else. The
//! current-controlled pair (`F`/`H`) is deliberately absent - their controlling quantity is
//! another element's branch current, which means naming that element and resolving it to its
//! branch row, a different problem from stamping.

use crate::analysis::AnalysisCtx;
use crate::instance::{ModelInstance, UnknownKind};
use crate::stamps::StampSink;
use crate::state::ModelState;

/// A voltage-controlled current source, `I(p -> n) = gm * (V(cp) - V(cn))` (SPICE's `G`).
///
/// Introduces **no extra unknown**: the output current is a function of node voltages the
/// solver already has, so this is four Jacobian entries and two residual terms - the same
/// shape a resistor stamps, but reading its voltage from a different node pair than the one
/// it drives. That is exactly what makes a `G` the cheapest way to express gain in a netlist.
///
/// # Limitations
///
/// Constant transconductance only: no frequency dependence, no saturation, no polynomial
/// (SPICE's `POLY(n)` form). A nonlinear transconductance belongs in a Verilog-A model, which
/// is the whole point of this simulator.
#[derive(Clone, Debug)]
pub struct Vccs {
    /// `[p, n, cp, cn]` - output pair then controlling pair.
    terminals: [usize; 4],
    gm: f64,
}

impl Vccs {
    /// A source driving `gm * (V(cp) - V(cn))` amps out of `p` and into `n`.
    pub fn new(p: usize, n: usize, cp: usize, cn: usize, gm: f64) -> Self {
        Self {
            terminals: [p, n, cp, cn],
            gm,
        }
    }
}

impl ModelInstance for Vccs {
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
        let [p, n, cp, cn] = self.terminals;
        let vc = x.get(cp).copied().unwrap_or(0.0) - x.get(cn).copied().unwrap_or(0.0);
        let i = self.gm * vc;

        sink.residual(p, i);
        sink.residual(n, -i);
        sink.jacobian(p, cp, self.gm);
        sink.jacobian(p, cn, -self.gm);
        sink.jacobian(n, cp, -self.gm);
        sink.jacobian(n, cn, self.gm);
    }
}

/// A voltage-controlled voltage source, `V(p) - V(n) = gain * (V(cp) - V(cn))` (SPICE's `E`).
///
/// Like an independent [`super::VSource`], this is a *constraint* rather than a current
/// contribution, so it claims its own branch-current unknown at `branch`: the row states the
/// constraint, and the current that satisfies it is injected into `p`/`n`. The only difference
/// from an independent source is that the right-hand side is another node-voltage difference
/// instead of a constant, which turns two entries of the constraint row into Jacobian terms.
///
/// # Limitations
///
/// Constant gain only - no frequency dependence and no `POLY(n)` form, for the same reason
/// [`Vccs`] states.
#[derive(Clone, Debug)]
pub struct Vcvs {
    /// `[p, n, cp, cn, branch]`.
    terminals: [usize; 5],
    gain: f64,
}

impl Vcvs {
    /// A source holding `V(p) - V(n) = gain * (V(cp) - V(cn))`, using `branch` for its current.
    pub fn new(p: usize, n: usize, cp: usize, cn: usize, branch: usize, gain: f64) -> Self {
        Self {
            terminals: [p, n, cp, cn, branch],
            gain,
        }
    }
}

impl ModelInstance for Vcvs {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn unknown_kind(&self, i: usize) -> UnknownKind {
        // As for `VSource`/`Inductor`: index 4 is this element's own constraint row, never a
        // KCL sum, so `gmin` must not shunt it.
        if i == 4 {
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
        let [p, n, cp, cn, b] = self.terminals;
        let at = |i: usize| x.get(i).copied().unwrap_or(0.0);
        let ib = at(b);

        // Constraint row: V(p) - V(n) - gain*(V(cp) - V(cn)) = 0.
        sink.residual(b, at(p) - at(n) - self.gain * (at(cp) - at(cn)));
        sink.jacobian(b, p, 1.0);
        sink.jacobian(b, n, -1.0);
        sink.jacobian(b, cp, -self.gain);
        sink.jacobian(b, cn, self.gain);

        // The branch current flows out of p and into n, matching `VSource`'s convention.
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

    /// A `G`'s stamp, entry by entry: current out of `p`, into `n`, with the four Jacobian
    /// terms that make it depend on the *controlling* pair rather than its own.
    #[test]
    fn a_vccs_reads_one_pair_and_drives_another() {
        let g = Vccs::new(0, 1, 2, 3, 0.5);
        let mut sink = DenseStamp::new(4);
        // V(cp) - V(cn) = 4 - 1 = 3, so the output current is 1.5 A.
        g.load(
            &[0.0, 0.0, 4.0, 1.0],
            &AnalysisCtx::dc(),
            &mut ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[0], 1.5, "current out of p");
        assert_eq!(sink.residual[1], -1.5, "current into n");
        assert_eq!(sink.jacobian[2], 0.5, "d(I_p)/d(V_cp)");
        assert_eq!(sink.jacobian[3], -0.5, "d(I_p)/d(V_cn)");
        // The output pair contributes nothing to its own row: a G is not a conductance.
        assert_eq!(sink.jacobian[0], 0.0);
        assert_eq!(sink.jacobian[1], 0.0);
    }

    /// An `E`'s constraint row is satisfied exactly when the output difference equals the gain
    /// times the controlling difference -- checked by evaluating the residual at a point that
    /// satisfies it, and at one that does not.
    #[test]
    fn a_vcvs_row_states_its_constraint() {
        let e = Vcvs::new(0, 1, 2, 3, 4, 10.0);
        let solved = |x: &[f64]| {
            let mut sink = DenseStamp::new(5);
            e.load(
                x,
                &AnalysisCtx::dc(),
                &mut ModelState::stateless(),
                &mut sink,
            );
            sink.residual[4]
        };
        // V(cp)-V(cn) = 0.2, gain 10 -> the output pair must differ by 2.0.
        assert_eq!(
            solved(&[2.0, 0.0, 0.2, 0.0, 0.0]),
            0.0,
            "constraint satisfied"
        );
        assert_eq!(solved(&[1.0, 0.0, 0.2, 0.0, 0.0]), -1.0, "1 V short");
        assert_eq!(e.unknown_kind(4), UnknownKind::Branch);
        assert_eq!(e.unknown_kind(0), UnknownKind::Node);
    }
}
