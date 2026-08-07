# Proposal: an analysis context on Interface β

**Status:** **Tier A delivered 2026-08-06.** Tiers B and C remain proposed, not ratified.
**Date:** 2026-08-05. **Affects:** `va-abi` (Interface β), `va-ir` (Interface α),
`va-frontend`, `va-codegen`, `va-core`, `va-transient`, `va-acnoise`, `va-cli`.
**CLAUDE.md §6 step 1** — this document is the written description the rule requires before any
code.

> **Delivered (2026-08-06): §2's Tier A, in full.** `analysis()`, `$abstime`, `ac_stim` and
> `bound_step` no longer fold; `AnalysisCtx` is on both Interface β entry points; `run_dynamic`
> and `build_instances_split` are deleted. See `docs/interfaces.md`'s paired revisions of that
> date for the ratified contracts and `docs/roadmap.md`'s "Analysis context — Tier A" section
> for the outcome and evidence.
>
> **Three things shipped differently from the recommendation below**, and the text is left
> unedited so the reasoning stays legible:
>
> 1. **§5 recommends deferring `ac_stim`; it was included.** The RHS channel it needs turned out
>    to be a single defaulted `StampSink::excitation` method, additive and never called outside
>    AC — the same shape as the three previous §6 revisions, and cheap enough that splitting it
>    into a follow-up would have cost more coordination than it saved.
> 2. **§4 places the phase bitmask in `va-abi`; it belongs in `va-ir`.** §3's dependency table
>    forbids `va-frontend` — which *produces* the mask — from seeing `va-abi` at all. The
>    encoding therefore lives in Interface α, `va-abi` answers the runtime question from a
>    `&str`, and `va-codegen` (the only crate depending on both) joins them.
> 3. **§1's table says `laplace_*`/`zi_*` fold to `num[0]/den[0]`.** They do fold to a DC gain,
>    but via `laplace_root_product_at_origin`, which handles the pole/zero forms too.
>    `token-reference.md`'s rows for that family were separately stale ("rejected at
>    elaboration") and were corrected in the same pass.
>
> **§6/§7's blocking spike was run (2026-08-06), and §6's `$abstime` plan works as written.**
> QSPICE's behavioral source does expose `time` (`B1 out 0 I=1*time`), so `$abstime` is now
> **golden-gated** by `circuits/abstime_ramp.net` at error `4.382e-17`, zoo 14/14 — and it
> discriminates, confirmed by watching it fail at `5.838e-1` with the original fold restored.
> One thing §6 could not have predicted: `UIC` offsets QSPICE's own `time` by exactly 1e-7 s,
> so this deck must skip the `cold_start_tran_deck` treatment every other transient gate needs.
>
> **Tier B shipped 2026-08-07** (`model-state.md`) and **Tier C on 2026-08-07**
> (`frequency-domain.md`), so all three tiers of §2 are now delivered. In particular §3's "no
> `freq` field" was a *conditional* refusal — "frequency arrives with Tier C, together with the
> re-linearization that makes it meaningful" — and Tier C supplied exactly that, so
> `AnalysisCtx::freq` now exists and is honest.
>
> `analysis()`, `ac_stim` and `bound_step` remain **unit-tested, not golden-gated** — §6's
> by-construction plan for `analysis()` was not pursued, and the other two have no QSPICE
> counterpart drivable from a model. `docs/validation.md` states the split per property.

---

## 1. The problem

`va-frontend` was written when DC was the only analysis. It **const-folds** a family of
Verilog-A constructs on that basis, permanently, at elaboration, before any analysis runs. T4
(transient) and T5 (AC/noise) then landed and the folds were never revisited. Each is now a
silent wrong answer in an analysis that exists today:

| Construct | Folds to | Now wrong in | Corpus files |
|---|---|---|---|
| `analysis("tran")` | `false`, always | transient — branch never fires | 0 |
| `analysis("dc"/"static")` | `true`, always | transient — DC-init branch fires every timepoint | 0 |
| `$abstime` | `0.0` | transient — a time-dependent model is frozen at t=0 | 4 |
| `ac_stim` | `0.0` | AC — a model's own AC excitation contributes nothing | 0 |
| `bound_step` | no-op | transient — the adaptive controller could consume it | 0 |
| `transition`, `slew`, `absdelay` | their own input | transient — these *are* the dynamics | 5 |
| `laplace_*`, `zi_*` | DC gain `num[0]/den[0]` | AC — every filter model reads flat | 5 |

The corpus column counts `external/`'s 150 industry compact models, and it is low on purpose:
compact device models are analysis-agnostic constitutive equations. The models that need these
constructs are **behavioral** ones — `external/photonic/`, `microring_modulator.va`, anything
modelling a PLL, ADC, driver or optical link. That is the half of Verilog-A's user base this
simulator currently cannot serve correctly, and no netlist feature fixes it.

**None of this is visible in `cargo xtask validate`.** All 13 gated circuits use textbook
devices (R, C, diode, MOS, BJT) containing no analysis-dependent construct. A green 13/13 is
not evidence against any of the above.

### The keystone

`ModelInstance::load(&self, x: &[f64], sink: &mut dyn StampSink)` carries **no time, no
frequency, and no analysis kind**. A model therefore *cannot* be told what is running, so the
frontend's only option was to guess at elaboration — and it guessed "DC", correctly at the time.

`va-transient` already works around this. `integrator::run_dynamic` takes
`impl FnMut(f64) -> Vec<Box<dyn ModelInstance>>` and **rebuilds boxed instances at every
timestep** purely to inject the current time into a source. That is not a one-off convenience;
it is the contract's missing parameter showing through, at the cost of an allocation per step.

---

## 2. What this proposal is *not*

"Un-fold the DC-only constructs" conflates **three different mechanisms**. Only the first is in
scope here. Separating them is the main analytical content of this document.

### Tier A — needs analysis context only *(this proposal)*

`analysis()`, `$abstime`, `ac_stim`, `bound_step`. Each is a pure function of *what is running
right now*. The instance stays stateless: given `x` and a context, `load` is deterministic and
repeatable, exactly as today. No new storage, no ordering constraints, no history.

### Tier B — needs per-instance state across evaluations *(separate, later)*

`transition`, `slew`, `absdelay`, `$limit`, `@(initial_step)`, `idt` with an initial condition.
These need the model to *remember* something between timesteps (a slew accumulator, a delay
line, a "have I initialised" flag). Interface β is deliberately `&self` with no interior
mutability, and `va-core`/`va-transient` re-enter `load` freely — including on **rejected**
timesteps that must not corrupt history. A state channel needs its own contract answering:
who owns the storage, when is it committed versus rolled back, and what happens across a
Newton iteration versus across an accepted step. That is a genuinely harder design than Tier A
and must not be smuggled into it.

### Tier C — needs frequency-domain evaluation *(separate, later, largest)*

`laplace_*`, `zi_*`. Today `ac::linearize` calls `load` **once**, outside the frequency loop,
because `G` and `C` are frequency-independent by construction. A Laplace filter's small-signal
response is *not* — it is a genuinely complex, frequency-dependent gain. Supporting it means
either re-linearizing per frequency point (an O(points) cost increase on every AC run) or
giving Interface β a complex-valued channel. Adding `freq` to a context does **not** deliver
this; the restructuring is the work.

**Recommendation: ship Tier A alone as one §6 change.** It is self-contained, it deletes an
existing workaround, and it is the prerequisite for both later tiers. Attempting all three at
once produces an unreviewable PR touching every crate and a state contract designed under
time pressure.

---

## 3. The contract change (Interface β)

```rust
// va-abi/src/analysis.rs (new)

/// Which analysis is driving this evaluation.
pub enum AnalysisKind { Dc, Transient, Ac, Noise }

/// What the simulator knows about the evaluation a model is being asked for.
pub struct AnalysisCtx {
    pub kind: AnalysisKind,
    /// Absolute simulation time (s). Meaningful for `Transient`; `0.0` otherwise.
    pub time: f64,
    /// Simulation temperature (K).
    pub temp: f64,
}
```

```rust
// va-abi/src/instance.rs
fn load(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn StampSink);
fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) { }
```

### Design decisions, with the reasoning

**No `freq` field.** Tempting, and wrong for Tier A. Nothing in Tier A is frequency-dependent
(`analysis("ac")` needs the *kind*, not the value), and `linearize` calls `load` once outside
the frequency loop — a `freq` field would be a lie at that call site, and a field that is
usually a lie is how the DC-only folds happened in the first place. Frequency arrives with
Tier C, together with the re-linearization that makes it meaningful.

**`temp` moves into the context.** `noise` already takes a bare `temp: f64` argument; folding it
in unifies the two signatures and gives `load` the temperature it never had — which is what
makes a `.temp` card implementable later, and what `$temperature` should read instead of a
compiled-in constant.

**Break `load`'s signature rather than adding `load_with_ctx`.** The three previous §6 changes
were default trait methods precisely because they were optional additions. This one is not:
every implementor must see the context or the whole point is lost, and a defaulted
`load_with_ctx` that falls back to `load` leaves two ways to write a model, one of which is
quietly wrong. The blast radius is small and fully enumerated:

- **6 production implementors** — `va-abi::reference::{Resistor, Capacitor, Diode, Bjt, VSource}`
  and `va-codegen::GeneratedModel`. The five reference models ignore the context and change by
  one parameter each.
- **4 production call sites** — `va-core::mna::assemble`, `va-core`'s `AbstolOverride` wrapper,
  `va-transient::integrator`, `va-acnoise::ac::linearize`.
- **~40 test call sites**, mechanical (`inst.load(&x, &mut sink)` → `inst.load(&x, &DC, &mut sink)`
  with a `const DC: AnalysisCtx` test helper).

One PR, no adapters, no deprecation window. The trait is internal to this workspace.

---

## 4. Interface α changes

Two builtins must survive elaboration instead of folding.

**`Builtin::Abstime`** — zero arguments. Trivial.

**`Builtin::Analysis`** — the interesting one, because `analysis("tran", "static")` takes
*string* arguments and `Expr::Call` carries `Vec<ExprId>` of numeric expressions. Three options:

1. A new `Expr` variant holding strings — pollutes every arena walk for one construct.
2. String interning in `Module` — new storage, new lifetime questions.
3. **Fold the argument list to a bitmask `Const` at elaboration.** The LRM requires these
   arguments to be string literals, and `analysis_matches` already enforces exactly that. So
   elaboration maps the recognized phase names to bits and emits
   `Call(Analysis, [Const(mask)])`.

**Recommend (3)**, for the same reason `Builtin::NoiseTable` flattens its table into `Const`
arguments: it keeps every existing arena walk, clone and validity check working with no change,
and it does the string-shaped work once, at the place that can still name a source file when a
phase name is unrecognized.

---

## 5. Downstream work, crate by crate

| Crate | Change |
|---|---|
| `va-abi` | new `analysis.rs`; `load`/`noise` signatures; 5 reference models pass the context through |
| `va-ir` | `Builtin::{Analysis, Abstime}` |
| `va-frontend` | stop folding; emit the two builtins; phase-name → bitmask; keep rejecting non-literal arguments |
| `va-codegen` | `ad::Ctx` carries the `AnalysisCtx`; two `eval` arms (both zero-gradient — neither is a function of `x`) |
| `va-core` | `assemble` takes and forwards a context; DC path builds `AnalysisKind::Dc` |
| `va-transient` | forwards `Transient { time: t }` per evaluation — **and `run_dynamic`'s per-step instance rebuild is deleted**, its callers collapsing onto the ordinary path |
| `va-acnoise` | `linearize` forwards `Ac`; `noise` forwards `Noise` |
| `va-cli` | drops the `build_instances_split` waveform-source special case that only existed to feed `run_dynamic` |

The `run_dynamic` deletion is the proposal's clearest payoff: a workaround removed, an
allocation per timestep removed, and one code path where there were two.

`ac_stim` needs one extra thing beyond the context: it contributes to the AC **right-hand
side**, not to `G`/`C`, and `StampSink` has no RHS channel (today the excitation vector is built
by `va-cli` from the netlist's `AC mag phase`). Either add an `excitation(row, re, im)` method
to `StampSink` — additive, defaulted, harmless in DC/transient where it is never called — or
defer `ac_stim` to a follow-up. **Recommend deferring**: it is 0 corpus files, and bundling an
RHS channel into a signature change dilutes a clean PR.

---

## 6. Validation — the hard part

**QSPICE cannot be a direct oracle here**, because it does not consume our Verilog-A models and
our netlist cannot express a behavioral model natively. This is a real constraint, not a
formality: `docs/…/no-fake-golden-data` forbids hand-computing a golden file to fill the gap.

The way through is to build, for each new gate, a circuit whose answer QSPICE **can** produce
from its own primitives, and drive it here from a Verilog-A model:

- **`$abstime`** — a model contributing `I <+ k·$abstime` into a resistor is a current ramp.
  QSPICE expresses the same thing with a behavioral source (`B1 … I=k*time`). Both sides then
  compute the same waveform from genuinely independent descriptions. **Open question to
  de-risk first:** confirm QSPICE's behavioral-source syntax and that `time` is available in
  it — a short spike with a throwaway deck, before committing to this plan.
- **`analysis()`** — no single QSPICE construct corresponds. Gate it *by construction* instead:
  one model whose DC branch and transient branch are deliberately different devices (say a
  resistor in DC, a resistor plus a known current offset in transient), then check the `.op`
  gate against a QSPICE resistor deck and the `.tran` gate against the QSPICE offset deck. Each
  half has a native oracle even though the model as a whole does not.
- **Regression floor** — every existing gate must stay bit-identical. Tier A adds no term to any
  model in the current zoo, so all 13 circuits must reproduce their present numbers exactly.
  That is the strongest single check available and it is free.

State plainly in `docs/validation.md` which properties are golden-gated and which are unit-test
only — the same split already documented for `noise_table`'s interpolation rules.

---

## 7. Sequencing

1. **Spike (first, blocking):** confirm the QSPICE behavioral-source oracle actually exists.
   If it doesn't, §6's validation plan needs rethinking before any code is written.
2. `va-abi`: `AnalysisCtx`, signature change, 5 reference models, tests compile.
3. `va-core` / `va-transient` / `va-acnoise` call sites; delete `run_dynamic`'s rebuild; the
   existing 13 gates must be unchanged to the last digit.
4. `va-ir` + `va-frontend`: the two builtins, the bitmask fold, elaboration tests.
5. `va-codegen`: two `eval` arms, FD tests confirming zero gradient.
6. New zoo models, circuits and QSPICE golden per §6.
7. Docs: `interfaces.md` (both revisions), `token-reference.md` (7 entries change status),
   `roadmap.md`, `validation.md`, and the T4/T5 tutorials that currently describe the folds as
   correct.

Steps 2–3 are a behaviour-preserving refactor and could land as their own PR, leaving 4–6 as
the feature. That split is worth taking: it puts the risky part (a frozen-interface change) in
a PR whose success criterion is "every number is identical".

## 8. Explicit non-goals

Tier B (`transition`, `slew`, `absdelay`, `$limit`, `@(initial_step)`, `idt` ICs) and Tier C
(`laplace_*`, `zi_*` in AC) are **not** delivered by this change and their folds remain wrong
afterwards. `token-reference.md` must say so per construct rather than implying the whole
family is fixed.
