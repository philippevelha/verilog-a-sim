# Proposal: what to speed up in a transient run — measured on c17

**Status:** measured 2026-09-26; nothing decided. §4 lists the options with what the measurement
says each can gain; §5 the decisions.
**Affects:** nothing yet. Candidates would touch `va-transient` (the integrator's per-step Newton
and its linear solve) and `va-core` (`par`, `btf`).
**Follows:** `docs/proposals/btf-solver.md` Step 4 ("transient, separately") and §5.1.1 of
`docs/proposals/parallel-assembly.md`, which found the transient matrix keeps one large block.
**Evidence:** 1.24.0, release build, i7-1185G7 (4 cores / 8 threads), mains power, one run per
configuration; timers placed temporarily around the integrator's `assemble()` and
`Linear::solve` (a scratch build, not committed — the counts are deterministic, the times are
single runs).

## 1. The measurement

ISCAS'85 c17 at transistor level, `.tran` 0–12 ns: 420 unknowns, ~30 PSP103 instances, 13 462
accepted timepoints.

| | 8 threads | 1 thread |
|---|---:|---:|
| wall | **75.5 s** | 156.1 s |
| device evaluation — 53 683 assemblies | **64.4 s (85%)**, 1.20 ms each | 147.8 s (95%), 2.75 ms each |
| linear solve (`faer`) — 40 219 solves | **6.9 s (9%)**, 0.172 ms each | 4.6 s (3%), 0.115 ms each |
| everything else (LTE, stepping, output) | ~4 s | ~4 s |

Per accepted timepoint: **~3.0 Newton solves and ~4.0 assemblies** — the assemblies exceed the
solves by 13 464, one per accepted point plus two: the evaluation at the accepted solution that
commits its charges and state (`StateBuffers::commit`), after the Newton iterations.

## 2. What it says

1. **The transient linear solve is not the bottleneck.** 9% of the 8-thread run; a solver that
   took it to zero would save ~7 s of 75. The block structure explains why the 1.21.0 block
   solver does not apply — c17's transient matrix keeps one strongly coupled block of 180 of its
   420 unknowns (gate–drain capacitance couples each stage back to its driver) — but also why
   that barely matters here: the solve is already 0.1–0.2 ms.
2. **Evaluation is 85%, and parallelises poorly at this size:** 2.75 → 1.20 ms per assembly on
   8 threads, 2.3×. Thirty instances is about four per thread; the hand-off and the replay are a
   larger share than on c432 (910 instances, where evaluation scaled 3.6×).
3. **The serial solve is 50% slower when evaluation runs in parallel** (0.115 → 0.172 ms per
   solve). Likely rayon's worker threads still spinning when the main thread starts the solve,
   competing for the cores — not measured beyond this.
4. **One evaluation in four is the post-accept one.** It is not waste — it is how the accepted
   point's charges and state are committed — but it is a quarter of the evaluation budget.

## 3. For scale: what a larger transient would look like

Not measured: a transient run of a circuit c432's size. Its DC matrix splits into small blocks,
its transient matrix keeps one block of 31–43% of the unknowns (`btf-solver.md` §2), and there
the solve per step would be far from 0.1 ms. c17 says "not now for c17", not "never".

## 4. Options

**(A) A block-triangular transient solve** — the 1.21.0 block solver with the large block
factored by `faer` instead of refusing the matrix. Gain on c17: at most ~7 s of 75.5 (9%), and
less in practice (the 180-unknown block still costs a sparse LU). Worth it for c432-size
transients, not measured.

**(B) Let the solve have the cores.** Make rayon's workers stop spinning before the serial solve
(e.g. a pool configured to sleep sooner, or evaluation and solve not overlapping). Gain on c17:
up to the 2.3 s the solve lost to contention (6.9 − 4.6 s). Cheap to try; the effect on
evaluation speed is the thing to measure.

**(C) Cheaper evaluation per assembly** — the single-thread evaluator work of 1.13.0–1.16.0,
resumed: the gradient-storage proposal (`docs/proposals/gradient-storage.md`, a 13–19% bound
measured on `load()`), since evaluation is 85% of this run and scales only 2.3× here.

**(D) Fewer evaluations per timepoint.** ~3 Newton iterations and one commit evaluation per
accepted point, over 13 462 points. Options include a better predictor for the first Newton
iterate, a bypass for instances whose terminal voltages did not move, and a look at the step
count itself (13 462 points for 12 ns with 100 ps edges). Each needs its own measurement; none
is small.

**(E) Better parallel scaling on small circuits** — less overhead per assembly: reuse the stamp
recordings instead of allocating one per instance per assembly (a stated limitation of
`va_core::par`), and measure what the hand-off itself costs at 30 instances. Gain bounded by the
gap between the 2.3× measured here and the ~3.6× evaluation scaling c432 reaches.

## 5. Decisions needed

1. Accept that the transient **solve** is not worth building for at c17's size (A deferred
   until a c432-size transient is measured)?
2. Which of (B)–(E) to measure next? The cheapest to try is (B); the largest share is
   evaluation, (C)/(D)/(E).
3. Should a c432-size transient be added as a benchmark deck (it would run for hours at today's
   speed), to decide (A) on evidence?
