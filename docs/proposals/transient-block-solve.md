# Decision: a block-triangular transient solve (A), and leaving the thread pool as it is (B)

**Status:** decided 2026-09-26 from the measurements below (the user delegated both decisions to
them). (A) is implemented in 1.26.0; (B) is not changed.
**Follows:** `docs/proposals/transient-solve.md`, options (A) and (B), and its decision 3 (add a
c432-size transient deck to decide (A) on evidence).
**Evidence:** release builds, i7-1185G7 (4 cores / 8 threads), mains power, one run per
configuration unless stated. Solve/evaluation splits from timers placed temporarily around the
integrator's `assemble()` and `Linear::solve` (a scratch build, not committed); the block-solve
figures from `cargo xtask btf <deck> --model <m> --time-tran-block-solve` (new).

## 1. The new deck

`circuits/benchmark/iscas85/c432_tran.net` — c432 at transistor level, the same circuit as
`c432.net` with its 36 inputs pulsing (`gen_iscas.py … tran 1n`): **1 ns**, not 12, because at
today's speed a nanosecond already takes ~20 minutes. Every input's first edge falls in the first
0.8 ns, so the logic switches. It is in `xtask deck-diff`'s slow list.

## 2. (A) — at c432's size the transient solve is the bottleneck

| c432, 1 ns transient, 8 threads (1.24.0) | total | per call |
|---|---:|---:|
| wall — 2 524 timepoints | 1 306.9 s | — |
| device evaluation — 17 301 assemblies | 686.3 s (52%) | 39.7 ms |
| linear solve (`faer`) — 14 775 solves | **585.5 s (45%)** | **39.6 ms** |

On c17 (420 unknowns) the solve was 9% of the run and not worth building for; at 15 416
unknowns it is nearly half. The matrix a transient step factors, `G + C/h`, keeps one large block
beside thousands of single unknowns — c432's: one block of **4 882** unknowns (31.7%), 10 534
singletons (`xtask btf`). A block solve that hands the large block to `faer` alone and does the
rest by substitution, on c432's companion matrix at the operating point (h = 1 ps), median of 5:

| | hybrid block solve | `faer`, whole matrix |
|---|---:|---:|
| c432 | **4.1 ms** (plan 14.6 ms, once per growth of the union) | 33 ms |
| max \|x − x_op\| | 4.8e-14 | 7.9e-13 |
| c17 | 0.06 ms | < 0.5 ms (below the tool's resolution) |

**Decided: build it.** `va_core::btf` factors a block larger than `MAX_BLOCK` (64) with `faer` on
that block alone instead of refusing the matrix, and the transient integrator uses the block path
(`TranConfig::btf`, from `VA_BTF`, default on). A block holding ≥ 90% of the unknowns
(`WHOLE_MATRIX_FRACTION`) still sends the whole matrix to `faer`: blocks would save nothing, and a
fully coupled circuit (the ring oscillator) keeps its exact answers.

What it did: see §4 (filled in with the implementation, 1.26.0).

## 3. (B) — the serial solve slows as the pool grows; not worth changing

c17's transient at each pool size (the solve is serial on the main thread; evaluation uses the
pool):

| threads | wall | evaluation per assembly | solve per call |
|---:|---:|---:|---:|
| 1 | 156.1 s | 2.75 ms | 0.115 ms |
| 2 | 118.6 s | 2.03 ms | 0.142 ms |
| 4 | 91.9 s | 1.51 ms | 0.170 ms |
| 6 | 88.4 s | 1.44 ms | 0.182 ms |
| 7 | 88.2 s | 1.42 ms | 0.189 ms |
| 8 | **80.7 s** | 1.29 ms | 0.183 ms |

The solve slows **steadily** with the pool size, not at 8 alone — so it is not the main thread
competing with an eighth worker for a logical core (7 threads is no better than 8). The pattern
fits the CPU running at a lower clock with more cores busy (rayon's workers spin briefly after
each assembly), but that is not isolated. Its whole cost at 8 threads is ~2.8 s of ~80 s
(3.5%), and the one available knob — fewer threads — loses more in evaluation than it gains in
the solve.

**Decided: no change.** Revisit only if a profile shows the solve's share growing, e.g. after (A)
makes c432's solve cheap and evaluation dominates again.

## 4. Gates for (A)

To be filled in with the implementation (1.26.0).
