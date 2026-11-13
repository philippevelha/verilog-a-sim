#!/usr/bin/env python3
"""T4 -- opto-electronic oscillator (Yao and Maleki, JOSA B 13, 1996).

    laser -> MZM (quadrature) -> delay td -> photodiode -> RL -> gm -> RLC tank -> MZM drive

The loop oscillates where the round-trip phase is a multiple of 2 pi and the saturated gain is
one. With v = V cos(wt) on the modulator, the photocurrent fundamental is -R P J1(pi V/Vpi), so

    gm * RL * Z(f) * exp(-j 2 pi f td) * R P J1(pi V / Vpi) / V = 1,
    Z(f) = Rt / (1 + j Q (f/f0 - f0/f)).

Solved numerically for (f, V) and compared with the transient started by a small kick.
Checked: oscillation frequency, amplitude, single-mode operation (neighbouring loop modes,
1/td apart, stay below threshold), and that the frequency follows the delay (mode k/td pulled
by the filter phase), for two delays.
The delay line's history needs VA_ABSDELAY_POINTS=4096 here; the default (512) is run once
and its error reported.
Phase noise: NOT simulated. The simulator has no transient noise and no periodic noise
analysis, so the last block is a semi-analytic ESTIMATE, Yao and Maleki's

    L(f') = delta / (2 pi f' tau)^2,    delta = (noise density) / (carrier power) at the detector load,

fed with simulated numbers: the open-loop noise density from a .noise run (shot, laser RIN at
-155 dB/Hz, load thermal; the amplifier is noiseless) and the oscillation amplitude from the
transient. tau is the loop delay plus the filter's group delay. It assumes white noise added
once per round trip and a noiseless limiter; it has no flicker up-conversion and no laser
frequency noise converted by the delay, so it is a floor, and it is reported, not checked.
"""
import numpy as np
from scipy.optimize import brentq
from scipy.special import jv
from mwp import run, o, freq_est, tones, Report, Q, KB

rep = Report("t04_oeo")
P, RESP, VPI, RL, RT, F0, QT, GM = 10.0, 0.8, 4.0, 50.0, 50.0, 10e9, 50.0, 0.25
L = RT / (2 * np.pi * F0 * QT)
C = QT / (2 * np.pi * F0 * RT)
z = lambda f: RT / (1 + 1j * QT * (f / F0 - F0 / f))


def predict(td):
    """(f, V) of the loop mode with the highest gain."""
    best = None
    for k in range(int(F0 * td) - 2, int(F0 * td) + 4):
        ph = lambda f: np.angle(z(f)) - 2 * np.pi * (f * td - k)       # zero at the mode
        try:
            f = brentq(ph, (k - 0.45) / td, (k + 0.45) / td)
        except ValueError:
            continue
        if best is None or abs(z(f)) > abs(z(best)):
            best = f
    f = best
    gs = GM * RL * abs(z(f)) * RESP * P * 1e-3                         # V = gs * J1(pi V/Vpi)
    v = brentq(lambda v: gs * jv(1, np.pi * v / VPI) - v, 0.2, 3.8 * VPI / np.pi)
    return f, v, gs * np.pi / VPI / 2

TSTOP, TFIT = 200e-9, 150e-9
osc = {}
for td, npts in ((5.02e-9, 4096), (4.98e-9, 4096), (5.02e-9, 512)):
    f_ref, v_ref, g_small = predict(td)
    deck = f"""* opto-electronic oscillator, loop delay {td:g} s
X1 {o('a')} nu mwp_laser p_mw={P}
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
X3 {o('b')} {o('c')} mwp_delay td={td}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
RL an gnd {RL}
G1 rf gnd an gnd {GM}
Rt rf gnd {RT}
Lt rf gnd {L:.9e}
Ct rf gnd {C:.9e}
Ik gnd rf PULSE(0 1e-3 0 10p 10p 50p 1)
.tran 1e-12 {TSTOP}
.end
"""
    r = run(deck, "tran", report=["rf"], tag="t04", env={"VA_ABSDELAY_POINTS": str(npts)})
    t, v = r["x"], r["y"]["V(rf)"]
    f_sim, v_sim = freq_est(t, v, f_ref, TFIT)
    if npts == 512:
        # the simulator's default delay history: 5 ns / 509 = 9.9 ps between stored samples
        rep.note(f"default history (512 samples), td = {td * 1e9:.2f} ns: frequency error {f_sim / f_ref - 1:+.1e}, "
                 f"amplitude error {v_sim / v_ref - 1:+.2%} -- the 10 GHz tone is under-resolved in the delay line; "
                 f"VA_ABSDELAY_POINTS=4096 is used above")
        continue
    rep.note(f"td = {td * 1e9:.2f} ns: small-signal loop gain {g_small:.3f}; {r['points']} points, {r['wall_s']:.0f} s")
    rep.check(f"td {td * 1e9:.2f} ns: oscillation frequency", f_sim, f_ref, 1e-4, unit="Hz")
    rep.check(f"td {td * 1e9:.2f} ns: amplitude at the modulator", v_sim, v_ref, 5e-3, unit="V")
    osc[td] = (f_sim, v_sim)
    a, dc, res = tones(t, v, [f_sim, f_sim - 1 / td, f_sim + 1 / td], t_start=TFIT)
    rep.check(f"td {td * 1e9:.2f} ns: neighbouring loop modes / carrier", max(abs(a[1]), abs(a[2])) / abs(a[0]), 0.0, 2e-3, rel=False)
    env = np.abs(v[(t > 120e-9) & (t < 150e-9)]).max() / np.abs(v[t > 170e-9]).max()
    rep.check(f"td {td * 1e9:.2f} ns: envelope settled (peak in 120-150 ns / after 170 ns)", env, 1.0, 2e-3)

# ---------------------------------------------------------------- phase noise: semi-analytic estimate
TD, RIN = 5.02e-9, 10 ** (-155 / 10)
f_osc, v_osc = osc[TD]
deck = f"""* OEO, loop opened at the modulator drive: noise at the detector load
X1 {o('a')} nu mwp_laser p_mw={P} rin={RIN}
Vb vb gnd DC {VPI / 2}
Vrf rf gnd DC 0 AC 1
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
X3 {o('b')} {o('c')} mwp_delay td={TD}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
RL an gnd {RL}
.noise V(an) Vrf dec 2 1e9 1e11
.end
"""
r = run(deck, "noise", tag="t04n")
sv = float(np.interp(f_osc, r["x"], r["y"]["S"]))                    # V^2/Hz at the load, around the carrier
idc = RESP * P * 1e-3 / 2                                             # mean photocurrent, the same when oscillating
rep.check("open-loop noise at the detector load: shot + RIN + thermal", sv,
          (2 * Q * idc + RIN * idc ** 2) * RL ** 2 + 4 * KB * 300.15 * RL, 1e-3, unit="V^2/Hz")
v_an = RL * RESP * P * 1e-3 * jv(1, np.pi * v_osc / VPI)              # carrier amplitude at the load, from the simulated drive
delta = sv / (0.5 * v_an ** 2)                                        # noise-to-carrier ratio, 1/Hz
dw = 2 * np.pi * 1e5
tau_f = -(np.angle(z(f_osc + 1e5)) - np.angle(z(f_osc - 1e5))) / (2 * dw)   # tank group delay
tau = TD + tau_f
rep.note(f"phase-noise ESTIMATE (not simulated), carrier {f_osc / 1e9:.4f} GHz, {20 * np.log10(v_an / np.sqrt(2)) + 13.01:.1f} dBm at the load: "
         f"delta = {10 * np.log10(delta):.1f} dB/Hz, tau = {TD * 1e9:.2f} + {tau_f * 1e9:.2f} ns")
for fo in (1e4, 1e5, 1e6, 1e7):
    rep.note(f"  L({fo:.0e} Hz) = {10 * np.log10(delta / (2 * np.pi * fo * tau) ** 2):.1f} dBc/Hz")
rep.note(f"  a 1 km loop (tau = 4.9 us) with the same delta: L(1e4 Hz) = {10 * np.log10(delta / (2 * np.pi * 1e4 * 4.9e-6) ** 2):.1f} dBc/Hz "
         f"(not simulable here: 1e5 RF periods in the delay line)")
rep.done()
