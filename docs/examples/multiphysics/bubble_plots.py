"""Draw the single-bubble decks of circuits/multiphysics/bubble_*.net, and check them.

  python docs/examples/multiphysics/bubble_plots.py [path/to/va-cli]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/bubble_linear.svg -- the small-signal radius response against the
      closed form of the paper's eq. (13b);
  docs/examples/multiphysics/bubble_bifurcation.svg -- the stroboscopic radius against the drive
      amplitude at omega = 2: a slow sweep, the fixed-amplitude orbits, scipy's, and the Floquet
      threshold of the period-1 orbit.
It prints the checks quoted in the decks' headers. The reference equations below are the ones
models/multiphysics/bubble/bubble_parts.va implements (Smereka & Banerjee, Phys. Fluids 31, 3519,
1988, eq. 32b, dimensionless, gamma = 4/3).
"""
import re
import subprocess
import sys

import matplotlib
import numpy as np
from scipy.integrate import solve_ivp
from scipy.optimize import brentq, fsolve

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/bubble"
DECKS = "circuits/multiphysics"
OUT = "docs/examples/multiphysics"
MU, NU, OM = 0.002, 0.015, 2.0
B = MU + NU
T = 2 * np.pi / OM
G_SINGLE = [0.05, 0.15, 0.3, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]   # bubble_single.net, in order
GDOT = 5e-5                                                        # bubble_ramp.net


def rhs(t, z, amp):
    """(32b) with ln y as the radius state, as in the model. `amp(t)` is the drive amplitude."""
    ly, w = z
    y = np.exp(ly)
    p = 1 + amp(t) * np.sin(OM * t)
    return [w / y, (0.25 * (y**-4 - p) - 1.5 * w * w - w * (MU / y + NU * y**-4)) / y]


def scipy_samples(amp, t):
    s = solve_ivp(rhs, (0, t[-1]), [0, 0], args=(amp,), method="DOP853",
                  rtol=1e-11, atol=1e-13, t_eval=t)
    return np.exp(s.y[0])


def run(deck, *flags):
    cmd = [VA, "sim", f"{DECKS}/{deck}", "--model", MODEL, *flags]
    r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed:\n{r.stdout}\n{r.stderr}")
    return r.stdout


def poincare(text):
    """{instance: (t, y)} from the cells' `poincare` lines."""
    rows = {}
    for m in re.finditer(r"^(\S+): poincare t=(\S+) y=(\S+)", text, re.M):
        rows.setdefault(m[1], []).append((float(m[2]), float(m[3])))
    return {k: (np.array(v)[:, 0], np.array(v)[:, 1]) for k, v in rows.items()}


def period(y, tol=1e-2, window=64):
    for n in range(1, 33):
        if np.allclose(y[-window + n:], y[-window:len(y) - n], atol=tol):
            return n
    return None


def floquet_threshold():
    """g at which the period-1 orbit's multiplier crosses -1 (period doubling)."""
    def flow(z0, g):
        return solve_ivp(rhs, (0, T), z0, args=(lambda t: g,), method="DOP853",
                         rtol=1e-12, atol=1e-14).y[:, -1]

    def mult(g):
        z = fsolve(lambda z: flow(z, g) - z, [0, 0], xtol=1e-11)
        jac = np.empty((2, 2))
        for k in range(2):
            e = np.zeros(2)
            e[k] = 1e-7
            jac[:, k] = (flow(z + e, g) - flow(z - e, g)) / 2e-7
        return min(np.linalg.eigvals(jac), key=lambda v: v.real).real

    return brentq(lambda g: mult(g) + 1, 0.05, 0.2, xtol=1e-7)


def jump(g, y, tol=0.02):
    return g[1 + np.argmax(np.abs(np.diff(y)) > tol)]


# --- 1a: small signal ---------------------------------------------------------------------------
out = run("bubble_linear.net", "--ac", "--report", "XB.y")
pts = re.findall(r"f=(\S+)Hz.*?Rad\(XB\.y\)=(\S+) \S+∠(\S+)°", out)
f = np.array([float(a) for a, _, _ in pts])
mag = np.array([float(b) for _, b, _ in pts])
ph = np.array([float(c) for _, _, c in pts])
w = 2 * np.pi * f
exact = -0.25 / (1 - w**2 + 1j * B * w)
rel = np.abs(mag - np.abs(exact)) / np.abs(exact)
print(f"[linear] {len(f)} points: max relative |y1| error {rel.max():.1e}; "
      f"peak at f = {f[np.argmax(mag)]:.6f} (omega = 1 is {1 / (2 * np.pi):.6f}), "
      f"|y1| = {mag.max():.4f} (1/(4b) = {1 / (4 * B):.4f})")

fig, (a1, a2) = plt.subplots(2, 1, figsize=(6.4, 5.2), sharex=True)
a1.semilogy(w, np.abs(exact), "k-", lw=1, label="closed form, eq. (13b)")
a1.semilogy(w[::8], mag[::8], "o", ms=3.5, mfc="none", color="C0", label="simulated (.ac)")
a1.set_ylabel("|y₁ / p₁|")
a1.legend(frameon=False)
a2.plot(w, np.degrees(np.angle(exact)) % 360, "k-", lw=1)
a2.plot(w[::8], ph[::8] % 360, "o", ms=3.5, mfc="none", color="C0")
a2.set_ylabel("phase (deg)")
a2.set_xlabel("ω / Ω₀")
fig.suptitle("One bubble, small signal (μ = 0.002, ν = 0.015)", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/bubble_linear.svg")

# --- 1b: fixed amplitudes -----------------------------------------------------------------------
g_c = floquet_threshold()
print(f"[single] period-1 orbit unstable from g_c = {g_c:.5f} (Floquet multiplier -1)")
fixed = poincare(run("bubble_single.net", "--tran", "--reltol", "1e-5", "--report", "p1"))
worst = 0.0
fixed_pts = []
for k, g in enumerate(G_SINGLE, 1):
    t, yv = fixed[f"XB{k}"]
    ys = scipy_samples(lambda _t, g=g: g, t)
    pv, ps = period(yv), period(ys)
    # Compare the sets of values on the final orbit: an orbit of period n may be entered at a
    # different point of its cycle by the two integrators.
    n = ps or 1
    err = np.abs(np.sort(yv[-n:]) - np.sort(ys[-n:])).max() if pv == ps else np.inf
    worst = max(worst, err)
    fixed_pts.append((g, yv[-32:], ys[-32:]))
    print(f"[single] g = {g:4.2f}: period {pv} (scipy {ps}), orbit values within {err:.1e}")
print(f"[single] worst orbit difference {worst:.1e}")

# --- 1c: slow sweep -----------------------------------------------------------------------------
t, yr = poincare(run("bubble_ramp.net", "--tran", "--reltol", "1e-5", "--report", "p"))["XB"]
ys = scipy_samples(lambda tt: GDOT * tt, t)
g = GDOT * t
after = g > 0.3
print(f"[ramp] jump to period 2 at g = {jump(g, yr):.4f} (scipy {jump(g, ys):.4f}); "
      f"for g > 0.3 the samples follow scipy's within {np.abs(yr - ys)[after].max():.1e}")

fig, ax = plt.subplots(figsize=(6.4, 4.2))
ax.plot(g, yr, ",", color="C0", alpha=0.6)
ax.plot([], [], ".", color="C0", label="slow sweep, simulated")
for i, (gg, yv, ysc) in enumerate(fixed_pts):
    if gg > 1.0:
        continue
    ax.plot(np.full(yv.size, gg), yv, "o", ms=5, mfc="none", color="C1",
            label="fixed g, simulated" if i == 0 else None)
    ax.plot(np.full(ysc.size, gg), ysc, "+", ms=6, color="k",
            label="fixed g, scipy" if i == 0 else None)
ax.axvline(g_c, color="0.5", ls="--", lw=1)
ax.text(g_c + 0.01, 1.33, f"g_c = {g_c:.3f}\n(Floquet)", fontsize=8, color="0.3")
ax.set_xlabel("drive amplitude g")
ax.set_ylabel("radius y at t = 2πn/ω")
ax.set_title("One bubble at ω = 2: period doubling, and a delayed jump in a sweep", fontsize=10)
ax.legend(frameon=False, loc="lower right", fontsize=8)
fig.tight_layout()
fig.savefig(f"{OUT}/bubble_bifurcation.svg")
print(f"wrote {OUT}/bubble_linear.svg and {OUT}/bubble_bifurcation.svg")
