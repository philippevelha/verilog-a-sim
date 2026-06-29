# Bridge β — the model-instance ABI (`va-abi`)

> Status: **draft**, describing the **frozen v0** traits in `crates/va-abi/src/`.
> Type sketch of record: [`../interfaces.md` §β](../interfaces.md). Changes follow §6.

## 1. Role

Bridge β is the project's internal **OSDI**: the uniform calling convention by which the
solver evaluates a device, regardless of where the device came from. It sits between
**model code** (whatever produces a stampable device) and the **numerics** (`va-core` and
the analyses built on it).

The whole point is *interchangeability*. To `va-core`, a hand-written reference resistor and
a `va-codegen`-generated BSIM model are the same thing: something that, given a solution
vector `x`, deposits a residual, a Jacobian, and (optionally) charge terms into a sink. The
solver never knows or cares which it is holding.

## 2. Producers & consumers

| Party        | Crate(s)                                   | Promise across this bridge                                                        |
|--------------|--------------------------------------------|-----------------------------------------------------------------------------------|
| **Producers**| `va-abi::reference` (hand-written), `va-codegen` (generated), `va-netlist` (primitives) | Implement `ModelInstance`. `load` is pure, deterministic, and stamps a correct residual + Jacobian (+ charge if it stores energy). |
| **Consumers**| `va-core` (T3), `va-transient` (T4), `va-acnoise` (T5) | Implement `StampSink` (the real one assembles the MNA system). Call `unknowns` once to map indices, then `load` repeatedly inside Newton. |

`va-abi` ships **working** `resistor`, `capacitor`, and `diode` reference models so `va-core`
has something real to solve on commit #1 — the core team is never blocked on the compiler
team. The reference models are also the fixtures the AD-generated models are diffed against.

## 3. The contract

Authoritative source: `crates/va-abi/src/instance.rs` and `stamps.rs`.

```rust
pub trait ModelInstance {
    fn unknowns(&self) -> &[usize];                       // global indices touched
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);  // evaluate at x → stamps
}

pub trait StampSink {
    fn residual(&mut self, row: usize, value: f64);            // current into node `row`
    fn jacobian(&mut self, row: usize, col: usize, value: f64); // ∂residual[row]/∂x[col]
    fn charge(&mut self, row: usize, value: f64);              // Q at `row`   (transient)
    fn dcharge(&mut self, row: usize, col: usize, value: f64); // ∂Q[row]/∂x[col] (transient)
}
```

There are **two channels**:

- **resistive**: `residual` + `jacobian`. Used by DC, and as the conductive part of
  transient and the operating point for AC.
- **charge**: `charge` + `dcharge`. Consumed by the transient integrator via a companion
  model. **DC ignores this channel entirely.**

`va-abi` also ships `DenseStamp`, a dense reference `StampSink` for tests and the crate's own
reference-model checks. Production assembly (sparse, ground-reduced) lives in `va-core`.

## 4. Semantics & invariants

### 4.1 Index space

`row`/`col` and the entries of `unknowns()` live in **one** index space: the global solution
vector `x`. The mapping `local terminal → global index` is the instance's private business,
fixed at construction; `unknowns()` reports the set, and `x[idx]` reads a terminal's value.

1. **`unknowns()` is complete and stable.** It lists every global index the instance ever
   stamps into (as a `row` or `col`) or reads from `x`, except the reference node. It does
   not change between calls.
2. **The reference node is a sentinel.** Ground is modelled as an index `>= dim` (the number
   of global unknowns). A `StampSink` **must** drop any contribution whose `row` or `col` is
   the reference node — `DenseStamp` does exactly this (out-of-range indices ignored). A
   model may therefore stamp "into ground" freely and rely on the sink folding it away.
3. **Accumulation, not assignment.** Every sink method **adds** to what is there. A model may
   stamp the same `(row, col)` more than once in a single `load`; the sink sums. Consumers
   zero the system before each `load` sweep, never the model.

### 4.2 The `load` contract

4. **Purity.** `load` is a pure function of `(self, x)`. No interior mutability, no I/O, no
   RNG, no dependence on call order. Calling it twice with the same `x` produces identical
   stamps. (This is what lets Newton, AC, and finite-difference checks call it freely.)
5. **Sign convention.** `residual(row, value)` is **current flowing into node `row`** (KCL
   residual). For the reference resistor at 2 V across 1 kΩ, `residual[0] = +2e-3` A. The
   Newton update solves `J Δx = −residual`; the sign here must match that.
6. **Jacobian is exact.** `jacobian(row, col, ·)` is `∂residual[row]/∂x[col]`, analytically
   correct — not a secant or numerical estimate. This is enforced by the §5 house rule:
   every differentiated model has a test asserting analytic vs central-difference agreement.
   A wrong Jacobian silently destroys Newton convergence, so this is non-negotiable.
7. **Charge channel is optional but consistent.** A memoryless device (resistor, ideal
   diode DC) stamps nothing on the charge channel. A storage device (capacitor) stamps
   `charge`/`dcharge` with the **same** sign and index conventions as the resistive channel,
   and `dcharge(row,col)` = `∂Q[row]/∂x[col]`. DC consumers ignore both; they must still be
   correct for transient.
8. **No panics on valid input.** Per §5, library `load` returns by stamping; it must not
   `panic!`/`unwrap`/`expect`. Degenerate device parameters (e.g. `R = 0`) are the
   producer's responsibility to guard at construction, returning a `Result`, not to blow up
   inside `load`.

## 5. Conventions

- **Units:** SI base — volts, amperes, kelvin, coulombs, siemens. No unit tags on the wire.
- **`dim`:** the count of global unknowns; the consumer owns it. Models learn their indices
  only through the values handed to their constructor and reported by `unknowns()`.
- **Internal unknowns:** a model needing extra unknowns (e.g. a branch current for a voltage
  source, an internal node for a series resistance) includes those global indices in
  `unknowns()` exactly like terminal indices. The consumer allocated them; the model just
  uses them.
- **Ground constant:** the reference crate exposes a `GROUND` sentinel; constructors accept
  it wherever a terminal may be the reference node.

## 6. Worked example — the reference resistor

Continuing the `R = V/R` module from [Bridge α §6](interface-alpha-ir.md). The shipped
reference model (`va-abi::reference::Resistor`) is the hand-written analogue of what codegen
emits:

```rust
let r = Resistor::new(0, GROUND, 1000.0);   // node 0 → ground, 1 kΩ
let mut sink = DenseStamp::new(1);           // one global unknown
r.load(&[2.0], &mut sink);                    // bias node 0 at 2 V

assert!((sink.residual[0] - 2e-3).abs() < 1e-15);  // I = V/R = 2 mA into node 0
assert!((sink.jac(0, 0) - 1e-3).abs() < 1e-18);    // G = 1/R = 1 mS on the diagonal
assert_eq!(sink.charge[0], 0.0);                    // memoryless: charge channel empty
```

The ground terminal's contributions (`residual[GROUND]`, `jacobian(0, GROUND)`, …) are
emitted by the model but folded away by the sink, because `GROUND >= dim`. This is invariant
4.1.2 in action: the model stamps a full 2×2 device matrix, the sink keeps the 1×1 it can
use. This exact check is `resistor_stamp_by_hand` in `va-abi`.

For a capacitor, the same trace additionally fills `sink.charge` / `sink.dcharge` with
`Q = C·V` and `∂Q/∂V = C`; DC drops them, the transient integrator companion-models them.

## 7. Edge cases & non-goals

- **DC vs transient.** A model does not know which analysis is running. It always stamps both
  channels it can fill; the *consumer* decides what to use. There is no "DC mode" flag on the
  bridge.
- **Stateless instances.** `load` takes `&self`. A model holds parameters and index maps, not
  per-iteration state. Newton state lives in the consumer. (AC/noise linearise around an
  operating point the consumer supplies as `x` — still no model-side state.)
- **Reference node only via sentinel.** There is no separate "this row is ground" flag;
  ground is purely the out-of-range index. Consumers must size `dim` so the sentinel cannot
  collide with a real unknown.
- **No time, no frequency on the bridge.** `load` sees only `x`. Time-dependence enters
  through the charge channel + the integrator; frequency enters in `va-acnoise` by
  linearising the same stamps. The bridge stays analysis-agnostic.

## 8. Evolution (per §6)

Bridge β is frozen and has the widest blast radius in the project: `va-core`,
`va-transient`, `va-acnoise`, `va-codegen`, `va-abi::reference`, and `va-netlist` all touch
it. To change it:

1. Open an issue naming the change and every crate above that it affects.
2. Get all affected owners to agree — this is a kickoff-grade coordination, not a solo edit.
3. Update this document, `../interfaces.md`, and `va-abi` in **one** PR with stub adapters so
   the workspace keeps compiling.

Adding a trait method is a breaking change for every implementor; prefer a default method or
a new sub-trait when the addition is optional (e.g. a future small-signal noise channel).

**Open items** (draft backlog, not yet contract):
- [~] The residual/Jacobian sign is now *established in code* (2026-06-29): `va-core`'s Newton
      solves `J·dx = −residual` (`newton.rs`), and `va-codegen`'s generated models reproduce
      `va-abi`'s reference stamps, so both producers agree. **Still open:** a single shared
      test fixture both crates import, rather than parallel checks.
- [ ] Decide how AC small-signal noise sources attach — extra channel vs separate trait.
- [ ] Specify the `Result`-returning constructor pattern for degenerate parameters.
