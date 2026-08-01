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
//! # Limitations
//!
//! - **White sources only.** [`NoiseSink::white_current`] is the one channel; flicker (`1/f`)
//!   noise has no representation here. A device with a `KF`/`AF` flicker model would need a
//!   `flicker_current` sibling — deliberately an *additive* future change of exactly the same
//!   shape, not a reshaping of this one. QSPICE reports a `1overf` column per device, which is
//!   identically zero for every model this crate ships (none declares a flicker coefficient),
//!   so this limit costs nothing against the current validation gate and would cost correctness
//!   immediately if a flicker-bearing model were added.
//! - **No noise from `va-codegen`-generated models.** Verilog-A's own `white_noise()`/
//!   `flicker_noise()` functions are not lowered yet, so a compiled model takes
//!   [`crate::ModelInstance::noise`]'s default (no sources) and a circuit built from one
//!   computes zero noise. That is why the noise validation circuit uses the hand-written
//!   [`crate::reference`] devices rather than a `--model` compiled one.

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

/// A collecting [`NoiseSink`] that just records every source it is handed — the noise-channel
/// analogue of [`crate::stamps::DenseStamp`], for tests and for any caller that wants the raw
/// per-device sources rather than an accumulated result.
#[derive(Clone, Debug, Default)]
pub struct CollectedNoise {
    /// Every `(p, n, psd)` reported, in the order the instances emitted them.
    pub sources: Vec<(usize, usize, f64)>,
}

impl NoiseSink for CollectedNoise {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        self.sources.push((p, n, psd));
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
}
