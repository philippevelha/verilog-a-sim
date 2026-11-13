#!/usr/bin/env python3
"""T9 -- chromatic dispersion inside the envelope band, by rational (all-pass) approximation.

25 km of standard fibre (17 ps/nm/km) after a chirp-free intensity modulator: the two
sidebands walk off in phase and the detected RF power fades as

    |H_RF(f)| = | cos( pi lambda^2 D L f^2 / c ) |,        first null at 12.1 GHz here.

The fibre is replaced by a chain of all-pass mwp_ring sections fitted to the linear group
delay over a 32 GHz band (fit_dispersion.py). Checked:
 - the simulator against the chain's own exact response (the models are right);
 - the chain against the ideal fibre inside the band (the approximation is good), for 4, 8 and
   12 sections;
 - where it stops: the response beyond the fitted band, which is the bandwidth limit of the
   method.
"""
import numpy as np
from mwp import run, o, Report, C0
from fit_dispersion import design, chain, response

rep = Report("t09_dispersion")
P, RESP, VPI, NU0 = 4.0, 0.8, 4.0, 193.4
DISP, LEN, BAND = 17.0, 25e3, 32e9
S0 = np.pi * RESP * P * 1e-3 / (2 * VPI)
lam = C0 / (NU0 * 1e12)
fade = lambda f: np.cos(np.pi * lam ** 2 * DISP * 1e-6 * LEN * f ** 2 / C0)
rep.note(f"first fading null at {np.sqrt(C0 / (2 * lam ** 2 * DISP * 1e-6 * LEN)) / 1e9:.2f} GHz")

for n in (4, 8, 12):
    poles, info = design(DISP, LEN, BAND, n)
    deck = "\n".join([f"* dispersion as {n} all-pass sections",
                      f"X1 {o('a')} nu mwp_laser p_mw={P} nu0={NU0}",
                      "Vrf rf gnd DC 0 AC 1", f"Vb vb gnd DC {VPI / 2}",
                      f"X2 {o('a')} {o('b')} {o('xx')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0"]
                     + chain(poles, "XR", "b", "c", "nu", NU0)
                     + ["Vk k gnd DC 3", f"X3 {o('c')} an k mwp_pd resp={RESP}", "Vs an gnd DC 0",
                        ".ac lin 251 0.1e9 25.1e9", ".end"])
    r = run(deck, "ac", report=["Vs"], tag="t09")
    f, h = r["x"], r["y"]["I(Vs)"]
    h0 = response(poles, np.array([0.0]))[0]
    exact = -S0 * 0.5 * (np.conj(h0) * response(poles, f) + h0 * np.conj(response(poles, -f)))
    inband = f <= BAND / 2
    rep.check(f"{n} sections: simulator against the chain's exact response, 0.1-25 GHz, max |dH| / S0",
              float(np.max(abs(h - exact)) / S0), 0.0, 3e-4, rel=False)
    dev = float(np.max(abs(abs(h[inband]) / S0 - abs(fade(f[inband])))))
    rep.note(f"{n} sections: group-delay fit error {info['peak'] * 1e12:.3f} ps peak over a {info['span'] * 1e12:.0f} ps span; "
             f"fading curve within {dev:.2e} of the ideal fibre up to {BAND / 2e9:.0f} GHz; bulk delay {info['tau0'] * 1e12:.0f} ps")
    if n >= 8:
        rep.check(f"{n} sections: |H_RF| against the fibre's cos(pi lambda^2 D L f^2/c), 0.1-16 GHz, max deviation", dev, 0.0, 2e-3, rel=False)
        k = np.argmin(abs(h[inband]))
        rep.check(f"{n} sections: position of the first null", f[inband][k], np.sqrt(C0 / (2 * lam ** 2 * DISP * 1e-6 * LEN)), 0.06e9, rel=False, unit="Hz")
    out = f > BAND / 2
    f_bad = f[out][np.argmax(abs(abs(h[out]) / S0 - abs(fade(f[out]))) > 0.1)] if np.any(abs(abs(h[out]) / S0 - abs(fade(f[out]))) > 0.1) else np.inf
    rep.note(f"{n} sections: beyond the fitted band the response departs from the fibre by more than 0.1 at {f_bad / 1e9:.1f} GHz")
rep.note(f"cost: {n} sections = {2 * n} extra unknowns; AC {r['points']} points in {r['wall_s']:.2f} s")
rep.done()
