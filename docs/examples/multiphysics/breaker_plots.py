"""Draw the circuit-breaker decks of circuits/multiphysics/breaker_*.net, and check them.

  python docs/examples/multiphysics/breaker_plots.py [path/to/va-cli]

runs both decks through va-cli and writes
  docs/examples/multiphysics/breaker_static.svg -- the arc voltage against its current, on the
      Mayr static characteristic V = P / I;
  docs/examples/multiphysics/breaker_interrupt.svg -- the recovery voltages and line currents of
      the two breakers, with and without gas blast.
It prints V*I along the static ramp, and for each breaker the chopped current and the
recovery-voltage peak beside the energy-conservation closed form.
"""
import math
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/breaker"
L, C, P = 31.8e-3, 10e-9, 20e3


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


# --- static characteristic ---------------------------------------------------------------------
rows, _ = run("breaker_static", "p,ctl")
arc = [r for r in rows if r["t"] > 0.02]
for r in (arc[0], arc[len(arc) // 2], arc[-1]):
    print(f"static: I = {r['V(ctl)']:.1f} A, V = {r['V(p)']:.2f} V, V*I = {r['V(p)'] * r['V(ctl)']:.1f} W "
          f"(closed form {P:.0f} W)")
fig, ax = plt.subplots(figsize=(6.0, 4.2))
ax.loglog([r["V(ctl)"] for r in arc], [r["V(p)"] for r in arc], lw=2, label="simulated arc")
ii = [10 * 1.1 ** k for k in range(50)]
ax.loglog(ii, [P / i for i in ii], "k:", lw=1, label="V = P / I, P = 20 kW")
ax.set_xlabel("arc current (A)")
ax.set_ylabel("arc voltage (V)")
ax.set_title("Mayr arc: the static characteristic")
ax.legend(fontsize=8)
ax.grid(True, which="both", alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/breaker_static.svg")

# --- interruption ------------------------------------------------------------------------------
rows, events = run("breaker_interrupt", "p1,p2,s1,s2,L1,L2,X1.lg,X2.lg")
for e in events:
    print("  " + e)
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.6, 5.6), sharex=True)
for k, lab in ((1, "gas blast"), (2, "no blast")):
    chop = next(i for i, r in enumerate(rows) if r["t"] > 0.01 and r[f"Lg(X{k}.lg)"] < math.log(1e-6))
    r = rows[chop]
    amp = math.sqrt((r[f"V(p{k})"] - r[f"V(s{k})"]) ** 2 + (L / C) * r[f"I(L{k})"] ** 2)
    ring = rows[chop:chop + 400]
    pk = max(ring, key=lambda q: abs(q[f"V(p{k})"]))
    closed = r[f"V(s{k})"] - amp if pk[f"V(p{k})"] < 0 else r[f"V(s{k})"] + amp
    print(f"{lab}: chopped {r[f'I(L{k})']:.3f} A at t = {r['t'] * 1e3:.3f} ms; recovery voltage peak "
          f"{pk[f'V(p{k})']:.0f} V (closed form {closed:.0f} V)")
    ax1.plot([q["t"] * 1e3 for q in rows], [q[f"V(p{k})"] / 1e3 for q in rows], label=lab)
    ax2.plot([q["t"] * 1e3 for q in rows], [q[f"I(L{k})"] for q in rows], label=lab)
ax1.set_ylabel("voltage across the breaker (kV)")
ax1.legend(fontsize=8)
ax1.grid(True, alpha=0.3)
ax2.set_ylabel("line current (A)")
ax2.set_xlabel("time (ms)")
ax2.grid(True, alpha=0.3)
ax1.set_xlim(15, 25)
fig.suptitle("Interruption by current chopping, with and without gas blast", fontsize=10)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/breaker_interrupt.svg")
print("wrote docs/examples/multiphysics/breaker_static.svg, breaker_interrupt.svg")
