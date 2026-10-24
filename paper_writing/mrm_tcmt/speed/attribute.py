import numpy as np, subprocess, os, sys
sys.argv=['x']; exec(open('sweep.py').read().split('if __name__')[0])
c=2.99792458e8
def qs(Vj,lamL,Qc,Qi=2e4,eta=20,Cj0=20e-15,Vbi=0.8,m=0.5,rabs=0.15):
    q=Cj0*Vbi/(1-m)*(1-(1-np.minimum(Vj,0.39)/Vbi)**(1-m)); dl=-eta*q/Cj0; lr=1550+1e-3*dl
    w0=2*np.pi*c/(lr*1e-9); gi=w0/(2/(1/Qi-rabs*1e-3*dl/1550)); gc=w0/(2*Qc)
    d=10/np.pi*np.sin(np.pi*(lamL-lr)/10); dw=2*np.pi*c*d*1e-9/(lamL*lr*1e-18)
    return np.abs((gi-gc-1j*dw)/(gi+gc-1j*dw))**2*1e3
def eye(t,P,lev,M,T):
    sym=np.rint(lev*(M-1)).astype(int); best=None
    for ph in np.arange(0,3*T,T/100):
        idx=np.arange(16,len(lev)-3); s=np.interp(idx*T+ph,t,P); k=sym[idx]
        e=[s[k==j+1].min()-s[k==j].max() for j in range(M-1)]
        means=[s[k==j].mean() for j in range(M)]
        if best is None or min(e)>best[0]: best=(min(e),(means[-1]-means[0])/(M-1))
    return best[0], best[0]/best[1]
for fmt,baud,Qc,det in (("NRZ",50e9,22000,-52),("NRZ",100e9,22000,-133),("PAM4",50e9,22000,-74),("PAM4",50e9,8000,-41),("NRZ",100e9,4000,-163)):
    lev,M=symbols(fmt); T=1/baud
    name=f"attr_{fmt}_{baud/1e9:.0f}_{Qc}"
    r=run(fmt,baud,Qc,det,tag="attr")   # writes deck; re-run with junction voltage
    deck=open(f"work/attr_{fmt}_{baud/1e9:.0f}_{Qc:.0f}_{('m%.0f'%-det)}.cir").read().replace("wrdata","wrdata").replace("v(th_re) v(th_im)","v(th_re) v(th_im) v(n1#ai) v(kk)").replace(".dat",".full.dat")
    fn=f"work/{name}.cir"; open(fn,"w").write(deck); subprocess.run([NG,"-b",os.path.basename(fn)],cwd="work",capture_output=True)
    datf=[f for f in os.listdir("work") if f.startswith(f"attr_{fmt}_{baud/1e9:.0f}_{Qc:.0f}_") and f.endswith(".full.dat")][0]
    d=np.loadtxt("work/"+datf,skiprows=1); t=d[:,0]; P=(d[:,1]**2+d[:,2]**2)*1e3; Vj=d[:,3]-d[:,4]
    Pq=qs(Vj,1550+det*1e-3,Qc)
    # ideal electrical: quasi-static ring on the driver's *ideal* target levels (no driver/RC limits)
    Vt=-2*np.interp(t,np.arange(len(lev))*T+T*0.5,lev,left=lev[0])  # step-like target sampled
    e_full=eye(t,P,lev,M,T); e_qs=eye(t,Pq,lev,M,T)
    print(f"{fmt:4s} {baud/1e9:4.0f} GBd Q_L {1/(1/2e4+1/Qc):6.0f} det {det:+4d} pm | full model: worst eye {e_full[0]:6.1f} uW ({100*e_full[1]:5.1f} %)"
          f" | same junction voltage, ring quasi-static: {e_qs[0]:6.1f} uW ({100*e_qs[1]:5.1f} %)")
