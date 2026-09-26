# Validation & the Model Zoo

Reference simulator: **QSPICE** (originally ngspice; switched 2026-07-13 to match the actual
dev environment) — an oracle only; we are not building on it. `va-harness` runs the pipeline
and compares to committed `golden/` outputs.

> **Registry of what matches a reference:** the repository-root `validation.md` lists every
> contribution that reproduces a reference paper, textbook circuit or QSPICE result, by domain,
> with what it was measured against. This file is the *mechanics* — metrics, tolerances, the
> golden set and how it is gated.

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

`golden/*.golden` — all twenty-eight — are real, QSPICE-generated data (`cargo xtask
gen-golden`): `{divider, vcvs_amp, cccs_mirror, mos_dc, diode_iv, diode_iv_params, diode_clamp,
divider_hv, rc_step, rc_step_hv, rc_discharge, rl_decay, rlc_ring, rectifier, ring_osc,
abstime_ramp, vsin_load, laplace_step, rc_ac, rc_ac_lin, rc_ac_oct, diode_ac, laplace_ac,
diode_noise, resistor_noise_va, diode_flicker, resistor_noise_table, resistor_noise_table_log}`. Every one of `xtask`'s known circuits
has a committed golden reference, closing the "which circuits aren't regenerated yet" gap this
file used to track.

**Added 2026-08-31: `circuits/diode_clamp.net`, the nonlinear `.dc` sweep.** `diode_iv.net`'s
`V1` forces the swept node directly, so its node voltage is `V(in) = V1` by construction and
only its `I(V1)` column (added 2026-07-18, above) exercises the diode at all. `diode_clamp.net`
puts a 1 k resistor in series (`Vin --[R1]-- mid --[D1]-- gnd`), which moves the exponential
into a *node voltage*: `V(mid)` tracks `Vin` below the knee, then clamps near 0.66 V as the
diode turns on. It passes at `error=6.421e-5` (tol `1e-4`) against real QSPICE golden — the
same order as `diode_iv.net`'s own `6.656e-5`, and traceable to the same diode nonlinearity.
It is also the circuit `t3-core/03-nonlinear-dc.qmd` plots, since a straight line makes a poor
figure for a chapter about curvature.

**Added 2026-08-31: `circuits/rc_discharge.net`, the initial-condition gate.** Every other
transient circuit in the zoo is driven by a source, so all of them would still produce a
plausible-looking waveform if a capacitor's initial condition were quietly ignored. This one has
**no source at all** — a capacitor charged to 5 V by `IC=5`, decaying through a 1 k resistor — so
`V(out) = 5*exp(-t/RC)` is driven entirely by the initial condition, and dropping it would leave
the circuit sitting at 0 V for the whole run. That makes it the one gate here that can fail
loudly rather than subtly. It passes at `error=7.692e-6` (tol `1e-3`) against real QSPICE golden,
generated through the same `UIC` cold-start translation the other transient decks use — which
already left an explicit `IC=` alone, so no change to `xtask` was needed to support it.

**Added 2026-08-31: `circuits/rlc_ring.net`, the inductor gate.** A series RLC step response
(`R=10`, `L=1mH`, `C=1uF`, so `zeta=0.158`), cold-started so the constant source acts as a step.
It is the first gated circuit with an `L`, and the first *second-order* one: `V(out)` overshoots
to 8.02 V and rings down with a 199 us period. That is what makes it discriminating — a
first-order stamp, a missing flux term, or a sign error on the inductor's constitutive row
cannot produce this waveform at all, where a resistive error would merely shift a level. The
golden file carries `I(L1)` alongside `I(V1)`, so **the inductor's own current is scored against
QSPICE's**, not just the node voltages it influences. Passes at `error=6.480e-5` (tol `1e-3`).

**Added 2026-08-31: `circuits/rl_decay.net`, the inductor's own initial condition.** `IC=` on an
`L` is **amps through it**, not volts across it, so it seeds the element's branch-current row
rather than a node voltage. A source-free `R`/`L` loop starting at 1 mA gates that: `i(t) =
1mA*exp(-t/tau)` with `tau = L/R = 100us`, and the golden scores `I(L1)` itself, so the seeded
quantity is the one compared. Like `rc_discharge.net` it has no source, so ignoring the
condition leaves the whole run flat at zero rather than slightly wrong. Passes at
`error=2.172e-8` (tol `1e-3`), the tightest agreement of any transient gate — unsurprising for a
single-pole linear decay with no nonlinearity for either engine to disagree about.

**Added 2026-08-31: `circuits/cccs_mirror.net`, the current-controlled pair.** `F` and `H`
sense another element's branch current rather than a node pair, so they need that element
resolved to its row before they can be built at all. Both sources in this deck sense the *same*
0 V sensing source deliberately: if either resolved the controlling row wrongly, the two
outputs would disagree about a current they must agree on, which a single-source deck could
not reveal. `F1` mirrors 1 mA times 3 into 200 ohms (`V(fout) = -0.6 V`) and `H1` converts the
same 1 mA at 2000 ohms (`V(hout) = 2 V`). QSPICE agrees on all seven columns exactly:
`error=0.000e0`.

**Added 2026-08-31: `circuits/vcvs_amp.net`, the controlled sources.** SPICE's `E`
(voltage-controlled voltage source) and `G` (voltage-controlled current source) both appear in
one deck, with every value computable by hand: a 3 V source across a 2k/1k divider gives
`V(mid) = 1 V`, the `E` at gain 4 holds `V(eout) = 4 V`, and the `G` pushing 2 mA through 500
ohms gives `V(gout) = -1 V`. QSPICE agrees on all six columns including `I(E1)`, which also
confirms both engines use the same sign convention for a controlled source's own current.
Passes at `error=8.496e-11`, the tightest gate in the suite — expected for a purely linear
circuit where neither engine has anything to be approximate about.

**Added 2026-08-31: `circuits/rc_ac_oct.net`, the octave sweep.** `oct`'s count is a density
like `dec`'s but per factor of 2, so a wrong base silently produces a different grid rather
than erroring — which makes it worth an oracle check rather than only a unit test. 10 Hz to
320 Hz is exactly five octaves, so the expected count is checkable by hand (5*2 + 1 = 11), and
QSPICE returns those same 11 points. Passes at `|mag| 1.942e-15`. Added while reviewing this
session's own work: `lin` had been gated and `oct` had not, which left the newer of the two
grid rules resting on unit tests alone.

**Added 2026-08-31: `circuits/diode_iv_params.net`, per-instance parameter overrides.** A
device line can now set the referenced model's parameters by name (`D1 in gnd diode Is=1e-12
N=1.3`), where before a device could override only the model's *first* parameter, positionally,
through the SPICE scalar value. This circuit is the same sweep as `diode_iv.net` with `Is` and
`N` moved off their `.va` defaults, gated against a QSPICE `.model diode D(IS=1e-12 N=1.3)`
carrying the matching values. It is discriminating: a dropped override would silently be
`diode_iv.net`'s curve again, which differs from this golden by orders of magnitude rather than
marginally. Passes at `error=6.826e-5` (tol `1e-4`).

The deck translator strips those overrides on the way to QSPICE, because SPICE expresses the
same values on the `.model` card instead — and strips them *only* from `D`/`M`/`Q` lines: a
`C`/`L` line's `IC=` is a genuine SPICE element parameter QSPICE reads as written, and removing
it would silently change the initial conditions the golden run starts from.

**Added 2026-08-31: `circuits/rc_ac_lin.net`, the linear AC sweep.** `.ac` accepts all three
SPICE sweep types now (`dec`, `oct`, `lin`), and `lin` is the one whose semantics differ: its
count is a **total** across the band, not a per-decade density. That is the easiest thing to get
wrong and the reason this circuit exists — QSPICE returns exactly the 41 points the card asks
for, so the gate confirms both engines read the count the same way, not merely that the
magnitudes agree. Same RC network as `rc_ac.net`, over a band straddling the -3 dB corner so the
grid samples the response's interesting part. Passes at `|mag| 1.304e-15`, `phase 1.554e-14 rad`.
`.noise` deliberately stays `dec`-only: its integrated-total maths assumes logarithmic spacing,
so a linear grid there would change what the reported total means.

### Ungated circuits, and why each one is (2026-09-01)

Nine of the decks in `circuits/` were not registered with `cargo xtask validate` when this
section was written; as of 2026-09-15 it is twenty-one of forty-nine (the two photonic noise
decks below, and — added between 2026-09-01 and now without being listed here —
`actuator_plant`, `delay_ac`, `interferometer_ac`, `lib_tee`, `microring_thermal`,
`microring_thermal_fast`, each a worked example in `docs/examples.md` or a `va-cli` test
fixture rather than an oracle comparison). They are not one category, and the distinction
matters — a deck that looks like coverage but is exercised by nothing is the failure mode this
file exists to prevent.

**Parked, to be gated later (6).** `nlcap_ramp`, `portprobe_dc`, `portprobe_ac`,
`portprobe_ramp`, `portprobe_sq`, `selfprobe_ramp` — the displacement-current set built for the
`I(<port>)` / `ddt`-coefficient work, with QSPICE-side evidence recorded in
`golden/qspice/README.md` (including a deliberate negative control, `portprobe_dc`, which must
show *no* displacement current at DC). Their numbers discriminate — the margins in that README
run from 50% to 10^6 — but the decks were run by hand rather than through
`translate_for_qspice`, so nothing re-runs them today. **Deliberately kept for future gating,** 
not dead weight to delete: gating them is the remaining step, and until then they are honestly
zero coverage rather than coverage-in-waiting.

**A unit-test fixture (1).** `hier_divider` — driven by a `va-cli` test, never intended for a
golden comparison.

**Deliberately ungated after trying (2).** `rc_pulse` and `transformer`, each documented above
with the measurement that led to the decision. Both are exercised by tests against closed-form
physics rather than against QSPICE.

**Ungated because QSPICE cannot express them (6 as of 2026-09-15: the two photonic noise
decks below, and the four traffic decks `fundamental_diagram`, `motorway_ramp`,
`motorway_ramp_alinea`, `motorway_ramp_mpc` — SPICE has no vehicles; each is pinned by a
`va-cli` test to a closed form or to the paper's own discretisation, `docs/traffic.md` §6).** `fiber_mzi_noise` and
`ring_gyro_noise`, the photonic noise decks of `docs/photonic-noise.md`: an optical-phase net,
an interferometer and a ring resonator have no SPICE primitive, so there is nothing to translate
for the oracle. Each is pinned by a `va-cli` test to closed forms evaluated independently of the
simulator — the Wanser/Duan phase-noise PSDs the waveguide tabulates (within the table's stated
0.7 % interpolation error; the transcription itself is checked against the paper's quoted
−125.5 dB re rad/√Hz), the detectors' `2q·I·R²`, and for the gyro the full
`R²(2q i_d + 4kT/R + RIN i_d²)` budget from the solved operating point with the input-referral
gain measured by two extra DC solves. The photonic *elements* that do have QSPICE analogues
(a resistor's Johnson noise, a diode's shot noise) are already gated by `resistor_noise_va`,
`diode_noise` and `diode_flicker`; these decks add no oracle coverage and are not counted as
such.

### Gear/BDF2 measured against trapezoidal (2026-09-01)

`Method::Gear` is implemented and reachable as `va-cli sim --integration gear`. It is **not the
default**, and the measurement is why — recorded here rather than in a commit message because
it is the answer to "is this worth using", not just "does it work".

Every gate passes under Gear (24/24), so this is a comparison of two working methods, not a
failure. Each circuit's golden error, trapezoidal vs Gear, at the same tolerances:

| circuit | trapezoidal | Gear | Gear / trap |
|---|--:|--:|--:|
| `rlc_ring` | 6.480e-5 | 6.042e-4 | 9.3x worse |
| `rc_step` | 2.248e-5 | 6.865e-5 | 3.1x worse |
| `ring_osc` | 4.553e-6 | 1.385e-5 | 3.0x worse |
| `rl_decay` | 2.172e-8 | 5.574e-8 | 2.6x worse |
| `rc_discharge` | 7.692e-6 | 1.355e-5 | 1.8x worse |
| `rectifier` | 8.269e-4 | 8.673e-4 | 1.05x worse |
| `abstime_ramp` | 4.382e-17 | 4.382e-17 | same |

Step counts are essentially identical (`rc_step` 267 vs 267, `rectifier` 718 vs 711, `rlc_ring`
1013 vs 1013), so this is not a speed-for-accuracy trade — it is the same work for less
accuracy. Backward Euler, for scale, needs 1980 steps on `rectifier` against trapezoidal's 718.

**Why the textbook argument did not show up.** BDF2's selling point is L-stability: it damps a
stiff mode to zero in one step where trapezoidal's amplification factor approaches -1 and can
ring numerically. That advantage appears when the step is *large* relative to the stiff mode.
Here the adaptive controller already shrinks `h` until the local error meets tolerance, which
is precisely the regime where trapezoidal does not ring — so Gear pays its extra damping
without collecting the benefit. `rlc_ring` is the clearest case and the worst result: its
ringing is **physical** (zeta = 0.158), so a method that damps harder is simply less faithful,
which is what the 9.3x says. The proposal predicted exactly this circuit would argue against
Gear rather than for it.

**Conclusion: keep trapezoidal as the default.** Gear earns its place as an opt-in for a future
genuinely stiff circuit, and the honest summary today is that this zoo has none — a fixed-step
run, or a deck whose stiffness outruns `tstep_min`, would be where to look next.

### A second ungated circuit: `circuits/transformer.net` (2026-08-31)

Mutual inductance (`K`) works, and the two engines agree on the whole waveform — peak
`V(s) = 1.681 V` at 3.9 us, to four digits. They disagree in the **first microsecond**, where
QSPICE swings the secondary to `-0.43 V` and this engine holds it at 0.

**Here we can show which is right, rather than only that they differ.** KCL at the secondary
node says `i_L2 + V(s)/R2 = 0`, and an inductor's current cannot jump, so `V(s)(0+)` is exactly
zero. QSPICE's early excursion violates that continuity; ours does not. The likely cause is on
the translation side: `gen-golden` injects `IC=0` into every reactive element plus `UIC` to
match this engine's cold start, and forcing an initial current on *coupled* inductors appears
to leave QSPICE's first timepoint inconsistent.

An RMS gate over the full window scores that disagreement (measured `1.749e-2` against a
`1e-3` tolerance, and still `1.7e-2` after dropping everything before 100 ns). The only thing
that would hide it is a per-circuit "ignore the early window" knob — a gate-weakening
mechanism, and one that should be a deliberate decision rather than a side effect of wanting a
green line. (The existing `RING_OSC_GOLDEN_TSTOP` is the *opposite*: it compares only an early
window and discards a late one that is chaotic-sensitive.) So the circuit stays out of the gate
and is validated on the two facts that need no oracle: `V(s)(0+) = 0` exactly, and removing the
`K` card leaves the secondary at exactly zero for the whole run — which is what makes the
first assertion a statement about coupling rather than about wiring.

### A circuit deliberately *not* gated: `circuits/rc_pulse.net` (2026-08-31)

`PULSE(v1 v2 td tr tf pw per)` sources are implemented and tested, but the RC circuit driven by
one is **not** compared against QSPICE golden, and the reason is worth recording because it is
the first case where this project and the oracle genuinely disagree about a *definition*.

**QSPICE starts a `PULSE` ramp slightly before `td`.** Measured by probing QSPICE directly with
single-source decks and extrapolating each ramp linearly back to `v1` (the ramp's own slope is
exact, so the intercept is exact too):

| deck `.tran` step | `td` | measured ramp start | offset |
|---|---|---|---|
| 2 us | 100 us | 99.9 us | -0.1 us |
| 2 us | 125 us | 124.9 us | -0.1 us |
| 2 us | 200 us | 199.9 us | -0.1 us |
| 0.5 us | 200 us | 199.95 us | -0.05 us |
| 0.2 us | 200 us | 199.961 us | -0.039 us |

The slope always matches `(v2-v1)/tr` exactly, so `tr` is honoured; only the *placement* moves.
The offset is independent of `td` (three values, same offset) and independent of `tr` (1 us and
10 us edges gave the same 0.1 us), and it is not a dyadic-grid snap — `td = 125 us` is exactly
`tstop/16` and still lands 0.1 us early. It varies with the run's timing setup in a way that is
not proportional to the timestep (a 10x smaller step moved it only 2.6x), which points at a
QSPICE-internal minimum edge or startup grid rather than anything derivable from the deck.

**Why that sinks an RMS gate.** A fixed time shift on a fast edge is a large amplitude error:
0.1 us on a 20 us / 5 V edge is 25 mV, and the RC integrates it into a persisting offset on the
output node. Measured: `error=5.779e-2` with 1 us edges, `5.254e-3` with 20 us edges,
`1.698e-3` with 100 us edges — all against a `1e-3` tolerance, and all traceable to that one
shift. Slowing the edges further until the number dips under the bar would be tuning the
circuit to the tolerance rather than testing anything, so it was not done.

**What is validated instead.** `PULSE`'s shape is pinned against its own definition, segment by
segment and from both sides of every boundary (`va-cli`'s
`a_pulse_waveform_follows_its_definition_segment_by_segment`), including the single-shot
(`per <= 0`) and ideal-edge (`tr = 0`) cases that must not divide by zero. The RC's response is
checked against the analytic charging law parameter-free: the ratio of successive gaps to the
source level decays as `exp(-dt/RC)` on the plateau and between pulses, which needs no absolute
reference at all. This engine starts the ramp at `td`, the textbook SPICE definition, and that
is what is tested.

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

### Compiled-model noise: the `noise_table()` gate (added 2026-08-04)

**`circuits/resistor_noise_table.net`** completes the set — the third and last of Verilog-A's
noise builtins (T5.6). Two 1 kΩ resistors across a 1 V source, both resolving to the compiled
`models/resistor_noise_table.va`, whose thermal source is written as a three-point table of
`4kT/R` instead of a `white_noise()` call. The sources add in power through `R1∥R2 = 500 Ω`,
giving a flat `4kT·500Ω = 8.288e-18` V²/Hz that a **plain QSPICE resistor pair reproduces
exactly** — no `.model` translation, the same arrangement `resistor_noise_va.net` uses.
Measured: **`1.9e-16`**, machine precision on the same terms as that gate.

Both resistors are 1 kΩ rather than the 1 k/3 k of `resistor_noise_va.net` for a reason worth
knowing before writing a tabulated model: **a table is const-folded at elaboration**, so it can
follow neither `$temperature` nor the per-device resistance `va-cli` overrides onto a compiled
model's first parameter afterwards. That is the LRM's own restriction (a table is an array
parameter or an assignment pattern, i.e. constant data), not a shortcut here — but it makes a
1 k/3 k deck silently wrong in a way a `white_noise()` deck is not.

**What this gate does and does not prove.** The deck's table spans 100 Hz – 1 MHz while the
sweep runs 10 Hz – 10 MHz, so every run walks all three of the LRM's code paths — clamp low,
interpolate, clamp high. But the table is *flat*, and on a constant table clamping and
extrapolating agree, so the gate pins the **absolute level and the end-to-end path** (frontend →
IR → codegen → Interface β → adjoint → harness) rather than the interpolation rules themselves.
Telling those apart is done by unit tests over deliberately shaped tables: the LRM's own
§4.6.4.3 example table read *between* decade points (which catches a log-interpolating
implementation), its Figure 4-9 two-point `1/f` log table, an unsorted table, a zero-power
segment, and a `va-acnoise` sweep over a rise-then-fall table read entirely between its knots.
A flat table is the only shape QSPICE has a native primitive to compare against at all, so
splitting the duties this way is the honest resolution rather than a hole — stated here so
nobody later reads `1.9e-16` as evidence the interpolator is right.

**`circuits/resistor_noise_table_log.net` (added 2026-08-05)** is that deck with one word
changed in the model — `noise_table_log` for `noise_table` (LRM §4.6.4.4 vs §4.6.4.3). Its
golden is deliberately the *same physics*, because on a flat table the LRM's two interpolation
rules must agree exactly and checking that they do is the point. Measured: **`1.9e-16`**,
identical to the linear deck. What it adds is that the logarithmic path — logs, a power, and
§4.6.4.4's formula — runs on every point of a real sweep against a real oracle, where a NaN, an
infinity or a badly-conditioned exponentiation would surface. What it still cannot check is that
two points describe an exact power law, the property that makes `noise_table_log` worth having:
that stays pinned by the unit test over the LRM's own Figure 4-9 example (`{1,1, 1e6,1e-6}` →
exactly `1/f`), plus a codegen test that the same table read under the two rules genuinely
diverges between its knots (`1e-3` vs `~1.0` at 1 kHz) while agreeing at them.

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

### Analysis-context constructs: what is gated and what is not (added 2026-08-06)

Tier A of `docs/proposals/analysis-context.md` — `analysis()`, `$abstime`, `ac_stim`,
`bound_step` — is **not golden-gated, and this section says so plainly rather than letting a
green 13/13 imply otherwise.**

**Why QSPICE cannot be the oracle here.** QSPICE does not consume our Verilog-A models, and our
netlist grammar cannot express a behavioral model natively, so there is no deck that exercises
these constructs on both sides. Hand-computing a golden file to fill the gap is forbidden
(`no-fake-golden-data`: QSPICE is the sole oracle, and a plausible hand-derived number is worse
than no number because it looks like evidence).

**What *is* checked, and how strongly:**

| Property | How it is validated | Strength |
|---|---|---|
| Every existing gate is unchanged | `cargo xtask validate` — all 13 pre-existing circuits reproduce their **previously recorded numbers to the last digit**, including `rectifier.net` (6.766e-4), the one that exercised the now-deleted `run_dynamic` | Strongest available, and free |
| **`$abstime` end to end** | **Golden-gated** against real QSPICE: `circuits/abstime_ramp.net` vs `golden/abstime_ramp.golden` (**error 4.382e-17**, tol 1e-3) | **Golden — a real oracle, see below** |
| `analysis()` selects per analysis | End-to-end unit test: one compiled model is a plain resistor in DC and a resistor plus a known 1 mA offset in transient; each half is solved and checked against its own closed form | Unit, but end-to-end through the real pipeline |
| `$abstime` tracks the clock | End-to-end unit test: a compiled `I <+ V/R + k·$abstime` ramp integrated over 1 ms, every accepted point checked against `−k·t·R`; DC reads exactly 0 V | Unit, closed-form |
| `$abstime`/`analysis()` have zero Jacobian | Central finite difference, per `CLAUDE.md` §5 | Unit, and mandatory |
| `ac_stim` sign and complex response | `va-acnoise` test: a model-supplied 1 A stimulus into R‖C, magnitude *and* phase checked against `−R/(1+jωRC)` at every swept point, with no netlist `AC` source anywhere | Unit, closed-form — and the sign is the part that is easy to get backwards |
| `bound_step` caps the step | `va-transient` test: no accepted step may exceed the requested bound; a second test confirms a bound inside an `if` applies only when that arm runs | Unit, property-based |

**The honest summary:** the regression floor is golden-gated, `$abstime` is golden-gated, and
the other three constructs are unit-tested only.

### The `$abstime` gate (added 2026-08-06)

The blocking spike the proposal called for **was run**, and it succeeded. QSPICE's behavioral
source does expose `time`: `B1 out 0 I=1*time` into a 1 kΩ resistor reproduces `V = −1000·t`
with **zero** error across all 1029 points. So `circuits/abstime_ramp.net` is a real gate — our
side drives a compiled `models/abstime_ramp.va` (`I(p,n) <+ K*$abstime`) through the whole
frontend → codegen → transient pipeline, QSPICE drives its own behavioral source, and the two
descriptions share no code.

Three things make it evidence rather than decoration:

- **The sign convention maps one-to-one with no fixup.** Verilog-A's `I(p,n) <+ expr` and
  SPICE's `B n+ n- I=expr` both drive current *out of* the first node, so the terminal order
  carries over unchanged. Had a sign flip been needed to make it pass, that would have been
  tuning, not translating.
- **The deck is deliberately resistive-only.** Every timepoint is an exact algebraic solve, so
  no integration error can contribute — a discrepancy can only come from `$abstime` itself.
- **It discriminates, verified by deliberately breaking it.** Reintroducing the original fold
  (`$abstime → 0.0`) moves the gate from `4.382e-17` to **`5.838e-1`**, ~580× over tolerance.
  A gate that only ever passes proves nothing; this one was watched failing for the right
  reason before being trusted.

**A gotcha worth its own paragraph, because it nearly poisoned the gate.** `UIC` shifts QSPICE's
own `time` variable by a fixed offset: with `.tran … UIC`, `I(B1) − time` is exactly `+1.0e-7`
at *every* point; without `UIC` it is exactly `0.0`. Every other transient gate here goes
through `cold_start_tran_deck`, which adds `UIC` on purpose (QSPICE otherwise solves the DC
point first and disagrees with our cold start). This deck must **not**, and can safely skip it
only because it contains nothing reactive — with no capacitor to seed, QSPICE's operating-point
solve lands on the same `t = 0` state we start from. A future behavioral gate containing a
reactive element would have to reconcile the two rather than inherit the exemption.

**What this gate does not catch.** A sub-tolerance *time offset* would slip through: the 1e-7 s
`UIC` shift is worth ~1e-4 V here, inside the 1e-3 tolerance. The gate is decisive about
`$abstime` being dead, frozen, or wrongly scaled; it is not a clock-accuracy measurement.

`analysis()`, `ac_stim` and `bound_step` remain unit-tested only. No single QSPICE construct
corresponds to `analysis()` (the by-construction split described in the table above is the
best available), and neither `ac_stim` nor `bound_step` has an expressible QSPICE counterpart
driven from a model rather than a netlist.

### Tier B: the state channel, `transition` and `slew` (added 2026-08-07)

**Unit-tested, not golden-gated**, and the reason is different from Tier A's.

QSPICE *can* express these — its behavioral sources compute `min`/`limit`/`sdt` exactly
(verified 2026-08-06: `sdt(2)` integrates to 4.6e-17 of `2t`) — so a `B1 o 0 V=min(1, R*time)`
deck would reproduce a slew-limited ramp's closed-form envelope. What that comparison would
check is *our numerical recurrence against the analytic answer*, which is genuinely useful but
is not two independent implementations of slew limiting. It was not built because the same
property is already asserted, more directly and without a QSPICE round-trip, by the end-to-end
test below.

| Property | How it is validated | Strength |
|---|---|---|
| Every existing gate is unchanged | `cargo xtask validate` — all **14** circuits reproduce their previous numbers to the last digit | Strongest available, and free |
| `slew` rate-limits end to end | `va-cli` test: a compiled `slew(k·$abstime, rate)` with `k = 10·rate`, solved through the real pipeline; output must follow `rate·t`, **a factor of ten below its own input** | Unit, closed-form, strongly discriminating |
| Static solves are unmoved | Same test's DC half: `is_initial_step` makes the limiter settle to its input, reproducing the old const-fold exactly | Unit |
| Read-old/write-new | `va_abi::state` unit test: a `set` is invisible to a `get` in the same evaluation | Unit — the channel's defining invariant |
| Unwritten slots mean "unchanged" | `va_abi::state` unit test on the consumer's pre-seed rule | Unit |

**What is *not* covered, stated rather than implied.** The slew test's circuit is purely
algebraic and its input is smooth, so the LTE controller almost certainly never rejects a step —
which means **rollback-on-reject is not exercised by a rejecting circuit**. It rests on the
`ModelState` unit tests and on the `StateBuffers` discipline being small enough to read. A
circuit that forces rejections while carrying state would be a real addition.

`transition` has **no** dedicated end-to-end test yet, only the shared channel's. It is also the
one construct here implemented as an acknowledged approximation (no exact corner breakpoints),
so it is the weakest link in this row and should be the next thing gated.

### The `laplace_*` gate (added 2026-08-07) — the strongest oracle of the three tiers

`circuits/laplace_ac.net` compares a compiled `models/laplace_lowpass.va`, written purely as
Laplace transfer functions, against a QSPICE deck built from **a real R and a real C**:

```
error |mag| 1.361e-15, phase 1.690e-13 rad   (tol 1e-4)   — zoo 15/15
```

Why this is better evidence than the two tiers before it. Tier A's `$abstime` gate had QSPICE
evaluate *the same formula* (`k*time`) in a different engine; Tier B had no oracle worth
building. Here the two sides do **genuinely different arithmetic** — a rational function of `s`
evaluated at `jω` on our side, two physical components solved as a network on QSPICE's — and
they agree to machine precision at all 60 frequency points.

**It discriminates, verified by deliberately breaking it.** Restoring the pre-Tier-C fold
(evaluate `H` at `s = 0` always) moves the gate from `1.361e-15` to **`6.282e3`**, seven orders
of magnitude over tolerance. A flat response versus a −20 dB/decade rolloff is not subtle.

**One detail worth recording, because getting it wrong made the gate fail for the wrong
reason.** The first version of the model implemented only the *voltage* transfer function, so it
drew no input current — while the reference RC network loads its source. `V(out)` matched
perfectly and the `I(V1)` column was 100% out. The fix was to model the divider's **input
admittance** too, `Y(s) = sC/(1 + sτ)`, as a second Laplace form. That is not a workaround: the
two circuits genuinely were not the same circuit until both observable properties matched, and
the repaired model now exercises a numerator with a zero at the origin as a bonus.

**Transient, since v0.9.16 (2026-09-11):** `circuits/laplace_step.net` runs the same model as a
1 V step from a cold start and compares it against the same R-C network in QSPICE, cold-started
(`UIC` — the behavioural-translation table now says per entry whether the replacement needs it,
since an R-C reads no `time` and must, while a `B` source reading `time` must not). The filter is
integrated as an ODE on auxiliary state unknowns (`va_codegen::lower::LaplaceStates`); the R-C is
two physical components; the two agree to 5.1e-6 RMS on `V(out)` and `I(V1)`. The `I(V1)`
column is the admittance filter `sC/(1 + sτ)`, whose numerator degree equals its denominator's,
so the feedthrough path of the realization is on the gate as well. **Not covered:** noise, where
a Laplace filter still evaluates to `H(0)` — a stated limitation at the construct.

## The circuit-size limit (re-measured 2026-09-23, v1.9.0)

Since 1.6.0–1.8.0 every analysis (`.op`, `.dc`, `.tran`, `.ac`, `.noise`) uses dense LU below
`va_core::sparse::SPARSE_THRESHOLD` unknowns and sparse LU from it
(`docs/proposals/sparse-solve.md`) — 500 until 1.9.0, **100 since 1.10.0** on the strength of the
crossover below; `--solver dense|sparse` overrides the choice. This section
is Step 5 of that proposal: both paths measured at every size, on two circuits, in one session.

**Method.** `cargo run --release -p xtask -- bench-scale --solver dense|sparse --topology
ladder|mesh`: a whole `.op`, `.tran`, `.ac` and `.noise` through `va_cli`, reference primitives
only (the compiler is not in the timing). The **ladder** is an RC ladder (`dim = n + 1`,
tridiagonal: no fill-in, sparse LU's best case). The **mesh** is a square `k × k` RC grid driven at
one corner (`dim = k² + 2`, the 5-point pattern of a 2-D grid, which fills in under LU: a harder
case, and a fair stand-in for a power grid or thermal network). Same element values on both
(`R = 1 kΩ`, `C = 1 nF`), `.tran` 5 µs with a 50 ns step hint (101 accepted points at every
size), `.ac`/`.noise` three decades at 10 points/decade (31 points). Two runs of each; every cell
is the **slower** of the two (spread up to 2.6× below 50 unknowns, at most 1.8× from 100).
**Machine:** 11th-gen Intel Core i7-1185G7 @ 3.0 GHz, 16 GB, Windows 11, release profile,
single-threaded.

### Dense against sparse: the crossover

Per-point milliseconds; the last column group is dense ÷ sparse (above 1: sparse is faster).

Ladder:

| dim | dense op | tran/pt | ac/pt | noise/pt | sparse op | tran/pt | ac/pt | noise/pt | ÷ op | ÷ tran | ÷ ac | ÷ noise |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 12 | 0.08 | 0.017 | 0.016 | 0.015 | 0.11 | 0.019 | 0.019 | 0.018 | 0.7 | 0.9 | 0.8 | 0.8 |
| 22 | 0.13 | 0.037 | 0.955 | 0.486 | 0.11 | 0.023 | 0.022 | 0.021 | 1.2 | 1.6 | 43 | 23 |
| 52 | 0.62 | 0.473 | 0.719 | 0.675 | 0.20 | 0.048 | 0.034 | 0.035 | 3.1 | 9.9 | 21 | 19 |
| 102 | 1.07 | 1.242 | 2.674 | 2.055 | 0.27 | 0.068 | 0.056 | 0.061 | 4.0 | 18 | 48 | 34 |
| 202 | 3.08 | 2.289 | 9.595 | 8.118 | 0.84 | 0.136 | 0.131 | 0.104 | 3.7 | 17 | 73 | 78 |
| 402 | 17.24 | 6.722 | 24.769 | 29.341 | 0.95 | 0.234 | 0.305 | 0.233 | 18 | 29 | 81 | 126 |
| 802 | 47.16 | 31.023 | 85.628 | 105.120 | 1.77 | 0.602 | 0.851 | 0.539 | 27 | 52 | 101 | 195 |

Mesh:

| dim | dense op | tran/pt | ac/pt | noise/pt | sparse op | tran/pt | ac/pt | noise/pt | ÷ op | ÷ tran | ÷ ac | ÷ noise |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 11 | 0.02 | 0.007 | 0.008 | 0.006 | 0.07 | 0.018 | 0.024 | 0.029 | 0.3 | 0.4 | 0.3 | 0.2 |
| 18 | 0.10 | 0.014 | 0.654 | 0.660 | 0.19 | 0.033 | 0.042 | 0.042 | 0.5 | 0.4 | 16 | 16 |
| 51 | 0.97 | 0.290 | 0.497 | 0.556 | 0.42 | 0.072 | 0.114 | 0.105 | 2.3 | 4.0 | 4.4 | 5.3 |
| 102 | 1.11 | 0.697 | 1.573 | 1.378 | 0.60 | 0.131 | 0.369 | 0.369 | 1.9 | 5.3 | 4.3 | 3.7 |
| 198 | 3.64 | 1.810 | 4.386 | 4.353 | 1.65 | 0.358 | 1.413 | 1.283 | 2.2 | 5.1 | 3.1 | 3.4 |
| 402 | 9.41 | 6.369 | 18.251 | 19.724 | 2.88 | 1.326 | 3.220 | 2.939 | 3.3 | 4.8 | 5.7 | 6.7 |
| 786 | 46.87 | 37.741 | 92.346 | 99.299 | 7.45 | 3.221 | 9.462 | 9.389 | 6.3 | 12 | 9.8 | 11 |

**Reading it.** Dense wins only at the smallest sizes — up to 5× at 11–18 unknowns on the
mesh, by tens of microseconds per point — and sparse wins every column from ~50 unknowns on
both circuits: 1.9–12× on the mesh, 3–200× on the ladder, growing with size. So the crossover lies
**between ~20 and ~50 unknowns**, an order of magnitude below the 500 the threshold was set at; it
was not measured between 22 and 51. The dense AC/noise columns jump between 12 and 22 unknowns
(the real `2·dim` embedding crossing some size inside `faer`'s dense LU); it is reproducible, and
it is why AC's crossover is below 20. **Decided on it (1.10.0):** the threshold moved from 500 to
**100**, clear of the crossover and its run-to-run noise (at ~100 sparse is 1.9–5× faster on the
mesh, 4–48× on the ladder). No validation gate is affected (the largest is `ring_osc.net` at 8
unknowns).

### Sparse beyond the dense range

| dim (ladder) | op ms | tran ms/pt | ac ms/pt | noise ms/pt | | dim (mesh) | op ms | tran ms/pt | ac ms/pt | noise ms/pt |
|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|---:|
| 802 | 1.77 | 0.602 | 0.851 | 0.539 | | 786 | 7.45 | 3.221 | 9.462 | 9.389 |
| 1 602 | 3.47 | 0.928 | 1.391 | 1.100 | | 1 602 | 21.16 | 6.361 | 32.598 | 24.645 |
| 3 202 | 8.98 | 2.138 | 2.328 | 2.431 | | 3 251 | 38.46 | 12.613 | 49.874 | 57.380 |
| 6 402 | 16.17 | 4.173 | 4.137 | 5.144 | | 6 402 | 92.61 | 36.798 | 142.561 | 142.783 |

Growth per doubling at the top: the ladder ~2× (exponent ~1), the mesh 2–3× (exponent 1.0–1.6),
against dense's 4.6–5.9× per doubling in the transient column over 400→800 alone. A
10 000-point transient at 6 400 unknowns is 42 s on the ladder and 6 min on the mesh; dense at 800 unknowns took 5–6 min for the same
point count.

### A real device circuit: the PSP103 inverter chain

The ladder and mesh are linear primitives. The same question on compiled models: `N` CMOS
inverters in a chain (`circuits/benchmark/psp103_inverter_card_tran.net`'s devices and cards,
chained; `.op`), peak working set sampled while `va-cli` runs, as in v1.3.2. **Each PSP103
instance adds ~16 internal rows**, so real device circuits reach the threshold fast: 5 inverters
are 171 unknowns (above the 100 threshold; 15 would have crossed the old 500).

| inverters | unknowns | dense peak | dense wall | sparse peak | sparse wall |
|---:|---:|---:|---:|---:|---:|
| 5 | 171 | 22.7 MB | 0.30 s | 22.7 MB | 0.25 s |
| 10 | 336 | 25.8 MB | 1.27 s | 22.6 MB | 0.66 s |
| 20 | 666 | 37.4 MB | 2.67 s | 23.0 MB | 1.10 s |
| 40 | 1 326 | 80.0 MB | 9.58 s | 24.8 MB | 2.18 s |
| 80 | 2 646 | 247.1 MB | 63.1 s | 30.2 MB | 12.1 s |

Memory: the dense slope is superlinear (the `dim²` buffers, as v1.3.2 found); the sparse slope is
~0.1 MB per inverter and linear. At 336 unknowns — below the 500 threshold of the time, so dense
under `Auto` until 1.10.0 moved it to 100 — sparse is already 1.9× faster on the whole run. One run each, not two: the wall times are
indicative, the memory is not noise-sensitive.

**Found on the way — not a sparse regression, fixed in 1.10.1 up to 160 inverters.** From ~95
inverters (≥ 3 150 unknowns) the `.op` failed with "singular matrix" **on both paths**, identically
(dense checked at 100 and 160). 1.9.0 recorded a guess that the solve's residual tolerance, absolute
in `b`, was rejecting good solves of a large matrix. **Instrumenting it disproved that:** every
rejected solve had a componentwise backward error of 1.0 and a solution of 1e14–1e20, and one step
of iterative refinement did not help — the matrices really were numerically singular. The cause
was upstream, in Newton: the plain solve always fails at iteration 1 (also at 80 inverters), the
`gmin` rescue takes over, and part-way down its ladder one undamped step proposes ~5.7e4 V at a
net near the end of the chain. The residual jumps from 1.5e-4 to 54, and the step back lands on a
singular Jacobian. Nothing limited that step: junction limiting only covers unknowns a model marks
as junctions. **Fix (1.10.1):** a second rescue tier reruns the ladder with Newton's residual line
search on (`va_core::dc`, `RESCUE_DAMPING_HALVINGS = 20`), reached only when the ladder alone has
failed.

| inverters | unknowns | before (1.10.0) | 1.10.1 | wall, 1.10.1 |
|---:|---:|---|---|---:|
| 80 | 2 646 | solves (ladder) | same path, same answer | 7.3 s |
| 95 | 3 141 | singular | solves (damped ladder) | 23.2 s |
| 100 | 3 306 | singular | solves | 20.2 s |
| 160 | 5 286 | singular | solves | 40.8 s |
| 320 | 10 566 | singular | **still singular** | 13.5 s to fail |

The answers are the physical ones (input at 0 V: odd stages end at 1.199991 V, even at 4.43 µV).
Damping for the whole rescue instead of as its own tier solved the same chains but took the
80-inverter run from 6.8 s to 35 s, which is why it is a tier. **At 320 inverters it fails
differently:** the Jacobian goes singular right after a 0.53 V step, not a runaway one, which no
line search can help. Not diagnosed; it is now the device-circuit size limit.

### The pre-flight estimate on the sparse path

`va-cli`'s cost bracket (`va_cli::estimate`) is calibrated on the path the run takes. On the
sparse path its two ends are the ladder (low) and the mesh (high) at the circuit's `dim`, from the
tables above; beyond 6 402 unknowns, exponents 1 and 2. It prints `sparse matrix` in place of a
memory figure, because sparse memory follows the nonzeros and their fill-in, not `dim²`.

**Its known blind spot, on both paths:** it is per Newton loop on a linear circuit. A deck whose
operating point needs many iterations, or a `gmin` rescue, is underestimated — on the PSP103
chain by 30× to 1 000× (40 inverters: 174–303 ms quoted dense against 9.6 s measured; 2.2–37 ms
quoted sparse against 2.2 s). The iteration count is not knowable before the solve, so the
bracket cannot include it; the line already says "rough".

### Dense LU below the threshold (measured 2026-09-17, v1.1.0)

What follows is the dense path's own record, which still describes every run below 100 unknowns
and every `--solver dense` run. Measured before the sparse path shipped, on the ladder only, with
`bench-scale` as it then was. The 2026-09-23 dense ladder columns above are faster than it from
102 unknowns up — 1.1× at 102, 2.1× at 402, 1.7× at 802 in the transient column — for reasons not
investigated; the tables in `va_cli::estimate` still carry the figures below, so on the dense path
the estimate errs slow.

Measured with `cargo run --release -p xtask -- bench-scale` (dense, as it then was): a whole `.op`,
`.tran`, `.ac` and `.noise` through `va_cli` on an RC ladder of `n` sections (`R = 1 kΩ`,
`C = 1 nF`, `dim = n + 1` unknowns; reference primitives, so the compiler is not in the timing). The `.tran` window is 5 µs with a 50 ns step hint, trapezoidal, LTE
`1e-3`; the `.ac`/`.noise` grid is three decades at 10 points/decade (31 points). **Machine:**
11th-gen Intel Core i7-1185G7 @ 3.0 GHz, 16 GB, Windows 11, release profile, single-threaded.

```
  n_nodes    dim     op_ms    tran_ms  points  tran_ms/pt  ac_ms/pt  noi_ms/pt  ac_pts
       10     12      0.02        1.2     120       0.010     0.006      0.006      31
       20     22      0.03        2.1     120       0.017     0.408      0.361      31
       50     52      0.38       56.0     120       0.466     0.544      0.474      31
      100    102      0.85      166.8     120       1.390     1.463      1.464      31
      200    202      2.24      398.3     120       3.319     4.735      6.093      31
      400    402      8.08     1709.0     120      14.242    21.494     22.297      31
      800    802     63.46     6280.2     120      52.335    78.766    120.293      31
```

Each cell is the **slower of two runs** taken back to back; run-to-run spread is ~30% at the
middle sizes and ~2× on the single-solve `op_ms` column, which is one Newton loop and too short
to time stably. The accepted-point count is the same at every size (the input edge at `t = 0` is
the same event), so the per-point columns isolate growth with `dim`.

**Read the columns for what each analysis pays per point:** `op_ms` is one whole Newton loop;
`tran_ms/pt` is per *accepted* timepoint, with the rejected ones' cost folded into it; `ac_ms/pt`
is one complex factorization and no Newton loop; `noi_ms/pt` is that plus the adjoint solve
behind the input-referred and per-device spectra. An AC point costs 1.2–1.5× a transient point
here despite doing no Newton iterations, because its arithmetic is complex.

### It got 2.8× faster since 2026-09-11, and that is a finding

The table this section carried before (v0.9.16+1, same machine, same profile) read:

| dim | `tran_ms/pt` 2026-09-11 | 2026-09-17 | ratio |
|---:|---:|---:|---:|
| 12 | 0.011 | 0.010 | 1.1 |
| 22 | 0.017 | 0.017 | 1.0 |
| 52 | 0.785 | 0.466 | 1.7 |
| 102 | 2.105 | 1.390 | 1.5 |
| 202 | 6.215 | 3.319 | 1.9 |
| 402 | 24.912 | 14.242 | 1.7 |
| 802 | 147.296 | 52.335 | **2.8** |

Two runs on 2026-09-17 agree with each other, so this is not measurement noise. The shape of the
change is the interesting part: the two smallest rows are unchanged and the speed-up grows with
`dim`. That is what a **drop in Newton iterations per timepoint** looks like — a small circuit's
per-point cost is dominated by fixed overhead that no iteration count touches, while a large one
pays a full O(dim³) factorization per iteration and gains the whole ratio. The plausible cause is
0.9.22's per-nature `abstol` in the transient Newton update test (it was a flat 1e-12, so the
loop kept iterating past the point where the answer had stopped moving). **Not bisected** —
stated as the likely explanation, not a verified one. The consequence is stated plainly because
it changes the advice: the limit below is roughly twice as far out as this document said a week
ago.

### The dense limit, for a 10 000-point transient

A typical `.tran` of this project's decks runs 1 000–3 000 accepted points; 10 000 is a long one.

| unknowns | per point | 10 000 points |
|---:|---:|---:|
| 100 | 1.4 ms | 14 s |
| 200 | 3.3 ms | 33 s |
| 400 | 14 ms | 2.4 min |
| 800 | 52 ms | 8.7 min |
| ~1 600 (extrapolated, exponent 2–3) | 0.21–0.42 s | 35–70 min |

So, on dense LU: **up to ~400 unknowns a transient is interactive; ~800 is a coffee break;
beyond ~1 600 dense LU is impractical.** Since 1.10.0 no `Auto` run is dense above 100 unknowns;
this now describes `--solver dense`, and the sparse figures are in the section above. A `.op` stays under 0.1 s even at 800 unknowns —
operating points and DC sweeps are not where the wall is.

### What "unknowns" counts, from a designer's side

The matrix dimension is not the component count. Every row comes from one of these:

| Contributes a row | Contributes no row of its own |
|---|---|
| each non-ground net in the deck | each resistor, capacitor, diode, VCCS — they only stamp into existing rows |
| each independent voltage source, inductor, and controlled source carrying a current unknown | `ddt`, which goes on the charge channel |
| each **internal node** of a Verilog-A module, **per instance** (hierarchy is flattened) | `zi_*` filters, sampled on the state channel |
| each branch-current unknown a potential contribution or a flow probe needs | parameters, variables, `analog function` locals |
| each `idt` call site (an accumulator) | |
| each `laplace_*` call site, **one row per denominator degree** | |

So modelling complexity inflates `dim` faster than schematic size does. A behavioural block with
three internal nodes and a 4th-order `laplace_vp` costs seven rows *per instance*; ten of them is
70 rows before any wiring. Two-terminal passives are nearly free.

### The budget: how many timepoints fit in a wait

| unknowns | ms/point | 1 minute | 10 minutes | 1 hour |
|---:|---:|---:|---:|---:|
| 100 | 1.4 | 43 000 pts | 430 000 | 2.6 M |
| 200 | 3.3 | 18 000 | 180 000 | 1.1 M |
| 400 | 14 | 4 200 | 42 000 | 250 000 |
| 800 | 52 | 1 100 | 11 000 | 69 000 |
| ~1 600 | ~210–420 | 140–290 | 1 400–2 900 | 8 600–17 000 |

A 2 500-point transient — the size of this repository's ring oscillator — is therefore about 8 s
at 200 unknowns, 35 s at 400, 2 minutes at 800, and 9–18 minutes at 1 600.

### Where this repository's own decks sit (measured 2026-09-17, release binary)

| Deck | unknowns | devices | points | wall time |
|---|---:|---:|---:|---:|
| `rectifier.net` | 3 | 4 | 718 | 0.19 s |
| `actuator_plant.net` | 5 | 2 | 1 023 | 0.20 s |
| `ring_osc.net` | 8 | 13 | 2 243 | 0.27 s |
| `microring_thermal.net` | 18 | 8 | 2 013 | 0.52 s |
| `motorway_ramp.net` | 28 | 11 | 12 646 | 5.3 s |

Every validated deck is two orders of magnitude below the solver limit. At `dim < 50` the
factorization is irrelevant and the cost is **model evaluation × Newton iterations × points**:
`motorway_ramp` is the slowest deck here because of 12 646 timepoints, not because of 28
unknowns. Below ~50 unknowns the lever is the timestep; above ~400 it is the matrix. About 77 ms
of each wall time above is process start-up and model compilation, independent of circuit size.

### Constraints, as design rules

1. **Budget rows, not devices.** Count nets + sources + inductors + (internal nodes + `idt`
   sites + Laplace order) × instances before running.
2. **Size is no longer the wall it was.** On the sparse path (from 100 unknowns) a transient
   point costs 4–37 ms at 6 400 unknowns (ladder to mesh); on dense it was 52 ms at 800. What
   caps a large device circuit now is convergence, not the matrix: the PSP103 chain's `.op`
   solves to 5 286 unknowns since 1.10.1 and fails at 10 566 (above) — and model evaluation.
3. **Do not buy internal nodes you do not need.** Collapsing a parasitic node, or writing one
   low-order `laplace_vp` instead of a cascade, removes a row from every timepoint permanently.
4. **Stiffness costs more than size at small `dim`.** Events, fast switching and
   near-discontinuities multiply accepted *and rejected* steps; no solver change helps there.
5. **Pick the cheapest analysis that answers the question.** `.op` is one solve; `.ac` is one
   factorization per frequency with no Newton loop; `.noise` adds the adjoint. A 100-point AC
   sweep at 800 unknowns is ~8 s where the same circuit in transient is minutes.
6. **Memory is not the binding constraint.** Peak dense storage is three `dim × dim` matrices
   (the assembled Jacobian, the copy `faer` factorizes, and the factors), doubled for the complex
   matrix an AC or noise sweep solves: 15 MB at 800 unknowns, 2.4 GB at 10 000 — but no `Auto`
   run is dense above 100 unknowns. On the sparse path the whole process peaked at 30 MB for a
   2 646-unknown PSP103 chain (247 MB dense).
7. **Re-measure on the machine at hand.** `cargo run --release -p xtask -- bench-scale`; the
   debug profile is 10–60× slower and is not a number to plan with.

### What sparse changed

A circuit matrix has ~3–5 nonzeros per row. At 800 unknowns that is ~3 200 entries in a
640 000-entry matrix: **99.5% of the dense factorization multiplies zeros.** This section used
to predict that sparse LU would run at roughly O(dim^1.2…1.5) on circuit topologies. Measured
since it shipped (1.5.0–1.8.0, the section above): exponent ~1 on the ladder and 1.0–1.6 on
the mesh up to 6 402 unknowns, and 6–195× faster than dense at ~800 unknowns.

### The pre-flight estimate `va-cli` prints

Since v1.1.0 every `sim` run prints, before solving, what it is about to solve and roughly what
that costs (`va_cli::estimate`):

```
[va-cli] circuit: 11 device(s) (8 compiled), 28 unknown(s) (17 net(s) + 11 auxiliary row(s)), ~12601 points (adaptive, 12601 is the card's floor)
[va-cli] estimate: 0.8-39.1 s of solve, 18.8 kB of matrix — rough, dense LU scaled from bench-scale on an i7-1185G7
```

(That is a dense-path line; on the sparse path the second line reads `… of solve, sparse matrix —
rough, sparse LU scaled from bench-scale (RC ladder to RC mesh) …`, calibrated as described in
the section above.) The size half is exact. The dense cost half is a bracket, calibrated
against the table above: **inside
the measured range its two ends are the neighbouring measured rows** — a statement of fact, not a
fit — and **beyond the last row they are the exponent-2 and exponent-3 scalings** of it, the
bounds dense LU sits between (measured growth per doubling at the top of the table is 3.67×, i.e.
exponent 1.88, so the extrapolation errs slow). On top of the solve it adds a per-point term for
model evaluation, which the ladder cannot speak for because the ladder is linear primitives:
0.006–0.2 ms per compiled Verilog-A instance and 0.002–0.04 ms per non-linear reference
primitive, calibrated against the five decks above. The bracket is wide — up to 30× on a small
circuit, where iteration counts and model size dominate and nothing in a matrix dimension can
predict them — and it is checked against every deck in the table by
`estimate::tests::the_bracket_contains_every_measured_deck`, so it cannot silently stop covering
reality.

## ISCAS'85 c17: a transistor-level logic benchmark (1.11.0)

The first circuit of the ISCAS'85 suite, in the style of the c7552 deck from
`external/benchmarkExt/` (PSP103 `N` devices in gate `.subckt`s, RC-net `.subckt`s, `.include`d
model cards), which the netlist parser reads since 1.11.0: 24 PSP103 devices, 420 unknowns,
sparse LU. **No golden** — the check is the logic function (`circuits/benchmark/iscas85/`):

- `.op` over all 32 input vectors: **192/192 gate outputs correct**, highs ≥ 1.799992 V, lows
  ≤ 25.3 µV on a 1.8 V rail, 0.9–1.3 s per vector.
- `.tran 1p 12n`: 13 466 points in 4.1 min; all 5 528 settled output samples, over 11 input
  vectors, match NAND logic. The pre-flight estimate (0.08–2.4 min) undershot by ~1.7×.

**c432 (1.12.0):** 160 gates, 910 PSP103 devices, 15 416 unknowns, from the standard `.bench`
(cross-checked against an independent Verilog copy) by `gen_iscas.py`, missing gates composed
from c7552's cells. Five input vectors solve; four checked on all 160 gate outputs, **640/640
correct**. It needed a new DC rescue tier — the `gmin` ladder with each node's Newton step capped
at 0.5 V (`va_core::dc`, `RESCUE_NODE_STEP`) — because leakage-only nodes inside 4-high NMOS
stacks made the ladder cycle and the damped ladder stall. Wall time 2.9–6.9 min per `.op`, the
same deck varying >2× between runs; per Newton step, assembly (PSP103 evaluation) 218 ms (~70%),
sparse LU 88 ms (~28%). Details and the full table: `circuits/benchmark/iscas85/README.md`.

c7552 itself (~250 000 unknowns) is out of reach for size, not syntax: see the circuit-size
limit above.

## Reading a `--logfull` trace (1.13.0)

`va-cli sim … --logfull` prints a line on stderr for every DC Newton iteration, and one for every
solve stage (a `gmin` step, or the single plain solve). It covers the operating point of every
analysis and each `.dc` sweep point; it does **not** cover the transient integrator's own
per-timestep Newton loop. Nothing it prints changes the solve (`log_full_changes_no_number_on_either_path`
checks the answers bit for bit, both paths, all aids on).

```
[logfull] iter  aids=ladder+cap gmin=1.000e-3 iter=4 assemble_ms=88.599 solve_ms=60.726 trial_ms=0.000 nnz=20332 new_symbolic=0 scale=1.000e0 residual=4.663e-1 max_step=7.619e-2
[logfull] stage aids=ladder+cap gmin=1.000e-3 iterations=14 outcome="converged" assemble_ms=1111.4 solve_ms=538.6 trial_ms=0.0 wall_ms=1660.4 new_symbolic=2 unknowns=5286 instances=483
```

| field | meaning |
|---|---|
| `aids` | the stage's convergence aids — `plain`, or `ladder` / `cap` / `damp` joined by `+`. For a rescued `.op` this names the tier of `va_core::dc`'s rescue: `plain` → `ladder+cap` → `ladder` → `ladder+damp` (the capped ladder before the plain one since 1.23.0, `docs/proposals/dc-rescue.md`) |
| `gmin` | the shunt this stage runs at (`0` for the plain solve and the ladder's last stage) |
| `assemble_ms` | evaluating every instance and stamping, plus the `gmin` shunt — the model-evaluation cost |
| `solve_ms` | the linear solve: dense LU, or sparse numeric LU (plus a symbolic one when `new_symbolic=1`) |
| `trial_ms` | the line search's extra assemblies (non-zero only with `damp`) |
| `nnz` | stored Jacobian entries on the sparse path; `dense` on the dense path |
| `new_symbolic` | the pattern grew, so this solve redid the symbolic factorization |
| `btf` | (`iter`) the block-triangular solve answered (`1`), handed the system to `faer` (`0`: blocks too large, a singular block, or a failed residual check), or was not in use (`-`: dense, or `VA_BTF=off`) — since 1.21.0 |
| `btf_solves`, `btf_fallbacks` | (`stage`) the same, counted over the stage |
| `scale`, `residual`, `max_step` | the step fraction taken (below 1 only with `damp`), the residual ∞-norm the step was solved from, and the largest change actually applied |
| `iterations`, `outcome` | per stage: the iterations it took, and `converged`, `no convergence` or `failed: <error>` |
| `instances` | every device after flattening, compiled or not — divide by the compiled count `va-cli`'s `circuit:` line gives for a per-model cost |
| `stamp_lookups` | (1.14.0) hash lookups by sparse stamping, one per Jacobian/`dcharge` stamp; `0` on the dense path (`va_core::counters`) |
| `ctx_maps_built` | (1.14.0) lookup maps a compiled instance builds at the start of each evaluation — three per call, so ÷3 = compiled-instance evaluations (`va_codegen::counters`) |
| `probe_allocs` | (1.14.0) gradient vectors allocated by a `V(…)`, `I(…)` or `idt` read, one per read, each the instance's own size |
| `ctx_map_lookups` | (1.14.0) hash lookups into those per-call maps and sets |
| `grad_allocs` | (1.14.0) dense gradient vectors the dual-number arithmetic allocates — one per operation on a value that depends on an unknown, and one per copy of such a value; probe reads excluded (they are `probe_allocs`) |
| `grad_clones` | (1.14.0) copies of a gradient (reading a local variable copies its value); since 1.14.0+1 a shared buffer, not an allocation |
| `grad_in_place` | (1.14.0+2) results written into an operand's own gradient buffer because the operation owned it and nothing else shared it — each an allocation avoided |

Each counter reports its **increase** over the line's iteration or stage, and a closing
`[logfull] run` line gives the whole process's totals, printed whether the run succeeded or not.
The run line also covers work that has no per-iteration line, such as the transient
integrator's timesteps. While `--logfull` is off the counters cost one relaxed atomic load each;
while it is on, one relaxed atomic add per event.

**What the counters found** (1.14.0, PSP103 chains of 10–320 instances and c432; the same rates
in every circuit, so properties of the model): per PSP103 evaluation, **~1 850 gradient
allocations, ~47% of them clones**, against 12 probe allocations, 18 context-map lookups, 3 map
builds, and ~79 stamp lookups per instance per iteration.

**A count is not a heap allocation, and not a cost.** `grad_allocs` counts gradient *vectors*.
The first attempt at removing the clones (1.14.0+1, as `Rc<Vec<f64>>`) cut it 47% and gained
nothing — because that representation takes two heap allocations per vector, so real heap
allocations per PSP103 `load()` went *up*, 1 845 → 1 946. A sampling profile found it (~29% of
the time inside the allocator, called from the dual-number operators); `Rc<[f64]>`, one
allocation per vector, took them to 988 and `load()` from 173 to 130 µs (`xtask bench-model`,
medians of seven alternating runs). 1.14.0+2 then wrote results into operands' own buffers where
unshared: 443 allocations, 101 µs — 42% below 1.14.0 — and on the 160-stage chain, assembly per
Newton iteration 65 → 40 ms. Count heap allocations with an allocator, and time a change,
before believing either a count or an estimate.

A failed iteration's line is not printed (it has no step), but its assembly and solve time are in
its stage's totals.

### What it measured (1.13.0, release, i7-1185G7, one run each)

`.op` of the PSP103 inverter chains (`docs/validation.md` § "A real device circuit") and of ISCAS'85
c432; medians over every iteration of the run. "Per instance" divides assembly by the compiled
(PSP103) instances; the R/C/V devices are in the numerator too, and cheap.

| circuit | unknowns | PSP103 | iterations | assemble | per instance | solve | assembly share | wall |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| chain, 40 stages | 1 326 | 80 | 118 | 14.1 ms | 176 µs | 0.62 ms | 96% | 2.2 s |
| chain, 80 | 2 646 | 160 | 137 | 28.2 ms | 176 µs | 8.1 ms | 77% | 5.5 s |
| chain, 95 | 3 141 | 190 | 151 | 34.3 ms | 181 µs | 11.3 ms | 75% | 7.5 s |
| chain, 160 | 5 286 | 320 | 148 | 65.2 ms | 204 µs | 22.8 ms | 72% | 14.3 s |
| c432 | 15 416 | 910 | 557 | 184 ms | 202 µs | 44 ms | 77% | 151 s |

- **Model evaluation costs ~0.18–0.20 ms per PSP103 instance per iteration, flat in circuit
  size** — 80 to 910 instances, two different model cards, within ~15%, which is about the
  run-to-run spread (the 160-stage chain measured 188 µs in a second run). Nothing in the
  assembly scales per instance with the circuit: a stamp is one hash lookup, and the
  circuit-sized work (clearing the system, the `gmin` shunt) is once per assembly, not per
  instance. An earlier figure of 0.7–0.8 ms per instance for the chains (2026-09-23) came
  from a combined step timer that included the solve, not from the model.
- **Assembly is 72–96% of every run;** the sparse solve is the rest. The solve does not scale
  smoothly: 0.62 ms at 1 326 unknowns but 8.1 ms at 2 646 (13× for 2×), then about linear to
  5 286. Not diagnosed — fill-in and ordering are the suspects; the trace does not report the
  factors' size.
- **Where c432's iterations go:** the plain solve fails in 3; the plain `gmin` ladder converges
  5 stages, then spends its full 150-iteration budget cycling at `gmin` = 3.2e-5 (48 s) and fails
  — 265 iterations, 75 s, half the run, discarded; the node-capped ladder then solves in 290
  iterations over 31 stages (73 s): 41 and 59 at `gmin` = 6.3e-5 and 3.2e-5 — the very stages
  where the plain ladder struggled and cycled — about 5 through the middle of the ladder, and 2–3
  at the small end. The first tier's cost is the price of 1.12.0's ordering, which keeps anything the plain
  ladder solves bit-identical.

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
