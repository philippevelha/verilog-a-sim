"""Draw the piezo harvester decks of circuits/multiphysics/piezo_*.net, and check them.

  python docs/examples/multiphysics/piezo_plots.py [path/to/va-cli]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/piezo_duffing.svg -- the swept amplitude of the linear and the
      hardening beam, against the harmonic-balance steady state (both branches);
  docs/examples/multiphysics/piezo_storage.svg -- linear and hardening storage capacitors
      charging under four realizations of broadband vibration.
It prints the linear deck's amplitude beside the closed form, the harmonic-balance folds beside
the sweep's jumps, and the storage voltages.
"""
import cmath
import math
import subprocess
import sys

import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/piezo"
M, F0, ZETA, TH, CP, R_LOAD, A = 0.01, 50.0, 0.01, 5e-4, 50e-9, 63662.0, 2.0
W0 = 2 * math.pi * F0
K = M * W0 * W0
C = 2 * ZETA * math.sqrt(K * M)


def run(deck, report):
    out = subprocess.run(
        [VA, "sim", f"circuits/multiphysics/{deck}.net", "--model", MODEL, "--tran",
         "--report", report],
        capture_output=True, text=True, check=True,
    ).stdout
    rows = []
    for line in out.splitlines():
        toks = line.split()
        if toks and toks[0].startswith("t="):
            vals = {k: float(v) for k, v in (t.split("=") for t in toks[1:] if "=" in t)}
            vals["t"] = float(toks[0][2:-1])
            rows.append(vals)
    return rows


def load(w):
    return 1 / (1 / R_LOAD + 1j * w * CP)


# --- linear: power into the load ---------------------------------------------------------------
z = load(W0)
x_cf = -M * A / (K - M * W0 * W0 + 1j * W0 * C + 1j * W0 * TH * TH * z)
v_cf = abs(1j * W0 * TH * x_cf * z)
rows = run("piezo_linear", "p")
seg = [r for r in rows if r["t"] >= 2.8]
energy = sum(0.5 * (seg[i]["V(p)"] ** 2 + seg[i + 1]["V(p)"] ** 2) * (seg[i + 1]["t"] - seg[i]["t"])
             for i in range(len(seg) - 1))
v_sim = math.sqrt(2 * energy / (seg[-1]["t"] - seg[0]["t"]))
print(f"linear: |V| {v_sim:.4f} V (closed form {v_cf:.4f} V), power {v_sim ** 2 / (2 * R_LOAD) * 1e3:.4f} mW "
      f"(closed form {v_cf ** 2 / (2 * R_LOAD) * 1e3:.4f} mW)")


# --- Duffing: harmonic balance and the sweep ---------------------------------------------------
def branches(f, k3):
    w = 2 * math.pi * f
    ce = TH * TH * load(w) * 1j * w
    a_ = K - M * w * w + ce.real
    b_ = w * C + ce.imag
    q = 0.75 * k3
    if q == 0:
        return [M * A / math.hypot(a_, b_)]
    roots = np.roots([q * q, 2 * a_ * q, a_ * a_ + b_ * b_, -(M * A) ** 2])
    return sorted(math.sqrt(r.real) for r in roots if abs(r.imag) < 1e-9 * abs(r) and r.real > 0)


freqs = np.arange(47.0, 57.0, 0.002)
hb = [branches(f, 2e8) for f in freqs]
folds = [freqs[i] for i in range(1, len(hb)) if len(hb[i]) != len(hb[i - 1])]
peak = max((b[-1], f) for b, f in zip(hb, freqs))
print("harmonic balance: folds at " + ", ".join(f"{f:.3f}" for f in folds)
      + f" Hz; upper-branch peak {peak[0] * 1e3:.4f} mm at {peak[1]:.3f} Hz")

rows = run("piezo_duffing", "X1.x,X2.x")
T = 40.0


def freq(t):
    return 48 + 8 * t / T if t <= T else 56 - 8 * (t - T) / T


env = []
w = 0.2
i = 0
for kk in range(int(2 * T / w)):
    t0 = kk * w
    seg = []
    while i < len(rows) and rows[i]["t"] < t0 + w:
        seg.append(rows[i])
        i += 1
    if seg:
        env.append((t0 + w / 2, freq(t0 + w / 2),
                    max(abs(r["Pos(X1.x)"]) for r in seg), max(abs(r["Pos(X2.x)"]) for r in seg)))
up = [e for e in env if e[0] < T]
down = [e for e in env if e[0] > T]
pu = max(up, key=lambda e: e[3])
drop = max(range(1, len(up)), key=lambda j: up[j - 1][3] - up[j][3])
jump = max(range(1, len(down)), key=lambda j: down[j][3] - down[j - 1][3])
print(f"sweep: up-sweep peak {pu[3] * 1e3:.4f} mm at {pu[1]:.3f} Hz, drop between {up[drop - 1][1]:.2f} "
      f"and {up[drop][1]:.2f} Hz; down-sweep jump between {down[jump - 1][1]:.2f} and {down[jump][1]:.2f} Hz")

fig, ax = plt.subplots(figsize=(6.6, 4.4))
ax.plot([e[1] for e in up], [e[3] * 1e3 for e in up], color="C1", lw=1.2, label="hardening, sweeping up")
ax.plot([e[1] for e in down], [e[3] * 1e3 for e in down], color="C1", lw=1.2, ls="--",
        label="hardening, sweeping down")
ax.plot([e[1] for e in up], [e[2] * 1e3 for e in up], color="C0", lw=1.0, label="linear")
for n in range(3):
    pts = [(f, b[n] if len(b) == 3 else (b[0] if n != 1 else None)) for f, b in zip(freqs, hb)]
    pts = [(f, v) for f, v in pts if v is not None]
    ax.plot([p[0] for p in pts], [p[1] * 1e3 for p in pts], color="k", lw=0.6, ls=":",
            label="harmonic balance" if n == 0 else None)
ax.set_xlim(48, 56)
ax.set_xlabel("drive frequency (Hz)")
ax.set_ylabel("tip amplitude (mm)")
ax.set_title("Duffing harvester: the response folds over, and the sweep jumps")
ax.legend(fontsize=8)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/piezo_duffing.svg")

# --- storage: an ensemble of four broadband realizations -------------------------------------
rows = run("piezo_storage", ",".join(f"s{k}{n}" for n in range(4) for k in "LH"))
fig, ax = plt.subplots(figsize=(6.4, 4.2))
finals = {"L": [], "H": []}
for n in range(4):
    for kind, ls in (("L", "-"), ("H", "--")):
        v = [r[f"V(s{kind}{n})"] for r in rows]
        finals[kind].append(v[-1])
        ax.plot([r["t"] for r in rows], v, color=f"C{n}", ls=ls, lw=1.1,
                label=f"seed {n}, {'linear' if kind == 'L' else 'hardening'}")
    print(f"seed {n}: linear {finals['L'][-1]:.3f} V, hardening {finals['H'][-1]:.3f} V at t = 10 s")
ml, mh = sum(finals["L"]) / 4, sum(finals["H"]) / 4
print(f"mean over the four: linear {ml:.3f} V, hardening {mh:.3f} V "
      f"(stored energy ratio {(mh / ml) ** 2:.3f})")
ax.set_xlabel("time (s)")
ax.set_ylabel("storage capacitor (V)")
ax.set_title("Broadband 4 m/s² RMS, four realizations: linear (solid) vs hardening (dashed)")
ax.legend(fontsize=7, ncol=2)
ax.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/piezo_storage.svg")
print("wrote docs/examples/multiphysics/piezo_duffing.svg, piezo_storage.svg")
