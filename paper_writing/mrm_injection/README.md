# mrm_pin — carrier-injection (p-i-n) micro-ring modulator, Verilog-A

Second model of the series, kept separate from the simple TCMT model (`mrm_tcmt/`).
Reference: B. Wang et al., "A Compact Verilog-A Model of Silicon Carrier-Injection Ring
Modulators for Optical Interconnect Transceiver Circuit Design", J. Lightwave Technol. 34(12),
2996–3005 (2016), below **[W]**. Its electrical part is Strollo's power p-i-n model
(IEEE TPEL 12(1), 1997) at tenth order.

## Files

| Path | What |
|---|---|
| `mrm_pin.va` | the model |
| `calib/calibrate_vact.py` | calibration of the active volume on [W] Fig. 10 |
| `ref_data/` | curves digitised from [W] Figs. 6, 7, 10 (CSV) |
| `tests/` | testbenches and scripts: DC, Fig. 7, Fig. 10, eyes (Figs. 12–15, 21a), Eq. 4 check, figures |
| `vasim/` | verilog-a-sim's `disciplines.vams` / `constants.vams` |
| `.spiceinit` | `set num_threads=1` for ngspice |

Ports are the same as `mrm_tcmt.va`: `(anode, cathode, htp, htn, o_in_re, o_in_im,
o_thru_re, o_thru_im, o_drop_re, o_drop_im, wl, dt)`. Optical pins carry the field envelope in
√mW (1 V on `o_in_re` = 1 mW laser), `V(wl)` = laser wavelength in nm, `dt` = thermal node.

## What is implemented

| Block | Source | Notes |
|---|---|---|
| p-i-n network: Rc, Repi ∥ (Rlim + Gmod), Gpin, junction law, tenth-order ladder, GE, G3 | [W] Fig. 4, Table I | element by element; the ladder is the continued fraction of (1+sτ)·tanh z / z, z² = (1+sτ)T0/τ |
| free carriers: τ_c dX/dt + X = I_src, Q_free = τ_c X | [W] Fig. 3 | integrated in an exactly equivalent derivative-free form (see Numerics) |
| plasma dispersion at 1.31 µm | [W] Eqs. (2)–(3) | ΔNe = ΔNh, Γ = 1 as in [W] |
| dynamic ring | [W] Eqs. (4)–(6), σ = 0.9944, a = 0.9931, n_eff = 2.5188, R = 5 µm | coupled-mode (TCMT) form, because Eq. (4) needs `absdelay()` (not in OpenVAF, refused in transient by verilog-a-sim); checked against Eq. (4) |
| drop port, depletion C, pad C, heater, self-heating | not in [W] | all OFF by default |

## Choices not given by [W] (flagged)

| Item | Value | How it was set |
|---|---|---|
| `Tvt` — temperature for V_T in the junction law | 331 K (58 °C) | Table I reproduces [W] Fig. 6 only with V_T at ~58 °C: RMS 12 % vs [W]'s simulated curve; at 27 °C the current is 4× too high at 0.7–0.9 V. `Tvt=0` uses the circuit temperature. |
| `V_act` — volume holding the free carriers | 4.41 µm³ | calibrated so the model gives [W]'s −90.7 pm shift at 0.86 V DC (Fig. 10). ≈ 0.14 µm² × 31.4 µm, a plausible rib-waveguide region. |
| `fc_src` — current integrated into free carriers | 1 = I_epi only | [W] integrates the total current I = I_epi + I_r. With correct numerics that keeps the anode-recombination charge for τ = 1 ns and the 80 ps optimum eye of Fig. 13(b) stays half-closed (ER 4.6 dB). Excluding I_r (it recombines in the p⁺ anode) reproduces Fig. 13(b) and most trends — see table below. `fc_src=0` with `V_act=4.581` gives the literal reading. |
| `lambda0` | 1318.812 nm | digitised from Fig. 10 |
| `ng` | = n_eff | [W]'s spectrum fit has no dispersion; this reproduces its linewidth but implies FSR = 22 nm. Set `ng` to the real group index for the real FSR (then re-fit σ, a). |
| thermal values | placeholders | not used by default (`self_heat=0`) |
| regularisation | `$limit(pnjlim)`, smooth max(PHI−Vj,0), clamp of the moving-boundary root at 0.9, smooth ΔN ≥ 0 | convergence only |

## Validation

| Check | Result |
|---|---|
| DC I–V vs [W] Fig. 6 | RMS 12 % vs [W] simulation, 22 % vs measurement (0.66–2 V) |
| Spectrum 0 V vs [W] Eq. (5) | max \|ΔT\| 8·10⁻⁵ |
| Spectrum 0.86 V (not used in calibration: depth) | −16.0 dB vs [W] −15.4 dB (meas. −14.9); RMS 0.6–0.8 dB vs measured curves |
| TCMT vs [W] Eq. (4), same carrier waveform | 1.7–2.2 µW RMS (max 12 µW) on 10–1500 µW, four eye cases |
| ngspice vs verilog-a-sim, 9 eye cases | eye heights within 0.3 µW |
| 10 Gb/s current vs [W] Fig. 7 | **not reproduced** (RMS 0.5–0.75 mA on ±2 mA): [W]'s near-sinusoidal current needs setup elements it does not give (drive at the probe, source impedance, ~60 fF of pad/probe capacitance) — sensitivity in `tests/validation_static.png` |

Eyes at 8 Gb/s, PRBS7, 2 Vpp, 40 ps edges, laser at the 0 V resonance, 1 mW, 20 GHz
Bessel-Thomson receiver (the paper's scope):

| Case [W] | Paper | Model eye height / ER | Agreement |
|---|---|---|---|
| Fig. 12, NRZ, no pre-emphasis | closed | closed | ✓ |
| Fig. 13(b), 80 ps, 0.8 V depth, 0.7 V bias | open (optimum) | 792 µW / 12.1 dB | ✓ |
| Fig. 13(a), 40 ps | degraded | 844 µW / 13.8 dB | ✗ |
| Fig. 14(a), depth 0.9 V | degraded | 292 µW | ✓ |
| Fig. 14(b), depth 0.7 V: slow fall | degraded | 784 µW, P0 70 vs 54 µW | weak |
| Fig. 15(a), bias 0.75 V: slow fall | degraded | 772 µW, P0 86 µW | ✓ (mild) |
| Fig. 15(b), bias 0.65 V: slow rise | degraded | 702 µW | ✓ |
| Fig. 21(a), 15 Gb/s asymmetric vs symmetric | asymmetric better | 428 vs 467 µW | ✗ |

The two misses have the same signature: in this implementation the rising edge depends less
on the charge left by the low level than in [W]. Candidates, none verifiable from the paper:
setup parasitics (same as Fig. 7), V_act set from a single DC point, the
missing depletion capacitance.

## Numerics (read before running ngspice)

* The direct form of the carrier filter contains two `ddt()` terms of the exponential
  V2 = q0/τ that cancel analytically; ngspice/OSDI lost up to 4× of the free charge after a
  falling edge with it. The model integrates the equivalent form
  `W = X − (τ/τ_c)(V2 − V3)`, `τ_c dW/dt + W = (1 − τ/τ_c)(V2 − V3) [+ I_r]`, which has no
  derivative of the exponential. verilog-a-sim was correct with both forms.
  The direct form is kept as `tests/mrm_pin_direct.va` (a test variant, not the model) with the
  ngspice deck `tests/pulse_direct_ng.cir`; `tests/fc_form_check.py` compares the forms.
  Measured 2026-10-24 (verilog-a-sim 2.42.0+3, single pre-emphasis bit): direct = W form to
  1e-6 mA in X (print precision), post-fall charge ratio 1.0000. The ngspice 4x has not been
  re-measured since the direct form was replaced: `openvaf-r mrm_pin_direct.va`, then
  `ngspice -b pulse_direct_ng.cir`, then `python fc_form_check.py`.
* The ladder state is stored in µA (`` `SC = 1e6``). In mA, the small shunt nodes sat below
  ngspice's default `vntol` (1 µV) and Newton stopped early (6.8 % current error at defaults).
  Discipline `abstol` would not help in ngspice: its OSDI interface passes only the global
  reltol/vntol/abstol to the model, not the nature tolerances.
* Remaining error with ngspice defaults: 0.4 %. The testbenches use `option reltol=1e-5`
  (0.08 %). verilog-a-sim defaults are converged (Newton reltol 1e-9).
* Static/JIT path in verilog-a-sim: `xtask sched-check` → static.

## Running

```
openvaf-r mrm_pin.va                         # ngspice >= 44 (OSDI 0.4)
cd tests
ngspice -b tb_dc_iv.cir; ngspice -b tb_fig10.cir; python3 fig7_variants.py
python3 eyes.py --sim ngspice                # or --sim va (verilog-a-sim, VA_JIT=on)
python3 eq4_check.py fig13b_pe80_va.npz      # TCMT vs Eq. (4)
python3 plot_validation.py; python3 plot_eyes.py va
cd ../calib && python3 calibrate_vact.py 1   # re-run after changing electrical parameters
```
Tool paths: `NGSPICE`, `VACLI`, `OPENVAF`.
