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

`golden/*.golden` — all twenty-four — are real, QSPICE-generated data (`cargo xtask
gen-golden`): `{divider, vcvs_amp, cccs_mirror, mos_dc, diode_iv, diode_iv_params, diode_clamp,
rc_step, rc_discharge, rl_decay, rlc_ring, rectifier, ring_osc, abstime_ramp, rc_ac, rc_ac_lin,
rc_ac_oct, diode_ac, laplace_ac, diode_noise, resistor_noise_va, diode_flicker,
resistor_noise_table, resistor_noise_table_log}`. Every one of `xtask`'s known circuits
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

**What this gate does not cover:** DC and transient, where a Laplace filter still evaluates to
`H(0)`. That is unchanged from before Tier C and is a stated limitation at the construct, not an
oversight — a time-domain Laplace filter is a convolution.

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
