# Spectre golden outputs — microwave-photonics library

Real Cadence Spectre output, as written by the simulator. Do not edit, trim or post-process a
file here; regenerate it on the server instead (below).

| | |
|---|---|
| Simulator | SPECTRE 24.1.0.078 (Cadence 2024-25), 64-bit |
| Host | Rocky Linux 9.7, Intel Xeon E5-2660 (`environment.txt`, kept locally) |
| Run | 2026-11-12, by the author, on the department's licensed server |
| Models | `paper_writing/mwp/lib/mwp_*.va` as of 2.55.0+1, with Cadence's own `disciplines.vams` |
| Format | ASCII PSF (`spectre -format psfascii`); read by `paper_writing/mwp/spectre/psf.py` |
| Gate | `paper_writing/mwp/tests/t12_spectre_golden.py` (verilog-a-sim against these files) |

Each directory holds the result files of one netlist and the netlist itself (`<deck>.scs`).
Spectre's log (`spectre.log`) and `environment.txt` are kept beside them locally but not
committed (`../.gitignore`): they name the user, the server and the licence server, and record
Spectre's timings, which may not be publishable under the Cadence licence agreement.

| Directory | Analysis | Used by the gate for |
|---|---|---|
| `00_models` | `dc` | every module compiles; a DC point exists (portability) |
| `01_link_dc` | `dc` sweep | link transfer curve |
| `01_link_ac` | `ac` | link response with two poles |
| `01_link_noise` | `noise` | shot + RIN + thermal |
| `01_link_tran` | `tran`, two tones | fundamental and IMD3 |
| `02_filter_ac` | `ac` | single-passband ring filter |
| `03_linewidth_noise` | `noise` | laser linewidth through a delay interferometer |
| `07_ttd_ac` | `ac` | four-element true-time-delay beamformer |
| `20_multiplier_hb` | `hb`, one tone | harmonics 2f, 4f, mean current |
| `21_mixer_hb_noise` | `hb`, two tones; `hbnoise` | mixer IF and mean; its periodic noise is checked against a closed form by `spectre/compare_spectre.py` only (verilog-a-sim has no periodic noise analysis) |
| `23_oeo_hb_noise` | autonomous `hb`; `hbnoise` | oscillation frequency and amplitude. **Its phase noise is not a reference** (13 dB above the formula, cause not settled: `paper_writing/mwp/spectre/README.md`) |
| `29_hbnoise_delay` | `noise`, `hb`, `hbnoise` | a signal plus its delayed copy (the delay at the noise frequency) |

Not committed, for size (30–100 MB of waveform each): `11_rc_trannoise`, `11_rc_libnoise`,
`22_oeo_tran`, `25_oeo_tran_bw`. Nor `24_oeo_hb_alt`, which converged to the non-oscillating
solution without an error, or `26`–`28`, which did not converge. Their outcomes are recorded in
`paper_writing/mwp/spectre/README.md`.

## Regenerating

On a machine with Spectre: copy `paper_writing/mwp/spectre/mwp_spectre_package.tar.gz`, unpack,
`bash run.sh [deck ...]`, and replace the directories here with those from `work/` (result
files, the `.scs`, and the `.log` renamed `spectre.log`). Then run the gate.
