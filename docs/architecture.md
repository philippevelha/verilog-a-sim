# Architecture

The pipeline has two halves joined by two **frozen interfaces** (see `interfaces.md`). The
whole multi-author build hinges on those interfaces being defined first and never casually
changed.

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
                                                              [va-harness] ─► vs QSPICE
```

## The load-bearing invariant

`va-core` depends on `va-abi` (Interface β) and **nothing else**. It is validated against
the hand-written reference models in `va-abi`, so the core team is never blocked waiting on
the compiler team. Both teams target the same trait.

## Data flow

1. **`va-frontend`** lexes/parses/elaborates Verilog-A into a `va-ir::Module` (Interface α).
   `token-reference.md` is the token-by-token/construct-by-construct reference for this
   crate — what each lexer token and parser construct means, whether it's resolved at
   elaboration or simulation time, and how it's implemented (or, honestly, not yet).
2. **`va-codegen`** lowers the IR and differentiates it (forward-mode AD) into a
   `va-abi::ModelInstance`.
3. **`va-netlist`** parses the circuit deck and wires devices to global unknown indices,
   instantiating reference models or codegen-produced models.
4. **`va-core`** assembles the MNA system from each instance's stamps and drives Newton with
   a dense `faer` solve plus convergence aids; `dc` wraps this as operating point / sweep.
5. **`va-transient`** and **`va-acnoise`** extend the core with time integration and
   small-signal/noise analysis.
6. **`va-cli`** wires the pipeline; **`va-harness`** validates results against `golden/`.

## House rules that shape the design

- Arena/index everything graph-shaped (no `Rc`/`RefCell`/`Box`-graphs/`&'a` ASTs).
- Pure-Rust numerics via `faer`; no `cgo`/native-link deps (enforced by `deny.toml`).
- Libraries return `Result<_, E>` (`thiserror`) and never panic on bad input.
- Every differentiated operator is checked against finite differences.
- No result is trusted until `va-harness` is green against a golden reference.
