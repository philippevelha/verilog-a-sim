//! Transient noise (2.55.0; flicker 2.57.0): the white and flicker noise sources a device declares for `.noise`, made
//! active in a transient run.
//!
//! Every device reports its noise through [`ModelInstance::noise`] as current sources with a
//! one-sided power spectral density. A small-signal noise analysis turns those into a spectrum
//! at an operating point. This module turns them into **waveforms**: [`WithTranNoise`] wraps an
//! instance, and at every transient evaluation adds, for each of its white sources, a random
//! current across the source's branch.
//!
//! # The waveform
//!
//! A source of density `S` (A²/Hz) becomes a sequence of independent normal samples `g_k` at the
//! instants `k·dt`, `dt = 1/(2·fmax)`, each of variance `S·fmax`, joined by straight lines:
//!
//! ```text
//!   i(t) = sqrt(S(x)·fmax) · [ g_k + (g_(k+1) − g_k)·(t − k·dt)/dt ],   k = floor(t/dt)
//! ```
//!
//! Its one-sided spectrum is `S·sinc⁴(f·dt)`: `S` at low frequency, 0.5 dB down at `0.26·fmax`,
//! 1.8 dB down at `fmax/2`, 7.8 dB at `fmax`. `fmax` is the caller's choice and should be several times the band
//! of interest. The time step is bounded to `dt`, so every straight segment is visited.
//!
//! **The step and the circuit's bandwidth.** With one step per sample the trapezoidal rule
//! compresses the frequency axis, and a pole at `fc` keeps about `1/(1 + π·fc/(2·fmax))` of its
//! noise variance (the bilinear transform's warping): 0.99 at `fmax = 125·fc`, 0.96 at `40·fc`.
//! Measured on an RC load (`paper_writing/mwp/tests/t11_transient_noise.py`): within 1.4 % of
//! that at `40·fc`; at `10·fc` the loss was larger than the formula's 0.86, about 0.74. Choose
//! `fmax` from the fastest pole that shapes the noise, not from the signal band alone.
//!
//! # What makes it safe inside an adaptive integrator
//!
//! - **No state.** `g_k` is a hash of `(seed, instance, source, k)`, not a draw from a running
//!   generator. A rejected step, a Newton iteration, an event re-solve and a second run with
//!   the same seed all see the same waveform, and the state channel is untouched.
//! - **Continuous, and landed on.** Straight lines between samples have no jumps. They do have
//!   kinks, and a node whose voltage is algebraic in a noise current (a resistor alone, PSP's
//!   induced-gate-noise node) follows them exactly: to the divided-difference error estimate a
//!   kink inside a step is a truncation error of the size of the noise, and the step collapsed
//!   to resolve each one (found 2026-11-17 on an SG13G2 driver: E0605 at t = 0, a single
//!   transistor at Vds = 0 not finishing 100 ps). So every sample instant is a **breakpoint**
//!   (registered through the event channel, the `zi_*` pattern): each step lies inside one
//!   straight segment, which trapezoidal integration integrates exactly.
//! - **Follows the operating point.** `S` is evaluated at each sample instant `k·dt` the run
//!   lands on (2.57.0, in the event poll the integrator makes at an accepted timepoint), and
//!   through the segment after it moves linearly from its value at `(k-1)·dt` to its value at
//!   `k·dt`. So shot noise follows the current that produces it (cyclostationary noise), at
//!   most two samples (`1/fmax`) late, and the injected current is continuous in time and a
//!   function of time alone: an independent source, on which Newton converges as it does
//!   without noise. Two designs were measured and dropped on an SG13G2 driver (2026-11-17):
//!   `S` at every Newton iterate (2.55.0) cost a second full model evaluation per load and
//!   left the amplitude's dependence on the unknowns out of the Jacobian, so Newton converged
//!   linearly under the strict transient tolerance (a noisy timepoint cost 6x a clean one);
//!   `S` held constant from one accepted timepoint to the next jumped at each of them, and
//!   during a switching edge, where the densities change by decades, the step controller
//!   could not get past the jump (E0605). The starting DC solve evaluates `S` at its own
//!   iterate.
//!
//! # Flicker sources (2.57.0)
//!
//! A source of density `coeff/f^e` (Verilog-A's `flicker_noise(coeff, e)`, PSP's and BSIM's
//! 1/f noise, a laser linewidth as `e = 2`) cannot be a sequence of independent samples, and a
//! filter with memory would break the statelessness above. It is synthesized instead as a sum
//! of sinusoids, one per band, [`FLICKER_BANDS_PER_DECADE`] bands per decade from `fmin` to
//! `fmax/4` ([`TranNoiseConfig::flicker_fmin`]):
//!
//! ```text
//!   i(t) = Σ_j sqrt(P_j·coeff(x)) · [ a_j·cos(2π f_j t) + b_j·sin(2π f_j t) ]
//!   P_j  = ∫ f^-e df over band j,   a_j, b_j ~ N(0, 1),   f_j log-uniform in band j
//! ```
//!
//! `a_j`, `b_j` and `f_j` are hashes of `(seed, instance, source, j)`, like the white samples:
//! the waveform is a pure function of time. Each band carries the band's exact power with a
//! Gaussian (Rayleigh-amplitude) coefficient, so the process is Gaussian, its variance is
//! `∫ coeff/f^e df` over `[fmin, fmax/4]`, and its spectrum averaged over a band is the
//! declared one. Within a band it is a line, not a continuum: a spectral estimate finer than
//! the band (`10^(1/10)`, 26 %) sees lines. Below `fmin` nothing is generated: choose it as
//! `1/(run length)` (va-cli's default), below which a run cannot tell a flicker source from a
//! constant. Above `fmax/4` nothing either: the top band has 8 time steps per period, and
//! flicker noise there is far below any device's white floor. The density follows the
//! operating point as the white sources do (its amplitude is scaled by `sqrt(coeff(x))`), which
//! is right for the bias-dependent 1/f of a transistor and is the same cyclostationary
//! approximation.
//!
//! # Limitations
//!
//! - **Tabulated sources are left out.** A `noise_table` source contributes **nothing**, and
//!   [`excluded_sources`] counts it so the caller can say so (`va-cli` raises `W0602`). So does
//!   a flicker source when [`TranNoiseConfig::flicker_fmin`] is `None`.
//! - Flicker is generated from `fmin` to `fmax/4` only, as discrete lines (above).
//! - **The start of a run.** The operating point is solved without noise, and every source is
//!   ramped in over the first sample, `i(t)·min(1, t/dt)` (2.57.0), so the run is continuous at
//!   `t = 0`. Before, the first step met every source at its `t = 0` value as a step input,
//!   which PSP103's charge rows turned into a step controller that never left `t = 0` (an
//!   SG13G2 transistor at Vds = 0; E0605 on a driver). Solving the operating point with the
//!   sources' `t = 0` currents instead was tried and dropped: it treats one broadband sample
//!   (variance `S·fmax`) as a DC current, and a slow node charges to that fiction -- the last
//!   node of a 50-section RC ladder read 17x kT/C over a microsecond
//!   (`paper_writing/mwp/tests/t11_transient_noise.py`). The noise then builds up from the
//!   noiseless state over each node's own time constants: discard those from a statistic, as
//!   one discards a start-up transient. A flicker source's slow part, too, is switched on over
//!   the first sample.
//! - The result is one realization. Statistics need a long run or several seeds.
//! - A source whose density is negative or not finite at some point contributes nothing there.
//! - The noise of a frequency-dependent model keeps its declared density; the model's own
//!   filtering acts on it as it does on any signal.

use va_abi::{
    noise::{NoiseSink, TableInterp},
    AnalysisCtx, AnalysisKind, EventSink, ModelInstance, ModelState, StampSink, UnknownKind,
};

/// The settings of a transient-noise run.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TranNoiseConfig {
    /// Noise bandwidth in hertz: samples are `1/(2·fmax)` apart. Must be positive and finite.
    pub fmax: f64,
    /// Seed of the realization. The same seed gives the same waveforms.
    pub seed: u64,
    /// Lowest frequency of the flicker sources, in hertz (2.57.0): they are synthesized from
    /// here to `fmax/4` (module documentation). `None` leaves flicker sources out, as before
    /// 2.57.0. `1/(run length)` is the natural choice. Must be positive, finite and below
    /// `fmax/4` to generate anything.
    pub flicker_fmin: Option<f64>,
}

/// Bands (one sinusoid each) per decade of a synthesized flicker source.
pub const FLICKER_BANDS_PER_DECADE: f64 = 10.0;

/// The flicker band edges for `fmin` .. `fmax/4`: `(lowest edge, ratio between edges, count)`,
/// or `None` when the range is empty or not finite.
fn flicker_bands(cfg: &TranNoiseConfig) -> Option<(f64, f64, u64)> {
    let fmin = cfg.flicker_fmin?;
    let ftop = 0.25 * cfg.fmax;
    if !(fmin.is_finite() && fmin > 0.0 && ftop > fmin) {
        return None;
    }
    let decades = libm::log10(ftop / fmin);
    let n = libm::ceil(decades * FLICKER_BANDS_PER_DECADE).max(1.0);
    Some((fmin, libm::pow(ftop / fmin, 1.0 / n), n as u64))
}

/// `∫ f^-e df` from `a` to `b`.
fn power_law_integral(a: f64, b: f64, e: f64) -> f64 {
    if (e - 1.0).abs() < 1e-12 {
        libm::log(b / a)
    } else {
        (libm::pow(b, 1.0 - e) - libm::pow(a, 1.0 - e)) / (1.0 - e)
    }
}

/// The value at time `t` of a unit flicker source (`coeff = 1`) of exponent `e`: the sum of
/// sinusoids of the module documentation. Multiply by `sqrt(coeff)`.
fn flicker_unit(cfg: &TranNoiseConfig, id: u64, source: u64, e: f64, t: f64) -> f64 {
    let Some((f0, r, n)) = flicker_bands(cfg) else {
        return 0.0;
    };
    // A stream apart from the white samples': those hash `k` (a sample index) where this hashes
    // a band index, under a different seed word.
    let seed = cfg.seed ^ 0xF11C_4E20_0000_0001;
    let mut sum = 0.0;
    let mut lo = f0;
    for j in 0..n {
        let hi = lo * r;
        let p = power_law_integral(lo, hi, e);
        let u = uniform(seed, id, source, 3 * j);
        let f = lo * libm::pow(r, u);
        let a = normal(seed, id, source, 3 * j + 1);
        let b = normal(seed, id, source, 3 * j + 2);
        let ph = 2.0 * std::f64::consts::PI * f * t;
        sum += libm::sqrt(p) * (a * libm::cos(ph) + b * libm::sin(ph));
        lo = hi;
    }
    sum
}

/// An instance with its white noise sources active in transient (this module's documentation).
///
/// Every other analysis, and everything but `load`, is forwarded unchanged.
pub struct WithTranNoise<'a> {
    inner: &'a dyn ModelInstance,
    id: u64,
    cfg: TranNoiseConfig,
    /// The densities at the last two sample instants landed on (the module documentation,
    /// "Follows the operating point"); `None` before the first accepted timepoint.
    hold: std::sync::Mutex<Option<Hold>>,
}

/// The sources' densities at sample instants `k - 1` (`prev`) and `k` (`cur`): through the
/// segment from `k·dt` to `(k+1)·dt` a density moves linearly from `prev`'s to `cur`'s, so it
/// is continuous in time and a function of time alone.
#[derive(Clone, Debug)]
struct Hold {
    k: u64,
    prev: Vec<Src>,
    cur: Vec<Src>,
}

/// One declared noise source, as [`Collect`] records it: `white` with density `a` (A²/Hz), or
/// flicker with coefficient `a` and exponent `e`; a tabulated source keeps its place with
/// `a = 0` (left out).
#[derive(Clone, Copy, Debug)]
struct Src {
    p: usize,
    n: usize,
    white: bool,
    a: f64,
    e: f64,
}

/// Records an instance's sources in the order it declares them.
struct Collect(Vec<Src>);

impl NoiseSink for Collect {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        self.0.push(Src {
            p,
            n,
            white: true,
            a: psd,
            e: 0.0,
        });
    }
    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {
        self.0.push(Src {
            p,
            n,
            white: false,
            a: coeff,
            e: exponent,
        });
    }
    fn table_current(&mut self, p: usize, n: usize, _points: &[(f64, f64)], _i: TableInterp) {
        self.0.push(Src {
            p,
            n,
            white: false,
            a: 0.0,
            e: 0.0,
        });
    }
}

impl<'a> WithTranNoise<'a> {
    /// Wrap `inner`. `id` distinguishes this instance's random sequences from every other
    /// instance's: give each instance of a circuit its own (its index is enough).
    pub fn new(inner: &'a dyn ModelInstance, id: u64, cfg: TranNoiseConfig) -> Self {
        WithTranNoise {
            inner,
            id,
            cfg,
            hold: std::sync::Mutex::new(None),
        }
    }
}

/// How many sources `instances` declare at `x` that a transient-noise run with `cfg` leaves
/// out, per instance index: the tabulated ones, and the flicker ones too when `cfg` generates
/// no flicker ([`TranNoiseConfig::flicker_fmin`] `None` or an empty band). Empty when there are
/// none.
pub fn excluded_sources(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    cfg: &TranNoiseConfig,
) -> Vec<(usize, usize)> {
    struct Count(usize, bool);
    impl NoiseSink for Count {
        fn white_current(&mut self, _p: usize, _n: usize, _psd: f64) {}
        fn flicker_current(&mut self, _p: usize, _n: usize, coeff: f64, _exponent: f64) {
            if coeff != 0.0 && !self.1 {
                self.0 += 1;
            }
        }
        fn table_current(&mut self, _p: usize, _n: usize, _points: &[(f64, f64)], _i: TableInterp) {
            self.0 += 1;
        }
    }
    let ctx = AnalysisCtx {
        kind: AnalysisKind::Noise,
        ..*ctx
    };
    instances
        .iter()
        .enumerate()
        .filter_map(|(i, inst)| {
            let mut c = Count(0, flicker_bands(cfg).is_some());
            inst.noise(x, &ctx, &mut c);
            (c.0 > 0).then_some((i, c.0))
        })
        .collect()
}

/// SplitMix64's finalizer.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The hash of four words that every deviate here is drawn from.
fn hash(seed: u64, id: u64, source: u64, k: u64) -> u64 {
    mix(mix(mix(mix(seed) ^ id) ^ source) ^ k)
}

/// A uniform deviate in `[0, 1)` for `(seed, instance, source, k)`.
fn uniform(seed: u64, id: u64, source: u64, k: u64) -> f64 {
    (hash(seed, id, source, k) >> 11) as f64 / (1u64 << 53) as f64
}

/// A standard normal deviate for `(seed, instance, source, sample)`, by hashing (SplitMix64's
/// finalizer over the four words) and the Box-Muller transform. Stateless on purpose.
fn normal(seed: u64, id: u64, source: u64, k: u64) -> f64 {
    let h = hash(seed, id, source, k);
    let h2 = mix(h);
    // (0, 1]: the logarithm's argument is never zero.
    let u1 = ((h >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
    let u2 = (h2 >> 11) as f64 / (1u64 << 53) as f64;
    libm::sqrt(-2.0 * libm::log(u1)) * libm::cos(2.0 * std::f64::consts::PI * u2)
}

/// The sample index of `time` and the fraction of the segment after it. An instant within a
/// millionth of a sample of `k·dt` is `k·dt` itself, whatever its last bits.
fn sample_of(time: f64, dt: f64) -> (u64, f64) {
    let s = (time / dt).max(0.0);
    let k = libm::floor(s + 1e-6);
    (k as u64, (s - k).max(0.0))
}

/// Adds the current at `time` of each source to the residual (the module documentation): the
/// density of `cur`, or, with `prev` of the same layout, `prev`'s moved toward `cur`'s by `w`.
fn inject(
    prev: Option<&[Src]>,
    cur: &[Src],
    w: f64,
    cfg: &TranNoiseConfig,
    id: u64,
    time: f64,
    sink: &mut dyn StampSink,
) {
    let dt = 0.5 / cfg.fmax;
    let (k, frac) = sample_of(time, dt);
    // Every source is ramped in over the first sample from the noiseless operating point (the
    // module documentation, "The start of a run").
    let ramp = (time / dt).clamp(0.0, 1.0);
    if ramp == 0.0 {
        return;
    }
    let prev = prev.filter(|p| p.len() == cur.len());
    for (source, src) in cur.iter().enumerate() {
        let a = match prev {
            Some(p) if p[source].a.is_finite() => p[source].a + (src.a - p[source].a) * w,
            _ => src.a,
        };
        let src = Src { a, ..*src };
        let source = source as u64;
        if !(src.a.is_finite() && src.a > 0.0) {
            continue;
        }
        let i = if src.white {
            let g0 = normal(cfg.seed, id, source, k);
            let g1 = normal(cfg.seed, id, source, k + 1);
            libm::sqrt(src.a * cfg.fmax) * (g0 + (g1 - g0) * frac)
        } else if src.e.is_finite() {
            libm::sqrt(src.a) * flicker_unit(cfg, id, source, src.e, time)
        } else {
            0.0
        };
        let i = ramp * i;
        if i != 0.0 {
            sink.residual(src.p, i);
            sink.residual(src.n, -i);
        }
    }
}

impl WithTranNoise<'_> {
    /// The instance's sources and densities at `x`.
    fn collect(&self, x: &[f64], ctx: &AnalysisCtx) -> Vec<Src> {
        let noise_ctx = AnalysisCtx {
            kind: AnalysisKind::Noise,
            ..*ctx
        };
        let mut c = Collect(Vec::new());
        self.inner.noise(x, &noise_ctx, &mut c);
        c.0
    }

    /// At an accepted timepoint: on a sample instant not yet seen, the densities there become
    /// `cur` (the old `cur` becomes `prev`); at the first accepted timepoint both are set. And
    /// the next sample instant is asked for as a breakpoint (module documentation,
    /// "Continuous, and landed on").
    fn next_sample(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn EventSink) {
        if ctx.kind != AnalysisKind::Transient {
            return;
        }
        let dt = 0.5 / self.cfg.fmax;
        let (k, frac) = sample_of(ctx.time, dt);
        let on_sample = frac < 1e-6;
        let mut any = true;
        if let Ok(mut h) = self.hold.lock() {
            match h.as_mut() {
                None => {
                    let srcs = self.collect(x, ctx);
                    *h = Some(Hold {
                        k,
                        prev: srcs.clone(),
                        cur: srcs,
                    });
                }
                Some(hold) if on_sample && k > hold.k => {
                    let srcs = self.collect(x, ctx);
                    hold.prev = std::mem::replace(&mut hold.cur, srcs);
                    hold.k = k;
                }
                Some(_) => {}
            }
            any = h.as_ref().is_some_and(|h| !h.cur.is_empty());
        }
        if any {
            sink.breakpoint((k as f64 + 1.0) * dt);
        }
    }
}

impl ModelInstance for WithTranNoise<'_> {
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
        self.inner.unknown_abstol(i)
    }

    fn load(&self, x: &[f64], ctx: &AnalysisCtx, state: &mut ModelState, sink: &mut dyn StampSink) {
        self.inner.load(x, ctx, state, sink);
        if ctx.kind != AnalysisKind::Transient {
            return;
        }
        let time = ctx.time;
        let dt = 0.5 / self.cfg.fmax;
        // The densities held at the last sample instants, moved linearly through the segment;
        // before a first accepted timepoint, those of the solution being tried.
        let held = self.hold.lock().ok().and_then(|h| h.clone());
        let any = match held {
            Some(h) => {
                let (k, frac) = sample_of(time, dt);
                // a step never crosses a sample instant (they are breakpoints); one that starts
                // before the hold was last moved reads it at its end
                let w = if k == h.k { frac } else { 1.0 };
                inject(Some(&h.prev), &h.cur, w, &self.cfg, self.id, time, sink);
                !h.cur.is_empty()
            }
            None => {
                let srcs = self.collect(x, ctx);
                inject(None, &srcs, 1.0, &self.cfg, self.id, time, sink);
                !srcs.is_empty()
            }
        };
        if any {
            sink.bound_step(dt);
        }
    }

    fn state_len(&self) -> usize {
        self.inner.state_len()
    }

    fn is_frequency_dependent(&self) -> bool {
        self.inner.is_frequency_dependent()
    }

    /// Never: the injected current depends on time, so an evaluation cannot be reused.
    fn is_pure_in_x(&self) -> bool {
        false
    }

    fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) {
        self.inner.noise(x, ctx, sink);
    }

    fn event_count(&self) -> usize {
        self.inner.event_count()
    }

    fn events(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn EventSink) {
        self.inner.events(x, ctx, sink);
        self.next_sample(x, ctx, sink);
    }

    fn events_with_state(
        &self,
        x: &[f64],
        ctx: &AnalysisCtx,
        state: &[f64],
        sink: &mut dyn EventSink,
    ) {
        self.inner.events_with_state(x, ctx, state, sink);
        self.next_sample(x, ctx, sink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrator::{run, LteEstimator, Method, TranConfig};
    use va_abi::reference::{Capacitor, Resistor, GROUND};

    fn tran(tstop: f64) -> TranConfig {
        TranConfig {
            tstart: 0.0,
            tstop,
            tstep: 25e-12,
            tstep_min: 1e-18,
            method: Method::Trapezoidal,
            lte_reltol: 1e-3,
            // The signal is microvolts: the default 1 uV floor would not see it.
            lte_abstol: 1e-9,
            lte_estimator: LteEstimator::DividedDifference,
            newton_reltol: crate::integrator::NEWTON_RELTOL_DEFAULT,
            solver: Default::default(),
            btf: true,
            bypass: None,
            on_task: None,
            on_file: None,
            stall_attempts: None,
        }
    }

    /// The hashed deviates are standard normal: mean 0, variance 1, fourth moment 3, and
    /// neighbouring samples, sources and instances are uncorrelated.
    #[test]
    fn the_hashed_deviates_are_standard_normal_and_independent() {
        let n = 200_000u64;
        let (mut m1, mut m2, mut m4, mut c_k, mut c_src, mut c_id) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for k in 0..n {
            let g = normal(7, 3, 0, k);
            m1 += g;
            m2 += g * g;
            m4 += g * g * g * g;
            c_k += g * normal(7, 3, 0, k + 1);
            c_src += g * normal(7, 3, 1, k);
            c_id += g * normal(7, 4, 0, k);
        }
        let n = n as f64;
        assert!((m1 / n).abs() < 0.01, "mean {}", m1 / n);
        assert!((m2 / n - 1.0).abs() < 0.01, "variance {}", m2 / n);
        assert!((m4 / n - 3.0).abs() < 0.1, "fourth moment {}", m4 / n);
        for (what, c) in [("lag 1", c_k), ("source", c_src), ("instance", c_id)] {
            assert!((c / n).abs() < 0.01, "{what} correlation {}", c / n);
        }
    }

    /// A resistor's thermal noise on a capacitor: the voltage variance is `kT/C`, whatever the
    /// resistance. 1 kΩ and 1 pF (1 ns), noise bandwidth 20 GHz, 4 µs: 4000 time constants, so
    /// the estimate is good to about 3 %. Time-weighted, since the steps are not uniform.
    #[test]
    fn thermal_noise_on_a_capacitor_is_kt_over_c() {
        let r = Resistor::new(0, GROUND, 1e3);
        let c = Capacitor::new(0, GROUND, 1e-12);
        let cfg = TranNoiseConfig {
            fmax: 20e9,
            seed: 11,
            flicker_fmin: None,
        };
        let noisy = WithTranNoise::new(&r, 0, cfg);
        let insts: [&dyn ModelInstance; 2] = [&noisy, &c];
        let wf = run(&insts, 1, vec![0.0], tran(4e-6)).expect("integrates");
        let (mut sum, mut sum2, mut span) = (0.0, 0.0, 0.0);
        for i in 1..wf.t.len() {
            let h = wf.t[i] - wf.t[i - 1];
            let v = 0.5 * (wf.value(i, 0).unwrap() + wf.value(i - 1, 0).unwrap());
            sum += h * v;
            sum2 += h * v * v;
            span += h;
        }
        let var = sum2 / span - (sum / span) * (sum / span);
        let kt_over_c = va_abi::noise::BOLTZMANN * va_abi::noise::TEMP_NOMINAL / 1e-12;
        assert!(
            (var / kt_over_c - 1.0).abs() < 0.1,
            "variance {var:e} V^2, kT/C = {kt_over_c:e} V^2"
        );
    }

    fn flicker_cfg(seed: u64) -> TranNoiseConfig {
        TranNoiseConfig {
            fmax: 4e9,
            seed,
            flicker_fmin: Some(1e6),
        }
    }

    /// A synthesized flicker source's variance, over realizations, is `∫ f^-e df` from `fmin`
    /// to `fmax/4`, for 1/f and for 1/f^2 (a laser linewidth's phase): 4000 seeds, about 2 %.
    #[test]
    fn flicker_variance_is_the_integral_of_its_density() {
        for e in [1.0, 2.0, 0.8] {
            let n = 4000;
            let mut v = 0.0;
            for seed in 0..n {
                let s = flicker_unit(&flicker_cfg(seed), 5, 2, e, 3.7e-7);
                v += s * s;
            }
            let v = v / n as f64;
            let want = power_law_integral(1e6, 1e9, e);
            assert!(
                (v / want - 1.0).abs() < 0.08,
                "e={e}: variance {v:e}, ∫ {want:e}"
            );
        }
    }

    /// The spectrum falls as the exponent says: a Hann-windowed periodogram, averaged over 40
    /// realizations, puts equal power in the decades 3–30 MHz and 30–300 MHz for 1/f, and ten
    /// times more in the lower one for 1/f^2. Each decade sum also matches `∫ f^-e df` to 15 %.
    /// Computed from the formula's own output on a uniform grid, independent of its construction.
    /// `fmax` 1.6 GHz puts the top band (400 MHz) below the grid's Nyquist frequency (500 MHz):
    /// with 4 GHz, tones at 700-970 MHz folded onto 30-300 MHz and read 17 % high.
    #[test]
    fn flicker_spectrum_falls_as_its_exponent() {
        let (dt, n) = (1e-9, 4000usize);
        let tt = dt * n as f64;
        for e in [1.0, 2.0] {
            let mut decade = [0.0f64; 2];
            let seeds = 40;
            for seed in 0..seeds {
                let cfg = TranNoiseConfig {
                    fmax: 1.6e9,
                    ..flicker_cfg(100 + seed)
                };
                let w: Vec<f64> = (0..n)
                    .map(|k| {
                        let hann =
                            0.5 - 0.5 * libm::cos(2.0 * std::f64::consts::PI * k as f64 / n as f64);
                        hann * flicker_unit(&cfg, 1, 0, e, k as f64 * dt)
                    })
                    .collect();
                // one-sided periodogram, Hann power normalization (mean of w^2 = 3/8)
                for (d, lo) in [(0usize, 3e6), (1, 3e7)] {
                    let (b0, b1) = ((lo * tt).ceil() as usize, (10.0 * lo * tt).floor() as usize);
                    for b in b0..b1 {
                        let (mut re, mut im) = (0.0, 0.0);
                        for (k, x) in w.iter().enumerate() {
                            let ph = 2.0 * std::f64::consts::PI * (b * k) as f64 / n as f64;
                            re += x * libm::cos(ph);
                            im -= x * libm::sin(ph);
                        }
                        decade[d] += 2.0 * dt * (re * re + im * im) / (n as f64 * 0.375) / tt;
                    }
                }
            }
            for (d, lo) in [(0usize, 3e6), (1, 3e7)] {
                let got = decade[d] / seeds as f64;
                let want = power_law_integral(lo, 10.0 * lo, e);
                assert!(
                    (got / want - 1.0).abs() < 0.15,
                    "e={e}, decade {d}: {got:e} vs {want:e}"
                );
            }
        }
    }

    /// A device with a 1/f current source across a 1 kΩ resistor: the integrator's node voltage
    /// is the synthesized current through the resistor at every time point (the wiring, end to
    /// end), and without `flicker_fmin` the source is left out and counted by `excluded_sources`.
    #[test]
    fn a_flicker_source_drives_the_circuit_and_is_counted_only_when_left_out() {
        struct Fl;
        impl ModelInstance for Fl {
            fn unknowns(&self) -> &[usize] {
                &[0]
            }
            fn load(&self, x: &[f64], _: &AnalysisCtx, _: &mut ModelState, s: &mut dyn StampSink) {
                s.residual(0, x[0] / 1e3);
                s.jacobian(0, 0, 1e-3);
            }
            fn noise(&self, _: &[f64], _: &AnalysisCtx, s: &mut dyn NoiseSink) {
                s.flicker_current(0, GROUND, 1e-16, 1.0);
            }
        }
        let fl = Fl;
        let cfg = flicker_cfg(3);
        let noisy = WithTranNoise::new(&fl, 0, cfg);
        let insts: [&dyn ModelInstance; 1] = [&noisy];
        let wf = run(&insts, 1, vec![0.0], tran(200e-9)).expect("integrates");
        let mut moved = 0.0f64;
        // row 0 is the initial vector the run was given, solved without noise; the source is
        // ramped in over the first sample (module documentation, "The start of a run")
        let dt = 0.5 / cfg.fmax;
        for i in 1..wf.t.len() {
            let ramp = (wf.t[i] / dt).min(1.0);
            let want = -1e3 * 1e-8 * ramp * flicker_unit(&cfg, 0, 0, 1.0, wf.t[i]);
            let v = wf.value(i, 0).unwrap();
            assert!(
                (v - want).abs() < 1e-9 * (1.0 + want.abs() * 1e3),
                "t={}: {v:e} vs {want:e}",
                wf.t[i]
            );
            moved = moved.max(v.abs());
        }
        assert!(
            moved > 1e-6,
            "the source moved the node by at most {moved:e} V"
        );

        let ctx = AnalysisCtx::transient(0.0);
        let plain: [&dyn ModelInstance; 1] = [&fl];
        assert!(excluded_sources(&plain, &[0.0], &ctx, &cfg).is_empty());
        let off = TranNoiseConfig {
            flicker_fmin: None,
            ..cfg
        };
        assert_eq!(excluded_sources(&plain, &[0.0], &ctx, &off), vec![(0, 1)]);
    }

    /// The same seed gives the same waveform, another seed another one, and without the wrapper
    /// the circuit stays at rest.
    #[test]
    fn a_run_is_reproducible_by_its_seed() {
        let r = Resistor::new(0, GROUND, 1e3);
        let c = Capacitor::new(0, GROUND, 1e-12);
        let go = |seed: Option<u64>| {
            let tran = tran(20e-9);
            match seed {
                Some(seed) => {
                    let noisy = WithTranNoise::new(
                        &r,
                        0,
                        TranNoiseConfig {
                            fmax: 20e9,
                            seed,
                            flicker_fmin: None,
                        },
                    );
                    let insts: [&dyn ModelInstance; 2] = [&noisy, &c];
                    run(&insts, 1, vec![0.0], tran).expect("integrates")
                }
                None => {
                    let insts: [&dyn ModelInstance; 2] = [&r, &c];
                    run(&insts, 1, vec![0.0], tran).expect("integrates")
                }
            }
        };
        let last = |wf: &crate::integrator::Waveform| wf.value(wf.t.len() - 1, 0).unwrap();
        let (a, b, other, quiet) = (go(Some(1)), go(Some(1)), go(Some(2)), go(None));
        assert_eq!(a.t, b.t);
        assert_eq!(last(&a), last(&b));
        assert_ne!(last(&a), last(&other));
        assert_eq!(last(&quiet), 0.0);
    }
}
