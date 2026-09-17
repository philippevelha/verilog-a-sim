# verilog-a-sim

---

![logo](Verilog-A-sim.png)

---

A clean-room, from-scratch Verilog-A circuit simulator written in pure Rust, built as a
coordinated set of master's theses. It compiles Verilog-A — the full language is the target,
and `docs/token-reference.md` records construct by construct how far the implementation is
from it — to automatically differentiated model instances and solves them with an MNA / Newton
core: DC operating point and sweep, transient, AC and noise, validated against QSPICE to stated
tolerances. Multi-physics comes from Verilog-A disciplines: the model zoo has electrical,
thermal, mechanical, optical and traffic examples solved in one system.

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

## Install

Since 1.0 the deliverable is a compiled executable — no Rust toolchain, no checkout. Download
the archive for your platform from
[releases](https://github.com/philippevelha/verilog-a-sim/releases), unpack it, and run from
inside the unpacked directory:

```bash
./va-cli sim circuits/rectifier.net --model models/diode.va --tran     # use .\va-cli.exe on Windows
```

Each archive carries `va-cli`, the model zoo (`models/`), every example deck (`circuits/`) and
`workflow.md`. x86-64 Linux, x86-64 Windows and Apple-silicon macOS; `docs/workflow.md` says
what a good run looks like on each.

## Build & test

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask validate        # run va-harness over the model zoo vs golden/
cargo xtask gen-golden      # (re)generate golden outputs from QSPICE, if installed
cargo run -p va-cli -- sim circuits/divider.net --model models/resistor.va
```

## Running a simulation

A run takes a Verilog-A model (`models/*.va`, the component), a SPICE-flavoured deck
(`circuits/*.net`, the testbench: instances, sources, and the analysis card), and one command:

```bash
cargo run -p va-cli -- sim circuits/rectifier.net --model models/diode.va --tran
```

There is no separate elaborate step — `sim` lexes, parses, elaborates, differentiates and
solves in one invocation. `docs/workflow.md` walks through it, from either the release
archive or the source tree.

See `CLAUDE.md` for the project constitution and `docs/` for the frozen interfaces,
architecture, thesis map, and validation plan; `validation.md` is the registry of every model
and circuit that reproduces a reference paper or reference result, by domain; `release.txt`
carries the release log and the road to 1.0 that closed with it, and
`docs/future_development.md` what comes after.
