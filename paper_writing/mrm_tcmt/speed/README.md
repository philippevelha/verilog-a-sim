# Speed study: loaded Q × modulation efficiency

Question: how far can the MRM + driver go in symbol rate (NRZ 50/100 GBd, PAM4 50 GBd) before
the ring's dynamic response (photon lifetime, transition-dependent ringing) closes the eye,
and how much do loaded Q and modulation efficiency buy back?

## Files

| File | What |
|---|---|
| `sweep_q_eta.py` | the sweep; `--sim ngspice` or `--sim va` (verilog-a-sim, `VA_JIT=on`), `--quick` for a 12-point smoke test, `--jobs N` |
| `plot_q_eta.py` | maps + optimum CSV for one simulator (`--sim`), and the va-sim vs ngspice comparison when both JSON files exist |
| `sweep_q_eta_ngspice.json`, `sweep_q_eta_va.json` | every run: format, Q_L, η, detuning, eye metrics, points, wall time |
| `q_eta_map_<sim>.png`, `q_eta_best_<sim>.csv` | optimum per (format, Q_L, η) |
| `q_eta_compare.png` | the two simulators run by run |
| `sweep.py`, `attribute.py` | the earlier rate scan and the quasi-static attribution |

Re-run:
```
python3 sweep_q_eta.py --sim ngspice     # ~4 min, 2 cores
python3 sweep_q_eta.py --sim va          # ~40 min, 2 cores
python3 plot_q_eta.py --sim va
```
Tool paths: `NGSPICE`, `VACLI`, `OPENVAF`. Models are taken from the folder above
(`../mrm_tcmt.va`, `../mrm_driver.va`); verilog-a-sim's headers from `../vasim/`.

## Grid

- Q_i = 20 000 fixed; Q_c = 22 000 … 3 000 → Q_L = 10 476, 8 235, 6 667, 5 714, 4 615, 3 333, 2 609
- η = 10 … 60 pm/V (small-signal at 0 V; large-signal follows the junction C–V law)
- laser detuning = −0.9 … +0.9 × FWHM (18 points, finer near resonance)
- 3 formats × 7 × 6 × 18 = 2 268 transients per simulator

## Assumptions (not modelled)

- C_j0 (20 fF), R_s (100 Ω) and Q_i do not change with η. In a real junction higher efficiency
  usually costs capacitance and loss; this sweep isolates the optical/dynamic trade-off.
- Driver: linear DAC mode, 2 real poles at 50 GHz overall, 50 Ω, 0–2 V reverse bias.
- Self-heating off (detuning is the true one), 1 mW laser, PRBS9 (PAM4: 256 Gray symbols), no noise.
- Metric: worst inner eye height (lowest of the M−1 eyes) at the best common sampling phase;
  not TDECQ.
