# Proposal: frequency-dependent small-signal stamps (Tier C)

**Status:** **delivered 2026-08-07**, with §7's non-goals intact. **Date:** 2026-08-07.
**Affects:** `va-abi` (Interface β), `va-ir` (Interface α), `va-frontend`, `va-codegen`,
`va-acnoise`.
**CLAUDE.md §6 step 1** — the written description the rule requires before any code.
**Follows:** `analysis-context.md` (Tier A, shipped 2026-08-06) and `model-state.md`
(Tier B, shipped 2026-08-07). This is the last of the three tiers, and the one both earlier
documents named as "largest".

> **Delivered 2026-08-07.** Gated against real QSPICE at `1.361e-15` (zoo 15/15), and it
> discriminates: restoring the DC-gain fold moves it to `6.282e3`. §7's non-goals (`zi_*`,
> transient Laplace, Laplace-shaped noise) all remain non-goals.
>
> **One thing the plan did not anticipate.** §5's oracle worked, but the first model implemented
> only the *voltage* transfer function and so drew no input current, while the reference RC
> network loads its source: `V(out)` matched perfectly and `I(V1)` was 100% out. Modelling the
> divider's **input admittance** `Y(s) = sC/(1+sτ)` as a second Laplace form fixed it — the two
> circuits genuinely were not the same circuit until both observable properties matched.
>
> Also found on the way, and unrelated to this tier: a single-terminal access (`V(out)`) makes
> elaboration intern an implicit `gnd` node, which netlist instantiation then hands a *floating*
> global unknown — a singular matrix. Pre-existing; the gate's model names its reference
> terminal explicitly to avoid it.

---

## 1. The problem, and the one insight that makes it small

`laplace_nd(value, num, den)` and its three siblings are rational transfer functions:
`H(s) = N(s)/D(s)`. They currently **fold to their DC gain `H(0)`** at elaboration
(`Elaborator::laplace_root_product_at_origin` / `array_lit_first`), so every filter model reads
flat: a one-pole lowpass and a straight wire produce identical AC responses.

`analysis-context.md` §2 framed the fix as expensive: *"either re-linearizing per frequency
point … or giving Interface β a complex-valued channel."* The second option is a large
restructuring. **The first is much cheaper than it looks, and this is the proposal's key
observation:**

> At a single frequency `ω`, any complex admittance `H = a + jb` is **exactly** representable by
> the *real* pair `G = a`, `C = b/ω` — because the assembled system is `(G + jω·C)`, which is
> then `a + jb`. Precisely.

So no complex channel is needed. `Interface β`'s existing `jacobian`/`dcharge` stamps already
span the complex plane at any given frequency; what is missing is only that the model is never
*told* the frequency, and that `linearize` is called once rather than per point.

That reduces Tier C to: **add `freq` to the context, and call `linearize` per frequency point
when — and only when — some model actually needs it.**

## 2. Scope, by corpus evidence

| Construct | Corpus (files/uses) | Decision |
|---|---|---|
| `laplace_nd` | 2 / 7 | **in scope** |
| `laplace_np` | 2 / 2 | **in scope** |
| `laplace_zp` | 1 / 1 | **in scope** |
| `laplace_zd` | 0 / 0 | in scope — free, same code path |
| `zi_nd`/`zi_np`/`zi_zd`/`zi_zp` | **0 / 0** | **out of scope** |

The `zi_*` family is the **Z-domain** (sampled-data) equivalent, and it needs something this
simulator does not have: a sampling interval, i.e. a clock. Implementing it would mean
inventing a discrete-time substrate for zero corpus demand. It keeps its DC-gain fold, and
`token-reference.md` says so per construct.

**Also out of scope: transient.** A Laplace filter in the time domain is a convolution, needing
either a state-space realization or a delay line — Tier B's `absdelay`-shaped problem, not this
one. In DC and transient a filter keeps evaluating to `H(0)`, exactly as today. That is a real
remaining limitation and is stated, not implied: this proposal makes AC right, and leaves
transient where it was.

One corpus use is `laplace_np(white_noise(...), {1}, {…})` — a Laplace-*shaped noise source*.
Colored noise needs the filter to multiply a PSD rather than an admittance, which is the noise
channel's business; also out of scope.

## 3. Contract changes

### Interface β

```rust
pub struct AnalysisCtx {
    …
    /// Small-signal frequency in Hz. Meaningful only when `kind` is `Ac`; 0.0 otherwise.
    pub freq: f64,
}

pub trait ModelInstance {
    /// Whether this instance's small-signal stamps depend on frequency.
    fn is_frequency_dependent(&self) -> bool { false }
}
```

**The `freq` field is the one Tier A deliberately refused**, and the refusal was conditional in
exactly this way: *"a `freq` field would be meaningless at the one call site that would most
obviously want it … Frequency arrives with Tier C, together with the re-linearization that makes
it meaningful."* This proposal supplies that re-linearization, so the field stops being a lie.
Both earlier documents' text must be updated rather than left contradicting the code.

`is_frequency_dependent` exists so the common case costs nothing: if no instance reports `true`,
`ac::run` linearizes **once**, exactly as today, and every existing golden gate stays
bit-identical. Only a circuit containing a real filter pays the O(points) cost. Making that
opt-in rather than unconditional is what keeps the regression floor free.

### Interface α

Four builtins, `Builtin::{LaplaceNd, LaplaceNp, LaplaceZd, LaplaceZp}`, carrying:

```text
Call(LaplaceNd, [value, Const(num_len), num_0, …, den_0, …])
```

Coefficients stay **lowered expressions, not const-folded numbers**. The corpus writes them as
parameter expressions (`` {1, `M_TWO_PI*Fgr, …} ``), and a model whose pole moves with a
netlist-overridden parameter is the normal case, not an exotic one. `num_len` is a `Const`
separator because a flat argument list has no other way to say where one list ends — the same
trick `noise_table` uses to avoid a string- or array-carrying `Expr` variant.

Root forms (`np`, `zd`, `zp`) are **not expanded into coefficients**. `H(jω)` is evaluated
directly in product form, `∏(1 − jω/ζ_k) / ∏(1 − jω/ρ_k)`, which is both simpler and better
conditioned than expanding high-order polynomials (the corpus has a 7-coefficient filter with
values around `1e71`). A root at the origin contributes a factor of `s` rather than
`(1 − s/ζ)`, which is what makes today's DC fold `0` there — the same rule, now evaluated at
`jω` instead of `0`.

## 4. How the stamp works

`va-codegen` splits a `laplace_*` call out of its containing contribution the way it already
splits `ddt` (charge), `white_noise` (noise) and `ac_stim` (excitation) — a fourth instance of
one pattern, not a new one.

At load, with `u` the input's `Dual` (value + gradient w.r.t. `x`):

- **DC / transient:** `H(0)` is real. Stamp `residual += H(0)·u.value` and
  `jacobian += H(0)·u.grad` — identical to today's folded behavior, which is why nothing moves.
- **AC:** `H(jω) = a + jb`. Stamp `jacobian += a·u.grad` and `dcharge += (b/ω)·u.grad`. The
  assembler forms `G + jω·C`, recovering `a + jb` exactly.

`ddt` is the special case `H(s) = s`, and it is reassuring that the general rule reproduces it:
`a = 0`, `b = ω`, so `C += u.grad` — precisely what the charge channel already stamps.

## 5. Validation — and this tier has a genuinely independent oracle

Tier A's `$abstime` gate compared against a QSPICE behavioral source; Tier B had no oracle worth
building. Tier C has the best of the three:

> `laplace_nd(V(in), {1}, {1, τ})` **is** a one-pole lowpass, `1/(1 + sτ)`. QSPICE computes the
> same response from an actual **R and C**, with `τ = RC`.

Those are genuinely independent descriptions — a rational transfer function on our side, two
physical components on QSPICE's — and neither side can be tuned toward the other without the
discrepancy showing up across the whole sweep. The existing `rc_ac.net` already proves QSPICE's
half is right, at machine precision.

**Discrimination check, as for `$abstime`:** restore the DC-gain fold and the gate must go red.
A flat response versus a −20 dB/decade rolloff is not a subtle difference; if the gate passes
with the fold restored, the gate is wrong.

**Regression floor:** all 14 existing gates must reproduce their numbers to the last digit. No
model in the zoo declares frequency dependence, so all take the single-linearize path.

## 6. Sequencing

1. `va-abi`: `freq`, `is_frequency_dependent`. Update Tier A's "no freq field" text.
2. `va-acnoise`: conditional per-frequency `linearize`. Gates unchanged — pure refactor.
3. `va-ir` + `va-frontend`: four builtins, stop folding, keep coefficients as expressions.
4. `va-codegen`: lowering split, `H(jω)` evaluation, the two stamp paths.
5. Zoo model + circuit + QSPICE golden; discrimination check.
6. Docs: `interfaces.md`, `token-reference.md` (8 entries), `validation.md`, `roadmap.md`, and
   the two earlier proposals' now-stale "frequency arrives later" notes.

## 7. Explicit non-goals

- **`zi_*`** — needs a clock; zero corpus demand.
- **Transient Laplace** — a convolution/state-space problem, not a stamping one.
- **Laplace-shaped noise** — belongs to the noise channel.
- **`laplace_*` with a null argument** (`,,`) — unchanged; needs an optional-argument grammar
  nothing else uses.
