# The SNN with real transistors

The decks in `circuits/snn/` and `circuits/snn/mnist/` simulate the Xuan et al. (IJCNN 2026)
SNN with **no transistor model**. The crossbar's access transistors and the neuron's clocked
switches are ideal smooth switches (`vswitch.va`, R_on = 10 Ω), and the op-amp, attenuator,
comparator and reset logic are ideal behavioural blocks. This directory replaces those
idealisations with MOS devices, one step at a time.

## How the technology is swapped

Every MOS device in these decks comes from `tech.py`. A generator asks it for device lines, and
it writes one `.model` card per geometry into the deck's `tech.inc`. The decks never name a
process. The only backend today is **`sg13g2_hv`**: IHP's SG13G2 open-PDK thick-oxide 3.3 V
devices through the PDK's own PSP103 (`models/sg13g2/`, typical corner, not validated against a
reference simulator). When the foundry's SG13S + MEMRES models arrive, they become a second
backend, and the decks are regenerated with `--tech`. A run names both libraries:

    va-cli sim <deck> --model models/snn --model models/sg13g2/psp103/psp103.va --tran

(`--model` repeats since 2.3.0, and each library resolves its own `` `include ``s.)

## Steps

| Step | Deck | State |
|---|---|---|
| 1. Access transistors in the Fig. 5 deck | `fig5_access.net` (`gen_fig5_access.py`) | **runs**, first measurements below |
| 2. The neuron at transistor level (Fig. 2's topology, sized by us: the paper gives no sizes and draws the op-amp and flip-flop as boxes) | — | next |
| 3. Wall-time projection to the MNIST network, and the optimisations it calls for | — | after 2 |

`uic_access.net` is one 2T2R branch started cold, kept as va-cli's regression deck for the
seed rule (below).

## First measurements (2026-10-01, i7-1185G7, 8 threads, 2.3.0)

**The access transistor is not a 10 Ω switch.** Its source sits on the source line at
V_ref = 1.65 V with the bulk at ground, so it has 1.65 V of gate drive and a large body effect.
Measured R_on (W = 10 µm unless stated, L = 0.45 µm, V_GS = 1.65 V, V_SB = 1.65 V):

| W | R_on | vs LRS1 (8 kΩ) |
|---|---|---|
| 1 µm | 3681 Ω | 46 % |
| 2 µm | 1759 Ω | 22 % |
| 5 µm | 731 Ω | 9.1 % |
| 10 µm | 377 Ω | 4.7 % |

So every synaptic conductance is really 1/(R_cell + R_on), and the codebook shifts with it.
`fig5_access.net` uses W = 10 µm (`--w`). Its output spikes are still at **steps 6, 10, 14**,
as in the paper. V_m departs from the paper's equations by up to **0.131 V** (the ideal-switch
deck: 0.012 V), and `check_snn.py fig5` reports FAIL against its 0.05 V tolerance. That
tolerance was written for ideal switches. The departure is the R_on effect, not an error. The
remedy is to put R_on into the codebook the training quantizes to, not to loosen the tolerance.

**Wall time** for the same 3.75 µs (15 clock cycles):

| | unknowns | timepoints | wall (8 threads) | wall (1 thread) |
|---|---|---|---|---|
| `fig5_neuron.net` (ideal switches) | 80 | 10 707 | 4.0 s | — |
| `fig5_access.net` (8 PSP103) | 208 | 13 645 | 64.3 s | 115.5 s |

The timepoint count grows 1.3×, and the cost per timepoint grows 12×. About 8.5 ms per
timepoint single-threaded puts **one PSP103 evaluation at roughly 200 µs**. That is the number to
attack before the MNIST network (17 664 access transistors) is worth attempting at transistor
level. One structural lever is visible already: in a spiking network most word lines are low
most of the time, so most access transistors sit at a constant bias between steps, and SPICE's
device bypass (reuse a device's stamps when its terminal voltages have not moved) would skip
them.

## The seed rule (va-transient, 2.3.0)

Every one of these decks starts from rest (`UIC`), because the flip-flops need it. The first
run of `fig5_access.net` ended at t = 0 with a timestep underflow. A PSP103's internal source
node carries charge but is tied to the 1.65 V source line through the device's terminal
resistance, so from the zero seed it has to jump at every step size, and the LTE controller
shrank to the floor. The step at the floor is exactly the t = 0+ solution UIC means (charges
held, everything else settled), so it is now accepted there, on the first step only, followed
by a restart. Runs that integrated before are bit-identical: `xtask validate` 28/28 and
Fig. 4, Fig. 5, the PSP103 inverter and c17_tran give identical outputs before and after.
