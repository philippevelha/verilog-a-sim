"""Check a `va-cli sim` operating point of a gen_iscas.py deck against the `.bench` logic.

Usage (from the repository root):
  python circuits/benchmark/iscas85/check_iscas.py <circuit.bench> <vector> <va-cli output file>
  python circuits/benchmark/iscas85/check_iscas.py tran <circuit.bench> <va-cli --tran output>

**`tran`** (Road to 2.0, item 2) checks a transient the way `check_c17.py tran` checks c17's, for
any circuit: at every timepoint the input vector is read from the reported input voltages; a
timepoint is *settled* once the inputs have been outside 10–90% of Vdd, and the vector unchanged,
for 0.5 ns; at every
settled timepoint every reported gate output must match the logic of that vector. The run must
`--report` every primary input and the gate outputs to check (`report_list` prints them).

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
    # Any net name, not only `g<digits>`: `net_name` keeps a non-numeric ISCAS name (c499's `Gid0`)
    # lower-cased, and a numeric-only pattern silently checked 0 of c499's signals.
    got = {m.group(1): float(m.group(2)) for m in re.finditer(r'V\(([A-Za-z0-9_]+?)_0\) = (\S+) V', text)}
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


SETTLE = 0.5e-9


def report_list(bench):
    """The `--report` argument a `tran` check needs: every primary input and gate output."""
    ins, _, gates = read_bench(bench)
    return ','.join(f'{net_name(x)}_0' for x in list(ins) + [g[0] for g in gates])


def tran(bench, output):
    ins, outs, gates = read_bench(bench)
    in_nets = [net_name(x) for x in ins]
    rows = 0
    last_edge = 0.0
    checked = bad = 0
    vectors = set()
    outputs_seen = set()
    worst_hi, worst_lo = VDD, 0.0
    cache = {}
    prev_vec = None
    for line in open(output, encoding='utf-8', errors='replace'):
        m = re.match(r'\s*t=(\S+)s\s+(.*)', line)
        if not m:
            continue
        rows += 1
        t = float(m.group(1))
        v = {k: float(x) for k, x in re.findall(r'V\(([A-Za-z0-9_]+?)_0\)=(\S+) V', m.group(2))}
        if any(n not in v for n in in_nets):
            raise SystemExit('the output does not report every primary input; use --report with '
                             'report_list(bench)')
        if any(0.1 * VDD < v[n] < 0.9 * VDD for n in in_nets):
            last_edge = t
            continue
        vec = tuple(int(v[n] > VDD / 2) for n in in_nets)
        # A vector change between two samples is an edge even if no sample fell inside the ramp
        # (the integrator puts breakpoints at a PWL source's corners, so one may not).
        if vec != prev_vec:
            last_edge = t
            prev_vec = vec
        if t - last_edge < SETTLE:
            continue
        vectors.add(vec)
        if vec not in cache:
            cache[vec] = {net_name(k): x for k, x in evaluate(ins, gates, list(vec)).items()}
        want = cache[vec]
        for n, got in v.items():
            if n in in_nets or n not in want:
                continue
            checked += 1
            if n in {net_name(o) for o in outs}:
                outputs_seen.add(n)
            if want[n]:
                worst_hi = min(worst_hi, got)
                ok = got > 0.9 * VDD
            else:
                worst_lo = max(worst_lo, got)
                ok = got < 0.1 * VDD
            if not ok:
                bad += 1
                if bad <= 5:
                    print(f't={t:.4e}: {n} = {got:.4g} V, want {want[n]}')
    print(f'{rows} points; {checked} settled gate-output samples over {len(vectors)} distinct input '
          f'vectors ({len(outputs_seen)}/{len(outs)} primary outputs among them): '
          f'{checked - bad} correct; worst high {worst_hi:.6f} V, worst low {worst_lo:.3e} V')
    return bad == 0 and checked > 0 and len(vectors) > 0


if __name__ == '__main__':
    if sys.argv[1] == 'tran':
        sys.exit(0 if tran(sys.argv[2], sys.argv[3]) else 1)
    if sys.argv[1] == 'report':
        print(report_list(sys.argv[2]))
        sys.exit(0)
    sys.exit(0 if main(*sys.argv[1:4]) else 1)
