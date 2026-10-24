import re,numpy as np
def va(f,keys):
    txt=open(f).read()
    if keys=='dc':
        a=np.array(re.findall(r"Vwl=([\d.]+): V\(tre\)=([-\d.e]+) V V\(tim\)=([-\d.e]+)",txt),float)
    else:
        a=np.array(re.findall(r"t=([-\d.e]+)s\s+V\(kk\)=([-\d.e]+) V\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V",txt),float)
    return a
def ng(f): return np.loadtxt(f,skiprows=1)
c=2.99792458e8
def analytic(lam):
    w0=2*np.pi*c/1550e-9; gi=w0/(2*2e4); gc=w0/(2*2.2e4)
    dl=10/np.pi*np.sin(np.pi*(lam-1550)/10); dw=2*np.pi*c*dl*1e-9/(lam*1550*1e-18)
    t=(gi-gc-1j*dw)/(gi+gc-1j*dw)   # thru field, s_in = 1, model sign convention
    return t
print("=== DC sweep, 2001 points, V_R = 0 (exact answer known in closed form) ===")
A=analytic
for name,d in [("ngspice default",ng("ng_dc_def.dat")),("ngspice tight",ng("ng_dc_tight.dat")),("va-sim default",va("vasim/va_dc_def.out","dc"))]:
    lam=d[:,0]; E=d[:,1]+1j*d[:,2]; ref=A(lam)
    print(f"{name:16s} n={len(lam)}  max|E-E_exact| = {np.max(np.abs(E-ref)):.2e}  max|P-P_exact| = {np.max(np.abs(abs(E)**2-abs(ref)**2)):.2e}")
d1=va("vasim/va_dc_def.out","dc"); d2=ng("ng_dc_def.dat"); n=min(len(d1),len(d2))
print(f"ngspice default vs va-sim default: max|dE| = {np.max(np.abs((d1[:n,1]+1j*d1[:n,2])-(d2[:n,1]+1j*d2[:n,2]))):.2e}")

print("\n=== Transient, 25 Gb/s PRBS7, 254 bits (no closed form) ===")
runs={"ngspice default":ng("ng_tran_def.dat"),"ngspice tight":ng("ng_tran_tight.dat"),
      "va-sim default":va("vasim/va_tran_def.out","tran"),"va-sim tight":va("vasim/va_tran_tight.out","tran")}
tg=np.arange(0.1e-9,10.15e-9,0.05e-12)
R={}
for k,d in runs.items():
    t=d[:,0]; R[k]=dict(n=len(t),kk=np.interp(tg,t,d[:,1]),P=np.interp(tg,t,d[:,2]**2+d[:,3]**2),dtmin=np.min(np.diff(t)[np.diff(t)>0]),dtmed=np.median(np.diff(t)))
for k,r in R.items(): print(f"{k:16s} accepted points {r['n']:7d}   median dt {r['dtmed']*1e15:7.1f} fs   min dt {r['dtmin']*1e15:7.2f} fs")
ref=R["ngspice tight"]; Pm=ref["P"].mean()
def cmp(a,b):
    dP=R[a]["P"]-R[b]["P"]; dV=R[a]["kk"]-R[b]["kk"]
    return np.sqrt(np.mean(dP**2))/Pm, np.max(np.abs(dP))/Pm, np.sqrt(np.mean(dV**2)), np.max(np.abs(dV))
print(f"\n{'pair':40s} {'RMS dP/Pm':>10s} {'max dP/Pm':>10s} {'RMS dV_K':>10s} {'max dV_K':>10s}")
for a,b in [("ngspice tight","va-sim tight"),("ngspice default","va-sim default"),("ngspice default","ngspice tight"),("va-sim default","ngspice tight"),("va-sim default","va-sim tight")]:
    r=cmp(a,b); print(f"{a+' vs '+b:40s} {r[0]:10.2e} {r[1]:10.2e} {r[2]:10.2e} {r[3]:10.2e}")
# where are the default-run differences?
dP=np.abs(R["ngspice default"]["P"]-R["va-sim default"]["P"])
slope=np.abs(np.gradient(ref["P"],tg))
edge=slope>0.05*slope.max()
print(f"\nngspice-default vs va-sim-default: fraction of time on edges {edge.mean():.2f}, "
      f"share of squared error on edges {np.sum(dP[edge]**2)/np.sum(dP**2):.2f}")
print(f"  RMS dP/Pm on edges {np.sqrt(np.mean(dP[edge]**2))/Pm:.2e}, on flats {np.sqrt(np.mean(dP[~edge]**2))/Pm:.2e}")
# timing shift? cross-correlate around edges
for k in ("ngspice default","va-sim default"):
    e=R[k]["P"]-ref["P"]; print(f"  {k} vs tight ref: RMS dP/Pm {np.sqrt(np.mean(e**2))/Pm:.2e}")
