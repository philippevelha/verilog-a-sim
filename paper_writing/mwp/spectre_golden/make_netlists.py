#!/usr/bin/env python3
"""Generate the Spectre netlists that give every test (t01-t11) a golden reference.

    python make_netlists.py          # writes netlists/*.scs and MANIFEST.md, then the package

One netlist per circuit and analysis of each test, with the test's own parameters (copied from
tests/tNN_*.py; where a test draws random symbols the same generator and seed are used here).
t12 is the gate itself and needs no circuit of its own.

Transient outputs are thinned with `strobeperiod` (and `strobestart` where only the settled part
is used), so a result stays small enough to commit; the strobe interval is stated per netlist.
Each netlist is plain Spectre: `include "models.scs"` and the library in the same directory.
"""
import os, sys, tarfile, shutil
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(PKG, "tests"))
from fit_dispersion import design                      # noqa: E402

C0 = 2.99792458e8
KB = 1.380649e-23
OUT = os.path.join(HERE, "netlists")
HEAD = ('simulator lang=spectre\nglobal 0\ninclude "models.scs"\n'
        "opts options reltol=1e-6 vabstol=1e-9 iabstol=1e-15 temp=27 tnom=27 gmin=1e-18 save=selected\n")
NETS = {}                                               # name -> (test, what, text)


def o(n):
    return f"{n}_re {n}_im"


def g(x):
    return f"{x:.12g}"


def add(name, test, what, body):
    NETS[name] = (test, what, f"// {name} -- golden reference for {test}: {what}\n" + HEAD + body.lstrip("\n"))


def pwl(t, v):
    pts = [f"{g(a)} {g(b)}" for a, b in zip(t, v)]
    lines = [" ".join(pts[i:i + 6]) for i in range(0, len(pts), 6)]
    return "type=pwl wave=[ \\\n    " + " \\\n    ".join(lines) + " ]"

RESP, VPI = 0.8, 4.0

# ---------------------------------------------------------------- t01 IMDD link
def link(extra, mzm="", pd="", laser="", vb="dc=2"):
    return f"""
X1 ({o('a')} nu) mwp_laser p_mw=10 {laser}
Vb (vb 0) vsource {vb}
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 {mzm}
Vk (k 0) vsource dc=3
X3 ({o('b')} an k) mwp_pd resp=0.8 {pd}
{extra}"""

add("t01_dc_ideal", "t01 (a)", "bias sweep, ideal extinction",
    link("Vrf (rf 0) vsource dc=0\nVs (an 0) vsource dc=0\nsave Vs:p\nsw dc dev=Vb param=dc start=0 stop=8 step=0.25\n"))
add("t01_dc_er20", "t01 (a)", "bias sweep, extinction ratio 20 dB",
    link("Vrf (rf 0) vsource dc=0\nVs (an 0) vsource dc=0\nsave Vs:p\nsw dc dev=Vb param=dc start=0 stop=8 step=0.25\n", mzm="er_db=20"))
add("t01_ac", "t01 (b)", "small-signal response, modulator 20 GHz and detector 30 GHz poles",
    link("Vrf (rf 0) vsource dc=0 mag=1\nVs (an 0) vsource dc=0\nsave Vs:p\nac1 ac start=1e8 stop=1e11 dec=10\n",
         mzm="f3db=20e9", pd="f3db=30e9"))
for va in (0.2, 0.4, 0.8):
    add(f"t01_tran_{int(va * 100):03d}", "t01 (c)", f"two tones 1.0/1.1 GHz, {va} V each (strobe 2 ps)",
        link(f"V1 (rf m) vsource type=sine ampl={va} freq=1.0e9\nV2 (m 0) vsource type=sine ampl={va} freq=1.1e9\n"
             "Vs (an 0) vsource dc=0\nsave Vs:p\ntr tran stop=20e-9 maxstep=2e-12 strobeperiod=2e-12\n"))
add("t01_noise", "t01 (d)", "output noise: shot + RIN + load thermal",
    link("Vrf (rf 0) vsource dc=0 mag=1\nRL (an 0) resistor r=50\nsave an\nnz (an 0) noise start=1e8 stop=1e10 dec=2 iprobe=Vrf\n",
         laser="rin=3.1622776601683794e-16", mzm="rterm=0"))

# ---------------------------------------------------------------- t02 ring and filter
add("t02_ring_dc", "t02 (a)", "add-drop ring, tuning sweep -60..60 V at 1 GHz/V", f"""
X1 ({o('a')} nu) mwp_laser p_mw=1 nu0=193.4
Vt (tune 0) vsource dc=0
X2 ({o('a')} {o('t')} {o('d')} nu tune) mwp_ring nu_res=193.4 q1=2e4 q2=2e4 qi=1e5 kf=1e9
Vk (k 0) vsource dc=3
X3 ({o('t')} at k) mwp_pd resp=1
X4 ({o('d')} ad k) mwp_pd resp=1
Vst (at 0) vsource dc=0
Vsd (ad 0) vsource dc=0
save Vst:p Vsd:p
sw dc dev=Vt param=dc start=-60 stop=60 step=0.5
""")


def filt(vt, src, ana):
    return f"""
X1 ({o('a')} nu) mwp_laser p_mw=1 nu0=193.4
{src}
X2 ({o('a')} {o('b')} rf 0) mwp_pm vpi=4 rterm=0
Vt (tune 0) vsource dc={vt}
X3 ({o('b')} {o('t')} {o('d')} nu tune) mwp_ring nu_res=193.41 q1=4e5 qi=4e5 kf=1e9
Vk (k 0) vsource dc=3
X4 ({o('t')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
{ana}"""

for vt in (0, 5):
    add(f"t02_filter_ac_{vt}v", "t02 (b)", f"single-passband filter, tuning {vt} V (381 points 1-20 GHz)",
        filt(vt, "Vrf (rf 0) vsource dc=0 mag=1", "ac1 ac start=1e9 stop=20e9 lin=380\n"))
add("t02_filter_tran", "t02 (c)", "filter in transient at 10 GHz, 0.05 V (strobe 1 ps)",
    filt(0, "Vrf (rf 0) vsource type=sine ampl=0.05 freq=10e9", "tr tran stop=6e-9 maxstep=1e-12 strobeperiod=1e-12\n"))

# ---------------------------------------------------------------- t03 RF generation
for df in (1e9, 10e9, 40e9, 100e9):
    add(f"t03_het_{int(df / 1e9):03d}g", "t03 (a)", f"two lasers {df / 1e9:g} GHz apart on one photodiode (strobe T/64)", f"""
X1 ({o('a')} nu1) mwp_laser p_mw=1
X2 ({o('b')} nu2) mwp_laser p_mw=2 df={g(df)}
X3 ({o('a')} {o('b')} {o('c')} {o('d')}) mwp_coupler
Vk (k 0) vsource dc=3
X4 ({o('c')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
tr tran stop={g(20 / df)} maxstep={g(1 / df / 64)} strobeperiod={g(1 / df / 64)}
""")
for name, vb, va in (("null", 4.0, 3.0), ("peak", 0.0, 3.0), ("j2zero", 4.0, 5.1356223 * VPI / np.pi)):
    add(f"t03_mult_{name}", "t03 (b)", f"MZM frequency multiplier at {name} bias, 10 GHz, {va:.6g} V (strobe T/400)", f"""
X1 ({o('a')} nu) mwp_laser p_mw=10
Vrf (rf 0) vsource type=sine ampl={g(va)} freq=10e9
Vb (vb 0) vsource dc={g(vb)}
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4
Vk (k 0) vsource dc=3
X3 ({o('b')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
tr tran stop=4e-10 maxstep=2.5e-13 strobeperiod=2.5e-13
""")
add("t03_linewidth_noise", "t03 (c)", "laser linewidth through a delayed self-homodyne discriminator", f"""
X1 ({o('a')} nu) mwp_laser p_mw=4 lw=1e5 rin=1e-14
X2 ({o('a')} 0 0 {o('c')} {o('d')}) mwp_coupler
X3 ({o('d')} {o('e')}) mwp_delay td=1e-9 phi=1.5707963267948966
X4 ({o('c')} {o('e')} {o('p')} {o('n')}) mwp_coupler
X5 ({o('p')} {o('n')} out 0) mwp_bpd resp=0.8
RL (out 0) resistor r=50
save out
nz (out 0) noise start=1e5 stop=3e9 dec=20
""")

# ---------------------------------------------------------------- t04 opto-electronic oscillator (no detector pole, as in t04)
RT, F0, QT = 50.0, 10e9, 50.0
LT, CT = RT / (2 * np.pi * F0 * QT), QT / (2 * np.pi * F0 * RT)
for td in (5.02e-9, 4.98e-9):
    add(f"t04_oeo_{int(round(td * 1e11)):03d}", "t04", f"oscillator start-up, loop delay {td * 1e9:.2f} ns (strobe 2.5 ps from 100 ns)", f"""
X1 ({o('a')} nu) mwp_laser p_mw=10
Vb (vb 0) vsource dc=2
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
X3 ({o('b')} {o('c')}) mwp_delay td={g(td)}
Vk (k 0) vsource dc=3
X4 ({o('c')} an k) mwp_pd resp=0.8
RL (an 0) resistor r=50
G1 (rf 0 an 0) vccs gm=0.25
Rt (rf 0) resistor r=50
Lt (rf 0) inductor l={g(LT)}
Ct (rf 0) capacitor c={g(CT)}
Ik (0 rf) isource type=pulse val0=0 val1=1e-3 delay=0 rise=10e-12 fall=10e-12 width=50e-12 period=1
save rf
tr tran stop=200e-9 maxstep=1e-12 strobeperiod=2.5e-12 strobestart=100e-9
""")
add("t04_open_noise", "t04 (estimate)", "loop opened at the modulator: noise at the detector load", f"""
X1 ({o('a')} nu) mwp_laser p_mw=10 rin=3.1622776601683794e-16
Vb (vb 0) vsource dc=2
Vrf (rf 0) vsource dc=0 mag=1
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
X3 ({o('b')} {o('c')}) mwp_delay td=5.02e-9
Vk (k 0) vsource dc=3
X4 ({o('c')} an k) mwp_pd resp=0.8
RL (an 0) resistor r=50
save an
nz (an 0) noise start=1e9 stop=1e11 dec=2 iprobe=Vrf
""")

# ---------------------------------------------------------------- t05 waveform generation
FC, BAUD, NSYM, TR = 20e9, 1e9, 32, 100e-12
rng = np.random.default_rng(7)
lev = np.array([-3, -1, 1, 3])
si, sq = rng.choice(lev, NSYM), rng.choice(lev, NSYM)
V0 = 0.1 * VPI


def nrz(sym):
    t, v = [0.0], [sym[0] * V0]
    for k in range(1, NSYM):
        t += [k / BAUD, k / BAUD + TR]; v += [sym[k - 1] * V0, sym[k] * V0]
    t.append(NSYM / BAUD); v.append(sym[-1] * V0)
    return pwl(t, v)

add("t05_qam", "t05 (a)", "16-QAM at 20 GHz by heterodyning, 32 symbols at 1 GBd, rng seed 7 (strobe T/32)", f"""
X1 ({o('a')} nu1) mwp_laser p_mw=4
X2 ({o('l')} nu2) mwp_laser p_mw=4 df=20e9
Vi (vi 0) vsource {nrz(si)}
Vq (vq 0) vsource {nrz(sq)}
Vbi (bi 0) vsource dc=4
Vbq (bq 0) vsource dc=4
Vbp (bp 0) vsource dc=2
X3 ({o('a')} {o('s')} vi vq bi bq bp) mwp_iqm vpi=4
X4 ({o('s')} {o('l')} {o('c')} {o('d')}) mwp_coupler
Vk (k 0) vsource dc=3
X5 ({o('c')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
tr tran stop=32e-9 maxstep={g(1 / FC / 32)} strobeperiod={g(1 / FC / 32)}
""")
T5, K5 = 10e-9, 4e9 / 10e-9
tp = np.linspace(0, T5, 2001)
add("t05_chirp", "t05 (b)", "4 GHz linear-FM chirp in 10 ns around 10 GHz (strobe 1/(14 GHz)/64)", f"""
X1 ({o('a')} nu1) mwp_laser p_mw=2
X2 ({o('l')} nu2) mwp_laser p_mw=2 df=10e9
Vp (vp 0) vsource {pwl(tp, VPI * K5 * (tp - T5 / 2) ** 2)}
X3 ({o('a')} {o('s')} vp 0) mwp_pm vpi=4 rterm=0
X4 ({o('s')} {o('l')} {o('c')} {o('d')}) mwp_coupler
Vk (k 0) vsource dc=3
X5 ({o('c')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
tr tran stop=10e-9 maxstep={g(1 / 14e9 / 64)} strobeperiod={g(1 / 14e9 / 64)}
""")

# ---------------------------------------------------------------- t06 optical sampling
CINT = RESP * 1e-3 * 1.0 / 1e9


def sampler(nsamp, mcyc, va, jit, seed):
    fin = mcyc * 1e9 / nsamp
    return f"""
X1 ({o('a')} nu) mwp_mll pavg_mw=1 frep=1e9 fwhm=2e-12 t0=0.5e-9 jit={g(jit)} seed0={seed}
Vrf (rf 0) vsource type=sine ampl={g(va)} freq={g(fin)}
Vb (vb 0) vsource dc=2
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
Vk (k 0) vsource dc=3
X3 ({o('b')} a1 k) mwp_pd resp=0.8
X4 ({o('bb')} a2 k) mwp_pd resp=0.8
C1 (a1 0) capacitor c={g(CINT)}
C2 (a2 0) capacitor c={g(CINT)}
R1 (a1 0) resistor r=1e13 isnoisy=no
R2 (a2 0) resistor r=1e13 isnoisy=no
save a1 a2
tr tran stop={g(nsamp / 1e9)} strobeperiod=50e-12
"""

add("t06_sampler", "t06 (a)", "1 GS/s photonic sampler, 64 pulses, 10.36 GHz tone; pulse charge on 0.8 pF (strobe 50 ps)",
    sampler(64, 663, 1.6, 0.0, 1))
add("t06_jitter", "t06 (b)", "the sampler with 100 fs rms timing jitter, 256 pulses (Spectre's $rdist_normal: statistics only)",
    sampler(256, 2653, 0.4, 100e-15, 3))
FR2, NS2, M2, T0 = 2e9, 64, 331, 0.25e-9
ph = 90.0 - 360.0 * (FR2 / 2) * T0
add("t06_demux", "t06 (c)", "2 GS/s samples demultiplexed 1:2 by a 1 GHz switch onto two 4 GHz detectors (strobe 25 ps)", f"""
X1 ({o('a')} nu) mwp_mll pavg_mw=1 frep=2e9 fwhm=2e-12 t0={g(T0)}
Vrf (rf 0) vsource type=sine ampl=1.6 freq={g(M2 * FR2 / NS2)}
Vb (vb 0) vsource dc=2
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
Vsw (sw 0) vsource type=sine sinedc=0.5 ampl=0.5 freq=1e9 sinephase={g(ph)}
X3 ({o('b')} 0 0 {o('c')} {o('d')} sw) mwp_switch vsw=1
Vk (k 0) vsource dc=3
X4 ({o('c')} a1 k) mwp_pd resp=0.8 f3db=4e9
X5 ({o('d')} a2 k) mwp_pd resp=0.8 f3db=4e9
C1 (a1 0) capacitor c={g(RESP * 1e-3 / FR2)}
C2 (a2 0) capacitor c={g(RESP * 1e-3 / FR2)}
R1 (a1 0) resistor r=1e13 isnoisy=no
R2 (a2 0) resistor r=1e13 isnoisy=no
save a1 a2
tr tran stop={g(NS2 / FR2 + 1e-9)} strobeperiod=25e-12
""")

# ---------------------------------------------------------------- t07 beamforming
ttd = ["Vrf (rf 0) vsource dc=0 mag=1", "Vb (vb 0) vsource dc=2", "Vk (k 0) vsource dc=3"]
tau = []
for n in range(4):
    nu = 193.4 + 0.1 * n
    ttd += [f"XL{n} ({o(f'a{n}')} nu{n}) mwp_laser p_mw=4 nu0={nu:.1f}",
            f"XM{n} ({o(f'a{n}')} {o(f'b{n}')} {o(f'x{n}')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0",
            f"XF{n} ({o(f'b{n}')} {o(f'c{n}')}) mwp_fiber len=918 disp=17 loss=0 nu={nu:.1f} nu_ref=193.4 td0=1e-9",
            f"XD{n} ({o(f'c{n}')} an{n} k) mwp_pd resp=0.8", f"Vs{n} (an{n} 0) vsource dc=0"]
    tau.append(1e-9 + 17e-6 * 918 * (C0 / (nu * 1e12) - C0 / 193.4e12))
dtau = float(np.mean(np.diff(tau)))
add("t07_ttd_ac", "t07 (a)", "dispersive WDM true-time-delay beamformer, four elements",
    "\n".join(ttd + ["save Vs0:p Vs1:p Vs2:p Vs3:p", "ac1 ac start=2e9 stop=20e9 lin=36", ""]))


def shifter(theta):
    return f"""
X1 ({o('a')} nu) mwp_laser p_mw=4
X2 ({o('a')} 0 0 {o('s')} {o('c')}) mwp_coupler
Vi (vi 0) vsource dc=0 mag=1 phase=0
Vq (vq 0) vsource dc=0 mag=1 phase=-90
Vbi (bi 0) vsource dc=4
Vbq (bq 0) vsource dc=4
Vbp (bp 0) vsource dc=2
X3 ({o('s')} {o('m')} vi vq bi bq bp) mwp_iqm vpi=4
X4 ({o('c')} {o('p')} 0) mwp_phase phi0={g(theta)}
X5 ({o('m')} {o('p')} {o('u')} {o('v')}) mwp_coupler
Vk (k 0) vsource dc=3
X6 ({o('u')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
ac1 ac start=2e9 stop=20e9 lin=36
"""

for deg in range(0, 360, 45):
    add(f"t07_shift_{deg:03d}", "t07 (b)", f"photonic RF phase shifter, optical phase {deg} deg", shifter(np.deg2rad(deg)))
for n in range(4):
    add(f"t07_beam_el{n}", "t07 (b)", f"phase-shifter beamformer element {n}, set for 12 GHz", shifter(2 * np.pi * 12e9 * n * dtau))
for code in range(8):
    bits = [(code >> k) & 1 for k in range(3)]
    ctl = [bits[0]] + [bits[k - 1] ^ bits[k] for k in range(1, 3)] + [bits[2]]
    L = [f"X1 ({o('a')} nu) mwp_laser p_mw=4", "Vrf (rf 0) vsource dc=0 mag=1", "Vb (vb 0) vsource dc=2",
         f"X2 ({o('a')} {o('s0a')} {o('xx')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0"]
    pa, pb = "s0a", None
    for k in range(4):
        L.append(f"Vc{k} (ctl{k} 0) vsource dc={ctl[k]}")
        bi = "0 0" if pb is None else o(pb)
        L.append(f"XS{k} ({o(pa)} {bi} {o(f'c{k}')} {o(f'd{k}')} ctl{k}) mwp_switch vsw=1")
        if k < 3:
            L.append(f"XD{k} ({o(f'd{k}')} {o(f'e{k}')}) mwp_delay td={g(10e-12 * 2 ** k)}")
            pa, pb = f"c{k}", f"e{k}"
    L += ["Vk (k 0) vsource dc=3", f"X9 ({o('c3')} an k) mwp_pd resp=0.8", "Vs (an 0) vsource dc=0",
          "save Vs:p", "ac1 ac start=2e9 stop=20e9 lin=36", ""]
    add(f"t07_switched_{code:03b}", "t07 (c)", f"3-bit switched delay line, code {code:03b} ({code * 10} ps)", "\n".join(L))
add("t07_ring_allpass", "t07 (d)", "all-pass ring on resonance as an RF delay", f"""
X1 ({o('a')} nu) mwp_laser p_mw=4
Vrf (rf 0) vsource dc=0 mag=1
Vb (vb 0) vsource dc=2
X2 ({o('a')} {o('b')} {o('xx')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
X3 ({o('b')} {o('t')} {o('d')} nu 0) mwp_ring nu_res=193.4 q1=2e4
Vk (k 0) vsource dc=3
X4 ({o('t')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
ac1 ac start=1e7 stop=2e10 dec=20
""")

# ---------------------------------------------------------------- t08 mixers
def mixer(kind, frf, f3=0.0):
    flo = frf - 80e6
    pole = f" f3db={g(f3)}" if f3 else ""
    mod = (f"Vb (vb 0) vsource dc=2\n"
           f"X2 ({o('a')} {o('b')} {o('x1')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0{pole}\n"
           f"X3 ({o('b')} {o('c')} {o('x2')} lo 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0{pole}") if kind == "series" else \
          (f"Vn (vn 0) vsource dc=4\nVp (vp 0) vsource dc=0\n"
           f"X2 ({o('a')} {o('c')} lo rf vn vn vp) mwp_iqm vpi=4{pole}")
    return f"""
X1 ({o('a')} nu) mwp_laser p_mw=10
Vrf (rf 0) vsource type=sine ampl=0.2 freq={g(frf)} fundname="frf"
Vlo (lo 0) vsource type=sine ampl=1.5 freq={g(flo)} fundname="flo"
{mod}
Vk (k 0) vsource dc=3
X4 ({o('c')} an k) mwp_pd resp=0.8
Vs (an 0) vsource dc=0
save Vs:p
hb1 hb funds=["flo" "frf"] maxharms=[9 5] errpreset=conservative
"""

add("t08_series_hb", "t08", "dual-series MZM mixer, RF 10 GHz, LO 9.92 GHz", mixer("series", 10e9))
add("t08_parallel_hb", "t08", "carrier-suppressed dual-parallel MZM mixer, RF 10 GHz, LO 9.92 GHz", mixer("parallel", 10e9))
for frf in (2e9, 6e9, 10e9, 16e9):
    add(f"t08_parallel_pole_{int(frf / 1e9):02d}g", "t08", f"parallel mixer with 16.7 GHz modulator poles, RF {frf / 1e9:g} GHz -> 80 MHz",
        mixer("parallel", frf, 16.7e9))

# ---------------------------------------------------------------- t09 in-band dispersion as all-pass rings
for n in (4, 8, 12):
    poles, info = design(17.0, 25e3, 32e9, n)
    L = [f"X1 ({o('a')} nu) mwp_laser p_mw=4 nu0=193.4", "Vrf (rf 0) vsource dc=0 mag=1", "Vb (vb 0) vsource dc=2",
         f"X2 ({o('a')} {o('b')} {o('xx')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0"]
    prev = "b"
    for k, (d, gk) in enumerate(poles):
        nxt = "c" if k == n - 1 else f"r{k}"
        nu_res = 193.4 + d * 1e-12
        q1 = 2 * np.pi * nu_res * 1e12 / (2 * 2 * np.pi * gk)
        L.append(f"XR{k} ({o(prev)} {o(nxt)} {o(f'dr{k}')} nu 0) mwp_ring nu_res={nu_res:.12f} q1={q1:.9e}")
        prev = nxt
    L += ["Vk (k 0) vsource dc=3", f"X3 ({o('c')} an k) mwp_pd resp=0.8", "Vs (an 0) vsource dc=0",
          "save Vs:p", "ac1 ac start=0.1e9 stop=25.1e9 lin=250", ""]
    add(f"t09_chain_{n:02d}", "t09", f"25 km of fibre as {n} all-pass ring sections (fit_dispersion.py)", "\n".join(L))

# ---------------------------------------------------------------- t10 optical amplifier


def amp(pin, ana):
    return f"""
X1 ({o('a')} nu) mwp_laser p_mw={g(pin)}
X2 ({o('a')} {o('b')}) mwp_oamp g0_db=20 psat=10 nf_db=5 nu0=193.4
Vk (k 0) vsource dc=3
X3 ({o('b')} an k) mwp_pd resp=0.8
RL (an 0) resistor r=50
Vd (dummy 0) vsource dc=0 mag=1
Rd (dummy 0) resistor r=1 isnoisy=no
save an
{ana}"""

for dbm in (-30, -20, -10, 0, 10):
    add(f"t10_gain_{'m' if dbm < 0 else 'p'}{abs(dbm):02d}", "t10 (a)", f"saturated gain at {dbm:+d} dBm in", amp(10 ** (dbm / 10), "op1 dc\n"))
for dbm in (-30, -10):
    add(f"t10_noise_m{abs(dbm):02d}", "t10 (b)", f"signal-spontaneous beat noise at {dbm:+d} dBm in",
        amp(10 ** (dbm / 10), "nz (an 0) noise start=1e8 stop=1e10 dec=2 iprobe=Vd\n"))

# ---------------------------------------------------------------- t11 noise in the time domain
PSD_R = 4 * KB * 300.15 / 1e3


def ladder(n, how, tstop):
    L = []
    for k in range(n):
        a = "0" if k == 0 else f"n{k - 1}"
        L += [f"R{k} ({a} n{k}) resistor r=1000" + (" isnoisy=no" if how == "lib" else ""), f"C{k} (n{k} 0) capacitor c=1e-12"]
        if how == "lib":
            L.append(f"XN{k} ({a} n{k}) mwp_noise psd={PSD_R:.9e} fmax=20e9 seed0={k + 1}")
    noise = " noisefmax=20e9 noiseseed=1" if how == "sim" else ""
    L += [f"save n{n - 1}", f"tr tran stop={g(tstop)} maxstep=25e-12 strobeperiod=25e-12{noise}", ""]
    return "\n".join(L)

for how, lab in (("sim", "Spectre's transient noise"), ("lib", "the library's mwp_noise")):
    add(f"t11_rc_{how}", "t11 (a)", f"1 kOhm on 1 pF, 4 us, {lab}: kT/C (strobe 25 ps)", ladder(1, how, 4e-6))
    add(f"t11_ladder_{how}", "t11 (b)", f"50-section RC ladder, 1 us, {lab}: equipartition (strobe 25 ps)", ladder(50, how, 1e-6))
VPI_, P_ = 4.0, 10.0
v_hi = 2 * VPI_ / np.pi * np.arccos(np.sqrt(0.9)); v_lo = 2 * VPI_ / np.pi * np.arccos(np.sqrt(0.1))
i_hi = RESP * P_ * 1e-3 * 0.9
CL = 1 / (2 * np.pi * 50 * 0.5e9)
for how in ("sim", "lib"):
    extra = (f"XN1 (0 an) mwp_noise psd={2 * 1.602176634e-19 * i_hi:.6e} fmax=20e9 seed0=1\n"
             f"XN2 (0 an) mwp_noise psd={4 * KB * 300.15 / 50:.6e} fmax=20e9 seed0=2\n") if how == "lib" else ""
    noise = " noisefmax=20e9 noiseseed=1" if how == "sim" else ""
    add(f"t11_shot_{how}", "t11 (c)", f"shot noise under a stepped optical power, 50 ohm / 500 MHz load, {how} (strobe 25 ps)", f"""
X1 ({o('a')} nu) mwp_laser p_mw=10
Vrf (rf 0) vsource dc=0
Vb (vb 0) vsource type=pulse val0={g(v_hi)} val1={g(v_lo)} delay=1000e-9 rise=1e-9 fall=1e-9 width=1000e-9 period=10
X2 ({o('a')} {o('b')} {o('bb')} rf 0 vb) mwp_mzm vpi=4 vpi_dc=4 rterm=0
Vk (k 0) vsource dc=3
X3 ({o('b')} an k) mwp_pd resp=0.8
RL (an 0) resistor r=50{' isnoisy=no' if how == 'lib' else ''}
CL (an 0) capacitor c={g(CL)}
{extra}save an
tr tran stop=2000e-9 maxstep=25e-12 strobeperiod=25e-12{noise}
""")

# ---------------------------------------------------------------- write
if __name__ == "__main__":
    shutil.rmtree(OUT, ignore_errors=True); os.makedirs(OUT)
    for name, (test, what, text) in NETS.items():
        with open(os.path.join(OUT, name + ".scs"), "w", newline="\n", encoding="utf-8") as f:
            f.write(text)
    lines = ["# Manifest: one Spectre netlist per circuit of every test", "",
             f"{len(NETS)} netlists, generated by `make_netlists.py`.", "",
             "| Netlist | Test | Circuit and analysis |", "|---|---|---|"]
    lines += [f"| `{n}` | {t} | {w} |" for n, (t, w, _) in NETS.items()]
    open(os.path.join(HERE, "MANIFEST.md"), "w", newline="\n", encoding="utf-8").write("\n".join(lines) + "\n")
    # the package to copy to the server: netlists, models.scs, run.sh, the library
    tar = os.path.join(HERE, "mwp_golden_package.tar.gz")
    with tarfile.open(tar, "w:gz") as tf:
        for name in sorted(os.listdir(OUT)):
            tf.add(os.path.join(OUT, name), arcname=f"mwp_golden/{name}")
        for f in ("run.sh", "models.scs", "README.md", "MANIFEST.md"):
            tf.add(os.path.join(HERE, f), arcname=f"mwp_golden/{f}")
        for f in sorted(os.listdir(os.path.join(PKG, "lib"))):
            if f.startswith("mwp") and (f.endswith(".va") or f.endswith(".vams")):
                tf.add(os.path.join(PKG, "lib", f), arcname=f"mwp_golden/lib/{f}")
    print(f"{len(NETS)} netlists -> {OUT}; package {tar}")
