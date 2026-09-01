# Proposal: `Method::Gear` (variable-step BDF2) in `va-transient`

**Status: RATIFIED and implemented, 2026-09-01.** **Proposed:** 2026-08-31.

> Outcome: stages 1-4 all landed. The measured checkpoint §5 asks for came out **against** Gear
> on this project's circuits 
> (`docs/validation.md`): same step count as trapezoidal, uniformly worse accuracy (1.05x on
> `rectifier`, 9.3x on `rlc_ring`). It ships as opt-in, trapezoidal stays the default. Two
> details in this document were corrected during implementation: `divided_difference_history`
> for Gear is 3, the *same* as trapezoidal rather than one more (both are order 2), and Gear can
> start at step 2 rather than step 3, since one backward-Euler step already supplies the second
> history charge.
**Affects:** `va-abi` (Interface β), `va-transient`, `va-codegen`, `va-cli`. `va-ir`
(Interface α), `va-core`, `va-acnoise` are **not** touched — see §2.
**CLAUDE.md §6 step 1** — the written description the rule requires before any code.

This is a decision document. No Rust code changes are part of this proposal.

---

## 1. What changes and why

### 1.1 The two halves of `ddt`, and why only one of them is a problem

`crates/va-transient/src/integrator.rs`'s `Companion` struct (lines 123–196) is the reason
`BackwardEuler` and `Trapezoidal` share one `newton_step` with no per-method branching: both
reduce to `residual(x) + coeff·charge(x) + offset = 0`, where `coeff`/`offset` are derived from
one previous charge vector `q_prev` (`Companion::backward_euler`, `Companion::trapezoidal`).
Variable-step BDF2's own coefficients,

```text
dQ/dt ≈ (1/h)·[ (1+2r)/(1+r)·Q_n − (1+r)·Q_{n−1} + r²/(1+r)·Q_{n−2} ],   r = h_n/h_{n−1}
```

(confirmed to reduce to the textbook uniform-step `(3/2·Q_n − 2·Q_{n−1} + ½·Q_{n−2})/h` at
`r = 1`) fit this shape **fine** — `coeff = (1/h)·(1+2r)/(1+r)`, and the `Q_{n−1}`/`Q_{n−2}`
terms both fold into `offset`, exactly the way `Companion::trapezoidal`'s `offset` already
folds in `q_prev` and `r_prev`. **The stamping/charge-channel path needs no Interface β
change at all** — it needs `va-transient` to track a second history vector (`q_prev2`) and a
step-ratio `r`, which is entirely internal to that crate.

The problem is the *other* path. `AnalysisCtx::ddt_coeff` / `ddt_prev_rate_weight`
(`crates/va-abi/src/analysis.rs`) let a compiled model read `ddt(q)` as a bare **value**
rather than routing it through the charge channel — used whenever a `ddt` site's result is
consumed by something other than a direct `<+` contribution (`I(<port>)` re-reads, a
bias-dependent coefficient's product rule; see `crates/va-codegen/src/ad.rs`'s `Builtin::Ddt`
arm, lines 1032–1050, and `crates/va-codegen/src/lower.rs`'s `StatefulKind::Ddt`, width 2:
`(q_prev, rate_prev)`, lines 695–714). The reconstruction is

```text
dq/dt = ddt_coeff·(q − q_prev) − ddt_prev_rate_weight·rate_prev
```

— one previous **charge** value plus one previous **rate** value, recursively defined (this
is exactly the "undamped recursion" `docs/roadmap.md`'s 2026-08-31 first-step-seeding
investigation documents at length for trapezoidal). I verified this is genuinely a two-term
recursion, not a three-point formula: it works for backward Euler (`rate_prev` weight `0`)
and for trapezoidal (whose own closed form, `Q_n − Q_{n-1} = h/2·(rate_n + rate_prev)`,
*is* exactly this recursion — the previous **rate**, not the previous-previous **charge**, is
the second free variable). BDF2's three-point formula is not expressible that way in general:
at the previous accepted step, `rate_{n−1}` was itself built from `Q_{n−1}`, `Q_{n−2}`,
`Q_{n−3}` under a *different* ratio `r_prev = h_{n−1}/h_{n−2}` — there is no algebraic identity
that recovers `Q_{n−2}`'s weight in `rate_n` from `rate_{n−1}` alone. BDF2 genuinely needs two
distinct history *charges*, not one charge and one derived rate.

**This confirms the background's central claim**, with the precise mechanism: it is not that
BDF2 "needs more numbers", it is that its recursion structure is a different *shape*
(three-point finite difference on `Q`) from backward Euler/trapezoidal's shape (one-step
recursion on a *rate*), and the existing two-field channel encodes the second shape only.

### 1.2 The proposed widening

Add one field and one builder method to `AnalysisCtx`, mirroring `ddt_prev_rate_weight`
exactly:

```rust
// va-abi/src/analysis.rs
pub struct AnalysisCtx {
    // ...unchanged fields...
    pub ddt_coeff: f64,
    pub ddt_prev_rate_weight: f64,
    /// The weight this discretization puts on the charge value from *two* accepted steps
    /// back, for a method (only `Gear`, today) whose `ddt` reconstruction is a genuine
    /// three-point finite difference rather than a one-step rate recursion. `0.0` for every
    /// method that does not need it (backward Euler, trapezoidal) and in DC/AC/noise.
    pub ddt_prev2_weight: f64,   // NEW — defaults to 0.0 everywhere existing
}

impl AnalysisCtx {
    /// This context carrying the weight on the charge two accepted steps back — see
    /// `AnalysisCtx::ddt_prev2_weight`. Only `Method::Gear`'s driver calls this; every other
    /// caller leaves the field at its constructor default of `0.0`.
    pub const fn with_ddt_prev2(self, ddt_prev2_weight: f64) -> Self {
        AnalysisCtx { ddt_prev2_weight, ..self }
    }
}
```

The reconstruction generalizes to

```text
dq/dt = ddt_coeff·(q − q_prev) − ddt_prev_rate_weight·rate_prev + ddt_prev2_weight·(q_prev − q_prev2)
```

I checked this algebraically reduces to BDF2's exact three-point formula with
`ddt_prev_rate_weight = 0` and `ddt_prev2_weight = −r²/((1+r)·h)`: expanding gives
coefficient `a0 = (1+2r)/((1+r)h)` on `Q_n` (matches `ddt_coeff`), `(w2 − a0)` on `Q_{n-1}`
and `−w2` on `Q_{n-2}`; solving `w2 − a0 = −(1+r)/h` and `−w2 = r²/((1+r)h)` both give
`w2 = −r²/((1+r)h)` — consistent, so one new field suffices; a fourth term is not needed.

**Why additive, not a signature change.** `ModelInstance::unknown_kind` (§6, 2026-07-04,
`crates/va-abi/src/instance.rs` lines 50–53, `docs/interfaces.md` lines 254–264) established
the precedent this proposal follows exactly: a new **default-valued** field/method that every
existing implementor keeps compiling against unchanged, because `AnalysisCtx` is constructed
everywhere in the workspace through its constructor methods (`dc()`, `transient()`,
`with_ddt()`, …), never as a bare struct literal outside `va-abi` itself — I grepped for
`AnalysisCtx {` across every crate and found zero external struct-literal construction sites,
which is exactly what makes a new field with a `0.0` default in every constructor safe. This
is unlike the 2026-08-05 change that added `ctx: &AnalysisCtx` to `ModelInstance::load` itself
(`docs/interfaces.md`'s note on why that one **did** break every implementor deliberately) —
that precedent does not apply here because nothing about *whether* a model reads `ddt` as a
value changes; only what it can reconstruct does.

`ModelState`'s per-instance storage does **not** need an Interface β change: `state_len()` is
already an instance-declared `usize` with no fixed per-`StatefulKind` width baked into
`va-abi`. Widening a `ddt` call site from 2 state slots to 3 (`q_prev`, `rate_prev`,
`q_prev2`) is entirely `va-codegen`'s own bookkeeping in `StatefulKind::Ddt::width()`
(`crates/va-codegen/src/lower.rs` line 708–714) — it changes what a *newly compiled* model
asks for, not the shape of the channel it asks through.

---

## 2. Every downstream crate affected

Verified by grepping the workspace for `AnalysisCtx {`, `ddt_coeff`, `ddt_prev_rate_weight`,
`with_ddt`, `Method::`, and `Integration` (`crates/va-cli/src/lib.rs`).

| Crate | Needs edits? | Why |
|---|---|---|
| `va-ir` | **No** | Interface α is untouched — `Builtin::Ddt`'s arity and IR shape don't change; only the *runtime* reconstruction gains a term. |
| `va-abi` | **Yes** | `AnalysisCtx::ddt_prev2_weight` + `with_ddt_prev2` (§1.2); defaulted `0.0` in `dc()`/`transient()`/`ac()`/`noise()`. Additive — no existing caller breaks. |
| `va-codegen` | **Yes** | `ad.rs`'s `Builtin::Ddt` arm (lines 1032–1050) needs the third term and a third state read; `lower.rs`'s `StatefulKind::Ddt.width()` goes from 2 to 3 and its state-slot indices shift. This is the crate that actually reads `ddt_prev_rate_weight` today (confirmed: it is the only non-test call site) so it is the only consumer of the new field too. |
| `va-transient` | **Yes** | This is where the actual feature lives: `Companion::gear`, a second history vector (`q_prev2`) and step-ratio tracking in `run_with_events`, `method_order(Gear) = 2.0`, `divided_difference_history(Gear)`, `reference_method`/embedded-pair handling, and removing the `Method::Gear` early return in `run_with_events` (line 603–605). See §4 for staging. |
| `va-core` | **No** | `Companion`/`newton_step` are `va-transient`-local; `va-core`'s own DC Newton loop never sees a `Method` at all. |
| `va-acnoise` | **No** | `AnalysisCtx::ac()`/`::noise()` default the new field to `0.0` like every other field on those paths; AC linearization has no notion of a BDF2 step history. Confirmed no `Method`/`ddt_coeff` reference in `crates/va-acnoise/src/`. |
| `va-cli` | **Only if Gear is exposed at the CLI** | `Integration` (`crates/va-cli/src/lib.rs` lines 780–786) currently has exactly two variants, `Trapezoidal` and `BackwardEuler` — **`Gear` is not wired at the CLI layer at all today**, contrary to nothing in the background but worth stating plainly since it's a gap the background didn't mention: even after `va-transient::Method::Gear` works, `va-cli sim --integration gear` does not exist until `Integration` gains a variant and `crates/va-cli/src/lib.rs` line ~814's match arm gains `Integration::Gear => Method::Gear`. Small, but it's a real, separate PR-sized piece of the rollout. |
| `va-abi::reference` (hand-written models) | **No** | None of `Resistor`/`Capacitor`/`Diode`/`Bjt`/`VSource` reads `ddt` as a bare value — they all route through the charge channel, which is method-independent by construction (confirmed by the `Companion` doc comment: "Models that never evaluate a bare `ddt` value … are unaffected"). Zero of the reference models change. |
| Test-only `ModelInstance`s in `integrator.rs` | **Yes, to exercise it** | `BiasDependentRate` (lines 1032–1082) is the *only* thing in the repo that manually reads `ctx.ddt_coeff`/`ctx.ddt_prev_rate_weight` outside `va-codegen`. It is the natural vehicle for a Gear order-of-convergence test (§3) and would need a `ddt_prev2_weight`/third state slot arm added — this is test code, not a crate boundary, but it's real work. |

**Nothing in `models/*.va` or `circuits/*.net` needs to change** — no `.va` file in the zoo is
compiled specifically "for" a method today (`va_codegen::Integration`, the compile-time
method-tagging enum, was retired 2026-08-31 per `docs/roadmap.md`; models are method-agnostic
now that the trapezoidal first-step fix landed). A Gear-compiled model is just a model, evaluated
under different `AnalysisCtx` numbers.

---

## 3. The blast radius if this is done wrong

**Which models would be silently wrong.** Only compiled models whose Verilog-A source reads
`ddt(...)` as a value rather than a direct contribution — an `I(<port>)` re-read of a term
containing `ddt`, or a bias-dependent coefficient multiplying a `ddt` (`c(x)·ddt(q(x))`, the
product-rule case `docs/roadmap.md`'s 2026-08-31 entry describes at length). A get-it-wrong
scenario is concrete: forget the `ddt_prev2_weight` term entirely (i.e. ship Gear with
`AnalysisCtx` unchanged) and such a model's stamped **charge channel** would be exact BDF2
(§1.1 — that path needs no Interface β change and would look completely correct), while its
bare-`ddt`-value contributions would silently keep using the *old two-point* reconstruction —
mixing an exact BDF2 charge stamp with a stale, wrong-order rate reconstruction in the *same*
model, at the *same* row, in some cases. That is exactly the kind of defect
`docs/roadmap.md`'s 2026-08-31 audit found for the product-rule case ("two other `Dual`-
consuming paths still stamped `grad` only") — a partial channel wiring that degrades Newton
or silently drops order rather than failing to compile.

**Does the existing order-of-convergence gate catch it?**
`a_bias_dependent_rate_is_second_order_under_trapezoidal_on_varying_steps`
(`crates/va-transient/src/integrator.rs` lines 1153–1182) is built for exactly this failure
class — it deliberately forces a *varying* step schedule (uniform steps hide the trapezoidal
recursion bug entirely, per the module's own doc comment) and asserts an *observed order*
against a closed form, with backward Euler as a discriminating control. **It would not, as
written, catch a Gear defect**: it is hard-coded to compare `Method::BackwardEuler` against
`Method::Trapezoidal` (the `order()` closure at line 1154 takes a `Method` but the test body
only ever calls it with those two), and `Companion::for_method`/`divided_difference_history`/
`method_order` all `unreachable!()` on `Method::Gear` today. **A new, analogous test is
required, not an extension of trust in the existing one** — same closed-form device
(`BiasDependentRate`), same alternating-step forcing (`step_pattern`), but asserting Gear's
observed order is ~2 (not ~1, the way a wrong or half-wired reconstruction would silently
report, since a formula that's exact on the *charge* channel but stale on the *value* channel
would still show *some* convergence — just at the wrong rate, on the wrong term, and a coarse
absolute-error check would not distinguish "Gear order 2" from "Gear-charge/BE-value hybrid
order 1" without exactly this kind of controlled corpus device). The existing test should stay
as the BE/Trap regression gate; a sibling `..._under_gear...` test is the right shape,
following the same "observed order, not absolute error" discipline the doc comment on the
existing test explains (an absolute bound would silently pass a first-order run at a small
enough step).

---

## 4. A staged plan

**Stage 1 — `va-abi` + `va-codegen`, behavior-preserving.** Add `ddt_prev2_weight`/
`with_ddt_prev2` to `AnalysisCtx` (defaulted `0.0` everywhere); widen `ad.rs`'s `Builtin::Ddt`
arm to read the third term and `lower.rs`'s `StatefulKind::Ddt` to 3 slots. **Every existing
gate must be bit-identical** (mirrors `model-state.md` §7 step 2's "behavior-preserving" split,
and Tier A/B/C's own repeated pattern of landing the plumbing before the feature) — no caller
outside `va-transient` yet sets `ddt_prev2_weight` away from `0.0`, so nothing observable
changes. This stage alone is reviewable and low-risk.

**Stage 2 — `va-transient`: the charge/stamping path only.** `Companion::gear`, step-ratio `r`
tracking, a second history vector `q_prev2`, `method_order(Gear) = 2.0`,
`divided_difference_history(Gear)` (needs `order + 2 = 4` points, one more than trapezoidal),
`reference_method` for the embedded pair (candidate: `Trapezoidal`, since BDF2 and trapezoidal
are both second order and disagree meaningfully on stiffness — needs its own small study, not
assumed). Startup needs **two** prior points before Gear can run at all, so — generalizing the
existing "first step is always backward Euler" rule (lines 634–654, which exists for exactly
this class of startup problem) — the natural rule is: **step 1 backward Euler, step 2
trapezoidal or BDF1-consistent, step 3 onward Gear**, with the `ddt_prev2_weight` term left at
`0.0` until a second history point actually exists (mirroring how `is_initial_step` seeds
`ddt_coeff`'s recursion at `0.0` today). Remove the `Method::Gear` early return in
`run_with_events` (line 603–605) and the `gear_is_not_yet_implemented` test's assertion
direction flips to "Gear now integrates."

**What Stage 1+2 would NOT support, honestly:** any compiled model reading `ddt` as a bare
value under Gear stays wrong until Stage 3 — Stage 2 alone gives an internally consistent BDF2
for the resistor/capacitor/diode/BJT zoo (all charge-channel-only) but is a silent trap for
anything else, which is precisely why Stage 3 cannot be deferred as "future work" the way, say,
`absdelay` was in `model-state.md` §8 (that deferral was safe because nothing consumed the
missing mechanism yet; here, `va-codegen`-generated models that read bare `ddt` already exist
in the corpus per `docs/roadmap.md`'s product-rule entries).

**Stage 3 — the bare-`ddt`-value path + the discriminating test.** Wire `AnalysisCtx`'s new
field through `va-transient`'s Gear companion construction, add the sibling
order-of-convergence test (§3) with `BiasDependentRate` extended for a third state slot, run it
before claiming Gear "done" the way the BE/Trap gate already gates trapezoidal's own
first-step fix.

**Stage 4 — `va-cli` + docs.** `Integration::Gear` variant, CLI flag wiring, `docs/roadmap.md`/
`docs/validation.md`/`docs/interfaces.md` revision entries (mirroring the `unknown_kind`/
`model-state.md` revision-note convention), and a decision on whether any `circuits/*.net`
gate should be re-run under `--integration gear` for a documented (not necessarily golden —
QSPICE has no variable-step-BDF2 knob to compare against bit-for-bit) comparison.

Stages 1 and 2 are each independently green (compiles, all existing gates unchanged) before
the next begins — the same discipline `model-state.md` §7 used for its own state-channel
rollout.

---

## 5. Is it worth doing?

**What BDF2 buys that trapezoidal doesn't already have.** Trapezoidal is already second order,
so order is not the argument. The real difference is **stability class**: trapezoidal is
A-stable but not L-stable — for a sufficiently stiff mode (eigenvalue far into the left
half-plane relative to the step), its amplification factor approaches `−1` rather than `0`, so
a stiff transient can **ring** at the numerical (not physical) level, weakly damped rather than
critically damped. BDF2 is L-stable: stiff modes are damped to (near-)zero in one step,
regardless of step size. This is a textbook, well-established distinction (Gear's original
motivation for the method), not something I can validate empirically inside this repo without
first building it — which is exactly why this is a proposal and not a spike report.

**Which `circuits/` would actually exercise this.** I read every `.net` file in `circuits/`
(§ list in the directory listing I took: `rc_step`, `rc_discharge`, `rc_ac`, `rectifier`,
`rlc_ring`, `mos_dc`, `diode_*`, `ring_osc`, and the analysis-context/state-channel fixtures).
Two candidates stand out, and neither is a clean case:

- **`rlc_ring.net`** — an underdamped series RLC step (`ζ = 0.158`, per the deck's own
  comment) is the obvious "does it ring" circuit, but its ringing is **physical**, not
  numerical: at `ζ = 0.158` the circuit is genuinely supposed to oscillate, so BDF2's stronger
  damping would make it numerically *wrong* (over-damped relative to QSPICE) unless the step is
  already small relative to the ring period — the opposite of the case where L-stability
  matters. This circuit argues **against** using Gear as a default, not for it.
- **`rectifier.net`** — a half-wave diode rectifier is the more plausible candidate: the
  diode's sharp turn-on/off against the RC load's much slower time constant is a textbook
  stiff pairing, and `docs/roadmap.md`'s LTE-controller notes already record `nlcap_ramp.net`-
  class deck rejecting steps repeatedly near a fast transition under trapezoidal. Whether it
  actually shows trapezoidal ringing (rather than the adaptive controller simply shrinking `h`
  until it doesn't) is an open empirical question this proposal does not answer — that is
  exactly the kind of thing Stage 2 should measure before Stage 3 is justified as "worth it"
  rather than "implemented because the enum slot exists."

**Recommendation: build it, but treat Stage 2 as a checkpoint, not a foregone conclusion.**
The interface cost is small and additive (§1.2, §2) — this is not a "changing a frozen
interface lightly" case, it is the same low-risk shape as `unknown_kind`. The real cost is
Stage 3 (§4) and the honest risk in §3 (a partially-wired Gear is a worse outcome than no
Gear, because it fails silently rather than refusing to compile). Given that, the concrete
recommendation is: **do Stages 1–2, then measure `rectifier.net` (or a new, deliberately
stiffer diode-RC deck) under Gear vs. trapezoidal at the same `lte_reltol`, and only commit to
Stage 3 if that measurement shows a real accuracy-per-step-count win** — i.e. let the L-stability
argument be demonstrated on this codebase's own circuits before paying for the bare-`ddt`-value
generalization, rather than building the whole thing on the textbook argument alone. If the
measurement is unconvincing, Stages 1–2 are still useful (BDF2 becomes available as an
option, method-order study, and a stiffness diagnostic tool) even without Stage 3 ever landing.

---

## 6. What I checked that isn't spelled out above

- No `AnalysisCtx` is ever constructed via a bare struct literal outside `va-abi` (grepped the
  whole workspace) — confirms the additive-field claim in §1.2 is actually safe, not merely
  asserted.
- `va-cli`'s `Integration` enum (the CLI-facing method selector, distinct from
  `va_transient::Method`) has exactly two variants today and **does not mention `Gear` at
  all** — Gear is entirely unreachable from the CLI even once `va-transient` implements it,
  until `va-cli` gets its own Stage 4 edit. Worth flagging since the background didn't call
  this out.
- `docs/roadmap.md` (line ~3263) records a **prior, different** BDF2 experiment: a
  "recursion-free BDF2 alternative" tried as a fix for trapezoidal's own undamped-recursion
  startup defect (not as an implementation of `Method::Gear`), which "composes badly with the
  outer trapezoid offset and stays first order." That is a different problem (reconstructing
  trapezoidal's own rate more robustly) from this proposal (a genuine second, independent
  BDF2 stepper with its own companion), but it's adjacent enough — and was hard enough to get
  wrong — that it's worth reading before Stage 2 starts, as a warning that "BDF2-flavored"
  fixes in this integrator have already surprised someone once.
- `gear_is_not_yet_implemented` (`crates/va-transient/src/integrator.rs` lines 1330–1341) is
  the one existing test that would need its assertion direction inverted once Stage 2 lands —
  flagged here so it isn't mistaken for a regression when it starts failing on purpose.
