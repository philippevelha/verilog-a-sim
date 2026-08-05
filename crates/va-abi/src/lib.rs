//! Interface β — the model-instance ABI, the project's internal "OSDI".
//!
//! This crate is a **frozen shared contract** (§4, §6) and a leaf crate with no internal
//! dependencies. `va-core` calls [`ModelInstance::load`]; both `va-codegen`'s generated
//! models and this crate's [`reference`] models implement it. Because the reference models
//! are real and working, `va-core` has something to solve on commit #1 — the core team is
//! never blocked on the compiler team.
//!
//! # The three channels
//!
//! A model contributes to two channels via the [`StampSink`] in [`ModelInstance::load`]:
//! - **resistive**: [`residual`](StampSink::residual) + [`jacobian`](StampSink::jacobian),
//!   used by DC and as the conductive part of transient.
//! - **charge**: [`charge`](StampSink::charge) + [`dcharge`](StampSink::dcharge), consumed
//!   by the transient integrator via a companion model. DC ignores this channel.
//!
//! and to a third via the [`NoiseSink`] in [`ModelInstance::noise`] (§6 change, 2026-08-01):
//! - **noise**: [`white_current`](NoiseSink::white_current) (frequency-flat) and
//!   [`flicker_current`](NoiseSink::flicker_current) (`coeff / f^exponent`), consumed by
//!   `va-acnoise`'s adjoint noise analysis. Every other analysis ignores it. It is a separate
//!   channel rather than something derived from the Jacobian because a device's noise is physics
//!   the matrices no longer carry — see [`noise`]'s own module doc for why a resistor and a diode
//!   with equal small-signal conductance are not equally noisy.

#![forbid(unsafe_code)]

pub mod instance;
pub mod noise;
pub mod reference;
pub mod stamps;

pub use instance::{ModelInstance, UnknownKind};
pub use noise::NoiseSink;
pub use stamps::StampSink;

#[cfg(test)]
mod tests {
    use crate::noise::{CollectedNoise, TEMP_NOMINAL};
    use crate::reference::{diode::VT_NOMINAL, Bjt, Capacitor, Diode, Resistor, VSource, GROUND};
    use crate::stamps::DenseStamp;
    use crate::ModelInstance;

    /// Hand-checked resistor thermal noise: `4kT/R` for a 1 kΩ resistor at 300.15 K is
    /// `1.6576e-23` A²/Hz — the same value QSPICE's own `onoise_r1` column implies for
    /// `circuits/diode_noise.net` (§ `docs/validation.md`'s noise-gate section).
    #[test]
    fn resistor_noise_is_thermal_and_bias_independent() {
        let r = Resistor::new(0, GROUND, 1000.0);
        let mut sink = CollectedNoise::default();
        r.noise(&[2.0], TEMP_NOMINAL, &mut sink);
        assert_eq!(sink.sources.len(), 1);
        let (p, n, source) = &sink.sources[0];
        assert_eq!((*p, *n), (0, GROUND));
        // White: the same at every frequency, so any probe frequency reads it.
        let psd = source.psd_at(1.0);
        assert!((psd - 1.657_6e-23).abs() < 1e-27, "psd = {psd}");

        // Biasing it differently must not change the answer — thermal noise depends on the
        // conductance and temperature only, never on the current through it.
        let mut other = CollectedNoise::default();
        r.noise(&[100.0], TEMP_NOMINAL, &mut other);
        assert_eq!(other.sources[0].2.psd_at(1.0), psd);
    }

    /// A diode's noise is shot, not thermal — the distinction [`crate::noise`]'s module doc
    /// calls out. At a bias drawing `Id`, its PSD is `2q·Id`, which is *not* `4kT·gd` for the
    /// same device's own small-signal conductance `gd = Id/Vt`: the two differ by
    /// `2q·Id / (4kT·Id/Vt) = 2q·Vt/(4kT) = 1/2`. This asserts that factor explicitly, so any
    /// future "just use 4kTg for everything" simplification fails loudly.
    #[test]
    fn diode_noise_is_shot_not_thermal() {
        let d = Diode::new(0, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let vd = 0.6;
        let mut sink = CollectedNoise::default();
        d.noise(&[vd], TEMP_NOMINAL, &mut sink);
        assert_eq!(sink.sources.len(), 1);
        let psd = sink.sources[0].2.psd_at(1.0);

        let id = d.current(vd);
        assert!(
            (psd - 2.0 * crate::noise::ELEMENTARY_CHARGE * id).abs() < 1e-30,
            "psd = {psd}"
        );

        let thermal_equivalent = crate::noise::thermal_current_psd(d.conductance(vd), TEMP_NOMINAL);
        assert!(
            (psd / thermal_equivalent - 0.5).abs() < 1e-3,
            "shot/thermal = {} — expected ~1/2, the physics this channel exists to preserve",
            psd / thermal_equivalent
        );
    }

    /// Both BJT terminal currents get their own shot source, between the right terminal pairs.
    #[test]
    fn bjt_noise_covers_both_terminal_currents() {
        let q = Bjt::new(0, 1, GROUND, 1e-15, 100.0, 1.0, VT_NOMINAL);
        let x = [0.7, 3.0];
        let mut sink = CollectedNoise::default();
        q.noise(&x, TEMP_NOMINAL, &mut sink);
        assert_eq!(sink.sources.len(), 2);

        let (vbe, vbc) = (0.7, 0.7 - 3.0);
        assert_eq!(sink.sources[0].0, 0, "base source is across b-e");
        assert_eq!(sink.sources[0].1, GROUND);
        assert!(
            (sink.sources[0].2.psd_at(1.0)
                - 2.0 * crate::noise::ELEMENTARY_CHARGE * q.ib(vbe, vbc))
            .abs()
                < 1e-30
        );
        assert_eq!(sink.sources[1].0, 1, "collector source is across c-e");
        assert!(
            (sink.sources[1].2.psd_at(1.0)
                - 2.0 * crate::noise::ELEMENTARY_CHARGE * q.ic(vbe, vbc))
            .abs()
                < 1e-30
        );
    }

    /// An ideal capacitor and an ideal voltage source are genuinely noiseless — they take the
    /// trait default, and that default is the physically right answer for them, not a stub.
    #[test]
    fn ideal_storage_and_sources_emit_no_noise() {
        let mut sink = CollectedNoise::default();
        Capacitor::new(0, GROUND, 1e-6).noise(&[1.0], TEMP_NOMINAL, &mut sink);
        VSource::new(0, GROUND, 1, 5.0).noise(&[1.0, 0.0], TEMP_NOMINAL, &mut sink);
        assert!(sink.sources.is_empty(), "{:?}", sink.sources);
    }

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
