#!/usr/bin/env python3
"""Eye diagrams from work/t5_*_va.npz (verilog-a-sim) with ngspice overlaid in the corner panel title."""
import json, numpy as np, matplotlib; matplotlib.use("Agg"); import matplotlib.pyplot as plt
res = {(r["case"], r["sim"]): r for r in json.load(open("t5_eyes.json"))}
cases = ["single_PAM4_80G", "dual_PAM4_80G", "single_PAM4_106G", "dual_PAM4_106G", "dual_PAM8_212G"]
fig, ax = plt.subplots(2, 3, figsize=(13, 7)); ax = ax.ravel()
for k, c in enumerate(cases):
    d = np.load(f"work/t5_{c}_va.npz"); t, P, T = d["t"], d["P"], float(d["T"])
    ph = res[(c, "va")]["eye"]["ph"]; tg = np.arange(16 * T, t[-1] - 2 * T, T / 60)
    Pg = np.interp(tg, t, P); x = ((tg - ph - 16 * T) % (2 * T)) / T - 1
    br = np.where(np.diff(x) < 0)[0] + 1
    for seg in np.split(np.c_[x, Pg * 1e3], br): ax[k].plot(seg[:, 0], seg[:, 1], color="#2a6fb0", lw=0.4, alpha=0.35)
    e, en = res[(c, "va")]["eye"], res[(c, "ngspice")]["eye"]
    ax[k].set_title(f"{c.replace('_', ' ')}\nworst eye {e['worst']*1e3:.1f} µW (va) / {en['worst']*1e3:.1f} µW (ng), RLM {e['rlm']:.2f}", fontsize=9)
    ax[k].set_xlabel("time (UI)"); ax[k].set_ylabel("through power (µW)")
ax[5].axis("off")
rows = [[c.replace("_", " "), f"{res[(c,'ngspice')]['time']['wall_s']:.2f}", f"{res[(c,'va')]['time']['wall_s']:.2f}",
         f"{res[(c,'ngspice')]['time']['rss_mb']:.1f}", f"{res[(c,'va')]['time']['rss_mb']:.1f}", f"{res[(c,'va')]['ng_vs_va_rms_rel']:.0e}"] for c in cases]
tb = ax[5].table(rows, colLabels=["case", "ng s", "va s", "ng MB", "va MB", "ΔP rms"], loc="center", fontsize=8); tb.auto_set_font_size(False); tb.set_fontsize(8); tb.scale(1, 1.4)
fig.suptitle("Bao et al. equivalent-circuit MRM, large-signal model: eyes (1 mW laser, 2.5 Vpp, no FFE)", fontsize=11)
fig.tight_layout(); fig.savefig("t5_eyes.png", dpi=130)
