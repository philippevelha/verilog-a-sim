"""Draw the Li-ion decks of circuits/multiphysics/li_*.net, and check them.

  python docs/examples/multiphysics/li_plots.py [path/to/va-cli]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/li_discharge.svg -- cell voltage against charge delivered at C/20, 1C
      and 2C, with the open-circuit curve;
  docs/examples/multiphysics/li_calendar.svg -- SEI thickness and lithium lost over two years at
      25, 45 and 60 C, with the closed form;
  docs/examples/multiphysics/li_oven.svg -- cell temperature in three ovens, below and above the
      runaway threshold.
It prints the figures the deck headers quote beside their closed forms.
"""
import math
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/liion"
F, R = 96485.33212, 8.314462618
QN = F * 30555 * 3.05e-6 / 3600          # Ah of graphite sites
QP = F * 51555 * 2.96e-6 / 3600          # Ah of LiCoO2 sites
AREA_N = 3 * 3.05e-6 / 2e-6              # m^2 of graphite surface


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


def u_n(y):
    y = min(max(y, 0.005), 0.995)
    return (0.7222 + 0.1387 * y + 0.029 * math.sqrt(y) - 0.0172 / y + 0.0019 / y ** 1.5
            + 0.2808 * math.exp(0.9 - 15 * y) - 0.7984 * math.exp(0.4465 * y - 0.4108))


def u_p(y):
    y = min(max(y, 0.4), 0.999)
    y2 = y * y
    return ((-4.656 + 88.669 * y2 - 401.119 * y2 ** 2 + 342.909 * y2 ** 3 - 462.471 * y2 ** 4
             + 433.434 * y2 ** 5)
            / (-1 + 18.933 * y2 - 79.532 * y2 ** 2 + 37.311 * y2 ** 3 - 73.083 * y2 ** 4
               + 95.96 * y2 ** 5))


# --- discharge ---------------------------------------------------------------------------------
ocv = []
q = 0.0
while True:
    v = u_p(0.5 + q / QP) - u_n(0.8 - q / QN)
    ocv.append((q, v))
    if v <= 3.0:
        break
    q += 1e-4
print(f"open-circuit capacity to 3.0 V: {q:.4f} Ah (Q_n {QN:.4f} Ah, Q_p {QP:.4f} Ah)")

rows, events = run("li_discharge", "p1,p2,p3,th2,th3")
for e in events:
    print("  " + e)
fig, ax = plt.subplots(figsize=(6.4, 4.2))
ax.plot([a for a, _ in ocv], [b for _, b in ocv], color="0.5", ls="--", lw=1,
        label="open circuit")
cutoff = {}
for e in events:
    name, rest = e.split(":", 1)
    if "cutoff at t=" in rest:
        cutoff[name] = float(rest.split("cutoff at t=")[1].split()[0])
for k, (i, lab) in enumerate(((0.1, "C/20"), (2.0, "1C"), (4.0, "2C")), start=1):
    on = [r for r in rows if 10.0 <= r["t"] <= cutoff.get(f"XL{k}", float("inf"))]
    ax.plot([(r["t"] - 10.0) * i / 3600 for r in on], [r[f"V(p{k})"] for r in on], label=lab)
ax.set_xlabel("charge delivered (Ah)")
ax.set_ylabel("cell voltage (V)")
ax.set_ylim(2.9, 4.25)
ax.set_title("Discharge at three rates (2 Ah cell)")
ax.legend(fontsize=8)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/li_discharge.svg")
print(f"peak temperature rise: 1C {max(r['Temp(th2)'] for r in rows) - 298.15:.1f} K, "
      f"2C {max(r['Temp(th3)'] for r in rows) - 298.15:.1f} K")

# --- calendar ----------------------------------------------------------------------------------
temps = (298.15, 318.15, 333.15)
rows, _ = run("li_calendar", "p1,p2,p3,XN1.dfilm,XN2.dfilm,XN3.dfilm,XN1.sh[0],XN2.sh[0],XN3.sh[0]")
days = [r["t"] / 86400 for r in rows]
fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(8.4, 3.6))
for k, tk in enumerate(temps, start=1):
    kk = 6e-5 * math.exp(5e4 / R * (1 / 298.15 - 1 / tk))
    closed = [math.sqrt(25 + 2 * kk * r["t"]) for r in rows]
    sim = [r[f"Ext(XN{k}.dfilm)"] for r in rows]
    lab = f"{tk - 273.15:.0f} C"
    ax1.plot(days, sim, label=lab)
    ax1.plot(days, closed, color="k", lw=0.6, ls=":")
    lost_sim = [(0.8 - r[f"Th(XN{k}.sh[0])"]) * QN for r in rows]
    ax2.plot(days, [x / 2.0 * 100 for x in lost_sim], label=lab)
    lost_closed = 2.09e4 * AREA_N * 1e-9 * (closed[-1] - 5) * F / 3600
    print(f"calendar {lab}: film {sim[-1]:.4f} nm (closed form {closed[-1]:.4f}), lithium lost "
          f"{lost_sim[-1]:.4f} Ah (closed form {lost_closed:.4f}), OCV "
          f"{rows[-1][f'V(p{k})']:.4f} V")
ax1.set_xlabel("days")
ax1.set_ylabel("SEI thickness (nm)")
ax1.set_title("Film: simulated, closed form dotted", fontsize=9)
ax1.legend(fontsize=8)
ax1.grid(True, alpha=0.3)
ax2.set_xlabel("days")
ax2.set_ylabel("lithium lost (% of 2 Ah)")
ax2.set_title("Capacity consumed by the film", fontsize=9)
ax2.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/li_calendar.svg")

# --- oven --------------------------------------------------------------------------------------
rows, events = run("li_oven", "th1,th2,th3")
for e in events:
    print("  " + e)
fig, ax = plt.subplots(figsize=(6.4, 4.2))
for k, tov in enumerate((403.15, 408.15, 413.15), start=1):
    vals = [r[f"Temp(th{k})"] for r in rows]
    pk = max(range(len(vals)), key=lambda i: vals[i])
    print(f"oven {tov - 273.15:.0f} C: peak {vals[pk]:.2f} K at t = {rows[pk]['t']:.0f} s")
    ax.plot([r["t"] / 3600 for r in rows], [v - 273.15 for v in vals],
            label=f"oven {tov - 273.15:.0f} C")
ax.axhline(407.22 - 273.15, color="0.6", lw=0.8, ls=":")
ax.text(0.1, 407.22 - 273.15 + 8, "threshold 134.07 C", fontsize=8)
ax.set_xlabel("time (h)")
ax.set_ylabel("cell temperature (C)")
ax.set_title("Oven test: below and above the runaway threshold")
ax.legend(fontsize=8)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/li_oven.svg")
print("wrote docs/examples/multiphysics/li_discharge.svg, li_calendar.svg, li_oven.svg")
