"""Draw the thin-fibre spike train of circuits/multiphysics/hh_thin_fibre.net.

Runs the deck through va-cli and plots, against time, the middle compartment's membrane
potential, each spike's peak, the sodium reversal potential ENa = (RT/F) ln(Na_o / Na_i) computed
from the intracellular sodium the run reports, and that sodium itself. Prints the first and last
spike's peak, ENa and conduction velocity.

Usage (from the repository root, after `cargo build --release`):
    python docs/examples/multiphysics/hh_thin_fibre.py [path/to/va-cli]
writes docs/examples/multiphysics/hh_thin_fibre.svg.
"""
import math
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
# The deck's cable: squid concentrations, 18.5 C.
NAO, NAI0, TC = 440.0, 55.2, 18.5
RTF = 8.314462618 * (TC + 273.15) / 96485.33212

out = subprocess.run(
    [VA, "sim", "circuits/multiphysics/hh_thin_fibre.net", "--model", "models/multiphysics/axon",
     "--tran", "--report", "X1.v[5],X1.dnai[5]"],
    capture_output=True, text=True, check=True,
).stdout

t, v, na = [], [], []
velocities = []
for line in out.splitlines():
    if "velocity" in line:
        velocities.append(float(line.rsplit("velocity", 1)[1].split()[0]))
        continue
    toks = line.split()
    if not toks or not toks[0].startswith("t="):
        continue
    vals = {k: float(x) for k, x in (tok.split("=") for tok in toks[1:] if "=" in tok)}
    t.append(float(toks[0][2:-1]))
    v.append(vals["V(X1.v[5])"])
    na.append(vals["Conc(X1.dnai[5])"])

# Spike peaks: local maxima above 0 V.
peaks = [(t[k], v[k], na[k]) for k in range(1, len(v) - 1)
         if v[k] > 0.0 and v[k] >= v[k - 1] and v[k] > v[k + 1]]
ena = [RTF * math.log(NAO / (NAI0 + x)) for x in na]

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(7.0, 6.0), sharex=True)
ax1.plot(t, [x * 1e3 for x in v], lw=0.4, color="0.6", label="V(middle compartment)")
ax1.plot([p[0] for p in peaks], [p[1] * 1e3 for p in peaks], "o", ms=2.5, label="spike peak")
ax1.plot(t, [x * 1e3 for x in ena], lw=1.4, label="ENa (Nernst)")
ax1.set_ylabel("mV")
ax1.set_title("Thin fibre, 100 Hz train: sodium accumulates, ENa falls, spikes shrink")
ax1.legend(loc="lower left", fontsize=8)
ax1.grid(True, alpha=0.3)
ax2.plot(t, [NAI0 + x for x in na], lw=1.2)
ax2.set_xlabel("time (s)")
ax2.set_ylabel("[Na+]i (mM)")
ax2.grid(True, alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/hh_thin_fibre.svg")

print(f"{len(peaks)} spikes at the middle compartment")
for label, (tp, vp, nap) in (("first", peaks[0]), ("last", peaks[-1])):
    print(f"{label:>5} spike: t={tp:.4f} s, peak {vp * 1e3:.2f} mV, [Na]i {NAI0 + nap:.2f} mM, "
          f"ENa {RTF * math.log(NAO / (NAI0 + nap)) * 1e3:.2f} mV")
print(f"velocity: first {velocities[0]:.3f} m/s, last {velocities[-1]:.3f} m/s")
print("wrote docs/examples/multiphysics/hh_thin_fibre.svg")
