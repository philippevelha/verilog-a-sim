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
| 2. The neuron at transistor level (Fig. 2's blocks, designed and sized by us: the paper gives no sizes and draws the op-amp and flip-flop as boxes) | `fig5_tx*.net` (`gen_neuron.py`) | **runs** (2.5.0), every block; measurements below |
| 3. Wall-time projection to the MNIST network, and the optimisations it calls for | — | next |

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

## The neuron at transistor level (2026-10-02, i7-1185G7, 8 threads, 2.5.0)

`gen_neuron.py` writes the Fig. 5 deck (access transistors as in `fig5_access.net`) with any
subset of the neuron's four blocks as SG13G2 transistors, the rest behavioural. The neuron is
the MNIST decks' form: refractory off, one flip-flop. **Every design and size below is ours.**
The paper gives the topology of Fig. 2 and no sizes, so none of this reproduces the authors'
circuit. It shows that a circuit of this shape works in this technology.

| Block | What it is | Where it departs from Fig. 2, and why |
|---|---|---|
| `switches` | leak transmission gate V_m ↔ C_L on qclk; NMOS discharge of C_L on clk; NMOS reset to V_reset on rs | The leak is a TG, not the single NMOS MN9: V_m reaches ~2.4 V, and an NMOS with its gate at 3.3 V and its bulk at ground passes little above ~2.2 V |
| `comparator` | NMOS-input 5T comparator against an explicit V_th = 2.4 V (tail from V_bn = 0.8 V, ~4.4 µA), two inverters | Not the Schmitt trigger: its threshold is set indirectly and needs sizes the paper does not give. No hysteresis; the flip-flop reads it at the end of a high phase, where V_m is settled |
| `dff` | master–slave TGs on clk/qclk onto 10 fF hold caps, inverters; reset rs = (m0·clkb) + (q0·clk) as an AOI22 + inverter | The power-on term is dropped (por is 0 in these decks) |
| `input` | holds the source line at V_ref and delivers I_in / K into V_m while clk is high: a two-stage op-amp drives a PMOS conveyor that sinks I_b + I_in, mirrors scale by 1/K, cascoded outputs, a trimmed I_b/K sink, steering onto V_m or onto a follower that tracks V_m | Fig. 2's push-pull conveyor has a dead zone at I_in = 0, across which the op-amp would have to slew ~1.4 V on every sign change; the bias current I_b removes it. `input_stage.py` carries the reasoning and the measured accuracy |

All biases (V_bn, V_bp, the cascode biases and the input stage's trim) are global ideal sources,
and every DC source powers up over 1 ns: under UIC, a PMOS gate or bulk on a source that is
already at 3.3 V at t = 0 has to charge PSP103's internal nodes through ohms into femtofarads,
faster than the step floor.

**The ladder.** Each rung adds one block. Run with
`va-cli sim <deck> --model models/snn --model models/sg13g2/psp103/psp103.va --tran --report clk,wl1,vm,spk`,
checked with `check_snn.py fig5`, all on the 2.5.0 release binary:

| Deck | Transistor blocks | PSP103 devices | Unknowns | Timepoints | Wall | Worst \|V_m − equations\| | Output spikes |
|---|---|---|---|---|---|---|---|
| `fig5_access.net` | none (access only) | 8 | 208 | 13 641 | 24.7 s | 0.1305 V | 6, 10, 14 |
| `fig5_tx_none.net` | none | 8 | 186 | 12 940 | 24.0 s | 0.1305 V | 6, 10, 14 |
| `fig5_tx_sw.net` | switches | 12 | 250 | 17 916 | 67.0 s | 0.1462 V | 6, 10, 14 |
| `fig5_tx_cmp.net` | + comparator | 21 | 397 | 18 560 | 110.8 s | 0.1558 V | 6, 10, 14 |
| `fig5_tx_dff.net` | + flip-flop and reset logic | 43 | 750 | 23 513 | 282.6 s | 0.1558 V | 6, 10, 14 |
| `fig5_tx_in.net` | input stage only | 33 | 605 | 16 997 | 167.2 s | 0.1472 V | 6, 10, 14 |
| **`fig5_tx.net`** | **all four** | **68** | **1169** | **23 786** | **443.9 s** | **0.1733 V** | **6, 10, 14** |

The output spikes are the paper's at every rung. The V_m error is measured against the paper's
equations with ideal switches, and 0.1305 V of it is the access transistors' R_on (first row;
see above), so `check_snn.py`'s 0.05 V tolerance reports FAIL throughout, for the reason given
there. What each block adds to the worst error: the switches +16 mV, the comparator +10 mV, the
flip-flop nothing measurable, the input stage +17 mV alone and +18 mV on the full neuron. (Where
the switches' and the comparator's shares come from -- R_on, charge injection, input
capacitance -- has not been separated.)

**The input stage, in numbers** (`bench_input_stage.py`, `scan_input_stage.py`,
`scan_point.py`): DC gain 1.429 at I_in = +10 µA and 1.448 at −10 µA against 1/K = 1.4368
(curvature in I_in, ±0.7 %), flat to 0.1 % over V_m = 0.45–2.4 V; |I_out| at I_in = 0 under
0.24 µA over the same range after the V_bs trim; the source line held at V_ref + 0.09 mV.
Against the behavioural `sl_stage` in the same deck (`fig5_tx_in` vs `fig5_tx_none`), V_m differs
by at most 20 mV per step: the integration slope is 0.7 % low at I_in ≈ 5.8 µA and 1.9 % low at
≈ 9 µA, and the switch edges add 10–19 mV at low V_m. Four things were found and fixed on the way
to that, in the transient deck rather than the DC bench:

  * the complementary clock clkb was written as a pulse starting low, so clk and clkb were both
    low for the first 124 ns, and the steering's PMOS put 0.26 V on V_m before the first window.
    `gen_neuron.py` now writes clkb as the exact complement of clk after the 1 ns power-up;
  * the op-amp had a 1.5 mV systematic offset (second-stage current density 2.5× the first's),
    1.6 % of V_s = 0.1 V; resized, 0.09 mV;
  * the output node's capacitance (~34 fF with the cascodes as wide as the mirrors) waited at
    V_ref between windows and shared charge with C_M at every window start: a 25–40 mV step
    toward V_ref per cycle, an extra leak. Narrower cascodes alone took it only to ~24 fF; the
    output now waits on a follower that tracks V_m (a 5-transistor PMOS-input OTA), so there is
    nothing to share;
  * during a window that capacitance (~12 fF after the above) still sits in parallel with C_M,
    so the mirror is made stronger by 1 + C_o / C_M (`C_O_TRIM`, measured from the slope).

**Cost.** The full neuron takes 18× the access-only deck's wall time for 8.5× the devices and
1.7× the timepoints. The MNIST projection (step 3) starts from these numbers.

## Breakpoint slivers (va-transient, 2.5.0)

The first full-neuron run with the follower ended at t = 1.372 µs in a timestep underflow, with
Newton failing at every step size down to the floor and its residual stuck at exact powers of
two (2⁻²⁰, 2⁻²¹, 2⁻²² A) on PSP103 internal rows. The cause was not the circuit: the run had landed on
a source corner at 1.3720000000000002e-6 s, and a model then registered a breakpoint at the
same corner one ulp later, so every attempt was clamped to a 2.1e-22 s step. A companion
coefficient of 4.7e21 makes the charge terms ~5.7e7, whose rounding is the residual. A breakpoint
less than the step floor after the current time is now treated as the instant already landed on.
Three existing decks took such slivers: `fig4_neuron`, `fig5_neuron` and `fig5_access` lose 2–4
timepoints each, all sub-floor duplicates of a corner, and every V_m sample their checks read is
unchanged at the printed precision (`check_snn.py` PASS with identical numbers for Figs. 4 and 5).
