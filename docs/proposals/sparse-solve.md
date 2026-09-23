# Proposal: a sparse linear solve above 500 unknowns

**Status:** proposed, 2026-09-23. The threshold (500) was chosen by the user. **Steps 1 and 2 done
(1.5.0, 1.6.0, 2026-09-23)**; Steps 3–5 open.
**Affects:** `va-core` (new sparse assembler + solver, the selector), `va-transient` and
`va-acnoise` (their assemblers and solve call sites), `va-cli` (`--solver`, the pre-flight line),
`xtask` (`bench-scale` gains a sparse column). **`va-abi` (Interface β) and `va-ir`
(Interface α) are not touched** — see §3.1, which corrects `docs/future_development.md` §1 on
this point.
**Follows:** `docs/future_development.md` §1; the 2026-08-31 measurement in `docs/roadmap.md`'s
T3 section (`cargo xtask bench-linsolve`); the 2026-09-17 size limit in `docs/validation.md`
("The dense-LU circuit-size limit").

This is a decision document. No Rust code changes are part of this proposal.

---

## 1. What is wrong today

Every linear solve in the engine is dense LU, and every assembled Jacobian is a dense
`dim × dim` buffer:

| Where | Buffer | Solve |
|---|---|---|
| DC Newton | `va_core::mna::System::jacobian` (`dim²`) | `crates/va-core/src/newton.rs:250` |
| Transient Newton | `va_abi::stamps::DenseStamp` — `jacobian` **and** `dcharge` (`2·dim²`), plus the companion matrix `J + coeff·dQ` cloned from them (`integrator.rs:966`) | `crates/va-transient/src/integrator.rs:976` |
| AC and noise | `ac::Linearization` — `g` and `c` (`2·dim²`), embedded into a real `2dim × 2dim` system per frequency | `crates/va-acnoise/src/ac.rs:310` (`solve_block_embedded`, shared with the noise adjoint) |

On top of each buffer, `faer`'s dense LU makes its own copy to factor.

**Time is the limit that binds first.** Measured 2026-09-17 at v1.1.0 (`docs/validation.md`):
a 10 000-point `.tran` is interactive up to about 400 unknowns, takes 2.4 minutes at 400 and
8.7 minutes at 800, and is impractical beyond about 1 600. Dense LU costs O(dim³) per Newton
iteration, and an MNA Jacobian is almost entirely zeros: the RC-ladder Jacobian in
`bench-linsolve` is 0.6% full at 500 unknowns and 0.03% full at 10 000.

**Memory becomes the limit later.** One `dim²` buffer of `f64` is 2 MB at 500 unknowns,
32 MB at 2 000 and 800 MB at 10 000. A transient run holds at least four of them, so
10 000 unknowns needs more than 3 GB before any model is loaded. Since v1.3.2 shared the IR
across instances, the dense Jacobian is what the memory slope measures: a PSP103 chain uses
0.78 MB per device and the slope grows superlinearly.

**The sparse prototype does not fix this.** `va_core::linsolve::solve_sparse` (2026-08-31)
takes the same dense row-major matrix and scans all `dim²` entries to find the nonzeros before
it factors. It measured 12–33× faster than dense on the solve alone, but it keeps the dense
buffer, so it saves no memory, and it is not wired into any analysis.

## 2. The decision: dense below 500, sparse from 500

The rule applies to `dim`, the number of global unknowns (nodes + branch currents + internal
and state unknowns), not the node count:

- `dim < 500` → **dense**, the current code path, **unchanged**.
- `dim ≥ 500` → **sparse**.

The choice is made **once per circuit**. Every analysis in one `sim` run (`.op`, `.dc`, `.tran`,
`.ac`, `.noise`) uses the same solver. AC compares against `dim`, not against its `2·dim`
embedded system, so a circuit never gets different solvers for different analyses.

**Why 500 and not 2 000.** Dense is already impractical for `.tran` between about 800 and 1 600
unknowns (§1), so a 2 000 threshold would keep that whole range on the slow path and gain no
safety from it. The safety comes from §5's gates, not from how high the threshold is. At 500:

- **Every existing golden gate stays on the dense path.** Every netlist in `circuits/` is under
  60 lines, and the models in them add tens of internal unknowns, not hundreds. So all 28
  `xtask validate` gates should be unaffected. Step 2 prints each gate's `dim` to confirm that
  rather than assume it, and §5 proves the output did not change.
- **Sparse is clearly faster at 500.** The prototype, which still scanned the dense matrix,
  measured 15.4 ms dense against 1.09 ms sparse at `dim = 501` (`bench-linsolve`, 2026-08-31).
- **500 is where the 2026-08-31 decision said to revisit** ("revisit at ≳500 nodes"), and it is
  just above the ~400 unknowns where `.tran` stops being interactive.

**500 is the starting value, not a permanent one.** Step 5 (§4) measures the crossover with the
real sparse path. If the data points to a different threshold, that becomes its own recorded
change with the measurement in its `release.txt` entry. The threshold is a single named constant
in `va-core`, not scattered across crates.

**An override exists from the start:** `va-cli sim --solver dense|sparse|auto` (default `auto`).
`--solver sparse` is what lets the sparse path run on the existing small circuits (§5).
`--solver dense` above 500 still works, and is the fallback if the sparse path misbehaves on a
circuit. The pre-flight size/cost line (v1.1.0) names the solver that was chosen, and why.

## 3. Design

### 3.1 Sparse assembly without changing Interface β

`StampSink` only receives `(row, value)` and `(row, col, value)` calls
(`crates/va-abi/src/stamps.rs`). It never hands a model a matrix. How the sink stores those
values is up to the sink. A sparse sink is therefore a new `StampSink` implementation in
`va-core`, next to `mna::System`, and **no model and no trait signature changes**.
`docs/future_development.md` §1 said "a sparse assembly needs `StampSink` to accumulate triplets,
which is a `va-abi` (Interface β) change under §6". That is not so, and this proposal corrects it
there. `DenseStamp` stays in `va-abi` as it is. `va-transient` and `va-acnoise` use the new sink
from `va-core`, which both already depend on.

The sink works in two phases:

1. **Discovering the pattern.** The first assembly at a given `dim` records every `(row, col)`
   any model stamps. It then builds the compressed-column structure and a lookup from each
   `(row, col)` to its slot in the value array.
2. **Filling values.** Every later assembly zeroes the value array and adds each stamp into its
   slot. There is no search and no allocation per stamp. A hash lookup per stamp is the simple
   version. A per-instance slot cache (each instance stamps the same entries in the same order)
   is the fast version, done only if Step 5's profile says the hash is where the time goes.

Rules the pattern must follow:

- **An entry stamped with `0.0` is still in the pattern.** The dense code, and the prototype's
  `nnz`, treat an exact zero as absent. Here that would be wrong: a model can stamp `0.0` at one
  operating point and a nonzero value at the next.
- **Every diagonal entry is in the pattern.** `System::shunt_gmin` writes the diagonal of every
  `Node` row, and `gmin` is the rescue path (v1.3.1), which must not fail for want of a slot.
- **Transient uses the union of the `jacobian` and `dcharge` patterns**, since the companion
  matrix is `J + coeff·dQ`. The two are held as value arrays over one shared structure, so
  forming the companion matrix is one pass over the values, not a new matrix.
- **A stamp outside the known pattern is not an error.** Verilog-A contributions can sit inside
  `if`/`case`, and `@(above)` bodies change the equations when they fire, so an entry that was
  not stamped at the first operating point can appear later. The sink records it, the pattern
  grows to the union, and the symbolic factorization is redone (§3.2). This case gets its own
  test (§5). It is expected to be rare. If it turns out to happen on every iteration for some
  model, that is something to measure, not something to tune for in advance.

### 3.2 The solve

`faer` 0.22 already provides everything needed, with no new dependency and nothing for
`deny.toml` to check:

- `SymbolicLu::try_new` once per pattern. Its result is `Arc`-backed, so it is cheap to keep and
  reuse.
- `Lu::try_new_with_symbolic` once per Newton iteration: only the numeric factorization is
  repeated. Within one circuit the pattern is fixed across iterations, timesteps, sweep points
  and frequencies.
- `Lu` is generic over `ComplexField`, so AC and noise can factor `G + jωC` directly as a complex
  sparse matrix instead of embedding it in a real system twice the size. That is Step 4's choice
  to make (§4).

Things `solve_dense` guarantees today that the sparse solve must also guarantee:

- **The `catch_unwind` boundary stays.** `faer`'s sparse LU panics on at least one singular
  matrix (`faer-0.22.6/src/sparse/linalg/lu.rs:1426`) instead of returning `Err`. CLAUDE.md §5
  forbids this crate from panicking, so the panic is caught and returned as
  `CoreError::Singular`, exactly as the prototype does.
- **`residual_ok` becomes a sparse matrix–vector product.** Today it is an O(dim²) loop over the
  dense matrix, and it runs after every solve. Left dense, it would reintroduce the cost that
  sparse removes.
- **`check_finite` scans the value array and reports the `(row, col)` of the entry it finds**, so
  `CoreError::NonFinite { row, col }` names the same row and column it does now. That error is
  how a user finds the model that produced the NaN (2026-09-11). Losing the column would be a
  regression.
- **Singular detection means the same thing on both paths.** A matrix dense rejects, sparse
  rejects. Which error each one returns on a *nearly* singular matrix may differ, since the pivot
  order is different, and the tests assert that both reject, not which error they return (see
  the macOS lesson in `release.txt` 1.3.2+1).

### 3.3 Pivoting on MNA rows

A voltage source's branch row has a zero on the diagonal. Sparse LU with a fill-reducing
ordering can choose that zero as a pivot unless its pivoting looks down the column.
`faer`'s sparse LU uses partial pivoting: within each column, after the fill-reducing column
ordering, it picks the row with the largest magnitude (`faer-0.22.6/src/sparse/linalg/lu.rs`,
around line 1403). So a zero diagonal is not chosen when a nonzero is available. The prototype
already solves the resistor-ladder-plus-source systems in `bench-linsolve`, but that is one
circuit shape. Step 1's tests include
voltage-source-heavy matrices and the constraint rows that `laplace_*` and `zi_*` states add.

## 4. Steps

Each step lands as its own release. Steps 1–4 each add a feature and bump the **minor**
(CLAUDE.md §12). Each step leaves the dense path unchanged and the workspace green.

1. **Sparse sink + sparse solve in `va-core`, not called by any analysis yet.**
   `SparseSystem` (the §3.1 sink), `SparseLu` (the §3.2 wrapper that reuses the symbolic
   factorization), the sparse `residual_ok` and `check_finite`, and a `Solver` enum
   `{ Dense, Sparse, Auto }` with the 500 constant. The prototype `solve_sparse` is kept as the
   benchmark reference until Step 5 replaces it. Tests: §5's agreement and pattern-growth tests.
2. **DC.** `newton` and `dc` take a `Solver`, and `va-cli` gains `--solver` and the pre-flight
   line. DC goes first because it is the simplest loop and every other analysis starts from its
   operating point.
3. **Transient.** `integrator.rs`'s assembler switches to the shared-structure sink, and the
   companion matrix is formed over the values. This is the step users feel, because transient
   is where the O(dim³) cost is multiplied by thousands of timepoints.
4. **AC and noise.** `solve_block_embedded` gets a sparse path. The open choice here is between
   a complex sparse LU and a sparse version of the current real embedding. The recommendation
   is complex: it halves the matrix size and the embedding's sign conventions live in one place
   today, so replacing them is one change. The noise adjoint solves the **plain transpose**
   (`noise.rs`, `transpose: true`). The sparse path gets that by factoring the transposed
   structure, not by a conjugate-transpose solve, and it gets a test that would fail if it were
   conjugated.
5. **Measure and set the threshold.** `bench-scale` gains a sparse column for `.op`, `.tran`,
   `.ac` and `.noise`, and the peak-RSS slope on the PSP103 chain is measured again
   (method in `docs/validation.md`). `docs/validation.md`'s size-limit section is rewritten
   from those numbers. If the crossover is far from 500, the threshold change is its own
   release with the measurement in its entry.

## 5. How this is proved not to break anything

- **The dense path is bit-identical.** Before and after each step, stash the change and diff
  the `cargo xtask validate` output, as was done for the `gmin` rescue in v1.3.1.
  Every one of the 28 gates is below 500, so the output must match exactly, not just within
  tolerance.
- **The whole golden suite runs on the sparse path.** `xtask validate` gains a `--solver sparse`
  pass, and all 28 gates must pass at their existing tolerances. The results are not expected to
  be bit-identical (the pivot order is different), and the release entry says so. This turns the
  existing QSPICE goldens into the sparse path's validation suite at no extra cost, which a
  2 000 threshold would never have done.
- **Sparse and dense agree on the same matrix.** Unit tests on MNA-shaped systems: resistor
  ladders, voltage-source-heavy matrices, branch rows with a zero diagonal, and `laplace_*`
  state rows. Singular inputs assert that both reject.
- **The pattern-growth case is exercised.** A test model stamps a Jacobian entry only above a
  threshold voltage, and the test drives the Newton loop across it. The solve must succeed, the
  pattern must grow once, and the answer must match dense.
- **One large QSPICE golden.** An RC ladder of about 5 000 unknowns (`.op` and a short `.tran`),
  generated with `cargo xtask gen-golden` from real QSPICE, never hand-computed. It is the only
  gate that exercises the sparse path at a size dense cannot reasonably run. If QSPICE cannot
  produce it on this machine, the gap is recorded rather than filled with made-up data.
- **The usual gates.** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace` on all three
  CI platforms.

## 6. Risks and limitations to state

- **Models with large dense blocks gain less.** A compact model with many internal nodes, all
  coupled to each other, contributes a dense block. Sparse LU handles it correctly, just with a
  smaller speedup. This is expected, not a bug.
- **Pattern growth mid-run costs a symbolic refactorization.** If some model makes this frequent,
  Step 5's profile will show it. The fix would be to discover the pattern by stamping at more
  than one point, and that is not done unless measured.
- **Sparse results differ from dense in the last digits.** The pivot order is different. Every
  gate tolerance is far above this, but a user comparing a run at 499 unknowns with one at 501
  may notice, and the pre-flight line naming the solver is what explains it.
- **Not in scope:** iterative solvers, parallel factorization, and any FFI solver (KLU,
  SuiteSparse, PARDISO), which CLAUDE.md §5 forbids. The sparse LU is `faer`'s, pure Rust.

## 6a. Progress

**Step 1 — done, 1.5.0 (2026-09-23).** `crates/va-core/src/sparse.rs`: `Solver` and
`SPARSE_THRESHOLD = 500`, `Pattern`, `SparseSystem` (a `StampSink`, pattern found by an
overflow map on first assembly and grown to the union when a new entry appears), `assemble_into`,
and `SparseLu` (symbolic factorization cached per pattern identity, `faer` panics caught, the
dense path's residual check and `NonFinite` entry). No analysis calls it yet. Tests assert it
against the dense path: the same assembled matrix entry by entry, the same solution on a ladder,
a chain of voltage sources (every branch row a zero diagonal), and a diode Jacobian; a Newton
loop reaching `dc::operating_point`'s answer with one symbolic factorization; pattern growth
refactoring exactly once; singular and `NonFinite` agreement; `gmin`; the companion matrix.

`cargo xtask bench-linsolve` gained the Step 1 path. Two release runs on the ladder, one solve
each (so the small sizes are noisy):

| dim | dense solve (ms) | Step 1 solve (ms) | dense iteration (ms) | Step 1 iteration (ms) |
|---:|---:|---:|---:|---:|
| 11 | 0.007–0.011 | 0.003–0.004 | 0.008 | 0.004–0.006 |
| 101 | 0.50–2.4 | 0.012–0.014 | 0.41–0.44 | 0.024–0.027 |
| 501 | 5.7–7.2 | 0.056–0.067 | 6.6–10.1 | 0.12–0.14 |
| 2 001 | 110–113 | 0.27–0.28 | 126–128 | 0.61–0.66 |
| 10 001 | 8 067–8 852 | 2.6–2.8 | 8 171–10 062 | 5.1–5.4 |

"Iteration" is assembly plus solve, which is what a Newton step costs. **Read with care:** the
ladder is tridiagonal, the best case for sparse LU, and the benchmark times only the matrix
work. In a real run the model evaluation is often the larger cost (PSP103 is ~30 ms per DC
point), so the whole-run gain at small sizes will be far smaller than these ratios. That the
Step 1 path beats dense even at 11 unknowns here does not by itself argue for a lower
threshold; Step 5 measures whole runs on real circuits before the 500 moves.

**Step 2 — done, 1.6.0 (2026-09-23).** DC runs on the sparse path from 500 unknowns.
`NewtonConfig::solver` (default `Auto`); `newton` keeps one `SparseSystem` and one `SparseLu` for
the whole solve, every `gmin` stage included, and damping's trial points assemble into a scratch
sparse system. The dense branch is the same statements as before. `va-cli sim --solver
auto|dense|sparse`; `solve_*_with` variants in `va-cli` and `run_*_with` in `va-harness`, the
old names delegating with `Auto`; a third pre-flight line names the solver, why, and what it
covers (for `.tran`/`.ac`/`.noise` only the operating point, until Steps 3–4); `xtask validate
--solver sparse`, also run as a test so CI keeps it.

Evidence: default `xtask validate` output identical line for line to 1.4.1; with `--solver
sparse` all 28 gates pass and every printed error figure is the same as dense's. `bench-scale`
(two runs): a whole `.op` at 802 unknowns, now sparse under `Auto`, takes 2.0–2.3 ms, against
63.46 ms dense on 2026-09-17 and 9.0–9.8 ms for the dense 402-unknown row in the same runs.

## 7. Decided, and still open

- **Decided (2026-09-23, the user):** dense below 500 unknowns, sparse from 500; the dense path
  is not modified.
- **Open, for Step 4:** a complex sparse LU for AC/noise (recommended) or a sparse version of the
  real embedding. Need to check what are the advantages abd drwaback of each before choosing.
- **Open, for Step 5:** whether the measured crossover moves the threshold.
