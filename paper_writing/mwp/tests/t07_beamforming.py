#!/usr/bin/env python3
"""T7 -- optical beamforming: true-time delay against phase shifting.

 (a) dispersive WDM true-time-delay beamformer: four carriers 100 GHz apart, each with its
     own modulator (the same RF), share a dispersive fibre and are detected separately. The
     element delays are D*L*(lambda_n - lambda_0); the RF phase of element n must be
     -2 pi f tau_n at every frequency, 2-20 GHz. The array factor built from the simulated
     element responses points at the same angle at every frequency (no squint).
 (b) photonic RF phase shifter (the principle of Minasian et al. 2013, Fig. 1-2: the optical
     phase between carrier and a single sideband becomes the RF phase): single-sideband
     suppressed-carrier modulation in an IQ modulator, recombined with a phase-shifted carrier.
     RF phase = optical phase, flat over 2-20 GHz, 0-360 degrees, constant amplitude.
     A beamformer built from such phase shifters (set for 30 degrees at 12 GHz) squints:
     pointing angle asin(sin(30) * 12 GHz / f).
 (c) switched binary delay line, 3 bits of 10 ps: delay = code * 10 ps for the 8 codes.
 (d) microring all-pass as an integrated delay: RF delay 2 tau at low frequency, and the
     closed-form roll-off that bounds its delay-bandwidth product.
"""
import numpy as np
from mwp import run, o, Report, C0

rep = Report("t07_beamforming")
P, RESP, VPI = 4.0, 0.8, 4.0
S0 = np.pi * RESP * P * 1e-3 / (2 * VPI)                 # quadrature MZM slope, A/V
FREQS = ".ac lin 37 2e9 20e9"

# ---------------------------------------------------------------- (a) dispersive TTD
NEL, NU0, DNU, DISP, LEN, TD0 = 4, 193.4, 0.1, 17.0, 918.0, 1e-9
lines = ["* dispersive WDM true-time-delay beamformer", "Vrf rf gnd DC 0 AC 1", f"Vb vb gnd DC {VPI / 2}", "Vk k gnd DC 3"]
tau = []
for n in range(NEL):
    nu = NU0 + n * DNU
    lines += [f"XL{n} {o(f'a{n}')} nu{n} mwp_laser p_mw={P} nu0={nu}",
              f"XM{n} {o(f'a{n}')} {o(f'b{n}')} {o(f'x{n}')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0",
              f"XF{n} {o(f'b{n}')} {o(f'c{n}')} mwp_fiber len={LEN} disp={DISP} loss=0 nu={nu} nu_ref={NU0} td0={TD0}",
              f"XD{n} {o(f'c{n}')} an{n} k mwp_pd resp={RESP}",
              f"Vs{n} an{n} gnd DC 0"]
    tau.append(TD0 + DISP * 1e-6 * LEN * (C0 / (nu * 1e12) - C0 / (NU0 * 1e12)))
r = run("\n".join(lines + [FREQS, ".end"]), "ac", report=[f"Vs{n}" for n in range(NEL)], tag="t07a")
f = r["x"]
H = np.array([r["y"][f"I(Vs{n})"] for n in range(NEL)])
tau = np.array(tau)
ref = -S0 * np.exp(-2j * np.pi * f[None, :] * tau[:, None])
rep.check("element responses 2-20 GHz: max |H - closed form| / |H| (phase printed to 0.01 deg)",
          float(np.max(abs(H - ref)) / S0), 0.0, 3e-4, rel=False)
dtau = np.diff(tau)
rep.note("element delay steps " + ", ".join(f"{x * 1e12:.3f}" for x in dtau) + " ps (carriers equally spaced in frequency, not wavelength)")
for n in range(1, NEL):
    ph = np.unwrap(np.angle(H[n] / H[0]))
    slope = np.polyfit(f, ph, 1)[0] / (-2 * np.pi)
    rep.check(f"element {n}: delay against element 0 from the phase slope", slope, tau[n] - tau[0], 5e-15, rel=False, unit="s")
D_EL = C0 / 20e9 / 2                                     # half a wavelength at 20 GHz
ang = np.deg2rad(np.linspace(-90, 90, 3601))


def pointing(Hn, freq):
    k = np.argmin(abs(f - freq))
    af = abs(sum(Hn[n][k] * np.exp(2j * np.pi * f[k] * n * D_EL * np.sin(ang) / C0) for n in range(NEL)))
    return np.rad2deg(ang[np.argmax(af)])

th_ttd = [pointing(H, x) for x in (8e9, 12e9, 16e9, 20e9)]
th0 = np.rad2deg(np.arcsin(C0 * np.mean(dtau) / D_EL))
rep.check("true-time delay: pointing angle at 8, 12, 16, 20 GHz -- largest deviation from asin(c dtau / d)",
          float(np.max(abs(np.array(th_ttd) - th0))), 0.0, 0.1, rel=False, unit="deg")
rep.note(f"TTD beam at {', '.join(f'{x:.2f}' for x in th_ttd)} deg (8, 12, 16, 20 GHz); design {th0:.2f} deg")

# ---------------------------------------------------------------- (b) RF phase shifter


def shifter(theta):
    return f"""* photonic RF phase shifter: SSB-SC sideband + phase-shifted carrier
X1 {o('a')} nu mwp_laser p_mw={P}
X2 {o('a')} gnd gnd {o('s')} {o('c')} mwp_coupler
Vi vi gnd DC 0 AC 1 0
Vq vq gnd DC 0 AC 1 -90
Vbi bi gnd DC {VPI}
Vbq bq gnd DC {VPI}
Vbp bp gnd DC {VPI / 2}
X3 {o('s')} {o('m')} vi vq bi bq bp mwp_iqm vpi={VPI}
X4 {o('c')} {o('p')} gnd mwp_phase phi0={theta}
X5 {o('m')} {o('p')} {o('u')} {o('v')} mwp_coupler
Vk k gnd DC 3
X6 {o('u')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
{FREQS}
.end
"""

h0 = run(shifter(0.0), "ac", report=["Vs"], tag="t07b")["y"]["I(Vs)"]
worst_ph, worst_amp = 0.0, 0.0
for deg in range(45, 360, 45):
    h = run(shifter(np.deg2rad(deg)), "ac", report=["Vs"], tag="t07b")["y"]["I(Vs)"]
    d = np.angle(h / h0 * np.exp(1j * np.deg2rad(deg)), deg=True)          # RF phase = -theta
    worst_ph = max(worst_ph, float(np.max(abs(d)))); worst_amp = max(worst_amp, float(np.max(abs(abs(h) / abs(h0) - 1))))
rep.check("RF phase = -(optical phase), 45..315 deg, 2-20 GHz: largest deviation", worst_ph, 0.0, 0.03, rel=False, unit="deg")
rep.check("RF amplitude change over the 360 deg range and 2-20 GHz", worst_amp, 0.0, 1e-5, rel=False)
# amplitude closed form: sideband (A/sqrt2)(pi/(4 Vpi)) per volt against carrier A/sqrt2, 50/50 combiner
rep.check("phase-shifter response magnitude", abs(h0[0]), RESP * P * 1e-3 * np.pi / (8 * VPI), 1e-5, unit="A/V")
F_DES = 12e9
Hps = []
for n in range(NEL):                                     # element n: phase -2 pi F_DES n dtau, at every f
    th = 2 * np.pi * F_DES * n * np.mean(dtau)
    Hps.append(run(shifter(th), "ac", report=["Vs"], tag="t07b")["y"]["I(Vs)"])
th_ps = np.array([pointing(Hps, x) for x in (8e9, 12e9, 16e9, 20e9)])
sq_ref = np.rad2deg(np.arcsin(np.sin(np.deg2rad(th0)) * F_DES / np.array([8e9, 12e9, 16e9, 20e9])))
rep.check("phase-shifter beamformer: squint follows asin(sin(th0) f0/f), largest deviation", float(np.max(abs(th_ps - sq_ref))), 0.0, 0.1, rel=False, unit="deg")
rep.note(f"phase-shifter beam at {', '.join(f'{x:.2f}' for x in th_ps)} deg (8, 12, 16, 20 GHz): {th_ps[0] - th_ps[-1]:.1f} deg of squint; TTD {max(th_ttd) - min(th_ttd):.2f} deg")

# ---------------------------------------------------------------- (c) switched delay line
DLT, NB = 10e-12, 3
for code in range(2 ** NB):
    bits = [(code >> k) & 1 for k in range(NB)]
    ctl = [bits[0]] + [bits[k - 1] ^ bits[k] for k in range(1, NB)] + [bits[NB - 1]]
    L = ["* 3-bit switched delay line", f"X1 {o('a')} nu mwp_laser p_mw={P}", "Vrf rf gnd DC 0 AC 1", f"Vb vb gnd DC {VPI / 2}",
         f"X2 {o('a')} {o('s0a')} {o('xx')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0"]
    prev_a, prev_b = "s0a", None
    for k in range(NB + 1):
        L.append(f"Vc{k} ctl{k} gnd DC {ctl[k]}")
        b_in = "gnd gnd" if prev_b is None else o(prev_b)
        L.append(f"XS{k} {o(prev_a)} {b_in} {o(f'c{k}')} {o(f'd{k}')} ctl{k} mwp_switch vsw=1")
        if k < NB:
            L.append(f"XD{k} {o(f'd{k}')} {o(f'e{k}')} mwp_delay td={DLT * 2 ** k}")
            prev_a, prev_b = f"c{k}", f"e{k}"
    L += ["Vk k gnd DC 3", f"X9 {o(f'c{NB}')} an k mwp_pd resp={RESP}", "Vs an gnd DC 0", FREQS, ".end"]
    h = run("\n".join(L), "ac", report=["Vs"], tag="t07c")["y"]["I(Vs)"]
    d = np.polyfit(f, np.unwrap(np.angle(-h)), 1)[0] / (-2 * np.pi)
    ok = abs(d - code * DLT) < 5e-15 and np.max(abs(abs(h) / S0 - 1)) < 1e-5
    if not ok or code in (0, 5, 7):
        rep.check(f"code {code:03b}: delay", d, code * DLT, 5e-15, rel=False, unit="s")
        rep.check(f"code {code:03b}: RF amplitude / through value, largest deviation", float(np.max(abs(abs(h) / S0 - 1))), 0.0, 1e-5, rel=False)
rep.note("all 8 codes checked; three printed")

# ---------------------------------------------------------------- (d) ring all-pass delay
Q1 = 2e4
deck = f"""* all-pass ring as a delay, carrier on resonance
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd DC 0 AC 1
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('xx')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
X3 {o('b')} {o('t')} {o('d')} nu gnd mwp_ring nu_res={NU0} q1={Q1}
Vk k gnd DC 3
X4 {o('t')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.ac dec 20 1e7 2e10
.end
"""
r = run(deck, "ac", report=["Vs"], tag="t07d")
fr, h = r["x"], r["y"]["I(Vs)"]
tr = 2 * Q1 / (2 * np.pi * NU0 * 1e12)
rep.check("ring all-pass: RF response = -S0 exp(-2j atan(2 pi f tau)), max deviation / S0",
          float(np.max(abs(h + S0 * np.exp(-2j * np.arctan(2 * np.pi * fr * tr))))) / S0, 0.0, 3e-4, rel=False)
dly = -np.angle(-h) / (2 * np.pi * fr)
rep.check("ring all-pass: low-frequency RF delay = 2 tau = 4 Q / omega", dly[np.argmin(abs(fr - 1e8))], 2 * tr, 2e-3, unit="s")
hi = fr >= 2e8                                           # below, the 0.01 deg phase print dominates the delay
f1 = fr[hi][np.argmax(dly[hi] < 0.99 * 2 * tr)]
rep.note(f"delay {2 * tr * 1e12:.1f} ps; within 1 % up to {f1 / 1e9:.2f} GHz (closed form 0.173/(2 pi tau) = {0.1735 / (2 * np.pi * tr) / 1e9:.2f} GHz): "
         f"delay x bandwidth = {2 * tr * f1:.3f}, fixed by the single resonance")
rep.done()
