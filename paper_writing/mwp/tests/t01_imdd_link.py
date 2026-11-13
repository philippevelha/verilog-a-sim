#!/usr/bin/env python3
"""T1 -- intensity-modulated direct-detection link: laser -> MZM -> photodiode.

Checks against closed forms (Cox, "Analog Optical Links", 2004; Urick et al., 2015):
 (a) DC transfer curve, ideal and with a finite extinction ratio;
 (b) small-signal response with the modulator and detector poles (AC);
 (c) two-tone transient at quadrature: fundamental and third-order intermodulation against
     the Bessel expansion  i_1 = R P J1(m) J0(m),  i_IMD3 = R P J1(m) J2(m),  and the input
     intercept  V_IP3 = sqrt(8) Vpi / pi  (peak volts per tone, small-signal limit);
 (d) output noise: shot + RIN + thermal, then noise figure and SFDR from (b)-(d).
"""
import numpy as np
from scipy.special import jv
from mwp import run, o, tones, Report, Q, KB

P, RESP, VPI, RIN, RL = 10.0, 0.8, 4.0, 10 ** (-155 / 10), 50.0     # mW, A/W, V, 1/Hz, ohm
rep = Report("t01_imdd_link")


def link(extra, mzm="", pd="", laser=""):
    return f"""* IMDD link
X1 {o('a')} nu mwp_laser p_mw={P} {laser}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} {mzm}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP} {pd}
{extra}
.end
"""

# ---------------------------------------------------------------- (a) DC transfer
for er in (None, 20.0):
    r = run(link("Vrf rf gnd DC 0\nVb vb gnd DC 0\nVs an gnd DC 0\n.dc Vb 0 8 0.25",
                 mzm="" if er is None else f"er_db={er}"), "dc", tag="t01a")
    vb = r["x"]; i = r["y"]["I(Vs)"]
    if er is None:
        ref = RESP * P * 1e-3 * np.cos(np.pi * vb / VPI / 2) ** 2
    else:
        g = (10 ** (er / 20) - 1) / (10 ** (er / 20) + 1)
        ref = RESP * P * 1e-3 * (1 + g * g + 2 * g * np.cos(np.pi * vb / VPI)) / (2 * (1 + g * g))
    rep.check(f"DC transfer, ER {'ideal' if er is None else str(er) + ' dB'}: max |i - closed form| / i_max",
              float(np.max(np.abs(i - ref)) / ref.max()), 0.0, 1e-6, rel=False)
    if er is not None:
        rep.check("extinction ratio from the swept curve", 10 * np.log10(i.max() / i.min()), er, 1e-3, rel=False, unit="dB")

# ---------------------------------------------------------------- (b) AC response
FM, FD = 20e9, 30e9
r = run(link("Vrf rf gnd DC 0 AC 1\nVb vb gnd DC 2\nVs an gnd DC 0\n.ac dec 10 1e8 1e11",
             mzm=f"f3db={FM}", pd=f"f3db={FD}"), "ac", report=["Vs"], tag="t01b")
f = r["x"]; h = r["y"]["I(Vs)"]
s0 = np.pi * RESP * P * 1e-3 / (2 * VPI)                             # A/V at quadrature
href = -s0 / ((1 + 1j * f / FM) * (1 + 1j * f / FD))
rep.check("AC slope efficiency at 100 MHz", abs(h[0]), abs(href[0]), 1e-5, unit="A/V")
rep.check("AC response 0.1-100 GHz, two poles: max |H - closed form| / |H(0)|",
          float(np.max(np.abs(h - href)) / s0), 0.0, 2e-4, rel=False)
g_link = (s0 ** 2) * RL * 50.0          # power gain: 50 ohm source into the 50 ohm termination, RL load
rep.note(f"intrinsic link gain (50 ohm in, {RL:.0f} ohm load): {10 * np.log10(g_link):.2f} dB")

# ---------------------------------------------------------------- (c) two-tone transient
F1, F2 = 1.0e9, 1.1e9
T = 10e-9                                                            # 10 and 11 periods
out = {}
for va in (0.2, 0.4, 0.8):
    r = run(link(f"V1 rf m SIN(0 {va} {F1})\nV2 m gnd SIN(0 {va} {F2})\nVb vb gnd DC 2\nVs an gnd DC 0\n"
                 f".tran 2e-12 {2 * T}"), "tran", report=["Vs"], tag="t01c")
    a, dc, res = tones(r["x"], r["y"]["I(Vs)"], [F1, F2, 2 * F1 - F2, 2 * F2 - F1, F1 + F2, 3 * F1], t_start=T)
    m = np.pi * va / VPI
    i1 = RESP * P * 1e-3 * jv(1, m) * jv(0, m)
    i3 = RESP * P * 1e-3 * jv(1, m) * jv(2, m)
    rep.check(f"two-tone {va} V: fundamental", abs(a[0]), i1, 1e-5, unit="A")
    rep.check(f"two-tone {va} V: IMD3 (2f1-f2)", abs(a[2]), i3, 1e-4, unit="A")
    rep.check(f"two-tone {va} V: second-order (f1+f2) at quadrature / fundamental", abs(a[4]) / abs(a[0]), 0.0, 1e-6, rel=False)
    out[va] = (abs(a[0]), abs(a[2]), r["points"], r["wall_s"])
va = 0.2
vip3 = va * np.sqrt(out[va][0] / out[va][1])                         # slope-1 / slope-3 extrapolation
rep.check("input IP3 (peak volts per tone), from the 0.2 V run", vip3, np.sqrt(8) * VPI / np.pi, 5e-3, unit="V")
iip3_dbm = 10 * np.log10(vip3 ** 2 / (2 * 50.0) / 1e-3)
rep.note(f"IIP3 = {iip3_dbm:.2f} dBm into 50 ohm; transient {out[va][2]} points in {out[va][3]:.1f} s")

# ---------------------------------------------------------------- (d) noise, NF, SFDR
r = run(link(f"Vrf rf gnd DC 0 AC 1\nVb vb gnd DC 2\nRL an gnd {RL}\n.noise V(an) Vrf dec 2 1e8 1e10",
             laser=f"rin={RIN}", mzm="rterm=0"), "noise", tag="t01d")
idc = RESP * P * 1e-3 / 2
T0 = 300.15                                                         # simulator default, 27 C
s_shot, s_rin, s_th = 2 * Q * idc * RL ** 2, RIN * idc ** 2 * RL ** 2, 4 * KB * T0 * RL
rep.check("output noise PSD (shot + RIN + load thermal)", r["y"]["S"][0], s_shot + s_rin + s_th, 2e-3, unit="V^2/Hz")
rep.note(f"  shot {s_shot:.3e}, RIN {s_rin:.3e}, thermal {s_th:.3e} V^2/Hz")
# noise figure: N_out / (G k T0); the matched source's own noise adds G*kT to N_out
n_out = (s_shot + s_rin + s_th) / RL                                 # W/Hz in RL
nf = 10 * np.log10(1 + n_out / (g_link * KB * 290.0))
n_tot_dbm = 10 * np.log10((n_out + g_link * KB * 290.0) / 1e-3)
oip3_dbm = iip3_dbm + 10 * np.log10(g_link)
sfdr = 2.0 / 3.0 * (oip3_dbm - n_tot_dbm)
rep.note(f"noise figure {nf:.2f} dB, OIP3 {oip3_dbm:.2f} dBm, SFDR {sfdr:.2f} dB Hz^(2/3) "
         f"(derived from the simulated gain, intercept and noise)")
sfdr_ref = 2.0 / 3.0 * (10 * np.log10((np.sqrt(8) * VPI / np.pi) ** 2 / 100.0 / 1e-3) + 10 * np.log10(g_link)
                        - 10 * np.log10(((s_shot + s_rin + s_th) / RL + g_link * KB * 290.0) / 1e-3))
rep.check("SFDR against the closed-form chain", sfdr, sfdr_ref, 0.05, rel=False, unit="dB Hz^(2/3)")
rep.done(dict(nf_db=nf, sfdr=sfdr, iip3_dbm=iip3_dbm, gain_db=10 * np.log10(g_link)))
