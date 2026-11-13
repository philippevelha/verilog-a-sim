#!/usr/bin/env python3
"""T3 -- optical generation of RF carriers over a wide frequency range.

 (a) optical heterodyning: two lasers `df` apart on one photodiode, 1 to 100 GHz. Beat
     amplitude R*sqrt(P1*P2) and beat frequency.
 (b) frequency multiplication: an MZM at its null (and at its peak) driven at f generates
     2f, 4f, ... with photocurrent amplitudes R*P*J_2k(m), and no odd harmonics.
 (c) linewidth: the laser's phase noise read by a delayed self-homodyne discriminator
     (unbalanced interferometer at quadrature + balanced detector),
       S_i(f) = (R P)^2 * 4 sin^2(pi f td) * lw/(pi f^2)  +  2 q R P,
     with the laser RIN rejected by the balanced detector.
"""
import numpy as np
from scipy.special import jv
from mwp import run, o, tones, Report, Q

rep = Report("t03_rf_generation")
RESP = 0.8

# ---------------------------------------------------------------- (a) heterodyne
P1, P2 = 1.0, 2.0
for df in (1e9, 10e9, 40e9, 100e9):
    n_per = 20
    deck = f"""* two lasers -> coupler -> PD
X1 {o('a')} nu1 mwp_laser p_mw={P1}
X2 {o('b')} nu2 mwp_laser p_mw={P2} df={df}
X3 {o('a')} {o('b')} {o('c')} {o('d')} mwp_coupler
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / df / 64} {n_per / df}
.end
"""
    r = run(deck, "tran", report=["Vs"], tag="t03a")
    t, i = r["x"], r["y"]["I(Vs)"]
    a, dc, res = tones(t, i, [df])
    rep.check(f"beat at {df / 1e9:.0f} GHz: amplitude R sqrt(P1 P2)", abs(a[0]), RESP * 1e-3 * np.sqrt(P1 * P2), 1e-6, unit="A")
    rep.check(f"beat at {df / 1e9:.0f} GHz: mean current R (P1+P2)/2", dc, RESP * 1e-3 * (P1 + P2) / 2, 1e-6, unit="A")
    # frequency from the phase advance between the two halves of the record
    h = t[-1] / 2
    a1, *_ = tones(t[t <= h], i[t <= h], [df]); a2, *_ = tones(t[t >= h] - h, i[t >= h], [df])
    dphi = np.angle(a2[0] / a1[0]) - 2 * np.pi * ((df * h) % 1.0)
    dphi = (dphi + np.pi) % (2 * np.pi) - np.pi
    rep.check(f"beat at {df / 1e9:.0f} GHz: frequency error / df", dphi / (2 * np.pi * h) / df, 0.0, 1e-6, rel=False)

# ---------------------------------------------------------------- (b) multiplication
P, VPI, F = 10.0, 4.0, 10e9
for name, vb, va in (("null", 4.0, 3.0), ("peak", 0.0, 3.0), ("null, J2 = 0", 4.0, 5.1356223 * VPI / np.pi)):
    deck = f"""* MZM frequency multiplier
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 {va} {F})
Vb vb gnd DC {vb}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / F / 400} {4 / F}
.end
"""
    r = run(deck, "tran", report=["Vs"], tag="t03b")
    a, dc, res = tones(r["x"], r["y"]["I(Vs)"], [k * F for k in range(1, 9)])
    m = np.pi * va / VPI
    rp = RESP * P * 1e-3
    for k in (2, 4):
        ref = rp * abs(jv(k, m))
        if ref > 1e-9:
            rep.check(f"{name} bias, m = {m:.3f}: {k}f = {k * F / 1e9:.0f} GHz amplitude R P J{k}(m)", abs(a[k - 1]), ref, 1e-5, unit="A")
        else:
            rep.check(f"{name} bias, m = {m:.3f}: {k}f suppressed, / (R P)", abs(a[k - 1]) / rp, 0.0, 1e-6, rel=False)
    rep.check(f"{name} bias: odd harmonics f, 3f / (R P)", max(abs(a[0]), abs(a[2])) / rp, 0.0, 1e-6, rel=False)

# ---------------------------------------------------------------- (c) linewidth discriminator
PL, LW, TD, RIN, RL = 4.0, 100e3, 1e-9, 1e-14, 50.0
deck = f"""* delayed self-homodyne discriminator
X1 {o('a')} nu mwp_laser p_mw={PL} lw={LW} rin={RIN}
X2 {o('a')} gnd gnd {o('c')} {o('d')} mwp_coupler
X3 {o('d')} {o('e')} mwp_delay td={TD} phi={np.pi / 2}
X4 {o('c')} {o('e')} {o('p')} {o('n')} mwp_coupler
X5 {o('p')} {o('n')} out gnd mwp_bpd resp={RESP}
RL out gnd {RL}
Vd dummy gnd DC 0 AC 1
Rd dummy gnd 1
.noise V(out) Vd dec 20 1e5 3e9
.end
"""
r = run(deck, "noise", tag="t03c")
f, s = r["x"], r["y"]["S"]
rp = RESP * PL * 1e-3
s_ph = rp ** 2 * 4 * np.sin(np.pi * f * TD) ** 2 * LW / (np.pi * f ** 2) * RL ** 2
s_fl = (2 * Q * rp + 4 * 1.380649e-23 * 300.15 / RL) * RL ** 2
rep.check("discriminator output PSD 0.1 MHz-3 GHz: max relative deviation from closed form",
          float(np.max(np.abs(s / (s_ph + s_fl) - 1))), 0.0, 2e-3, rel=False)
rep.check("low-frequency plateau (R P)^2 4 pi td^2 lw", s[0] - s_fl, rp ** 2 * 4 * np.pi * TD ** 2 * LW * RL ** 2, 1e-3, unit="V^2/Hz")
rep.note(f"RIN of {10 * np.log10(RIN):.0f} dB/Hz would add {RIN * (rp / 2) ** 2 * RL ** 2:.2e} V^2/Hz per diode; "
         f"it cancels in the balanced detector (floor is shot + load thermal, {s_fl:.2e})")
k = np.argmin(np.abs(f - 1e9))
rep.note(f"at the first null f = 1/td = 1 GHz the output is {s[k]:.3e} V^2/Hz (floor {s_fl:.3e})")
rep.done()
