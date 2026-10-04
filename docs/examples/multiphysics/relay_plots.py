"""Draw the relay decks of circuits/multiphysics/relay_*.net, and check them.

  python docs/examples/multiphysics/relay_plots.py [path/to/va-cli]

runs both decks through va-cli and writes
  docs/examples/multiphysics/relay_bh.svg -- the core's major and minor B-H loops;
  docs/examples/multiphysics/relay_switch.svg -- coil voltage and the three armatures' travel.
It prints the loop figures (remanence, coercivity) and the energy check: the loop area times
the core volume against the integral of (V - R I) I over one period.
"""
import math
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/relay"
MU0, AREA, LC = 1.25663706212e-6, 2e-5, 0.05


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


# --- B-H loops ---------------------------------------------------------------------------------
rows, _ = run("relay_bh", "X1.h,X1.mm,X2.h,X2.mm,p1,p2,c1,c2,th1,th2")
period = [r for r in rows if 0.02 <= r["t"] <= 0.04 + 1e-12]   # the second period
fig, ax = plt.subplots(figsize=(6.0, 4.4))
for k, lab in ((1, "major loop (0.3 A)"), (2, "minor loop (0.02 A)")):
    h = [r[f"Hf(X{k}.h)"] for r in period]
    b = [MU0 * (r[f"Hf(X{k}.h)"] + r[f"Hf(X{k}.mm)"]) for r in period]
    ax.plot([x / 1e3 for x in h], b, label=lab)
    area = sum(0.5 * (h[i] + h[i + 1]) * (b[i + 1] - b[i]) for i in range(len(h) - 1))

    def p_in(r):
        i = r[f"V(c{k})"]
        res = 200 * (1 + 0.00393 * (r[f"Temp(th{k})"] - 293.15))
        return (r[f"V(p{k})"] - res * i) * i

    energy = sum(0.5 * (p_in(period[i]) + p_in(period[i + 1])) * (period[i + 1]["t"] - period[i]["t"])
                 for i in range(len(period) - 1))
    br = [b[i] for i in range(len(h) - 1) if (h[i] > 0) != (h[i + 1] > 0)]
    hc = [h[i] for i in range(len(h) - 1) if (b[i] > 0) != (b[i + 1] > 0)]
    print(f"{lab}: B max {max(b):.4f} T, remanence {', '.join(f'{x:.3f}' for x in br)} T, "
          f"coercivity {', '.join(f'{x:.0f}' for x in hc)} A/m; loop {area * AREA * LC * 1e3:.5f} mJ "
          f"vs electrical {energy * 1e3:.5f} mJ per cycle")
ax.set_xlabel("H (kA/m)")
ax.set_ylabel("B (T)")
ax.set_title("Jiles-Atherton core: major and minor loops")
ax.legend(fontsize=8)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/relay_bh.svg")

# --- switching ---------------------------------------------------------------------------------
rows, events = run("relay_switch", "x1,x2,x3,c")
for e in events:
    print("  " + e)
t = [r["t"] for r in rows]
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.4, 5.4), sharex=True)
ax1.plot(t, [r["V(c)"] for r in rows], color="k", lw=1)
ax1.set_ylabel("coil voltage (V)")
ax1.grid(True, alpha=0.3)
for k, lab in ((1, "20 C, 0.2 mm residual gap"), (2, "85 C, 0.2 mm residual gap"),
               (3, "20 C, 20 um residual gap")):
    ax2.plot(t, [r[f"Pos(x{k})"] * 1e3 for r in rows], label=lab)
ax2.set_ylabel("armature travel (mm)")
ax2.set_xlabel("time (s)")
ax2.legend(fontsize=8, loc="center right")
ax2.grid(True, alpha=0.3)
fig.suptitle("Relay: pickup, hot pickup, release -- and one held by remanence", fontsize=10)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/relay_switch.svg")
print("wrote docs/examples/multiphysics/relay_bh.svg, relay_switch.svg")
