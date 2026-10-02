"""How far can the transient tolerances be loosened on the transistor neuron before the answer moves?

Runs `fig5_tx.net` (the Fig. 5 neuron, every block as SG13G2 transistors) at the default
tolerances -- the reference -- and then loosens one tolerance at a time, each sweep continuing
until the worst V_m difference from the reference exceeds 10 % (or the run fails):

  * `--reltol`: the step controller's relative truncation-error budget (default 1e-3): fewer
    timepoints;
  * `--newton-reltol`: each step's Newton convergence tolerance (default 1e-9): fewer
    iterations per timepoint;
  * `--abstol`: the step controller's absolute floor (default 1e-6, in each unknown's units):
    for the current-valued unknowns (|x| ~ uA) it, not reltol, sets the budget.

"The answer" is what `check_snn.py fig5` reads: V_m at the end of each of the 15 clock cycles
(the latch samples) and the output spike steps. The difference is the worst, over the 15
samples, of |V_m - V_m,ref| / |V_m,ref| (every reference sample is above 0.8 V, so the ratio is
well defined). Also recorded: wall time, timepoints, exit code.

    python circuits/snn/transistor/tolerance_study.py [--va-cli target/release/va-cli]
    python circuits/snn/transistor/tolerance_study.py --plot   # the figure, from results.jsonl

Resumable: each finished run is a line in `tolerance/results.jsonl`, and a run already there is
not repeated. Raw outputs go to `tolerance/*.out` (not committed, ~2.5 MB each).
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
DECK = "circuits/snn/transistor/fig5_tx.net"
OUT = os.path.join(HERE, "tolerance")
RESULTS = os.path.join(OUT, "results.jsonl")
T, N = 250e-9, 15
LIMIT = 0.10
SWEEPS = {
    "reltol": (1e-3, [2e-3, 5e-3, 1e-2, 2e-2, 5e-2, 0.1, 0.2, 0.5, 1.0]),
    "newton-reltol": (1e-9, [1e-7, 1e-5, 1e-4, 1e-3, 1e-2, 3e-2, 0.1, 0.3, 1.0]),
    "abstol": (1e-6, [1e-5, 1e-4, 1e-3, 1e-2, 3e-2, 0.1, 0.3, 1.0]),
}


def samples(path):
    """V_m at each latch instant and the output spike steps, exactly as `check_snn.py fig5` reads
    them (V_m at (k+1)T - 0.5 ns; a spike at step k if V(spk) > 1.65 V at kT + T/2; both
    interpolated linearly between accepted points), streamed from a va-cli output file."""
    want = sorted([((k + 1) * T - 0.5e-9, "vm", k) for k in range(N)]
                  + [(k * T + T / 2, "spk", k) for k in range(N)])
    got = {"vm": [None] * N, "spk": [None] * N}
    pat = re.compile(r"\s*t=([-\d.e+]+)s.*V\(vm\)=([-\d.e+]+) V\s+V\(spk\)=([-\d.e+]+)")
    prev, i, points = None, 0, 0
    with open(path) as fh:
        for line in fh:
            m = pat.match(line)
            if not m:
                continue
            points += 1
            cur = (float(m.group(1)), {"vm": float(m.group(2)), "spk": float(m.group(3))})
            while i < len(want) and cur[0] >= want[i][0]:
                tw, key, k = want[i]
                if prev is None or cur[0] == prev[0]:
                    got[key][k] = cur[1][key]
                else:
                    (t0, a), (t1, b) = prev, cur
                    got[key][k] = a[key] + (b[key] - a[key]) * (tw - t0) / (t1 - t0)
                i += 1
            prev = cur
    spikes = [k for k, v in enumerate(got["spk"]) if v is not None and v > 1.65]
    return got["vm"], spikes, points


def run(va_cli, flag, value):
    tag = f"{flag}_{value:g}"
    out = os.path.join(OUT, tag + ".out")
    cmd = [va_cli, "sim", DECK, "--model", "models/snn", "--model",
           "models/sg13g2/psp103/psp103.va", "--tran", "--report", "clk,vm,spk", f"--{flag}",
           f"{value:g}"]
    t0 = time.time()
    with open(out, "w") as fh:
        rc = subprocess.run(cmd, cwd=ROOT, stdout=fh, stderr=subprocess.STDOUT).returncode
    wall = time.time() - t0
    vm, spikes, points = samples(out) if rc == 0 else ([None] * N, [], 0)
    return {"flag": flag, "value": value, "rc": rc, "wall_s": round(wall, 1), "points": points,
            "vm": vm, "spikes": spikes}


def load():
    if not os.path.exists(RESULTS):
        return []
    with open(RESULTS) as fh:
        return [json.loads(l) for l in fh if l.strip()]


def diff(r, ref):
    if r["rc"] != 0 or None in r["vm"]:
        return None
    return max(abs(a - b) / abs(b) for a, b in zip(r["vm"], ref["vm"]))


def study(va_cli):
    os.makedirs(OUT, exist_ok=True)
    done = {(r["flag"], r["value"]): r for r in load()}

    def get(flag, value):
        if (flag, value) not in done:
            r = run(va_cli, flag, value)
            with open(RESULTS, "a") as fh:
                fh.write(json.dumps(r) + "\n")
            done[(flag, value)] = r
        return done[(flag, value)]

    # One reference, the defaults; recorded under the first sweep's flag at its default value.
    ref = get("reltol", SWEEPS["reltol"][0])
    print(f"reference: {ref['wall_s']} s, {ref['points']} points, spikes {ref['spikes']}",
          flush=True)
    for flag, (_, values) in SWEEPS.items():
        for v in values:
            r = get(flag, v)
            d = diff(r, ref)
            print(f"--{flag} {v:g}: rc={r['rc']} {r['wall_s']} s {r['points']} points, "
                  f"spikes {r['spikes']}, worst |dV_m|/V_m = "
                  f"{'-' if d is None else f'{100 * d:.3f} %'}", flush=True)
            if d is None or d > LIMIT:
                break


def plot():
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    rows = load()
    ref = next(r for r in rows if r["flag"] == "reltol" and r["value"] == SWEEPS["reltol"][0])
    # The dataviz reference palette: categorical slots 1-3, the critical status red (with its own
    # marker shape, never colour alone), and its chart ink / grid / surface.
    colors = {"reltol": "#2a78d6", "newton-reltol": "#eb6834", "abstol": "#1baf7a"}
    ink, ink2, grid, surface, critical = "#0b0b0b", "#52514e", "#e1e0d9", "#fcfcfb", "#d03b3b"
    plt.rcParams.update({"text.color": ink, "axes.labelcolor": ink, "xtick.color": ink2,
                         "ytick.color": ink2, "axes.edgecolor": grid})
    labels = {"reltol": "step control, relative (--reltol, default 1e-3)",
              "abstol": "step control, absolute (--abstol, default 1e-6)",
              "newton-reltol": "Newton (--newton-reltol, default 1e-9)"}
    fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(7.5, 7.5), sharex=False, facecolor=surface)
    for flag, (default, values) in SWEEPS.items():
        pts = [ref if flag == "reltol" else dict(ref, flag=flag, value=default)]
        more = sorted((r for r in rows if r["flag"] == flag and r["value"] != default),
                      key=lambda r: r["value"])
        if not more:  # a sweep not run yet: nothing to show beyond the shared reference
            continue
        pts += more
        xs = [p["value"] for p in pts]
        ds = [diff(p, ref) for p in pts]
        ok = [(x, 100 * d) for x, d in zip(xs, ds) if d is not None]
        # The reference itself has zero difference: drawn at a floor so the log axis can show it.
        floor = 1e-4
        ax1.plot([x for x, _ in ok], [max(d, floor) for _, d in ok], "o-", color=colors[flag],
                 label=labels[flag], lw=2, ms=6)
        if ok:
            ax1.annotate(flag, (ok[-1][0], max(ok[-1][1], floor)), xytext=(6, 0),
                         textcoords="offset points", va="center", fontsize=8, color=ink2)
        for p, d in zip(pts, ds):
            if d is not None and p["spikes"] != ref["spikes"]:
                ax1.plot(p["value"], max(100 * d, floor), "x", color=critical, ms=11, mew=2)
        ax2.plot(xs, [p["wall_s"] for p in pts], "o-", color=colors[flag], label=labels[flag],
                 lw=2, ms=6)
        ax2.annotate(flag, (xs[-1], pts[-1]["wall_s"]), xytext=(6, 0),
                     textcoords="offset points", va="center", fontsize=8, color=ink2)
    ax1.axhline(100 * LIMIT, color=ink2, ls="--", lw=1)
    ax1.annotate("10 %", (1, 100 * LIMIT), xycoords=("axes fraction", "data"), xytext=(-4, 4),
                 textcoords="offset points", ha="right", color=ink2, fontsize=9)
    ax1.set_xscale("log")
    ax1.set_yscale("log")
    ax1.set_ylabel("worst |V_m − V_m,ref| / V_m,ref  (%)\nover the 15 latch samples")
    ax1.set_title("fig5_tx.net (68 PSP103): result vs tolerance, one tolerance loosened at a time",
                  fontsize=10)
    ax1.plot([], [], "x", color=critical, ms=9, mew=2, label="output spikes differ from the reference")
    ax1.legend(fontsize=8, loc="center left")
    ax2.set_xscale("log")
    ax2.set_xlabel("tolerance")
    ax2.set_ylabel("wall time (s), i7-1185G7, 8 threads")
    ax2.set_ylim(bottom=0)
    ax2.legend(fontsize=8)
    for ax in (ax1, ax2):
        ax.grid(True, which="major", color=grid, lw=0.6)
        ax.set_facecolor(surface)
        for side in ("top", "right"):
            ax.spines[side].set_visible(False)
    fig.tight_layout()
    path = os.path.join(HERE, "tolerance_study.png")
    fig.savefig(path, dpi=130)
    print("wrote", path)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--va-cli", default=os.path.join(ROOT, "target", "release", "va-cli"))
    ap.add_argument("--plot", action="store_true")
    a = ap.parse_args()
    plot() if a.plot else study(a.va_cli)
    sys.exit(0)
