"""Compare the fitted access switches with PSP103 on the MNIST hidden-2 crossbar, one digit at a time.

    python circuits/snn/transistor/cmp_digits.py <runs-dir> [digit ...]

`<runs-dir>/<digit>/<kind>.out` and `.err` are the va-cli `--tran` output and log of
`scripts/benchmark/xbar_digits.sh`, one per access device: `tx` (PSP103), `txbp` (PSP103 with
`--bypass`), `fitbc` (rungs B and C), `fitc` (rung C) and `fit` (rung A). A run counts only if
its `.err` ends in `exit 0`. The reference is `tx`, or `txbp` where `tx` is missing; the other kinds are compared with it.

As cmp_vm.py does, V_m is sampled 0.5 ns before each cycle ends, and a spike is V(spk) > 1.65 V
at mid-cycle. The threshold margin is V_m - V_th at the cycle's end, where the flip-flop
latches. V_m is still rising there by ~13 mV/ns, so the earlier sample reads several mV low.
The comparator switches over a logistic ~2 mV wide (`comparator.va`, `vw`), and the flip-flop
reads it just before the edge. So a margin within a few mV of 0 does not decide the spike on
its own, and the spike columns are read from the spike outputs, not from the margin.
"""
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cmp_vm import T, at, read  # noqa: E402

VTH = 2.4
N = 16
NAMES = {"tx": "PSP103", "txbp": "PSP103 --bypass", "fitbc": "rungs B+C", "fitc": "rung C",
         "fit": "rung A"}


def vm(run, n, dt):
    return {i: [at(run, (k + 1) * T + dt, f"XN_h2_{i}.vm") for k in range(n)] for i in range(N)}


def spikes(run, n):
    return {i: [k for k in range(n) if at(run, k * T + T / 2, f"h2_{i}") > 1.65] for i in range(N)}


def main():
    root = sys.argv[1]
    digits = sys.argv[2:] or sorted(d for d in os.listdir(root) if d.isdigit())
    worst = {}
    for d in digits:
        path = lambda k, ext: os.path.join(root, d, k + ext)  # noqa: E731

        def ok(k):
            try:
                return open(path(k, ".err")).read().rstrip().endswith("exit 0")
            except OSError:
                return False

        rk = next((k for k in ("tx", "txbp") if ok(k)), None)
        if rk is None:
            print(f"digit {d}: no PSP103 run")
            continue
        ref = read(path(rk, ".out"))
        n = int(round(ref[0][-1] / T))
        rv, rs, re_ = vm(ref, n, -0.5e-9), spikes(ref, n), vm(ref, n, 0.0)
        m = min((abs(re_[i][k] - VTH), i, k) for i in range(N) for k in range(n - 1))
        print(f"digit {d} (reference {NAMES[rk]}): {sum(map(len, rs.values()))} spikes on "
              f"{sum(1 for v in rs.values() if v)} neurons; closest call h2_{m[1]} step {m[2]}, "
              f"{1e3 * (re_[m[1]][m[2]] - VTH):+.1f} mV")
        for k in ("txbp", "fitbc", "fitc", "fit"):
            if k == rk or not ok(k):
                continue
            t = read(path(k, ".out"))
            tv, ts, te = vm(t, n, -0.5e-9), spikes(t, n), vm(t, n, 0.0)
            e = [tv[i][j] - rv[i][j] for i in range(N) for j in range(n)]
            w = max(range(len(e)), key=lambda q: abs(e[q]))
            diff = [i for i in range(N) if rs[i] != ts[i]]
            print(f"   {NAMES[k]}: V_m - reference mean {1e3 * sum(e) / len(e):+.2f}, rms "
                  f"{1e3 * math.sqrt(sum(x * x for x in e) / len(e)):.2f}, worst "
                  f"{1e3 * abs(e[w]):.3f} mV (h2_{w // n} step {w % n}); above / below "
                  f"{sum(x > 0 for x in e)} / {sum(x < 0 for x in e)}; closest call "
                  f"{1e3 * (te[m[1]][m[2]] - VTH):+.1f} mV; "
                  + ("every spike agrees" if not diff else
                     "SPIKES DIFFER on " + ", ".join(f"h2_{i}" for i in diff)))
            worst[k] = max(worst.get(k, 0.0), abs(e[w]))
            worst[k + " differ"] = worst.get(k + " differ", 0) + len(diff)
    for k in ("txbp", "fitbc", "fitc", "fit"):
        if k in worst:
            print(f"{NAMES[k]}: worst {1e3 * worst[k]:.3f} mV over the digits, "
                  f"{worst[k + ' differ']} neuron(s) with a different spike train")
    sys.exit(1 if any(v for k, v in worst.items() if k.endswith("differ")) else 0)


if __name__ == "__main__":
    main()
