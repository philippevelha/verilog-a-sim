"""Draw the loudspeaker decks of circuits/multiphysics/speaker_*.net, and check them.

  python docs/examples/multiphysics/speaker_plots.py [path/to/va-cli]

runs the three decks through va-cli and writes
  docs/examples/multiphysics/speaker_ac.svg -- input impedance and radiated power at 2.83 V, for
      64- and 256-section horns, against the exact continuous horn;
  docs/examples/multiphysics/speaker_large.svg -- cone excursion and the mouth pressure's
      harmonics under a 30 Hz tone below the horn's cutoff, nonlinear driver against linear;
  docs/examples/multiphysics/speaker_thermal.svg -- the voice coil's temperature and the sound's
      compression under a sustained tone, against the quasi-static small-signal prediction.
It prints the checks: the small-signal errors against the exact horn, the harmonic content and an
energy balance for the large-signal run, and the compression against its prediction.

The reference is the same lumped driver, front chamber and mouth on the exact continuous
exponential horn: the Webster equation's solutions p = exp((-m/2 +- j beta) z), beta^2 = k^2 - m^2/4,
give the horn's transfer matrix in closed form.
"""
import math
import re
import subprocess
import sys

import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

VA = sys.argv[1] if len(sys.argv) > 1 else "target/release/va-cli"
MODEL = "models/multiphysics/speaker"
OUT = "docs/examples/multiphysics"

# The models' defaults (models/multiphysics/speaker/speaker_parts.va).
RHO, C = 1.2, 343.0
RE20, LE, BL, MMS, RMS, KMS, SD, VB = 6.0, 0.5e-3, 15.0, 0.06, 2.0, 3000.0, 0.05, 0.1
VF, ST, SM, LEN = 2e-3, 0.02, 3.0, 3.42
RR = RHO * C / SM                                         # mouth radiation resistance
MR = 8 * RHO / (3 * math.pi ** 2 * math.sqrt(SM / math.pi))  # mouth radiation mass


def horn_matrix(w):
    """Exact transfer matrix of the continuous exponential horn: [p, U](len) = T [p, U](0)."""
    m, k = math.log(SM / ST) / LEN, w / C
    beta = np.sqrt(complex(k * k - m * m / 4))

    def basis(z):
        s = ST * math.exp(m * z)
        cols = []
        for sign in (1, -1):
            g = -m / 2 + sign * 1j * beta
            p = np.exp(g * z)
            cols.append([p, -s / (1j * w * RHO) * g * p])   # U = -S/(j w rho) dp/dz
        return np.array(cols).T

    return basis(LEN) @ np.linalg.inv(basis(0.0))


def reference(f, volts, re_ohm=RE20):
    """Exact small-signal response: (coil current, mouth pressure, radiated power, cone velocity)."""
    w = 2 * math.pi * f
    zm = 1 / (1 / RR + 1 / (1j * w * MR))
    pu = np.linalg.solve(horn_matrix(w), np.array([zm, 1.0]))   # [p, U] at the throat per U_mouth
    zt = pu[0] / pu[1]
    zf = 1 / (1 / zt + 1j * w * VF / (RHO * C * C))             # the front chamber in parallel
    zmech = 1j * w * MMS + RMS + (KMS + SD * SD * RHO * C * C / VB) / (1j * w) + SD * SD * zf
    i = volts / (re_ohm + 1j * w * LE + BL * BL / zmech)
    vel = BL * i / zmech
    pm = zm * (SD * vel * zf / zt) / pu[1]
    return i, pm, abs(pm) ** 2 / (2 * RR), vel


def run(deck, analysis, report):
    return subprocess.Popen(
        [VA, "sim", f"circuits/multiphysics/{deck}.net", "--model", MODEL, analysis,
         "--report", report],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
        errors="replace",
    )


def tran_rows(proc, keep=lambda line: None):
    rows = []
    for line in proc.stdout:
        if line.startswith("  t="):
            toks = line.split()
            row = {"t": float(toks[0][2:-1])}
            for tok in toks[1:]:
                if "=" in tok:
                    k, v = tok.split("=")
                    row[k] = float(v)
            rows.append(row)
        else:
            keep(line)
    proc.wait()
    return rows


# --- small signal ------------------------------------------------------------------------------
AC_ITEM = re.compile(r"(\S+)=(\S+) \S*?∠(\S+)°")
proc = run("speaker_ac", "--ac", "V1,V2,m1,m2")
f, data = [], {}
for line in proc.stdout:
    if line.startswith("  f="):
        f.append(float(line.split()[0][2:-2]))
        for name, mag, ph in AC_ITEM.findall(line):
            data.setdefault(name, []).append(float(mag) * np.exp(1j * math.radians(float(ph))))
proc.wait()
f = np.array(f)
ref = [reference(x, 2.83) for x in f]
z_ref = np.array([2.83 / abs(r[0]) for r in ref])
w_ref = np.array([r[2] for r in ref])
pm_ref = np.array([r[1] for r in ref])
print("small signal, 2.83 V: radiated power and mouth phase against the exact continuous horn")
print(f"{'f (Hz)':>8s} {'exact (W)':>10s} {'N=64':>8s} {'N=256':>8s}   {'phase err 64':>12s} {'256':>6s}")
for target in (20, 40, 100, 250, 500, 1000):
    j = int(np.argmin(abs(f - target)))
    row = f"{f[j]:8.1f} {w_ref[j]:10.5f}"
    for name in ("m1", "m2"):
        pm = data[f"Pac({name})"][j]
        row += f" {100 * (abs(pm) ** 2 / (2 * RR) / w_ref[j] - 1):+7.2f}%"
    row += "  "
    for name in ("m1", "m2"):
        dphi = math.degrees(np.angle(data[f"Pac({name})"][j] / pm_ref[j]))
        row += f" {dphi:+8.1f} deg"
    print(row)
zmax = max(np.max(abs(2.83 / abs(np.array(data["I(V2)"])) / z_ref - 1)), 0)
print(f"input impedance, N = 256: largest deviation from exact {100 * zmax:.3f} %")

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.4, 6.0), sharex=True)
ax1.semilogx(f, 2.83 / abs(np.array(data["I(V2)"])), lw=2, label="simulated, N = 256")
ax1.semilogx(f, z_ref, "k:", lw=1.2, label="exact continuous horn")
ax1.set_ylabel("|Z| (ohm)")
ax1.legend(fontsize=8)
ax1.grid(True, which="both", alpha=0.3)
for name, lab in (("m1", "simulated, N = 64"), ("m2", "simulated, N = 256")):
    ax2.semilogx(f, 10 * np.log10(abs(np.array(data[f"Pac({name})"])) ** 2 / (2 * RR)), label=lab)
ax2.semilogx(f, 10 * np.log10(w_ref), "k:", lw=1.2, label="exact continuous horn")
ax2.axvline(40, color="grey", lw=0.8, ls="--")
ax2.text(41, -45, "horn cutoff", fontsize=7, color="grey")
ax2.set_ylabel("radiated power at 2.83 V (dB re 1 W)")
ax2.set_xlabel("frequency (Hz)")
ax2.set_ylim(-50, -5)
ax2.legend(fontsize=8)
ax2.grid(True, which="both", alpha=0.3)
fig.suptitle("Horn-loaded driver: small signal", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/speaker_ac.svg")

# --- large signal ------------------------------------------------------------------------------
F0, VPK = 30.0, 20.0
rows = tran_rows(run("speaker_large", "--tran", "V1,m1,X1.x,X1.v,th1,V2,m2,X2.x,X2.v,th2"))
t = np.array([r["t"] for r in rows])
win = t >= t[-1] - 10 / F0 - 1e-12          # the last ten cycles
tw = t[win]
uniform = np.linspace(tw[0], tw[0] + 10 / F0, 4096, endpoint=False)


def harmonics(y):
    spec = np.abs(np.fft.rfft(np.interp(uniform, t, y))) * 2 / len(uniform)
    return np.array([spec[10 * h] for h in range(1, 8)])   # 10 cycles: harmonic h in bin 10 h


fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(9.0, 3.8))
print(f"\nlarge signal, {VPK:.0f} V at {F0:.0f} Hz, last ten cycles:")
for k, lab in ((1, "nonlinear"), (2, "linear")):
    col = {name: np.array([r[f"{name}"] for r in rows]) for name in
           (f"I(V{k})", f"Vel(X{k}.v)", f"Pos(X{k}.x)", f"Temp(th{k})", f"Pac(m{k})")}
    i = -col[f"I(V{k})"]                     # current out of the source's + terminal
    v, x, pm = col[f"Vel(X{k}.v)"], col[f"Pos(X{k}.x)"], col[f"Pac(m{k})"]
    r_coil = RE20 * (1 + 0.00393 * (col[f"Temp(th{k})"] - 293.15))
    vin = VPK * np.sin(2 * math.pi * F0 * t)

    def energy(y):
        return np.trapezoid(y[win], tw)

    e_in, e_j, e_m, e_r = (energy(vin * i), energy(r_coil * i * i), energy(RMS * v * v),
                           energy(pm * pm / RR))
    hx, hp = harmonics(x), harmonics(pm)
    print(f"  {lab:9s}: peak excursion {1e3 * np.max(abs(x[win])):.2f} mm; excursion 3rd/5th "
          f"harmonic {100 * hx[2] / hx[0]:.2f} % / {100 * hx[4] / hx[0]:.2f} %; mouth pressure "
          f"fundamental {hp[0]:.3f} Pa, 3rd {hp[2]:.3f} Pa, 5th {hp[4]:.3f} Pa")
    print(f"  {'':9s}  energy in {e_in:.4f} J = Joule {e_j:.4f} + mechanical {e_m:.4f} + radiated "
          f"{e_r:.4f} J, residual {100 * (e_in - e_j - e_m - e_r) / e_in:+.3f} %")
    last = t >= t[-1] - 2 / F0
    ax1.plot((t[last] - t[last][0]) * 1e3, x[last] * 1e3, label=lab)
    ax2.bar(np.arange(1, 8) + (-0.2 if k == 1 else 0.2), hp, width=0.4, label=lab)
i0, _, _, vel0 = reference(F0, VPK)
print(f"  linear small-signal excursion at {F0:.0f} Hz: {1e3 * abs(vel0) / (2 * math.pi * F0):.2f} mm")
ax1.set_xlabel("time (ms)")
ax1.set_ylabel("cone excursion (mm)")
ax1.legend(fontsize=8)
ax1.grid(True, alpha=0.3)
ax2.set_xlabel(f"harmonic of {F0:.0f} Hz")
ax2.set_ylabel("mouth pressure amplitude (Pa)")
ax2.legend(fontsize=8)
ax2.grid(True, axis="y", alpha=0.3)
fig.suptitle(f"{VPK:.0f} V at {F0:.0f} Hz, below the horn's 40 Hz cutoff", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/speaker_large.svg")

# --- thermal compression -----------------------------------------------------------------------
F1, V1 = 200.0, 40.0
logs = []
rows = tran_rows(run("speaker_thermal", "--tran", "m,th"),
                 keep=lambda line: logs.append(line.strip()) if "coil" in line else None)
for line in logs:
    print("  " + line)
t = np.array([r["t"] for r in rows])
pm = np.array([r["Pac(m)"] for r in rows])
temp = np.array([r["Temp(th)"] for r in rows])
edges = np.arange(0.0, t[-1], 1 / F1)
tc, amp, tcoil = [], [], []
idx = np.searchsorted(t, edges)
for a, b in zip(idx[:-1], idx[1:]):
    if b - a > 4:
        tc.append(t[a:b].mean())
        amp.append((pm[a:b].max() - pm[a:b].min()) / 2)
        tcoil.append(temp[a:b].mean())
tc, amp, tcoil = np.array(tc), np.array(amp), np.array(tcoil)
pred = np.array([abs(reference(F1, V1, RE20 * (1 + 0.00393 * (tk - 293.15)))[1]) for tk in tcoil])
# Both curves referred to their own value at t0 = 0.1 s, once the horn has settled: the transient
# carries a fixed offset of its own (the 32-section ladder against the continuous horn, the step),
# and the check is that it stays fixed while the coil heats.
j0 = int(np.argmin(abs(tc - 0.1)))
comp_sim, comp_pred = 20 * np.log10(amp / amp[j0]), 20 * np.log10(pred / pred[j0])
print(f"\nthermal compression, {V1:.0f} V at {F1:.0f} Hz (output change from t = 0.1 s):")
for when in (0.1, 1, 2, 4, 8, 11.9):
    j = int(np.argmin(abs(tc - when)))
    print(f"  t = {tc[j]:5.1f} s: coil {tcoil[j] - 273.15:6.1f} C, mouth {amp[j]:.4f} Pa, quasi-static "
          f"{pred[j]:.4f} Pa (ratio {amp[j] / pred[j]:.4f}); change {comp_sim[j]:+.3f} dB, predicted "
          f"{comp_pred[j]:+.3f} dB")
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(6.4, 5.6), sharex=True)
ax1.plot(tc, tcoil - 273.15, lw=2)
ax1.set_ylabel("voice-coil temperature (C)")
ax1.grid(True, alpha=0.3)
late = tc >= tc[j0]   # before t0 the sound is still on its way down the horn
ax2.plot(tc[late], comp_sim[late], lw=2, label="simulated (transient)")
ax2.plot(tc[late], comp_pred[late], "k:", lw=1.2, label="small-signal at the coil's R(T)")
ax2.set_ylabel("output change from t = 0.1 s (dB)")
ax2.set_xlabel("time (s)")
ax2.legend(fontsize=8)
ax2.grid(True, alpha=0.3)
fig.suptitle(f"Thermal compression: {V1:.0f} V at {F1:.0f} Hz", fontsize=10)
fig.tight_layout()
fig.savefig(f"{OUT}/speaker_thermal.svg")
print(f"wrote {OUT}/speaker_ac.svg, speaker_large.svg, speaker_thermal.svg")
