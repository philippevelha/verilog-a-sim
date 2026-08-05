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
//! # Three source kinds
//!
//! [`NoiseSink::white_current`] is frequency-flat; [`NoiseSink::flicker_current`] is
//! `coeff / f^exponent`, the `1/f`-family shape (§6 change, 2026-08-01 — added when
//! `va-codegen` learned to lower Verilog-A's `flicker_noise()`); [`NoiseSink::table_current`] is
//! an arbitrary PSD given as interpolated `(frequency, power)` pairs (§6 change, 2026-08-04 —
//! added when `va-codegen` learned to lower Verilog-A's `noise_table()`). Splitting them by
//! *shape* rather than passing a closure keeps the channel a plain data contract: a sink stores
//! the coefficients (or the table) and evaluates the shape itself, so nothing here has to be
//! re-entered per frequency.
//!
//! # Limitations
//!
//! - **These three shapes only.** They cover every noise function Verilog-A defines
//!   (LRM §4.6.4) except the file-input form of `noise_table()`, which is a frontend question
//!   (reading a `.tbl` file) rather than an ABI one — the table reaches this channel the same
//!   way either way.
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

    /// A **tabulated** current-noise source across the branch `p`-`n`, whose one-sided PSD is
    /// given by `points` — `(frequency Hz, power A²/Hz)` pairs interpolated per `interp`
    /// (Verilog-A's `noise_table()`/`noise_table_log()`, LRM §4.6.4.3/§4.6.4.4).
    ///
    /// `points` **must be sorted by ascending frequency with no duplicates**; the LRM makes
    /// sorting the simulator's job, and this project does it once at elaboration (where the
    /// table is const-folded) rather than per emission. [`NoiseSource::table`] enforces the
    /// invariant for anything built through it. Outside the tabulated range the PSD is clamped
    /// to the nearest endpoint's power, per the LRM.
    ///
    /// Unlike the other two channels there are no coefficients to evaluate at the operating
    /// point: an LRM table is constant data (an array parameter or an assignment pattern), so a
    /// tabulated source is bias-independent by construction.
    ///
    /// Default: **no source**, on the same "an existing sink needs no change" grounds as
    /// [`NoiseSink::flicker_current`].
    fn table_current(&mut self, p: usize, n: usize, points: &[(f64, f64)], interp: TableInterp) {
        let _ = (p, n, points, interp);
    }
}

/// How a tabulated PSD is interpolated between its `(frequency, power)` points.
///
/// The two rules are the difference between Verilog-A's `noise_table()` and
/// `noise_table_log()`, which are otherwise the same function over the same data
/// (LRM §4.6.4.3/§4.6.4.4, and its Figure 4-9 comparing them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableInterp {
    /// Piecewise linear in `(f, power)` — `noise_table()`. Straight segments on a *linear*
    /// plot, which is why the LRM's own Figure 4-9 shows it drooping between decade points
    /// where a `1/f` law would be straight.
    Linear,
    /// Piecewise linear in `(log₁₀ f, log₁₀ power)` — `noise_table_log()`. Straight segments on
    /// a *log-log* plot, so two points suffice to describe an exact power law.
    Log,
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

/// PSD at frequency `f` of a table of `(frequency, power)` pairs, per `interp`
/// (LRM §4.6.4.3/§4.6.4.4).
///
/// `points` must be sorted by ascending frequency (§ [`NoiseSink::table_current`]); an unsorted
/// table would silently produce a wrong interpolation rather than an error, which is why the
/// sort happens once, at the single place tables are built. Outside the tabulated range the
/// nearest endpoint's power is returned — the LRM's own clamping rule, not an extrapolation. An
/// empty table has no power at any frequency and yields `0.0`.
///
/// [`TableInterp::Log`] falls back to linear interpolation across any segment whose endpoints
/// are not both strictly positive in frequency *and* power: `log(0)` is undefined, and a table
/// point at zero power is legal data (a band where the model declares no noise). The fallback is
/// per segment, so one such point never degrades the rest of the table.
pub fn table_psd_at(points: &[(f64, f64)], interp: TableInterp, f: f64) -> f64 {
    let (Some(&(f_lo, p_lo)), Some(&(f_hi, p_hi))) = (points.first(), points.last()) else {
        return 0.0;
    };
    if f <= f_lo {
        return p_lo;
    }
    if f >= f_hi {
        return p_hi;
    }
    // The bracketing segment: the last point at or below `f`, and the one after it. Linear scan
    // rather than a binary search — an LRM noise table is a handful of points, and the
    // straight-line loop is the cheaper of the two at that size.
    let seg = points
        .windows(2)
        .find(|w| w[0].0 <= f && f <= w[1].0)
        .expect("f lies strictly inside the tabulated range, so some segment brackets it");
    let ((f1, p1), (f2, p2)) = (seg[0], seg[1]);
    if f2 == f1 {
        return p1;
    }
    let log_ok = interp == TableInterp::Log && f1 > 0.0 && p1 > 0.0 && p2 > 0.0;
    if log_ok {
        // P = 10^( log p1 + (log p2 - log p1)·(log f - log f1)/(log f2 - log f1) ), verbatim from
        // the LRM's §4.6.4.4 formula.
        let (lf1, lf2, lf) = (f1.log10(), f2.log10(), f.log10());
        let (lp1, lp2) = (p1.log10(), p2.log10());
        return 10f64.powf(lp1 + (lp2 - lp1) * (lf - lf1) / (lf2 - lf1));
    }
    p1 + (p2 - p1) * (f - f1) / (f2 - f1)
}

/// One noise source, as collected from an instance — kept as its *shape* plus coefficients
/// rather than a number, so a frequency sweep can evaluate it per point.
///
/// Not `Copy`: [`NoiseSource::Table`] owns its points (§6 change, 2026-08-04). Cloning happens
/// once per source at collection time, never inside a frequency loop.
#[derive(Clone, Debug, PartialEq)]
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
    /// An interpolated table of `(frequency Hz, power A²/Hz)` pairs — Verilog-A's
    /// `noise_table()`/`noise_table_log()`.
    Table {
        /// The pairs, **sorted by ascending frequency** (§ [`NoiseSource::table`]).
        points: Vec<(f64, f64)>,
        /// Which interpolation rule applies between them.
        interp: TableInterp,
    },
}

impl NoiseSource {
    /// Build a [`NoiseSource::Table`], establishing the ascending-frequency invariant
    /// [`table_psd_at`] relies on.
    ///
    /// The LRM makes sorting the simulator's responsibility ("the simulator shall internally
    /// sort the pairs into ascending frequency if required"), so an out-of-order table is valid
    /// input, not an error. Duplicate frequencies are *not* rejected here — the LRM forbids them
    /// and this project rejects them where the diagnostic is useful (at elaboration, naming the
    /// source file), while `table_psd_at` degrades gracefully if one slips through, returning
    /// the first of the two powers rather than dividing by a zero-width segment.
    pub fn table(mut points: Vec<(f64, f64)>, interp: TableInterp) -> Self {
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        NoiseSource::Table { points, interp }
    }

    /// This source's one-sided PSD (A²/Hz) at frequency `f` (Hz).
    pub fn psd_at(&self, f: f64) -> f64 {
        match self {
            NoiseSource::White { psd } => *psd,
            NoiseSource::Flicker { coeff, exponent } => flicker_psd_at(*coeff, *exponent, f),
            NoiseSource::Table { points, interp } => table_psd_at(points, *interp, f),
        }
    }

    /// Whether this source has zero power at *every* frequency — a source an analysis can drop
    /// without changing any result.
    ///
    /// Not simply "is the coefficient zero": a table is powerless only if every one of its
    /// points is, and an empty table is powerless too.
    pub fn is_powerless(&self) -> bool {
        match self {
            NoiseSource::White { psd } => *psd <= 0.0,
            NoiseSource::Flicker { coeff, .. } => *coeff <= 0.0,
            NoiseSource::Table { points, .. } => points.iter().all(|&(_, p)| p <= 0.0),
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

    fn table_current(&mut self, p: usize, n: usize, points: &[(f64, f64)], interp: TableInterp) {
        self.sources
            .push((p, n, NoiseSource::table(points.to_vec(), interp)));
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

    /// The LRM's own §4.6.4.3 example table (its `noise_table_input.tbl` listing, whose powers
    /// double per decade), interpolated at a frequency *between* two decade points. Linear
    /// interpolation is linear in `f`, not in `log f` — at 55 Hz, five-ninths of the way from
    /// 10 Hz to 100 Hz *in frequency*, the power is five-ninths of the way from 3.31516e-23 to
    /// 6.63632e-23. A log-interpolating implementation would instead return ~4.9e-23 (half a
    /// decade up), which is the specific wrong answer this pins against.
    #[test]
    fn linear_table_interpolates_in_frequency_not_in_log_frequency() {
        let points = vec![
            (1.0e0, 1.657_580e-23),
            (1.0e1, 3.315_160e-23),
            (1.0e2, 6.636_320e-23),
            (1.0e3, 1.326_064e-22),
        ];
        let got = table_psd_at(&points, TableInterp::Linear, 55.0);
        let frac = (55.0 - 10.0) / (100.0 - 10.0);
        let want = 3.315_160e-23 + frac * (6.636_320e-23 - 3.315_160e-23);
        assert!((got - want).abs() < 1e-30, "got {got}, want {want}");
        // And it is emphatically *not* the geometric midpoint a log rule would give.
        let log_answer = (3.315_160e-23f64 * 6.636_320e-23).sqrt();
        assert!((got - log_answer).abs() > 1e-25);
    }

    /// Exact tabulated points come back exactly — no interpolation error at a knot.
    #[test]
    fn table_returns_its_own_points_exactly() {
        let points = vec![(1.0, 2e-20), (10.0, 5e-20), (100.0, 1e-20)];
        for &(f, p) in &points {
            assert_eq!(table_psd_at(&points, TableInterp::Linear, f), p);
        }
    }

    /// The LRM clamps outside the tabulated range rather than extrapolating: below the lowest
    /// frequency the lowest point's power, above the highest the highest point's.
    #[test]
    fn table_clamps_outside_its_range_instead_of_extrapolating() {
        let points = vec![(10.0, 4e-20), (100.0, 1e-20)];
        assert_eq!(table_psd_at(&points, TableInterp::Linear, 1.0), 4e-20);
        assert_eq!(table_psd_at(&points, TableInterp::Linear, 0.0), 4e-20);
        assert_eq!(table_psd_at(&points, TableInterp::Linear, 1e9), 1e-20);
        // A downward-extrapolating implementation would have gone negative above 133 Hz.
        assert!(table_psd_at(&points, TableInterp::Linear, 1e9) > 0.0);
    }

    /// `noise_table_log`'s rule: two points describe an exact power law, so a `1/f` table
    /// reproduces `1/f` at every intermediate frequency — the LRM's Figure 4-9 point.
    #[test]
    fn log_table_reproduces_a_power_law_from_two_points() {
        let points = vec![(1.0, 1.0), (1e6, 1e-6)];
        for f in [2.0, 37.0, 1e3, 4.2e4] {
            let got = table_psd_at(&points, TableInterp::Log, f);
            assert!(
                (got - 1.0 / f).abs() < 1e-12 * (1.0 / f),
                "at {f} Hz got {got}, want {}",
                1.0 / f
            );
        }
        // The same two points read linearly are a straight line on a *linear* plot, which at
        // 1 kHz is still essentially the 1.0 endpoint — three orders away from the log answer.
        let lin = table_psd_at(&points, TableInterp::Linear, 1e3);
        assert!(lin > 0.99, "linear interpolation gave {lin}");
    }

    /// A zero-power table point is legal data; `log(0)` is not. That segment interpolates
    /// linearly instead of producing `NaN`/`-inf`, and only that segment.
    #[test]
    fn log_table_falls_back_to_linear_across_a_zero_power_point() {
        let points = vec![(1.0, 0.0), (10.0, 1e-20), (100.0, 1e-22)];
        let across_zero = table_psd_at(&points, TableInterp::Log, 5.5);
        assert!(across_zero.is_finite(), "got {across_zero}");
        assert!((across_zero - 0.5e-20).abs() < 1e-33, "got {across_zero}");
        // The next segment still interpolates logarithmically: 1e-21 at the geometric midpoint.
        let log_segment = table_psd_at(&points, TableInterp::Log, (10.0f64 * 100.0).sqrt());
        assert!((log_segment - 1e-21).abs() < 1e-33, "got {log_segment}");
    }

    /// An unsorted table is legal input the simulator must sort (LRM §4.6.4.3) — the constructor
    /// is where that happens, so nothing downstream has to re-check.
    #[test]
    fn table_source_sorts_its_points_by_ascending_frequency() {
        let src = NoiseSource::table(
            vec![(100.0, 1e-22), (1.0, 1e-20), (10.0, 1e-21)],
            TableInterp::Linear,
        );
        let NoiseSource::Table { points, .. } = &src else {
            panic!("built a table");
        };
        assert_eq!(points[0].0, 1.0);
        assert_eq!(points[2].0, 100.0);
        // And it evaluates as if it had been written in order.
        assert_eq!(src.psd_at(1.0), 1e-20);
        assert_eq!(src.psd_at(0.1), 1e-20);
    }

    /// An empty table is powerless everywhere rather than a panic on `first()`/`last()`.
    #[test]
    fn empty_table_has_no_power_and_does_not_panic() {
        assert_eq!(table_psd_at(&[], TableInterp::Linear, 1e3), 0.0);
        assert!(NoiseSource::table(vec![], TableInterp::Linear).is_powerless());
    }

    /// `is_powerless` is per-source, not per-coefficient: a table with any nonzero point carries
    /// power even though most of it is zero.
    #[test]
    fn a_table_is_powerless_only_if_every_point_is() {
        let all_zero = NoiseSource::table(vec![(1.0, 0.0), (10.0, 0.0)], TableInterp::Linear);
        let one_point = NoiseSource::table(vec![(1.0, 0.0), (10.0, 1e-20)], TableInterp::Linear);
        assert!(all_zero.is_powerless());
        assert!(!one_point.is_powerless());
        assert!(NoiseSource::White { psd: 0.0 }.is_powerless());
        assert!(!NoiseSource::White { psd: 1e-20 }.is_powerless());
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
