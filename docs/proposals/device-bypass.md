# Proposal: device bypass — reuse a device's stamps while its terminals have not moved

**Status:** prototype, 2026-10-01, on the branch `proto/device-bypass`. Nothing is ratified.
**Affects:** `va-abi` (**Interface β**: one new default-`false` method,
`ModelInstance::is_pure_in_x`, a CLAUDE.md §6 change), `va-codegen` (an analysis that answers it),
`va-core` (`par::Bypass`, `par::load_all_bypass`), `va-transient` (uses it in transient
assembly). `va-ir` is untouched.
**Motivation:** the transistor-level SNN (`circuits/snn/transistor/`). One SG13G2 PSP103 load
costs about 233 µs here. In a spiking network most word lines are low most of the time, so most
access transistors sit at a constant bias for many timesteps, and re-evaluating them every Newton
iteration is wasted work. This is SPICE's `BYPASS`.
**Evidence:** release build, i7-1185G7 (4 cores / 8 threads), Windows 11, mains power, one run
per configuration. This laptop's wall times vary by up to ~15 % from run to run, so read the
timings as ranges.

## 1. What it does

- **Who may be bypassed.** An instance is eligible only if its `load` is a **pure function of
  `x`**: the same stamps at the same `x` whatever the time, step, phase, `gmin` or history. That
  is `ModelInstance::is_pure_in_x`, default `false`. A compiled model answers `true` when the
  lowering proves it, i.e. when the model has none of the following:
  - a context read (`$abstime`, `analysis()`, `$simparam`, initial/final step);
  - an event, or `$bound_step`;
  - `idt`, `transition`, `slew`, `absdelay`, `laplace_*` or `zi_*`;
  - a persistent variable;
  - a `ddt` evaluated **inside an expression**.

  A whole charge term (`I <+ ddt(q(x))`) is pure: `q(x)` goes to the charge channel and the
  integrator supplies d/dt. Every `ddt` site gets state slots either way, so `state_len` cannot
  tell the two apart; a new walk over the lowered statements (`Lowered::evaluates_ddt`) does.
  Noise sources are `0.0` in `load` and do not count. A bias-dependent coefficient times `ddt`
  (`V·ddt(q)`) is *not* pure: its rate is reconstructed from history, and the analysis says so.
  The reference models keep the default, `false`.
- **How.** Each eligible instance keeps its last recorded stamps and the `x` they were taken at.
  If every one of its unknowns satisfies `|x − x0| ≤ abstol + reltol·max(|x|, |x0|)`, the
  recording is replayed **with a first-order correction**: a Jacobian entry `(r, c, g)` adds
  `g·(x[c] − x0[c])` to residual row `r`, and a `dcharge` entry adds the same to charge row `r`.
  The device is then linearised at `x0` rather than frozen there, so the error left is second
  order in the drift. `x0` is not advanced while an instance is bypassed, so the drift stays
  bounded; past tolerance the instance is evaluated again and `x0` is reset.
- **Deterministic.** Whether an instance is bypassed depends only on `x` and the cache, never on
  timing or thread count, and stamps reach the sink in instance order. The result is the same on
  every thread count, but not bit-identical to a run without bypass.
- **Switched on by `VA_BYPASS=on`** (tolerances `VA_BYPASS_RELTOL`, default 1e-6, and
  `VA_BYPASS_ABSTOL`, default 1e-9 V), in transient only. Prints one line of statistics at the end
  of the run. Read from the environment because this is a prototype; a kept version would be a
  `TranConfig` field set by a `va-cli` flag, as `btf` is.

## 2. Results

| Deck | Eligible / instances | Loads replayed | Plain | Bypass | Worst difference at shared timepoints |
|---|---|---|---|---|---|
| `fig5_access.net` (8 SG13G2 PSP103) | 35 / 74 | 86.6 % | 56–61 s | **15.7–16.3 s** | 1e-6 V (the output's print precision) |
| same, 1 thread | | | 116 s | **19.2 s** | |
| ISCAS c17 transient (24 PSP103) | 24 / 81 | 70.5 % | 84.6 s | **37.5 s** | 1e-10 |
| PSP103 inverter | 2 / 7 | 53.3 % | 2.0 s | 1.5 s | 1e-11 |
| `fig5_neuron.net` (ideal switches only) | 35 / 74 | 89.0 % | 4.1 s | 4.5 s (**slower**) | 7e-17 |

On the Fig. 5 decks the timepoint counts are unchanged (13 645 and 10 707), and so are the output
spikes, steps 6, 10, 14. The V_m sampled at the end of each high phase is identical to the
6 digits printed. In `fig5_access` the two runs differ in where about 800 of 13 645 timepoints
land near clock edges. That is why naive interpolated waveform differences show up to 27 mV at
the edges; at the timepoints both runs share, the difference is at print precision.

**Looser tolerances do not help** on `fig5_access`: reltol 1e-4 gave 18.3 s and the SPICE-like
1e-3 / 1 µV gave 17.6 s (replay rate 90.6 % and 91.9 %), against 16.3 s at the default. They were
also less exact (1.4e-4 V on the clock source at shared timepoints with 1e-3). The default stays
tight.

## 3. What is left, and what it says about the next step

Profiled serially with bypass on, `fig5_access` spends 16.6 s in device loads. **13.4 s of that
is the eight PSP103s' real evaluations**: about 7 000 per device (out of 59 000 loads),
233 µs each, mostly while their word line is high or switching. The linear solve is 0.5 s.
So bypass has removed the idle cost, and what remains is **the cost of one PSP103 evaluation
itself**. 233 µs is roughly 20–40× what a natively compiled PSP103 takes (OpenVAF-class). That is
the next lever, and it is a separate piece of work: profile the evaluator on PSP103, then decide
between tuning the tape interpreter and compiling to native code.

For the MNIST network the gain from bypass should grow with sparsity. A synapse's access
transistor is re-evaluated only when its word line (a presynaptic spike) or its cell node moves,
and the source lines are held at V_ref. This is a reasoned expectation, not a measurement.

## 4. Limitations, stated

- **A §6 interface change** (`is_pure_in_x`): needs ratification before merging.
- **Slower on cheap models** (Fig. 5 ideal switches: 4.1 → 4.5 s). The cache bookkeeping and the
  recording cost more than a cheap load. A kept version should bypass only instances whose
  measured evaluation cost exceeds the replay's, the same way `par::load_all` already decides
  when to go parallel.
- **Transient only.** DC operating points and sweeps, AC and noise do not use it.
- **Memory:** one stamp recording kept per eligible instance (PSP103: a few hundred stamps).
- **One tolerance pair for every unknown**, whatever its nature (voltage, current, temperature).
  This is fine for these decks and wrong in principle for a multi-physics circuit; a kept version
  should take the nature's `abstol`, as the Newton check does.
- An instance whose Jacobian names a row of `x` outside its own `unknowns()` is never bypassed
  (ground columns are fine).

## 5. Tests (on the branch)

- `va-core`: `bypass_replays_exactly_corrects_to_first_order_and_reevaluates_past_tolerance`. A
  replay at the recorded `x` is exact; within tolerance the residual error is under 1 % of the
  first-order change (without the correction it would be 100 %); past tolerance it re-evaluates;
  a non-pure instance is never bypassed.
- `va-cli`: `a_pure_in_x_model_stamps_the_same_at_the_same_x_whatever_the_context`. Tests the
  property rather than the flag: every module that claims purity stamps identically at the same
  `x` under a different time, step coefficients and committed state. `$abstime`, a `ddt` inside
  `exp`, `idt` and `V·ddt(q)` must not claim it.
- **No-op proof:** without `VA_BYPASS` the branch is bit-identical to `main` on `xtask validate`
  (28/28), Fig. 4, Fig. 5, `fig5_access`, the PSP103 inverter and c17_tran, with both binaries
  confirmed and every run exiting 0.
