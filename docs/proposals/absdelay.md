# Proposal: `absdelay` — a real transport delay, across disciplines

**Status:** proposed, not ratified. **Date:** 2026-09-01.
**Affects:** `va-ir` (Interface α), `va-frontend`, `va-codegen`. `va-abi` (Interface β),
`va-core`, `va-transient`, `va-acnoise`, `va-netlist` are **not** touched — see §3.
**CLAUDE.md §6 step 1** — the written description the rule requires before any code.

This is a decision document. No Rust code changes are part of this proposal.

---

## 1. What is wrong today

`absdelay(expr, delay [, maxdelay])` (LRM §4.5.9) delays a continuous waveform by `delay`
seconds. This engine **folds it to its undelayed input** at elaboration
(`crates/va-frontend/src/elaborate.rs`, the `name == "absdelay"` arm: `return
self.lower_expr(value)`), so nothing about the delay survives into the IR.

For DC that fold is **correct** — a delayed signal at a fixed operating point equals its input,
and this is the same reasoning that makes `transition`/`slew` settle to their inputs in a static
solve. For **AC and transient it is wrong**, and silently so: the model builds, converges, and
returns a plausible waveform that is missing the physics the operator was written for.

`va-cli check`'s 2026-09-01 soundness split makes the scale concrete: of 104 passing corpus
files, **9 compute something other than what their source says**, and 5 of those 9 are
`absdelay`.

### 1.1 Why this one matters more than its use count suggests

| File | Uses | Delay expression | Domain |
|---|--:|---|---|
| `photonic/Waveguide.va` | 4 | `length * groupIndex / \`P_C` | optical |
| `photonic/Pcw.va` | 4 | `length * groupIndex / \`P_C` | optical |
| `photonic/PhaseModulator.va` | 4 | `length * groupIndex / \`P_C` | optical |
| `photonic/PcwPhaseModulator.va` | 4 | `length * groupIndex / \`P_C` | optical |
| `fbh_hbt-2_1.va` | 1 | `Tf` (forward transit time) | electrical |
| **total** | **17** | | |

`length * groupIndex / P_C` is `L·n_g/c` — the group delay of an optical waveguide. It is not
an incidental term in those models; **it is what a waveguide is**. With the fold in place, light
crosses the guide instantly, which makes an interferometer built from these primitives report
the wrong interference and a modulator report the wrong response. `CLAUDE.md` §1 names optical
among the disciplines this simulator is meant to govern, so this is not an obscure corner.

`fbh_hbt-2_1.va`'s `absdelay(V(ni), Tf)` is the same operator doing the same job in a different
discipline: an HBT's forward transit time is a transport lag through the base.

### 1.2 What the operator is, discipline-independently

`absdelay` delays a **value**. It has no opinion about what that value is: a voltage, an optical
field envelope, a temperature, a pressure. That is worth stating explicitly because it decides
the design — nothing in the implementation may assume an electrical nature, and nothing needs
to: the state channel stores plain `f64`, and the delay is in seconds regardless of discipline.

---

## 2. The three regimes, and why they are different problems

The corpus shows one regime. The disciplines this project targets contain three, and they place
genuinely different demands on the implementation. Sizing the design to the corpus alone would
produce something that quietly fails the moment a thermal or fluid model arrives.

### 2.1 Photonic — delay comparable to, or below, one timestep

A 100 µm waveguide at `n_g = 4.34` gives **τ ≈ 1.45 ps**. A 1 cm guide gives ≈ 145 ps.
Envelope-modulation timesteps for a 10 GHz signal land around 1–10 ps, so τ/h spans roughly
`0.1` to `100`.

Two consequences:

- **The sub-timestep case is normal, not exotic.** An implementation that only looks up stored
  history points cannot answer `x(t − 1.45 ps)` when the last accepted step was 10 ps ago and
  the *current* step is still being solved. It must interpolate, and near the origin it must
  interpolate between the previous accepted point and the point currently being solved for —
  which makes the delayed value a function of `x`, i.e. it enters the Jacobian.
- **The delayed quantity is smooth.** These models separate the fast optical carrier (handled
  analytically as a constant phase rotation, `OptE(transfer_pol[1])` in `Waveguide.va`) from the
  slowly-varying envelope, and only the envelope is delayed. Linear interpolation on a smooth
  envelope is sound; it would *not* be sound on a raw carrier at 193 THz, and this proposal does
  not claim otherwise.

### 2.2 Thermal — delay spanning many thousands of timesteps

A thermal transport lag is milliseconds to seconds; a transient solving electrical detail
alongside it may take microsecond steps. `τ/h` of `10³–10⁶` is ordinary.

This is the regime that decides the **memory** design. A history buffer of `N` points costs
`2N` f64 per call site (time and value). At `τ/h = 10⁶` a naive "store every accepted point back
to `t − τ`" buffer is 16 MB **per call site**, which is not acceptable and, worse, degrades
silently as a simulation runs longer.

It also decides an **error** question. The buffer must be bounded, so either the model declares
`maxdelay` (the LRM's optional third argument exists precisely for this) or the implementation
must choose a bound and say what happens past it. Silently returning the oldest stored value —
i.e. quietly shortening the delay — is the failure this proposal most wants to avoid, because it
looks exactly like a working simulation.

### 2.3 Fluid mechanics — a delay that *varies with time*

A transport delay in a flow is `τ = L/v(t)`, and `v` is a solved quantity. The LRM permits the
delay argument to be an expression, so this is legal Verilog-A.

It is a different algorithm, not a harder version of the same one:

- The lookup becomes a **search** through history rather than an index, because the target time
  `t − τ(t)` no longer advances monotonically with `t`.
- It has a **physical validity condition**. If `dτ/dt < −1`, the target time runs backwards
  faster than simulation time runs forwards: two different present times map to the same past
  time, and the delay line "overtakes" itself. Physically that corresponds to information
  arriving out of order — in a flow, to a velocity increase so abrupt that later fluid arrives
  before earlier fluid. A simulator must either reject this or document what it produces.
- `maxdelay` becomes load-bearing rather than advisory: the buffer must span the *largest* delay
  the run will ever request, which is not knowable from the parameters alone.

**Zero corpus demand today** (all 17 uses have a delay that is a constant parameter expression).
That is the argument for deferring it — not for pretending it is a small extension of the
constant-delay case.

---

## 3. What changes, and what does not

### 3.1 Interface α — one additive `Builtin` (the change needing ratification)

```rust
// va-ir/src/lib.rs
pub enum Builtin {
    // ...unchanged...
    /// `absdelay(value, delay [, maxdelay])` — LRM §4.5.9. Arguments are flattened to
    /// `[value, delay]` or `[value, delay, maxdelay]`, matching how `Builtin::Transition`
    /// already normalizes its optional arity at elaboration.
    Absdelay,
}
```

**Precedent:** T5.6 added `Builtin::NoiseTable` to Interface α as an additive variant, and
T5.7 added `Builtin::NoiseTableLog` beside it. This is the same shape. The cost is that
`va-codegen` must handle the new variant — but it must anyway, since it is the crate that will
implement the operator, and a non-exhaustive match is not how this workspace is written.

**Why the fold cannot simply stay and the work happen elsewhere.** The fold happens *at
elaboration*, so by the time codegen sees the IR there is no delay left to implement — the tree
says `OptE(fwd[0])` and nothing more. Any implementation therefore requires the operator to
survive into the IR, and that is an Interface α change by definition.

### 3.2 Interface β — **not** touched

Deliberately verified rather than assumed:

- **History storage** rides the existing state channel. `ModelInstance::state_len()` is already
  an instance-declared `usize` with no per-construct width baked into `va-abi`
  (`StatefulKind::{Slew, Transition, Ddt}` already declare 2, 5 and 3 slots respectively). A
  delay line declaring `2N` slots is `va-codegen`'s own bookkeeping.
- **The AC path** already exists. `AnalysisCtx::freq` carries the frequency, and `laplace_*` is
  implemented in AC by evaluating `H(jω)` per frequency point and stamping `G = Re(H)`,
  `C = Im(H)/ω`, which the assembled `G + jω·C` reconstitutes exactly (token-reference §1.6,
  Tier C 2026-08-07).
- **Step control** already exists. A model can request a bound on the integrator's step through
  `StampSink::bound_step`, which `va-transient` honours. A delay line that needs the step kept
  below some fraction of τ can ask for it itself, with no new mechanism.

| Crate | Needs edits? | Why |
|---|---|---|
| `va-ir` | **Yes** | `Builtin::Absdelay` (§3.1). Additive. |
| `va-frontend` | **Yes** | Stop folding; lower to the new builtin, with arity normalized and `delay`/`maxdelay` const-evaluated where they are constant. |
| `va-codegen` | **Yes** | The whole implementation: AC stamping, transient history, state-slot allocation. |
| `va-abi` | **No** | §3.2. |
| `va-core`, `va-transient`, `va-acnoise`, `va-netlist`, `va-cli` | **No** | No new plumbing; the transient path may *use* `bound_step`, which already exists. |
| `va-abi::reference` | **No** | No hand-written reference model uses a delay. |

---

## 4. A staged plan

Each stage is independently green and independently useful, and stage 1 is where most of the
corpus value is.

### Stage 1 — AC, which is exact and nearly free

`H(jω) = e^(−jωτ)` is a *pure* delay's exact frequency response, so:

```text
G = Re(H) = cos(ωτ)        C = Im(H)/ω = −sin(ωτ)/ω
```

This is not an approximation of a delay — it *is* the delay, at that frequency. It reuses the
`laplace_*` machinery unchanged, and it is the analysis a photonic interferometer is usually
studied in: an MZI's fringe pattern is a frequency-domain statement about two path delays.

At `ω → 0` the `C` term is `−τ` in the limit; the implementation must use that limit rather than
dividing by zero, exactly as the `laplace_*` path must already handle `s = 0`.

**Deliverable:** `absdelay` correct in AC and DC, still folded (and still warned about) in
transient. Gate: an interferometer deck whose fringe spacing is set by a known delay, checked
against the closed form `|1 + e^(−jωτ)|`, and — if QSPICE will accept the deck — against it.

### Stage 2 — transient, constant delay

Per call site, a ring buffer of `(t, value)` pairs on the state channel, linear interpolation to
`t − τ`, and:

- **Bounded, with the bound stated.** `N` is chosen from `maxdelay` when the model supplies it,
  otherwise from the constant-folded `delay`. A request that falls off the end of the buffer is
  an **error**, not a silently-shortened delay (§2.2).
- **Sub-timestep delays handled** (§2.1): when `t − τ` lies inside the step being solved, the
  delayed value depends on the unknown `x`, so it contributes a Jacobian entry — the AD path
  must carry the interpolation weight, not just the value.
- **Self-bounding.** The site requests `bound_step(τ/k)` for a small `k` so the integrator
  cannot step clean over a delay it is supposed to resolve. This is the mechanism that keeps the
  photonic sub-timestep case honest rather than merely tolerable.

**Deliverable:** transient delay for a constant `τ`. Gate: a delay line whose output is the
input shifted by exactly τ, checked point-by-point against the analytic shift, plus a
sub-timestep case (τ < h) where a naive implementation returns the undelayed input and this one
does not — the discriminating test, in the shape the BDF2 order study established.

### Stage 3 — time-varying delay (fluid transport), conditional

Only if a model needs it. Requires the search-based lookup, and must decide the `dτ/dt < −1`
question explicitly (§2.3): reject with a clear error, or document the overtaking behaviour.
**Not** to be attempted as a small extension of stage 2.

### What stage 1+2 will *not* support, stated plainly

A time-varying delay; a delay longer than the buffer bound; and interpolation good enough for a
signal that is not smooth on the timestep scale (a raw optical carrier rather than an envelope).
Each of those should produce an **error naming the limit**, never a quietly wrong waveform —
the standing lesson from `absdelay`'s own fold.

---

## 5. Is it worth doing?

**Stage 1: yes, clearly.** It is a small addition to a mechanism that already works, it is exact
rather than approximate, and it fixes 4 of the 5 affected corpus files in the analysis those
models are most used in. It also removes the most embarrassing current behaviour — a waveguide
with no delay — at the lowest cost available.

**Stage 2: yes, but it is the real work.** The sub-timestep case makes the delayed value part of
the Jacobian, which is where the difficulty lives; a delay line that interpolates but does not
differentiate would degrade Newton convergence rather than fail outright, which is the same
silent-failure shape the BDF2 half-wiring had. Budget the discriminating test first.

**Stage 3: not now.** Zero demand, a different algorithm, and a physical validity condition that
deserves its own decision.

**A caution against over-generalising.** The temptation is to build stage 3's machinery first
because it subsumes the others. It does not pay: the constant-delay ring buffer is a genuinely
simpler object than a searchable history, the corpus needs only the simpler one, and the thermal
regime's real constraint (§2.2) is memory, which a general implementation makes *worse* rather
than better.

---

## 6. Open questions for the supervisor

1. **Stage 1 alone, or stages 1+2 together?** Stage 1 leaves transient still folded — but now
   with AC correct, which is arguably a *more* confusing state for a reader than uniform
   foldedness, unless the warning is clear.
2. **What should exceeding the buffer do?** This proposal says error. The alternative — clamp to
   the oldest stored value — is what some simulators do, and it is the friendlier behaviour for a
   long thermal run at the cost of being wrong quietly.
3. **Is a photonic gate circuit in scope for the zoo?** Stage 1's natural gate is an
   interferometer, which needs the optical discipline and the vector-net machinery the corpus
   models use. That is a bigger step than adding another `.net` deck, and may deserve its own
   decision.
