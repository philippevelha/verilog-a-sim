"""Where in the 250 ns cycle a fitted switch's V_m error builds up, on the MNIST hidden-2 crossbar.

    python circuits/snn/transistor/bias_phases.py <ref.out> <test.out> <deck.net>

`ref.out` and `test.out` are va-cli `--tran` runs of the same crossbar (gen_xbar_deck.py /
xbar_digits.sh, reporting XN_h2_*.vm and h2_*), the reference with PSP103 and the test with a
fitted switch. `deck.net` is either deck; only its word-line sources (Vh1_*) are read.

Part 1 splits each cycle into phases and sums, over the 16 neurons and the cycles, how much
V_m(test) - V_m(ref) changes in each. The phases partition [kT - 1 ns, (k+1)T - 1 ns): the word
lines switch at kT (a 1 ns ramp) while clk falls (kT - 0.5 .. kT + 0.5 ns), and sl_stage.va passes
the source-line current to V_m only while clk is high, through a tanh 0.05 V wide, so it is
effectively closed ~50 ps after kT. Phases with a reset (a spike at step k in either run) are
skipped where the reset falls in them.

Part 2 fits, by least squares over the neuron-cycles, the V_m change in the first 50 ps after kT
as r * (word lines rising at kT) + f * (word lines falling at kT) + c, for each run separately:
the per-edge kick each model gives V_m while the stage is still integrating.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cmp_vm import T, at, read  # noqa: E402

N = 16
PHASES = [(-1.0, -0.5, "clk high, word lines still"), (-0.5, 0.0, "clk falling"),
          (0.0, 0.05, "word-line edges start, stage closing"), (0.05, 2.5, "edges, stage closed"),
          (2.5, 122.5, "qclk: V_m shares with CL"), (122.5, 127.0, "clk rising"),
          (127.0, 249.0, "integrating, word lines steady")]


def word_lines(deck, n):
    """For each Vh1_* source, whether it is high in each cycle."""
    out = []
    for m in re.finditer(r"^Vh1_\d+ \S+ gnd PWL\((.*)\)", deck, re.M):
        v = m.group(1).split()
        pts = [(float(a.rstrip("n")) * (1e-9 if a.endswith("n") else 1.0), float(b))
               for a, b in zip(v[0::2], v[1::2])]

        def level(t):
            return next((b for (a, b), (c, _) in zip(pts, pts[1:]) if a <= t < c), pts[-1][1])
        out.append([level(k * T + T / 2) > 1.65 for k in range(n)])
    return out


def solve3(rows, y):
    """Least squares for three unknowns, by the normal equations."""
    m = [[sum(r[i] * r[j] for r in rows) for j in range(3)] for i in range(3)]
    b = [sum(r[i] * v for r, v in zip(rows, y)) for i in range(3)]
    for i in range(3):
        for j in range(i + 1, 3):
            f = m[j][i] / m[i][i]
            m[j] = [x - f * z for x, z in zip(m[j], m[i])]
            b[j] -= f * b[i]
    x = [0.0] * 3
    for i in reversed(range(3)):
        x[i] = (b[i] - sum(m[i][j] * x[j] for j in range(i + 1, 3))) / m[i][i]
    return x


def main():
    ref, test = read(sys.argv[1]), read(sys.argv[2])
    deck = open(sys.argv[3]).read()
    n = int(round(min(ref[0][-1], test[0][-1]) / T))
    wl = word_lines(deck, n)
    edges = [(sum(1 for w in wl if w[k] and not w[k - 1]),
              sum(1 for w in wl if w[k - 1] and not w[k])) for k in range(n)]

    tot, cnt = [0.0] * len(PHASES), [0] * len(PHASES)
    kick = {"ref": ([], []), "test": ([], [])}
    for i in range(N):
        v, s = f"XN_h2_{i}.vm", f"h2_{i}"
        spk = {k for k in range(n) for r in (ref, test) if at(r, k * T + T / 2, s) > 1.65}
        for k in range(1, n - 1):
            for j, (a, b, _) in enumerate(PHASES):
                if a < 2.5 and k in spk:
                    continue
                ta, tb = k * T + a * 1e-9, k * T + b * 1e-9
                tot[j] += (at(test, tb, v) - at(test, ta, v)) - (at(ref, tb, v) - at(ref, ta, v))
                cnt[j] += 1
            if k not in spk:
                for name, r in (("ref", ref), ("test", test)):
                    kick[name][0].append((*edges[k], 1.0))
                    kick[name][1].append(at(r, k * T + 0.05e-9, v) - at(r, k * T, v))
    print("V_m(test) - V_m(ref), change per neuron-cycle in each phase (ns from the cycle start):")
    for (a, b, name), t, c in zip(PHASES, tot, cnt):
        print(f"  {a:+6.2f} .. {b:+7.2f}  {name:38s} {1e6 * t / c:+9.1f} uV  ({c})")
    print("V_m kick in the first 50 ps of a cycle, per word-line edge:")
    for name in ("ref", "test"):
        r, f, c = solve3(*kick[name])
        print(f"  {name:4s}  rising {1e6 * r:+8.1f} uV   falling {1e6 * f:+8.1f} uV   "
              f"constant {1e6 * c:+7.1f} uV")


if __name__ == "__main__":
    main()
