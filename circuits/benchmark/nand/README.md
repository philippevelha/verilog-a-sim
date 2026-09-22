# NAND2, one deck per MOSFET model

`Out = NOT(A AND B)`: a series NMOS stack pulling down, a parallel PMOS pair pulling up. The
topology is terminal-for-terminal the LTspice deck it came from —

```
M1 Out  A  N001 Vss NMOS        M3 Vdd  B  Out  Vdd PMOS
M2 N001 B  Vss  Vss NMOS        M4 Vdd  A  Out  Vdd PMOS
```

— with `M<name> d g s b` rewritten as `X<name> d g s b <module>`, because `M` in this netlist
parser is the three-terminal form and `X` is the general one. N and P channel come from each
model's own polarity parameter (`TYPE = ±1`, or `type` in BSIM4); an unknown override is
rejected with the model's parameter list, so a model without one fails loudly rather than
silently giving two NMOS in the pull-up.

**Why a NAND and not another inverter.** The series stack has an internal node, `N001`, that
floats whenever the lower device is off. A single transistor never exercises that, and it is
where most of the results below come from.

**No model cards.** Every parameter is the `.va` file's default, so these are not calibrated
devices and the logic levels are not anyone's silicon. The question each deck answers is whether
the model elaborates, differentiates and solves a two-high stack at all.

## Status, measured 2026-09-22 at Vdd = 1.2 V

`Vout(A=0)` and `Vout(A=1)` are the ends of the swept transfer curve with B held high.

| deck | model | Vout(A=0) | Vout(A=1) | |
|---|---|---|---|---|
| `bsim4.net` | BSIM4 | 1.199992 V | 4.26e−6 V | works |
| `bsim6.net` | BSIM6 | 1.200000 V | 4.20e−9 V | works |
| `bsimbulk107.net` | BSIM-BULK 107 | 1.200000 V | 3.16e−14 V | works |
| `psp102.net` | PSP102 | 1.199999 V | 1.09e−6 V | works |
| `psp103.net` | PSP103 | 1.200000 V | 8.96e−7 V | works |
| `psp104.net` | PSP104 | 1.200000 V | 8.31e−8 V | works |
| `bsimcmg.net` | BSIM-CMG | 1.200000 V | 3.70e−8 V | works (needs the `gmin` rescue) |
| `lutsoi.net` | L-UTSOI | 1.200000 V | 1.41e−10 V | works (needs the `gmin` rescue) |
| `bsimsoi.net` | BSIM-SOI | 1.200000 V | 0.437124 V | solves, weak pull-down — **device, not solver** |
| `ekv26.net` | EKV2.6 | 1.200000 V | 1.098795 V | solves, barely pulls down — **device, not solver** |
| `bsimimg.net` | BSIM-IMG | 1.157092 V | 0.370723 V | degraded — **deck, not solver** |
| `hisim2.net` | HiSIM2 | — | — | non-finite at `V(XM3.dp)` |
| `zoo_nmos.net` | `models/mosfet.va` | 5 V | 0.115 V | works, with a caveat in its header |

**Eleven of twelve solve; eight are clean NANDs.** What remains is not one problem but three,
and they are worth separating.

### 1. `gmin` — wired in v1.3.1, as a rescue

`va_core::convergence` had gmin stepping and `NewtonConfig::gmin_steps` defaulted to **0**, so
the homotopy never ran and a floating `N001` was simply fatal: only six of these decks solved.

It is now a **fallback**, not a default path (`va_core::dc::with_gmin_rescue`): the plain solve
is tried first, and only a failure it can plausibly rescue — singular, non-convergent, or
non-finite — earns a second attempt with the ladder. That ordering is the whole design. The
ladder is `gmin_steps + 1` full Newton solves, so running it unconditionally would multiply the
cost of every DC point in every circuit, including the large majority that converge first try.
As a fallback those circuits pay nothing and their answers stay bit-identical — which is why
`xtask validate` is unchanged, digit for digit, across the change.

That took the decks from 6 to **11 of 12 solving**. `bsimcmg` and `lutsoi` became clean NANDs;
`bsimsoi` and `ekv26` now solve but pull down only to 0.437 V and 1.099 V, which at default
parameters is the device rather than the solver.

`zoo_nmos.net` still carries an explicit 1 GΩ leak on `N001` rather than leaning on the rescue,
and its header says why: it is the eight-line reproduction of what a floating node does, and it
is more useful demonstrating the problem than hiding it.

### 2. Two instances sharing a thermal node give a singular matrix — a bug, still open

Reproduced minimally, and it is not about the NAND:

```
X1 out g gnd gnd tn bsimbulk TYPE=1
X2 out g gnd gnd tn bsimbulk TYPE=1     <- same tn
RT tn gnd 1e-3
```

One instance on that node solves. Two instances on *separate* nodes solve. Two sharing one
node: singular. Four-port models sharing a node are fine — `psp103.net` has M3 and M4 sharing
`vdd` as their bulk and works — so it is specific to the extra port. Root cause not established.

The five-port decks here therefore give each instance its own thermal node, which is a
workaround and is marked as one in each header.

### 3. `bsimimg.net` is the deck's fault, not the simulator's

BSIM-IMG is an independent-double-gate device: its ports are `(d, fg, s, bg, t)`, front gate and
**back gate**, not gate and bulk. Mapping the NAND's bulk connection onto `bg` biases the back
gate as if it were a body contact, which is not how the device is meant to be driven. The
degraded 0.79 V swing is that mistake, faithfully simulated. Fixing it means deciding a back-gate
bias, which is a device-engineering choice rather than a netlist one.

### 4. HiSIM2 fails for its own reasons

`V(XM3.dp)` is an internal node of one of the PMOS devices, and it goes non-finite with gmin on
or off. Not diagnosed.
