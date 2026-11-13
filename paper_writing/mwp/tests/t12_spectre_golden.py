#!/usr/bin/env python3
"""T12 -- verilog-a-sim against Cadence Spectre, the golden reference for this library.

Spectre (SPECTRE 24.1.0.078) was adopted as a golden reference on 2026-11-12. Its outputs are
committed in golden/spectre/mwp/<deck>/ (ASCII PSF, with the netlist and the log); this test runs
the same circuits here and compares the two simulators directly. Each deck below is the
verilog-a-sim spelling of the Spectre netlist in that directory, parameter for parameter.

Not in this gate, and why:
 - the 4 us noise transients (11_*) and the oscillator transients (22, 25): 30-100 MB of
   waveform each, not committed; they were checked against closed forms (spectre/README.md);
 - the mixer's periodic noise (21) and the oscillator's phase noise (23): verilog-a-sim has no
   periodic noise analysis. The oscillator's phase noise is an open question (README).
Spectre's lin=N is N steps (N+1 points), ours N points: the decks here ask for N+1.
Sign conventions differ (Spectre reports a source's current into its + terminal), so current
responses are compared up to a global sign.
"""
import os, sys
import numpy as np
from mwp import run, o, tones, freq_est, Report, REPO

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "spectre"))
import psf

GOLD = os.path.join(REPO, "golden", "spectre", "mwp")
rep = Report("t12_spectre_golden")
RESP, VPI, P = 0.8, 4.0, 10.0


def gold(deck, pattern, trace):
    import glob
    f = sorted(glob.glob(os.path.join(GOLD, deck, pattern)))
    if not f:
        raise FileNotFoundError(f"{deck}/{pattern} not in {GOLD}")
    d = psf.read(f[0])
    return d[d["_sweep"]], np.asarray(d[trace])


def same_sign(a, b):
    """b, or -b, whichever is closer to a."""
    return b if np.sum(abs(a - b)) <= np.sum(abs(a + b)) else -b


def link(extra, mzm="", pd="", laser=""):
    return f"""* IMDD link (golden/spectre/mwp/01_*)
X1 {o('a')} nu mwp_laser p_mw={P} {laser}
Vb vb gnd DC 2
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} {mzm}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP} {pd}
{extra}
.end
"""

# ---------------------------------------------------------------- 01 link: DC, AC, noise, two tones
fs, gs = gold("01_link_dc", "*.dc", "Vs:p")
r = run(link("Vrf rf gnd DC 0\nVs an gnd DC 0\n.dc Vb 0 8 0.25"), "dc", tag="t12a")
i = r["y"]["I(Vs)"]
rep.check("01 DC transfer: max |i - Spectre| / max |i|", float(np.max(abs(i - same_sign(i, np.real(gs)))) / np.max(abs(i))), 0.0, 1e-6, rel=False)

fs, gs = gold("01_link_ac", "*.ac", "Vs:p")
r = run(link("Vrf rf gnd DC 0 AC 1\nVs an gnd DC 0\n.ac dec 10 1e8 1e11", mzm="f3db=20e9", pd="f3db=30e9"), "ac", report=["Vs"], tag="t12b")
h = r["y"]["I(Vs)"]
rep.check("01 AC response 0.1-100 GHz (complex): max |H - Spectre| / |H(0)|",
          float(np.max(abs(h - same_sign(h, gs))) / abs(gs[0])), 0.0, 3e-4, rel=False)

fs, gs = gold("01_link_noise", "*.noise", "out")
r = run(link("Vrf rf gnd DC 0 AC 1\nRL an gnd 50\n.noise V(an) Vrf dec 2 1e8 1e10",
             laser="rin=3.1622776601683794e-16", mzm="rterm=0"), "noise", tag="t12c")
rep.check("01 output noise at 100 MHz against Spectre", r["y"]["S"][0], float(np.real(gs[0])) ** 2, 1e-4, unit="V^2/Hz")

ts, gs = gold("01_link_tran", "*.tran", "Vs:p")
r = run(link("V1 rf m SIN(0 0.2 1e9)\nV2 m gnd SIN(0 0.2 1.1e9)\nVs an gnd DC 0\n.tran 2e-12 20e-9"), "tran", report=["Vs"], tag="t12d")
F = [1.0e9, 1.1e9, 0.9e9, 1.2e9]
a_us, *_ = tones(r["x"], r["y"]["I(Vs)"], F, t_start=10e-9)
a_sp, *_ = tones(ts, np.real(gs), F, t_start=10e-9)
rep.check("01 two tones: fundamental against Spectre", abs(a_us[0]), abs(a_sp[0]), 1e-5, unit="A")
rep.check("01 two tones: IMD3 (2f1 - f2) against Spectre", abs(a_us[2]), abs(a_sp[2]), 1e-4, unit="A")

# ---------------------------------------------------------------- 02 filter
fs, gs = gold("02_filter_ac", "*.ac", "Vs:p")
deck = f"""* PM -> notch ring -> PD (golden/spectre/mwp/02_filter_ac)
X1 {o('a')} nu mwp_laser p_mw=1 nu0=193.4
Vrf rf gnd DC 0 AC 1
X2 {o('a')} {o('b')} rf gnd mwp_pm vpi={VPI} rterm=0
Vt tune gnd DC 0
X3 {o('b')} {o('t')} {o('d')} nu tune mwp_ring nu_res=193.41 q1=4e5 qi=4e5 kf=1e9
Vk k gnd DC 3
X4 {o('t')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.ac lin 382 1e9 20e9
.end
"""
r = run(deck, "ac", report=["Vs"], tag="t12e")
h = r["y"]["I(Vs)"]
rep.check("02 filter response 1-20 GHz (complex): max |H - Spectre| / max |H|", float(np.max(abs(h - same_sign(h, gs))) / np.max(abs(gs))), 0.0, 3e-4, rel=False)

# ---------------------------------------------------------------- 03 linewidth noise through the delay
fs, gs = gold("03_linewidth_noise", "*.noise", "out")
deck = f"""* delayed self-homodyne discriminator (golden/spectre/mwp/03_linewidth_noise)
X1 {o('a')} nu mwp_laser p_mw=4 lw=1e5 rin=1e-14
X2 {o('a')} gnd gnd {o('c')} {o('d')} mwp_coupler
X3 {o('d')} {o('e')} mwp_delay td=1e-9 phi=1.5707963267948966
X4 {o('c')} {o('e')} {o('p')} {o('n')} mwp_coupler
X5 {o('p')} {o('n')} out gnd mwp_bpd resp={RESP}
RL out gnd 50
Vd dummy gnd DC 0 AC 1
Rd dummy gnd 1
.noise V(out) Vd dec 20 1e5 3e9
.end
"""
r = run(deck, "noise", tag="t12f")
sp = np.real(gs) ** 2
# the two decade grids differ by an endpoint: compare on Spectre's frequencies, log-log interpolated,
# at the points both grids share (relative error at the nulls of sin^2 is not meaningful)
# (va-cli lists the stop frequency after the last decade point, so sort first)
k = np.argsort(r["x"]); fx, sx = r["x"][k], r["y"]["S"][k]
ours = 10 ** np.interp(np.log10(fs), np.log10(fx), np.log10(sx))
common = np.isin(np.round(np.log10(fs), 6), np.round(np.log10(fx), 6))
rep.check("03 noise spectrum 0.1 MHz - 3 GHz: max relative deviation from Spectre", float(np.max(abs(ours[common] / sp[common] - 1))), 0.0, 1e-4, rel=False)

# ---------------------------------------------------------------- 07 true-time-delay beamformer
from mwp import C0
L = ["* TTD beamformer (golden/spectre/mwp/07_ttd_ac)", "Vrf rf gnd DC 0 AC 1", "Vb vb gnd DC 2", "Vk k gnd DC 3"]
for n in range(4):
    nu = 193.4 + 0.1 * n
    L += [f"XL{n} {o(f'a{n}')} nu{n} mwp_laser p_mw=4 nu0={nu:.1f}",
          f"XM{n} {o(f'a{n}')} {o(f'b{n}')} {o(f'x{n}')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0",
          f"XF{n} {o(f'b{n}')} {o(f'c{n}')} mwp_fiber len=918 disp=17 loss=0 nu={nu:.1f} nu_ref=193.4 td0=1e-9",
          f"XD{n} {o(f'c{n}')} an{n} k mwp_pd resp={RESP}", f"Vs{n} an{n} gnd DC 0"]
r = run("\n".join(L + [".ac lin 38 2e9 20e9", ".end"]), "ac", report=[f"Vs{n}" for n in range(4)], tag="t12g")
worst = 0.0
for n in range(4):
    fs, gs = gold("07_ttd_ac", "*.ac", f"Vs{n}:p")
    h = r["y"][f"I(Vs{n})"]
    worst = max(worst, float(np.max(abs(h - same_sign(h, gs))) / np.max(abs(gs))))
rep.check("07 four element responses 2-20 GHz (complex): max |H - Spectre| / |H|", worst, 0.0, 3e-4, rel=False)

# ---------------------------------------------------------------- 20 multiplier: our transient against Spectre's harmonic balance
fs, gs = gold("20_multiplier_hb", "*.fd.pss_hb", "Vs:p")
deck = f"""* MZM multiplier (golden/spectre/mwp/20_multiplier_hb)
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 3 10e9)
Vb vb gnd DC 4
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / 10e9 / 400} {4 / 10e9}
.end
"""
r = run(deck, "tran", report=["Vs"], tag="t12h")
a, dc, _ = tones(r["x"], r["y"]["I(Vs)"], [10e9 * k for k in range(1, 6)])
for k in (2, 4):
    rep.check(f"20 harmonic {k}f: our transient against Spectre's harmonic balance", abs(a[k - 1]),
              float(abs(gs[np.argmin(abs(fs - k * 10e9))])), 1e-5, unit="A")
rep.check("20 mean photocurrent against Spectre", abs(dc), float(abs(gs[np.argmin(abs(fs))])), 1e-6, unit="A")

# ---------------------------------------------------------------- 21 mixer: our transient against Spectre's two-tone harmonic balance
fs, gs = gold("21_mixer_hb_noise", "*.fd.qpss_hb", "an")
deck = f"""* carrier-suppressed mixer (golden/spectre/mwp/21_mixer_hb_noise)
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 0.2 10e9)
Vlo lo gnd SIN(0 1.5 9.92e9)
Vn vn gnd DC 4
Vp vp gnd DC 0
X2 {o('a')} {o('c')} lo rf vn vn vp mwp_iqm vpi={VPI}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
RL an gnd 50
.tran {1 / 10e9 / 40} {2 / 80e6}
.end
"""
r = run(deck, "tran", report=["an"], tag="t12i")
a, dc, _ = tones(r["x"], r["y"]["V(an)"], [80e6, 10e9, 9.92e9, 19.92e9, 20e9, 19.84e9], t_start=1 / 80e6)
# print resolution of time (7 digits, 1e-15 s at 25 ns) limits this to ~3e-4, as in t08
rep.check("21 IF voltage at 80 MHz: our transient against Spectre's harmonic balance", abs(a[0]), float(abs(gs[np.argmin(abs(fs - 80e6))])), 3e-4, unit="V")
rep.check("21 mean voltage against Spectre", abs(dc), float(abs(gs[np.argmin(abs(fs))])), 1e-4, unit="V")

# ---------------------------------------------------------------- 23 oscillator: our transient against Spectre's autonomous harmonic balance
fs, gs = gold("23_oeo_hb_noise", "*.fd.pss_hb", "rf")
RT, F0, QT = 50.0, 10e9, 50.0
deck = f"""* OEO with a 40 GHz detector pole (golden/spectre/mwp/23_oeo_hb_noise)
X1 {o('a')} nu mwp_laser p_mw={P}
Vb vb gnd DC 2
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
X3 {o('b')} {o('c')} mwp_delay td=5.02e-9
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP} f3db=40e9
RL an gnd 50
G1 rf gnd an gnd 0.25
Rt rf gnd {RT}
Lt rf gnd {RT / (2 * np.pi * F0 * QT):.9e}
Ct rf gnd {QT / (2 * np.pi * F0 * RT):.9e}
Ik gnd rf PULSE(0 1e-3 0 10p 10p 50p 1)
.tran 1e-12 200e-9
.end
"""
r = run(deck, "tran", report=["rf"], tag="t12j", env={"VA_ABSDELAY_POINTS": "4096"})
f_us, a_us = freq_est(r["x"], r["y"]["V(rf)"], float(fs[1]), 150e-9)
rep.check("23 oscillation frequency: our transient against Spectre's harmonic balance", f_us, float(fs[1]), 1e-4, unit="Hz")
rep.check("23 oscillation amplitude against Spectre", a_us, float(abs(gs[1])), 5e-3, unit="V")
rep.note(f"23: {r['points']} points, {r['wall_s']:.0f} s; Spectre's phase noise for this deck is NOT compared (open question, README)")

# ---------------------------------------------------------------- 29 delay at the noise frequency
fs, gs = gold("29_hbnoise_delay", "nzc.noise", "out")
deck = """* thermal noise plus a delayed copy (golden/spectre/mwp/29_hbnoise_delay)
Rn x gnd 1000
XZa x gnd y yi mwp_delay td=1e-9
Ea out m x gnd 1
Eb m gnd y gnd 1
Vd dummy gnd DC 0 AC 1
Rd dummy gnd 1
.noise V(out) Vd lin 201 1e7 2e9
.end
"""
r = run(deck, "noise", tag="t12k")
sp = np.real(gs) ** 2
m = sp > 1e-3 * sp.max()                         # away from the nulls, where a relative error is meaningless
rep.check("29 noise of a signal plus its delayed copy, 10 MHz - 2 GHz: max relative deviation from Spectre",
          float(np.max(abs(r["y"]["S"][m] / sp[m] - 1))), 0.0, 1e-4, rel=False)
rep.done()
