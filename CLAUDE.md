# CLAUDE.md — `verilog-a-sim`

> A clean-room, from-scratch Verilog-A circuit simulator written in Rust, built as a
> coordinated set of master's theses. This file is the project constitution **and** the
> one-shot bootstrap spec. If the workspace is empty, jump to **§9 Bootstrap** and build
> the hierarchy exactly as specified. Otherwise, treat §1–§8 as standing rules for every
> session.
>
> *(Working name `verilog-a-sim`, crate prefix `va-`. Rename the repo freely; keep the
> `va-` prefix consistent so imports and docs stay coherent.)*

---

## 1. What this project is — and is NOT

We are building a **complete pipeline ** simulator:

- **In scope:** a defined subset of the Verilog-A LRM (single-module compact models;
  electrical + thermal disciplines; `<+` contributions; `ddt`/`idt`; `if/else`; analog
  functions; parameters with ranges), DC operating point + sweep, transient, and
  (stretch) AC + noise. A handful of validated models. Dense → simple sparse solve.
  Basic convergence aids. multi-physics implementation governed by disciplines optical, thermal, mechanical, etc


"Done" means: **the declared subset works end-to-end and is validated against a reference
simulator (ngspice) to stated tolerances.** Scope creep past the declared subset is the
primary failure mode — resist it.

Every public item carries an honest caveat about its limitations. We prefer incremental,
verification-driven work with explicit model-limitation notes over silent breadth.

---

## 2. Architecture in one screen

The pipeline has two halves joined by two **frozen interfaces**. The whole multi-author
build hinges on these interfaces being defined first and never casually changed.

```
 Verilog-A source                         circuit netlist
        │                                        │
   [va-frontend]  ──IR (Interface α)──►  [va-codegen]      [va-netlist]
   lex/parse/elab                        IR → AD → models       │
                                                │               │
                                          ModelInstance (Interface β)
                                                │               │
                                                ▼               ▼
                                    ┌──────────────────────────────────┐
                                    │            [va-core]             │  ← depends ONLY
                                    │   MNA · Newton · linsolve · conv │    on Interface β
                                    └──────────────────────────────────┘
                                       │              │            │
                                 [va-transient]  [va-acnoise]   [va-cli]
                                                                    │
                                                              [va-harness] ─► vs ngspice
```

**The load-bearing invariant:** `va-core` depends on `va-abi` (Interface β) and **nothing
else**. It is validated against the hand-written reference models in `va-abi`, so the core
team is never blocked waiting on the compiler team. Both teams target the same trait.

---

## 3. Workspace layout & crate ownership

Crate boundaries **are** thesis boundaries. A student owns a crate; they do not edit other
crates except via a coordinated interface change (see §6). Allowed internal dependencies
are listed and are enforced by what each `Cargo.toml` declares — keep them honest.

| Crate           | Thesis | Owns                                              | May depend on (internal)     |
|-----------------|--------|---------------------------------------------------|------------------------------|
| `va-ir`         | shared | Interface α: elaborated IR data types             | — (leaf)                     |
| `va-abi`        | shared | Interface β: `ModelInstance`/`StampSink` + ref models | — (leaf)                 |
| `va-frontend`   | T1     | lexer, parser, AST, elaboration → `va-ir`         | `va-ir`                      |
| `va-codegen`    | T2     | IR → automatic differentiation → model instances  | `va-ir`, `va-abi`            |
| `va-core`       | T3     | MNA assembly, Newton, linear solve, convergence (DC) | `va-abi`                  |
| `va-transient`  | T4     | integration, timestep/LTE, events                 | `va-core`, `va-abi`          |
| `va-acnoise`    | T5     | AC linearization + noise (PSD, adjoint)           | `va-core`, `va-abi`          |
| `va-netlist`    | T6     | circuit-level netlist parser                      | `va-abi`                     |
| `va-cli`        | T6     | binary front-door wiring the pipeline             | all                          |
| `va-harness`    | T6     | golden-reference validation + metrics             | `va-cli`                     |
| `xtask`         | infra  | dev automation (`cargo xtask ...`)                | (dev-only)                   |

`va-ir` and `va-abi` are **leaf crates with no internal dependencies** — that is what makes
them safe shared contracts. Do not add internal deps to them.

Full file tree the bootstrap must produce:

```
verilog-a-sim/
├── CLAUDE.md
├── Cargo.toml                 # [workspace]
├── rust-toolchain.toml        # pinned channel
├── rustfmt.toml
├── clippy.toml
├── deny.toml                  # cargo-deny: forbids cgo/native-link deps (see §5)
├── .cargo/config.toml         # xtask alias
├── .gitignore
├── README.md
├── LICENSE                    # placeholder; pick before publishing
├── docs/
│   ├── architecture.md
│   ├── interfaces.md          # the RATIFIED §4 contracts; freeze at kickoff
│   ├── thesis-map.md          # crate ↔ thesis ↔ owner ↔ fallback
│   └── validation.md          # metrics, tolerances, the model zoo
├── crates/
│   ├── va-ir/{Cargo.toml, src/lib.rs}
│   ├── va-abi/
│   │   ├── Cargo.toml
│   │   └── src/{lib.rs, stamps.rs, instance.rs, reference/{mod.rs, resistor.rs, capacitor.rs, diode.rs}}
│   ├── va-frontend/{Cargo.toml, src/{lib.rs, lexer.rs, parser.rs, ast.rs, elaborate.rs}}
│   ├── va-codegen/{Cargo.toml, src/{lib.rs, lower.rs, ad.rs}}
│   ├── va-core/{Cargo.toml, src/{lib.rs, mna.rs, newton.rs, linsolve.rs, convergence.rs, dc.rs}}
│   ├── va-transient/{Cargo.toml, src/{lib.rs, integrator.rs, events.rs}}
│   ├── va-acnoise/{Cargo.toml, src/{lib.rs, ac.rs, noise.rs}}
│   ├── va-netlist/{Cargo.toml, src/{lib.rs, parser.rs}}
│   ├── va-cli/{Cargo.toml, src/main.rs}
│   └── va-harness/{Cargo.toml, src/{lib.rs, metrics.rs}}
├── models/                    # Verilog-A model zoo (.va)
│   ├── resistor.va
│   ├── capacitor.va
│   └── diode.va
├── circuits/                  # test netlists
│   ├── divider.net
│   └── rectifier.net
├── golden/                    # ngspice reference outputs (committed)
├── tests/                     # workspace-level integration tests
└── xtask/{Cargo.toml, src/main.rs}
```

---

## 4. The two frozen interfaces (ratify at kickoff, then freeze)

These sketches are the **v0 contract**. Copy them verbatim into the scaffold, mirror them
into `docs/interfaces.md`, and treat any change as a coordinated event (§6). Stub bodies
with `todo!()` so the workspace compiles from day one.

### Interface α — elaborated IR (`va-ir`)

Arena/index representation is mandatory (see §5). Expressions and statements are stored in
`Vec`s and referenced by index types, never by `&` references or `Box` graphs.

```rust
// va-ir/src/lib.rs  (sketch — flesh out, do not restructure casually)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct NodeId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ParamId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ExprId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct BranchId(pub u32);

pub struct Module {
    pub name: String,
    pub ports: Vec<NodeId>,
    pub nodes: Vec<NodeDecl>,
    pub branches: Vec<Branch>,
    pub params: Vec<Param>,
    pub exprs: Vec<Expr>,   // arena
    pub analog: Vec<Stmt>,  // top-level analog block
}

pub enum Expr {
    Const(f64),
    Param(ParamId),
    Probe(Access),               // V(b) or I(b)
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Call(Builtin, Vec<ExprId>),  // exp, ln, ddt, idt, $vt, $temperature, ...
}

pub enum Stmt {
    Contribute { target: Access, value: ExprId },  // <+
    If { cond: ExprId, then_: Vec<Stmt>, else_: Vec<Stmt> },
    Assign { lhs: VarId, rhs: ExprId },
    Block(Vec<Stmt>),
}
```

### Interface β — model instance ABI (`va-abi`)

This is the project's internal "OSDI." `va-core` calls `load`; both `va-codegen`'s
generated models and `va-abi`'s hand-written reference models implement it. DC ignores the
charge channel; the transient integrator consumes it via a companion model.

```rust
// va-abi/src/stamps.rs
pub trait StampSink {
    fn residual(&mut self, row: usize, value: f64);            // current into node `row`
    fn jacobian(&mut self, row: usize, col: usize, value: f64); // dResidual[row]/dx[col]
    fn charge(&mut self, row: usize, value: f64);              // Q at `row`  (transient)
    fn dcharge(&mut self, row: usize, col: usize, value: f64); // dQ[row]/dx[col]
}

// va-abi/src/instance.rs
pub trait ModelInstance {
    /// Global unknown indices this instance contributes to (nodes + internal unknowns).
    fn unknowns(&self) -> &[usize];
    /// Evaluate at solution vector `x`; emit residual + Jacobian (+ charge in transient).
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
}
```

`va-abi` ships **working** `resistor`, `capacitor`, and `diode` reference models against
this trait at bootstrap, so `va-core` has something real to solve on commit #1.

---

## 5. House rules (non-negotiable)

- **Arena/index everything graph-shaped.** IR nodes, branches, expressions: `Vec<T>` +
  `Copy` index newtypes. No `Rc`/`RefCell`/`Box`-graph/`&'a`-threaded ASTs. This is the
  single rule that keeps the borrow checker out of students' way — follow it and Rust
  stays easy; ignore it and you will fight lifetimes for a month.
- **Pure-Rust numerics. No `cgo`, no native-link deps.** Linear algebra via `faer`. No
  BLAS/LAPACK/KLU/SuiteSparse FFI. `deny.toml` enforces this; reproducible `cargo build`
  is a project requirement (it is literally T6's deliverable).
- **No `unsafe`** without a written justification in a doc comment and owner sign-off.
- **Libraries never panic on bad input.** Return `Result<_, E>` with `thiserror` error
  enums. `unwrap()`/`expect()` allowed only in tests, `xtask`, and `main`.
- **AD is validated against finite differences.** Every model/operator that `va-codegen`
  differentiates has a test asserting analytic vs central-difference Jacobian agreement.
  No exceptions — a wrong Jacobian silently destroys Newton convergence.
- **Numerics validated against ngspice.** No analysis result is trusted until `va-harness`
  checks it against a committed golden output to a stated tolerance (§7).
- **Every public item has a doc comment**, and limitations are stated, not hidden.
- **Small PRs, one crate each.** Touching another crate means an interface change (§6).
- `cargo fmt` + `cargo clippy -- -D warnings` clean before every commit.

---

## 6. Changing a frozen interface

`va-ir` and `va-abi` are shared contracts. To change one:

1. Open an issue describing the change and every downstream crate affected.
2. Get the owners of affected crates to agree (this is a kickoff-style coordination, not a
   solo edit).
3. Update `docs/interfaces.md` and the crate together, in one PR, with stub adapters so
   the workspace keeps compiling.

Never silently widen or reshape these in a feature PR. A broken contract blocks every
sibling thesis at once.

---

## 7. Validation & the model zoo

Reference simulator: **ngspice** (used as an oracle only — we are not building on it).
`va-harness` runs the pipeline and compares to committed `golden/` outputs.

Metrics & default tolerances (tune in `docs/validation.md`):

- **DC:** max relative I–V error ≤ 1e-4 on the operating point / sweep.
- **Transient:** waveform RMS error ≤ 1e-3 (after a shared timebase resample).
- **AC:** magnitude/phase error within stated band.
- **Convergence:** fraction of zoo circuits that reach a solution (track as a number; it
  only ever needs to go up).

Bring-up ladder (each rung is a checkpoint): resistor divider → diode I–V → RC transient →
diode rectifier → a MOS DC → ring oscillator. A rung is "passed" only when `va-harness` is
green against golden.

---

## 8. Common commands

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask validate        # run va-harness over the model zoo vs golden/
cargo xtask gen-golden      # (re)generate golden outputs from ngspice, if installed
cargo run -p va-cli -- sim circuits/divider.net --model models/resistor.va
```

---

## 9. Bootstrap (run once, on an empty repo)

When this repo contains only `CLAUDE.md`, generate the **entire** hierarchy in §3, then
verify it builds and tests green. Be deterministic and faithful to the names above.

**Step 1 — Workspace root.** Create:
- `Cargo.toml` with `[workspace]`, `resolver = "2"`, `members = ["crates/*", "xtask"]`,
  and a `[workspace.dependencies]` table holding shared versions: `faer`, `thiserror`,
  `logos` (optional, for the lexer), `anyhow` (binaries/xtask only). Crates reference
  these with `dep = { workspace = true }`.
- `rust-toolchain.toml` pinning a recent stable channel + `rustfmt`, `clippy` components.
- `rustfmt.toml` (e.g. `max_width = 100`), a minimal `clippy.toml`.
- `deny.toml` configured to deny any dependency that links a native library (enforces the
  no-cgo rule).
- `.cargo/config.toml` with `[alias] xtask = "run --package xtask --"`.
- `.gitignore` (`/target`, editor cruft), a `README.md` (one paragraph + the §2 diagram +
  build commands), a placeholder `LICENSE`.

**Step 2 — Shared contract crates first.** Create `va-ir` and `va-abi` with the §4 types.
`va-abi` must include **compiling, working** `resistor`, `capacitor`, and `diode`
reference models implementing `ModelInstance` (these are real, not `todo!()`), plus a unit
test that loads the resistor and checks its stamp by hand.

**Step 3 — Remaining crates as compiling stubs.** Create every other crate per §3 with:
- the module files listed in the tree,
- public function/type signatures that express the crate's job,
- bodies as `todo!()` (libraries) — except where a trivial real implementation is obvious,
- at least one `#[test]` per crate (may be `#[ignore]` with a `// T<n>:` note describing
  the first milestone), so the test harness is wired from the start.

Respect the dependency table in §3 exactly — e.g. `va-core`'s `Cargo.toml` lists `va-abi`
and **must not** list `va-codegen` or `va-frontend`.

**Step 4 — Docs.** Populate `docs/interfaces.md` from §4 (verbatim contracts), and write
short stubs for `architecture.md` (the §2 diagram + prose), `thesis-map.md` (the §3 table
plus a "fallback deliverable" line per thesis), and `validation.md` (the §7 metrics).

**Step 5 — Zoo & harness skeleton.** Add minimal `models/*.va`, `circuits/*.net`, an empty
`golden/` with a `README`, and a `va-harness` that defines the metric functions (§7) with
`todo!()` comparison bodies and one wired example test.

**Step 6 — `xtask`.** Implement `validate` and `gen-golden` subcommand skeletons.

**Step 7 — Verify.** `cargo build --workspace`, `cargo fmt --all`, and
`cargo clippy --workspace --all-targets -- -D warnings` must all pass. `cargo test
--workspace` must pass (ignored tests are fine). Fix anything that doesn't. Report the
final tree and the green build at the end.

After bootstrap, delete nothing from §1–§8 — those are the standing rules. Update §3's
ownership table with real names once students are assigned.

---

## 10. For the supervisor (delete or keep)

- Staff `va-core` (T3) and `va-harness`/`va-cli` (T6) first and with reliable students;
  they are the critical path and the shared substrate respectively.
- `va-codegen`'s AD (T2) is the highest-risk, highest-value crate — strongest student.
- Each thesis has a "a rigorous report is itself the thesis" fallback; record it in
  `docs/thesis-map.md` at kickoff so nobody's defense depends on a sibling shipping.
- The whole program lives or dies on §4 being ratified and frozen before the topics are
  advertised. Do that meeting first.

## 11 Testing Strategy

- Unit tests for all model classes and utility functions: `tests/unit/`
- Integration tests for full simulation runs with known seeds: `tests/integration/`
- Every new crate must have at least one test that runs a minimal test.
- Integration tests must assert on deterministic metrics output (e.g., mean job 
  completion time) given a fixed seed.
- Do not mock internals. Run actual functions and modules in tests.
- Fixtures for standard electronic discipline live in `tests/fixtures/`.