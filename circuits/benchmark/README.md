# Benchmark circuits

Decks that exercise the simulator on **real vendor compact models** rather than on the model
zoo, and decks that reproduce bugs those models found.

These are deliberately **not** part of `cargo xtask validate`. Most of them need a `.va` file
from `external/`, which is gitignored, so they cannot be gated on — and that is precisely the
gap they exist to cover: every bug recorded here survived a green 28/28 gate, because nothing
in the zoo looked like them.

| deck | needs | what it is for |
|---|---|---|
| `cap_fast_edge.net` | nothing | Minimal reproduction of the LTE timestep-underflow fixed in 1.2.5. Self-contained: reference capacitor, resistor and waveform source, no Verilog-A. |
| `psp103_inverter_vtc.net` | `psp103.va` | CMOS inverter transfer curve. Rails, a ~0.588 V transition, ~99 µA of crowbar current. |
| `psp103_inverter_tran.net` | `psp103.va` | The same inverter switching. Impossible before 1.2.5 for two independent reasons. |
| `hicum_output_family.net` | `hicumL2V3p0p0.va` | One inner sweep of a nested `.dc` output family. |

Each file's header says how to run it, what it should produce, and — for the two adapted from
external decks — exactly how it differs from the deck it came from. Those differences matter
more than the numbers: none of these run on their original text, because `.subckt`,
`.param`/`'expr'`, `+` continuation lines and nested `.dc` are all unsupported, and no model
cards were available, so every one of them runs on the `.va` file's default parameters.

`benchmark.md` at the repository root carries the measured figures and the methodology.
