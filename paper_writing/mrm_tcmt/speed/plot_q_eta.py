#!/usr/bin/env python3
"""Figures for the Q x efficiency sweep.

    python3 plot_q_eta.py                  # uses sweep_q_eta_va.json (+ ngspice for comparison)
    python3 plot_q_eta.py --sim ngspice

q_eta_map_<sim>.png   : best worst-eye height vs (Q_L, eta) per format, with the optimal
                        detuning written in each cell and the optimum framed
q_eta_compare.png     : verilog-a-sim vs ngspice, every run (if both JSON files exist)
q_eta_best_<sim>.csv  : the optimum per (format, Q_L, eta)
"""
import csv, json, os, sys
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.colors import LinearSegmentedColormap

HERE = os.path.dirname(os.path.abspath(__file__))
sim = sys.argv[sys.argv.index("--sim") + 1] if "--sim" in sys.argv else "va"
LABEL = {"va": "verilog-a-sim", "ngspice": "ngspice + OSDI"}
INK, INK2, GRID, SURF = "#0b0b0b", "#52514e", "#e6e5e0", "#fcfcfb"
BLUE = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"]
CMAP = LinearSegmentedColormap.from_list("seq_blue", BLUE)
plt.rcParams.update({"font.size": 9, "axes.edgecolor": INK2, "axes.labelcolor": INK, "xtick.color": INK2,
                     "ytick.color": INK2, "figure.facecolor": SURF, "axes.facecolor": SURF})

def load(s):
    f = os.path.join(HERE, f"sweep_q_eta_{s}.json")
    return json.load(open(f)) if os.path.exists(f) else None

D = load(sim)
if D is None:
    sys.exit(f"no sweep_q_eta_{sim}.json — run sweep_q_eta.py --sim {sim} first")
runs = [r for r in D["runs"] if not r.get("failed")]
FMTS = [("PAM4", 50e9), ("NRZ", 50e9), ("NRZ", 100e9)]
QCS = sorted({r["Qc"] for r in runs}, reverse=True)
ETAS = sorted({r["eta"] for r in runs})
QL = {q: 1 / (1 / 2e4 + 1 / q) for q in QCS}

def best(fmt, baud, q, e):
    rs = [r for r in runs if r["fmt"] == fmt and r["baud"] == baud and r["Qc"] == q and r["eta"] == e]
    return max(rs, key=lambda r: r["worst"]) if rs else None

# ------------------------------------------------------------------ map figure
fig, axes = plt.subplots(1, 3, figsize=(15, 5.4))
rows = []
for ax, (fmt, baud) in zip(axes, FMTS):
    Z = np.full((len(QCS), len(ETAS)), np.nan); F = np.full_like(Z, np.nan); N = np.full_like(Z, np.nan)
    for i, q in enumerate(QCS):
        for j, e in enumerate(ETAS):
            b = best(fmt, baud, q, e)
            if b is None:
                continue
            Z[i, j], F[i, j], N[i, j] = b["worst"], b["frac"], b["norm"]
            rows.append(dict(format=fmt, baud_GBd=baud / 1e9, Q_L=round(QL[q]), Qc=q, eta_pm_per_V=e,
                             best_detuning_FWHM=b["frac"], best_detuning_pm=round(b["det_pm"], 1),
                             worst_eye_uW=round(b["worst"], 2), OMA_uW=round(b["oma"], 2),
                             eye_over_ideal=round(b["norm"], 3), RLM=round(b["rlm"], 3)))
    vmax = np.nanmax(Z)
    Zm = np.ma.masked_less_equal(Z, 0)
    ax.imshow(np.zeros_like(Z), cmap=LinearSegmentedColormap.from_list("g", ["#e6e5e0", "#e6e5e0"]),
              aspect="auto", origin="upper")
    im = ax.imshow(Zm, cmap=CMAP, vmin=0, vmax=vmax, aspect="auto", origin="upper")
    for i in range(len(QCS)):
        for j in range(len(ETAS)):
            z = Z[i, j]
            if np.isnan(z):
                continue
            dark = z > 0 and z / vmax > 0.55
            col = "white" if dark else INK
            txt = f"{z:.0f}" if z > 0 else "closed"
            ax.text(j, i - 0.12, txt, ha="center", va="center", fontsize=9, color=col, fontweight="bold")
            ax.text(j, i + 0.22, f"{F[i, j]:+.2f}", ha="center", va="center", fontsize=7, color=col)
    ib, jb = np.unravel_index(np.nanargmax(Z), Z.shape)
    ax.add_patch(plt.Rectangle((jb - 0.5, ib - 0.5), 1, 1, fill=False, ec="#eb6834", lw=2.2))
    ax.set_xticks(range(len(ETAS))); ax.set_xticklabels(ETAS)
    ax.set_yticks(range(len(QCS))); ax.set_yticklabels([f"{QL[q]:.0f}" for q in QCS])
    ax.set_xlabel("Modulation efficiency η (pm/V at 0 V)"); ax.set_ylabel("Loaded Q (Q_i = 20 000)")
    ax.set_title(f"{fmt} {baud/1e9:.0f} GBd — best {Z[ib, jb]:.0f} µW at Q_L {QL[QCS[ib]]:.0f}, η {ETAS[jb]}",
                 loc="left", color=INK, fontsize=10)
    cb = fig.colorbar(im, ax=ax, fraction=0.046, pad=0.02); cb.set_label("worst inner eye height (µW)")
    for s in ax.spines.values():
        s.set_visible(False)
fig.suptitle(f"Q × efficiency sweep ({LABEL[sim]}): bold = worst eye height (µW, 1 mW laser), "
             f"small = optimal laser detuning (fraction of FWHM, − = blue side); orange frame = optimum",
             x=0.01, ha="left", fontsize=10, color=INK2)
fig.tight_layout(rect=(0, 0, 1, 0.95))
fig.savefig(os.path.join(HERE, f"q_eta_map_{sim}.png"), dpi=150)
with open(os.path.join(HERE, f"q_eta_best_{sim}.csv"), "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=list(rows[0])); w.writeheader(); w.writerows(rows)
print("wrote", f"q_eta_map_{sim}.png", f"q_eta_best_{sim}.csv")

# ------------------------------------------------------------------ comparison figure
A, B = load("va"), load("ngspice")
if A and B:
    key = lambda r: (r["fmt"], r["baud"], r["Qc"], r["eta"], r["frac"])
    a = {key(r): r for r in A["runs"] if not r.get("failed")}
    b = {key(r): r for r in B["runs"] if not r.get("failed")}
    ks = sorted(set(a) & set(b))
    x = np.array([b[k]["worst"] for k in ks]); y = np.array([a[k]["worst"] for k in ks])
    fig, ax = plt.subplots(1, 2, figsize=(11, 4.2))
    cols = {"PAM4": "#2a78d6", "NRZ": "#eb6834"}
    for fmt, baud in FMTS:
        m = np.array([k[0] == fmt and k[1] == baud for k in ks])
        ax[0].scatter(x[m], y[m], s=6, alpha=0.6, label=f"{fmt} {baud/1e9:.0f} GBd",
                      color=cols[fmt] if baud == 50e9 else "#1baf7a")
        ax[1].scatter(x[m], y[m] - x[m], s=6, alpha=0.6, color=cols[fmt] if baud == 50e9 else "#1baf7a")
    lim = [min(x.min(), y.min()), max(x.max(), y.max())]
    ax[0].plot(lim, lim, color=INK2, lw=0.8, ls="--")
    ax[0].set_xlabel("ngspice: worst eye (µW)"); ax[0].set_ylabel("verilog-a-sim: worst eye (µW)")
    ax[0].legend(frameon=False, fontsize=8); ax[0].grid(color=GRID)
    ax[0].set_title(f"(a) every run ({len(ks)})", loc="left", color=INK)
    ax[1].set_xlabel("ngspice: worst eye (µW)"); ax[1].set_ylabel("verilog-a-sim − ngspice (µW)")
    ax[1].grid(color=GRID)
    d = y - x
    ax[1].set_title(f"(b) difference: RMS {np.sqrt(np.mean(d**2)):.3f} µW, max |Δ| {np.abs(d).max():.3f} µW",
                    loc="left", color=INK)
    fig.tight_layout(); fig.savefig(os.path.join(HERE, "q_eta_compare.png"), dpi=150)
    # same optimum?
    same = 0; tot = 0
    for fmt, baud in FMTS:
        for q in QCS:
            for e in ETAS:
                ra = [a[k] for k in ks if k[:4] == (fmt, baud, q, e)]; rb = [b[k] for k in ks if k[:4] == (fmt, baud, q, e)]
                if ra and rb:
                    tot += 1; same += max(ra, key=lambda r: r["worst"])["frac"] == max(rb, key=lambda r: r["worst"])["frac"]
    wa = sum(r.get("wall_s", 0) for r in A["runs"]); wb = sum(r.get("wall_s", 0) for r in B["runs"])
    print(f"compare: {len(ks)} runs, RMS {np.sqrt(np.mean(d**2)):.4f} uW, max {np.abs(d).max():.4f} uW, "
          f"same optimal detuning in {same}/{tot} cells; summed run time va {wa:.0f} s vs ngspice {wb:.0f} s; "
          f"points va {np.mean([a[k]['points'] for k in ks]):.0f} vs ngspice {np.mean([b[k]['points'] for k in ks]):.0f} per run")
