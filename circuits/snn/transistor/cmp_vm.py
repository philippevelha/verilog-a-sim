"""Compare two transient runs of the same SNN circuit, a reference (the PSP103 deck) and a test
(the same circuit with a fitted switch), where the neuron reads them.

    python circuits/snn/transistor/cmp_vm.py <ref.out> <test.out> [--vm vm] [--spk spk]

Both files are va-cli `--tran` output. `--vm` and `--spk` take comma-separated node names (or
glob patterns, e.g. 'XN_h2_*.vm'), matched in the reference. As check_snn.py does, V_m is sampled
at the end of each cycle, at (k+1)T - 0.5 ns, the value the flip-flop latches, and a spike is
V(spk) > 1.65 V at mid-cycle. Prints the per-cycle V_m of each node in both runs, the largest
difference, and whether the spike steps agree. Exits 1 if any spike step differs.
"""
import argparse
import bisect
import fnmatch
import re
import sys

T = 250e-9


def read(path):
    ts, rows = [], []
    for line in open(path, encoding="utf-8", errors="replace"):
        m = re.match(r"\s*t=(\S+)s\s+(.*)", line)
        if m:
            ts.append(float(m.group(1)))
            rows.append({k: float(x) for k, x in re.findall(r"V\(([\w.]+)\)=(\S+) V", m.group(2))})
    if not ts:
        raise SystemExit(f"{path}: no transient points")
    return ts, rows


def at(run, t, key):
    """Linear interpolation of `key` at time t."""
    ts, rows = run
    i = bisect.bisect_left(ts, t)
    if i <= 0:
        return rows[0][key]
    if i >= len(ts):
        return rows[-1][key]
    t0, t1 = ts[i - 1], ts[i]
    a, b = rows[i - 1][key], rows[i][key]
    return a + (b - a) * (t - t0) / (t1 - t0) if t1 > t0 else b


def names(run, spec):
    keys = list(run[1][0])
    out = []
    for p in spec.split(","):
        hit = [k for k in keys if fnmatch.fnmatchcase(k, p)]
        if not hit:
            raise SystemExit(f"no node matches {p!r}")
        out += [k for k in hit if k not in out]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ref")
    ap.add_argument("test")
    ap.add_argument("--vm", default="vm")
    ap.add_argument("--spk", default="spk")
    a = ap.parse_args()
    ref, test = read(a.ref), read(a.test)
    n = int(round(min(ref[0][-1], test[0][-1]) / T))
    vms, spks = names(ref, a.vm), names(ref, a.spk)

    worst = (0.0, None, None)
    for v in vms:
        r = [at(ref, (k + 1) * T - 0.5e-9, v) for k in range(n)]
        s = [at(test, (k + 1) * T - 0.5e-9, v) for k in range(n)]
        if len(vms) <= 4:
            print(f"V({v}) at the end of each cycle (V): step, reference, test, test - reference")
            for k in range(n):
                print(f"  {k:4d}  {r[k]:10.6f}  {s[k]:10.6f}  {(s[k] - r[k]) * 1e3:+9.3f} mV")
        for k in range(n):
            if abs(s[k] - r[k]) > worst[0]:
                worst = (abs(s[k] - r[k]), v, k)
    same = True
    for p in spks:
        r = [k for k in range(n) if at(ref, k * T + T / 2, p) > 1.65]
        s = [k for k in range(n) if at(test, k * T + T / 2, p) > 1.65]
        if r != s or len(spks) <= 4:
            print(f"spikes on {p}: reference steps {r}, test steps {s}"
                  + ("" if r == s else "  DIFFER"))
        same &= r == s
    print(f"{len(vms)} V_m node(s), {n} cycles: largest |test - reference| = "
          f"{worst[0] * 1e3:.3f} mV" + (f" (V({worst[1]}), step {worst[2]})" if worst[1] else ""))
    print(f"{len(spks)} spike node(s): " + ("every spike step agrees" if same else "SPIKES DIFFER"))
    sys.exit(0 if same else 1)


if __name__ == "__main__":
    main()
