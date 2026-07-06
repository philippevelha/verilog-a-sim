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

> Updated 2026-06-29. Legend:
> **✅ Complete** — code, tests, the validation gate, *and* the tutorial are all green.
> **🟢 Code complete** — implementation + unit/FD tests committed and green (`fmt`,
> `clippy -D warnings`, `test` clean), but at least one of {harness-vs-golden gate, Quarto
> tutorial} is still outstanding, so the phase is **not yet "done"** by criteria 1–2 above.
> **⬜ Not started.**

| Phase | What exists | Status |
|-------|-------------|--------|
| 0 — shared contracts | `va-ir`/`va-abi` frozen; resistor/capacitor/diode reference models pass stamp tests; bridge specs in `docs/bridges/` | 🟢 |
| T1.1 — lexing | `logos` lexer over the subset; 8 tests | 🟢 |
| T1.2 — parsing | recursive-descent parser + arena AST; precedence/associativity; 6 tests | 🟢 |
| T1.3 — elaboration | AST → `va_ir::Module`; the three zoo models elaborate end-to-end; 6 tests | 🟢 |
| T2.1 — AD core | forward-mode dual numbers over the IR arena; FD-checked | 🟢 |
| T2.2 — lowering | IR → `ModelInstance`; generated resistor/diode reproduce the reference stamps | 🟢 |
| T2.3 — charge channel | `ddt` terms routed to the charge channel (capacitor); broad coverage ongoing | 🟢 |
| T3.1 — MNA & dense solve (staff-maintained, not a thesis — see T3 section) | `assemble` + `faer` LU solve with singularity detection | 🟢 |
| T3.2 — Newton & divider (staff-maintained, not a thesis) | Newton loop; resistor divider solves to the analytic midpoint | 🟢 |
| T3.3 — nonlinear DC & sweep (staff-maintained, not a thesis) | diode–resistor clamp converges; DC `sweep`; `convergence` aids (helpers) | 🟢 |
| T4.1 — integration (fixed-step superseded by T4.2) | backward Euler + trapezoidal companion model; RC charging curve matches analytic to <1% | 🟢 |
| T4.2 — adaptive timestep & LTE | embedded-pair LTE estimate drives accept/reject + grow/shrink; 9 tests | 🟢 |
| T4.3 · T5 · T6 | crate stubs only (`todo!()`) | ⬜ |

**Two caveats that keep every "🟢" honest** (per criteria 1–2 at the top):

1. **No harness-vs-golden validation yet.** `va-harness`, `golden/`, and the CLI are still
   stubs, so the analysis crates are validated against *analytic values and inline unit
   tests*, **not** against committed ngspice golden. The formal ladder-rung gates (rung 1 DC
   ≤ 1e-4, etc.) cannot go green until T6 lands. Rungs below track *implementation*, not a
   passed gate.
2. **No Quarto tutorials written yet.** Only the `docs/tutorials/` scaffold exists. A phase
   with green tests but no tutorial is explicitly *not finished* (criterion 2), which is why
   nothing above is marked ✅.

Also approximated vs. the literal phase wording, and worth tightening later: T1.3 uses
structural IR assertions rather than committed golden-IR snapshots; T2.2 checks the generated
diode at an operating point + FD rather than a full committed sweep; T2/T3 currently run over
hand-built IR / reference instances — the frontend→codegen→core path is not yet wired by a
netlist driver (that is T6).

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
the **whole `external/` tree, ~118 `.va`/`.vams` files**: real industry-standard compact models
(BSIM3/4/6/CMG/SOI/BULK, HiSIM/HiSIM-HV/SOI, HICUM L0/L2, PSP, EKV, VBIC, MOSVAR, JFET, MVSG,
ASM-HEMT, and more), plus their shared headers/macro-definition/nature-definition fragments.
Of the 118, roughly 20 are auxiliary include fragments (`*MacrosAndDefines*.va`,
`constants.vams`, `disciplines.vams`, `ekv3_*_def*.va`, …) never meant to compile standalone —
`va-cli check` naively tries anyway, so their "failures" are a scan artifact, not a language
gap; don't read the raw pass count as a language-completeness percentage without excluding
them. A second, distinct artifact category (8 more files, found this pass — see "Not chased,
unclear if real" below): top-level `.va` files whose module body was itself split into a sibling
`` `include ``d file that the corpus snapshot never shipped (the PSP102/103/104 family,
`L_UTSOI_102[_nqs]`, `r2_cmc`/`r2_et_cmc`) — these fail with a misleading "port has no
discipline declaration" (an empty module body, not a language gap) and are excluded from the
gap accounting below for the same reason as the ~20 fragments. As of this pass: **62/118 pass
outright**, with the remainder split across real, now-categorized gaps below and the ~28
expected non-language-gap failures.

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
the baseline itself never regresses. Net *declarations* still only accept the
`electrical`/`thermal` keywords — a stated v1 limit (see the backlog below), not corpus-tested
against any real file (none in `external/` declares a net with a custom discipline). And
**`absdelay(value, delay[, max_delay])`** (LRM §4.5.9) — same DC-steady-state-fold family as
`transition`/`slew`/`$limit`: settles to its undelayed `value` with no delay history at a fixed
operating point, so it folds transparently at elaboration exactly like those (`delay`/
`max_delay` parsed, never evaluated). Closes `external/fbh_hbt-2_1.va`, moving the pass count
from 61 to 62.

**Backlog, prioritized** (highest-value/most-tractable first, re-derived against the full
118-file corpus):

1. **Laplace/Z-domain filters** (`laplace_nd`/`np`/`zd`/`zp`, `zi_nd`/`np`/`zd`/`zp`) — blocked
   on array/list-literal expression syntax (`{1, 2, 3}`), which the grammar doesn't have at all
   yet; a DC answer (the filter's gain at s=0/z=1, from the coefficient arrays) is sound once
   that syntax exists. Do the array-literal grammar work once, then revisit.
2. **Time-history-dependent event functions** (`last_crossing`, real `cross`/`timer`/`edge`
   semantics) — cannot be soundly approximated at DC the way `transition`/`slew` can (their
   whole purpose is time history); genuinely blocked on `va-transient` existing.
3. **Escaped identifiers** (`` \name `` — LRM §2.7) and a stray `` \ `` line-continuation lexed
   as an error in `external/bsimsoi.va` — not yet triaged in detail; low file count (1) so low
   priority, but a real lexer gap (escaped identifiers are legitimate Verilog-A, not a fragment
   artifact).
4. **Custom-discipline net declarations** — a net can still only be declared `electrical`/
   `thermal` (dedicated keyword tokens); accepting an arbitrary parsed-discipline identifier
   (`optical p1, p2;`) needs new lookahead disambiguation against module instantiation's "a bare
   leading `Ident` at item level → `parse_instance`" rule (e.g. `Ident Ident (` = instance vs.
   `Ident Ident ,`/`;`/`[` = net declaration). Zero real-world need found in `external/`, so not
   urgent, but the natural next step toward `CLAUDE.md` §1's multi-physics goal ("disciplines
   optical, thermal, mechanical, etc") — `va_ir::Discipline::Other` already exists in the IR for
   exactly this, still never constructed.
5. **Wiring parsed nature metadata into convergence/multi-physics** — `units`/`abstol`/
   `idt_nature`/`ddt_nature` are parsed and stored (`disciplines.rs::NatureDecl`) but never read
   by `va-core` or elaboration; a real per-discipline `abstol` could feed `convergence.rs`'s
   `gmin`/damping aids once a net's discipline round-trips that far.
6. **`Elaborator::reference_node`'s hardcoded-electrical ground** — every single-terminal
   access's implicit "gnd" second terminal is hardcoded `Discipline::Electrical` regardless of
   the access's own discipline (e.g. a bare `Temp(dt)` still resolves against an
   electrical-tagged reference node); pre-existing, not introduced by the discipline/nature
   pass, and not fixable without per-access discipline tracking that doesn't exist even for
   electrical/thermal today.
7. **`ground` declaration** — `Token::Ground` is lexed and reserved but still has no grammar
   production in `parse_item` at all; the implicit "gnd" node (`reference_node`, above) is the
   only reference-node convention this project has.

**Permanently out of scope, not a backlog item** (LRM Annex C.7: "No digital behavior or
events are supported in Verilog-A" — these are excluded from Verilog-A *itself*, not narrowed
further by this project): gate/switch-level primitives (`and`/`nand`/`nmos`/`bufif0`/…), net
strength/charge-storage keywords (`strong0`/`trireg`/`highz0`/…), and digital procedural/timing
constructs (`always`/`initial`/`fork`/`join`/`task`/`wait`/`specify`/`casex`/`casez`/…). See
`token-reference.md` §1.6 for the full, word-by-word accounting.

**Not chased, unclear if real**: `external/hicumL0_v2p0p0.va` and its siblings (6 HICUM/L0
files) contain `IB = I(<b>);` — literal angle brackets around the terminal name, inside an
`` `ifdef PORT_CURR `` block that *is* active (`PORT_CURR` is `` `define ``d at the top of the
file). This isn't recognizable Verilog-A syntax under any reading found so far; before writing
a parser rule for it, worth checking the model's own upstream source/changelog (it's guarded by
`CALC_OP`/`OP_STATIC`, an operating-point-debug-only code path) for whether this is a
known-broken construct in the CMC release itself rather than something this project should
parse.

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
  posture (`CLAUDE.md` §5) intact end to end, including in the tutorials. It lives in `va-cli`
  and `va-harness` (T6 already owns both, so no cross-crate/interface change): a `--plot
  out.svg` flag on `sim`/`sweep` and on `va-harness`'s golden comparison emits an SVG that the
  `.qmd` embeds as a plain markdown image. Not wired up yet — `va-transient` (T4) is still a
  stub, so there's no waveform to plot; add the `plotters` dependency when T4.1 lands its
  first RC transient (ladder rung 3), rather than speculatively now.
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
> comments. Subset documented in the module header (no separate grammar file yet). 8 tests.
> *Outstanding:* `t1-frontend/01-lexing.qmd`.

- Define the supported Verilog-A subset precisely (tokens, keywords, operators). Write it
  down as a grammar before writing code.
- Implement the lexer (optionally `logos`); property/round-trip tests on token streams.
- **Tutorial:** `t1-frontend/01-lexing.qmd` — the subset grammar + tokenization, with the
  "what we deliberately do *not* support" section.

### Phase T1.2 — Parsing to an AST
> **Status: 🟢 code complete** — recursive-descent parser + arena AST in
> `va-frontend/src/{parser,ast}.rs`; precedence-climbing expressions (correct `*`/`+`
> precedence, right-associative `**`). Returns `FrontendError::Parse` (no panics). 6 tests.
> *Outstanding:* `t1-frontend/02-parsing.qmd`.

- Recursive-descent (or chosen) parser → AST for module headers, ports, params with ranges,
  the analog block, `<+`, `if/else`, analog function calls.
- Error handling returns `Result` with `thiserror` enums (never panics — §5).
- **Tutorial:** `t1-frontend/02-parsing.qmd` — AST shape, parsing strategy, error reporting.

### Phase T1.3 — Elaboration → `va-ir`
> **Status: 🟢 code complete** — `va-frontend/src/elaborate.rs` lowers AST → `va_ir::Module`:
> nets→`NodeId`, const-eval'd params + ranges, branch accesses→`BranchId`, builtins→`Builtin`.
> All three zoo models elaborate end-to-end (the `compile()` milestone test is green). 6 tests.
> *Outstanding:* committed golden-IR snapshots (currently structural assertions);
> `t1-frontend/03-elaboration.qmd`.

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
> **Status: 🟢 code complete** — `va-codegen/src/ad.rs`: forward-mode `Dual` over the IR
> arena (`+ - * / neg`, `exp/ln/log10/sqrt/abs`, variable-exponent `pow`) with an eval `Ctx`.
> Each operator is FD-checked (`div_matches_finite_difference`, `exp_chain_rule`).
> *Outstanding:* `t2-codegen/01-ad-core.qmd`.

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
> conductance; **§5 AD-vs-FD milestone green**. *Outstanding:* `if/else` + analog functions
> (v0 rejects them); full committed sweep; `t2-codegen/02-lowering.qmd`.

- Generate (or interpret) a `ModelInstance` from an elaborated `Module`: map `<+`
  contributions to residual stamps and their AD-derived Jacobian entries.
- Handle `if/else` branches and analog functions.
- **Validation gate:** the generated diode model's stamps match `va-abi`'s hand-written
  reference diode within FD tolerance, across a voltage sweep.
- **Tutorial:** `t2-codegen/02-lowering.qmd` — from Interface α to Interface β; generated vs
  reference diode, side by side.

### Phase T2.3 — Charge channel (transient-ready) & coverage
> **Status: 🟢 partial** — `ddt(q)` terms are routed to the charge/`dcharge` channel; the
> generated capacitor stamps only charge (`Q=C·V`, `dQ/dV=C`), ready for T4. `idt` and a
> formal coverage matrix are still open; `ddt` is recognised only as a top-level additive
> term. *Outstanding:* coverage tracking; `t2-codegen/03-charge-and-coverage.qmd`.

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
> **Status: 🟢 code complete** — `va-core/src/mna.rs` `assemble` walks instances into the
> `System` sink (ground reduction via `row < dim`); `linsolve.rs` does a `faer` LU solve with
> singularity detection (non-finite output or failed `A·x≈b` check). 6 tests.
> *Outstanding:* `t3-core/01-mna.qmd`.

- Assemble the system (`mna.rs`) from a set of `ModelInstance`s via `StampSink`; dense solve
  through `faer` (`linsolve.rs`). Pure-Rust, no native deps (§5).
- **Tutorial:** `t3-core/01-mna.qmd` — nodal analysis, how stamps become a matrix, solving a
  linear resistor network by hand vs by code.

### Phase T3.2 — Newton & the resistor-divider rung
> **Status: 🟢 code complete (harness gate pending)** — `va-core/src/newton.rs` Newton loop
> (assemble → `J·dx=−f` → `x+=dx`), converging on residual≤abstol **or** relative update≤reltol.
> The resistor divider solves to the analytic midpoint (`1.0 V`, < 1e-9). *Outstanding:* the
> formal rung-1 gate is vs **ngspice golden via `va-harness`** — awaits T6; currently checked
> against the analytic value. `t3-core/02-newton.qmd`.

- Newton–Raphson loop (`newton.rs`) with abstol/reltol; solve the linear resistor divider.
- **Validation gate (ladder rung 1):** resistor divider DC matches golden ≤ 1e-4.
- **Tutorial:** `t3-core/02-newton.qmd` — the Newton iteration, convergence criteria, the
  first green `va-harness` run.

### Phase T3.3 — Nonlinear DC, sweeps & convergence aids
> **Status: 🟢 code complete (harness gate pending)** — nonlinear Newton converges on a
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
> stage finishes it off in a handful of iterations. *Outstanding:* rung-2 gate vs golden (T6);
> `t3-core/03-nonlinear-dc.qmd`.

- Diode I–V; DC operating point + parameter sweep (`dc.rs`); convergence aids (`gmin`
  stepping, source stepping, damping) in `convergence.rs`.
- **Validation gate (ladder rung 2):** diode I–V sweep matches golden ≤ 1e-4; convergence
  fraction tracked.
- **Tutorial:** `t3-core/03-nonlinear-dc.qmd` — why diodes are hard, what each convergence
  aid does, the convergence-rate metric.

---

## T4 — `va-transient` (integration · timestep/LTE · events)

**Fallback:** a report on integration methods + LTE timestep control.

### Phase T4.1 — Fixed-step integration & the RC rung
> **Status: 🟢 code complete (harness gate pending)** — `integrator.rs` implements both
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
> *Outstanding:* rung-3 gate is vs **ngspice golden via `va-harness`** — awaits T6; currently
> checked against the analytic RC solution. `t4-transient/01-integration.qmd`.

- Companion-model the charge channel; implement an implicit integrator (backward Euler →
  trapezoidal) in `integrator.rs`; fixed timestep first.
- **Validation gate (ladder rung 3):** RC transient waveform RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/01-integration.qmd` — companion models, BE vs trapezoidal, the
  first transient waveform vs ngspice.

### Phase T4.2 — Adaptive timestep & LTE control
> **Status: 🟢 code complete (harness gate pending)** — `run()` adapts `h` within
> `[cfg.tstep_min, cfg.tstep]` via an **embedded-pair LTE estimate**, not a rigorous
> divided-difference truncation-error calculation: every accepted step computes *both*
> `BackwardEuler` and `Trapezoidal` from the same `(x_prev, h)` (one reported, one purely an
> error reference), and their disagreement — weighted by `cfg.lte_reltol`/`cfg.lte_abstol`,
> the same `reltol·|x|+abstol` combination `va-core`'s Newton `reltol`/`abstol` use — drives
> accept/reject and grow/shrink (`SHRINK_FACTOR`/`GROWTH_FACTOR`, fixed multiplicative
> constants, not a power-law order-based controller). Below `cfg.tstep_min` without meeting
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
> *Outstanding:* rung-4 gate vs golden (T6, needs a diode model in the loop — not yet tried
> here, only the linear RC circuit); a rigorous divided-difference LTE estimator to replace
> the embedded-pair heuristic; `t4-transient/02-lte-timestep.qmd`.

- Local truncation error estimate driving adaptive step size; step accept/reject logic.
- **Validation gate (ladder rung 4):** diode rectifier transient RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/02-lte-timestep.qmd` — LTE estimation, the step controller, why
  the rectifier needs it.

### Phase T4.3 — Events & breakpoints
- Event handling / breakpoints (`events.rs`) for sources and discontinuities; ring-oscillator
  shakedown.
- **Validation gate (ladder rung 6):** ring oscillator transient is stable and matches golden
  within band.
- **Tutorial:** `t4-transient/03-events.qmd` — breakpoints, forced timepoints, the oscillator
  demo.

---

## T5 — `va-acnoise` (AC linearization · noise: PSD, adjoint)

**Fallback:** an AC/noise-formulation report (adjoint-method derivation).

### Phase T5.1 — AC linearization
- Linearize about a DC operating point; complex-valued solve over a frequency sweep
  (`ac.rs`).
- **Validation gate:** RC / RLC AC magnitude & phase within the stated band vs golden.
- **Tutorial:** `t5-acnoise/01-ac.qmd` — small-signal linearization, the complex MNA system,
  a Bode plot vs ngspice.

### Phase T5.2 — Noise analysis
- Per-element noise sources → output PSD; adjoint method for transfer functions (`noise.rs`).
- **Validation gate:** resistor thermal noise / diode shot noise PSD within band vs golden.
- **Tutorial:** `t5-acnoise/02-noise.qmd` — noise-source models, the adjoint derivation, the
  output-referred PSD plot.

---

## T6 — `va-netlist` + `va-cli` + `va-harness` (integration & validation)

**Shared substrate — staff first, reliable student (§10).** This thesis owns three crates and
is the glue: it makes everyone else's work runnable and trustworthy.
**Fallbacks:** netlist-format design note · pipeline integration/UX report · validation-
methodology + metrics report vs ngspice.

### Phase T6.1 — Netlist parser & the harness/metrics skeleton
- Circuit-level netlist parser (`va-netlist`): elements, nodes, model bindings, analysis
  directives. Define the metric functions in `va-harness` (`DC_REL`, `TRAN_RMS`, …).
- **Tutorial:** `t6-integration/01-netlist.qmd` — the netlist format and how a circuit maps
  onto Interface β instances.

### Phase T6.2 — CLI wiring & golden generation
- `va-cli` wires the full pipeline (parse model → codegen → assemble → solve → report); flesh
  out `xtask gen-golden` (ngspice) and `xtask validate`.
- **Validation gate:** `cargo run -p va-cli -- sim circuits/divider.net …` reproduces ladder
  rung 1 end-to-end through the real pipeline.
- **Tutorial:** `t6-integration/02-cli.qmd` — driving the simulator, the golden-generation
  workflow.

### Phase T6.3 — Full validation harness & the metrics dashboard
- `va-harness` runs the whole zoo vs `golden/`, reports per-rung pass/fail and the convergence
  fraction; resample-and-compare for transient.
- **Validation gate:** all passed ladder rungs are green under one `cargo xtask validate`.
- **Tutorial:** `t6-integration/03-validation.qmd` — the metrics, tolerances, and the
  ladder-status dashboard; how "done" is measured.

---

## Cross-thesis milestones (the bring-up ladder)

Each rung is a shared demo where the responsible theses present their tutorials together:

| Rung | Circuit            | Analysis  | Lights up                | Tutorials presented           | Status |
|------|--------------------|-----------|--------------------------|-------------------------------|--------|
| 1    | resistor divider   | DC        | T3 (+ T6 via CLI)        | T3.2, T6.2, shared            | solves analytically in `va-core`; **harness/CLI gate pending T6** |
| 2    | diode I–V          | DC sweep  | T1, T2, T3               | T1.3, T2.2, T3.3              | pieces work in isolation (frontend, codegen, nonlinear DC); not yet wired or golden-gated |
| 3    | RC                 | transient | T4 (+ T2 charge)         | T2.3, T4.1                    | solves to the analytic charging curve in `va-transient`; **harness/CLI gate pending T6** |
| 4    | diode rectifier    | transient | T4                       | T4.2                          | ⬜ |
| 5    | a MOS              | DC        | T1, T2, T3 (model reach) | T1/T2 coverage updates        | ⬜ |
| 6    | ring oscillator    | transient | T4 (full stack)          | T4.3                          | ⬜ |

Stretch rungs for T5 (AC/noise) hang off rung 1–2 circuits (RC/RLC) once a DC operating point
is available.

> **No rung is formally "passed" yet** — passing requires `va-harness` green against committed
> `golden/` (per `validation.md`), which awaits T6. The table records *implementation reach*,
> not passed gates.

---

## How to keep this document honest

- Update a phase's status when its gate goes green; link the proving `va-harness` run or test.
- When the declared subset is in question, resist scope creep (`CLAUDE.md` §1) — add a
  *Limitations* note to the relevant tutorial instead of silently widening scope.
- If a phase forces an interface change, that is a §6 coordinated event, not a solo edit —
  note it here and in `interfaces.md`.
