#!/usr/bin/env python3
"""T10 -- optical amplifier: gain saturation and signal-spontaneous beat noise.

 (a) operating point against G = G0 / (1 + G0 P_in / P_sat), input from -30 to +10 dBm;
 (b) noise of an amplified link (laser -> amplifier -> photodiode -> load) against
         S_i = 4 R^2 G P_in S_sp + 2 q R G P_in,    S_sp = (F/2) h nu (G - 1),
     the signal-spontaneous beat term (Olsson, JLT 7, 1989) plus shot, and the noise figure
     recovered from it, (2 n_sp (G-1) + 1) / G with 2 n_sp = F (referred to an ideal detector).
"""
import numpy as np
from mwp import run, o, Report, Q, H, KB

rep = Report("t10_optical_amplifier")
RESP, G0_DB, PSAT, NF_DB, NU, RL = 0.8, 20.0, 10.0, 5.0, 193.4, 50.0
G0, F = 10 ** (G0_DB / 10), 10 ** (NF_DB / 10)


def deck(pin, card):
    return f"""* amplified link
X1 {o('a')} nu mwp_laser p_mw={pin}
X2 {o('a')} {o('b')} mwp_oamp g0_db={G0_DB} psat={PSAT} nf_db={NF_DB} nu0={NU}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP}
RL an gnd {RL}
Vd dummy gnd DC 0 AC 1
Rd dummy gnd 1
{card}
.end
"""

for dbm in (-30, -20, -10, 0, 10):
    pin = 10 ** (dbm / 10)
    r = run(deck(pin, ""), "op", tag="t10a")
    pout = r["y"]["V(an)"][0] / RL / (RESP * 1e-3)
    g_ref = G0 / (1 + G0 * pin / PSAT)
    rep.check(f"P_in {dbm:+d} dBm: gain", 10 * np.log10(pout / pin), 10 * np.log10(g_ref), 2e-3, rel=False, unit="dB")

for dbm in (-30, -10):
    pin = 10 ** (dbm / 10)
    g = G0 / (1 + G0 * pin / PSAT)
    r = run(deck(pin, ".noise V(an) Vd dec 2 1e8 1e10"), "noise", tag="t10b")
    ssp = 0.5 * F * H * NU * 1e12 * (g - 1)                               # W/Hz
    s_sig_sp = 4 * RESP ** 2 * (g * pin * 1e-3) * ssp
    s_shot = 2 * Q * RESP * g * pin * 1e-3
    s_th = 4 * KB * 300.15 / RL
    rep.check(f"P_in {dbm:+d} dBm: output noise (signal-spontaneous + shot + load thermal)", r["y"]["S"][0],
              (s_sig_sp + s_shot + s_th) * RL ** 2, 1e-4, unit="V^2/Hz")
    eta = RESP * H * NU * 1e12 / Q                                        # detector quantum efficiency
    nf_meas = 10 * np.log10((r["y"]["S"][0] / RL ** 2 - s_shot - s_th) / (2 * Q * RESP * pin * 1e-3 * g * g * eta) + 1 / g)
    rep.check(f"P_in {dbm:+d} dBm: noise figure recovered from the simulated noise, 2 n_sp (G-1)/G + 1/G", nf_meas,
              10 * np.log10(F * (g - 1) / g + 1 / g), 1e-3, rel=False, unit="dB")
    rep.note(f"  signal-spontaneous {s_sig_sp:.3e}, shot {s_shot:.3e}, thermal {s_th:.3e} A^2/Hz at G = {10 * np.log10(g):.1f} dB")
rep.done()
