"""Train the MNIST SNN of Xuan et al. (IJCNN 2026, Sec. IV-B, Fig. 9) with the circuit's own
neuron, quantize its crossbar layers to the RRAM codebook, and export everything the circuit
decks need.

Usage (from the repository root; needs numpy, scikit-learn, torch):
  python circuits/snn/mnist/train_mnist.py [--epochs 15]

Reads external/train-images.idx3-ubyte and external/train-labels.idx1-ubyte (MNIST's 60 000
training images; external/ is not committed). Writes circuits/snn/mnist/model.json.

The flow is the paper's (Fig. 6): pixels / 255, PCA to 64 features, a 64-64-64-64-10 network
trained with Adam at the paper's learning rate (1e-3), V_th = 2.4 V, beta = 0.6, V_reset = 1 V,
refractory disabled, then nearest-neighbour quantization of the weights to the crossbar's
9-value codebook (Eq. 13) with a scaling lambda per layer (Eq. 12).

**The neuron is the circuit's, cycle for cycle** (circuits/snn/README.md, check_snn.py), not
snnTorch's `Leaky`, so that what is trained is what the circuit computes:
  * a spike arriving in cycle k is integrated in cycle k: V[k] = beta V[k-1] + sum w S_in[k];
  * the flip-flop latches V[k] >= V_th at the end of cycle k, so the neuron's own spike fills
    cycle k+1: each layer delays its output by one cycle;
  * through its spike cycle, V_m is held at V_reset and inputs are ignored;
  * V starts at 0.
Held-out data: images 50 000-59 999 of the file, never trained on. The first 50 000 train.

Limitations: the first hidden layer runs in software, as in the paper (its spikes are the
circuit's stimuli), and keeps a bias; the crossbar layers have none (the crossbar has no bias
line). Input encoding (the PCA features as a constant current into hidden layer 1 every
timestep) is our choice: the paper does not say how its MNIST inputs become spikes.
"""
import argparse
import json
import os
import struct
import time

import numpy as np
import torch
from sklearn.decomposition import PCA

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
OUT = os.path.join(ROOT, 'circuits', 'snn', 'mnist', 'model.json')

VTH, BETA, VRESET = 2.4, 0.6, 1.0
N_STEPS = 25                   # timesteps of input (the paper's 6.75 us at 4 MHz: N + L - 1 = 27)
N_CROSSBAR = 3                 # hidden 2, hidden 3, output: each delays its spikes one cycle
N_CYCLES = N_STEPS + 1 + N_CROSSBAR   # hidden 1's own delay too: every output spike lands
LEVELS_OHM = [8e3, 10.5e3, 15e3, 210e3]
G = [1.0 / r for r in LEVELS_OHM]      # S
# The reduced codebook (Sec. II-C): 0, +-0.0298, +-0.0583, +-0.0907, +-0.1205 mS, each as a
# (G_p level, G_n level) pair of RRAM levels. 0 is two HRS cells.
PAIRS = {0: (3, 3), 1: (0, 1), 2: (0, 2), 3: (1, 3), 4: (0, 3)}
CODEBOOK_POS = [G[p] - G[n] for p, n in (PAIRS[i] for i in range(5))]
CODEBOOK = sorted({s * c for c in CODEBOOK_POS for s in (1, -1)})  # 9 values, S


def read_idx(path):
    with open(path, 'rb') as f:
        magic, n = struct.unpack('>II', f.read(8))
        if magic == 2051:
            rows, cols = struct.unpack('>II', f.read(8))
            return np.frombuffer(f.read(), dtype=np.uint8).reshape(n, rows * cols)
        return np.frombuffer(f.read(), dtype=np.uint8)


class Spike(torch.autograd.Function):
    """Heaviside forward, fast-sigmoid surrogate backward (slope in 1/V)."""
    slope = 5.0

    @staticmethod
    def forward(ctx, x):
        ctx.save_for_backward(x)
        return (x >= 0).float()

    @staticmethod
    def backward(ctx, g):
        (x,) = ctx.saved_tensors
        return g / (1.0 + Spike.slope * x.abs()) ** 2


def run_layers(currents_fn, weights, n_cycles, record=False):
    """The circuit's neuron, layer after layer, over `n_cycles`.

    `currents_fn(k)` gives hidden layer 1's input current at cycle k (a [batch, 64] tensor, or
    None past the input window). `weights` are the crossbar layers' [out, in] matrices, in volts
    per spike. Returns every layer's spike trains [layer][cycle] -> [batch, n].
    """
    layers = 1 + len(weights)
    v = [None] * layers
    fire = [None] * layers      # latched at the end of the previous cycle: spike in this cycle
    spikes = [[] for _ in range(layers)]
    for k in range(n_cycles):
        prev_out = None
        for li in range(layers):
            if li == 0:
                cur = currents_fn(k)
            else:
                cur = prev_out @ weights[li - 1].t()
            if v[li] is None:
                v[li] = torch.zeros_like(cur)
                fire[li] = torch.zeros_like(cur)
            s_now = fire[li]                       # this layer's spike, filling cycle k
            integrated = BETA * v[li] + cur
            v[li] = s_now.detach() * VRESET + (1 - s_now.detach()) * integrated
            fire[li] = Spike.apply(v[li] - VTH) * (1 - s_now.detach())
            spikes[li].append(s_now)
            prev_out = s_now
    return spikes


class Net(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.fc1 = torch.nn.Linear(64, 64)
        self.w = torch.nn.ParameterList(
            [torch.nn.Parameter(torch.randn(o, i) * 0.6) for o, i in ((64, 64), (64, 64), (10, 64))])
        self.quant = None          # per-layer lambda when quantization-aware

    def weights(self):
        if self.quant is None:
            return list(self.w)
        # straight-through: quantized forward, full-precision gradient
        return [w + (quantize(w.detach(), lam) - w).detach() for w, lam in zip(self.w, self.quant)]

    def forward(self, x):
        cur = self.fc1(x)
        spk = run_layers(lambda k: cur if k < N_STEPS else torch.zeros_like(cur),
                         self.weights(), N_CYCLES)
        return spk


def quantize(w, lam):
    """Eq. (13): nearest codebook conductance to w / lambda, returned in volts (lambda * G)."""
    cb = torch.tensor(CODEBOOK, dtype=w.dtype)
    idx = (w.unsqueeze(-1) / lam - cb).abs().argmin(-1)
    return cb[idx] * lam


def best_lambda(w):
    """The lambda (V/S) minimising the quantization error of one layer (Eq. 12's scaling)."""
    wmax = float(w.abs().max())
    best = None
    for f in np.linspace(0.2, 1.5, 131):
        lam = f * wmax / max(CODEBOOK)
        err = float(((quantize(w, lam) - w) ** 2).sum())
        if best is None or err < best[1]:
            best = (lam, err)
    return best[0]


def accuracy(model, x, y, batch=1000):
    correct = 0
    with torch.no_grad():
        for i in range(0, len(x), batch):
            spk = model(x[i:i + batch])
            counts = torch.stack(spk[-1]).sum(0)
            correct += int((counts.argmax(1) == y[i:i + batch]).sum())
    return correct / len(x)


def train(model, x, y, epochs, lr=1e-3, batch=128, tag=''):
    opt = torch.optim.Adam(model.parameters(), lr=lr)
    gen = torch.Generator().manual_seed(0)
    for ep in range(epochs):
        t0 = time.time()
        perm = torch.randperm(len(x), generator=gen)
        total = 0.0
        for i in range(0, len(x), batch):
            idx = perm[i:i + batch]
            spk = model(x[idx])
            counts = torch.stack(spk[-1]).sum(0)
            loss = torch.nn.functional.cross_entropy(counts, y[idx])
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += float(loss) * len(idx)
        print(f'{tag} epoch {ep + 1}/{epochs}: loss {total / len(x):.4f} ({time.time() - t0:.0f} s)',
              flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--epochs', type=int, default=15)
    ap.add_argument('--qat-epochs', type=int, default=5)
    args = ap.parse_args()
    torch.manual_seed(0)
    torch.set_num_threads(os.cpu_count() or 1)

    imgs = read_idx(os.path.join(ROOT, 'external', 'train-images.idx3-ubyte')).astype(np.float64) / 255
    labels = read_idx(os.path.join(ROOT, 'external', 'train-labels.idx1-ubyte')).astype(np.int64)
    tr, te = slice(0, 50000), slice(50000, 60000)
    pca = PCA(n_components=64, random_state=0).fit(imgs[tr])
    feats = pca.transform(imgs).astype(np.float32)
    x, y = torch.from_numpy(feats), torch.from_numpy(labels)

    model = Net()
    train(model, x[tr], y[tr], args.epochs, tag='float')
    acc_float = accuracy(model, x[te], y[te])
    print(f'held-out accuracy, float weights: {acc_float:.4f}')

    lams = [float(best_lambda(w.detach())) for w in model.w]
    model.quant = lams
    acc_ptq = accuracy(model, x[te], y[te])
    print(f'held-out accuracy, post-training quantized: {acc_ptq:.4f}  lambda (V/mS) '
          f'{[round(float(l) / 1e3, 3) for l in lams]}')
    acc_qat = None
    if args.qat_epochs > 0:
        train(model, x[tr], y[tr], args.qat_epochs, lr=2e-4, tag='quantization-aware')
        acc_qat = accuracy(model, x[te], y[te])
        print(f'held-out accuracy, after quantization-aware fine-tuning: {acc_qat:.4f}')

    # Ten held-out samples, one per digit: the first of each class in the held-out range.
    picks = [50000 + int(np.flatnonzero(labels[te] == d)[0]) for d in range(10)]
    with torch.no_grad():
        spk = model(x[picks])
    samples = []
    for b, idx in enumerate(picks):
        per_layer = [torch.stack(layer)[:, b, :].int().tolist() for layer in spk]   # [cycle][neuron]
        counts = torch.stack(spk[-1])[:, b, :].sum(0).int().tolist()
        samples.append({'index': idx, 'label': int(labels[idx]), 'model_counts': counts,
                        'model_class': int(np.argmax(counts)), 'spikes': per_layer})

    codes = []
    for w, lam in zip(model.w, lams):
        q = quantize(w.detach(), lam)
        cb = torch.tensor(CODEBOOK, dtype=q.dtype)
        codes.append(((q.unsqueeze(-1) / lam - cb).abs().argmin(-1)).int().tolist())
    json.dump({
        'paper': 'Xuan et al., IJCNN 2026, Sec. IV-B',
        'vth': VTH, 'beta': BETA, 'vreset': VRESET, 'n_steps': N_STEPS, 'n_cycles': N_CYCLES,
        'levels_ohm': LEVELS_OHM, 'codebook_S': CODEBOOK,
        'codebook_pairs': [[PAIRS[abs(i - 4)][0], PAIRS[abs(i - 4)][1]] if i >= 4 else
                           [PAIRS[4 - i][1], PAIRS[4 - i][0]] for i in range(9)],
        'lambda_V_per_S': lams,
        'layers': ['hidden2 (64x64)', 'hidden3 (64x64)', 'output (10x64)'],
        'codes': codes,
        'accuracy': {'float': acc_float, 'post_training_quantized': acc_ptq,
                     'quantization_aware': acc_qat, 'held_out': 10000},
        'samples': samples,
    }, open(OUT, 'w'))
    print('written', OUT)


if __name__ == '__main__':
    main()
