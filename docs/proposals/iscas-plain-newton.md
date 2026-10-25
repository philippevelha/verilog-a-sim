# Why plain Newton fails on ISCAS'85: the rows, named

**Status:** findings, 2026-10-25 (verilog-a-sim 2.44.0+2). No simulator change. **Decision
(2026-10-25): leave the rescue as it is for now.** Option 2 is kept as a future possibility,
`docs/future_development.md` §11. This answers the first question of `docs/roadmap.md` "After 2.0"
item 1, "name the rows whose pivots vanish".
**Tool:** `cargo xtask newton-trace` (new with this document), which runs plain Newton on a deck
and names what moves: dense on a small deck, `--sparse` on a large one, with `--cap`, `--shunt`,
`--accept-backward` and `--solve` (the real `va_core::newton`) to test each hypothesis below.
**Evidence:** i7-1185G7, release build, single runs.

## 1. Summary

The "singular matrix" that ends plain Newton on c432 and the larger circuits is **not a
singular Jacobian**. Two separate things happen:

1. **A runaway on leakage-held stack nodes.** Between two off NMOS transistors in a series stack,
   a cell-internal node is held only by subthreshold leakage. Plain Newton multiplies its step
   by about 2.4 per iteration, to 10¹⁵ V by iteration 34, while the residual falls. The node is
   not a junction unknown, so junction limiting never touches it. **A node-step cap (0.5 V)
   cures it.**
2. **A false "singular" after the first capped step.** One capped step puts every node near
   mid-rail, in the high-gain region of the CMOS gates. The linearized circuit is then a chain
   of amplifiers, about 17 gates deep on c432. The correct Newton step grows by about an order
   of magnitude per logic level: 1.3 V at the inputs, 2×10¹⁰ V at depth 9, 8×10¹³ V at depth 13.
   That step is backward stable (backward error 3.3×10⁻¹⁷). But `SparseLu`'s acceptance test is
   absolute, |r| ≤ 10⁻⁶(1 + |b|). A backward-stable residual is about ε|A||x|, which at
   |x| = 10¹⁵ is far above that bound, so the solve is reported as `Singular`.

Accepting that step by its backward error, and capping the update as in (1), **capped plain Newton
converges on c432 in 60 iterations**. The current rescue takes 228: 3 failing plain iterations,
then 225 over 13 capped `gmin` stages. All 8 099 printed nets agree with the rescued operating
point to 1.8 µV, and all 196 gate outputs give the same logic. **But it is not general:** across
seven circuits it converges on two (§3).

## 2. How each step was shown

**One cell reproduces the runaway.** A single `nand4c` (c432's own cells, ideal input sources,
`scripts/diagnostics/iscas_cells.py`) fails plain Newton for every input with `a` = 1. `newton-trace` names
the unknown: `V(X1.X1.n1)`, the `and4` stack node between `nm02` and `nm04`. It goes −42 V, −1 684 V,
−4 311 V, … (×2.4 per iteration). Its Jacobian column and row hold only the two ideal-short branch
rows that tie it to the transistors' internal `DI`/`SI` nodes. With `--cap 0.5` the same cell
converges (22 iterations through `va_core::newton`). With the cap, all 10 cell types converge for
all 70 input combinations.

**c432 fails differently, and not structurally.** Capped plain Newton on c432 fails at the second
assembly, as `initial-guess.md` recorded. At that point the Jacobian has no all-zero row or
column and no duplicated row. Its entries span 6.6×10⁻¹⁵ to 5.2×10⁷. Its block triangular form
has 12 669 diagonal blocks of at most 30 unknowns, and **the worst block has σ_min/σ_max =
4.9×10⁻¹⁰**, well conditioned enough for double precision. A block triangular matrix is singular
only if a diagonal block is, so the matrix is not singular. faer factors it, and its solution
has steps of 9.5×10¹⁴ V with a residual of 6×10⁻². That residual is ε|A||x| (10⁻¹⁶ × 4 × 10¹⁵):
the solve is as accurate as floating point allows.

**The size of the step is the circuit's gain.** Grouping the step on each gate's output net
`V(gN_0)` by the gate's logic depth in `c432.bench` gives the geometric growth quoted in §1:

| depth | 0 | 1 | 4 | 5 | 7 | 9 | 11 | 13 |
|---|---|---|---|---|---|---|---|---|
| median \|dx\| (V) | 1.3 | 20 | 1.2×10³ | 1.2×10⁵ | 1.8×10⁶ | 2.9×10¹⁰ | 5.3×10¹² | 7.9×10¹³ |

Past depth 13 it falls again (gates out of their high-gain region at this state). A fixed shunt
on every node does not help: at 10⁻⁹ and 10⁻⁶ S the step grows (1.7×10¹⁸, 2×10²⁷ V), because the
shunt changes the operating point the gain is evaluated at, not the gain's structure.

## 3. Capped plain Newton with backward-error acceptance, measured

`scripts/diagnostics/iscas_capped.py` runs each deck with `newton-trace --sparse --cap 0.5 --accept-backward`
(the tool, not `va-cli`, so its wall times include its own overheads) against `va-cli`'s current
`.op`:

| circuit | capped plain | current rescue | answers |
|---|---|---|---|
| c432 | converged, 60 iterations | 228 iterations | max \|ΔV\| 1.8 µV, logic 196/196 |
| c880 | converged, 55 | 177 | max \|ΔV\| 11 µV, logic 443/443 |
| c499 | **failed** at 13 | 181 | — |
| c1355 | **failed** at 13 | 191 | — |
| c1908 | **failed** at 18 | 180 | — |
| c2670 | **failed** at 16 | 240 | — |
| c3540 | **failed** at 10 | 367 | — |

Where it fails, it is not the linear solve. The capped node updates leave the branch currents of
the ideal shorts uncapped, these grow to 2×10⁴ A, Jacobian entries reach 10⁴⁰ and the solve ends
in NaN. Capped Newton is not globally convergent, and the `gmin` ladder is what provides that.
c5315, c6288 and c7552 were not run.

**Backward-error acceptance alone changes nothing.** With `SparseLu` accepting a solve whose
backward error is below 10⁻¹², and nothing else changed, the rescue takes exactly the same 228 /
177 / 181 iterations on c432 / c880 / c499. Only the plain tier meets the false "singular", and
without a cap the huge step it would accept is useless.

## 4. Options

1. **Leave it.** The rescue works, and the cause is now understood. *Cost:* nothing.
2. **A capped-plain tier before the ladder**, solving with backward-error acceptance. It helps
   c432 and c880 (about 70 % fewer iterations) and costs 10–18 failing iterations elsewhere, about
   5–8 % of their rescue. A deck that needs no rescue is untouched. A rescued deck's answer moves
   at the µV level, deterministically on every platform. *Cost:* a change to `va_core::dc` and
   `sparse`; the tier's iteration bound and failure test need tuning on the full suite.
3. **Cap the branch-current unknowns too**, so the capped tier fails less often. It is untested.
   The cap on a branch current would need a scale (amperes), which a node cap in volts does not
   give.
4. **The SPICE answer is device-level limiting**: `fetlim`/`limvds` on each MOSFET's terminal
   voltages, which a Verilog-A model requests through `$limit`. PSP103's source has no `$limit`,
   so this would mean limiting the models' terminal voltages from the outside.

The finding that matters for `docs/roadmap.md` is the first one: **the ISCAS'85 failure is a
gain problem, not a pivot problem**. The phrase "rows whose pivots vanish" in the roadmap was
the hypothesis this replaces.
