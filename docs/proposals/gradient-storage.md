# Proposal: stop paying the system heap for every gradient

**Status:** proposed, 2026-09-24; re-profiled 2026-10-01; option D tried and set aside (§0.1); a statically scheduled tape implemented in 2.4.0 (§0.2).
**Affects:** option A — the three binaries (`va-cli`, `xtask`, `va-harness`'s test runner), not the
library crates; option B — `va-codegen` (`ad.rs`, `tape.rs`, `lib.rs`). **`va-ir` (Interface α)
and `va-abi` (Interface β) are not touched** by either. Option A needs `unsafe` and therefore
CLAUDE.md §5's written justification and owner sign-off; option B is T2's crate.
**Follows:** `docs/proposals/evaluator-tree-walk.md` (its Stage 3 result and its correction of §1);
`release.txt` 1.14.0+1 to 1.16.0.

This is a decision document. No Rust code changes are part of this proposal.

---

## 0. Update, 2026-10-01: re-profiled on a real process card, and option D moves up

The transistor-level SNN (`circuits/snn/transistor/`) runs PSP103 with IHP's SG13G2 thick-oxide card
(`models/sg13g2/`), and a `load()` there costs **~200–240 µs**, not the ~100–110 µs the `.va`
defaults give. Measured with the new `cargo xtask bench-instance` (a deck's instances at its own
operating point; `circuits/snn/transistor/bench_access_bias.net`), i7-1185G7, release, 3 rounds:

| what changes | load (µs) | gradient allocs / clones per load |
|---|---:|---:|
| SG13G2 card, transient context | 195–237 | 870 / 1 669 |
| SG13G2 card, DC context | 189–205 | 870 / 1 669 |
| `.va` defaults, same bias, transient | 106–111 | 407 / 857 |
| card with `swjuncap=0` (JUNCAP off) | 107 | 461 / 968 |

Bias and analysis context make no difference. **The card's full JUNCAP (`swjuncap=3`) doubles the
work**: it is real physics (junction charge and leakage on both junctions), so the remedy is a
cheaper evaluator, not a different card. No other card switch measurably changed the cost (`swigate`,
`swimpact`, `swgeo`, the gate, bulk and well resistances, `qmc`; this laptop's ±20 % noise makes
that a statement about large effects only).

**Where the time goes** (samply, 10 695 samples, release build as shipped, inclusive unless noted):

| activity | share |
|---|---:|
| heap: `Arc<[f64]>` gradient allocate/free (`from_iter_exact`, ntdll heap, `drop_slow`) | ~25–30 % |
| tape interpreter, `eval_root` **self** (with `Dual` construction inlined) | 26 % |
| `Dual` arithmetic (`mul`/`add`/`sub`/`div`, excluding their allocations) | ~20 % |
| variable table (`get_var`/`set_var`: cloning `Option<Dual>`, atomic refcounts) | ~11 % |
| maths library (`exp`, `log`, `pow`) | **~2 %** |

6 186 expression nodes are visited per `load()` (≈ 37 ns each), and **a dense gradient carries
2.9 non-zero entries out of 21 on average** (530 000 gradients counted at `zip_owned`'s output,
scratch build). So about 86 % of the gradient storage, copying and arithmetic is spent on zeros.

**What that changes in §2.** Option D was deferred because its gain looked limited to "some of the
10 % arithmetic". At 2.9 of 21 it is the option that removes the most:
- an **inline sparse gradient** (a few `(slot, value)` pairs held in the `Dual`, spilling to the heap
  only past a small fixed count) removes most allocations, as A does but without `unsafe`;
- it removes the atomic refcount on every variable read, which A does not touch;
- it cuts each derivative loop from 21 entries to about 3.

It also keeps bit-identity in reach: each slot's arithmetic is unchanged, and only slots that are
zero in both operands are skipped. The one exception to check is a rule whose `f(0, 0)` is `-0.0`.
**Proposed order now: D first** (in `va-codegen`, safe Rust, checked by `tape-check`, `deck-diff`
and the finite-difference tests), then re-profile; A only if allocation still shows; native code
generation (OpenVAF-class, 20–40×) as the separate, larger step after that. With the device-bypass
prototype (`docs/proposals/device-bypass.md`) already removing the idle loads, D speeds up the loads
that remain. An estimate, not a measurement: 2–3× on PSP103 with this card.

### 0.1 Option D tried: the estimate was wrong (branch `sparse-gradients`, not merged)

Sparse inline gradients were bit-identical and cut allocations from 870 to 25 per `load()`, but the
`load()` itself was no faster on one thread. The first version was 15–20 % slower: the `Dual`
grew, and the tape copies one per node. In circuit, 8 threads ran 12–17 % faster (no shared
allocator) and 1 thread 5–8 % slower. The cost was never the storage. It is the interpreter's
**per-node overhead**: a `load()` runs ~6 100 tape instructions over ~1 225 assignments, and 57 % of
them only push a variable (2 551) or a constant (939). Only 788 of the nodes are hoistable.

### 0.2 What was done instead: a statically scheduled tape (2.4.0, `va-codegen/src/sched.rs`)

Which unknowns an expression can depend on is fixed by the model's structure. So, once per model:

- **Support:** compute each expression's support: a probe's slots, the union of an operator's
  operands, and for a variable the union over all of its assignments (a fixpoint).
- **Registers:** give every variable, intermediate and probe a register (one value, one partial per
  support slot).
- **Compile:** turn each statement's expression into instructions that read operands in place and
  write in place. An assignment's last operation writes the variable's register directly; a
  variable read or a constant is no instruction at all.

A contribution's terms become `Dual`s only at the end, for the unchanged stamping code. Everything
around the statement walk (accumulators, state, `bound_step`) is untouched.

**Fallback.** A model the compiler cannot take whole runs on the tree walk exactly as before.
Refused:
- persistent variables;
- `idt`/`laplace_*`/`zi_*` state;
- a `ddt` inside an expression;
- a non-maths call whose arguments read a variable;
- a variable that may be read before it is assigned (a definite-assignment check).

**Correctness: bit-identical**, checked three ways:
- **Model corpus:** `cargo xtask sched-check` compares every stamp and state proposal of the two
  paths at 42 bias points in DC and in transient, for every model. Over 167 model files (the
  benchmarks, `models/`, `external/`, `benchmark/models/`) there were **0 differing finite
  evaluations**. 66 models run on the static path; the rest fall back, each with its reason
  reported. Some random bias points make a model non-finite on both paths. There the tree walk
  spreads the NaN into partials the value cannot depend on and the static path leaves them `0.0`
  (the one designed difference); the solver rejects such a point on either path.
- **Decks:** `cargo xtask deck-diff`, with one binary and `VA_SCHED=off` against on, gave 85
  identical and 0 different (74 decks run, 11 fail the same way as on `main`).
- **Tests:** 924 workspace tests pass with the static path on by default.

The 13 slow ISCAS'85 decks, overnight, one at a time: all byte-identical, every run exiting 0 (release 2.4.0+1).

**Speed** (i7-1185G7, `fig5_access`, 8 SG13G2 PSP103):

| | 8 threads | 1 thread |
|---|---:|---:|
| tree walk | 60.8–61.5 s | 115.9 s |
| **static schedule** | **31.9–33.2 s** | **41.0 s** |
| tree walk + bypass (branch `proto/device-bypass`) | 22.4 s | 18.4 s |
| **static schedule + bypass** | **14.9 s** | **9.3 s** |

One PSP103 `load()` costs 76 µs instead of ~245 µs (`bench-instance`). With bypass, one thread beats
eight: the parallel-evaluation threshold (`va_core::par`, a cost estimate per instance) does not yet
know that most instances are replayed.

**Limitations, stated.**
- Each instance keeps a register file (~one `f64` per register and per support entry), behind an
  uncontended `Mutex`.
- A tree-walked call inside a compiled expression gets the full support.
- The definite-assignment check is conservative: an untaken `?:` arm, or a loop body that reads a
  variable before assigning it, sends the model to the tree walk.
- `VA_SCHED=off` turns the path off (for A/B measurement and `sched-check`).

---

## 1. What is wrong today

A compiled model's `load()` builds a dual number — a value plus a gradient over the model's local
unknowns — at every expression node it evaluates, and every gradient that depends on an unknown
lives in its own heap allocation (`Grad::Dense(Rc<[f64]>)`). 1.14.0+1 and +2 cut those from 1 845
to 443 per PSP103 `load()` (sharing on copy, results written in place); 432 remain.

**Where the time goes, measured.** A sampling profile of PSP103's `load()` with the dual-number
operations kept out of line (`#[inline(never)]`, a scratch build, so each shows under its own name;
108.8 µs per `load()` in that build), self time by activity:

| activity | µs | share |
|---|---:|---:|
| **heap allocate / free** — gradient buffers | **24.1** | **22%** |
| tape loop — values moved on and off the stack | 31.6 | 29% |
| **gradient arithmetic** — the derivative loops | 10.9 | 10% |
| building and copying dual numbers | 10.4 | 10% |
| variable table, reads and writes | 9.8 | 9% |
| operator wrappers | 8.6 | 8% |
| statement walk | 7.5 | 7% |
| dropping values | 2.7 | 2% |
| **maths library** — `exp`, `pow`, … | **1.2** | 1% |

The arithmetic a model actually asks for — its maths functions and their derivatives — is ~12 µs,
about 11%. Keeping small functions out of line inflates the copying and wrapper rows (every call now
moves its 40-byte dual numbers through memory, which the real build partly optimises away); the
heap row is library code and is not inflated.

**How much of it is removable — an upper bound, measured.** A scratch build of `xtask bench-model`
with a size-class caching allocator in front of the Windows heap (freed blocks up to 512 bytes kept
on per-size lists and reused, so almost no allocation reaches the system heap), nine interleaved runs
against 1.16.0 on mains power:

| model | 1.16.0 median | caching allocator | median | best |
|---|---:|---:|---:|---:|
| PSP103 | 119.4 µs | 96.7 µs | −19.0% | −16.1% |
| BSIM4 | 124.5 µs | 106.6 µs | −14.4% | −12.9% |
| BSIM-BULK107 | 195.9 µs | 170.7 µs | −12.9% | −15.1% |

**Heap allocation is 13–19% of a real `load()`** — several times what the flat tape bought, and the
largest single cost left that is not the model's own arithmetic. It is a bound: it counts every
allocation during `load()`, not only gradients (the gradients are ~all of them), and a benchmark
repeating one instance keeps the allocator's lists warm.

## 2. Options

| | What | Removes | Cost and risk |
|---|---|---|---|
| **A** | **A caching allocator for the binaries**: `#[global_allocator]` in `va-cli`, `xtask` and the harness, keeping freed small blocks on per-size lists for reuse — the scratch allocator above, made production-grade, or a vetted pure-Rust allocator crate (none evaluated yet; CLAUDE.md §5 forbids anything that links native code, which rules out the usual C allocators). | The measured 13–19%, with no change to the evaluator. | `unsafe` (implementing `GlobalAlloc`) — §5 sign-off, and a small module that must be right, since a bug corrupts memory process-wide. Freed small blocks are kept, not returned to the OS: memory stays at its peak. Library users (the crates embedded elsewhere) do not get it — they bring their own allocator. |
| **B** | **A per-evaluation gradient arena in `va-codegen`**: gradients live in one buffer owned by the evaluation context, bump-allocated and released all at once when `load()` ends; a dual number becomes a value plus a slot index. | The allocation and free cost (≥ A's), and most of the copying — a dual number shrinks from 40 bytes to ~16. | A rewrite of the AD core: every operator's gradient handling, `Dual` gains a lifetime tied to its context (or an index valid only inside it), the tape, the variable table. Safe Rust. Bit-identity is preserved — the same arithmetic on the same values, stored elsewhere — and checked by `tape-check`'s bit-for-bit comparison and the finite-difference tests. Setup values live beyond a `load()` but are bias-independent, so their gradients are all `Zero` and need no arena storage. |
| **C** | **Recycled gradient buffers inside `va-codegen`**: a per-model free list of fixed-length buffers, with a hand-written reference count in place of `Rc`. | Most of the allocation cost. | Neither A's simplicity nor B's safety by construction: a refcounted handle that must return its buffer to the right pool on drop. |
| **D** | **Sparse gradients** — a both-dense gradient averages 21 slots, 3.6 of them non-zero, on PSP103. | Some of the 10% arithmetic and of the buffer size. | Every derivative rule changes; a separate proposal if the profile after A or B points there. |

## 3. Proposal

1. **Decide on A first** — it is the measured win with no change to the evaluator. If §5's sign-off
   for an in-tree `GlobalAlloc` is given, write it (with its `unsafe` justification, tests of its
   own, and the scratch allocator's design: size classes to 512 bytes, 16-byte alignment, larger
   requests passed to the system), install it in the three binaries, and measure: `bench-model`
   interleaved, peak RSS on the PSP103 chains and c432 (it keeps memory), and in-circuit time.
   If the sign-off is not given, say so here and go to B.
2. **Re-profile** on the result. If allocation is gone and the copying rows dominate, **B** is the
   next step, with its own design review before code (it is the largest change to `va-codegen`
   since the AD itself). If not, stop: the model's arithmetic is then most of what is left.

## 4. What each stage must show

As for the tree-walk proposal (§4 there), plus:

- `cargo xtask tape-check` — every stamp bit-identical with and without tapes (it exercises the
  allocator and, for B, both storage paths).
- For A: the allocator's own tests (every size class, alignment, reuse after free, realloc across
  classes), peak memory against 1.16.0 on the largest decks, and the whole suite under it.
- For B: bit-identical against 1.16.0 on every deck (`deck-diff`), the finite-difference Jacobian
  tests, and `tape-check`.

## 5. Decisions needed

1. §5 sign-off for an in-tree `unsafe` `GlobalAlloc` in the binaries (option A) — or not.
2. Whether B should be designed now regardless, since only it also removes the copying.
