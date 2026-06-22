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

## The model zoo

| Model         | File                  | Status     |
|---------------|-----------------------|------------|
| resistor      | `models/resistor.va`  | bring-up   |
| capacitor     | `models/capacitor.va` | bring-up   |
| diode         | `models/diode.va`     | bring-up   |

Reference (hand-written) implementations of all three ship in `va-abi` so the core can solve
before the compiler path is ready. The convergence metric only ever needs to go up.
