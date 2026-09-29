# MNIST through the memristor-crossbar SNN: review page

Ten held-out MNIST images through the three circuit layers (hidden 2, hidden 3, output) of the Xuan
et al. (IJCNN 2026) SNN. The summary and the choices are in `circuits/snn/mnist/README.md`; the
full step-by-step record, with every command, figure and dead end, is `circuits/snn/mnist/LOG.md`.

![summary](summary.png)

Top: `va-cli` wall time per image (i7-1185G7, 8 threads; 9.76 h for the ten, 58.6 min on average).
Bottom: spike slots where the circuit differs from the quantized model (40 011 of 40 020 agree;
every difference is on an image whose model membrane passes within ~2 mV of threshold).

The circuit's class equals the model's on all ten; 8/10 equal the label (the model's own errors:
the 4 is read as 9; the 9 is a 12–12 tie between 4 and 9, broken toward 4).

## Per image

Each plot: output spike rasters (circuit and model), output spike counts, and the ten output
neurons' membrane voltage (the labelled class in red, V_th dashed).

**Note the V_m scale:** the membranes reach about −6.7 V to +8.7 V, outside a 3.3 V supply. The
circuit's input stage is ideal and, like the model, does not bound V_m; a 3.3 V circuit would clip
at the rails (`circuits/snn/mnist/README.md`, limitations).

| | |
|---|---|
| ![0](digit0.png) | ![1](digit1.png) |
| ![2](digit2.png) | ![3](digit3.png) |
| ![4](digit4.png) | ![5](digit5.png) |
| ![6](digit6.png) | ![7](digit7.png) |
| ![8](digit8.png) | ![9](digit9.png) |

## The netlist

`digit0_full.net` is the complete deck for image 0 as simulated, with its one `.include` (the
flip-flop subcircuit) inlined: 9 141 lines, 8 832 `synapse_2t2r` cells, 138 neuron subcircuits,
64 hidden-1 word-line sources. The other nine differ only in the word-line sources; regenerate any
with `python circuits/snn/mnist/gen_mnist_deck.py <digit>`. The Verilog-A models are in
`models/snn/` (the single-neuron review page, `docs/examples/snn_memristor/README.md`, lists them
in full; `synapse_2t2r.va` is new here).
