# Proposal: `zi_nd` / `zi_np` / `zi_zd` / `zi_zp` — Z-domain filters, implemented

**Status: proposed 2026-09-12, a 1.0 blocker by supervisor decision** ("the fix of zi_* should
be done before release" — refusing, which v0.9.17 does, is the honest interim, not the
answer). Source: Verilog-AMS LRM 2.4 §4.5.12 (read from `references/VAMS-LRM-2-4.pdf`).

**Affects:** `va-ir` (**Interface α, additive** — four `Builtin` variants; CLAUDE.md §6
applies, and this document is the written description step 1 requires), `va-frontend`,
`va-codegen`. `va-abi` (Interface β), `va-core`, `va-transient`, `va-acnoise` are **not**
touched: the state channel, breakpoints and `bound_step` the implementation needs all exist.
**Depends on** `` `default_transition `` from `docs/proposals/directives.md` (§3 below).

## 1. What the LRM says the construct is

```
zi_nd ( expr , n , d , T [ , τ [ , t0 ] ] )      H(z) = Σ n_k z^-k / Σ d_k z^-k
zi_np ( expr , n , ρ , T [ , τ [ , t0 ] ] )      H(z) = Σ n_k z^-k / Π (1 − z^-1 ρ_k)
zi_zd ( expr , ζ , d , T [ , τ [ , t0 ] ] )      H(z) = Π (1 − z^-1 ζ_k) / Σ d_k z^-k
zi_zp ( expr , ζ , ρ , T [ , τ [ , t0 ] ] )      H(z) = Π (1 − z^-1 ζ_k) / Π (1 − z^-1 ρ_k)
```

- `T` — the sampling period, mandatory, positive. The input is sampled every `T` seconds
  starting at `t0` (default 0), and the output changes only at those instants: "a filter with
  unity transfer function acts like a simple sample-and-hold which samples every T seconds and
  exhibits no delay."
- `τ` — the transition time of the output between samples, optional, non-negative. Non-zero:
  the timestep is controlled to resolve both corners of the transition. Omitted: "one unit of
  time as defined by the `` `default_transition `` directive", timestep not controlled for the
  trailing corner. Zero: the output is abruptly discontinuous, and such a filter "shall not be
  directly assigned to a branch".
- Roots come as `(re, im)` pairs; a complex root's conjugate must be present; a root at zero
  contributes a factor `z` rather than `(1 − z^-1·0)`. The zeros argument may be a null
  argument (`,,`) — a grammar this parser does not have (`frequency-domain.md` §7), and which
  stays out of scope here: a filter with no zeros is written in `zi_nd`/`zi_np` form with the
  numerator `{1}`, which is the same transfer function.

In the time domain, with `x_k = expr(t0 + kT)`:

```
d_0·y_k = Σ_{i=0..M−1} n_i·x_{k−i} − Σ_{j=1..N−1} d_j·y_{k−j}
```

## 2. Design

### 2.1 Interface α — four additive builtins

```rust
pub enum Builtin {
    // ...
    /// `zi_nd(value, num, den, T [, tt [, t0]])` — LRM §4.5.12. Argument layout, flattened:
    /// `[value, T, tt, t0, Const(num_len), num…, den…]`, where an omitted `tt` is the
    /// constant `-1.0` ("use the default transition time") and an omitted `t0` is `0.0`.
    /// The `Const(num_len)` separator is the `LaplaceNd` trick.
    ZiNd, ZiNp, ZiZd, ZiZp,
}
```

Precedent: `NoiseTable`/`NoiseTableLog`, `Absdelay` — additive variants, `docs/interfaces.md`
updated in the same PR, `va-codegen` handling them (it must anyway, being the implementer).

### 2.2 Frontend

The refusal arm in `elaborate.rs` (v0.9.17) becomes a lowering arm: `value` lowered; `T`,
`tt`, `t0` lowered as expressions (parameter expressions, like Laplace coefficients); the
coefficient/root lists as `array_lit_values`, exactly as the Laplace forms do. `tt` omitted →
`Const(-1.0)`; codegen resolves `-1.0` against the module's `default_transition` value, which
elaboration records on `va_ir::Module` from the directive (a new `Module::default_transition:
Option<f64>` field — also additive). Inside a runtime loop: rejected, as every other analog
operator is (v0.9.14). The `,,` null argument: not parsed, error naming the workaround.

### 2.3 Codegen — the three analyses

**DC.** The steady-state gain `H(1)` (every `z^-k` is 1, so a coefficient list sums; a root
list gives `Π(1 − r_k)`, `z` at a zero root gives 1). This is exactly the fold v0.9.16 and
earlier did at elaboration, moved to load time where the coefficients are parameter values.
Stamped like a resistive gain: `y = H(1)·u`, Jacobian `H(1)·u.grad`.

**AC.** `H(e^{jωT})` per frequency point, stamped as `G = Re(H)`, `C = Im(H)/ω` through the
existing `stamp_laplace` path — a `zi_at(omega, T, …)` beside `ad::laplace_at`, same root
convention (`z` for a zero root). This is the response of the discrete filter to a sampled
sinusoid; it does **not** include a zero-order-hold `sinc` factor, and the entry says so.
Above the Nyquist frequency `π/T` the response aliases exactly as the mathematics does.
Like the Laplace family, a top-level additive term of a contribution only
(`lower::buried_frequency_domain_call` grows the four variants).

**Transient.** A sampled difference equation on the state channel — the `transition`/`slew`
mechanism, not the `laplace_*` state-space one, because a sampled system is *not* an ODE. Per
call site, state slots:

| slots | contents |
|---|---|
| `M` | past inputs `x_{k−1} … x_{k−M+1}` (and `x_k` once sampled) |
| `N − 1` | past outputs `y_{k−1} … y_{k−N+1}` |
| 1 | `t_next`, the next sample instant |
| 2 | `y_held`, `y_prev` — the current output target and the one before it (for the ramp) |
| 1 | `t_sample`, when `y_held` was set (the ramp's start) |

Each `load()` in transient:

1. **Breakpoint.** Request `t_next` via `StampSink::breakpoint` (the mechanism `@(timer)`
   already uses), so the integrator lands *on* the sample instant rather than stepping over
   it. With `tt > 0`, also request `t_sample + tt` (the trailing corner) and `bound_step` the
   ramp as `transition` does.
2. **Sample.** If `t ≥ t_next` (i.e. this evaluation is at the sample instant the breakpoint
   put us on): read `u = expr` from the *current* iterate, shift the input history, compute
   `y_k` from the difference equation, shift the output history, set `y_prev = y_held`,
   `y_held = y_k`, `t_sample = t`, `t_next += T`. All of this into `state_next` — committed
   only if the timepoint is accepted (§ `va_abi::state`), so a rejected step re-samples.
3. **Output.** `tt == 0`: `y = y_held`. `tt > 0`: `y = y_prev + (y_held − y_prev)·clamp((t −
   t_sample)/tt, 0, 1)`.
4. **Jacobian.** The output depends on the current iterate `x` only through `u` at a sampling
   evaluation, and then only in the `tt == 0` case (with `tt > 0` the ramp fraction is 0 at
   `t = t_sample`, so `y = y_prev` there and the sampled value first shows at the next
   point, by which time it is committed state). So: at a sampling load with `tt == 0`, stamp
   `(n_0/d_0)·u.grad`; otherwise nothing. Exact, not an approximation.

**Default transition time.** `tt` omitted and no `` `default_transition `` in force: the LRM
says "controlled by the simulator". This simulator's choice: **`tt = 0`**, with breakpoints
making the step exact. The LRM's "shall not be directly assigned to a branch" is aimed at
simulators without breakpoints, for which an abrupt branch value is a convergence hazard;
here it is not, so the assignment is allowed and the entry states why. (`transition()` gets the
same default from the same directive; today it defaults to `0.0` as well — consistent.)

**Noise.** `H(1)`, the same stated limitation `laplace_*` carries.

### 2.4 What is refused

Roots not in conjugate pairs (the AC/DC root products would be complex); `T ≤ 0`; `tt < 0`;
a null `,,` argument (grammar); `d_0 = 0` (the difference equation cannot be solved for
`y_k`); a call inside a runtime loop.

## 3. Dependency on `` `default_transition ``

The directive proposal's item 3. Implementable before it: `tt` omitted then means `0`, and the
directive later changes only what "omitted" resolves to. Order the work: directive first (2
hours), then this, so the gate below can include a directive-driven case.

## 4. Gates — discriminating, closed-form, then the integrator's own order

1. **Sample-and-hold of a ramp.** `zi_nd(V(in), {1}, {1}, T)` on `V(in) = t`: a staircase
   whose value at every point is `T·floor((t − t0)/T)` exactly (breakpoints land on every
   step). The fold gave `y = t`; a history buffer that samples late gives a shifted staircase.
2. **First-order IIR on a step.** `zi_nd(V(in), {1−a}, {1, −a}, T)` on a unit step: at every
   sample instant `y_k = 1 − a^k` exactly, in closed form, for `a = 0.8`. Checked at the
   instants and, with `tt = T/10`, along the ramps between them.
3. **Second-order with a complex pole pair (`zi_zp`)** on a step, against the difference
   equation iterated in the test itself — the root expansion at `z^-1` and the two-deep output
   history.
4. **AC.** `|H(e^{jωT})|` and phase for the first-order IIR against the closed form
   `(1−a)/|1 − a e^{−jωT}|`, across a decade below Nyquist; the `sinc`-free statement checked
   by the same test.
5. **DC.** `H(1)` for each form, including a zero root (factor `z` → 1) and a conjugate pair.
6. **Jacobian.** The `tt = 0` sampling load's `(n_0/d_0)·u.grad` against a finite difference,
   through `assert_assembled_jacobian_matches_fd`.
7. **QSPICE.** QSPICE has a sample-and-hold behavioural element; whether its Z-filter (if
   any) matches the LRM's definition needs checking before a golden is cut. If it does not,
   gates 1–6 are the oracle, as for `absdelay` in AC, and the entry says so.

## 5. Size

About two days: half a day for the IR/frontend/interface update, a day for codegen's three
paths and the state bookkeeping, half a day for the gates and the `token-reference.md` rows.
The `` `default_transition `` directive is separate and small.
