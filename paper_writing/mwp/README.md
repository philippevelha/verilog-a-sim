# mwp — a Verilog-A component library for microwave photonics

Fifteen Verilog-A modules (`lib/mwp_*.va`) that put an optical carrier, its modulation and its
detection into the same netlist as the RF electronics around them, and eleven validation tests
(`tests/t01…t11`) that run typical microwave-photonics applications in verilog-a-sim and compare
them with closed forms. It is the component library of the paper outlined in
`../microwavephotonics.txt` (sections 3, 4 and 6).

`RESULTS.md` is the generated record of the last full run: **158 of 158 checks pass** over 13
tests (2.56.0+1, `python tests/run_all.py`, 346 s on the development laptop; t12 is the Spectre gate,
which reports itself skipped where Spectre's outputs are absent).

## The formalism: envelope + carrier tag

| Thing | Representation |
|---|---|
| Optical port | two electrical nodes `x_re`, `x_im`: the complex envelope `A = re + j·im` of `E(t) = A(t)·exp(+j2πνt)`, in **√mW** (`re² + im²` is mW) |
| Carrier tag | one electrical node `nu`, **1 V = 1 THz**: the reference frequency ν of that envelope. Driven by the source, read by parts whose response depends on optical frequency (the ring) |
| Light at another frequency, same bus | a rotating envelope (`mwp_laser df=…`): two lasers `df` apart beat on a detector |
| Light on another tag (WDM) | another bus. Powers add on separate detectors wired in parallel; there is no beat term between buses |
| Direction | one-way: an output is an ideal source, an input only probes. No reflections |
| Polarisation | one |

Electrical disciplines only, on purpose: the same source should compile in any Verilog-A
simulator. Verified in **Cadence Spectre 24.10** (2026-11-12): all fifteen modules compile and run
as written, with Cadence's own `disciplines.vams`; not yet tried in OpenVAF/ngspice or Xyce.

Validity: the envelope band must hold everything that matters. A time step resolves the
fastest envelope feature (an RF tone, a beat, a pulse), not the optical cycle.

## Components

| Module | Ports | What it models | Noise |
|---|---|---|---|
| `mwp_laser` | `o_re o_im nu` | CW laser: power, offset `df` from the tag, phase | RIN (white), linewidth (1/f² phase noise) |
| `mwp_mll` | `o_re o_im nu` | mode-locked laser: Gaussian pulse train, per-pulse timing jitter (`$rdist_normal`) | — |
| `mwp_pm` | `i o rf_p rf_n` | phase modulator: Vπ, loss, one-pole bandwidth, termination | termination thermal |
| `mwp_mzm` | `i o ob rf_p rf_n bias` | Mach-Zehnder modulator, both outputs: Vπ (RF and bias), chirp `alpha`, extinction ratio, bandwidth | termination thermal |
| `mwp_iqm` | `i o vi vq bi bq bp` | IQ / dual-parallel MZM with its three biases | — |
| `mwp_coupler` | `a b c d` | 2×2 coupler, any ratio (1×2 splitter with `b` grounded) | — |
| `mwp_switch` | `a b c d ctl` | 2×2 switch / tunable coupler | — |
| `mwp_phase` | `i o ctl` | phase shifter and attenuator | — |
| `mwp_delay` | `i o` | true time delay (`absdelay`), separate optical-phase parameter | — |
| `mwp_fiber` | `i o` | fibre span: loss, group delay, dispersion **between carriers** | — |
| `mwp_ring` | `i t d nu tune` | microring, all-pass or add-drop, coupled-mode theory, tunable | — |
| `mwp_pd` | `i a c` | photodiode: responsivity, saturation, bandwidth, dark current | shot |
| `mwp_bpd` | `p n out ref` | balanced detector with finite common-mode rejection | shot |
| `mwp_oamp` | `i o` | EDFA/SOA: saturable gain with recovery time | ASE (both quadratures) |
| `mwp_noise` | `p n` | white current noise generated in the time domain by the model itself (`$rdist_normal`), fixed density | the same density in `.noise` |

`i`, `o`, `a`… are optical buses (two nodes each). Every file starts with its equations and
its limitations. Dispersion **inside** the envelope band (RF fading) is built as a chain of
`mwp_ring` all-pass sections fitted by `tests/fit_dispersion.py` — a rational approximation
with a stated band.

## A link in six lines

```
X1 a_re a_im nu                         mwp_laser p_mw=10 rin=3.16e-16
Vb vb gnd DC 2
X2 a_re a_im b_re b_im x_re x_im rf gnd vb  mwp_mzm vpi=4 vpi_dc=4 f3db=20e9
Vk k gnd DC 3
X3 b_re b_im an k                       mwp_pd resp=0.8 f3db=30e9
RL an gnd 50
```

```
va-cli sim link.net --model lib            # operating point
va-cli sim link.net --model lib --ac       # with  Vrf rf gnd DC 0 AC 1  and an .ac card
va-cli sim link.net --model lib --noise    # with  .noise V(an) Vrf dec 10 1e8 1e10
va-cli sim link.net --model lib --tran     # with  Vrf rf gnd SIN(0 0.2 1e9)  and a .tran card
```

## Validation: application → test

| Application | Test | Compared with | Agreement |
|---|---|---|---|
| IMDD link: gain, IP3, noise figure, SFDR | `t01` | cos² curve; two-pole response; Bessel J1·J0, J1·J2; shot + RIN + thermal | DC 6e-8; AC 9e-5; fundamental 3e-8, IMD3 2e-6 (rel); noise 2e-7 |
| RF processing (filter) | `t02` | ring Lorentzian; PM → notch ring → PD response from the ring's transfer function | spectrum 5e-8; RF response 7e-5; transient vs AC 8e-5 |
| RF carriers over a wide range | `t03` | beat `R√(P1P2)` at 1, 10, 40, 100 GHz; ×2/×4 `R·P·J2k(m)`; discriminator `(RP)²·4sin²(πfτ)·Δν/(πf²)` | amplitude < 1e-6, frequency < 3e-8; harmonics < 1e-6; noise 7e-6 |
| Ultra-stable carriers: OEO | `t04` | Yao–Maleki loop equation solved for (f, V) | frequency 5e-5; amplitude 0.2–0.3 %; side modes 4e-5  Phase noise is a semi-analytic estimate, not simulated: −104 dBc/Hz at 100 kHz for the 6.5 ns loop |
| Wideband AM/PM signals | `t05` | 16-QAM at 20 GHz, exact symbols; 4 GHz LFM chirp, closed-form current | EVM 9e-5 (print resolution); chirp 4e-5, rate 8e-5 |
| Optical sampling | `t06` | pulse-averaged modulator transfer; `J3/J1`; `1/(2πfσ)²` | charge 1.2e-5; HD3 1e-6; jitter SNR 43.1 dB vs 43.7 (256 samples) |
| Processing optical samples | `t06` | difference/sum + arcsin; 1:2 demux to two slow detectors | SINAD 22.8 → 63.1 dB; interleaved record = ideal samples |
| Beamforming, true-time delay | `t07` | `−2πfτ_n`, `τ_n = D·L·Δλ_n`; squint `asin(sinθ0·f0/f)`; 3-bit switched line; ring `2·atan(2πfτ)` | delays to 1e-16 s; TTD beam fixed at −29.95° over 8–20 GHz, phase-shifter beam moves 31° |
| Mixing | `t08` | series `(RP/2)J1J1`, carrier-suppressed parallel `RP·J1J1` | IF 9e-5 / 3e-5; advantage 10.98 dB vs 10.98 |
| Dispersion in the band | `t09` | chain's exact response; fibre `cos(πλ²DLf²/c)`, 25 km | 8 sections: 3e-5 up to 16 GHz |
| Optical amplification | `t10` | saturation law; signal-spontaneous beat + shot | gain 4e-4 dB; noise 3e-7 |
| Noise in the time domain, two ways | `t11` | kT/C on one RC and on a 50-section ladder; spectrum; shot noise following a stepped optical power | variance within 1–6 % (statistical); see below |

What "validated" means here: **against closed forms** (t01–t11), and since 2026-11-12 **against
Cadence Spectre**, adopted as the golden reference for this library (`CLAUDE.md` §7). Spectre's
outputs sit in `golden/spectre/mwp/` on the machines that hold them (not distributed: Cadence
NDA; `t12` reports itself skipped without them); `t12_spectre_golden.py` runs the same circuits
here and compares, 16 checks, all passing: DC 6e-8, AC 9e-5 (the print resolution of the phase),
noise 1e-6 and 1e-5, two-tone fundamental and IMD3 3e-8 and 2e-6, filter and beamformer 9e-5,
a delay at the noise frequency 2e-5; against Spectre's harmonic balance, the multiplier's
harmonics 4e-8 and 4e-7 and the mixer's IF 2.5e-5; against its autonomous harmonic balance, the
oscillator's frequency 5.5e-5 and amplitude 4.2e-3. The netlists, the run script and the
Spectre-side comparisons with the closed forms are in `spectre/`. Not measurements.

Two results are quoted from the literature and **not** reproduced: the 23.7 dB conversion-
efficiency improvement measured by Minasian, Chan and Yi (Opt. Express 21, 22918, 2013 —
`references/microwavephotonics.pdf`) depends on drive levels the paper does not give; this
model gives 10.98 dB at an LO drive of 0.375 Vπ, equal to its own closed form. Their
SBS-based filters and mixer are not modelled at all (no Brillouin gain element).

## What the work found in the simulator

1. **`.noise` ignored frequency-dependent models** (fixed, 2.53.1). The noise analysis
   linearised once, with no frequency, so an `absdelay` entered every point as a wire and a
   `laplace_*` filter with its DC gain. The linewidth discriminator of `t03` returned exactly
   the shot-noise floor. AC was right; noise was silently wrong.
2. **`absdelay` under-resolves a fast signal in a long delay** (knob added, 2.54.0). The
   history is 512 samples, so a 5 ns delay stores one sample per 9.8 ps: a 10 GHz tone comes
   out 3.3 % low (measured; the interpolation bound is (ωh)²/12 = 3.2 %).
   `VA_ABSDELAY_POINTS=4096` gives 0.12 %, at two to four times the run time. A
   kilometre of fibre carrying a 10 GHz subcarrier is out of reach either way (10⁵ RF periods
   in the line): remove the bulk delay (`mwp_fiber td0=`) when only differences matter.
3. **Print resolution.** va-cli prints time with 7 significant digits and voltages with 6
   decimals. At 30 ns that is 1e-15 s, 1.3e-4 rad of a 20 GHz carrier, which is the floor of
   several checks above; a 2 ps pulse cannot be integrated from the printed samples at 256 ns,
   so `t06` integrates in the circuit (a capacitor). Currents are read through 0 V sources for
   dynamic range. Not changed here.
4. **Junction heuristic.** A module with `exp()` of a variable has all its electrical nodes
   treated as junctions, and their Newton step limited to ~0.23 V: the tag node (193.4 "V")
   then needs 840 iterations and the operating point fails. `mwp_mll` writes its Gaussian as
   `pow(4, …)` for that reason. A consequence of carrying non-electrical quantities on
   electrical nodes; a dedicated discipline would avoid it at the price of portability.

## Noise in the time domain: library module against simulator feature

Two ways to get noise into a transient run, compared in `t11` on the same circuits:

| | `mwp_noise` (library) | `va-cli --tran-noise <fmax>` (simulator, 2.55.0) |
|---|---|---|
| What generates it | the model: `$rdist_normal` on a timer | the integrator: every white source a device declares for `.noise` |
| Netlist | one instance per source, density given by hand | nothing to add |
| Follows the operating point | no (density is a parameter) | yes: shot noise tracks the current (ratio 9.4 measured for a current ratio of 9) |
| Portable to another simulator | yes, if it has `$rdist_normal` and `@(timer)` | no |
| kT/C, one RC, 4 µs | −5.5 % | −1.2 % |
| Time, 1 source / 50 sources | 10.6 s / 118 s | 4.2 s / 39 s |
| Peak memory, 1 / 50 sources | 60 MB / 97 MB | 45 MB / 28 MB |
| Time points, 1 / 50 sources | 1.00 M / 0.44 M | 0.72 M / 0.40 M |

The statistical uncertainty of the variances is about 3 % (one RC) and larger on the ladder.
The module costs more because each instance is a device with a timer event that the integrator
must land on and re-solve; the simulator's version is a hash per source per evaluation and
keeps no state. A fixed-density module set for the high level of a stepped optical power
gives 4.6 times the noise the low level has. Both produce the spectrum `S·sinc⁴(f/2fmax)`
and both leave out flicker noise (the simulator warns, `W0602`). With one step per noise
sample, a pole at `fc` keeps about `1/(1 + π·fc/(2·fmax))` of its noise variance: take
`fmax ≥ 40·fc`.

## Limits of the library

- Noise in the time domain is white sources only (below), one realization per run. There is no
  periodic noise analysis: phase noise of the OEO close to the carrier, and the noise figure of
  a mixer or sampler, are not computed (`t04` reports Yao and Maleki's formula fed with
  simulated noise density and amplitude, labelled an estimate).
- No harmonic balance: intermodulation is a transient plus a least-squares tone fit
  (`mwp.tones`), accurate here because the nonlinear parts are memoryless.
- The ring is a single resonance; the modulators are lumped (one pole; `cel` gives the electrode's
  capacitance, 2.56.0+2); the photodiode has no junction diode (`cj` gives a fixed junction
  capacitance, 2.56.0+2); the amplifier has no ASE power, no ASE–ASE beat and no chirp; the fibre has no
  Kerr effect, no SBS, no PMD.
- The trapezoidal rule warps a pole by (ωh)²/12: `t08` needs 160 steps per RF period for 5e-4.
- A coupler does not check that its two inputs carry the same tag.

## Open: the oscillator's phase noise

Noted 2026-11-12 and left open by decision. Three estimates of the opto-electronic oscillator's
phase noise (the 40 GHz-detector circuit of `spectre/23_oeo_hb_noise.scs`) disagree:

| Source | L(10 MHz) | Status |
|---|---|---|
| Spectre, autonomous harmonic balance + `hbnoise` | −132.3 dBc/Hz (−92.4 at 100 kHz) | 13 dB above the formula; equals the formula with the tank's group delay alone, to 0.01 dB (load-resistor contribution) |
| verilog-a-sim, `--tran-noise`, 2 µs | −137.8 dBc/Hz (numerical floor −146 removed) | 5–8 dB above the formula at 10–20 MHz; floor from the delay line's resampling |
| Yao–Maleki, `δ/(2(2πfτ)²)`, simulated noise and amplitude | −145.3 dBc/Hz | a textbook model, not a simulation |

What was ruled out: a mis-scaled RIN source in transient noise (its variance is within 0.7 %);
Spectre ignoring the delay at the noise frequency (`spectre/29_hbnoise_delay.scs`: `hbnoise`
applies `absdelay` correctly, to 2e-5). What is left is an inference, not tested: Spectre's
oscillator phase noise comes from its periodic adjoint, whose normalization, for a circuit with a
delay, needs the delay's memory; leaving it out would scale the result by
`((τ_tank+τ_delay)/τ_tank)² = 21`, the 13.2 dB observed. The test that would settle it: the same
oscillator with the delay made of lumped LC sections. The transient's own excess is unexplained.

Consequences: Spectre is **not** a reference for this quantity (its other OEO results are); no
phase-noise number for the OEO goes into the paper as validated; step 4 of
`docs/proposals/harmonic-balance.md` needs another gate for its oscillator case.

## Not done

- **OpenVAF/ngspice and Xyce.** Spectre runs the library as written; these two are untried.
  `mwp_delay`/`mwp_fiber` use `absdelay`, `mwp_mll` and `mwp_noise` use `@(timer)` and
  `$rdist_normal`, and `$bound_step` is used in three models.
- In Spectre's harmonic balance, `$abstime` is invalid: `mwp_laser` with a frequency offset
  (`df != 0`) and `mwp_mll` cannot be used there (a warning; with `df = 0` the term is zero).
- Parameter extraction from measured data (skeleton §4), the Hilbert transformer and the
  DPMZM phase shifter exactly as in Minasian et al. Fig. 1 (the test uses the same principle
  with an IQ modulator and a separate carrier path).

## Files

```
lib/        mwp_*.va, mwp.vams (conventions and constants), disciplines.vams, constants.vams
tests/      mwp.py            harness: run va-cli, parse, tone fit, report
            fit_dispersion.py all-pass chain design for in-band dispersion
            t01 … t10         one application each; writes tNN_*.json
            t11               noise in the time domain: library module against --tran-noise
            t12               verilog-a-sim against Spectre's golden outputs (golden/spectre/mwp)
            t13               electrical loads: modulator electrode admittance, photodiode junction pole
spectre/    Spectre netlists, run.sh, psf.py, compare_spectre.py, results notes (README)
            run_all.py        everything, then RESULTS.md
            make_figures.py   twelve figures, at least one per test, re-run from the same circuits (about 5 minutes)
figures/    fig_*.pdf and .png: transparent, no titles, 3.5 pt lines, 20 pt text, every curve with
            its own line style and marker (readable in black and white)
RESULTS.md  generated
```

`VACLI=/path/to/va-cli python tests/run_all.py` to use another binary; Python needs numpy and
scipy.
