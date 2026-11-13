#!/usr/bin/env python3
"""T8 -- microwave photonic mixing (frequency down-conversion).

Two structures from Minasian, Chan and Yi, Opt. Express 21, 22918 (2013), Sec. 5:

 (a) the conventional mixer, two intensity modulators in series (Gopalakrishnan et al., 1993),
     both at quadrature:
         i_IF = (R P / 2) J1(m_RF) J1(m_LO),        mean current R P / 4;
 (b) the dual-parallel MZM mixer (their Fig. 15): LO on one child, RF on the other, both
     children at their null so the optical carrier is suppressed, children combined in phase:
         i_IF = R P J1(u_RF) J1(u_LO),   u = pi V / (2 Vpi),
         mean current (R P / 4) [ (1 - J0(2 u_RF)) / 2 + (1 - J0(2 u_LO)) / 2 ].

Checked: IF amplitude and mean photocurrent of both against these forms; RF-to-IF conversion
flat from 2 to 16 GHz at an 80 MHz IF (as their Fig. 17, with a 16.7 GHz modulator pole
included and its closed-form roll-off); carrier and LO/RF feed-through at the detector;
and the conversion-efficiency advantage of carrier suppression at equal detector current.
"""
import numpy as np
from scipy.special import jv
from mwp import run, o, tones, Report

rep = Report("t08_mixer")
P, RESP, VPI, FIF = 10.0, 0.8, 4.0, 80e6
RP = RESP * P * 1e-3
VRF, VLO = 0.2, 1.5


def series(frf, flo, f3=0.0, ppp=40):
    return f"""* dual-series MZM mixer
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 {VRF} {frf})
Vlo lo gnd SIN(0 {VLO} {flo})
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('x1')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0 f3db={f3}
X3 {o('b')} {o('c')} {o('x2')} lo gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0 f3db={f3}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / frf / ppp} {2 / FIF}
.end
"""


def parallel(frf, flo, f3=0.0, ppp=40):
    return f"""* dual-parallel MZM mixer, carrier suppressed
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 {VRF} {frf})
Vlo lo gnd SIN(0 {VLO} {flo})
Vn vn gnd DC {VPI}
Vp vp gnd DC 0
X2 {o('a')} {o('c')} lo rf vn vn vp mwp_iqm vpi={VPI} f3db={f3}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / frf / ppp} {2 / FIF}
.end
"""

FRF, FLO = 10.0e9, 10.0e9 - FIF
res = {}
for name, deck in (("series", series(FRF, FLO)), ("parallel", parallel(FRF, FLO))):
    r = run(deck, "tran", report=["Vs"], tag="t08")
    a, dc, _ = tones(r["x"], r["y"]["I(Vs)"], [FIF, FRF, FLO, FRF + FLO, 2 * FLO, 2 * FRF], t_start=1 / FIF)
    res[name] = (abs(a[0]), dc, abs(a[1]), abs(a[2]), r)
m_rf, m_lo = np.pi * VRF / VPI, np.pi * VLO / VPI
u_rf, u_lo = m_rf / 2, m_lo / 2
ref = {"series": (RP / 2 * jv(1, m_rf) * jv(1, m_lo), RP / 4),
       "parallel": (RP * jv(1, u_rf) * jv(1, u_lo), RP / 4 * (1 - 0.5 * jv(0, 2 * u_rf) - 0.5 * jv(0, 2 * u_lo)))}
# tolerance: va-cli prints time with 7 significant digits, 1e-15 s at 25 ns = 6e-5 rad at 10 GHz
for name in ("series", "parallel"):
    rep.check(f"{name}: IF amplitude at 80 MHz", res[name][0], ref[name][0], 3e-4, unit="A")
    rep.check(f"{name}: mean photocurrent", res[name][1], ref[name][1], 1e-4, unit="A")
rep.check("series: RF feed-through at the detector, R P J1(m_RF)/2", res["series"][2], RP / 2 * jv(1, m_rf), 1e-3, unit="A")
rep.check("parallel: RF and LO feed-through / IF (carrier suppressed: none)", max(res["parallel"][2], res["parallel"][3]) / res["parallel"][0], 0.0, 1e-4, rel=False)
# conversion efficiency at equal detector current: IF power scales as (i_IF / i_dc)^2
adv = 20 * np.log10((res["parallel"][0] / res["parallel"][1]) / (res["series"][0] / res["series"][1]))
adv_ref = 20 * np.log10((ref["parallel"][0] / ref["parallel"][1]) / (ref["series"][0] / ref["series"][1]))
rep.check("conversion-efficiency advantage of the carrier-suppressed mixer at equal detector current", adv, adv_ref, 0.01, rel=False, unit="dB")
rep.note(f"(LO drive {VLO} V = {m_lo / np.pi:.3f} Vpi. Minasian et al. measured 23.7 dB with a 45.5 dB LO suppression and an EDFA; "
         f"their drive levels are not given, so the number here is this model's, not a reproduction.)")
rep.note(f"each run: {res['series'][4]['points']} points, {res['series'][4]['wall_s']:.1f} s")

# conversion against RF frequency, 80 MHz IF, modulator pole at 16.7 GHz. 160 steps per RF period:
# the trapezoidal rule warps a pole's corner by (w h)^2/12, 2e-3 in amplitude at 40 steps
F3 = 16.7e9
conv = []
for frf in (2e9, 6e9, 10e9, 16e9):
    r = run(parallel(frf, frf - FIF, F3, ppp=160), "tran", report=["Vs"], tag="t08f")
    a, dc, _ = tones(r["x"], r["y"]["I(Vs)"], [FIF], t_start=1 / FIF)
    pole = lambda f: 1 / np.sqrt(1 + (f / F3) ** 2)
    refc = RP * jv(1, u_rf * pole(frf)) * jv(1, u_lo * pole(frf - FIF))
    rep.check(f"parallel mixer, RF {frf / 1e9:.0f} GHz -> 80 MHz, modulator pole 16.7 GHz: IF amplitude", abs(a[0]), refc, 5e-4, unit="A")
    conv.append(20 * np.log10(abs(a[0])))
rep.note(f"conversion roll-off 2 -> 16 GHz: {conv[0] - conv[-1]:.2f} dB (two modulator poles, RF and LO)")
rep.done()
