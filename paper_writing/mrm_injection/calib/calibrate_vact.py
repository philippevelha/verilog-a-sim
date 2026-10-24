#!/usr/bin/env python3
"""Calibrate V_act (not given in Wang et al. 2016) from Fig. 10: the model must shift the
resonance by the paper's -90.7 pm at 0.86 V DC.  Uses the model's own DC carrier count, so
re-run this whenever the electrical parameters change.  Gamma and ng as in the model card."""
import numpy as np, subprocess, os, sys
from scipy.optimize import brentq
HERE=os.path.dirname(os.path.abspath(__file__))
NG=os.environ.get("NGSPICE","/opt/ngspice-new/bin/ngspice")
import sys
src=int(sys.argv[1]) if len(sys.argv)>1 else 1   # fc_src of the card being calibrated
lam0=1318.812; shift_pm=-90.7; neff=2.5188; ng=neff; Gamma=1.0     # Fig. 10 (digitised), model defaults
ce,ch=6.2e-22,6.0e-18; QE=1.602176634e-19; tau=1e-9
deck=f"""* DC 0.86 V carrier count
.model pin mrm_pin fc_src={src}
Va a 0 0.86
Vin_re in_re 0 1.0
Vin_im in_im 0 0
Vwl wl 0 {lam0}
Vh hp 0 0
N1 a 0 hp 0 in_re in_im th_re th_im dr_re dr_im wl tdt pin
.control
pre_osdi {os.path.join(HERE,'..','mrm_pin.osdi')}
option reltol=1e-5
op
print -i(Va) v(n1#xfc)
quit
.endc
.end
"""
open(os.path.join(HERE,"op086.cir"),"w").write(deck)
out=subprocess.run([NG,"-b","op086.cir"],cwd=HERE,capture_output=True,text=True).stdout
I=float([l for l in out.splitlines() if l.strip().startswith("-i(va)")][0].split("=")[1])
X=float([l for l in out.splitlines() if l.strip().startswith("v(n1#xfc)")][0].split("=")[1])  # mA
Ncar=tau*1e-3*X/QE
dn_req=shift_pm*1e-3/lam0*ng/Gamma
f=lambda N: -(ce*N+ch*N**0.8)-dn_req
N=brentq(f,1e12,1e20)
Vact=Ncar/N*1e12   # um^3
dalpha=10e-18*N
print(f"DC 0.86 V: I = {I*1e3:.4f} mA, filter state X = {X:.5f} mA -> free carriers {Ncar:.4e}")
print(f"required dn = {dn_req:.4e}  ->  dN = {N:.4e} cm^-3  (dalpha = {dalpha:.3f} cm^-1)")
print(f"V_act = {Vact:.3f} um^3")
