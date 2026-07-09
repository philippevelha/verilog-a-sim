//! Modified Nodal Analysis assembly.
//!
//! Builds the system Jacobian and residual by walking every [`va_abi::ModelInstance`] and
//! letting it stamp into a [`va_abi::StampSink`]. This module owns the dense system buffers
//! and the ground/reference reduction.

use va_abi::stamps::StampSink;
use va_abi::{ModelInstance, UnknownKind};

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

    /// Add a `gmin` shunt conductance from ground to every unknown `kinds` marks
    /// [`UnknownKind::Node`], skipping every [`UnknownKind::Branch`] entry.
    ///
    /// This is the one place the `Node`/`Branch` distinction actually matters: a `Node` row
    /// is a KCL current sum, so `+gmin·x[i]` on the residual and `+gmin` on the diagonal is
    /// exactly "add a conductance to ground" — a sound homotopy aid. A `Branch` row enforces
    /// some other equation entirely (e.g. `va_abi::reference::VSource`'s `V(p)-V(n)=value`);
    /// applying the same edit there would silently change what that constraint says, not aid
    /// convergence. `kinds` must have length `dim` (see [`crate::mna::classify_unknowns`]) —
    /// out-of-range panics would indicate a caller bug, not a runtime input error, so this
    /// intentionally indexes directly rather than validating (`#[debug_assert]` is not needed
    /// here: a length mismatch is a `va-core`-internal wiring bug, not malformed user input).
    pub fn shunt_gmin(&mut self, x: &[f64], gmin: f64, kinds: &[UnknownKind]) {
        if gmin <= 0.0 {
            return;
        }
        for (i, &kind) in kinds.iter().enumerate() {
            if kind == UnknownKind::Node {
                self.residual[i] += gmin * x[i];
                self.jacobian[i * self.dim + i] += gmin;
            }
        }
    }
}

/// Classify every global unknown `0..dim` as [`UnknownKind::Node`] (safe for `gmin` to shunt)
/// or [`UnknownKind::Branch`] (never shunt), by asking each instance about the unknowns it
/// declares via [`va_abi::ModelInstance::unknown_kind`].
///
/// Defaults every index to `Node`, then lets any instance that reports `Branch` for one of
/// its own indices override that default — a `Branch` report always wins, since only the
/// instance that owns a row (stamps its defining equation) can know it isn't a KCL sum, and
/// getting this wrong in the unsafe direction (shunting a constraint row) is a correctness
/// bug, while getting it wrong in the safe direction (failing to shunt an actual node) only
/// costs `gmin` stepping some of its effectiveness.
pub fn classify_unknowns(instances: &[&dyn ModelInstance], dim: usize) -> Vec<UnknownKind> {
    let mut kinds = vec![UnknownKind::Node; dim];
    for inst in instances {
        for (local, &global) in inst.unknowns().iter().enumerate() {
            if global < dim && inst.unknown_kind(local) == UnknownKind::Branch {
                kinds[global] = UnknownKind::Branch;
            }
        }
    }
    kinds
}

/// Collect each global unknown's per-unknown absolute-tolerance override (§ nature-metadata
/// wiring), by asking each instance about the unknowns it declares via
/// [`va_abi::ModelInstance::unknown_abstol`]. Every index defaults to `default` (the solver's
/// own configured tolerance); any instance reporting `Some(_)` for one of its own indices
/// overrides just that entry — structurally identical to [`classify_unknowns`], the same
/// collection shape for a different per-unknown property.
pub fn classify_abstol(instances: &[&dyn ModelInstance], dim: usize, default: f64) -> Vec<f64> {
    let mut abstol = vec![default; dim];
    for inst in instances {
        for (local, &global) in inst.unknowns().iter().enumerate() {
            if global < dim {
                if let Some(a) = inst.unknown_abstol(local) {
                    abstol[global] = a;
                }
            }
        }
    }
    abstol
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
    use va_abi::reference::{Resistor, VSource, GROUND};

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

    #[test]
    fn classify_unknowns_tags_vsource_branch_only() {
        // Divider: node0, node1, branch=2 for the source's current. Only index 2 is a
        // constraint row; 0 and 1 are ordinary KCL nodes (also touched by the resistors).
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let kinds = classify_unknowns(&insts, 3);
        assert_eq!(
            kinds,
            vec![UnknownKind::Node, UnknownKind::Node, UnknownKind::Branch]
        );
    }

    #[test]
    fn classify_abstol_lets_one_instance_override_its_own_unknown() {
        // Divider: node0, node1, branch=2. Only node1's resistor (`r2`, wrapped) reports an
        // override; every other index falls back to `default`.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let r2_override = crate::testutil::AbstolOverride {
            inner: &r2,
            // r2's own unknowns()[0] is global index 1 ("n" = node1).
            overrides: &[(0, 1e-3)],
        };
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2_override];

        let abstol = classify_abstol(&insts, 3, 1e-12);
        assert_eq!(abstol, vec![1e-12, 1e-3, 1e-12]);
    }

    #[test]
    fn shunt_gmin_skips_branch_rows() {
        let kinds = [UnknownKind::Node, UnknownKind::Branch];
        let x = [1.0, 1.0];
        let mut sys = System::new(2);

        sys.shunt_gmin(&x, 1e-3, &kinds);

        assert!(
            (sys.residual[0] - 1e-3).abs() < 1e-15,
            "node row not shunted"
        );
        assert_eq!(sys.residual[1], 0.0, "branch row must be untouched");
        assert!(
            (sys.jacobian[0] - 1e-3).abs() < 1e-15,
            "node diagonal not shunted"
        );
        assert_eq!(sys.jacobian[3], 0.0, "branch diagonal must be untouched");
    }

    #[test]
    fn shunt_gmin_is_a_noop_at_zero() {
        let kinds = [UnknownKind::Node];
        let x = [5.0];
        let mut sys = System::new(1);
        sys.shunt_gmin(&x, 0.0, &kinds);
        assert_eq!(sys.residual[0], 0.0);
        assert_eq!(sys.jacobian[0], 0.0);
    }
}
