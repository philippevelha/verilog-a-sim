# Golden reference outputs

This directory holds **committed** QSPICE reference outputs, one per zoo circuit. `va-harness`
compares the simulator's results against these to the tolerances in `docs/validation.md`.

QSPICE is used as an oracle only (§7) — we are not building on it. (Originally ngspice; switched
2026-07-13 to match the actual dev environment — QSPICE is Windows-only.) **A file in this
directory must be real QSPICE output**, not a hand-computed/analytic stand-in, even a correct
one — `cargo xtask validate` (below) trusts what's here as ground truth, so laundering a
hand-derived value in as if it were QSPICE's would defeat the entire point of an external oracle.

`divider.golden` is real, QSPICE-generated data (via `cargo xtask gen-golden`) — the project's
first genuine golden reference. The rest of this directory is still empty; see "Regenerating"
below for exactly why, circuit by circuit.

## Spectre references (`golden/spectre/`, adopted 2026-11-12)

Cadence Spectre is the second oracle (`CLAUDE.md` §7), for what QSPICE cannot run: circuits that
are Verilog-A throughout and harmonic balance with periodic noise. `golden/spectre/mwp/<deck>/`
holds, per deck, the netlist (`<deck>.scs`) and, on the machines that have them, Spectre's ASCII
PSF results as written (SPECTRE 24.1.0.078). **Only the netlists are committed:** Spectre's
output files, logs, summaries and environment files are not distributed (Cadence NDA;
`golden/spectre/.gitignore`). Accuracy figures of verilog-a-sim against them may be published;
numbers comparing speed or memory with Spectre may not. The outputs
come from `paper_writing/mwp/spectre/run.sh`, run on the author's licensed server, and the same
rule applies as above: **real Spectre output only**, never a hand-made or post-processed file.
Large transient waveforms (30–100 MB: the noise transients `11_*` and the oscillator transients
`22`, `25`) are not committed; what was measured on them is recorded in
`paper_writing/mwp/spectre/README.md`.

The gate is `paper_writing/mwp/tests/t12_spectre_golden.py` (part of that library's `run_all.py`),
not `cargo xtask validate`: the comparisons need the library's tone fits and PSF reader.

## The `.golden` format (DC and `.dc` sweep, so far)

A single-operating-point `.golden` file is plain text, one `<node> <value>` pair per line, in
the circuit's own `node_order` (`va_harness::golden::GoldenDc` — see that module's doc comment
for the full format). A `.dc`-sweep golden file instead has a header line naming the swept
source and every node, then one `<swept value> <node value>...` row per point
(`va_harness::golden::GoldenSweep`). Either way, name it `<circuit-stem>.golden`, e.g.
`circuits/divider.net` → `golden/divider.golden`, `circuits/diode_iv.net` →
`golden/diode_iv.golden`. A `.tran` transient waveform has no golden format yet (§
`docs/roadmap.md`'s T6.3 notes) — `xtask validate`'s own circuit tables only list the rungs that
qualify (`divider.net`, `mos_dc.net`, `diode_iv.net`).

## Regenerating

```bash
cargo xtask gen-golden    # shells out to QSPICE (if installed) and writes outputs here
```

Locates `QSPICE64.exe` via the `QSPICE_PATH` env var, then `PATH`, then the standard install
location (`C:\Program Files\QSPICE\QSPICE64.exe`).

**Only `circuits/divider.net` is regenerated today** (`xtask::QSPICE_NATIVE_CIRCUITS`) — the
sole circuit that's both a pure `R`/`C`/`V` deck (so QSPICE's own built-in primitives apply with
zero translation) and has no temperature-sensitive nonlinearity. `mos_dc.net`/`diode_iv.net`
each use a custom `.va` model (`models/mosfet.va`/`diode.va`); regenerating their golden needs
two more things, neither done yet: (1) an equivalent QSPICE-native `.model` card for each, and
(2) reconciling a real, diagnosed mismatch — QSPICE's default simulation temperature is 27°C
(300.15 K), while this project's codegen fixes a single constant pair (`va_codegen::TEMP=300.0`,
`VT=0.025_852`, exactly 300 K) for every model. That 0.15 K difference is invisible for a linear
circuit but produces a real ~0.85% divergence in `diode.va`'s exponential I–V law — comfortably
past `DC_REL`'s `1e-4` tolerance — and forcing QSPICE's `.temp` to exactly 300 K makes it *worse*,
not better, since it moves the simulation away from the diode model's own implicit nominal
temperature (`TNOM`, which SPICE diode models rescale `IS` relative to). See
`docs/roadmap.md`'s T6.3 section for the full empirical derivation. `cargo xtask validate`
correctly reports these as "skipped, no golden" in the meantime, not a failure.

Commit the regenerated files alongside the change that motivated them, and note in the PR
why the golden data moved.
