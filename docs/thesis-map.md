# Thesis Map

Crate boundaries **are** thesis boundaries. A student owns a crate and does not edit other
crates except via a coordinated interface change (§6). Fill in the owner column at kickoff.
Each thesis has a "a rigorous report is itself the thesis" fallback so no defense depends on
a sibling shipping (§10).

Progress legend (2026-06-29): 🟢 code complete + tests green (tutorial/harness gate may still
be outstanding — see `roadmap.md`); 🟡 partial; ⬜ stub only.

| Crate          | Thesis | Owns                                              | May depend on            | Owner | Progress | Fallback deliverable |
|----------------|--------|---------------------------------------------------|--------------------------|-------|----------|----------------------|
| `va-ir`        | shared | Interface α: elaborated IR data types             | — (leaf)                 | TBD   | 🟢 frozen | The ratified IR spec + rationale is itself a contribution. |
| `va-abi`       | shared | Interface β: `ModelInstance`/`StampSink` + ref models | — (leaf)             | TBD   | 🟢 frozen + ref models | The ABI design + reference-model report stands alone. |
| `va-frontend`  | T1     | lexer, parser, AST, elaboration → `va-ir`         | `va-ir`                  | TBD   | 🟢 lex/parse/elaborate | A rigorous Verilog-A subset grammar + parser study. |
| `va-codegen`   | T2     | IR → automatic differentiation → model instances  | `va-ir`, `va-abi`        | TBD   | 🟢 AD + lowering + charge | An AD-for-compact-models report (forward vs reverse, FD validation). |
| `va-core`      | T3     | MNA assembly, Newton, linear solve, convergence (DC) | `va-abi`              | TBD   | 🟢 MNA/Newton/DC (golden gate pending) | A study of MNA + Newton + convergence aids on the reference models. |
| `va-transient` | T4     | integration, timestep/LTE, events                 | `va-core`, `va-abi`      | TBD   | ⬜ stub | A report on integration methods + LTE timestep control. |
| `va-acnoise`   | T5     | AC linearization + noise (PSD, adjoint)           | `va-core`, `va-abi`      | TBD   | ⬜ stub | An AC/noise-formulation report (adjoint method derivation). |
| `va-netlist`   | T6     | circuit-level netlist parser                      | `va-abi`                 | TBD   | ⬜ stub | A netlist-format + parser design note. |
| `va-cli`       | T6     | binary front-door wiring the pipeline             | all                      | TBD   | ⬜ stub | An integration/UX report on driving the pipeline. |
| `va-harness`   | T6     | golden-reference validation + metrics             | `va-cli`                 | TBD   | ⬜ stub | A validation-methodology + metrics report vs ngspice. |

## Staffing notes (§10)

- Staff `va-core` (T3) and `va-harness`/`va-cli` (T6) first, with reliable students — the
  critical path and the shared substrate.
- `va-codegen`'s AD (T2) is the highest-risk, highest-value crate — strongest student.
- Ratify and freeze §4 (the interfaces) **before** advertising topics. That meeting comes
  first; the whole program lives or dies on it.
