#!/usr/bin/env python3
"""Overlay of t2b (cold-resonance offsets + 0.6 mW self-heating) on the digitised [B] Fig. 8 curves."""
import json, numpy as np, matplotlib; matplotlib.use("Agg"); import matplotlib.pyplot as plt
d = json.load(open("t2b_eo_selfheat.json"))["final"]; col = dict(green="#2a9d4b", red="#c0392b", blue="#2a6fb0")
keys = {"8a": ["green", "red", "blue"], "8b": ["green", "red", "blue"]}
fig, ax = plt.subplots(1, 2, figsize=(11, 4))
for k, fg in enumerate(("8a", "8b")):
    for c, key in zip(d[f"{fg}_va"]["cases"], keys[fg]):
        ref = np.loadtxt(f"../ref_data/fig{fg}_{key}.csv", delimiter=",", skiprows=1)
        ax[k].plot(ref[:, 0], ref[:, 1], "o", ms=3, color=col[key], alpha=0.6)
        ax[k].plot(np.array(c["f"]) / 1e9, c["dB"], "-", color=col[key], label=f"{c['case']} (va)")
        cn = [x for x in d[f"{fg}_ngspice"]["cases"] if x["case"] == c["case"]][0]
        ax[k].plot(np.array(cn["f"]) / 1e9, cn["dB"], "--", color="k", lw=0.7)
    ax[k].set_title(f"[B] Fig. {fg[0]}({fg[1]}): dots digitised, lines model (dashed = ngspice)", fontsize=9)
    ax[k].set_xlabel("frequency (GHz)"); ax[k].set_ylabel("normalised EO response (dB)"); ax[k].set_ylim(-8, 4); ax[k].grid(alpha=0.3); ax[k].legend(fontsize=8)
fig.tight_layout(); fig.savefig("t2b_fig8_overlay.png", dpi=130)
