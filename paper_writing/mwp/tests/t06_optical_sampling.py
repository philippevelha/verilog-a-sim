#!/usr/bin/env python3
"""T6 -- optical sampling of RF signals and processing of the optical samples
(photonic analog-to-digital conversion; Valley, Opt. Express 15, 2007; Khilo et al.,
Opt. Express 20, 2012).

    mode-locked laser -> dual-output MZM (RF) -> [demultiplexer] -> photodiodes -> integrating
    capacitors (1 V per pulse), read between pulses

The charge is integrated in the circuit, not in post-processing: va-cli prints time with 7
significant digits (0.1 ps at 256 ns), too coarse to integrate a 2 ps pulse afterwards.

 (a) sampling: 1 GS/s, 2 ps pulses, a 10.36 GHz tone (sub-sampled, coherent record of 64).
     Pulse energies at the two modulator outputs against the closed form (the modulator
     transfer averaged over the Gaussian pulse). Processing of the samples:
       x_k = (E1 - E2)/(E1 + E2) = <cos(theta)>   removes the pulse-energy dependence;
       asin(x_k)                                   removes the modulator's sine law.
     Third harmonic before linearisation against J3(m)/J1(m); SINAD/ENOB before and after.
 (b) timing jitter: 100 fs rms per pulse, 256 samples. SNR against 1/(2 pi f sigma)^2.
 (c) time-demultiplexing for a high-rate receiver: a 2 GS/s pulse train split by a 1 GHz
     driven 1:2 optical switch onto two 4 GHz-bandwidth detectors, each read once per its own
     1 ns slot (charge integration). Channel energies, crosstalk, and the interleaved record.
"""
import numpy as np
from scipy.special import jv
from mwp import run, o, Report

rep = Report("t06_optical_sampling")
RESP, VPI, PAVG = 0.8, 4.0, 1.0
FREP, FWHM = 1e9, 2e-12
SIG = FWHM / (2 * np.sqrt(2 * np.log(2)))             # Gaussian sigma of the intensity
FLAGS = ("--reltol", "1e-6")                          # step control: the charge integral is the result
CINT = RESP * 1e-3 * PAVG / FREP                      # integrating capacitor: 1 V per unmodulated pulse


def sampler(nsamp, mcyc, va, jit=0.0, seed=1):
    fin = mcyc * FREP / nsamp
    deck = f"""* photonic sampler: {FREP / 1e9:.0f} GS/s, RF {fin / 1e9:.6f} GHz
X1 {o('a')} nu mwp_mll pavg_mw={PAVG} frep={FREP} fwhm={FWHM} t0={0.5 / FREP} jit={jit} seed0={seed}
Vrf rf gnd SIN(0 {va} {fin})
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
Vk k gnd DC 3
X3 {o('b')} a1 k mwp_pd resp={RESP}
X4 {o('bb')} a2 k mwp_pd resp={RESP}
C1 a1 gnd {CINT:.6e}
C2 a2 gnd {CINT:.6e}
R1 a1 gnd 1e13
R2 a2 gnd 1e13
.tran 50e-12 {nsamp / FREP}
.end
"""
    r = run(deck, "tran", report=["a1", "a2"], tag="t06a", flags=FLAGS)
    t = r["x"]
    edges = np.arange(nsamp + 1) / FREP                  # between pulses: the capacitor voltage is flat
    e = [CINT * np.diff(np.interp(edges, t, r["y"][lab])) for lab in ("V(a1)", "V(a2)")]
    return fin, np.array(e[0]), np.array(e[1]), r


def sinad(x, mcyc):
    """SINAD (dB) of a coherent record: bin mcyc (aliased) against everything else but DC."""
    n = len(x); X = np.abs(np.fft.rfft(x)) ** 2
    b = mcyc % n
    b = n - b if b > n // 2 else b
    return 10 * np.log10(X[b] / (np.sum(X[1:]) - X[b])), X, b

# ---------------------------------------------------------------- (a) sampling and linearisation
NS, M, VA = 64, 663, 1.6                                # m = 0.4 pi
fin, e1, e2, r = sampler(NS, M, VA)
m = np.pi * VA / VPI
tk = (np.arange(NS) + 0.5) / FREP
u = np.linspace(-6, 6, 2401) * SIG                      # quadrature over the pulse
g = np.exp(-u ** 2 / (2 * SIG ** 2)); g /= g.sum()
th = m * np.sin(2 * np.pi * fin * (tk[:, None] + u[None, :])) + np.pi / 2
qp = RESP * 1e-3 * PAVG / FREP                          # charge of one unmodulated pulse
ref1 = qp * np.sum(g * np.cos(th / 2) ** 2, axis=1)
ref2 = qp * np.sum(g * np.sin(th / 2) ** 2, axis=1)
rep.check("pulse charge, output 1: max |E - closed form| / E_pulse", float(np.max(abs(e1 - ref1)) / qp), 0.0, 2e-5, rel=False)
rep.check("pulse charge, output 2: max |E - closed form| / E_pulse", float(np.max(abs(e2 - ref2)) / qp), 0.0, 2e-5, rel=False)
rep.check("pulse energy conserved between the outputs: max |E1 + E2 - E_pulse| / E_pulse", float(np.max(abs(e1 + e2 - qp)) / qp), 0.0, 2e-5, rel=False)
x = (e1 - e2) / (e1 + e2)                               # = -<sin(m sin(w t))>
s_raw, X, b1 = sinad(x, M)
b3 = (3 * M) % NS; b3 = NS - b3 if b3 > NS // 2 else b3
ap = lambda k: np.exp(-(2 * np.pi * k * fin * SIG) ** 2 / 2)          # aperture roll-off of harmonic k
rep.check("third harmonic / fundamental before linearisation: J3(m)/J1(m) with the pulse aperture",
          np.sqrt(X[b3] / X[b1]), jv(3, m) * ap(3) / (jv(1, m) * ap(1)), 2e-3)
y = np.arcsin(x) / m                                     # linearised samples: -sin(w t_k)
s_lin, X2, _ = sinad(y, M)
rep.note(f"SINAD of the raw difference/sum samples {s_raw:.1f} dB ({(s_raw - 1.76) / 6.02:.1f} bit); "
         f"after arcsin {s_lin:.1f} dB ({(s_lin - 1.76) / 6.02:.1f} bit)")
rep.check("raw SINAD = third-harmonic limit -20 log10(J3/J1) (higher harmonics included)", s_raw,
          -10 * np.log10(sum((jv(k, m) * ap(k)) ** 2 for k in (3, 5, 7)) / (jv(1, m) * ap(1)) ** 2), 0.05, rel=False, unit="dB")
rep.check("linearised samples: amplitude = aperture factor exp(-(w sigma)^2/2)", 2 * np.sqrt(X2[b1]) / NS, ap(1), 3e-3)
rep.check("linearised SINAD above 60 dB (limit: the finite pulse inside the sine law)", float(s_lin > 60.0), 1.0, 0, rel=False)
rep.note(f"{r['points']} time points for {NS} pulses, {r['wall_s']:.1f} s")

# ---------------------------------------------------------------- (b) jitter-limited SNR
NJ, MJ, JIT = 256, 2653, 100e-15                        # 10.363 GHz
fin, e1, e2, r = sampler(NJ, MJ, 0.4, jit=JIT, seed=3)
x = np.arcsin((e1 - e2) / (e1 + e2))
s_j, _, _ = sinad(x, MJ)
snr_ref = -20 * np.log10(2 * np.pi * fin * JIT)
rep.check(f"jitter {JIT * 1e15:.0f} fs rms at {fin / 1e9:.2f} GHz: SNR vs 1/(2 pi f sigma)^2 ({NJ} samples: +-0.6 dB statistical)",
          s_j, snr_ref, 1.5, rel=False, unit="dB")
rep.note(f"jitter-limited resolution {(s_j - 1.76) / 6.02:.2f} bit; {r['points']} points, {r['wall_s']:.1f} s")

# ---------------------------------------------------------------- (c) 1:2 time demultiplexing
FR2, NS2, M2, VA2, FPD = 2e9, 64, 331, 1.6, 4e9         # RF 10.34 GHz, 2 GS/s aggregate
fin2 = M2 * FR2 / NS2
T0 = 0.25 / FR2 * 2                                     # first pulse at 0.25 ns
ph_deg = 90.0 - 360.0 * (FR2 / 2) * T0                  # switch drive: +1 at even pulses, -1 at odd
q2 = RESP * 1e-3 * PAVG / FR2
deck = f"""* 2 GS/s optical samples demultiplexed 1:2
X1 {o('a')} nu mwp_mll pavg_mw={PAVG} frep={FR2} fwhm={FWHM} t0={T0}
Vrf rf gnd SIN(0 {VA2} {fin2})
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
Vsw sw gnd SIN(0.5 0.5 {FR2 / 2} 0 0 {ph_deg})
X3 {o('b')} gnd gnd {o('c')} {o('d')} sw mwp_switch vsw=1
Vk k gnd DC 3
X4 {o('c')} a1 k mwp_pd resp={RESP} f3db={FPD}
X5 {o('d')} a2 k mwp_pd resp={RESP} f3db={FPD}
C1 a1 gnd {q2:.6e}
C2 a2 gnd {q2:.6e}
R1 a1 gnd 1e13
R2 a2 gnd 1e13
.tran 25e-12 {NS2 / FR2 + 1e-9}
.end
"""
r = run(deck, "tran", report=["a1", "a2"], tag="t06c", flags=FLAGS)
t = r["x"]
ch = [q2 * r["y"]["V(a1)"], q2 * r["y"]["V(a2)"]]         # collected charge
tk = T0 + np.arange(NS2) / FR2
th = m * np.sin(2 * np.pi * fin2 * (tk[:, None] + u[None, :])) + np.pi / 2
ref = q2 * np.sum(g * np.cos(th / 2) ** 2, axis=1)
# switch drive = 1 (cross, a -> d) at even pulses, 0 (bar, a -> c) at odd ones; each channel is
# read over its own 1 ns slot starting 0.1 ns before its pulse
slot = lambda q, k: np.interp(tk[k] + 0.9e-9, t, q) - np.interp(tk[k] - 0.1e-9, t, q)
ev, od = np.arange(0, NS2, 2), np.arange(1, NS2, 2)
e_even = np.array([slot(ch[1], k) for k in ev]); e_odd = np.array([slot(ch[0], k) for k in od])
rep.check("channel D (even pulses): max |E - closed form| / E_pulse", float(np.max(abs(e_even - ref[ev])) / q2), 0.0, 2e-5, rel=False)
rep.check("channel C (odd pulses): max |E - closed form| / E_pulse", float(np.max(abs(e_odd - ref[od])) / q2), 0.0, 2e-5, rel=False)
# crosstalk: charge channel C collects in the 0.4 ns around an even pulse (which belongs to D);
# the detector tail of C's own previous pulse has decayed by exp(-0.3 ns / 40 ps) = 5e-4 there
xt = max(abs(np.interp(tk[k] + 0.2e-9, t, ch[0]) - np.interp(tk[k] - 0.2e-9, t, ch[0])) for k in ev[1:]) / q2
rep.check("charge reaching channel C around a pulse routed to D / E_pulse (bound: detector tail 5e-4)", xt, 0.0, 6e-4, rel=False)
inter = np.empty(NS2); inter[ev] = e_even; inter[od] = e_odd
s_i, _, _ = sinad(inter / q2, M2)
s_r, _, _ = sinad(ref / q2, M2)
rep.check("interleaved 2 GS/s record: SINAD equals that of the ideal samples", s_i, s_r, 0.2, rel=False, unit="dB")
rep.note(f"{r['points']} points, {r['wall_s']:.1f} s")
rep.done()
