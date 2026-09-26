# Proposal: a block-triangular (BTF) front end for the sparse DC solve

**Status:** proposed, 2026-09-26. Steps 1–2 measured (§6.1): **go**, with blocks from the
running union of nonzero entries. A last-digit shift in DC answers is accepted. Step 3 (build
it in `va-core`) awaits the go-ahead.
**Affects:** `va-core` only (`sparse.rs`: `SparseLu` and its callers' choice of solver). No
interface change: models, `StampSink` and `ModelInstance` are untouched.
**Follows:** `docs/proposals/parallel-assembly.md` §5.1 (option a) and its measurement §5.1.1;
`docs/proposals/sparse-solve.md` (the current sparse path).
**Evidence:** `cargo xtask btf <deck> --model <m> [--time-lu]` (1.19.0, plus the block-work and
BTF-time lines added with this document) and `va-cli --logfull`, release build, i7-1185G7
(4 cores / 8 threads), Windows 11, GNU toolchain, mains power. Timings are single runs unless
stated; the structural counts are deterministic.

> **What is measured and what is not.** Measured: the block structure of three circuits' matrices,
> the time `faer` takes per solve, how many solves a run makes, the cost of computing a BTF, and
> the dense work of factoring the blocks. **Not measured:** any block solver — none exists yet.
> Every speed-up below for the proposed design is an estimate from the flop counts, and §6's first
> step exists to replace it with a measurement before anything goes into `va-core`.

## 1. The problem

Since 1.18.0 evaluates devices in parallel, the linear solve is about half of every large DC run
(8 threads):

| deck | unknowns | Newton iterations | solve per iteration | solve total | run |
|---|---:|---:|---:|---:|---:|
| c432 `.op` | 15 416 | 558 (38 gmin stages) | 40.7 ms | 22.7 s | 43.6 s |
| chain160 `.op` | 5 286 | 150 (36 stages) | 10.8 ms | 1.62 s | 3.55 s |

The solve is serial, so it does not shrink with more cores: on a 32-core machine it would be most
of the run. `faer`'s own parallel LU makes it slower, not faster (parallel-assembly §5.1).

## 2. What the matrices look like

A matrix in **block triangular form** (BTF) factors as its diagonal blocks alone; everything
outside them is used once, in substitution. §5.1.1 of the parallel-assembly proposal counts the
blocks for three patterns — every stored entry, and only the entries numerically nonzero (DC `G`;
transient `G` or `C`), each unioned over four bias points:

| c432 | blocks | largest | blocks > 1 (unknowns in them) | Σ s³/3 (dense-block LU) | BTF time |
|---|---:|---:|---|---:|---:|
| stored | 53 | 15 364 | 1 (15 364) | 1.2 × 10¹² | 3.2 ms |
| **DC nonzero** | **10 829** | **52** | **234 (4 821)** | **1.6 × 10⁶** | 7.3 ms |
| transient nonzero | 8 767 | 6 650 | 1 (6 650) | 9.8 × 10¹⁰ | 12.3 ms |

| chain160 | blocks | largest | blocks > 1 (unknowns in them) | Σ s³/3 | BTF time |
|---|---:|---:|---|---:|---:|
| **DC nonzero** | **4 024** | **9** | **160 (1 422)** | **3.9 × 10⁴** | 1.5 ms |
| transient nonzero | 3 229 | 2 058 | 1 (2 058) | 2.9 × 10⁹ | 7.3 ms |

Three facts decide the design:

1. **By value, a DC logic matrix is almost triangular.** Logic feeds forward — a gate's output
   depends on its inputs, not the reverse — so the DC Jacobian splits into thousands of blocks, the
   largest 52 unknowns. Factoring every block densely is 1.6 × 10⁶ flops for c432: about a
   millisecond, against 40 ms for `faer` today.
2. **By structure, it is one block.** Models stamp explicit zeros (a MOSFET's gate row carries no
   DC current, yet the entry is stored), and the stored pattern keeps them on purpose
   (`sparse.rs`: "a value that is zero at one operating point can be nonzero at the next"). The
   explicit zeros glue the matrix into one block — but they are **not** what `faer` pays for:
   dropping them takes c432 from 34 to 31 ms per solve. The cost is the general-purpose
   factorization, which does not exploit the triangular structure the values have.
3. **Transient keeps one large block** (31–43% of unknowns): gate–drain capacitance couples each
   stage back to its driver. BTF peels off the rest as singletons, but the big block still needs a
   sparse LU.

## 3. Options

**(A) BTF front end on the value pattern, small blocks dense, large blocks through `faer`.**
Before factoring, find the blocks of the numerically nonzero pattern; factor each block of up to
some size `S` with a dense partial-pivoting LU, and any larger block with `faer`'s sparse LU
restricted to it; solve by block back-substitution. Pure Rust, no new dependency, no interface
change. What KLU does, minus its per-block ordering and its left-looking kernel.

**(B) A full KLU-style solver in Rust.** (A), plus a fill-reducing ordering per block (AMD) and a
Gilbert–Peierls left-looking sparse LU with refactorization that reuses the pivot sequence. The
best-known answer for circuit matrices, including the transient large block; several times the
effort of (A), and the numeric kernel becomes ours to maintain and validate.

**(C) Fewer solves instead of cheaper ones.** c432 needs 558 iterations because plain Newton
fails and the gmin rescue walks 38 stages. Making the rescue shorter, or reusing a factorization
across iterations (parallel-assembly §5.2), cuts evaluation *and* solve together. Orthogonal to
(A)/(B), and not measured here.

**(D) Another pure-Rust sparse solver crate.** Not surveyed. §5 rules out FFI (KLU, PARDISO,
SuiteSparse); a survey would go first.

## 4. Proposed: (A), measured before it is built

(A) captures the DC gain — the block work is small enough that the kernel does not matter — at a
fraction of (B)'s cost, and leaves (B) available for the transient large block if that turns out to
matter. The design points:

- **Blocks from values, cached.** Computing the BTF costs 7 ms on c432 — as much as the target
  solve — so it must not run on every iteration. Key it on the set of nonzero entries: recompute
  only when an entry becomes nonzero or zero. The set is bias-dependent (the stored pattern itself
  grew by 7 072 entries across four bias points), so how often it changes during a Newton solve is
  **the first thing to measure** (§6). If it changes on most iterations, the plan changes: compute
  blocks on the union of every nonzero seen so far (a union that only grows, like the stored
  pattern), trading a slightly coarser decomposition for a stable one.
- **Exact.** Dropping an entry that is exactly `0.0` in this matrix changes nothing about this
  solve. No threshold, no approximation.
- **Pivoting stays where it was.** A block permutation needs no pivoting across blocks; inside a
  block, dense partial pivoting. Numerical stability is then that of partial pivoting within each
  block (how it compares with `faer`'s sparse pivoting on these matrices is not checked — the
  prototype's `max |x − x_op|` against `faer`'s is the check).
- **Singularity** is detected as today: a zero pivot in a block, and the existing residual check
  (`RESIDUAL_TOL`) after the solve, so "failed" keeps meaning the same thing as on the dense and
  `faer` paths.
- **Reproducible.** The block order and each block's elimination order come from the pattern and
  the values, never the thread count. Serial first; the blocks on one level of the block DAG
  (widest 4 420 on c432) could be factored in parallel later with a fixed schedule — but c432's
  levels are mostly singletons, so this is unlikely to pay, and chain160's DAG is 324 deep (a
  chain is a chain).
- **Answers move** in the last digits: a different elimination order rounds differently. Not
  bit-identical to `faer`; the release reports the shift the way 1.17.0 did (`deck-diff` with
  magnitudes, `validate` errors), and keeps `faer` selectable for comparison.
- **Where:** inside `va_core::sparse::SparseLu` (or a sibling), chosen per solve: BTF when the
  largest block is small, `faer` on the whole matrix otherwise. Transient and AC unchanged at
  first.

**Expected, not measured:** c432's DC solve from ~40 ms to a few ms (dense-block LU ~1 ms,
substitution and the residual check each well under a millisecond, BTF amortised by the cache).
That would take about 20 s off c432's 43.6 s at 8 threads (≈ 1.9×), and about 21 s off its 80 s
at 1 thread. chain160 would lose most of its 1.6 s solve. Transient: little until the large block
is addressed.

## 5. Risks

- **The cache may not hold.** If the nonzero set changes every iteration, BTF costs as much as it
  saves. The union fallback above is the answer, if its blocks stay small — also to be measured.
- **A block can be large on another circuit.** Analog circuits with feedback (op-amps, the ring
  oscillator) are one big block by construction. The per-solve switch back to `faer` covers it, at
  the cost of computing the BTF to find out — cached, so once per pattern.
- **Exact zeros depend on the model.** A model that stamps `1e-300` instead of `0.0` couples blocks
  that are physically independent. That costs speed, not correctness.
- **Two solvers to keep in agreement.** The existing sparse-vs-dense agreement tests extend to
  a third path.

## 6. Steps

1. **Measure the cache.** Instrument a c432 and chain160 DC run (xtask, no `va-core` change): at
   every Newton iteration, record whether the nonzero set changed since the last one, and the
   block sizes of the running union. Decides between per-set caching and the union.
2. **Prototype the block solve in `xtask btf`** (`--time-block-solve`): factor and solve c432's and
   chain160's operating-point matrices by BTF + dense blocks, check `max |x − x_op|` as
   `--time-lu` does, and time it against `faer` on the same matrices. **Go** if the DC solve is at
   least 5× faster on c432; otherwise stop here and report.
3. **Build it in `va-core`** behind the solver choice, DC only. Gate: agreement tests against dense
   and `faer`; `deck-diff --all` with magnitudes against the previous release; `validate`.
4. **Transient**, separately: measure how much of a transient step's solve is the large block, and
   only then choose between (A) as is and (B) for that block.

### 6.1 Steps 1–2, measured (2026-09-26)

`cargo xtask btf <deck> --model <m> --track-newton --time-block-solve`, release build, mains
power. Decision 2 (§8) is taken: a last-digit shift in DC answers is acceptable.

**Step 1 — the cache.** `--track-newton` re-runs the deck's DC solve exactly as `va-cli` does
(default Newton configuration, same gmin rescue ladder), with every instance wrapped so each
assembly's summed Jacobian is recorded. The count of assemblies equals `--logfull`'s count of
Newton iterations (558 on c432), so every assembly is a solve.

| | assemblies | nonzero set changed | distinct sets | entries | running union stopped growing |
|---|---:|---:|---:|---|---|
| c432 | 558 | **251** times | 147 | 31 687 – 37 147 | **at assembly 1** (37 147 entries) |
| chain160 | 150 | 11 times | 5 | 11 206 – 13 126 | at assembly 1 (13 126 entries) |

**Per-set caching would not hold** on c432 — the set changes on almost every other iteration, as
devices move between regions. **The union does:** every later set is a subset of the second
assembly's, so a BTF of the running union is computed once or twice per solve. Its blocks are
the final assembly's: c432 12 597 blocks, largest **34** (the four-bias union of §2, largest 52,
was a looser bound); chain160 largest 5. **Decided by the measurement: blocks from the running
union** of nonzero entries, recomputed only when the union grows.

**Step 2 — the block solve.** `--time-block-solve`: transversal + Tarjan once (symbolic), then
per solve fill each block from the values, dense LU with partial pivoting, substitute in block
order, and run the same residual check `SparseLu::solve` makes. On each deck's operating-point
matrix (nonzero entries), against `faer` on the same matrix:

| | symbolic (once) | block solve (median of 5) | `faer` (median of 5) | speed-up | max \|x − x_op\| block / `faer` |
|---|---:|---:|---:|---:|---|
| c432 | 6.1 ms | **1.24 ms** | 29 ms | **23×** | 4.0e-13 / 5.7e-13 |
| chain160 | 2.2 ms | **0.40 ms** | 8 ms | **20×** | 2.2e-16 / 2.2e-16 |

**Go** (the criterion was 5×). Accuracy is `faer`'s or better on both. What it implies,
estimated from these numbers rather than measured end to end: c432's 558 solves from 22.7 s to
about 0.7 s (plus two symbolic phases, ~12 ms), taking its 8-thread `.op` from 43.6 s to roughly
22 s; chain160's 1.62 s of solve to about 0.06 s.

Not yet covered, and to be handled in Step 3: an entry in the union that is exactly zero in a
given iteration can make a block numerically singular where the matrix is not (the dense LU then
fails, and the solve must fall back to `faer` rather than report singular); the `gmin` shunt adds
diagonal values the tracked stamps did not include (the diagonal adds no edges, so the blocks do
not change, but it changes pivots); and transient remains the separate Step 4.

## 7. How it is proved not to break anything

- A solve returns `x` with `‖A·x − b‖` within `RESIDUAL_TOL`, checked after every solve, on every
  path — unchanged.
- Unit tests: known block structures (the `xtask btf` fixtures), a singular block, a block with a
  zero diagonal that needs a pivot, agreement with dense and `faer` on random sparse systems.
- Whole-program: `deck-diff --all` against the previous release, reported with magnitudes;
  `validate` 28/28 with errors compared to the previous release's.

## 8. Decisions needed

1. ~~Proceed with steps 1–2?~~ Done (§6.1): go.
2. ~~Is a last-digit shift in DC answers acceptable?~~ Yes (decided 2026-09-26).
3. Should (C) — why c432 needs 38 gmin stages, and factorization reuse (§5.2 of the
   parallel-assembly proposal) — be measured in parallel? It reduces both halves of the run.
4. Build Step 3 in `va-core` (DC only, `faer` kept as the fallback and as a selectable path)?
