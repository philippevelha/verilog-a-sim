#!/usr/bin/env python3
"""Joint sweep of loaded Q (via the bus coupling Qc, Qi fixed) x modulation efficiency eta_mod,
with the laser detuning scanned on both sides of resonance, for PAM4 50 GBd, NRZ 50 GBd and
NRZ 100 GBd.  Runs in ngspice (OpenVAF/OSDI) or verilog-a-sim, same models, same stimulus.

    python3 sweep_q_eta.py --sim ngspice        # ~4 min on 2 cores
    python3 sweep_q_eta.py --sim va             # verilog-a-sim (VA_JIT=on), slower
    python3 sweep_q_eta.py --sim va --quick     # a 12-point subset, for a smoke test
    options: --jobs N (parallel processes, default 2)

Writes sweep_q_eta_<sim>.json (one record per run).  Plot with plot_q_eta.py.
Tool paths: NGSPICE, VACLI, OPENVAF environment variables (defaults below).

Assumptions (stated, not modelled): Cj0, Rs and Qi do not change with eta_mod; driver is a
linear DAC (50 GHz, 2 poles, 50 ohm), 0-2 V reverse bias; self-heating off; PRBS9; no noise.
Metric: worst inner eye height at the best common sampling phase (first 16 symbols skipped).
"""
import json, os, re, shutil, subprocess, sys, time
from multiprocessing import Pool
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)                       # folder holding mrm_tcmt.va / mrm_driver.va
NG = os.environ.get("NGSPICE", "/opt/ngspice-new/bin/ngspice")
VACLI = os.environ.get("VACLI", "/home/claude/verilog-a-sim/target/release/va-cli")
OPENVAF = os.environ.get("OPENVAF", "/home/claude/arpadbuermen/openvaf/target/release/openvaf-r")

QI = 2e4
QC = [22000, 14000, 10000, 8000, 6000, 4000, 3000]          # Q_L 10.5k ... 2.6k
ETA = [10, 20, 30, 40, 50, 60]                               # pm/V at 0 V
FRAC = [-0.9, -0.7, -0.5, -0.4, -0.3, -0.2, -0.15, -0.1, -0.05, 0.0,
        0.05, 0.1, 0.2, 0.3, 0.4, 0.5, 0.7, 0.9]             # laser detuning / FWHM
FMTS = [("PAM4", 50e9), ("NRZ", 50e9), ("NRZ", 100e9)]

MRM = ("lambda0=1550 FSR=10 Qi={Qi:g} Qc={Qc:g} eta_mod={eta} Rs=100 Cj0=20e-15 Vbi=0.8 Mj=0.5 "
       "Cpad=10e-15 dlam_dT=75 Rth=5e3 Cth=1e-9 Rh0=1e3 f_abs=0.5 self_heat=0")
DRV = "linear=1 vlo=0 vhi=2 f3db=50e9 Rout=50"

def prbs9(n):
    s = 0x1FF; out = []
    for _ in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1; s = ((s << 1) | b) & 0x1FF; out.append(b)
    return out

def symbols(fmt):
    bits = prbs9(511)
    if fmt == "NRZ":
        return np.array(bits, float), 2
    b = bits + bits[:1]                                      # 256 Gray-coded PAM4 symbols
    gray = {(0, 0): 0, (0, 1): 1, (1, 1): 2, (1, 0): 3}
    return np.array([gray[(b[2 * i], b[2 * i + 1])] for i in range(256)], float) / 3.0, 4

def pwl(levels, T, tr=0.1):
    pts = [(0.0, levels[0])]
    for i in range(1, len(levels)):
        if levels[i] != levels[i - 1]:
            pts += [(i * T, levels[i - 1]), (i * T + tr * T, levels[i])]
    pts.append((len(levels) * T, levels[-1]))
    return " ".join(f"{a:.6e} {b:.6g}" for a, b in pts)

def eye(t, P, lev, M, T):
    sym = np.rint(lev * (M - 1)).astype(int); idx = np.arange(16, len(lev) - 3); k = sym[idx]
    best = None
    for ph in np.arange(0, 3 * T, T / 100):                  # latency < 3 symbols
        s = np.interp(idx * T + ph, t, P)
        means = np.array([s[k == j].mean() for j in range(M)])
        if means[-1] < means[0]:                             # red side: inverted polarity
            s = -s; means = -means
        e = [s[k == j + 1].min() - s[k == j].max() for j in range(M - 1)]
        if best is None or min(e) > best["worst"]:
            oma = means[-1] - means[0]
            best = dict(worst=float(min(e)), oma=float(oma), norm=float(min(e) / (oma / (M - 1))),
                        rlm=float(np.min(np.diff(means)) / (oma / (M - 1))) if M > 2 else 1.0,
                        ph=float(ph))
    return best

def deck_ngspice(name, lev, T, Qc, eta, lam):
    return f"""* {name}
.model drv mrm_driver {DRV}
.model mrm mrm_tcmt mode=0 {MRM.format(Qi=QI, Qc=Qc, eta=eta)}
Vd din 0 PWL({pwl(lev, T)})
N0 din kk 0 drv
Vin_re in_re 0 1.0
Vin_im in_im 0 0
Vwl wl 0 {lam:.6f}
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

def deck_va(name, lev, T, Qc, eta, lam):
    # verilog-a-sim: parameters on the instance line; .tran tstep acts as the step ceiling
    return f"""* {name}
Vd din gnd PWL({pwl(lev, T)})
X0 din kk gnd mrm_driver {DRV}
Vre inre gnd DC 1.0
Vim inim gnd DC 0
Vwl wl gnd DC {lam:.6f}
Vh hp gnd DC 0
X1 gnd kk hp gnd inre inim tre tim dre dim wl tdt mrm_tcmt {MRM.format(Qi=QI, Qc=Qc, eta=eta)}
.tran {T/40:.4e} {len(lev)*T:.4e}
.end
"""

VA_RE = re.compile(r"t=([-\d.e]+)s\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V")

def job(args):
    sim, wd, fmt, baud, Qc, eta, frac = args
    QL = 1 / (1 / QI + 1 / Qc); fwhm = 1550e3 / QL; det = frac * fwhm
    lev, M = symbols(fmt); T = 1 / baud; lam = 1550 + det * 1e-3
    name = f"{fmt}_{baud/1e9:.0f}_{Qc}_{eta}_{frac:+.2f}".replace("+", "p").replace("-", "m").replace(".", "d")
    rec = dict(sim=sim, fmt=fmt, baud=baud, Qc=Qc, QL=QL, eta=eta, frac=frac, det_pm=det)
    t0 = time.time()
    try:
        if sim == "ngspice":
            open(os.path.join(wd, name + ".cir"), "w").write(deck_ngspice(name, lev, T, Qc, eta, lam))
            subprocess.run([NG, "-b", name + ".cir"], cwd=wd, capture_output=True, timeout=1800)
            d = np.loadtxt(os.path.join(wd, name + ".dat"), skiprows=1)
            os.remove(os.path.join(wd, name + ".dat"))
            t, er, ei = d[:, 0], d[:, 1], d[:, 2]
        else:
            open(os.path.join(wd, name + ".net"), "w").write(deck_va(name, lev, T, Qc, eta, lam))
            env = dict(os.environ, VA_JIT="on")
            p = subprocess.run([VACLI, "sim", name + ".net", "--model", ".", "--tran", "--report", "tre,tim"],
                               cwd=wd, capture_output=True, text=True, env=env, timeout=3600)
            a = np.array(VA_RE.findall(p.stdout), float)
            if len(a) == 0:
                raise RuntimeError((p.stdout + p.stderr)[-400:])
            t, er, ei = a[:, 0], a[:, 1], a[:, 2]
        os.remove(os.path.join(wd, name + (".cir" if sim == "ngspice" else ".net")))
    except Exception as e:
        rec.update(failed=True, error=str(e)[-300:]); return rec
    rec["wall_s"] = time.time() - t0
    rec["points"] = int(len(t))
    rec.update(eye(t, (er ** 2 + ei ** 2) * 1e3, lev, M, T))
    return rec

def main():
    sim = sys.argv[sys.argv.index("--sim") + 1] if "--sim" in sys.argv else "ngspice"
    nj = int(sys.argv[sys.argv.index("--jobs") + 1]) if "--jobs" in sys.argv else 2
    assert sim in ("ngspice", "va")
    wd = os.path.join(HERE, f"work_{sim}"); shutil.rmtree(wd, ignore_errors=True); os.makedirs(wd)
    if sim == "ngspice":
        ov = os.path.join(wd, "ov"); os.makedirs(ov)     # compile without va-sim's headers around
        for f in ("mrm_tcmt.va", "mrm_driver.va"):
            shutil.copy(os.path.join(ROOT, f), ov)
            subprocess.run([OPENVAF, f], cwd=ov, capture_output=True, check=True)
            shutil.copy(os.path.join(ov, f.replace(".va", ".osdi")), wd)
        open(os.path.join(wd, ".spiceinit"), "w").write("set num_threads=1\n")
    else:
        for f in ("mrm_tcmt.va", "mrm_driver.va"):
            shutil.copy(os.path.join(ROOT, f), wd)
        for f in ("disciplines.vams", "constants.vams"):     # verilog-a-sim's own headers
            shutil.copy(os.path.join(ROOT, "vasim", f), wd)
    if "--quick" in sys.argv:
        jobs = [(sim, wd, f, b, q, e, fr) for (f, b) in FMTS for q in (22000, 6000) for e in (20, 50) for fr in (-0.1,)]
    else:
        jobs = [(sim, wd, f, b, q, e, fr) for (f, b) in FMTS for q in QC for e in ETA for fr in FRAC]
    t0 = time.time()
    with Pool(nj) as p:
        res = p.map(job, jobs, chunksize=2)
    out = os.path.join(HERE, f"sweep_q_eta_{sim}{'_quick' if '--quick' in sys.argv else ''}.json")
    json.dump(dict(meta=dict(sim=sim, runs=len(res), wall_total_s=time.time() - t0, jobs=nj,
                             assumptions=__doc__.split("Assumptions")[1].strip()), runs=res),
              open(out, "w"), indent=1, default=float)
    nf = sum(r.get("failed", False) for r in res)
    print(f"{sim}: {len(res)} runs, {nf} failed, {time.time()-t0:.0f} s -> {out}")
    for r in res:
        if r.get("failed"):
            print("  failed:", r["fmt"], r["baud"], r["Qc"], r["eta"], r["frac"], r["error"][:200]); break

if __name__ == "__main__":
    main()
