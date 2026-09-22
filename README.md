# verilog-a-sim

---

![logo](Verilog-A-sim.png)

---

A clean-room, from-scratch **Verilog-A circuit simulator written in pure Rust**, built as a
coordinated set of master's theses. It compiles Verilog-A — the full language is the target,
and `docs/token-reference.md` records construct by construct how far the implementation is
from it — to automatically differentiated model instances and solves them with an MNA / Newton
core: DC operating point and sweep, transient, AC and noise, validated against QSPICE to stated
tolerances. Multi-physics comes from Verilog-A disciplines: the model zoo has electrical,
thermal, mechanical, optical and traffic examples solved in one system.

**Version 1.4.0** (2026-09-22). The deliverable is the compiled `va-cli` executable, run on
Windows, Linux and macOS. No `cgo`, no BLAS/LAPACK/KLU — every dependency is pure Rust, so a
build is reproducible and a binary has nothing to install alongside it.

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

---

## Install the executable

Download the archive for your platform from
[**releases**](https://github.com/philippevelha/verilog-a-sim/releases), unpack it, and run
from inside the unpacked directory. Nothing to install, no Rust toolchain, no checkout.

| Platform | Archive |
|---|---|
| Linux, x86-64 | `va-cli-v1.4.0-x86_64-unknown-linux-gnu.tar.gz` |
| Windows, x86-64 | `va-cli-v1.4.0-x86_64-pc-windows-msvc.zip` |
| macOS, Apple silicon | `va-cli-v1.4.0-aarch64-apple-darwin.tar.gz` |

```bash
tar xzf va-cli-v1.4.0-x86_64-unknown-linux-gnu.tar.gz
cd va-cli-v1.4.0-x86_64-unknown-linux-gnu
./va-cli sim circuits/rectifier.net --model models/diode.va --tran
```

```powershell
# Windows: unzip, then from the unpacked directory
.\va-cli.exe sim circuits/rectifier.net --model models/diode.va --tran
```

On macOS the first run may need `xattr -d com.apple.quarantine va-cli` — the binary is not
notarised. Each archive carries `va-cli`, the model zoo (`models/`), every example deck
(`circuits/`), `workflow.md`, `README.md`, `LICENSE` and `release.txt`.

**Check the install.** The rectifier is the reference run: it should report
`Transient analysis (718 points, …)` and `V(out)=4.304467 V` at `t=2.536873e-4s`, the same
digits on all three platforms — the adaptive timestep sequence is deterministic, so an equal
point count means the same floating-point environment and not merely a plausible waveform.
`docs/workflow.md` §"What it works on this machine means" has the full three-deck check and
what to report.

## A run in one command

A simulation takes three things: a **Verilog-A model** (`models/*.va`, the component), a
**SPICE-flavoured deck** (`circuits/*.net`, the testbench — instances, sources and the analysis
card), and one `sim` call.

```bash
./va-cli sim circuits/rectifier.net --model models/diode.va --tran
```

```
[va-cli] sim netlist=circuits/rectifier.net model=models/diode.va analysis=Transient
[va-cli] compiled 1 Verilog-A module(s) from models/diode.va
[va-cli] circuit: 4 device(s) (1 compiled), 3 unknown(s) (2 net(s) + 1 auxiliary row(s)), ~502 points (adaptive, 502 is the card's floor)
[va-cli] estimate: 3.1-151.1 ms of solve, 216 B of matrix — rough, dense LU scaled from bench-scale on an i7-1185G7
Transient analysis (718 points, t=0 to t=5e-3s):
  t=0.000000e0s  V(in)=0.000000 V  V(out)=0.000000 V  I(V1)=0.000000e0 A
  t=1.000000e-5s  V(in)=0.313953 V  V(out)=1.850106e-8 V  I(V1)=-1.868607e-9 A
  …
  t=2.536873e-4s  V(in)=4.998658 V  V(out)=4.304467 V  I(V1)=-4.529304e-3 A
```

There is no separate elaborate step: `sim` lexes, parses, elaborates, differentiates and solves
in one invocation, in memory. Nothing is written between the `.va` and the result, so there is
no stale artefact and nothing to rebuild when a model changes — edit the `.va` and run again.

### Every analysis, one line each

The analysis comes from the deck's card; the flag selects which one to run.

```bash
# DC operating point                    deck: .op
./va-cli sim circuits/divider.net
# DC sweep                              deck: .dc V1 0 1 0.01
./va-cli sim circuits/diode_iv_params.net --model models/diode.va --report V1
# Transient                             deck: .tran <tstep> <tstop>
./va-cli sim circuits/rectifier.net --model models/diode.va --tran
# Small-signal AC                       deck: .ac dec <pts> <fstart> <fstop>
./va-cli sim circuits/rc_ac.net --ac
# Noise                                 deck: .noise V(out) V1 dec <pts> <fstart> <fstop>
./va-cli sim circuits/diode_noise.net --model models/diode.va --noise
```

Useful flags: `--report <a,b,…>` to print only some quantities (a net, a device current, or a
full label like `V(mid)`), `--plot <out.svg>` for a waveform / sweep / Bode plot,
`--integration be|trap|gear` to choose the transient integrator (default trapezoidal),
`--model <dir>` to hand a whole directory of models to a multi-component deck.

`va-cli check <model.va|dir> [--codegen]` is the standalone front-end diagnostic — "does this
model build?", with or without lowering through `va-codegen` — and never a prerequisite for
`sim`.

### Refusals

If a model uses a construct the simulator recognises but deliberately declines, the run stops
with a `refused:` block saying **what** was refused, **where**, **why** an approximation would
have been wrong, what to write **instead**, and where the limitation is **tracked**. That is
deliberately distinct from a parse error ("your model is malformed") and from a convergence
failure ("the numerics gave up") — the three call for opposite next steps.

## What is in the box

- **35 Verilog-A models** and 5 discipline/nature headers in `models/` — `disciplines.vams`
  plus `mechanical.vams`, `photonic.vams`, `traffic.vams`, `constants.vams`.
- **49 decks** in `circuits/`, from the resistor divider to a motorway with ramp metering.
- **29 committed QSPICE golden files** in `golden/`, the oracle `cargo xtask validate` checks
  against.

The physics is not only electrical. Each domain is a Verilog-A discipline, solved in the same
matrix as everything it is coupled to:

| Domain | Examples |
|---|---|
| Electrical | resistor, capacitor, diode, MOSFET, non-linear cap, noise models |
| Thermal | `heater.va`, the opto-thermal microring tuning example |
| Mechanical | sprung mass, voice-coil actuator and its plant (mobility analogy) |
| Optical / photonic | waveguide, MZI, splitter, microring, photodiode, CW laser, ring gyro — including shot noise and RIN |
| Traffic | METANET-style sections, origin queues, ALINEA ramp metering, MPC |

`validation.md` (repository root) is the registry of every model and deck that reproduces a
published reference, by domain, with the equation it implements and what it was checked
against; `docs/examples.md` walks the worked examples with plots.

## Validation

No analysis result is trusted until `va-harness` checks it against committed reference output.
At 1.4.0: **`cargo xtask validate` is 28/28, convergence 28/28.**

| Analysis | Metric | Tolerance |
|---|---|---|
| DC | max relative I–V error on the operating point / sweep | ≤ 1e-4 |
| Transient | waveform RMS error (shared-timebase resample) | ≤ 1e-3 |
| AC | max relative magnitude error · max absolute phase error | ≤ 1e-4 · ≤ 1e-4 rad |
| Noise | max relative error on output, input-referred *and* per-device PSD | ≤ 1e-3 |

Three tiers of oracle are used, and every entry says which: **QSPICE golden** (the reference
simulator, committed output, the strongest tier — but only where QSPICE has a primitive for the
physics), **closed form** (the reference's own equation evaluated independently in a test, with
the tolerance stated), and **paper figure** (a number a paper quotes, reproduced with its
residual stated, never silently). All six rungs of the bring-up ladder pass — resistor divider,
diode I–V, RC transient, diode rectifier, MOS DC, ring oscillator. Automatic differentiation is
separately gated against central finite differences for every operator, because a wrong
Jacobian destroys Newton convergence silently.

## Limitations, stated

Shipped this way on purpose, with the reasoning in `release.txt`'s 1.0.0 entry:

- **Dense LU** is the linear solve. For a 10 000-point transient, ~400 unknowns is interactive,
  ~800 is a 9-minute coffee break, beyond ~1 600 it is impractical (measured 2026-09-17,
  `docs/validation.md` — and 2.8× faster at 800 unknowns than the same measurement a week
  earlier, which that section explains). Every `sim` prints its own size and a rough cost
  bracket before it starts. A sparse path is the first item of `docs/future_development.md`.
- **Refused, not approximated:** `absdelay` in a transient run (a pure delay is not an ODE),
  the analog events `absdelta` and `last_crossing`, compound triggers mixing a step event with
  a scheduled one, and `$rdist_*` (this engine has no RNG). Each raises the standard refusal
  block above.
- `I(<port>)` inside case statements and loops, and vector ports in `I(<port>)`, are narrower
  than the LRM.
- A cross-domain plot puts watts and amperes on one axis; `--report` one unit at a time is the
  documented answer.
- Coverage of real-world models is **112 of 132** module-declaring third-party Verilog-A files
  (94 of 99 self-contained ones) through frontend *and* codegen, measured with
  `va-cli check external --codegen`. That corpus is a locally collected set of published
  models and is not shipped in the repository; the 35 models in `models/` are, and all 35
  build.
- Out of scope by the language's own definition (Verilog-A LRM Annex C): `casex`/`casez`, the
  `===`/`!==` case-equality operators, `wreal`, discrete-domain nets and digital events, and
  the Verilog-AMS-only hierarchy constructs (`config`, `paramset`, `connectmodule`).

## Build from source

The source path is for developing the simulator; the executable above is for using it.

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask validate        # run va-harness over the model zoo vs golden/
cargo xtask gen-golden      # (re)generate golden outputs from QSPICE, if installed
cargo run -p va-cli -- sim circuits/rectifier.net --model models/diode.va --tran
```

The toolchain is pinned in `rust-toolchain.toml` (1.92.0, with `rustfmt` and `clippy`).
CI runs the whole gate — fmt, clippy with warnings denied, the workspace tests, `cargo xtask
validate` against committed golden (no QSPICE needed, the golden files are committed), and
`cargo deny` for the no-native-link rule — on Ubuntu, Windows and macOS for every push to
`main` and every pull request. Pushing a `v*` tag builds and publishes the three release
archives.

### Crates

Crate boundaries are thesis boundaries; `va-core` depends on `va-abi` and nothing else, which
is what lets the solver be built and validated without waiting on the compiler front end.

| Crate | Owns |
|---|---|
| `va-ir` | Interface α — the elaborated IR (arena/index, no graph of references) |
| `va-abi` | Interface β — `ModelInstance` / `StampSink` plus hand-written reference models |
| `va-frontend` | lexer, parser, AST, elaboration → `va-ir` |
| `va-codegen` | IR → automatic differentiation → model instances |
| `va-core` | MNA assembly, Newton, linear solve, convergence, DC |
| `va-transient` | integration, timestep/LTE control, events |
| `va-acnoise` | AC linearization, noise (PSD, adjoint) |
| `va-netlist` | the circuit-level netlist parser |
| `va-cli` | the binary front door wiring the pipeline |
| `va-harness` | golden-reference validation and metrics |

## Documentation

| File | What it holds |
|---|---|
| `docs/workflow.md` | from a model to a result, both the archive and the source path |
| `docs/token-reference.md` | every lexer token and parser construct, one by one, against the LRM |
| `docs/validation.md` | metrics, tolerances, every gate and why the ungated ones are ungated |
| `validation.md` | the reference registry: what reproduces which paper, by domain |
| `docs/examples.md` | worked examples with plots |
| `docs/interfaces.md` | the two frozen contracts (Interfaces α and β) |
| `docs/architecture.md`, `docs/thesis-map.md` | the pipeline, and crate ↔ thesis ↔ fallback |
| `docs/roadmap.md`, `release.txt` | the phased plan, and the release log with the road to 1.0 |
| `docs/future_development.md` | what comes after 1.0, sparse solve first |
| `docs/traffic.md`, `docs/photonic-noise.md`, `docs/prior-art.md` | domain notes and context |
| `CLAUDE.md` | the project constitution: scope, house rules, how an interface changes |

## License

Apache License 2.0 — see `LICENSE`.
