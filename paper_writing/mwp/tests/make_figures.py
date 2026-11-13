#!/usr/bin/env python3
"""Figures for the paper, from the same circuits as the validation tests t01..t11.

    python make_figures.py            # all figures (about 4 minutes), into ../figures/
    python make_figures.py oeo beam   # only those whose name contains one of the words

Each figure re-runs its circuit in verilog-a-sim (the tests keep numbers, not curves) with the
parameters of the corresponding test, and draws the simulation against the closed form.

Style, fixed for every figure: transparent background, no title, lines 3.5 pt, 20 pt text
(legible when the figure is reduced to one column of a two-column page), and every curve
has its own line style and marker as well as its colour, so a black-and-white print keeps
them apart. Closed forms are black; simulations are coloured markers or lines.
Output: PDF (vector) and PNG (300 dpi) per figure.
"""
import os, sys
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from scipy.special import jv
from scipy.optimize import brentq
from scipy.signal import hilbert
from mwp import run, o, tones, pwl, resample, dbm, C0, HERE, Q, KB
from fit_dispersion import design, chain

OUT = os.path.join(os.path.dirname(HERE), "figures")
os.makedirs(OUT, exist_ok=True)
BLACK, BLUE, ORANGE, VIOLET = "#000000", "#2a78d6", "#eb6834", "#6250d6"   # validated: CVD dE >= 9
LW, MS = 3.5, 11
plt.rcParams.update({
    "font.size": 20, "axes.labelsize": 22, "xtick.labelsize": 20, "ytick.labelsize": 20, "legend.fontsize": 19,
    "lines.linewidth": LW, "lines.markersize": MS, "lines.markeredgewidth": 2.5,
    "axes.linewidth": 1.8, "xtick.major.width": 1.8, "ytick.major.width": 1.8, "xtick.major.size": 7, "ytick.major.size": 7,
    "xtick.minor.width": 1.2, "ytick.minor.width": 1.2, "xtick.direction": "in", "ytick.direction": "in",
    "xtick.top": True, "ytick.right": True, "legend.frameon": False, "legend.handlelength": 2.6,
    "savefig.transparent": True, "figure.figsize": (7.4, 5.4), "pdf.fonttype": 42,
})
MK = dict(markerfacecolor="none")                # open markers: the line under them stays visible


def save(fig, name):
    fig.tight_layout(pad=0.4)
    for ext in ("pdf", "png"):
        fig.savefig(os.path.join(OUT, f"{name}.{ext}"), dpi=300, transparent=True, bbox_inches="tight", pad_inches=0.04)
    plt.close(fig)
    print("wrote", name)


FIGS = {}


def figure(f):
    FIGS[f.__name__] = f
    return f


RESP, VPI = 0.8, 4.0

# ---------------------------------------------------------------- t01: two-tone, IP3


@figure
def link_ip3():
    P, F1, F2, T = 10.0, 1.0e9, 1.1e9, 10e-9
    vas = np.array([0.05, 0.1, 0.2, 0.4, 0.8, 1.2, 1.6, 2.0])
    p1, p3 = [], []
    for va in vas:
        deck = f"""* two-tone
X1 {o('a')} nu mwp_laser p_mw={P}
V1 rf m SIN(0 {va} {F1})
V2 m gnd SIN(0 {va} {F2})
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran 2e-12 {2 * T}
.end
"""
        r = run(deck, "tran", report=["Vs"], tag="fig_ip3")
        a, *_ = tones(r["x"], r["y"]["I(Vs)"], [F1, F2, 2 * F1 - F2, 2 * F2 - F1], t_start=T)
        p1.append(dbm(a[0])); p3.append(dbm(a[2]))
    pin = lambda v: 10 * np.log10(v ** 2 / 100.0 / 1e-3)
    vv = np.logspace(np.log10(0.03), np.log10(6.0), 300)
    m = np.pi * vv / VPI
    rp = RESP * P * 1e-3
    fig, ax = plt.subplots()
    ax.plot(pin(vv), dbm(rp * jv(1, m) * jv(0, m)), "-", color=BLACK, label="Closed form")
    ax.plot(pin(vv), dbm(rp * np.abs(jv(1, m) * jv(2, m))), "--", color=BLACK)
    s0 = np.pi * rp / (2 * VPI)
    x = np.array([-25.0, 26.0])
    g = 10 * np.log10(s0 ** 2 * 50 * 50)
    iip3 = pin(np.sqrt(8) * VPI / np.pi)
    ax.plot(x, x + g, ":", color="0.45"); ax.plot(x, 3 * x + g - 2 * iip3, ":", color="0.45", label="Slopes 1 and 3")
    ax.plot(pin(vas), p1, "o", color=BLUE, label="Fundamental", **MK)
    ax.plot(pin(vas), p3, "s", color=ORANGE, label="IMD3", **MK)
    ax.set_xlabel("RF input power per tone (dBm)"); ax.set_ylabel("RF output power (dBm)")
    ax.set_xlim(-25, 27); ax.set_ylim(-120, 42)
    ax.legend(loc="upper left", ncol=2, fontsize=17, columnspacing=1.0)
    save(fig, "fig_link_ip3")

# ---------------------------------------------------------------- t02: single-passband filter


@figure
def filter_passband():
    NU, P, QC, F0 = 193.4, 1.0, 4e5, 10e9

    def ring_t(det_hz):
        w0 = 2 * np.pi * (NU + F0 * 1e-12) * 1e12
        r1 = w0 / (2 * QC); tau = 1 / (2 * r1)
        return 1 - 2 * r1 * tau / (1 - 1j * 2 * np.pi * det_hz * tau)

    def href(f, fr):
        H = lambda off: ring_t(fr - off)
        return RESP * P * 1e-3 * 1j * np.pi / VPI * (np.conj(H(0)) * H(f) - H(0) * np.conj(H(-f)))

    fig, ax = plt.subplots()
    for vt, ls, mk, col in ((0.0, "-", "o", BLUE), (4.0, "--", "s", ORANGE), (8.0, "-.", "^", VIOLET)):
        deck = f"""* PM -> notch ring -> PD
X1 {o('a')} nu mwp_laser p_mw={P} nu0={NU}
Vrf rf gnd DC 0 AC 1
X2 {o('a')} {o('b')} rf gnd mwp_pm vpi={VPI} rterm=0
Vt tune gnd DC {vt}
X3 {o('b')} {o('t')} {o('d')} nu tune mwp_ring nu_res={NU + F0 * 1e-12} q1={QC} qi={QC} kf=1e9
Vk k gnd DC 3
X4 {o('t')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.ac lin 461 1e9 24e9
.end
"""
        r = run(deck, "ac", report=["Vs"], tag="fig_filter")
        f, h = r["x"], r["y"]["I(Vs)"]
        ref = abs(href(f, F0)).max()
        ax.plot(f / 1e9, 20 * np.log10(abs(href(f, F0 + 1e9 * vt)) / ref), ls, color=BLACK)
        ax.plot(f[::12] / 1e9, 20 * np.log10(abs(h[::12]) / ref), mk, color=col, label=f"Tuning {vt:.0f} V", **MK)
    ax.plot([], [], "-", color=BLACK, label="Closed form")
    ax.set_xlabel("RF frequency (GHz)"); ax.set_ylabel("RF response (dB)")
    ax.set_xlim(1, 24); ax.set_ylim(-45, 21)
    ax.legend(loc="upper left", ncol=2, fontsize=17, columnspacing=1.0)
    save(fig, "fig_filter_passband")

# ---------------------------------------------------------------- t03: linewidth discriminator


@figure
def linewidth_noise():
    PL, TD, RL = 4.0, 1e-9, 50.0
    fig, ax = plt.subplots()
    for lw, ls, mk, col in ((1e6, "-", "o", BLUE), (1e5, "--", "s", ORANGE), (1e4, "-.", "^", VIOLET)):
        deck = f"""* delayed self-homodyne discriminator
X1 {o('a')} nu mwp_laser p_mw={PL} lw={lw} rin=1e-14
X2 {o('a')} gnd gnd {o('c')} {o('d')} mwp_coupler
X3 {o('d')} {o('e')} mwp_delay td={TD} phi={np.pi / 2}
X4 {o('c')} {o('e')} {o('p')} {o('n')} mwp_coupler
X5 {o('p')} {o('n')} out gnd mwp_bpd resp={RESP}
RL out gnd {RL}
Vd dummy gnd DC 0 AC 1
Rd dummy gnd 1
.noise V(out) Vd dec 6 1e5 6e9
.end
"""
        r = run(deck, "noise", tag="fig_lw")
        f, s = r["x"], r["y"]["S"] / RL ** 2
        ff = np.logspace(5, np.log10(6e9), 2000)
        rp = RESP * PL * 1e-3
        ref = rp ** 2 * 4 * np.sin(np.pi * ff * TD) ** 2 * lw / (np.pi * ff ** 2) + 2 * Q * rp + 4 * KB * 300.15 / RL
        ax.loglog(ff, ref, ls, color=BLACK)
        lab = f"{lw / 1e6:g} MHz" if lw >= 1e6 else f"{lw / 1e3:g} kHz"
        ax.loglog(f, s, mk, color=col, label=f"Linewidth {lab}", **MK)
    ax.axhline(2 * Q * rp + 4 * KB * 300.15 / RL, ls=":", color="0.45", label="Shot + thermal floor")
    ax.plot([], [], "-", color=BLACK, label="Closed form")
    ax.set_xlabel("Frequency (Hz)"); ax.set_ylabel("Current noise (A$^2$/Hz)")
    ax.set_xlim(1e5, 6e9); ax.set_ylim(3e-22, 2e-12)
    ax.legend(loc="upper left", ncol=2, fontsize=15, columnspacing=0.8, handletextpad=0.4)
    save(fig, "fig_linewidth_noise")

# ---------------------------------------------------------------- t04: opto-electronic oscillator


@figure
def oeo_startup():
    P, RL, RT, F0, QT, GM, TD = 10.0, 50.0, 50.0, 10e9, 50.0, 0.25, 5.02e-9
    L, C = RT / (2 * np.pi * F0 * QT), QT / (2 * np.pi * F0 * RT)
    z = lambda f: RT / (1 + 1j * QT * (f / F0 - F0 / f))
    k = 50
    fo = brentq(lambda f: np.angle(z(f)) - 2 * np.pi * (f * TD - k), (k - 0.45) / TD, (k + 0.45) / TD)
    gs = GM * RL * abs(z(fo)) * RESP * P * 1e-3
    vref = brentq(lambda v: gs * jv(1, np.pi * v / VPI) - v, 0.2, 3.8 * VPI / np.pi)
    deck = f"""* opto-electronic oscillator
X1 {o('a')} nu mwp_laser p_mw={P}
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
X3 {o('b')} {o('c')} mwp_delay td={TD}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
RL an gnd {RL}
G1 rf gnd an gnd {GM}
Rt rf gnd {RT}
Lt rf gnd {L:.9e}
Ct rf gnd {C:.9e}
Ik gnd rf PULSE(0 1e-3 0 10p 10p 50p 1)
.tran 1e-12 120e-9
.end
"""
    r = run(deck, "tran", report=["rf"], tag="fig_oeo", env={"VA_ABSDELAY_POINTS": "4096"})
    t, v = r["x"], r["y"]["V(rf)"]
    edges = np.arange(0, 120.01e-9, 0.25e-9)
    idx = np.searchsorted(t, edges)
    env = np.array([np.abs(v[a:b]).max() if b > a else np.nan for a, b in zip(idx[:-1], idx[1:])])
    fig, ax = plt.subplots()
    ax.plot(0.5 * (edges[1:] + edges[:-1]) * 1e9, env, "-", color=BLUE, label="Simulated")
    ax.axhline(vref, ls="--", color=BLACK, label="Loop equation")
    for n in range(1, 6):
        ax.axvline(n * (TD + QT / (np.pi * F0)) * 1e9, ymax=0.06, color="0.45", lw=3)
    ax.plot([], [], "-", color="0.45", lw=3, label="Round trips 1-5")
    ax.set_xlabel("Time (ns)"); ax.set_ylabel("Drive amplitude (V)")
    ax.set_xlim(0, 120); ax.set_ylim(0, 3.6)
    ax.legend(loc="center left", bbox_to_anchor=(0.0, 0.5))
    save(fig, "fig_oeo_startup")

# ---------------------------------------------------------------- t05: 16-QAM and chirp


@figure
def qam_constellation():
    PS, PLO, FC, BAUD, NSYM, TR = 4.0, 4.0, 20e9, 1e9, 64, 100e-12
    rng = np.random.default_rng(7)
    lev = np.array([-3, -1, 1, 3])
    si, sq = rng.choice(lev, NSYM), rng.choice(lev, NSYM)
    fig, ax = plt.subplots(figsize=(6.4, 6.0))
    for frac, mk, col, lab in ((0.1, "o", BLUE, "Peak drive 0.3 $V_\\pi$"), (0.3, "s", ORANGE, "Peak drive 0.9 $V_\\pi$")):
        V0 = frac * VPI

        def nrz(sym):
            t, v = [0.0], [sym[0] * V0]
            for k in range(1, NSYM):
                t += [k / BAUD, k / BAUD + TR]; v += [sym[k - 1] * V0, sym[k] * V0]
            t.append(NSYM / BAUD); v.append(sym[-1] * V0)
            return pwl(t, v)
        deck = f"""* 16-QAM at 20 GHz
X1 {o('a')} nu1 mwp_laser p_mw={PS}
X2 {o('l')} nu2 mwp_laser p_mw={PLO} df={FC}
Vi vi gnd {nrz(si)}
Vq vq gnd {nrz(sq)}
Vbi bi gnd DC {VPI}
Vbq bq gnd DC {VPI}
Vbp bp gnd DC {VPI / 2}
X3 {o('a')} {o('s')} vi vq bi bq bp mwp_iqm vpi={VPI}
X4 {o('s')} {o('l')} {o('c')} {o('d')} mwp_coupler
Vk k gnd DC 3
X5 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / FC / 32} {NSYM / BAUD}
.end
"""
        r = run(deck, "tran", report=["Vs"], tag="fig_qam")
        t, i = r["x"], r["y"]["I(Vs)"]
        amp = RESP * 1e-3 * np.sqrt(PS * PLO) / 2
        rec = []
        for k in range(NSYM):
            m = (t >= k / BAUD + 0.35e-9) & (t <= k / BAUD + 0.85e-9)
            a, *_ = tones(t[m] - k / BAUD, i[m], [FC])
            rec.append((-a[0].imag - 1j * a[0].real) / amp)
        rec = np.array(rec)
        g = np.sum(np.conj(si + 1j * sq) * rec) / np.sum(abs(si + 1j * sq) ** 2)
        ax.plot(rec.real / g.real, rec.imag / g.real, mk, color=col, label=lab, **MK)
    gx, gy = np.meshgrid(lev, lev)
    ax.plot(gx.ravel(), gy.ravel(), "+", color=BLACK, markersize=16, markeredgewidth=3, label="Ideal grid")
    ax.set_xlabel("In-phase"); ax.set_ylabel("Quadrature")
    ax.set_xlim(-4.2, 4.2); ax.set_ylim(-4.2, 5.9); ax.set_aspect("equal")
    ax.set_xticks(lev); ax.set_yticks(lev)
    ax.legend(loc="upper center", ncol=1, fontsize=16, handletextpad=0.2)
    save(fig, "fig_qam_constellation")


@figure
def chirp():
    PA, PB, F0, T, B = 2.0, 2.0, 10e9, 10e-9, 4e9
    K = B / T
    tp = np.linspace(0, T, 2001)
    deck = f"""* LFM chirp
X1 {o('a')} nu1 mwp_laser p_mw={PA}
X2 {o('l')} nu2 mwp_laser p_mw={PB} df={F0}
Vp vp gnd {pwl(tp, VPI * K * (tp - T / 2) ** 2)}
X3 {o('a')} {o('s')} vp gnd mwp_pm vpi={VPI} rterm=0
X4 {o('s')} {o('l')} {o('c')} {o('d')} mwp_coupler
Vk k gnd DC 3
X5 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / (F0 + B) / 64} {T}
.end
"""
    r = run(deck, "tran", report=["Vs"], tag="fig_chirp")
    tu, iu = resample(r["x"], r["y"]["I(Vs)"] - np.mean(r["y"]["I(Vs)"]), 8192)
    fi = np.gradient(np.unwrap(np.angle(hilbert(iu))), tu) / (2 * np.pi)
    m = (tu > 0.6e-9) & (tu < 9.4e-9)
    fig, ax = plt.subplots()
    ax.plot(tu[m][::8] * 1e9, fi[m][::8] / 1e9, "-", color=BLUE, label="Simulated photocurrent")
    ax.plot(tu * 1e9, (F0 - K * (tu - T / 2)) / 1e9, "--", color=BLACK, label="$f_0 - K\\,(t - T/2)$")
    ax.set_xlabel("Time (ns)"); ax.set_ylabel("Beat frequency (GHz)")
    ax.set_xlim(0, 10); ax.set_ylim(7.5, 12.5)
    ax.legend(loc="upper right")
    save(fig, "fig_chirp")

# ---------------------------------------------------------------- t06: optical sampling


@figure
def sampling_spectrum():
    PAVG, FREP, FWHM, NS, M, VA = 1.0, 1e9, 2e-12, 64, 663, 1.6
    CINT = RESP * 1e-3 * PAVG / FREP
    fin = M * FREP / NS
    deck = f"""* photonic sampler
X1 {o('a')} nu mwp_mll pavg_mw={PAVG} frep={FREP} fwhm={FWHM} t0={0.5 / FREP}
Vrf rf gnd SIN(0 {VA} {fin})
Vb vb gnd DC {VPI / 2}
X2 {o('a')} {o('b')} {o('bb')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0
Vk k gnd DC 3
X3 {o('b')} a1 k mwp_pd resp={RESP}
X4 {o('bb')} a2 k mwp_pd resp={RESP}
C1 a1 gnd {CINT:.6e}
C2 a2 gnd {CINT:.6e}
R1 a1 gnd 1e13
R2 a2 gnd 1e13
.tran 50e-12 {NS / FREP}
.end
"""
    r = run(deck, "tran", report=["a1", "a2"], tag="fig_samp", flags=("--reltol", "1e-6"))
    edges = np.arange(NS + 1) / FREP
    e1, e2 = (np.diff(np.interp(edges, r["x"], r["y"][k])) for k in ("V(a1)", "V(a2)"))
    x = (e1 - e2) / (e1 + e2)
    y = np.arcsin(x) / (np.pi * VA / VPI)
    fb = np.arange(NS // 2 + 1) * FREP / NS / 1e6
    fig, ax = plt.subplots()
    for d, ls, mk, col, lab in ((x, "--", "s", ORANGE, "Difference / sum"), (y, "-", "o", BLUE, "After arcsine")):
        X = np.abs(np.fft.rfft(d)) ** 2
        ax.plot(fb[1:], np.maximum(10 * np.log10(X[1:] / X.max()), -118), ls + mk, color=col, label=lab, markersize=8, **MK)
    ax.set_xlabel("Frequency after sampling (MHz)"); ax.set_ylabel("Power (dB re. signal)")
    ax.set_xlim(0, 500); ax.set_ylim(-125, 28)
    ax.legend(loc="upper center", ncol=2, fontsize=17, columnspacing=1.0)
    save(fig, "fig_sampling_spectrum")

# ---------------------------------------------------------------- t07: beam squint


@figure
def beam_squint():
    P, NEL, NU0, DNU, DISP, LEN, TD0, FDES = 4.0, 4, 193.4, 0.1, 17.0, 918.0, 1e-9, 12e9
    AC = ".ac lin 25 8e9 20e9"
    L = ["* TTD", "Vrf rf gnd DC 0 AC 1", f"Vb vb gnd DC {VPI / 2}", "Vk k gnd DC 3"]
    tau = []
    for n in range(NEL):
        nu = NU0 + n * DNU
        L += [f"XL{n} {o(f'a{n}')} nu{n} mwp_laser p_mw={P} nu0={nu}",
              f"XM{n} {o(f'a{n}')} {o(f'b{n}')} {o(f'x{n}')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0",
              f"XF{n} {o(f'b{n}')} {o(f'c{n}')} mwp_fiber len={LEN} disp={DISP} loss=0 nu={nu} nu_ref={NU0} td0={TD0}",
              f"XD{n} {o(f'c{n}')} an{n} k mwp_pd resp={RESP}", f"Vs{n} an{n} gnd DC 0"]
        tau.append(TD0 + DISP * 1e-6 * LEN * (C0 / (nu * 1e12) - C0 / (NU0 * 1e12)))
    r = run("\n".join(L + [AC, ".end"]), "ac", report=[f"Vs{n}" for n in range(NEL)], tag="fig_beam")
    f = r["x"]
    Httd = [r["y"][f"I(Vs{n})"] for n in range(NEL)]
    dtau = np.mean(np.diff(tau))

    def shifter(theta):
        return f"""* RF phase shifter
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
{AC}
.end
"""
    Hps = [run(shifter(2 * np.pi * FDES * n * dtau), "ac", report=["Vs"], tag="fig_beam")["y"]["I(Vs)"] for n in range(NEL)]
    d_el = C0 / 20e9 / 2
    ang = np.deg2rad(np.linspace(-90, 90, 7201))

    def point(H):
        return np.array([np.rad2deg(ang[np.argmax(abs(sum(H[n][k] * np.exp(2j * np.pi * f[k] * n * d_el * np.sin(ang) / C0)
                                                              for n in range(NEL))))]) for k in range(len(f))])
    th0 = np.rad2deg(np.arcsin(C0 * dtau / d_el))
    ff = np.linspace(8e9, 20e9, 300)
    fig, ax = plt.subplots()
    ax.plot(ff / 1e9, np.full_like(ff, th0), "-", color=BLACK, label="True-time delay, closed form")
    ax.plot(ff / 1e9, np.rad2deg(np.arcsin(np.sin(np.deg2rad(th0)) * FDES / ff)), "--", color=BLACK, label="Phase shifters, closed form")
    ax.plot(f[::2] / 1e9, point(Httd)[::2], "o", color=BLUE, label="True-time delay, simulated", **MK)
    ax.plot(f[::2] / 1e9, point(Hps)[::2], "s", color=ORANGE, label="Phase shifters, simulated", **MK)
    ax.set_xlabel("RF frequency (GHz)"); ax.set_ylabel("Beam direction (deg)")
    ax.set_xlim(8, 20); ax.set_ylim(-52, 8)
    ax.legend(loc="upper left", fontsize=16)
    save(fig, "fig_beam_squint")

# ---------------------------------------------------------------- t08: mixer conversion


@figure
def mixer_conversion():
    P, FIF, VRF, VLO, F3 = 10.0, 80e6, 0.2, 1.5, 16.7e9
    RP = RESP * P * 1e-3
    frfs = np.array([2e9, 4e9, 7e9, 10e9, 13e9, 16e9])

    def deck(kind, frf):
        mod = (f"Vb vb gnd DC {VPI / 2}\n"
               f"X2 {o('a')} {o('b')} {o('x1')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0 f3db={F3}\n"
               f"X3 {o('b')} {o('c')} {o('x2')} lo gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0 f3db={F3}") if kind == "series" else \
              (f"Vn vn gnd DC {VPI}\nVp vp gnd DC 0\nX2 {o('a')} {o('c')} lo rf vn vn vp mwp_iqm vpi={VPI} f3db={F3}")
        return f"""* mixer {kind}
X1 {o('a')} nu mwp_laser p_mw={P}
Vrf rf gnd SIN(0 {VRF} {frf})
Vlo lo gnd SIN(0 {VLO} {frf - FIF})
{mod}
Vk k gnd DC 3
X4 {o('c')} an k mwp_pd resp={RESP}
Vs an gnd DC 0
.tran {1 / frf / 160} {2 / FIF}
.end
"""
    pole = lambda f: 1 / np.sqrt(1 + (f / F3) ** 2)
    ff = np.linspace(1e9, 17e9, 200)
    ref = {"series": lambda f: (RP / 2 * jv(1, np.pi * VRF / VPI * pole(f)) * jv(1, np.pi * VLO / VPI * pole(f - FIF)), RP / 4),
           "parallel": lambda f: (RP * jv(1, np.pi * VRF / (2 * VPI) * pole(f)) * jv(1, np.pi * VLO / (2 * VPI) * pole(f - FIF)),
                                  RP / 4 * (1 - 0.5 * jv(0, np.pi * VRF / VPI * pole(f)) - 0.5 * jv(0, np.pi * VLO / VPI * pole(f - FIF))))}
    fig, ax = plt.subplots()
    for kind, ls, mk, col, lab in (("parallel", "-", "o", BLUE, "Carrier-suppressed (dual-parallel)"), ("series", "--", "s", ORANGE, "Conventional (series)")):
        y = []
        for frf in frfs:
            r = run(deck(kind, frf), "tran", report=["Vs"], tag="fig_mixer")
            a, dc, _ = tones(r["x"], r["y"]["I(Vs)"], [FIF, frf, frf - FIF, 2 * frf - FIF, 2 * frf, 2 * (frf - FIF)], t_start=1 / FIF)
            y.append(20 * np.log10(abs(a[0]) / dc))
        i_if, i_dc = ref[kind](ff)
        ax.plot(ff / 1e9, 20 * np.log10(i_if / i_dc), ls, color=BLACK)
        ax.plot(frfs / 1e9, y, mk, color=col, label=lab, **MK)
    ax.plot([], [], "-", color=BLACK, label="Closed form")
    ax.set_xlabel("RF frequency (GHz)"); ax.set_ylabel("IF current / DC current (dB)")
    ax.set_xlim(1, 17); ax.set_ylim(-34, 0); ax.set_xticks(range(2, 17, 2))
    ax.legend(loc="upper right", fontsize=16)
    save(fig, "fig_mixer_conversion")

# ---------------------------------------------------------------- t09: dispersion fading


@figure
def dispersion_fading():
    P, NU0, DISP, LEN, BAND = 4.0, 193.4, 17.0, 25e3, 32e9
    S0 = np.pi * RESP * P * 1e-3 / (2 * VPI)
    lam = C0 / (NU0 * 1e12)
    ff = np.linspace(0.1e9, 25e9, 1500)
    fig, ax = plt.subplots()
    ax.plot(ff / 1e9, np.maximum(20 * np.log10(abs(np.cos(np.pi * lam ** 2 * DISP * 1e-6 * LEN * ff ** 2 / C0))), -60), "-", color=BLACK,
            label="Fibre, 25 km")
    for n, ls, col in ((8, "--", BLUE), (4, "-.", ORANGE)):
        poles, info = design(DISP, LEN, BAND, n)
        deck = "\n".join([f"* {n} all-pass sections", f"X1 {o('a')} nu mwp_laser p_mw={P} nu0={NU0}", "Vrf rf gnd DC 0 AC 1",
                          f"Vb vb gnd DC {VPI / 2}", f"X2 {o('a')} {o('b')} {o('xx')} rf gnd vb mwp_mzm vpi={VPI} vpi_dc={VPI} rterm=0"]
                         + chain(poles, "XR", "b", "c", "nu", NU0)
                         + ["Vk k gnd DC 3", f"X3 {o('c')} an k mwp_pd resp={RESP}", "Vs an gnd DC 0", ".ac lin 499 0.1e9 25e9", ".end"])
        r = run(deck, "ac", report=["Vs"], tag="fig_disp")
        ax.plot(r["x"] / 1e9, np.maximum(20 * np.log10(abs(r["y"]["I(Vs)"]) / S0), -60), ls, color=col, label=f"{n} all-pass sections")
    ax.axvline(BAND / 2e9, ls=":", color="0.45", label="Edge of the fitted band")
    ax.set_xlabel("RF frequency (GHz)"); ax.set_ylabel("RF response (dB)")
    ax.set_xlim(0, 25); ax.set_ylim(-50, 36)
    ax.legend(loc="upper left", ncol=1, fontsize=17)
    save(fig, "fig_dispersion_fading")

# ---------------------------------------------------------------- t10: optical amplifier


@figure
def amplifier_gain():
    G0_DB, NF_DB = 20.0, 5.0
    G0 = 10 ** (G0_DB / 10)
    pin_dbm = np.arange(-30.0, 12.1, 3.0)
    pp = np.logspace(-3, 1.2, 300)
    fig, ax = plt.subplots()
    for psat, ls, mk, col in ((10.0, "-", "o", BLUE), (1.0, "--", "s", ORANGE)):
        pout = []
        for d in pin_dbm:
            deck = f"""* amplifier
X1 {o('a')} nu mwp_laser p_mw={10 ** (d / 10)}
X2 {o('a')} {o('b')} mwp_oamp g0_db={G0_DB} psat={psat} nf_db={NF_DB}
Vk k gnd DC 3
X3 {o('b')} an k mwp_pd resp=1
RL an gnd 1
.end
"""
            r = run(deck, "op", tag="fig_oamp")
            pout.append(10 * np.log10(r["y"]["V(an)"][0] * 1e3))
        ax.plot(10 * np.log10(pp), 10 * np.log10(pp * G0 / (1 + G0 * pp / psat)), ls, color=BLACK)
        ax.plot(pin_dbm, pout, mk, color=col, label=f"$P_{{sat}}$ = {10 * np.log10(psat):.0f} dBm", **MK)
    ax.plot([], [], "-", color=BLACK, label="Closed form")
    ax.plot(10 * np.log10(pp), 10 * np.log10(pp) + G0_DB, ":", color="0.45", label="Small-signal gain, 20 dB")
    ax.set_xlabel("Optical input power (dBm)"); ax.set_ylabel("Optical output power (dBm)")
    ax.set_xlim(-31, 13); ax.set_ylim(-14, 24)
    ax.legend(loc="upper left", fontsize=17)
    save(fig, "fig_amplifier_gain")

# ---------------------------------------------------------------- t11: noise in the time domain


@figure
def transient_noise():
    from scipy.signal import welch
    T0, R, C, FMAX, GAIN, TSTOP = 300.15, 1e3, 1e-12, 20e9, 1e4, 4e-6
    DT = 0.5 / FMAX
    fig, ax = plt.subplots()
    ff = np.logspace(6.5, np.log10(FMAX), 400)
    fc = 1 / (2 * np.pi * R * C)
    ax.loglog(ff, 4 * KB * T0 * R / (1 + (ff / fc) ** 2) * np.sinc(ff * DT) ** 4, "-", color=BLACK,
              label="Closed form, sampled noise")
    ax.loglog(ff, 4 * KB * T0 * R / (1 + (ff / fc) ** 2), ":", color="0.45", label="Closed form, unlimited band")
    for how, mk, col, lab in (("SIM", "o", BLUE, "Simulator, --tran-noise"), ("LIB", "s", ORANGE, "Library module")):
        L = ["* RC thermal noise", f"R0 gnd n0 {R}", f"C0 n0 gnd {C}"]
        if how == "LIB":
            L.append(f"XN0 gnd n0 mwp_noise psd={4 * KB * T0 / R:.9e} fmax={FMAX} seed0=1")
        L += [f"E1 amp gnd n0 gnd {GAIN}", "R99 amp gnd 1e9", f".tran {DT} {TSTOP}", ".end"]
        r = run("\n".join(L), "tran", report=["amp"], tag="fig_tnoise",
                flags=("--abstol", "1e-9") + (("--tran-noise", str(FMAX)) if how == "SIM" else ()))
        tu = np.arange(0, TSTOP, DT / 2)
        f, s = welch(np.interp(tu, r["x"], r["y"]["V(amp)"] / GAIN), fs=2 / DT, nperseg=16384, detrend="constant")
        edges = np.logspace(6.6, np.log10(FMAX), 19)
        fb, sb = [], []
        for lo, hi in zip(edges[:-1], edges[1:]):
            m = (f >= lo) & (f < hi)
            if m.any():
                fb.append(np.sqrt(lo * hi)); sb.append(s[m].mean())
        ax.loglog(fb, sb, mk, color=col, label=lab, markersize=16 if how == "SIM" else 9, **MK)
    ax.set_xlabel("Frequency (Hz)"); ax.set_ylabel("Voltage noise (V$^2$/Hz)")
    ax.set_xlim(3e6, 2.2e10); ax.set_ylim(1e-23, 1e-16)
    ax.legend(loc="lower left", fontsize=17)
    save(fig, "fig_transient_noise")


if __name__ == "__main__":
    want = sys.argv[1:]
    for name, f in FIGS.items():
        if not want or any(w in name for w in want):
            f()
