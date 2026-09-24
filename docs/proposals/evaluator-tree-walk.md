# Proposal: cut the evaluator's tree-walk overhead

**Status:** proposed, 2026-09-24. **Stage 1 done (1.15.0, 2026-09-24)** — results and the
decision it leaves open are at the end of Stage 1 below.
**Affects:** `va-codegen` only — `lower.rs` (a new analysis and a new lowered form), `ad.rs` and
`lib.rs` (the evaluation loop), `xtask` (`bench-model` reporting). **`va-ir` (Interface α) and
`va-abi` (Interface β) are not touched**: everything proposed here is internal to how
`va-codegen` evaluates the IR it already receives, so no §6 coordination is triggered. It is,
however, T2's crate (CLAUDE.md §3), so the T2 owner should agree before Stage 2.
**Follows:** the 2026-09-24 profiling of PSP103's `load()` (`release.txt` 1.14.0+1 to +3,
`docs/validation.md` § "Reading a `--logfull` trace"), and the open item in `docs/roadmap.md`'s
T2 section: "a flat SSA tape would remove the recursive-`eval` dispatch and `Result` plumbing
that remain, and a Cranelift JIT … would go further".

This is a decision document. No Rust code changes are part of this proposal.

---

## 1. What is wrong today

A compiled Verilog-A model is evaluated by walking its IR: `GeneratedModel::walk` steps through
the lowered statements, and `ad::eval` recurses through each expression tree, building a `Dual`
(value plus gradient) at every node. After 1.14.0+3 the walk itself is the largest cost of an
evaluation.

**Where one PSP103 `load()` goes** — a sampling profile (`samply`, attached to `xtask
bench-model`), self time converted to µs with the binary's measured 96.4 µs per `load()`:

| | µs per `load()` | share |
|---|---:|---:|
| **tree walk** — `eval`, `walk`, `eval_call`, arena lookups | **45.8** | **48%** |
| heap allocation and free | 22.2 | 23% |
| local variables — `get_var`, `set_var` | 11.1 | 12% |
| dual-number arithmetic | 10.1 | 10% |
| dropping values | 3.4 | 4% |
| maths library, stamping, other | 3.8 | 4% |

The heap share came down from ~101 µs in two steps (1.14.0+1, +2); the walk did not move
(46.6 → 45.8 µs across those changes) and is now half of every evaluation.

**What the walk visits** — nodes `eval` enters per `load()`, counted over 100 calls after
warm-up (scratch instrumentation, 1.14.0+3). "No gradient" means the node's result carries a
structurally-zero gradient in both channels: it depends on no unknown *at that point*.

| model | exprs in IR | statements (setup) | visited per `load()` | of which no gradient | biggest kinds visited |
|---|---:|---:|---:|---:|---|
| PSP103 | 40 807 | 1 220 (757) | 3 441 | 1 629 (47%) | variable reads 1 550, binary ops 1 229, constants 492 |
| BSIM4 | 19 109 | 1 949 (1 556) | 3 390 | 2 010 (59%) | variable reads 1 437, binary ops 1 232, constants 562 |
| BSIM-BULK | 23 134 | 1 010 (306) | 7 106 | 4 887 (69%) | binary ops 2 579, variable reads 2 158, constants 1 217 |

Two readings:

1. **The per-node cost is the overhead, not the arithmetic.** ~46 µs over 3 441 nodes is ~13 ns
   a node, spent on the `match` over the node kind, the arena lookup (`Module::expr`), the
   recursion, and wrapping every result in `Result<Dual, CodegenError>` — for nodes most of
   which are a constant or a variable read. The existing setup split (`Lowered::static_prefix`)
   already removed ~80% of the nodes a `load()` used to visit (2026-09-22); what remains is
   visited through the same machinery.
2. **Up to half of what remains may still not depend on the unknowns.** The setup split takes
   only a *leading run* of bias-independent statements and stops at the first dependent one
   (`lower::static_prefix_len` explains why: no dependence analysis). Everything bias-independent
   after that point is recomputed on every Newton iteration of every timepoint. 47–69% of
   visited nodes carrying no gradient is an **upper bound** on that — a node with no gradient
   can still depend on the unknowns through control flow (a comparison of a voltage is
   gradient-free but changes with bias) — and a hoisted subtree still leaves one read in its
   place. The real figure needs the analysis Stage 1 builds.

What has been ruled out, measured: allocating the variable table per call (a pooled table was
bit-identical and saved nothing — `perf-logs`, 2026-09-24, reverted), and the counted overheads
of context maps, probe reads and stamp lookups (~2% together, 1.14.0).

## 2. Options

| | What | Removes | Risk | Effort |
|---|---|---|---|---|
| **A** | **Hoist bias-independent subexpressions** found by dependence analysis, beyond the leading prefix: evaluate each once, at setup, and read the stored value. | Nodes whose value cannot change between calls. | Freezing a value that *can* change — silent wrong answer. Same failure mode as the prefix, same defence (whitelist + exhaustive match). | Moderate: an analysis pass in `lower`, a hoisted-slot read in `eval`. |
| **B** | **Flat tape**: lower each statement's expression tree to a linear list of instructions (post-order, SSA registers) and evaluate it in a loop. | Recursion, per-node arena lookup and `match` depth, per-node `Result` propagation (validation already guarantees a validated model cannot fail at `load`). | Low for correctness if the tape preserves the evaluation order (then bit-identical); moderate code size. | Larger: a second evaluator beside the tree walk, kept until it is proven identical. |
| **C** | **Compile to machine code** (Cranelift, pure Rust). | Nearly all interpretive overhead. | `unsafe` to call emitted code (CLAUDE.md §5 sign-off); the `Dual` arithmetic and its gradient vectors must be emitted too, which is most of the work; a much larger surface to validate. | Large. |
| **D** | **Change the AD representation** — sparse gradients (a both-dense gradient averages 21 slots, 3.6 non-zero, on PSP103), or reverse mode per contribution. | Arithmetic and allocation inside `Dual`, not the walk. | High: touches every operator's derivative rule. | Large; a separate proposal. |

**A and B are complementary.** A shrinks the number of nodes; B shrinks the cost of each one that
remains. C subsumes B but not A (a compiler would still recompute what A hoists, unless it also
does A). D is about a different row of the table in §1 and is out of scope here.

## 3. Proposal

Do **A, then B**, each as its own measured release; decide about C only if the walk still
dominates afterwards.

### Stage 1 — measure what A would hoist (no behaviour change)

Write the dependence analysis in `lower` as a pure function over the lowered statements that
**classifies** every expression node — bias-independent or not — and changes nothing yet. Report
from `xtask bench-model`, per model: nodes visited per `load()`, and how many of them sit inside
a maximal bias-independent subtree that is *not* in the prefix (the nodes A would remove, net of
the one read left in each subtree's place). Run it over the corpus (`external/code`, and the models the 28
validation gates use).

The rule extends the prefix's whitelist from statements to expressions, with reaching
definitions for variables: a variable read is bias-independent only if **every** assignment
that can reach it is bias-independent and none sits under bias-dependent control flow (an `if`
on a voltage makes whatever it assigns bias-dependent, whatever the right-hand side). Loops,
`case`, user functions and analog operators are classified conservatively — dependent — unless
the analysis can show otherwise. Exhaustive `match` with no wildcard, exactly as
`static_prefix_len`, so a new `Expr` variant or `Builtin` cannot be inherited into the safe set.

**Decision point:** if the net removable nodes are small (say under 15% of the walk), skip A and
go to B. The count answers this before any evaluation changes.

**Stage 1 result (1.15.0).** Built as `lower::invariance` (stored in `Lowered::invariance`,
unused by evaluation) and counted per visited node behind a `walk-stats` feature. `cargo run
--release -p xtask --features walk-stats -- bench-model`, the benchmark's seven CMC models:

| model | visited per `load()` | net hoistable |
|---|---:|---:|
| PSP103 | 3 441 | **11.0%** |
| BSIM-SOI | 4 530 | 11.1% |
| BSIM4 | 3 390 | 12.5% |
| EKV2.6 | 1 806 | 14.3% |
| JUNCAP200 | 1 238 | 16.5% |
| HICUM/L2v3 | 1 974 | 21.5% |
| BSIM-BULK107 | 6 168 | 21.7% |

Far below §1's 47–69% of nodes carrying no gradient: most of those are leaves — constants and
variable reads — feeding bias-dependent operations, which a hoist would leave in place as reads,
or they sit under bias-dependent control. With the walk at ~48% of a PSP103 `load()`, hoisting
11% of its nodes is worth at most ~5% of the evaluation, and less if hoisted nodes are cheaper
than average. The three largest models sit under the 15% line, the other four above it.

**Recommendation:** skip A for now and do B, which lowers the cost of *every* visited node, and
revisit A on top of the tape, where a hoisted subtree is a contiguous run of instructions and
cheaper to move. Not decided — the proposal's owner and T2 to confirm.

Two findings from building it, both kept:

- The per-node counter is **not on by default**. A check on every `ad::eval` call, even a
  disabled one, measured +6.6%, +7.0% and +0.5% on PSP103's median in three interleaved sets —
  this laptop cannot settle a few percent, and the evaluator's hottest path is not the place to
  leave an unproven cost. The analysis itself runs once per model build and stays on.
- **Deck comparison is now `cargo xtask deck-diff <old va-cli> <new va-cli>`** (§4), replacing
  the scratch script used for 1.14.0–1.14.0+3.

### Stage 2 — A: hoist (bit-identical)

Evaluate each hoisted subtree once in the setup phase (`ensure_setup`) into a slot, and give
`eval` a way to read the slot where the subtree root was. The operations are the same, on the
same values, in the same order — only earlier — so every result is **bit-identical**, and the
check is exact: all 70 decks, `xtask validate`, the finite-difference Jacobian tests.

Plus a **differential test** as the real defence against a misclassification: every corpus model
evaluated at several bias points (including sign changes of every terminal voltage, so
bias-dependent branches flip) by the hoisting evaluator and by the plain one, values and every
partial compared bit for bit. A frozen value that should have moved shows up as a mismatch at
the point where the branch flips.

### Stage 3 — B: flat tape (bit-identical)

Lower each statement's expression to a post-order instruction list over numbered registers
(`Dual`s), with operands referring to earlier registers, hoisted slots, parameters, variables
and probes; evaluate in a loop. Statement-level control flow (`if`, `case`, loops, user
functions) stays in `walk`, driving tapes rather than trees, so the change is confined to
expressions. Validation has run once, at build, so tape instructions do not return `Result`;
the few operations that can still fail at run time (a loop's iteration cap) keep their check.

Keep the tree walker as the reference: the Stage 2 differential test runs tape against tree. When
it has been bit-identical across the corpus for a release, the tree walker can become test-only.

### Stage 4 — decide about C

Re-profile. If the walk is still the largest cost after A and B, write the Cranelift proposal
then, with its `unsafe` justification for §5 sign-off — against measured numbers rather than
today's.

## 4. What each stage must show

Per stage, on the same machine, **on mains power** (a 2026-09-24 timing on battery ran 3.5×
slow for both binaries alike):

- `cargo test --workspace` green, including the finite-difference Jacobian tests; fmt, clippy.
- `cargo xtask validate` 28/28, identical line for line to the previous release.
- All decks under `circuits/` give output identical to the previous release — every deck run
  through both binaries, stdout, stderr (timing lines aside) and exit code compared. This was a
  scratch script for 1.14.0–1.14.0+3; Stage 1 should make it an `xtask` subcommand, since every
  later stage leans on it.
- The corpus figure (`va-cli check external --codegen`) unchanged — a stage that makes a model
  stop building is a regression even if everything else is faster.
- `xtask bench-model` on PSP103, BSIM4 and BSIM-BULK, at least seven runs of each binary
  interleaved, medians and minima, with the heap allocations per `load()` from a counting
  allocator.
- In-circuit: the 160-stage PSP103 chain `.op`, alternating runs, assembly per Newton iteration
  from `--logfull`.
- A fresh profile, so the next stage starts from where the time actually went, not from this
  document's table.

A stage that is bit-identical but not measurably faster is reverted, as the pooled variable
table was: the code it adds has to be paid for by a measurement.

## 5. Out of scope

- Option D (the AD representation) — a separate proposal if the profile after Stage 3 points at
  the `Dual` arithmetic.
- The transient integrator's own Newton loop and the linear solve — not evaluator costs.
- Temperature: the prefix's cache is valid because `$temperature` reads the build temperature
  (see `lower::static_prefix_len`'s caveat). Hoisting inherits that exact assumption and the
  exact same warning: if `$temperature` is ever re-sourced from the analysis context, hoisted
  values must be keyed on it.

## 6. Decisions needed

1. Approve Stage 1 (analysis and counts only; no evaluation change).
2. Agreement from the T2 owner before Stage 2, since `va-codegen` is their crate.
3. Whether the Stage 1 decision threshold (net removable nodes under ~15% → skip A) is right.
