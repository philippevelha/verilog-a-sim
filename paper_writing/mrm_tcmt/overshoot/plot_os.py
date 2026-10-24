import numpy as np, matplotlib
matplotlib.use("Agg"); import matplotlib.pyplot as plt
exec(open("analyse.py").read().split("for name,lamL")[0])
C=["#2a78d6","#eb6834","#1baf7a","#52514e"]; INK="#0b0b0b"; INK2="#52514e"; GRID="#e6e5e0"
plt.rcParams.update({"font.size":9,"axes.edgecolor":INK2,"axes.labelcolor":INK,"xtick.color":INK2,"ytick.color":INK2,
 "axes.grid":True,"grid.color":GRID,"grid.linewidth":0.6,"axes.spines.top":False,"axes.spines.right":False,
 "lines.linewidth":1.6,"figure.facecolor":"#fcfcfb","axes.facecolor":"#fcfcfb"})
fig,ax=plt.subplots(1,2,figsize=(10,3.8))
for a,(name,lamL,title) in zip(ax,(("fast_m50",1549.95,"(a) laser −50 pm, step 0→2 V→0"),("fast_m150",1549.85,"(b) laser −150 pm: larger detuning, faster ringing"))):
    d=np.loadtxt(name+".dat",skiprows=1); t,vk,vai,er,ei=d.T; P=er**2+ei**2; Pqs=abs(ss(lamL,vai-vk)[0])**2
    m=(t>0.95e-9)&(t<2.35e-9); x=(t[m]-1e-9)*1e12
    a.plot(x,Pqs[m]*1e3,color=C[3],lw=1.2,ls="--",label="quasi-static (Lorentzian at instantaneous V)")
    a.plot(x,P[m]*1e3,color=C[0],label="TCMT, 35 GHz driver")
    if name=="fast_m50":
        ds=np.loadtxt("slow_m50.dat",skiprows=1); ms=(ds[:,0]>0.95e-9)&(ds[:,0]<2.35e-9)
        a.plot((ds[ms,0]-1e-9)*1e12,(ds[ms,3]**2+ds[ms,4]**2)*1e3,color=C[2],label="TCMT, 3 GHz driver")
    a2=a.twinx(); a2.grid(False); a2.spines["right"].set_visible(True)
    a2.plot(x,vk[m],color=C[1],lw=1.0,alpha=0.8); a2.set_ylabel("Driver output V$_K$ (V)"); a2.set_ylim(-0.2,6)
    a.set_xlabel("Time from first edge (ps)"); a.set_ylabel("Through power (µW)")
    a.set_title(title,loc="left",color=INK)
ax[0].legend(frameon=False,fontsize=8,loc="upper right")
ax[0].text(620,330,"V$_K$ (orange, right axis):\nno overshoot",fontsize=8,color=INK2)
ax[1].annotate("period 1/22.2 GHz = 45 ps",xy=(48,880),xytext=(250,890),fontsize=8,color=INK2,arrowprops=dict(arrowstyle="->",color=INK2,lw=0.8))
fig.tight_layout(); fig.savefig("overshoot.png",dpi=150)
