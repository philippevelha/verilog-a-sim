import numpy as np
c=2.99792458e8; lam0=1550; Qi=2e4; Qc=2.2e4; eta=20; Cj0=20e-15; Vbi=0.8; m=0.5; rabs=0.15; FSR=10
dldT=75; Rth=5e3; Rh=1e3
def qdep(v): return Cj0*Vbi/(1-m)*(1-(1-v/Vbi)**(1-m))
def T(lam, vr=0.0, vh=0.0):
    dlc = -eta*qdep(-vr)/Cj0                     # pm
    dT  = vh**2/Rh*Rth
    lr  = lam0 + 1e-3*(dlc + dldT*dT)
    w0  = 2*np.pi*c/(lr*1e-9)
    Qie = 1/(1/Qi - rabs*1e-3*dlc/lam0)
    gi, gc = w0/(2*Qie), w0/(2*Qc)
    dl  = FSR/np.pi*np.sin(np.pi*(lam-lr)/FSR)
    dw  = 2*np.pi*c*dl*1e-9/(lam*lr*1e-18)
    return np.abs((1j*dw + gi - gc)/(1j*dw + gi + gc))**2, lr
for f,kw in [('spec_vr0.dat',dict(vr=0)),('spec_vr1.dat',dict(vr=1)),('spec_vr2.dat',dict(vr=2)),
             ('spec_vh0.dat',dict(vh=0)),('spec_vh05.dat',dict(vh=0.5)),('spec_vh1.dat',dict(vh=1))]:
    d=np.loadtxt(f,skiprows=1); lam=d[:,0]; p=d[:,1]
    ta,lr=T(lam,**kw)
    i=np.argmin(p)
    print(f"{f:15s} sim min {lam[i]:.4f} nm  T={10*np.log10(p[i]):6.2f} dB | analytic lres {lr:.4f}  "
          f"max|dT| = {np.max(np.abs(p-ta)):.2e}")
