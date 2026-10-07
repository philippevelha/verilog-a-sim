# Transistor-level SNN: where the time goes, and what would make training affordable

**Status:** analysis and plan, 2026-10-07. Section 3's changes are in 2.17.0. Section 4 is the
ranked list of what is still open. Nothing in Section 4 has been decided.
**Affects (2.17.0):** `va-codegen` (`sched.rs`, `lib.rs`: how a compiled model is evaluated),
`va-core` (`par.rs`: device bypass's parallel decision and replay bookkeeping). Interfaces α and β
are not touched.
**Workload:** `circuits/snn/transistor/`: the Xuan et al. memristor-crossbar SNN with SG13G2 PSP103
access transistors (`fig5_access.net`) and the MNIST crossbars (`gen_mnist_tx.py`).

All figures below come from one machine (Xeon @ 2.8 GHz, 4 vCPU, Linux, `--release`). The
"before" binary is 2.16.2+6 and the "after" binary is 2.17.0, both built from this tree. Every
pair of runs gives byte-identical output (SHA-256 of stdout).

---

## 1. The workload, in numbers

| Deck | PSP103 | Unknowns | Notes |
|---|---|---|---|
| `fig5_access.net` | 8 | 208 | one neuron (behavioural), 8 access transistors, 15 steps |
| `xbar_h2_8` (`gen_mnist_tx.py --layers h2 --per-layer 8 --neurons behavioural --cycles 1`) | 1 024 | 17 710 | hidden-2 crossbar, 8 neurons, one 250 ns cycle |
| MNIST, every layer at transistor level (2.6.0) | 17 664 + neurons | 437 480 | 29 cycles |

**One SG13G2 PSP103 has 21 unknowns:** 4 terminals, 8 internal nodes and 8 branch-current rows.
Of the 17 710 unknowns in `xbar_h2_8`, 16 519 are rows that the transistors add. Most of those rows
are the ideal shorts of PSP103's collapsible resistors (`V(S,SI) <+ 0` when `RSE = 0`), plus
mixed branches whose card makes them resistors.

## 2. Where a run spends its time (2.17.0, sampling profile)

`xbar_h2_8`, one thread, `--bypass`:

| | share |
|---|---|
| fresh device evaluation (`GeneratedModel::load`) | ~64 % |
| … of which the static program (`sched::exec`) | ~47 % |
| … of which contributions turned into `Dual`s and stamped | ~10 % |
| bypass bookkeeping (staleness check, corrected replay, recordings) | ~15 % |
| sparse LU (factor + solve) | ~7 % |
| everything else (LTE, output) | ~14 % |

Without bypass, device evaluation is ~95 % of the run. The linear solve is not the bottleneck at
these sizes. One evaluation is.

## 3. What 2.17.0 does

**The static program runs less code** (`crates/va-codegen/src/sched.rs`, module doc "Less to
run"). The program folds constants, drops assignments nothing reads, lays out an `if` whose arms
are only assignments as jumps, and runs consecutive such statements as one stretch. In the old
build, a PSP103 load ran ~3 000 instructions, with one `exec` call per assignment and a trip
through the statement walk for every condition. Constant folding and dead-assignment removal cut
the instruction count. The whole-`if` layout and stretches cut the statement-walk overhead between
instructions, which had been about a fifth of a load in the profile.

**Bypass's parallel decision uses the actual fresh work** (`crates/va-core/src/par.rs`). Before,
an assembly with bypass went parallel based on a per-instance cost estimate that it never measured.
After the operating point the estimate was zero, and zero meant parallel. So a bypassed assembly
handed even two or three cheap leftover devices to the thread pool, and 4 threads ran 1.9× slower
than 1. Now each instance's evaluation time is measured, serially, the first time and every
256 assemblies after. An assembly goes parallel when the instances it must evaluate fresh add up
to `PARALLEL_MIN_WORK_NS`. Also, a stored recording takes its sparse slots from the slot cache
that has just placed it, so its first replay does not look every stamp up again. Each stamp's
drift column is found once, when the recording is stored, not on every replay.

| | before | after | |
|---|---|---|---|
| one PSP103 `load()` (`xtask bench-instance …/bench_access_bias.net`) | 78 µs | **47 µs** | 1.66× |
| `fig5_access`, 1 thread | 51.5 s | **33.2 s** | 1.55× |
| `fig5_access`, 4 threads | 37.5 s | **30.2 s** | 1.24× |
| `fig5_access`, 1 thread, `--bypass` | 10.2 s | **7.6 s** | 1.34× |
| `fig5_access`, 4 threads, `--bypass` | 19.3 s | **9.8 s** | 1.97× |
| `xbar_h2_8`, 4 threads | 81.1 s | **55.7 s** | 1.46× |
| `xbar_h2_8`, 4 threads, `--bypass` | 37.5 s | **29.1 s** | 1.29× |
| `xbar_h2_8`, 1 thread, `--bypass` | 81.5 s | **57.2 s** | 1.42× |

Single runs, so read them as ±5 %. Each before/after pair ran back to back on an otherwise idle
machine.

**Still true after 2.17.0:** on a small deck, more threads help little. `fig5_access` on 4 threads
is 30 s against 33 s on one, because 8 expensive devices among 74 split poorly across threads. With
bypass, 4 threads are still slower than 1 (9.8 s against 7.6 s): the thread hand-off on this
machine costs more than the 200 µs threshold assumes. The threshold was tuned on an i7. It was not
re-tuned here.

## 4. What is still open, ranked by what it would buy for training

### 4.1 Parallelism across runs, not within one (available now; workflow, no code)

Training-set images are independent runs. One run splits its devices across threads at 25–60 %
efficiency on these decks: `xbar_h2_8` gets 2.9× on 4 threads, and `fig5_access` 3.5× on 32
threads (2.3.0+3). N single-threaded runs side by side give close to N×, limited by memory
bandwidth and RAM. `xbar_h2_8` needs a few hundred MB. The 64-neuron crossbar needed 3.4 GB
(2.6.0). **For throughput, run `--threads 1` per image and one image per core.** A batch driver
(`run_all.sh` with a job count) is the only thing missing.

### 4.2 Compile the static program to machine code (largest single win; needs sign-off)

`sched::exec` is still an interpreter. Every instruction is a `match`, every partial costs an
index load, and the dispatch branch mispredicts. All register offsets and maps are known when the
model is built, so the program is ready to hand to a code generator. In a compiled function every
partial becomes a load from a fixed address, one or two flops and a store, with no dispatch.
Cranelift is pure Rust, so the no-native-link rule (`CLAUDE.md` §5) holds. **But calling generated
code needs `unsafe`, which needs a written justification and owner sign-off (`CLAUDE.md` §5).**
Expected gain: `exec` is ~50 % of a load. Compiled models run PSP103 in single-digit µs, so 5–10×
on that half is plausible. That would put one load at roughly 20–25 µs. That figure is an
estimate, not a measurement. `xtask sched-check` already compares every stamp against the tree
walk, and it would be the gate for this change.

### 4.3 Contributions without allocations (~10 % of a load)

Each contribution term is turned back into a dense, heap-allocated `Dual` (`Run::dual` →
`Dual::from_entries`). Its terms are then summed and scaled through `Arc` buffers before
stamping. Stamping straight from the result register's partials would remove the allocations.
The scaling and summing would have to happen in the same order to keep the bits.

### 4.4 Collapse PSP103's ideal shorts (matrix and memory, not evaluation)

Currently an ideal short (`V(a,b) <+ 0` under the card's parameters, `lower::IdealShort`) keeps
its own node and its own branch-current row. Merging the two nodes and dropping the row, as
SPICE's node collapse does, would take one SG13G2 PSP103 from 21 unknowns to roughly 12. Not
allocating a row for a mixed branch that the card proves is always a resistor would take it lower
still. That roughly halves the crossbar's matrix and the bypass bookkeeping per device. **This
changes behaviour:** fewer unknowns are reported (`X1.SI`, `X1.b1`), and the factorization rounds
differently. So it needs its own release and its own deck-diff tolerance argument, not a
bit-identity claim.

### 4.5 Bypass: tolerance and default

At `--bypass-reltol 1e-3` instead of 1e-6, `xbar_h2_8` replays 85.3 % of loads instead of
78.6 %, and runs ~18 % faster (measured in 2.16.2+6, one thread each, four runs side by side).
What that costs in V_m and spike timing is **not characterized**. A tolerance study like 2.6.0's
`tolerance_study.py`, but over bypass tolerances, would show it. Bypass is still opt-in, and no
golden covers a bypassed transistor (`device-bypass.md` §6).

### 4.6 What none of this changes: transistor-level runs cannot be the training loop

Scale the measured figures up. 2.6.0 measured ~160 µs per access transistor per timepoint
(plain, i7). With 2.17.0's ~1.5× that is ~100 µs. The MNIST crossbars have 17 664 access
transistors. The behavioural run needed ~2 200 timepoints per 250 ns cycle, and 29 cycles is
~64 000 timepoints. That gives about 1.1 × 10⁵ core-seconds for the crossbars of **one image's
forward pass**, roughly 30 core-hours. Bypass's ~4× and Section 4.2's estimated 2× on top would
bring that to a few core-hours. That is fine for validating a trained network on a few dozen
images, and four to five orders of magnitude too slow for a training set. The workable shape is
**hardware-aware training**:

1. characterize the cell at transistor level (the access transistor's R_on against its bias,
   which already moved V_m by 131 mV in 2.3.0, and the neuron's transfer, leak and threshold);
2. fold those characteristics into the behavioural model the training uses (as 2.3.0 suggested
   for the codebook);
3. check the trained network at transistor level on a handful of images, as
   `circuits/snn/mnist/` did at circuit level for ten.

The simulator work above makes steps 1 and 3 faster. It does not make step 2 unnecessary.
