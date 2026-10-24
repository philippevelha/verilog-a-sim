import numpy as np, matplotlib
matplotlib.use("Agg"); import matplotlib.pyplot as plt
C=["#2a78d6","#eb6834","#1baf7a"]; INK="#0b0b0b"; INK2="#52514e"; GRID="#e6e5e0"
plt.rcParams.update({"font.size":9,"axes.edgecolor":INK2,"axes.labelcolor":INK,"xtick.color":INK2,
  "ytick.color":INK2,"axes.grid":True,"grid.color":GRID,"grid.linewidth":0.6,"axes.spines.top":False,
  "axes.spines.right":False,"lines.linewidth":1.6,"figure.facecolor":"#fcfcfb","axes.facecolor":"#fcfcfb"})
fig,ax=plt.subplots(2,2,figsize=(10,7))
# (a) spectra vs reverse bias
a=ax[0,0]
for i,v in enumerate([0,1,2]):
    d=np.loadtxt(f"spec_vr{v}.dat",skiprows=1)
    a.plot((d[:,0]-1550)*1e3,10*np.log10(d[:,1]),color=C[i],label=f"V$_R$ = {v} V")
a.set_xlim(-150,150); a.set_ylim(-36,1); a.set_xlabel("λ$_L$ − 1550 nm (pm)"); a.set_ylabel("Through transmission (dB)")
a.axvline(-50,color=INK2,lw=0.8,ls=":"); a.text(-48,-33,"laser for (c,d)",color=INK2,fontsize=8)
a.legend(frameon=False,loc="lower left"); a.set_title("(a) Static response vs reverse bias",loc="left",color=INK)
# (b) heater tuning
a=ax[0,1]
for i,(f,l) in enumerate([("spec_vh0.dat","0 mW"),("spec_vh05.dat","0.25 mW"),("spec_vh1.dat","1 mW")]):
    d=np.loadtxt(f,skiprows=1)
    a.plot((d[:,0]-1550)*1e3,10*np.log10(d[:,1]),color=C[i],label=f"P$_h$ = {l}")
a.set_xlabel("λ$_L$ − 1550 nm (pm)"); a.set_ylabel("Through transmission (dB)"); a.set_ylim(-36,1)
a.legend(frameon=False,loc="lower right"); a.set_title("(b) Thermo-optic tuning (heater)",loc="left",color=INK)
# (c) transient
d=np.loadtxt("tran.dat",skiprows=1); t=d[:,0]; vk=d[:,2]; p=d[:,3]
a=ax[1,0]; m=(t>1e-9)&(t<3e-9)
a.plot(t[m]*1e9,p[m]*1e3,color=C[0],label="P$_{thru}$ (µW)")
a.set_xlabel("Time (ns)"); a.set_ylabel("Optical power (µW)")
a2=a.twinx(); a2.grid(False); a2.spines["right"].set_visible(True)
a2.plot(t[m]*1e9,vk[m],color=C[1],lw=1.0,alpha=0.85); a2.set_ylabel("Driver output V$_K$ (V)",color=INK)
a.set_title("(c) 25 Gb/s NRZ: drive (orange) and optical out (blue)",loc="left",color=INK)
# (d) eye
a=ax[1,1]; UI=40e-12; tt=np.arange(0.5e-9,t[-1]-2*UI,0.2e-12); pp=np.interp(tt,t,p)
ph=((tt-0.5e-9)%(2*UI)); idx=np.argsort(ph,kind="stable")
seg=np.where(np.diff(ph)<0)[0]+1
for s in np.split(np.arange(len(tt)),seg): a.plot(ph[s]*1e12,pp[s]*1e3,color=C[0],lw=0.6,alpha=0.35)
a.set_xlabel("Time (ps)"); a.set_ylabel("Optical power (µW)"); a.set_xlim(0,80)
a.set_title("(d) Optical eye, 25 Gb/s PRBS7, laser at −50 pm",loc="left",color=INK)
fig.tight_layout(); fig.savefig("mrm_results.png",dpi=150)
hi=np.percentile(pp,90); lo=np.percentile(pp,10); print("P1~%.1f uW P0~%.1f uW ER~%.2f dB"%(hi*1e3,lo*1e3,10*np.log10(hi/lo)))
