# mrm_tcmt — Verilog-A micro-ring / micro-disk modulator + driver

Two Verilog-A modules for electronic–photonic co-simulation:

| File | Module | What it is |
|---|---|---|
| `mrm_tcmt.va` | `mrm_tcmt` | Ring/disk modulator: TCMT optical core, lumped pn junction, heater, thermal node |
| `mrm_driver.va` | `mrm_driver` | Behavioural voltage-mode driver: logic in, 2-pole bandwidth, Thevenin output |
| `mdm_saxena2023.lib` | — | `mode=1` card with the AIM microdisk parameters of Saxena & Shawon, arXiv:2311.16174 |

Only `ddt`, `limexp`, `$limit(pnjlim)` and standard disciplines are used — no `transition`,
`laplace`, `absdelay`, analog events or hierarchy — so the same source runs in
OpenVAF → ngspice/Xyce (OSDI), verilog-a-sim, Spectre, AFS, ADS, HSPICE and Eldo.
The driver and the ring are joined at netlist level (OpenVAF does not compile hierarchy).

## Ports of `mrm_tcmt`

```
mrm_tcmt(anode, cathode, htp, htn,
         o_in_re, o_in_im, o_thru_re, o_thru_im, o_drop_re, o_drop_im,
         wl, dt)
```

| Port | Discipline | Meaning |
|---|---|---|
| `anode`, `cathode` | electrical | pn junction; reverse bias = V(cathode) − V(anode) |
| `htp`, `htn` | electrical | integrated heater |
| `o_in_re/_im` | electrical (input) | optical field envelope, **√mW**: \|E\|² = V_re² + V_im² is mW |
| `o_thru_re/_im`, `o_drop_re/_im` | electrical (output) | through / drop fields, ideal sources |
| `wl` | electrical (input) | laser wavelength, **V = nm** |
| `dt` | thermal | ring temperature rise above ambient (K); may float or link rings |

The envelope is referenced to the laser itself, so a CW laser is a DC source on `o_in_re`
(1 V = 1 mW). One carrier per bus; propagation is unidirectional.

## Physics

**Optical (both modes)** — single-mode temporal coupled-mode theory:

  da/dt = [ j(ω₀ − ω_L) − 1/τᵢ − 1/τ_c − 1/τ_d ] a − j μ_c s_in
  s_thru = s_in − j μ_c a,  s_drop = −j μ_d a,  μ² = 2/τ,  Q = ω₀τ/2

solved as two real ODEs on internal nodes (state y = μ_n·a with fixed μ_n, so the result is
exact even when τ_c depends on bias). Away from resonance the detuning is replaced by
(FSR/π)·sin(π(λ_L − λ_res)/FSR) — exact near resonance, smooth and FSR-periodic far away.

**`mode = 0` (physical)** — parameters `lambda0`, `FSR`, `Qi`, `Qc`, `Qd`, `eta_mod`.
The carrier-induced shift is tied to the junction depletion charge,
Δλ = −eta_mod · Q_dep(V)/Cj0, so `eta_mod` is the small-signal efficiency at 0 V (pm/V) and
the large-signal roll-off follows the C–V law (Cj0, Vbi, Mj). Free-carrier absorption follows
from the same shift via `r_abs` = Δα/(k₀Δn); forward injection adds a blue shift (`eta_inj`).

**`mode = 1` (measured fit)** — the Saxena–Shawon parameterisation: λ₀(v), τ_c(v), τ_l(v) as
quadratics in reverse bias (`lf0..2`, `tc0..2`, `tl0..2`), τ as amplitude decay times.

**Electrical** — Rs, Cj(V) with SPICE FC linearisation, forward diode with diffusion charge
τ_c·I_d, Cpad, and substrate branch Cox–R_Si (Cox = 0 disables it).

**Thermal** — Rth ∥ Cth on `dt`; heater power η_h·V²/R_h(T) plus, with `self_heat=1`,
junction dissipation and absorbed intracavity power (thermal bistability appears at high power).
A measured heater efficiency γ (pm/mW) and time constant τ_h map to
Rth = γ/(η_h·dλ/dT), Cth = τ_h/Rth.

Not modelled: CW/CCW backscatter splitting, TPA/Kerr, wavelength dependence of Q.

## Key parameters (defaults)

| Parameter | Default | Unit | | Parameter | Default | Unit |
|---|---|---|---|---|---|---|
| `mode` | 0 | – | | `Rs` | 100 | Ω |
| `lambda0` | 1550 | nm | | `Cj0` | 20f | F |
| `FSR` | 10 | nm | | `Vbi`, `Mj` | 0.8, 0.5 | V, – |
| `Qi`, `Qc` | 2e4, 2e4 | – | | `Cpad` | 10f | F |
| `Qd` | 0 (all-pass) | – | | `Cox`, `RSi` | 0, 1k | F, Ω |
| `eta_mod` | 20 | pm/V | | `dlam_dT` | 75 | pm/K |
| `r_abs` | 0.15 | – | | `Rth`, `Cth` | 5k, 1n | K/W, J/K |
| `ilossdB` | 0 | dB | | `Rh0`, `TCR` | 1k, 0 | Ω, 1/K |

## Running it

**ngspice (≥ 44) via OpenVAF**
```
openvaf-r mrm_tcmt.va && openvaf-r mrm_driver.va
ngspice -b tb_static.cir      # spectra vs bias and heater
ngspice -b tb_tran.cir        # 25 Gb/s PRBS7, driver -> ring
ngspice -b tb_mdm_static.cir  # mode 1, paper parameters
ngspice -b tb_mdm_tran.cir
```
ngspice 42 (Ubuntu 24.04 apt) only reads OSDI 0.3; current OpenVAF emits 0.4.

**verilog-a-sim** — decks in `vasim/`
```
va-cli sim vasim/static_vr0.net --model vasim --report tre,tim
va-cli sim vasim/tran.net --model vasim --tran --report kk,tre,tim
```

**Spectre**
```
ahdl_include "mrm_tcmt.va"
ahdl_include "mrm_driver.va"
X0 (din kk 0) mrm_driver vlo=0 vhi=2 f3db=35G Rout=50
X1 (0 kk hp 0 in_re in_im th_re th_im dr_re dr_im wl tdt) mrm_tcmt lambda0=1550 Qi=20k Qc=22k
```

## Validation performed

| Check | Result |
|---|---|
| Static through-port, mode 0, V_R = 0/1/2 V and heater 0/0.25/1 mW, vs closed-form Lorentzian | ngspice max \|ΔT\| 2.8e-5; verilog-a-sim 1.2e-6 |
| Mode 1 with Saxena–Shawon parameters vs their Eq. 6, v = −0.5…2 V, heater 1 mW | resonance position and depth exact; max \|ΔT\| 1.2e-4 (far skirts, from the periodic detuning map) |
| 25 Gb/s transient driver → ring, ngspice vs verilog-a-sim | RMS ΔP/P̄ 2.4e-5 over 254 bits |
| Mode 1, 25 Gb/s, 2 Vpp, laser −50 pm | ≈6.4 dB ER; fast rise / slow fall as in the paper's Fig. 10 (scope bandwidth not modelled) |

## Assumptions to replace with your data

- `mdm_saxena2023.lib`: FSR (30 nm), dλ/dT (75 pm/K), Is, Nd, τ_c and f_abs are **not** in the
  paper and are placeholders; only γ = 251.5 pm/mW and τ_h = 15 µs are from it.
- Mode-0 defaults are generic silicon values, not a specific PDK.

## Reference runs and speed

`reference/` holds stored outputs (CSV, full precision), the time-step statistics, run times
and memory for every case, plus `make_reference.py` / `check_rerun.py` to reproduce and compare
them — see `reference/REFERENCE.md`.

Model sources are written so that verilog-a-sim can run them on its statically scheduled
(and, with `VA_JIT=on`, compiled) path: no analog functions, each `ddt()` in its own
contribution, `$limit` arguments from parameters only. `v1_backup/` has the earlier, numerically
identical form.
