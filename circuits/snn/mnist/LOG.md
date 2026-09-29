# MNIST through the memristor-crossbar SNN: lab notebook

A running record, in the order things happened, of reproducing the MNIST case study of Xuan et
al., IJCNN 2026 (`references/`, Sec. IV-B, Figs. 9–10) with verilog-a-sim: every command, every
measured result, every choice with its reason, and every dead end. Figures are from the run at the
time; nothing here is edited afterwards except to append.

Started 2026-09-28 at verilog-a-sim 2.1.0+1 (commit ddfc274), Windows 11, i7-1185G7 (4 cores /
8 threads), on mains power.

---

## Step 0 — The request, and what the paper does

**Request:** put 10 images of the MNIST dataset (`external/train-images.idx3-ubyte`, added by the
user, together with `external/train-labels.idx1-ubyte`) through the SNN circuit.

**What the paper does for MNIST (Sec. IV-B, Fig. 9):**
- each 28×28 image flattened to 784, scaled to [0, 1] (÷255), PCA to 64 features;
- network 64 inputs → hidden 1 (64) → hidden 2 (64) → hidden 3 (64) → output (10);
- trained in snnTorch, Adam, learning rate 10⁻³; V_th = 2.4 V, V_reset = 1 V, β = 0.6, refractory
  disabled;
- weights quantized to the crossbar's 9-value codebook (Eq. 13);
- **only hidden 2, hidden 3 and the output layer are simulated at transistor level**, driven by
  the 64 spike trains hidden 1 produces in software;
- 1000 balanced test samples: 97.46 % post-quantized model, 96.78 % transistor level;
- clock 4 MHz (T = 250 ns), inference time 6.75 µs = (N + L − 1)·T, so N + L − 1 = 27 cycles.

**What the paper does not say, and has to be chosen here:**
- how the 64 PCA features become input to hidden 1 (encoding);
- N (timesteps) exactly: with L = 3 simulated layers, 27 cycles gives N = 25 — taken;
- the scaling λ per layer (Eq. 12) and hence K per layer;
- how a class is read out (spike count assumed: the paper's ECG discussion mentions "nearly
  identical spike counts" causing mismatches, which implies count-based readout);
- the train/test split beyond "a balanced subset of 1000 test samples".

## Step 1 — Tools

```
python -c "import torch, sklearn, numpy"   # torch 2.9.1+cpu, scikit-learn 1.7.2, numpy 2.2.0
```
snnTorch is not installed. **Choice: not installed; the neuron is written in plain PyTorch.**
Reason: the training model should be the *circuit's* neuron cycle for cycle (circuits/snn/README.md,
check_snn.py), not snnTorch's `Leaky`, so that the circuit computes exactly what was trained. The
paper's own Fig. 5 shows its model and circuit differ at the reset step (model shows no reset);
training on the circuit's behaviour removes that source of mismatch by construction.

The circuit's neuron, as measured on Figs. 4–5 (`circuits/snn/`):
- a spike arriving in cycle k is integrated in cycle k: V[k] = β·V[k−1] + Σ w·S_in[k];
- the flip-flop latches V[k] ≥ V_th at the end of cycle k: the neuron's own spike fills cycle k+1
  (every layer delays its output by one cycle);
- through its own spike cycle V_m is held at V_reset and inputs are ignored;
- V starts at 0.

## Step 2 — Training script

`circuits/snn/mnist/train_mnist.py` (header comment documents all of the below).

Choices:
- **split:** file images 0–49 999 train, 50 000–59 999 held out (never trained on). The paper
  uses MNIST's separate 10 000-image test set, which is not in `external/`.
- **encoding:** the 64 PCA features, through hidden 1's weights and bias, as a constant input
  current every cycle for N = 25 cycles. Hidden 1 is software in the paper too.
- **no bias** on the three crossbar layers (a crossbar has no bias line).
- **cycles simulated:** 25 input cycles + 1 (hidden 1's own delay) + 3 (one per crossbar layer)
  = 29, so every output spike caused by the last input cycle is counted.
- **loss:** cross-entropy on the output spike counts; **readout:** argmax of spike counts.
- **surrogate gradient:** fast sigmoid, slope 5 V⁻¹ (V_th is 2.4 V, so the slope is per volt).
- **quantization:** per layer, the λ minimizing Σ(w − λ·q(w/λ))² over a grid (Eq. 12–13); then,
  if post-training quantization loses accuracy, a few epochs of quantization-aware fine-tuning
  (straight-through estimator) — a departure from the paper's plain post-training quantization,
  to be reported separately if used.
- **ten samples:** the first held-out image of each digit 0–9 (one per class, as the paper's
  test subset is balanced).
- seeds fixed (torch 0, PCA random_state 0).

Smoke test (no training, checks the pipeline end to end):
```
python circuits/snn/mnist/train_mnist.py --epochs 0 --qat-epochs 0
held-out accuracy, float weights: 0.0988           <- chance, as expected untrained
held-out accuracy, post-training quantized: 0.0991
```

## Step 3 — A merged 2T2R cell for the full-size crossbars

The three crossbar layers hold 64×64 + 64×64 + 10×64 = **8 832 synapses**. Built from the Fig. 4/5
parts (two `rram` cells in series with two `vswitch` access devices), each synapse adds two internal
nodes: ~17 700 unknowns of pure wiring. **Choice:** `models/snn/synapse_2t2r.va`, the same two
series paths (cell + switch on-resistance, gated by the word line) as one device with no internal
node. It is the same circuit with the two internal nodes eliminated algebraically, not an
approximation.

**Check** (the Fig. 5 deck with its four synapses swapped for merged cells, everything else equal):
```
separate parts: 74 devices, 80 unknowns     merged: 62 devices, 72 unknowns
per-step |dV_m| separate vs merged: max 0.000e+00 V (identical at the 1 µV printed resolution)
output spikes: separate [6, 10, 14], merged [6, 10, 14]
```
Dead end on the way: the level parameters were first called `lp`/`ln`, and `ln` is a reserved word
(the natural-log function), a parse error. Renamed to `levp`/`levn`.

## Step 4 — Deck generator and checker

`circuits/snn/mnist/gen_mnist_deck.py <digit>` writes the three-layer circuit for one sample:
- word lines `h1_0..h1_63`: hidden 1's spike trains from the model, as PWL sources, high for a
  whole cycle with 1 ns edges (the waveform a neuron's own output has);
- crossbars hidden1→hidden2 (64×64), hidden2→hidden3 (64×64), hidden3→output (10×64): one
  `synapse_2t2r` per crosspoint, its (G_p, G_n) level pair realizing the quantized weight:
  0 → (HRS, HRS); ±0.0298 mS → (8 k, 10.5 k); ±0.0583 → (8 k, 15 k); ±0.0907 → (10.5 k, 210 k);
  ±0.1205 → (8 k, 210 k), swapped for a negative weight. A zero weight still gets a cell;
- neurons: `circuits/snn/neuron.inc` without the refractory flip-flop chain (refractory is
  disabled for MNIST in the paper), as one `.subckt` per layer carrying that layer's
  K = V_s·Δt/(λ·C_M) (Eq. 3). Each neuron's spike output (an ideal E-source buffer) is the next
  layer's word line;
- 29 cycles of 250 ns, `.tran 0.5n 7.25u UIC` (V_m starts at 0, as in training).

`circuits/snn/mnist/check_mnist.py <digit>` runs it, reads every spike output at mid-cycle, and
compares layer by layer with the quantized model's spike trains for the same sample, then reports
output spike counts, predicted class (argmax), label, wall time; appends to `results.jsonl`.

Checked in the netlist code first: `gnd` is global inside `.subckt`, nested subcircuits are
allowed, internal nets are named by instance path (`XN_o_3.vm`) — `crates/va-netlist/src/spice.rs`.

**Size and speed probe** (untrained smoke-test weights, the deck cut to 2 cycles, run while the
training used the CPU):
```
circuit: 10973 device(s) (9936 compiled), 2626 unknown(s) (1589 net(s) + 1037 auxiliary row(s))
linear solve: sparse LU; 1002 timepoints; 39.6 s wall
```
So ~20 s and ~500 timepoints per cycle (the 0.5 ns floor of the validated Fig. 4/5 decks, kept for
consistency), ~10 min per image for 29 cycles with the CPU shared, less alone. Kept as is: the ten
images run sequentially in the background (~1–1.5 h).

## Step 5 — Training run

```
python -u circuits/snn/mnist/train_mnist.py --epochs 15 --qat-epochs 5   # log: train_log.txt
```
~22–29 s per epoch on 8 threads; loss 1.71 → 0.20 over 15 float epochs, 0.20 → 0.18 over 5
quantization-aware ones. Held-out accuracy (images 50 000–59 999, 10 000 images):

| weights | accuracy |
|---|---|
| float | 93.98 % |
| post-training quantized to the 9-value codebook (Eq. 13) | 92.87 % |
| after 5 epochs of quantization-aware fine-tuning (straight-through) | **94.04 %** |

λ per layer (least-squares fit of the codebook to each layer's weights, before fine-tuning):
11.798, 11.338, 10.776 V/mS → K = 1.0617, 1.1047, 1.1623 (hidden 2, hidden 3, output).

**Below the paper's 97.46 %.** Not chased here: the aim is the circuit reproducing the model. Likely
contributors, none tested: the input encoding (constant current, our choice); 15 epochs; the
surrogate slope; our held-out split instead of MNIST's test set. The quantization-aware step is a
departure from the paper's plain post-training quantization, and it recovers the 1.1-point loss.

The ten samples (first held-out image of each digit), in the quantized model:

| digit | sample | model class | output counts (0–9) | spikes h1/h2/h3/out |
|---|---|---|---|---|
| 0 | 50018 | 0 | 12 0 2 1 1 3 5 0 1 0 | 129/165/201/25 |
| 1 | 50016 | 1 | 0 11 0 7 2 1 0 0 7 0 | 119/164/193/28 |
| 2 | 50011 | 2 | 1 0 13 5 0 0 0 0 0 0 | 149/252/168/19 |
| 3 | 50000 | 3 | 0 3 6 10 0 0 0 0 2 0 | 134/235/178/21 |
| 4 | 50005 | **9** | 1 0 0 0 7 0 0 6 0 9 | 142/167/149/23 |
| 5 | 50006 | 5 | 0 0 0 0 0 13 0 0 0 0 | 136/201/250/13 |
| 6 | 50002 | 6 | 2 0 2 0 4 0 11 0 0 0 | 184/218/192/19 |
| 7 | 50021 | 7 | 0 1 0 1 1 0 0 9 0 3 | 131/184/179/15 |
| 8 | 50001 | 8 | 0 0 0 3 0 0 0 0 12 0 | 104/169/142/15 |
| 9 | 50003 | **4 (tie 4/9, 12 each)** | 0 0 0 2 12 0 0 4 0 12 | 123/179/189/30 |

8/10 correct in the model; the 9 is a tie broken toward the lower index by argmax, the kind of
near-equal count the paper names as its main source of mismatches (Sec. IV-A).

## Step 6 — The ten circuit runs

```
for d in 0..9: gen_mnist_deck.py d; check_mnist.py d target/release/va-cli.exe --plot plots/digit<d>.png
# log: run_log.txt; one JSON line per sample: results.jsonl
```

**Wall time, first sample (digit 0):** started 20:33:26. At 20:53, still running after 20 min: a
live check showed 5 120 s of CPU in ~21 min of wall (≈4 cores busy on average, the parallel
device evaluation) and 563 MB of memory. So the 2-cycle probe of Step 4 underestimated: it ran on
*untrained* weights, where almost nothing spikes, while the trained network switches hundreds of
spike lines per cycle (64 word lines + 138 neuron outputs, each edge a breakpoint the integrator
resolves). Nothing else CPU-heavy was run on the machine during the sample runs, to keep these
timings clean.

## Step 7 — Linux benchmark script (user request, during Step 6)

`scripts/benchmark/bench_snn_linux.sh /path/to/va-cli [digits…]`, run from the root of a repository
checkout (the v2.0.0 archive has no SNN models, decks or weights; its `va-cli` is enough to run
them). Per sample it times **the va-cli run alone** (`date +%s.%N` around it; deck generation and
checking are outside the timed span), and records the timepoint count, exit code and the output's
SHA-256, then checks the saved output against the model
(`check_mnist.py <d> --from <out> --wall <s>`, added for this). Options: `THREADS=n`, `KEEP=1`.

`check_mnist.py` changes made for this: `--from/--wall` (check an output produced and timed
elsewhere), `report` (print the `--report` list), and the per-cycle lookup changed from a linear
scan to a binary search (~4 000 lookups over ~10⁴–10⁵ timepoints per sample would otherwise add
minutes of Python time to each check).

**21:18, digit 0 at 45 min wall:** CPU 10 782 → 11 020 s over 60 s of wall (~4 cores), working set
1.16 GB and growing. The simulator holds the whole waveform in memory, 2 626 unknowns × 8 B ≈ 21 kB
per timepoint, so ~55 000 timepoints so far — against ~700 per cycle on the Fig. 4/5 decks and
~500 per cycle in the (untrained) probe. The cost is set by the trained network's switching
activity, not by the matrix size. Letting digit 0 finish: its timepoint count and wall time are
the measurement.

### Result, digit 0 (sample 50018) — finished 21:36

| | |
|---|---|
| **va-cli wall time** | **3 761.7 s (62.7 min)**, 20:33:26 → 21:36, nothing else heavy running |
| timepoints | 63 214 (≈2 180 per cycle; the card's floor was 14 501) |
| peak memory seen | ≥1.16 GB (waveform held in memory, ~21 kB per timepoint) |
| circuit | 10 973 devices (9 936 compiled), 2 626 unknowns, sparse LU |
| hidden 2 | 1 856 / 1 856 spike slots agree (165 spikes each) |
| hidden 3 | 1 855 / 1 856 agree — **one spike fewer** in the circuit (200 vs 201) |
| output | 290 / 290 agree; counts `[12,0,2,1,1,3,5,0,1,0]` in both |
| class | circuit 0, model 0, label 0 |

The one missing hidden-3 spike did not propagate to the output. Not yet diagnosed; the likely cause
is a membrane ending a cycle within a few mV of V_th, where the circuit's latch closes ~0.3 ns
before the 125 ns integration window ends (`dff.inc`, −0.2 % of that cycle's increment).

**Projection:** at ~63 min per sample, digits 1–9 need ~9.5 h more on this laptop. Left running.

## Step 8 — Resumable driver (user: "in case of failure at any point restart the point that failed")

`circuits/snn/mnist/run_all.sh [va-cli]`: skips every digit already in `results.jsonl`, retries a
failing digit up to 3 times, and timestamps each attempt in `run_log.txt`. Reason: a background job
in this session was already stopped once by the host for low memory, and each sample's simulation
holds >1.2 GB. The original loop (Step 6) is left running; if it fails or is killed, `run_all.sh`
is launched and resumes at the missing digits.

**Keep-awake (21:51):** a temporary helper (PID 7852, `SetThreadExecutionState(ES_CONTINUOUS |
ES_SYSTEM_REQUIRED)`) stops the laptop from sleeping during the overnight run, so a sleep cannot
inflate the wall times; the display may still turn off. No power setting is changed; it releases
itself and writes a line to `run_log.txt` once no MNIST process is left.

## Step 9 — The overnight run was stopped for low memory; checker fixed; resumed

**What happened:** at ~00:20 the host (Claude Code) stopped the background loop because the
machine was critically low on memory. Digits 0–2 had completed and are recorded; digit 3 was lost
mid-run. Free memory afterwards: 4.7 of 15.7 GB (the rest: the OS and the usual desktop apps).

Results recorded before the stop:

| digit | va-cli wall | timepoints | h2 agree | h3 agree | out agree | class (circuit / model / label) |
|---|---|---|---|---|---|---|
| 0 | 3 761.7 s | 63 214 | 1856/1856 | 1855/1856 | 290/290 | 0 / 0 / 0 |
| 1 | 3 367.2 s | 61 745 | 1856/1856 | 1856/1856 | 290/290 | 1 / 1 / 1 |
| 2 | 3 795.1 s | 68 604 | 1856/1856 | 1856/1856 | 290/290 | 2 / 2 / 2 |

**The memory was largely the checker's.** It captured va-cli's whole stdout through a pipe
(~0.25 GB of text per sample: 148 reported columns × ~65 000 timepoints) and then parsed every row
into a dict of strings — several GB at the end of each sample, on top of va-cli's own ~1.4 GB.

**Fix** (`check_mnist.py`): va-cli now writes to a file (`decks/digit<d>.out`, deleted after the
check unless `--keep`), and the checker streams it once, keeping only what it uses: each spike line
interpolated at mid-cycle from the two bracketing timepoints, and the ten output V_m traces (as
`array('d')`) for the plot. **Validated** on a 4-cycle cut of the digit 3 deck (256 s, 5 292
timepoints, 16.8 MB of output): the streaming values equal the old whole-file method's exactly,
max |difference| 0.0 over all 552 sampled values; the checker's full `--from` path then ran clean
(its results were not recorded: `results.jsonl` was backed up and restored around the test).

Resumed per the user's standing instruction ("in case of failure at any point restart the point
that failed"): `bash circuits/snn/mnist/run_all.sh target/release/va-cli.exe`, which skips digits
0–2 and restarts at digit 3.

## Step 10 — Results: all ten samples

The resumed driver ran digits 3–9 with one attempt each, no failure: 00:28 → 07:12 (the log's
timestamps, `run_log.txt`). Table from `python circuits/snn/mnist/summarize.py`:

| label | va-cli wall | timepoints | slots differing h2 / h3 / out | circuit class | model class |
|---|---|---|---|---|---|
| 0 | 3761.7 s | 63214 | 0 / 1 / 0 | 0 | 0 |
| 1 | 3367.2 s | 61745 | 0 / 0 / 0 | 1 | 1 |
| 2 | 3795.1 s | 68604 | 0 / 0 / 0 | 2 | 2 |
| 3 | 3675.5 s | 67482 | 0 / 0 / 0 | 3 | 3 |
| 4 | 3093.1 s | 57880 | 0 / 0 / 0 | 9 | 9 |
| 5 | 3820.6 s | 69527 | 0 / 0 / 0 | 5 | 5 |
| 6 | 3765.0 s | 68077 | 0 / 0 / 0 | 6 | 6 |
| 7 | 3385.0 s | 62219 | 2 / 5 / 1 | 7 | 7 |
| 8 | 3061.8 s | 56642 | 0 / 0 / 0 | 8 | 8 |
| 9 | 3420.2 s | 63066 | 0 / 0 / 0 | 4 (tie 4/9) | 4 |

- **Class: circuit = quantized model on 10/10**; against the labels 8/10, the model's own two
  errors (the 4 read as 9 by both; the 9 an exact 12–12 tie between 4 and 9 in both, which argmax
  breaks toward 4).
- **Spikes: 40 011 / 40 020 cycle×neuron slots agree (99.978 %)**; eight of ten samples agree on
  every slot.
- **Wall time (va-cli alone): 35 145 s in total (9.76 h); mean 3 514.5 s (58.6 min) per sample;
  range 3 061.8–3 820.6 s**; mean 63 846 timepoints (~2 200 per 250 ns cycle), i7-1185G7,
  8 threads, mains, keep-awake on, nothing else heavy running. Wall time tracks the timepoint count
  (fewest points, digit 8: 56 642 in 3 062 s; most, digit 5: 69 527 in 3 821 s), about 55 ms per
  timepoint at 2 626 unknowns and 9 936 compiled devices.

**Where the mismatches come from (measured, not assumed).** The quantized model was recomputed in
plain Python from `model.json` (codes, λ, hidden-1 spikes) — reproducing its recorded spikes
exactly on all ten samples — and each image's closest approach of any V_m to V_th was measured:

| label | closest approaches to V_th (mV) | slots within 5 mV | circuit mismatches |
|---|---|---|---|
| 0 | 1.91, 2.00, 2.27 | 5 | 1 |
| 1 | 2.27, 2.27, 2.27 | 5 | 0 |
| 2 | 4.61, 5.22, 11.71 | 1 | 0 |
| 3 | 2.27, 2.27, 2.27 | 5 | 0 |
| 4 | 4.18, 4.29, 6.31 | 2 | 0 |
| 5 | 10.98, 11.83, 12.16 | 0 | 0 |
| 6 | 2.27, 10.09, 10.98 | 1 | 0 |
| 7 | **0.69**, 1.91, 2.11 | 3 | **8** |
| 8 | 14.69, 18.97, 20.88 | 0 | 0 |
| 9 | 1.70, 3.49, 4.75 | 3 | 0 |

Every mismatch is on an image whose model trajectory passes within ~2 mV of V_th, and the image
with the closest approach of all (digit 7, 0.69 mV) is the one with the most: one flipped decision
changes that neuron's later cycles and the layers downstream (2 + 5 + 1 slots). Images passing
within 1.7–2.3 mV without a mismatch (1, 3, 9) show the circuit's offset has a sign: it moves a
near-threshold decision only one way. Its sources are known and small: the latch sampling
~0.3 ns before the 125 ns integration window closes (`dff.inc`) and the 10 Ω access-switch
on-resistance in series with each cell. Not further reduced: at this size it never changed a
class, and the paper reports the same kind of near-threshold mismatch (Sec. IV-A).

The keep-awake helper releases itself once no MNIST process is left.

## Step 11 — A limitation the plots show: V_m is not bounded by the supply

Looking at the per-image plots (`docs/examples/snn_mnist/digit<d>.png`, bottom panel): the output
neurons' V_m swings from about **−6.7 V to +8.7 V** (digit 7), far outside a 3.3 V supply. The
circuit's input stage and attenuator are ideal (`models/snn/sl_stage.va`), so nothing limits the
charge delivered to C_M — exactly as nothing limits V in the model's linear equation, which is why
circuit and model agree. A real 3.3 V circuit, like the paper's transistor-level one, would clip
V_m at the rails, and the trained network's large per-spike steps (λ·G up to ~1.4 V for one
synapse, with many synapses active at once) and negative excursions would then behave differently.

**So this reproduction establishes that the circuit computes the trained model's equations, spike
for spike; it does not establish that a 3.3 V implementation of these weights classifies the same.**
Not addressed here. The fix would bound V_m to [0, V_DD] in both places: a clamp in the circuit's
membrane (or a saturating current source in `sl_stage.va`) and the same clip in the training model,
then retraining so the network learns within the rails.

## Step 12 — Linux benchmark result (2026-09-29)

`scripts/benchmark/bench_snn.txt`: all ten samples on an AMD Ryzen 9 9950X (32 threads, Ubuntu
26.04, `performance` governor, load average 1.33 before the run) with the **v2.0.0** release
`va-cli` (sha256 `7eac3716…`) on checkout 08a9772. Every sample exits 0, and against the Windows
run of Step 10 (`results.jsonl`, a 2.1.0 build) **every digit has the same timepoint count, the same
per-layer spike-slot agreement, the same output spike counts and the same class** (40 011 / 40 020
slots; the same nine mismatches). So the 2.1.0 persistence fix does not touch these decks, as the
script's header claims, and the result holds across x86-64 Linux and Windows.

va-cli wall time: **7 579.6 s (2.1 h) for the ten**, mean 758 s, range 663.7–830.7 s, against
35 145 s on the i7-1185G7 — 4.64x overall, 4.5–5.0x per digit. The Windows run recorded no output
hashes, so the Linux SHA-256s in `bench_snn.txt` are the first ones, for the next machine to match.
