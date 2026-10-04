"""Draw the power-grid swing decks of circuits/multiphysics/grid_*.net.

  python docs/examples/multiphysics/grid_plots.py [path/to/va-cli]

runs both decks through va-cli and writes
  docs/examples/multiphysics/grid_equal_area.svg -- the two machines' rotor angles in degrees,
      with the equal-area critical angle and the 180 - d0 limit marked;
  docs/examples/multiphysics/grid_cascade.svg -- the three tie-line flows (computed from the bus
      angles, zero from each line's reported opening) and the two islands' frequency deviation.
It also prints the figures the deck headers quote.
"""
import math
import re
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/powergrid"


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


# --- equal-area -------------------------------------------------------------------------------
rows, events = run("grid_equal_area", "g1,g2")
H, F0, PM, PMAX = 5.0, 50.0, 0.8, 1.6
M = 2 * H / (2 * math.pi * F0)
d0 = math.asin(PM / PMAX)
dcr = math.acos((math.pi - 2 * d0) * math.sin(d0) - math.cos(d0))
tcr = math.sqrt(2 * M * (dcr - d0) / PM)
t = [r["t"] for r in rows]
g1 = [math.degrees(r["Ang(g1)"]) for r in rows]
g2 = [math.degrees(r["Ang(g2)"]) for r in rows]
print(f"equal area: d0 {math.degrees(d0):.2f} deg, d_cr {math.degrees(dcr):.2f} deg, "
      f"t_cr {tcr:.4f} s; G1 peak {max(g1):.2f} deg")
for e in events:
    print("  " + e)
fig, ax = plt.subplots(figsize=(6.4, 4.0))
ax.plot(t, g1, label="G1, cleared after 0.255 s")
ax.plot(t, g2, label="G2, cleared after 0.270 s")
ax.axhline(math.degrees(dcr), ls="--", lw=0.8, color="0.4")
ax.text(0.05, math.degrees(dcr) + 4, f"critical clearing angle {math.degrees(dcr):.1f} deg",
        fontsize=8)
ax.axhline(180 - math.degrees(d0), ls=":", lw=0.8, color="0.4")
ax.text(0.05, 180 - math.degrees(d0) + 4, "180 - d0: no way back", fontsize=8)
ax.axvspan(1.0, 1.0 + tcr, color="0.9")
ax.set_ylim(-60, 400)
ax.set_xlabel("time (s)")
ax.set_ylabel("rotor angle (deg)")
ax.set_title(f"Equal-area criterion: t_cr = {tcr * 1e3:.1f} ms (shaded)")
ax.legend(loc="upper left", fontsize=8)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/grid_equal_area.svg")

# --- cascade ----------------------------------------------------------------------------------
rows, events = run("grid_cascade", "a1,a2,a3,b1,b2,b3,XA1.w,XB1.w")
ties = {"XT1": ("a1", "b1", 2.5), "XT2": ("a2", "b2", 2.0), "XT3": ("a3", "b3", 1.5)}
opened = {}
for e in events:
    m = re.match(r"(XT\d): (?:opened|TRIPPED) at t=([0-9.]+)", e)
    if m and m.group(1) not in opened:
        opened[m.group(1)] = float(m.group(2))
t = [r["t"] for r in rows]
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.4, 5.6), sharex=True)
for name, (a, b, bmax) in ties.items():
    flow = [0.0 if r["t"] >= opened.get(name, 1e30)
            else bmax * math.sin(r[f"Ang({a})"] - r[f"Ang({b})"]) for r in rows]
    ax1.plot(t, flow, label=f"{name} (bmax {bmax} pu)")
    print(f"cascade: {name} carries {flow[0]:.3f} pu at the operating point, "
          f"open from t = {opened.get(name, float('nan')):.4f} s")
ax1.axhline(1.6, ls="--", lw=0.8, color="0.4")
ax1.text(0.1, 1.65, "relay setting 1.6 pu", fontsize=8)
ax1.set_ylabel("tie flow A -> B (pu)")
ax1.legend(fontsize=8)
ax1.grid(True, alpha=0.3)
fa = [r["Om(XA1.w)"] / (2 * math.pi) for r in rows]
fb = [r["Om(XB1.w)"] / (2 * math.pi) for r in rows]
ax2.plot(t, fa, label="area A (generation)")
ax2.plot(t, fb, label="area B (load)")
ax2.set_xlabel("time (s)")
ax2.set_ylabel("frequency deviation (Hz)")
ax2.legend(fontsize=8)
ax2.grid(True, alpha=0.3)
fig.suptitle("Cascade: one tie lost, two tripped, two islands drifting apart", fontsize=10)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/grid_cascade.svg")
print(f"cascade: final frequency deviation A {fa[-1]:+.4f} Hz, B {fb[-1]:+.4f} Hz "
      f"(closed form +-{3.6 / 0.4 / (2 * math.pi):.4f})")
print(f"cascade: {sum('TRIPPED' in e for e in events)} relay trips, "
      f"{sum('pole slip' in e for e in events)} pole-slip reports")
print("wrote docs/examples/multiphysics/grid_equal_area.svg, grid_cascade.svg")
