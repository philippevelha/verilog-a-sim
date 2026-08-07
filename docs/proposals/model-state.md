# Proposal: a per-instance state channel on Interface β (Tier B)

**Status:** **delivered 2026-08-07**, with the §8 non-goals intact. **Date:** 2026-08-06.
**Affects:** `va-abi` (Interface β), `va-ir` (Interface α), `va-frontend`, `va-codegen`,
`va-core`, `va-transient`, `va-acnoise`, `va-cli`.
**CLAUDE.md §6 step 1** — the written description the rule requires before any code.
**Follows:** `analysis-context.md`, whose Tier A shipped 2026-08-06. This is that document's
Tier B, which it deliberately deferred as "a genuinely harder design … must not be smuggled
into it."

> **Delivered 2026-08-07.** The state channel, `transition`, `slew` and `@(initial_step)` are
> implemented; §8's non-goals (`$limit`, `absdelay`, `idt` ICs, exact `transition` breakpoints)
> all remain non-goals. All 14 golden gates unchanged to the last digit; 547 tests pass.
>
> **One thing shipped differently from §4's recommendation.** `@(initial_step)` needed no
> `Builtin`-wrapped-by-the-frontend dance in the end: the *parser* desugars
> `@(initial_step) stmt` straight into `if (initial_step()) stmt`, and elaboration lowers that
> synthetic call to `Builtin::InitialStep`. Same outcome, one less moving part, and every
> existing control-flow walk selects the arm for free.
>
> **§6's validation question was asked and answered "yes, but not worth it".** QSPICE's
> behavioral sources do compute `min`/`limit`/`sdt` exactly, so a closed-form slew envelope
> *could* be golden-gated — but that compares our recurrence against the analytic answer rather
> than against an independent slew implementation, and the same property is asserted more
> directly by an end-to-end unit test. `docs/validation.md` records the choice and, more
> importantly, what stays uncovered: rollback-on-reject has no rejecting-circuit test, and
> `transition` has no dedicated end-to-end test at all.

---

## 1. What Tier A left behind, and why it is not one problem

`analysis-context.md` §2 named Tier B as a single bucket: *"`transition`, `slew`, `absdelay`,
`$limit`, `@(initial_step)`, `idt` with an initial condition — these need the model to remember
something between timesteps."*

**Measured against the corpus, that bucket does not hold together.** It conflates four
different storage lifetimes with two very different failure modes. Separating them is the main
analytical content of this document, exactly as separating A/B/C was for the last one.

| Construct | Corpus (files/uses) | What it actually needs | Failure mode of today's fold |
|---|---|---|---|
| `$limit` | **10 / 72** | the previous **Newton iterate** | **converges worse — same answer** |
| `absdelay` | 5 / 17 | an unbounded `(t, value)` **trajectory** | **wrong answer** in transient |
| `transition` | 7 / 14 | one committed `(t, value)` **per accepted step** | **wrong answer** in transient |
| `slew` | **0 / 0** | same as `transition` | wrong answer in transient |
| `@(initial_step)` | 0 / 0 | **nothing** — the solver already knows | body runs at *every* timepoint |
| `idt` w/ IC | — | the initial value of an unknown that already exists | wrong initial value |

### 1.1 `$limit` is not a correctness bug, and it is the most-used construct

This is the finding that most changes the shape of the work, so it goes first.

Every corpus use of `$limit` has the same shape — `$limit(V(a,b), "pnjlim", vt, vcrit)` — and
its first argument is always a *probe*. What it wants is the value of that branch voltage **at
the previous Newton iteration**, so it can clamp the step the way SPICE's classic `pnjlim` does.

That is a **within-timepoint** lifetime. It is never committed across an accepted step and never
rolled back; it lives and dies inside one Newton solve. It therefore needs a different channel
from anything else in this table — and, critically, it does not need *storage* at all, because
the solver already holds the previous iterate: it is the `x` that `newton_step` is about to
update.

More importantly: **a converged Newton solve is a fixed point of the *unlimited* equations.** A
limiter reshapes the iteration path toward that fixed point; it never moves the fixed point.
So folding `$limit` to its first argument yields *the same answer*, just reached less robustly
(or not at all, on a hard circuit). Contrast `transition`, where folding changes the waveform
itself.

`va-core` also already applies `convergence::limit_junction` to every unknown in its Newton
loop, so the project is not un-limited today — it is limited *globally* rather than where the
model asked.

**Consequence for this proposal: `$limit` is out of scope, and belongs with convergence work,
not with a state channel.** Putting the most-used construct out of scope needs saying plainly,
so: it is excluded because its failure mode is convergence robustness, its lifetime is the
Newton iteration, and its fix is "let a model direct `va-core`'s existing limiter" — three
reasons that all point away from per-instance state. Bundling it here would mean designing a
second, unrelated channel under cover of this one.

### 1.2 `@(initial_step)` needs no state at all

The solver knows whether it is evaluating the first timepoint of an analysis. That is one
`bool` on `AnalysisCtx` — Tier A's shape, not Tier B's. It is in scope here only because the
state channel needs the same flag for its own initialization (§3.3), so the two arrive together.

Today the parser accepts `@(initial_step) begin … end` and **discards the trigger, keeping the
body**, which is correct for a DC operating point and wrong in transient, where the body then
runs at every timepoint.

### 1.3 `absdelay` needs a trajectory, not a state vector

`absdelay(value, delay)` must produce `value(t − delay)`. No fixed-size state vector holds
that: the number of samples inside the delay window depends on the step size the LTE controller
happens to choose. It needs either a ring buffer whose depth is a guess, or a genuine
interpolated history buffer, plus a `bound_step` to keep the samples dense enough to
interpolate between.

That is buildable *on top of* the channel this proposal defines, but it is a second design
decision (how deep, interpolate how, what happens when the buffer underflows) with its own
accuracy story. **Deferred, explicitly, and the channel is shaped so it does not preclude it.**

### 1.4 What is left, and is genuinely one mechanism

`transition` and `slew` — and the state channel itself, which is the reusable part. Both need
exactly one thing: **a small, fixed-size chunk of `f64` committed at each accepted timepoint and
rolled back on a rejected one.** That is a tractable, self-contained contract, and it is what
this proposal delivers.

`slew` has **zero corpus uses** and is included anyway, because it is `transition`'s continuous
twin: it exercises the same channel with strictly simpler semantics (no breakpoints), which
makes it the honest first test of the mechanism rather than a feature in its own right.

---

## 2. Why the instance cannot own the storage

The obvious implementation — give the model a `RefCell<Vec<f64>>` and let it remember — is
wrong here, and it is worth writing down why, because it is the tempting one.

`ModelInstance::load` takes `&self` and is documented as **pure**: identical `(x, ctx)` in,
identical stamps out. Three consumers rely on that:

1. **Newton** re-enters `load` many times per timepoint. A model that mutated history on each
   call would evolve its state *within* a single solve, so the equations being solved would
   change under the solver's feet and the iteration would chase a moving fixed point.
2. **The LTE controller** solves every candidate step **twice** (primary + reference method,
   `integrator`'s embedded pair) and then **throws away rejected steps entirely**. A
   self-mutating model would commit history for a timepoint that never happened.
3. **Finite-difference Jacobian checks** (`CLAUDE.md` §5) perturb `x` and re-evaluate. If
   evaluation has side effects, the check measures the side effect too.

So the storage must be **solver-owned**, and the contract must answer three questions the
proposal for Tier A correctly refused to answer in passing:

- **Who allocates?** The consumer, from a size the instance declares.
- **When is it committed?** Only from the evaluation at an *accepted* timepoint.
- **What does a model see mid-solve?** The last committed value — never a sibling iteration's
  proposal.

---

## 3. The contract

### 3.1 Declaration

```rust
// va-abi/src/instance.rs
pub trait ModelInstance {
    /// Number of `f64` state slots this instance needs. Default 0 — stateless.
    fn state_len(&self) -> usize { 0 }
}
```

Defaulted, so every existing implementor is untouched. A consumer sums `state_len()` over its
instances once, at setup, and slices the resulting flat buffer per instance — the same
"declare your size, the consumer owns the array" shape `unknowns()` already uses for the
solution vector.

### 3.2 Access

```rust
// va-abi/src/state.rs (new)
pub struct ModelState<'a> {
    prev: &'a [f64],       // committed at the last accepted timepoint
    next: &'a mut [f64],   // this evaluation's proposal
}

impl ModelState<'_> {
    pub fn get(&self, slot: usize) -> f64;        // reads `prev`
    pub fn set(&mut self, slot: usize, v: f64);   // writes `next`
    pub fn len(&self) -> usize;
}
```

and `load` gains it:

```rust
fn load(&self, x: &[f64], ctx: &AnalysisCtx, state: &mut ModelState, sink: &mut dyn StampSink);
```

**Read-old / write-new is the whole trick.** A model can never observe another iteration's
proposal, so `load` stays a pure function of `(x, ctx, prev)` — the purity invariant is
*preserved*, not weakened, because `prev` is an input like any other. What changed is that
there is now an output channel besides the sink.

**Why break `load` again rather than add `load_stateful`.** Same reason as Tier A: a defaulted
second entry point leaves two ways to write a model, and the solver would have to guess which.
The parameter is inert for the ~all models with `state_len() == 0`.

### 3.3 Initialization

`prev` is zero-filled before the first evaluation, which is not a meaningful state for a slew
limiter (whose output should start *at* its input, not at 0 V). So the model needs to know it is
looking at the first evaluation:

```rust
pub struct AnalysisCtx {
    pub kind: AnalysisKind,
    pub time: f64,
    pub temp: f64,
    pub is_initial_step: bool,   // new
}
```

`true` for the first evaluation of a transient run, and **always `true` in DC/AC/noise** — a
static solve is definitionally its own initial step, which is also what makes `transition`/
`slew` keep folding to their input there, matching today's behavior exactly. This field is what
`@(initial_step)` reads too (§1.2).

### 3.4 Commit and rollback

The consumer holds two buffers:

```text
committed  — state as of the last accepted timepoint
scratch    — what the current evaluation proposes
```

Per evaluation sweep: **copy `committed` into `scratch`**, then `load` each instance with
`prev = &committed[range]`, `next = &mut scratch[range]`.

The copy matters and is not just hygiene: a model whose `set` sits inside an `if` may not write
every slot on every path. Pre-seeding `scratch` from `committed` makes an unwritten slot mean
"unchanged", which is the only sane reading — without it, a slot would silently inherit whatever
a *rejected* candidate wrote.

On **accept**: `committed = scratch`, taken from the post-accept assemble the integrator already
performs at the accepted point (it is a full, fresh evaluation at the accepted `x` and `t` —
exactly the right commit point, and it already exists).
On **reject**: nothing. `scratch` is overwritten at the next attempt.

DC/AC/noise never commit: they evaluate once (or iterate to a fixed point) and have no notion of
an accepted timepoint. Their `committed` stays zero and `is_initial_step` stays `true`.

---

## 4. Interface α changes

`Builtin::{Transition, Slew}`, replacing the elaboration folds. Argument normalization mirrors
`ac_stim`'s (§ Tier A): fixed arity, LRM defaults filled at elaboration, so no consumer
re-derives them.

- `transition(value, delay, rise, fall)` — 4 arguments, defaults `0, 0, 0`.
- `slew(value, pos_rate, neg_rate)` — 3 arguments; `neg_rate` defaults to `−pos_rate`.

`@(initial_step)` needs no IR change if the parser keeps producing the body as a block: it
becomes `Stmt::If { cond: Call(Builtin::Analysis-like initial-step query) }`. **Recommend** a
dedicated zero-argument `Builtin::InitialStep` reading `ctx.is_initial_step`, wrapped by the
frontend into an ordinary `Stmt::If` — no new statement kind, and the existing control-flow walk
handles arm selection for free.

---

## 5. Semantics to implement

**`slew`** — the clean one. With `y` the previous committed output and `Δt = t − t_prev`:

```text
y_new = clamp(value, y − |neg_rate|·Δt, y + pos_rate·Δt)
```

At the initial step, `y_new = value` (settle immediately). In DC, likewise — which reproduces
today's fold exactly. State: 2 slots, `(t_prev, y_prev)`.

**`transition`** — needs more than state. The LRM's `transition` is driven by *discrete* changes
in its input: when `value` changes, the output ramps to the new target over `rise`/`fall`, after
`delay`. Correctly resolving that ramp needs the integrator to take steps inside it, or the
waveform is sampled at whatever points the LTE controller happened to choose and the ramp is
mis-shaped.

**Tier A's `bound_step` is exactly the mechanism for that**, which is a pleasant consequence of
having shipped it: while a transition is in flight, the model requests a step bound of roughly
`rise/8`, and the ramp resolves. State: 4 slots — `(t_start, y_start, y_target, t_end)`.

This is an approximation of the LRM's event-scheduled semantics, not an implementation of them:
a real simulator forces exact breakpoints at the ramp's corners. It should be documented as
such rather than claimed as complete.

---

## 6. Validation

The Tier A `$abstime` gate worked because QSPICE could express the same physics independently
(a behavioral source reading `time`). **Ask the same question here before building anything**,
and expect a worse answer:

- **`slew`** — a slew-rate limiter is not a SPICE primitive. But a *ramp input* through a slew
  limiter has an exact closed form (the output is the input, clipped to a line of known slope),
  so a closed-form unit test is genuinely decisive even without QSPICE. **Spike first:** check
  whether a QSPICE behavioral source can express `clamp`/`limit` of a time expression; if it
  can, the gate is real, and if not, say so.
- **`transition`** — a piecewise-linear ramp is exactly what SPICE's `PWL` source produces. A
  deck driving a known step through `transition(v, 0, rise, fall)` should match a QSPICE `PWL`
  source with the same corners. **That is a plausible real gate** and is worth the spike.
- **Regression floor** — every existing gate must stay bit-identical. No model in the zoo
  declares state, so all 14 circuits must reproduce their present numbers exactly.

Per `no-fake-golden-data`, anything without an oracle is stated as unit-tested-only in
`docs/validation.md`, per construct — the split Tier A already documents.

---

## 7. Sequencing

1. `va-abi`: `state_len`, `ModelState`, `is_initial_step`; signature change; tests compile.
2. Consumers: `va-core` (stateless pass-through), `va-transient` (the commit/rollback loop),
   `va-acnoise`. **All 14 gates must be unchanged to the last digit** — this step is a pure
   refactor.
3. `va-ir` + `va-frontend`: `Builtin::{Transition, Slew, InitialStep}`, un-fold, arity
   normalization.
4. `va-codegen`: state-slot allocation per call site, evaluation, `bound_step` for `transition`.
5. Validation spikes (§6), then zoo models/circuits/golden for whatever has an oracle.
6. Docs: `interfaces.md` (both revisions), `token-reference.md`, `validation.md`, `roadmap.md`.

Steps 1–2 are behavior-preserving and could land alone, with "every number identical" as the
success criterion — the same split that made Tier A's risky half reviewable.

## 8. Explicit non-goals

- **`$limit`** (§1.1) — different lifetime, different failure mode; belongs with `va-core`'s
  convergence work.
- **`absdelay`** (§1.3) — needs an interpolated history buffer, a second design.
- **`idt` initial conditions** — an initialization concern for an unknown that already exists,
  not a per-evaluation state one.
- **True event-scheduled `transition`** (§5) — approximated via `bound_step`, not implemented
  with exact breakpoints.
