# Platform benchmark for the compiled `va-cli`

One file per platform, each meant to be **pasted into a terminal** inside an unpacked release
archive (`va-cli-v2.0.0-<target>`, from the GitHub releases page). Each file starts with its own
step-by-step instructions as comments.

| Platform | File | Archive |
|---|---|---|
| Linux, x86-64 | `bench_linux.sh` | `va-cli-v2.0.0-x86_64-unknown-linux-gnu.tar.gz` |
| macOS, Apple silicon | `bench_macos.sh` | `va-cli-v2.0.0-aarch64-apple-darwin.tar.gz` |
| Windows, x86-64 | `bench_windows.ps1` | `va-cli-v2.0.0-x86_64-pc-windows-msvc.zip` |

Each run writes **`benchmark.txt`** in the archive directory:

- **the machine:** CPU (with its SIMD level on Linux, and P/E cores on macOS), core and thread
  counts, memory, OS version, power source, and the power plan or governor. A laptop on battery
  or in a power-saving mode is flagged, since its timings are not a baseline;
- **one line per deck:** wall time, exit code, and the SHA-256 of the deck's output.

## The deck set (~4 min on an i7-1185G7, 8 threads)

| Deck | What it exercises |
|---|---|
| `rectifier` (.tran) | the smallest end-to-end run: a diode compiled from Verilog-A |
| `ring_osc` (.tran) | reference BJTs, a long adaptive transient |
| `nand/psp103` (.op) | one PSP103 NAND: compact-model compile and evaluation |
| `psp103_inverter_tran` (.tran) | PSP103 transient on the dense path |
| `iscas85/c17` (.op) | 6 gates at transistor level |
| `iscas85/c432` (.op), default threads and `--threads 1` | 15 416 unknowns, sparse/BTF solve and the `gmin` rescue; the pair shows the parallel-evaluation speed-up |
| `iscas85/c499`, `c880` (.op) | 28 492 and 32 366 unknowns |
| `iscas85/c17_tran` (.tran, 12 ns) | ~13 500 transient points with PSP103 |
| *optional* `c432_tran` (.tran, 1 ns), `c7552` (.op) | ~15 min, and ~6 min with ~4 GB of RAM (268 676 unknowns) |

## Reading the results

- **Times** are single runs, including model compilation and output writing. For a steadier
  figure, run the benchmark twice and keep the lower time of each deck.
- **Hashes** should be **identical across all three platforms**: since 2.0.0 the output is
  bit-identical on Linux, macOS and Windows (CI checks the same thing on its own runners). A
  differing hash on a deck with exit code 0 is worth reporting. The two outputs are in each
  machine's `bench-out/` and can be diffed.
- A nonzero exit code on any deck is a failure to report, with that deck's `bench-out/*.err`.

## The full ISCAS'85 suite (Linux)

`bench_iscas_linux.sh`, pasted the same way, runs all eleven circuits (c17, c432 ... c7552 as
`.op`, plus c17's transient; `LONG=1` adds `c432_seq` and `c432_tran`) and writes
`iscas_suite.txt`: wall time, exit code, output hash, and a logic check of every reported gate
output with `check_iscas.py` (needs `python3`). ~19 min of operating points on an i7-1185G7.

## Results received

| File | Machine | Notes |
|---|---|---|
| `benchmark_linux.txt` | AMD Ryzen 9 9950X, 32 threads, Ubuntu 26.04 | desktop, `performance` governor |
| `benchmark_windows.txt` | Intel i7-1185G7, 8 threads, Windows 11 | **on battery**, balanced plan, and a test run overlapped c17_tran: timings not a baseline |
| `benchmark_macos.txt` | Apple M2 Pro (6P+4E), 10 cores, macOS 26.6.2 | on mains, but load average 4.09/9.11/8.27 before the run: timings indicative, not a baseline |

All ten hashes agree across all three (x86_64 Linux, x86_64 Windows, aarch64 macOS).
