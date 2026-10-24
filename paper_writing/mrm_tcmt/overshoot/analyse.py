import numpy as np
c=2.99792458e8; Qi0=2e4; Qc=2.2e4; lam0=1550; eta=20; Cj0=20e-15; Vbi=0.8; m=0.5; rabs=0.15; FSR=10
def qdep(v): return Cj0*Vbi/(1-m)*(1-(1-v/Vbi)**(1-m))
def ss(lamL,Vj):
    dl=-eta*qdep(Vj)/Cj0; lr=lam0+1e-3*dl; w0=2*np.pi*c/(lr*1e-9)
    gi=w0/(2/(1/Qi0-rabs*1e-3*dl/lam0)); gc=w0/(2*Qc)
    d=FSR/np.pi*np.sin(np.pi*(lamL-lr)/FSR); dw=2*np.pi*c*d*1e-9/(lamL*lr*1e-18)
    return (gi-gc-1j*dw)/(gi+gc-1j*dw), dw, gi+gc, lr
for name,lamL in (("fast_m50",1549.95),("slow_m50",1549.95),("fast_m150",1549.85)):
    d=np.loadtxt(name+".dat",skiprows=1); t,vk,vai,er,ei=d.T; E=er+1j*ei; P=abs(E)**2
    Vj=vai-vk
    Eqs,_,_,_=ss(lamL,Vj); Pqs=abs(Eqs)**2       # quasi-static: Lorentzian at instantaneous bias
    print(f"\n== {name}: laser {1e3*(lamL-1550):+.0f} pm | V_K max {vk.max():.4f} V (no electrical overshoot: {vk.max()<=2.0+1e-9}), V_j min {Vj.min():.4f} V")
    for lab,t0,t1 in (("rise 0->2 V",1.0e-9,2.0e-9),("fall 2->0 V",2.0e-9,3.5e-9)):
        k=(t>t0)&(t<t1); Pf=P[k][-1]; Pi=P[(t<t0)][-1]
        over=(P[k].max()-Pf) if Pf>Pi else (Pf-P[k].min())
        Eend,dw,gam,lr=ss(lamL,Vj[k][-1])
        print(f"  {lab}: P {Pi*1e3:6.1f} -> {Pf*1e3:6.1f} uW, {'overshoot' if Pf>Pi else 'undershoot'} {over*1e3:5.1f} uW "
              f"({100*over/abs(Pf-Pi):4.0f}% of step); quasi-static model gives {max(0,(Pqs[k].max()-Pf) if Pf>Pi else (Pf-Pqs[k].min()))*1e3:4.1f} uW")
        # transient part of the field: E - E_final rotates at detuning and decays with photon lifetime
        tt=t[k]; R=E[k]-E[k][-1]; sel=(tt>t0+40e-12)&(abs(R)>1e-3*abs(R).max())&(tt<t0+300e-12)
        if sel.sum()>20:
            ph=np.unwrap(np.angle(R[sel])); f_fit=np.polyfit(tt[sel],ph,1)[0]/(2*np.pi)
            tau_fit=-1/np.polyfit(tt[sel],np.log(abs(R[sel])),1)[0]
            print(f"     ringing: fitted f = {abs(f_fit)/1e9:5.2f} GHz vs |w0-wL|/2pi = {abs(dw)/2/np.pi/1e9:5.2f} GHz;"
                  f"  decay tau = {tau_fit*1e12:5.1f} ps vs 1/gamma = {1e12/gam:5.1f} ps")
