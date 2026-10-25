"""Draw the bubble-cloud sweeps (circuits/multiphysics/bubble_cloud_sweep_*.net) as the paper does.

  python docs/examples/multiphysics/bubble_cloud_sweeps.py [path/to/va-cli] [--run]

The sweeps are long (minutes to an hour each), so this script reads their logs from
target/bubble_sweeps/, written by scripts/poincare.py as each deck's header says. With --run it
first runs any sweep whose log is missing, all of them in parallel, one thread each. It writes
docs/examples/multiphysics/bubble_sweep_*.svg: Poincare snapshots of the radius or pressure
along the cloud as the drive grows (Smereka & Banerjee 1988, Figs. 10-19) and local bifurcation
diagrams (Figs. 9 and 18). It prints what it measures in each: where the edge bubbles jump or
double, and when the core of the cloud starts to move.
"""
import os
import re
import subprocess
import sys

import numpy as np

sys.path.insert(0, "scripts")
import poincare  # noqa: E402

ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
VA = ARGS[0] if ARGS else os.path.join("target", "release", "va-cli")
LOGS = "target/bubble_sweeps"
DECKS = "circuits/multiphysics"
OUT = "docs/examples/multiphysics"
MODEL = "models/multiphysics/bubble"
X = "index-100"                                  # node p<i> sits at x = i - 100


def deck_sweep(name):
    """(omega, gdot, period) from a sweep deck's drive card."""
    text = open(f"{DECKS}/{name}.net", encoding="utf-8").read()
    m = re.search(r"^XD pb bubble_drive g=\S+ gdot=(\S+) omega=(\S+)", text, re.M)
    gdot, omega = float(m[1]), float(m[2])
    return omega, gdot, 2 * np.pi / omega


def ensure(names):
    missing = [n for n in names if not os.path.exists(f"{LOGS}/{n}.txt")]
    if not missing:
        return
    if "--run" not in sys.argv:
        sys.exit(f"missing logs in {LOGS}: {', '.join(missing)} (run them as the decks' headers "
                 "say, or pass --run)")
    os.makedirs(LOGS, exist_ok=True)
    procs = [subprocess.Popen([sys.executable, "-I", "scripts/poincare.py", "run",
                               f"{DECKS}/{n}.net", "--model", MODEL, "--va", VA,
                               "--log", f"{LOGS}/{n}.txt", "--", "--reltol", "1e-5",
                               "--threads", "1", "--report", "pb"]) for n in missing]
    if any(p.wait() for p in procs):
        sys.exit("a sweep failed")


def profiles(rows, var="y"):
    """(t[], V[sample, node]) with node i - 1 in column i - 1."""
    ts = sorted({r[2] for r in rows})
    k = {t: n for n, t in enumerate(ts)}
    v = np.full((len(ts), 199), np.nan)
    for _, idx, t, vals in rows:
        v[k[t], idx - 1] = vals[var]
    return np.array(ts), v


def period2_onset(y, run=8, amp=0.02):
    """First sample from which `y` alternates for `run` periods: consecutive samples differ by more
    than `amp`, samples two apart by less than a fifth of that. None if it never does."""
    d1 = np.abs(np.diff(y))
    d2 = np.abs(y[2:] - y[:-2])
    ok = (d1[1:] > amp) & (d2 < 0.2 * d1[1:])
    for n in range(run, len(ok) + 1):
        if ok[n - run:n].all():
            return n - run + 2
    return None


def first_change(v, level):
    """First sample at which some bubble moved by more than `level` since the previous period."""
    d = np.nanmax(np.abs(np.diff(v, axis=0)), axis=1)
    return int(np.argmax(d > level)) + 1 if (d > level).any() else None


SWEEPS = {
    "bubble_cloud_sweep_w2": "Figs. 9-10",
    "bubble_cloud_sweep_w085": "Fig. 11",
    "bubble_cloud_sweep_w025": "Figs. 12-13",
    "bubble_cloud_sweep_w062": "Fig. 15",
    "bubble_cloud_sweep_w055": "Figs. 17-18",
    "bubble_cloud_sweep_w05_a01": "Fig. 19",
}
ensure(SWEEPS)

for name, fig in SWEEPS.items():
    rows = poincare.load(f"{LOGS}/{name}.txt")
    omega, gdot, period = deck_sweep(name)
    param = f"g={gdot!r}*t"
    t, y = profiles(rows)
    g_at = (lambda k: f"{gdot * t[k]:.3f}" if k is not None else "never")
    print(f"[{name}] omega = {omega}: {len(rows)} samples. Period 2 from g = "
          f"{g_at(period2_onset(y[:, 0]))} at the edge (x = -99), {g_at(period2_onset(y[:, 99]))} "
          f"at the centre; the first change of a radius by > 0.1 in one period at g = "
          f"{g_at(first_change(y, 0.1))}; the centre first moves by > 0.02 in one period at "
          f"g = {g_at(first_change(y[:, 99:100], 0.02))}")
    frames = len({r[2] for r in rows})
    every = max(1, frames // 120)
    tag = name.replace("bubble_cloud_sweep_", "")
    if tag != "w05_a01":   # Fig. 19 shows the pressure only
        poincare.snapshots(rows, "y", param, X, every=every,
                           out=f"{OUT}/bubble_sweep_{tag}_y.svg",
                           title=f"Radius along the cloud, ω = {omega} ({fig})")
    if tag in ("w025", "w05_a01"):
        poincare.snapshots(rows, "p", param, X, every=every,
                           out=f"{OUT}/bubble_sweep_{tag}_p.svg",
                           title=f"Pressure along the cloud, ω = {omega} ({fig})")
    if tag == "w2":
        # Fig. 9's positions: x = 0, 40, 70, 90, 98 (from the centre to the edge).
        poincare.bifurcation(rows, "y", ["XB100", "XB140", "XB170", "XB190", "XB198"], param,
                             out=f"{OUT}/bubble_sweep_w2_bif.svg",
                             title="Local bifurcation diagrams, ω = 2 (Fig. 9): x = 0, 40, 70, 90, 98")
        late = [r for r in rows if gdot * r[2] >= 3.0]
        poincare.snapshots(late, "y", param, X, every=1, out=f"{OUT}/bubble_sweep_w2_late_y.svg",
                           title="Radius along the cloud past g = 3, ω = 2 (Fig. 10)")
    if tag == "w055":
        # Fig. 18's positions: x = 0, 24, 42, 72, 99.
        poincare.bifurcation(rows, "y", ["XB100", "XB124", "XB142", "XB172", "XB199"], param,
                             out=f"{OUT}/bubble_sweep_w055_bif.svg",
                             title="Local bifurcation diagrams, ω = 0.55 (Fig. 18): x = 0, 24, 42, 72, 99")
print(f"wrote {OUT}/bubble_sweep_*.svg")
