# verilog-a-sim

A clean-room, from-scratch Verilog-A compact-model circuit simulator written in pure Rust,
built as a coordinated set of master's theses. It compiles a defined subset of Verilog-A to
differentiated model instances and solves them with an MNA / Newton core (DC, transient, and
— as a stretch — AC + noise), validated against ngspice to stated tolerances.

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

## Build & test

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask validate        # run va-harness over the model zoo vs golden/
cargo xtask gen-golden      # (re)generate golden outputs from ngspice, if installed
cargo run -p va-cli -- sim circuits/divider.net --model models/resistor.va
```

See `CLAUDE.md` for the project constitution and `docs/` for the frozen interfaces,
architecture, thesis map, and validation plan.
