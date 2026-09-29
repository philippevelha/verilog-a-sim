# MNIST through the memristor-crossbar SNN (Xuan et al., IJCNN 2026, Sec. IV-B)

Ten held-out MNIST images put through the SNN of `circuits/snn/` at circuit level. As in the paper,
hidden layer 1 runs in software and its spike trains drive the circuit, which holds hidden 2,
hidden 3 and the output layer: three memristor crossbars (8 832 2T2R synapses) and 138 analog
neurons, 10 973 devices and 2 626 unknowns per image.

**The step-by-step record, with every command, figure, choice and dead end, is
[`LOG.md`](LOG.md).** This page is the summary.

## Results

| label | va-cli wall | timepoints | slots differing from the model, h2 / h3 / out | circuit class | model class |
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

- **The circuit's class equals the quantized model's on all 10**; against the labels 8/10, the
  model's own two errors.
- **40 011 of 40 020 cycle×neuron spike slots agree (99.978 %).** Every mismatch is on an image
  whose model membrane passes within ~2 mV of V_th (LOG.md, Step 10).
- **Wall time: 9.76 h for the ten, 58.6 min per image on average (51.0–63.7 min)**, i7-1185G7,
  8 threads, `va-cli` alone. It follows the timepoint count, ~2 200 per 250 ns cycle, which the
  network's switching activity sets, not the matrix size.
- Model accuracy on 10 000 held-out images: 93.98 % float, 92.87 % post-training quantized,
  **94.04 %** after quantization-aware fine-tuning. The paper reports 97.46 %; see below.

Plots, per image and in summary: `docs/examples/snn_mnist/`.

## Reproduce

```bash
python circuits/snn/mnist/train_mnist.py            # ~9 min; needs torch, scikit-learn, and
                                                    # external/train-{images,labels}-idx*-ubyte
bash circuits/snn/mnist/run_all.sh target/release/va-cli   # ~1 h per image; resumable
python circuits/snn/mnist/summarize.py              # table + summary figure
```
`model.json` (the trained, quantized network and the ten samples' hidden-1 spike trains) is
committed, so the circuit runs need neither the dataset nor torch. On Linux,
`scripts/benchmark/bench_snn_linux.sh /path/to/va-cli [digits]` runs them and records the wall
time of each.

| File | Role |
|---|---|
| `train_mnist.py` | PCA, training with the circuit's own neuron, codebook quantization, export |
| `gen_mnist_deck.py` | one image's three-layer circuit deck |
| `check_mnist.py` | runs a deck (or reads an output), compares spike for spike with the model |
| `run_all.sh` | the ten images, resumable, retries a failure up to 3 times |
| `summarize.py` | the table above and `summary.png` |
| `model.json`, `results.jsonl`, `train_log.txt`, `run_log.txt` | the trained network and the records |

## Choices the paper leaves open, and limitations

- **The neuron trained is the circuit's**, cycle for cycle (integrate in the spike's own cycle,
  latch at the cycle's end, spike in the next cycle, hold at V_reset through it), not snnTorch's
  `Leaky`, so the circuit computes what was trained.
- **Input encoding:** the 64 PCA features as a constant current into hidden 1 for 25 cycles (the
  paper does not say how its inputs become spikes). 29 cycles are simulated so the last input's
  effect reaches the output.
- **Quantization-aware fine-tuning** (5 epochs) on top of the paper's post-training quantization; it
  recovers the 1.1 points quantization cost. λ per layer by least squares; K from λ (Eq. 3).
- **Split:** the first 50 000 training images train, the last 10 000 are held out; the ten samples
  are the first held-out image of each digit. The paper uses MNIST's separate test set.
- **Accuracy 94.0 % against the paper's 97.5 %**: not chased; the encoding, epoch count and
  surrogate slope are the untested suspects.
- **V_m is not bounded by the supply.** The output neurons' membranes swing from about −6.7 V
  to +8.7 V (plots, bottom panel): the input stage and attenuator are ideal, so nothing limits the
  charge into C_M, exactly as nothing limits V in the model's linear equation. A 3.3 V circuit
  would clip at the rails. **This run shows the circuit computes the trained model's equations;
  it does not show that a 3.3 V implementation of these weights classifies the same.** Bounding
  V_m to [0, V_DD] in both circuit and training, then retraining, is the follow-up (LOG.md, Step 11).
- **Circuit level**, as for the single neuron (`circuits/snn/README.md`): real RRAM conductances and
  capacitors, ideal op-amp, comparator and switches, flip-flops as switched capacitors; the 2T2R
  synapse as one device (`models/snn/synapse_2t2r.va`), checked identical to the two-part form.
