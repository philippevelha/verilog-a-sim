# Manuscript: presenting verilog-a-sim

**Target:** IEEE Transactions on Computer-Aided Design of Integrated Circuits and Systems (TCAD),
regular paper, two-column `IEEEtran` (about 12 pages plus references for a regular submission;
check the current author guidelines). **Status:** skeleton with a first draft of the abstract,
introduction and tables. Everything else is `\todo{}` notes.

## Build

```bash
cd paper_writing
latexmk            # pdflatex + bibtex, writes main.pdf here (build products are gitignored)
```

The build runs in place, not into an output directory: MiKTeX's bibtex looks for `refs.bib`
next to the `.aux`, so an `out_dir` breaks the bibliography.

`\drafttrue` in `main.tex` shows the red `\todo{}` notes; set `\draftfalse` for a clean build.

## Layout

| File | Content |
|---|---|
| `main.tex` | preamble, title, section order |
| `sections/00-abstract.tex` ... `12-conclusion.tex` | one file per section |
| `figures/` | TikZ sources and plots (`pipeline.tikz` = Fig. 1) |
| `refs.bib` | bibliography; **every entry is marked VERIFY** and has to be checked against the publisher's record |

## Rule: every number has a source

A figure goes into the paper only if it was measured, and the source goes next to it as
`\src{...}` (prints nothing): the `release.txt` version, a doc, or a result file. Before
submission, re-measure at the version the paper cites. Don't carry numbers forward from older
entries. The claims below are the ones in the draft now.

| Claim in the draft | Value | Source |
|---|---|---|
| QSPICE golden circuits, all pass | 28/28 | `cargo xtask validate`, 2.0.0+2, 2026-09-28 |
| Worst errors by analysis | DC 6.8e-5, tran 8.3e-4 (rectifier), AC mag 1.3e-5 / phase 6.4e-6 rad, noise 2.6e-5 | same run |
| Workspace tests | 920 passed | release.txt 2.0.0 |
| ISCAS'85: all 11 solve and match logic | c17 32 vectors, others 5 each | release.txt 1.27.0+5 |
| Largest circuit | c7552, 268 676 unknowns, 15 882 PSP103 instances | release.txt 2.0.0 |
| Time per vector (i7-1185G7, 8 thr) | table in `10-scale.tex` | release.txt 1.27.0+5 |
| c432 transient sequence | 11 570 points, 1 196 800 samples correct, 46.8 min | release.txt 1.28.0 |
| Cross-platform identity in CI | 68 decks, Linux = macOS = Windows | release.txt 1.31.0+1 |
| Cross-machine identity | 10/10 hashes, Ryzen 9 9950X (Linux) = i7-1185G7 (Windows) | `scripts/benchmark/benchmark_*.txt`, 2.0.0+2 |
| c432 thread speed-up | 15.76 s → 1.75 s, 1 → 32 threads (9950X) | `benchmark_linux.txt` |
| BTF: c432 linear solve | 22.7 s → 1.2 s | release.txt 2.0.0 |
| Adaptive gmin: c432 `.op` | 87.5 s → ~11 s | release.txt 2.0.0 |
| Model prep vs OpenVAF | 5–50 ms vs 0.23–6.7 s (different trade-off) | release.txt 1.1.1 |
| Corpus | 200/224, 182/191 self-contained | release.txt 1.26.0: **re-measure** |

## Open decisions (for the author)

- **Lead contribution.** Determinism across platforms and threads is the most novel measured
  result; ISCAS'85 at transistor level is the most striking scale result. The introduction
  should lead with one of them.
- **Multi-physics** (photonic noise, thermal, traffic): its own section, or cut for length.
- **Authors and affiliations**, funding line, and the AI-use disclosure IEEE requires in the
  acknowledgments.
- **A Linux full-suite run** (`scripts/benchmark/bench_iscas_linux.sh`) would give the scaling
  figure a second machine and confirm the full suite's cross-platform identity.
- **macOS benchmark**, to complete the three-platform identity on independent hardware.
