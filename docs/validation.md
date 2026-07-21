# Validation & the Model Zoo

Reference simulator: **QSPICE** (originally ngspice; switched 2026-07-13 to match the actual
dev environment) — an oracle only; we are not building on it. `va-harness` runs the pipeline
and compares to committed `golden/` outputs.

## Metrics & default tolerances

| Analysis     | Metric                                              | Default tolerance |
|--------------|-----------------------------------------------------|-------------------|
| DC           | max relative I–V error on the operating point/sweep | ≤ 1e-4            |
| Transient    | waveform RMS error (after shared-timebase resample) | ≤ 1e-3            |
| AC           | magnitude/phase error within a stated band          | band-dependent    |
| Convergence  | fraction of zoo circuits that reach a solution      | track upward      |

These mirror the constants in `va-harness` (`tol::DC_REL`, `tol::TRAN_RMS`). Tune here as the
zoo grows; record any change with its justification.

**Updated 2026-07-18: three of the four metrics are real, verified implementations now,** not
`todo!()` stubs (AC/noise remains a stretch goal per `CLAUDE.md` §1 and `t5-acnoise`'s own
honest status — see `docs/roadmap.md`'s T5 section):

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

`golden/{divider,mos_dc,diode_iv,rc_step,rectifier,ring_osc}.golden` are all real, QSPICE-
generated data (`cargo xtask gen-golden`) — every one of `xtask`'s known circuits now has a
committed golden reference, closing the "which circuits aren't regenerated yet" gap this file
used to track.

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
