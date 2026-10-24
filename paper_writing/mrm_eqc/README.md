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

## 3b. Can the PAM8 eye be opened? (t6, `tests/t6_pam8_sweep.py`)

[B] reports an open 212 Gb/s PAM8 eye but gives no drive swing, laser power, detuning or
equalisation. t6 sweeps the conditions for the t5 PAM8 case. It keeps the same pattern, segment split, card, 50 Ω sources and 8 ps edges, and uses `t5_eyes.eye()` as the metric. The runs use verilog-a-sim 2.42.0+7 on Windows (`VA_JIT=on`, `--threads 1`) and were made 2026-10-24. ngspice was not re-run; t5 shows the two simulators agree to 6e-5 on this case.

| Run | What is swept | Best worst-eye (% of ideal) |
|---|---|---|
| t6 (`t6_pam8_sweep.json`) | no FFE; 5 bias windows × laser 0.5/1/2/4 mW × detuning −40…+10 pm (180 runs) | **−86 %** (0.5–3 V, 0.5 mW, −40 pm); the t5 point reproduces exactly (−96.5 %) |
| t6b (`--ffe`) | 3-tap FFE, pre 0…−0.2, post 0…−0.35; 0.5/1 mW (640 runs) | −13 % |
| t6c (`--refine`) | finer taps, a 4th tap, 0.1/0.25/0.5 mW, +5…+20 pm (1440 runs) | +11 %, but only at 0.1 mW (0.2 µW eye) |
| t6d (`--noselfheat`) | 1 mW, `self_heat=0`, laser from the cold best-OMA point (240 runs) | **+17 %**: pre −0.08, post −0.25, 0.5–3 V, +30 pm; eye 2.3 µW on 93 µW OMA |
| t6e (`--detscan`) | 1 mW, `self_heat=1`, the t6c taps, detuning −100…+300 pm | **+11 %** at +180 pm (1.8 µW on 112 µW OMA); +240 pm fails the operating point (`E0601`, thermal bistability) |

The FFE is peak-limited. Its taps satisfy |pre| + |main| + |post| (+ |post2|) = 1, so the equalised drive stays inside the same bias window, as a fixed driver swing would require.

What this shows:
- **Without equalisation the eye cannot be opened** by swing, laser power or detuning. Bandwidth closes it, not the drive. The same pattern and settings give −86 % at 70.7 GBd, −25 % at 35.3 GBd, and +45 % at 17.7 GBd. Even at 7.1 GBd the eye reaches only +56 %, the limit set by the static level spacing (RLM ≈ 0.6).
- **With a 3-tap FFE the eye opens**, to 11–17 % of ideal. That is about 2 µW per mW of laser power, a small eye. The fourth tap never helped.
- **Self-heating does not close it, but moves the laser.** With `self_heat=0` the result is the same at 1 and 2 mW (12.7 %), and the eye height scales with power. With self-heating on, the eye opens only once the laser is placed 180 pm from the static estimate. The static model heats each level separately, while the transient keeps the first symbol's temperature: `tau_th` = 1 µs is much longer than the 2.4 ns pattern. Placed correctly, 1 mW gives the same eye as low power. The apparent "worse at high power" in t6/t6b/t6c is this mis-placement, not physics.
- The optimum lies at the edge of the t6d grid (detuning +30 pm). A wider search may find a few more percent; it will not change the conclusion.

So [B]'s open PAM8 eye is consistent with this model only with equalisation (FFE or pre-emphasis). [B] does not state what it used. A wider-bandwidth ring than the Table I/II parameters describe is the other possible explanation. The eye here has no receiver noise or receiver bandwidth.

## 3c. A lower-Q, wider-bandwidth ring (t7, `tests/t7_lowq_sweep.py`)

This variant of the [B] dual-segment ring changes **only the bus coupling**, i.e. a narrower ring–bus gap. τ_e is swept; everything else stays the DUAL card: intrinsic loss τ_l(V) (junction doping and waveguide), modulation efficiency c1/c2 (junction), circumference (FSR 6.5 nm), and every electrical parasitic.

The coupling is given as power coupling per round trip, κ² = 2T_rt/τ_e with T_rt = λ²/(FSR·c) = 1.23 ps. Q_L = ωτ/2, and the optical pole is 1/(2πτ), where τ is the amplitude decay time.

The setup is otherwise t6's: PAM8 case, eye metric, peak-limited FFE, 1 mW, 0.5–3 V. `self_heat=0`, because t6d/t6e showed that self-heating only moves the right laser position. Detuning is scaled to each ring's half-linewidth. The runs use verilog-a-sim 2.42.0+8 on Windows, made 2026-10-24.

| τ_e (ps) | κ² | Q_L | optical pole | no FFE | best with FFE (eye height / OMA) |
|---|---|---|---|---|---|
| 27.1 ([B]) | 9 % | 7540 | 12.8 GHz | −117 % | +8 % (1.1 / 92 µW) |
| 20 | 12 % | 6480 | 14.9 GHz | −61 % | +27 % (4.1 / 106 µW) |
| 15 | 17 % | 5500 | 17.6 GHz | −17 % | **+37 %** (5.6 / 105 µW)¹ |
| 12 | 21 % | 4780 | 20.2 GHz | −16 % | +34 % (5.6 / 114 µW) |
| 10 | 25 % | 4230 | 22.9 GHz | +8 % | +34 % (4.8 / 98 µW) |
| 8 | 31 % | 3600 | 26.8 GHz | **+25 %**² | +31 % (3.9 / 87 µW)² |
| 6 | 41 % | 2890 | 33.5 GHz | +18 % | +18 % (1.9 / 74 µW) |

¹ t7b (`--refine`): pre −0.15, post −0.10, laser 1.0 half-linewidth (141 pm) from the best-OMA point. The optimum is inside the finer grid.
² t7b: 24.7 % with no FFE at 0.2 half-linewidth (43 pm), eye 3.9 µW on 112 µW OMA. The optimum is broad: >22 % anywhere from 0.15 to 0.35 half-linewidth. With pre −0.06 the eye reaches 31.3 %.

Points in the first table are from the coarse t7 grid (pre 0…−0.15, post 0…−0.3, 8 detunings). The [B] ring's 8 % here is below t6d's 17 % only because this grid is coarser.

What this shows:
- **Stronger coupling opens the PAM8 eye without equalisation** once τ_e ≤ 10 ps (κ² ≥ 25 %, Q_L ≤ 4200). The best is τ_e = 8 ps (κ² 31 %, Q_L 3600), at +25 %.
- **There is a trade-off.** Wider coupling widens the bandwidth but also the linewidth, so the same junction shift gives less modulation depth. At τ_e = 6 ps the OMA falls to 74 µW and FFE no longer helps.
- **With FFE**, a moderate change is best: τ_e = 15 ps (κ² 17 %, Q_L 5500) gives +37 %, four times what the [B] ring reaches on the same grid.
- These rings are over-coupled (τ_e < τ_l ≈ 23 ps), so the static dip is shallower than [B]'s. A κ² of 17–31 % per round trip is within reach of a narrower gap or a pulley coupler. That is a judgement, not a design: no coupler geometry is modelled here.

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
