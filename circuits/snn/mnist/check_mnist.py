"""Run one MNIST sample's circuit deck and compare it, spike for spike, with the quantized model.

Usage (from the repository root, after gen_mnist_deck.py):
  python circuits/snn/mnist/check_mnist.py <digit 0-9> --from <va-cli stdout> [--wall <seconds>]
  python circuits/snn/mnist/check_mnist.py report   (the --report list a run needs)
  python circuits/snn/mnist/check_mnist.py <digit 0-9> [path/to/va-cli] [--plot out.png]

Runs va-cli on circuits/snn/mnist/decks/digit<d>.net, reporting every spike output (h2_*, h3_*,
o_*) and the ten output neurons' membrane voltages, and times the run. Each spike output is
read once per cycle, at mid-cycle (a spike fills its whole cycle), and compared with the spike
trains train_mnist.py recorded for the same sample from the quantized model -- the paper's own
comparison (Figs. 5, 10), made per spike rather than per class.

Prints: agreement per layer (spikes matching / total cycle x neuron slots), the output spike
counts from circuit and model, the predicted class (argmax of the counts) and the label.
Appends a JSON line to circuits/snn/mnist/results.jsonl.
"""
import array
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
T = 250e-9


def report_list():
    """What a run must --report: every spike output, and the output neurons' V_m."""
    return ([f'h2_{i}' for i in range(64)] + [f'h3_{i}' for i in range(64)]
            + [f'o_{i}' for i in range(10)] + [f'XN_o_{i}.vm' for i in range(10)])


def main():
    if sys.argv[1] == 'report':
        print(','.join(report_list()))
        return
    digit = int(sys.argv[1])
    va = next((a for a in sys.argv[2:] if 'va-cli' in os.path.basename(a)),
              os.path.join(ROOT, 'target', 'release', 'va-cli'))
    model = json.load(open(os.path.join(HERE, 'model.json')))
    sample = next(s for s in model['samples'] if s['label'] == digit)
    n_cycles = model['n_cycles']
    deck = os.path.join(HERE, 'decks', f'digit{digit}.net')
    if '--from' in sys.argv:
        # an output produced (and timed) elsewhere: scripts/benchmark/bench_snn_linux.sh
        out_path = sys.argv[sys.argv.index('--from') + 1]
        wall = float(sys.argv[sys.argv.index('--wall') + 1]) if '--wall' in sys.argv else float('nan')
        info = []
    else:
        # va-cli writes to a file, not a pipe held in memory: the output of one sample is
        # ~0.25 GB of text, and holding it (and a parsed copy) is what ran the machine out of
        # memory on the first overnight run (LOG.md, Step 9).
        out_path = os.path.join(HERE, 'decks', f'digit{digit}.out')
        t0 = time.time()
        with open(out_path, 'w') as fout, open(out_path + '.err', 'w') as ferr:
            code = subprocess.run([os.path.abspath(va), 'sim', deck, '--model',
                                   os.path.join(ROOT, 'models', 'snn'), '--tran', '--report',
                                   ','.join(report_list())], stdout=fout, stderr=ferr,
                                  cwd=ROOT).returncode
        wall = time.time() - t0
        err = open(out_path + '.err').read()
        if code != 0:
            raise SystemExit(f'va-cli failed ({wall:.0f} s):\n{err[-3000:]}')
        info = [l for l in err.splitlines() if 'circuit:' in l or 'linear solve' in l]

    # Stream the output once, keeping only what is used: each spike line at mid-cycle (a spike
    # fills its cycle), interpolated between the two timepoints that bracket it, and the ten
    # output V_m traces for the plot.
    spike_keys = [f'{p}_{i}' for p, n in (('h2', 64), ('h3', 64), ('o', 10)) for i in range(n)]
    vm_keys = [f'XN_o_{i}.vm' for i in range(10)]
    targets = [k * T + T / 2 for k in range(n_cycles)]
    sampled = []                       # [cycle] -> {key: value}
    vm_t, vm = array.array('d'), [array.array('d') for _ in vm_keys]
    prev_t, prev = None, None
    n_points = 0
    ti = 0
    with open(out_path, encoding='utf-8') as f:
        for line in f:
            m = re.match(r'\s*t=(\S+)s\s+(.*)', line)
            if not m:
                continue
            t = float(m.group(1))
            vals = dict(re.findall(r'V\(([\w.]+)\)=(\S+) V', m.group(2)))
            n_points += 1
            vm_t.append(t)
            for a, key in zip(vm, vm_keys):
                a.append(float(vals[key]))
            while ti < len(targets) and t >= targets[ti]:
                if prev is None or t == prev_t:
                    sampled.append({k: float(vals[k]) for k in spike_keys})
                else:
                    w = (targets[ti] - prev_t) / (t - prev_t)
                    sampled.append({k: float(prev[k]) + (float(vals[k]) - float(prev[k])) * w
                                    for k in spike_keys})
                ti += 1
            prev_t, prev = t, vals
    while len(sampled) < n_cycles:     # a run that ended early: hold its last value
        sampled.append({k: float(prev[k]) for k in spike_keys})
    if '--from' not in sys.argv and '--keep' not in sys.argv:
        os.remove(out_path)

    circ = {}
    for prefix, n in (('h2', 64), ('h3', 64), ('o', 10)):
        circ[prefix] = [[int(sampled[k][f'{prefix}_{i}'] > 1.65) for i in range(n)]
                        for k in range(n_cycles)]
    report = {'digit': digit, 'index': sample['index'], 'wall_s': round(wall, 1),
              'points': n_points, 'circuit_info': info}
    print(f'digit {digit} (sample {sample["index"]}): {n_points} timepoints, {wall:.1f} s')
    for line in info:
        print(' ', line.strip())
    for li, prefix in ((1, 'h2'), (2, 'h3'), (3, 'o')):
        model_tr = sample['spikes'][li]
        same = sum(model_tr[k][i] == circ[prefix][k][i]
                   for k in range(n_cycles) for i in range(len(model_tr[0])))
        total = n_cycles * len(model_tr[0])
        n_model = sum(map(sum, model_tr))
        n_circ = sum(map(sum, circ[prefix]))
        print(f'  {prefix}: {same}/{total} spike slots agree; spikes model {n_model}, circuit {n_circ}')
        report[prefix] = {'agree': same, 'total': total, 'model_spikes': n_model,
                          'circuit_spikes': n_circ}
    counts = [sum(circ['o'][k][i] for k in range(n_cycles)) for i in range(10)]
    pred = max(range(10), key=lambda i: counts[i])
    ties = [i for i in range(10) if counts[i] == counts[pred]]
    print(f'  output counts, circuit: {counts}')
    print(f'  output counts, model:   {sample["model_counts"]}')
    print(f'  predicted: circuit {pred}{" (tie " + str(ties) + ")" if len(ties) > 1 else ""}, '
          f'model {sample["model_class"]}, label {digit}')
    report.update({'circuit_counts': counts, 'model_counts': sample['model_counts'],
                   'circuit_class': pred, 'ties': ties, 'model_class': sample['model_class']})
    with open(os.path.join(HERE, 'results.jsonl'), 'a') as f:
        f.write(json.dumps(report) + '\n')
    if '--plot' in sys.argv:
        plot(vm_t, vm, circ, sample, n_cycles, digit, sys.argv[sys.argv.index('--plot') + 1])


def plot(vm_t, vm, circ, sample, n_cycles, digit, out):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots(3, 1, figsize=(8, 8), gridspec_kw={'height_ratios': [2, 1, 2]})
    for i in range(10):
        ks_c = [k for k in range(n_cycles) if circ['o'][k][i]]
        ks_m = [k for k in range(n_cycles) if sample['spikes'][3][k][i]]
        ax[0].scatter(ks_c, [i] * len(ks_c), marker='|', s=200, c='C0',
                      label='circuit' if i == 0 else None)
        ax[0].scatter(ks_m, [i + 0.3] * len(ks_m), marker='|', s=120, c='C1',
                      label='quantized model' if i == 0 else None)
    ax[0].set_yticks(range(10))
    ax[0].set_ylabel('output neuron')
    ax[0].set_xlabel('cycle')
    ax[0].set_title(f'MNIST digit {digit}: output spikes')
    ax[0].legend(loc='upper right', fontsize=7)
    cc = [sum(circ['o'][k][i] for k in range(n_cycles)) for i in range(10)]
    ax[1].bar([i - 0.2 for i in range(10)], cc, width=0.4, label='circuit')
    ax[1].bar([i + 0.2 for i in range(10)], sample['model_counts'], width=0.4, label='model')
    ax[1].set_xticks(range(10))
    ax[1].set_ylabel('spike count')
    ax[1].legend(fontsize=7)
    t = [x * 1e6 for x in vm_t]
    for i in range(10):
        ax[2].plot(t, list(vm[i]), lw=0.8,
                   c='C3' if i == digit else '0.7', label=f'neuron {digit}' if i == digit else None)
    ax[2].axhline(2.4, ls='--', c='k', lw=0.6)
    ax[2].set_xlabel('time (us)')
    ax[2].set_ylabel('output V_m (V)')
    ax[2].legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(out, dpi=130)


if __name__ == '__main__':
    main()
