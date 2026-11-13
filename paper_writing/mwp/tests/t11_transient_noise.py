#!/usr/bin/env python3
"""T11 -- noise in the time domain, two ways, compared on results, run time and memory.

  LIB : the library module mwp_noise -- the model draws its own samples ($rdist_normal on a
        timer); one instance per noise source, fixed density.
  SIM : the simulator's transient noise (va-cli --tran-noise <fmax>, 2.55.0) -- every white
        source a device already declares for .noise becomes a random current, at the density
        of the solution being computed.

 (a) one resistor on one capacitor: voltage variance against kT/C, spectrum against
     4kTR / (1 + (f/fc)^2) * sinc^4(f dt), both ways; time, points, peak memory.
 (b) the same on a 50-section RC ladder (50 noise sources): every node must sit at kT/C
     (equipartition); cost of each way as the number of sources grows.
 (c) shot noise that follows the signal: an optical power stepped between two levels on a
     photodiode. SIM tracks 2 q I(t); LIB cannot (its density is a parameter), shown by what a
     fixed-density module gives.
The node voltages are microvolts and va-cli prints six decimals, so a noiseless x10^4
amplifier (an E source) reads them.
"""
import numpy as np
from scipy.signal import welch
from mwp import run, o, Report, KB, Q

rep = Report("t11_transient_noise")
T0, R, C, FMAX, GAIN = 300.15, 1e3, 1e-12, 20e9, 1e4
DT = 0.5 / FMAX
KTC = KB * T0 / C
FLAGS = ("--abstol", "1e-9")                     # step control must see microvolts


def tvar(t, v, t0=0.0):
    """Time-weighted variance (the steps are not uniform)."""
    m = t >= t0
    t, v = t[m], v[m]
    w = np.diff(t); vm = 0.5 * (v[1:] + v[:-1])
    mean = np.sum(w * vm) / w.sum()
    return np.sum(w * (vm - mean) ** 2) / w.sum()


def ladder(n, how, tstop):
    L = [f"* RC ladder, {n} section(s), noise: {how}"]
    for k in range(n):
        a = "gnd" if k == 0 else f"n{k - 1}"
        L += [f"R{k} {a} n{k} {R}", f"C{k} n{k} gnd {C}"]
        if how == "LIB":
            L.append(f"XN{k} {a} n{k} mwp_noise psd={4 * KB * T0 / R:.9e} fmax={FMAX} seed0={k + 1}")
    L += [f"E1 amp gnd n{n - 1} gnd {GAIN}", "R99 amp gnd 1e9", f".tran {DT} {tstop}", ".end"]
    r = run("\n".join(L), "tran", report=["amp"], tag=f"t11_{how}",
            flags=FLAGS + (("--tran-noise", str(FMAX)) if how == "SIM" else ()))
    return r["x"], r["y"]["V(amp)"] / GAIN, r

# ---------------------------------------------------------------- (a) one RC
TSTOP = 4e-6
cost = {}
for how in ("LIB", "SIM"):
    t, v, r = ladder(1, how, TSTOP)
    cost[how, 1] = (r["wall_s"], r["peak_mb"], r["points"])
    rep.check(f"{how}: variance on the capacitor against kT/C (4000 time constants: +-3 % statistical)",
              tvar(t, v, 10e-9), KTC, 0.10, unit="V^2")
    tu = np.arange(0, TSTOP, DT / 2)
    f, s = welch(np.interp(tu, t, v), fs=2 / DT, nperseg=8192, detrend="constant")
    fc = 1 / (2 * np.pi * R * C)
    ref = 4 * KB * T0 * R / (1 + (f / fc) ** 2) * np.sinc(f * DT) ** 4
    for fb in (20e6, 159e6, 1e9, 4e9):
        m = (f > 0.8 * fb) & (f < 1.25 * fb)
        rep.check(f"{how}: spectrum around {fb / 1e6:.0f} MHz against 4kTR/(1+(f/fc)^2) sinc^4(f dt)",
                  float(np.mean(s[m])), float(np.mean(ref[m])), 0.15, unit="V^2/Hz")
    rep.note(f"{how}, 1 source: {r['wall_s']:.1f} s, {r['points']} points, peak memory {r['peak_mb']:.0f} MB")

# ---------------------------------------------------------------- (b) 50 sources
N = 50
for how in ("LIB", "SIM"):
    t, v, r = ladder(N, how, 1e-6)
    cost[how, N] = (r["wall_s"], r["peak_mb"], r["points"])
    # the last node's slowest mode is ~N^2 RC/2: fewer independent samples, wider tolerance
    rep.check(f"{how}, {N}-section ladder: variance at the last node against kT/C (equipartition)", tvar(t, v, 0.2e-6), KTC, 0.35, unit="V^2")
    rep.note(f"{how}, {N} sources: {r['wall_s']:.1f} s, {r['points']} points, peak memory {r['peak_mb']:.0f} MB")
rep.note("cost, LIB / SIM: " + "; ".join(
    f"{n} source(s): time x{cost['LIB', n][0] / cost['SIM', n][0]:.1f}, points x{cost['LIB', n][2] / cost['SIM', n][2]:.1f}, "
    f"memory x{cost['LIB', n][1] / cost['SIM', n][1]:.2f}" for n in (1, N)))

# ---------------------------------------------------------------- (c) shot noise following the signal
P, RESP, VPI, FN, RLD = 10.0, 0.8, 4.0, 20e9, 50.0
CL = 1 / (2 * np.pi * RLD * 0.5e9)                # 500 MHz receiver, 40 times below the noise bandwidth:
# with one step per noise sample the trapezoidal rule keeps 1/(1 + pi fc/(2 fmax)) of a pole's noise
# variance (0.96 here; a 2 GHz pole lost about a quarter, which is why the receiver is not that fast)
v_hi = 2 * VPI / np.pi * np.arccos(np.sqrt(0.9)); v_lo = 2 * VPI / np.pi * np.arccos(np.sqrt(0.1))
i_hi, i_lo = RESP * P * 1e-3 * 0.9, RESP * P * 1e-3 * 0.1


def stepped(extra, flags):
    deck = f"""* photodiode under a stepped optical power, 50 ohm / 500 MHz load
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd DC 0
Vb vb gnd PULSE({v_hi:.6f} {v_lo:.6f} 1000e-9 1e-9 1e-9 1000e-9 10)
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP}
RL an gnd {RLD}
CL an gnd {CL:.6e}
E1 amp gnd an gnd {GAIN}
R99 amp gnd 1e9
{extra}
.tran {0.5 / FN} 2000e-9
.end
"""
    r = run(deck, "tran", report=["amp"], tag="t11c", flags=FLAGS + flags)
    t, v = r["x"], r["y"]["V(amp)"] / GAIN
    return tvar(t[t < 995e-9], v[t < 995e-9], 5e-9), tvar(t, v, 1010e-9), r

# load thermal noise kT/C plus shot noise 2 q I through R and the RC noise bandwidth 1/(4RC)
expect = lambda i: (KB * T0 / CL + 2 * Q * i * RLD / (4 * CL)) / (1 + np.pi * 0.5e9 / (2 * FN))
vh, vl, r = stepped("", ("--tran-noise", str(FN)))
rep.check("SIM: noise variance at the high level, kT/C + 2 q I R/(4C), with the step factor 0.96", vh, expect(i_hi), 0.10, unit="V^2")
rep.check("SIM: noise variance at the low level", vl, expect(i_lo), 0.10, unit="V^2")
rep.note(f"SIM: shot part high / low = {(vh - KB * T0 / CL) / (vl - KB * T0 / CL):.2f} (current ratio 9)")
vh2, vl2, r2 = stepped(f"XN1 gnd an mwp_noise psd={2 * Q * i_hi:.6e} fmax={FN} seed0=1\n"
                       f"XN2 gnd an mwp_noise psd={4 * KB * T0 / RLD:.6e} fmax={FN} seed0=2", ())
rep.check("LIB (two modules: load thermal, and shot at the high level's density): variance at the high level", vh2, expect(i_hi), 0.10, unit="V^2")
rep.note(f"LIB at the low level: {vl2 / expect(i_lo):.1f} times the noise that level has "
         f"(the module's density is a parameter; it does not follow the current)")
rep.note(f"(c) cost: SIM {r['wall_s']:.1f} s, {r['peak_mb']:.0f} MB; LIB {r2['wall_s']:.1f} s, {r2['peak_mb']:.0f} MB")
rep.done(dict(cost={f"{k[0]}_{k[1]}": dict(wall_s=v[0], peak_mb=v[1], points=v[2]) for k, v in cost.items()}))
