# benchmark/ — third-party compact models and model cards

**Nothing in this folder was written by this project.** It holds third-party Verilog-A compact
models and SPICE model cards that the decks under `circuits/benchmark/` need. They are copied
here so those decks run from a fresh clone, and on CI (Road to 2.0 item 3, reproducibility
across platforms). Before this folder existed they lived only in the untracked `external/` tree.

The files are copied **byte for byte** from that tree. Where a model directory has a
`Changelog`, it records edits made upstream of us (for example "Prevent division by 0",
"Use disciplines.vams"); we made no further changes. Each model keeps its own license. The
license of this repository does **not** apply to anything under `benchmark/`.

## Models (`benchmark/models/<name>/`)

| Directory | Model, version | Owners | License, as stated in the directory |
|---|---|---|---|
| `psp102/` | PSP 102.5.0 (JUNCAP2 200.4.0) | NXP Semiconductors, Delft University of Technology | `IP_NOTICE_DISCLAIMER_LICENSE`: free redistribution, no charge for the model itself, notices kept |
| `psp103/` | PSP 103.8.2 (JUNCAP 200.6.2) | NXP Semiconductors, CEA-Leti (earlier: TU Delft, Arizona State University) | the license in `releasenotesPSP103.8.2.txt`: royalty-free, redistribution with the notices kept |
| `psp104/` | PSP 104.0.0 | NXP Semiconductors, CEA-Leti | the license in `releasenotesPSP104.0.0.txt` (same terms as PSP103) |
| `hisim2/` | HiSIM2 (CMC release 2021-10-08) | Hiroshima University, STARC | Educational Community License 2.0 (`LICENSE.txt`, `NOTICE.txt`) |
| `bsimsoi/` | BSIM-SOI 4.6.1 | Regents of the University of California; Analog Devices (VA code) | the BSD-style terms in the header of `bsimsoi.va` |
| `bsim6/` | BSIM6 6.0 / 6.1.1 | Regents of the University of California | the BSD-style terms in the header of each `.va` |
| `bsimbulk/` | BSIM-BULK 106 / 107 | Regents of the University of California | Educational Community License 2.0 (`LICENSE.txt`, `NOTICE.txt`) |
| `bsimcmg/` | BSIM-CMG 111.0.0 | Regents of the University of California | Educational Community License 2.0 (`LICENSE.txt`, `NOTICE.txt`) |
| `bsimimg/` | BSIM-IMG 102.9.4 | Regents of the University of California | Educational Community License 2.0 (`LICENSE.txt`, `NOTICE.txt`) |
| `L-UTSOI/` | L-UTSOI 102.7.0 | CEA-Leti | Educational Community License 2.0 (`LICENSE.txt`, `NOTICE.txt`) |

For where each model was published, see the `Changelog` or release notes in its directory.
Several of these models are Compact Model Coalition (CMC) standards. The CMC names identify
the reference implementations: this simulator's results from these files are **not** a
CMC-qualified implementation, and nothing here claims they pass CMC QA.

## Model cards (`benchmark/modelcards/`)

> **Origin disclaimer.** `psp103_nmos.mod` and `psp103_pmos.mod` are **not** ours and describe
> **no real process**. They are the PSP103 example cards distributed with the PSP reference
> data (`psp_VA_and_CMC_ref_data`, level 103.3.x, as named in each file's header line). CEA-Leti
> publishes that data as the PSP support package, and this copy came to us inside a
> third-party ISCAS'85 benchmark drop (`iscas85_benchmark_circuit/Modelcards/`). They are
> test-parameter sets for exercising a PSP implementation, not calibrated to any foundry.
> Circuit results obtained with them (delays, currents, noise margins) say nothing about any
> real technology. They are kept unmodified, including their header comments, and remain
> under the PSP license terms above.

## Deliberately left out

| Left out | Why |
|---|---|
| EKV 2.6 (`external/code/ekv/`) | Excluded at the user's decision. `circuits/benchmark/nand/ekv26.net` still points at `external/`. |
| BSIM4 (`external/code/bsim4/`) | This Verilog-A port states **CC-BY-NC 4.0** (non-commercial), which may be incompatible with the repository's eventual license. Awaiting a decision; `nand/bsim4.net` still points at `external/`. |
| HICUM/L2 3.0.0 (`external/code/hicum2/`) | The file carries a copyright notice (Michael Schröter, 1993–2020) and **no license statement**, so redistribution is not granted in writing. `hicum_output_family.net` still points at `external/`. |
| `psp_inverter.sp` (`external/benchmarkExt/psp_inverter/`) | Excluded at the user's decision. The decks converted from it (`psp103_inverter_card_*.net`) are self-contained. |

On a fresh clone the decks that still point at `external/` fail: their model file is missing.
`cargo xtask deck-compare` reports such decks as "failed on both sides", not as matches.
