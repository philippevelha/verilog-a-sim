# Spectre golden outputs — circuits without a QSPICE reference

Real Cadence Spectre output (SPECTRE 24.1.0.078, the author's licensed server), one directory per
deck: the result files as written (ASCII PSF, with Spectre's `logFile` index) and the translated
netlist (`<deck>.scs`, from `scripts/spectre_golden/`). Do not edit or trim a file; regenerate on
the server.

**Kept locally, not committed** (`../.gitignore`): Spectre's `spectre.log` per deck and each
round's `summary*.txt` and `environment*.txt`. They name the user, the server and the licence
server, and record Spectre's timings, which may not be publishable under the Cadence licence
agreement (the author is checking). The warnings this README cites (SFE-101, SFE-3239, ...) are
in those logs; the rule "no SFE-101/3239/30 in a deck's log" is applied locally, before a result
is copied here.

| Round | Run | Clean | Kept here |
|---|---|---|---|
| 1 | 2026-11-12 | 33 of 105 | 3 (the bubble-cloud decks round 2 did not rerun) |
| 2 | 2026-11-13 | 72 of 86 (standard set) | 56 |

A deck that ran in both rounds is kept from round 2 only. Results over 10 MB are in
`../zoo_large/`, which git ignores: the six bubble-cloud sweeps of round 1 and, from round 2,
`bubble_ramp`, `bubble_single`, `piezo_storage`, `ringchaos_bistable`, `ringchaos_selfpulse`.

## What is *not* here, and why

**Clean Spectre runs of the wrong circuit (24 decks, round 2).** Spectre accepted the netlist,
warned, and simulated something else. These outputs are not golden data and were left out:

- `TYPE` given on an instance of a module that declares it a model parameter: Spectre ignored
  `TYPE=-1` (SFE-3239) and simulated four NMOS devices. NAND `bsimcmg`, `bsimimg`, `lutsoi`,
  `psp103`, `psp104`, and the four `psp103_inverter_*` decks. Round 1's fix only saw the
  `type="instance"` tags when they were in the top `.va` file; these models keep them in
  `` `include``d files.
- SG13G2 PSP103 model cards written with lower-case names (`type`, `tr`, `swgeo`, ...): Spectre's
  Verilog-A parameter names are case-sensitive and it dropped every one (SFE-101), so all the
  transistors ran with PSP103's defaults. `snn/transistor/bench_access_bias`, `fig5_access`,
  `fig5_tx` (and `_none`, `_sw`), `scan_input_stage`, `uic_access`; also the four `fig5_tx*`
  decks that stopped on convergence (`fig5_tx`, `_cmp`, `_dff`, `_in`), which were simulating the
  same wrong devices.
- The li-ion and speaker decks, whose models did not compile (below), for completeness.

Both translator defects are fixed (`net2spectre.py` now reads a model with its includes and
writes each parameter in the spelling the module declares); the 28 affected netlists are in the
next package.

**Decks Spectre could not run (round 2):**

| Decks | Cause | Whose |
|---|---|---|
| `gray_scott_spots`, `gray_scott_wellmixed` | two-dimensional net array (VACOMP-2259) | model; already in `REFUSED.md`, the package should not have shipped them |
| `hh_giant_axon`, `hh_thin_fibre` | `save X1.v[0]`: Spectre needs the brackets escaped (SFE-874) | translator, fixed |
| `li_calendar`, `li_discharge`, `li_oven`, `speaker_ac`, `speaker_large`, `speaker_thermal` | a net-array index written with a parameter (`sh[N-1]`, `p[N]`): LRM-legal (a constant expression), but Spectre's compiler takes only a literal or a genvar expression (VACOMP-1192) | model portability; a rewrite is a decision, not made |
| `thermistor_rt` | `*` comments in `models/thermistor.tbl`; the LRM (9.21.1) allows only `#` (VLOGA-5070) | data file, fixed |

## verilog-a-sim against these files (`scripts/spectre_golden/compare_zoo.py`)

Run 2026-11-13 on round 2 with verilog-a-sim 2.55.0+4, tolerances of `CLAUDE.md` §7 (DC 1e-4;
transient RMS 1e-3 of the swing; AC 1e-3). 56 decks:

| Result | Decks |
|---|---|
| **agree** (41) | NAND `bsim6`, `hisim2`, `psp102`, `zoo_nmos`; `actuator_plant`; `delay_ac`, `interferometer_ac`; `fundamental_diagram`; `hier_divider`, `lib_tee`, `portprobe_dc`; `breaker_static`; `bubble_linear`, `bubble_ramp`, `bubble_single`; `grid_cascade`, `grid_equal_area`; `mems_hysteresis`, `mems_pullin`, `mems_resonance`; `piezo_storage`; `relay_bh`, `relay_switch`; `reram_freq_5k`, `_500k`, `_5meg`; `ring_bank`, `ring_selfheat`; `seu_sram`; `tec_dtmax`, `tec_energy`, `tec_pid`; `rc_pulse`; `snn/fig4_neuron`, `snn/fig5_neuron`; `snn/transistor/fig5_fit`, `fig5_fitbc`, `fig5_fitc`; `tasks/rc_logger`, `tasks/rc_watch`; `transformer` |
| **verilog-a-sim is wrong, known** (5) | `portprobe_ramp`, `portprobe_sq`, `nlcap_ramp`, `selfprobe_ramp` (transient) and now `portprobe_ac` (AC, error 1.0 of the response): our `I(<port>)` and branch self-probe report the conduction current only (`va-frontend`'s `resistive_terms_only`); Spectre reports the total current, as the decks' own headers expect. Our simulator gives no warning when it applies |
| **verilog-a-sim differs, not diagnosed** (2) | NAND `bsimsoi`: our output never falls below 0.44 V, Spectre's reaches 0 (error 0.93 of the swing). The four-terminal build leaves the body floating (SOIMOD=0); a floating body's DC state hangs on tiny conductances, and Spectre's `gmin=1e-18` on every node is one we do not add — a hypothesis, untested. NAND `bsimbulk107`: the curves are 0.7–7 mV apart in input, under one 12 mV sweep step, but the edge is steep enough that the output error is 0.23 |
| **just over tolerance** (3) | `piezo_linear` 1.31e-3, `reram_sweep` 1.05e-3, `seu_collection` 1.29e-3 |
| **Spectre differs by its own semantics** (1) | `tasks/current_limit`: the model calls `$finish` at 3 V; verilog-a-sim stops the sweep there (LRM), Spectre continues to 5 V. Identical where both have points |
| **sensitive to an event** (3) | `breaker_interrupt`: agree to 1e-8 of the swing until the arc chops the current at 19.1 ms, then the chop instant moves with the step (2.6e-2 RMS). `ringchaos_bistable` (0.11) and `ringchaos_selfpulse` (7.3e-2): bistable and self-pulsing rings, where a waveform RMS measures switching and pulse timing; not checked further. All three need an event-level metric |
| **comparison not tight enough** (1) | `cap_fast_edge`: voltages agree; the current spike at the 0.2 ns edge is 8.7e-3 RMS on Spectre's grid, the interpolation of a spike between two different step sequences |

The three round-1 decks kept here (`bubble_cloud_check`, `_profiles`, `_response`) were compared
in round 1 and agreed.
