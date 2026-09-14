# Reference validation registry

Every contribution that reproduces a **reference** — a published paper's model, a textbook
circuit, or a reference simulator's result — is listed here, by physical domain, with the
reference it matches, the equation it implements, the model and deck that carry it, and what it
was checked against. This is the registry; the *mechanics* of validation (metrics, tolerances,
`golden/`, `cargo xtask validate`) stay in `docs/validation.md`, and each entry's full derivation
stays in the model header or the linked document.

Three kinds of oracle appear, and the "Validated against" column always says which:

- **QSPICE golden** — the project's reference simulator (`CLAUDE.md` §7), committed output in
  `golden/`, gated by `cargo xtask validate`. The strongest tier; only available where QSPICE
  has a primitive for the physics.
- **Closed form** — the reference's own equation evaluated independently in a test, with the
  tolerance stated. Used where no SPICE primitive exists (photonics, sampled systems).
- **Paper figure** — a number the paper quotes, reproduced to the stated dB/percent. Weaker
  than a closed form (a quoted number carries the paper's unstated assumptions); listed with
  the residual, never silently.

A row is added when the match is *measured*, not when the model is written. Domains are added
as they are encountered; the ones met so far are photonics, optics, thermal, mechanical and
electrical, plus the multi-domain couplings between them.

---

## Photonics — noise (2026-09-14, v0.9.21)

The three papers in `references/` (`docs/photonic-noise.md` maps every equation to its line):

| Paper | Physics | Where |
|---|---|---|
| **Glenn 1989** / **Wanser** via **Bartolo 2012** | Thermorefractive phase-noise PSD of a guided wave, `S_φ(f) = (2πk_BT²L/κλ²)(dn/dT+nα_L)²·ln[(k_max⁴+ω²/D²)/(k_min⁴+ω²/D²)]` | `models/waveguide.va` — a 73-point `noise_table_log` (8/decade, 1 Hz–1 GHz) built by a macro from the instance's own parameters; tracks the closed form to 0.7 % |
| **Duan** via Bartolo eq. (5) | Thermomechanical `1/f` phase noise | same model, an exact `flicker_noise(…, 1.0)` |
| **Bartolo** App. B | Phase → intensity at quadrature, differential readout cancels RIN | `mzi.va` (its Jacobian *is* eq. B2), `splitter.va` |
| **Scheuer 2016** eq. 13–15 | Sagnac phase in a ring; Ω_min from shot + Johnson + RIN | `ring_gyro.va` (rotation rate on a control node), shot noise in `photodiode.va`, `rin` in `cw_laser.va` |

| Deck | Validated against | Result | Residual / caveat |
|---|---|---|---|
| `circuits/fiber_mzi_noise.net` (Bartolo's 40 m + 40 m MZI, 1319 nm) | Closed form: fiber share of the output, referred to the 1 rad/V PZT input, vs `2·(Wanser + Duan)` at all 51 frequencies (test `fiber_mzi_noise_matches_wanser_and_duan_through_the_interferometer`) | within 1 % (table interpolation 0.7 %) | — |
| same | Paper figure: −125.5 dB re rad/√Hz zero-frequency level (test `wanser_formula_reproduces_bartolos_quoted_figure`) | −125.50 dB; 1319 vs 1550 nm scaling 1.6 dB (paper: 1.4 + ~0.2) | Duan's term with Table II's printed values gives −120.6 dB at 100 Hz where the paper quotes −124.4 — 3.8 dB not recovered |
| same | Closed form: each detector's shot noise `2q(resp·P_det + I_s)·R²`; laser RIN at quadrature | shot to 1e-6; RIN < 1e-40 (exactly cancelled), `cos²(3°)` of itself 3° off | paper's −148.9 dB shot line not reproducible from 0.5 mW at any resp ≤ 1 (ours −154.5) |
| same, at 633 / 1310 / 1550 nm (`docs/examples/fiber_mzi_wavelengths.py`) | Paper figure: Fig. 3(b)'s wavelength scaling, 1319 vs 1550 nm | 1310 − 1550 nm = +1.6 dB at low frequency (paper: 1.4 dB from λ + ~0.2 from `w0`); closed forms overlay each run within 1 % | 633 nm is outside this fiber's single-mode range (V = 7.1): formula for the LP01 mode, not a possible measurement |
| `circuits/ring_gyro_noise.net` (Scheuer's RWOG, 1 mm ring) | Closed form: `S_V = R²(2q i_d + 4kT/R + RIN i_d²)` from the solved operating point, attributed per device; `S_in = S_V/H²` with `H` from two extra DC solves (test `ring_gyro_noise_is_scheuers_three_term_budget_referred_to_rotation_rate`) | `S_V` to 1e-6; `S_in` to 1e-3; `Ω_min = √S_in = 0.0137 rad/s/√Hz`, 69/15/17 % shot/Johnson/RIN | no QSPICE primitive for a ring; eq. (15) read with the `T_D/S` prefactor made explicit |

## Optics — passive components (2026-09-11, v0.9.15)

| Reference | Physics | Model(s) | Deck(s) | Validated against |
|---|---|---|---|---|
| **Bogaerts et al.**, "Silicon microring resonators", Laser Photon. Rev. 6 (2012), eqs. 5–6 | Add-drop ring transfer `T_drop`, `T_pass`; FSR `λ²/(n_g L)`; thermo-optic tuning `dλ/dT = λ·(dn/dT)/n_g` | `models/microring.va` (`ring_gyro.va` reuses its all-pass form, eq. 3) | `circuits/microring_thermal.net`, `microring_thermal_fast.net` | Closed form in the deck header: resonance at exactly 1550.000 nm for m = 347, FSR 3.105 nm, 0.0703 nm/K, laser reached at a 7.11 K rise (every crossing at 7.11 K to three digits), 787 µW on-resonance drop peak, 0.057 nm linewidth |
| Shot-noise photodetection; InGaAs flat responsivity | `I = I_s(e^{V/NV_t} − 1) − resp·P`, `2q(|I_j| + resp·P)` | `models/photodiode.va`, `models/cw_laser.va` | as above; both noise decks | Photovoltaic clamping at 25 µA/200 kΩ reproduced and explained in the deck header; shot noise vs closed form in both noise decks (above) |

## Thermal (2026-09-11, v0.9.15)

| Reference | Physics | Model(s) | Deck(s) | Validated against |
|---|---|---|---|---|
| Electrothermal analogy (Verilog-AMS standard `thermal` discipline: `Temp`/`Pwr`) | Heat balance `Pwr = −V·I + Temp/R_th + C_th·ddt(Temp)`; τ = R_th·C_th = 12 µs; rise ∝ V² | `models/heater.va`, `models/photonic.vams` | `circuits/microring_thermal*.net` | Closed form in `docs/examples.md` §8/§11: 10.6 K peak 8 µs after the 5 V peak (12.5 K if held), parabolic lag; under a 10 µs ramp the plant ripples 2.4–5.5 K and never reaches 7.11 K (measured max 5.48 K) |

## Mechanical (2026-08/09)

| Reference | Physics | Model(s) | Deck(s) | Validated against |
|---|---|---|---|---|
| Mobility analogy (force ↔ current, velocity ↔ voltage; `mechanical.vams`) | Sprung mass with viscous damping, `ω₀ = √(k/m)`; gyrator coupling `F = k_f·I`, `V_emf = k_f·v` with a `tanh`-saturated force law | `models/spring_load.va`, `models/actuator.va`, `models/actuator_plant.va`, `models/vsin.va` | `circuits/actuator_plant.net` | Closed form in `docs/examples.md` §5–7: `ω₀ = √(5000/0.5) = 100 rad/s` exactly (`f₀ = 15.9155 Hz`), Q = 10, driven on resonance; coil current and back-EMF coupling |

## Electrical (2026-07 onward — the bring-up ladder and the noise/AC gates)

All QSPICE-golden unless stated; tolerances and the full gate list are in `docs/validation.md`.

| Reference | Physics | Model(s) | Deck(s) | Validated against |
|---|---|---|---|---|
| Ohm's law / KCL (ladder rung 1) | Resistor divider | `models/resistor.va`, `leg.va`, `series_divider.va` | `circuits/divider.net`, `divider_hv.net` (`hier_divider.net` is a `va-cli` test fixture for module instantiation, not a golden gate) | QSPICE golden |
| **Shockley** diode law (rung 2) | `I = I_s(e^{V/NV_t} − 1)` | `models/diode.va` | `circuits/diode_iv.net`, `diode_iv_params.net`, `diode_clamp.net` | QSPICE golden via a hand-translated `.model` card |
| RC transient (rung 3), rectifier (rung 4) | `1 − e^{−t/τ}`; half-wave rectification | reference `Capacitor`, `models/diode.va` | `circuits/rc_step.net`, `rc_discharge.net`, `rectifier.net` | QSPICE golden (`UIC` cold start) |
| **Shichman–Hodges** level-1 MOS (rung 5) | Square-law `I_d(V_gs, V_ds)` with λ | `models/mosfet.va` | `circuits/mos_dc.net` | QSPICE golden via a hand-translated `.model` card |
| Ring oscillator (rung 6) | Odd-stage inverter chain oscillation | `models/mosfet.va` | `circuits/ring_osc.net` | QSPICE golden, early-window comparison (deck header) |
| **Johnson–Nyquist** thermal noise, **Schottky** shot noise | `4kT/R`, `2q\|I\|` | `models/resistor.va`, `diode.va`; reference `Resistor`/`Diode`/`Bjt` | `circuits/diode_noise.net`, `resistor_noise_va.net` | QSPICE golden (`onoise_spectrum`, per-device `onoise_<dev>`, `inoise_spectrum`, printed 22.3055 µV total) |
| SPICE flicker model `KF·I^AF/f` | 1/f noise | `models/diode_flicker.va` | `circuits/diode_flicker.net` | QSPICE golden, 2.6e-5 over a 209×-shaped spectrum |
| LRM §4.6.4.3/4 `noise_table` / `noise_table_log` | tabulated PSD, linear vs log-log interpolation | `models/resistor_noise_table.va`, `resistor_noise_table_log.va` | `circuits/resistor_noise_table*.net` | QSPICE golden (flat table vs a plain resistor pair, 1.9e-16); interpolation rules discriminated by shaped-table unit tests |
| RC low-pass AC | `1/(1 + jωRC)` | reference `Resistor`/`Capacitor` | `circuits/rc_ac*.net`, `diode_ac.net` | QSPICE golden (magnitude/phase) |
| Transport delay, two-path interferometer (electrical analogue) | `H = e^{−jωt_d}`; `\|cos(ω(t₂−t₁)/2)\|` fringes | `models/delay_line.va`, `models/interferometer.va` | `circuits/delay_ac.net`, `interferometer_ac.net` | Closed form in AC (exact `absdelay`); refused in transient (a fold would be a wrong number) |
| One-pole Laplace filter | `laplace_nd(V, {1}, {1, τ})` | `models/laplace_lowpass.va` | `circuits/laplace_ac.net`, `laplace_step.net` | QSPICE golden (the strongest of the frequency-domain tiers) |
| Controlled sources, inductor, transformer, RLC | `E`/`G` sources, `L di/dt`, coupled coils, ringing | netlist elements | `circuits/vcvs_amp.net`, `cccs_mirror.net`, `rl_decay.net`, `rlc_ring.net`, `transformer.net` | QSPICE golden (transformer: closed form, deliberately ungated — `docs/validation.md`) |
| LRM Z-domain filters `zi_*` (v0.9.20) | sampled difference equation | test fixtures | — | Closed form: staircase, `1 − aᵏ`, iterated recurrence, `H(e^{jωT})` (QSPICE has no LRM Z-filter element) |

## Multi-domain couplings

| Coupling | Disciplines in one solution vector | Deck | Where the match is shown |
|---|---|---|---|
| Electrical ↔ thermal ↔ optical ↔ wavelength | heater → ring → photodiode → load | `circuits/microring_thermal.net` | Optics and Thermal rows above; `docs/examples.md` §8–11 |
| Electrical ↔ mechanical (gyrator) | actuator ↔ sprung mass | `circuits/actuator_plant.net` | Mechanical row; `docs/examples.md` §5–7 |
| Optical power ↔ optical phase ↔ electrical | laser → splitter → waveguides → MZI → photodiodes → difference amplifier | `circuits/fiber_mzi_noise.net` | Photonics rows; `docs/photonic-noise.md` §3.1 |
| Electrical (rotation-rate control) ↔ optical → electrical | `Vrot` → ring → photodiode → load | `circuits/ring_gyro_noise.net` | Photonics rows; `docs/photonic-noise.md` §3.2 |

## How to add a row

1. Name the reference precisely (paper, edition/equation numbers, or "QSPICE golden").
2. State the equation as implemented, and the model line that carries it.
3. Say what it was measured against — golden, closed form, or a paper figure — with the
   tolerance or residual. A row without a measured residual is a model, not a validation.
4. If the reference's own number is *not* reproduced, put the gap in the caveat column rather
   than dropping the row (the Duan and shot-noise entries above are the pattern).
5. New domain → new section, with the analogy that maps it onto the solver (mobility for
   mechanics, electrothermal for heat, signal flow for optics) stated once at the top.
