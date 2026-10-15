# Plan: defects we share with OpenVAF-Reloaded

**Status: proposed, 2026-10-15 (2.32.1+1).** A plan, not a change: no code moves with this
document. **Batch A (items 3-7 and the diagnostics of section 3) landed in 2.33.0**; each item
below still describes the workaround, which is what to tell a user on an older version.

In October 2026 we read 36 OpenVAF-Reloaded pull requests, each fixing a defect in their compiler
or simulator, and asked of each one whether this simulator has the same defect. The defects that
were theirs alone are left out. The defects that were ours alone, found by the reproducers written
for the check, were fixed in 2.32.1. This document covers the rest: defects both projects have.

For each one it says how much it costs a user, whether the user can avoid it today by writing the
model or deck differently, and what fixing it would take. A defect with a clean workaround is
worth less to fix than one with none. And some items turn out to be semantic choices the LRM
leaves open, not defects at all.

Every behaviour quoted below was checked against 2.32.1 on the day of writing unless marked
otherwise.

---

## 1. Ranking

Impact puts a silent wrong answer above a loud failure, and a construct real models use above
one that only appears in a typo.

| # | Defect | OpenVAF PR | Failure | Who hits it | Workaround today | Fix effort |
|--:|---|---|---|---|---|---|
| 1 | `idt(x, ic)` ignores `ic` | — | loud (`E0602`) at the operating point; silent with `UIC` | VCO/PLL phase integrators, any integrator with a start value | partial: `UIC` + `ic + idt(x)` | medium |
| 2 | `transition` with a delay drops a pending change | #44, #46 | silent: a pulse shorter than the delay vanishes | delay cells, digital-ish behavioural models | partial: keep input changes `≥ td` apart | medium |
| 3 | an invalid event trigger runs its body every evaluation | #29, #34 | silent | typos, models ported from Verilog-AMS | yes: write only valid triggers | small |
| 4 | deck parameter values bypass `from`/`exclude` ranges | #60 | silent | any deck with a bad value | yes: an `$error` guard in the model | small |
| 5 | an out-of-range array index reads the last element | #23 | silent | buggy models, index computed from a signal | yes: guard the index | small |
| 6 | `absdelay` in AC: negative delay advances, `maxdelay` ignored | #47 | silent, AC only | delay lines whose delay is computed | yes: clamp in the model | small |
| 7 | extra arguments to `transition`/`slew`/`absdelay` accepted | #30 | silent but harmless (ignored) | typos | yes: don't write them | trivial |
| 8 | `cross` without `time_tol` fires up to a step late | — | accuracy, not correctness | event-timed models | yes: give `time_tol` | none (choice) |
| 9 | slope of an interrupted `transition` ramp | — | none: the LRM does not define it | — | n/a | none (choice) |
| 10 | `$rdist_*` returns its mean | — | documented limitation | Monte Carlo models | partial: sweep a parameter instead | large |
| 11 | `$discontinuity` is a no-op | — | accuracy only | models with hard switches | yes: `bound_step` or a `timer` | small |
| 12 | `<<<`, `>>>` not parsed | — | loud (parse error) | integer bit manipulation | yes: rewrite with `<<` / `floor` | small |
| 13 | `` `begin_keywords `` inside a module | — | loud (parse error) | rare | yes: move it before the module | trivial |
| 14 | `paramset` | — | loud | Verilog-AMS only | n/a: out of scope (CLAUDE.md §1) | none |

Items 8, 9 and 14 are not defects and are listed so nobody fixes them by mistake.

---

## 2. The items, one by one

### 2.1 `idt(x, ic)` ignores `ic` — rank 1

**What happens.** LRM 4.5.4: with an initial condition, `idt`'s output in a static analysis
*is* `ic`, and the integration starts from `ic`. Here the `ic` argument is parsed and dropped.
The integrator is an internal unknown whose static row says only "the input is zero", so with no
feedback loop around the integrator the operating point is singular (`E0602`), with or without
`ic`. With `.tran ... UIC` the run skips the operating point and integrates from 0, not `ic`.

**Workaround, partial.** Write the start value outside the operator and run with `UIC`:

```verilog
V(out) <+ 0.5 + idt(1e6 * V(inp));   // instead of idt(1e6 * V(inp), 0.5)
```

```
.tran 0.1u 1u UIC
```

Checked: this gives `0.5 + ∫` from the first step. The printed `t = 0` point is the zero vector
`UIC` starts from, not 0.5. There is **no** workaround that keeps the operating point: guarding
the operator with `if (analysis("static"))` still leaves its unknown in the static system, and the
solve stays singular. A model that needs an operating point with an open integrator cannot run
today.

**Fix.** In a static analysis, when `ic` is present, stamp the integrator's row as
`y = ic` instead of `x = 0`; at the transient's first point, seed the state from the solved `y`.
Without `ic`, keep today's row (the LRM's "the input is driven to zero by feedback"). That
touches `va-codegen` (the `idt` lowering and its state) only. Gate: an open integrator with `ic`
solves its operating point to `ic` and ramps from it; one with feedback is unchanged.

### 2.2 `transition` with a delay drops a pending change — rank 2

**What happens.** `transition(x, td, tr, tf)` holds one scheduled change. A second change of `x`
that arrives before the first has started replaces it. A 0.5 ns pulse through `td = 2 ns`
produces nothing at all (checked); a 3 ns pulse is delayed correctly. The LRM's `transition` is a
transport delay: every change comes out, `td` later.

**Workaround, partial.** The output is right whenever consecutive input changes are at least
`td` apart, so a model whose input never pulses faster than its delay is unaffected. Otherwise
there is no in-model fix: `absdelay` would be the natural replacement
(`absdelay(V(mid), td)` after a zero-delay `transition`), but `absdelay` is refused in transient
(`docs/proposals/absdelay.md`, stage 2). When the input comes from a deck source, the delay can
be moved there instead (a `PULSE`'s own delay).

**Fix.** Give each `transition` site a bounded queue of pending (time, value) changes in its state
slots instead of one, and pop them in order. The state length grows by two slots per queue entry;
when the queue is full, the right answer is a refusal naming the site, not silently dropping the
oldest change. `va-codegen` only. Gate: the 0.5 ns pulse above comes out whole, 2 ns later.

### 2.3 An invalid event trigger runs its body every evaluation — rank 3

**What happens.** `@(foo) k = 1;` with `foo` undeclared, a variable, or anything else that is not
an analog event, elaborates and runs the body at every evaluation (checked). Verilog-A has no
named events (Annex C excludes them), so this is invalid input accepted silently.

**Workaround.** Write only `cross`, `above`, `timer`, `initial_step`, `final_step` and their `or`
lists. The defect only bites on a typo or a construct ported from Verilog-AMS, but the user cannot
tell it happened, which is why it ranks above the cheaper items.

**Fix.** An elaboration error, coded, naming the trigger. `va-frontend` only. Small.

### 2.4 Deck values bypass parameter ranges — rank 4

**What happens.** `parameter real r = 1k from (0:inf);` with `X1 a 0 rr r=-5` in the deck
solves happily with `r = -5` (checked).

**Workaround** (checked: the run ends with `E0739`):

```verilog
@(initial_step) if (!(r > 0)) $error("r = %g is outside (0:inf)", r);
```

**Fix.** Check the range wherever a value is bound to an instance (an instance-line parameter,
a `.param` it reads), with a coded error naming the parameter, the value and the range. Small; the
range is already in the IR.

### 2.5 An out-of-range array index reads the last element — rank 5

**What happens.** With `real g[0:3]` and `i = 7`, `g[i]` reads `g[3]` (checked). A constant
out-of-range index is already an elaboration error; a computed one is clamped silently.

**Workaround.** Guard the index in the model: `if (i < 0 || i > 3) $error(...)`, or clamp it
explicitly so the clamping is visible in the source.

**Fix.** A run-time diagnostic the first time it happens, coded, naming the array and the index.
The read itself has to return something to keep the solve going; `0.0` is what Verilog's real
arrays give. Small to medium, depending on how the diagnostic reaches `va-cli` from inside a load.

### 2.6 `absdelay` in AC: negative delay, `maxdelay` — rank 6

**What happens.** A negative `delay` becomes a phase advance, and `maxdelay` is not applied.
Transient is unaffected, because transient refuses `absdelay` altogether.

**Workaround.** `absdelay(x, min(max(td, 0), tmax))` in the model.

**Fix.** Clamp in the AC stamp, and warn once when the clamp binds. Small.

### 2.7 Extra arguments accepted — rank 7

`transition(x, td, tr, tf, tol, extra)` elaborates and ignores `extra`. Harmless, since nothing
is computed from it, but a typo goes unreported. **Fix:** an arity check at elaboration, coded.
Trivial.

### 2.8 `cross` without `time_tol` — a choice, not a defect

Without `time_tol`, the crossing is located only to within the step the integrator took over it.
The LRM leaves the default tolerance to the simulator. **Users who need the crossing time should
give `time_tol`** (and, where the step is coarse, a `bound_step`). Nothing to fix; worth one
sentence in the `cross` row of `docs/token-reference.md`.

### 2.9 Interrupted `transition` ramps — a choice, not a defect

When a new target arrives mid-ramp, the LRM does not say what slope the new ramp has, so any
consistent choice is conforming, OpenVAF's and ours alike. Document ours; change nothing.

### 2.10 `$rdist_*` returns its mean — documented limitation

Already stated in `docs/token-reference.md`. A Monte Carlo model gets its nominal corner. Until
real random streams exist (seeded, reproducible per instance, which is the large part), a deck
can sweep the parameter instead. Large; not urgent.

### 2.11 `$discontinuity` is a no-op

The step controller finds most kinks on its own. Where it does not, `bound_step` near the switch,
or an `@(timer(t_switch))` that forces a timepoint there, does what `$discontinuity` asks for.
Small fix (turn it into a breakpoint request at the current time), low value.

### 2.12–2.14 Loud parse failures

- `<<<` is `<<`; `x >>> n` on a signed integer is `floor(x / 2**n)`. Rewriting is exact.
  Implementing them is small: they are two more lexer tokens and operators.
- `` `begin_keywords `` inside a module: move it before `module`. Trivial to accept.
- `paramset` is Verilog-AMS, outside the language this project implements (CLAUDE.md §1). The
  diagnostic should say that instead of `E0202 expected '='`; see §3.

---

## 3. Our own limitations the same reproducers surfaced

Not shared defects, but they turned up on the same day and belong in the same queue.

- **Integer arithmetic is real arithmetic.** `integer i = 7; i / 2` is 3.5, not 3, and an
  assignment to an `integer` does not round. This is stated in `docs/token-reference.md`
  ("v0 performs no integer-vs-real type checking or truncation"). **Workaround:** write the
  conversion: `int(i / 2)` truncates toward zero exactly as integer division does (`-7/2 → -3`),
  and `integer(x)` rounds as an integer assignment does. `%` is already right. **Fix:** integer
  typing in elaboration and an integer-division operator. That changes `va-ir`'s `BinOp`, a
  frozen interface, so it goes through CLAUDE.md §6. Large.
- **Wrong-construct diagnostics.** A `string` variable reports a discipline error (`E0217`),
  `break`/`continue` report `expected '='` (`E0202`), whole-array assignment reports `E0330`,
  `paramset` reports `E0202`. Each fails loudly, so nothing is computed wrongly, but each message
  sends the user to the wrong line of thought. **Fix:** one coded diagnostic per construct, saying
  what it is and whether it is refused or simply not yet implemented. Small each.

---

## 4. Proposed order

1. **Batch A: the cheap silent ones — done in 2.33.0.** Items 3, 4, 5, 6 and 7, plus the diagnostic wording in §3.
   Each is small, local to one crate, and turns a silent wrong answer into a coded message.
   One release.
2. **Batch B: `idt(x, ic)`** (item 1) — **done in 2.34.0**, with the `W0401` warning. The highest-impact defect and the only one that blocks
   ordinary models outright.
3. **Batch C: the `transition` queue** (item 2). Silent, and the queue needs a state-layout
   decision (fixed depth and refusal when full).
4. **Later:** integer typing (§3, an interface change), `$rdist_*`, `<<<`/`>>>`,
   `$discontinuity`.
5. **Documentation only:** items 8 and 9.

Until a batch lands, the workarounds in §2 are what to tell a user, and the `refused`/error
messages for items 1, 3 and 4 should point at them.
