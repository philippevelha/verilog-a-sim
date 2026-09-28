"""Check the Xuan et al. (IJCNN 2026) spiking-neuron decks against the paper.

Usage (from the repository root, after running a deck with `va-cli ... --tran --report ...`):
  python circuits/snn/check_snn.py codebook
  python circuits/snn/check_snn.py fig4 <va-cli output> [--plot out.png]
  python circuits/snn/check_snn.py fig5 <va-cli output> [--plot out.png]

The circuit runs in continuous time; the paper's model (snnTorch) in discrete timesteps. As the
paper does for its own Fig. 5 (Sec. II-D2), V_m is sampled at the end of each clock cycle, at the
latch edge, and compared with:

  * **the paper's equations** (1)-(7), (9) evaluated here with the deck's own component values:
    beta = C_M / (C_M + C_L), a spike adding sum_i G_i * V_s * dt / (K * C_M), the flip-flop
    latching V_m >= V_th at the end of the cycle, a spike filling the next cycle, and the reset
    (to V_reset, or to 0 for the refractory period, then V_reset) as the paper's Fig. 4 shows it;
  * **what the paper reports:** Fig. 4's spike times (1.50 us and 3.75 us), and Fig. 5's
    model-level V_m and output spikes. The Fig. 5 model-level V_m below is not read off the
    figure: it is V[k] = 0.6 V[k-1] + sum w S[k] with the paper's weights and input spikes,
    which reproduces every plotted point (the paper's model trace shows no reset). The
    transistor-level values are read off the figure by eye, to about +-0.05 V.

Exits non-zero if the output spikes differ from the paper's or the circuit departs from the
equations by more than TOL.
"""
import os
import re
import sys

T = 250e-9          # clock period
DT = 125e-9         # integration window (duty 50 %)
CM, CL = 998e-15, 660e-15
VS = 0.1            # V_P - V_ref = V_ref - V_N
VTH, VRESET = 2.4, 1.0
TOL = 0.05          # V: circuit vs equations, per timestep

LEVELS = [8e3, 10.5e3, 15e3, 210e3]


def g(level):
    return 1.0 / LEVELS[level]


FIGS = {
    # K, nref, por, steps, synapses as (Gp level, Gn level, spike steps)
    'fig4': dict(K=3.0, nref=4, por=1, steps=20,
                 syn=[(0, 2, [k for k in range(20) if k % 3 != 0])] * 5,
                 paper_spikes=[6, 15]),
    'fig5': dict(K=0.696, nref=0, por=0, steps=15,
                 syn=[(0, 2, [0, 4, 8, 12]), (1, 3, [1, 5, 9, 13]),
                      (2, 1, [2, 6, 10, 14]), (1, 2, [3, 7, 11])],
                 paper_spikes=[6, 10, 14],
                 weights=[1.10, 1.51, -0.71, 0.60],
                 paper_tl=[1.17, 2.33, 0.72, 0.97, 1.75, 2.70, 0.95, 1.10, 1.85, 2.75, 0.95,
                           1.10, 1.85, 2.75, 0.95]),
}


def equations(cfg):
    """The paper's discrete neuron, Eqs. (1)-(7), (9), with the deck's component values."""
    beta = CM / (CM + CL)
    d = [(g(p) - g(n)) * VS * DT / (cfg['K'] * CM) for p, n, _ in cfg['syn']]
    state = 3 if cfg['por'] else 0          # 0 integrate, 1 spike, 2 refractory, 3 reset
    cnt, v, vs, spikes = 0, 0.0, [], []
    for k in range(cfg['steps']):
        inp = sum(di for di, (_, _, s) in zip(d, cfg['syn']) if k in s)
        if state == 1:
            spikes.append(k)
            v = VRESET if cfg['nref'] == 0 else 0.0
        elif state == 2:
            v = 0.0
        elif state == 3:
            v = VRESET + inp
        else:
            v = beta * v + inp
        vs.append(v)
        # the latch edge at the end of cycle k
        if state == 1:
            state, cnt = (2, cfg['nref']) if cfg['nref'] > 0 else (0, 0)
        elif state == 2:
            cnt -= 1
            state = 3 if cnt == 0 else 2
        elif state in (0, 3):
            state = 1 if v >= VTH else 0
    return vs, spikes


def paper_model(cfg):
    """Fig. 5's model-level trace: V[k] = beta V[k-1] + sum w S[k], as the paper plots it."""
    v, vs = 0.0, []
    for k in range(cfg['steps']):
        v = 0.6 * v + sum(w for w, (_, _, s) in zip(cfg['weights'], cfg['syn']) if k in s)
        vs.append(v)
    return vs


def read(path):
    rows = []
    for line in open(path, encoding='utf-8', errors='replace'):
        m = re.match(r'\s*t=(\S+)s\s+(.*)', line)
        if m:
            vals = {k: float(x) for k, x in re.findall(r'V\((\w+)\)=(\S+) V', m.group(2))}
            rows.append((float(m.group(1)), vals))
    return rows


def at(rows, t, key):
    """Linear interpolation of `key` at time t."""
    for (t0, a), (t1, b) in zip(rows, rows[1:]):
        if t0 <= t <= t1:
            return a[key] + (b[key] - a[key]) * (t - t0) / (t1 - t0) if t1 > t0 else a[key]
    return rows[-1][1][key]


def codebook():
    """Sec. II-C: the differential weights G_p - G_n two cells of the four levels can make."""
    gs = [g(i) * 1e3 for i in range(4)]                      # mS
    vals = sorted({round(a - b, 4) for a in gs for b in gs})
    paper = [0, 0.0298, 0.0583, 0.1205, 0.0286, 0.0907, 0.0621]
    paper = sorted({v for p in paper for v in (p, -p)})
    print(f'levels (mS): {[round(x, 4) for x in gs]}')
    print(f'G_p - G_n: {len(vals)} values {vals}')
    print(f'paper:     {len(paper)} values {paper}')
    # The paper's values are rounded, and not all from the levels it states: 0.1205 and 0.0907 are
    # 1/8k - 1/222k and 1/10.5k - 1/222k, while it gives HRS = 210 kOhm (0.0048 mS, not 0.0047).
    worst = max(abs(a - b) for a, b in zip(vals, paper)) if len(vals) == len(paper) else 1.0
    print(f'worst difference: {worst:.4f} mS (tolerance 0.0005 mS)')
    ok = len(vals) == 13 and worst <= 5e-4
    print('PASS' if ok else 'FAIL')
    sys.exit(0 if ok else 1)


def main():
    if sys.argv[1] == 'codebook':
        codebook()
    fig, path = sys.argv[1], sys.argv[2]
    cfg = FIGS[fig]
    rows = read(path)
    n = cfg['steps']
    # the value the flip-flop latches: just before clk starts to fall at (k+1)T - 0.5 ns. A later
    # sample sees the reset, which acts from the same edge; this one misses the last 0.5 ns of the
    # 125 ns integration window (-0.4 % of that cycle's increment).
    circ_v = [at(rows, (k + 1) * T - 0.5e-9, 'vm') for k in range(n)]
    circ_s = [k for k in range(n) if at(rows, k * T + T / 2, 'spk') > 1.65]
    eq_v, eq_s = equations(cfg)
    cols = ['circuit', 'equations']
    extra = []
    if fig == 'fig5':
        extra = [('paper model', paper_model(cfg)), ('paper circuit (by eye)', cfg['paper_tl'])]
    print(f'{fig}: V_m at the end of each cycle (V)')
    print('  step  ' + '  '.join(f'{c:>10}' for c in cols + [e[0] for e in extra]))
    for k in range(n):
        row = [circ_v[k], eq_v[k]] + [e[1][k] for e in extra]
        mark = '  spike' if k in circ_s else ''
        print(f'  {k:4d}  ' + '  '.join(f'{x:10.3f}' for x in row) + mark)
    worst = max(abs(a - b) for a, b in zip(circ_v, eq_v))
    print(f'output spikes, circuit:   steps {circ_s} (t = {[round(k * T * 1e6, 3) for k in circ_s]} us)')
    print(f'output spikes, equations: steps {eq_s}')
    print(f'output spikes, paper:     steps {cfg["paper_spikes"]}')
    print(f'circuit vs equations: worst |dV| = {worst:.4f} V (tolerance {TOL} V)')
    if fig == 'fig5':
        for name, trace in extra:
            e = max(abs(a - b) for a, b in zip(circ_v, trace))
            print(f'circuit vs {name}: worst |dV| = {e:.3f} V')
    ok = circ_s == cfg['paper_spikes'] == eq_s and worst <= TOL
    print('PASS' if ok else 'FAIL')
    if '--plot' in sys.argv:
        plot(rows, cfg, fig, circ_v, eq_v, extra, sys.argv[sys.argv.index('--plot') + 1])
    sys.exit(0 if ok else 1)


def plot(rows, cfg, fig, circ_v, eq_v, extra, out):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    t = [r[0] * 1e6 for r in rows]
    keys = [k for k in ('clk', 'wl', 'wl1', 'vm', 'spk') if k in rows[0][1]]
    labels = {'clk': 'clk', 'wl': '$S_i$', 'wl1': '$S_1$', 'vm': '$V_m$', 'spk': '$S_j$'}
    fig_, axes = plt.subplots(len(keys) + 1, 1, figsize=(7, 1.5 * (len(keys) + 1)))
    for ax, k in zip(axes, keys):
        ax.plot(t, [r[1][k] for r in rows], lw=1)
        ax.set_ylabel(labels[k])
        if k == 'vm':
            ax.axhline(VTH, ls='--', c='r', lw=0.8)
            ax.axhline(VRESET, ls='--', c='r', lw=0.8)
        for j in range(cfg['steps'] + 1):
            ax.axvline(j * T * 1e6, ls=':', c='r', lw=0.4)
    axes[len(keys) - 1].set_xlabel('time (us)')
    ax = axes[-1]
    steps = list(range(cfg['steps']))
    ax.plot(steps, circ_v, 'o-', label='circuit (va-cli)')
    ax.plot(steps, eq_v, 's--', label='paper equations')
    for name, trace in extra:
        ax.plot(steps, trace, '.:', label=name)
    ax.axhline(VTH, ls='--', c='gray', lw=0.8)
    ax.set_xlabel('timestep')
    ax.set_ylabel('$V_m$ at step end')
    ax.legend(fontsize=7)
    fig_.tight_layout()
    fig_.savefig(out, dpi=150)
    print(f'plot written to {os.path.basename(out)}')


if __name__ == '__main__':
    main()
