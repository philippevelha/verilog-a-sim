# Golden reference outputs

This directory holds **committed** ngspice reference outputs, one per zoo circuit. `va-harness`
compares the simulator's results against these to the tolerances in `docs/validation.md`.

ngspice is used as an oracle only (§7) — we are not building on it. **A file in this directory
must be real ngspice output**, not a hand-computed/analytic stand-in, even a correct one —
`cargo xtask validate` (below) trusts what's here as ground truth, so laundering a hand-derived
value in as if it were ngspice's would defeat the entire point of an external oracle.

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
cargo xtask gen-golden    # shells out to ngspice (if installed) and writes outputs here
```

**Not yet implemented** (as of 2026-07-13): `gen-golden` returns a clear error rather than a
silent no-op — either ngspice isn't on `PATH` at all, or (if it is) this project still needs a
`circuits/*.net` → ngspice-deck translator, which hasn't been written or verified against a real
ngspice install. Until then, this directory is expected to stay empty, and `cargo xtask
validate` treats every circuit as legitimately "skipped, no golden" rather than failing — see
`docs/roadmap.md`'s T6.3 section for the full account.

Commit the regenerated files alongside the change that motivated them, and note in the PR
why the golden data moved. An empty `golden/` means no rung has been captured yet; this file
is the placeholder until the first reference output lands.
