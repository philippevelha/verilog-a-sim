"""Draw the thermo-optic ring decks of circuits/multiphysics/ring_*.net, and check them.

  python docs/examples/multiphysics/ring_plots.py [path/to/va-cli]

runs both decks through va-cli and writes
  docs/examples/multiphysics/ring_selfheat.svg -- through-port transmission against the laser
      wavelength, swept up and down, at 0.1 mW and 20 mW, with the bistable range's folds;
  docs/examples/multiphysics/ring_bank.svg -- the four rings' drop power (as a fraction of their
      input) and their heater power through the lock, the channel hop and the power step.
It prints every figure the deck headers quote, beside the closed form it is checked against.
The ring parameters below are the model's defaults, which both decks use.
"""
import math
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/ringbank"
C = 2.99792458e8
QI, QE, NG, DNDT, ETA = 1e5, 2.2e4, 4.2, 1.86e-4, 0.5
RTH, RX = 4000.0, 16000.0


def taus(l0):
    w0 = 2 * math.pi * C / (l0 * 1e-9)
    ti, te = 2 * QI / w0, 2 * QE / w0
    return ti, te, 1 / (1 / ti + 2 / te)


def run(deck, report):
    out = subprocess.run(
        [VA, "sim", f"circuits/multiphysics/{deck}.net", "--model", MODEL, "--tran",
         "--report", report],
        capture_output=True, text=True, check=True,
    ).stdout
    rows, events = [], []
    for line in out.splitlines():
        toks = line.split()
        if toks and toks[0].startswith("t="):
            vals = {k: float(v) for k, v in (t.split("=") for t in toks[1:] if "=" in t)}
            vals["t"] = float(toks[0][2:-1])
            rows.append(vals)
        elif line.startswith("X"):
            events.append(line)
    return rows, events


# --- self-heating ------------------------------------------------------------------------------
ti, te, tau = taus(1550.0)
dldt = 1550.0 * DNDT / NG
print(f"tau = {tau * 1e12:.2f} ps, linewidth {1550.0 / (2 * math.pi * C / 1550e-9 * tau / 2):.4f} nm, "
      f"drop max {4 * tau ** 2 / te ** 2:.4f}, through min {(tau / ti) ** 2:.5f}, "
      f"dlambda/dT {dldt:.5f} nm/K")


def folds(p_mw):
    """Laser wavelengths bounding the bistable range: the turning points of lambda_L(T)."""
    k = ETA * (2 / ti) * (2 / te) * p_mw * 1e-3 * RTH
    tmax = k * tau ** 2
    pts = []
    n = 200000
    for i in range(1, n):
        t = tmax * i / n
        d2 = k / t - 1 / tau ** 2
        if d2 <= 0:
            continue
        d = -math.sqrt(d2)                # laser red of the heated resonance
        lres = 1550.0 + dldt * t
        pts.append(1 / (d / (2 * math.pi * C * 1e9) + 1 / lres))
    out = []
    for i in range(1, len(pts) - 1):
        if (pts[i] - pts[i - 1]) * (pts[i + 1] - pts[i]) < 0:
            out.append(pts[i])
    return tmax, out


tmax, fl = folds(20.0)
print(f"20 mW: hottest {tmax:.4f} K (shift {tmax * dldt:.4f} nm), folds at "
      + ", ".join(f"{x:.4f}" for x in fl) + " nm")

rows, _ = run("ring_selfheat", "th1,th2,t2")
T_HALF = 1.0


def wl(t):
    return 1549.7 + 0.9 * t / T_HALF if t <= T_HALF else 1550.6 - 0.9 * (t - T_HALF) / T_HALF


up = [r for r in rows if r["t"] <= T_HALF]
down = [r for r in rows if r["t"] > T_HALF]
fig, ax = plt.subplots(figsize=(6.4, 4.2))
ax.plot([wl(r["t"]) for r in up], [r["Popt(th1)"] / 0.1 for r in up], color="0.5", lw=1,
        label="0.1 mW (either direction)")
ax.plot([wl(r["t"]) for r in up], [r["Popt(th2)"] / 20 for r in up], lw=1.4,
        label="20 mW, sweeping up")
ax.plot([wl(r["t"]) for r in down], [r["Popt(th2)"] / 20 for r in down], lw=1.4, ls="--",
        label="20 mW, sweeping down")
for x in fl:
    ax.axvline(x, color="0.7", lw=0.8, ls=":")
ax.ticklabel_format(axis="x", useOffset=False)
ax.set_xlabel("laser wavelength (nm)")
ax.set_ylabel("through-port transmission")
ax.set_title("Self-heating: the resonance is dragged, then snaps (folds dotted)")
ax.legend(fontsize=8, loc="lower left")
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/ring_selfheat.svg")


def jump(seg, rising):
    best, at = 0.0, None
    for a, b in zip(seg, seg[1:]):
        d = (b["Popt(th2)"] - a["Popt(th2)"]) * (1 if rising else -1)
        if d > best:
            best, at = d, wl(b["t"])
    return at


lo = min(up, key=lambda r: r["Popt(th1)"])
print(f"X1: through min {lo['Popt(th1)'] / 0.1:.5f} at {wl(lo['t']):.4f} nm")
print(f"X2: up-sweep jump at {jump(up, True):.4f} nm, down-sweep snap at "
      f"{jump(down, False):.4f} nm, hottest {max(r['Temp(t2)'] for r in rows):.4f} K")

# --- the bank ----------------------------------------------------------------------------------
rows, events = run("ring_bank", "in1,in2,in3,in4,dr1,dr2,dr3,dr4,u1,u2,u3,u4")
for e in events:
    print("  " + e)
LAMS = [1549.75, 1550.50, 1551.30, 1552.15]


def predicted(lasers, powers):
    """Heater powers (mW) with every ring held where its drop is 40 % of its input."""
    temps, pabs = [], []
    for l0, ll, p in zip(LAMS, lasers, powers):
        ti_, te_, tau_ = taus(l0)
        dmax = 4 * tau_ ** 2 / te_ ** 2
        d = -math.sqrt(dmax / 0.4 - 1) / tau_          # blue side: resonance below the laser
        lres = 1 / (1 / ll - d / (2 * math.pi * C * 1e9))
        temps.append((lres - l0) / (l0 * DNDT / NG))
        pabs.append(ETA * (te_ / ti_) * 0.4 * p)         # mW absorbed at the lock point
    g = [[0.0] * 4 for _ in range(4)]
    for i in range(4):
        g[i][i] = 1 / RTH
    for i in range(3):
        g[i][i] += 1 / RX
        g[i + 1][i + 1] += 1 / RX
        g[i][i + 1] = g[i + 1][i] = -1 / RX
    return [sum(g[i][j] * temps[j] for j in range(4)) * 1e3 - pabs[i] for i in range(4)]


phases = [
    ("before 30 ms", 0.0299, [1550.0, 1550.8, 1551.6, 1552.4], [1, 1, 1, 1]),
    ("30-60 ms", 0.0599, [1550.2, 1550.8, 1551.6, 1552.4], [1, 1, 1, 1]),
    ("after 60 ms", 0.0999, [1550.2, 1550.8, 1551.6, 1552.4], [1, 1, 4, 1]),
]
for name, t_at, lasers, powers in phases:
    r = next(x for x in rows if x["t"] >= t_at)
    sim = [r[f"V(u{k})"] for k in range(1, 5)]
    pre = predicted(lasers, powers)
    print(f"{name:>13}: heaters " + "  ".join(f"{s:.4f}/{p:.4f}" for s, p in zip(sim, pre))
          + "  mW (simulated/closed form)")

t = [r["t"] * 1e3 for r in rows]
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.4, 5.6), sharex=True)
for k in range(4):
    ax1.plot(t, [r[f"Popt(dr{k + 1})"] / r[f"Popt(in{k + 1})"] for r in rows],
             label=f"ring {k + 1}")
    ax2.plot(t, [r[f"V(u{k + 1})"] for r in rows], label=f"ring {k + 1}")
ax1.axhline(0.4, color="0.6", lw=0.8, ls="--")
ax1.set_ylabel("drop / input")
ax1.legend(fontsize=8, ncol=4)
ax1.grid(True, alpha=0.3)
ax2.set_ylabel("heater power (mW)")
ax2.set_xlabel("time (ms)")
ax2.grid(True, alpha=0.3)
for x, txt in ((30, "ch. 1 hops +0.2 nm"), (60, "ch. 3: 1 -> 4 mW")):
    for a in (ax1, ax2):
        a.axvline(x, color="0.8", lw=0.8)
    ax2.text(x + 1, ax2.get_ylim()[1] * 0.92, txt, fontsize=8)
fig.suptitle("Ring bank: four locks against crosstalk and self-heating", fontsize=10)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/ring_bank.svg")
print("wrote docs/examples/multiphysics/ring_selfheat.svg, ring_bank.svg")
