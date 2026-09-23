"""Check a `va-cli sim` operating point of a gen_iscas.py deck against the `.bench` logic.

Usage (from the repository root):
  python circuits/benchmark/iscas85/check_iscas.py <circuit.bench> <vector> <va-cli output file>

Every `V(g<N>_0)` the output reports (a gate's driver pin; use `--report` to choose them) is
compared with the logic value of that signal for `<vector>`: a 1 must be above 90% of Vdd and a
0 below 10%. Exits non-zero on any mismatch, or if nothing was checked.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_iscas import VDD, evaluate, net_name, read_bench  # noqa: E402


def main(bench, vector, output):
    ins, outs, gates = read_bench(bench)
    val = evaluate(ins, gates, [int(c) for c in vector])
    by_net = {net_name(sig): v for sig, v in val.items()}
    text = open(output, encoding='utf-8', errors='replace').read()
    got = {m.group(1): float(m.group(2)) for m in re.finditer(r'V\((g\d+)_0\) = (\S+) V', text)}
    bad = 0
    worst_hi, worst_lo = VDD, 0.0
    for net, v in sorted(got.items()):
        want = by_net.get(net)
        if want is None:
            continue
        if want:
            worst_hi = min(worst_hi, v)
            ok = v > 0.9 * VDD
        else:
            worst_lo = max(worst_lo, v)
            ok = v < 0.1 * VDD
        if not ok:
            bad += 1
            print(f'{net}: {v:.4g} V, want {want}')
    n_out = sum(1 for o in outs if net_name(o) in got)
    print(f'{len(got)} signals checked ({n_out}/{len(outs)} primary outputs): '
          f'{len(got) - bad} correct; worst high {worst_hi:.6f} V, worst low {worst_lo:.3e} V')
    return bad == 0 and len(got) > 0


if __name__ == '__main__':
    sys.exit(0 if main(*sys.argv[1:4]) else 1)
