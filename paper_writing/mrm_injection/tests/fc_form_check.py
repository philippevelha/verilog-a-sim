#!/usr/bin/env python3
"""Free-carrier filter: direct form (mrm_pin_direct.va, a ddt() of the exponential V2) against the
derivative-free W form (../mrm_pin.va), on the single pre-emphasis bit of pulse_ng.cir.

Runs verilog-a-sim on both forms; if pulse_ng.dat / pulse_direct_ng.dat (ngspice through OpenVAF,
decks pulse_ng.cir / pulse_direct_ng.cir) are present, compares them too. The quantity is the
free-carrier current X (mA, monitor node xfc), whose integral sets the resonance shift.

    python fc_form_check.py                 # VACLI = path to va-cli
"""
import os, re, shutil, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
VACLI = os.environ.get("VACLI", "va-cli")
PWL = "0 0.5 1n 0.5 1.04n 1.7 1.08n 1.7 1.12n 0.9 1.5n 0.9 1.54n -0.3 1.58n -0.3 1.62n 0.5 3n 0.5"
DECK = f"""* single pre-emphasis bit (as pulse_ng.cir)
Va a gnd PWL({PWL})
Vre inre gnd DC 1.0
Vim inim gnd DC 0
Vwl wl gnd DC 1318.812
Vh hp gnd DC 0
X1 a gnd hp gnd inre inim tre tim dre dim wl tdt mrm_pin
.tran 0.1p 3n
.end
"""
RE = re.compile(r"t=([-\d.e]+)s\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V\s+V\(X1\.xfc\)=([-\d.e]+) V")


def run_va(model):
    wd = os.path.join(HERE, "fc_work_" + os.path.splitext(model)[0]); os.makedirs(wd, exist_ok=True)
    for f in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(HERE, "..", "vasim", f), wd)
    shutil.copy(os.path.join(HERE, model if model != "mrm_pin.va" else os.path.join("..", model)),
                os.path.join(wd, "mrm_pin.va"))
    open(os.path.join(wd, "pulse.net"), "w").write(DECK)
    p = subprocess.run([VACLI, "sim", "pulse.net", "--model", ".", "--tran", "--report", "tre,tim,X1.xfc"],
                       cwd=wd, capture_output=True, text=True)
    a = np.array(RE.findall(p.stdout), float)
    shutil.rmtree(wd, ignore_errors=True)
    if len(a) == 0:
        sys.exit(p.stdout[-1500:] + p.stderr[-1500:])
    return dict(t=a[:, 0], X=a[:, 3], P=a[:, 1] ** 2 + a[:, 2] ** 2)


def load_ng(name):
    f = os.path.join(HERE, name)
    if not os.path.exists(f):
        return None
    d = np.loadtxt(f, skiprows=1)        # time ia xfc n12 n20 l3 th_re th_im
    return dict(t=d[:, 0], X=d[:, 2], P=d[:, 6] ** 2 + d[:, 7] ** 2)


def summary(name, r, ref, tg):
    X = np.interp(tg, r["t"], r["X"]); Xr = np.interp(tg, ref["t"], ref["X"])
    P = np.interp(tg, r["t"], r["P"]); Pr = np.interp(tg, ref["t"], ref["P"])
    fall = (tg > 1.58e-9) & (tg < 2.5e-9)               # after the falling edge
    q = np.trapezoid(X[fall], tg[fall]); qr = np.trapezoid(Xr[fall], tg[fall])
    print(f"{name:34s} pts {len(r['t']):6d}  max|dX| {np.max(np.abs(X - Xr)):9.3e} mA  "
          f"X-integral after fall {q:+.4e} (ref {qr:+.4e}, ratio {q / qr:7.4f})  "
          f"max|dP| {np.max(np.abs(P - Pr)) * 1e3:8.3f} uW")


tg = np.arange(0.5e-9, 3.0e-9, 0.05e-12)
w = run_va("mrm_pin.va"); d = run_va("mrm_pin_direct.va")
print("reference: verilog-a-sim, W form (../mrm_pin.va)")
summary("verilog-a-sim direct form", d, w, tg)
summary("verilog-a-sim W form (self)", w, w, tg)
for name, f in (("ngspice/OSDI W form", "pulse_ng.dat"), ("ngspice/OSDI direct form", "pulse_direct_ng.dat")):
    ng = load_ng(f)
    if ng is None:
        print(f"{name:34s} {f} not present (needs ngspice + OpenVAF)")
    else:
        summary(name, ng, w, tg)
