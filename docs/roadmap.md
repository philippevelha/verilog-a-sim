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
| T3.1 — MNA & dense solve | `assemble` + `faer` LU solve with singularity detection | 🟢 |
| T3.2 — Newton & divider | Newton loop; resistor divider solves to the analytic midpoint | 🟢 |
| T3.3 — nonlinear DC & sweep | diode–resistor clamp converges; DC `sweep`; `convergence` aids (helpers) | 🟢 |
| T4 · T5 · T6 | crate stubs only (`todo!()`) | ⬜ |

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

**Critical path — staff first, reliable student (§10).**
**Fallback:** a study of MNA + Newton + convergence aids on the reference models.

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
> `operating_point` + `sweep`. `convergence.rs` ships `pnjlim`-style junction limiting and a
> geometric `gmin` schedule as **tested helpers**, not yet wired into the loop (limiting needs
> per-device state the stateless ABI doesn't carry). *Outstanding:* rung-2 gate vs golden
> (T6); wiring the aids into a homotopy loop; `t3-core/03-nonlinear-dc.qmd`.

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
- Companion-model the charge channel; implement an implicit integrator (backward Euler →
  trapezoidal) in `integrator.rs`; fixed timestep first.
- **Validation gate (ladder rung 3):** RC transient waveform RMS ≤ 1e-3 vs golden.
- **Tutorial:** `t4-transient/01-integration.qmd` — companion models, BE vs trapezoidal, the
  first transient waveform vs ngspice.

### Phase T4.2 — Adaptive timestep & LTE control
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
| 3    | RC                 | transient | T4 (+ T2 charge)         | T2.3, T4.1                    | charge channel ready (T2.3); needs T4 |
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
