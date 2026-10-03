"""The ReRAM frequency fingerprint: draw and measure the pinched I-V loops at 5 kHz, 500 kHz, 5 MHz.

Runs circuits/multiphysics/reram_freq_{5k,500k,5meg}.net through va-cli, keeps each run's second
period, and plots the cell current (the 2 kOhm series resistor's) against the cell voltage. Each
lobe's area (the positive-voltage and the negative-voltage one, integrated separately, since the
two are traversed in opposite directions and a single signed integral would cancel them) and the
range the filament gap covers are printed: both shrink as the drive outruns the ion drift.

Usage (from the repository root, after `cargo build --release`):
    python docs/examples/multiphysics/reram_loops.py [path/to/va-cli]
writes docs/examples/multiphysics/reram_loops.svg.
"""
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
RSERIES = 2e3
RUNS = (("5k", 5e3, "5 kHz"), ("500k", 5e5, "500 kHz"), ("5meg", 5e6, "5 MHz"))

fig, ax = plt.subplots(figsize=(6.4, 4.4))
for name, freq, label in RUNS:
    out = subprocess.run(
        [VA, "sim", f"circuits/multiphysics/reram_freq_{name}.net", "--model",
         "models/multiphysics/reram", "--tran", "--report", "in,te,X1.g"],
        capture_output=True, text=True, check=True,
    ).stdout
    rows = []
    for line in out.splitlines():
        toks = line.split()
        if not toks or not toks[0].startswith("t="):
            continue
        vals = {k: float(v) for k, v in (t.split("=") for t in toks[1:] if "=" in t)}
        vals["t"] = float(toks[0][2:-1])
        rows.append(vals)
    period = 1.0 / freq
    seg = [r for r in rows if r["t"] >= period - 1e-15]
    v = [r["V(te)"] for r in seg]
    i = [(r["V(in)"] - r["V(te)"]) / RSERIES for r in seg]
    pos = neg = 0.0
    for k in range(len(seg) - 1):
        da = 0.5 * (i[k] + i[k + 1]) * (v[k + 1] - v[k])
        if v[k] + v[k + 1] > 0:
            pos += da
        else:
            neg += da
    gap = [r["Gap(X1.g)"] for r in seg]
    print(f"{label:>8}: lobe areas {abs(pos):.3e} + {abs(neg):.3e} = {abs(pos) + abs(neg):.3e} V*A, "
          f"gap {min(gap):.3f}..{max(gap):.3f} nm")
    ax.plot(v, [x * 1e3 for x in i], lw=1.2, label=label)
ax.set_xlabel("cell voltage V(te) (V)")
ax.set_ylabel("cell current (mA)")
ax.set_title("ReRAM: the pinched loop shrinks as the drive outruns the ion drift")
ax.grid(True, alpha=0.3)
ax.legend()
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/reram_loops.svg")
print("wrote docs/examples/multiphysics/reram_loops.svg")
