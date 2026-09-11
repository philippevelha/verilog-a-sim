# Workflow — from a Verilog-A model to a simulation result

This is the workflow **as it stands today (v0.9.x)**: the simulator is driven from source, on
the developer's machine, through `cargo run`. The last section says what changes at 1.0.

## The three pieces

A simulation needs exactly three things:

| Piece | Where | Role |
|---|---|---|
| A Verilog-A model, `*.va` | `models/` (or anywhere) | A **component**: one `module` with ports, parameters and an `analog` block. It may instantiate other modules (hierarchy), but it holds no sources and names no analysis. |
| A netlist deck, `*.net` | `circuits/` (or anywhere) | The **testbench**: instantiates the components, wires them to nets, adds the sources, and names the analysis with a dot-card (`.op`, `.dc`, `.tran`, `.ac`, `.noise`). |
| One command | `va-cli sim` | Runs the whole pipeline — there is no separate elaborate/compile step to run first. |

The testbench is **not** a Verilog-A file. Verilog-A has no source elements or analysis
statements; the SPICE-flavoured deck supplies both. That is why `circuits/` and `models/` are
different directories with different syntaxes.

## Step by step, on the half-wave rectifier

### 1. The component — `models/diode.va`

A Verilog-A module named `diode`, two ports, parameters `Is`, `N`, … and an `analog` block with
`I(a,c) <+ …`. Nothing in it says how it will be driven.

### 2. The testbench — `circuits/rectifier.net`

```
* Half-wave diode rectifier — bring-up rung 4 (transient).
V1 in  gnd  SIN(0 5 1k)
D1 in  out  diode        <- the trailing token names the Verilog-A module
R1 out gnd  1000
C1 out gnd  1e-6
.tran 10u 5m
.end
```

Element lines follow SPICE conventions: `R`/`C`/`L`/`V`/`E`/`F`/`G`/`H`/`K` are built-in
primitives; `D`/`M`/`Q` are two- and three-terminal devices whose trailing token is a model
name; `X<name> <node>... <model> [param=value]...` places a compiled Verilog-A module with
**any** number of ports. `gnd` or `0` is the reference node. The full deck grammar and its
limitations are in `crates/va-netlist/src/parser.rs`'s crate docs.

### 3. Run

```bash
cargo run -p va-cli -- sim circuits/rectifier.net --model models/diode.va --tran
```

- `--model <file.va | dir>` names the Verilog-A source(s). A directory compiles every `.va`
  in it, which is what a deck with several custom modules needs. Every device whose model
  name matches a compiled module uses it; any other device falls back to `va-abi`'s
  hand-written reference primitives, so a deck of only `R`/`C`/`L`/`V`/… needs no `--model`
  at all.
- `--tran` / `--ac` / `--noise` select the analysis; with none of them the run is DC
  (`.op`, or `.dc` if the deck carries one). The flag must agree with the deck's dot-card.

What that prints (v0.9.14, 2026-09-11, abbreviated — the full run is 718 timepoints):

```
[va-cli] sim netlist=circuits/rectifier.net model=models/diode.va analysis=Transient
[va-cli] compiled 1 Verilog-A module(s) from models/diode.va
Transient analysis (718 points, t=0 to t=5e-3s):
  t=0.000000e0s  V(in)=0.000000 V  V(out)=0.000000 V  I(V1)=0.000000e0 A
  t=1.000000e-5s  V(in)=0.313953 V  V(out)=0.000000 V  I(V1)=-1.868607e-9 A
  ...
  t=2.536873e-4s  V(in)=4.998658 V  V(out)=4.304467 V  I(V1)=-4.529304e-3 A
  ...
  t=4.965734e-3s  V(in)=-1.068193 V  V(out)=2.148842 V  I(V1)=1.000000e-14 A
  t=5.000000e-3s  V(in)=-0.000000 V  V(out)=2.076457 V  I(V1)=1.000000e-14 A
```

Read it as a physicist would: `V(out)` peaks at 4.30 V, i.e. the 5 V crest minus a ~0.7 V
silicon diode drop; it then decays through the 1 kΩ / 1 µF load (RC = 1 ms) while the diode
is reverse-biased, during which `I(V1)` collapses to the `gmin` floor (1e-14 A). The timestep
is adaptive — dense around the diode's turn-on, coarse on the smooth decay — which is why the
`t=` column is not uniform.

### 4. Optional: a plot, a narrower report, a different integrator

```bash
cargo run -p va-cli -- sim circuits/rectifier.net --model models/diode.va --tran \
    --report in,out --plot rectifier.svg --integration trap
```

- `--plot <out.svg>` writes the transient waveform, the `.dc` sweep, or the AC Bode plot.
- `--report a,b,...` prints only those quantities — a net (`out`), a device current (`V1`),
  or a full label (`V(out)`); an unknown name is an error rather than a silent omission.
- `--integration be|trap|gear` picks the transient method (default trapezoidal).

`docs/examples.md` has one worked, plotted example per analysis.

## What one `sim` call does inside

```
--model x.va ─► va-frontend (lex · parse · elaborate) ─► IR
                                                          │
                                        va-codegen (lower · AD) ─► ModelInstance
                                                                        │
deck.net ────► va-netlist (nodes · devices · analysis) ─────────────────┤
                                                                        ▼
                       va-core (MNA · Newton) / va-transient / va-acnoise ─► report
```

Elaboration happens on every run, in memory. Nothing is written to disk between the `.va`
and the result, so there is no stale-artefact problem and nothing to "rebuild" when a model
changes — edit the `.va`, run again.

### `check`: the standalone front-end diagnostic

```bash
cargo run -p va-cli -- check models/diode.va            # lexes, parses, elaborates
cargo run -p va-cli -- check external --codegen         # ... and lowers through va-codegen
```

`check` is the closest thing to a separate "elaborate" step, but it is a **diagnostic, not a
prerequisite**: it answers "does this model build?" for one file or a whole directory without
a deck, and reports the corpus figure the release log quotes. `sim` never depends on it having
been run.

### Refusals

If the model uses a construct the simulator recognises but deliberately declines, the run
stops with a `refused:` block saying what was refused, where, why an approximation would have
been wrong, what to write instead, and where the limitation is tracked. That is distinct from
a parse error ("your model is malformed") and from a convergence failure ("the numerics gave
up") — see CLAUDE.md §5.

## Every analysis, one line each

```bash
# DC operating point                     deck: .op
cargo run -p va-cli -- sim circuits/divider.net
# DC sweep                               deck: .dc V1 0 1 0.01
cargo run -p va-cli -- sim circuits/diode_iv_params.net --model models/diode.va --report V1
# Transient                              deck: .tran <tstep> <tstop>
cargo run -p va-cli -- sim circuits/rectifier.net --model models/diode.va --tran
# Small-signal AC                        deck: .ac dec <pts> <fstart> <fstop>  +  V1 ... AC 1
cargo run -p va-cli -- sim circuits/rc_ac.net --ac
# Noise                                  deck: .noise V(out) V1 dec <pts> <fstart> <fstop>
cargo run -p va-cli -- sim circuits/diode_noise.net --model models/diode.va --noise
```

Validation against QSPICE golden data is a separate command, `cargo xtask validate`, which
drives the same `va_cli::run_sim` library entry point over every deck in `circuits/` that has
a committed `golden/` file (`docs/validation.md`).

## What changes at 1.0

Everything above runs from source: `cargo run` builds the workspace on the machine it is on
and executes the fresh binary. That is the right shape while the language front end and the
analyses are still growing, because the person running a simulation is also the person
changing the simulator.

At **1.0 the deliverable becomes the executable.** The sources are compiled once from the
tagged release, and the resulting `va-cli` binary (`va-cli.exe` on Windows) is what users
run — no Rust toolchain, no checkout:

```bash
va-cli sim rectifier.net --model diode.va --tran
```

The workflow's three pieces (model, deck, one command) do not change; what changes is that
the command is a shipped program rather than `cargo run`. Two things follow, and both are 1.0
blockers recorded in `release.txt`'s "Road to 1.0":

- **The executable is tested on several different platforms** — different computers,
  different operating systems — not only the Windows development machine. The pure-Rust,
  no-native-link rule (CLAUDE.md §5, `deny.toml`) exists precisely so that this is a build
  matrix and not a porting effort, but "it should build anywhere" is not evidence; the
  release entry for 1.0 must list the machines and OSes the binary was actually run and
  validated on, with `cargo xtask validate`'s figures from each.
- **This document is rewritten for the binary**: install/unpack, invoke, and where the
  reference models ship, replacing the `cargo run -p va-cli --` prefix throughout.

Until then, every command in this file is the `cargo run` form, and that form is correct.
