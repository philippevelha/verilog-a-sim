"""Draw the ReRAM I-V loops of circuits/multiphysics/reram_sweep.net.

Runs the deck through va-cli, takes each cell's voltage V(te) and current (V(in) - V(te))/2k
(the series resistor's), and plots |I| against V on a log scale, the usual way a switching
curve is shown: the pinched loop, SET on the positive branch, RESET on the negative.

Usage (from the repository root, after `cargo build --release`):
    python docs/examples/multiphysics/reram_iv.py [path/to/va-cli]
writes docs/examples/multiphysics/reram_iv.svg.
"""
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
RSERIES = 2e3
out = subprocess.run(
    [VA, "sim", "circuits/multiphysics/reram_sweep.net", "--model",
     "models/multiphysics/reram", "--tran", "--report", "in,te1,te2"],
    capture_output=True, text=True, check=True,
).stdout

vin, v1, v2 = [], [], []
for line in out.splitlines():
    toks = line.split()
    if not toks or not toks[0].startswith("t="):
        continue
    vals = {k: float(v) for k, v in (t.split("=") for t in toks[1:] if "=" in t)}
    vin.append(vals["V(in)"])
    v1.append(vals["V(te1)"])
    v2.append(vals["V(te2)"])

fig, ax = plt.subplots(figsize=(6.4, 4.4))
for v, label in ((v1, "300 K"), (v2, "400 K")):
    i = [abs((a - b) / RSERIES) + 1e-12 for a, b in zip(vin, v)]
    ax.semilogy(v, i, label=f"cell at {label} ambient", lw=1.2)
ax.set_xlabel("cell voltage V(te) (V)")
ax.set_ylabel("|cell current| (A)")
ax.set_title("Filamentary ReRAM: pinched hysteresis, SET (+) and RESET (-)")
ax.grid(True, which="both", alpha=0.3)
ax.legend()
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/reram_iv.svg")
print("wrote docs/examples/multiphysics/reram_iv.svg")
