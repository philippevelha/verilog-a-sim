#!/usr/bin/env python3
"""T13 -- the electrical loads a driver and a receiver see (2.56.0+2).

What an electronic designer co-designs against:
 (a) the modulator electrode: mwp_mzm with `cel` presents Y = 1/rterm + j w cel to its driver
     (and Y = j w cel unterminated), while the optical modulation is unchanged;
 (b) the photodiode junction: mwp_pd with `cj` into a load resistor R gives the photocurrent's
     transfer to V(R) one extra pole at 1/(2 pi R cj).
"""
import numpy as np
from mwp import run, o, Report

P, RESP, VPI = 10.0, 0.8, 4.0
rep = Report("t13_electrical_loads")

# ---------------------------------------------------------------- (a) modulator electrode
CEL = 300e-15
for rterm in (50.0, 0.0):
    deck = f"""* MZM electrode admittance
X1 {o('a')} nu mwp_laser p_mw={P}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm={rterm} cel={CEL}
Vb vb gnd DC 2
Vrf rf gnd DC 0 AC 1
X3 {o('b')} an k mwp_pd resp={RESP}
Vk k gnd DC 3
Vs an gnd DC 0
.ac dec 10 1e8 1e11
.end
"""
    r = run(deck, "ac", report=["Vrf", "Vs"], tag=f"t13a_{int(rterm)}")
    f = r["x"]
    y = -r["y"]["I(Vrf)"]                                # current drawn from the source
    yref = (1.0 / rterm if rterm > 0 else 0.0) + 1j * 2 * np.pi * f * CEL
    rep.check(f"electrode admittance, rterm={rterm:g} ohm, cel={CEL * 1e15:.0f} fF, 0.1-100 GHz: "
              "max |Y - (1/rterm + jwC)| / max |Y|",
              float(np.max(np.abs(y - yref)) / np.max(np.abs(yref))), 0.0,
              # va-cli prints AC phase to 0.01 deg (1.7e-4 rad): the terminated case, whose phase
              # is not a round 0 or 90 deg, is resolved to that and no further
              1e-4 if rterm > 0 else 1e-5, rel=False)
    s0 = np.pi * RESP * P * 1e-3 / (2 * VPI)
    h = r["y"]["I(Vs)"]
    rep.check(f"modulation unchanged by cel (rterm={rterm:g}): max |H + s0| / s0",
              float(np.max(np.abs(h + s0)) / s0), 0.0, 1e-5, rel=False)

# ---------------------------------------------------------------- (b) photodiode junction
CJ, RL = 200e-15, 500.0
deck = f"""* PD junction into a load
X1 {o('a')} nu mwp_laser p_mw={P}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI}
Vb vb gnd DC 2
Vrf rf gnd DC 0 AC 1
X3 {o('b')} an k mwp_pd resp={RESP} cj={CJ}
Vk k gnd DC 3
Rl an gnd {RL}
.ac dec 10 1e8 1e11
.end
"""
r = run(deck, "ac", report=["an"], tag="t13b")
f = r["x"]
v = r["y"]["V(an)"]
pole = 1.0 / (1.0 + 1j * 2 * np.pi * f * RL * CJ)
ratio = v / v[0] * pole[0]                       # V/V0 = pole/pole0
rep.check(f"junction pole, cj={CJ * 1e15:.0f} fF into {RL:g} ohm (f_3dB {1 / (2 * np.pi * RL * CJ) / 1e9:.2f} GHz): "
          "max |V/V0 - 1/(1 + jwRC)|",
          float(np.max(np.abs(ratio - pole))), 0.0,
          # two printed phases (V and V0), each to 0.01 deg: up to 1.7e-4 rad together
          2e-4, rel=False)
s0 = np.pi * RESP * P * 1e-3 / (2 * VPI)
rep.check("low-frequency transimpedance |V/Vrf| = s0 R", abs(v[0]), s0 * RL * abs(pole[0]), 1e-5, unit="V/V")

rep.done()
