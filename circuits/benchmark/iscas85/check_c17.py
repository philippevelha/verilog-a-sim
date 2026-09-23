"""Check ISCAS'85 c17 against NAND logic (run from the repository root).

  python circuits/benchmark/iscas85/check_c17.py truth <va-cli binary>
      Solve the .op for all 32 input vectors (decks written by gen_c17.py) and check all six
      gates against the truth table.
  python circuits/benchmark/iscas85/check_c17.py tran <va-cli output file>
      Check a `sim c17_tran.net --tran --report g1_0,g2_0,g3_0,g6_0,g7_0,g22_0,g23_0` output:
      both outputs against NAND logic wherever the inputs have been still for >= 0.5 ns.

Needs external/code/psp103/vacode/psp103.va and the c7552 drop's model cards, which are not in
the repository (external/ is not tracked).
"""
import os
import re
import subprocess
import sys
import time

VDD = 1.8
GATES = [(10, 1, 3), (11, 3, 6), (16, 2, 11), (19, 11, 7), (22, 10, 16), (23, 16, 19)]
HERE = os.path.dirname(os.path.abspath(__file__))


def truth(exe):
    deck = os.path.join(HERE, 'c17_tt.net')
    bad = 0
    worst_hi, worst_lo = VDD, 0.0
    times = []
    for v in range(32):
        bits = format(v, '05b')
        subprocess.run([sys.executable, os.path.join(HERE, 'gen_c17.py'), deck, 'op', bits],
                       check=True)
        t0 = time.time()
        r = subprocess.run([exe, 'sim', deck, '--model', 'external/code/psp103/vacode/psp103.va',
                            '--report', ','.join(f'g{g}_0' for g, _, _ in GATES)],
                           capture_output=True, text=True)
        times.append(time.time() - t0)
        got = {int(m.group(1)): float(m.group(2))
               for m in re.finditer(r'V\(g(\d+)_0\) = (\S+) V', r.stdout + r.stderr)}
        val = dict(zip([1, 2, 3, 6, 7], (int(b) for b in bits)))
        for g, a, b in GATES:
            val[g] = 0 if (val[a] and val[b]) else 1
        for g, _, _ in GATES:
            if g not in got:
                print(f'{bits}: g{g} missing: {r.stderr.strip().splitlines()[-2:]}')
                bad += 1
                continue
            if val[g]:
                worst_hi = min(worst_hi, got[g])
                ok = got[g] > 0.9 * VDD
            else:
                worst_lo = max(worst_lo, got[g])
                ok = got[g] < 0.1 * VDD
            if not ok:
                bad += 1
                print(f'{bits}: g{g} = {got[g]:.4g} V, want {val[g]}')
    os.remove(deck)
    print(f'32 vectors x 6 gates: {192 - bad}/192 correct; worst high {worst_hi:.6f} V, '
          f'worst low {worst_lo:.3e} V; wall {min(times):.2f}-{max(times):.2f} s per vector')
    return bad == 0


def tran(path):
    rows = []
    for line in open(path, encoding='utf-8', errors='replace'):
        m = re.match(r'\s*t=(\S+)s\s+(.*)', line)
        if m:
            vals = {k: float(v) for k, v in re.findall(r'V\(g(\d+)_0\)=(\S+) V', m.group(2))}
            rows.append((float(m.group(1)), vals))
    last_edge = 0.0
    checked = bad = 0
    vectors = set()
    worst_hi, worst_lo = VDD, 0.0
    for t, v in rows:
        if any(0.1 * VDD < v[str(n)] < 0.9 * VDD for n in (1, 2, 3, 6, 7)):
            last_edge = t
            continue
        if t - last_edge < 0.5e-9:
            continue
        x = {n: int(v[str(n)] > VDD / 2) for n in (1, 2, 3, 6, 7)}
        for g, a, b in GATES:
            x[g] = 0 if (x[a] and x[b]) else 1
        vectors.add(tuple(x[n] for n in (1, 2, 3, 6, 7)))
        for g in (22, 23):
            got = v[str(g)]
            checked += 1
            if x[g]:
                worst_hi = min(worst_hi, got)
                ok = got > 0.9 * VDD
            else:
                worst_lo = max(worst_lo, got)
                ok = got < 0.1 * VDD
            if not ok:
                bad += 1
                if bad <= 5:
                    print(f't={t:.4e}: g{g} = {got:.4g} V, want {x[g]}')
    print(f'{len(rows)} points; {checked} settled output samples over {len(vectors)} distinct '
          f'input vectors: {checked - bad} correct; worst high {worst_hi:.6f} V, '
          f'worst low {worst_lo:.3e} V')
    return bad == 0 and checked > 0


if __name__ == '__main__':
    ok = truth(sys.argv[2]) if sys.argv[1] == 'truth' else tran(sys.argv[2])
    sys.exit(0 if ok else 1)
