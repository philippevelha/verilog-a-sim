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

### Current status (2026-06-29)

**No rung is "passed" yet:** `va-harness`, `golden/`, and the CLI are still stubs, so there is
no harness-vs-ngspice comparison in place. What *does* work today is validated against analytic
values and inline unit tests, not golden:

- **Rung 1 (resistor divider, DC):** solves in `va-core` to the analytic midpoint (< 1e-9). The
  ngspice-golden gate awaits T6 (netlist + CLI + harness).
- **Rung 2 (diode I–V):** the constituent pieces work in isolation — `va-frontend` elaborates
  `diode.va`, `va-codegen` differentiates it (AD vs FD green), and `va-core` converges a
  nonlinear diode–resistor clamp — but they are not yet wired by a netlist driver or gated
  against golden.
- **Rungs 3–6:** not started (T4/T5/T6 crates are stubs); the T2 charge channel that rung 3
  depends on is ready.

See `roadmap.md` → *Status at a glance* for the per-phase breakdown.

## The model zoo

| Model         | File                  | Status   | Reference (`va-abi`) | Elaborates (T1) | Generated (T2) |
|---------------|-----------------------|----------|----------------------|-----------------|----------------|
| resistor      | `models/resistor.va`  | bring-up | ✅                   | ✅              | ✅ (matches ref stamp) |
| capacitor     | `models/capacitor.va` | bring-up | ✅                   | ✅              | ✅ (charge channel)    |
| diode         | `models/diode.va`     | bring-up | ✅                   | ✅              | ✅ (AD vs FD < 1e-5)   |

Reference (hand-written) implementations of all three ship in `va-abi` so the core can solve
before the compiler path is ready. As of 2026-06-29 the generated models reproduce the
reference stamps (resistor hand-checked, diode against finite differences), but generated and
reference instances are not yet exercised through a full netlist+harness run. The convergence
metric only ever needs to go up.
