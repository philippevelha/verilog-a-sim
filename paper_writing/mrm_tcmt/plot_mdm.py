import numpy as np, matplotlib
matplotlib.use("Agg"); import matplotlib.pyplot as plt
C=["#2a78d6","#eb6834","#1baf7a","#eda100"]; INK="#0b0b0b"; INK2="#52514e"; GRID="#e6e5e0"
plt.rcParams.update({"font.size":9,"axes.edgecolor":INK2,"axes.labelcolor":INK,"xtick.color":INK2,
  "ytick.color":INK2,"axes.grid":True,"grid.color":GRID,"grid.linewidth":0.6,"axes.spines.top":False,
  "axes.spines.right":False,"lines.linewidth":1.6,"figure.facecolor":"#fcfcfb","axes.facecolor":"#fcfcfb"})
fig,ax=plt.subplots(1,2,figsize=(10,3.6))
a=ax[0]
for i,(f,l) in enumerate([("mdm_vrm05.dat","−0.5 V"),("mdm_vr0.dat","0 V"),("mdm_vr1.dat","1 V"),("mdm_vr2.dat","2 V")]):
    d=np.loadtxt(f,skiprows=1); a.plot(d[:,0],10*np.log10(d[:,1]),color=C[i],label=f"v = {l}")
a.axvline(1566.65,color=INK2,lw=0.8,ls=":"); a.text(1566.0,-12.3,"laser −50 pm →",color=INK2,fontsize=8)
a.set_xlim(1565.9,1567.7); a.set_xlabel("Wavelength (nm)"); a.set_ylabel("Through transmission (dB)")
a.legend(frameon=False,loc="lower right"); a.set_title("(a) mode 1, AIM microdisk fit: static response",loc="left",color=INK)
d=np.loadtxt("mdm_tran.dat",skiprows=1); t=d[:,0]; p=d[:,3]
a=ax[1]; UI=40e-12; tt=np.arange(0.5e-9,t[-1]-2*UI,0.2e-12); pp=np.interp(tt,t,p); ph=(tt-0.5e-9)%(2*UI)
for s in np.split(np.arange(len(tt)),np.where(np.diff(ph)<0)[0]+1): a.plot(ph[s]*1e12,pp[s]*1e3,color=C[0],lw=0.6,alpha=0.35)
a.set_xlim(0,80); a.set_xlabel("Time (ps)"); a.set_ylabel("Optical power (µW)")
a.set_title("(b) 25 Gb/s PRBS7, 2 Vpp, laser at −50 pm",loc="left",color=INK)
fig.tight_layout(); fig.savefig("mdm_mode1.png",dpi=150)
hi=np.percentile(pp,92); lo=np.percentile(pp,8); print("ER ~ %.2f dB, P1 %.0f uW, P0 %.0f uW"%(10*np.log10(hi/lo),hi*1e3,lo*1e3))
