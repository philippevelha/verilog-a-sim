//! T5.2 — noise analysis: per-device noise sources propagated to an output PSD via the adjoint.
//!
//! # The adjoint method, and why it is worth the indirection
//!
//! Every device contributes uncorrelated white current sources across its own branches
//! (`va_abi::noise`, Interface β's noise channel). Because they are uncorrelated, the output
//! noise PSD is a **sum of powers**, each weighted by the squared magnitude of the transfer
//! impedance from that source's own branch to the output:
//!
//! ```text
//! S_out(ω) = Σ_k |Z_k(jω)|² · S_k        where  Z_k = ∂V(out) / ∂I_k
//! ```
//!
//! The direct way to get every `Z_k` is one linear solve per source per frequency: inject a unit
//! current across source `k`'s branch, read `V(out)`. For `K` sources that is `K` solves at every
//! frequency point.
//!
//! The adjoint gets all `K` from **one** solve. Writing the small-signal system as `A·X = B` with
//! `A = G + jω·C`, injecting a unit current across branch `(p, n)` means `B = e_p − e_n`, and the
//! output is `V(out) = e_outᵀ·A⁻¹·(e_p − e_n)`. Define the adjoint vector `y` by
//!
//! ```text
//! Aᵀ · y = e_out
//! ```
//!
//! Then `e_outᵀ·A⁻¹ = (A⁻ᵀ·e_out)ᵀ = yᵀ`, so `Z_k = yᵀ·(e_p − e_n) = y_p − y_n` — a *subtraction*
//! per source once `y` is known. One solve per frequency, regardless of how many noise sources
//! the circuit has.
//!
//! Note this is the **plain** transpose, not the conjugate transpose: the identity above needs
//! `(A⁻¹)ᵀ`, and no conjugation enters because nothing here is an inner product over a complex
//! space — it is a bilinear identity. (The overall sign of `Z_k` is likewise immaterial: only
//! `|Z_k|²` reaches the answer, which is why this module doesn't have to reconcile the residual
//! channel's own current-injection sign convention.)
//!
//! # Input-referred noise, for free
//!
//! The adjoint vector turns out to answer a second question at no extra cost. Referring output
//! noise back to an input means dividing by the squared gain from that input:
//!
//! ```text
//! S_in(ω) = S_out(ω) / |H(jω)|²        where  H = V(out) / V(input source)
//! ```
//!
//! and `H` is *already* a component of `y`. An ideal voltage source of AC magnitude 1 excites
//! the system with `B = e_k` at its own branch-current row `k` (§ `va_cli::solve_ac` — a
//! source's stimulus is purely an RHS term), so
//!
//! ```text
//! H = e_outᵀ · A⁻¹ · e_k = yᵀ · e_k = y_k
//! ```
//!
//! — the adjoint vector read at the input source's branch row. The same one solve per frequency
//! that gives every noise source its transfer impedance also gives the forward gain, so
//! input-referral costs one extra division per point rather than a second linear solve.
//!
//! # Per-device attribution
//!
//! [`NoiseSpectrum::per_instance`] breaks the total down by *which device produced it* — the
//! answer to "where is my noise coming from?", and the one a designer actually acts on.
//!
//! Device identity does not come from Interface β: a [`ModelInstance`] has no name, and
//! [`va_abi::NoiseSink`] receives only `(p, n, psd)`. It comes from **position** instead. This
//! module calls `inst.noise(..)` over `instances` in order, so it knows which instance emitted
//! each source and tags it with that index; the caller, which built the instance list, maps the
//! index back to a device name (`va_cli::noise_contributors`). No ABI change was needed, and the
//! attribution is exact rather than inferred from topology — two identical resistors in
//! parallel stay distinguishable, which a `(p, n)`-keyed grouping could never manage.
//!
//! Attribution is **per device, not per mechanism**: a diode contributing both shot and flicker
//! noise reports one combined figure. QSPICE additionally splits its own `onoise_d1` into
//! `onoise_d1.id`/`onoise_d1.1overf`/`onoise_d1.rs`; reproducing that split would mean naming
//! each model's internal call sites, which this project has no representation for.
//!
//! # Limitations
//!
//! - **White and flicker sources only, and only what a model declares** — inherited from
//!   Interface β's noise channel; see `va_abi::noise`'s own module doc.
//! - **No per-mechanism split within a device**, as above.

use crate::ac::{linearize, solve_block_embedded, AcSweep, Complex};
use crate::AcNoiseError;
use va_abi::noise::{NoiseSink, NoiseSource, TableInterp, TEMP_NOMINAL};
use va_abi::ModelInstance;

/// Output noise power spectral density over frequency, and — when an input source was named —
/// the same noise referred back to that input.
#[derive(Clone, Debug, Default)]
pub struct NoiseSpectrum {
    /// Frequency points (Hz).
    pub f: Vec<f64>,
    /// Output noise PSD at each frequency, V²/Hz — a **one-sided** density, matching both the
    /// convention `va_abi::noise`'s source PSDs are stated in and QSPICE's own
    /// `onoise_spectrum` output.
    pub psd: Vec<f64>,
    /// Total integrated noise over the swept band, as an **RMS voltage** (V) — i.e.
    /// `sqrt(∫ psd df)`, the quantity QSPICE prints as "Total integrated output-referenced
    /// noise". See [`integrate_rms`] for the quadrature used and its accuracy caveat.
    pub total: f64,
    /// Input-referred noise PSD at each frequency, V²/Hz (`psd / |H|²`) — matching QSPICE's own
    /// `inoise_spectrum`. **Empty** when [`run`] was given no input source, which is the honest
    /// representation of "this question wasn't asked" rather than a column of zeros.
    ///
    /// A frequency at which the input has no path to the output (`H = 0`) reports
    /// [`f64::INFINITY`]: referring output noise to an input that cannot reach the output is
    /// genuinely undefined, and a zero there would read as "no noise", which is the opposite of
    /// the truth.
    pub input_psd: Vec<f64>,
    /// [`Self::input_psd`] integrated to an RMS voltage, as [`Self::total`] is for the output.
    /// `0.0` when no input source was given. Non-finite points are skipped by the quadrature
    /// (see [`integrate_rms`]).
    pub input_total: f64,
    /// Per-device output-noise breakdown: `(index into the `instances` slice, that instance's
    /// own contribution over [`Self::f`])`, ascending by index and listing **only** instances
    /// that emitted at least one source.
    ///
    /// Summing these across instances reproduces [`Self::psd`] exactly — they are the same
    /// per-source terms, bucketed rather than accumulated straight into the total.
    ///
    /// The index is positional because that is the only identity available at this layer (§ this
    /// module's doc comment); a caller that built the instance list maps it back to a device
    /// name.
    pub per_instance: Vec<(usize, Vec<f64>)>,
}

/// Collects noise sources as `(p, n, source)` while each instance emits them.
///
/// A source is kept as its **shape** ([`NoiseSource`]) rather than a single number, because a
/// flicker source's PSD depends on frequency: the sweep evaluates `source.psd_at(f)` per point,
/// while the transfer impedance `Z` it is weighted by comes from that point's own adjoint solve.
#[derive(Default)]
struct SourceList {
    /// `(emitting instance index, p, n, source)`.
    sources: Vec<(usize, usize, usize, NoiseSource)>,
    /// The instance currently being polled — stamped onto every source it emits, which is how
    /// per-device attribution happens without the ABI carrying any identity (§ this module's
    /// doc comment).
    current: usize,
}

impl SourceList {
    /// Record a source unless it is identically powerless at every frequency.
    ///
    /// A zero-power source contributes nothing to any output and would only cost a subtraction
    /// per frequency — drop it here rather than in the hot loop. (A reverse-biased junction at
    /// exactly zero current, or a model with a zero `KF`, both land here.) A device whose every
    /// source is dropped this way simply doesn't appear in the per-device breakdown, which is
    /// the right answer: it contributes nothing.
    fn push(&mut self, p: usize, n: usize, source: NoiseSource) {
        if !source.is_powerless() {
            self.sources.push((self.current, p, n, source));
        }
    }

    /// The distinct instance indices that emitted at least one source, ascending.
    fn contributors(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self.sources.iter().map(|&(i, ..)| i).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

impl NoiseSink for SourceList {
    fn white_current(&mut self, p: usize, n: usize, psd: f64) {
        self.push(p, n, NoiseSource::White { psd });
    }

    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {
        self.push(p, n, NoiseSource::Flicker { coeff, exponent });
    }

    fn table_current(&mut self, p: usize, n: usize, points: &[(f64, f64)], interp: TableInterp) {
        self.push(p, n, NoiseSource::table(points.to_vec(), interp));
    }
}

/// Read `y` at a global unknown index, treating any index at or past the system dimension
/// (notably `va_abi::reference::GROUND`) as the 0 V reference — the same ground-folding
/// convention the stamping channels use.
fn at(y: &[Complex], i: usize) -> Complex {
    y.get(i).copied().unwrap_or((0.0, 0.0))
}

/// Compute the noise spectrum about DC point `x_dc` over `sweep`, with the output taken at
/// global unknown index `output`.
///
/// `instances` and `x_dc` are the same linearization inputs [`crate::ac::run`] takes; `temp` is
/// the simulation temperature in kelvin that thermal sources are evaluated at (pass
/// [`va_abi::noise::TEMP_NOMINAL`] for the project's nominal 300.15 K). One adjoint solve is
/// performed per frequency point (this module's own doc comment derives why one suffices).
///
/// `input` optionally names an AC source's **branch-current row**, which adds the
/// input-referred spectrum ([`NoiseSpectrum::input_psd`]) at the cost of one division per
/// frequency — see this module's doc comment for why no second solve is needed. Pass `None` to
/// compute the output-referred spectrum alone.
///
/// A circuit whose devices report no noise sources at all is not an error — it yields an
/// identically zero spectrum, which is the correct answer for, say, an ideal-source-and-
/// capacitor network.
///
/// # Errors
///
/// [`AcNoiseError::InvalidOutput`] if `output` (or `input`) is not an unknown of this system;
/// [`AcNoiseError::Core`] from the underlying adjoint solve (one per frequency point).
pub fn run(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    dim: usize,
    sweep: AcSweep,
    output: usize,
    input: Option<usize>,
    temp: f64,
) -> Result<NoiseSpectrum, AcNoiseError> {
    if output >= dim {
        return Err(AcNoiseError::InvalidOutput { index: output, dim });
    }
    if let Some(k) = input {
        if k >= dim {
            return Err(AcNoiseError::InvalidOutput { index: k, dim });
        }
    }

    // A noise run linearizes under its *own* analysis kind, not AC's: a compiled model whose
    // source distinguishes `analysis("noise")` from `analysis("ac")` must see the difference.
    // `ac_stim` is deliberately ignored here — an applied stimulus is not a noise source, and
    // this analysis's excitation is the adjoint probe at the output, built below.
    let ctx = va_abi::AnalysisCtx::noise().with_temp(temp);
    let lin = linearize(instances, x_dc, &ctx, dim);
    let (g, c) = (lin.g, lin.c);
    let mut collected = SourceList::default();
    for (i, inst) in instances.iter().enumerate() {
        collected.current = i;
        inst.noise(x_dc, &ctx, &mut collected);
    }
    let contributors = collected.contributors();
    // Position within `contributors` for each instance index, so the hot loop can bucket by a
    // direct array index rather than searching per source.
    let mut bucket_of = vec![usize::MAX; instances.len()];
    for (slot, &id) in contributors.iter().enumerate() {
        bucket_of[id] = slot;
    }

    // The adjoint right-hand side: a unit "probe" at the output unknown, the one thing that
    // makes `y` specific to this output. Real-valued, so its imaginary half is zero.
    let mut e_out = vec![(0.0, 0.0); dim];
    e_out[output] = (1.0, 0.0);

    let f = sweep.frequencies();
    let mut psd = Vec::with_capacity(f.len());
    let mut input_psd = Vec::with_capacity(if input.is_some() { f.len() } else { 0 });
    let mut per_instance: Vec<Vec<f64>> = vec![Vec::with_capacity(f.len()); contributors.len()];
    for &freq in &f {
        let omega = 2.0 * std::f64::consts::PI * freq;
        let y = solve_block_embedded(&g, &c, dim, omega, &e_out, true)?;

        let mut buckets = vec![0.0; contributors.len()];
        for (id, p, n, source) in &collected.sources {
            let (id, p, n) = (*id, *p, *n);
            let (pre, pim) = at(&y, p);
            let (nre, nim) = at(&y, n);
            // Z_k = y_p - y_n; the contribution is |Z_k|² · S_k(f). The transfer impedance is
            // frequency-dependent through the adjoint solve, and `S_k` is too for a flicker
            // source — hence `psd_at(freq)` rather than a precomputed number.
            let (zre, zim) = (pre - nre, pim - nim);
            buckets[bucket_of[id]] += (zre * zre + zim * zim) * source.psd_at(freq);
        }
        // The total is the sum of the per-device buckets, computed once rather than
        // independently — so the breakdown and the total cannot drift apart.
        let total: f64 = buckets.iter().sum();
        for (slot, value) in buckets.into_iter().enumerate() {
            per_instance[slot].push(value);
        }
        psd.push(total);

        // The forward gain from the input source, read straight out of the adjoint vector at
        // that source's own branch row (this module's doc comment derives `H = y_k`).
        if let Some(k) = input {
            let (hre, him) = at(&y, k);
            let gain_sq = hre * hre + him * him;
            input_psd.push(if gain_sq > 0.0 {
                total / gain_sq
            } else {
                f64::INFINITY
            });
        }
    }

    let total = integrate_rms(&f, &psd);
    let input_total = integrate_rms(&f, &input_psd);
    Ok(NoiseSpectrum {
        f,
        psd,
        total,
        input_psd,
        input_total,
        per_instance: contributors.into_iter().zip(per_instance).collect(),
    })
}

/// Convenience wrapper over [`run`] at the project's nominal temperature
/// ([`va_abi::noise::TEMP_NOMINAL`], 300.15 K).
///
/// # Errors
///
/// As [`run`].
pub fn run_at_nominal_temp(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    dim: usize,
    sweep: AcSweep,
    output: usize,
    input: Option<usize>,
) -> Result<NoiseSpectrum, AcNoiseError> {
    run(instances, x_dc, dim, sweep, output, input, TEMP_NOMINAL)
}

/// Integrate a PSD over its frequency points and return the RMS value `sqrt(∫ psd df)`.
///
/// Trapezoidal quadrature in **linear** frequency, over points that are (for a `dec` sweep)
/// logarithmically spaced. That is exact for a flat spectrum — which is what makes it checkable
/// against QSPICE's own printed total for a resistive circuit — and progressively coarse for a
/// steeply rolling-off one, since the widest trapezoids sit in the top decade where a low-pass
/// response is smallest. It is a summary statistic, not the gated quantity: `docs/validation.md`
/// compares the *spectrum* against golden, point by point.
///
/// Returns `0.0` for fewer than two points (nothing to integrate over), which is also the
/// answer for an empty spectrum — the case [`NoiseSpectrum::input_psd`] is in when no input
/// source was named.
///
/// A trapezoid touching a non-finite sample is **skipped** rather than propagating `inf`/`NaN`
/// through the whole integral: an input-referred spectrum reports `f64::INFINITY` at any
/// frequency where the input cannot reach the output, and one such point would otherwise make
/// the entire band's total meaningless. Skipping understates the total there, which the caller
/// can see for itself in the per-frequency `input_psd`.
fn integrate_rms(f: &[f64], psd: &[f64]) -> f64 {
    if f.len() < 2 || psd.len() < 2 {
        return 0.0;
    }
    let power: f64 = f
        .windows(2)
        .zip(psd.windows(2))
        .filter(|(_, pw)| pw[0].is_finite() && pw[1].is_finite())
        .map(|(fw, pw)| 0.5 * (pw[0] + pw[1]) * (fw[1] - fw[0]))
        .sum();
    power.max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::noise::{BOLTZMANN, ELEMENTARY_CHARGE};
    use va_abi::reference::{diode::VT_NOMINAL, Capacitor, Diode, Resistor, VSource, GROUND};

    fn flat_sweep() -> AcSweep {
        AcSweep {
            fstart: 1.0,
            fstop: 1e6,
            points_per_decade: 10,
        }
    }

    /// The textbook result every noise analysis must reproduce: a lone resistor to ground,
    /// probed at its own node, has output voltage-noise PSD `4kTR`.
    ///
    /// This is the end-to-end check that the adjoint transfer impedance is right, not just the
    /// source PSD: the source is `4kT/R` A²/Hz and the transfer impedance is `R`, so the `R`
    /// dependence *inverts* between the two — getting either wrong gives `4kT/R` or `4kTR³`,
    /// not `4kTR`.
    #[test]
    fn lone_resistor_output_psd_is_4ktr() {
        let r = 1000.0;
        let res = Resistor::new(0, GROUND, r);
        let insts: [&dyn ModelInstance; 1] = [&res];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");

        let expected = 4.0 * BOLTZMANN * TEMP_NOMINAL * r;
        assert!(!spectrum.psd.is_empty());
        for (&f, &p) in spectrum.f.iter().zip(&spectrum.psd) {
            assert!(
                (p - expected).abs() < 1e-24,
                "f={f}: psd = {p}, expected {expected}"
            );
        }
    }

    /// Two resistors in parallel: the sources add in power and the transfer impedance is the
    /// parallel combination, so the result is `4kT·(R1∥R2)` — the same `4kTR` law applied to the
    /// resistance actually seen at the node. Confirms sources genuinely *sum* rather than the
    /// last one winning.
    #[test]
    fn parallel_resistors_sum_in_power() {
        let (r1, r2) = (1000.0, 3000.0);
        let a = Resistor::new(0, GROUND, r1);
        let b = Resistor::new(0, GROUND, r2);
        let insts: [&dyn ModelInstance; 2] = [&a, &b];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");

        let r_par = r1 * r2 / (r1 + r2);
        let expected = 4.0 * BOLTZMANN * TEMP_NOMINAL * r_par;
        assert!(
            (spectrum.psd[0] - expected).abs() < 1e-24,
            "psd = {}, expected {expected}",
            spectrum.psd[0]
        );
    }

    /// An RC low-pass shapes the resistor's own thermal noise: the PSD rolls off as
    /// `4kTR / (1 + (ωRC)²)`, the squared magnitude of the same transfer function
    /// `circuits/rc_ac.net` checks in the AC domain. This is the test that exercises the charge
    /// channel's contribution to the adjoint solve — with `C` ignored the spectrum would be flat.
    #[test]
    fn rc_lowpass_shapes_thermal_noise() {
        let (r, cap) = (1000.0, 1e-6);
        let res = Resistor::new(0, GROUND, r);
        let capacitor = Capacitor::new(0, GROUND, cap);
        let insts: [&dyn ModelInstance; 2] = [&res, &capacitor];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");

        let s0 = 4.0 * BOLTZMANN * TEMP_NOMINAL * r;
        for (&f, &p) in spectrum.f.iter().zip(&spectrum.psd) {
            let wrc = 2.0 * std::f64::consts::PI * f * r * cap;
            let expected = s0 / (1.0 + wrc * wrc);
            assert!(
                (p - expected).abs() <= 1e-9 * expected.max(1e-30),
                "f={f}: psd = {p}, expected {expected}"
            );
        }

        // And it really is shaped, not flat — the top of the band is orders down from the bottom.
        let last = *spectrum.psd.last().unwrap();
        assert!(last < spectrum.psd[0] * 1e-6, "not rolled off: {last}");
    }

    /// The circuit `circuits/diode_noise.net` drives, checked against the hand-derived physics
    /// before any golden file exists: a forward-biased diode fed through a series resistor, both
    /// contributing at the output node, with transfer impedance `R ∥ rd` for each.
    #[test]
    fn resistor_plus_diode_matches_hand_derivation() {
        let (r, vsrc) = (1000.0, 0.7);
        // Unknowns: 0 = in, 1 = a, 2 = V1's branch current.
        let v1 = VSource::new(0, GROUND, 2, vsrc);
        let res = Resistor::new(0, 1, r);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&v1, &res, &d];

        // Solve the real DC operating point rather than assuming one.
        let op = va_core::dc::operating_point(&insts, 3, va_core::newton::NewtonConfig::default())
            .expect("DC bias");
        let vd = op.x[1];
        let id = d.current(vd);
        let gd = d.conductance(vd);
        let z = 1.0 / (1.0 / r + gd); // R ∥ rd, the impedance seen at node `a`

        let spectrum =
            run_at_nominal_temp(&insts, &op.x, 3, flat_sweep(), 1, None).expect("noise solves");

        let thermal = 4.0 * BOLTZMANN * TEMP_NOMINAL / r;
        let shot = 2.0 * ELEMENTARY_CHARGE * id;
        let expected = (thermal + shot) * z * z;
        for &p in &spectrum.psd {
            assert!(
                (p / expected - 1.0).abs() < 1e-9,
                "psd = {p}, expected {expected}"
            );
        }
        // The diode dominates here (2qId vs 4kT/R at this bias) — asserted so the test would
        // notice if the shot source silently went missing, which a total-only check might not.
        assert!(
            shot > thermal,
            "expected the diode to dominate: shot={shot}, thermal={thermal}"
        );
    }

    /// A flat spectrum integrates exactly under trapezoidal quadrature, so `total` is checkable
    /// in closed form: `sqrt(S · Δf)`.
    #[test]
    fn total_is_the_rms_of_the_integrated_psd() {
        let r = 1000.0;
        let res = Resistor::new(0, GROUND, r);
        let insts: [&dyn ModelInstance; 1] = [&res];
        let sweep = flat_sweep();
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, sweep, 0, None).expect("noise solves");

        let s = 4.0 * BOLTZMANN * TEMP_NOMINAL * r;
        let expected = (s * (sweep.fstop - sweep.fstart)).sqrt();
        assert!(
            (spectrum.total / expected - 1.0).abs() < 1e-9,
            "total = {}, expected {expected}",
            spectrum.total
        );
    }

    /// An ideal-source-and-capacitor circuit has no noise mechanism at all — a zero spectrum is
    /// the right answer, not an error.
    #[test]
    fn a_noiseless_circuit_yields_a_zero_spectrum() {
        let v1 = VSource::new(0, GROUND, 1, 5.0);
        let cap = Capacitor::new(0, GROUND, 1e-6);
        let insts: [&dyn ModelInstance; 2] = [&v1, &cap];
        let spectrum = run_at_nominal_temp(&insts, &[5.0, 0.0], 2, flat_sweep(), 0, None)
            .expect("noise solves");
        assert!(spectrum.psd.iter().all(|&p| p == 0.0), "{:?}", spectrum.psd);
        assert_eq!(spectrum.total, 0.0);
    }

    /// A flicker source must fall a decade in power per decade of frequency, *through the whole
    /// analysis* — not merely in `NoiseSource::psd_at`. A hand-built instance emits one directly,
    /// so this tests the sweep's per-frequency evaluation rather than any particular device.
    #[test]
    fn a_flicker_source_rolls_off_as_one_over_f() {
        struct FlickerOnly {
            terminals: [usize; 2],
        }
        impl ModelInstance for FlickerOnly {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn load(
                &self,
                _x: &[f64],
                _ctx: &va_abi::AnalysisCtx,
                sink: &mut dyn va_abi::StampSink,
            ) {
                // A 1 S conductance to ground, so the transfer impedance is exactly 1 Ω and the
                // output PSD is the source PSD unchanged — isolating the frequency shape.
                sink.jacobian(0, 0, 1.0);
            }
            fn noise(&self, _x: &[f64], _ctx: &va_abi::AnalysisCtx, sink: &mut dyn NoiseSink) {
                sink.flicker_current(0, GROUND, 1e-19, 1.0);
            }
        }

        let dev = FlickerOnly {
            terminals: [0, GROUND],
        };
        let insts: [&dyn ModelInstance; 1] = [&dev];
        let sweep = AcSweep {
            fstart: 10.0,
            fstop: 1e4,
            points_per_decade: 1,
        };
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, sweep, 0, None).expect("noise solves");

        // 10, 100, 1k, 10k Hz -> 1e-20, 1e-21, 1e-22, 1e-23 V²/Hz.
        assert_eq!(spectrum.f.len(), 4);
        for (&f, &p) in spectrum.f.iter().zip(&spectrum.psd) {
            let expected = 1e-19 / f;
            assert!(
                (p / expected - 1.0).abs() < 1e-9,
                "f={f}: psd = {p}, expected {expected}"
            );
        }
    }

    /// A tabulated source must be interpolated *by the analysis*, per frequency — the same bar
    /// the flicker test above sets. The table here is deliberately shaped and read at
    /// frequencies that are all strictly *between* its points, so a "take the nearest point" or
    /// "take the first power" implementation lands nowhere near the expected values.
    #[test]
    fn a_tabulated_source_is_interpolated_across_the_whole_sweep() {
        struct TableOnly {
            terminals: [usize; 2],
        }
        impl ModelInstance for TableOnly {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn load(
                &self,
                _x: &[f64],
                _ctx: &va_abi::AnalysisCtx,
                sink: &mut dyn va_abi::StampSink,
            ) {
                // 1 S to ground: transfer impedance exactly 1 Ω, so the output PSD *is* the
                // source PSD and the frequency shape is what's under test.
                sink.jacobian(0, 0, 1.0);
            }
            fn noise(&self, _x: &[f64], _ctx: &va_abi::AnalysisCtx, sink: &mut dyn NoiseSink) {
                sink.table_current(
                    0,
                    GROUND,
                    &[(1.0, 1e-20), (1e3, 5e-20), (1e5, 1e-20)],
                    TableInterp::Linear,
                );
            }
        }

        let dev = TableOnly {
            terminals: [0, GROUND],
        };
        let insts: [&dyn ModelInstance; 1] = [&dev];
        let sweep = AcSweep {
            fstart: 10.0,
            fstop: 1e4,
            points_per_decade: 1,
        };
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, sweep, 0, None).expect("noise solves");

        assert_eq!(spectrum.f.len(), 4); // 10, 100, 1k, 10k Hz
        for (&f, &p) in spectrum.f.iter().zip(&spectrum.psd) {
            // The same piecewise-linear rule, written out independently here rather than by
            // calling `table_psd_at` — otherwise this would only assert the analysis calls the
            // helper, not that the helper is right.
            let expected = if f <= 1e3 {
                1e-20 + (5e-20 - 1e-20) * (f - 1.0) / (1e3 - 1.0)
            } else {
                5e-20 + (1e-20 - 5e-20) * (f - 1e3) / (1e5 - 1e3)
            };
            assert!(
                (p / expected - 1.0).abs() < 1e-9,
                "f={f}: psd = {p}, expected {expected}"
            );
        }
        // The rising and falling segments really do rise and fall — a flat implementation
        // returning one constant would pass none of this, but state it outright anyway.
        assert!(spectrum.psd[0] < spectrum.psd[2], "{:?}", spectrum.psd);
        assert!(spectrum.psd[3] < spectrum.psd[2], "{:?}", spectrum.psd);
    }

    /// White and flicker sources on the same branch add in power, and their sum crosses over:
    /// flicker dominates at low frequency, white at high. This is the shape every real
    /// `1/f`-plus-thermal device has, and it fails if either channel is dropped.
    #[test]
    fn white_and_flicker_sum_with_a_crossover() {
        struct Both {
            terminals: [usize; 2],
        }
        impl ModelInstance for Both {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn load(
                &self,
                _x: &[f64],
                _ctx: &va_abi::AnalysisCtx,
                sink: &mut dyn va_abi::StampSink,
            ) {
                sink.jacobian(0, 0, 1.0);
            }
            fn noise(&self, _x: &[f64], _ctx: &va_abi::AnalysisCtx, sink: &mut dyn NoiseSink) {
                sink.white_current(0, GROUND, 1e-22);
                sink.flicker_current(0, GROUND, 1e-19, 1.0);
            }
        }
        let dev = Both {
            terminals: [0, GROUND],
        };
        let insts: [&dyn ModelInstance; 1] = [&dev];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");

        for (&f, &p) in spectrum.f.iter().zip(&spectrum.psd) {
            let expected = 1e-22 + 1e-19 / f;
            assert!(
                (p / expected - 1.0).abs() < 1e-9,
                "f={f}: psd = {p}, expected {expected}"
            );
        }
        // The corner is at f = 1e-19/1e-22 = 1000 Hz: flicker-dominated below, white above.
        let first = spectrum.psd[0];
        let last = *spectrum.psd.last().unwrap();
        assert!(first > 10.0 * 1e-22, "low end should be flicker-dominated");
        assert!(
            (last / 1e-22 - 1.0).abs() < 1e-3,
            "high end should be white-dominated: {last}"
        );
    }

    /// The per-device breakdown must attribute each share to the right device *and* sum back to
    /// the total. Two parallel resistors of different value at the same node give unequal,
    /// individually-known shares: each contributes `4kT/R · Z²` with the same `Z = R1∥R2`, so
    /// the 1 kΩ contributes three times what the 3 kΩ does.
    #[test]
    fn per_device_shares_are_attributed_and_sum_to_the_total() {
        let (r1, r2) = (1000.0, 3000.0);
        let a = Resistor::new(0, GROUND, r1);
        let b = Resistor::new(0, GROUND, r2);
        let insts: [&dyn ModelInstance; 2] = [&a, &b];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");

        assert_eq!(spectrum.per_instance.len(), 2);
        assert_eq!(spectrum.per_instance[0].0, 0, "instance 0 is the 1 kΩ");
        assert_eq!(spectrum.per_instance[1].0, 1, "instance 1 is the 3 kΩ");

        let z = 1.0 / (1.0 / r1 + 1.0 / r2);
        for (idx, r) in [(0usize, r1), (1usize, r2)] {
            let expected = 4.0 * BOLTZMANN * TEMP_NOMINAL / r * z * z;
            let got = spectrum.per_instance[idx].1[0];
            assert!(
                (got / expected - 1.0).abs() < 1e-9,
                "instance {idx}: {got}, expected {expected}"
            );
        }
        // The smaller resistor is the noisier one, by exactly R2/R1 = 3.
        let ratio = spectrum.per_instance[0].1[0] / spectrum.per_instance[1].1[0];
        assert!((ratio - 3.0).abs() < 1e-9, "ratio = {ratio}");

        // And the shares reconstruct the total at every frequency.
        for (i, &t) in spectrum.psd.iter().enumerate() {
            let sum: f64 = spectrum.per_instance.iter().map(|(_, s)| s[i]).sum();
            assert!((sum / t - 1.0).abs() < 1e-12, "sum {sum} vs total {t}");
        }
    }

    /// Two **identical** resistors stay separate entries. This is the case a `(p, n)`-keyed
    /// grouping could never handle — both sources sit across the same branch with the same PSD —
    /// and it is why attribution is positional (§ this module's doc comment).
    #[test]
    fn identical_devices_remain_distinguishable() {
        let a = Resistor::new(0, GROUND, 1000.0);
        let b = Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 2] = [&a, &b];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");
        assert_eq!(spectrum.per_instance.len(), 2, "must not merge into one");
        assert_eq!(
            spectrum.per_instance[0].1[0], spectrum.per_instance[1].1[0],
            "identical devices contribute equally"
        );
    }

    /// A noiseless device is absent from the breakdown rather than present with zeros — the
    /// breakdown lists contributors, and `VSource`/`Capacitor` are not among them.
    #[test]
    fn noiseless_devices_are_omitted_from_the_breakdown() {
        // Unknowns: 0 = in, 1 = out, 2 = V1's branch current. Only the resistor is noisy.
        let v1 = VSource::new(0, GROUND, 2, 1.0);
        let res = Resistor::new(0, 1, 1000.0);
        let cap = Capacitor::new(1, GROUND, 1e-9);
        let insts: [&dyn ModelInstance; 3] = [&v1, &res, &cap];
        let spectrum = run_at_nominal_temp(&insts, &[1.0, 1.0, 0.0], 3, flat_sweep(), 1, None)
            .expect("noise solves");
        assert_eq!(spectrum.per_instance.len(), 1);
        assert_eq!(
            spectrum.per_instance[0].0, 1,
            "the resistor is instance 1, and is the only contributor"
        );
    }

    /// A device carrying *both* a white and a flicker source reports them as one combined
    /// figure — attribution is per device, not per mechanism (§ this module's doc comment).
    #[test]
    fn one_device_with_two_mechanisms_reports_one_combined_share() {
        struct Both {
            terminals: [usize; 2],
        }
        impl ModelInstance for Both {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn load(
                &self,
                _x: &[f64],
                _ctx: &va_abi::AnalysisCtx,
                sink: &mut dyn va_abi::StampSink,
            ) {
                sink.jacobian(0, 0, 1.0);
            }
            fn noise(&self, _x: &[f64], _ctx: &va_abi::AnalysisCtx, sink: &mut dyn NoiseSink) {
                sink.white_current(0, GROUND, 1e-22);
                sink.flicker_current(0, GROUND, 1e-19, 1.0);
            }
        }
        let dev = Both {
            terminals: [0, GROUND],
        };
        let insts: [&dyn ModelInstance; 1] = [&dev];
        let spectrum =
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, None).expect("noise solves");
        assert_eq!(spectrum.per_instance.len(), 1, "one device, one entry");
        // Its single entry is the sum of both mechanisms (Z = 1 Ω here).
        for (i, &f) in spectrum.f.iter().enumerate() {
            let expected = 1e-22 + 1e-19 / f;
            let got = spectrum.per_instance[0].1[i];
            assert!((got / expected - 1.0).abs() < 1e-9, "f={f}: {got}");
        }
    }

    /// Input-referral against a closed form: a resistive divider from the source to the output
    /// has gain `H = R2/(R1+R2)`, so the input-referred noise is the output noise divided by
    /// `H²` — a *larger* number, since referring noise back through an attenuating network
    /// magnifies it.
    ///
    /// The gain is read out of the adjoint vector rather than solved for separately (this
    /// module's doc comment derives `H = y_k`), so this also verifies that identity holds
    /// against an independently-known value.
    #[test]
    fn input_referral_divides_by_the_squared_forward_gain() {
        let (r1, r2) = (1000.0, 3000.0);
        // Unknowns: 0 = in, 1 = a (output), 2 = V1's branch current.
        let v1 = VSource::new(0, GROUND, 2, 1.0);
        let top = Resistor::new(0, 1, r1);
        let bot = Resistor::new(1, GROUND, r2);
        let insts: [&dyn ModelInstance; 3] = [&v1, &top, &bot];

        let spectrum = run_at_nominal_temp(&insts, &[1.0, 0.75, 0.0], 3, flat_sweep(), 1, Some(2))
            .expect("noise solves");

        let h = r2 / (r1 + r2); // 0.75
        assert_eq!(spectrum.input_psd.len(), spectrum.psd.len());
        for (&out, &inp) in spectrum.psd.iter().zip(&spectrum.input_psd) {
            let expected = out / (h * h);
            assert!(
                (inp / expected - 1.0).abs() < 1e-9,
                "input-referred {inp}, expected {expected}"
            );
            // Referring back through an attenuator magnifies: 1/0.75² ≈ 1.78×.
            assert!(inp > out);
        }

        // The output spectrum itself is unchanged by asking the extra question.
        let alone = run_at_nominal_temp(&insts, &[1.0, 0.75, 0.0], 3, flat_sweep(), 1, None)
            .expect("noise solves");
        assert_eq!(alone.psd, spectrum.psd);
        assert!(
            alone.input_psd.is_empty() && alone.input_total == 0.0,
            "no input source named -> no input-referred column, not a column of zeros"
        );
    }

    /// With no attenuation between input and output (`H = 1`), input- and output-referred
    /// spectra coincide — the degenerate case that would hide a wrong `H` in the test above if
    /// it were the only one.
    #[test]
    fn unity_gain_makes_input_and_output_referral_agree() {
        // A single resistor from the source to an otherwise-open output node: no current flows,
        // so V(out) = V(in) exactly (H = 1), yet the node is *not* the source-forced one, so it
        // has a real impedance (R, looking back through the resistor into the source's own zero
        // impedance) and a real 4kTR of noise. Probing the forced node instead would give an
        // output PSD of exactly zero — correct, since an ideal source shorts it, but useless as
        // a ratio test.
        let r = 1000.0;
        // Unknowns: 0 = in (forced), 1 = out, 2 = V1's branch current.
        let v1 = VSource::new(0, GROUND, 2, 1.0);
        let res = Resistor::new(0, 1, r);
        let insts: [&dyn ModelInstance; 2] = [&v1, &res];
        let spectrum = run_at_nominal_temp(&insts, &[1.0, 1.0, 0.0], 3, flat_sweep(), 1, Some(2))
            .expect("solves");

        let expected = 4.0 * BOLTZMANN * TEMP_NOMINAL * r;
        assert!(
            (spectrum.psd[0] - expected).abs() < 1e-24,
            "output psd {} , expected 4kTR = {expected}",
            spectrum.psd[0]
        );
        for (&out, &inp) in spectrum.psd.iter().zip(&spectrum.input_psd) {
            assert!((inp / out - 1.0).abs() < 1e-9, "out {out}, in {inp}");
        }
        assert!((spectrum.input_total / spectrum.total - 1.0).abs() < 1e-9);
    }

    /// A frequency where the input cannot reach the output has undefined input-referred noise.
    /// Reporting `0` there would read as "no noise" — the opposite of the truth — so it is
    /// `INFINITY`, and the integrated total skips it rather than becoming `NaN`.
    #[test]
    fn a_zero_gain_point_is_infinite_and_does_not_poison_the_total() {
        // The source's branch row is not connected to the output node at all: an isolated noisy
        // resistor at node 0, and a source across node 1, which node 0 never sees.
        let r = Resistor::new(0, GROUND, 1000.0);
        let v1 = VSource::new(1, GROUND, 2, 1.0);
        let insts: [&dyn ModelInstance; 2] = [&r, &v1];
        let spectrum = run_at_nominal_temp(&insts, &[0.0, 0.0, 0.0], 3, flat_sweep(), 0, Some(2))
            .expect("solves");

        assert!(
            spectrum.input_psd.iter().all(|p| p.is_infinite()),
            "expected an undefined input referral, got {:?}",
            &spectrum.input_psd[..3]
        );
        assert!(
            spectrum.input_total.is_finite(),
            "one undefined point must not make the whole total NaN: {}",
            spectrum.input_total
        );
        // The output spectrum is still the ordinary 4kTR.
        let expected = 4.0 * BOLTZMANN * TEMP_NOMINAL * 1000.0;
        assert!((spectrum.psd[0] - expected).abs() < 1e-24);
    }

    #[test]
    fn an_input_outside_the_system_is_an_error() {
        let res = Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 1] = [&res];
        assert!(matches!(
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 0, Some(7)),
            Err(AcNoiseError::InvalidOutput { index: 7, dim: 1 })
        ));
    }

    #[test]
    fn an_output_outside_the_system_is_an_error() {
        let res = Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 1] = [&res];
        assert!(matches!(
            run_at_nominal_temp(&insts, &[0.0], 1, flat_sweep(), 5, None),
            Err(AcNoiseError::InvalidOutput { index: 5, dim: 1 })
        ));
    }

    /// Thermal noise scales linearly with temperature — checked through the whole analysis, not
    /// just the source formula, since `temp` has to actually reach the instances.
    #[test]
    fn temperature_reaches_the_sources() {
        let res = Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 1] = [&res];
        let cold = run(&insts, &[0.0], 1, flat_sweep(), 0, None, 150.0).expect("solves");
        let hot = run(&insts, &[0.0], 1, flat_sweep(), 0, None, 300.0).expect("solves");
        assert!(
            (hot.psd[0] / cold.psd[0] - 2.0).abs() < 1e-9,
            "hot/cold = {}",
            hot.psd[0] / cold.psd[0]
        );
    }
}
