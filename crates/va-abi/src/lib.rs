//! Interface β — the model-instance ABI, the project's internal "OSDI".
//!
//! This crate is a **frozen shared contract** (§4, §6) and a leaf crate with no internal
//! dependencies. `va-core` calls [`ModelInstance::load`]; both `va-codegen`'s generated
//! models and this crate's [`reference`] models implement it. Because the reference models
//! are real and working, `va-core` has something to solve on commit #1 — the core team is
//! never blocked on the compiler team.
//!
//! # The two channels
//!
//! A model contributes to two channels via the [`StampSink`]:
//! - **resistive**: [`residual`](StampSink::residual) + [`jacobian`](StampSink::jacobian),
//!   used by DC and as the conductive part of transient.
//! - **charge**: [`charge`](StampSink::charge) + [`dcharge`](StampSink::dcharge), consumed
//!   by the transient integrator via a companion model. DC ignores this channel.

#![forbid(unsafe_code)]

pub mod instance;
pub mod reference;
pub mod stamps;

pub use instance::{ModelInstance, UnknownKind};
pub use stamps::StampSink;

#[cfg(test)]
mod tests {
    use crate::reference::{Resistor, GROUND};
    use crate::stamps::DenseStamp;
    use crate::ModelInstance;

    /// Hand-checked resistor stamp (§9 Step 2): a 1 kΩ resistor from node 0 to ground,
    /// biased at 2 V, must draw 2 mA into node 0 with a 1 mS self-conductance.
    #[test]
    fn resistor_stamp_by_hand() {
        let r = Resistor::new(0, GROUND, 1000.0);
        let mut sink = DenseStamp::new(1);
        r.load(&[2.0], &mut sink);

        // I = V/R = 2 / 1000 = 2 mA into node 0.
        assert!((sink.residual[0] - 2e-3).abs() < 1e-15);
        // G = 1/R = 1 mS on the diagonal.
        assert!((sink.jac(0, 0) - 1e-3).abs() < 1e-18);
        // Ground column/row folded away — nothing else stamped.
        assert_eq!(sink.charge[0], 0.0);
    }
}
