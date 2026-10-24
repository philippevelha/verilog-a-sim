import numpy as np, matplotlib, sys
sys.path.insert(0,'.'); from eyes import scope
matplotlib.use("Agg"); import matplotlib.pyplot as plt
sim=sys.argv[1] if len(sys.argv)>1 else "ngspice"
INK,INK2,GRID="#0b0b0b","#52514e","#e6e5e0"
plt.rcParams.update({"font.size":9,"axes.edgecolor":INK2,"xtick.color":INK2,"ytick.color":INK2,"figure.facecolor":"#fcfcfb","axes.facecolor":"#fcfcfb"})
cases=[("fig12_nrz","Fig. 12: NRZ, 2 Vpp at 0.7 V, no pre-emphasis"),("fig13a_pe40","Fig. 13(a): pre-emphasis 40 ps, 0.8 V depth"),("fig13b_pe80","Fig. 13(b): pre-emphasis 80 ps (optimum)")]
fig,ax=plt.subplots(2,3,figsize=(14,6.2),gridspec_kw=dict(height_ratios=[1,1.4]))
for j,(n,title) in enumerate(cases):
    z=np.load(f"{n}_{sim}.npz"); t,P,v,UI=z["t"],z["P"],z["v"],float(z["UI"])
    T=2*UI; t0=16*UI; tt=np.arange(t0,t[-1]-T,0.5e-12); pp=np.interp(tt,t,P); ph=((tt-t0+0.5*UI)%T)
    ts,Ps=scope(t,P); pps=np.interp(tt,ts,Ps)
    for s in np.split(np.arange(len(tt)),np.where(np.diff(ph)<0)[0]+1):
        ax[1,j].plot(ph[s]*1e12,pp[s],color="#9ec5f4",lw=0.5,alpha=0.35)
        ax[1,j].plot(ph[s]*1e12,pps[s],color="#c0392b",lw=0.5,alpha=0.3)
    ax[1,j].set_xlim(0,T*1e12); ax[1,j].set_xlabel("time (ps)"); ax[1,j].set_ylabel("through power (µW, 1 mW in)")
    ax[1,j].grid(color=GRID)
    if j==0: ax[1,j].text(0.02,0.03,"red: 20 GHz Bessel-Thomson (scope)  light blue: unfiltered",transform=ax[1,j].transAxes,fontsize=7.5,color=INK2)
    m=(t>t0)&(t<t0+16*UI)
    ax[0,j].plot((t[m]-t0)*1e9,v[m],color="#2a78d6",lw=1); ax[0,j].set_ylabel("V anode (V)"); ax[0,j].set_title(title,loc="left",color=INK,fontsize=9)
    ax[0,j].set_xlabel("time (ns)"); ax[0,j].grid(color=GRID)
fig.tight_layout(); fig.savefig(f"eyes_8G_{sim}.png",dpi=140)
