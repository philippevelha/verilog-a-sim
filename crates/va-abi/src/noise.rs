//! Interface β's **noise channel** (§6 change, 2026-08-01): the per-device noise sources a
//! [`crate::ModelInstance`] contributes, and the physical constants they are computed from.
//!
//! This is a third channel alongside the resistive and charge channels
//! ([`crate::StampSink`]), and it exists for the same reason those do: only the instance itself
//! knows what it is. A device's noise is **physics, not arithmetic** — it cannot be recovered
//! from the assembled matrices afterwards. A 200 Ω resistor and a diode biased to a 200 Ω
//! small-signal resistance stamp *identical* conductances, yet their noise differs by a factor
//! of `2qI/(4kTg) = V_T`-ish — thermal versus shot. Anything that tried to infer noise from a
//! `G` entry would be silently wrong for exactly the nonlinear devices that matter most.
//!
//! # What a source means
//!
//! Every source is a **current** source in parallel with the branch it names, described by its
//! one-sided power spectral density in A²/Hz. That is the standard small-signal noise model: a
//! noisy two-terminal device is its noiseless small-signal equivalent (already in `G`/`C` via
//! [`crate::ModelInstance::load`]) plus a parallel current source. Sources are assumed mutually
//! **uncorrelated**, so an analysis sums their contributions in power — true for thermal and
//! shot noise in the devices this crate models, and stated here because it is an assumption a
//! correlated-source model (e.g. a full BSIM's induced gate noise) would violate.
//!
//! # Two source kinds
//!
//! [`NoiseSink::white_current`] is frequency-flat; [`NoiseSink::flicker_current`] is
//! `coeff / f^exponent`, the `1/f`-family shape (§6 change, 2026-08-01 — added when
//! `va-codegen` learned to lower Verilog-A's `flicker_noise()`). Splitting them by *shape*
//! rather than passing a closure keeps the channel a plain data contract: a sink stores two
//! numbers and evaluates the shape itself, so nothing here has to be re-entered per frequency.
//!
//! # Limitations
//!
//! - **These two shapes only.** Verilog-A's `noise_table()` (a piecewise-linear PSD over
//!   frequency) has no representation; it stays folded to zero in the frontend. A third sink
//!   method would be the additive way to add it.
//! - **Uncorrelated sources only**, as above — there is no way to declare that two sources
//!   share a correlation coefficient.

/// Boltzmann's constant `k`, J/K (exact, SI 2019 redefinition).
pub const BOLTZMANN: f64 = 1.380_649e-23;

/// The elementary charge `q`, C (exact, SI 2019 redefinition).
pub const ELEMENTARY_CHARGE: f64 = 1.602_176_634e-19;

/// The project's nominal simulation temperature, K — 300.15 K (27 °C).
///
/// Matches `va_codegen::TEMP`, [`crate::reference::diode::VT_NOMINAL`]'s own basis, and QSPICE's
/// default `TNOM` (`CLAUDE.md` §7's oracle). Not an arbitrary round 300 K: that discrepancy was
/// a real, measured ~0.85% error against golden before it was fixed (§ `docs/roadmap.md`).
pub const TEMP_NOMINAL: f64 = 300.15;

/// Receives the noise sources one [`crate::ModelInstance`] contributes, mirroring how
/// [`crate::StampSink`] receives its residual/Jacobian contributions.
///
/// A sink is free to do anything with them — an analysis accumulates transfer-weighted power,
/// while a test can simply collect them.
pub trait NoiseSink {
    /// A **white** current-noise source of one-sided PSD `psd` (A²/Hz), in parallel with the
    /// branch from unknown `p` to unknown `n`.
    ///
    /// `p`/`n` are global unknown indices on the same convention as everything else in this ABI:
    /// an index at or past the system dimension (notably [`crate::reference::GROUND`]) is the
    /// reference node and its contribution folds away.
    fn white_current(&mut self, p: usize, n: usize, psd: f64);

    /// A **flicker** (`1/f`-family) current-noise source across the branch `p`-`n`, whose
    /// one-sided PSD at frequency `f` is `coeff / f^exponent` (A²/Hz).
    ///
    /// `exponent` is `1.0` for textbook `1/f` noise; SPICE's `AF`-style models and Verilog-A's
    /// `flicker_noise(pwr, exp)` both allow other values, so it is carried rather than assumed.
    /// `coeff` already includes any bias dependence the model applies (e.g. SPICE's `KF·I^AF`),
    /// evaluated at the operating point — the *frequency* dependence is the only thing this
    /// channel defers to the analysis.
    ///
    /// Default: **no source**, so a sink that only cares about white noise (and every existing
    /// implementor from before this method existed) needs no change.
    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {
        let _ = (p, n, coeff, exponent);
    }
}

/// Thermal (Johnson-Nyquist) current-noise PSD of a conductance `g` (S) at temperature `temp`
/// (K): `4kTg` A²/Hz.
///
/// Depends only on the conductance and the temperature — never on the current through it, which
/// is what distinguishes it from [`shot_current_psd`].
pub fn thermal_current_psd(g: f64, temp: f64) -> f64 {
    4.0 * BOLTZMANN * temp * g
}

/// Shot-noise current PSD of a DC current `i` (A) crossing a potential barrier: `2q|i|` A²/Hz.
///
/// Uses `|i|` because the PSD of a noise power is non-negative regardless of which way the
/// current flows; a reverse-biased junction passing `-Is` is as shot-noisy as a forward one
/// passing `+Is`. Independent of temperature except through `i` itself.
pub fn shot_current_psd(i: f64) -> f64 {
    2.0 * ELEMENTARY_CHARGE * i.abs()
}

/// Flicker PSD at frequency `f`: `coeff / f^exponent`, guarding `f = 0`.
///
/// A `1/f` density is unbounded at DC and a noise sweep never legitimately includes `f = 0`
/// (`va_acnoise::ac::AcSweep` rejects a non-positive `fstart`), but returning `inf`/`NaN` from a
/// stray zero would poison an entire spectrum rather than one point — so a non-positive `f`
/// yields `0.0` instead.
pub fn flicker_psd_at(coeff: f64, exponent: f64, f: f64) -> f64 {
    if f <= 0.0 {
        return 0.0;
    }
    coeff / f.powf(exponent)
}

/// One noise source, as collected from an instance — kept as its *shape* plus coefficients
/// rather than a number, so a frequency sweep can evaluate it per point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoiseSource {
    /// Frequency-flat: `psd` A²/Hz at every frequency.
    White {
        /// One-sided PSD, A²/Hz.
        psd: f64,
    },
    /// `coeff / f^exponent` A²/Hz.
    Flicker {
        /// Numerator, already including any bias dependence.
        coeff: f64,
        /// Frequency exponent (`1.0` for textbook `1/f`).
        exponent: f64,
    },
}

impl NoiseSource {
    /// This source's one-sided PSD (A²/Hz) at frequency `f` (Hz).
    pub fn psd_at(&self, f: f64) -> f64 {
        match *self {
            NoiseSource::White { psd } => psd,
            NoiseSource::Flicker { coeff, exponent } => flicker_psd_at(coeff, exponent, f),
        }
    }
}

/// A collecting [`NoiseSink`] that just records every source it is handed — the noise-channel
/// analogue of [`crate::stamps::DenseStamp`], for tests and for any caller that wants the raw
/// per-device sources rather than an accumulated result.
#[derive(Clone, Debug, Default)]
pub struct CollectedNoise {
    /// Every `(p, n, source)` reported, in the order the instances emitted them.
    pub sources: Vec<(usize, usize, NoiseSource)>,
}

impl NoiseSink for CollectedNoise {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        self.sources.push((p, n, NoiseSource::White { psd }));
    }

    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {
        self.sources
            .push((p, n, NoiseSource::Flicker { coeff, exponent }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two formulas against hand-computed values at the project's nominal temperature —
    /// these exact numbers are what `circuits/diode_noise.net`'s golden comparison rests on, so
    /// they are checked here directly rather than only end to end.
    #[test]
    fn thermal_and_shot_psd_match_hand_computation() {
        // 4kT/R for R = 1 kΩ at 300.15 K.
        let thermal = thermal_current_psd(1e-3, TEMP_NOMINAL);
        let expected = 4.0 * 1.380_649e-23 * 300.15 * 1e-3;
        assert!(
            (thermal - expected).abs() < 1e-30,
            "thermal = {thermal}, expected {expected}"
        );
        // ~1.6576e-23 A²/Hz — the value QSPICE's own `onoise_r1` column implies.
        assert!((thermal - 1.657_6e-23).abs() < 1e-27, "thermal = {thermal}");

        // 2qI for a 100 µA junction current.
        let shot = shot_current_psd(1e-4);
        assert!(
            (shot - 2.0 * 1.602_176_634e-19 * 1e-4).abs() < 1e-30,
            "shot = {shot}"
        );
    }

    #[test]
    fn shot_psd_is_sign_independent() {
        assert_eq!(shot_current_psd(-1e-4), shot_current_psd(1e-4));
    }

    #[test]
    fn thermal_psd_scales_with_temperature_and_conductance() {
        let base = thermal_current_psd(1e-3, 300.0);
        assert!((thermal_current_psd(2e-3, 300.0) - 2.0 * base).abs() < 1e-30);
        assert!((thermal_current_psd(1e-3, 600.0) - 2.0 * base).abs() < 1e-30);
    }

    /// Textbook `1/f`: one decade up is one decade down in power. These exact ratios are what
    /// `circuits/diode_flicker.net`'s golden comparison rests on — QSPICE's own `1overf` column
    /// steps `4.1365e-16 → e-17 → e-18` across decades (§ `docs/validation.md`).
    #[test]
    fn flicker_psd_falls_a_decade_per_decade() {
        let (coeff, exponent) = (1e-19, 1.0);
        let at10 = flicker_psd_at(coeff, exponent, 10.0);
        let at100 = flicker_psd_at(coeff, exponent, 100.0);
        assert!((at10 - 1e-20).abs() < 1e-32, "at10 = {at10}");
        assert!((at10 / at100 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn flicker_exponent_is_honoured_not_assumed_to_be_one() {
        // exponent 2 falls two decades per decade.
        let a = flicker_psd_at(1.0, 2.0, 10.0);
        let b = flicker_psd_at(1.0, 2.0, 100.0);
        assert!((a / b - 100.0).abs() < 1e-9, "ratio = {}", a / b);
    }

    #[test]
    fn flicker_psd_at_zero_frequency_is_zero_not_infinite() {
        // A `1/f` density is unbounded at DC; one stray zero must not poison a whole spectrum.
        assert_eq!(flicker_psd_at(1e-19, 1.0, 0.0), 0.0);
        assert_eq!(flicker_psd_at(1e-19, 1.0, -1.0), 0.0);
    }

    #[test]
    fn a_sink_that_ignores_flicker_still_compiles_and_drops_it() {
        // The default method's whole point: a pre-existing white-only sink is untouched by this
        // §6 addition, and silently receives nothing rather than failing to build.
        #[derive(Default)]
        struct WhiteOnly(Vec<f64>);
        impl NoiseSink for WhiteOnly {
            fn white_current(&mut self, _p: usize, _n: usize, psd: f64) {
                self.0.push(psd);
            }
        }
        let mut sink = WhiteOnly::default();
        sink.white_current(0, 1, 4e-23);
        sink.flicker_current(0, 1, 1e-19, 1.0);
        assert_eq!(sink.0, vec![4e-23]);
    }

    #[test]
    fn collected_noise_records_both_shapes() {
        let mut sink = CollectedNoise::default();
        sink.white_current(0, 1, 4e-23);
        sink.flicker_current(2, 3, 1e-19, 1.0);
        assert_eq!(sink.sources[0], (0, 1, NoiseSource::White { psd: 4e-23 }));
        assert_eq!(
            sink.sources[1],
            (
                2,
                3,
                NoiseSource::Flicker {
                    coeff: 1e-19,
                    exponent: 1.0
                }
            )
        );
        // And each evaluates its own shape at a frequency.
        assert_eq!(sink.sources[0].2.psd_at(1e6), 4e-23);
        assert!((sink.sources[1].2.psd_at(10.0) - 1e-20).abs() < 1e-32);
    }
}
