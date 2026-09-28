# Memristor-crossbar spiking neuron (Xuan et al., IJCNN 2026)

A reproduction of the neuron-level results of H. Xuan, Z. Jouni, M. Manokhin, D. Galayko and
H.-G. Stratigopoulos, *Automated Model-to-Transistor Design Framework for Memristor
Crossbar-Based Spiking Neural Network Architectures*, IJCNN 2026 (hal-05590496; the PDF is in
`references/`). The paper's circuit is a 2T2R RRAM crossbar feeding a clocked analog
leaky-integrate-and-fire neuron. It is designed so that, sampled once per clock cycle, the
neuron matches the snnTorch neuron it was trained as.

## What is reproduced, and how well

| Paper result | Here | Result |
|---|---|---|
| Sec. II-C: the 2T2R synapse codebook from the four RRAM levels | `check_snn.py codebook` | the 13 values, within 0.0003 mS of the paper's rounded figures |
| Fig. 4: one neuron, five identical synapses, refractory 1 µs | `fig4_neuron.net` | output spikes at **1.50 µs and 3.75 µs, as in the paper**; V_m within 11 mV of the paper's equations at all 20 steps |
| Fig. 5: one neuron, four codebook synapses, circuit vs snnTorch model | `fig5_neuron.net` | output spikes at **steps 6, 10, 14, as in the paper**; V_m within 12 mV of the paper's equations; within 0.14 V of the paper's transistor-level trace (read off the figure by eye) |

Plots: `fig4_reproduced.png` and `fig5_reproduced.png` (clock, input spikes, V_m and the output
spike in continuous time, then V_m per timestep against the references).

**Not reproduced:** the transistor-level circuit itself, the ECG and MNIST networks (Figs. 7–10),
the process corners (Fig. 11), and the power figures (Tables I–II). The circuit is built at the
level of the paper's functional diagram (Fig. 3), because the IHP SG13S process with its MEMRES
module is not public and the paper gives no device sizes. What that level keeps and drops:

- **real circuit elements:** C_M = 998 fF and C_L = 660 fF, the RRAM conductances at the stated
  levels, clocked switches, the 0.1 V read bias. **The leak β is not a parameter.** It comes out
  of charge sharing between C_M and C_L, and matches 998/(998+660) = 0.602;
- **ideal blocks:** the op-amp and attenuator (`models/snn/sl_stage.va`), the comparator
  (memoryless, so the Schmitt trigger's hysteresis is dropped), and the switches (no charge
  injection or clock feedthrough, which is part of why the paper's transistor trace differs from
  its model by up to 0.2 V and this circuit's does not);
- **flip-flops built as circuits** (`dff.inc`: two sample-and-hold stages each), holding their
  state on capacitors as a transistor DFF does. (They began as a workaround for the simulator gap
  at the end of this file, now fixed; they stay because they are the closer model.) The refractory timer is a chain of them, so T_ref
  is a whole number of cycles (the paper's 1 µs = 4 cycles is exact). The paper's timer is an
  analog countdown set by V_bias2.

## Run

```bash
va-cli sim circuits/snn/fig4_neuron.net --model models/snn --tran --report clk,wl,vm,spk > fig4.out
python circuits/snn/check_snn.py fig4 fig4.out --plot fig4.png
va-cli sim circuits/snn/fig5_neuron.net --model models/snn --tran --report clk,wl1,vm,spk > fig5.out
python circuits/snn/check_snn.py fig5 fig5.out --plot fig5.png
python circuits/snn/check_snn.py codebook
```

Each deck runs in about 3 s (release build, i7-1185G7): 76 unknowns, about 13 000 timepoints.
`check_snn.py` exits non-zero if the output spikes differ from the paper's or V_m departs from
the paper's equations by more than 50 mV at any step.

## What the check compares against

The circuit runs in continuous time. As the paper does for its own Fig. 5, V_m is read once per
cycle, at the latch edge, and compared with:

1. **The paper's equations** (1)–(7) and (9), evaluated with the deck's component values: this is
   the test of the circuit;
2. **What the paper reports:** Fig. 4's spike times; Fig. 5's output spikes; Fig. 5's model-level
   V_m; and Fig. 5's transistor-level V_m, read off the figure to about ±0.05 V.

The Fig. 5 model-level V_m is recomputed, not read off: V[k] = 0.6·V[k−1] + Σ w·S[k] with the
paper's weights {1.10, 1.51, −0.71, 0.60} and input spikes reproduces every plotted point.

## Things the paper leaves open, and what was chosen

- **The attenuator factor K is not given**, and the two figures imply different values. Sweeping
  K through the paper's equations: Fig. 4's spike times need K in [2.872, 3.047]; Fig. 5's output
  spikes need K in [0.656, 0.762]. No single K reproduces both with the stated V_s = 0.1 V,
  Δt = 125 ns and C_M = 998 fF. Each deck therefore sets its own: K = 3 (Fig. 4), and K = 0.696
  (Fig. 5), from the least-squares λ = 17.99 V/mS that maps the paper's weights to the
  conductances it lists (Eqs. (3), (12)).
- **Fig. 5's model trace shows no reset.** Its step after each crossing is β·V + w, whereas the
  circuit, and the paper's own transistor trace, sit at about V_reset there.
- **The codebook values listed for Fig. 5 are not all from the reduced codebook.** The paper
  lists −0.0286 and 0.0286 mS; the reduced 9-value codebook it defines has ±0.0298 instead. The
  decks use the values the paper lists for that figure.
- **Initial V_m:** Fig. 4 starts at V_reset, as the paper states (a power-on reset in the first
  low phase); Fig. 5 starts at 0 V, since its first plotted value is the first spike alone.
- **The paper's inhibitory step in Fig. 5 is larger than its codebook allows.** The paper's
  transistor trace falls by about 0.68 V at step 2, where λ·0.0286 mS is 0.51 V, and its model
  uses −0.71 (unquantized). This circuit gives the quantized 0.51 V, which is most of the
  0.25 V gap to the paper's model at that step.

## Built for this, and found while building it

- **Hold time matters, even with ideal switches.** With the flip-flop's master switch opening at
  the same clock threshold where the reset starts, the latch followed the comparator partway down
  and spikes came out at 2.4–2.6 V. The master now opens earlier on the falling edge (`dff.inc`).
- **A transparent latch cannot gate its own input.** Gating the reset with the master latch
  ungated closed a loop through the comparator, and V_m settled exactly at V_th. Master outputs
  are now used only while clk is low (`models/snn/neuron_logic.va`).
- **Simulator gap, found here and fixed in 2.1.0: variables assigned in an event body did not
  keep their values.** The first version kept the flip-flop and refractory counter as
  Verilog-A variables updated in `@(cross(...))` and `@(initial_step)`, the usual way to write
  this. Their values never reached a later timepoint, so the neuron never fired, and nothing
  reported why. Since 2.1.0 such variables persist across accepted timepoints (the LRM's static
  variables; `docs/token-reference.md`, `Real`/`Integer`), and that event-based controller
  reproduces Fig. 4 as well: spikes at 1.50 and 3.75 us, V_m within 12 mV of the equations. The
  decks keep the capacitor flip-flops, which are the closer model of the paper's circuit.
