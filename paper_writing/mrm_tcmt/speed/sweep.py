#!/usr/bin/env python3
"""NRZ / PAM4 at increasing symbol rate: how much does the ring's dynamic response close the eye,
and how do loaded Q and laser detuning change that?  ngspice + OSDI, self-heating off."""
import json, os, subprocess, sys
import numpy as np

NG = "/opt/ngspice-new/bin/ngspice"
HERE = os.path.dirname(os.path.abspath(__file__))

def prbs9(n):
    s = 0x1FF; out = []
    for _ in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1; s = ((s << 1) | b) & 0x1FF; out.append(b)
    return out

def symbols(fmt):
    bits = prbs9(511)
    if fmt == "NRZ":
        return np.array(bits, float), 2
    b = bits + bits[:1]                                   # 512 bits -> 256 PAM4 symbols, Gray
    gray = {(0, 0): 0, (0, 1): 1, (1, 1): 2, (1, 0): 3}
    return np.array([gray[(b[2 * i], b[2 * i + 1])] for i in range(256)], float) / 3.0, 4

def pwl(levels, T, tr_frac=0.1):
    pts = [(0.0, levels[0])]; t = 0.0
    for i in range(1, len(levels)):
        t = i * T
        if levels[i] != levels[i - 1]:
            pts.append((t, levels[i - 1])); pts.append((t + tr_frac * T, levels[i]))
    pts.append((len(levels) * T, levels[-1]))
    return " ".join(f"{a:.6e} {b:.6g}" for a, b in pts)

def run(fmt, baud, Qc, det_pm, drv_bw=50e9, Qi=2e4, tag="x"):
    lev, M = symbols(fmt); T = 1 / baud
    name = f"{tag}_{fmt}_{baud/1e9:.0f}_{Qc:.0f}_{det_pm:+.0f}".replace("+", "p").replace("-", "m")
    deck = f"""* {name}
.model drv mrm_driver linear=1 vlo=0 vhi=2 f3db={drv_bw:g} Rout=50
.model mrm mrm_tcmt mode=0 lambda0=1550 FSR=10 Qi={Qi:g} Qc={Qc:g} eta_mod=20
+ Rs=100 Cj0=20f Vbi=0.8 Mj=0.5 Cpad=10f dlam_dT=75 Rth=5k Cth=1n Rh0=1k f_abs=0.5 self_heat=0
Vd din 0 PWL({pwl(lev, T)})
N0 din kk 0 drv
Vin_re in_re 0 1.0
Vin_im in_im 0 0
Vwl wl 0 {1550 + det_pm * 1e-3:.6f}
Vh hp 0 0
N1 0 kk hp 0 in_re in_im th_re th_im dr_re dr_im wl tdt mrm
.control
pre_osdi mrm_tcmt.osdi
pre_osdi mrm_driver.osdi
set wr_singlescale
set wr_vecnames
option numdgt=10
tran {T/100:.4e} {len(lev)*T:.4e} 0 {T/40:.4e}
wrdata {name}.dat v(th_re) v(th_im)
quit
.endc
.end
"""
    wd = os.path.join(HERE, "work"); os.makedirs(wd, exist_ok=True)
    open(os.path.join(wd, name + ".cir"), "w").write(deck)
    subprocess.run([NG, "-b", name + ".cir"], cwd=wd, capture_output=True)
    d = np.loadtxt(os.path.join(wd, name + ".dat"), skiprows=1); t = d[:, 0]; P = (d[:, 1] ** 2 + d[:, 2] ** 2) * 1e3
    sym = np.rint(lev * (M - 1)).astype(int); skip = 16
    best = None
    for ph in np.arange(0, 3 * T, T / 100):                # latency < 3 symbols
        idx = np.arange(skip, len(lev) - 3)
        s = np.interp(idx * T + ph, t, P); k = sym[idx]
        eyes = [s[k == j + 1].min() - s[k == j].max() for j in range(M - 1)]
        if best is None or min(eyes) > min(best[1]):
            means = [s[k == j].mean() for j in range(M)]
            best = (ph, eyes, means)
    ph, eyes, means = best
    oma = means[-1] - means[0]
    spacing = np.diff(means)
    return dict(fmt=fmt, baud=baud, Qc=Qc, det_pm=det_pm, ph_ps=ph * 1e12, oma=oma,
                er_db=10 * np.log10(means[-1] / means[0]),
                worst_eye=min(eyes), worst_eye_norm=min(eyes) / (oma / (M - 1)),
                rlm=(min(spacing) / (oma / (M - 1))) if M > 2 else 1.0, eyes=eyes, means=means)

if __name__ == "__main__":
    out = []
    configs = [("NRZ", 25e9), ("NRZ", 50e9), ("NRZ", 100e9), ("PAM4", 25e9), ("PAM4", 50e9)]
    # loaded Q via coupling: Qc 22k (Q_L 10.5k, tau 17 ps), 8k (Q_L 5.7k, 9.4 ps), 4k (Q_L 3.3k, 5.5 ps)
    for Qc in (22000, 8000, 4000):
        QL = 1 / (1 / 2e4 + 1 / Qc); fwhm = 1550e3 / QL
        for fmt, baud in configs:
            rows = []
            for frac in (-0.15, -0.25, -0.35, -0.5, -0.7, -0.9):
                r = run(fmt, baud, Qc, frac * fwhm); r["det_over_fwhm"] = frac; rows.append(r)
            b = max(rows, key=lambda r: r["worst_eye"])
            out += rows
            print(f"Q_L {QL:6.0f} (FWHM {fwhm:4.0f} pm)  {fmt:4s} {baud/1e9:4.0f} GBd | best detuning {b['det_pm']:+5.0f} pm "
                  f"({b['det_over_fwhm']:+.2f} FWHM): OMA {b['oma']:6.1f} uW  worst eye {b['worst_eye']:6.1f} uW "
                  f"= {100*b['worst_eye_norm']:5.1f} % of ideal  RLM {b['rlm']:.2f}", flush=True)
    json.dump(out, open(os.path.join(HERE, "sweep.json"), "w"), indent=1, default=float)
