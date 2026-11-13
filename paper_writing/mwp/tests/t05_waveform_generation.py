#!/usr/bin/env python3
"""T5 -- optical generation of wideband amplitude- and phase-modulated RF signals.

 (a) vector signal: an optical IQ modulator writes 16-QAM at 1 GBd on the signal laser; a
     second laser 20 GHz away is the optical local oscillator; the photodiode delivers
         i(t) = R (A B / 2) [ s_I sin(W t) - s_Q cos(W t) ],   s = sin(pi v / (2 Vpi)),
     a 16-QAM carrier at 20 GHz. Coherent demodulation in post-processing (least-squares fit of the carrier in the
     centre half of each symbol): error vector
     against the exact symbol values, and against the ideal grid (the modulator's sine
     compression, closed form).
 (b) linear FM (radar chirp): a phase modulator driven by a parabolic voltage, heterodyned
     at 10 GHz: i(t) = -R A B sin(W t - phi(t)), phi = pi K (t - T/2)^2, a 4 GHz chirp in
     10 ns (time-bandwidth product 40). Instantaneous phase against the closed form.
"""
import numpy as np
from scipy.signal import hilbert
from mwp import run, o, pwl, resample, tones, Report

rep = Report("t05_waveform_generation")
RESP, VPI = 0.8, 4.0

# ---------------------------------------------------------------- (a) 16-QAM at 20 GHz
PS, PLO, FC, BAUD, NSYM, TR = 4.0, 4.0, 20e9, 1e9, 32, 100e-12
rng = np.random.default_rng(7)
lev = np.array([-3, -1, 1, 3])
si, sq = rng.choice(lev, NSYM), rng.choice(lev, NSYM)
V0 = 0.1 * VPI                                   # volts per level unit: peak drive 0.3 Vpi


def nrz(sym):
    t, v = [0.0], [sym[0] * V0]
    for k in range(1, NSYM):
        t += [k / BAUD, k / BAUD + TR]; v += [sym[k - 1] * V0, sym[k] * V0]
    t.append(NSYM / BAUD); v.append(sym[-1] * V0)
    return pwl(t, v)

deck = f"""* 16-QAM at {FC / 1e9:.0f} GHz by optical heterodyning
X1 {o('a')} nu1 mwp_laser p_mw={PS}
X2 {o('l')} nu2 mwp_laser p_mw={PLO} df={FC}
Vi vi gnd {nrz(si)}
Vq vq gnd {nrz(sq)}
Vbi bi gnd DC {VPI}
Vbq bq gnd DC {VPI}
Vbp bp gnd DC {VPI / 2}
X3 {o('a')} {o('s')} vi vq bi bq bp mwp_iqm vpi={VPI}
X4 {o('s')} {o('l')} {o('c')} {o('d')} mwp_coupler
Vk k gnd DC 3
X5 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / FC / 32} {NSYM / BAUD}
.end
"""
r = run(deck, "tran", report=["Vs"], tag="t05a")
t, i = r["x"], r["y"]["I(Vs)"]
amp = RESP * 1e-3 * np.sqrt(PS * PLO) / 2
rec = []
for k in range(NSYM):                            # centre 0.5 ns of each symbol: 10 carrier periods
    m = (t >= k / BAUD + 0.35e-9) & (t <= k / BAUD + 0.85e-9)
    a, dc, res = tones(t[m] - k / BAUD, i[m], [FC])       # i = Re(a e^{jWt}); FC/BAUD is an integer
    rec.append((-a[0].imag - 1j * a[0].real) / amp)
rec = np.array(rec)
exact = np.sin(np.pi * si * V0 / (2 * VPI)) + 1j * np.sin(np.pi * sq * V0 / (2 * VPI))
evm_exact = np.sqrt(np.mean(abs(rec - exact) ** 2) / np.mean(abs(exact) ** 2))
# va-cli prints time with 7 significant digits: at 30 ns that is 1e-15 s, 1.3e-4 rad of a 20 GHz
# carrier. The tolerance is that print resolution, not a model error.
rep.check("16-QAM at 20 GHz: rms error vector against the exact symbols", evm_exact, 0.0, 3e-4, rel=False)
ideal = (si + 1j * sq)


def evm_grid(x):
    g = np.sum(np.conj(ideal) * x) / np.sum(abs(ideal) ** 2)         # best-fit gain
    return np.sqrt(np.mean(abs(x - g * ideal) ** 2) / np.mean(abs(g * ideal) ** 2))

rep.check("EVM against the ideal 16-QAM grid = the modulator's sine compression (closed form)",
          evm_grid(rec), evm_grid(exact), 1e-4, rel=False)
rep.note(f"EVM on the ideal grid: {100 * evm_grid(rec):.3f} % at a peak drive of 0.3 Vpi; {r['points']} points, {r['wall_s']:.1f} s")

# ---------------------------------------------------------------- (b) linear FM chirp
PA, PB, F0, T, B = 2.0, 2.0, 10e9, 10e-9, 4e9
K = B / T
tp = np.linspace(0, T, 2001)
vph = (VPI / np.pi) * np.pi * K * (tp - T / 2) ** 2
deck = f"""* linear-FM chirp, {B / 1e9:.0f} GHz in {T * 1e9:.0f} ns around {F0 / 1e9:.0f} GHz
X1 {o('a')} nu1 mwp_laser p_mw={PA}
X2 {o('l')} nu2 mwp_laser p_mw={PB} df={F0}
Vp vp gnd {pwl(tp, vph)}
X3 {o('a')} {o('s')} vp gnd mwp_pm vpi={VPI} rterm=0
X4 {o('s')} {o('l')} {o('c')} {o('d')} mwp_coupler
Vk k gnd DC 3
X5 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / (F0 + B) / 64} {T}
.end
"""
r = run(deck, "tran", report=["Vs"], tag="t05b")
t, i = r["x"], r["y"]["I(Vs)"]
phi = lambda x: np.pi * K * (x - T / 2) ** 2
ref = RESP * 1e-3 * ((PA + PB) / 2 - np.sqrt(PA * PB) * np.sin(2 * np.pi * F0 * t - phi(t)))
rep.check("chirp: max |i - closed form| / amplitude, every time point", float(np.max(abs(i - ref)) / (RESP * 1e-3 * np.sqrt(PA * PB))), 0.0, 1e-4, rel=False)
tu, iu = resample(t, i - np.mean(i), 8192)
ph = np.unwrap(np.angle(hilbert(iu)))
finst = np.gradient(ph, tu) / (2 * np.pi)
mid = (tu > 1e-9) & (tu < 9e-9)                  # the Hilbert transform is poor at the record ends
fref = F0 - K * (tu - T / 2)
rep.check("chirp: instantaneous frequency (Hilbert), rms deviation from f0 - K(t - T/2), 1-9 ns",
          float(np.sqrt(np.mean((finst[mid] - fref[mid]) ** 2))), 0.0, 0.05e9, rel=False, unit="Hz")
slope = np.polyfit(tu[mid], finst[mid], 1)[0]
rep.check("chirp rate", slope, -K, 5e-3, unit="Hz/s")
rep.note(f"swept band {abs(slope) * T / 1e9:.2f} GHz, time-bandwidth product {abs(slope) * T * T:.1f}; "
         f"peak phase deviation {phi(0):.1f} rad = {phi(0) / np.pi:.1f} Vpi of drive")
rep.done()
