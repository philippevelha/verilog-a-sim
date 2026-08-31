# Roadmap

This is the phased plan for `verilog-a-sim`, broken down per thesis (T1–T6) plus the shared
kickoff. It complements — and does not replace — the standing rules in `CLAUDE.md`. Read it
alongside [`thesis-map.md`](thesis-map.md) (who owns what + fallbacks),
[`interfaces.md`](interfaces.md) (the two frozen contracts), and
[`validation.md`](validation.md) (metrics + the bring-up ladder).

Three things are true of every phase below:

1. **A phase is "done" only when its validation gate is green.** For analysis-producing
   crates that means `va-harness` passes against committed `golden/` to the stated tolerance;
   for compiler/IR crates it means the unit + finite-difference tests pass and the workspace
   builds clean (`fmt`, `clippy -D warnings`, `test`).
2. **Every phase ships a Quarto tutorial.** See [Quarto tutorials](#quarto-tutorials) below.
   The tutorial is a deliverable of the phase, not an afterthought — a phase with green tests
   but no tutorial is not finished.
3. **Crate boundaries are thesis boundaries.** Cross-crate needs go through a coordinated
   interface change (`CLAUDE.md` §6), never a solo edit of someone else's crate.

The phases are sequenced against the **bring-up ladder** (`validation.md`): resistor divider
→ diode I–V → RC transient → diode rectifier → MOS DC → ring oscillator. Each rung is a
shared, demoable milestone that several theses light up at once.

---

## Status at a glance

> **Fully refreshed 2026-08-04** — every row below was re-derived from a real run that day, not
> carried forward: `cargo test --workspace` (500 passed, 0 failed, 0 ignored; **516 after T5.6**
> landed later the same day, **520 after T5.7** on 2026-08-05), `cargo fmt --check` + `cargo
> clippy --workspace --all-targets -- -D warnings` (both clean), `cargo xtask validate`
> (**11/11 circuits pass vs committed QSPICE golden, convergence 11/11 = 100%** at the time of
> the refresh; **12/12** after T5.6 added `resistor_noise_table` later the same day, **13/13**
> after T5.7 added `resistor_noise_table_log`; **15/15** as of 2026-08-30), `va-cli check
> external` (**114/150** files pass the frontend — a figure since retired as unsound, see the
> metric-honesty entry below; **86/88 self-contained module-declaring files** as of
> 2026-08-30, on a corpus grown to 158 files — see the port-qualifier entry), and a one-off
> frontend+codegen scan of the same corpus (**104/150** build into a `ModelInstance`; **107/150**
> since 2026-08-29 — both superseded by **85/88** as of 2026-08-31, and note that the figure is
> now re-derivable with `va-cli check external --codegen` rather than a hand-written scan). The
> previous revision of this table dated from 2026-07-18 and had gone stale in three ways, all
> corrected here: T2.2's corpus figure predated the corpus growing from 115 to 150 files; the
> "no harness-vs-golden validation yet" and "no Quarto tutorials written yet" caveats had both
> been false since 2026-07-18; and T6.4 was missing from the table entirely. Legend:
> **✅ Complete** — code, tests, the validation gate, *and* the tutorial are all green.
> **🟢 Code complete** — implementation + unit/FD tests committed and green (`fmt`,
> `clippy -D warnings`, `test` clean), but at least one of {harness-vs-golden gate, Quarto
> tutorial} is still outstanding, so the phase is **not yet "done"** by criteria 1–2 above.
> **⬜ Not started.**

| Phase | What exists | Status |
|-------|-------------|--------|
| 0 — shared contracts | `va-ir`/`va-abi` frozen; resistor/capacitor/diode reference models pass stamp tests; bridge specs in `docs/bridges/` | ✅ |
| T1.1 — lexing | `logos` lexer; 20 tests. **Gate green as originally scoped** (a fixed subset); `CLAUDE.md` §1 has since widened T1's target to the *full* Verilog-A token set, tracked live in `token-reference.md` + the corpus scan, not by this row | 🟢 |
| T1.2 — parsing | recursive-descent parser + arena AST; precedence/associativity. Same "gate green as scoped, target since widened" caveat as T1.1 | 🟢 |
| T1.3 — elaboration | AST → `va_ir::Module`; **86/88 self-contained, module-declaring corpus files pass the full frontend** (2026-08-30, corpus 158 files; the old headline "114/150" counted 14 files declaring no module and 18 whose body an unresolved `` `include `` had deleted — see the metric-honesty entry). The phase's literal gate — the three zoo models elaborating to committed golden IR — closed 2026-08-30 (`crates/va-frontend/tests/golden_ir.rs`) | ✅ |
| T2.1 — AD core | forward-mode dual numbers over the IR arena; every differentiated operator FD-checked (§5) | ✅ |
| T2.2 — lowering | IR → `ModelInstance` incl. local-variable assignments, `if`/`else`, potential contributions (incl. mixed flow/potential), loops, `case`, user-defined analog functions, and parameter-scaled `ddt` (incl. through local-variable coefficients, and — since 2026-08-29 — a coefficient distributed over a parenthesised *sum* of `ddt`s); **85/88 self-contained, module-declaring corpus files pass frontend+codegen** (`va-cli check external --codegen`, 2026-08-30 on a 158-file corpus; was 75/108 on 2026-08-29, whose raw form was 107/150, up from 104/150 on 2026-08-04, which itself superseded the old 50/115 measured before the corpus grew to 150 files). The **one** remaining codegen failure is a bug in the corpus file itself (`amp_dynamic.va` declares `parameter real gain` and `real gain` in the same scope) — not a recognizer gap; the four product-rule files closed 2026-08-30 and `hicumL0_v2p1p0` on 2026-08-31 | 🟢 |
| T2.3 — charge channel | `ddt` terms routed to the charge channel (capacitor); broad coverage ongoing | 🟢 |
| T3.1 — MNA & dense solve (staff-maintained, not a thesis — see T3 section) | `assemble` + `faer` LU solve with singularity detection | ✅ |
| T3.2 — Newton & divider (staff-maintained, not a thesis) | Newton loop; resistor divider solves to the analytic midpoint; **ladder rung 1 passes vs QSPICE golden** (`divider` 0.0e0) | ✅ |
| T3.3 — nonlinear DC & sweep (staff-maintained, not a thesis) | diode–resistor clamp converges; DC `sweep`; `convergence` aids wired into `newton::solve`; **rungs 2/5 pass vs golden** (`diode_iv` 6.7e-5, `diode_clamp` 6.4e-5, `mos_dc` 1.5e-6) | ✅ |
| T4.1 — integration (fixed-step superseded by T4.2) | backward Euler + trapezoidal companion model; **rung 3 passes vs golden** (`rc_step` 1.8e-5) | ✅ |
| T4.2 — adaptive timestep & LTE | divided-difference LTE estimate drives accept/reject + grow/shrink (the embedded pair remains available, and covers the opening steps); a `SIN` source reads `ctx.time` off Interface β's analysis context (was `run_dynamic`, deleted 2026-08-06); **rung 4 passes vs golden** (`rectifier` 8.2e-4, re-validated 2026-08-31 under divided differences; 6.8e-4 under the embedded pair) | ✅ |
| T4.3 — events & breakpoints | `EventQueue` wired into `run_with_events`: forced exact landings, interpolated crossing detection; **rung 6 passes vs golden** (`ring_osc` 4.5e-6 since the 2026-08-31 first-step fix; 1.8e-4 before it) — the "harness gate blocked" note in T4.3's own section was resolved 2026-07-09 by adding `va-abi::reference::Bjt` | ✅ |
| T6.1 — netlist parser | R/C/L/D/M/Q/V elements (`L` and a `C`'s SPICE `IC=<volts>` initial condition both 2026-08-31) (`M`/`Q` = 3-terminal model-referencing devices, § rungs 5/6), dot-cards incl. `.tran` timing, `.dc <source> <start> <stop> <step>` sweep, `.ac dec <ppd> <fstart> <fstop>` + a `V` line's `AC <mag> [phase]` (T5), and `.noise V(<out>) <src> dec …` (T5.2); `va_ir::Discipline` unaware, SPICE-flavored `.net` format | ✅ |
| T6.2 — CLI wiring (DC + sweep + transient + AC + noise) | `va-cli sim` drives a DC operating point, a `.dc` sweep, `.tran` (incl. `SIN`-sourced circuits like the rectifier), `--ac` small-signal sweeps, and `--noise` spectra through the real pipeline; every one of the 23 golden gates runs through this path | ✅ |
| T6.3 — validation harness | `va-harness::metrics`/`golden::{GoldenDc, GoldenSweep, GoldenTran}`/`dc::{run_dc, compare_dc, run_dc_sweep, compare_dc_sweep}`/`tran::{run_tran, compare_tran}`; `xtask validate`/`gen-golden` real and wired; **all six ladder rungs formally passed** against committed, real QSPICE golden (rungs 2/5 via a hand-translated `.model` card; rungs 3/4 via that plus a `UIC` cold-start fix; rung 6 via that plus a `gnd`-aliasing bug fix and an honest early-window comparison) — see this file's T6.3 section. The zoo has grown to **23 gated circuits, all green** (2026-08-31; it was 13 on 2026-08-04 — the six ladder rungs plus T5's noise/AC set, since joined by the initial-condition, inductor, parameter-override, controlled-source and AC-sweep-type circuits listed in `docs/validation.md`) | ✅ |
| T6.4 — convergence dashboard | `xtask::Tally` separates `skipped`/`not_converged`/`failed`, and `try_solve` records a non-converging circuit instead of aborting the run — so `CLAUDE.md` §7's fourth metric is computable from a real run for the first time; `validate` prints **convergence 23/23 (100.0%)** (13/13 when the dashboard was built); `t6-integration/04-convergence-dashboard.qmd` | ✅ |
| T5.1 — AC linearization | `ac::{linearize, run}`: `(G+jωC)` complex solve via a real 2n×2n block embedding + `va-core`'s dense LU; **golden gate closed 2026-08-01** — `.ac` card/`AC` source parsing, `va-cli::solve_ac`, `GoldenAc` + separate magnitude/phase verdicts, complex `.qraw` parsing, and both AC circuits green vs real QSPICE (`rc_ac` 1.3e-15, `diode_ac` 1.3e-5) | ✅ |
| T5.2 — noise analysis | adjoint output-noise PSD (one `Aᵀy = e_out` solve per frequency gives every source's transfer impedance); Interface β gained a §6 noise channel (`NoiseSink` + `ModelInstance::noise`) since noise is physics the Jacobian doesn't carry; thermal/shot sources on `Resistor`/`Diode`/`Bjt`; validated vs closed form (`4kTR`), vs an RC-shaped spectrum, and vs real QSPICE golden (`diode_noise` 1.7e-5) | ✅ |
| T5.3 — Verilog-A noise lowering (T1/T2) | `white_noise`/`flicker_noise` lower to real IR calls instead of folding to `0`; codegen splits them into the noise channel like `ddt`→charge, leaving `load` bit-identical; Interface β gained a §6 flicker channel; compiled-model gates vs QSPICE (`resistor_noise_va` exact, `diode_flicker` 1.7e-5 over a 209×-shaped spectrum) | ✅ |
| T5.4 — input-referred noise | `S_in = S_out/|H|²`, with `H = y_k` read straight out of the adjoint vector at the input source's branch row — no second solve; golden format gained a second column, scored separately from the output one; verified against QSPICE's `inoise_spectrum` and its printed 22.3055 µV total | ✅ |
| T5.5 — per-device noise attribution | one column per contributing device, matching QSPICE's `onoise_<dev>`; identity is **positional** (instances are polled in order and tagged), so two identical parallel devices stay distinguishable where a `(p,n)` grouping could not; each column scored against its own peak, which made the gate stricter (`diode_noise` 1.7e-5 → 2.6e-5) | ✅ |
| T5.6 — `noise_table()` lowering | the last of Verilog-A's three noise builtins (see T5.7 for its log-interpolated twin): Interface α gained `Builtin::NoiseTable` (the table flattened into sorted, const-folded `Const` arguments) and Interface β `NoiseSink::table_current` + `TableInterp` (both LRM interpolation rules implemented, though only `noise_table`'s spelling is lexed); all table validation happens once at elaboration, where a source file can be named; gated by `resistor_noise_table` at 1.9e-16 vs a plain QSPICE resistor pair, with the interpolation/clamping rules discriminated by shaped-table unit tests instead (a flat table cannot tell them apart) | ✅ |
| T5.7 — `noise_table_log()` lowering | the LRM's other table function (§4.6.4.4, log-log interpolation): lexed, added to `RESERVED_WORDS` (where it had been missing — the LRM does reserve it), and lowered to its own `Builtin::NoiseTableLog`. **Interface α only** — no β revision, because T5.6 shipped `TableInterp::Log` a day earlier for exactly this; gated by `resistor_noise_table_log` at 1.9e-16, whose golden is deliberately identical to the linear deck's, since on a flat table both rules must agree | ✅ |

**The two caveats this section used to carry are both retired** (recorded here rather than
deleted, because they governed how every earlier revision of the table was read): "no
harness-vs-golden validation
yet" was true until 2026-07-17 and is now false — `va-harness`, `golden/`, and the CLI are real,
the oracle is QSPICE (not ngspice, switched in `f094bbe`), and 13 circuits are gated; "no Quarto
tutorials written yet" was true until 2026-07-18 and is now false — all 21 `.qmd` files under
`docs/tutorials/` are written, so criterion 2 no longer blocks any ✅.

**What still keeps every remaining "🟢" honest**, as of 2026-08-04:

1. **T1.1/T1.2 passed a gate that has since moved.** Both were scoped against a fixed
   Verilog-A *subset*; `CLAUDE.md` §1 now targets the full language. Their implementations are
   green against the original wording, but "done" for T1 is now measured by
   `docs/token-reference.md` and the corpus scan, which are explicitly still growing.
2. ~~**T1.3's literal gate is unmet.**~~ **Closed 2026-08-30** — `crates/va-frontend/tests/
   golden_ir.rs` commits a full `{:#?}` snapshot of the elaborated `va_ir::Module` for
   `resistor.va`/`capacitor.va`/`diode.va` and fails on any difference. T1.3 is ✅.
3. **Both remaining frontend failures are bugs in the corpus files, not recognizer gaps**
   (established 2026-08-31 by reading each one, not inferred from the error text). The 86/88
   headline is therefore 88/88 of the files that are actually valid Verilog-A:
   `verilogaLib-master/ctle.va` writes `V(out) <+ gain * laplace_zp(...)` but never declares
   `gain` anywhere — no parameter, no variable — so "unknown identifier `gain`" is the
   correct answer; and `verilogAlib/example_mzi_modulator.vams` connects `.therm_en(1)` in a
   port-connection list, but `therm_en` is a `parameter` of `photonic_waveguide` (line 50 of
   `photonic_waveguide.vams`), so it belongs in the `#(...)` override list — a port takes a
   net, never a value. This mirrors the single remaining *codegen* failure, `amp_dynamic.va`,
   which is also a file bug. Both diagnostics were sharpened rather than the parser widened
   (see the 2026-08-31 located-diagnostics entry below): rejecting these is right, so the work
   was making the rejection say why.

4. **T2.2/T2.3 are not corpus-complete.** 85 of the 88 self-contained, module-declaring corpus
   files build into a `ModelInstance` (2026-08-31). The frontend→codegen gap is **1 file**, and it is
   a bug in a corpus file rather than a recognizer gap (`amp_dynamic.va` declares `parameter real
   gain` and `real gain` in the same scope). T2.2's own generated-diode check is an operating
   point + FD rather than a full committed sweep.

No longer true and removed from this list: "T2/T3 run over hand-built IR / reference instances —
the frontend→codegen→core path is not yet wired by a netlist driver." `va-cli sim --model` has
driven that full path since T6.2, and six of the eleven gated circuits exercise it end to end
(`mos_dc`, `diode_iv`, `rectifier`, `diode_ac`, `resistor_noise_va`, `diode_flicker` — each
prints its own `[va-cli] compiled Verilog-A module …` line during `xtask validate`).

---

## Language coverage (T1 — full Verilog-A, not a subset)

Per the updated `CLAUDE.md` §1, `va-frontend` now targets the **complete Verilog-A language**
(LRM Annex C), not the previously-declared "single-module compact models" slice.
`docs/token-reference.md` is the living, token-by-token coverage record — this section is the
prioritized backlog against it.

**Corpus baseline.** Coverage work is re-derived by running `va-cli check` over real models,
not by guessing what's missing. Early passes under-sampled this — only
`external/verilogaLib-master/` (11 files) plus `external/ekv3.va` — which both overstated the
pass rate and missed real gaps those 12 files don't happen to exercise. The actual corpus is
the **whole `external/` tree, 150 `.va`/`.vams` files** as of 2026-08-04 (it was ~118 when this
paragraph was first written — the tree has grown since, so older counts below are against a
smaller denominator and are kept as history, not restated): real industry-standard compact
models (BSIM3/4/6/CMG/SOI/BULK, HiSIM/HiSIM-HV/SOI, HICUM L0/L2, PSP, EKV, VBIC, MOSVAR, JFET,
MVSG, ASM-HEMT, and more), plus their shared headers/macro-definition/nature-definition
fragments. A large minority are auxiliary include fragments (`*MacrosAndDefines*.va`,
`constants.vams`, `disciplines.vams`, `ekv3_*_def*.va`, …) never meant to compile standalone —
`va-cli check` naively tries anyway, so their "failures" are a scan artifact, not a language
gap; don't read the raw pass count as a language-completeness percentage without excluding
them. A second, distinct artifact category: top-level `.va` files whose module body was itself
split into a sibling `` `include ``d file that the corpus snapshot never shipped (the
PSP102/103/104 family, `L_UTSOI_102[_nqs]`, `r2_cmc`/`r2_et_cmc`) — these fail with a
misleading "port has no discipline declaration" (an empty module body, not a language gap) and
are excluded from the gap accounting below for the same reason as the fragments.

> **Re-measured 2026-08-04** (`va-cli check external`, plus a one-off frontend+codegen scan):
> **114/150 pass the frontend**, **104/150 also build into a `ModelInstance`**. Every one of
> the 36 frontend failures now falls into the two artifact categories above — there are **no
> remaining uncategorized frontend failures in the tracked corpus**:
>
> ⚠️ **Both numerators above are now known to be inflated and are kept only as history** — see
> "Corpus metric honesty (2026-08-29)" below. The categories in this list are correct; the
> `114`/`104` totals are not. Current figures: **86/88** (frontend) and **85/88** (frontend +
> codegen) on files that both declare a module and are self-contained (2026-08-30, corpus grown
> to 158 files — see the port-qualifier entry; the 2026-08-29 revision of this line said
> 82/108 and 75/108, of which the frontend half was itself off by one, see below).
>
> **Re-derived 2026-08-30** (`va-cli check external`, 158 files). Of the **18** frontend
> failures, **15 are truncated distributions**, now reported as such on both the pass and the
> failure side (see the metric-honesty entries below) — not language gaps:
>
> - **10 — "port `X` has no discipline declaration"**: the split-body family whose module body
>   lives in a sibling `` `include `` the snapshot never shipped (PSP103/104 and their `_nqs`
>   variants, `L_UTSOI_102[_nqs]`, `r2_cmc`/`r2_et_cmc`). The message names a port, but the
>   cause is an empty module body.
> - **5 — undefined macro** (`` `GMIN ``, `` `IPRoz ``, `` `MPInb ``, `` `P ``): the same defect
>   one stage earlier — the absent include also held the macro definitions the surviving text
>   uses (`diode_cmc`, `juncap200`, `psphv`, `psphvrr`, `r3_cmc`).
>
> **Only 2 failures are real, and both are bugs in the corpus file rather than here** (the third,
> `bsimsoi.va`, was closed on 2026-08-30 by real block scoping — see that entry):
>
> - `verilogaLib-master/ctle.va` — `unknown identifier `gain``: the file uses `gain` without
>   declaring it. (It separately needs array-variable arguments to the Laplace filters; see the
>   backlog.)
> - `verilogAlib/example_mzi_modulator.vams` — passes a *parameter* in a port-connection list
>   (`.therm_en(1)`; a numeric literal is not a legal analog port connection) and instantiates
>   modules from `photonic_primitives.vams` without `` `include ``ing it.

**Progress so far** (each closes a specific corpus failure or a gap `token-reference.md`
itself flagged): `genvar`/`generate` loops and vector nets (elaboration-time unrolling); the
three reserved-word gaps (`localparam`/`electrical`/`thermal`, `floor`/`ceil`/`round`/`int`/
`limexp`); `transition`, `slew`, `ac_stim`, `bound_step` (all fold soundly under v0's DC-only
model — see `token-reference.md` §1.5); `$abstime` (folds to `0.0`); vector net declarations
with both the shared-prefix and per-identifier-suffix range syntax
(`` electrical in[`W-1:0], out; ``); the full bitwise/shift operator family (`&`, `|`, `^`,
`^~`/`~^`, `~`, `<<`, `>>`) with correct Verilog operator-precedence, wired through `va-ir` and
`va-codegen`'s AD (zero-gradient, like the comparison operators); **array variables**
(`real out_val[0:15];`, `out_val[i]`) with a constant/genvar-indexed element resolution that
mirrors vector nets exactly (`token-reference.md` §2.2b); `real(expr)`/`integer(expr)`
type-cast *calls*, distinct from the declaration keywords of the same spelling
(`digital = integer(v * scale);`, real-to-integer rounding semantics, not `int()`'s truncation);
**vector ports** — `va_ir::Module::ports` reshaped from `Vec<NodeId>` to `Vec<Vec<NodeId>>`
(Interface α change, §6 — see `../bridges/interface-alpha-ir.md`'s 2026-07-02 revision), so a
port declared with a `[msb:lsb]` range resolves to all of its nodes instead of erroring;
`%` (modulus, `BinOp::Mod`, zero-gradient in AD like the bitwise family); `vt`/`temperature`
**un-reserved** again — real models very commonly declare a plain `vt` variable
(`external/igbt3.va`), and the bare word had no grammar production to justify reserving it in
the first place; `Temp`/`Pwr` recognized as the thermal discipline's access functions
alongside `V`/`I` (`disciplines.vams`'s standard names), fixing about a dozen files that
contribute to a `thermal` branch (`token-reference.md` §2.17); and **`ddx(expr, probe)`**, the
analog partial-derivative operator (LRM §4.5.13) — lowered to `Expr::Ddx` (Interface α change,
§6 — see `../bridges/interface-alpha-ir.md`'s 2026-07-02 revision) and evaluated in
`va-codegen` by reading the AD gradient component already carried at the probed node, exactly
as the LRM's own VCCS and diode worked examples require (both now regression tests, the latter
cross-checked against a central finite difference); confirmed needed by 10+ corpus files
(BSIM4/6/BULK, MVSG) and part of what moved the pass count from 34 to 44; and
**`$param_given(name)`/`$port_connected(name)`/`$mfactor`/`$limit`** — `$mfactor` (the
instance `m=` multiplicity factor) folds to its LRM default `1.0`; `$param_given`/
`$port_connected` fold to `false` (their argument is a bare parameter/port-name reference,
validated against the module's own declarations but never lowered as a value — v0's pipeline
has no netlist-driven instantiation, so no parameter is ever explicitly overridden and no
optional port is ever connected, making `false` the honest answer rather than an approximation);
`$limit(access, "fn_name", ...)` (a Newton convergence aid, LRM §4.5.14) folds transparently to
`access`'s value, since a converged solve is a fixed point of the *unlimited* equations and the
stateless `ModelInstance::load` ABI has no previous-iteration history to limit against regardless
(`token-reference.md`'s `SysFunc` entry). Part of what moved the pass count from 44 to 56
(BSIM6.1.1/bsimbulk*/asmhemt/asmhemt101_0/fbh_hbt-2_3 and others); and **`$simparam` folding
inside a parameter default**, not just the analog block — `const_eval` (the separate,
non-mutating evaluator behind parameter defaults/ranges/genvar bounds) gets the same
"fold to the `default` argument, or error if none" treatment `lower_expr` already had, fixing
`bsim6.0.va`/`bsimbulk.va`/`bsimbulk107.va` (`parameter real GMIN = $simparam("gmin", ...);`)
and moving the pass count from 56 to 59; and **runtime-indexed vector-net/array-variable
access** — `out[j]`/`out_val[j]` where `j` is a genuinely dynamic runtime value (an ordinary
loop variable, not a genvar or a constant). Turned out *not* to need the `va-ir` interface
change the previous pass had speculated: since `V(...)`/`I(...)` still ultimately resolve to a
fixed `BranchId`/`VarId` at elaboration, a runtime index instead expands into an
elaboration-time chain over every statically-known candidate index — a nested `Expr::Select` of
`Expr::Probe`s for a probe *read*, an if/else-if chain of `Stmt::Contribute`/`Stmt::Assign` for
a contribution *target*/array-variable *write* — guarded by an `index == k` equality check per
arm, which is sound precisely because the array/vector's range is always static even when the
selecting index isn't (`token-reference.md` §2.2b/§2.18). No `va-ir` change at all: both
`Expr::Select` and `Stmt::If` already existed. Closes the sole remaining blocker for both
`adc_16bit_ideal.va`/`dac_16bit_ideal.va`, moving the pass count from 59 to 61. **Module
instantiation** (LRM Annex C.8, `resistor r1(p, n);` / `divider #(.gain(2.0)) d1(.in(a),
.out(b));`) — previously the single biggest remaining "full Verilog-A" gap, now closed:
`va-frontend` parses every module a file defines and recursively elaborates+inlines an
`Item::Instance`'s referenced submodule into the instantiating module's own IR arenas, entirely
inside `va-frontend` — no `va-ir`/`va-codegen`/`va-core` change at all (`docs/interfaces.md`
records why). Scalar port connections only, no module-item-level `generate` around an instance
(no genvar-driven *array* of instances) yet — both stated v1 limits, not silent gaps. And
**discipline/nature declarations** — `discipline...enddiscipline`/`nature...endnature` (the
kind `` `include "disciplines.vams" `` expands to) are now genuinely parsed into a small
in-`va-frontend` table (`disciplines.rs`), instead of discarded as an opaque token span. This
widens the recognized access-function name set beyond the hardcoded `V`/`I`/`Temp`/`Pwr`
baseline — any access name a parsed discipline binds (e.g. `Q`, `Phi`, `MMF` from the real
corpus's magnetic/kinematic/rotational discipline families) is recognized too, additively, so
the baseline itself never regresses. (Net *declarations* under a custom discipline were a
stated v1 limit at the time — see the closed backlog item further down.) And
**`absdelay(value, delay[, max_delay])`** (LRM §4.5.9) — same DC-steady-state-fold family as
`transition`/`slew`/`$limit`: settles to its undelayed `value` with no delay history at a fixed
operating point, so it folds transparently at elaboration exactly like those (`delay`/
`max_delay` parsed, never evaluated). Closes `external/fbh_hbt-2_1.va`, moving the pass count
from 61 to 62. And **custom-discipline net declarations** (backlog item 4, below) — a net can
now be declared under any user-defined discipline, not just `electrical`/`thermal`:
`Parser::parse_item` checks a bare leading `Ident` against `self.disciplines` (populated by a
prior `discipline...enddiscipline` block) before falling back to the module-instantiation
reading, and both forms now share one `Parser::parse_net_item` helper; elaboration lowers a
custom discipline to the already-existing `va_ir::Discipline::Other`. Found not via the counted
118-file corpus (still none of those declare a net under a custom discipline, so the pass count
is unmoved by this) but via a locally-authored, not-yet-validated `optical`-discipline model
(`external/microring_modulator.va`, gitignored, not part of the corpus) that hit exactly the gap
item 4 predicted.

Three more gaps closed together, all found the same way — via a second locally-authored,
not-yet-validated library (`external/photonic/`, gitignored, not part of the tracked 118-file
corpus; a set of basic photonic building-block models) rather than the tracked corpus itself,
so none of these move the 62/118 count either:

- **Module-level/block-local `real`/`integer` inline initializers**, `real laser_freq =
  `P_C / wavelength / 1e-9;` — the LRM allows a name to carry either an array range or an `=
  expr` initializer, never both; only the range half was parsed before. `Parser::parse_var_entry`
  now looks for `= expr` when there's no range; elaboration lowers it to a `Stmt::Assign`
  (prepended to the analog block at module scope, emitted in place at block scope) — the same
  DC-only "runs where it's written" approximation `@(initial_step)` already uses. Closed 6 files
  in `external/photonic/` (`CwLaser.va`, `NoisyEDFA.va`, `Pcw.va`, `PcwPhaseModulator.va`,
  `PhaseModulator.va`, `Waveguide.va`).
- **Vector-net slices as instance port-connection arguments**, `CartesianMultiplier1(transfer,
  in[0:1], out[0:1]);` — connecting a `[msb:lsb]` sub-range of a wider vector net (or a whole
  bare vector net) to a same-width vector port. This also lifts the old "scalar port connections
  only (v1 scope limit)" restriction entirely: `Elaborator::resolve_conn_nodes` resolves a
  connection argument to its full ordered node list (one node for a scalar/single-index
  argument, the ascending-index-order list for a slice or bare vector name) and
  `bind_port_nodes` zips it element-wise against the submodule port's own node list — a scalar
  port is just the width-1 case of the same path now, not a separate one. Closed 3 files
  (`Attenuator.va`, `Isolator.va`, `PhaseShifter.va`) *for slice parsing/binding itself* — all
  three (plus 5 more `real`-initializer-fixed files) still fail for the next reason below, a
  distinct, newly-discovered gap.
- **Array-literal `{...}` expressions**, needed as `laplace_nd`'s coefficient-list arguments
  (this backlog's former item 1) — `{` and `}` are now lexed, `{expr, ...}` parses into a new
  `ExprAst::ArrayLit` (no `va-ir` change: it never survives past elaboration as a runtime value,
  so Interface α needed no §6 coordination). `laplace_nd(value, num, den)` is genuinely
  time-domain, but its DC (`s=0`) steady-state gain is exactly `num[0]/den[0]` — a constant
  scale factor on `value`, folded the same way `transition`/`absdelay` already fold to their
  input (§ this file's earlier discipline/nature entry). An array literal anywhere else is a
  clear elaboration error. Closed `external/photonic/PhotoDetector.va` outright;
  `TunableFilter.va` now hits the cross-file-instantiation gap below instead (same as the slice
  fix's 3 files) — the other 7 array-literal-consuming filter builtins (`laplace_np`/`zd`/`zp`,
  `zi_nd`/`np`/`zd`/`zp`) remain unimplemented, no corpus need found beyond `laplace_nd`.

That work surfaced one more, previously-unexercised gap: every instantiated module had to be a
sibling `module...endmodule` **in the same source file** — a submodule declared in a different
`.va` file was "unknown module" (`Elaborator::library` is built per-compilation-unit, i.e.
per-file, by `crate::compile`). Nothing in the tracked 118-file corpus happened to need
cross-file instantiation, but 9 of `external/photonic/`'s 31 files did (e.g. `Attenuator.va`'s
`Polar2Cartesian1` instance references `Polar2Cartesian`, declared in the sibling
`Polar2Cartesian.va`) — real Verilog-A practice, one module per file, is exactly this shape.

**Now closed, at the `va-cli` layer, not `va-frontend`**: `va_frontend::elaborate_with_library`
already took an arbitrary `library: &[ModuleAst]` — it never cared which file an entry came
from, so no frontend/Interface α change was needed at all. `check_models` (`crates/va-cli/src/
lib.rs`) now groups every file it's about to check by its own immediate parent directory
(`BTreeMap<PathBuf, Vec<_>>`), and the new `check_group` parses each file in a group individually
(still reporting that file's own read/preprocess/lex/parse failure on its own line) but
elaborates every module from every successfully-parsed file in the group against one *combined*
library. Grouping is deliberately scoped to "files sharing one directory," not "everything under
the top-level scanned root": several real corpus files at the same nesting depth directly under
`external/` declare a module with the same name (`hisimsoi_va`, `hicumL2va`, `mvsg_cmc`, `psphv`,
…, confirmed by `grep -h '^module ' external/*.va | sort | uniq -d`), so a directory-wide merge
across unrelated vendor releases would have risked an instantiation silently resolving against
the wrong same-named module; a folder someone actually put files into together is the one case
with an established intent to be used as one library. `external/photonic/` now passes 29/31 (up
from 20/31) — the remaining two are the expected header-only `disciplines.vams` and
`NoisyEDFA.va`, which hits a distinct, unrelated gap: an unrecognized system function,
`$rdist_normal` (a random-distribution noise source query), added to the backlog below.

**Also now closed**: `external/bsimsoi.va`'s `begin : load ... real ... MJSWG; ... end` — a
named block declaring a local variable that shares its name with a module-level parameter
(there, macro-declared via `` `MPRoo(MJSWG, ...)` ``) — used to fail elaboration with
"assignment to unknown variable `MJSWG`". Root cause: `Elaborator::register_var` (used to
auto-register a bare, declaration-less assignment target) treats "a same-named parameter
already exists" as "nothing to register" — a reasonable heuristic for its own weaker,
no-declaration-required convenience (assigning to an actual parameter is invalid Verilog-A, so
that case is never a real shadow), but it was also the *only* path `Stmt::VarDecl` (an
*explicit* `real`/`integer` declaration) used to register a name, silently applying the same
wrong heuristic there — an explicit declaration must always introduce a new identifier in its
block's scope, shadowing a same-named outer parameter, per ordinary nested-scope rules. Fixed
with a dedicated `declare_local_var` for the explicit-declaration path (no parameter check), plus
reordering `Ident` resolution to check `vars` before `params` (a local variable, once declared,
must shadow a same-named parameter for *reads* too, not just the initial assignment). Moved the
corpus from 105/150 to 106/150.

**Also now closed**: the other 7 Laplace/Z-domain filter builtins (`laplace_np`/`zd`/`zp`,
`zi_nd`/`np`/`zd`/`zp` — `laplace_nd` was already done). Implemented against the *normative* LRM
text (§4.5.11/§4.5.12 of `references/VAMS-LRM-2-4.pdf`, read via rendered page images after
`pdftotext`'s math-formula extraction proved ambiguous — worth knowing if this section is
revisited, since the garbled text alone would have produced a wrong formula), not memory: each
form settles to its DC (`s=0`, Laplace) or steady-state (`z=1`, Z-domain) gain the same way
`laplace_nd`/`transition`/`absdelay` already fold. Two helpers now back all 8: a `num`/`den`
polynomial-in-`s`/`z⁻¹` coefficient list contributes its `s⁰`/`z⁰` term for Laplace
(`array_lit_first`, unchanged) but the *sum of every* term for Z-domain (`array_lit_values`,
since `z⁻¹ = 1` at `z=1` for every power, not just the constant one); a `zero`/`pole` array
(flattened `(re, im)` root pairs) contributes a root-product term that is real-only and trivial
for Laplace (`laplace_root_product_at_origin`: `1.0` for any non-origin root regardless of it
being real or complex, `0.0` for a root exactly at the origin) but genuinely complex-valued for
Z-domain (`z_root_product_at_one`: `1 - root`, `1.0` for a root at the origin — note the origin
case's fold value differs *by domain*, `0` vs `1`, since `s=0` is the Laplace-plane origin a
root there coincides with, while `z=1` is a different point from the Z-plane origin `z=0`).
Validated against the LRM's own worked example (`laplace_zp('{-1,0}, '{-1,-1,-1,1})` → gain 1)
and hand-derived cases covering an origin zero (→ 0 gain), an origin pole (→ error), and a
complex-conjugate zero pair reducing to a real Z-domain gain — 11 new tests, all passing.
**Does not move the corpus count** (106/150, unchanged): of the 3 files in `external/`
referencing these builtins, `angelov.va`/`angelov_gan.va`'s `laplace_np` call sits inside a
permanently-disabled `` `ifdef HAVE_GRN_NOISE `` (never `` `define ``d — the whole block is
preprocessed away, so this was never live code to validate against), and
`verilogaLib-master/ctle.va`'s `laplace_zp` call — genuinely live — passes its zero/pole as
*array variables* (`wz`, `wp`, assigned element-by-element earlier in the analog block), not
literal `{...}` expressions; that's a new, separate, harder gap (below), and `ctle.va`
independently still has its own pre-existing bug (`gain` used but never declared anywhere in
the file — confirmed by inspection, not a frontend gap).

**Now closed** (three backlog items, resolved 2026-07-12): **`$rdist_normal` and friends** —
`$rdist_uniform`/`$rdist_normal`/`$rdist_exponential`/`$rdist_poisson`/`$rdist_chi_square`/
`$rdist_t`/`$rdist_erlang` (LRM §9.13.2's repeatable seeded random-distribution family, confirmed
against the normative grammar at `references/VAMS-LRM-2-4.pdf`, not memory) now fold to their own
distribution's *mean* in `Elaborator::fold_rdist` — `(start+end)/2` for `rdist_uniform` (the one
form with no single mean-bearing argument, built as a real IR `Add`/`Div` pair), the bare
`mean`/`degree_of_freedom` argument for every other form except `rdist_t` (`0.0`, the only
well-defined center for a distribution symmetric about zero) — a more honest DC operating point
than the arbitrary `0.0` the noise-source builtins (`white_noise`/`flicker_noise`/`noise_table`)
already use, though the underlying gap is the same: v0 has no simulator random-number generator
to actually draw a sample from. `seed` (always first) and an optional trailing `type_string`
(LRM Table 9-2) are parsed but never evaluated. Closes `external/photonic/NoisyEDFA.va` — moves
that directory from 29/31 to 30/31 (only the expected header-only `disciplines.vams` remains) and
the tracked corpus from 112/150 to 113/150. **`ground` declaration** — `Item::Ground`
(`Parser::parse_ground_item`) now parses `ground list_of_net_identifiers;` (LRM §3.6.4, Syntax
3-7); `Elaborator::collect_ground` resolves each named net (which must already be declared) and
aliases it to the module's global reference node — the *first* grounded net's own `NodeId`
becomes the reference node directly (so it keeps its real declared name instead of a synthetic
`"gnd"`), and any additional grounded net in the same module is merged into that same `NodeId`,
since every net a `ground` declaration names is electrically the same reference node per the LRM.
Runs right after `collect_nodes` and before anything that could lazily create the implicit
`"gnd"` node (`Elaborator::reference_node`, unchanged, now simply reusing whichever `NodeId` an
explicit `ground` declaration already claimed). No corpus file surveyed uses a `ground`
declaration, so this doesn't move the pass count — added because it's real, reserved LRM grammar
with a token already sitting unused, not because a corpus failure demanded it. **Escaped
identifiers** (`` \name ``, LRM §2.8.1) — a second `#[regex(...)]` on `Token::Ident` now matches
`` \[!-~]+ `` (backslash through the next whitespace), stripping the leading backslash in its
callback so `` \cpu3 `` lexes identically to the plain identifier `cpu3` (the LRM's own example)
— genuinely interchangeable from every later pass onward, since both produce the same
`Token::Ident`. Also doesn't move the pass count (no corpus file surveyed uses one); added for
the same "real reserved grammar, not a fragment artifact" reason as `ground` above.

**Now closed** (a fresh gap, not from the numbered backlog — found chasing `external/ekv3.va`
itself, resolved 2026-07-12): two distinct, previously-uncategorized blockers, both real language
gaps rather than the "missing companion file" artifact category most of this corpus's remaining
failures fall into. (1) **`` `include `` resolution now falls back to basename matching** — see
this doc's own `Directive(String)` entry's mirror in `docs/token-reference.md` for the full
account; in short, `external/ekv3.va`'s 15 `` `include "ekv3_include/*.va" `` directives named a
vendor subdirectory this corpus snapshot flattened away without rewriting the directives
themselves, so every macro those headers defined (`EXPL_THRESHOLD`, `MAX`/`MAXA`/`MINA`, …) came
back "undefined" even though the target files are still physically present, just directly under
`external/`. (2) **`electrical`/`thermal`/`ground` now also parse as an ordinary identifier**
wherever the grammar expects a bare name, not just at the start of their own declaration —
`external/ekv3_variables.va` (one of the files (1) unblocked) declares `real thermal;`, a plain
variable literally spelled `thermal`, later read/reassigned as a bare identifier throughout
`ekv3_noise.va`/`ekv3_oppoints.va`; the same "real word, real corpus, dedicated token" tension the
`vt`/`temperature` un-reservation (above) already resolved for two *non*-dedicated reserved
words, now extended to the three dedicated single-word declaration-starting tokens
(`Parser::ident_like_keyword`, `docs/token-reference.md`'s `Electrical`/`Thermal`/`Ground`
entries). Both were needed together to get `ekv3.va` itself past the frontend — fixing only one
would have still left it failing on the other. Moves the tracked corpus from 113/150 to 114/150
(`external/ekv3.va` itself; its 17 `ekv3_*.va` body/header fragments remain in the known
"never meant to compile standalone" scan-artifact bucket, now genuinely confirmed as such since
the file that actually `` `include ``s them all now passes).

**Backlog, prioritized** (highest-value/most-tractable first, re-derived against the full
118-file corpus):

1. **Array-variable arguments to Laplace/Z-domain filters** — every filter builtin above only
   accepts a literal `{...}` for its numerator/zero/denominator/pole argument
   (`array_lit_values` requires `ExprAst::ArrayLit`); `external/verilogaLib-master/ctle.va`
   instead declares `real wz[1:0], wp[3:0];` and assigns each element in the analog block
   (`wz[1] = -`M_TWO_PI * fz;`, …) before passing the whole array *variable* to `laplace_zp`.
   Supporting this needs a real capability this project doesn't have anywhere else: tracing a
   variable's value through its own (straight-line, unconditional) assignment statements at
   elaboration time — a small constant-propagation pass, not just an AST pattern match. Every
   other DC fold in this codebase only ever inspects the expression being evaluated, never other
   statements in the block.
2. **Time-history-dependent event functions** (`last_crossing`, real `cross`/`timer`/`edge`
   semantics) — cannot be soundly approximated at DC the way `transition`/`slew` can (their
   whole purpose is time history); `va-transient` now exists (T4 is code-complete), but that only
   supplies a time axis to *run* — nothing in Interface β lets a `ModelInstance::load` call see
   its own history (past crossing times, a running timer) at all, so this is still blocked on a
   design question, not just an engine being absent.
3. **`Elaborator::reference_node`'s hardcoded-electrical ground** — every single-terminal
   access's implicit "gnd" second terminal is hardcoded `Discipline::Electrical` regardless of
   the access's own discipline (e.g. a bare `Temp(dt)` still resolves against an
   electrical-tagged reference node); pre-existing, not introduced by the discipline/nature
   pass, and not fixable without per-access discipline tracking that doesn't exist even for
   electrical/thermal today. (Unaffected by the `ground` declaration closed above: an *explicit*
   `ground` statement aliases to whatever discipline the named net already has; this item is
   about the separate *implicit* single-terminal-access path's hardcoded discipline.)

**Permanently out of scope, not a backlog item** (LRM Annex C.7: "No digital behavior or
events are supported in Verilog-A" — these are excluded from Verilog-A *itself*, not narrowed
further by this project): gate/switch-level primitives (`and`/`nand`/`nmos`/`bufif0`/…), net
strength/charge-storage keywords (`strong0`/`trireg`/`highz0`/…), and digital procedural/timing
constructs (`always`/`initial`/`fork`/`join`/`task`/`wait`/`specify`/`casex`/`casez`/…). See
`token-reference.md` §1.6 for the full, word-by-word accounting.

**Now closed** (was "not chased, unclear if real" — resolved 2026-07-09): `IB = I(<b>);` in
`external/hicumL0_v2p0p0.va` and its 5 HICUM/L0 siblings turned out to be real, normative
Verilog-A grammar, not a broken/vendor-specific construct — confirmed directly against
`references/VAMS-LRM-2-4.pdf` (§3.12.1 "Port Branches", §5.4.3 "Accessing flow through a
port"): `port_probe_function_call ::= nature_access_function ( < analog_port_reference > )`.
`I(<a>)` accesses the current flowing *into the module* through port `a`, distinct from an
ordinary `I(a)` branch access; the LRM's own diode worked example uses exactly this idiom
(`if (I(<a>) > imax) $strobe(...)`). Two hard constraints, both enforced at parse time:
flow-only (`V(<port>)` is explicitly invalid) and read-only (never a contribution target).
Implemented entirely in `va-frontend` — no `va-ir`/`va-abi` change needed, mirroring the
runtime-indexed vector-net/array-variable fold above: `Elaborator::lower_port_probe` computes
the probed port's current as the signed sum of every flow contribution already made (elsewhere
in the same analog block) to a branch touching the port's node — `+value` where the port is a
branch's `p` terminal, `-value` where it's `n` (sign convention verified against the LRM's own
diode example: a forward-biased `branch(a,c)` contributes positive current from anode `a` to
cathode `c`, so current must be *supplied* into the module at `a`). A contribution found inside
an `if`/`else` is wrapped in a matching `Expr::Select` guard (so it only counts when the
condition holds, closing the exact HICUM idiom of a threshold-guarded series-resistance branch);
one found inside a `case`/`for`/`while`/`repeat` is rejected with a clear "not yet supported"
error rather than silently mis-summed or dropped — no corpus need for either has surfaced.
Vector ports are a stated v1 limitation (scalar only). Moved the corpus from 106/150 to
112/150 (the 6 HICUM/L0 files).

**Now closed** (was backlog item 5, "wiring parsed nature metadata into convergence" —
resolved 2026-07-09): a discipline's `abstol` now round-trips all the way from a parsed
`nature...endnature` block into `va-core`'s Newton convergence check for a real `va-cli sim`
run, not just into `disciplines.rs::NatureDecl` where it used to stop. Turned out to be a
four-hop gap, not one: (1) `Parser::natures`/`disciplines` never left `Parser` — `parse()`'s
public return type was `Vec<ModuleAst>` only, fixed with an additive `parse_with_disciplines`
(`parse` becomes a thin wrapper); (2) `Elaborator` had nowhere to receive them — fixed with
`elaborate_with_library_and_disciplines` (again additive; `elaborate`/`elaborate_with_library`
now thin wrappers passing empty tables, so a net with no resolvable metadata still gets
`abstol: None`, exactly the old behavior); (3) `va_ir::NodeDecl` had nowhere to carry a
resolved value — closed by an Interface α §6 change, `NodeDecl.abstol: Option<f64>`, sourced
from the node's discipline's **potential** nature (`disciplines::resolve_abstol`); (4)
`va_abi::ModelInstance` had no way to expose a per-unknown tolerance to `va-core` at all —
closed by an Interface β §6 change, `unknown_abstol`, a default trait method in the exact
shape of the 2026-07-04 `unknown_kind` addition. `va-codegen`'s generated models implement it
by reading their own `NodeDecl.abstol`; `va-core::mna::classify_abstol` collects it (mirroring
`classify_unknowns`); `newton::solve_from`'s per-unknown convergence check now consults it
instead of always using `NewtonConfig::abstol`. `va-cli` itself needed **no changes** — its
`--model <m.va>` flag already compiled a real `.va` file through `va-frontend` → `va-codegen`
and matched it against netlist devices by model name (`build_from_model`), so switching
`compile_with_includes` to the discipline-aware entry points was the entire integration.
Two stated v1 limits: no wiring for a discipline's *flow* nature (e.g. `Current`'s own
`abstol`) — only a `Node`-kind unknown has a natural `NodeDecl`-shaped home for one, a
branch-current unknown stays on the global default; and the separate `residual_norm <=
cfg.abstol` gate in `solve_from` stays a single global scalar (reweighting an `inf_norm` check
into a per-row form is a different design question). Also added `models/disciplines.vams` (a
minimal, self-written electrical-only header — not a copy of the ~700-line Accellera annex) so
the project's own bring-up model zoo, previously silently missing this `` `include ``, now
resolves a real `abstol` too. Doesn't move the corpus pass count (no tracked corpus file's DC
answer depends on convergence-aid tolerance, by design — this is a convergence-aid change, not
a modeling one, confirmed by a regression test asserting the divider's operating point is
bit-for-bit identical with and without `disciplines.vams` resolved).

**Corpus artifact, not a language gap** (found chasing what first looked like the discipline/
nature gap above): the PSP102/103/104 family, `L_UTSOI_102[_nqs]`, and `r2_cmc`/`r2_et_cmc` (8
files) each declare their module header, then `` `include `` a sibling file
(`PSP103_module.include`, `L_UTSOI_102_module.include`, `r2_cmc_body.include`, …) for the
*entire* body — every net/branch/analog-block statement lives there, not in the top-level `.va`
file. None of those sibling files exist anywhere in this `external/` snapshot (confirmed by
`find`), so the preprocessor's "unresolved include is skipped" behavior (correct — matches how a
real toolchain would report a missing file, not a parse error) leaves an effectively empty
module body. The elaborator then reports the first port it can't resolve as "no discipline
declaration," which reads exactly like a custom-discipline gap but isn't one — verified by
checking that no `discipline`/`nature` keyword appears anywhere in these 8 files at all. Nothing
to fix here; treat like the ~20 known auxiliary fragments.

---

## Quarto tutorials

Every student documents the features they build as [Quarto](https://quarto.org) tutorials, so
each person can **explain what they developed to everyone else** — supervisors, sibling
students, and future maintainers. The tutorials are the project's living, executable
documentation and the backbone of the recurring "show-and-tell" days.

### Layout

Tutorials live in a single Quarto project so they cross-link and render as one site/book:

```
docs/tutorials/
├── _quarto.yml              # project config: book or website, one part per thesis
├── index.qmd                # landing page: what the simulator is, how to read these
├── shared/                  # Phase 0: the two interfaces, the ABI, reference models
│   ├── 01-interfaces.qmd
│   └── 02-reference-models.qmd
├── t1-frontend/             # one part per thesis, one .qmd per phase/feature
│   ├── 01-lexing.qmd
│   ├── 02-parsing.qmd
│   └── 03-elaboration.qmd
├── t2-codegen/
├── t3-core/
├── t4-transient/
├── t5-acnoise/
└── t6-integration/
```

### Conventions

- **One tutorial per phase, named for the feature** (`02-newton.qmd`), not the date.
- **Executable, not just prose.** Prefer runnable code cells — a Rust snippet via a code
  block, or a shelled-out `cargo run -p va-cli -- …` whose output (a sweep, a waveform, a
  convergence trace) is captured and plotted in the document. A tutorial that cannot be
  re-run to reproduce its figures has rotted.
- **Plotting: `plotters`, not a Python/R plotting stack.** I–V curves, transient waveforms,
  and sim-vs-golden overlays are rendered with the `plotters` crate (SVG backend only — skip
  the bitmap backend, which pulls in font-rasterization deps for no benefit here) rather than
  shelling out to matplotlib/ggplot from the `.qmd`. This keeps the pure-Rust, no-native-deps
  posture (`CLAUDE.md` §5) intact end to end, including in the tutorials.
  **2026-07-06: built.** `va-cli`'s new `plot.rs` module (`plotters = { default-features =
  false, features = ["svg_backend", "line_series"] }` — confirmed zero native/`-sys`
  dependencies pulled in) draws every node's voltage over time as an SVG line chart; a
  `--plot <out.svg>` flag on `sim` wires it in, gated to transient runs only (a DC operating
  point is a single point, not a waveform — plotting one isn't implemented, and asking for it
  is a clear error rather than an empty/misleading image). Verified against the
  rectifier: `cargo run -p va-cli -- sim circuits/rectifier.net --tran --plot rectifier.svg`.
  **2026-08-31: both outstanding figure types closed, and embedded.** `plot_sweep` (`va-cli`)
  draws a `.dc` sweep's node voltages against the swept source's own value — `run_sim` now
  accepts `--plot` for `Analysis::Dc` when the netlist carries a `.dc` card, alongside the
  existing transient path; a bare operating point (no `.dc` card) is still refused with a clear
  error, not an empty image. `va_harness::plot::{overlay_sweep, overlay_tran}` draw a freshly
  solved sim trace over its committed `golden/` reference on one shared axis (golden as grey
  dots underneath, sim as a solid line on top; a transient overlay resamples golden onto the
  simulated timebase first, via the same `resample_linear` `compare_tran` scores against, so the
  picture and the pass/fail number describe the same comparison) — new module, unit-tested
  (`cargo test -p va-harness --lib`), not yet wired into `xtask validate` itself (regenerating a
  tutorial's figure is a separate, explicit step from checking a golden gate; see
  `crates/va-harness/examples/gen_figures.rs`). Both are now embedded as plain markdown images:
  the diode I–V sweep in `t3-core/03-nonlinear-dc.qmd` (honestly captioned — `plot_sweep` draws
  only node voltages, so this particular circuit's figure is a straight `V(in)=V1` line, not the
  diode's *I–V* law) and the rectifier sim-vs-golden overlay in `t6-integration/03-validation.qmd`
  (chosen because rung 4's margin — `6.766e-4` then, `8.226e-4` since the 2026-08-31 estimator
  switch — is the tightest of the six — the rung most worth
  seeing, not just reading as a number). Each embed states its exact regeneration command per
  this section's own "executable, not just prose" rule.
  **2026-08-31: and noise, which completes the set.** Every analysis this simulator performs
  can now be drawn: transient waveform, `.dc` sweep, AC Bode pair, and a log-log noise
  spectrum (`va_cli::plot::plot_noise`, output- and input-referred on one pair of axes).
  Log-log is the substance rather than the styling: flicker noise is a straight line of slope
  -1 per decade and thermal noise is flat, so the knee between them is the entire content of
  the figure, and linear axes collapse both into a spike at the left edge. `V^2/Hz` is plotted
  rather than `V/sqrt(Hz)` so the picture carries the same quantity the golden files and
  QSPICE's `onoise_spectrum` do. Points a log axis cannot show are skipped — a zero PSD, or
  the infinity `input_psd` reports where the input cannot reach the output — and a spectrum
  with nothing left is an error, not a blank canvas. Embedded in `t5-acnoise/02-noise.qmd`.

  **2026-08-31 (later still): `--plot` learned AC.** An AC sweep now draws as a Bode pair
  (`va_cli::plot::plot_ac`): magnitude in dB above phase in degrees, sharing a logarithmic
  frequency axis. Two panels rather than one for the same reason branch currents were kept off
  a voltage axis earlier the same day — decibels and degrees share neither units nor a useful
  scale, and forcing them onto one axis makes the flatter of the two unreadable. `t5-acnoise/
  01-ac.qmd` embeds `rc_ac.net`'s response, where `V(in)` is flat at 0 dB/0° (it is the 1 V
  source, i.e. the reference the other curve is read against) and `V(out)` shows the
  -20 dB/decade rolloff and 0° → -90° phase swing of the single pole. A frequency point at or
  below zero cannot appear on a log axis and is skipped; a sweep with nothing left is an error
  rather than a blank canvas, which is the same contract the transient and sweep plots keep.

  **2026-08-31 (later): `overlay_sweep` put to work, and a units bug found by using it.** The
  DC-sweep overlay had been written and unit-tested but drawn by nothing, because until
  `circuits/diode_clamp.net` existed no gated `.dc` circuit had a node voltage worth looking at.
  It now draws that clamp curve against its QSPICE golden in `t6-integration/03-validation.qmd`,
  from the same `cargo run -p va-harness --example gen_figures` as the rectifier overlay.
  Generating it exposed a real defect in *both* overlays: they plotted every column of a golden
  file's `node_order`, which since 2026-07-18 includes branch currents — amps, drawn against an
  axis labelled "Voltage (V)", where a milliamp trace flatlines along the bottom under a legend
  calling it a voltage. `render` now draws node-voltage columns only, matching the rule
  `va_cli::plot::plot_sweep` already stated for itself; the gate still scores every column, so
  nothing is checked less, only drawn honestly. Pinned by a test whose current column is 1000x
  the voltages, so a regression that re-includes it would visibly rescale the axis.
- **Standard skeleton** for each tutorial: *Goal* (one sentence) → *Where it fits* (the §2
  pipeline diagram, the relevant box highlighted) → *The idea* (theory, the equations, the
  design choice) → *The code* (the public API the student built, with the doc-comment
  caveats surfaced) → *It works* (the test or `va-harness` result that proves it, including a
  plot vs ngspice golden where applicable) → *Limitations* (stated honestly, per `CLAUDE.md`
  §5) → *What's next*.
- **Render in CI / `xtask`.** Add a `cargo xtask tutorials` (or a `quarto render`
  invocation) so the site builds reproducibly; a broken tutorial fails like a broken test.
- **Tutorial day cadence.** At the end of each ladder rung, every student presents their new
  tutorial(s) to the group. This is the integration heartbeat — it surfaces interface
  friction early, while it is still cheap to fix.

---

## Phase 0 — Kickoff & shared contracts (everyone)

> The whole multi-author build hinges on this happening first. Nothing else is safe to start
> until the two interfaces are ratified and frozen (`CLAUDE.md` §10).

**Goal:** ratify and freeze Interface α (`va-ir`) and Interface β (`va-abi`); ship working
reference models so `va-core` has something real to solve from commit #1.

**Steps**

- Hold the interface-ratification meeting. Walk through §4 of `CLAUDE.md` line by line; agree
  on the IR shape and the `ModelInstance`/`StampSink` ABI. Record decisions in
  `interfaces.md`.
- Lock `va-ir` types (arena/index representation — `CLAUDE.md` §5) and `va-abi` traits.
- Verify the hand-written `resistor`, `capacitor`, `diode` reference models implement
  `ModelInstance` and pass their stamp unit tests.
- Stand up the Quarto project skeleton (`docs/tutorials/_quarto.yml`, `index.qmd`).

**Validation gate:** workspace builds green; `va-abi` reference-model tests pass; `interfaces.md`
matches the code verbatim.

**Quarto tutorials**

- `shared/01-interfaces.qmd` — the two contracts, why they are frozen, how a coordinated
  change works (§6).
- `shared/02-reference-models.qmd` — walk the resistor/capacitor/diode stamps by hand; this
  is the Rosetta Stone every other thesis refers back to.

---

## T1 — `va-frontend` (lexer · parser · AST · elaboration → `va-ir`)

**Fallback (thesis-map):** a rigorous Verilog-A subset grammar + parser study.

### Phase T1.1 — Lexing & the grammar subset
> **Status: 🟢 code complete** — `logos` lexer in `va-frontend/src/lexer.rs`; tokens, `<+`,
> numeric literals with scientific notation + SI suffixes, `$`-system funcs, directives,
> comments. Subset documented in the module header (no separate grammar file yet). 20 tests
> (up from 8, growing alongside the reserved-word/escape-handling backlog closures below).
> `t1-frontend/01-lexing.qmd` written 2026-07-18.
>
> **String-literal escapes now resolve.** The naive `"[^"]*"` string regex broke on any literal
> containing an escaped quote (`bsimsoi.va`'s error-message string embedding `\"`` `define
> ...\"``) — the coarse match stopped at the *inner* `"`, leaving a stray `\` that failed to
> lex at all, taking down every token after it in the file. The regex now allows `\\.` pairs
> (`"([^"\\]|\\.)*"`), and `parse_string` resolves the LRM's quoted-string escapes (`\\`, `\"`,
> `\n`, `\t`, `\v`, `\f`, `\a`, `\%`, and up to three octal digits `\ddd`), permissively keeping
> an unrecognized escape's literal character (dropping just the backslash) rather than erroring
> — sound here since this project never executes `$display`-style output.

- Define the supported Verilog-A subset precisely (tokens, keywords, operators). Write it
  down as a grammar before writing code.
- Implement the lexer (optionally `logos`); property/round-trip tests on token streams.
- **Tutorial:** `t1-frontend/01-lexing.qmd` — the subset grammar + tokenization, with the
  "what we deliberately do *not* support" section.

### Phase T1.2 — Parsing to an AST
> **Status: 🟢 code complete** — recursive-descent parser + arena AST in
> `va-frontend/src/{parser,ast}.rs`; precedence-climbing expressions (correct `*`/`+`
> precedence, right-associative `**`). Returns `FrontendError::Parse` (no panics). 59 tests
> (up from 6, growing alongside module instantiation/generate loops/vector nets below).
> `t1-frontend/02-parsing.qmd` written 2026-07-18.
>
> **Two real-corpus parser gaps closed.** (1) The empty statement — a bare `;`, legal wherever a
> statement is expected (LRM) — now parses as a no-op (`Stmt::Block(vec![])`);
> `mvsg_cmc_3.2.0.va`'s `if ($port_connected(dt) == 0);` uses one as an `if`'s entire body,
> deliberately doing nothing when its optional thermal port is left unconnected. (2) A source
> file that defines **zero** modules is no longer a parse error — real corpus headers
> (`generalMacrosAndDefines.va`, `simulatorFlags.va`, `cmcGeneralMacrosAndDefines.va`, and
> others) exist purely to be `` `include ``d by an actual device file, carrying nothing but
> `` `define ``s; the LRM never requires a module in a compilation unit, so `parse` now returns
> `Ok(vec![])` for one instead of erroring. Re-scanned the full external corpus (115 files):
> **72/115 pass frontend+codegen, up from 62** (+10, all previously "expected at least one
> `module`" failures — the entire macro-only-header bucket closed in one shot).
>
> **2026-08-31: parse errors carry a line, a column, and the offending line's text.** Every
> `FrontendError::Parse` used to open `at token 733:` — a token index into a stream the reader
> cannot see, useless for finding the mistake in a 3000-line vendor model. `lexer::lex_spanned`
> now returns each token's start offset alongside the token, and
> `parser::parse_with_disciplines_located` turns the failing index into `at preprocessed line
> 471, column 19 (`.therm_en(1)`)`. Both additions are strictly additive: `lex`/`parse` keep
> their old signatures and fall back to the token-index wording, so no existing caller changed
> behavior — `compile_with_includes` and `va-cli check` opt in.
>
> The number is deliberately labelled **preprocessed**, because that is what it counts: the
> pipeline lexes the expanded text, so a file whose includes resolved (or were silently
> skipped) is renumbered relative to its own source — `example_mzi_modulator.vams`'s failure
> is its line 120 but the expansion's line 471. Quoting the offending line's *text* is what
> makes the diagnostic usable across that drift: the quote is exact and greppable in the
> original file no matter how far the count has moved. Mapping expanded lines back to original
> ones needs the preprocessor to keep a line map, which it does not — recorded here as the
> honest limitation rather than papered over with a number that might be wrong.
>
> One diagnostic was also made specific rather than generic: a **value in a port-connection
> list** (`.therm_en(1)`) now says a port connects to a net and points at the `#(...)`
> parameter-override list, instead of `expect_ident`'s "expected an identifier". That is the
> real mistake in the one corpus file it rejects — `therm_en` is a `parameter` of the
> instantiated module — and the parser is right to refuse it, so the fix was in the wording,
> not the grammar. Four tests cover the machinery: the located form, the token-index fallback,
> the port-connection message, and the long-line cap.

- Recursive-descent (or chosen) parser → AST for module headers, ports, params with ranges,
  the analog block, `<+`, `if/else`, analog function calls.
- Error handling returns `Result` with `thiserror` enums (never panics — §5).
- **Tutorial:** `t1-frontend/02-parsing.qmd` — AST shape, parsing strategy, error reporting.

### Phase T1.3 — Elaboration → `va-ir`
> **Status: ✅ complete** (2026-08-30) — `va-frontend/src/elaborate.rs` lowers AST →
> `va_ir::Module`: nets→`NodeId`, const-eval'd params + ranges, branch accesses→`BranchId`,
> builtins→`Builtin`. All three zoo models elaborate end-to-end, and **the validation gate is
> now met literally**: `crates/va-frontend/tests/golden_ir.rs` compares each against a
> committed snapshot of the whole elaborated module.
> `t1-frontend/03-elaboration.qmd` written 2026-07-18 (120 tests by then, up from 6; 114/150
> real corpus files pass the full frontend).

- Resolve names/params, flatten to the arena IR (`Module`, `Expr`, `Stmt`), validate
  parameter ranges, lower `ddt`/`idt`/built-ins into IR `Call`s.
- Golden-IR tests: source in, expected `va-ir` out, for `resistor.va`, `capacitor.va`,
  `diode.va`.
- **Validation gate:** the three zoo models elaborate to IR that matches committed golden IR.
- **Tutorial:** `t1-frontend/03-elaboration.qmd` — from text to Interface α, end to end on
  the diode model.

---

## T2 — `va-codegen` (IR → automatic differentiation → model instances)

**Highest-risk, highest-value crate — strongest student (§10).**
**Fallback:** an AD-for-compact-models report (forward vs reverse, FD validation).

### Phase T2.1 — Evaluator & dual-number AD core
> **Status: ✅ complete** (marker refreshed 2026-08-04: the FD gate and
> `t2-codegen/01-ad-core.qmd` are both green, which is this phase's whole bar) — `va-codegen/src/ad.rs`: forward-mode `Dual` over the IR
> arena (`+ - * / neg`, `exp/ln/log10/sqrt/abs`, variable-exponent `pow`) with an eval `Ctx`.
> Each operator is FD-checked (`div_matches_finite_difference`, `exp_chain_rule`).
> `t2-codegen/01-ad-core.qmd` written 2026-07-18.

- Walk the IR arena and evaluate expressions; implement forward-mode AD (`Dual`) over the
  unknowns.
- **Every differentiated operator has a finite-difference test** (analytic vs central
  difference) — non-negotiable (§5).
- **Tutorial:** `t2-codegen/01-ad-core.qmd` — dual numbers, why a wrong Jacobian silently
  kills Newton, the FD validation methodology.

### Phase T2.2 — Lowering IR to a `ModelInstance`
> **Status: 🟢 code complete** — `va-codegen/src/{lower,lib}.rs`: flow contributions split
> into resistive/charge terms; `build_instance` validates the subset then emits a
> `GeneratedModel` whose `load` stamps like `stamp_conductance`/`stamp_charge`. Generated
> resistor reproduces `va-abi`'s hand-checked stamp; diode matches analytic current +
> conductance; **§5 AD-vs-FD milestone green**.
>
> **Corpus baseline (2026-07-14), the T2 analogue of T1's `token-reference.md` tracking**:
> passing the *frontend* (T1, `docs/token-reference.md`'s domain) and passing *codegen* —
> actually buildable into a `ModelInstance`, i.e. actually simulatable — are different bars,
> and only the first was ever measured against the real, recursively-scanned 115-file
> `external/` corpus. Scanning the second (`va_codegen::build_instance` on every module that
> already elaborates): of the 62 that pass the frontend, **50 now also pass codegen** (up from
> 44). Of the 12 that don't: a nested (non-top-level) `ddt`/`idt` (6, down from 14 — see below
> for what's left), a branch's flow probe with no potential contribution of its own (4, up from
> 2 — `asmhemt.va`/`asmhemt101_0.va` newly re-attributed here now that their `ddt`-scaling
> blocker is fixed, not a regression), and a local-variable read before assignment (2,
> `mvsg_cmc_*.va` — unchanged, still out of this round's scope). **6 net new files** pass versus
> the prior baseline.
>
> **`if`/`else` is now lowered** (previously the single biggest codegen blocker — 35 of the 43
> non-frontend-clean-but-codegen-failing files as of the prior baseline; the fix removed that
> whole category from the corpus scan's failure buckets). Genuinely different from a flat
> contribution or a sequential assignment: which branch runs depends on `x`, so `lower.rs`
> can't flatten an `if` away structurally the way it flattens `+`/`-` into signed terms —
> `LoweredStmt::If { cond, then_, else_ }` carries *both* arms as their own lowered statement
> sequences, and `GeneratedModel::run` (used by `load`) picks one at evaluation time based on
> the condition's value, the same "only the taken branch is ever evaluated" rule `Expr::Select`
> (the ternary) already followed in `ad::eval`. The one real design problem: `build_instance`
> validates eagerly at the all-zero point specifically so `load` can never fail later, and a
> naive "validate only the branch selected at x=0" scheme would miss an unsupported construct
> hiding in the *other* arm — so `GeneratedModel::validate`/`validate_stmts` walk **both** arms
> unconditionally instead, an honest over-approximation (sound for the common case of a
> region-selecting compact model where both arms assign the same variables; not full
> path-sensitive flow analysis). Regression-tested directly against that failure mode: a test
> builds a module where the arm *not* selected at x=0 contains an unassigned-variable read, and
> asserts `build_instance` still rejects it eagerly rather than only failing at a later `load`
> call with a different operating point — plus a branch-selection test asserting both the
> stamped residual *and* Jacobian differ correctly across the two arms (proving the selected
> arm's own gradient is what gets stamped, not the other arm's).
>
> **Potential (voltage) contributions are now lowered** (previously the single biggest codegen
> blocker — 23 of the 40 non-frontend-clean-but-codegen-failing files as of the prior baseline).
> `V(p,n) <+ expr` is a *constraint* (`V(p)-V(n) = expr`), not a current balance, so it needs its
> own auxiliary branch-current unknown — the same shape `va_abi::reference::VSource` already
> uses (`UnknownKind::Branch`, never safe for `gmin` to shunt). `lower::lower` scans the whole
> analog block once (`if`/`else` arms included) for every branch targeted by a potential
> contribution and allocates each one a fresh local terminal slot past the node slots;
> `build_instance`'s signature grew a `next_unknown: &mut usize` parameter so it can claim those
> extra global indices itself, the same counter-threading pattern `va-cli`'s device-building
> loop already used for `vsource`'s own branch current — `va-cli`'s call site needed exactly one
> line changed. `GeneratedModel::stamp_branch_currents` stamps the constraint row's structural
> `V(p)-V(n)` term and the branch current's ordinary two-terminal KCL injection once per branch,
> unconditionally, regardless of which (if any) `if`/`else` arm actually contributes to it that
> call — an uncontributing path defaults to `V(p)-V(n) = 0`, matching the LRM's
> implicit-zero-contribution rule; `GeneratedModel::stamp` then subtracts each executed
> `V(...)<+expr` statement's own value/gradient from that same row. A flow probe `I(...)` now
> resolves too (`ad::eval`), but *only* for a branch that has an allocated current unknown (i.e.
> also receives a potential contribution) — this is what let the common "voltage in terms of its
> own current" series-resistance idiom (`diode.va`, `jfet.va`, `mosvar.va`: `V(a,b) <+
> I(a,b)*rs`) lower at all. `ddt` inside a potential contribution (an inductor spelled as
> `V(p,n) <+ L*ddt(I(p,n))`, `varistor.va`'s series-inductance branch) routes to the *constraint
> row's* charge channel rather than the node rows — a different stamp shape than a flow
> contribution's `ddt`, regression-tested directly. Verified against a real 5 V/1 kΩ circuit
> through the full pipeline (`va-cli sim`, not just isolated stamp assertions): the
> potential-contribution resistor idiom converges to exactly 5 mA via Newton, alongside the
> reference `vsource`.
>
> **Branches mixing flow and potential contributions are now lowered too** (previously rejected
> outright — 22 of the 33 non-frontend-clean-but-codegen-failing files as of the prior
> baseline). Real compact models do sometimes gate between the two per-branch by a *parameter*
> (the widely-reused `` `collapsibleR `` macro, `diode_cmc.va`'s several collapsible branches):
> below some threshold the branch behaves as an ordinary current-defined element, above it, it
> collapses to a forced/near-zero-impedance voltage constraint — always via mutually-exclusive
> `if`/`else` arms. The problem an always-allocated, always-unconditionally-stamped constraint
> row (the non-mixed design) can't handle: the row's very *shape* depends on which arm this
> particular `load()` call's control flow actually takes, which isn't known until the statement
> walk runs. `lower::BranchCurrent` gained a `mixed` flag (a branch is mixed if it appears in
> both the flow-targeted and potential-targeted branch sets `lower` already collects); a
> non-mixed branch keeps the exact unconditional-upfront-stamp code path from before (zero
> behavior change, zero regression risk for the 29 files that already worked). A mixed branch's
> structural `V(p)-V(n)` term and KCL injection are instead stamped *lazily*, from
> `GeneratedModel::stamp` itself, the first time a potential contribution actually executes for
> it this call (`ad::Ctx::mark_potential_used` reports "first time" via a per-call `HashSet`,
> the same interior-mutability pattern `Ctx::vars` already used). If no potential contribution
> ever claims the row this call — the flow arm ran instead, ordinary KCL stamped directly at
> `p`/`n` as always — the auxiliary current is otherwise a free unknown with no equation of its
> own, which would leave the system singular; `GeneratedModel::finalize_mixed_branch_currents`
> runs once after the whole statement walk finishes and pins any such row to zero
> (`residual(gb,x[gb])`, `jacobian(gb,gb,1.0)`), sound because the flow arm's own KCL stamp
> already carries the branch's real current. Regression-tested with the `collapsibleR` shape
> itself (`if (rt>1.0) I(b)<+V(b)/rt; else V(b)<+0;`), both ways: above threshold reproduces the
> exact ordinary-resistor stamp with the auxiliary row correctly pinned and *not* leaking into
> the node KCL rows; below threshold reproduces the forced-short constraint row and its KCL
> injection. Also verified end-to-end (`va-cli sim`, not just stamp assertions) for both
> regimes: a 5 V source across the ordinary-resistor arm (rt=2000) converges to exactly 2.5 mA;
> across the forced-short arm (rt=0.5, wired in series to an otherwise-floating node) the
> floating node collapses to exactly the source's own voltage with ~0 A flowing, both via
> Newton.
>
> **`while`/`for`/`repeat` loops and `case` are now lowered** (previously rejected outright —
> 19 of the 31 non-frontend-clean-but-codegen-failing files as of the prior baseline).
> `case` needed nothing new: it's an n-ary `if`/`else`, so `LoweredStmt::Case` just carries every
> arm's labels/body plus a default, and `GeneratedModel::run`/`validate_stmts` extend the
> existing "run only the selected arm, validate every arm once" split from two arms to however
> many a `case` has. Loops are different in kind: a real corpus survey (not guessed) found `for`
> and `repeat` almost always bound a parameter-derived trip count for a per-finger accumulation
> (`bsim4.va`'s `for (i=0;i<nf;i=i+1) acc=acc+term;`), and `while` almost always bounds a
> capped Newton-style convergence sub-iteration inside the analog block itself
> (`hicumL2*.va`'s `while (abs(d_Q)>=tol && iters<=max) ...`) — never anything array-indexed,
> since `va-frontend::elaborate` already expands array/genvar indexing into an ordinary
> `if`/`else` chain before this IR exists, so a loop body here is just an ordinary statement
> sequence, nothing new to support. `GeneratedModel::run` interprets a loop for real: it
> actually iterates, re-evaluating the condition/count against the current variable bindings
> each time, so forward-mode AD accumulates correctly across iterations exactly like any other
> statement sequence (AD doesn't know or care a loop produced it). Since a `while`/`for`
> condition can depend on `x` or on loop-carried state, its trip count isn't knowable in
> advance, so `run` bounds every loop at a fixed cap (`MAX_LOOP_ITERATIONS = 1_000_000`,
> generous headroom over anything the corpus actually needs) rather than risk hanging forever —
> the one case `GeneratedModel::validate` cannot rule out ahead of time (see below), so unlike
> every other `CodegenError` this crate raises, exceeding it can genuinely surface for the first
> time from `load()`, not just from `build_instance`'s eager validation; a documented, tested
> exception to "validated eagerly so `load` can never fail." `validate`, by contrast, never
> actually iterates a loop at all — it runs the body exactly once (same as any other statement
> block), which already covers every construct a real iteration could execute, without needing
> to resolve a real trip count or risk hanging during eager validation itself. Regression-tested:
> `case` with a multi-label arm and a `default` fallthrough (both the residual *and* Jacobian
> checked per arm); `repeat` and an explicit `for` with its own counter variable both
> accumulating `n` copies of the branch voltage through the loop (plus a central-finite-
> difference check on the accumulated gradient, §5); a `while` loop halving a local variable
> down past a threshold, checked against an independent Rust reference computation rather than
> a hardcoded constant; and, directly proving the iteration-cap design actually works rather
> than just being documented, a `while (1>0)` loop that never terminates — `build_instance`
> still succeeds (validation only ran the body once), but `load()` hits the cap, aborts before
> the statement after the loop ever runs, and returns promptly rather than hanging.
>
> **User-defined analog functions are now lowered too** (previously rejected outright — 17 of
> the 25 non-frontend-clean-but-codegen-failing files as of the prior baseline). A
> `va_ir::Function` is pure and non-recursive with its arguments/return variable/locals living
> as ordinary globally-unique `VarId`s in `Module::vars` (not a separate stack frame — the LRM
> already forbids recursion, so no call ever needs to save/restore a binding another call is
> still using), and its body can never contain a `<+` contribution (another LRM rule). That
> combination means a function call needs nothing `crate::GeneratedModel::run` has (no
> `StampSink`, no branch-current bookkeeping) — just expression evaluation and the variable
> environment `ad::Ctx` already carries, so the whole feature landed inside `ad.rs` alone,
> without touching `lower.rs`'s structural extraction of the *analog block* at all: a small,
> self-contained statement interpreter (`ad::exec_stmt`/`exec_stmts`) that `ad::call_function`
> drives, reusing `ad::eval` for every expression exactly as before. `Expr::CallUser` binds each
> argument's evaluated `Dual` into the function's own argument `VarId`s, runs the body, and
> reads back the `ret` variable's final binding as the call's result — so forward-mode AD
> composes through a function call by ordinary chain rule, no special-casing needed. The one
> design question worth calling out: a function can have its own internal `if`/`case`/loops, and
> `build_instance`'s eager, all-zero-point `validate()` must not miss an unsupported construct
> hiding in one of *those* just because this codegen crate already solved the identical problem
> once for the top-level analog block — so `ad::Ctx` grew a `validating` flag (set once, at
> `Ctx` construction, by whichever of `GeneratedModel::load`/`validate` built it), and
> `exec_stmt` consults it to pick the exact same "run only the selected/taken path" vs "visit
> every arm once, never actually iterate a loop" split `GeneratedModel::run`/`validate_stmts`
> already established for the outer block — applied recursively, inside every function call,
> for the same soundness reason. Regression-tested: a basic call (`sq(x)=x*x`) with both a
> hand-computed value/gradient check and a central-finite-difference cross-check (§5); a
> function whose own body region-selects between a valid `else` arm and a `then` arm that reads
> an unassigned variable — proving `build_instance` still rejects it even though a real call at
> the all-zero point would only ever take the (valid) `else` arm; a wrong-argument-count call;
> and a `<+` contribution inside a function body, both rejected as `CodegenError::Unsupported`.
>
> **A parameter-scaled `ddt` is now lowered too** (a real corpus survey — not guessed — found
> this the single dominant "nested `ddt`" shape: `coeff*ddt(charge)`/`ddt(charge)*coeff`/
> `ddt(charge)/coeff`, ~139 occurrences across the 18 previously-blocked files, e.g. `bsim4.va`'s
> `I(gi,si) <+ BSIM4type * ddt(qgate);`, a polarity-selection parameter scaling a charge term —
> every *other* nested shape the survey checked for, ternaries, `ddt` as another builtin's
> argument, `ddt` inside a user function, `ddt(a)*ddt(b)`, had zero occurrences anywhere in the
> corpus). The correctness constraint driving the whole design: `coeff(x)*dQ/dt` only equals
> `d(coeff*Q)/dt` — letting it fold into the ordinary charge channel exactly as an unscaled
> `ddt` already does — when `coeff` doesn't itself depend on the unknowns `x`; this project's
> `va_abi::StampSink` charge channel has no way to express the general product-rule case where
> it does (that would need the whole companion-model discretization, currently owned entirely
> by `va-transient`'s integrator via one time-stepping coefficient per row, to also carry a
> per-term, model-supplied coefficient — a `va_abi`/`va_transient` interface change, out of
> scope here). So `lower::is_param_only` recursively proves a coefficient is built from nothing
> but `Const`/`Param` and pure arithmetic/builtin combinations of those (later extended to
> provably parameter-only local variables too — see below) before `lower::charge_term_shape`
> will fold it in at all; anything else (a node/branch probe, a function call) falls back to the
> exact same rejection an unscaled nested `ddt` already got, rather than risk a silently wrong
> Jacobian. `lower::ChargeTerm` (replacing the
> reused `Term` type for the charge channel specifically) carries the coefficient expression and
> whether it divides; `GeneratedModel::sum_charge_terms` evaluates it once per stamp and scales
> the `ddt` argument's `Dual` by its plain value — exact, not an approximation, precisely because
> a proven-zero-gradient coefficient makes the general product rule collapse to this simpler
> form. Regression-tested: all three syntactic shapes, each checked against hand-computed
> charge/charge-Jacobian values *and* a central finite difference on the charge value itself
> (§5's charge-channel analogue); and, proving the safety check actually bites rather than just
> being documented, the same shape with the coefficient replaced by the branch's own voltage (a
> genuinely `x`-dependent "coefficient") — still rejected, exactly like before.
>
> **A `ddt`-scaling coefficient can now be a local variable, too** (previously rejected outright
> — a follow-up investigation into the remaining 14 nested-`ddt` files found this the dominant
> remaining cause: `bsimbulk.va`'s `devsign*ddt(...)`, `bsim4.va`'s `BSIM4type*ddt(...)`,
> `asmhemt.va`'s `ct*ddt(...)`, all scaling coefficients assigned via `if`/`else` rather than
> read directly off a bare parameter). The same correctness constraint applies — the coefficient
> must be provably `x`-independent — so `lower::param_only_vars` computes, once per module, the
> set of local variables where **every** `Stmt::Assign` to them anywhere in the analog block
> assigns a parameter-only expression, to a fixed point (so a short dependency chain like `a=W/L;
> b=a*2;` is still recognised — `b` only counts once `a` already does). This is deliberately the
> same eager, non-path-sensitive over-approximation character as the `if`/`else`-validation split
> elsewhere in this crate: sound (an accepted variable really is parameter-only on every path
> that could reach it) but not complete (one that's parameter-only on the specific path a given
> `ddt` site cares about, but genuinely `x`-dependent on some unrelated path, still stays
> rejected) — and, crucially, the *guard* of an `if` assigning the coefficient doesn't matter,
> only what actually gets assigned in every arm (`asmhemt.va`'s `if (V(g)>voff) ct=ctrap3; else
> ct=1.0e-9;` guards on a node voltage but assigns only parameter-only values either way, and is
> correctly accepted). `lower::is_param_only` gained an `Expr::Var` case consulting this set.
> Regression-tested: the real `devsign`/`ct` `if`/`else`-assigned-coefficient idiom, checked at
> operating points that take *both* branches of the guard; a two-variable dependency chain
> (`a=W/L; b=a*2;`) proving the fixed point actually propagates transitively rather than only
> recognizing a variable assigned directly from a bare `Const`/`Param`; and, proving the
> soundness check still bites, a variable assigned a parameter-only value in one arm but the
> branch voltage itself in the other — still rejected, since not *every* assignment is
> parameter-only.
> **`charge_term_shape` now recurses through arbitrarily many nested multiplications/divisions**
> instead of only inspecting the immediate operands of the outermost one — `ekv26.va`'s
> `ddt(qjd)*TYPE*M` parses as `(ddt(qjd)*TYPE)*M`, two levels deep, which the single-level version
> couldn't see past. `ChargeTerm` changed from a single `Option<ExprId>` coefficient to a
> `Vec<(ExprId, bool)>` of every scaling factor found, applied in sequence at evaluation time
> (`GeneratedModel::sum_charge_terms`) — still exact, since each is independently provably
> `x`-independent.
>
> **A `ddt` result assigned to a local variable and read back later is now tracked**, closing the
> other half of the previously-`if`/`case`-restricted-placement workaround real models use —
> `angelov_gan.va`'s `T0 = ddt(Ldc*I(rf,si)); // Avoid analog operator in if/else block` and
> `hisim2.va`'s `I_nqs_b = ddt(...); I(int_nqs_b) <+ I_nqs_b;`. `lower::DdtVars` maps a variable to
> its defining RHS *only* when that RHS is itself a recognized `ddt` shape; such an assignment
> never becomes an ordinary `LoweredStmt::Assign` (there's no sound value to give it — evaluating a
> bare `ddt(...)` outside the charge channel is exactly what this project can't do), and a later
> bare-variable read inside a `<+` substitutes it in. This is forward and single-pass, not a full
> reaching-definitions analysis: entering an `if`/`case`/loop body clones the map (so a definition
> from before the construct is visible inside it — always sound, since it necessarily already ran),
> but any variable assigned *anywhere* inside is forgotten in the outer map afterward, regardless of
> which arm actually executes — a variable can't be soundly treated as still holding a stale `ddt`
> shape (or any other stale value) once a branch might have overwritten it. Regression-tested
> including the specific danger this guards against: a variable holding a `ddt` shape before an
> `if`, reassigned to an ordinary value in only one arm, read again after the `if` — must never
> stamp as though it were still the discarded `ddt` shape (it doesn't; worst case, when the
> reassigning arm didn't actually run, `load` silently leaves the sink unstamped, a pre-existing
> "cannot happen post-validation" fallback rather than a regression this fix introduces).
>
> Re-scanned the full external/ corpus (115 .va files, recursive): 53/115 pass frontend+codegen,
> up from 50 (+3 net new files — `ekv26.va`, `angelov_gan.va`, `hisim2.va`, exactly the three
> concrete shapes above).
>
> **`idt` (the time-*integral* operator) is now lowered too**, closing the last of the three
> outstanding shapes and unblocking PSP102's NQS variants
> (`psp102_nqs.va`/`psp102b_nqs.va`/`psp102e_nqs.va`:
> `V(SPLINE1) <+ vnorm_inv * idt(-Tnorm*fk1, Qp1_0);`). Architecturally distinct from `ddt`:
> `idt`'s value at a given instant depends on the *entire history* of its argument, not just the
> current unknowns, so it can't be recovered symbolically from a top-level contribution shape the
> way `ddt`'s charge argument is. Instead, every distinct `idt(expr)` call site gets its own
> auxiliary "accumulator" unknown `Y` (`lower::IdtAccumulator`), enforcing `ddt(Y) = expr` via the
> *existing* charge-channel machinery — self-contained exactly like a potential contribution's own
> branch-current unknown (`GeneratedModel::stamp_idt_accumulators` stamps it unconditionally every
> `load()` call, after the statement walk finishes, since `expr` may itself read a local variable
> the walk just bound — PSP102's NQS argument is built from `Tnorm`/`fk1`, both ordinary earlier
> assignments). Reading `idt(expr)`'s *value* is then just an ordinary read of `Y`
> (`ad::Ctx::idt_slots`/`ad::eval`'s new `Builtin::Idt` case) — so, unlike `ddt`, `idt` may appear
> **anywhere** in an expression, not only as a top-level contribution term: no special-casing was
> needed for PSP102's `coeff*idt(...)` shape at all, since the multiplication just evaluates `idt`'s
> value like any other sub-expression. `build_instance` allocates each accumulator's global index
> the same way it already allocates branch-current unknowns (generalized to `while full.len() <
> lowered.n_unknowns`, so it stays correct regardless of how many auxiliary-unknown categories
> exist). *Honest limitation, not a special gap in `idt`:* the optional initial-condition argument
> is accepted syntactically but not applied — this project already starts every transient run from
> the all-zero vector with no `.ic`/`UIC` support at all, so an accumulator's true initial value is
> whatever the DC operating point resolves it to, the same limitation every other reactive state in
> this codegen already has.
>
> Re-scanned again: 56/115 pass frontend+codegen, up from 53 (+3 — all three PSP102 NQS variants).
> The nested-`ddt`/`idt` bucket that opened this round of work is now fully closed.
>
> **A purely flow-defined branch can now also be read via a bare `I(...)` probe.** Previously a
> flow probe only resolved for a branch that also received a potential contribution somewhere
> (the branch current's own auxiliary unknown, allocated for a completely different reason);
> reading a branch's own current where nothing else about the branch needed one at all — real
> models do this two ways — failed outright. `asmhemt.va`/`asmhemt101_0.va`'s
> `idisi = I(di,si);` reads the branch's total current strictly *after* every contribution to it,
> purely to feed an `` `OPM `` operating-point-report variable (never anything electrical).
> `diode_basic.va`'s `Id = I(anode,cathode);` is genuinely self-referential: read *before* the
> branch's own contribution, to compute a series-resistance voltage drop that itself determines
> `Id` via `Im`/`Qe`/`kfwd` — a real implicit equation. Both are handled uniformly by
> `lower::FlowCurrentAccumulator`: the branch gets its own auxiliary unknown, exactly like a
> potential contribution's branch current, but with the *opposite* defining equation — instead of
> constraining `V(p)-V(n)` to the contributed value, this unknown constrains *itself* to equal the
> branch's own total resistive contribution (`GeneratedModel::stamp_flow_current_accumulators`,
> stamped after the statement walk so every contribution to the branch has already run). The node
> KCL injection is completely unaffected — this accumulator is a pure bookkeeping shadow of
> a value the branch's contributions already determine, not a new physical degree of freedom.
> Every `I(...)` read of the branch, before or after its contribution, then just reads this same
> unknown via the *existing* flow-probe machinery, so Newton resolves the self-referential case
> exactly like any other implicit equation, with zero special-casing at read sites.
> *Limitation:* the defining equation only sums resistive contributions, not any `ddt`/charge term
> also contributed to the branch (consistent with this project's DC solve already ignoring the
> charge channel entirely) — no corpus file surveyed feeds such a probe back into anything
> electrical, only diagnostic output, so this wasn't worth a second, charge-aware equation.
>
> Re-scanned again: 59/115 pass frontend+codegen, up from 56 (+3:
> `asmhemt.va`/`asmhemt101_0.va`/`diode_basic.va`).
>
> **A user-defined analog function's `output`/`inout` arguments are now honored** — the
> non-path-sensitive "variable read before assignment" gap left over from earlier rounds
> (`mvsg_cmc_1.1.1.va`'s `qgsrs`, `mvsg_cmc_2.1.0.va`'s `cofsmt`) turned out to be this, not a
> path-sensitivity problem at all: both are `output`-direction arguments
> (`mvsg_cmc_*.va`'s `calc_iq`/`calc_capt`: `output idsout,qgsout,...; input vgsin,vdsin,...;`),
> passed as a never-otherwise-assigned actual argument (`idsrs = calc_iq(idsrs, qgsrs, ...);` —
> only `idsrs` is bound by the outer assignment; `qgsrs` and the other six outputs are pure
> write-only results, read only through the call's own write-back). `va-frontend` already parsed
> argument direction (`ast::FuncArg::dir`) but elaboration discarded it, binding every argument as
> a plain input with no way to write a result back to the caller — a genuine Interface α gap, not
> a `va-codegen`-local one. `va_ir::Function` gained `arg_dirs: Vec<ArgDir>` (`ArgDir` =
> `Input`/`Output`/`Inout`, same length/order as `args` — `docs/interfaces.md`, §6-revised);
> `va-codegen`'s `call_function` now binds an `Input`/`Inout` argument's caller-side value in as
> before, but for `Output`/`Inout` also writes the parameter's *final* binding back into the
> caller's own variable after the call — enforced to be a plain `Expr::Var` (the LRM's own
> restriction on output/inout actual arguments; anything else is rejected, since there'd be
> nowhere to write the result). An `Output`-only argument starts genuinely unassigned inside the
> function (no silent default), so a body that reads one before writing it is still correctly
> rejected, not silently miscomputed. Additive: every existing `Function` construction site
> needed only `arg_dirs: vec![ArgDir::Input; args.len()]`, an exact behavioral no-op.
>
> Re-scanned again: **61/115 pass frontend+codegen, up from 59** (+2: both `mvsg_cmc_*.va`
> files — the entire non-path-sensitive variable-read-before-assignment bucket closed in one
> shot, since it was never actually that). *Outstanding:* `verilogaLib-master/ohmmeter.va` alone
> — `I(iprobe)` there is a single-terminal implicit-ground probe (not the same branch as the
> explicit `V(dutm,iprobe)<+0` contribution), whose value can only be derived from a genuine
> node-KCL sum across every other branch touching that node, not from any one branch's own
> contribution — not attempted, a different and harder feature than anything in this or the
> preceding two rounds. Full committed sweep. `t2-codegen/02-lowering.qmd` written 2026-07-18.
>
> **`ohmmeter.va` now lowers too.** A branch that receives *no* contribution anywhere (neither
> flow nor potential) but is read via a bare `I(...)` probe with one terminal being the module's
> implicit ground reference resolves via a genuine node-KCL sum at its *other* terminal, over
> every other contributing branch touching that same node (`lower::NodeKclProbe`) — exactly the
> gap the previous round left open. `ohmmeter.va`'s two branches, `(dutm,iprobe)` (the
> `V(dutm,iprobe)<+0` ideal-ammeter wire, an ordinary `BranchCurrent`) and `(iprobe,gnd)` (the
> bare `I(iprobe)` probe, contributed to nowhere), share node `iprobe`; the probe's own auxiliary
> unknown gets a purely linear defining equation, `Y = -(±other_branch_current)`, sign matching
> whichever terminal (`p`/`n`) of the other branch node `iprobe` is (`GeneratedModel::
> stamp_node_kcl_probes`) — no expression evaluation needed at all, since every referenced slot is
> already resolved by the time this stamps (an existing `BranchCurrent`, or a `FlowCurrentAccumulator`
> forced into existence if the touching branch is flow-only and wasn't independently probed
> elsewhere). *Limitations:* only the single-terminal (implicit-ground) case is handled — a bare
> `I(a,b)` probe of an uncontributed branch between two other, non-ground nodes stays rejected,
> no corpus file surveyed needing it; a touching branch that is itself a **mixed** `BranchCurrent`
> whose flow arm ran a given call reads back `0` here, the same pre-existing character every
> other bare `I(...)` read of a mixed branch already has.
>
> Re-scanned again: **62/115 pass frontend+codegen, up from 61** (+1: `verilogaLib-master/
> ohmmeter.va`, the last item on the previous round's outstanding list). *Outstanding:* the
> remaining 53 failures are almost entirely earlier-pipeline gaps unrelated to this crate — see
> T1.2 (macro-only headers, now fixed) and T1.1 (`bsimsoi.va`'s string-escape lex error, now
> fixed, though the file still fails deeper in elaboration) for two that have since moved;
> `hicumL*.va`'s `<` is the LRM's `I(<b>)` **port-branch** probe syntax (a different, real
> construct from the `NodeKclProbe` this round added — not attempted); `psp10{3,4}*.va`,
> `L_UTSOI_102*.va`, and `r2*_cmc.va` are not actually fixable from this corpus snapshot at all —
> each `` `include ``s a companion `*_module.include`/`*_body.include` file (the port/discipline
> declarations themselves) that simply isn't present here; the rest are `ekv3*.va`/`r3_cmc.va`/
> `psphv*.va` preprocessor-macro-ordering issues (a macro used before this scan's `` `include ``
> chain reaches its `` `define ``).
>
> **Re-measured 2026-08-04, against the grown 150-file corpus: 104/150 pass frontend+codegen**
> (of the 114 that pass the frontend). Note the denominator change — every count above this line
> is against the older 115/118-file tree and is kept as history, not restated. The scan was a
> one-off (`va_codegen::build_instance` over every module that elaborates, mirroring `va-cli
> check`'s directory-grouped library); **`va-cli check` itself reported only the frontend
> count**, so re-deriving this number meant re-running that scan by hand.
>
> **That flag now exists (2026-08-29).** `va-cli check <paths> --codegen` runs the same
> directory-grouped library through `build_instance` and prints its own tally, tagging a
> codegen-only failure `[cgen ]` so it is distinguishable from an `[elab ]` one at a glance. The
> one-off scan is retired; both figures come from one command. A `va-cli` unit test
> (`check_group_codegen_flag_is_a_strictly_later_stage`) pins the flag to being a *strictly
> later stage* — a module that elaborates but that codegen rejects must count as passed without
> it and failed with it — so the two numbers cannot silently become one measurement under two
> names.
>
> **Re-measured with it, 2026-08-29: 105/150** (**107/150** later the same day, after the
> `I(<port>)` fold stopped manufacturing nested `ddt`s — see that entry for the final
> per-file accounting). At the time of this measurement the 9-file gap was two categories:
> **`ddt` nested inside an expression** rather than as a top-level contribution term (7:
> `HICUML0-2`, `hicumL0_v2p0p0`, `hicumL0_v2p1p0`, `hicumL2V2p4p0`, `hicumL2V3p0p0`,
> `hicumL2_v310`, `mvsg_cmc_3.2.0` — § this phase's "recognized only as a top-level additive
> term" rule, and T2.3's restatement of it), and a **local variable read before assignment** (2:
> `bsimsoi`, `verilogaLib-master/amp_dynamic`). Nothing else in the corpus elaborates but fails
> to build.
>
> **`ekv3.va` left the first category the same day.** Its gate charge is contributed as
>
> ```verilog
> I(d, g) <+ SIGN_M * (d_gt_s * ddt(QD) + s_gt_d * ddt(QS)) * QON;
> ```
>
> — a *sum* of `ddt` calls inside a parenthesis that is then scaled on both sides. `lower.rs`'s
> `collect_terms` flattens only the top level of a contribution, so it never reached inside the
> parentheses, and `charge_term_shape` saw a `Mul` whose operands were an `Add` (not a `ddt`
> shape) and `QON` (not a `ddt` shape) — `None`, resistive channel, rejected downstream by
> `ad::eval`. `charge_term_shape` now returns a **list** of shapes and recognizes `Add`/`Sub`/
> unary negation, so a coefficient distributes over the sum: the contribution above yields
> `[(+1, QD, [d_gt_s, SIGN_M, QON]), (+1, QS, [s_gt_d, SIGN_M, QON])]`, exactly the
> hand-distributed form. All six coefficients here are provably parameter-only, so the existing
> `is_param_only` rule (why a `ddt` coefficient may not depend on the unknowns — § this phase's
> product-rule note) is unchanged; only the *shape* recognizer widened.
>
> A sum distributes **only when every one of its terms is itself a charge shape**. A mixed
> `(resistive + ddt(q)) * coeff` stays rejected, because splitting it would mean synthesising a
> new `resistive * coeff` expression and this crate only ever *reads* the IR arena — it has no
> way to write one. That restriction has its own negative-control test
> (`a_coefficient_over_a_mixed_resistive_and_ddt_sum_is_still_rejected`) so the widening cannot
> quietly grow into dropping a resistive half or a coefficient.
>
> **Evidence.** Corpus 104/150 → **105/150** (`ekv3.va`, and only `ekv3.va`, moves). The
> positive test was confirmed to discriminate by disabling the new `Add`/`Sub` arm and watching
> it fail. Workspace: **550 tests pass** (was 547), `fmt`/`clippy -D warnings` clean, and all
> **15 golden gates reproduce their previously recorded numbers to the last digit** — no zoo
> model writes this shape, so the change is inert for every gated circuit.

- Generate (or interpret) a `ModelInstance` from an elaborated `Module`: map `<+`
  contributions to residual stamps and their AD-derived Jacobian entries.
- Handle `if`/`else`, `case`, loops, and user-defined analog functions (all done).
- **Validation gate:** the generated diode model's stamps match `va-abi`'s hand-written
  reference diode within FD tolerance, across a voltage sweep.
- **Tutorial:** `t2-codegen/02-lowering.qmd` — from Interface α to Interface β; generated vs
  reference diode, side by side.

### Phase T2.3 — Charge channel (transient-ready) & coverage
> **Status: 🟢 partial** — `ddt(q)` terms are routed to the charge/`dcharge` channel; the
> generated capacitor stamps only charge (`Q=C·V`, `dQ/dV=C`), ready for T4. `idt(expr)` is now
> lowered too, via its own auxiliary accumulator unknown reusing the same charge channel (T2.2's
> `IdtAccumulator`) — no initial-condition (`.ic`/UIC) support, the one honest gap left, shared
> with every other reactive state in this codegen. A formal coverage matrix is still open; `ddt`
> is recognised only as a top-level additive term (by design — see T2.2). *Outstanding:* coverage
> tracking — a dedicated T2-specific matrix never materialized (`t2-codegen/03-charge-and-
> coverage.qmd`, written 2026-07-18, states this honestly rather than inventing one); real
> coverage tracking consolidated into `docs/token-reference.md` and this file's own changelog.

- Emit the charge/`dcharge` channel from `ddt`/`idt` so T4 can integrate.
- Broaden operator/built-in coverage toward the declared subset; track what is supported.
- **Tutorial:** `t2-codegen/03-charge-and-coverage.qmd` — the companion-model charge path
  and the honest coverage matrix.

---

## T3 — `va-core` (MNA assembly · Newton · linear solve · convergence, DC)

> **Staffing update (2026-07-04): reclassified as staff-maintained shared infrastructure, not
> a student thesis.** No T3 student was found. Of the fallback options considered — scoping T3
> down to a smaller thesis, folding it into T2/T6, or treating it like `va-ir`/`va-abi` — we
> picked the last: the phases below were already 🟢 code-complete (MNA, Newton, dense solve, DC
> sweep, tested against analytic values) *before* the staffing gap became apparent, so the risk
> this decision is retiring was already retired. See `docs/thesis-map.md`'s staffing notes and
> `CLAUDE.md` §3's footnote for the full reasoning. What remains below (sparse solve, the
> golden-vs-ngspice gate, and the `t3-core/*.qmd` tutorials) now proceeds as a staff-owned
> maintenance backlog rather than a thesis with its own defense — it is not blocking, and not
> urgent relative to the theses that are staffed. **Update (2026-07-04):** junction limiting
> *and* `gmin` stepping are now both wired into the Newton loop (see T3.3), the latter via a
> small, additive Interface β change (`docs/interfaces.md`, `docs/bridges/interface-beta-abi.md`
> §8) — see `convergence.rs`'s module doc comment for the full account.

**Formerly:** critical path, staff first, reliable student (§10).
**Fallback (moot now — no student assigned):** a study of MNA + Newton + convergence aids on
the reference models.

### Phase T3.1 — MNA assembly & dense linear solve
> **Status: ✅ complete** (marker refreshed 2026-08-04: exercised by all 13 gated
> circuits) — `va-core/src/mna.rs` `assemble` walks instances into the
> `System` sink (ground reduction via `row < dim`); `linsolve.rs` does a `faer` LU solve with
> singularity detection (non-finite output or failed `A·x≈b` check). 6 tests.
> `t3-core/01-mna.qmd` written 2026-07-18.

- Assemble the system (`mna.rs`) from a set of `ModelInstance`s via `StampSink`; dense solve
  through `faer` (`linsolve.rs`). Pure-Rust, no native deps (§5).
- **Tutorial:** `t3-core/01-mna.qmd` — nodal analysis, how stamps become a matrix, solving a
  linear resistor network by hand vs by code.

### Phase T3.2 — Newton & the resistor-divider rung
> **Status: ✅ complete** (2026-08-04: the "harness gate pending" qualifier is retired — rung 1
> has been green against real QSPICE golden since 2026-07-17, `divider` at 0.0e0) — `va-core/src/newton.rs` Newton loop
> (assemble → `J·dx=−f` → `x+=dx`), converging on residual≤abstol **or** relative update≤reltol.
> The resistor divider solves to the analytic midpoint (`1.0 V`, < 1e-9). Rung 1's golden gate
> has since formally passed for real, against QSPICE (2026-07-17, T6.3). `t3-core/02-newton.qmd`
> written 2026-07-18.

- Newton–Raphson loop (`newton.rs`) with abstol/reltol; solve the linear resistor divider.
- **Validation gate (ladder rung 1):** resistor divider DC matches golden ≤ 1e-4.
- **Tutorial:** `t3-core/02-newton.qmd` — the Newton iteration, convergence criteria, the
  first green `va-harness` run.

### Phase T3.3 — Nonlinear DC, sweeps & convergence aids
> **Status: ✅ complete** (2026-08-04: gate closed — rungs 2 and 5 pass vs QSPICE golden,
> `diode_iv` 6.7e-5 and `mos_dc` 1.5e-6) — nonlinear Newton converges on a
> diode–resistor clamp from the zero guess (KCL balances < 1e-9); `dc.rs` provides
> `operating_point` + `sweep`. `convergence.rs` ships `pnjlim`-style junction limiting
> (`limit_junction`, plus `default_vcrit`) and a geometric `gmin` schedule (`gmin_for_step`).
> **2026-07-04: junction limiting is now wired into `newton::solve`**
> (`NewtonConfig::limit_junctions`, default on) — the earlier "needs per-device state" blocker
> didn't actually hold (the loop already has both the pre- and post-update value for every
> unknown); it's applied as a blanket per-unknown clamp instead of a per-junction one, since
> `va-core` has no way to tell which unknowns are real junction voltages (see
> `convergence.rs`'s module doc comment) — all 16 `va-core` tests still pass with it on by
> default, including the resistor-divider/diode-clamp tests to their original tight
> tolerances. **`gmin_for_step` is now wired in too**, via the small Interface β change this
> genuinely needed: `va_abi::ModelInstance::unknown_kind` (default `Node`, a new `Branch` case
> `VSource` overrides for its own branch-current index) lets `mna::classify_unknowns` build a
> per-unknown map that `mna::System::shunt_gmin` uses to shunt only `Node` rows — never a
> branch-current constraint row like `VSource`'s `V(p)−V(n)=value`, which a naive "shunt every
> row" implementation would have silently corrupted. Added as a **default trait method** (§6,
> `docs/interfaces.md`), so every existing `ModelInstance` — including every `va-codegen`-
> generated model, which only ever declares node unknowns today — kept compiling with no
> changes of its own. `NewtonConfig::gmin_steps` (default `0`, off) drives it; two new tests
> confirm the divider/diode-clamp circuits still solve to the same answer with it enabled, in
> particular that the VSource branch survives intact (`gmin_stepping_does_not_corrupt_the_
> vsource_branch`). **A genuine needs-`gmin` demo now exists too**
> (`gmin_stepping_converges_a_circuit_plain_newton_cannot`): 20 diodes in series behind a 10 Ω
> resistor at 20 V, cold-started at zero. A real operating point exists (~0.81 V/diode,
> ~0.38 A), but plain Newton's per-unknown log-ramp limiting walks the chain's internal node
> voltages there one at a time with no competing conductance to keep them in check, and some
> node's voltage crosses into the exponential's `f64` overflow range en route — a genuine
> `Err(Singular)` from a non-finite Jacobian entry, confirmed independent of iteration budget
> (still fails at `max_iters: 2000`). `gmin` stepping's early, well-conditioned stages keep the
> whole chain in range long enough to land near the true point before the final, unshunted
> stage finishes it off in a handful of iterations. Rung 2's golden gate has since formally
> passed for real, against QSPICE (2026-07-17, T6.3). `t3-core/03-nonlinear-dc.qmd` written
> 2026-07-18.
> **2026-08-31: damping, the third convergence aid, is implemented.** T3.3's own step list
> named three (`gmin` stepping, source stepping, damping) and only two existed.
> `NewtonConfig::max_damping_halvings` adds a backtracking line search: a step that increases
> the residual infinity norm is retried at 1/2, 1/4, ... and the first scale that improves on
> the starting residual is taken. Default `0` (off), so every gated result is unchanged — and
> `validate` confirms 20/20 with it in the tree.
>
> It is genuinely complementary rather than a third way of doing the same thing: junction
> limiting bounds a step's *size* per unknown without knowing whether that helps, `gmin`
> stepping changes the *circuit*, and damping is the only one that consults the residual the
> step actually produced. Demonstrated the way the `gmin` aid was: one test checks the same
> circuit *fails* undamped and *succeeds* damped — a hard-driven diode with junction limiting
> deliberately off, where the first cold-start step lands far past the exponential's usable
> range — and then checks KCL at the junction, so it cannot pass by merely returning.
>
> **Source stepping, the third name on that list, is still missing, and is blocked rather than
> merely undone.** Ramping independent sources means telling each source instance that its
> excitation is scaled, which `va-core` cannot do through Interface β as it stands: `load` sees
> an `AnalysisCtx` with no such field. That is the *same* additive-`AnalysisCtx` shape
> `docs/proposals/bdf2-interface-change.md` proposes for BDF2, so the two should probably be
> decided together rather than as two separate §6 events.
>
> **2026-08-31: the rung-2 gate gains a nonlinear `.dc` circuit, `circuits/diode_clamp.net`.**
> Rung 2's existing `diode_iv.net` sweeps a source that forces the swept node directly, so
> `V(in) = V1` identically and only the `I(V1)` column added in T6.3 exercises the diode at
> all — every *node voltage* in that gate is a straight line by construction. The new deck
> puts a 1 k resistor in series (`Vin --[R1]-- mid --[D1]-- gnd`), moving the exponential into
> `V(mid)`: it tracks `Vin` below the knee, then clamps near 0.66 V while `R1` takes the rest.
> Real QSPICE golden via the same one-to-one `.model diode D(IS=1e-14 N=1)` translation;
> passes at `error=6.421e-5` (tol `1e-4`), the same order as `diode_iv`'s `6.656e-5` and from
> the same source — the diode's own nonlinearity, not plumbing. `validate` is now 16/16.
> Motivated by `t3-core/03-nonlinear-dc.qmd`'s figure: the chapter about curvature had a
> straight line for a plot, because `plot_sweep` draws node voltages and rung 2 had no
> nonlinear one to draw. Regenerating every golden in the same run reproduced all fifteen
> existing files byte-for-byte, which is its own small evidence that the oracle path is
> stable.

- Diode I–V; DC operating point + parameter sweep (`dc.rs`); convergence aids (`gmin`
  stepping, source stepping, damping) in `convergence.rs`.
- **Validation gate (ladder rung 2):** diode I–V sweep matches golden ≤ 1e-4; convergence
  fraction tracked.
- **Tutorial:** `t3-core/03-nonlinear-dc.qmd` — why diodes are hard, what each convergence
  aid does, the convergence-rate metric.

### Phase T3.4 — Sparse-solve benchmark (backlog measurement)
> **Status: ✅ measured, not built** (2026-08-31) — the sparse-solve backlog item this T3
> section's own staffing note (2026-07-04) flagged as "remaining" now has a real answer instead
> of an open question: `cargo xtask bench-linsolve` (new subcommand, `xtask/src/main.rs`) scales
> a leaky resistor-ladder MNA system — real `va_abi::reference::{Resistor, VSource}` instances
> assembled through the actual `va_core::mna::assemble` path, not a hand-fabricated matrix —
> from 10 to 10,000 nodes and times [`linsolve::solve_dense`] against a new prototype
> [`linsolve::solve_sparse`] on the identical assembled Jacobian at each size.

**What `solve_sparse` is.** `faer` 0.22's `sparse-linalg` feature is **on by default** and was
already being pulled in by `faer = { workspace = true }`'s plain `"0.22"` version spec — no new
dependency, no `Cargo.toml` feature flag change, nothing for `deny.toml` to re-vet. `faer::sparse
::linalg::solvers::{SymbolicLu, Lu}` gives a pure-Rust sparse LU (column-AMD ordering, symbolic
then numeric factorization) with no BLAS/LAPACK/KLU FFI, matching §5 exactly. `solve_sparse` has
the same `(a: &[f64] (dense, row-major), b, n) -> Result<Vec<f64>, CoreError>` signature as
`solve_dense`, converts `a`'s nonzero entries to a triplet list, and solves — deliberately a
**prototype comparison path**, not a production one: it pays an `O(n²)` scan of the dense buffer
before the sparse solve even starts, which a real sparse rewrite (assembling triplets directly
from `ModelInstance::load`, never touching a dense buffer) would not. `mna::System` itself is
still 100% dense; this only swaps the *solve* step, on a like-for-like Jacobian, to isolate the
one question this phase asks. `linsolve.rs` gained 7 new tests (`solve_sparse` agreeing with
`solve_dense` on a banded system, its own singular/identity/empty cases, `nnz`'s two cases) —
`va-core` is now 31 tests, all green, `solve_dense` and its original 6 tests untouched.

**A real bug found along the way:** `faer`'s sparse LU `panic!()`s — does not return `Err` —
on at least one genuinely-singular input (a structural pivot candidate whose numeric value
collapses to exactly zero; `faer-0.22.6/src/sparse/linalg/lu.rs:1426`). Confirmed empirically by
this phase's own `sparse_singular_matrix_is_rejected` test before it was caught, not merely
suspected from reading the source. `CLAUDE.md` §5 forbids *this crate* panicking on bad input;
since the panic originates one layer down, `solve_sparse` wraps the factorization in
`std::panic::catch_unwind` (no `unsafe` needed — `#![forbid(unsafe_code)]` stays intact) and
folds a caught panic into the same `CoreError::Singular` a graceful rejection would have
produced. A caller cannot tell the difference between "rejected gracefully" and "rejected via a
caught dependency panic," and should not have to.

**The measured table** (`cargo xtask bench-linsolve --release`, one representative run — wall
times carry the usual single-machine/single-run noise, especially at the smallest sizes, but the
asymptotic shape and the crossover point are stable across reruns):

| n_nodes | dim | nnz | fill | dense (MB) | dense (ms) | sparse (ms) | speedup |
|--:|--:|--:|--:|--:|--:|--:|--:|
| 10 | 11 | 30 | 24.8% | 0.001 | 0.010 | 0.026 | 0.37× (dense wins) |
| 20 | 21 | 60 | 13.6% | 0.003 | 0.144 | 0.032 | 4.5× |
| 50 | 51 | 150 | 5.8% | 0.020 | 2.13 | 0.055 | 38.6× |
| 100 | 101 | 300 | 2.9% | 0.078 | 3.03 | 0.135 | 22.5× |
| 200 | 201 | 600 | 1.5% | 0.31 | 3.77 | 0.201 | 18.8× |
| 500 | 501 | 1,500 | 0.60% | 1.9 | 15.4 | 1.09 | 14.1× |
| 1,000 | 1,001 | 3,000 | 0.30% | 7.6 | 46.4 | 3.16 | 14.7× |
| 2,000 | 2,001 | 6,000 | 0.15% | 30.5 | 129.9 | 10.5 | 12.3× |
| 4,000 | 4,001 | 12,000 | 0.07% | 122.1 | 712.5 | 38.8 | 18.4× |
| 6,000 | 6,001 | 18,000 | 0.05% | 274.8 | 2,074 | 88.5 | 23.4× |
| 8,000 | 8,001 | 24,000 | 0.04% | 488.4 | 5,377 | 177.7 | 30.3× |
| 10,000 | 10,001 | 30,000 | 0.03% | 763.1 | 9,341 | 282.1 | 33.1× |

**Crossover:** between `n_nodes = 10` and `n_nodes = 20` (`dim` 11→21) — dense is faster only at
the very smallest measured size (where both solves are sub-millisecond and dominated by
factorization setup overhead, not floating-point work), and sparse wins at every size at or above
`n_nodes = 20`, by a widening margin that settles around 12–33× once `n_nodes` clears a few
hundred. The fill fraction — real, measured from the assembled Jacobian's actual nonzero count
via the new `linsolve::nnz`, not estimated — drops from 25% at `n_nodes = 10` to 0.03% at
`n_nodes = 10,000`, exactly the `O(1/n)` shape a banded/leaky-ladder MNA matrix predicts, and
exactly why dense storage becomes untenable well before dense *solve time* forces the issue: 763
MB for one Jacobian buffer at `dim = 10,001` is already a real cost, before `newton::solve`'s own
"assemble/solve per iteration" loop or `convergence::gmin_for_step`'s multi-stage schedule
multiply it.

**Recommendation: do not build sparse now; the trigger is circuit size, not the calendar.**
Every circuit in `circuits/` today has a node count in the low single-to-double digits — the
whole ladder-rung zoo is nowhere near where this table's crossover sits, so dense remains the
right default with enormous headroom (`golden/`'s largest gated circuit is `ring_osc.net`, a
handful of nodes; `bench_linsolve`'s own `n_nodes = 20` row, 21 unknowns, is already larger).
Building a full sparse *assembly* path (not just swapping the solve step, as this prototype
does) is real work — a new `StampSink` implementation, a triplet-native `mna` alternative, an
API decision about when `newton`/`dc` pick dense vs. sparse — that is not worth taking on
speculatively per `CLAUDE.md` §1's own "incremental, verification-driven... not silent breadth"
principle. The concrete, checkable trigger to revisit this: **build the sparse path when a
target circuit (in `circuits/`, or a model class the zoo is about to grow into) needs
roughly `n_nodes ≳ 500`** — the point in this table where dense is already paying double-digit
milliseconds *per solve*, which `newton`'s per-iteration re-assembly and `gmin_for_step`'s
multi-stage schedule both multiply, and where sparse's ~14–33× measured advantage is large enough
to matter rather than being noise. Until then this phase's deliverable is the measurement itself,
not new production code — `solve_dense` remains what `newton`/`dc` call, unchanged.

- **Reproduce:** `cargo build --release -p xtask && cargo xtask bench-linsolve` (debug build
  works too, just slower — the `--release` note in `bench_linsolve`'s own doc comment). Fully
  deterministic: fixed node-count list, fixed resistances, no randomness.
- **Files:** `crates/va-core/src/linsolve.rs` (`solve_sparse`, `nnz`, shared `residual_ok`);
  `xtask/src/main.rs` (`bench_linsolve`, `build_ladder`, `run_bench_row`, `BENCH_NODE_COUNTS`).

---

## T4 — `va-transient` (integration · timestep/LTE · events)

**Fallback:** a report on integration methods + LTE timestep control.

### Phase T4.1 — Fixed-step integration & the RC rung
> **Status: ✅ complete** (2026-08-04: gate closed — rung 3, `rc_step` at 1.8e-5 vs QSPICE
> golden) — `integrator.rs` implements both
> `Method::BackwardEuler` and `Method::Trapezoidal` as a single companion-model abstraction:
> both discretizations reduce to the same per-iteration nodal equation
> `residual(x) + coeff·charge(x) + offset = 0` (`Companion::backward_euler`/`::trapezoidal`
> just derive `coeff`/`offset` differently from history), so `newton_step` — otherwise a copy
> of `va-core`'s DC Newton loop, reusing `va_core::linsolve::solve_dense` and
> `va_core::convergence::limit_junction` directly — needs no per-method branching at all.
> Assembly uses `va_abi::stamps::DenseStamp` directly (captures `charge`/`dcharge`, unlike
> `va-core::mna::System`, which intentionally drops them for DC) rather than anything from
> `va-core`'s own `mna.rs`. `run()` takes an explicit initial condition `x0` (the caller's
> job — typically a DC operating point, or, as in the RC test, a deliberately different one
> to observe a charging transient). `Method::Gear` returns `TransientError::UnsupportedMethod`,
> never silently falls back.
> **Superseded by T4.2 (2026-07-06): fixed-`cfg.tstep` stepping no longer exists** — `run()`
> is adaptive now (see T4.2 below); `cfg.tstep` is the *maximum* step, not the constant one.
> Rung 3's golden gate has since formally passed for real, against QSPICE (2026-07-18, T6.3).
> `t4-transient/01-integration.qmd` written 2026-07-18.

- Companion-model the charge channel; implement an implicit integrator (backward Euler →
  trapezoidal) in `integrator.rs`; fixed timestep first.
- **Validation gate (ladder rung 3):** RC transient waveform RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/01-integration.qmd` — companion models, BE vs trapezoidal, the
  first transient waveform vs ngspice.

### Phase T4.2 — Adaptive timestep & LTE control
> **Status: ✅ complete** (2026-08-04: gate closed — rung 4, `rectifier` at 8.2e-4 under the
> current divided-difference estimator, 6.8e-4 as originally gated under the embedded pair, vs QSPICE
> golden) — `run()` adapts `h` within
> `[cfg.tstep_min, cfg.tstep]` via an **embedded-pair LTE estimate**, not a rigorous
> divided-difference truncation-error calculation: every accepted step computes *both*
> `BackwardEuler` and `Trapezoidal` from the same `(x_prev, h)` (one reported, one purely an
> error reference), and their disagreement — weighted by `cfg.lte_reltol`/`cfg.lte_abstol`,
> the same `reltol·|x|+abstol` combination `va-core`'s Newton `reltol`/`abstol` use — drives
> accept/reject and grow/shrink (since 2026-08-31 a **power-law order-based controller**,
> `step_factor`; `SHRINK_FACTOR`/`GROWTH_FACTOR`'s fixed multiplicative constants before that). Below `cfg.tstep_min` without meeting
> tolerance, returns `TransientError::TimestepUnderflow` rather than silently accepting an
> out-of-tolerance step. **A real bug found and fixed while building this:** the trapezoidal
> companion's history term (`residual_prev − (2/h)·Q_prev`) is only valid for a row some
> device's charge channel actually touches (a genuine state variable); applying it to a purely
> *algebraic* row (an ordinary KCL node with no capacitor, or a branch-current constraint row)
> injects a spurious permanent history term whenever the caller's `x0` doesn't already satisfy
> that row's constraint exactly — an easy mistake (this module's own first test made it: a
> placeholder `0.0` branch current inconsistent with the source's actual current at `t=0`).
> Fixed via `classify_dynamic_rows` (computed once from `x0`'s assembled `charge`/`dcharge`,
> not a full per-step or Interface-β-level classification — a stated, honest simplification,
> not a fully general fix for a hypothetical nonlinear charge model that's zero exactly at
> `x0`). 9 tests: the RC charging curve still matches analytic; accepted steps demonstrably
> grow as the transient flattens; a tighter `lte_reltol` demonstrably needs more steps than a
> looser one (the actual point of this phase); trapezoidal is more accurate than backward
> Euler *at the same schedule* — not fewer steps, since both directions' accept/reject
> decisions come from the same symmetric embedded-pair estimate regardless of which method is
> "primary," a real, documented property of this design, not a bug; plus the underflow,
> unsupported-method, empty-circuit, and error-propagation edge cases.
> **2026-07-06: the rectifier itself now runs, through the real CLI pipeline** — see T6.2's
> update and rung 4 below. That needed one more piece not in scope when this phase's status
> was first written: `va_abi::ModelInstance::load` has no time parameter (Interface β's "no
> time, no frequency on the bridge" — `docs/bridges/interface-beta-abi.md` §7), so a genuinely
> time-varying source (`SIN(...)`, not a constant `DC` value) can't be expressed as a normal
> stateful-free instance. `integrator::run_dynamic` is the fix: it rebuilds a caller-supplied
> subset of devices fresh at every step attempt (the value baked in fresh each time), while
> everything else in the circuit stays a fixed, borrowed instance exactly as before —
> `va-cli`'s `build_instances_split` is the one caller that needs this today.

> **Superseded 2026-08-06:** `run_dynamic` and `build_instances_split` are **deleted**. Interface β now carries an `AnalysisCtx` (time + analysis kind), so a `SIN` source is an ordinary stateless `ModelInstance` reading `ctx.time` (`va_cli::WaveformSource`) and every device takes the same path — see this file's "Analysis context — Tier A" section. The reasoning below is kept as history; the mechanism it describes is gone.
> ~~*Outstanding:* a rigorous divided-difference LTE estimator to replace the embedded-pair
> heuristic.~~ **Built 2026-08-31 as `LteEstimator::DividedDifference`, and deliberately *not*
> made the default** — see below. `t4-transient/02-lte-timestep.qmd` written 2026-07-18 (rung 4's
> golden gate has since formally passed for real too, against QSPICE, same date).
>
> **2026-08-31: the divided-difference estimator exists, is validated against the closed-form
> truncation error, and is opt-in.** `integrator::divided_difference` computes a Newton divided
> difference over the candidate point plus the last accepted ones; the leading-term constants
> are the textbook ones rewritten in terms of what is actually available (`LTE ~ h^2*DD2` for
> backward Euler, `LTE ~ (h^3/2)*DD3` for trapezoidal). It needs 2 past accepted points for BE
> and 3 for trapezoidal, and **falls back to the embedded pair until it has them** rather than
> guessing, so the opening steps of a run are unchanged.
>
> **The measured trade-off, on an RC charge under trapezoidal at `lte_reltol` 1e-3: 686 model
> evaluations for the embedded pair vs 279 for divided differences (2.5x fewer), with relative
> error at one time constant of 4.6e-5 vs 2.1e-4.** The cheaper estimator being the less
> accurate one at the same nominal tolerance is not a defect in it — it is the pair being
> *accidentally conservative*: `|x_BE - x_Trap|` is dominated by backward Euler's own
> first-order error, so it systematically over-estimates the trapezoidal step's true LTE, takes
> smaller steps than the tolerance asked for, and pays a second Newton solve per attempt for
> the privilege. Divided differences deliver what `lte_reltol` actually requests; a caller who
> wants the pair's accuracy tightens the tolerance rather than buying it by accident.
>
> **The default changed on the same day, on the supervisor's instruction.** Divided differences
> are now `va-cli`'s production estimator and **the transient gates were re-run under them:
> 16/16 green**, `rectifier` 6.766e-4 -> 8.226e-4 (tolerance 1e-3, so headroom falls from 1.5x
> to 1.2x), `rc_step` 1.839e-5 -> 2.193e-5, `ring_osc` unchanged at 4.464e-6. The three va-cli
> pipeline tests that build their own `TranConfig` were flipped with it, so they exercise the
> path production takes (41 tests green).
>
> **Nothing in `golden/` changed, and nothing could have.** A golden file is QSPICE's answer to
> the circuit; this project's integrator plays no part in producing it, so an estimator switch
> cannot move it — what moves is *our* number, which is exactly what the gate compares. Checked
> rather than assumed: a full `cargo xtask gen-golden` before the switch reproduced
> `rc_step`/`rectifier`/`ring_osc` byte-for-byte (md5 identical, clean `git status golden/`).
> The re-validation is therefore a re-run of the comparison, not a re-baselining of it — the
> distinction matters, because re-baselining a gate to whatever the code now prints would make
> it unfalsifiable.
>
> **2026-08-31: say out loud when a transient run is approximating an operator.** Some
> analog operators fold to something simpler here, and the fold is *correct* for DC and AC —
> `absdelay` and the `laplace_*` family both settle to their steady-state value at a fixed
> operating point. In a **transient** run the same fold is a plausible number that is wrong:
> `absdelay` returns its undelayed input, and a `laplace_*` filter returns its DC gain.
> Corpus demand is real, not hypothetical: 5 files call `absdelay` and 5 call a `laplace_*`.
>
> `va-cli` now prints a warning naming the operator and what it did instead, *before* the
> waveform rather than after it. A warning and not an error, deliberately: refusing the model
> would block DC and AC analyses that are perfectly sound. What is not acceptable is a
> transient run handing back a confident waveform without mentioning that one of its operators
> was never really evaluated.
>
> Detection lexes rather than string-searches, so the word in a comment or an identifier that
> merely contains it (`absdelay_count`) does not trigger it. That precision is a direct
> dividend of reserving `absdelay` earlier the same day: it now arrives as a `Keyword` token
> rather than a bare identifier. `transition`/`slew` are deliberately **not** on the list —
> they are genuinely evaluated against Interface β's state channel — and the Z-domain family is
> not either, since elaboration rejects it outright, which is already loud.
>
> **2026-08-31: linear controlled sources (`E`, `G`), and a mislabelled current they
> exposed.** `va_abi::reference::{Vcvs, Vccs}` plus the netlist's `E`/`G` lines, each taking
> four nodes — the driven pair then the controlling pair. Both are expressible through
> Interface β unchanged: a `G` needs no extra unknown at all (its output current is a function
> of node voltages the solver already carries, so it stamps like a resistor reading a different
> pair than it drives), and an `E` claims a branch row exactly as an independent source does,
> differing only in that two entries of its constraint row are Jacobian terms rather than a
> constant. SPICE's current-controlled `F`/`H` are deliberately absent: their controlling
> quantity is another element's branch current, which is a resolution problem rather than a
> stamping one.
>
> `circuits/vcvs_amp.net` gates both at `error=8.496e-11`, the tightest in the suite, with every
> value hand-computable; `validate` is **23/23**.
>
> **The real find was a bug they made visible.** `va-cli`'s DC and sweep reports re-derived
> branch-current identity by assuming only `vsource` devices claim branch rows, and walking
> them in device order. That was true when it was written. Inductors (added earlier today) and
> now controlled sources claim branch rows too, so a deck declaring an inductor *before* its
> source printed the inductor's current under the source's name — a confidently mislabelled
> number, not a missing one, and invisible unless you checked the sign. Both reports now take
> `branch_currents`' own `(name, index)` map, which is what `report_ac` already did. The golden
> files were never affected — they were built from that same map — so no gate moved.
>
> **2026-08-31: a device line can set a model's parameters by name.** Before this, a netlist
> device could override exactly one of its model's parameters — the *first* one, positionally,
> through the SPICE scalar value. For a Verilog-A simulator that is a real limitation (models
> routinely declare a dozen parameters) and a quietly fragile one: reordering `parameter`
> declarations in a `.va` file would change what every existing deck means, with nothing to
> catch it. `D1 in gnd diode Is=1e-12 N=1.3` now works, on `D`/`M`/`Q` lines, in SPICE's own
> spelling.
>
> An override naming a parameter the model does not declare is an **error** that names both the
> offending name and the ones the model does declare, rather than a no-op: silently dropping it
> would leave a deck looking like it set something it did not, which is the exact failure the
> feature exists to prevent. A trailing token that is not a `name=value` pair is refused too.
>
> `circuits/diode_iv_params.net` gates it against a QSPICE `.model diode D(IS=1e-12 N=1.3)`
> carrying the matching values, at `error=6.826e-5`; `validate` is **21/21**. It discriminates:
> a dropped override would silently be `diode_iv.net`'s curve, which differs by orders of
> magnitude rather than marginally. The deck translator strips instance parameters on the way
> to QSPICE (SPICE puts them on the `.model` card) but **only** from `D`/`M`/`Q` lines — a
> `C`/`L` line's `IC=` is a real SPICE element parameter, and stripping it would silently
> change the initial conditions the golden run starts from.
>
> **2026-08-31: `.ac` gained `oct` and `lin`.** The card had been `dec`-only, and the stated
> reason was sound — `AcSweep::frequencies` produced a per-decade grid, so parsing a type the
> analysis could not deliver would have been a promise the engine could not keep. The fix was
> therefore to implement the grids, not to loosen the parser: `AcSweepKind` now selects the
> spacing, `points` is a density for `dec`/`oct` and a **total** for `lin` (SPICE's own
> convention), and every grid still ends exactly on `fstop` rather than approaching it through
> accumulated arithmetic. An unrecognized sweep type is still refused rather than guessed at.
>
> `circuits/rc_ac_lin.net` gates the linear grid at `|mag| 1.3e-15` / `phase 1.6e-14 rad`, and
> gates the *semantics* rather than just the numbers: QSPICE returns exactly the 41 points the
> card asks for, confirming both engines read `lin`'s count as a total. `validate` is **20/20**.
>
> **`.noise` deliberately stays `dec`-only.** Its integrated-total maths assumes logarithmic
> spacing, so accepting a linear grid there would quietly change what the reported total means
> rather than merely resampling a spectrum — a different and larger change than this one.
>
> **2026-08-31: `PULSE` sources, and the first real disagreement with the oracle.**
> `V1 in gnd PULSE(v1 v2 td tr tf pw per)` parses and drives a transient run. SPICE's optional
> trailing parameters default from the `.tran` card (one timestep for an omitted rise/fall, the
> run length for an omitted width/period), and since that card may appear *after* the source
> line, they are resolved in an explicit post-pass over the parsed deck rather than defaulted
> against timing that has not been read yet. A `PULSE`'s `v1` is its DC/AC value, the same rule
> `SIN`'s offset already followed.
>
> **`circuits/rc_pulse.net` exists but is deliberately not gated against QSPICE.** QSPICE starts
> a `PULSE` ramp a sub-timestep amount *before* `td` — measured at 0.039-0.1 us across five
> probe decks, independent of `td` and of `tr`, not a dyadic-grid snap, and not proportional to
> the timestep — while honouring `tr`'s slope exactly. This engine starts at `td`, the textbook
> definition. On a fast edge that fixed shift is a large amplitude error the RC then integrates
> into a persistent offset: `5.779e-2` with 1 us edges, `5.254e-3` with 20 us, `1.698e-3` with
> 100 us, against a `1e-3` tolerance. Slowing the edges until the number cleared the bar would
> be tuning the circuit to the tolerance rather than testing anything, so the circuit keeps
> physically sensible 20 us edges and is validated against the waveform's own definition
> instead — segment by segment, from both sides of every boundary, plus a parameter-free
> `exp(-dt/RC)` ratio check on the RC's charging and discharging. `validate` stays **19/19**;
> the full measurement table is in `docs/validation.md`.
>
> Open question for the supervisor: is QSPICE's early edge a deliberate convention worth
> matching, or an artifact to leave alone? Matching it is not obviously possible anyway, since
> the offset is not derivable from the deck.
>
> **2026-08-31: `IC=` completed for the other reactive element.** An inductor's initial
> condition is **amps through it**, not volts across it, so it seeds its own branch-current row
> rather than a node voltage — the units follow the state the element carries. The row index is
> the one `build_instances` already returns for branch currents, so `initial_solution` needed
> only that mapping, not new plumbing. `IC=` on a resistor stays an error: no state to seed.
>
> `circuits/rl_decay.net` gates it: a source-free `R`/`L` loop starting at 1 mA, decaying as
> `exp(-t/tau)` with `tau = L/R = 100us`. Its golden scores `I(L1)` itself, so the seeded
> quantity is the compared one, and like `rc_discharge.net` it has no source, so ignoring the
> condition leaves the run flat at zero rather than slightly wrong. Passes at `error=2.172e-8`;
> `validate` is **19/19**.
>
> Honest wrinkle, documented on `initial_solution` rather than hidden: seeding a branch current
> does **not** back-solve the node voltages that current implies, so the `t = tstart` sample can
> be inconsistent until the first real solve corrects it. That is the same unsolved-seed sample
> `va-harness` already excludes from every transient golden comparison, which is why the gate is
> unaffected and the end-to-end test asserts from the first *solved* point onward.
>
> **2026-08-31: inductors, with no interface change needed.** `va_abi::reference::Inductor`
> and the netlist's `L` element. An inductor claims its own branch-current unknown like a
> voltage source, because its row is the constitutive law rather than a KCL sum; written as
> `-(V(p)-V(n)) + d(L*i)/dt = 0`, the flux `L*i` rides the **existing** charge channel and the
> integrator's companion model discretizes it exactly as it does a capacitor's charge. Interface
> beta needed nothing new. The same formulation also gives the right DC answer for free: with no
> charge channel in a DC solve the row collapses to `V(p) = V(n)`, which is an inductor as a
> short circuit. `unknown_kind` returns `Branch` for that row, so `gmin` never shunts it.
>
> `circuits/rlc_ring.net` gates it — a series RLC step response, `zeta = 0.158`, overshooting
> to 8.02 V and ringing with a 199 us period. Second order is the point: a first-order stamp, a
> dropped flux term, or a sign error on the constitutive row cannot produce that waveform at
> all, whereas a purely resistive mistake would just shift a level. Its golden carries `I(L1)`
> beside `I(V1)`, so the inductor's own current is scored against QSPICE's. Passes at
> `error=6.480e-5` (tol 1e-3); `validate` is now **18/18**. The end-to-end test checks the
> closed form rather than the golden: peak overshoot against
> `1 + exp(-pi*zeta/sqrt(1-zeta^2))` and the ringing period against `2*pi/(w0*sqrt(1-zeta^2))`,
> both to 5e-3 relative, plus the settle toward the full source voltage that only holds if the
> inductor really is a DC short.

> **2026-08-31: initial conditions, and the first gate that fails loudly.** `C<name> p n <value>
> IC=<volts>` now parses (`va_netlist::Device::ic`) and seeds a transient run's initial solution
> vector (`va_cli::initial_solution`). These are SPICE's `UIC` semantics exactly: transient only,
> no DC operating point solved first, a capacitor without `IC=` starting at 0 V as this engine
> always did. Element-level `IC=` was chosen over a `.ic` card specifically because `xtask`'s
> golden-deck translator already leaves an explicit `IC=` alone while injecting `IC=0` into the
> reactive elements that lack one — so the QSPICE side needed no change whatsoever to support
> the new form.
>
> `circuits/rc_discharge.net` gates it, and is deliberately shaped to be *falsifiable*: it has
> **no source at all**, so `V(out) = 5*exp(-t/RC)` is driven entirely by the initial condition
> and an implementation that quietly ignored `IC=` would sit at 0 V for the whole run rather
> than producing a slightly-wrong waveform. Every other transient circuit in the zoo is
> source-driven and would keep looking plausible under that bug. Passes at `error=7.692e-6`
> (tol 1e-3) against real QSPICE golden; `validate` is now **17/17**. The end-to-end test checks
> the closed form at 1, 2.5 and 5 time constants, not just at `t=0`, where any implementation
> that merely copied the seed would also agree.

> **2026-08-31, same day: the step controller became order-based too.** With a real local-error
> estimate finally in hand, multiplying by 1.5 on a good step and 0.5 on a bad one was leaving
> the useful part of the estimate on the floor: a local error of `O(h^(p+1))` and a measured
> `err_ratio` say exactly which step would have landed on the budget, `h*err_ratio^(-1/(p+1))`.
> `step_factor` computes that, biased by a `SAFETY` of 0.9 and clamped — growth capped at 2x
> (one very accurate step must not launch the next one past where the local-error model still
> holds), shrink floored at 0.1x, and a rejected step forced to at least a 0.9x reduction, since
> the raw power law predicts ~0.997x for a ratio barely over 1.0 and would retry near-forever.
> The exponent now differs per method, which the fixed factors could not express: backward Euler
> shrinks harder than trapezoidal for the same overshoot, because its error falls off more
> slowly with `h`.
>
> **Measured on the RC charge with divided differences: 279 -> 271 model evaluations and
> 2.1e-4 -> 8.6e-6 relative error** — 25x more accurate for slightly less work, because the
> steps are now sized to the tolerance instead of ratcheting toward it. The golden gates barely
> moved (`rectifier` 8.226e-4 -> 8.269e-4, `rc_step` 2.193e-5 -> 2.248e-5, `ring_osc` 4.464e-6
> -> 4.553e-6, all 16/16 green): those circuits are breakpoint- and source-driven, so their
> stepping is constrained by more than the error estimate. The gain shows up where the
> controller is actually free to choose.
>
> `step_factor` is verified in closed form rather than by watching behaviour: a ratio of 8 under
> trapezoidal (`p+1 = 3`) must predict exactly `SAFETY * 1/2`, the same ratio under backward
> Euler (`p+1 = 2`) exactly `SAFETY / sqrt(8)`, and both clamps plus the zero-error
> division-by-zero case are pinned by their own test.
>
> Verified against the *closed form*, not against the other estimator (which would only prove
> the two agree): `divided_difference_matches_the_analytic_truncation_error` checks the
> estimate on `x(t) = e^t` against `(h^2/2)*x''` to 1e-3 relative, and
> `divided_differences_recover_a_polynomial_leading_coefficient` pins the differencing itself
> both ways — exact on a degree-k polynomial, exactly zero on a lower-degree one, and
> spacing-independent, which is the case an adaptive controller actually produces.

- Local truncation error estimate driving adaptive step size; step accept/reject logic.
- **Validation gate (ladder rung 4):** diode rectifier transient RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/02-lte-timestep.qmd` — LTE estimation, the step controller, why
  the rectifier needs it.

### Phase T4.3 — Events & breakpoints
> **Status: ✅ complete** (2026-08-04: the gate is neither blocked nor pending any more — the
> blocker below was resolved 2026-07-09 by adding `va-abi::reference::Bjt`, and rung 6's
> `ring_osc` now passes at 1.8e-4 vs QSPICE golden) —
> `events::EventQueue` (already implemented, previously unwired) now genuinely drives
> `integrator::run_with_events`: breakpoints clamp the adaptive step so it never overshoots a
> forced timepoint (the underlying `h` schedule is untouched by the clamp, so a forced short
> step doesn't corrupt subsequent step-size growth); `EventQueue::push_watch(unknown,
> threshold)` registers a crossing watch, checked against every pair of consecutive *accepted*
> points and reported via linear interpolation in the new `Waveform::crossings`. `run()` is now
> a thin wrapper over `run_with_events` with an empty queue, so every T4.1/T4.2 test still
> passes unchanged. 6 new tests: exact breakpoint landing (an "awkward" time no natural
> adaptive step would hit); a breakpoint past `tstop` changing nothing; the RC charging curve's
> crossing of `Vs/2` matches the analytic `t = RC·ln(2)`; no false crossing when the threshold
> is never reached; `run`/`run_with_events` agree given an empty queue.
> **Ladder rung 6 (ring oscillator): now closed (resolved 2026-07-09)** — was "structurally out
> of reach with the current model zoo," since `va-abi::reference` was entirely passive
> (resistor, capacitor, diode, ideal source) and no wiring inside `va-transient` can make a
> passive-only circuit oscillate. Closed by adding the missing gain element: `va-abi::
> reference::Bjt`, a three-terminal simplified (no Early effect, no ohmic/parasitic
> resistance, no saturation-charge smoothing) Ebers-Moll NPN — hand-derived Jacobian, validated
> against a central finite difference the same way `Diode`'s already is. A 3-stage RC-coupled
> common-emitter BJT ring (`integrator::tests::ring_oscillator_sustains_oscillation`, instances
> built directly — no netlist file, since `va-netlist` has no 3-terminal-device grammar yet)
> runs through the exact same DC (gmin-stepping) and transient machinery every other circuit in
> this crate uses. Finding working component values needed real iteration, not just a hand
> calculation: a lower-impedance "linear-gain" bias point converges at DC but turned out
> small-signal *stable* (no oscillation) once the coupling network's own loading was properly
> accounted for; a too-aggressive deep-saturation bias point made the DC solve itself
> numerically singular (both BJT junctions strongly forward-biased blows up the simplified
> model's exponential terms). A MΩ-range `Rb` sits in the working middle: comfortably
> forward-active at DC, genuinely unstable in the loop. The DC operating point Newton finds
> *is* that unstable equilibrium (Newton doesn't know or care that a fixed point is unstable) —
> a deliberate few-mV perturbation plus mismatched per-stage component values (breaking the
> three-way symmetry a real circuit's tolerances always break) diverges into real, growing
> oscillation, confirmed by a deepening trough over time, not just a couple of crossings.
> **Stated limitation, found empirically, not hidden:** as the oscillation grows, it eventually
> pushes a junction into strong forward bias on both sides at once, where the LTE embedded-pair
> estimator stops agreeing at any step size — the test's `tstop` stays inside the confirmed
> well-behaved region rather than chasing that numerical edge.
> *Outstanding:* the golden-vs-ngspice gate generally still awaits T6.3 — this validates that
> the circuit oscillates (and grows, as an unstable equilibrium should), not a specific
> frequency against a reference simulator. `t4-transient/03-events.qmd` written 2026-07-18,
> covering both this hand-built fixture and the circuit's newer, real netlist-driven form (§
> this file's own later rung-6 entries).
>
> **Now driven through the real netlist pipeline too (2026-07-18)** — until now this rung's
> oscillation only ran via hand-built `va-abi` instances inside a `va-transient` unit test, since
> `va-netlist` had no 3-terminal-device grammar for a BJT. Closed with a `'Q'` element arm
> (`` Q<name> c b e model ``, SPICE's own collector/base/emitter order, mirroring `'M'`'s
> no-body/substrate-terminal simplification) and a `"bjt"` `reference_instance` branch in
> `va-cli` (fixed parameters matching the hand-built fixture's own: `Is=1e-15`, `βF=100`,
> `βR=1`). `circuits/ring_osc.net` mirrors the fixture's topology and component values exactly.
>
> **A real surprise, confirmed empirically before writing the regression test**: no `.ic`/`UIC`
> support was needed after all, even though the hand-built fixture starts from a *perturbed DC
> operating point*, not `x=0`, specifically because a deterministic solver has no noise to break
> the ring's exact 3-way symmetry otherwise. Cold-starting from `x=0` (`va-cli::solve_transient`'s
> only mode) turns out to do the same job by a different route: every stage sees an identical
> `Vbe` from `x=0`, so all three charge to nearly the same forward-active bias within ~12.5 µs —
> close enough to the ring's own symmetric-but-unstable equilibrium that the stages' mismatched
> `R` values are enough to kick off the same genuine, sustained oscillation, confirmed by
> inspecting a full run's node trajectories (each collector swings between ~0.06 V and ~5 V,
> repeatedly, staggered in phase per stage) before trusting it, not assumed from the topology
> alone. `cargo test -p va-cli ring_osc_sustains_oscillation_through_the_real_pipeline` checks
> this properly — across all three collectors, not just one (a single node can sit on a quiet
> stretch of its own cycle within one `tstop` while the ring as a whole keeps oscillating;
> checking only one node would have been a flaky, component-value-specific assertion).
>
> **The QSPICE golden gate is now closed too (2026-07-18) — for a real, different reason than
> first diagnosed.** The initial diagnosis (translating this circuit the same way rungs 3–5 were
> — a native `.model bjt NPN(IS=1e-15 BF=100 BR=1)` card, `UIC` + `IC=0` on the coupling
> capacitors — runs cleanly but doesn't reproduce the oscillation) blamed `UIC` mode not
> re-solving non-reactive nodes. **That diagnosis was wrong, found by testing it directly rather
> than accepting it**: adding an explicit `.ic V(b1)=0 …` for every node still landed on the same
> degenerate all-`VCC` state, and a plain `.op` DC solve of the same topology (no `UIC` involved
> at all) reproduced the identical wrong answer — with `V(gnd)` itself reported as `5`, not `0`.
> Ground reading anything other than `0` V is impossible by definition, which pointed at the real
> bug: **QSPICE does not reliably alias a net literally named `gnd` to the reference ground node
> for a `Q` (BJT) element's own terminal**, unlike `R`/`V`/`M` (confirmed with the smallest
> possible isolating case — a single BJT bias circuit — not just the full ring). Rewiring the
> identical circuit's `gnd` references to literal `0` gives the correct forward-active bias
> (`V(b1)=0.662`, matching this project's own cold-start value almost exactly) and the full
> oscillation. `xtask::rewrite_gnd_to_zero` now normalizes every translated deck's `gnd` to `0`
> unconditionally (topology-neutral, since this project's own net interning already treats them
> as synonymous — no need to track which device kinds are actually affected by QSPICE's quirk).
>
> **A second, genuine wrinkle, handled honestly rather than forced**: even with the ground bug
> fixed, comparing the *full* `circuits/ring_osc.net` run (`.tran 100u 0.2`) against QSPICE gave
> `error=2.243e-2` against `TRAN_RMS`'s `1e-3` — not a bug, but this circuit's genuinely *unstable*
> equilibrium being exponentially sensitive to tiny model/solver differences once the oscillation
> grows large. Confirmed, not assumed: comparing the same real golden data at successively later
> cutoffs gave `error≈1.6e-4`–`2.4e-4` (well inside tolerance) for every cutoff up to `0.10` s,
> then jumped to `1.16e-2` at `0.12` s and `2.24e-2` over the full `0.2` s — a real, physically
> located divergence point (collector voltages reaching within noise of `0`/`5` V, base voltages
> going transiently negative — this model's own known numerical edge, same one the hand-built
> fixture's `tstop` already avoids). Rather than loosen `TRAN_RMS` or crop the *tracked* circuit's
> own `.tran` (which would cost `va-cli`'s oscillation-count test its needed ≥4 rail crossings —
> those only accumulate over ~2 growth cycles, past where the trajectories decohere), `xtask::
> RING_OSC_GOLDEN_TSTOP`/`truncate_tran_tstop` truncate *only the golden-generation deck* to
> `0.1` s — the tracked `circuits/ring_osc.net` file, and what `va-cli sim --tran` actually runs,
> stays at its full `0.2` s throughout. `golden/ring_osc.golden` is now real, committed QSPICE
> output (1041 points); `cargo xtask validate` reports `PASS circuits/ring_osc.net: error=1.923e-4
> (tol 1e-3)` — **all six ladder rungs now formally pass**.

- Event handling / breakpoints (`events.rs`) for sources and discontinuities; ring-oscillator
  shakedown.
- **Validation gate (ladder rung 6):** ring oscillator transient genuinely oscillates (done,
  2026-07-09) *and* matches golden within band (done, 2026-07-18 — see the status block above).
- **Tutorial:** `t4-transient/03-events.qmd` — breakpoints, forced timepoints, the oscillator
  demo.

---

## T5 — `va-acnoise` (AC linearization · noise: PSD, adjoint)

**Fallback:** an AC/noise-formulation report (adjoint-method derivation).

### Phase T5.1 — AC linearization

**Implementation reach closed 2026-07-21** — was a `todo!()` stub (T5 was entirely unstarted;
`va-acnoise` had never had a working line of code). `ac::linearize` runs every
`ModelInstance::load` at a fixed DC point `x_dc` into a purpose-built `StampSink` that keeps
only the two channels AC analysis actually needs — the Jacobian (`G = ∂residual/∂x`) and the
charge-Jacobian (`C = ∂charge/∂x`) — discarding the residual/charge values themselves (they're
DC-only quantities, irrelevant once the circuit is linearized). `ac::run` then sweeps a log
frequency grid (`AcSweep::frequencies`, SPICE `.ac dec`-style: `points_per_decade` points per
decade from `fstart` to `fstop` inclusive) solving `(G + jω·C)·X(ω) = B` at each point.

**No complex-linear-algebra dependency was added** — `CLAUDE.md` §5 restricts numerics to
`faer`, so the complex solve is embedded as a real `2·dim × 2·dim` block system (`[Re(X);
Im(X)]` stacked, the standard `[G, -ωC; ωC, G]` block form) and handed straight to
`va_core::linsolve::solve_dense`, the same dense LU `va-core`'s own Newton loop already uses.
An independent AC source's excitation (e.g. a `VSource`'s own branch-current row) is purely an
RHS term, never a `G`/`C` entry — the row's Jacobian already captures `∂/∂x` from its DC
constraint (`V(p)-V(n) = value`); only the source's own AC magnitude/phase is new, and it can
only ever appear on the right-hand side.

**Validated against a real closed form, not yet against QSPICE golden**: `ac::tests::
rc_lowpass_response` builds an actual `VSource`+`Resistor`+`Capacitor` circuit (the same
`va-abi::reference` primitives `va-core`'s own tests use) and checks the output-node magnitude
and phase against the textbook RC low-pass transfer function (`H(jω) = 1/(1+jωRC)`) across a
6-decade sweep (1 Hz–1 MHz, 5 points/decade) to `1e-6` — the same "closed-form before golden"
path T4.1's RC transient took before `va-harness`/QSPICE wiring existed for it. One real bug
caught in the process: the first `AcSweep::frequencies` implementation appended `fstop` as a
literal duplicate when repeated-multiplication float drift left the loop's own last point a few
ULPs short of it (`100.00000000000003` vs `100.0`) instead of recognizing it as the same point —
fixed by snapping within a relative tolerance instead of requiring bit-exact equality.

**Golden gate closed 2026-08-01** — the outstanding `va-cli`/`va-harness`/`xtask` AC wiring this
entry used to list is done, and `cargo xtask validate` now checks AC against real QSPICE golden
alongside DC and transient (8/8 circuits, 100% convergence). What that took, end to end:

- **`va-netlist`**: `.ac dec <points-per-decade> <fstart> <fstop>` → `Netlist::ac`
  (`AcSweepCard`), and a `V` line's `AC <magnitude> [phase]` → `Device::ac` (`AcSpec`).
  Deliberately only `dec` is accepted: `AcSweep::frequencies` produces a per-decade grid, so
  parsing `lin`/`oct` would promise a grid the analysis can't produce — those leave `net.ac` as
  `None` and `va-cli` says so plainly.
- **`va-cli`**: `solve_ac` solves the DC operating point, builds the complex excitation vector
  from each AC-marked source's own branch-current row, and sweeps. The `gate_analysis` rejection
  of AC decks is gone, replaced by the same deck-says-X/you-asked-for-Y checks `.tran` already
  had. A deck whose sources carry no `AC` token is a clear error, not an all-zero answer.
- **`va-harness`**: `GoldenAc` (an `@ac`-marked table, two columns per name for re/im — lossless,
  with magnitude/phase derived at comparison time) plus `ac::{run_ac, compare_ac}` returning an
  `AcVerdict` with *separate* magnitude and phase verdicts, since §7's AC metric is genuinely two
  bands, not one.
- **`xtask`**: complex `.qraw` parsing and an `AC_CIRCUITS` pass in both `validate` and
  `gen-golden`.

**Two empirical findings worth keeping** (both confirmed against real QSPICE runs, neither
guessable from the format docs):

1. An `.ac` `.qraw` payload is *not* uniformly complex despite its `Flags: complex` header. A
   5-variable 12-point file carries 108 f64s — **9 per point**, not 10: the `Frequency` abscissa
   is one real value and only the remaining variables get `(re, im)` pairs. A naive all-complex
   read misaligns every value after the first.
2. QSPICE's own frequency grid has an off-by-one at the top end (60 points where the clean grid
   has 61, dropping `10^5.9`). `compare_ac` matches by frequency rather than imitating it — see
   `docs/validation.md`'s AC-gate section for both, and for the measured errors.

**Measured against golden**: `rc_ac.net` at magnitude `1.3e-15` / phase `1.7e-13` rad (machine
precision — the same linear system on both sides); `diode_ac.net`, which linearizes a compiled
`models/diode.va` about a real forward bias, at magnitude `1.3e-5` / phase `6.4e-6` rad. The
second is the one with teeth: its passband gain depends exponentially on the solved bias, so
agreeing that closely constrains the DC point, the AD-derived Jacobian, and the linearization
together. `noise.rs` (T5.2) remains untouched.

**Tutorial**: `t5-acnoise/01-ac.qmd`, written 2026-08-01 — covers the linearization, the
`2n×2n` block embedding that avoids a complex-linear-algebra dependency, both QSPICE `.qraw`
findings, and why the AC metric is two bands rather than one.

### Phase T5.2 — Noise analysis

**Implemented and golden-gated 2026-08-01** — was a `todo!()` stub. `cargo xtask validate` now
checks a real noise spectrum against real QSPICE golden (9/9 circuits, 100% convergence).

**This needed a §6 change to Interface β first**, the first one since 2026-07-09: a device's
noise is physics the assembled matrices no longer carry, so it cannot be derived after the fact.
A 200 Ω resistor and a diode biased to a 200 Ω small-signal resistance stamp *identical* `G`
entries, yet the resistor's noise is thermal (`4kTg`, bias-independent) and the diode's is shot
(`2q|Id|`, bias-dependent) — for that pair they differ by exactly 2×. So `va-abi` gained a
`NoiseSink` trait and a `ModelInstance::noise` **default method**, in the same additive shape as
`unknown_kind`/`unknown_abstol`: every existing implementor kept compiling untouched. `Resistor`
overrides it (thermal), `Diode` and `Bjt` (shot); `Capacitor` and `VSource` keep the default,
which for them is the physically right answer rather than a stub. See `docs/interfaces.md`'s
Revision block for the full rationale and the channel's stated limits.

**The adjoint, and why it's worth the indirection.** Output PSD is `Σ_k |Z_k(jω)|²·S_k` over
uncorrelated sources. The direct route costs one linear solve *per source per frequency*: inject
a unit current across source `k`, read the output. The adjoint gets all of them from **one**
solve per frequency — solving `Aᵀ·y = e_out` makes every transfer impedance a subtraction,
`Z_k = y_p − y_n`, because `e_outᵀ·A⁻¹ = (A⁻ᵀ·e_out)ᵀ = yᵀ`. Note it is the *plain* transpose,
not the conjugate one: the identity is bilinear, nothing here is a complex inner product. The
`2n×2n` real block embedding T5.1 already used for the complex solve was factored out and reused
with the blocks transposed, so there remains exactly one place in the codebase where the
complex-to-real convention lives.

**Validated at three levels, not just against golden.** Closed form first: a lone resistor's
output PSD is `4kTR` — a check with real content, since the source is `4kT/R` and the transfer
impedance is `R`, so `R` *inverts* between them and getting either wrong yields `4kT/R` or
`4kTR³`. Then an RC low-pass shaping that same thermal noise by `1/(1+(ωRC)²)`, which is what
exercises the charge channel's contribution to the adjoint (with `C` ignored the spectrum would
come out flat). Then the real circuit against hand-derived physics, and finally against QSPICE.

**Measured**: `circuits/diode_noise.net` at `1.7e-5` relative on the PSD (tol `1e-3`), with the
absolute value `1.9877e-18` V²/Hz agreeing with QSPICE to five figures and the integrated total
`4.4584 µV` rms matching QSPICE's own printed figure exactly. See `docs/validation.md`'s
noise-gate section for why the gate has teeth and the two traps it avoids (a compiled model
silently contributing no noise; the circuit-scale error floor making a PSD comparison vacuous).

**Stated limits, all deliberate and all additive to fix**: white sources only (no flicker
channel); output-referred only (no `S_out/|H|²` input-referral, which QSPICE does report); no
per-device breakdown in the output; and no noise from `va-codegen`-generated models until
Verilog-A's `white_noise()`/`flicker_noise()` are lowered — the reason the gate's circuit uses
hand-written reference devices.

**Two of those four closed 2026-08-01b (T1/T2 noise lowering)** — see the phase entry below.

**Tutorial**: `t5-acnoise/02-noise.qmd`, written 2026-08-01 — carries the full adjoint
derivation (why one solve per frequency suffices, and why it is the plain rather than the
conjugate transpose), the argument for why noise needed its own ABI channel, and both
green-but-meaningless-gate traps as a worked lesson about tolerance constants carrying implicit
assumptions about scale.

**T5.1 and T5.2 are complete** — both implemented, golden-gated, and documented.

### Phase T5.3 — Verilog-A `white_noise()`/`flicker_noise()` lowering (T1/T2)

**Implemented and golden-gated 2026-08-01b.** Closes the two limits T5.2 stated as "additive to
fix": a `va-codegen`-compiled model now contributes real noise, and Interface β carries a
flicker channel. They were the same gap seen from two ends — lowering `flicker_noise()` is
pointless if the ABI can only carry white sources, and a flicker channel is untestable if no
model can declare one — so they closed together.

**T1 (`va-ir`/`va-frontend`)**: the frontend already *lexed* both functions and elaborated them
to `Expr::Const(0.0)`. That fold was correct for DC/transient/AC and destroyed exactly the
information noise analysis needs. Now `Builtin::WhiteNoise`/`FlickerNoise` are real IR calls
carrying their argument expressions, with the optional string label dropped (no per-source
breakdown is reported, and dropping it keeps every `Expr::Call` argument a number rather than a
string). `noise_table` keeps the old fold — its piecewise-linear PSD has no ABI channel, and
pretending otherwise would silently drop a declared source.

**T2 (`va-codegen`)**: noise terms are split out of a contribution exactly as `ddt` is split into
the charge channel — same `collect_terms` flattening, same "top-level additive term" rule. The
LRM's "value is zero outside noise analysis" becomes a single `ad::eval` arm returning
`Dual::constant(0.0)`, which is why adding a noise line to a model leaves its residual and
Jacobian **bit-identical** (asserted directly in `va-codegen`'s tests). `GeneratedModel::noise`
then walks the same control flow `load` does, so a source declared inside an `if` arm is emitted
only when that arm is taken; `run` was refactored into a generic `walk` to share that traversal
rather than duplicate it.

**One deliberate non-feature**: a *scaled* or nested noise call (`2*white_noise(p)`) is
**rejected at build time**, not accepted. A factor around the call would have to be applied as
its square to the PSD, and a model author writing that almost certainly means "twice the power."
Rather than guess, `validate` refuses — the alternative is a declared noise source that
evaluates quietly to zero and contributes nothing.

**Gated by two new circuits**, both driven through `--model` so the noise comes from the compiled
`.va` alone: `resistor_noise_va.net` (compiled thermal noise, **exact** agreement with QSPICE)
and `diode_flicker.net` (compiled shot + `1/f`, `1.7e-5`). The latter is the zoo's only *shaped*
spectrum — 209× across the band — which is what gives it teeth: a white-only implementation
would be ~99.5% wrong at 10 Hz. `models/constants.vams` is new (`diode.va` had `include`d it for
months without it existing), with exact SI 2019 values matching `va_abi::noise`'s own so a
compiled model and its hand-written counterpart agree to the last digit.

**Remaining T5 limits after this phase**: output-referred only, no per-device breakdown, and
`noise_table` unlowered. The first of those closed next — see T5.4.

### Phase T5.4 — Input-referred noise

**Implemented and golden-gated 2026-08-01c.** `S_in = S_out / |H|²`, matching QSPICE's own
`inoise_spectrum`.

**It needed no second linear solve**, which is the interesting part. The forward gain from the
`.noise` card's input source is *already* a component of the adjoint vector T5.2 solves for: an
ideal source of AC magnitude 1 excites the system at its own branch-current row `k`, so
`H = e_outᵀ·A⁻¹·e_k = yᵀ·e_k = y_k`. The same `y` that gives every noise source its transfer
impedance gives the forward gain by indexing. Input-referral is one division per frequency.

The `.noise` card's input-source name — parsed since T5.2 and deliberately unused, with a doc
comment saying it was there for exactly this — is now resolved to that branch row. Naming
something that isn't a voltage source is a clear error rather than a silently output-only answer.

**Verified before any golden existed**, against the QSPICE probe that motivated the feature: its
`inoise/onoise` ratio of `25.0306` implies `|H| = 0.199878`, matching
`golden/diode_ac.golden`'s independently-computed AC gain for the same network to six figures;
the integrated total agrees at `22.30538 µV` rms against QSPICE's printed `22.3055 µV`.

**The golden format gained a column** (`@noise <output> <source>`, rows
`<f> <output psd> <input psd>`), and the two columns are scored **separately**, each against its
own peak: the input-referred column is larger by `1/|H|²`, so a shared near-zero floor would be
set by whichever is bigger and under-check the other. An input-referred-only failure is
diagnostic — it implicates the transfer function, since nothing else distinguishes the columns.
A zero-gain frequency reports `inf` rather than `0` (referring noise to an unreachable input is
undefined; `0` would read as "no noise"), and the integrated total skips non-finite points.

One measured number moved: `resistor_noise_va.net` was exactly `0.0` and is now `1.4e-16`, since
the new column is a division. Still machine precision.

**Remaining T5 limits after this phase**: no per-device breakdown, and `noise_table` unlowered.
The first of those closed next — see T5.5.

### Phase T5.5 — Per-device noise attribution

**Implemented and golden-gated 2026-08-01d.** The output spectrum now breaks down by *which
device produced it* — the answer to "where is my noise coming from?", and the one a designer
acts on. Matches QSPICE's own `onoise_<dev>` columns.

**Device identity came from position, not from the ABI.** A `va_abi::ModelInstance` has no name
and `NoiseSink` receives only `(p, n, psd)`, so there was a real question of where attribution
could come from without a third §6 change. The answer: `va-acnoise` polls instances in order and
tags each source with the emitting instance's index; `va-cli` maps that index back to a device
name, which is sound because `build_instances` pushes exactly one instance per netlist device,
in order. That coupling was previously implicit and is now stated and guarded in
`va_cli::noise_contributors`.

Positional attribution is also *exact* where a topological one would fail: two identical
resistors in parallel emit sources with the same `(p, n)` and the same PSD, and stay
distinguishable anyway. A test pins that.

**Attribution is per device, not per mechanism** — a diode contributing both shot and flicker
noise reports one combined figure. QSPICE splits `onoise_d1` further into `.id`/`.1overf`/`.rs`;
reproducing that would mean naming each model's internal call sites, which this project has no
representation for. Only the aggregate column is read from QSPICE.

**The gate got stricter and the numbers moved to say so**: `diode_noise.net` went from `1.7e-5`
to `2.6e-5`, because each column is now scored on its own and errors that partially cancelled
inside the summed total no longer can. Every column is floored against its own peak — a quiet
device's column can sit orders below the total, so a shared floor would under-check it.

**It demonstrates its own value on `diode_flicker.net`**: `D1` falls from `4.15e-16` to
`1.33e-18` across the band while `R1` stays flat at `6.62e-19`, so the `1/f` roll-off is visibly
*in the diode* — something the summed total could only imply.

One latent bug fixed on the way: `xtask::va_model_for` never chained `AC_CIRCUITS` or
`NOISE_CIRCUITS`, so golden generation for those loaded them *without* their `--model`. It
happened to work because every affected device had a `va-abi` reference fallback; it would have
failed outright for `diode_flicker`, whose model has none.

**Remaining T5 limit**: `noise_table` unlowered (it needs a third `NoiseSink` method for a
piecewise-linear PSD). *Closed 2026-08-04 — see T5.6, immediately below.*

### Phase T5.6 — `noise_table()` lowering (T1/T2, Interface α+β)

> **Status: ✅ complete (2026-08-04)** — Verilog-A's `noise_table()` (LRM §4.6.4.3) lowers
> end to end and is golden-gated against QSPICE: `circuits/resistor_noise_table.net` passes at
> **1.859e-16** relative, taking `cargo xtask validate` to 12 circuits, 12/12 convergence (13
> after T5.7, below).
> This closes the last limit T5.5 stated, and with it the whole noise-function family — all
> three of Verilog-A's noise builtins are now real.

**Two coordinated interface changes**, both additive, both recorded in `docs/interfaces.md`:
Interface α gained `Builtin::NoiseTable`, and Interface β gained `NoiseSink::table_current`
plus a `TableInterp` enum. Each is one new variant/one default method, so nothing downstream
needed touching to keep compiling.

**Where the work actually goes: elaboration, not the ABI.** Unlike `white_noise`/`flicker_noise`,
whose arguments are expressions evaluated per bias, a `noise_table`'s argument is *data* — the
LRM restricts it to an array parameter or an array assignment pattern. So the frontend does
everything table-shaped exactly once, at the one point that still has a source file to name in
an error message: const-fold each entry, reject an odd count / a repeated frequency (the LRM
demands uniqueness) / a negative frequency or power / the unimplemented file-name form, then
**sort ascending** ("the simulator shall internally sort the pairs into ascending frequency if
required"). Everything downstream reads a table that is already valid and ordered, and
`va_abi::noise::table_psd_at` documents that as its precondition rather than re-checking it per
frequency.

**The table travels as flattened `Const` call arguments**, `Call(NoiseTable, [f1, p1, f2, p2, …])`
— not as a new `Expr` variant owning a `Vec<(f64,f64)>`. That is the decision that kept this a
one-variant IR change: every arena walk, clone and validity check in the pipeline already
handles a `Call` with constant arguments. `NoiseTerm::Table` likewise stores the *call's*
`ExprId` rather than an owned point list, so a `NoiseTerm` stays `Copy` however long the table
is; `GeneratedModel::noise_table_points` re-reads the pairs out of the arena at emit time.

**Both LRM interpolation rules went in together, deliberately.** `TableInterp::Linear`
(`noise_table`, piecewise-linear in `f`) is what the lowered builtin uses; `TableInterp::Log`
(`noise_table_log`, §4.6.4.4, piecewise-linear in `log₁₀ f`/`log₁₀ power`) is implemented and
tested alongside it even though **`noise_table_log` is not yet a lexer token**. The reason is
narrow and worth stating: the two functions differ only in that rule, so shipping one rule would
have guaranteed a *fourth* §6 interface event the moment anyone wired the second spelling.
Implementing both costs ~15 lines of `va-abi` and widens no user-visible surface — no new
Verilog-A construct is accepted by this change.

**Also normative, and easy to get backwards**: outside the tabulated range the LRM *clamps* to
the nearest endpoint's power rather than extrapolating, and the log rule falls back to linear
across any segment with a zero-power endpoint (`log(0)` is undefined, and a band where a model
declares no noise is legal data). Both are tested; the fallback is per segment, so one zero
point never degrades the rest of a table.

**The gate, and an honest account of what it proves.** `models/resistor_noise_table.va` is
`models/resistor.va` with its `white_noise(4kT/R)` rewritten as a three-point table of that same
`4kT/R`, and `circuits/resistor_noise_table.net` puts two of them in series so the answer is the
textbook `4kT·(R1‖R2)` — which a *plain QSPICE resistor pair* reproduces exactly, no `.model`
translation needed. The comparison is essentially exact (1.9e-16), and it pins the whole path:
frontend → IR → codegen → Interface β → adjoint solve → harness. But a flat table cannot tell
the three interpolation rules apart — clamping and extrapolating agree on a constant — so
**discriminating them is the unit tests' job**, and those use deliberately shaped tables: the
LRM's own §4.6.4.3 example table read between decade points (which catches a log-interpolating
implementation), a two-point `1/f` log table (Figure 4-9's own example), an unsorted table, a
zero-power segment, and a `va-acnoise` sweep over a rise-then-fall table read entirely between
its knots. A flat table is the only shape QSPICE has a native primitive to compare against at
all; splitting the duties this way is the honest resolution, not a gap.

**Two limitations this creates for model authors**, both stated in
`models/resistor_noise_table.va`'s header because they are surprising: a const-folded table
cannot track `$temperature` (so a table is only right at the temperature it was written for —
use `white_noise` if the PSD must follow temperature), and it cannot track the resistance
`va-cli` overrides onto a compiled model's first parameter from an `R` line, because that
happens after elaboration. The gate deck uses two 1 kΩ resistors rather than the 1 k/3 k of
`resistor_noise_va.net` for exactly this reason, and says so in its header.

**No corpus movement, and that was expected**: not one of the 150 `external/` files calls
`noise_table` (checked, not assumed). This closes a stated T5 limit and an LRM construct, not a
corpus failure — the same reason `ground` declarations and escaped identifiers were added.

### Phase T5.7 — `noise_table_log()` lowering (T1/T2, Interface α)

> **Status: ✅ complete (2026-08-05)** — `noise_table_log()` (LRM §4.6.4.4) is lexed, reserved,
> and lowered; `circuits/resistor_noise_table_log.net` passes at **1.859e-16** vs QSPICE,
> taking `cargo xtask validate` to **13 circuits, 13/13 convergence**. With this, the noise
> family is complete: every noise function Verilog-A defines is implemented, including both of
> the LRM's interpolation rules.

**It cost one Interface α variant and nothing else** — which was the whole point of shipping
`TableInterp::{Linear, Log}` together in T5.6 a day earlier. Interface β was untouched, so this
was not a coordinated multi-crate event at all; the bet that "both rules now, one spelling
later" would avoid a fourth §6 change paid off exactly as stated.

**It was also missing from `RESERVED_WORDS`** (182 → 183 words). That is a real omission rather
than a deliberate exclusion: the Accellera LRM v2.4.0 reserves `noise_table_log` right beside
`noise_table`, and the LRM's own revision history dates the *function* to 2.4 — so the older
document this table was first transcribed from simply predates it. Same category as the
`floor`/`ceil`/`round`/`int`/`limexp` additions earlier in T1.

**A separate `Builtin` variant, not an interpolation flag.** The two are separate LRM functions
with separate spellings, and a flag argument would have to be smuggled into the flattened
argument list as a magic `Const` sitting among real `(frequency, power)` data — indistinguishable
from a table entry to every generic arena walk that currently needs no special case at all.
`lower::NoiseTerm::Table` resolves the variant to a `TableInterp` once, at lowering, so nothing
downstream re-inspects the call.

**One validator, two names.** Both spellings share `noise_table_points`, since the LRM imposes
identical requirements on the table itself and differs only in interpolation — but the
diagnostics now name whichever function the author actually wrote, rather than always saying
`noise_table`. A test pins that, because a message naming the wrong function is the kind of
small wrongness that survives for years.

**The gate, and again what it does not prove.** `circuits/resistor_noise_table_log.net` is
`resistor_noise_table.net` with one word changed in the model, and its golden is deliberately
the *same physics*: on a flat table the LRM's two rules must agree exactly, and checking that
they do is the point. What it adds over the linear deck is that the logarithmic path — logs, a
power, and §4.6.4.4's formula — runs on every point of a real sweep against a real oracle, where
a NaN, an infinity, or a badly-conditioned exponentiation would surface. It does **not** check
the interesting half of `noise_table_log`, that two points describe an exact power law; QSPICE
has no arbitrary-PSD primitive to compare that against, so it stays pinned by the unit test over
the LRM's own Figure 4-9 example (`'{1,1, 1e6,1e-6}` → exactly `1/f`), alongside a codegen test
that the same table read under the two rules genuinely diverges between its knots (`1e-3` vs
`~1.0` at 1 kHz) while agreeing at them.

**Still no corpus movement**: no `external/` file calls either table function.

---

## T6 — `va-netlist` + `va-cli` + `va-harness` (integration & validation)

**Shared substrate — staff first, reliable student (§10).** This thesis owns three crates and
is the glue: it makes everyone else's work runnable and trustworthy.
**Fallbacks:** netlist-format design note · pipeline integration/UX report · validation-
methodology + metrics report vs ngspice.

### Phase T6.1 — Netlist parser & the harness/metrics skeleton
> **Status: ✅ complete** (2026-08-04: T6.3's gate is closed, and every one of the 13 gated
> decks is parsed by this crate) — `va-netlist/src/parser.rs`
> is a real line-oriented SPICE-flavored parser: `R`/`C`/`D`/`V` elements (SI-suffixed values,
> `k`/`meg`/`u`/`n`/`p`/…), `0`/`gnd` as the reference sentinel, and dot-cards (`.op`/`.dc`/
> `.tran`/`.ac`). **2026-07-06: `.tran <tstep> <tstop>` timing is now captured**
> (`Netlist::tran`), not just the card marker — needed once `va-cli` actually drives a
> transient run (see T6.2). `va-harness`'s metric functions (`DC_REL`, `TRAN_RMS`) are declared
> but still `todo!()` at the time — since resolved for real, T6.3. `va-netlist` also gained a
> `'Q'` element (BJT, § ladder rung 6) on 2026-07-18, alongside `'M'`'s (§ ladder rung 5).
> `t6-integration/01-netlist.qmd` written 2026-07-18.

- Circuit-level netlist parser (`va-netlist`): elements, nodes, model bindings, analysis
  directives. Define the metric functions in `va-harness` (`DC_REL`, `TRAN_RMS`, …).
- **Tutorial:** `t6-integration/01-netlist.qmd` — the netlist format and how a circuit maps
  onto Interface β instances.

### Phase T6.2 — CLI wiring & golden generation
> **Status: ✅ complete** (2026-08-04: golden generation is real — `xtask gen-golden` shells out
> to QSPICE, and all 13 gated circuits run through `va-cli sim`) — `va-cli sim` already
> wired DC end to end before this pass: `--model <m.va>` compiles through the real
> `va-frontend` → `va-codegen` pipeline (including module instantiation — see
> `hierarchical_divider_solves_through_codegen_pipeline`), falling back to `va-abi` reference
> primitives for unmatched devices, then `va-core::dc::operating_point` solves it.
> **2026-07-06: transient is wired too** — `va-cli sim <deck> --tran` runs the same device-
> building path through `va_transient::integrator::run` over the deck's `.tran` window,
> reported via a new `report_transient`. Always starts from the zero vector (v0 has no `.ic`/
> `UIC` support): for a plain `DC`-valued source that cold start plus the constant source *is*
> the step response — exactly what `circuits/rc_step.net` (a step voltage into an R/C)
> exercises: `cargo run -p va-cli -- sim circuits/rc_step.net --tran` matches the analytic
> `V(t)=Vs·(1−e^{−t/RC})` closely (4.966 V vs analytic 4.9663 V at `t=5·RC`).
> **Same day, second update: `SIN(...)` sources are wired too** — `va-netlist` now retains a
> `V` line's full `(offset, amplitude, freq)`, not just the DC offset it collapses to for a DC
> solve, as `Device::waveform`. `va-cli`'s new `build_instances_split` separates a
> waveform-carrying `vsource` from every other (fixed) device, and `solve_transient` hands it
> to the new `va_transient::integrator::run_dynamic` (see T4.2), which rebuilds that one
> source fresh at each step from the waveform instead of the fixed-instance path everything
> else uses — needed because Interface β has no time parameter for a device to read a
> waveform from directly (§7, T4.2's update above). Verified against
> `circuits/rectifier.net`: `cargo run -p va-cli -- sim circuits/rectifier.net --tran`

> **Superseded 2026-08-06:** `run_dynamic` and `build_instances_split` are **deleted**. Interface β now carries an `AnalysisCtx` (time + analysis kind), so a `SIN` source is an ordinary stateless `ModelInstance` reading `ctx.time` (`va_cli::WaveformSource`) and every device takes the same path — see this file's "Analysis context — Tier A" section. The reasoning below is kept as history; the mechanism it describes is gone.
> produces a textbook half-wave-rectified, RC-filtered waveform — `V(out)` never follows
> `V(in)`'s swing to −5 V, peaks near 4.3 V (5 V minus a silicon diode drop), and shows the
> expected ripple decay between cycles, all driven through the real frontend/netlist/core/
> transient pipeline, no golden reference needed to see it's doing the right thing.
> `xtask gen-golden`/`xtask validate` remain unimplemented at the time (T6.3/`xtask` territory)
> — since built for real, closing rungs 1-5. `t6-integration/02-cli.qmd` written 2026-07-18.

- `va-cli` wires the full pipeline (parse model → codegen → assemble → solve → report); flesh
  out `xtask gen-golden` (ngspice) and `xtask validate`.
- **Validation gate:** `cargo run -p va-cli -- sim circuits/divider.net …` reproduces ladder
  rung 1 end-to-end through the real pipeline.
- **Tutorial:** `t6-integration/02-cli.qmd` — driving the simulator, the golden-generation
  workflow.

### Phase T6.3 — Full validation harness & the metrics dashboard
> **Status: ✅ complete — all six ladder rungs formally pass against real, committed QSPICE golden, as of
> 2026-07-18** (rung 6 closed last, and needed two real, sequential fixes — a genuine QSPICE
> ground-aliasing bug for `Q`-element terminals, then an honest early-window comparison scoped to
> where this circuit's unstable equilibrium stays deterministically comparable at all — § T4.3's
> 2026-07-18 entries have the full account). The two gaps this note used to list as outstanding
> have both since closed: branch-current golden (`e6b06b0`, real `I(device)` checks vs QSPICE)
> and the convergence-fraction dashboard (`3d31a9f`, § T6.4).
> `va-harness::metrics::{max_relative_error, rms_error}` are real implementations now (were
> `todo!()`), including the near-zero-reference division guard `max_relative_error`'s own doc
> comment always specified but never implemented; 10 tests. `va-harness::golden::GoldenDc` is a
> small, documented `.golden` text format (`<node> <value>` per line) with a tested
> parse/read/render round-trip. `va-harness::dc::{run_dc, compare_dc}` drive a single DC
> operating point through `va-cli`'s pipeline and diff it against a `GoldenDc`, erroring (not
> silently comparing unrelated data) on a node-order mismatch. This needed one small, additive
> `va-cli` change: `solve_dc` is now `pub`, and the netlist-parse/model-compile prelude
> previously inlined in `run_sim` is its own `pub fn load` (which `run_sim` itself now calls, no
> logic duplicated) — so `va-harness` gets real `OperatingPoint` values back, not `run_sim`'s
> printed stdout to re-parse. `cargo xtask validate` is wired for real over a small, explicit
> `DC_CIRCUITS` table (`divider.net`, `mos_dc.net` — the two single-`.op`-point circuits; a
> `.dc` sweep (rung 2) or `.tran` deck (rungs 3/4/6) isn't wired to golden yet, a stated scope
> limit, not an oversight): a circuit with no committed `golden/<name>.golden` is *skipped*, not
> failed, matching this project's actual state — `golden/` is still empty (see below). Manually
> verified all three outcomes work (pass/fail/skip), including planting a deliberately-wrong
> golden value and confirming `validate` correctly reports `FAIL` and a nonzero exit.
> `cargo xtask gen-golden` is **not** implemented — it now fails with a clear, honest message
> instead of a bare `todo!()` panic, but actually shelling out to ngspice needs a
> `circuits/*.net` → ngspice-deck translator this pass didn't attempt, and this environment has
> no ngspice installed to develop *or verify* that translator against (confirmed, not assumed —
> `cargo xtask gen-golden` here reports "ngspice not found on PATH"). **This is the honest
> reason no `golden/*.golden` file is committed by this pass**: fabricating one by hand from
> this project's own analytic/hand-derived reference values (already used throughout this
> session's own unit tests) would misrepresent it as ngspice output, exactly the "ngspice is the
> oracle" methodology `CLAUDE.md` §7 establishes — better to leave `golden/` honestly empty than
> to launder a hand-computed value as a golden reference.
> *Outstanding at the time:* the ngspice deck translator + `gen_golden`'s real implementation
> (still blocked on having ngspice available to develop against); `.dc`-sweep golden support;
> `.tran`-waveform golden support; a per-rung/convergence-fraction dashboard;
> `t6-integration/03-validation.qmd`.
>
> **`.dc`-sweep golden support: closed the same day.** `golden::GoldenSweep` (a header line
> naming the swept source and every node, then one `<value> <node value>...` row per point) and
> `dc::{run_dc_sweep, compare_dc_sweep}` extend the same pattern `GoldenDc`/`run_dc`/`compare_dc`
> already established, reusing `max_relative_error` over every point's node voltages flattened
> into one series. Needed `solve_dc_sweep` (already implemented for T6.2's rung-2 CLI wiring)
> made `pub`, mirroring `solve_dc` — no other `va-cli` change. `xtask validate` now also drives a
> `SWEEP_CIRCUITS` table (`diode_iv.net`); manually verified all three outcomes again
> (pass/fail/skip) the same way, including a deliberately-wrong sweep point. Still no
> `golden/*.golden` committed, for the identical ngspice-provenance reason as the DC case above.
> *Now outstanding:* `.tran`-waveform golden support (needs `rms_error`'s still-unwritten
> shared-timebase resample step first) and everything else in the paragraph above.
>
> **The oracle switched from ngspice to QSPICE, and rung 1 is now formally passed — same day.**
> The project decision changed (QSPICE, not ngspice, per the actual dev environment), and unlike
> the earlier ngspice search, **QSPICE turned out to already be installed**
> (`C:\Program Files\QSPICE\QSPICE64.exe`) — confirmed by running it, not by finding the
> directory. Three things fell out of actually trying it: (1) `circuits/divider.net` runs
> through QSPICE **completely unmodified** — it's SPICE-flavored `.net` syntax already, no
> deck-translation layer needed for a pure `R`/`C`/`V` circuit; (2) QSPICE's `.qraw` output is an
> ASCII header (`Title:`/`Plotname:`/`No. Variables:`/a `Variables:` block/`Binary:`) followed by
> one little-endian `f64` per variable — the same shape ngspice's own `.raw` format uses, and
> genuinely parseable; (3) running it on `divider.net` gives `V(in)=1`, `V(mid)=0.5` — bit-for-bit
> the value this project's own pipeline already computes. `xtask::{find_qspice, run_qspice_op,
> parse_qraw, golden_dc_from_qraw}` turn that into a real `gen_golden()`: locate `QSPICE64.exe`
> (`QSPICE_PATH` env var, then `PATH`, then the standard install location), run it on a scratch
> copy of the deck, parse the `.qraw`, look up each of this project's own `node_order` names by
> their `"V(name)"` label (QSPICE's own variable ordering isn't assumed to match), and write a
> `GoldenDc`. **`golden/divider.golden` is now a real, committed, QSPICE-generated reference** —
> `cargo xtask validate` reports `PASS circuits/divider.net: error=0.000e0 (tol 1e-4)`. This is
> the project's first rung to satisfy `CLAUDE.md` §7's actual definition of "passed," not just
> "implementation reach."
>
> **`circuits/mos_dc.net`/`diode_iv.net` are deliberately still not regenerated** — investigating
> exactly why surfaced a second real, useful finding: QSPICE's default simulation temperature is
> 27°C (300.15 K), while this project's codegen fixes a single constant pair
> (`va_codegen::TEMP=300.0`, `VT=0.025_852`, i.e. exactly 300 K) for every model's `$vt`/
> `$temperature`. Irrelevant for a linear circuit; not for `diode.va`'s exponential law — a
> forced-0.5 V diode measures `2.50974869898304e-6` A from this project's fixed-300 K model
> against `2.48560822992004e-6` A from QSPICE's native diode model at its default 27°C, a ~0.85%
> relative difference, well past `DC_REL`'s `1e-4`. Forcing QSPICE's `.temp` to exactly 300 K
> does **not** close the gap — it opens a *different* one: standard SPICE diode models rescale
> `IS` relative to their own nominal temperature (`TNOM`, defaulting to the simulation's own
> default 27°C) whenever `.temp` differs from it, so moving `.temp` away from `TNOM` invokes that
> rescaling rather than landing on a cleanly-matched answer (confirmed empirically: `.temp 26.85`
> — literally `300 K` — gave an *even further* mismatched implied `Vt`, `0.0258827` instead of
> the expected `0.0258520`). `QSPICE_NATIVE_CIRCUITS`'s doc comment carries the full derivation.
> Fixing this for real is a genuine, scoped next step — most likely moving this project's own
> fixed thermal-voltage convention to the 300.15 K SPICE-standard value — but it touches
> `va-codegen`'s constants and every test that hardcodes a value derived from them, so it wasn't
> done as a side effect of this pass.
>
> **Rungs 2 and 5 now formally pass too (2026-07-17)** — both blockers above are closed.
> (1) *The temperature convention fix*: `va_codegen::TEMP`/`VT` (and every `VT_300K` reference
> constant across `va-abi`/`va-core`/`va-cli`/`va-transient`, renamed `VT_NOMINAL`) moved from
> a bare 300 K/`0.025_852` to QSPICE's own default `TNOM`, 300.15 K/`0.025_865` — the least
> invasive fix available: aligning this project's own fixed constant to the oracle's default
> rather than fighting QSPICE's per-model `TNOM` rescaling behavior (§ the paragraph above).
> (2) *QSPICE-native `.model` translations*: `xtask::{QSPICE_MODEL_TRANSLATIONS,
> QSPICE_SWEEP_MODEL_TRANSLATIONS, translate_for_qspice}` hand-translate `models/mosfet.va`'s
> Level-1 square-law and `models/diode.va`'s Shockley law into SPICE-native `.model mosfet
> NMOS(LEVEL=1 VTO=0.7 KP=200u LAMBDA=0.01 W=10u L=1u)` / `.model diode D(IS=1e-14 N=1)` cards —
> both are exact one-to-one parameter-name translations, not approximations, since both `.va`
> models were themselves written to reproduce the textbook SPICE equations. `translate_for_qspice`
> also widens this project's simplified 3-terminal `M<name> d g s model` line into QSPICE's native
> 4-terminal form by tying body to source (matching `mosfet.va`'s own no-body-effect scope).
> A real SPICE gotcha caught this mid-pass, worth recording: **a deck's first line is
> unconditionally its title in every SPICE dialect**, including QSPICE — the first version of
> `translate_for_qspice` prepended the `.model` card as line 1, which QSPICE silently read as the
> title string instead of a directive, printed `Didn't find a model for "MOSFET" -- defaults
> assumed`, and solved `mos_dc.net` against a generic built-in NMOS instead (`V(d)=4.96`, nowhere
> near the analytic ~3.255 V) — caught by manually sanity-checking the regenerated golden against
> the netlist's own hand-derived comment before committing it, not by trusting a clean `xtask
> gen-golden` exit code. Fixed by inserting the `.model` card as line 2, after the deck's own
> title/comment line. `xtask::{QspiceRawSweep, parse_qraw_sweep, run_qspice_sweep,
> golden_sweep_from_qraw}` add multi-point `.qraw` parsing (point-major payload layout, confirmed
> empirically against a real translated `diode_iv.net` run) so rung 2's `.dc` sweep — not just
> rung 5's single `.op` point — can regenerate at all; `parse_qraw`'s single-point path and the
> new sweep path now share one `parse_qraw_header` rather than duplicating the header scan.
> `golden/mos_dc.golden` (`V(d)=3.25499065144549`, matching the netlist's own hand-derived
> `3.254991…` to 7 figures) and `golden/diode_iv.golden` are now real, committed QSPICE output;
> `cargo xtask validate` reports `PASS` for both (`error=1.977e-9` and `error=1.850e-16`
> respectively, both well inside `DC_REL`'s `1e-4`).
>
> **An honest caveat on rung 2's actual coverage**, already flagged in `va-harness::dc`'s own doc
> comments before this pass and unchanged by it: `GoldenSweep`/`GoldenDc` record node *voltages*
> only, never branch currents (`GoldenDc::from_operating_point` deliberately drops a source's own
> branch-current unknown). `circuits/diode_iv.net`'s only node is `in`, directly forced by `V1` —
> so `V(in)` trivially equals the swept value regardless of whether the diode model is right at
> all, and rung 2's QSPICE cross-check, as currently shaped, doesn't actually exercise the diode's
> exponential law. The real Shockley-law cross-check for this circuit is (and remains)
> `va-cli`'s own `diode_iv_sweep_solves_through_codegen_pipeline` test, which checks every point
> against the closed-form `Id(V)` — not this rung's golden comparison. Extending the golden format
> to carry device/branch currents (so a QSPICE cross-check could catch a genuinely wrong `Is`/`N`,
> not just a wiring bug) is real future work, not attempted here.
>
> *Now outstanding:* `.tran`-waveform golden support (rungs 3/4/6); a per-rung/convergence-fraction
> dashboard; refreshing `t6-integration/03-validation.qmd` for the three now-real golden files;
> the branch-current golden gap noted just above.
>
> **`.tran`-waveform golden support closed for rungs 3/4 (2026-07-18)** — rung 6 (ring
> oscillator) still has no `circuits/*.net` deck to drive at all (its demo builds `va-abi::Bjt`
> instances directly in a `va-transient` test; `va-netlist` has no BJT netlist element yet, a
> real, scoped gap, not attempted here). `golden::GoldenTran` (a `@tran`-marked table, time in
> place of a sweep's swept value — the `@tran` marker exists so a `.golden` file's own text says
> unambiguously which of the two table-shaped parsers it needs) and `metrics::resample_linear`
> (piecewise-linear, clamped outside its source's own covered range) extend the `GoldenDc`/
> `GoldenSweep` pattern to a transient waveform; `tran::{run_tran, compare_tran}` mirror `dc.rs`'s
> shape, needing one small additive `va-cli` change (`solve_transient` made `pub`, identically to
> `solve_dc`/`solve_dc_sweep` before it). `xtask` gained `TRAN_CIRCUITS`/multi-point `.qraw`
> handling for `.tran`: it turns out to need **no new `.qraw` parsing at all** — a `.dc` sweep and
> a `.tran` run are both just "point-major multi-point `.qraw`" to `parse_qraw_sweep`, which
> rung 2 already built; only `golden_tran_from_qraw` (keyed off QSPICE's always-present `Time`
> variable, vs. `golden_sweep_from_qraw`'s swept-source-name key) is new.
>
> Two more real, empirically-found mismatches had to be closed before rungs 3/4 actually
> validated anything, both caught by sanity-checking the regenerated golden against a
> hand-derived expectation rather than trusting a clean run (the same discipline the earlier
> title-line bug — § this section's 2026-07-17 entry — established):
>
> 1. **QSPICE solves the DC operating point before a `.tran` run, by standard SPICE convention;
>    `va-transient` never does** (`va-cli::solve_transient`'s own doc comment: no `.ic`/`UIC`
>    support, always starts from the zero vector). An unmodified `circuits/rc_step.net` run
>    through QSPICE reported `V(out)` already at its settled ~5 V for the *entire* 5 ms window,
>    not climbing the RC charging curve from 0 — `cargo xtask validate` genuinely failed against
>    it. Fixed with `xtask::cold_start_tran_deck`: seed every reactive (`C`/`L`) element's own
>    `IC=0` device parameter and append `UIC` to the `.tran` card, SPICE's standard mechanism for
>    skipping the operating-point solve — confirmed against the analytic RC curve afterward
>    (`V(out)≈3.169` at `t≈1.004 ms` vs. `5·(1−e⁻¹·⁰⁰⁴)≈3.170`; `≈4.966` at `t=5 ms` vs.
>    `5·(1−e⁻⁵)≈4.9663`).
> 2. **`va_transient::integrator`'s `Waveform` always reports the caller's raw seed as its own
>    first sample** (`t: vec![cfg.tstart], x: vec![x0.clone()]`, before any integration step
>    runs) — for `rc_step.net`'s `V(in)`, held at its source value `5` at every genuinely solved
>    step but `0` in that raw seed, comparing it in raised the circuit's own error to `1.097e-1`
>    (bit-for-bit `5² / (2 × 1038 points)` under a square root — the whole error, concentrated in
>    one sample). Dropping just that one sample wasn't enough by itself, though: still resampling
>    *every* golden time — including QSPICE's own densely-clustered early adaptive-step
>    samples, far finer than `va-transient`'s own first real step — clamps that whole early
>    region to the first real `got` value, which is fine for `rc_step.net` (already settled) but
>    wrong for `rectifier.net`'s fast-changing early `SIN` waveform, raising *its* error from
>    `7.8e-4` to `3.1e-2`. `tran::compare_tran` now excludes both: `got`'s own seed sample, and
>    every golden sample earlier than `got`'s own first real solved time — the same "don't ask a
>    coarser series to explain a region it never resolved" principle applied to each side in turn.
>    Two regression tests (`compare_tran_ignores_gots_seed_for_an_algebraically_forced_node`,
>    `compare_tran_excludes_golden_samples_earlier_than_gots_first_real_step`) encode each half so
>    a future change can't silently reintroduce either failure mode.
>
> `golden/rc_step.golden` (1038 points) and `golden/rectifier.golden` (1065 points) are now real,
> committed QSPICE output; `cargo xtask validate` reports `PASS` for both (`error=2.260e-5` and
> `7.925e-4` respectively, both inside `TRAN_RMS`'s `1e-3` — `rectifier.net`'s margin is real and
> unpadded, not artificially loosened).
>
> *Now outstanding:* rung 6's golden gate closed the same day too (§ T4.3's 2026-07-18 entries —
> a genuine QSPICE ground-aliasing bug for `Q`-element terminals, then an honestly-scoped
> early-window comparison for this circuit's unstable equilibrium); refreshing
> `t6-integration/03-validation.qmd` for all six now-real golden files (done, same date); the
> branch-current golden gap (rung 2, noted above) — real, but scoped to its own T6.4 phase below
> and rung 2 respectively, not blocking T6.3 itself.
>
> **The branch-current golden gap closed for real, same day.** `GoldenDc::from_operating_point`/
> `GoldenSweep::from_sweep`/`GoldenTran::from_waveform` now take a fourth `branch_currents: &[(String,
> usize)]` argument (typically `va_cli::branch_currents(net, compiled)`'s own return, a new `pub`
> function alongside `build_instances`) and append one `I(<device name>)`-labeled entry per branch
> current after the node entries — the same flat `<name> <value>` file shape, `node_order` just
> isn't only node names anymore (`va_harness::golden`'s own doc comment has the worked example).
> Zero changes needed to any `compare_dc`/`compare_dc_sweep`/`compare_tran` comparison logic or the
> `.golden` parse/render format itself — this was a deliberate design choice to minimize blast
> radius. `xtask`'s own QSPICE-side mapping needed two changes: `node_values_from_row` now looks up
> an already-`"I(<name>)"`-shaped `node_order` entry *literally* (QSPICE spells a device's own
> current the same way in its own `.qraw` variable list) rather than wrapping it as `"V(...)"`; and
> a new `golden_node_order`/`va_model_for` pair resolves `circuit`'s own branch currents by
> compiling the *same* `--model` `va-harness`'s own `run_dc`/`run_dc_sweep`/`run_tran` will use at
> validate time (via `va_cli::load`), so a device with no hand-written `va-abi` reference (e.g.
> `mos_dc.net`'s `mosfet`) resolves correctly too — `xtask` gained a `va-cli` dependency for this.
>
> Regenerating all six golden files surfaced a genuine, useful side effect: two circuits'
> `error=` figures moved from their old (voltage-only, trivially-forced) values to new,
> substantively-checked ones — `mos_dc.net` from `1.977e-9` to `1.490e-6` (now checking `I(VDD)`/
> `I(VG)`, not just the two directly-forced source voltages), and `diode_iv.net` from `1.850e-16`
> to `6.656e-5` (now checking `I(V1)`, i.e. the diode's own current, against QSPICE's Shockley
> law for real). Both stayed comfortably inside `DC_REL`'s `1e-4`, but doing so needed a real fix,
> not just a rebuild: `max_relative_error`'s own near-zero-reference floor (`REL_ERROR_FLOOR`) was
> `1e-12`, calibrated back when only node voltages (solved to near-machine precision) ever hit it.
> Femtoamp-scale branch currents (`diode_iv.net`'s own `I(V1)` at `V1=0.1` — QSPICE's golden
> `~5.7e-13` A vs. this project's own `~4.7e-13` A) are both simulators' *own* Newton-residual noise
> floor, not a real model disagreement, but at `1e-12` that ~`1e-13`-scale absolute difference
> blew up to a spurious ~10% "error," and `mos_dc.net`'s `I(VG)` (a MOSFET gate current this
> Level-1 model has no pathway for — exactly `0` here, `~-1.5e-14` from QSPICE's own noise) did the
> same. Widened to `1e-8` (`va_harness::metrics::REL_ERROR_FLOOR`'s own doc comment has the full
> per-point empirical derivation) — clears every near-zero branch current in the zoo with room to
> spare while leaving every physically-meaningful current (everything from `diode_iv.net`'s own
> `~5.2e-8` A upward) checked against its own real relative precision, worst case `6.6e-5`, still
> well inside `1e-4`. `docs/validation.md`'s rung-2 scope-limit note is now closed, not just
> tracked as future work.

- `va-harness` runs the whole zoo vs `golden/`, reports per-rung pass/fail and the convergence
  fraction; resample-and-compare for transient.
- **Validation gate:** all passed ladder rungs are green under one `cargo xtask validate`.
- **Tutorial:** `t6-integration/03-validation.qmd` — the metrics, tolerances, and the
  ladder-status dashboard; how "done" is measured.

### Phase T6.4 — Convergence-fraction dashboard
> **Status: ✅ complete** (code 2026-07-18; marker refreshed 2026-08-04 — the dashboard now
> reports 13/13, and `t6-integration/04-convergence-dashboard.qmd` is written) — `CLAUDE.md` §7's fourth metric ("fraction of zoo
> circuits that reach a solution … it only ever needs to go up") had no real implementation
> before this: a circuit that failed to *converge* at all (not just mismatch golden) propagated
> a hard error out of `xtask::validate_{dc,sweep,tran}_circuits` via `?`, aborting the *entire*
> `validate` run before any circuit ordered after it was even attempted — so the "convergence
> fraction" was never actually computable from a real run, only from however much of the zoo
> happened to be reached before the first failure.

`xtask::Tally` now tracks three distinct outcomes, not two: `skipped` (no golden committed yet),
`not_converged` (the solver itself failed — a `CoreError` propagating out of `va-harness`'s own
`run_dc`/`run_dc_sweep`/`run_tran`), and `failed` (converged, but outside golden's tolerance).
`xtask::try_solve` is the seam: it calls the solve, and on `Err` prints `NOCONV <circuit>: <why>`
and records `not_converged` instead of propagating — the rest of the zoo is still attempted and
reported. `validate()`'s own final report now prints the convergence fraction as its own line:

```console
$ cargo run -q -p xtask -- validate
...
[xtask] validate: 9 checked, 0 failed golden, 0 did not converge, 0 skipped (no golden)
[xtask] validate: convergence 9/9 (100.0%) — CLAUDE.md §7's convergence metric
```

Every known circuit converges today (**13/13 as of 2026-08-05** — six ladder rungs plus T5's two
AC circuits and five noise circuits; the transcript above is the 2026-08-01 run, kept verbatim
rather than edited, from when the zoo was 9 circuits; unsurprising, since every one of them already passes
golden, a strictly harder bar), so this reads `100.0%` right now; the real deliverable is the
*mechanism* — verified with a genuinely non-convergent synthetic circuit (two nets joined by a
resistor with no path to ground anywhere, confirmed to produce `CoreError::Singular` via
`va_core`, not assumed), asserting `try_solve` records it as `not_converged` and returns `None`
rather than erroring the caller. `validate` still exits non-zero if anything didn't converge or
missed golden — the fix is that the *whole zoo gets reported first*, not that a real regression
stops failing the gate.

- Track convergence as its own outcome, distinct from golden-comparison pass/fail, across every
  known circuit — the number `CLAUDE.md` §7 says should only ever go up.
- **Validation gate:** `cargo xtask validate` reports a convergence fraction that reflects every
  known circuit, even when one fails to converge (verified with a synthetic non-convergent
  circuit, not just the passing zoo).
- **Tutorial:** `t6-integration/04-convergence-dashboard.qmd` — why "didn't converge" and
  "converged but wrong" are different failure modes, and why aborting the whole batch at the
  first one would have made the metric meaningless.

---

## Cross-thesis milestones (the bring-up ladder)

Each rung is a shared demo where the responsible theses present their tutorials together:

| Rung | Circuit            | Analysis  | Lights up                | Tutorials presented           | Status |
|------|--------------------|-----------|--------------------------|-------------------------------|--------|
| 1    | resistor divider   | DC        | T3 (+ T6 via CLI)        | T3.2, T6.2, shared            | ✅ **formally passed** — `cargo xtask validate` is green against `golden/divider.golden`, real QSPICE output, now also checking `I(V1)` (error=0.000e0, tol 1e-4) |
| 2    | diode I–V          | DC sweep  | T1, T2, T3               | T1.3, T2.2, T3.3              | ✅ **formally passed** — green against `golden/diode_iv.golden`, real QSPICE output via a native `.model diode D(...)` translation (error=6.656e-5, tol 1e-4); now also checks `I(V1)` (the diode's own current) against QSPICE's Shockley law for real — the former "voltage-only, doesn't exercise the diode" caveat is closed (§ T6.3's 2026-07-18 branch-current entry) |
| 3    | RC                 | transient | T4 (+ T2 charge)         | T2.3, T4.1                    | ✅ **formally passed** — green against `golden/rc_step.golden` (1038 pts), real QSPICE output via `UIC` cold-start, now also checking `I(V1)` (error=2.193e-5 under divided differences since 2026-08-31; 1.845e-5 as first gated, tol 1e-3) |
| 4    | diode rectifier    | transient | T4                       | T4.2                          | ✅ **formally passed** — green against `golden/rectifier.golden` (1065 pts), real QSPICE output via a native `.model diode D(...)` translation + `UIC` cold-start, now also checking `I(V1)` (error=8.226e-4 under divided differences since 2026-08-31; 6.766e-4 as first gated, tol 1e-3) |
| 5    | a MOS              | DC        | T1, T2, T3 (model reach) | T1/T2 coverage updates        | ✅ **formally passed** — green against `golden/mos_dc.golden`, real QSPICE output via a native `.model mosfet NMOS(...)` translation (error=1.490e-6, tol 1e-4); now also checks `I(VDD)`/`I(VG)` |
| 6    | ring oscillator    | transient | T4 (full stack)          | T4.3                          | ✅ **formally passed** — green against `golden/ring_osc.golden` (1041 pts, an honestly-scoped 0.1s window — § T4.3's 2026-07-18 entry), real QSPICE output via a native `.model bjt NPN(...)` translation + a `gnd`-to-`0` ground-aliasing fix, now also checking `I(VCC)` (error=**4.464e-6** since the 2026-08-31 first-step fix; 1.799e-4 before it, tol 1e-3); `cargo run -p va-cli -- sim circuits/ring_osc.net --tran` (full 0.2s) and `cargo test -p va-transient ring_oscillator_sustains_oscillation` (hand-built instances) both still demonstrate the full growing oscillation |

Stretch rungs for T5 (AC/noise) hang off rung 1–2 circuits (RC/RLC) once a DC operating point
is available. **These now exist and pass** (2026-08-01, § the T5 sections): six more gated
circuits beyond the six rungs above — `rc_ac` (1.3e-15 magnitude, 1.7e-13 rad phase),
`diode_ac` (1.3e-5 / 6.4e-6 rad), `diode_noise` (2.6e-5), `resistor_noise_va` (1.4e-16, a
compiled Verilog-A `white_noise()`), `diode_flicker` (2.6e-5, compiled shot + 1/f over a
209×-shaped spectrum), `resistor_noise_table` (1.9e-16, a compiled `noise_table()`, added
2026-08-04 with T5.6), and `resistor_noise_table_log` (1.9e-16, its `noise_table_log()` twin,
2026-08-05 with T5.7) — bringing `cargo xtask validate` to **13 circuits, all green**.

> **All six ladder rungs are formally "passed"** as of 2026-07-18 — each has both real
> implementation reach *and* a green `cargo xtask validate` against a committed, genuinely
> QSPICE-generated golden file (rungs 2/5 via a hand-translated QSPICE-native `.model` card;
> rungs 3/4 via that plus a `UIC` cold-start fix; rung 6 via that plus a genuine QSPICE
> ground-aliasing bug fix for `Q`-element terminals and an honestly-scoped early comparison
> window for this circuit's unstable equilibrium; § the T6.3 section's 2026-07-17/2026-07-18
> entries have the full account of each).

**Ladder rung 5 (a MOS): implementation reach closed 2026-07-12** — was the sole fully
unstarted rung; closed the same way rung 6's ring oscillator was, with a new hand-written
reference model rather than waiting on an industrial compact model (every BSIM/HiSIM/PSP family
`.va` file in `external/` that passes the frontend is far past what this codegen's if/else/
loop/function coverage can build into a `ModelInstance` today — confirmed by inspection, not
assumed, since no per-file codegen breakdown is exposed via any `va-cli` subcommand yet).
`models/mosfet.va`: a three-terminal (`d`, `g`, `s` — no body/bulk terminal, matching
`va-abi::reference::Bjt`'s own no-body-effect scope for the analogous three-terminal BJT), Level-1
(Shichman-Hodges) square-law NMOS — cutoff/triode/saturation region selection via ordinary
`if`/`else if`/`else`, no new codegen capability needed (T2.2's `if`/`else` lowering already
covers it). Unlike `Bjt` (a hand-written `va-abi` Rust struct with **no** netlist wiring at all —
its ring-oscillator demo builds instances directly in a `va-transient` test, since
`va-netlist` had no 3-terminal-device grammar), rung 5 is a real `.va` source file compiled
through the actual frontend→codegen pipeline via `va-cli`'s existing `--model` flag, genuinely
exercising T1+T2, not just T3 — matching the ladder table's own "T1, T2, T3 (model reach)"
description. Needed one small, additive `va-netlist` grammar change: `Parser::parse_device`
gained an `'M'` element-line arm (`M<name> d g s model`, mirroring the existing `'D'`
two-terminal model-referencing-device arm) — `va_ir::Module`/`va_abi::ModelInstance` and
`va-cli`'s own device-building code needed **no** changes at all, since `Device::terminals`
(already `Vec<usize>`, arbitrary length) and `build_from_model` (already zips port nodes against
`terminals` of any length) were both already terminal-count-generic; only the 2-terminal-specific
netlist *grammar* was missing. `circuits/mos_dc.net`: an NMOS common-source bias point (`VDD`
through a drain resistor, gate held at a fixed bias, source grounded) that needs genuine Newton
iteration to solve — not a linear divider — and lands well inside the saturation region, checked
against a hand-derived fixed point (`(VDD-Vd)/RD = 0.5·kp·(w/l)·Vov²·(1+λ·Vd)` collapses to
`Vd = 3.31/1.0169 = 3.254991…` for this circuit's values), not just against the tool's own output
— `cargo test -p va-cli mos_dc_solves_through_codegen_pipeline` asserts this to `1e-6`.
*Outstanding, same as every other rung*: the golden-vs-ngspice gate awaits T6.3; no `t1/t2`
tutorial written yet. [Both closed since — the gate is green against **QSPICE** golden (the
oracle switched in `f094bbe`) as of 2026-07-18, and all 21 tutorials are written.]

**Ladder rung 2 (diode I–V, DC sweep): implementation reach closed 2026-07-13** — was the last
rung marked "pieces work in isolation… not yet wired," since `va-cli` only ever solved a single
DC operating point, never an actual sweep, even though `va-frontend`/`va-codegen`/`va-core` had
each independently exercised a diode in isolation since T1.3/T2.2/T3.3. Closed by giving
`va-netlist` a `.dc <source> <start> <stop> <step>` card (`DcSweep`, mirroring the existing
`.tran <tstep> <tstop>` precedent) and `va-cli` a `solve_dc_sweep`/`report_sweep` pair: for each
swept value, `solve_dc_sweep` clones the netlist, overrides the named `vsource` device's own
`value`, and re-solves the whole circuit from scratch via the existing `solve_dc` — the simplest
correct implementation, not the most efficient one (a real sweep could reuse the previous point
as a Newton warm-start; this doesn't, matching how `va-core::dc::sweep` itself is deliberately
agnostic about *what* changed between points). `circuits/diode_iv.net`: `V1` forces `V(in))`
directly (no series resistor — the simplest circuit that isolates the diode's own I–V law) and
`D1` is `models/diode.va`'s Shockley diode, swept 0–0.6 V in sequence. `cargo test -p va-cli
diode_iv_sweep_solves_through_codegen_pipeline` checks all 7 points against the closed-form
`Id(V) = Is·(exp(V/(N·vt)) − 1)` (`Is=1e-14`, `N=1.0` — the model's own defaults; `vt` =
`va_codegen::VT`, the same room-temperature constant the generated model itself evaluates `$vt`
to) — not just against the tool's own output — confirming the sweep reproduces the textbook
exponential I–V curve at every point, from `I(V1)≈0` at `V1=0` to `I(V1)≈-1.2e-4 A` at
`V1=0.6`. *Outstanding*: only a single linear sweep of one source (no nested/multi-source
sweeps); golden-vs-ngspice gate awaits T6.3; no `t1/t2` tutorial written yet. [Same correction
as rung 5 above: gate green vs QSPICE golden since 2026-07-18, tutorials all written. Only the
"single linear sweep of one source" limit still stands.] **With this, every
ladder rung has reached "implementation reach" through the real pipeline** — the entire
remaining bring-up-ladder gap, across every rung, was at that point uniformly the same one
thing: `va-harness` (T6.3), not an unimplemented circuit. That gap has since closed too.

---

## Analysis context — Tier A (delivered 2026-08-06)

**A §6 coordinated change to *both* frozen interfaces**, and the first one to break an existing
signature rather than add a defaulted method. Written up beforehand in
`docs/proposals/analysis-context.md`; this section records what actually shipped.

**The problem.** `va-frontend` const-folded a family of constructs on the basis that DC was the
only analysis. T4 and T5 then landed and the folds were never revisited, so each had become a
silent wrong answer *in an analysis that already existed*:

| Construct | Folded to | Was wrong in |
|---|---|---|
| `analysis("tran")` | `false`, always | transient — the branch never fired |
| `analysis("dc"/"static")` | `true`, always | transient — a DC-init branch fired at every timepoint |
| `$abstime` | `0.0` | transient — every time-dependent model frozen at t=0 |
| `ac_stim` | `0.0`, arguments discarded | AC — a model's own excitation contributed nothing |
| `bound_step` | no-op | transient — the adaptive controller never saw the hint |

The keystone was that `ModelInstance::load` carried no time, no frequency and no analysis kind,
so a model *could not* be told what was running. **None of this was visible in `cargo xtask
validate`** — all 13 gated circuits use textbook devices (R, C, diode, MOS, BJT) containing no
analysis-dependent construct, so a green 13/13 was never evidence against any of it.

**What shipped.** Interface β gained `AnalysisCtx { kind, time, temp }` on both `load` and
`noise`, plus two defaulted `StampSink` channels (`excitation` for `ac_stim`, `bound_step`).
Interface α gained `Builtin::{Abstime, Analysis, AcStim}`, `Stmt::BoundStep`, and the shared
phase-name/bitmask encoding. See `interfaces.md`'s paired revisions of 2026-08-06 for the
reasoning behind each design decision — particularly why there is deliberately **no `freq`
field**, and why the phase bit order lives in `va-ir` rather than `va-abi`.

**The clearest payoff: a workaround deleted.** `va_transient::integrator::run_dynamic` was a
near-duplicate of `run_with_events` whose only reason to exist was re-boxing a freshly-valued
`VSource` at every step *attempt*, because `load` had no time parameter. It is gone, along with
`va_cli::build_instances_split` that fed it. A `SIN` source is now an ordinary stateless
`ModelInstance` reading `ctx.time`; every device takes one path. That removes an allocation per
timestep and ~100 lines of duplicated integrator.

**Evidence it was behaviour-preserving:** all 13 golden gates reproduce their previously
recorded numbers **to the last digit** — including `rectifier.net` at `6.766e-4`, the one gate
that actually exercised `run_dynamic`. Workspace: 538 tests pass, `fmt`/`clippy -D warnings`
clean, `va-cli check external` unmoved at 114/150 (the new hard errors — an unrecognized
`analysis()` phase name, `bound_step` in expression position — reject nothing in the real
corpus).

**`$abstime` is golden-gated (2026-08-06).** The proposal's blocking spike was run and
succeeded: QSPICE's behavioral source does expose `time`, so `circuits/abstime_ramp.net` now
compares a compiled `models/abstime_ramp.va` against a QSPICE `B1 out 0 I=1*time` deck —
**error 4.382e-17**, and the zoo is **14/14**. It discriminates: reintroducing the original
fold moves it to `5.838e-1`, ~580x over tolerance. A `UIC` gotcha found on the way is recorded
in `validation.md` (it offsets QSPICE's own `time` by exactly 1e-7 s, which would have
poisoned the very quantity under test).

**What is still *not* claimed.** `analysis()`, `ac_stim` and `bound_step` remain **unit-tested,
not golden-gated** — no QSPICE construct corresponds to them when driven from a model rather
than a netlist. `validation.md`'s "Analysis-context constructs" section states exactly which
property is checked how.

**Tiers B and C remain open, and remain wrong:**

- **Tier B** — `transition`, `slew`, `absdelay`, `$limit`, `@(initial_step)`, `idt` with an
  initial condition. These need per-instance **state across evaluations**, and Interface β is
  deliberately stateless: `load` takes `&self`, and is re-entered per Newton iteration and again
  on *rejected* timesteps that must not corrupt history. A state channel needs its own contract
  answering who owns the storage and what is committed versus rolled back. That is a genuinely
  harder design and was deliberately not smuggled into Tier A.
- **Tier C** — `laplace_*`, `zi_*`, currently folded to their DC gain. A filter's small-signal
  response is genuinely frequency-dependent, so this needs either per-frequency
  re-linearization (an O(points) cost on every AC run) or a complex-valued channel on
  Interface β. Adding `freq` to the context would not deliver it; the restructuring is the work.

`docs/token-reference.md` marks both tiers as still-wrong **per construct**, rather than letting
Tier A's arrival imply the whole family is fixed.

---

## Analysis context — Tier B: the state channel (delivered 2026-08-07)

A second §6 change to Interface β, designed up front in `docs/proposals/model-state.md`.

**The finding that shaped it: Tier B was not one problem.** `analysis-context.md` named six
constructs as "need to remember something between timesteps". Measured against the corpus they
split into four lifetimes and two failure modes:

| Construct | Corpus | Needs | Fold's failure mode |
|---|---|---|---|
| `$limit` | **10 / 72** | previous Newton **iterate** | **converges worse — same answer** |
| `absdelay` | 5 / 17 | unbounded **trajectory** | wrong answer |
| `transition` | 7 / 14 | per-accepted-step state | wrong answer |
| `slew` | 0 / 0 | per-accepted-step state | wrong answer |
| `@(initial_step)` | 0 / 0 | **nothing** — the solver knows | body ran every timepoint |

**Delivered:** the state channel (`state_len`, `ModelState`, `AnalysisCtx::is_initial_step`,
`va-transient`'s commit/rollback), plus `transition`, `slew` and `@(initial_step)` un-folded.
`@(initial_step)` is desugared in the parser into an ordinary `if (initial_step())`, so it
needed no new AST or IR statement kind and the existing control-flow walk selects the arm.

**Not delivered, each for its own reason** (not one blanket deferral):

- **`$limit`** — the most-used construct in the corpus, excluded deliberately. Its fold costs
  convergence robustness, **not correctness**: a converged Newton solve is a fixed point of the
  *unlimited* equations. Its lifetime is the Newton iterate, and `va-core` already limits every
  unknown globally. The real work is "let a model direct the existing limiter" — convergence
  work, not a state channel.
- **`absdelay`** — needs an interpolated history buffer; no fixed-size state vector holds a
  trajectory.
- **Exact `transition` breakpoints** — approximated with Tier A's `bound_step` (~8 points per
  ramp) rather than scheduled corners. Labelled as an approximation at the construct.

**Evidence:** 547 tests pass, fmt/clippy clean, and **all 14 golden gates reproduce their
numbers to the last digit** — the state channel is inert for every model that declares no state,
and `is_initial_step` keeps static solves bit-identical. The new behaviour is unit-tested, not
golden-gated; `validation.md` states exactly what is and is not covered, including that
rollback-on-reject is not exercised by a rejecting circuit and that `transition` has no
dedicated end-to-end test yet.

---

## Analysis context — Tier C: frequency-dependent stamps (delivered 2026-08-07)

The last of the three tiers, and the one both earlier proposals called "largest". It turned out
to be the **smallest**, because of one observation (`docs/proposals/frequency-domain.md` §1):

> At a single frequency `ω`, a complex admittance `H = a + jb` is **exactly** the real pair
> `G = a`, `C = b/ω`, since the assembler forms `G + jω·C`.

So Interface β needed no complex channel — `jacobian`/`dcharge` already span the complex plane
at a given frequency. All that was missing was telling the model *which* frequency
(`AnalysisCtx::freq`, the field Tier A conditionally refused) and re-linearizing per point.

**Delivered:** `laplace_nd`/`np`/`zd`/`zp` evaluated at `s = jω` in AC, root forms in product
form (never expanded — the corpus has a 7-coefficient filter with values near `1e71`),
coefficients kept as *expressions* so a netlist-overridden pole still moves.

**Cost is opt-in.** `is_frequency_dependent()` defaults false, so an ordinary AC sweep still
linearizes once and every pre-existing AC gate is bit-identical. Only a circuit with a real
filter pays O(points).

**Gated against real QSPICE at 1.361e-15** — `circuits/laplace_ac.net` vs an R-C network, zoo
**15/15**. It discriminates: restoring the DC-gain fold moves it to `6.282e3`.

**Not delivered, by evidence and by kind:**

- **`zi_*`** — **zero** corpus uses in 150 files, and it needs a clock. Keeps its fold.
- **Transient Laplace** — a convolution, not a stamping problem. DC and transient still get
  `H(0)`, exactly as before; this tier makes AC right and says so rather than implying more.
- **Laplace-shaped noise** — one real corpus use; belongs to the noise channel.

With this, all three tiers of `analysis-context.md` are closed. `$limit` and `absdelay` remain
the two named non-goals across the whole programme, each with its own recorded reason.

---

## Corpus metric honesty (2026-08-29)

**`114/150 files passed the frontend` was not measuring frontend capability.** The denominator
was already known to be soft (this file's language-coverage section has classified the 36
failures as corpus artifacts since 2026-08-04). What had not been noticed is that the
**numerator was inflated from two directions at once**, and by more than the denominator was.

**Inflation 1 — 14 files passed while declaring no module.** `check_group` iterated each file's
range of parsed modules and asked "did every one elaborate?". For a macro header
(`constants.vams`, `simulatorFlags.va`, `ekv3_definitions.va`, …) that range is empty, the loop
body never runs, `all_ok` stays `true`, and the file is scored as a pass. Confirmed by counting:
the run printed **100 `[ok ]` module lines but claimed 114 passes**.

**Inflation 2 — 16 model files passed with their entire body deleted.** `preprocess`'s
unresolved-`` `include ``-is-skipped rule is deliberate and load-bearing (the standard headers
are built into the frontend, so requiring them on disk would reject nearly every real model).
But several vendor compact models are a licence block, a `module` line, one
`` `include "..._module.include" `` holding the whole body, and `endmodule`. With the body file
absent, an *empty module* reaches the parser — and elaborates perfectly. These reported `[ok ]`
with **0 params, 0 funcs**: `bjt504/505[t]`, `bjtd504/505[t]`, `bsimcmg`, `bsimimg`,
`hisimhv[_n4,_n5]`, `hisimsoi[_n4,_n5]`. A BSIM-CMG with zero parameters is self-evidently not a
pass.

**The two halves are the same defect wearing opposite verdicts.** Those 16 files differ from the
ten that *fail* with "port `D` has no discipline declaration" (`psp103/104*`, `L_UTSOI_102*`,
`r2_cmc*`) only in whether their ports happen to be declared inline *before* the vanished
include. One missing file, one pass and one failure — and a metric that measured neither. The
`psp102/` family is the control that proves it: `psp102.va` is structurally identical to
`psp103.va` and differs only in that its `PSP102_module.include` actually ships. It passes with
317 parameters.

**What shipped.** `preprocess_reporting` returns every `` `include `` name it dropped
(`preprocess` stays as a thin wrapper, so no caller broke), and `va-cli check` uses it:

- Every status line — `[ok ]`, `[elab ]`, `[cgen ]`, `[parse]`, `[lex ]` — carries an
  `[after skipping unresolved `include: …]` clause when one was dropped. The ten "port has no
  discipline declaration" failures now name `PSP103_module.include` in the same breath, so the
  message is attributable instead of misleading.
- A file declaring no module is reported `[none ]` and counted separately, not as a pass.
- The summary reports four numbers instead of one, ending with the defensible one:

```text
100/136 files declaring a module passed the frontend (lex → parse → elaborate)
  14 further file(s) declare no module at all (macro/nature headers, statement fragments)
  of the 100 passes, 18 are on an incomplete module (an unresolved `include was dropped); 82 are self-contained
  10 of the 36 failures also dropped an unresolved `include (truncated distributions, not gaps)
  => self-contained files declaring a module: 82/108
  150 file(s) scanned in total
```

**The honest headline is 82/108 (frontend) and 75/108 (frontend + codegen)**, not 114/150 and
107/150. It is a *lower* number than the one it replaces and that is the point — the previous
figure counted 32 files as coverage that demonstrate nothing.

**What this does not claim.** The 18 incomplete passes are not re-classified as failures: an
empty module genuinely does elaborate, and the frontend is not at fault. They are counted
separately because a pass on source the file itself says is incomplete cannot support a claim
about language coverage. Nor is the missing-include situation itself fixed — the vendor
`.include` files are absent from the corpus snapshot and no amount of frontend work conjures
them.

**Evidence:** 551 tests pass (was 550), fmt/clippy clean, all 15 golden gates reproduce their
recorded numbers to the last digit (nothing in the simulation path changed). A `va-cli` test,
`a_dropped_include_and_a_module_less_file_are_not_counted_as_clean_passes`, builds all three
shapes from scratch — a whole model, a `hollow.va` whose body is an absent include (it
reproduces the exact `0 params, 0 funcs` signature), and a module-less header — and pins each to
its own bucket, so the accounting cannot quietly collapse back into one number.

---

## A block-local declaration could silently capture a parameter (2026-08-29)

Found while tracing why `external/bsimsoi.va` failed codegen with "variable #881 read before
assignment". The codegen message was a symptom; the cause was in `va-frontend`, and it was not a
coverage gap but a **silent wrong answer**.

**The bug.** `Elaborator::vars` is a single flat, module-wide `name -> VarId` map, and neither
`collect_vars_stmt` nor `lower_stmt` pushes or pops a scope at a `Stmt::Block`. A declaration
inside `begin : blk ... end` therefore leaked over the *entire* analog block — including
statements **before** the block, which no scoping rule could justify. Given

```verilog
parameter real k = 1000.0;
analog begin
  begin : inner  real k;  k = 1.0;  end
  g = 1.0 / k;            // must divide by the parameter, 1000.0
  I(p, n) <+ g * V(p, n);
end
```

the read of `k` resolved to the block-local variable, and the device became a **1-ohm resistor
instead of a 1-kilohm one, with no diagnostic anywhere**. In `bsimsoi.va` the same defect has a
`begin : load` block declare `real ... MJSWG;`, hijacking the read of the `MJSWG` *parameter*
about 2200 lines earlier at `B4SOIbodyJctGateSideSGradingCoeff = MJSWG;`. There it happened to
surface as a codegen error only because nothing had assigned the variable yet; the neighbouring
`... = MJSWGD;` works purely because no block-local `MJSWGD` exists. Had any assignment preceded
the read, `bsimsoi` would have compiled and produced wrong junction-capacitance physics.

**What shipped, and why it is a rejection rather than a fix.** Real block scoping means pushing
and popping a scope in *both* passes — the pre-pass that allocates `VarId`s and the pass that
resolves names — and keeping them in agreement. Getting that subtly wrong would introduce *new*
silent mis-resolutions across the whole frontend, which is a strictly worse failure than the one
being fixed. So the conservative half shipped: `declare_local_var` **rejects** the collision with
an accurate message. Rejecting can only turn a wrong answer into a diagnostic, never the reverse.

**The rejection is narrowed by nesting depth, so it does not undo the earlier fix it looks like
it contradicts.** `analog begin : load real MJSWG; ... end`, where the named block *is* the whole
analog block, already spans every statement its scope could reach — the flat map is
indistinguishable from correct scoping there, and that shape stays supported (pinned by the
pre-existing `block_local_variable_shadows_a_same_named_parameter`, which this change had to
leave green). Only a declaration nested inside a block with statements outside it can capture a
read it should not, and only that is refused.

**Not addressed:** a block-local declaration colliding with an outer *variable* still aliases the
two. It is the same absence of scoping, but both names denote a variable of the same type, so the
failure mode is a shared slot rather than a read of an entirely different quantity. Recorded at
`declare_local_var` as a known limitation. Real block scoping remains the open item.

**Evidence:** exactly **one** corpus file changes verdict — `bsimsoi.va`, which was already
failing, now failing at elaboration with a message that names `MJSWG` and the actual rule instead
of an opaque "variable #881". Corpus totals unmoved (73/108 with `--codegen`). 553 tests pass
(was 551), fmt/clippy clean, all 15 golden gates reproduce their recorded numbers to the last
digit. Two new tests: the leaking shape is rejected, and — as the control that stops the
rejection from being vacuous — the same shape *without* a name collision still elaborates with
the outer read resolving to `Expr::Param`.

---

## A `ddt` bound in a branch could vanish from the charge channel (2026-08-29)

The second silent wrong answer found the same day as the block-scoping one, and the same shape
of fix. This one is in `va-codegen`.

**The bug.** `DdtVars` — the tracking that lets `real dqdt; dqdt = ddt(q); I <+ dqdt;` fold into
the charge channel — is forward and single-pass by design: entering a branch clones the map and
the clone is discarded on exit, so a variable reassigned in only one arm never leaks a stale
definition. Sound for *values*. But `external/hicumL0_v2p1p0.va` writes its self-heating
capacitance as

```verilog
if (flsh == 0 || cth == 0.0)  I_cth = 0.0;
else                          I_cth = ddt(cth*V(br_sht));
...                                       // a *separate* if statement
if (...) ... else begin
  I(br_sht) <+ V(br_sht)/rth_t - pterm;
  I(br_sht) <+ I_cth;                     // the ddt binding is long gone
end
```

By the contribution the binding has been discarded, so `I_cth` lowered as an ordinary resistive
term — **and it compiled**, because the other arm's `I_cth = 0.0` had emitted a real assignment,
so the read was defined. The result: **no charge stamped at all**, the device's entire thermal
capacitance gone, with no diagnostic anywhere. This is not the limitation the module already
documented ("read as an ordinary value ... silently drops that read's assignment"); that one is
about reads *outside* a `<+`. This is a read *at* a `<+`, which was supposed to work.

**What shipped.** `invalidate_ddt_vars` now records which variables lost a `ddt` shape when a
branch closed. That set is deliberately **not** cloned per branch — unlike `DdtVars` itself, a
drop inside one arm must stay visible to everything after it. An ordinary assignment clears the
mark (a real value supersedes the `ddt`, so `angelov_gan.va`-style scratch reuse of the same
variable is unaffected). A `<+` reading a still-marked variable is **rejected**, naming what
would be lost.

Rejecting rather than supporting, again for a reason rather than convenience: the correct
semantics here is a charge term contributed *only on one branch*, which needs the charge channel
evaluated inside control flow — a real feature, not a recognizer tweak. Between compiling the
wrong physics and refusing, refusing is the only honest option.

**Evidence:** exactly one corpus file changes — `hicumL0_v2p1p0.va`, already failing on the
separate port-probe issue below, which now reports the charge-drop first. Corpus totals unmoved
(73/108 with `--codegen`), so nothing that previously built stopped building. 555 tests pass
(was 553), fmt/clippy clean, all 15 golden gates bit-identical. Two tests: the branch-bound
shape is refused, and — the control that stops the refusal from being vacuous — the same
variable-indirection shape with the `ddt` assigned *outside* any branch still lowers and still
stamps `Q = cth*V`.

**Still open, and now the only blocker for six corpus files:** `va-frontend`'s `I(<port>)`
probe fold (`lower_port_probe`/`collect_port_flow_contributions`) inlines the *raw* RHS of every
flow contribution to a branch touching that node, `ddt(...)` terms included, into an `Add` chain
assigned to a variable. The six HICUM files all do `IB = I(<b>);` where some of those
contributions are charge terms, so codegen meets a `ddt` nested in an ordinary assignment. None
of those files contains a nested `ddt` in its own source text — the nesting is manufactured by
the fold. The fix is to give the fold the same carve-out `FlowCurrentAccumulator` already
documents (sum only the *resistive* contributions); it is a `va-frontend` change and has not
been attempted here.

---

## `I(<port>)` no longer manufactures a nested `ddt` (2026-08-29)

The last of the day's findings, and the one that had been misattributed the longest. Six corpus
files failed codegen with "ddt must appear as a top-level contribution term, not inside an
expression" — and **none of them contains a nested `ddt` anywhere in its source**. The nesting
was manufactured by the frontend.

**The cause.** `lower_port_probe` folds `I(<port>)` into the signed sum of every flow
contribution already made to a branch touching that port's node, by inlining each contribution's
*raw* right-hand side. On a compact model's base node those contributions include charge terms —
HICUM writes `I(br_bci) <+ ddt(qjcx);` beside its conduction currents — so `IB = I(<b>);` became
an ordinary `Stmt::Assign` whose RHS contained `ddt(...)`. `va-codegen` refused it, correctly:
`ddt`'s value depends on the whole history of its argument, not on the current unknowns. The
error named the right construct and the wrong culprit.

**What shipped.** `resistive_terms_only` splits each contribution into signed additive terms and
keeps only those containing no `ddt`. If every term is a charge term, the branch adds nothing to
the probe (a purely capacitive branch carries no conduction current). `idt` is deliberately not
filtered — its value is an ordinary read of its accumulator unknown, evaluable anywhere.

**This is an approximation, stated rather than hidden.** `I(<port>)` now reports **conduction
current only**, omitting displacement current. It is **exact at a DC operating point**, where
every `ddt` is zero by definition, and an approximation in transient. Two things make it the
right call rather than a convenient one:

- It is the rule `va-codegen`'s flow-current accumulator **already applies** to a branch
  self-probe `I(branch)` (`lower.rs`'s `FlowCurrentAccumulator`). `I(<port>)` was the one
  construct that failed outright instead of following it; the two are now consistent.
- Every real corpus read is an operating-point output inside a `` `ifdef CALC_OP `` block
  (`IB = I(<b>);`), where conduction current is what is being asked for.

The alternative — actually evaluating `dQ/dt` — needs the integrator's per-term time-stepping
coefficient exposed through Interface β. That is a §6 coordinated change, not a fold-local one.

**Evidence:** corpus **73/108 → 75/108** with `--codegen`. `HICUML0-2.va` and
`hicumL0_v2p0p0.va` are unblocked outright — the fold was their only blocker. 557 tests pass
(was 555), fmt/clippy clean, all 15 golden gates bit-identical. Two tests, both confirmed to fail
without the change: one pins that the probe carries no `ddt` **while the resistive half of the
same contribution survives** (a probe folding to a bare `0.0` would pass a "no ddt" assertion for
entirely the wrong reason), and one pins the purely-capacitive boundary at exactly `0.0`.

**The six remaining codegen failures are now all genuine, and each has its own reason:**

| File(s) | Blocker | Reachable? |
|---|---|---|
| `hicumL2V2p4p0`, `hicumL2V3p0p0`, `hicumL2_v310` | `n_2/n_w*ddt(n_w*V(b_n1))` where `n_2` derives from `Tf`/`betadc` — genuinely bias-dependent | No — the product-rule case, needs Interface β |
| `mvsg_cmc_3.2.0` | `csh*ddt(V(gi2,gi2p))` with `csh` reading `V(gi2,gi2p)`; also `tdut` via `Temp(dt)` | No — same |
| `hicumL0_v2p1p0` | a `ddt` bound in one `if` arm, contributed later | No — needs the charge channel evaluated inside control flow |
| `verilogaLib-master/amp_dynamic` | declares `parameter real gain` **and** `real gain` in the same scope | No — a bug in the corpus file |

None is a recognizer gap that more pattern-matching would close. That is a meaningfully different
statement from where this section started the day.

---

## Combined port declarations, and a module-less file is not an error (2026-08-30)

Two `va-frontend` parser defects, found by triaging the six new parse failures that appeared when
the corpus grew from 150 to **158 files** (a photonic model library: `Attenuator_compliant.va`
into `external/photonic/`, and `external/verilogAlib/` — five model/header files plus that
vendor's own `constants.vams`/`disciplines.vams`, co-located because all three `disciplines.vams`
variants now in the corpus differ, and without them these files would silently bind to a
*different* library's header).

**Headline: 81/108 → 85/88 frontend, 75/108 → 79/88 frontend+codegen.** Full gate green —
`fmt`, `clippy -D warnings`, **569 tests** (was 557; +12 here), and `xtask validate` **15/15
golden reproducing their recorded numbers to the last digit**. Nothing in the simulation path
changed.

**The 2026-08-29 frontend figure was itself off by one.** Re-measuring the *pre-expansion* corpus
gives **81/108**, not the 82/108 recorded then: commit `82045f0` (a nested-block declaration
shadowing a parameter is now rejected) turned `bsimsoi.va` from a frontend pass into an `[elab]`
failure, and the figure was taken before the last commit of that session landed and never
re-derived. The codegen half (75/108) was correct. This is the third time a corpus figure has
survived a revision without being re-measured — see the entry below, and §"How to keep this
document honest".

### 1. The `discipline_identifier` slot of a port declaration was missing

LRM §6.5.2.2 (from A.2.1.2) defines
`inout [ discipline_identifier ] [ net_type | wreal ] [ signed ] [ range ] names ;`. Only the
bare `inout p, n;` form was implemented, so the combined `inout electrical p, n;` failed with
`expected Semicolon, found Ident("p")` — because `expect_ident` accepts `electrical` as an
ident-like keyword, the declaration parsed as *one port named `"electrical"`* and then demanded
its semicolon. A misleading error for a squarely legal construct, and `token-reference.md`
documented the grammar as though the slot did not exist.

`parse_item`'s direction arm now accepts the qualifier — a built-in `electrical`/`thermal` or any
name a `discipline` block registered — **and with it the range that follows**
(`inout electrical [0:3] bus;`), by handing off to the same `parse_net_item` a standalone
`electrical [0:3] bus;` already takes. The combined form is split back into the two-statement
form (an `Item::Direction` plus an `Item::Net`, queued through `Parser::pending_items`), so
nothing downstream of the parser can tell the two spellings apart — the test asserts that
equivalence directly rather than asserting a shape.

**Guarded, with a negative control.** Recognizing the qualifier requires a name or `[` to follow
it, so `inout electrical;` still declares a port *named* `electrical` — a spelling
`ident_like_keyword` deliberately permits. That case is a test.

**`net_type` stays rejected — and that is the language's boundary, not ours.** `wire`, `wand`,
`wor`, `tri`, `triand`, `trior`, `supply0`, `supply1` name discrete-domain nets, which Annex C
excludes from Verilog-A (CLAUDE.md §1), and `token-reference.md` already recorded each as
"reserved, no grammar production". Zero corpus files use the slot. What changed is only the
*diagnostic*: instead of surfacing as `expected an identifier, but 'wire' is a reserved word`, it
now names the construct and says why it cannot appear. `signed` is likewise not accepted — it has
no meaning for a continuous net.

Fixes `mrr_weight.va`, `photonic_primitives.vams` (all **7** modules) and
`photonic_waveguide.vams` outright.

### 2. A file of nothing but `nature`/`discipline` blocks was a parse error

`parse`'s own contract says a stream defining no module "is not an error — it's a valid, if
degenerate, compilation unit". That held only for files that lex to **zero** tokens (a pure
`` `define `` header). The top-level loop checked for end-of-stream and *then* called
`parse_module`, which consumes the `nature`/`discipline` preamble before demanding `module` — so
a file whose entire content is that preamble was swallowed and then failed
`expected Module, found None` at EOF. **A standard `disciplines.vams` is exactly that shape**, so
the bucket was structurally unreachable for the most common header in the corpus.

The preamble is now consumed at the top level, before the end-of-stream check. Five files move
from `[parse]` to `[none ] declares no module` — `external/disciplines.vams`,
`external/ekv3_natures.va`, `external/photonic/disciplines.vams`, and the two newly added
`external/verilogAlib/{disciplines,photonic_discipline}.vams`. A negative control asserts that
skipping the preamble does not skip a module that follows it.

This was **a misclassification, not a coverage gain**: those files were never models. It moves
them out of the denominator, where they had been counted as failing models.

### The failure side of the denominator, closed — 17 files

The 2026-08-29 cleanup fixed the metric's *numerator* (it had counted module-less and
include-gutted files as passes). The symmetric hole on the failure side is now closed too. A file
that dies at `[pp]` or `[parse]` used to be counted as a failing model without anyone asking
whether it declares a module — because at that point the tool cannot parse it to find out.

`check_group` now settles "is this a checkable model at all?" **before** preprocessing, and does
so only where it can *prove* the answer. `cannot_declare_a_module` requires **both**:

1. the byte sequence `module` occurs nowhere in the raw source — Verilog-A's preprocessor has no
   token-pasting operator, so the keyword cannot be assembled from fragments; if those six bytes
   are absent, no macro expansion can produce them; and
2. the file contains no `` `include `` — an included header could carry a `module`, and when a
   file dies at preprocess we cannot know what its includes would have expanded to.

For the files it accepts that is a proof, not a heuristic, which is what makes it fit to move a
file out of the failure count. All 17 affected corpus files satisfy both halves: 4 `ekv3_*`
statement fragments whose token stream opens with `begin`, and 13 `ekv3_*`/`sp_functions`-class
include fragments that fail preprocessing. Every one has **zero** `` `include `` directives and
**zero** occurrences of `module` anywhere, comments included.

**Deliberately crude in the safe direction, and tested as such.** Any occurrence of `module`
counts — inside a comment, a string, or the word `endmodule`. A wrong "declares no module" would
hide a real frontend gap; a wrong "might declare a module" only understates coverage. Two
negative controls pin the asymmetry: the same un-parseable fragment *with* the keyword in a
comment, and one with no keyword but an `` `include ``, both stay in the failure count.

**Effect: 85/110 → 85/93 frontend, 79/110 → 79/93 frontend+codegen** — and on to 85/88 and 79/88 once the `[pp]` half below landed. The numerators do not move
— nothing new compiles. This is a measurement correction, and it is worth being explicit that it
flatters the figure: the honest reading is "of the corpus files that are actually models and are
self-contained, 85 of 93 pass the frontend", not "coverage improved".

### The last hole in the same family, closed — 5 files

`parse_file` discarded a failing file's skipped-include list, so `failed_incomplete` was
**unreachable for a `[pp]` failure**. That left the 2026-08-29 "one defect, two opposite
verdicts" shape intact in miniature: a truncated distribution whose vanished `` `include ``
merely broke *elaboration* was quarantined, while one whose vanished include broke
*preprocessing* was scored as a frontend gap.

`preprocess_reporting` now returns `(Result<String, _>, Vec<String>)` — the skipped list comes
back on **both** arms. It has to: the absent body file usually also held the macro definitions
the surviving text goes on to use, so a truncated file characteristically drops an include *and
then* fails on `` `GMIN ``/`` `IPRoz ``/`` `MAXA ``. Returning the list only on success made those
two facts unreportable together. `parse_file` correspondingly returns
`Result<ParsedFile, Vec<String>>`, carrying what was skipped before the failure.

All 5 remaining `[pp]` failures are truncated vendor distributions (`diode_cmc`, `juncap200`,
`psphv`, `psphvrr`, `r3_cmc`), 1–5 missing `.include` files each. **85/93 → 85/88 frontend,
79/93 → 79/88 frontend+codegen**, numerators again unmoved.

**The diagnostics improved more than the metric did.** The `[pp]` lines now explain themselves:

```
[pp] external\psphv.va: preprocess error: undefined macro `GMIN
     [after skipping unresolved `include: PSP103_macrodefs.include]
```

The bare form named a symbol and implied a frontend gap; the clause shows the macro lived in the
file that never shipped.

This is a two-crate change (`va-frontend`'s preprocess entry point and `va-cli`'s reporting), but
not a §6 event — `preprocess_reporting` is T1's own API, not one of the two frozen interfaces.
Negative controls both sides: a preprocess failure with nothing skipped reports an empty list,
and a file that fails on a genuinely undefined macro with no `` `include `` to blame stays in the
self-contained denominator.

### Remaining parse failures: 13 → 5

Four are the `ekv3_*` fragments above. The fifth, `example_mzi_modulator.vams`, advances past the
port fix to two **source** bugs in the corpus file, not frontend gaps: it passes a *parameter*
in a port-connection list (`.therm_en(1)` — a numeric literal is not a legal analog port
connection), and it instantiates modules from `photonic_primitives.vams` without `` `include ``ing
it.

---

## T1.3's validation gate, met literally — golden IR (2026-08-30)

The last thing keeping T1.3 at 🟢 was its own gate: *"the three zoo models elaborate to IR that
matches committed golden IR."* The tests asserted IR **structure** instead — a node count here,
a parameter name there. This roadmap had called that "cheap to close" since 2026-08-04 without
closing it. **T1.3 is now ✅.**

`crates/va-frontend/tests/golden_ir.rs` elaborates `resistor.va`, `capacitor.va` and `diode.va`
through the real include path and compares each against a committed snapshot of the whole
`va_ir::Module` (98, 151 and 218 lines).

**The snapshot is `{:#?}`, deliberately, and not a hand-written pretty-printer.** A bespoke
printer reads better and is exactly the wrong tool: it can only print the fields its author
remembered, so a field added to Interface α tomorrow would be invisible to it and the gate would
silently stop covering it. `Debug` is **exhaustive by construction** — every field of every
nested type, or it does not compile. Total coverage beats readability for a gate whose whole job
is to notice the change nobody predicted. That is the same reasoning that made structural
assertions insufficient in the first place, applied one level up.

**Generated, never hand-written**, via `UPDATE_GOLDEN_IR=1 cargo test -p va-frontend --test
golden_ir` — which then *fails on purpose*, so a regeneration run can never be mistaken for a
passing verification. A snapshot records what the code did, so a diff is a question ("did I mean
to change Interface α?"), never a licence to edit the file until it matches.

**Not in `golden/`, and that is not an oversight.** `golden/` holds **QSPICE** reference output
for numerical results, under the standing rule that QSPICE is the sole oracle and nothing
hand-computed goes in there. IR is not a physical quantity and QSPICE cannot produce it, so these
snapshots are a different kind of artifact and live beside the test that owns them.

**Two ways of knowing the gate bites**, because a snapshot test that cannot fail is worse than no
test: a committed negative control asserts two different models do not produce identical dumps,
and the gate was verified empirically by adding a `parameter real BOGUS` to `models/resistor.va`
— it failed with `first difference at line 50`, naming the file and both sides, then passed again
on restore. The failure message points at the regeneration command rather than leaving a 300-line
`assert_eq!` to be read by eye.

**Also refreshed here:** this file's frontend failure-category list, which still described
categories that no longer exist (the "7 — expected Module, found None/Some(Begin)" bucket was
retired when a module-less file stopped being a parse error). Re-derived: of 18 frontend
failures, **15 are truncated distributions** and only **3 are real** — two bugs in corpus files
(`ctle.va` uses an undeclared `gain`; `example_mzi_modulator.vams` passes a parameter in a port
list) and one genuine elaborator limitation (`bsimsoi.va`'s block-scoped shadowing, rejected
rather than silently mis-bound since 2026-08-29).

---

## Real block scoping (2026-08-30)

Closes `bsimsoi.va`. (The commit called this "the last genuine elaborator limitation in the frontend column"; an audit on 2026-08-31 showed that is too strong — see that entry.) **86/88 frontend, 80/88
frontend+codegen** — `external/bsimsoi.va` now passes both (14 nodes, 996 params, 2 funcs), and
it is the **only** file whose verdict changed: a per-file diff of the whole corpus before and
after shows exactly one line moving, `[elab] -> [ok]`. 573 tests, `fmt`/`clippy -D warnings`
clean, `xtask validate` 15/15 unchanged.

**What was wrong.** Verilog-A says a declaration inside `begin ... end` shadows an outer name
*only within that block*. Elaboration had one flat, module-wide `name -> VarId` map with no
push/pop at a `Stmt::Block`, so a block-local declaration leaked over the **entire** analog
block — statements *before* the block included, which no scoping rule could justify. On
2026-08-29 that was made a hard error (the conservative half: a diagnostic can only replace a
wrong answer, never the reverse) and real scoping was deferred. This is the other half.

**What shipped.** `Elaborator::block_scopes` is a stack of `name -> VarId` maps, pushed and
popped at every `Stmt::Block` during lowering (popped on the error path too). `lookup_var`
searches it innermost-outward and falls back to the module/function scope — the single place
shadowing is now decided, replacing three separate `self.vars.get(...)` sites. A `Stmt::VarDecl`
allocates a **fresh** `VarId` into the innermost scope, fresh even when an outer variable or a
parameter of that name exists, which is what shadowing means. Function bodies swap the stack out
and back, so their blocks neither see nor outlive the analog block's.

**The two passes stay decoupled, which is the design's load-bearing part.** The variable-
collecting pre-pass and lowering both walk the same AST, and an obvious implementation has them
agree on a `VarId` allocation order — a coupling that breaks silently the moment their traversal
orders diverge. Instead only *lowering* allocates. The pre-pass carries `decl_scopes`, a mirror
stack of **names only**, whose sole job is to stop `register_var` auto-registering a module-scope
variable for an assignment whose target is really a block-local declaration. Neither pass needs
to know what the other numbered anything.

**Also retired:** the weaker sibling limitation that sat beside the parameter case — a
block-local declaration colliding with an outer *variable* silently **aliased** the two rather
than shadowing. Both are ordinary shadowing now, so `declare_local_var` no longer rejects
anything and `block_depth` (the nesting-depth heuristic that decided which shadows "actually
leaked") is gone.

**Tested on resolution, not on success.** The bug's signature was *succeeding with the wrong
answer*, so a test that only asserts "it elaborates" cannot see it. The tests assert which
`VarId`/`ParamId` a read actually landed on, via an expression walker whose `match` is
exhaustive on purpose — a new `Expr` variant must break the helper loudly rather than let a
resolution test quietly stop looking inside it. Both directions are pinned: after the block,
`g = 1.0 / k` must read the **parameter** (mis-resolving it silently made a 1 kΩ device 1 Ω);
inside the block, `k` must read the **block-local variable**. Without that second test the first
would pass for the wrong reason — resolving every `k` to the parameter is not scoping either.
A third test pins that two sibling blocks declaring the same name get two distinct variables,
the property a flat map cannot express at all.

**The golden-IR snapshots did not move**, which is the regression signal worth having: the zoo
models declare nothing block-locally (`diode.va`'s `real Id;` is module scope), so a change that
reallocated their `VarId`s would have shown up immediately in the gate committed hours earlier.
The six `[cgen]` failures are also unchanged, so the reallocation did not disturb codegen.

---

## The product rule: a bias-dependent `ddt` coefficient (2026-08-30)

`c(x)·ddt(q(x))` — a charge rate scaled by a coefficient that itself depends on the unknowns.
**84/88 frontend+codegen**, up from 80/88: all four files this had been blocking now build
(`hicumL2V2p4p0`, `hicumL2V3p0p0`, `hicumL2_v310`, `mvsg_cmc_3.2.0`), and only **two** codegen
failures remain in the whole corpus. 574 tests, `xtask validate` 15/15 unchanged.

**No Interface β change was needed after all.** This file had recorded the case as needing "the
whole companion-model discretization … to also carry a per-term, model-supplied coefficient — a
`va_abi`/`va_transient` interface change". That assessment predated the Route B groundwork
(`3fd301a`): `ad::Dual` already carries `grad_ddt`, the sensitivity to each unknown's *time
derivative*, and `StampSink::dcharge` is already the channel for it. The work was to make the
**resistive** stamping path consume `grad_ddt`, which is six lines, plus knowing when it is
valid — which was the actual content.

**The algebra.** Such a term is a *rate*, not a charge, so its value belongs in the residual
(reconstructed from committed history by `Builtin::Ddt`) and it stamps **no** `charge`, which is
what keeps it out of the companion offset. Its exact derivative is the product rule, split
across two channels:

```text
∂/∂x [ c(x) · dq/dt ]  =  (dq/dt)·∂c/∂x  +  c·coeff·∂q/∂x
                          └── jacobian ──┘   └─ coeff · dcharge ─┘
```

and `va_transient::newton_step` assembles exactly `jacobian + coeff·dcharge`. Under **backward
Euler** (`offset = −q_prev/h`, `prev_rate_weight = 0`) that is exact.

**Under trapezoidal it is refused, and that is the whole reason the method is a compile-time
choice.** ⚠️ **The justification first published here was wrong** — corrected below the same day;
the *refusal* is right, the reason is not what it said. See "Why trapezoidal is refused, correctly
this time".

**`va_codegen::Integration`** carried the choice: `build_instance` defaulted to `BackwardEuler`,
`build_instance_with` took it explicitly, and `va-cli sim --integration be|trap` set the
integrator's method and the models' compiled-for method together so they could not disagree.
⚠️ **Retired 2026-08-31** — the method-dependence was an integrator defect, not a property of the
model; see "Retiring `Integration`" below. `--integration` remains, now selecting the integrator's
method and nothing else.

**`sim` still defaults to trapezoidal, deliberately**, because every committed transient golden
was validated against it; changing that default would move five validated waveforms for reasons
unrelated to this feature. The consequence is worth stating plainly: **a model with a
bias-dependent `ddt` coefficient is refused by `va-cli sim` unless you pass `--integration be`.**
That is a build error, never a silent approximation.

**The gate is a finite difference on the *assembled* Jacobian**, not on `jacobian` alone — which
is the only version of this test that could fail correctly. The missing `c·∂q/∂x` lives entirely
in `dcharge`, so checking `jacobian` by itself would pass while the feature was wholly wrong.
The model is `I(p,n) <+ V(p,n)*ddt(c0*V(p,n))`, whose coefficient is genuinely bias-dependent, on
a step with **nonzero committed history** (a `q_prev = 0` test passes even if the history term is
mishandled). Verified to bite: deleting the `dcharge` stamp gives an assembled Jacobian of `0`
against a true `7e-4`, and the test fails — which is precisely the "degrades Newton without
failing" mode the old refusal comment predicted. A second assertion pins that the charge-channel
half is a *substantial* fraction of the answer, so the FD check cannot pass with `grad_ddt`
dropped and the term merely small.

**`lower::is_param_only` still bites** — it is what keeps such a term *out* of the plain charge
channel, where folding it in would be wrong. What changed is only what happens next: instead of
being refused, the term now takes the product-rule path. The three negative controls that pinned
those rejections became method-aware, asserting both halves together (exact under BE, refused
under trapezoidal) so neither can silently drift.

---

## Why trapezoidal is refused — correcting the same day's claim (2026-08-30)

The product-rule entry above originally justified refusing `c(x)·ddt(q)` under trapezoidal by
saying the companion offset *double-counts*, because a term that is itself a rate violates the
`dQ/dt = −residual` identity the offset rests on. **That is wrong.** Recording it rather than
quietly editing it, because the wrong reason is the more plausible one and would be re-derived
by the next person.

**There is no double count.** With `charge ≡ 0` and the term's value in the residual, and with
`r_prev` taking the whole stamped residual, the assembled row is

```text
f = (2/h)(Q_n − Q_{n−1}) + (A_n + B_n) + (A_{n−1} + B_{n−1}) = 0
```

— verbatim the trapezoid rule applied to `A + B + Q̇ = 0` (exact integration of `Q̇`, trapezoid
quadrature of the rest). `B` is the rate of `q`, not of `Q`, so it sits on the `r` side of the
identity, where it belongs. And `J = ∂f/∂x` **exactly, under both methods**, because the model is
handed the integrator's own `coeff`.

**Which means the finite-difference gate cannot see this.** The §5 check compares `J` against a
central difference of `f`; it passes under trapezoidal too. It is a consistency check between the
Jacobian and the residual, not a check that the residual is the right *discrete equation*. Worth
internalising: an FD gate on the assembled Jacobian is necessary and was the right thing to
build, but it is not sufficient, and it was never going to catch this.

**The real defect is the rate reconstruction's initial condition.** `Builtin::Ddt` seeds
`ρ₀ = 0` on the first step, while the true `q̇(0)` is nonzero for any start that is not a steady
state. The trapezoidal recursion's multiplier is exactly `−1` — *undamped* — so that `O(1)` seed
error never decays; it alternates. On a **uniform** step it cancels to `O(h²)` and the scheme
looks second order. Once the step varies — which the adaptive controller does on essentially
every step — the cancellation breaks:

| step pattern | charge channel (trap) | this path (trap) | this path (BE) |
|---|---|---|---|
| uniform, N=800 | 6.4e-10 (order 2.00) | 4.2e-9 (order 2.00) | 1.1e-5 (order 1.00) |
| alternating `[1,2]`, N=800 | 8.6e-10 (order 2.00) | **2.2e-5 (order 1.00)** | 1.2e-5 (order 1.00) |
| cycle `[1,2,1,4]`, N=3200 | 9.3e-11 (order 2.00) | **9.3e-6 (order 1.00)** | 3.8e-6 (order 1.00) |

So under the real controller the trapezoidal path is **first order with a worse error constant
than backward Euler** — while a fixed-step test would have shown clean second order and "proved"
the refusal unnecessary.

**It is fixable, cheaply, and with no interface change.** Take the **first** step with the
backward-Euler companion even when the method is trapezoidal, so `ρ₁` is seeded `O(h)` instead of
`O(1)`; then continue trapezoidal. Nothing about what a model stamps changes, and
`AnalysisCtx::with_ddt` already carries the per-step companion numbers down. Measured: 3.76e-10
vs 5.54e-6 on the alternating pattern at N=3200, and clean second order under arbitrary step
variation. Once that lands, the refusal can be lifted entirely and `va_codegen::Integration`
becomes unnecessary. A recursion-free BDF2 alternative was tried and does **not** work — it
composes badly with the outer trapezoid offset and stays first order.

**Standing limitation to keep in view even after the fix:** the recursion is undamped, so an
`O(h)` alternating error in `ρ` persists for a whole run. It does not cost order, but it is
genuine trapezoidal ringing; SPICE damps the equivalent with periodic backward-Euler steps.

### A second finding: the LTE probe bypasses the refusal at run time

`reference_method(BackwardEuler) = Trapezoidal`, and `run_with_events` solves **every** candidate
step a second time with the reference companion to form the embedded-pair LTE estimate. So a
model `va-codegen` refused to compile for trapezoidal is nevertheless evaluated under trapezoidal
companion coefficients at every timepoint of an `--integration be` run — behind the very coupling
`va-cli` introduced to make the two choices agree.

The reported waveform is still backward-Euler-exact: state is committed only from the post-accept
assemble, and the reference solve writes to scratch. But the accept/reject decision and therefore
the **entire step schedule** come from `|x_BE − x_trap|`, where `x_trap` carries the first-order
startup ringing above — so the estimator measures that, not local truncation error, and forces
needlessly small steps. `nlcap_ramp.net` visibly rejects four times before its first accepted
step. Worse, if that reference solve fails to converge the `?` aborts the whole run, for a model
the build deliberately refused to compile for trapezoidal — an error with no honest explanation to
offer. Fixing the first-step seeding closes this too, since the reference solve then becomes
legitimate.

---

## Two paths the product rule missed, and a block-scoping gap (2026-08-31)

An independent audit of the day's changes found a **regression introduced by the product-rule
commit** and a **live silent wrong answer** the block-scoping commit's claim did not cover.
Both are recorded here in full, including the part where the claim was too strong.

### Fixed: two stamping paths dropped `grad_ddt`

Lifting the blanket refusal on a nested `ddt` in a resistive term (`40baec7`) let such models
build — and two *other* `Dual`-consuming paths still stamped `grad` only, so the term's `c·∂q/∂x`
went nowhere:

- `stamp_flow_current_accumulators` — a contribution re-read through a bare `I(branch)` probe.
- `stamp_idt_accumulators` — the same coefficient inside an `idt` integrand.

Both produced an accumulator-row Jacobian of **zero where the truth is −7.0e-4**. Before the
lift these models were refused outright, so this is squarely a regression the lift created: the
silent-Newton-degradation the refusal had been preventing, moved one path over. Each fix is one
loop mirroring the resistive path. Both now carry an assembled-Jacobian finite-difference gate,
and both gates were verified to bite by deleting the stamps again (analytic −3.0e-4 vs a true
−1.0e-3).

**The lesson is about the shape of the change, not the arithmetic.** "Make path X consume a new
channel" is only safe once you have enumerated *every* path that can receive that channel. The
audit did that enumeration as a table; it should have been part of the original commit.

### Still open, from the same enumeration

- ~~**`stamp_laplace` drops it too**~~ — **fixed 2026-08-31**, see below.
- **The charge channel's `debug_assert!` premise is false.** Its comment says `validate()` should
  have rejected a charge argument that itself carries charge; `lower::contains_ddt_call` is
  *syntactic* and cannot see through `Expr::Var`, so `x = V*ddt(q); I <+ ddt(c0*x);` reaches it
  — panicking in debug, silently dropping the second-derivative sensitivity in release.
- **The trapezoidal guard is bypassable for the same syntactic reason**: `x = V*ddt(q); I <+ x;`
  builds under `Integration::Trapezoidal` while the direct spelling is correctly refused. Harm is
  bounded by whatever the first-step seeding fix addresses (see the correction entry above), but
  the guard should be closed or documented.

### Block scoping covers less than claimed

`69abf1f` called `bsimsoi.va` "the last genuine elaborator limitation in the frontend column."
**That is too strong.** `parse_block_or_single` returns a flat `Vec<Stmt>` and **drops the
`begin…end` boundary**, so only a *standalone* block becomes a `Stmt::Block`. When `begin…end` is
the body of an `if`/`else`/`for`/`while`/`repeat`/`case` arm — the shape real compact models
actually use — no scope is pushed by either pass, and a `Stmt::VarDecl` writes its fresh `VarId`
straight into the module-wide map, shadowing the parameter for the rest of the analog block.

Same circuit, same declaration, only the enclosing construct differing:

```text
if-arm  begin…end:  V(mid) = 0.001996 V   ← a 0.5 Ω device
standalone begin…end: V(mid) = 0.500000 V   ← the correct 1 kΩ
```

This is **not a regression** — identical before block scoping landed, and `82045f0`'s hard-error
guard never covered this shape either — but it is a live instance of the very 1 kΩ→1 Ω bug the
commit describes.

**Fixed the same day.** `parse_stmt_body` now returns a `Stmt::Block` for a `begin ... end`
wherever one can appear, so an arm body is a block like any other and the scoping machinery
applies unchanged — no elaboration change at all. `parse_block_or_single` becomes a thin wrapper
returning that as the one-element list the arm-bearing AST nodes store, and the two sites that
already wrapped (a standalone block in `parse_stmt`, and the `analog` item) no longer double-wrap,
which is what keeps the IR shape — and the committed golden IR — unmoved.

The audit's reproducer now reads `V(mid) = 0.500000 V`, the correct 1 kΩ, where it read
`0.001996 V` before. Tests pin both an `if`-arm body and a `while` body, so the fix is not
special-cased to one construct.

**One deliberate shape change:** each iteration of an unrolled `for ... begin ... end` generate
loop is now its own `Stmt::Block`. That is correct — an iteration *is* a scope — and the four
generate tests that asserted a flat unrolled list now flatten before asserting, via a
`flatten_blocks` helper that says why in its doc comment.

**Also from the audit, in the safe direction:** a function body's `collect_assign_targets` is
scope-blind (no `decl_scopes` analogue), so a block-local declaration inside a function now
allocates a second `VarId` and leaves the hoisted one dead. Codegen catches it as
`variable #N read before assignment` — an error rather than the silently wrong value it gave
before — but the diagnostic names neither the position nor the cause.

---

## The first step is backward Euler (2026-08-31)

The fix the correction entry above specified. `run_with_events` now takes the **first step with
the backward-Euler companion regardless of `cfg.method`**, then hands over to the configured
method — standard SPICE practice, and here it removes a specific defect.

**Why.** A `ddt` evaluated as an ordinary sub-expression reconstructs its own primal as
`dq/dt = coeff·(q − q_prev) − prev_rate_weight·dq/dt|_prev`. At `tstart` there is no previous
rate, so `is_initial_step` seeds it `0.0`, while the true `q̇(0)` is nonzero for any start that is
not a steady state. Trapezoidal's `prev_rate_weight` then feeds that `O(1)` seed into a recursion
whose multiplier is exactly `−1` — **undamped**, so it never decays, only alternates. Backward
Euler has `prev_rate_weight = 0` and never reads the seed; one BE step is enough, because from
the second step on `dq/dt|_prev` is a real reconstructed rate accurate to `O(h)`.

**All 15 gates pass, and one improved by 40×:**

| gate | before | after |
|---|---|---|
| `ring_osc` | 1.799e-4 | **4.464e-6** |
| `rc_step` | 1.839e-5 | 1.839e-5 |
| `rectifier` | 6.766e-4 | 6.766e-4 |
| `abstime_ramp` | 4.382e-17 | 4.382e-17 |

`ring_osc` is the one gate whose accuracy is dominated by its startup transient, so it is exactly
where a better-seeded first step should show up — and it is a recorded golden number moving, which
this file records rather than quietly re-baselining. The other three are unchanged to every digit,
which is the expected result: they route every `ddt` through the charge channel and so read
neither `coeff` nor `prev_rate_weight`.

### The trapezoidal refusal stays, deliberately

The analysis says this fix makes the bias-dependent-`ddt` case exact under trapezoidal too, so
`va_codegen::Integration`'s refusal could now be lifted and the type retired. **Not done here.**

The evidence for lifting is an off-repo numerical study; the evidence *against* being hasty is
that twice today a plausible-looking lift turned out to drop physics silently — and the §5
finite-difference gate provably **cannot** see this class of defect, because it checks `J` against
`f` rather than checking that `f` is the right discrete equation. Lifting a refusal on strength of
numerics this repo cannot re-run is the same mistake in a new place.

What would justify lifting it: an **in-repo order-of-convergence test** — a model with a genuinely
bias-dependent `ddt` coefficient, integrated on a deliberately *varying* step pattern, against a
closed-form solution, asserting second order. A fixed-step version would pass either way and prove
nothing, which is precisely how this defect stayed invisible. Until that exists, the refusal is
**conservative rather than necessary**, and this entry is the record of which it is.

**That test now exists** (see the entry below); the bar is met, and the refusal is confirmed
unnecessary rather than merely suspected so.

## An order-of-convergence gate for the rate reconstruction (2026-08-31)

The evidence the entry above named as the bar for lifting `va_codegen::Integration`'s trapezoidal
refusal, now in the repo and reproducible.

`va-transient`'s tests gained a device carrying `c(v)·dv/dt + v/R = 0` with `c(v) = 1 + a·v` — a
genuinely bias-dependent charge coefficient — stamped exactly the way `va-codegen` stamps one
(reconstructed rate into `residual`, its instantaneous sensitivity into `jacobian`, `c·∂q/∂v` into
`dcharge`, and **no** `charge`). That ODE separates to `ln v + a·v = const − t/R`, so there is a
closed form to measure against. The observed convergence order is asserted, not an absolute error:
an absolute bound would silently pass a first-order run at a small enough step.

**It discriminates**, which is the only property that matters here:

| | backward Euler (control) | trapezoidal |
|---|---|---|
| with the first-step fix | 0.982 | **1.999** |
| with it disabled | 0.982 | **0.954** |

Backward Euler is measured alongside as a control precisely so a passing trapezoidal number cannot
be mistaken for a harness that reports 2 no matter what.

**The part worth remembering is how nearly it proved nothing.** The first version of this study
came out order 2.0 with the fix *and* order 2.0 without it. The schedule was uniform: `bound_step`
can only *shrink* a landing and the controller already caps `h` at `cfg.tstep`, so requesting `h`
or `2h` when `h` was already the cap left every step identical. The long step of the pattern has to
*be* `cfg.tstep` for the short one to bind. A `DUMP_SCHEDULE=1` hook now prints the realized
pattern, kept deliberately: a convergence study that silently degenerates to fixed-step is a test
that passes for the wrong reason, and this one did, once.

## Retiring `va_codegen::Integration` (2026-08-31)

With the first step taken by backward Euler and an order-of-convergence gate holding it in place,
the bias-dependent `ddt` coefficient is second order under trapezoidal too. So the refusal is
gone, and with it the type that carried the choice.

**A generated model is method-independent again.** `build_instance` is back to its original
three-argument signature; `build_instance_with`, `va_codegen::Integration`, and the
`integration` parameter threaded through `validate_stmts`/`lower`/`lower_stmt` and through
`va-cli`'s `build_instances`/`build_instance`/`build_from_model` are all gone. That is the right
architecture, and the reason is worth stating: **a compiled model should not know which
discretization will step it.** The method-dependence was never a property of the model — it was
the integrator's rate reconstruction seeding its first step wrong, and once that was fixed there
was nothing left for a model to be compiled *for*.

**`va-cli sim --integration be|trap` stays**, now meaning exactly what its name says: the
integrator's method. It no longer has to keep two choices in step, because there is only one.
Default remains trapezoidal — second order, and what every committed transient golden was
validated against.

**The three negative controls became positive ones.** They had asserted "exact under backward
Euler, refused under trapezoidal"; they now assert the term *reaches a channel* — that
`dcharge` is non-zero after a load — rather than merely that the build succeeds. A build that
quietly dropped the charge would otherwise pass, which is the exact failure mode the original
refusal existed to prevent, and the reason the assertion is on the stamp rather than on the
`Result`.

**Unchanged, and checked:** 583 tests, `xtask validate` 15/15, corpus 86/88 frontend and 84/88
frontend+codegen with all four product-rule files (`hicumL2V2p4p0`, `hicumL2V3p0p0`,
`hicumL2_v310`, `mvsg_cmc_3.2.0`) still building. `lower::is_param_only` still bites, and the
separate `dropped_ddt` refusal — a `ddt` assigned inside a branch arm and contributed after it,
`hicumL0_v2p1p0`'s shape — is untouched and still correct: that one is not about the integration
method, and lifting it naively makes the term vanish entirely (see this file's own note).

## A `laplace_*` input carrying a time derivative (2026-08-31)

The last of the paths the `grad_ddt` enumeration turned up, and the only one that was
**pre-existing** rather than exposed by lifting the nested-`ddt` refusal: nothing ever checked a
filter's input for a `ddt`, so `laplace_nd(c(x)·ddt(q(x)), num, den)` stamped `u.grad` only and
dropped the input's `c·∂q/∂x` entirely — Newton degrading without failing.

**Stamped rather than refused**, unlike the two sibling refusals it sits next to (`laplace` nested
in a resistive term, `ac_stim` nested). The reason is not taste: **carrying it is unconditional,
whereas detecting it is not.** `lower::contains_ddt_call` is syntactic and cannot see through an
`Expr::Var` holding a `ddt`-derived value, so a refusal would be bypassable in exactly the shape
that matters, while the stamp is correct however the term was spelled.

Linearizing `y = H·u` with `δu = (grad + jω·grad_ddt)·δx`:

```text
δy = (re + j·im)(grad + jω·grad_ddt)·δx
   → jacobian:  re·grad − ω·im·grad_ddt
   → dcharge :  im·grad/ω + re·grad_ddt        (the assembler forms G + jωC)
```

The `grad` halves are what the code already had. In DC and transient `ω` is zero and a
real-coefficient filter has `im = 0` there, so only `re·grad_ddt` survives — `re` being the DC
gain the filter already folds to.

**Gated, and the gate bites:** a central difference of the assembled residual against
`jacobian + coeff·dcharge`, plus an explicit "`dcharge` is not all zero" assertion. With the new
stamp deleted the test fails on that assertion, which is the point — an FD check alone would pass
a model whose contribution had silently become zero.

**Both halves are now gated**, the AC one by a trick worth reusing: spell **one** transfer
function two ways. `laplace_nd(V, {0,c}, {1,τ})` goes entirely through `grad`;
`laplace_nd(ddt(c*V), {1}, {1,τ})` goes entirely through `grad_ddt` — `grad` vanishes because `c`
is a parameter, and in AC the primal is zero. With `D = 1 + ω²τ²`:

```text
A:  re = ω²cτ/D, im = ωc/D   →  G = re·grad        = ω²cτ/D,  C = im/ω·grad   = c/D
B:  re = 1/D,    im = −ωτ/D  →  G = −ω·im·grad_ddt = ω²τc/D,  C = re·grad_ddt = c/D
```

Identical in both channels at every ω, and both equal the closed form for a series R–C branch
with `R = τ/c`, `C = c` — the very network the committed `laplace_ac` golden already uses. So it
is an equivalence *and* an absolute check, needing **no new golden data**. Mutation-tested:
flipping the cross-term's sign turns `G` into a negative conductance and deleting it makes the
input admittance identically zero; both are caught.

An independent check also confirmed the algebra, the `Some(gb)` branch-constraint signs, and that
`im` is **exactly** zero at ω=0 for all four spellings including complex root forms — the LRM's
`∏(1 − s/ζ)` normalization makes `H(0)` identically 1 regardless of the roots. So the `if ac`
guard on the cross-term is defensive, not load-bearing.

**Stated limitation, pre-existing:** under `AnalysisKind::Noise` the `ac` flag is always false, so
every `laplace_*` collapses to `H(0)` and this term degenerates to a plain capacitor. The `grad`
half already behaved that way; this term inherits it rather than introducing it.

## An analog operator reaching a contribution through a variable (2026-08-31)

`lower`'s `contains_noise_call`, `contains_ac_stim_call` and `contains_ddt_call` establish
**silent-drop** safety properties: a `white_noise` buried where `noise_term_shape` cannot pull it
out contributes nothing, so it is refused rather than dropped. All three were purely syntactic,
with **no `Expr::Var` arm** — and one assignment defeated all three.

**The noise case was a live wrong answer, not a diagnostic gap.** `2.0*white_noise(...)` written
directly is refused. Written through a variable it built clean and the source contributed
**exactly zero**, silently:

| | S(a) @ 1 kHz | contributors |
|---|---|---|
| via a variable | 4.144e-18 V²/Hz | `R1 100.0%` |
| written directly | 8.284e-18 V²/Hz | `R1 50.0%`, `D1 50.0%` |

`ac_stim` behaved the same way (its value is zero in every analysis; only the split-out excitation
channel carries it). The `ddt` case was louder but no better: it reached the charge channel as a
*second* time derivative, tripping a `debug_assert` whose comment claimed `validate()` had already
rejected it — a false premise — and in release dropping the sensitivity.

**The fix is a taint fixed point, not a numeric check.** `Dual::carries_charge()` answers "does
this value carry a time derivative" exactly, but only *at a point*, and `validate` runs at the
all-zero operating point where a coefficient is routinely zero: `x = V(p,n)*ddt(c0*V(p,n))` has
`grad_ddt = 0` there, so a numeric check **would have passed the very module that then panics**.
`lower::Taint` instead grows a set of variables that can carry each operator, iterating over every
assignment to a fixed point — the same shape as `param_only_vars` right beside it, and
point-independent by construction. Computed once in `lower()` and carried on `Lowered`.

**Over-approximating, deliberately and in the safe direction:** non-path-sensitive, so
`x = ddt(q); x = 0.0; I <+ ddt(x);` is refused too. Refusing a shape nothing in the corpus writes
costs nothing; missing one costs physics.

**Zero corpus movement**, as predicted before the change: 86/88 frontend and 84/88
frontend+codegen, unchanged. A scan of all 158 files found 1136 `ddt` call sites across 53 files
and **not one** whose argument mentions a variable ever assigned from a `ddt`, nor any noise or
`ac_stim` call assigned to a variable at all.

**Known limit, recorded rather than fixed:** taint does not flow through an `analog function`'s
body, so a function that computes a `ddt` internally and returns it still defeats the scans. No
corpus file does this — 0 of 158 contain a `ddt` inside a function body — so it is documented at
`Taint`'s definition instead of speculatively implemented.

**The `va-frontend` twin is now fixed too** (same day): `Elaborator::contains_ddt` had the
identical blindness, which let `I(<port>)` carry displacement current when the `ddt` arrived
through a variable — so the direct spelling `I(a,c) <+ is*V(a,c) + ddt(cj*V(a,c));` and the
through-a-variable spelling of the *same physics* disagreed about what the probe reports,
contradicting the documented "conduction current only" invariant. `Elaborator::ddt_tainted_vars`
mirrors `lower::Taint`'s fixed point inside the frontend; `resistive_terms_only` consults it.
Five tests, including one that pins the probe to be *exactly* the surviving untainted read rather
than merely "something survived" — so an over-broad taint that dropped every variable term would
fail it.

**The last item from that audit is closed too.** The potential-contribution charge path
(`lib.rs`'s `c.charge` block for `V(p,n) <+ …`) had **no** assert in any build profile, so a
second time derivative reaching it dropped its sensitivity in silence, while the flow path at
least failed loudly in debug — the same defect differing only in which contribution kind it was
written under. It now carries the same backstop.

And the flow path's assert finally has a **true** premise. Its message had always claimed
"validate() should have rejected this module" while `contains_ddt_call` was purely syntactic, so
`x = V*ddt(q); I <+ ddt(c*x);` built cleanly and then tripped it mid-solve. With the taint fixed
point in place, `validate` really does reject that, and both asserts are what they say they are:
backstops, not the primary defence.

**Three diagnostics were printing their own source indentation.** A `\`-continued string literal
in this codebase does not survive `cargo fmt` intact — the continuation is flattened into a
literal run of spaces, so the message reaches the user with a 25-space gap in the middle of a
sentence. Affected: the `laplace_*` top-level-term refusal, and both charge-channel asserts.
Rewritten as single-line literals, which cannot be mangled. Two more in `va-frontend`
(`laplace_*`'s "needs at least one numerator/zero…" and the zero/pole parity message) had the
same defect and are fixed the same way. Worth knowing before writing the next long diagnostic.

## An escaping rate: `hicumL0`'s self-heating idiom (2026-08-31)

The last codegen gap. **84/88 → 85/88**, and the single remaining failure corpus-wide is a bug in
the corpus file itself. `hicumL0_v2p1p0.va` builds (11 nodes, 112 params).

The shape: `if (flsh == 0) i_cth = 0.0; else i_cth = ddt(cth*V(br_sht));` followed by a *later,
separate* `I(br_sht) <+ i_cth;`. `DdtVars` is forward and single-pass, so the binding is gone by
the time the contribution is reached.

**The naive lift is wrong, and I tried it first.** Simply letting the read fall through to an
ordinary resistive term makes the term contribute **nothing** — residual, jacobian, charge and
dcharge all zero — because a `ddt`-shape assignment was never *emitted* as a statement, so the
variable keeps its earlier value. That is worse than the refusal it removes. The fix has to change
the **write** site, not the read site.

**The discriminator is the LRM, not an implementation limit.** §4.5.15: an analog operator inside
`if`/`case` is legal only when the controlling condition is a *constant* expression, and is
illegal inside a loop at all. That is exactly the boundary between "exact" and "silently wrong"
here — under a time-invariant guard the arm choice cannot flip mid-run, so the `ddt` site is
evaluated at every accepted timepoint and its committed history really is the previous step's.
Under a solution-dependent guard it is not: `ModelState` pre-seeds scratch from committed, so on a
step where the arm is skipped the site's `q_prev` survives stale, and the next time it is taken
`coeff` belongs to the current step while `q_prev` is several steps old — an O(1) wrong rate with
no diagnostic. So the refusal survives, **narrowed, with the language as its justification**.
`hicumL0`'s guards are `flsh` and `cth`, both parameters, so it falls on the legal side.

**Two passes, because the mark cannot be made at the assignment.** It is created when the
*enclosing* construct closes, strictly after the assignment was lowered — a forward single pass
cannot know. Pass 1 discovers the escaping set and throws its statements away; pass 2 reruns and
emits what pass 1 elided. Emission is *conditional* on that set, which bounds the blast radius to
files that error today: an unconditional emit would start evaluating every `ddt`-shape assignment
in `validate`, dead ones included, where `Dual::into_ddt` errors on an argument already carrying
charge.

**The trap worth recording:** `is_param_only` is sitting in the same file and looks like the
obvious guard predicate. It would be **wrong** — it accepts any variable in `param_only`, and
`param_only_vars` is deliberately non-path-sensitive, so `asmhemt.va`'s
`if (V(g) > voff) ct = ctrap3; else ct = 1.0e-9;` makes `ct` "parameter-only" while its value
genuinely changes with bias. Guarding on it would admit precisely the case this refuses. The new
`is_time_invariant` accepts `Const`/`Param` and arithmetic over them only — no `Expr::Var`, no
`Expr::Call`.

**Tested on stamps, never on `Result::is_ok()`** — the naive fix built fine and stamped nothing.
The positive test asserts four things that separate this path from every neighbour:
`dcharge == cth` (the capacitance is there, and the right size), `charge == 0` (it took the
product-rule path, *not* the charge channel — a non-zero charge would double-count through the
offset), `jacobian == 0` (a constant coefficient has no `(dq/dt)·∂c/∂x` half), and a residual
matching the rate reconstructed from **non-zero committed history** — run under a transient
context, because under DC `ddt_coeff` is zero and a broken primal would sail through. Verified to
bite: deleting the emit gives `dcharge = 0`. Two negatives pin the narrowed refusal
(solution-dependent guard, loop body), and the pre-existing straight-line control still asserts
`charge != 0`, so the fix cannot have over-reached into the fold.

**Also corrected here:** this module's doc comment and `token-reference.md` both stated the old
unconditional rule — "a `ddt`-shape assign never becomes a `LoweredStmt::Assign`", and that
§4.5.15 is entirely unenforced. The first is no longer true; the second is now true only of
`va-frontend`, since `va-codegen` enforces one slice of it.

## How to keep this document honest

- Update a phase's status when its gate goes green; link the proving `va-harness` run or test.
- When the declared subset is in question, resist scope creep (`CLAUDE.md` §1) — add a
  *Limitations* note to the relevant tutorial instead of silently widening scope.
- If a phase forces an interface change, that is a §6 coordinated event, not a solo edit —
  note it here and in `interfaces.md`.
