"""Stroboscopic (Poincare) samples from a va-cli run: collect, tabulate, plot.

A model opts in by printing one line per sample, from an `@(timer(start, period))` event so that
every sample is an exact time point:

    $strobe("%m: poincare t=%.15e y=%.12e w=%.12e", $abstime, ...);

i.e. `<instance>: poincare t=<time> <name>=<value> ...`, any number of named values. Lines that do
not match are ignored, so the tool reads a deck's whole stdout. The bubble cells of
models/multiphysics/bubble/ are the first users; any oscillator can be (a driven Duffing beam, a
ring, a neuron).

Subcommands

  run <deck> --model <dir> [--va <va-cli>] [--log out.txt] [-- <extra va-cli flags>]
      Run `va-cli sim <deck> --model <dir> --tran <extra>`, save its stdout to the log (default
      <deck name>.poincare.txt in the current directory) and write <log>.csv. A long run is not
      lost if a plot goes wrong: every other subcommand reads the log.

  table <log> [--csv out.csv]
      The samples as a CSV, one row per sample: instance, index, t, then every value.

  snapshots <log> --var y [--param g=g0+gdot*t] [--x 'index-100'] [--every k] [--out f.svg]
      A waterfall of the profile <var>(x) across instances, one line per sample time, stacked by
      the sweep parameter (Figs. 10-19 of Smereka & Banerjee 1988): the paper's "Poincare
      snapshots". Instances are ordered by `index`: the last integer in the instance name
      (XB17 -> 17), mapped to a position by --x (a Python expression in `index`).

  bifurcation <log> --var y --at XB100,XB150 [--param ...] [--out f.svg]
      Local bifurcation diagrams: <var> against the sweep parameter at the named instances, one
      panel each (Figs. 9 and 18).

--param NAME=EXPR names the sweep parameter and computes it from t (EXPR is Python with `t`, and
any numbers; e.g. g=5e-5*t for `bubble_drive gdot=5e-5`). Without it, plots are against t.
--skip T drops samples before t = T (the initial transient).

Limitations: the expressions are evaluated with Python's eval on the command line's own text: do
not feed it untrusted input. One sample line per instance per sample time is assumed; a model
that prints more than one is tabulated as it prints, and the snapshot pairs them up by time.
"""
import argparse
import csv
import math
import os
import re
import subprocess
import sys
from collections import defaultdict

LINE = re.compile(r"^(\S+): poincare t=(\S+)((?: \S+=\S+)*)\s*$")
PAIR = re.compile(r"(\S+?)=(\S+)")
INDEX = re.compile(r"(\d+)(?!.*\d)")


def parse(text):
    """Every sample line of a run's stdout: a list of (instance, index, t, {name: value})."""
    rows = []
    for line in text.splitlines():
        m = LINE.match(line)
        if not m:
            continue
        inst, t, rest = m[1], float(m[2]), m[3]
        idx = INDEX.search(inst)
        vals = {k: float(v) for k, v in PAIR.findall(rest)}
        rows.append((inst, int(idx[1]) if idx else 0, t, vals))
    return rows


def load(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return parse(f.read())


def write_csv(rows, path):
    names = sorted({k for r in rows for k in r[3]})
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["instance", "index", "t", *names])
        for inst, idx, t, vals in rows:
            w.writerow([inst, idx, repr(t), *(repr(vals.get(n, math.nan)) for n in names)])


def param_fn(spec):
    """'g=5e-5*t' -> ('g', f(t)); None -> ('t', identity)."""
    if not spec:
        return "t", lambda t: t
    name, expr = spec.split("=", 1)
    code = compile(expr, "<param>", "eval")
    return name.strip(), lambda t: eval(code, {"math": math, "__builtins__": {}}, {"t": t})


def by_time(rows, var, skip, xmap):
    """{t: (x[], v[])} with instances sorted by position."""
    frames = defaultdict(list)
    xcode = compile(xmap, "<x>", "eval")
    for inst, idx, t, vals in rows:
        if t < skip or var not in vals:
            continue
        x = eval(xcode, {"math": math, "__builtins__": {}}, {"index": idx})
        frames[round(t, 9)].append((x, vals[var]))
    return {t: tuple(map(list, zip(*sorted(f)))) for t, f in sorted(frames.items())}


def _plt():
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    return plt


def snapshots(rows, var, param, xmap="index", skip=0.0, every=1, out="snapshots.svg",
              title=None, height=None):
    """The paper's Poincare-snapshot waterfall; returns the figure's path."""
    plt = _plt()
    pname, pf = param_fn(param)
    frames = list(by_time(rows, var, skip, xmap).items())[::every]
    if not frames:
        sys.exit(f"no `{var}` samples after t = {skip}")
    ps = [pf(t) for t, _ in frames]
    lo = min(min(v) for _, (_, v) in frames)
    hi = max(max(v) for _, (_, v) in frames)
    span = (hi - lo) or 1.0
    # Each frame is raised by its parameter; the profiles' own height is `height` x the parameter
    # range (default: a third of it), so that neighbouring frames overlap the way the paper's do.
    prange = (max(ps) - min(ps)) or 1.0
    scale = (height if height is not None else 0.33) * prange / span
    fig, ax = plt.subplots(figsize=(6.4, 5.4))
    for z, ((t, (x, v)), p) in enumerate(reversed(list(zip(frames, ps)))):
        base = p
        yy = [base + (val - lo) * scale for val in v]
        ax.fill_between(x, base, yy, color="white", zorder=z, lw=0, rasterized=True)  # hide those behind
        ax.plot(x, yy, color="k", lw=0.4, zorder=z + 0.5, rasterized=True)
    ax.set_xlabel("position")
    ax.set_ylabel(f"{pname}  (each profile raised by it; {var} from {lo:.3g} to {hi:.3g} spans "
                  f"{scale * span:.3g})", fontsize=8)
    ax.set_title(title or f"Poincaré snapshots of {var}", fontsize=10)
    fig.tight_layout()
    fig.savefig(out, dpi=130)   # the curves are rasterised inside an SVG: thousands of them
    plt.close(fig)
    return out


def bifurcation(rows, var, at, param, skip=0.0, out="bifurcation.svg", title=None):
    """Local bifurcation diagrams at the named instances; returns the figure's path."""
    plt = _plt()
    pname, pf = param_fn(param)
    fig, axes = plt.subplots(len(at), 1, figsize=(6.4, 1.3 + 1.1 * len(at)), sharex=True,
                             squeeze=False)
    for ax, inst in zip(axes[:, 0], at):
        pts = [(pf(t), vals[var]) for i, _, t, vals in rows if i == inst and t >= skip
               and var in vals]
        if not pts:
            sys.exit(f"no `{var}` samples for {inst}")
        ax.plot(*zip(*pts), ",", color="k", rasterized=True)
        ax.set_ylabel(var)
        ax.text(0.01, 0.85, inst, transform=ax.transAxes, fontsize=8)
    axes[-1, 0].set_xlabel(pname)
    fig.suptitle(title or f"Local bifurcation diagrams of {var}", fontsize=10)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def run(deck, model, va, log, extra):
    cmd = [va, "sim", deck, "--model", model, "--tran", *extra]
    with open(log, "w", encoding="utf-8") as f:
        r = subprocess.run(cmd, stdout=f, stderr=subprocess.PIPE, text=True, encoding="utf-8")
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed ({r.returncode}):\n{r.stderr}")
    return load(log)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("deck")
    r.add_argument("--model", required=True)
    r.add_argument("--va", default=os.path.join("target", "release", "va-cli"))
    r.add_argument("--log")
    t = sub.add_parser("table")
    t.add_argument("log")
    t.add_argument("--csv")
    for name in ("snapshots", "bifurcation"):
        s = sub.add_parser(name)
        s.add_argument("log")
        s.add_argument("--var", required=True)
        s.add_argument("--param")
        s.add_argument("--skip", type=float, default=0.0)
        s.add_argument("--out", default=f"{name}.svg")
        s.add_argument("--title")
        if name == "snapshots":
            s.add_argument("--x", default="index")
            s.add_argument("--every", type=int, default=1)
            s.add_argument("--height", type=float)
        else:
            s.add_argument("--at", required=True)
    argv = sys.argv[1:] if argv is None else list(argv)
    extra = argv[argv.index("--") + 1:] if "--" in argv else []
    a = ap.parse_args(argv[:argv.index("--")] if "--" in argv else argv)
    if a.cmd == "run":
        log = a.log or os.path.splitext(os.path.basename(a.deck))[0] + ".poincare.txt"
        rows = run(a.deck, a.model, os.path.abspath(a.va) if os.path.exists(a.va) else a.va,
                   log, extra)
        write_csv(rows, log + ".csv")
        print(f"{len(rows)} samples from {len({r[0] for r in rows})} instance(s): {log}, {log}.csv")
    elif a.cmd == "table":
        rows = load(a.log)
        write_csv(rows, a.csv or a.log + ".csv")
        print(f"{len(rows)} samples -> {a.csv or a.log + '.csv'}")
    elif a.cmd == "snapshots":
        print(snapshots(load(a.log), a.var, a.param, a.x, a.skip, a.every, a.out, a.title,
                        a.height))
    else:
        print(bifurcation(load(a.log), a.var, a.at.split(","), a.param, a.skip, a.out, a.title))


if __name__ == "__main__":
    main()
