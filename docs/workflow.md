# Workflow — from a Verilog-A model to a simulation result

Two ways to run the same program: from a **release archive** (the 1.0 deliverable — see
"Running the release archive" below) or from source with `cargo run` (the developer path,
used for the walk-through). The three pieces a simulation needs are the same either way.

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

## Running the release archive (the 1.0 workflow)

Since `v1.0.0-rc1` (2026-09-12) the deliverable is a compiled executable. Each release on
https://github.com/philippevelha/verilog-a-sim/releases carries one archive per platform:

| Archive | Platform |
|---|---|
| `va-cli-<tag>-x86_64-unknown-linux-gnu.tar.gz` | Linux, x86-64 |
| `va-cli-<tag>-x86_64-pc-windows-msvc.zip` | Windows, x86-64 |
| `va-cli-<tag>-aarch64-apple-darwin.tar.gz` | macOS, Apple silicon |

Each unpacks to one directory holding `va-cli` (`va-cli.exe` on Windows), `models/` (the
model zoo, including `disciplines.vams`, `constants.vams`, `mechanical.vams`, `photonic.vams`),
`circuits/` (every deck in this repository), `workflow.md` (this file), `README.md`,
`LICENSE` and `release.txt`. No Rust toolchain, no checkout, nothing to install:

```bash
tar xzf va-cli-v1.0.0-rc1-x86_64-unknown-linux-gnu.tar.gz     # or unzip the .zip on Windows
cd va-cli-v1.0.0-rc1-x86_64-unknown-linux-gnu
./va-cli sim circuits/rectifier.net --model models/diode.va --tran
./va-cli sim circuits/microring_thermal.net --model models --tran --report drop
./va-cli sim circuits/laplace_step.net --model models/laplace_lowpass.va --tran --report out
```

(On macOS the first run may need `xattr -d com.apple.quarantine va-cli`, since the binary is
not notarised. On Windows, `.\va-cli.exe`.) Everything in the sections above applies with
`./va-cli` in place of `cargo run -p va-cli --`: the same three pieces — a `.va` component, a
`.net` deck as the testbench, one `sim` call — and the same flags.

### What "it works on this machine" means

A release candidate is tested on real machines, not only CI runners, and the result is
recorded in `release.txt`'s entry for the release. The test is the three commands above, and
"pass" is:

1. `rectifier.net`: `Transient analysis (718 points, …)`, and the line at
   `t=2.536873e-4s` reads `V(out)=4.304467 V` — the same numbers the source build produces
   on the development machine (the adaptive timestep sequence is deterministic, so a
   different point count means a different floating-point environment, which is itself a
   finding to report).
2. `microring_thermal.net`: 2015 points, `Popt(drop)` peaking at `7.87e-4 W` four times.
3. `laplace_step.net`: `V(out)` at `t ≈ 1.0e-3 s` is `1 − e^{−1} = 0.632…`, matching the
   QSPICE golden the repository carries to `5e-6`.

Report the machine (CPU, OS and version), the archive name, and the three outcomes. The
2026-09-12 entry for rc1 has the first such record (Windows 11, the development machine,
from the archive rather than the checkout).

## Developing: the `cargo run` path

Everything above the archive section runs from source: `cargo run` builds the workspace on
the machine it is on and executes the fresh binary. That is the right shape when the person
running a simulation is also the person changing the simulator, and it is how every command
in this file was validated. The two forms are the same program; only the prefix differs.

### Toolchain note for developers

`rust-toolchain.toml` pins `1.92.0` as a bare version so rustup picks each host's own target.
On a Windows machine where an MSYS/MinGW install puts its own `link`/`dlltool` ahead of
MSVC's on `PATH` (the symptom is `link: extra operand` from a build script), use the GNU
toolchain and keep that choice out of the repo with a directory override:

```bash
rustup toolchain install 1.92.0-x86_64-pc-windows-gnu
rustup override set 1.92.0-x86_64-pc-windows-gnu      # inside the checkout
```

`.cargo/config.toml` already forces the GNU toolchain's self-contained linker for that target.
CI (`.github/workflows/ci.yml`) runs the full gate — fmt, clippy, tests, `xtask validate`,
`cargo deny` — on Windows (MSVC), Linux and macOS on every push.
