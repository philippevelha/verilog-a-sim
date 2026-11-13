#!/usr/bin/env python3
"""Compare the Spectre runs (the archive run.sh produced) with the closed forms of the tests.

    mkdir results && tar xzf mwp_spectre_results.tar.gz -C results
    python compare_spectre.py results

Run against the archive of 2026-11-12 (SPECTRE 24.1.0.078). Where a result file or a trace is
missing the script lists what the directory and the file do contain instead of failing.
Closed forms are the ones of tests t01, t02, t03, t07, t11.
"""
import glob, os, sys
import numpy as np
from scipy.special import jv
import psf

Q, KB, C0, T0 = 1.602176634e-19, 1.380649e-23, 2.99792458e8, 300.15
root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "golden", "spectre", "mwp")
rows = []


def load(net, pattern):
    files = sorted(glob.glob(os.path.join(root, net, pattern)))
    if not files:
        print(f"[missing] {net}: no {pattern}; directory has {sorted(os.listdir(os.path.join(root, net))) if os.path.isdir(os.path.join(root, net)) else 'nothing'}")
        return None
    return psf.read(files[0])


def get(d, net, name):
    if d is None:
        return None
    if name not in d:
        print(f"[missing] {net}: no trace {name!r}; file has {[k for k in d if k != '_sweep']}")
        return None
    return d[name]


def check(what, sim, ref, tol, rel=True):
    err = abs(sim - ref) / abs(ref) if rel else abs(sim - ref)
    rows.append((what, sim, ref, err, tol, err <= tol))
    print(f"  [{'ok' if err <= tol else 'DIFF'}] {what}: spectre {sim:.6g}, closed form {ref:.6g} ({'rel' if rel else 'abs'} {err:.1e}, tol {tol:g})")


def tones(t, y, freqs, t0):
    m = t >= t0
    t, y = t[m], y[m]
    w = np.zeros_like(t); w[1:] += 0.5 * np.diff(t); w[:-1] += 0.5 * np.diff(t)
    sw = np.sqrt(w / w.sum())
    M = np.stack([np.ones_like(t)] + [g(2 * np.pi * f * t) for f in freqs for g in (np.cos, np.sin)], axis=1)
    sol, *_ = np.linalg.lstsq(M * sw[:, None], y * sw, rcond=None)
    return [np.hypot(sol[1 + 2 * k], sol[2 + 2 * k]) for k in range(len(freqs))]

P, RESP, VPI, RL = 10.0, 0.8, 4.0, 50.0
RP = RESP * P * 1e-3
print("01 IMDD link")
d = load("01_link_dc", "*.dc"); i = get(d, "01_link_dc", "Vs:p")
if i is not None:
    vb = d[d["_sweep"]]
    # the sign of a source's current is Spectre's convention: compare magnitudes
    check("DC transfer, max |i - RP cos^2| / RP", float(np.max(abs(abs(i) - RP * np.cos(np.pi * vb / VPI / 2) ** 2)) / RP), 0.0, 1e-6, rel=False)
d = load("01_link_ac", "*.ac"); h = get(d, "01_link_ac", "Vs:p")
if h is not None:
    f = d[d["_sweep"]]; s0 = np.pi * RP / (2 * VPI)
    ref = s0 / abs((1 + 1j * f / 20e9) * (1 + 1j * f / 30e9))
    check("AC response, max ||H| - two poles| / |H(0)|", float(np.max(abs(abs(h) - ref)) / s0), 0.0, 2e-4, rel=False)
d = load("01_link_noise", "*.noise"); n = get(d, "01_link_noise", "out")
if n is not None:
    idc = RP / 2; rin = 10 ** -15.5
    ref = (2 * Q * idc + rin * idc ** 2) * RL ** 2 + 4 * KB * T0 * RL
    v = float(np.real(n[0]))
    # Spectre reports V/sqrt(Hz) for the total output noise; accept either that or V^2/Hz
    check("output noise (V^2/Hz)", v * v if abs(v * v / ref - 1) < abs(v / ref - 1) else v, ref, 2e-3)
d = load("01_link_tran", "*.tran"); i = get(d, "01_link_tran", "Vs:p")
if i is not None:
    t = d[d["_sweep"]]; a = tones(t, np.real(i), [1.0e9, 1.1e9, 0.9e9], 10e-9); m = np.pi * 0.2 / VPI
    check("two-tone fundamental", a[0], RP * jv(1, m) * jv(0, m), 1e-4)
    check("two-tone IMD3", a[2], RP * jv(1, m) * jv(2, m), 1e-3)

print("02 single-passband filter")
d = load("02_filter_ac", "*.ac"); h = get(d, "02_filter_ac", "Vs:p")
if h is not None:
    f = d[d["_sweep"]]
    w0 = 2 * np.pi * 193.41e12; r1 = w0 / (2 * 4e5); tau = 1 / (2 * r1)
    H = lambda off: 1 - 2 * r1 * tau / (1 - 1j * 2 * np.pi * (10e9 - off) * tau)
    ref = abs(RESP * 1e-3 * np.pi / VPI * (np.conj(H(0)) * H(f) - H(0) * np.conj(H(-f))))
    check("RF response, max ||H| - closed form| / max", float(np.max(abs(abs(h) - ref)) / ref.max()), 0.0, 2e-4, rel=False)
    check("passband centre (Hz)", float(f[np.argmax(abs(h))]), 10e9, 0.06e9, rel=False)

print("03 linewidth discriminator (noise through absdelay)")
d = load("03_linewidth_noise", "*.noise"); n = get(d, "03_linewidth_noise", "out")
if n is not None:
    f = d[d["_sweep"]]; rp = RESP * 4e-3
    ref = (rp ** 2 * 4 * np.sin(np.pi * f * 1e-9) ** 2 * 1e5 / (np.pi * f ** 2) + 2 * Q * rp + 4 * KB * T0 / RL) * RL ** 2
    v = np.real(n); v2 = v * v if abs(v[0] ** 2 / ref[0] - 1) < abs(v[0] / ref[0] - 1) else v
    check("noise spectrum 0.1 MHz - 3 GHz, max relative deviation", float(np.max(abs(v2 / ref - 1))), 0.0, 2e-3, rel=False)

print("07 true-time-delay beamformer")
d = load("07_ttd_ac", "*.ac")
if d is not None and get(d, "07_ttd_ac", "Vs0:p") is not None:
    f = d[d["_sweep"]]
    for n_el in (1, 2, 3):
        nu = 193.4 + 0.1 * n_el
        dt = 17e-6 * 918 * (C0 / (nu * 1e12) - C0 / 193.4e12)
        ph = np.unwrap(np.angle(d[f"Vs{n_el}:p"] / d["Vs0:p"]))
        check(f"element {n_el} delay against element 0 (s)", float(np.polyfit(f, ph, 1)[0] / (-2 * np.pi)), dt, 5e-15, rel=False)

print("11 noise in the time domain (kT/C = %.4e V^2)" % (KB * T0 / 1e-12))
for net, what in (("11_rc_trannoise", "Spectre transient noise"), ("11_rc_libnoise", "library mwp_noise")):
    d = load(net, "*.tran"); v = get(d, net, "out")
    if v is not None:
        t = d[d["_sweep"]]; v = np.real(v)
        w = np.diff(t); vm = 0.5 * (v[1:] + v[:-1]); mean = np.sum(w * vm) / w.sum()
        check(f"{what}: variance against kT/C ({len(t)} points)", float(np.sum(w * (vm - mean) ** 2) / w.sum()), KB * T0 / 1e-12, 0.10)

print("20 frequency multiplier (harmonic balance)")
d = load("20_multiplier_hb", "*.fd.pss_hb"); i = get(d, "20_multiplier_hb", "Vs:p")
if i is not None:
    f = d[d["_sweep"]]; m = np.pi * 3 / VPI
    amp = lambda k: float(abs(i[np.argmin(abs(f - k * 10e9))]))
    check("2f amplitude R P J2(m)", amp(2), RP * abs(jv(2, m)), 1e-9)
    check("4f amplitude R P J4(m)", amp(4), RP * abs(jv(4, m)), 1e-9)
    check("odd harmonics f, 3f / (R P)", max(amp(1), amp(3)) / RP, 0.0, 1e-9, rel=False)

print("21 carrier-suppressed mixer (two-tone harmonic balance + periodic noise)")
u1, u2 = np.pi * 0.2 / (2 * VPI), np.pi * 1.5 / (2 * VPI)
i_dc = RP / 4 * (1 - 0.5 * jv(0, 2 * u1) - 0.5 * jv(0, 2 * u2))
d = load("21_mixer_hb_noise", "*.fd.qpss_hb"); v = get(d, "21_mixer_hb_noise", "an")
if v is not None:
    f = d[d["_sweep"]]
    check("IF current at 80 MHz, R P J1 J1", float(abs(v[np.argmin(abs(f - 80e6))])) / RL, RP * jv(1, u1) * jv(1, u2), 1e-9)
    check("mean photocurrent", float(abs(v[np.argmin(abs(f))])) / RL, i_dc, 1e-9)
d = load("21_mixer_hb_noise", "*hbnoise"); n = get(d, "21_mixer_hb_noise", "out")
if n is not None:
    # cyclostationary sources, time-averaged: shot 2 q <i>, RIN rin <i^2>, with i(t) = (RP/4)(sin a + sin b)^2
    ph = (np.arange(256) + 0.5) * 2 * np.pi / 256
    a, b = np.meshgrid(u1 * np.sin(ph), u2 * np.sin(ph))
    i2 = float(np.mean((RP / 4 * (np.sin(a) + np.sin(b)) ** 2) ** 2))
    rin = 10 ** -15.5
    ref = (2 * Q * i_dc + rin * i2) * RL ** 2 + 4 * KB * T0 * RL
    check("noise at the IF port: thermal + 2 q <i> + RIN <i^2> (V^2/Hz)", float(np.real(n[0])) ** 2, ref, 1e-3)
    if "X1" in d:
        check("  of which laser RIN, rin <i^2> R^2", float(np.asarray(d["X1"])[0][-1]), rin * i2 * RL ** 2, 1e-3)

print("22 opto-electronic oscillator (transient)")
d = load("22_oeo_tran", "*.tran"); v = get(d, "22_oeo_tran", "rf")
if v is not None:
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests"))
    from mwp import freq_est
    f_osc, a_osc = freq_est(d[d["_sweep"]], np.real(v), 9.9695e9, 150e-9)
    # loop-equation values, from tests/t04_oeo.py for td = 5.02 ns
    check("oscillation frequency against the loop equation", f_osc, 9.96955e9, 1e-4)
    check("amplitude at the modulator against the loop equation", a_osc, 2.70507, 5e-3)

print("23-25 opto-electronic oscillator with a 40 GHz detector pole")
from scipy.optimize import brentq
RT_, F0_, QT_, GM_, TD_, FPD_ = 50.0, 10e9, 50.0, 0.25, 5.02e-9, 40e9
zt = lambda f: RT_ / (1 + 1j * QT_ * (f / F0_ - F0_ / f)) / (1 + 1j * f / FPD_)
f_loop = brentq(lambda f: np.angle(zt(f)) - 2 * np.pi * (f * TD_ - 50), 49.55 / TD_, 50.45 / TD_)
gs = GM_ * RL * abs(zt(f_loop)) * RP
v_loop = brentq(lambda v: gs * jv(1, np.pi * v / VPI) - v, 0.2, 3.8 * VPI / np.pi)
d = load("25_oeo_tran_bw", "*.tran"); v = get(d, "25_oeo_tran_bw", "rf")
if v is not None:
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests"))
    from mwp import freq_est
    f_t, a_t = freq_est(d[d["_sweep"]], np.real(v), f_loop, 150e-9)
    check("25 transient: frequency against the loop equation", f_t, f_loop, 1e-4)
    check("25 transient: amplitude against the loop equation", a_t, v_loop, 5e-3)
for net in ("23_oeo_hb_noise", "24_oeo_hb_alt"):
    d = load(net, "*.fd.pss_hb"); v = get(d, net, "rf")
    if v is None:
        continue
    fr = d[d["_sweep"]]
    if abs(v[1]) < 1e-3:
        print(f"  [WRONG SOLUTION] {net}: harmonic balance converged to the non-oscillating state "
              f"(fundamental {abs(v[1]):.1e} V at {fr[1]:.6g} Hz) without reporting an error")
        rows.append((f"{net}: oscillating solution found", 0.0, 1.0, 1.0, 0.0, False))
        continue
    check(f"{net[:2]} harmonic balance: frequency against the loop equation", float(fr[1]), f_loop, 1e-4)
    check(f"{net[:2]} harmonic balance: amplitude against the loop equation", float(abs(v[1])), v_loop, 5e-3)
    dn = load(net, "*.pm.pnoise_hbnoise")
    if dn is not None and "out" in dn:
        fo = dn[dn["_sweep"]]
        # noiseout=pm is single-sideband (help_hbnoise.txt), V/sqrt(Hz) at the output node:
        # L(f) = out^2 / (carrier power V1^2/2)
        L = 10 * np.log10(np.real(dn["out"]) ** 2 / (abs(v[1]) ** 2 / 2))
        for x in (1e4, 1e5, 1e6, 1e7):
            print(f"    {net[:2]} phase noise L({x:.0e} Hz) = {np.interp(np.log10(x), np.log10(fo), L):.1f} dBc/Hz")
        if "cornerFreq" in dn:
            print(f"    {net[:2]} Lorentzian corner {float(np.ravel(np.real(dn['cornerFreq']))[0]):.1f} Hz")

print("29 delay at the noise frequency: plain noise and hbnoise, absdelay and tline")
ref29 = lambda f: 4 * KB * T0 * 1e3 * 4 * np.cos(np.pi * f * 1e-9) ** 2
for pat, what in (("nzc.noise", "noise, mwp_delay"), ("nzt.noise", "noise, tline"),
                  ("hbn*hbnoise", "hbnoise, mwp_delay"), ("hbt*hbnoise", "hbnoise, tline (known: Spectre loses the tline path here)")):
    d = load("29_hbnoise_delay", pat)
    n = None if d is None else (d.get("out") if "out" in d else d.get("outt"))
    if n is None:
        continue
    f = d[d["_sweep"]]; v = np.real(n); v2 = v if v.max() < 1e-12 else v * v      # ~8e-9 V/sqrt(Hz) or ~7e-17 V^2/Hz
    ok = f < 1.9e9
    m = ok & (ref29(f) > 1e-18)
    dev = float(np.max(abs(v2[m] / ref29(f[m]) - 1)))
    flat = float(np.median(v2[ok]) / (16 * KB * T0 * 1e3))
    check(f"{what}: max deviation from 4kTR 4cos^2(pi f td) (away from the nulls)", dev, 0.0, 1e-2, rel=False)
    print(f"      {what}: value near the 0.5 GHz null / 4*4kTR = {float(np.interp(0.5e9, f, v2)) / (16 * KB * T0 * 1e3):.3g}  (0 = delay applied, 1 = delay ignored)")

print(f"\n{sum(r[5] for r in rows)} of {len(rows)} comparisons within tolerance")
for name in ("summary.txt", "environment.txt"):
    p = os.path.join(root, name)
    if os.path.exists(p):
        print(f"\n--- {name}\n" + open(p, errors="replace").read().strip())
