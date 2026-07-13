# Validation & the Model Zoo

Reference simulator: **ngspice** — an oracle only; we are not building on it. `va-harness`
runs the pipeline and compares to committed `golden/` outputs.

## Metrics & default tolerances

| Analysis     | Metric                                              | Default tolerance |
|--------------|-----------------------------------------------------|-------------------|
| DC           | max relative I–V error on the operating point/sweep | ≤ 1e-4            |
| Transient    | waveform RMS error (after shared-timebase resample) | ≤ 1e-3            |
| AC           | magnitude/phase error within a stated band          | band-dependent    |
| Convergence  | fraction of zoo circuits that reach a solution      | track upward      |

These mirror the constants in `va-harness` (`tol::DC_REL`, `tol::TRAN_RMS`). Tune here as the
zoo grows; record any change with its justification.

## Bring-up ladder

Each rung is a checkpoint; it is "passed" only when `va-harness` is green against golden:

1. resistor divider (DC)
2. diode I–V (DC sweep)
3. RC transient
4. diode rectifier (transient)
5. a MOS DC
6. ring oscillator (transient)

### Current status (updated 2026-07-13)

**No rung is "passed" yet:** `va-harness`/`golden/`/`xtask gen-golden` are still stubs (no
ngspice available in this environment to generate golden from, either), so there is still no
harness-vs-ngspice comparison in place for *any* rung. What has changed since the table above
was first written is *implementation reach* — **every rung now solves through the real `va-cli`
pipeline**, validated against analytic/hand-derived values and inline unit tests, not golden:

- **Rung 1 (resistor divider, DC):** `cargo run -p va-cli -- sim circuits/divider.net` solves to
  the analytic midpoint (< 1e-9).
- **Rung 2 (diode I–V, DC sweep):** `cargo run -p va-cli -- sim circuits/diode_iv.net --model
  models/diode.va` sweeps `V1` 0–0.6 V and matches the closed-form Shockley law
  `Id(V)=Is·(exp(V/(N·vt))−1)` at every point.
- **Rung 3 (RC transient):** `cargo run -p va-cli -- sim circuits/rc_step.net --tran` matches the
  analytic `V(t)=Vs·(1−e^{−t/RC})` closely.
- **Rung 4 (diode rectifier, transient):** `cargo run -p va-cli -- sim circuits/rectifier.net
  --tran` produces a correct half-wave-rectified/RC-filtered waveform.
- **Rung 5 (a MOS, DC):** `cargo run -p va-cli -- sim circuits/mos_dc.net --model
  models/mosfet.va` solves an NMOS common-source bias point to a hand-derived fixed point
  (< 1e-6) — the first rung to exercise a real `.va` model file through the full
  frontend→codegen→core path, not just a hand-written `va-abi` reference.
- **Rung 6 (ring oscillator, transient):** `cargo test -p va-transient
  ring_oscillator_sustains_oscillation` shows real, growing oscillation from an unstable DC
  equilibrium (`va-abi::Bjt`, a hand-written reference — no netlist wiring yet).

Every ngspice-golden gate still awaits T6.3. See `roadmap.md`'s *Status at a glance* and its
*Cross-thesis milestones* ladder table for the authoritative, continuously-updated per-rung
detail — this section is a summary, not the source of truth.

## The model zoo

| Model         | File                  | Status   | Reference (`va-abi`) | Elaborates (T1) | Generated (T2) |
|---------------|-----------------------|----------|----------------------|-----------------|----------------|
| resistor      | `models/resistor.va`  | bring-up | ✅                   | ✅              | ✅ (matches ref stamp) |
| capacitor     | `models/capacitor.va` | bring-up | ✅                   | ✅              | ✅ (charge channel)    |
| diode         | `models/diode.va`     | bring-up | ✅                   | ✅              | ✅ (AD vs FD < 1e-5)   |
| mosfet (NMOS, Level-1) | `models/mosfet.va` | ladder rung 5 | — (no hand-written `va-abi` reference; solved entirely via the generated model) | ✅ | ✅ (solves `circuits/mos_dc.net` to a hand-derived fixed point < 1e-6) |
| bjt (NPN, simplified Ebers-Moll) | `crates/va-abi/src/reference/bjt.rs` | ladder rung 6 | ✅ (hand-written only — no `.va` source, no netlist wiring) | — | — |

Reference (hand-written) implementations of resistor/capacitor/diode ship in `va-abi` so the
core can solve before the compiler path is ready. As of 2026-06-29 the generated models
reproduce the reference stamps (resistor hand-checked, diode against finite differences); as of
2026-07-12, `mosfet.va` and the ring-oscillator's `Bjt` extend the zoo to nonlinear multi-terminal
devices, though `Bjt` still has no `.va` counterpart or netlist grammar (§ `roadmap.md`'s rung 5
closure note) and `mosfet.va` has no hand-written `va-abi` reference to cross-check against — its
correctness is checked against a hand-derived analytic operating point instead (`cargo test -p
va-cli mos_dc_solves_through_codegen_pipeline`). The convergence metric only ever needs to go up.
