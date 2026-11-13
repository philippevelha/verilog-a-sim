# Proposal: harmonic balance with periodic noise

**Status:** proposed, 2026-11-12. **Not ratified, nothing built.** Every effort figure below is
an estimate, not a measurement; step 1 exists to replace the first of them with one.
**Affects:** a new analysis (a new crate, or `va-acnoise`), `va-netlist` (a card), `va-cli`
(wiring, report), `va-diag` (codes). Steps 2 and 4 need an Interface β change (§6 of
`CLAUDE.md`). `va-core` is used as it is.
**Outside the ratified scope.** `CLAUDE.md` §1 lists the analyses as DC, transient, AC and
noise. This adds a fifth and a sixth. It needs the same decision the sparse solve and BDF2 did.
**Evidence for the need:** `paper_writing/mwp` (2.54.0+1 .. 2.55.0+1), see §1.

## 1. What cannot be done today

Three things the microwave-photonics library ran into, each with the number it cost:

| Need | What we do instead | What that costs |
|---|---|---|
| Oscillator phase noise (the OEO, `t04`) | a formula fed with simulated numbers (Yao and Maleki), labelled an estimate | no simulated spectrum at all; `--tran-noise` (2.55.0) would need about 1e9 steps for a 1 kHz offset |
| Intermodulation, mixer conversion (`t01`, `t08`) | a transient over the beat period and a least-squares tone fit | right, because the nonlinear parts are memoryless; the run length grows with the ratio of the tones (80 MHz IF under 10 GHz: 14 000 points per run, and 160 steps per RF period once a pole is in the path) |
| Noise figure of a mixer or a sampler | nothing | `.noise` linearizes about a DC point; a circuit pumped by an LO or a pulse train has none that means anything |
| A loop with a long delay (1 km of fibre) | not simulable | `absdelay` keeps a sampled history: 1e5 RF periods do not fit (2.54.0) |

Harmonic balance answers all four: it solves directly for the periodic steady state as a set
of harmonics, a delay is an exact phase per harmonic whatever its length, and the noise analysis
linearized about that solution gives sidebands, conversion and, for an oscillator, phase noise.

**Why not shooting**, which `docs/future_development.md` §7 calls the cheaper of the two. It
is, for a circuit whose state is a handful of capacitor voltages: it reuses the integrator and
adds a Newton loop over the initial state. The circuit that motivates this proposal has a delay
line in its loop, and a delay's state is its whole history (512 to 4096 samples per call site
here), so the shooting unknowns and their sensitivity matrix grow with the delay; and the long
delay that shooting would have to integrate through is exactly what the sampled history cannot
hold. In the frequency domain the same delay is one complex number per harmonic. Shooting
remains the better choice for strongly nonlinear circuits without delays (a ring oscillator's
sharp edges need many harmonics); it is not proposed here and is not excluded by this.

## 2. What the method is, in this code base's terms

Unknowns: every circuit unknown at `N = 2K+1` time samples over one period `T` (equivalently
its harmonics `-K..K`). The equations, at sample `k`:

```text
  i(x_k, t_k)  +  sum_j D_kj · q(x_j, t_j)  +  (linear, frequency-domain part)_k  =  0
```

`i` and `q` are exactly what a device already reports through `StampSink::residual` and
`StampSink::charge`, with `jacobian` and `dcharge` for Newton. `D` is the spectral
differentiation matrix (`jkω` per harmonic). So for a device that is a memoryless function of
its terminals plus whole charge terms, **`ModelInstance::load` is already the device
interface harmonic balance needs**: call it at each `(x_k, t_k)` with a static context and
collect the two channels. A source that depends on `$abstime` (a `SIN`, the rotating envelope
of `mwp_laser`, the pulse train of `mwp_mll`) is evaluated at `t_k` and needs nothing new.

What does **not** fit, and why:

1. **A `ddt` inside an expression** (`lower::Lowered::evaluates_ddt`). The model reconstructs
   the rate from `AnalysisCtx::ddt_coeff` and its own previous value: a time-marching
   construct. Here the rate of an inner quantity is `D` applied to that quantity's samples,
   which one evaluation at one sample cannot see.
2. **Everything on the state channel**: `transition`, `slew`, `idt`, `laplace_*` and `zi_*` in
   their transient form, `absdelay`'s history, analog events, persistent variables,
   `$rdist_*`. Each is defined by what happened at earlier time points of a march.
3. **Frequency-domain terms** (`absdelay`, `laplace_*`). In a static context they stamp their
   zero-frequency gain; in an `Ac` context, their complex gain at one frequency on a
   linearization. Harmonic balance needs the gain at every harmonic `kω`, applied to the
   *large-signal* argument.

## 3. Steps

Each step ends at a gate. There is no QSPICE oracle for any of this; Spectre is one since
2026-11-12 (§5). Originally the gates were
closed forms the `mwp` tests already hold, and agreement with a long transient.

### Step 0 — ratify the scope and the home (a decision, no code)

- Is a fifth analysis in scope? (`CLAUDE.md` §1.)
- Where does it live? A new crate `va-hb` depending on `va-core` and `va-abi`, or inside
  `va-acnoise` (T5), which already owns the complex small-signal solve the noise half reuses.
- What is the oracle? (§5.)

### Step 1 — driven, single-tone, memoryless-plus-charge devices

Time-sample (collocation) formulation; one fundamental given on a `.hb <f0> <harmonics>` card;
initial guess the DC operating point plus the AC response at `f0`; Newton on the full
`N·dim` system with a direct solve (the block structure is dense in the harmonic index and
sparse in the circuit index: for the circuits at hand, 30–60 unknowns and 15–31 harmonics,
that is a few thousand real unknowns, within `va_core::sparse`'s reach). A source ramp as the
convergence aid. Report: harmonics of the requested quantities, and the waveform.

Models with anything from §2's list are **refused**, by name and construct (new `R` codes):
the refusal reads the lowered IR, as the transient ones do.

- Pure Rust, no new dependency: a direct DFT is enough at these sizes.
- Gate: `t03`'s multiplier harmonics `R·P·J_2k(m)` and `t01`'s single-tone compression against
  their Bessel forms to 1e-6; the diode rectifier and `rc_ac` against a long transient's
  steady state.
- Estimate: 8–10 working days. Main risk: Newton robustness on strongly driven junctions
  without the transient's natural continuation.

### Step 2 — linear frequency-domain terms: delays and filters (Interface β)

A delay or a `laplace_*` filter whose argument is **linear** in the unknowns (every
`mwp_delay`/`mwp_fiber`, and the usual filter on a port voltage) contributes `H(jkω)` times
that argument at harmonic `k`. `va-codegen` already stamps exactly that gain for `Ac` and,
since 2.53.1, `Noise`. Two things are missing: a context that tells the model *"omit this
term from the time samples, the analysis supplies it per harmonic"*, and a check that the
argument is linear. The first is a new `AnalysisKind` (or a field): a §6 change. A nonlinear
argument stays refused; lifting that needs a two-pass operator protocol (the model reports the
argument's samples, the analysis returns the filtered ones), which is a larger §6 change and
is not proposed here.

- This is the step that makes a long fibre loop possible: the delay costs nothing per harmonic.
- Gate: `t07`'s element phases, `t02`'s filter, the discriminator of `t03` driven by a tone.
- Estimate: 5 days, plus the interface discussion.

### Step 3 — autonomous circuits: the oscillator

The frequency becomes an unknown and one phase is pinned. The difficulty is not the extra
row: the non-oscillating solution always satisfies the equations, and Newton prefers it. The
standard cure is a probe (a source at the oscillation frequency whose current is driven to
zero) or starting from a transient that has already built up. Both are needed in practice.

- Gate: the OEO's frequency and amplitude against the loop equation (`t04`: 5e-5 and 0.3 %
  from transient today), with a 5 ns loop and then a 5 µs one; `ring_osc` against its
  transient.
- Estimate: 6–8 days. This is the least predictable step.

### Step 4 — periodic small-signal and periodic noise (Interface β, small)

Linearize about the periodic solution: the Jacobian of step 1 evaluated at an offset `f`
is the conversion matrix between sidebands `kf0 + f`. One adjoint solve per offset gives the
transfer from every noise source at every sideband to the output, as `va_acnoise::noise` does
for one frequency. The sources are cyclostationary: `ModelInstance::noise` is called at each
time sample, so a shot source's density follows the current over the period (the same
evaluation `--tran-noise` makes), and its harmonics give the correlation between sidebands.
Flicker and tabulated sources enter at each sideband's own frequency, so up-conversion of
flicker noise is included; that is what transient noise cannot do.

For an oscillator the output is split into amplitude and phase parts, and the phase part is
`L(f)`. The linearization is singular at `f = 0` (the free phase): the result is valid above
the linewidth corner and says so.

- Interface: the noise context needs the sample time and a way to say which sideband; a field.
- Gate: the mixer of `t08` against its closed-form noise figure; the OEO's `L(f)` against
  Yao and Maleki's `δ/(2πfτ)²`, which `t04` already computes from simulated quantities
  (−104.0 dBc/Hz at 100 kHz for the 6.5 ns loop); an LC oscillator against Leeson.
- Estimate: 8–10 days.

### Step 5 — two tones (quasi-periodic)

Intermodulation and mixers with unrelated frequencies need a two-dimensional harmonic set
(box or diamond truncation) and a mapping to an artificial single period. Independent of
steps 3 and 4.

- Gate: `t01`'s IMD3 `R·P·J1·J2` and intercept; `t08`'s IF amplitudes and feed-through.
- Estimate: 6–8 days.

### Step 6 — size (only if a circuit asks for it)

A direct solve of the full system stops scaling at a few hundred unknowns times a few tens
of harmonics. Beyond that: matrix-free GMRES with a per-harmonic block preconditioner. Not
needed for anything in `paper_writing/mwp`; listed so the direct solve of step 1 is not
mistaken for the design's limit.

## 4. Order, dependencies, total

```text
0 ──► 1 ──► 2 ──► 3 ──► 4          (the OEO phase-noise path)
        └──► 5                      (intermodulation, mixers; any time after 1)
```

Steps 0–4, the path to a simulated OEO phase-noise spectrum: about 27–33 working days by the
estimates above. With step 5: 33–41. These are one person's estimates before any code exists;
step 1 is the calibration.

The earliest point with something a user can use is the end of step 1 (harmonics of a driven
nonlinear circuit without a transient). The earliest point that answers the question this
proposal started from is the end of step 4.

## 5. What has to be decided, and what could stop it

1. **Scope** (step 0). This is a new analysis family, not a refinement.
2. **The oracle.** *Decided 2026-11-12: Cadence Spectre (golden/spectre/), whose harmonic balance
   and periodic noise reproduced the multiplier, the mixer IF and the mixer's periodic noise to
   1e-13 .. 1.4e-4. Not for an oscillator's phase noise with a delay in its loop, which is open.*
   Before that: `CLAUDE.md` §5: no analysis result is trusted until it is checked against a
   committed golden output, and the oracle is QSPICE, which has neither harmonic balance nor
   periodic noise. Options: (a) closed forms plus agreement with this project's own transient,
   stated as showcase status; (b) a second oracle for this analysis only (Xyce has harmonic
   balance and is open source; not installed, not evaluated here). The rule needs an explicit
   exception or an explicit second oracle; it should not be bent quietly.
3. **Interface β**, twice (steps 2 and 4). Both are additions (a context kind or field), not
   reshapes; both still go through §6.
4. **Coverage of compact models.** Any model that evaluates a `ddt` inside an expression or
   keeps state is refused. How much of `external/code` that excludes has not been counted; it
   should be, before step 1 is called done, and the count belongs in the release entry.
5. **Convergence.** Harmonic balance has no time axis to walk along. A circuit that the
   transient integrates without trouble can fail here, and the autonomous case (step 3) is the
   known hard one.

## 6. What this does not give

- Transients, start-up, lock acquisition: still the integrator's job.
- Chaos and non-periodic behaviour (`ringchaos`, the bubble examples): outside the method.
- Anything for a model with events or random draws (`mwp_mll` with jitter, `mwp_noise`): they
  are refused; their periodic part can be modelled without the event.
- Phase noise close to the carrier, inside the oscillator's own linewidth: the linearization
  of step 4 diverges there. A phase-domain (Demir) treatment would be a further step.
