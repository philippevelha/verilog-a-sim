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
| T2.2 — lowering | IR → `ModelInstance` incl. local-variable assignments, `if`/`else`, potential contributions (incl. mixed flow/potential), loops, `case`, user-defined analog functions, and parameter-scaled `ddt` (incl. through local-variable coefficients); 50/115 real corpus files pass frontend+codegen | 🟢 |
| T2.3 — charge channel | `ddt` terms routed to the charge channel (capacitor); broad coverage ongoing | 🟢 |
| T3.1 — MNA & dense solve (staff-maintained, not a thesis — see T3 section) | `assemble` + `faer` LU solve with singularity detection | 🟢 |
| T3.2 — Newton & divider (staff-maintained, not a thesis) | Newton loop; resistor divider solves to the analytic midpoint | 🟢 |
| T3.3 — nonlinear DC & sweep (staff-maintained, not a thesis) | diode–resistor clamp converges; DC `sweep`; `convergence` aids (helpers) | 🟢 |
| T4.1 — integration (fixed-step superseded by T4.2) | backward Euler + trapezoidal companion model; RC charging curve matches analytic to <1% | 🟢 |
| T4.2 — adaptive timestep & LTE | embedded-pair LTE estimate drives accept/reject + grow/shrink; `run_dynamic` rebuilds a time-varying source per step; 16 tests | 🟢 |
| T4.3 — events & breakpoints | `EventQueue` wired into `run_with_events`: forced exact landings, interpolated crossing detection; 15 `va-transient` tests total | 🟢 |
| T6.1 — netlist parser | R/C/D/V elements, dot-cards incl. `.tran` timing; `va_ir::Discipline` unaware, SPICE-flavored `.net` format | 🟢 |
| T6.2 — CLI wiring (DC + transient) | `va-cli sim` drives DC and `.tran` (incl. `SIN`-sourced circuits like the rectifier) through the real pipeline | 🟢 |
| T5 · T6.3 | crate stubs only (`todo!()`) | ⬜ |

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
  is a clear error rather than an empty/misleading image). `.qmd` tutorials embed the emitted
  SVG as a plain markdown image, unchanged from the original plan. Verified against the
  rectifier: `cargo run -p va-cli -- sim circuits/rectifier.net --tran --plot rectifier.svg`.
  *Outstanding:* a `va-harness` golden-comparison overlay plot (needs `va-harness` itself
  first, still `todo!()` — T6.3) and a DC sweep plot (`sim`'s DC path doesn't sweep yet either).
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
> precedence, right-associative `**`). Returns `FrontendError::Parse` (no panics). 6 tests.
> *Outstanding:* `t1-frontend/02-parsing.qmd`.
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
> preceding two rounds. Full committed sweep; `t2-codegen/02-lowering.qmd`.
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
> tracking; `t2-codegen/03-charge-and-coverage.qmd`.

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
> **2026-07-06: the rectifier itself now runs, through the real CLI pipeline** — see T6.2's
> update and rung 4 below. That needed one more piece not in scope when this phase's status
> was first written: `va_abi::ModelInstance::load` has no time parameter (Interface β's "no
> time, no frequency on the bridge" — `docs/bridges/interface-beta-abi.md` §7), so a genuinely
> time-varying source (`SIN(...)`, not a constant `DC` value) can't be expressed as a normal
> stateful-free instance. `integrator::run_dynamic` is the fix: it rebuilds a caller-supplied
> subset of devices fresh at every step attempt (the value baked in fresh each time), while
> everything else in the circuit stays a fixed, borrowed instance exactly as before —
> `va-cli`'s `build_instances_split` is the one caller that needs this today.
> *Outstanding:* a rigorous divided-difference LTE estimator to replace the embedded-pair
> heuristic; `t4-transient/02-lte-timestep.qmd`.

- Local truncation error estimate driving adaptive step size; step accept/reject logic.
- **Validation gate (ladder rung 4):** diode rectifier transient RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/02-lte-timestep.qmd` — LTE estimation, the step controller, why
  the rectifier needs it.

### Phase T4.3 — Events & breakpoints
> **Status: 🟢 code complete (harness gate blocked — see below, not just pending)** —
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
> frequency against a reference simulator; `t4-transient/03-events.qmd`.

- Event handling / breakpoints (`events.rs`) for sources and discontinuities; ring-oscillator
  shakedown.
- **Validation gate (ladder rung 6):** ring oscillator transient genuinely oscillates (done,
  2026-07-09 — see the status block above) *and* matches golden within band (still pending
  T6.3's harness).
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
> **Status: 🟢 code complete (harness gate pending — see T6.3)** — `va-netlist/src/parser.rs`
> is a real line-oriented SPICE-flavored parser: `R`/`C`/`D`/`V` elements (SI-suffixed values,
> `k`/`meg`/`u`/`n`/`p`/…), `0`/`gnd` as the reference sentinel, and dot-cards (`.op`/`.dc`/
> `.tran`/`.ac`). **2026-07-06: `.tran <tstep> <tstop>` timing is now captured**
> (`Netlist::tran`), not just the card marker — needed once `va-cli` actually drives a
> transient run (see T6.2). `va-harness`'s metric functions (`DC_REL`, `TRAN_RMS`) are declared
> but still `todo!()` — that's the genuinely outstanding piece, tracked under T6.3, not this
> phase. *Outstanding:* `t6-integration/01-netlist.qmd`.

- Circuit-level netlist parser (`va-netlist`): elements, nodes, model bindings, analysis
  directives. Define the metric functions in `va-harness` (`DC_REL`, `TRAN_RMS`, …).
- **Tutorial:** `t6-integration/01-netlist.qmd` — the netlist format and how a circuit maps
  onto Interface β instances.

### Phase T6.2 — CLI wiring & golden generation
> **Status: 🟢 code complete (harness/golden-generation gate pending)** — `va-cli sim` already
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
> produces a textbook half-wave-rectified, RC-filtered waveform — `V(out)` never follows
> `V(in)`'s swing to −5 V, peaks near 4.3 V (5 V minus a silicon diode drop), and shows the
> expected ripple decay between cycles, all driven through the real frontend/netlist/core/
> transient pipeline, no golden reference needed to see it's doing the right thing.
> `xtask gen-golden`/`xtask validate` remain unimplemented (T6.3/`xtask` territory).
> *Outstanding:* `t6-integration/02-cli.qmd`.

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
| 1    | resistor divider   | DC        | T3 (+ T6 via CLI)        | T3.2, T6.2, shared            | `cargo run -p va-cli -- sim circuits/divider.net` solves it through the real pipeline; **golden gate pending `va-harness` (T6.3)** |
| 2    | diode I–V          | DC sweep  | T1, T2, T3               | T1.3, T2.2, T3.3              | pieces work in isolation (frontend, codegen, nonlinear DC); not yet wired or golden-gated |
| 3    | RC                 | transient | T4 (+ T2 charge)         | T2.3, T4.1                    | `cargo run -p va-cli -- sim circuits/rc_step.net --tran` solves it through the real pipeline; **golden gate pending `va-harness` (T6.3)** |
| 4    | diode rectifier    | transient | T4                       | T4.2                          | `cargo run -p va-cli -- sim circuits/rectifier.net --tran` produces a correct half-wave-rectified/RC-filtered waveform; **golden gate pending `va-harness` (T6.3)** |
| 5    | a MOS              | DC        | T1, T2, T3 (model reach) | T1/T2 coverage updates        | ⬜ |
| 6    | ring oscillator    | transient | T4 (full stack)          | T4.3                          | `cargo test -p va-transient ring_oscillator_sustains_oscillation` — real, growing oscillation from an unstable DC equilibrium, `va-abi::Bjt` (new); **golden gate pending `va-harness` (T6.3)** (see T4.3) |

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
