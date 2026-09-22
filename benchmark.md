# Benchmarks

What this simulator costs, measured rather than estimated, on the CMC standard compact models.

Everything here was measured on **2026-09-22** on one machine (i7-1185G7, Windows 11, `--release`),
against the vendor `vacode/` distributions under `external/` — which is gitignored, so the raw
inputs are not in this repository. Figures are reproducible with the commands given; they are not
carried forward between versions.

**Read the methodology section before quoting any number.** Several of these comparisons are not
like-for-like, and the ways they are not are as important as the figures.

---

## 1. Model evaluation — `ModelInstance::load()`

One `load()` is one residual + Jacobian evaluation: what every Newton iteration of every timepoint
pays for. On a real compact model it is three orders of magnitude larger than the linear solve it
stamps into, which is why `xtask bench-scale` (the solver, on an RC ladder of reference primitives)
measures the wrong half.

```
cargo run --release -p xtask -- bench-model
```

`before` is v1.1.1, `after` is v1.2.0. `stmts`/`setup` are the module's top-level lowered
statements and how many of them are the bias-independent prefix (§3).

| model | exprs | unknowns | stmts | setup | before | after | |
|---|---|---|---|---|---|---|---|
| BSIM4 | 19109 | 18 | 1949 | 1556 | 1215 µs | **131 µs** | 9.3× |
| BSIM-SOI | 16301 | 23 | 1216 | 700 | 1045 µs | **159 µs** | 6.6× |
| PSP103 | 40807 | 21 | 1220 | 757 | 987 µs | **161 µs** | 6.1× |
| BSIM-BULK 107 | 19070 | 24 | 932 | 282 | 1067 µs | **212 µs** | 5.0× |
| HICUM/L2 v3.0 | 7408 | 22 | 191 | **1** | 271 µs | **61 µs** | 4.4× |
| JUNCAP200 | 12932 | 3 | 215 | 194 | 200 µs | **49 µs** | 4.1× |
| EKV2.6 | 2490 | 4 | 268 | 47 | 278 µs | **81 µs** | 3.4× |

Broken out on BSIM4, each change measured on its own:

| | µs | cumulative |
|---|---|---|
| v1.1.1 baseline | 1215 | — |
| `Grad::Zero` — a gradient channel that can be nothing | 334 | 3.6× |
| `Ctx::vars` indexed (`Vec<Option<Dual>>`) rather than hashed | 206 | 5.9× |
| setup/eval split (`Lowered::static_prefix`) | 131 | 9.3× |

### Why there was so much to win

86.7% of the `Dual`s one BSIM4 `load()` builds depend on **no unknown at all** — 7816 of 9020.
(BSIM-SOI 84%, PSP103 80%.) A compact model is mostly parameter range checks, temperature scaling,
geometry and corner binning. Every one of those values had been allocating two `Vec<f64>` of length
`n_unknowns` to carry a gradient that was identically zero, on every Newton iteration, for the whole
of a simulation. The bottleneck was the representation, not the tree walk — which is why a JIT would
have been the wrong first move: it would have compiled the same waste faster.

### The limit of the setup/eval split

It hoists a **prefix** and reorders nothing, because a bias-independent statement sitting after a
bias-dependent one may read what that statement wrote. HICUM/L2 v3.0's setup is not a prefix — it
hoists 1 statement of 191 and gains nothing from that change. Widening this needs real dependence
analysis and is not done.

---

## 2. Model preparation (frontend + codegen)

Best of 5, wall clock, minus 79 ms of process startup. This is `va-cli check <model> --codegen`:
lex → preprocess → parse → elaborate → `build_instance`.

| | PSP103 | BSIM4 | EKV2.6 | JUNCAP200 | HICUM/L2v3 | BSIM-SOI | BSIM-BULK107 |
|---|---|---|---|---|---|---|---|
| **this simulator** | **0.049** | **0.029** | **0.005** | **0.020** | **0.013** | **0.028** | **0.028** |
| OpenVAF | 3.48 | 6.7 | 0.23 | 0.61 | 0.72 | 2.1 | 2.9 |
| Xyce ADMS | 109 | 25.1 | 9.6* | 16.6 | 22* | 102.1* | –* |
| ADS | 33.9 | 27.0 | 2.5 | 5.1 | 7.7 | 30.7 | 34.5 |
| Spectre | 27.4 | – | 6.1 | 11.4 | 19.7 | 16.9 | 19.1 |

*(seconds; the four comparison rows are published figures supplied by the user, not measured here,
and were taken on other hardware.)*

**This row is not a like-for-like win and should not be quoted as one.** OpenVAF emits optimised
machine code through LLVM; this project builds a tree-walking AD evaluator. The compile-time lead is
real and so is the bill for it — see §1, where one BSIM4 evaluation costs 131 µs against a compiled
model's single-digit microseconds. The fair summary is: *model preparation is ~100× cheaper, model
evaluation is ~1000× dearer.* Closing the second half is the open work (§7).

---

## 3. DC sweeps, end to end

`va-cli sim <deck>` including parse, compile, build, solve and output.

### BSIM4 Id–Vg, 1001 points

`.dc VG 0 1 0.001` on a common-source stage, no model card.

| version | time | what changed |
|---|---|---|
| v1.1.1 | 30.0 s | — |
| v1.2.0 | 10.7 s | evaluator (§1) |
| v1.2.1 | 3.43 s | build instances once per sweep, not once per point |
| **v1.2.2** | **0.63 s** | continue Newton from the previous point |

**48× overall, with all 1001 output lines identical throughout.** The v1.1.1 and v1.2.0 figures are
single measurements; v1.2.1 and v1.2.2 are best of three (see §6).

### HICUM/L2 v3.0 output family, 1206 points

The card in §4. Best of three.

| version | time | net of six process launches |
|---|---|---|
| v1.2.1 | 2811 ms | 2337 ms |
| **v1.2.2** | **1050 ms** | **576 ms** |

Roughly 10 s is quoted for other simulators on the same card. Read §4's caveats before comparing.

### Where the time goes now

About 0.6 ms per BSIM4 sweep point, of which one model evaluation is 0.13 ms. The remaining
per-*run* fixed cost is that `quantities()`, `branch_currents()` and `sizing()` each call
`build_instances` independently, so one `sim` builds the deck three or four times before it solves
anything — which is why a single-point `.op` on BSIM4 still costs ~48 ms (of which ~16 ms is the
frontend).

---

## 4. The HICUM/L2 v3.0 output-family card

The reference deck (ngspice + OSDI):

```
VB B  0 DC 0.1 AC 1 SIN (0.5 0.4 1M)
VC C  0 DC 1
.model npn_full_sh hicuml2va
.include model.l
N1 C B 0 0 npn_full_sh
.control
pre_osdi hicumL2V3p0p0.osdi
dc VC 0 2 0.01 VB 0.65 0.9 0.05
plot -i(VC)
.endc
.end
```

All 1206 points converge here. `-i(VC)`, with the `.va` file's **default** parameters:

| VC | VB = 0.65 | VB = 0.75 | VB = 0.85 | VB = 0.90 |
|---|---|---|---|---|
| 0.0 | −8.2045e−6 | −3.9186e−4 | −1.8716e−2 | −1.2935e−1 |
| 0.1 | 7.8508e−6 | 3.7497e−4 | 1.7909e−2 | 1.2377e−1 |
| 0.5 | 8.1941e−6 | 3.9137e−4 | 1.8693e−2 | 1.2919e−1 |
| 1.0 | 8.1941e−6 | 3.9137e−4 | 1.8693e−2 | 1.2919e−1 |
| 1.5 | 8.1941e−6 | 3.9137e−4 | 1.8693e−2 | 1.2919e−1 |
| 2.0 | 8.1941e−6 | 3.9137e−4 | 1.8693e−2 | 1.2919e−1 |

Saturation below ~0.05 V, flat forward-active above, ×48 per 0.1 V of VB until 0.90 rolls off to
×6.9 on high injection and series resistance.

### Five ways this run differs from the reference deck

1. **No `model.l`.** It is not in the tree, so these are the `.va`'s default parameters. **The
   currents above are not the reference's currents.** The timing comparison additionally assumes
   the default card converges comparably to a real one, which is unverified.
2. **Nested `.dc` is not supported.** `DcSweep` names one source, so this was run as the six inner
   sweeps the card expands to (6 × 201 = 1206 points) — same work, six process launches instead of
   one, which is why §3 also reports the figure net of launches.
3. **`tnode` is terminated to ground.** `N1 C B 0 0` connects four nodes; the module declares five
   (`c b e s tnode`) and this project's `X` element requires every port. See §5 — with the default
   card this makes no difference whatsoever.
4. **`AC 1` and `SIN(0.5 0.4 1M)` dropped from VB.** Irrelevant to a `.dc`, but this netlist parser
   would otherwise build a waveform source and ignore the DC value entirely.
5. **Continuation is new here (v1.2.2).** Before it, every sweep point was a cold Newton start — so
   the earlier HICUM figures were achieved *without* an advantage the reference almost certainly
   had.

---

## 5. Thermal-node termination (`tnode`)

Since this project cannot leave a declared port unconnected, `tnode` is tied to ground through a
resistor. How much does that choice cost? Measured at VB = 0.90 V, VC = 2 V, across four decades.

**With the default card (`flsh = 0`, `rth = 0`) it costs nothing at all.** HICUM only drives the
thermal node when self-heating is enabled, so every value from 1 mΩ to 10 Ω gives an identical
answer to all seven printed digits — which is also what leaving the port unconnected would give.

| RT | I(VC) @ 2 V | ΔT @ 2 V |
|---|---|---|
| 1 mΩ … 10 Ω | −1.291858e−1 A (identical) | 0.000000 |

**With self-heating on it matters a great deal.** `flsh=1 rth=300` (`rth` invented — there is no
`model.l`):

| RT | ΔT @ 2 V | I(VC) @ 2 V | P |
|---|---|---|---|
| 1 mΩ | 2.5837e−4 K | −1.291872e−1 A | 0.2584 W |
| 0.01 Ω | 0.002584 K | −1.292005e−1 A | 0.2584 W |
| 0.1 Ω | 0.025858 K | −1.293331e−1 A | 0.2587 W |
| 1 Ω | 0.260484 K | −1.306762e−1 A | 0.2614 W |
| 10 Ω | 2.829469 K | −1.461892e−1 A | 0.2924 W |

The effect scales with dissipated power, as it must: at VB = 0.75 V (0.8 mW) the same 10 Ω gives
only 0.0076 K and a 0.05% shift in current, against 2.83 K and +13% at VB = 0.90 V (0.26 W).

ΔT/P at RT = 10 Ω works out to 9.68 K/W, which is 10 ∥ 300 — **the external resistor is a thermal
resistance to ambient in parallel with the model's own internal `rth`, not a replacement for it.**
Worth knowing before wiring up a real card.

---

## 6. Methodology, and what these numbers are not

- **Best of N, stated per figure.** This machine's run-to-run spread on the sweep benchmarks is
  20–40%; one measurement of a 600 ms run is worth very little. `bench-model` warms up 20 calls and
  reports the best of five batches of fifty. Where a figure is a single measurement it says so.
- **Before/after is measured, not asserted.** Every "before" figure in §1 and §3 comes from building
  the pristine tree — `git checkout` of the changed files, full rebuild, same tool, same decks — not
  from a number remembered from an earlier run. A stash that silently breaks the build produces an
  empty before-file, so line counts are checked.
- **Process startup is 79 ms** on this machine and is subtracted where stated.
- **"Identical" means the printed digits** (6–7 significant figures) unless stated otherwise. Two
  claims in this document are bit-level, and both come from a test comparing raw `f64` solution
  vectors with `==` rather than from printed output.
- **Correctness evidence for every optimisation here:** `cargo xtask validate` is 28/28 and its
  error figures were diffed line by line against a run of the pristine tree at each step — all
  identical. Seven `.op` runs across the models in §1 produce 116 reported values, all identical
  before and after. The 1001-point BSIM4 sweep and the 1206-point HICUM sweep are identical
  line for line.
- **No model cards.** Every model here runs on its `.va` file's default parameters. Default
  parameters are not a realistic device, and both the currents and the convergence behaviour of a
  real card will differ.
- **`external/` is gitignored**, so none of these inputs are in the repository and the tables cannot
  be regenerated from a clean clone alone.

---

## 7. Open, in the order worth attacking

1. **Nothing is compiled to machine code.** The evaluator is still a tree walk over an expression
   arena; the remaining per-`load` cost is recursive dispatch, `Result` plumbing and arena
   indirection on the bias-dependent core. A flat SSA tape would remove most of it; a Cranelift JIT
   (pure Rust, so `CLAUDE.md` §5's no-native-link rule is satisfied, but `unsafe` is needed to call
   what it emits and so is owner sign-off) would go further.
2. **The setup/eval split is a prefix.** Real dependence analysis would reach models like
   HICUM/L2 v3.0, which currently hoists 1 statement of 191.
3. **`build_instances` is called three or four times per `sim` run** — by `quantities()`,
   `branch_currents()`, `sizing()` and the solve. Fixed cost, not per-point, but it is most of a
   single `.op`'s time on a large model.
4. **Nested `.dc`** (`dc VC 0 2 0.01 VB 0.65 0.9 0.05`) is unsupported; `DcSweep` names one source.
5. **The sweep's cold-start fallback is untested.** Continuation retries a failed point from the
   origin, so convergence can only improve — but constructing a circuit where a warm start fails and
   a cold one succeeds is a research question, not a fixture, and no test exercises that path.

*Closed since this file was written:* a capacitor pinned by a fast-slewing source underflowed the
timestep controller, which is what blocked the PSP103 inverter transient — fixed in v1.2.5 by
letting a row with no integrated state lose its veto at the floor, and only there. Its
reproduction is `circuits/benchmark/cap_fast_edge.net`, which contains no Verilog-A. Also, a
transient started from the zero vector unconditionally,
which made every CMC MOSFET unrunnable — at `x = 0` a compact model's charge is inconsistent and
shrinking the timestep makes the first step's current *larger*. Fixed in v1.3.0 by taking SPICE's
default (operating point first, `UIC` to opt out); BSIM4, BSIM-BULK 107, BSIM-SOI, PSP103 and
EKV2.6 all integrate now, and no golden needed regenerating. Also `@(above)` in a swept deck fired on "already positive" at
every point rather than on a crossing from the point before, because `solve_dc_sweep` passed `None`
where `dc::operating_point_with_events` wanted the previous point's site values. Fixed in v1.2.3;
it is the only change in this series that moves results, and the only one `xtask validate` could
not have caught, since no zoo deck puts an `above` in a `.dc` sweep.
