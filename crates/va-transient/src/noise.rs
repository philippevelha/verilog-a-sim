//! Transient noise (2.55.0): the white noise sources a device declares for `.noise`, made
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
//! - **Continuous.** Straight lines between samples have no jumps, so no breakpoints are needed
//!   and trapezoidal integration sees a piecewise-linear input, which it integrates exactly.
//! - **Follows the operating point.** `S` is evaluated at the solution being tried, so shot
//!   noise follows the current that produces it (cyclostationary noise). The dependence of the
//!   amplitude on the unknowns is left out of the Jacobian: Newton converges to the same point,
//!   by a slightly inexact step. That term is the noise amplitude's own sensitivity, which is
//!   small against the device's conductance wherever the device is well-conditioned.
//!
//! # Limitations
//!
//! - **White sources only.** A flicker or tabulated source has a spectrum that a sequence of
//!   independent samples cannot have; synthesizing one needs a filter with memory, which is not
//!   done here. Such sources contribute **nothing**, and [`excluded_sources`] counts them so
//!   the caller can say so (`va-cli` raises `W0602`). A laser linewidth declared as
//!   `flicker_noise(…, 2)` is among them.
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
}

/// An instance with its white noise sources active in transient (this module's documentation).
///
/// Every other analysis, and everything but `load`, is forwarded unchanged.
pub struct WithTranNoise<'a> {
    inner: &'a dyn ModelInstance,
    id: u64,
    cfg: TranNoiseConfig,
}

impl<'a> WithTranNoise<'a> {
    /// Wrap `inner`. `id` distinguishes this instance's random sequences from every other
    /// instance's: give each instance of a circuit its own (its index is enough).
    pub fn new(inner: &'a dyn ModelInstance, id: u64, cfg: TranNoiseConfig) -> Self {
        WithTranNoise { inner, id, cfg }
    }
}

/// How many flicker and tabulated sources `instances` declare at `x`: the ones transient noise
/// leaves out, per instance index. Empty when there are none.
pub fn excluded_sources(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
) -> Vec<(usize, usize)> {
    struct Count(usize);
    impl NoiseSink for Count {
        fn white_current(&mut self, _p: usize, _n: usize, _psd: f64) {}
        fn flicker_current(&mut self, _p: usize, _n: usize, coeff: f64, _exponent: f64) {
            if coeff != 0.0 {
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
            let mut c = Count(0);
            inst.noise(x, &ctx, &mut c);
            (c.0 > 0).then_some((i, c.0))
        })
        .collect()
}

/// A standard normal deviate for `(seed, instance, source, sample)`, by hashing (SplitMix64's
/// finalizer over the four words) and the Box-Muller transform. Stateless on purpose.
fn normal(seed: u64, id: u64, source: u64, k: u64) -> f64 {
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let h = mix(mix(mix(mix(seed) ^ id) ^ source) ^ k);
    let h2 = mix(h);
    // (0, 1]: the logarithm's argument is never zero.
    let u1 = ((h >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
    let u2 = (h2 >> 11) as f64 / (1u64 << 53) as f64;
    libm::sqrt(-2.0 * libm::log(u1)) * libm::cos(2.0 * std::f64::consts::PI * u2)
}

/// Adds each white source's current to the residual as the instance reports it.
struct Inject<'s> {
    sink: &'s mut dyn StampSink,
    cfg: TranNoiseConfig,
    id: u64,
    k: u64,
    frac: f64,
    source: u64,
}

impl NoiseSink for Inject<'_> {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        let source = self.source;
        self.source += 1;
        if !(psd.is_finite() && psd > 0.0) {
            return;
        }
        let g0 = normal(self.cfg.seed, self.id, source, self.k);
        let g1 = normal(self.cfg.seed, self.id, source, self.k + 1);
        let i = libm::sqrt(psd * self.cfg.fmax) * (g0 + (g1 - g0) * self.frac);
        self.sink.residual(p, i);
        self.sink.residual(n, -i);
    }

    // Counted, so that a later white source keeps its own sequence whatever comes before it.
    fn flicker_current(&mut self, _p: usize, _n: usize, _coeff: f64, _exponent: f64) {
        self.source += 1;
    }

    fn table_current(&mut self, _p: usize, _n: usize, _points: &[(f64, f64)], _i: TableInterp) {
        self.source += 1;
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
        let dt = 0.5 / self.cfg.fmax;
        let s = (ctx.time / dt).max(0.0);
        let k = libm::floor(s);
        let mut inject = Inject {
            sink,
            cfg: self.cfg,
            id: self.id,
            k: k as u64,
            frac: s - k,
            source: 0,
        };
        // The densities are what a noise analysis would read, at this time and this solution.
        let noise_ctx = AnalysisCtx {
            kind: AnalysisKind::Noise,
            ..*ctx
        };
        self.inner.noise(x, &noise_ctx, &mut inject);
        if inject.source > 0 {
            inject.sink.bound_step(dt);
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
    }

    fn events_with_state(
        &self,
        x: &[f64],
        ctx: &AnalysisCtx,
        state: &[f64],
        sink: &mut dyn EventSink,
    ) {
        self.inner.events_with_state(x, ctx, state, sink);
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
                    let noisy = WithTranNoise::new(&r, 0, TranNoiseConfig { fmax: 20e9, seed });
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
