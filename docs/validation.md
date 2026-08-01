# Validation & the Model Zoo

Reference simulator: **QSPICE** (originally ngspice; switched 2026-07-13 to match the actual
dev environment) — an oracle only; we are not building on it. `va-harness` runs the pipeline
and compares to committed `golden/` outputs.

## Metrics & default tolerances

| Analysis     | Metric                                              | Default tolerance |
|--------------|-----------------------------------------------------|-------------------|
| DC           | max relative I–V error on the operating point/sweep | ≤ 1e-4            |
| Transient    | waveform RMS error (after shared-timebase resample) | ≤ 1e-3            |
| AC           | max relative magnitude error / max absolute phase error | ≤ 1e-4 · ≤ 1e-4 rad |
| Noise        | max relative error on the output, input-referred, *and* per-device PSD | ≤ 1e-3 |
| Convergence  | fraction of zoo circuits that reach a solution      | track upward      |

These mirror the constants in `va-harness` (`tol::DC_REL`, `tol::TRAN_RMS`, `tol::AC_MAG_REL`,
`tol::AC_PHASE_RAD`, `tol::NOISE_PSD_REL`). Tune here as the zoo grows; record any change with
its justification.

**Updated 2026-08-01: the noise row is new** (T5.2). Its band is `1e-3`, looser than DC's `1e-4`
deliberately rather than by drift: a noise PSD is a *derived* quantity two levels removed from
the operating point — `Σ|Z|²·S`, where the transfer impedance `Z` carries the AC path's error
squared and each source PSD carries the DC bias's error through `2q·Id`. Demanding `1e-4` of a
product of squares of quantities each held to `1e-4` would be asking for better agreement than
the inputs have. Measured: `1.7e-5`.

**Updated 2026-08-01: all four metrics are now real, golden-gated implementations** — the AC row
above is no longer "band-dependent"/unwired. Its previously-stated band is now two concrete
numbers checked by `cargo xtask validate`: max relative error on the response **magnitude**
(`1e-4`, the same band as DC — an AC solve reuses the very Jacobian a DC solve assembles, so
there is no reason to accept a looser one) and max absolute error on its **phase** (`1e-4` rad
≈ `0.0057°`). They are reported and enforced separately (`va_harness::ac::AcVerdict`): a
magnitude that tracks golden while the phase drifts is a different bug (a wrong reactive/charge
stamp) from the reverse (a wrong conductance), and collapsing them into one number would hide
which happened.

**Updated 2026-07-18: three of the four metrics are real, verified implementations now,** not
`todo!()` stubs:

- **DC** (`va_harness::metrics::max_relative_error`) and **transient** (`rms_error`, plus the
  `resample_linear` shared-timebase step two independent adaptive-timestep integrators need) —
  see `docs/roadmap.md`'s T6.3 section.
- **Convergence** — `xtask::validate` now tracks a circuit's own solver failure as a distinct,
  tracked outcome (not folded into "failed golden comparison," and no longer aborting the whole
  validation run before the rest of the zoo is even attempted) — see `docs/roadmap.md`'s T6.4
  section and `t6-integration/04-convergence-dashboard.qmd`.

**Updated 2026-07-18: `GoldenDc`/`GoldenSweep`/`GoldenTran` now carry named branch currents, not
just node voltages** (`va_cli::branch_currents`, § `va_harness::golden`'s own doc comment) — see
`docs/roadmap.md`'s T6.3 section for the full story. This closed rung 2's last stated scope
limit: `circuits/diode_iv.net`'s golden comparison used to check only `V(in)`, which trivially
matches regardless of whether the diode model is right at all (it's directly forced by `V1`).
The golden file now also carries `I(V1)`, which by KCL equals the diode's own current — a real
Shockley-law cross-check against QSPICE, not just plumbing. Widening the golden format surfaced a
genuine gap in `max_relative_error`'s own near-zero floor (`1e-12` was too tight once femtoamp-
scale branch currents entered the comparison — QSPICE's and this project's own solver-noise
floors disagree at that scale by construction, not because either model is wrong); the floor is
now `1e-8` (`va_harness::metrics::REL_ERROR_FLOOR`'s own doc comment has the full empirical
derivation).

`golden/*.golden` — all eleven — are real, QSPICE-generated data (`cargo xtask gen-golden`):
`{divider, mos_dc, diode_iv, rc_step, rectifier, ring_osc, rc_ac, diode_ac, diode_noise,
resistor_noise_va, diode_flicker}`. Every one of `xtask`'s known circuits has a committed golden
reference, closing the "which circuits aren't regenerated yet" gap this file used to track.

### The AC gate (added 2026-08-01)

Two circuits, chosen so the pair separates "the complex solve works" from "the model's own
small-signal behavior is right":

- **`circuits/rc_ac.net`** — the same 1 kΩ/1 µF network `rc_step.net` drives in the time domain,
  swept 1 Hz–1 MHz at 10 points/decade. Pure `R`/`C`/`V`, so QSPICE runs it with no model
  translation at all. Measured against golden: **magnitude `1.3e-15`, phase `1.7e-13` rad** —
  machine precision, as it should be for two simulators assembling the identical linear system.
- **`circuits/diode_ac.net`** — a compiled `models/diode.va` forward-biased through a 1 kΩ
  resistor with a 100 nF load, swept 10 Hz–10 MHz. Measured: **magnitude `1.3e-5`, phase
  `6.4e-6` rad**, the same order as `diode_iv.net`'s own DC `6.7e-5` and traceable to the same
  cause (both simulators' diode temperature conventions), not to the AC path.

The second circuit is what gives the gate teeth. Its passband gain is the small-signal divider
`1/(1 + R1·gd)`, and `gd = Is/(N·Vt)·exp(Vd/(N·Vt))` depends *exponentially* on the solved bias:
at the golden's own measured gain of `0.19988`, a mere 1% error in `Vd` would move `gd` by ~46%
and the gain far outside `1e-4`. Agreeing to `1.3e-5` therefore constrains the DC operating
point, `va-codegen`'s AD-derived Jacobian, and the linearization that consumes it, all at once —
`rc_ac.net` alone would only have exercised `R`/`C` stamps.

**Two grids, matched by frequency.** Asked for `.ac dec 10 1 1meg`, QSPICE emits **60** points —
`10^(k/10)` for `k = 0..=58`, then jumps straight to `fstop`, silently dropping `10^5.9 ≈
794.3 kHz` (confirmed empirically against a real run). `va_acnoise::ac::AcSweep::frequencies`
emits the mathematically clean **61**, both endpoints included. Rather than teach this project's
sweep to reproduce QSPICE's off-by-one, `va_harness::ac::compare_ac` aligns the two by frequency
and compares every golden point exactly — no interpolation, since the grids genuinely coincide
wherever they overlap (unlike the transient case, where two adaptive integrators share no
timebase at all and resampling is unavoidable).

**Phase needs two guards the other metrics don't** (`va_harness::metrics::max_phase_error`):
angle differences are wrapped into `(−π, π]`, so a reference sitting on the ±180° branch cut —
`rc_ac.net`'s own `I(V1)` approaches `−180°` at high frequency — doesn't report a ~2π "error"
for a negligible disagreement; and points whose reference magnitude is below `REL_ERROR_FLOOR`
are skipped entirely, since the phase of a value at both simulators' noise floor is arbitrary.

### The noise gate (added 2026-08-01)

One circuit, `circuits/diode_noise.net`: a 0.7 V source feeding a forward-biased diode through a
1 kΩ resistor, probed at their junction, swept 10 Hz–10 MHz. It exercises **both** noise
mechanisms `CLAUDE.md` §7 names, at comparable size so neither can hide the other —
`4kT/R₁ = 1.66e-23` A²/Hz from the resistor and `2q·I_d ≈ 3.3e-23` A²/Hz from the diode, each
reaching the output through the same `Z = R₁ ∥ r_d`. Measured against golden: **`1.7e-5`**, with
the absolute value (`1.9877e-18` V²/Hz, flat) agreeing with QSPICE to five figures and the
band-integrated total (`4.4584 µV` rms) matching QSPICE's own printed figure exactly.

**Three things about this gate are worth knowing before changing it:**

1. **It must not use a `--model` compiled diode.** Verilog-A's `white_noise()`/`flicker_noise()`
   are not lowered by `va-codegen` yet, so a compiled device contributes *no* noise sources
   (`va_abi::noise`'s stated limitation). The deck's `D1` deliberately resolves to the
   hand-written `va-abi::reference::Diode` instead. `va-cli::solve_noise` refuses to report an
   identically-zero spectrum rather than let that failure mode pass as a result, and
   `va-harness`'s own test suite pins that refusal.
2. **The metric is not the DC one.** `metrics::REL_ERROR_FLOOR` is `1e-8`, calibrated for volts
   and milliamps. Applied to a `~2e-18` V²/Hz PSD it would divide every point by the floor and
   report `~1e-10` no matter how wrong the answer — a **vacuous** gate. `max_relative_psd_error`
   floors relative to the spectrum's own peak (`1e-12` of it) instead, and a unit test asserts
   the general metric really would have hidden a doubled spectrum.
3. **The teeth are in the shot term.** Dropping the diode's noise entirely leaves the resistor's
   `6.62e-19`, a 67% error; computing it as `4kTg` instead of `2q·I_d` is off by exactly 2× on
   that term, ~33%. Both are three to four orders outside the `1e-3` band.

### Compiled-model noise: the `white_noise()`/`flicker_noise()` gates (added 2026-08-01b)

The noise gate above uses `va-abi`'s *hand-written* devices, because when it was built a
`va-codegen`-compiled model contributed no noise at all. Lowering Verilog-A's `white_noise()`
and `flicker_noise()` (T1/T2) closed that, and two further circuits gate the result — both
driven through `--model`, so the noise comes from the compiled `.va` and nothing else.

**`circuits/resistor_noise_va.net`** — two resistors (1 kΩ, 3 kΩ) across a 1 V source, probed at
their junction, both resolving to the compiled `models/resistor.va` and its
`white_noise(4*`P_K*$temperature/R)`. The sources add in power through the same `R1∥R2`, giving
the textbook `4kT·750Ω = 1.2432e-17` V²/Hz, flat. Measured against golden: **`1.4e-16`** —
machine precision. (It was exactly `0.0` until the input-referred column joined the comparison;
that column is a division, which costs a few last bits.) The agreement is not a
zero-versus-zero artifact: the golden carries a real `1.24321e-17` at every point, and both
simulators compute it from constants that now agree to the last digit
(`models/constants.vams` takes the exact SI 2019 values, deliberately matching
`va_abi::noise`'s own). Pure `R`/`V`, so QSPICE needs no `.model` translation.

**`circuits/diode_flicker.net`** — the `diode_noise.net` bias network with `D1` resolving to
`models/diode_flicker.va`, which declares both a shot source and
`flicker_noise(KF*|Id|^AF, 1.0)`. Measured: **`1.7e-5`**, the same as the shot-only gate, which
is what one expects when both terms scale with the same solved `Id`.

This is the only **shaped** spectrum in the zoo — `4.156e-16` V²/Hz at 10 Hz falling to
`1.988e-18` at 10 MHz, a factor of **209** across the band, crossing over from flicker-dominated
to the flat shot+thermal floor. That shape is what gives the gate teeth: a white-only
implementation would produce a flat spectrum and be **~99.5% wrong at 10 Hz**, three orders
outside the `1e-3` band. QSPICE's own diode uses exactly the same `KF`/`AF` parameterization
(its `1overf` column steps `4.1365e-16 → e-17 → e-18` per decade, confirmed by probing a real
run), so `models/diode_flicker.va` mirrors it one-to-one and the comparison is meaningful across
the whole band rather than only where flicker is negligible.

`models/diode_flicker.va` is a standalone copy of `diode.va`'s equations plus the flicker term
rather than a parameterization of it, because this project's netlist format has no syntax for
passing device parameters — a `D` line names a model and nothing more — so a nonzero `KF` has to
come from a model file's own defaults. Keeping it separate leaves `diode.va` with the physically
sane `KF = 0` its other three circuits want.

### Input-referred noise (added 2026-08-01c)

Every noise golden file now carries **two** value columns — the output PSD and that same noise
referred back to the `.noise` card's input source, `S_in = S_out / |H|²` — matching QSPICE's own
`onoise_spectrum`/`inoise_spectrum` pair. The header names both ends: `@noise <output> <source>`.

**It costs no extra solve.** The forward gain is already a component of the adjoint vector the
analysis solves for anyway: an ideal source of AC magnitude 1 excites the system at its own
branch row `k`, so `H = e_outᵀ·A⁻¹·e_k = yᵀ·e_k = y_k`. Input-referral is therefore one division
per frequency, not a second linear system. See `t5-acnoise/02-noise.qmd` for the derivation.

Verified before any golden existed, against the QSPICE probe that motivated it: the probe's
`inoise/onoise` ratio is `25.0306`, implying `|H| = 0.199878` — which matches
`golden/diode_ac.golden`'s independently-computed AC gain for the same network to six figures.
The integrated total agrees too: this project reports `22.30538 µV` rms against QSPICE's printed
`22.3055 µV`.

**The two columns are scored separately**, each against its own peak, rather than flattened into
one series. The input-referred column is larger than the output one by `1/|H|²`, so a shared
near-zero floor would be set by whichever column happens to be bigger and would under-check the
other. The reported verdict is the worse of the two — and an input-referred-only failure is
diagnostic in itself, implicating the *transfer function* rather than the noise sources, since
the two columns differ by nothing else.

A frequency at which the input cannot reach the output reports `inf` rather than `0`: referring
noise to an input with no path to the output is genuinely undefined, and a zero there would read
as "no noise", the opposite of the truth. The integrated total skips non-finite points instead of
becoming `NaN`.

### Per-device noise attribution (added 2026-08-01d)

Every noise golden file now also carries **one column per contributing device**, matching
QSPICE's own `onoise_<dev>` columns. The header names them: `@noise <output> <source> R1 D1`.

**Where device identity comes from.** Not from Interface β — a `ModelInstance` has no name, and
`NoiseSink` receives only `(p, n, psd)`. It comes from **position**: `va-acnoise` polls
instances in order and tags each source with the emitting instance's index, and `va-cli` maps
that index back to a device name, which is sound because `build_instances` pushes exactly one
instance per netlist device in order. No ABI change was needed, and the attribution is *exact*
rather than inferred from topology — two identical resistors in parallel stay distinguishable,
which a `(p, n)`-keyed grouping could never manage. A test pins that case.

**Attribution is per device, not per mechanism.** A diode contributing both shot and flicker
noise reports one combined figure. QSPICE splits its own `onoise_d1` further into
`onoise_d1.id`/`.1overf`/`.rs`; reproducing that would mean naming each model's internal call
sites, which this project has no representation for. Only the aggregate column is read.

**The gate got stricter, and the numbers moved to prove it.** `diode_noise.net` went from
`1.7e-5` to **`2.6e-5`** — not a regression: each column is now scored on its own, so errors
that partially cancelled inside the summed total no longer can. Every column is floored against
its *own* peak for the same reason the two totals already were: a quiet device's column can sit
orders below the total, and a shared floor set by the biggest column would under-check the rest.

**The breakdown demonstrates its own value on `diode_flicker.net`**, where the two columns
separate cleanly: `D1` falls from `4.15e-16` to `1.33e-18` across the band while `R1` stays flat
at `6.62e-19`. The `1/f` roll-off is visibly *in the diode*, which the summed total could only
imply.

The per-device columns sum to the output total by construction — they are the same terms,
bucketed rather than accumulated straight. Committing both is deliberate redundancy: a golden
diff then shows *which* device's contribution moved, not merely that the total did.

## Bring-up ladder

Each rung is a checkpoint; it is "passed" only when `va-harness` is green against golden:

1. resistor divider (DC)
2. diode I–V (DC sweep)
3. RC transient
4. diode rectifier (transient)
5. a MOS DC
6. ring oscillator (transient)

### Current status (updated 2026-07-18)

**All six rungs are formally passed** — `cargo xtask validate` is green against real,
QSPICE-generated golden for every one, not analytic/hand-derived stand-ins:

```console
$ cargo run -q -p xtask -- validate
[xtask]   PASS circuits/divider.net: error=0.000e0 (tol 1e-4)
[xtask]   PASS circuits/mos_dc.net: error=1.490e-6 (tol 1e-4)
[xtask]   PASS circuits/diode_iv.net: error=6.656e-5 (tol 1e-4)
[xtask]   PASS circuits/rc_step.net: error=1.845e-5 (tol 1e-3)
[xtask]   PASS circuits/rectifier.net: error=6.766e-4 (tol 1e-3)
[xtask]   PASS circuits/ring_osc.net: error=1.799e-4 (tol 1e-3)
[xtask] validate: 6 checked, 0 failed golden, 0 did not converge, 0 skipped (no golden)
[xtask] validate: convergence 6/6 (100.0%) — CLAUDE.md §7's convergence metric
```

Two rungs needed real fixes beyond a straightforward QSPICE-native `.model` translation, both
detailed in `docs/roadmap.md`'s T6.3 section and `t6-integration/03-validation.qmd`: rungs 3/4
needed a `UIC` cold-start translation (QSPICE solves the DC operating point before a `.tran` run
by default; this project's own `va-transient` never does); rung 6 needed that plus a genuine
QSPICE ground-aliasing bug fix (`gnd` doesn't reliably resolve to ground for a `Q`-element
terminal) and an honestly-scoped early comparison window (this circuit's unstable equilibrium
makes a full-run comparison chaotic-sensitive, not meaningfully comparable past ~0.1s). Rung 2's
former scope limit is closed (2026-07-18): the golden format now carries `I(V1)` alongside
`V(in)` (§ above), so `mos_dc.net`'s and `diode_iv.net`'s own `error=` figures above moved from
`1.977e-9`/`1.850e-16` (voltage-only, both trivially forced by their own sources) to
`1.490e-6`/`6.656e-5` — larger, but still comfortably inside tolerance, because they now
genuinely check `I(VDD)`/`I(VG)` and `I(V1)` against QSPICE, not just an echoed source voltage.

See `roadmap.md`'s *Status at a glance* and its *Cross-thesis milestones* ladder table for the
authoritative, continuously-updated per-rung detail — this section is a summary, not the source
of truth.

## The model zoo

| Model         | File                  | Status   | Reference (`va-abi`) | Elaborates (T1) | Generated (T2) | Netlist element (T6) |
|---------------|-----------------------|----------|----------------------|-----------------|----------------|-----------------------|
| resistor      | `models/resistor.va`  | bring-up | ✅                   | ✅              | ✅ (matches ref stamp) | `R` |
| capacitor     | `models/capacitor.va` | bring-up | ✅                   | ✅              | ✅ (charge channel)    | `C` |
| diode         | `models/diode.va`     | bring-up | ✅                   | ✅              | ✅ (AD vs FD < 1e-5)   | `D` |
| mosfet (NMOS, Level-1) | `models/mosfet.va` | ladder rung 5 | — (no hand-written `va-abi` reference; solved entirely via the generated model) | ✅ | ✅ (solves `circuits/mos_dc.net` to a hand-derived fixed point < 1e-6) | `M` |
| bjt (NPN, simplified Ebers-Moll) | `crates/va-abi/src/reference/bjt.rs` | ladder rung 6 | ✅ (hand-written only — no `.va` source) | — | — | `Q` |

Reference (hand-written) implementations of resistor/capacitor/diode ship in `va-abi` so the
core can solve before the compiler path is ready; the generated models reproduce those stamps
(resistor hand-checked, diode against finite differences). `mosfet.va` has no hand-written
`va-abi` reference to cross-check against — its correctness is checked against a hand-derived
analytic operating point instead (`cargo test -p va-cli mos_dc_solves_through_codegen_pipeline`).
`bjt` still has no `.va` counterpart (it resolves via `va-cli::reference_instance`'s `"bjt"`
branch, not a compiled model), but it *does* have a netlist element now (`va-netlist`'s `Q`,
added 2026-07-18 alongside `mosfet`'s `M`) — `circuits/ring_osc.net` drives it through the real
pipeline, not just a hand-built `va-transient` instance list. The convergence metric (above) is
real and tracked, not just a stated aspiration.
