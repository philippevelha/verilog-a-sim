import numpy as np, matplotlib, json
matplotlib.use("Agg"); import matplotlib.pyplot as plt
import sys; sys.path.insert(0,"."); from eyes import scope
INK,INK2,GRID="#0b0b0b","#52514e","#e6e5e0"; B,O,G,R="#2a78d6","#eb6834","#1baf7a","#e34948"
plt.rcParams.update({"font.size":9,"axes.edgecolor":INK2,"xtick.color":INK2,"ytick.color":INK2,"figure.facecolor":"#fcfcfb","axes.facecolor":"#fcfcfb","axes.grid":True,"grid.color":GRID,"axes.spines.top":False,"axes.spines.right":False})
fig,ax=plt.subplots(1,3,figsize=(15,4.2))
# (a) DC
d=np.loadtxt("dc_iv.dat",skiprows=1); s=np.loadtxt("../ref_data/fig6_simulated.csv",delimiter=",",skiprows=1); m=np.loadtxt("../ref_data/fig6_measured.csv",delimiter=",",skiprows=1)
k=d[:,0]>0.55
ax[0].semilogy(d[k,0],d[k,1],color=B,lw=1.8,label="this model (Table I, V_T at 58 °C)")
ax[0].semilogy(s[:,0],s[:,1],color=R,lw=1.2,ls="--",label="[W] Fig. 6 simulation")
ax[0].semilogy(m[:,0],m[:,1],"s",color=INK2,ms=4,label="[W] Fig. 6 measurement")
import os
if os.path.exists("dc_T27.dat"):
    d27=np.loadtxt("dc_T27.dat",skiprows=1); ax[0].semilogy(d27[k,0],d27[k,1],color=O,lw=1,label="Table I at 27 °C (4× high)")
ax[0].set_xlim(0.55,2.05); ax[0].set_ylim(1e-5,12); ax[0].set_xlabel("V (V)"); ax[0].set_ylabel("I (mA)")
ax[0].legend(frameon=False,fontsize=8,loc="lower right"); ax[0].set_title("(a) DC I–V vs [W] Fig. 6",loc="left",color=INK)
# (b) Fig 10
for f,ref,c,lab in (("spec_0V.dat","fig10_meas_0V.csv",B,"0 V"),("spec_0p86V.dat","fig10_meas_0p86V.csv",O,"0.86 V")):
    dd=np.loadtxt(f,skiprows=1); r=np.loadtxt("../ref_data/"+ref,delimiter=",",skiprows=1)
    ax[1].plot(dd[:,0],10*np.log10(dd[:,1]),color=c,lw=1.8,label=f"model {lab}")
    ax[1].plot(r[:,0],r[:,1],".",color=c,ms=3,alpha=0.6,label=f"[W] Fig. 10 meas. {lab}")
ax[1].set_xlim(1318.5,1319.1); ax[1].set_xlabel("λ (nm)"); ax[1].set_ylabel("through (dB)"); ax[1].legend(frameon=False,fontsize=8,loc="lower right")
ax[1].set_title("(b) spectra vs [W] Fig. 10 (V_act from the shift only)",loc="left",color=INK)
# (c) Fig 7
p=np.loadtxt("../ref_data/fig7_simulated.csv",delimiter=",",skiprows=1); me=np.loadtxt("../ref_data/fig7_measured.csv",delimiter=",",skiprows=1)
tt=np.arange(0.02,0.98,0.001); pm=np.interp(tt,p[:,0],p[:,1])
for f,lab,c in (("f7_trap40_0.dat","40 ps trapezoid, no pads",B),("f7_sine_0.dat","sinusoid, no pads",G),("f7_sine_40.dat","sinusoid, 40 fF pads",O)):
    dd=np.loadtxt(f,skiprows=1); t=dd[:,0]*1e9; I=dd[:,2]; best=None
    for sh in np.arange(4.0,4.2,0.0005):
        e=np.sqrt(np.mean((np.interp(tt+sh,t,I)-pm)**2))
        if best is None or e<best[0]: best=(e,sh)
    ax[2].plot(tt,np.interp(tt+best[1],t,I),color=c,lw=1.4,label=f"model, {lab}")
ax[2].plot(p[:,0],p[:,1],color=R,ls="--",lw=1.2,label="[W] Fig. 7 simulation")
ax[2].set_xlabel("t (ns)"); ax[2].set_ylabel("I (mA)"); ax[2].legend(frameon=False,fontsize=7.5,loc="lower right",ncol=2)
ax[2].set_title("(c) 10 Gb/s clock vs [W] Fig. 7 (drive/pads unknown)",loc="left",color=INK)
fig.tight_layout(); fig.savefig("validation_static.png",dpi=140)
