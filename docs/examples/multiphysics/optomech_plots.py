"""Draw the cavity-optomechanics decks of circuits/multiphysics/optomech_*.net, and check them.

  python docs/examples/multiphysics/optomech_plots.py [path/to/va-cli]

runs both decks through va-cli and writes
  docs/examples/multiphysics/optomech_threshold.svg -- the mechanical amplitude of the four
      instances (below, at and above the blue-side threshold; red-detuned), on a log scale;
  docs/examples/multiphysics/optomech_limit_cycle.svg -- the phonon laser's envelope, from the
      model's `$strobe` of every 100th displacement peak, growing and saturating.
It prints, for each instance, the simulated amplitude rate beside two references: the exact
eigenvalue of the model's own equations linearized about their steady state (computed here, with
numpy), and the weak-coupling formula -(Gamma_m + Gamma_opt) / 2. For the limit cycle it prints
the settled amplitude beside the one at which a cycle's radiation-pressure work balances its
mechanical loss.
"""
import math
import subprocess
import sys

import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/optomech"

# The model's defaults (models/multiphysics/optomech/optomech.va).
WC = 2 * math.pi * 2.99792458e8 / 1550e-9
KAPPA, KEX, G, M = 6.283185307e7, 3.1415926535e7, 2.9796e19, 1e-11
OM = 2 * math.pi * 10e6
GM = OM / 1e4


def steady(p, d0):
    """The steady field (sqrt(fJ)) and deflection (m), by fixed-point iteration."""
    s, x = math.sqrt(p) * 3.16227766e7, 0.0
    for _ in range(200):
        a = math.sqrt(KEX) * s / (0.5 * KAPPA - 1j * (d0 + G * x))
        x = G * abs(a) ** 2 * 1e-15 / (WC * M * OM * OM)
    return a, x


def exact_rate(p, d0):
    """Real part of the mechanical eigenvalue of the linearized (ar, ai, x, v) system, 1/s."""
    a, x = steady(p, d0)
    d, k = d0 + G * x, G / WC * 1e-15
    jac = np.array([
        [-0.5 * KAPPA, -d, -G * a.imag, 0],
        [d, -0.5 * KAPPA, G * a.real, 0],
        [0, 0, 0, 1],
        [2 * k * a.real / M, 2 * k * a.imag / M, -OM * OM, -GM],
    ])
    # The two field eigenvalues decay at ~kappa/2; the mechanical pair is the slow one.
    return min(np.linalg.eigvals(jac), key=lambda e: abs(e.real)).real


def textbook_rate(p, d0):
    """-(Gamma_m + Gamma_opt) / 2 with Gamma_opt from the weak-coupling formula."""
    a, x = steady(p, d0)
    d, u = d0 + G * x, abs(a) ** 2 * 1e-15
    g2 = G * G * u / (2 * M * OM * WC)
    gopt = g2 * KAPPA * (1 / (KAPPA ** 2 / 4 + (d + OM) ** 2) - 1 / (KAPPA ** 2 / 4 + (d - OM) ** 2))
    return -(GM + gopt) / 2


def limit_cycle_amplitude(p, d0, n=4000, periods=12):
    """The amplitude A at which harmonic motion x = x_s + A cos(Omega t) neither gains nor loses
    energy over a cycle: the work of the radiation pressure, the integral of F v dt with the field
    integrated (RK4) through the motion, equals the mechanical loss pi m Gamma_m Omega A^2.
    Independent of the circuit simulation: only the field ODE is integrated, the motion imposed."""
    s = math.sqrt(p) * 3.16227766e7
    a_s, xs = steady(p, d0)
    dt = 2 * math.pi / OM / n

    def net(amp):
        def rhs(t, a):
            return (1j * (d0 + G * (xs + amp * math.cos(OM * t))) - 0.5 * KAPPA) * a + math.sqrt(KEX) * s
        a, t, work = a_s, 0.0, 0.0
        for k in range(n * periods):
            if k >= n * (periods - 1):  # the last period, once the field has settled
                work += G / WC * abs(a) ** 2 * 1e-15 * (-amp * OM * math.sin(OM * t)) * dt
            k1 = rhs(t, a)
            k2 = rhs(t + dt / 2, a + dt / 2 * k1)
            k3 = rhs(t + dt / 2, a + dt / 2 * k2)
            k4 = rhs(t + dt, a + dt * k3)
            a += dt / 6 * (k1 + 2 * k2 + 2 * k3 + k4)
            t += dt
        return work - math.pi * M * GM * OM * amp * amp

    lo, hi = 1e-12, 1e-11
    for _ in range(25):
        mid = (lo + hi) / 2
        lo, hi = (mid, hi) if net(mid) > 0 else (lo, mid)
    return mid


def sim(deck, report):
    proc = subprocess.Popen(
        [VA, "sim", f"circuits/multiphysics/{deck}.net", "--model", MODEL, "--tran",
         "--report", report],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    )
    return proc


# --- threshold ---------------------------------------------------------------------------------
CASES = [("X1", 1.417e-8, OM, "blue, 0.5 P_th"), ("X2", 2.834e-8, OM, "blue, 1 P_th"),
         ("X3", 5.668e-8, OM, "blue, 2 P_th"), ("X4", 5.668e-8, -OM, "red, 2 P_th")]
proc = sim("optomech_threshold", ",".join(f"{c[0]}.x" for c in CASES))
period = 1 / 10e6
t_cyc, amp = [], {c[0]: [] for c in CASES}
lo = {c[0]: math.inf for c in CASES}
hi = {c[0]: -math.inf for c in CASES}
edge = period
for line in proc.stdout:
    if not line.startswith("  t="):
        continue
    toks = line.split()
    t = float(toks[0][2:-1])
    if t >= edge:  # close one mechanical period: amplitude = half the peak-to-peak swing
        t_cyc.append(edge - period / 2)
        for c in CASES:
            amp[c[0]].append((hi[c[0]] - lo[c[0]]) / 2)
            lo[c[0]], hi[c[0]] = math.inf, -math.inf
        edge += period
    for tok in toks[1:]:
        if "=" not in tok:  # a unit
            continue
        name, val = tok.split("=")
        inst = name[name.index("(") + 1:name.index(".")]
        v = float(val)
        lo[inst], hi[inst] = min(lo[inst], v), max(hi[inst], v)
proc.wait()

t_cyc = np.array(t_cyc)
fit = t_cyc > 20e-6  # past the kick's transient
fig, ax = plt.subplots(figsize=(6.4, 4.4))
print(f"{'case':16s} {'simulated':>10s} {'exact':>10s} {'formula':>10s}   (amplitude rate, 1/s)")
for inst, p, d0, lab in CASES:
    a = np.array(amp[inst])
    rate = np.polyfit(t_cyc[fit], np.log(a[fit]), 1)[0]
    print(f"{lab:16s} {rate:10.1f} {exact_rate(p, d0):10.1f} {textbook_rate(p, d0):10.1f}")
    ax.semilogy(t_cyc * 1e3, a * 1e15, label=f"{lab}: {rate:+.0f} /s")
for name, rate_of in (("exact", exact_rate), ("weak-coupling formula", textbook_rate)):
    lo_p, hi_p = 1e-8, 1e-7
    for _ in range(60):
        mid = (lo_p + hi_p) / 2
        lo_p, hi_p = (lo_p, mid) if rate_of(mid, OM) > 0 else (mid, hi_p)
    print(f"threshold, {name}: {mid * 1e9:.3f} nW")
ax.set_xlabel("time (ms)")
ax.set_ylabel("mechanical amplitude (fm)")
ax.set_title("Optomechanical anti-damping and cooling")
ax.legend(fontsize=8)
ax.grid(True, which="both", alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/optomech_threshold.svg")

# --- limit cycle -------------------------------------------------------------------------------
proc = sim("optomech_limit_cycle", "pin")
tp, xp = [], []
for line in proc.stdout:
    if ": peak " in line:  # X1: peak <n> at t=<t> s, x=<x> m
        toks = line.replace(",", " ").split()
        tp.append(float(toks[4].split("=")[1]))
        xp.append(float(toks[6].split("=")[1]))
proc.wait()
tp, xp = np.array(tp), np.array(xp)
_, xs = steady(1.1336e-7, OM)
env = xp - xs
r = exact_rate(1.1336e-7, OM)
early = env < 0.1 * env.max()
c0 = np.exp(np.mean(np.log(env[early]) - r * tp[early]))
amp_eb = limit_cycle_amplitude(1.1336e-7, OM)
print(f"limit cycle: amplitude {env[-1] * 1e12:.4f} pm, energy balance {amp_eb * 1e12:.4f} pm "
      f"(kappa / G = {KAPPA / G * 1e12:.2f} pm); linear growth at the exact rate {r:.0f} /s")
fig, ax = plt.subplots(figsize=(6.4, 4.2))
ax.semilogy(tp * 1e3, env * 1e12, ".", ms=3, label="simulated: every 100th peak ($strobe)")
tt = tp[early]
ax.semilogy(tt * 1e3, c0 * np.exp(r * tt) * 1e12, "k:", label=f"linear growth, {r:.0f} /s (eigenvalue)")
ax.axhline(amp_eb * 1e12, color="k", lw=0.8, ls="--", label="limit cycle, energy balance")
ax.axhline(KAPPA / G * 1e12, color="grey", lw=0.8, ls=":", label=r"$\kappa / G$")
ax.set_xlabel("time (ms)")
ax.set_ylabel("displacement amplitude (pm)")
ax.set_title("A phonon laser: growth and saturation at 4 x threshold")
ax.legend(fontsize=8)
ax.grid(True, which="both", alpha=0.3)
fig.tight_layout()
fig.savefig("docs/examples/multiphysics/optomech_limit_cycle.svg")
print("wrote docs/examples/multiphysics/optomech_threshold.svg, optomech_limit_cycle.svg")
