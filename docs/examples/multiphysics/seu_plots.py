"""Draw the single-event-upset decks of circuits/multiphysics/seu_*.net, and check them.

  python docs/examples/multiphysics/seu_plots.py [path/to/va-cli]

runs the decks through va-cli and writes
  docs/examples/multiphysics/seu_collection.svg -- the current and charge a drain collects from
      an ion track by diffusion, against the diffusion equation's exact series solution;
  docs/examples/multiphysics/seu_sram.svg -- an SRAM cell struck just below and just above its
      critical charge, collecting by diffusion;
  docs/examples/multiphysics/seu_qcrit.svg -- the critical charge against the collection pulse's
      duration, with the two limits computed from the cell's own equations.
The critical charges are found by bisection: the script writes decks of eight struck cells into
target/ and narrows the bracket between runs.
"""
import math
import os
import subprocess
import sys

import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/seu"
OUT = "docs/examples/multiphysics"
VDD, CNODE = 1.2, 1e-15
NMOS, PMOS, ACCESS = 4e-4, 2e-4, 2e-4            # beta, A/V^2 (deck values)
VT0, NSLOPE, UT = 0.4, 1.3, 0.025852
DN, DEPTH, TRACK, TAUR = 3.6e-3, 5e-6, 2e-6, 1e-7  # strike_diffusion defaults


def sim(deck, report):
    out = subprocess.run([VA, "sim", deck, "--model", MODEL, "--tran", "--report", report],
                         capture_output=True, text=True, check=True).stdout
    rows, notes = [], []
    for line in out.splitlines():
        if line.startswith("  t="):
            toks = line.split()
            row = {"t": float(toks[0][2:-1])}
            for tok in toks[1:]:
                if "=" in tok:
                    k, v = tok.split("=")
                    row[k] = float(v)
            rows.append(row)
        elif line.startswith("X"):
            notes.append(line)
    return rows, notes


# --- the model's own equations, for the references ---------------------------------------------
def softplus(x):
    return x if x > 40 else math.log1p(math.exp(x))


def ids(vg, vd, vs, beta, typ):
    """mos's drain current (into d, out of s), as models/multiphysics/seu/seu_parts.va has it."""
    vp = (typ * (vg - vs) - VT0) / NSLOPE
    vds = typ * (vd - vs)
    return typ * 2 * NSLOPE * beta * UT * UT * (softplus(vp / (2 * UT)) ** 2
                                                 - softplus((vp - vds) / (2 * UT)) ** 2)


def into(q, qb):
    """Net current the cell's transistors drive into node q (the bit lines at VDD, word line 0)."""
    return (-ids(qb, q, 0.0, NMOS, 1) - ids(qb, q, VDD, PMOS, -1) + ids(0.0, VDD, q, ACCESS, 1))


def settle(q, qb, t_end=400e-12, h=0.05e-12):
    """Integrate the unstruck cell, C dq/dt = into(q, qb) (and the same for qb), by RK4."""
    y = np.array([q, qb])

    def f(y):
        return np.array([into(y[0], y[1]), into(y[1], y[0])]) / CNODE

    for _ in range(int(t_end / h)):
        k1 = f(y)
        k2 = f(y + h / 2 * k1)
        k3 = f(y + h / 2 * k2)
        k4 = f(y + h * k3)
        y = y + h / 6 * (k1 + 2 * k2 + 2 * k3 + k4)
    return y


def qcrit_instant():
    """Charge whose instantaneous removal from node q flips the cell: the tau -> 0 limit. The cell
    is symmetric, so its separatrix is the diagonal q = qb: with qb still at 0 the removal has to
    take q to 0, and the answer should be C VDD exactly. Found here from the ODE all the same."""
    lo, hi = 0.0, 3 * CNODE * VDD
    for _ in range(30):
        mid = (lo + hi) / 2
        lo, hi = (lo, mid) if settle(VDD - mid / CNODE, 0.0)[0] < VDD / 2 else (mid, hi)
    return mid


def icrit_static():
    """The largest current the stored 1 can hold off: the peak of the cell's static N-curve, with
    qb at its DC value for each q. The tau -> infinity limit is Q = icrit tau."""
    best = 0.0
    for q in np.linspace(VDD / 2, VDD, 2401):
        lo, hi = 0.0, VDD                     # qb from its own node's balance, by bisection
        for _ in range(60):
            mid = (lo + hi) / 2
            lo, hi = (mid, hi) if into(mid, q) > 0 else (lo, mid)
        best = max(best, into(q, mid))
    return best


def current_exact(t):
    """The series for the collected current (A) at t > 0, for qdep = 1 C."""
    m = np.arange(20000)
    k = (m + 0.5) * math.pi / DEPTH
    lam = DN * k * k + 1 / TAUR
    return 2 * DN / (DEPTH * TRACK) * np.sum((1 - np.cos(k * TRACK)) * np.exp(-lam * t))


def collected_exact(t):
    """The diffusion equation's series for the charge collected by time t (C), for qdep = 1 C."""
    m = np.arange(20000)
    k = (m + 0.5) * math.pi / DEPTH
    lam = DN * k * k + 1 / TAUR
    return 2 * DN / (DEPTH * TRACK) * np.sum((1 - np.cos(k * TRACK)) * (1 - np.exp(-lam * t)) / lam)


# --- 1. collection -----------------------------------------------------------------------------
rows, notes = sim("circuits/multiphysics/seu_collection.net", "V1,X1.col")
t = np.array([r["t"] for r in rows])
qcol = np.array([r["Qc(X1.col)"] for r in rows])
icol = -np.array([r["I(V1)"] for r in rows])
exact = np.array([collected_exact(x) for x in t]) * 10e-15
for note in notes:
    print("  " + note)
print("collection of 10 fC by diffusion, against the exact series:")
for when in (0.1e-9, 0.5e-9, 2e-9, 5e-9, 20e-9):
    j = int(np.argmin(abs(t - when)))
    print(f"  t = {t[j] * 1e9:5.2f} ns: {qcol[j] * 1e15:.4f} fC, exact {exact[j] * 1e15:.4f} fC "
          f"({100 * (qcol[j] / exact[j] - 1):+.3f} %)")
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.2, 5.4), sharex=True)
ax1.loglog(t[1:] * 1e9, icol[1:] * 1e6, lw=2, label="simulated, 50 slices")
ax1.loglog(t[1:] * 1e9, [current_exact(x) * 10e-15 * 1e6 for x in t[1:]], "k:", lw=1.2,
           label="exact series")
ax1.set_ylabel("collected current (uA)")
ax1.grid(True, which="both", alpha=0.3)
ax1.legend(fontsize=8)
ax2.semilogx(t[1:] * 1e9, qcol[1:] * 1e15, lw=2, label="simulated")
ax2.semilogx(t[1:] * 1e9, exact[1:] * 1e15, "k:", lw=1.2, label="exact series")
ax2.set_xlabel("time after the strike (ns)")
ax2.set_ylabel("charge collected (fC)")
ax2.legend(fontsize=8)
ax2.grid(True, alpha=0.3)
fig.suptitle("Charge collection by diffusion from a 2 um track", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/seu_collection.svg")

# --- 3. critical charge, by bisection over decks of eight cells --------------------------------
CELL = """VDD{k} vdd{k} gnd DC 1.2
VBL{k} bl{k} gnd DC 1.2
XN1{k} q{k} qb{k} gnd mos
XP1{k} q{k} qb{k} vdd{k} mos type=-1 beta=2e-4
XN2{k} qb{k} q{k} gnd mos
XP2{k} qb{k} q{k} vdd{k} mos type=-1 beta=2e-4
XA1{k} bl{k} gnd q{k} mos beta=2e-4
XA2{k} bl{k} gnd qb{k} mos beta=2e-4
C1{k} q{k} gnd 1f
C2{k} qb{k} gnd 1f
XS{k} q{k} gnd {strike}
"""
os.makedirs("target", exist_ok=True)


def flips(strikes, tstop, tstep):
    """Run one deck of struck cells; return which ended with node q low."""
    deck = "target/seu_bisect.net"
    with open(deck, "w") as fh:
        fh.write("* generated by seu_plots.py: critical-charge bisection\n")
        for k, s in enumerate(strikes):
            fh.write(CELL.format(k=k, strike=s))
        fh.write(".nodeset " + " ".join(f"V(q{k})=1.2 V(qb{k})=0" for k in range(len(strikes))))
        fh.write(f"\n.tran {tstep} {tstop}\n.end\n")
    _, notes = sim(deck, "q0")
    final = {}
    for note in notes:   # "XS<k>: ... drain at <v> V"
        if "drain at" in note:
            final[int(note.split(":")[0][2:])] = float(note.split("drain at")[1].split()[0])
    return [final[k] < VDD / 2 for k in range(len(strikes))]


def bisect(strike_of, lo, hi, tstop, tstep, rounds=4):
    for _ in range(rounds):
        qs = np.geomspace(lo, hi, 8)
        res = flips([strike_of(q) for q in qs], tstop, tstep)
        first = res.index(True) if True in res else None
        if first is None:
            lo, hi = qs[-1], qs[-1] * 4
        elif first == 0:
            lo, hi = qs[0] / 4, qs[0]
        else:
            lo, hi = qs[first - 1], qs[first]
    return math.sqrt(lo * hi)


q0 = qcrit_instant()
i0 = icrit_static()
print(f"\ncritical charge, the cell's own limits: instantaneous {q0 * 1e15:.4f} fC; "
      f"static current {i0 * 1e6:.2f} uA")
taus = np.geomspace(0.1e-12, 3e-9, 10)
qc = []
for tau in taus:
    tstop = max(300e-12, 12 * tau)
    q = bisect(lambda q, tau=tau: f"strike_exp q={q:.6e} tau={tau:.6e}", 0.1e-15, 1000e-15,
               f"{tstop:.4e}", f"{min(tau / 2, 1e-12):.4e}")
    qc.append(q)
    print(f"  tau {tau:9.3e} s: Q_crit {q * 1e15:8.4f} fC  (instantaneous {q0 * 1e15:.4f}, "
          f"I_crit tau {i0 * tau * 1e15:8.4f})")
qd = bisect(lambda q: f"strike_diffusion qdep={q:.6e}", 1e-15, 1e-12, "20e-9", "10e-12")
print(f"  diffusion from a 2 um track: Q_crit {qd * 1e15:.3f} fC deposited")
fig, ax = plt.subplots(figsize=(6.4, 4.4))
ax.loglog(taus, np.array(qc) * 1e15, "o-", lw=2, label="simulated (bisection)")
ax.axhline(q0 * 1e15, color="k", ls=":", lw=1.2, label="instantaneous limit (cell ODE)")
tt = np.geomspace(taus[0], taus[-1], 50)
ax.loglog(tt, i0 * tt * 1e15, "k--", lw=1, label="static limit, I_crit tau (N-curve peak)")
ax.axhline(qd * 1e15, color="C3", lw=1.2, label="diffusion collection (deposited charge)")
ax.set_xlabel("collection pulse time constant tau (s)")
ax.set_ylabel("critical charge (fC)")
ax.set_ylim(0.3, 300)
ax.legend(fontsize=8)
ax.grid(True, which="both", alpha=0.3)
ax.set_title("SRAM critical charge against the pulse's duration")
fig.tight_layout()
fig.savefig(f"{OUT}/seu_qcrit.svg")

# --- 2. the struck cell ------------------------------------------------------------------------
rows, notes = sim("circuits/multiphysics/seu_sram.net", "q1,qb1,q2,qb2")
for note in notes:
    print("  " + note)
t = np.array([r["t"] for r in rows])
fig, ax = plt.subplots(figsize=(6.4, 4.0))
for k, lab in ((1, "10 fC, below critical"), (2, "11 fC, above critical")):
    ax.semilogx(t[1:] * 1e9, [r[f"V(q{k})"] for r in rows[1:]], lw=2, label=f"q, {lab}")
    ax.semilogx(t[1:] * 1e9, [r[f"V(qb{k})"] for r in rows[1:]], lw=1, ls="--", label=f"qb, {lab}")
ax.set_xlabel("time after the strike (ns)")
ax.set_ylabel("storage node (V)")
ax.legend(fontsize=8)
ax.grid(True, alpha=0.3)
ax.set_title("A struck SRAM cell, collecting by diffusion")
fig.tight_layout()
fig.savefig(f"{OUT}/seu_sram.svg")
print(f"wrote {OUT}/seu_collection.svg, seu_qcrit.svg, seu_sram.svg")
