# Future development — beyond 1.0

**What this document is.** The list of things this simulator could become after 1.0, kept
separately from `release.txt`'s "Road to 1.0" (which is what 1.0 *requires*) and from
`docs/roadmap.md` (which is what has been done and why). Nothing here is committed to; each
entry states what it would give a user, what it would cost, what it depends on, and what
decision has to be made first. An entry moves out of this file into the roadmap when work on it
starts, and into a release entry when it lands.

**How to read the priority.** Items are ordered by the project's own rule (CLAUDE.md §1):
correctness before breadth, a construct is done only when validated, and a wrong number that
looks right is the failure mode to resist. So "faster" comes after "handles bigger circuits
correctly", and "more analyses" comes after "the ones we have are trusted at scale".

---

## 1. Sparse linear solve

**What.** Replace the dense LU in `va-core::linsolve` with the sparse LU that already exists
beside it (`solve_sparse`, `faer`'s sparse module) as the production path, with a dense
fallback below a size threshold.

**Why.** The project's own benchmark (`cargo xtask bench-linsolve`, 2026-08-31) measured
sparse winning by 12–33× above ~20 unknowns. Dense LU is O(n³) per Newton iteration; a
200-node circuit with a few hundred auxiliary unknowns (branch currents, `idt` accumulators,
`laplace_*` states — every one of which is another row) is where a `.tran` stops being
interactive. The decision recorded on 2026-08-31 was "not yet; the trigger is circuit size, not
the calendar" — and the trigger is now measured (`docs/validation.md`, "The dense-LU
circuit-size limit", v0.9.16+1): a 10 000-point transient is interactive to ~200 unknowns,
minutes at 400, half an hour at 800, impractical beyond ~1 000.

**Cost and dependencies.** The solver exists and is tested for singular-matrix behaviour
(`sparse_singular_matrix_is_rejected`). What is missing: (a) the Jacobian is assembled dense
(`mna::System` is a `Vec<f64>` of `dim²`) — the sparse solve currently *re-derives* the
pattern from the dense matrix every call, which throws away most of the win; a sparse
assembly needs `StampSink` to accumulate triplets, which is a `va-abi` (Interface β) change
under §6; (b) symbolic factorization should be reused across Newton iterations and timesteps
(the pattern is fixed for a given circuit); (c) `CoreError::NonFinite`'s row/column reporting
must survive the format change. **Decide first:** the size threshold, and whether pattern reuse
is worth the API surface (it is, above ~100 unknowns).

## 2. Per-unit-axis plots and a proper results file

**What.** `--plot` with one y-axis per unit (V, A, K, W, …) or normalized traces with a legend
carrying each series' scale; and a machine-readable results file (CSV, or the `.qraw`-like
layout `xtask` already parses) so results can be post-processed without regex-scraping the
report.

**Why.** Found by the opto-thermal example (roadmap, 2026-09-11): five series in four units on
one axis put the watts and amps on the zero line. `--report` one unit at a time is the
workaround and the documented workflow. A results file is what every user of a real simulator
expects and what `va-harness` effectively re-implements internally.

**Cost.** Small. `va-cli` only. **Decide first:** the results format — the harness's golden
format already exists and is the obvious candidate.

## 3. `absdelay` in transient (proposal stage 2)

**What.** A real transport delay: a bounded `(t, value)` ring buffer per call site on the
state channel, linear interpolation to `t − τ`, the sub-timestep case carrying its
interpolation weight through AD, and `bound_step(τ/k)` so the integrator cannot step over a
delay. `docs/proposals/absdelay.md` §4 stage 2 is the design; it is the last transient
refusal in the analog-operator family after `laplace_*` landed (v0.9.16).

**Why.** Four of the five corpus uses are `L·n_g/c` — the group delay of an optical
waveguide. With the fold, light crosses a guide instantly. The photonic discipline this project
targets (CLAUDE.md §1) is not credible in transient without it.

**Cost and dependencies.** `va-codegen` only (the state channel and `bound_step` exist). The
memory question for long thermal delays (§2.2 of the proposal: a buffer of `τ/h` points at
`τ/h = 10⁶` is 16 MB per site) needs an answer — `maxdelay` sizing with an error, never a
silently shortened delay. **Decide first:** what exceeding the buffer does (the proposal says
error).

## 4. Optimization and parameter fitting

**What.** An outer loop that drives the simulator to minimize a cost: fit a model's
parameters to measured data (I–V, S-parameters, a transient), or tune a circuit's component
values to a specification. Gradient-free (Nelder–Mead, CMA-ES) first; then sensitivities.

**Why.** Compact-model extraction is the canonical use of a Verilog-A simulator, and it is
what makes the model zoo useful against real devices. It is also the analysis that most
rewards this codebase's particular strength: the models are *automatically differentiated*.

**Cost and dependencies.** The pipeline is already a library (`va_cli::run_sim` → `load` →
`solve_dc`/`solve_transient`), so the loop is a new crate (`va-fit`?) over that API. The real
prize needs one piece of infrastructure: **parameter sensitivities** `∂x/∂p` from the same AD
that gives `∂f/∂x` — a second gradient channel through `va-codegen`'s `Dual`, and the adjoint
machinery `va-acnoise` already has for noise is the same linear algebra. **Decide first:** the
cost-function interface (what a "measurement" is: a golden-like table), and whether the first
version is gradient-free (cheap, robust, slow) or sensitivity-based (a codegen change).

## 5. Monte Carlo and statistical simulation

**What.** Run a deck N times with parameters drawn from declared distributions; report the
distribution of any reported quantity. Global and mismatch variation; correlated draws.

**Why.** Yield and corner analysis are the second canonical use. Verilog-A itself has the
hooks: `$rdist_normal`, `$rdist_uniform`, … (LRM §9.13) are exactly this, and they are today a
recorded *refusal* because the engine has no random number generator ("Expired premises" in the
roadmap: a premise that has *not* expired).

**Cost and dependencies.** (a) A seeded, reproducible RNG in the analysis context (Interface β
addition, §6) so `$rdist_*` can be answered; (b) a netlist syntax for distributions on device
parameters (`R1 a b 1k dist=normal(0.05)` or a `.mc` card); (c) an outer loop that is
embarrassingly parallel across runs — the first thing in this project that would use more than
one core. Reproducibility (CLAUDE.md §11: "known seeds", deterministic metrics) is the
constraint that shapes it. **Decide first:** the distribution syntax, and that seeds are part of
the deck, not the command line.

## 6. Multi-core and GPU execution

**What.** Parallelism at three grains, in order of payoff per unit of risk:

1. **Across runs** — Monte Carlo, corners, parameter sweeps, the harness: independent
   simulations on independent threads. No shared state, no numerical change. `rayon` over the
   existing library API.
2. **Across devices within a load** — evaluate every `ModelInstance::load` in parallel and
   reduce the stamps. The `StampSink` accumulation is the reduction; per-thread sinks merged
   by row. Pays above a few hundred devices; changes nothing numerically if the merge order is
   fixed.
3. **GPU** — for the linear solve (batched dense LU on many small systems is where GPUs shine,
   i.e. Monte Carlo again) or for model evaluation (thousands of identical devices — a
   memory array — evaluated as one kernel). This is the only item on this page that conflicts
   with a house rule: CLAUDE.md §5's "pure Rust, no native-link deps" excludes CUDA/ROCm
   bindings outright. `wgpu` (pure-Rust WebGPU, compute shaders in WGSL) is the only
   admissible route, and it means a second implementation of every model in WGSL — either
   generated by `va-codegen` from the same IR (the honest way; codegen already emits a
   closure, emitting a kernel is the same lowering with a different target) or hand-written
   (not acceptable: two implementations of one model drift).

**Why.** (1) is free and needed by items 4–5. (2) is the standard route to bigger circuits
after sparse. (3) is a research topic more than a feature; it is listed so the constraint is
recorded before someone reaches for a CUDA crate.

**Decide first.** (1): nothing, do it with Monte Carlo. (2): the merge strategy and the
threshold. (3): whether `va-codegen` growing a second backend is a thesis topic — it is a
good one — and that `wgpu` is the only tool allowed.

## 7. Analyses this engine does not have

- **Periodic steady state (shooting / harmonic balance)** — the RF analysis; needed for
  oscillators and mixers beyond "run a long transient". Shooting reuses the transient
  integrator plus a Newton loop over the initial state, and is the cheaper of the two.
- **S-parameters** — AC with port normalization; small, mostly `va-cli`.
- **Sensitivity analysis** (`.sens`) — item 4's infrastructure exposed directly.
- **Pole-zero** — an eigenvalue problem on the linearized system; `faer` has the
  decomposition.
- **Noise with frequency-dependent `laplace_*`** — the one stated limitation left in the
  noise analysis: a filter evaluates to `H(0)` there. The state-space rows (v0.9.16) would fix
  it if noise assembly used them; it does not yet, because the noise gates were validated on
  the frequency-domain path and a switch needs its own gate.

## 8. Language completeness — the long tail

Tracked construct by construct in `docs/token-reference.md`; the roadmap's "Language
coverage" backlog lists what is open. The items with a known user: array-variable arguments to
`laplace_*`/`zi_*` (needs constant propagation through straight-line assignments — `ctle.va`);
`last_crossing`, `absdelta`, and compound event triggers mixing step and scheduled events;
`I(<port>)` inside `case`/loops; vector ports in `I(<port>)`; an empty array literal `{}` as a
`laplace_zp` zero list (found 2026-09-11 — parses as an error today; check the LRM's grammar
for whether it is legal before implementing it). (The `zi_*` Z-domain family, listed here on
2026-09-11 as needing a clock, was implemented on 2026-09-12 — v0.9.20 — the clock being the
filter's own `T` and a breakpoint per sample instant.)

## 9. Netlist and usability

- SPICE `.subckt`/`.ends`, `.include`, `.param`/expressions, `.model` cards (parameters go
  on the device line today), `I` current sources, `PWL`/`EXP` sources, `.ic`/`UIC`, multi-source
  and nested `.dc` sweeps, `.step`.
- A `--json` report for tooling; `--plot` improvements (item 2).
- A Python binding (`pyo3` is pure Rust and admissible) so the simulator can be driven from
  notebooks — the way most users of items 4–5 would actually use them.
- Error messages that quote the model source line for every refusal and `NonFinite` (the
  latter names the unknown today, not the contribution).

## 10. Platform and distribution

- Release archives for Linux arm64 and Windows arm64 (the workflow builds three targets today).
- A `.gitattributes` fixing line endings (release archives are cut from a Windows checkout).
- Package-manager distribution (`cargo install`, a Homebrew tap, winget) once 1.0 is tagged.

---

## Not planned, and why

- **Verilog-AMS mixed-signal** (`connectmodule`, discrete events, `wreal`) — excluded by
  CLAUDE.md §1: Annex C already excludes it from Verilog-A, and the scope is the language, not
  the superset.
- **A schematic front end / GUI** — the deliverable is the executable driven by a deck
  (`docs/workflow.md`). Plotting is as far as the CLI goes; a GUI belongs in a separate
  project that shells out to it.
- **FFI to an existing sparse/BLAS library for speed** — forbidden by §5, and the reason is
  reproducible builds on every platform, which CI now enforces.
