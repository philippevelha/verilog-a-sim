# Proposal: stop paying the system heap for every gradient

**Status:** proposed, 2026-09-24. Nothing decided.
**Affects:** option A — the three binaries (`va-cli`, `xtask`, `va-harness`'s test runner), not the
library crates; option B — `va-codegen` (`ad.rs`, `tape.rs`, `lib.rs`). **`va-ir` (Interface α)
and `va-abi` (Interface β) are not touched** by either. Option A needs `unsafe` and therefore
CLAUDE.md §5's written justification and owner sign-off; option B is T2's crate.
**Follows:** `docs/proposals/evaluator-tree-walk.md` (its Stage 3 result and its correction of §1);
`release.txt` 1.14.0+1 to 1.16.0.

This is a decision document. No Rust code changes are part of this proposal.

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
