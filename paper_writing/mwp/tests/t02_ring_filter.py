#!/usr/bin/env python3
"""T2 -- RF processing: the microring and the single-passband microwave photonic filter.

 (a) static through/drop spectrum of an add-drop ring against the coupled-mode Lorentzian;
 (b) phase modulator + notch ring + photodiode (Palaci et al., PTL 22, 2010; Minasian et al.,
     Opt. Express 21, 2013, Sec. 4.1): phase modulation gives no photocurrent until the ring
     removes one sideband, so the RF response is a single passband at the laser-ring detuning.
     AC response against the closed form built from the ring's optical transfer function,
     at two tuning voltages (the passband follows the ring);
 (c) the same filter in large-signal transient at the passband centre, against the AC value.
"""
import numpy as np
from mwp import run, o, tones, Report

rep = Report("t02_ring_filter")
NU = 193.4                                     # THz
P, RESP, VPI = 1.0, 0.8, 4.0


def ring_h(det_hz, q1, q2, qi, nu_res=NU):
    """Through and drop field transfer at detuning det = nu_res_eff - nu_light (Hz)."""
    w0 = 2 * np.pi * nu_res * 1e12
    r1 = w0 / (2 * q1); r2 = w0 / (2 * q2) if q2 else 0.0; ri = w0 / (2 * qi) if qi else 0.0
    tau = 1 / (r1 + r2 + ri)
    den = 1 - 1j * 2 * np.pi * det_hz * tau
    return 1 - 2 * r1 * tau / den, -np.sqrt(r2 / r1) * 2 * r1 * tau / den

# ---------------------------------------------------------------- (a) static spectrum
Q1, Q2, QI, KF = 2e4, 2e4, 1e5, 1e9            # tuning 1 GHz/V
deck = f"""* add-drop ring, tuning sweep
X1 {o('a')} nu mwp_laser p_mw={P} nu0={NU}
Vt tune gnd DC 0
X2 {o('a')} {o('t')} {o('d')} nu tune mwp_ring nu_res={NU} q1={Q1} q2={Q2} qi={QI} kf={KF}
Vk k gnd DC 3
X3 {o('t')} at k mwp_pd resp=1
X4 {o('d')} ad k mwp_pd resp=1
Vst at gnd DC 0
Vsd ad gnd DC 0
.dc Vt -60 60 0.5
.end
"""
r = run(deck, "dc", tag="t02a")
ht, hd = ring_h(KF * r["x"], Q1, Q2, QI)
it, idr = r["y"]["I(Vst)"] * 1e3, r["y"]["I(Vsd)"] * 1e3              # = mW (resp 1)
rep.check("through spectrum, +-60 GHz: max |T - Lorentzian|", float(np.max(np.abs(it - abs(ht) ** 2))), 0.0, 1e-6, rel=False)
rep.check("drop spectrum: max |D - Lorentzian|", float(np.max(np.abs(idr - abs(hd) ** 2))), 0.0, 1e-6, rel=False)
half = r["x"][idr >= idr.max() / 2]
ql = 1 / (1 / Q1 + 1 / Q2 + 1 / QI)
rep.check("drop FWHM = nu/Q_L (0.5 GHz grid)", half[-1] - half[0], NU * 1e3 / ql, 0.6, rel=False, unit="GHz")

# ---------------------------------------------------------------- (b) single-passband filter, AC
QC = 4e5                                        # critically coupled notch: q1 = qi
F0 = 10e9                                       # ring 10 GHz above the laser


def filt(extra, vt):
    return f"""* PM -> notch ring -> PD
X1 {o('a')} nu mwp_laser p_mw={P} nu0={NU}
X2 {o('a')} {o('b')} rf gnd mwp_pm vpi={VPI} rterm=0
Vt tune gnd DC {vt}
X3 {o('b')} {o('t')} {o('d')} nu tune mwp_ring nu_res={NU + F0 * 1e-12} q1={QC} qi={QC} kf=1e9
Vk k gnd DC 3
X4 {o('t')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
{extra}
.end
"""


def h_rf(f, f_ring):
    """Photocurrent per RF volt (complex) of PM -> filter H -> PD."""
    H = lambda off: ring_h(f_ring - off, QC, 0, QC, NU + F0 * 1e-12)[0]
    m = np.pi / VPI
    return RESP * P * 1e-3 * 1j * m * (np.conj(H(0)) * H(f) - H(0) * np.conj(H(-f)))

for vt in (0.0, 5.0):
    r = run(filt("Vrf rf gnd DC 0 AC 1\n.ac lin 381 1e9 20e9", vt), "ac", report=["Vs"], tag="t02b")
    f = r["x"]; h = r["y"]["I(Vs)"]; ref = h_rf(f, F0 + 1e9 * vt)
    pk = f[np.argmax(abs(h))]
    rep.check(f"tune {vt:.0f} V: passband centre", pk, F0 + 1e9 * vt, 0.06e9, rel=False, unit="Hz")
    rep.check(f"tune {vt:.0f} V: RF response 1-20 GHz, max |H - closed form| / |H|max",
              float(np.max(np.abs(h - ref)) / np.max(abs(ref))), 0.0, 2e-4, rel=False)
    bw = f[abs(h) >= abs(h).max() / np.sqrt(2)]
    rep.check(f"tune {vt:.0f} V: -3 dB bandwidth = ring linewidth nu/Q_L", bw[-1] - bw[0],
              (NU * 1e12 + F0) / (QC / 2), 0.11e9, rel=False, unit="Hz")
rep.note(f"peak response {abs(h).max():.4e} A/V; a phase modulator alone gives "
         f"{abs(h_rf(np.array([10e9]), 1e15))[0]:.1e} A/V")

# ---------------------------------------------------------------- (c) transient at the passband centre
VA = 0.05
r = run(filt(f"Vrf rf gnd SIN(0 {VA} {F0})\n.tran 1e-12 6e-9", 0.0), "tran", report=["Vs"], tag="t02c")
a, dc, res = tones(r["x"], r["y"]["I(Vs)"], [F0, 2 * F0], t_start=4e-9)   # ring settled: tau = 0.33 ns
# the fit returns y = Re(A e^{jwt}); the drive is sin = Re(-j e^{jwt})
rep.check("transient at 10 GHz (0.05 V drive) vs AC: |i|/V", abs(a[0]) / VA, abs(h_rf(np.array([F0]), F0))[0], 3e-3, unit="A/V")
rep.check("transient vs AC: phase", float(np.angle(a[0] / (-1j) / h_rf(np.array([F0]), F0)[0], deg=True)), 0.0, 0.5, rel=False, unit="deg")
rep.note(f"transient: {r['points']} points, {r['wall_s']:.1f} s")
rep.done()
