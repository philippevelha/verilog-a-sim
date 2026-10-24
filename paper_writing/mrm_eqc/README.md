# mrm_eqc — Verilog-A models from Bao et al., JLT 42(14), 2024 ([B])

*A Comprehensive Equivalent Circuit Model of Silicon Microring Modulators for Photonics-Electronics Co-design.*
This is the third MRM package. The others are `mrm_tcmt` (the simple model) and `mrm_injection` (Wang 2016 carrier injection). A depletion-ring model is planned and will get its own folder.

Every simulation runs in **verilog-a-sim** (`va-cli`, VA_JIT=on) and in **OpenVAF-reloaded (openvaf-r) + ngspice-47** (`set num_threads=1`, `reltol=1e-5`). Each run records wall time and peak RSS with `/usr/bin/time`.

## 1. Models

| file | what it implements from [B] | ports |
|---|---|---|
| `mrm_eqc_rlc.va` | **Linear small-signal equivalent circuit** (Sec. III, Fig. 3). Electrical parasitics per segment: Cpad, Rsi‖Csi, Rs + Cj. Optical part: R1‖C ‖ (R2 + L) driven by a VCCS g·V(Cj), using Eqs. 6–9. N segments (1 or 2) share the optical RLC. | `in1, in2, out` (out = Δ through power in mW, read as a voltage) |
| `mrm_eqc_ls.va` | **Large-signal model**. Same parasitics plus TCMT in √mW with complex field I/O. Resonance shift is c1·V + c2·V² per segment, weighted by s_k (Eq. 14/15). Intrinsic loss τl(V) follows Eqs. 16/17. **Thermal node** covers heater (Eq. 11) and self-heating (Eq. 12, driven by the circulating power). Wavelength is an input pin, so a DC sweep is a spectrum. | `in1,in2,ref, htp,htn, o_in_re/im, o_thru_re/im, o_drop_re/im, wl, dt` (+ internal `pmon` = through power, mW) |

**Mapping checks.**
- The RLC element values are the exact small-signal linearisation of TCMT: poles at −1/τ ± jΔω, zero at −2/τl.
- In t1, `mrm_eqc_rlc` and `mrm_eqc_ls` agree to **0.06–0.16 %** (6 operating points, both simulators). That holds when loss modulation is frozen (`loss_mod=0`).
- Including τl(V) modulation, which the paper's RLC omits, changes the response by **1.5–5 %**.

**Choices I made — flagged:**
- `op_mode` in the RLC model. Mode 0 means the detuning is given. Mode 1 solves the laser wavelength by an unrolled 50-step bisection that includes self-heating.
- Combining τl across segments: I sum the loss-rate changes, weighted by s_k/Σs. [B] does not say how.
- Thermal values not given in [B]: `dlam_dT` = 0.08 nm/K, `tau_th` = 1 µs, `Rh` = 1 kΩ. Only the products dλ/dP_heater = 0.150 nm/mW and dλ/dP_in = 0.215 nm/mW are fitted (from the Fig. 7 and Fig. 6b points).
- `lam0` in the cards is the measured 0.025 mW dip minus 0.215·0.025 nm, so the cold resonance excludes the measurement self-heating.

The parameter cards (`SINGLE`, `DUAL`) are in `tests/harness.py`. They use Table I/II values plus the c1, c2, s1, s2 fits to Fig. 4.

## 2. Validation against [B] (both simulators give identical numbers to the digits shown)

| test | result |
|---|---|
| **t3** Fig. 4(a) resonances 0/−1/−2/−3 V | match within 0.3 pm (these are fitted, so this only checks consistency) |
| **t3** Fig. 4(b) single segments | match within 0.2 pm. Both segments at −4 V: model 1551.4740 nm vs paper 1551.4621 nm. The paper's shift is **sub-additive** (61 vs 72.9 pm), which Eq. 15 cannot reproduce. |
| **t3** dip depths | **model 2–10 dB too deep**. Single segment: −25.0…−30.8 dB vs −22.9…−21.2 dB. Dual: −21.4…−23.0 dB vs −15.6…−14.0 dB. Table II has τl < τe, so the ring is under-coupled and the dip must *deepen* with reverse bias. The measured dip *shallows*. **The paper's numbers contradict each other here.** |
| **t3** slopes | self-heating 0.2149 nm/mW and heater 0.1500 nm/mW, both fitted |
| **t4** Fig. 6(a) shape (different, lower-Q ring, Q_L ≈ 2700, fitted at 0.025 mW) | 1 mW dip position right (1551.218 vs 1551.219 nm). Shape RMS 2.49 dB vs 1.01 dB at 0.025 mW. The measured 1 mW curve is strongly triangular; the model is more symmetric. There is no hysteresis at this Q and power. See `tests/t4_overlay.png`. |
| **t2** Fig. 8 with the detunings as stated (from the heated resonance) | f3dB ≈ 15 GHz vs 32 GHz in the paper. **Not reproducible.** |
| **t2b** Fig. 8 assuming offsets from the *cold* resonance and 0.6 mW self-heating (P fitted once on 8b) | 8(b) peaks within 0.15 dB (RMS 0.13 dB). 8(a) peak +0.9/+2.3/+3.3 dB vs +0.8/+1.7/+2.7. f3dB 28/36/41 vs 32/35/41 GHz. The model rolls off faster above ≈30 GHz than the measured curves. See `tests/t2b_fig8_overlay.png`. |

**Text vs figures in [B]:** the text gives 0.12 nm/mW for the heater, but the Fig. 7 points give 0.150 nm/mW. R = 0.519 /mW in Eq. 12 cannot be used as written, so I used the Fig. 6b slope.

## 3. Eyes (t5) — conditions are mine, not [B]'s

Conditions: 2.5 Vpp per driver from −0.5 V to −3.0 V, 50 Ω source, 8 ps edges, no FFE. Laser 1 mW at the blue-side wavelength of maximum static OMA (self-heating included). Pattern derived from PRBS9.

The dual-segment case is an optical DAC with two NRZ drivers, and the MSB goes to the *stronger* segment. In [B] Fig. 4(b) the segment labelled "LSB" shifts more (s = 0.642 vs 0.407). For PAM8 the MSB is NRZ and the two LSBs are PAM4, with an efficiency ratio of 3:2.

| case | worst eye opening (% of ideal) | RLM | ngspice vs va waveform (RMS ΔP/P̄) |
|---|---|---|---|
| single PAM4 80 Gb/s | 53 % | 0.84 | 5e-5 |
| dual PAM4 80 Gb/s | 42 % | 0.73 | 5e-5 |
| single PAM4 106 Gb/s | 31 % | 0.83 | 6e-5 |
| dual PAM4 106 Gb/s | 17 % | 0.70 | 6e-5 |
| dual PAM8 212 Gb/s | **closed** | 0.65 | 6e-5 |

Without FFE and with these drive conditions, PAM8 at 212 Gb/s is closed. [B] reports an open PAM8 eye; their drive swing, laser power and detuning are not stated. The dual DAC has a lower RLM than single-segment PAM4. That comes from the unequal segment efficiencies and the nonlinear spectral transfer, which equal NRZ swings do not compensate. See `tests/t5_eyes.png`.

## 4. Wall time and peak memory (`tests/benchmark.py` → `benchmark.json`)

| test | model | ngspice: runs / total wall / peak RSS | verilog-a-sim: runs / total wall / peak RSS |
|---|---|---|---|
| t1 AC cross-check | RLC | 6 / <0.01 s / 10.7 MB | 6 / 0.01 s / 9.7 MB |
| t1 AC cross-check | LS | 12 / <0.01 s / 11.0 MB | 12 / 0.18 s / 11.0 MB |
| t2 EO response | RLC + LS | 12 / <0.01 s / 11.0 MB | 12 / 0.14 s / 11.0 MB |
| t2b Fig. 8 (final) | LS | 6 / <0.01 s / 11.0 MB | 6 / 0.06 s / 10.9 MB |
| t3 static DC sweeps (2000 pts each) | LS | 16 / 0.17 s / 11.4 MB | 16 / 2.37 s / 11.1 MB |
| t4 self-heating sweeps (2400 pts) | LS | 4 / 0.08 s / 11.3 MB | 4 / 0.63 s / 11.0 MB |
| t5 eyes (7.7–12.9 k time points) | LS | 5 / 0.37 s / 13.6 MB | 5 / 4.07 s / 11.0 MB |

Timer resolution is 10 ms, so "<0.01 s" means below the resolution. Fixed start-up cost: ngspice under 10 ms, va-cli about 30 ms (it parses and JIT-compiles both modules).

On transient and DC sweeps, verilog-a-sim takes **about 10× longer per time point** than ngspice: about 60 µs vs 4–5 µs. It uses about 20 % less peak memory. The waveforms agree to 5–6e-5 relative.

## 5. Compiler note (`compiler_note/`)

A `for` loop with 60 iterations (a bisection) in the same module as the inductor's `ddt` was reported to make OpenVAF drop the inductor's reactive term: the node ended up pinned at 0 V in AC. Unrolling the loop fixes it (`mrm_eqc_rlc_loop2.va` works; `_loop60.va` fails), so the shipped models use an unrolled macro. `ng_ac_10GHz.cir` is the ngspice/OpenVAF reproduction; it has not been re-run since.

**verilog-a-sim is not affected** (corrected 2026-10-24; an earlier version of this note said both compilers were). Checked with verilog-a-sim 2.42.0+6, `VA_JIT` off and on, AC 0.1–100 GHz, 13 points, using `va_ac_10GHz.net` (the same parameters as `ng_ac_10GHz.cir`):
- `loop2` and `loop60` give byte-identical output, and the inductor node is not pinned (|V(nind)| = 4.7e-2 at 5.6 GHz);
- `loop60` equals the shipped, unrolled `mrm_eqc_rlc.va` (`op_mode=1`) at all 13 points;
- the shipped model with its 50 `BISECT` steps written back as a `for` loop, self-heating on (`dlam_dPin=0.215`), equals the unrolled model at all 13 points.

Run it with `va-cli sim va_ac_10GHz.net --model <dir> --ac --report out,X1.nind`, where `<dir>` holds `mrm_eqc_rlc_loop60.va` (module `mrm_eqc_rlc_loop`) and `../vasim/*.vams`. To test `loop2`, change the module name in the deck. Whether OpenVAF is really affected still needs the ngspice deck re-run before anything is reported upstream.

## 6. Re-running

```
cd tests
python3 t1_ac_crosscheck.py; python3 t2_eo_response.py; python3 t2b_eo_selfheat.py
python3 t3_static.py; python3 t4_selfheat_shape.py; python3 t5_eyes.py
python3 plot_eyes.py; python3 plot_fig8.py; python3 benchmark.py
```
Environment variables `VACLI`, `NGSPICE` and `OPENVAF` point to the binaries (see the top of `harness.py`). Every JSON stores x/y data and the point counts, so a re-run can be compared against it.
