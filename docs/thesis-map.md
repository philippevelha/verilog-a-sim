# Thesis Map

Crate boundaries **are** thesis boundaries. A student owns a crate and does not edit other
crates except via a coordinated interface change (§6). Fill in the owner column at kickoff.
Each thesis has a "a rigorous report is itself the thesis" fallback so no defense depends on
a sibling shipping (§10).

Progress legend (2026-06-29): 🟢 code complete + tests green (tutorial/harness gate may still
be outstanding — see `roadmap.md`); 🟡 partial; ⬜ stub only.

| Crate          | Thesis  | Owns                                              | May depend on            | Owner | Progress | Fallback deliverable |
|----------------|---------|-----------------------------------------------------|--------------------------|-------|----------|----------------------|
| `va-ir`        | shared  | Interface α: elaborated IR data types             | — (leaf)                 | TBD   | 🟢 frozen | The ratified IR spec + rationale is itself a contribution. |
| `va-abi`       | shared  | Interface β: `ModelInstance`/`StampSink` + ref models | — (leaf)             | TBD   | 🟢 frozen + ref models | The ABI design + reference-model report stands alone. |
| `va-frontend`  | T1      | lexer, parser, AST, elaboration → `va-ir`         | `va-ir`                  | TBD   | 🟢 lex/parse/elaborate (incl. LRM string-literal escapes, the empty statement `;`, and a zero-module compilation unit) | A rigorous full Verilog-A grammar + parser study — see `token-reference.md` for the token-by-token reference and coverage record this thesis is built on. |
| `va-codegen`   | T2      | IR → automatic differentiation → model instances  | `va-ir`, `va-abi`        | TBD   | 🟢 AD + lowering (incl. local variables, `if`/`else`, `case`, loops, user-defined functions incl. `output`/`inout` arguments, potential contributions incl. mixed flow/potential branches, parameter-scaled `ddt` incl. through local-variable coefficients and nested multiplications, a `ddt` result read back through a local variable, `idt` via its own auxiliary accumulator unknown, a self-probed purely-flow-defined branch via the same auxiliary-unknown mechanism, and a single-terminal implicit-ground probe of an uncontributed branch via a node-KCL sum) + charge; 72/115 real corpus files simulatable (see `roadmap.md` T1.1/T1.2/T2.2/T2.3) | An AD-for-compact-models report (forward vs reverse, FD validation). |
| `va-core`      | shared* | MNA assembly, Newton, linear solve, convergence (DC) | `va-abi`              | staff | 🟢 MNA/Newton/DC (golden gate pending) | N/A — no student assigned; see staffing notes below. |
| `va-transient` | T4      | integration, timestep/LTE, events                 | `va-core`, `va-abi`      | TBD   | 🟢 BE/trapezoidal + adaptive LTE + events + time-varying sources (T4.1–T4.3; ring-oscillator rung blocked on a gain-capable model) | A report on integration methods + LTE timestep control. |
| `va-acnoise`   | T5      | AC linearization + noise (PSD, adjoint)           | `va-core`, `va-abi`      | TBD   | ⬜ stub | An AC/noise-formulation report (adjoint method derivation). |
| `va-netlist`   | T6      | circuit-level netlist parser                      | `va-abi`                 | TBD   | 🟢 R/C/D/V elements + dot-cards incl. `.tran` timing + `SIN` waveform | A netlist-format + parser design note. |
| `va-cli`       | T6      | binary front-door wiring the pipeline             | all                      | TBD   | 🟢 `sim` drives DC and transient (incl. `SIN`-sourced circuits) through the real pipeline, `--plot` renders SVG waveforms (golden-gen/`xtask` still stubs) | An integration/UX report on driving the pipeline. |
| `va-harness`   | T6      | golden-reference validation + metrics             | `va-cli`                 | TBD   | 🟢 metrics + golden formats (`GoldenDc`/`GoldenSweep`) + `xtask validate` are real; `golden/` has one real QSPICE-generated reference (`divider.golden`), the rest still empty | A validation-methodology + metrics report vs QSPICE. |

\* `va-core` was advertised as T3 at kickoff. No T3 student was found (as of 2026-07-04). Of
the three fallback options considered — (1) scope T3 down to a smaller "harden the existing
core" thesis, (2) fold it into T2 or T6, (3) make it staff-maintained shared infrastructure
like `va-ir`/`va-abi` — we went with **(3)**: the risky part (does MNA/Newton/dense-solve even
work) was already implemented and green *before* the staffing gap became apparent, so treating
it like the other shared/leaf crates carries little risk and unblocks T4/T5/T6 immediately.
Unlike `va-ir`/`va-abi` it is not a leaf (it depends on `va-abi`), so it still participates in
§6's coordinated-interface-change process if `va-abi` ever changes underneath it.

## Staffing notes (§10)

- `va-core` is staff-maintained shared infrastructure, not a student thesis (see the table
  footnote above) — its remaining work (sparse solve, golden-vs-QSPICE validation once T6
  lands, and the `t3-core/*.qmd` tutorials) proceeds as a staff-owned maintenance backlog,
  tracked in `roadmap.md`'s T3 section, rather than a thesis deliverable with its own defense.
  Junction limiting (`convergence.rs`'s `limit_junction`) and `gmin` stepping
  (`gmin_for_step`/`NewtonConfig::gmin_steps`) are both now wired into the Newton loop
  (2026-07-04) — the latter via a small, additive Interface β change (`ModelInstance::
  unknown_kind`, a default method, `docs/interfaces.md`) so no existing implementor needed
  updating. See `roadmap.md`'s T3.3 for the full account.
- Staff `va-harness`/`va-cli` (T6) first among the remaining student theses — the shared
  substrate everyone else's demo depends on.
- `va-codegen`'s AD (T2) is the highest-risk, highest-value crate — strongest student.
- Ratify and freeze §4 (the interfaces) **before** advertising topics. That meeting comes
  first; the whole program lives or dies on it.
