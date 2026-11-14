# Spectre golden outputs — circuits without a QSPICE reference

Real Cadence Spectre output (SPECTRE 24.1.0.078, the author's licensed server), one directory per
deck. **Only the translated netlist (`<deck>.scs`, from `scripts/spectre_golden/`) is committed.**
Spectre's own output — the result files (ASCII PSF) with their `logFile` index, `spectre.log`, and
each round's `summary*.txt` and `environment*.txt` — stays on the machines that hold it and is
not distributed (Cadence NDA, confirmed 2026-11-14; `../.gitignore`). On such a machine, do not
edit or trim a file; regenerate on the server. Without the outputs, `compare_zoo.py` has nothing to
compare against.

What may be published is how verilog-a-sim's results compare with these outputs (the accuracy
figures below); numbers comparing speed or memory with Spectre may not, only qualitative words.
The warnings this README cites (SFE-101, SFE-3239, ...) are in the logs; the rule "no
SFE-101/3239/30 in a deck's log" is applied locally, before a result is copied here.

| Round | Run | Clean | Kept here |
|---|---|---|---|
| 1 | 2026-11-12 | 33 of 105 | 3 (the bubble-cloud decks rounds 2 and 3 did not rerun) |
| 2 | 2026-11-13 | 71 of 86 (standard set; exit 0 and no errors) | 1 (NAND `psp102`, wrong in round 3) |
| 3 | 2026-11-13 | 76 of 84 (standard set, `gray_scott` no longer shipped) | 75 |

A deck is kept from the latest round in which it ran correctly: round 3 for 75 decks, round 2
for NAND `psp102`, round 1 for the three bubble-cloud decks. Results over 10 MB are in
`../zoo_large/`, which git ignores: the six bubble-cloud sweeps of round 1 and, from round 3,
`bubble_ramp`, `bubble_single`, `piezo_storage`, `ringchaos_bistable`, `ringchaos_selfpulse`.

## What is *not* here, and why

**A clean Spectre run of the wrong circuit (1 deck, round 3).** Spectre accepted the netlist,
warned, and simulated something else; the output is not golden data and was left out:

- NAND `psp102`. Round 3's translator moved `TYPE` to a model card, because PSP102's source
  carries `type="instance"` tags. They are inside the model's `` `P(...) `` macro, which
  `Common102_macrodefs.include` defines as empty: the tags are comments, the compiled module has
  no parameter attributes, and Spectre treats every parameter as an instance parameter. It
  ignored `TYPE` on the four model cards (SFE-101, SFE-3238) and simulated four NMOS devices.
  A translator defect, fixed in 2.56.0+1 (an attribute that a macro expands to nothing no
  longer counts; the regenerated netlist is round 2's again). Round 2's netlist, with `TYPE` on the instance, ran correctly and is the one kept here
  (5.3e-7 against verilog-a-sim 2.56.0).

The round-2 wrong-circuit decks whose models compile are fixed: the nine `TYPE` decks (NAND
`bsimcmg`, `bsimimg`, `lutsoi`, `psp103`, `psp104`, the four `psp103_inverter_*`) and the ten
SG13G2 PSP103 SNN transistor decks now run with no SFE-101/3239/30 in their logs, including the
four `fig5_tx*` decks that stopped on convergence in round 2 and now complete. The li-ion and
speaker decks still do not compile (below).

**Decks Spectre could not run (round 3):**

| Decks | Cause | Whose |
|---|---|---|
| `li_calendar`, `li_discharge`, `li_oven`, `speaker_ac`, `speaker_large`, `speaker_thermal` | a net-array index written with a parameter (`sh[N-1]`, `p[N]`): LRM-legal (a constant expression), but Spectre's compiler takes only a literal or a genvar expression (VACOMP-1192) | model portability; a rewrite is a decision, not made |
| `hh_giant_axon`, `hh_thin_fibre` | the same VACOMP-1192, now that the escaped `save X1.v\[0\]` is accepted (no SFE-874): `hh_cable.va` lines 170–171, `cross(V(v[probe_a]), +1)` with `probe_a`, `probe_b` integer parameters. The SFE-30 warnings on `N` and `len` in these logs follow from the failed compilation | model portability, same decision |

`gray_scott_spots` and `gray_scott_wellmixed` (two-dimensional net array, in `REFUSED.md`) were
not shipped in round 3. `thermistor_rt` runs since its table file uses `#` comments.

## verilog-a-sim against these files (`scripts/spectre_golden/compare_zoo.py`)

Run 2026-11-13 on round 3 with verilog-a-sim 2.56.0, tolerances of `CLAUDE.md` §7 (DC 1e-4;
transient RMS 1e-3 of the swing; AC 1e-3). 75 round-3 decks, plus NAND `psp102` from round 2:

| Result | Decks |
|---|---|
| **agree** (66) | the 19 decks that were the wrong circuit in round 2, all within tolerance: NAND `bsimcmg` 4.1e-7, `bsimimg` 7.1e-7, `lutsoi` 4.2e-7, `psp103` 4.3e-7, `psp104` 4.2e-7; `psp103_inverter_card_vtc` 4.1e-7, `_vtc` 4.2e-7, `_card_tran` 3.2e-4, `_tran` 4.1e-4; `snn/transistor/bench_access_bias` 1.6e-7, `scan_input_stage` 9.8e-7, `fig5_access` 2.1e-4, `uic_access` 1.4e-5, `fig5_tx` 2.8e-5, `fig5_tx_cmp` 5.2e-5, `fig5_tx_dff` 2.8e-5, `fig5_tx_in` 3.8e-5, `fig5_tx_none` 3.5e-4, `fig5_tx_sw` 4.7e-5; `thermistor_rt` 2.0e-7 (new); the five port-current decks `portprobe_ramp` 2.9e-5, `portprobe_sq` 5.9e-5, `portprobe_ac` 5.4e-5, `selfprobe_ramp` 2.9e-5, `nlcap_ramp` 8.5e-5 (they failed by 0.30–1.0 before 2.56.0: the probes dropped the displacement current); NAND `bsim6`, `hisim2`, `psp102` (round 2), `zoo_nmos`; `actuator_plant`; `delay_ac`, `interferometer_ac`; `fundamental_diagram`; `hier_divider`, `lib_tee`, `portprobe_dc`; `breaker_static`; `bubble_linear`, `bubble_ramp`, `bubble_single`; `grid_cascade`, `grid_equal_area`; `mems_hysteresis`, `mems_pullin`, `mems_resonance`; `piezo_storage`; `relay_bh`, `relay_switch`; `reram_freq_5k`, `_500k`, `_5meg`; `ring_bank`, `ring_selfheat`; `seu_sram`; `tec_dtmax`, `tec_energy`, `tec_pid`; `rc_pulse`; `snn/fig4_neuron`, `snn/fig5_neuron`; `snn/transistor/fig5_fit`, `fig5_fitbc`, `fig5_fitc`; `tasks/rc_logger`, `tasks/rc_watch`; `transformer` |
| **verilog-a-sim differs, not diagnosed** (2) | NAND `bsimsoi`: our output never falls below 0.44 V, Spectre's reaches 0 (error 0.93 of the swing). The four-terminal build leaves the body floating (SOIMOD=0); a floating body's DC state hangs on tiny conductances, and Spectre's `gmin=1e-18` on every node is one we do not add — a hypothesis, untested. NAND `bsimbulk107`: the curves are 0.7–7 mV apart in input, under one 12 mV sweep step, but the edge is steep enough that the output error is 0.23 |
| **just over tolerance** (3) | `piezo_linear` 1.31e-3, `reram_sweep` 1.05e-3, `seu_collection` 1.29e-3 |
| **Spectre differs by its own semantics** (1) | `tasks/current_limit`: the model calls `$finish` at 3 V; verilog-a-sim stops the sweep there (LRM), Spectre continues to 5 V. Identical where both have points |
| **sensitive to an event** (3) | `breaker_interrupt`: agree to 1e-8 of the swing until the arc chops the current at 19.1 ms, then the chop instant moves with the step (2.6e-2 RMS). `ringchaos_bistable` (0.11) and `ringchaos_selfpulse` (7.3e-2): bistable and self-pulsing rings, where a waveform RMS measures switching and pulse timing; not checked further. All three need an event-level metric |
| **comparison not tight enough** (1) | `cap_fast_edge`: voltages agree; the current spike at the 0.2 ns edge is 8.7e-3 RMS on Spectre's grid, the interpolation of a spike between two different step sequences |

The ten disagreements are the round-2 ones, with the same worst-case errors; round 3 added none. In
NAND `bsimcmg`'s log Spectre reports that it loosened `reltol` to 5e-5 to find the DC solution;
the result still agrees to 4.1e-7.

The three round-1 decks kept here (`bubble_cloud_check`, `_profiles`, `_response`) were compared
in round 1 and agreed.

## Wall time and memory, qualitatively

Measured on the 59 decks above (the NDA allows no numbers; the per-deck table is kept locally, in
`performance-vs-spectre.local.md`). The two ran on different machines, Spectre on one core of the
author's server and verilog-a-sim on a newer laptop, so these are observations, not a benchmark.

- **Memory:** verilog-a-sim uses an order of magnitude less memory than Spectre on every deck.
  Most of Spectre's is in use before it reads the netlist, and it grows further while compiling
  the large transistor models (PSP, HiSIM2).
- **Model compilation:** Spectre compiles each Verilog-A model to C before simulating, and on
  these small circuits that dominates its run time; it caches the result, so later decks using
  the same model start faster. verilog-a-sim compiles every model on every run, in a small
  fraction of a second.
- **Simulation:** on most transient decks Spectre is faster, mainly because it takes far fewer
  time steps: verilog-a-sim never steps more coarsely than the netlist's `.tran` step, which
  Spectre treats only as a hint. Where the two take similar numbers of steps, their cost per step
  is comparable. verilog-a-sim is faster on a few decks (`bubble_ramp`, `ringchaos_bistable`,
  `ring_bank`, `piezo_linear`), and markedly slower per step on `piezo_storage` and
  `bubble_cloud_check`, which are worth profiling.
