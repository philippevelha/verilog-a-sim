"""Draw the nonlinear microring decks of circuits/multiphysics/ringchaos_*.net, and check them.

  python docs/examples/multiphysics/ringchaos_plots.py [path/to/va-cli] [--lyapunov]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/ringchaos_bistable.svg -- the through power and ring temperature
      against a slow power sweep, up and down, over the steady-state branches and their stability;
  docs/examples/multiphysics/ringchaos_selfpulse.svg -- self-pulsing near the Hopf point and deep
      in the window, simulated and integrated with scipy;
  docs/examples/multiphysics/ringchaos_bank.svg -- return maps of a periodic and a chaotic
      two-ring bank, and the chaotic bank's separation from its twin.
It prints the checks: folds and Hopf points against the sweep, periods and extremes against scipy,
and (with --lyapunov, about 1.5 hours) the chaotic bank's largest Lyapunov exponent by the
Benettin method (100 us after a 20 us transient), which it also writes to docs/examples/multiphysics/ringchaos_lyapunov.txt for the
figure's reference slope.

The reference equations below are the ones models/multiphysics/ringchaos/rc_parts.va implements.
"""
import math
import os
import subprocess
import sys

import matplotlib
import numpy as np
from scipy.integrate import solve_ivp
from scipy.optimize import brentq, minimize_scalar

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
VA = ARGS[0] if ARGS else "target/release/va-cli"
LYAP = "--lyapunov" in sys.argv
MODEL = "models/multiphysics/ringchaos"
OUT = "docs/examples/multiphysics"
LYAP_FILE = f"{OUT}/ringchaos_lyapunov.txt"

# --- the model (siring's defaults) -------------------------------------------------------------
C, HB, SQ = 2.99792458e8, 1.054571817e-34, 3.16227766e7    # SQ: sqrt(W) -> sqrt(fJ/s)
LAM0, QI, QE, NG, VOL = 1550e-9, 1e5, 1e5, 4.2, 3e-18
BETA, SFCA, DNDN, DNDT = 8.4e-12, 1.45e-21, -1.73e-27, 1.86e-4
TFC, TTH, RTH, ETA = 5e-9, 100e-9, 1e4, 1.0
LAM_L = 1550.155016e-9                                     # the decks' laser
W0 = 2 * math.pi * C / LAM0
TI = 2 * QI / W0
G0 = 1 / TI + W0 / (2 * QE)                                 # cold amplitude decay rate (1/tau_i + 1/tau_e)
CG = BETA * C * C / (2 * NG * NG * VOL)
CF = SFCA * C / (2 * NG)
CGEN = BETA * C * C / (2 * HB * W0 * NG * NG * VOL * VOL)


def pwl(points):
    tt, pp = zip(*points)
    return lambda t: float(np.interp(t, tt, pp)) * 1e-3            # mW -> W


def bank_rhs(power, qes, tths, lam_l=LAM_L):
    """A bank of rings on one bus, ring k driven by ring k-1's through field. State per ring:
    field quadratures (sqrt fJ), carrier density (1/um^3), temperature rise (K)."""
    n = len(qes)
    te = 2 * np.asarray(qes, float) / W0
    kap = np.sqrt(2 / te)
    tths = np.asarray(tths, float)
    d0 = 2 * math.pi * C / lam_l - W0

    def f(t, y):
        ar, ai, nc, temp = y[:n], y[n:2 * n], y[2 * n:3 * n], y[3 * n:]
        u = (ar * ar + ai * ai) * 1e-15
        nm = nc * 1e18
        gt, gf = CG * u, CF * nm
        g = 1 / TI + 1 / te + gt + gf
        dl = d0 + W0 / NG * (DNDT * temp + DNDN * nm)
        sr, si = np.empty(n), np.empty(n)
        sr[0], si[0] = math.sqrt(max(power(t), 0.0)) * SQ, 0.0
        for k in range(1, n):
            sr[k] = sr[k - 1] - kap[k - 1] * ar[k - 1]
            si[k] = si[k - 1] - kap[k - 1] * ai[k - 1]
        return np.concatenate([-g * ar - dl * ai + kap * sr, -g * ai + dl * ar + kap * si,
                               (CGEN * u * u - nm / TFC) * 1e-18,
                               ((ETA * 2 / TI + 2 * gt + 2 * gf) * u * RTH - temp) / tths])

    def through(y, t):
        sr = np.sqrt(np.maximum([power(x) for x in t], 0.0)) * SQ
        si = np.zeros_like(sr)
        for k in range(n):
            sr, si = sr - kap[k] * y[k], si - kap[k] * y[n + k]
        return (sr * sr + si * si) / SQ ** 2 * 1e3                     # mW
    return f, through


KW = dict(method="LSODA", rtol=1e-10, atol=1e-12, max_step=0.5e-9)


# --- 0. steady states and their stability (one ring, power as the parameter) -------------------
def steady(u_fj):
    """The steady state holding stored energy u_fj, and the power that holds it."""
    u = u_fj * 1e-15
    nm = CGEN * u * u * TFC
    gt, gf = CG * u, CF * nm
    g = G0 + gt + gf
    temp = (ETA * 2 / TI + 2 * gt + 2 * gf) * u * RTH
    dl = 2 * math.pi * C / LAM_L - W0 + W0 / NG * (DNDT * temp + DNDN * nm)
    te = 2 * QE / W0
    p = u * (g * g + dl * dl) / (2 / te)
    a = math.sqrt(2 / te) * math.sqrt(p) * SQ / (g - 1j * dl)
    return p, np.array([a.real, a.imag, nm * 1e-18, temp])


def eigs(u_fj):
    p, y = steady(u_fj)
    f, _ = bank_rhs(lambda t: p, [QE], [TTH])
    jac = np.zeros((4, 4))
    for j in range(4):
        h = 1e-7 * max(abs(y[j]), 1e-6)
        yp, ym = y.copy(), y.copy()
        yp[j] += h
        ym[j] -= h
        jac[:, j] = (f(0, yp) - f(0, ym)) / (2 * h)
    return np.linalg.eigvals(jac)


def osc_growth(lu):
    ev = [e for e in eigs(math.exp(lu)) if abs(e.imag) > 1e3]
    return max(e.real for e in ev) if ev else float("nan")


lus = np.linspace(math.log(1e-3), math.log(3e4), 4000)
pw = np.array([steady(math.exp(x))[0] for x in lus])
folds = []
for i in np.where(np.sign(np.diff(pw)[1:]) != np.sign(np.diff(pw)[:-1]))[0]:
    sgn = 1 if pw[i + 1] > pw[i] else -1
    r = minimize_scalar(lambda x: -sgn * steady(math.exp(x))[0], bounds=(lus[i], lus[i + 2]),
                        method="bounded", options=dict(xatol=1e-12))
    folds.append(steady(math.exp(r.x))[0] * 1e3)
gr = np.array([osc_growth(x) for x in lus])
hopfs = []
for i in range(len(lus) - 1):
    if np.isfinite(gr[i]) and np.isfinite(gr[i + 1]) and np.sign(gr[i]) != np.sign(gr[i + 1]):
        x = brentq(osc_growth, lus[i], lus[i + 1], xtol=1e-12)
        om = max(abs(e.imag) for e in eigs(math.exp(x)) if abs(e.real) < 1e6)
        hopfs.append((steady(math.exp(x))[0] * 1e3, om / 2 / math.pi))
stable = np.array([all(e.real < 0 for e in eigs(math.exp(x))) for x in lus])
temps = np.array([steady(math.exp(x))[1][3] for x in lus])
print("steady states of one ring (laser 10 linewidths red of the cold resonance):")
print("  folds at " + ", ".join(f"{p:.5f} mW" for p in folds))
print("  Hopf points at " + ", ".join(f"{p:.5f} mW ({f / 1e6:.2f} MHz)" for p, f in hopfs))


def sim(deck, report):
    out = subprocess.run([VA, "sim", f"circuits/multiphysics/{deck}.net", "--model", MODEL,
                          "--tran", "--report", report], capture_output=True, text=True,
                         check=True).stdout
    t, cols = [], {}
    for line in out.splitlines():
        if line.startswith("  t="):
            toks = line.split()
            t.append(float(toks[0][2:-1]))
            for tok in toks[1:]:
                if "=" in tok:
                    k, v = tok.split("=")
                    cols.setdefault(k, []).append(float(v))
    # The report prints time to 7 significant digits; steps finer than that repeat a time stamp.
    t = np.array(t)
    keep = np.concatenate([[True], np.diff(t) > 0])
    return t[keep], {k: np.array(v)[keep] for k, v in cols.items()}


def peaks(t, x, floor=-np.inf):
    """Times and heights of local maxima, refined by a parabola through each and its neighbours."""
    i = np.where((x[1:-1] > x[:-2]) & (x[1:-1] >= x[2:]) & (x[1:-1] > floor))[0] + 1
    tp, hp = [], []
    for j in i:
        (t0, t1, t2), (y0, y1, y2) = t[j - 1:j + 2], x[j - 1:j + 2]
        if not t0 < t1 < t2:
            tp.append(t1)
            hp.append(y1)
            continue
        a = np.polyfit([t0 - t1, 0.0, t2 - t1], [y0, y1, y2], 2)
        dt = -a[1] / (2 * a[0]) if a[0] < 0 else 0.0
        tp.append(t1 + dt)
        hp.append(np.polyval(a, dt))
    return np.array(tp), np.array(hp)


# --- 1. bistability ----------------------------------------------------------------------------
t, c = sim("ringchaos_bistable", "pctl,pout,t1")
p, po, temp = c["V(pctl)"], c["V(pout)"], c["Temp(t1)"]
up = t <= 30e-6
mid = 0.5 * (steady(math.exp(lus[0]))[1][3] + 2.0)          # a temperature between the branches
j_up = np.argmax(up & (temp > 2.0))
j_dn = np.argmax(~up & (temp < 1.0))
print("\nslow sweep, 0.2 mW/us:")
print(f"  jumps up at {p[j_up]:.3f} mW (fold {folds[0]:.3f} mW), drops back at {p[j_dn]:.3f} mW "
      f"(fold {folds[1]:.3f} mW)")
# Pulsing amplitude on the way down, in windows of 0.5 us: it should vanish near the Hopf point.
for a in np.arange(55e-6, 59.5e-6, 0.5e-6):
    w = (t >= a) & (t < a + 0.5e-6)
    print(f"  down, {p[w].mean():.3f} mW: through power swings {po[w].min() / p[w].mean():.3f}"
          f"..{po[w].max() / p[w].mean():.3f} of the input")
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.6, 6.4), sharex=True)
# The pulsing is far faster than the sweep: draw each leg as the band its signal fills in
# successive 0.25 us windows (min to max), so a steady state is a line and pulsing a band.
for leg, col, lab in ((up, "C0", "sweep up"), (~up, "C1", "sweep down")):
    tl = t[leg]
    edges = np.arange(tl[0], tl[-1], 0.25e-6)
    idx = np.searchsorted(tl, edges)
    pm, tr_lo, tr_hi, te_lo, te_hi = [], [], [], [], []
    for a, b in zip(idx[:-1], idx[1:]):
        if b - a < 3:
            continue
        sl = np.where(leg)[0][a:b]
        pm.append(p[sl].mean())
        r = po[sl] / np.maximum(p[sl], 1e-9)
        tr_lo.append(r.min()); tr_hi.append(r.max())
        te_lo.append(temp[sl].min()); te_hi.append(temp[sl].max())
    ax1.fill_between(pm, tr_lo, tr_hi, color=col, alpha=0.45, lw=0.8, label=lab)
    ax2.fill_between(pm, te_lo, te_hi, color=col, alpha=0.45, lw=0.8)
ax1.set_ylabel("through / input")
ax1.set_ylim(0, 1.05)
ax1.legend(fontsize=8, loc="lower right")
ax1.grid(True, alpha=0.3)
seg = np.split(np.arange(len(lus)), np.where(np.diff(stable.astype(int)) != 0)[0] + 1)
for s in seg:
    ax2.plot(pw[s] * 1e3, temps[s], "k-" if stable[s[0]] else "k:", lw=1.2)
for pf in folds:
    ax2.axvline(pf, color="grey", lw=0.6, ls="--")
for ph, _ in hopfs:
    ax2.axvline(ph, color="C3", lw=0.6, ls="--")
ax2.set_xlim(0, 6.2)
ax2.set_ylim(0, 7)
ax2.set_xlabel("laser power (mW)")
ax2.set_ylabel("ring temperature rise (K)")
ax2.grid(True, alpha=0.3)
ax2.text(0.02, 0.92, "black: steady states (solid stable, dotted unstable); grey: folds; red: Hopf",
         transform=ax2.transAxes, fontsize=7)
fig.suptitle("A silicon microring: bistability and self-pulsing", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/ringchaos_bistable.svg")


# --- 2. self-pulsing, against scipy ------------------------------------------------------------
def period_and_extremes(t, x, t0):
    w = t > t0
    tp, hp = peaks(t[w], x[w], floor=x[w].min() + 0.6 * (x[w].max() - x[w].min()))
    return np.mean(np.diff(tp)), x[w].min(), x[w].max()


t, c = sim("ringchaos_selfpulse", "o1,o2")
print("\nself-pulsing, against scipy (LSODA, rtol 1e-10):")
fig, axs = plt.subplots(1, 2, figsize=(9.0, 3.6))
for k, (prof, lab) in enumerate((([(0, 0), (20e-9, 5), (2e-6, 5), (4e-6, 0.40), (1, 0.40)], "0.40 mW"),
                                 ([(0, 0), (20e-9, 6), (1, 6)], "6 mW"))):
    power = pwl(prof)
    f, through = bank_rhs(power, [QE], [TTH])
    ts = np.arange(0, 20e-6, 0.05e-9)
    sol = solve_ivp(f, (0, 20e-6), np.zeros(4), t_eval=ts, **KW)
    ref = through(sol.y, sol.t)
    sim_p = period_and_extremes(t, c[f"V(o{k + 1})"], 12e-6)
    ref_p = period_and_extremes(sol.t, ref, 12e-6)
    print(f"  {lab}: period {sim_p[0] * 1e9:.4f} ns (scipy {ref_p[0] * 1e9:.4f}), through power "
          f"{sim_p[1]:.4f}..{sim_p[2]:.4f} mW (scipy {ref_p[1]:.4f}..{ref_p[2]:.4f})")
    w, ws = (t > 19e-6), (sol.t > 19e-6)
    axs[k].plot((t[w] - 19e-6) * 1e9, c[f"V(o{k + 1})"][w], lw=2, label="simulated")
    axs[k].plot((sol.t[ws] - 19e-6) * 1e9, ref[ws], "k:", lw=1.2, label="scipy")
    axs[k].set_title(f"{lab} (period {sim_p[0] * 1e9:.2f} ns)", fontsize=9)
    axs[k].set_xlabel("time (ns, from 19 us)")
    axs[k].grid(True, alpha=0.3)
axs[0].set_ylabel("through power (mW)")
axs[0].legend(fontsize=8)
fig.suptitle("Self-pulsing: free carriers against heat", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/ringchaos_selfpulse.svg")

# --- 3. the bank -------------------------------------------------------------------------------
t, c = sim("ringchaos_bank", "o1,o2,o3,o4")
f, through = bank_rhs(pwl([(0, 0), (20e-9, 7), (1, 7)]), [QE, QE], [TTH, TTH])
ts = np.arange(0, 20e-6, 0.05e-9)
sol = solve_ivp(f, (0, 20e-6), np.zeros(8), t_eval=ts, **KW)
ref = through(sol.y, sol.t)
sim_p = period_and_extremes(t, c["V(o1)"], 12e-6)
ref_p = period_and_extremes(sol.t, ref, 12e-6)
print("\ntwo-ring banks, 7 mW:")
print(f"  identical rings: periodic, period {sim_p[0] * 1e9:.4f} ns (scipy {ref_p[0] * 1e9:.4f}), "
      f"through {sim_p[1]:.4f}..{sim_p[2]:.4f} mW (scipy {ref_p[1]:.4f}..{ref_p[2]:.4f})")
late = t > 10e-6
maps = {}
for k in (1, 3):
    _, hp = peaks(t[late], c[f"V(o{k})"][late])
    maps[k] = hp / 7
    print(f"  bank {k}: {len(hp)} maxima after 10 us, {len(np.unique(np.round(hp / 7, 3)))} "
          f"distinct to 1e-3")
d = np.abs(c["V(o3)"] - c["V(o4)"]) / 7
win = [(a, d[(t >= a) & (t < a + 1e-6)].max()) for a in np.arange(0, 59e-6, 1e-6)]
grow = [(a, v) for a, v in win if 1e-4 < v < 0.1]
rate = np.polyfit([a for a, _ in grow], np.log([v for _, v in grow]), 1)[0] if len(grow) > 2 else 0
print(f"  chaotic bank vs its twin: separation 1e-4 -> 0.1 at ~{rate:.2e} /s (largest difference in "
      f"1 us windows; a crude measure), then uncorrelated")
if LYAP:
    f3, _ = bank_rhs(pwl([(0, 0), (20e-9, 7), (1, 7)]), [QE, QE], [TTH, 260e-9])
    kw = dict(method="LSODA", rtol=1e-10, atol=1e-12, max_step=1e-9)
    y = solve_ivp(f3, (0, 20e-6), np.zeros(8), **kw).y[:, -1]
    probe = solve_ivp(f3, (20e-6, 22e-6), y, t_eval=np.linspace(20e-6, 22e-6, 2000), **kw).y
    scale, y, t0 = probe.std(axis=1) + 1e-30, probe[:, -1], 22e-6
    v = np.random.default_rng(1).standard_normal(8)
    z = y + 1e-7 * v / np.linalg.norm(v) * scale
    acc, hist = 0.0, []
    for k in range(500):                                     # 500 renormalizations of 200 ns
        y = solve_ivp(f3, (t0, t0 + 200e-9), y, **kw).y[:, -1]
        z = solve_ivp(f3, (t0, t0 + 200e-9), z, **kw).y[:, -1]
        t0 += 200e-9
        dist = np.linalg.norm((z - y) / scale)
        acc += math.log(dist / 1e-7)
        hist.append(acc / ((k + 1) * 200e-9))
        z = y + (z - y) * (1e-7 / dist)
    with open(LYAP_FILE, "w") as fh:
        fh.write(f"{hist[-1]:.6e}\n")
    print(f"  Benettin, 100 us: largest Lyapunov exponent {hist[-1]:.3e} /s (running estimate at "
          f"25/50/75 us: {hist[124]:.3e}, {hist[249]:.3e}, {hist[374]:.3e})")
lam = float(open(LYAP_FILE).read()) if os.path.exists(LYAP_FILE) else None
fig, axs = plt.subplots(1, 3, figsize=(11.5, 3.7))
for ax, k, lab in ((axs[0], 1, "identical rings: periodic"),
                   (axs[1], 3, "ring B 260 ns: chaotic")):
    m = maps[k]
    ax.plot(m[:-1], m[1:], ".", ms=2.5)
    ax.set_title(lab, fontsize=9)
    ax.set_xlabel("maximum n (of input)")
    ax.set_ylabel("maximum n+1")
    ax.grid(True, alpha=0.3)
axs[2].semilogy(np.array([a for a, _ in win]) * 1e6 + 0.5, [v for _, v in win], "o-", ms=3,
                label="|bank 3 - twin| (largest per us)")
if lam:
    a0, v0 = grow[0] if grow else (10e-6, 1e-4)
    tt = np.linspace(a0, a0 + 8e-6, 20)
    axs[2].semilogy(tt * 1e6 + 0.5, v0 * np.exp(lam * (tt - a0)), "k:",
                    label=f"slope of the Lyapunov exponent, {lam:.2e} /s")
axs[2].set_ylim(1e-6, 3)
axs[2].set_xlabel("time (us)")
axs[2].set_ylabel("through-power difference (of input)")
axs[2].legend(fontsize=7)
axs[2].grid(True, which="both", alpha=0.3)
fig.suptitle("Two self-pulsing rings on one bus, 7 mW", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/ringchaos_bank.svg")
print(f"wrote {OUT}/ringchaos_bistable.svg, ringchaos_selfpulse.svg, ringchaos_bank.svg")
