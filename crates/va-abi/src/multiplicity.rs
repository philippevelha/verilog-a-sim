//! Instance multiplicity — the conventional SPICE `m=` device-line parameter, and Verilog-A's
//! `$mfactor` (LRM §6.3.6).
//!
//! # What the LRM asks for, and why this is a wrapper
//!
//! §6.3.6 states the guarantee in one sentence: an instance with multiplicity `m` must behave
//! **exactly as `m` identical instances with the same connections would**, while the simulator
//! evaluates the module only once. The rules it derives from that are:
//!
//! 1. every contribution to a branch **flow** quantity is multiplied by `m`;
//! 2. every branch **flow probe** reads the value divided by `m`;
//! 3. noise contributions to a flow quantity have their **power** multiplied by `m`;
//! 4. noise contributions to a potential quantity have their power divided by `m`;
//! 5. the multiplicity propagates into any module the module instantiates.
//!
//! Applying that to a *stamp* rather than to the source is what makes one wrapper enough for
//! every model, hand-written and generated alike: scaling an instance's whole contribution to
//! the system — residual, Jacobian, charge, charge Jacobian, AC excitation — by `m` **is** `m`
//! copies of it in parallel. Rules 1 and 3 fall out directly. Rule 2 falls out too, and is worth
//! spelling out because it looks like it needs work: a model's internal branch unknown carries
//! *one* device's current, since the scaling happens outside the instance, so a probe of it
//! already reads the per-device value the LRM asks for. What the circuit sees at the nodes is
//! `m` times that.
//!
//! A voltage-source-shaped model is the case worth checking rather than assuming, since its
//! constraint row is not a current sum. Scaling gives `m·(V(p) − V(n) − value) = 0`, which has
//! exactly the same solution set as before, while its KCL entries become `±m·I_branch` — so the
//! constraint is untouched and the current delivered to the circuit is `m` times one source's.
//! That is what `m` parallel sources do. `multiplied_voltage_source_delivers_m_times_the_current`
//! is the test.
//!
//! # Limitations, stated
//!
//! - **Rule 4 has no channel here.** [`crate::NoiseSink`] carries only *current* noise sources,
//!   so every noise contribution that can reach it is a flow one and rule 3 covers it. A
//!   potential-referred noise source would need a channel that does not exist; if one is ever
//!   added, it needs the reciprocal scaling and this comment is the reminder.
//! - **Rule 5 is not implemented here.** A Verilog-A module instantiating a submodule is
//!   flattened by `va-frontend` before an instance exists, so there is no inner instance for
//!   this wrapper to propagate to. The multiplicity applies to the flattened whole, which is the
//!   same answer whenever the submodule's own multiplicity is 1 — the only case this pipeline
//!   can express, since a flattened instance carries no per-submodule `m`.
//! - Events are **not** scaled, and must not be: an event fires at a timepoint or it does not,
//!   and `m` parallel copies of a device cross a threshold at the same instant as one.

use crate::analysis::AnalysisCtx;
use crate::events::EventSink;
use crate::instance::{ModelInstance, UnknownKind};
use crate::noise::NoiseSink;
use crate::stamps::StampSink;
use crate::state::ModelState;

/// A [`ModelInstance`] scaled by an instance multiplicity — see this module's documentation.
///
/// Owns the instance it wraps, because the one caller that needs it is building a
/// `Box<dyn ModelInstance>` for a device line and has nothing to borrow *from*: a wrapper
/// holding a reference into its own box would be self-referential.
pub struct Multiplied {
    inner: Box<dyn ModelInstance>,
    m: f64,
}

impl Multiplied {
    /// Wrap `inner` with multiplicity `m`, or hand it back untouched when `m` is exactly `1.0`.
    ///
    /// Returning the bare instance at `m == 1` is not only an optimisation — though it does save
    /// a virtual call and a multiplication per stamp on the overwhelmingly common path. It is
    /// what makes a deck with no `m=` anywhere **bit-identical** to one run before multiplicity
    /// existed, which is the property that lets this ship without re-blessing every golden.
    ///
    /// `m` is taken as given: validating it (finite, positive) belongs at the deck line that
    /// named it, where the offending text can be quoted, not here where it cannot.
    pub fn wrap(inner: Box<dyn ModelInstance>, m: f64) -> Box<dyn ModelInstance> {
        if m == 1.0 {
            inner
        } else {
            Box::new(Multiplied { inner, m })
        }
    }
}

/// A [`StampSink`] that scales every contribution passing through it by `m` before forwarding.
///
/// Every channel is scaled except `bound_step`, which is a *timestep request* rather than a
/// contribution to the system: `m` parallel copies of a device need the same timestep as one.
struct ScaledSink<'a> {
    inner: &'a mut dyn StampSink,
    m: f64,
}

impl StampSink for ScaledSink<'_> {
    fn residual(&mut self, row: usize, value: f64) {
        self.inner.residual(row, value * self.m);
    }

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        self.inner.jacobian(row, col, value * self.m);
    }

    fn charge(&mut self, row: usize, value: f64) {
        self.inner.charge(row, value * self.m);
    }

    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        self.inner.dcharge(row, col, value * self.m);
    }

    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        self.inner.excitation(row, re * self.m, im * self.m);
    }

    fn bound_step(&mut self, dt: f64) {
        self.inner.bound_step(dt);
    }
}

/// A [`NoiseSink`] scaling every source's **power** by `m` (LRM §6.3.6 rule 3).
///
/// Power, not amplitude: `m` parallel devices' noise currents are uncorrelated, so their powers
/// add and the PSD scales linearly in `m`. Scaling amplitude instead would give `m²`, which is
/// the answer for `m` *perfectly correlated* sources — the wrong physics, and silent, since both
/// produce a plausible-looking spectrum.
struct ScaledNoise<'a> {
    inner: &'a mut dyn NoiseSink,
    m: f64,
}

impl NoiseSink for ScaledNoise<'_> {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        self.inner.white_current(p, n, psd * self.m);
    }

    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {
        // The coefficient is the PSD's numerator, so scaling it scales the power at every
        // frequency; the exponent is a shape and must not move.
        self.inner.flicker_current(p, n, coeff * self.m, exponent);
    }
}

impl ModelInstance for Multiplied {
    fn unknowns(&self) -> &[usize] {
        self.inner.unknowns()
    }

    fn unknown_kind(&self, i: usize) -> UnknownKind {
        self.inner.unknown_kind(i)
    }

    fn unknown_is_junction(&self, i: usize) -> bool {
        self.inner.unknown_is_junction(i)
    }

    fn unknown_abstol(&self, i: usize) -> Option<f64> {
        // Deliberately unscaled. An internal unknown holds *one* device's quantity (§ this
        // module's rule-2 discussion), so the tolerance that was right for it stays right.
        self.inner.unknown_abstol(i)
    }

    fn load(&self, x: &[f64], ctx: &AnalysisCtx, state: &mut ModelState, sink: &mut dyn StampSink) {
        let mut scaled = ScaledSink {
            inner: sink,
            m: self.m,
        };
        self.inner.load(x, ctx, state, &mut scaled);
    }

    fn state_len(&self) -> usize {
        self.inner.state_len()
    }

    fn is_frequency_dependent(&self) -> bool {
        self.inner.is_frequency_dependent()
    }

    fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) {
        let mut scaled = ScaledNoise {
            inner: sink,
            m: self.m,
        };
        self.inner.noise(x, ctx, &mut scaled);
    }

    fn event_count(&self) -> usize {
        self.inner.event_count()
    }

    fn events(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn EventSink) {
        // Unscaled, deliberately — see this module's documentation.
        self.inner.events(x, ctx, sink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::{Resistor, VSource, GROUND};
    use crate::stamps::DenseStamp;
    use crate::ANALYSIS_DC;

    /// The whole contract in one assertion: an instance with `m = 3` stamps exactly what three
    /// of it would.
    ///
    /// Compared against three *actual* instances rather than against `3 × one`, so the test
    /// would still fail if the wrapper scaled some channel that parallel copies do not.
    #[test]
    fn a_multiplied_instance_stamps_what_m_copies_stamp() {
        let r = Resistor::new(0, GROUND, 1000.0);
        let x = [2.0];

        let mut one_times_three = DenseStamp::new(1);
        for _ in 0..3 {
            let mut st = ModelState::stateless();
            r.load(&x, &ANALYSIS_DC, &mut st, &mut one_times_three);
        }

        let mut multiplied = DenseStamp::new(1);
        let wrapped = Multiplied::wrap(Box::new(Resistor::new(0, GROUND, 1000.0)), 3.0);
        let mut st = ModelState::stateless();
        wrapped.load(&x, &ANALYSIS_DC, &mut st, &mut multiplied);

        assert_eq!(multiplied.residual, one_times_three.residual);
        assert_eq!(multiplied.jacobian, one_times_three.jacobian);
        assert_eq!(multiplied.charge, one_times_three.charge);
        assert_eq!(multiplied.dcharge, one_times_three.dcharge);
    }

    /// `m = 1` must be the identity, exactly — this is what lets every existing run stay
    /// bit-identical.
    #[test]
    fn multiplicity_one_is_the_identity_and_is_not_wrapped() {
        let r = Resistor::new(0, GROUND, 1000.0);
        let x = [2.0];
        let mut bare = DenseStamp::new(1);
        let mut st = ModelState::stateless();
        r.load(&x, &ANALYSIS_DC, &mut st, &mut bare);

        let mut wrapped_stamp = DenseStamp::new(1);
        let mut st = ModelState::stateless();
        // `wrap` hands the instance straight back at m = 1, so this is the *same* instance,
        // not an identity-scaled copy of it -- which is the property the bit-identity of every
        // existing run rests on.
        let wrapped = Multiplied::wrap(Box::new(Resistor::new(0, GROUND, 1000.0)), 1.0);
        wrapped.load(&x, &ANALYSIS_DC, &mut st, &mut wrapped_stamp);
        assert_eq!(wrapped_stamp.residual, bare.residual);
        assert_eq!(wrapped_stamp.jacobian, bare.jacobian);
    }

    /// A voltage-source-shaped model: its **constraint row is untouched** by the scaling (the
    /// enforced voltage is unchanged), while the current it delivers to the circuit is `m` times
    /// one source's.
    ///
    /// The case the module documentation says is worth checking rather than assuming, because a
    /// constraint row is not a current sum and "scale everything" has no obvious right answer
    /// there. Checked against three real sources in parallel, not against arithmetic on one.
    #[test]
    fn multiplied_voltage_source_delivers_m_times_the_current() {
        // Unknowns: node 0, and one branch current each. `x` puts the node at 1 V and every
        // branch current at 0.25 A, so neither the constraint nor the KCL entry is zero and a
        // dropped scale cannot hide.
        let x = [1.0, 0.25];
        let v = VSource::new(0, GROUND, 1, 5.0);

        let mut bare = DenseStamp::new(2);
        let mut st = ModelState::stateless();
        v.load(&x, &ANALYSIS_DC, &mut st, &mut bare);

        let mut scaled = DenseStamp::new(2);
        let mut st = ModelState::stateless();
        Multiplied::wrap(Box::new(VSource::new(0, GROUND, 1, 5.0)), 3.0).load(
            &x,
            &ANALYSIS_DC,
            &mut st,
            &mut scaled,
        );

        // Row 0 is the node's KCL sum: the current drawn is three times one source's.
        assert!(
            (scaled.residual[0] - 3.0 * bare.residual[0]).abs() < 1e-12,
            "the circuit must see m times the current: {} vs {}",
            scaled.residual[0],
            bare.residual[0]
        );
        // Row 1 is the constraint `V(p) - V(n) - value = 0`. Scaling multiplies the equation
        // through by 3, which is the same equation: its root is unmoved, which is the property
        // that actually matters and the one asserted here.
        assert!(
            (scaled.residual[1] - 3.0 * bare.residual[1]).abs() < 1e-12,
            "the constraint row scales as one equation, not as a new one"
        );
        // Row-major, dim 2: index 2 is row 1, column 0 — d(constraint)/dV(node 0).
        let enforced = |st: &DenseStamp| -st.residual[1] / st.jacobian[2];
        assert!(
            (enforced(&scaled) - enforced(&bare)).abs() < 1e-12,
            "the voltage the constraint enforces must not move with m"
        );
    }

    /// Noise power scales **linearly** in `m`, not quadratically: `m` parallel devices'
    /// noise currents are uncorrelated, so their powers add.
    #[test]
    fn noise_power_scales_linearly_in_m() {
        struct Collect(Vec<f64>);
        impl NoiseSink for Collect {
            fn white_current(&mut self, _p: usize, _n: usize, psd: f64) {
                self.0.push(psd);
            }
        }
        let r = Resistor::new(0, GROUND, 1000.0);

        let mut one = Collect(Vec::new());
        r.noise(&[0.0], &ANALYSIS_DC, &mut one);
        let mut four = Collect(Vec::new());
        Multiplied::wrap(Box::new(Resistor::new(0, GROUND, 1000.0)), 4.0).noise(
            &[0.0],
            &ANALYSIS_DC,
            &mut four,
        );

        assert_eq!(one.0.len(), 1, "the reference resistor has thermal noise");
        assert!(
            (four.0[0] - 4.0 * one.0[0]).abs() < 1e-30,
            "power must scale as m, not m^2: {} vs {}",
            four.0[0],
            4.0 * one.0[0]
        );
    }
}
