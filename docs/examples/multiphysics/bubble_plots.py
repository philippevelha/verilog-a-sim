"""Draw the bubble decks of circuits/multiphysics/bubble_*.net, and check them.

  python docs/examples/multiphysics/bubble_plots.py [path/to/va-cli]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/bubble_linear.svg -- the small-signal radius response against the
      closed form of the paper's eq. (13b);
  docs/examples/multiphysics/bubble_bifurcation.svg -- the stroboscopic radius against the drive
      amplitude at omega = 2: a slow sweep, the fixed-amplitude orbits, scipy's, and the Floquet
      threshold of the period-1 orbit.
  docs/examples/multiphysics/bubble_cloud_response.svg -- the cloud's response function G(omega)
      (Fig. 8) against the ladder's exact solution, the paper's continuous eq. (38) and eq. (52);
  docs/examples/multiphysics/bubble_cloud_profiles.svg -- the pressure amplitude and in-phase part
      along the cloud at omega = 0.85, 0.97, 2.0 (Figs. 5-7), against eq. (38).
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

# --- 2: the cloud, linear ------------------------------------------------------------------------
# bubble_cloud_*.net: N = 199 cells at dx = 1 between two boundary nodes at x = -L, L (L = 100),
# alpha0 = 0.001. q is the pressure perturbation's complex amplitude per unit drive.
A0, N, L = 0.001, 199, 100
XS = np.arange(1, N + 1) - 100                         # node p<i> sits at x = i - 100


def h2(w):
    """eq. (36b) at s = i omega: q_xx = h^2 q."""
    return -3 * A0 * w * w / (1 - w * w + 1j * B * w)


def q_ladder(w):
    """The discrete cloud's exact steady state: q[i+1] - (2 + h^2) q[i] + q[i-1] = 0, q = 1 at
    both boundaries (A3 linearised; dx = 1)."""
    m = (np.diag(np.full(N, 2 + h2(w))) - np.eye(N, k=1) - np.eye(N, k=-1)).astype(complex)
    r = np.zeros(N, complex)
    r[0] = r[-1] = 1
    return np.linalg.solve(m, r)


def q_paper(w, x):
    """The continuous cloud, eq. (38) at s = i omega: cosh(h x)/cosh(h L)."""
    h = np.sqrt(h2(w) + 0j)
    return np.cosh(h * x) / np.cosh(h * L)


def ac_grid(deck):
    """The deck's frequencies from its `.ac lin` card (the printout carries 7 digits only)."""
    n, f0, f1 = re.search(r"\.ac lin (\d+) (\S+) (\S+)", open(f"{DECKS}/{deck}").read()).groups()
    return 2 * np.pi * np.linspace(float(f0), float(f1), int(n))


def ac_nodes(line):
    return {int(i): float(a) * np.exp(1j * np.radians(float(p)))
            for i, a, p in re.findall(r"Pb\(p(\d+)\)=(\S+) \S+∠(\S+)°", line)}


def omega_n(n):
    """eq. (52): the undamped resonances of the continuous cloud, n odd."""
    return (1 + 12 * L * L * A0 / (n * n * np.pi**2)) ** -0.5


def omega_n_ladder(n):
    """The same for the discrete ladder: h^2 = -4 sin^2(n pi / 2(N+1))."""
    c = 4 * np.sin(n * np.pi / (2 * (N + 1))) ** 2
    return np.sqrt(c / (3 * A0 + c))


def beta(w):
    """eq. (43c-d): beta+ sets the wavelength, beta- the decay into the cloud."""
    d = (1 - w * w) ** 2 + (w * B) ** 2
    k = 3 * A0 / (2 * d)
    return (w * np.sqrt(k * (np.sqrt(d) + (1 - w * w))),
            w * np.sqrt(k * (np.sqrt(d) - (1 - w * w))))


# 2a: the response function G = |q(0)|, eqs. (49)-(50), Fig. 8.
w = ac_grid("bubble_cloud_response.net")
lines = [ln for ln in run("bubble_cloud_response.net", "--ac", "--report", "p100").splitlines()
         if ln.lstrip().startswith("f=")]
assert len(lines) == w.size
g_sim = np.array([abs(ac_nodes(ln)[100]) for ln in lines])
g_lad = np.array([abs(q_ladder(x)[99]) for x in w])
g_pap = np.abs(q_paper(w, 0.0))
print(f"[cloud] G against the ladder's exact solution: within {np.max(np.abs(g_sim - g_lad) / g_lad):.1e} "
      f"relative at {w.size} points")
for lo, hi in [(0.1, 0.95), (0.95, 1.0)]:
    s = (w >= lo) & (w < hi)
    print(f"[cloud] ladder against the continuous eq. (38), omega in [{lo}, {hi}): "
          f"{np.max(np.abs(g_lad - g_pap)[s] / g_pap[s]):.1e} relative")
peaks = [i for i in range(1, w.size - 1) if g_sim[i - 1] < g_sim[i] > g_sim[i + 1]]
for n, i in zip(range(1, 99, 2), peaks):
    y0, y1, y2 = np.log(g_sim[i - 1:i + 2])
    wp = w[i] + 0.5 * (y0 - y2) / (y0 - 2 * y1 + y2) * (w[1] - w[0])
    print(f"[cloud] peak {n}: omega = {wp:.5f}, G = {g_sim[i]:.2f} on the grid; undamped "
          f"eq. (52) {omega_n(n):.5f}, ladder {omega_n_ladder(n):.5f}")

fig, ax = plt.subplots(figsize=(6.4, 4.0))
ax.semilogy(w, g_pap, "-", color="0.6", lw=2.2, label="continuous, eq. (38)")
ax.semilogy(w, g_lad, "k-", lw=0.8, label="ladder, exact")
ax.semilogy(w[::12], g_sim[::12], "o", ms=2.5, mfc="none", color="C0", label="simulated (.ac)")
for n in range(1, 12, 2):
    ax.axvline(omega_n(n), color="C3", lw=0.6, ls=":")
ax.plot([], [], color="C3", lw=0.6, ls=":", label="eq. (52), n = 1, 3, 5 …")
ax.set_xlabel("ω / Ω₀")
ax.set_ylabel("G = |q(0)| / g")
ax.set_title("Cloud response, L = 100, α₀ = 0.001, b = 0.017 (Fig. 8)", fontsize=10)
ax.legend(frameon=False, fontsize=8)
fig.tight_layout()
fig.savefig(f"{OUT}/bubble_cloud_response.svg")

# 2b: the pressure along the cloud, eqs. (43)-(48), Figs. 5-7.
w = ac_grid("bubble_cloud_profiles.net")
report = ",".join(f"p{i}" for i in range(1, N + 1))
lines = [ln for ln in run("bubble_cloud_profiles.net", "--ac", "--report", report).splitlines()
         if ln.lstrip().startswith("f=")]
assert len(lines) == w.size
fig, axes = plt.subplots(3, 1, figsize=(6.4, 6.6), sharex=True)
for ax, wt, figno in zip(axes, (0.85, 0.97, 2.0), (5, 6, 7)):
    k = int(np.argmin(np.abs(w - wt)))
    v = ac_nodes(lines[k])
    q = np.array([v[i] for i in range(1, N + 1)])
    ql, qp = q_ladder(w[k]), q_paper(w[k], XS)
    bp, bm = beta(w[k])
    edge = slice(0, 30)
    decay = -np.polyfit(XS[edge] + L, np.log(np.abs(q[edge])), 1)[0]
    print(f"[cloud] omega = {w[k]:.2f}: profile within {np.abs(q - ql).max():.1e} of the ladder's "
          f"(the 0.01 deg phase printout), ladder within {np.abs(ql - qp).max():.1e} of eq. (38); "
          f"|q(0)| = {abs(q[99]):.3e}; beta+ = {bp:.5f} (wavelength {2 * np.pi / bp:.1f}), "
          f"beta- = {bm:.5f}" + (f"; decay fitted at the edge {decay:.5f}" if wt > 0.9 else ""))
    ax.plot(XS, np.abs(qp), "-", color="0.6", lw=2.2, label="A(x), eq. (38)")
    ax.plot(XS, qp.real, "-", color="0.75", lw=1.2, label="R(x), eq. (38)")
    ax.plot(XS[::3], np.abs(q[::3]), "o", ms=2.5, mfc="none", color="C0", label="A(x), simulated")
    ax.plot(XS[::3], q.real[::3], "+", ms=3.5, color="C1", label="R(x), simulated")
    ax.set_ylabel("q / g")
    ax.set_title(f"ω = {w[k]:.2f} (Fig. {figno})", fontsize=9)
axes[-1].legend(frameon=False, fontsize=7, ncol=2, loc="upper center")
axes[-1].set_xlabel("x / R₀")
fig.tight_layout()
fig.savefig(f"{OUT}/bubble_cloud_profiles.svg")
print(f"wrote {OUT}/bubble_cloud_response.svg and {OUT}/bubble_cloud_profiles.svg")
