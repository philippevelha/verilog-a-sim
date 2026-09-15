#!/usr/bin/env python3
"""Figures for the traffic example (Bellemans, De Schutter, De Moor 2002, Section 5), from
va-cli runs of the decks in `circuits/`:

  docs/examples/traffic_fundamental_diagram.svg   the paper's Fig. 5/7 — circuits/fundamental_diagram.net
  docs/examples/traffic_no_control.svg            the paper's Fig. 12 — circuits/motorway_ramp.net
  docs/examples/traffic_control.svg               no control / ALINEA / optimised window —
                                                  circuits/motorway_ramp{,_alinea,_mpc}.net

Usage, from the repository root (runs va-cli; ~10 s):
    python docs/examples/traffic_figures.py
"""
import re
import subprocess
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "docs" / "examples"
INK, MUTED, GRID, SURFACE = "#0b0b0b", "#52514e", "#e6e5e1", "#fcfcfb"
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100"]   # validated categorical slots 1–4


def run(deck, analysis, report):
    args = ["cargo", "run", "-q", "--release", "-p", "va-cli", "--", "sim",
            str(ROOT / "circuits" / deck), "--model", str(ROOT / "models")]
    if analysis:
        args.append(analysis)
    args += ["--report", report]
    return subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=True).stdout


def parse_tran(text):
    rows = []
    for line in text.splitlines():
        if line.startswith("  t="):
            t = float(re.search(r"t=([0-9.e+-]+)s", line).group(1))
            v = {k: float(x) for k, x in re.findall(r"([A-Za-z]+\([A-Za-z0-9.]+\))=([0-9.e+-]+)", line)}
            rows.append((t / 3600.0, v))
    return rows


def style(ax):
    ax.set_facecolor(SURFACE)
    ax.grid(True, color=GRID, linewidth=0.8)
    ax.tick_params(colors=MUTED, labelsize=9)
    for s in ax.spines.values():
        s.set_color(GRID)


def fundamental_diagram():
    text = run("fundamental_diagram.net", None, "c,q,v")
    c, q, v = [], [], []
    for line in text.splitlines():
        m = re.search(r"Vc=([0-9.e+-]+): V\(c\)=([0-9.e+-]+) V V\(q\)=([0-9.e+-]+) V V\(v\)=([0-9.e+-]+)", line)
        if m:
            c.append(float(m.group(2))); q.append(float(m.group(3))); v.append(float(m.group(4)))
    fig, (a1, a2) = plt.subplots(1, 2, figsize=(9, 3.6), facecolor=SURFACE)
    for a in (a1, a2):
        style(a)
    a1.plot(c, q, color=SERIES[0], linewidth=2)
    a1.axvline(33.5, color=MUTED, linewidth=1, linestyle=(0, (2, 3)))
    a1.text(35, 300, "C_cr = 33.5", color=MUTED, fontsize=9)
    a1.axvline(180, color=MUTED, linewidth=1, linestyle=(0, (2, 3)))
    a1.text(150, 300, "C_jam", color=MUTED, fontsize=9)
    qmax = max(q)
    a1.annotate(f"{qmax:.0f} veh/h", (c[q.index(qmax)], qmax), xytext=(8, -4),
                textcoords="offset points", color=INK, fontsize=9)
    a1.set_xlabel("density  (veh/km/lane)", color=INK); a1.set_ylabel("flow, 2 lanes  (veh/h)", color=INK)
    a1.set_title("Fundamental diagram — circuits/fundamental_diagram.net", loc="left", fontsize=10, color=INK)
    a2.plot(c, v, color=SERIES[1], linewidth=2)
    a2.axvline(33.5, color=MUTED, linewidth=1, linestyle=(0, (2, 3)))
    a2.set_xlabel("density  (veh/km/lane)", color=INK); a2.set_ylabel("speed  (km/h)", color=INK)
    a2.set_title("Speed–density law, eq. (5)", loc="left", fontsize=10, color=INK)
    fig.tight_layout()
    fig.savefig(OUT / "traffic_fundamental_diagram.svg", format="svg")
    print("wrote traffic_fundamental_diagram.svg")


def fig12(rows):
    t = [r[0] for r in rows]
    fig, ax = plt.subplots(2, 2, figsize=(9.5, 6.4), facecolor=SURFACE)
    for a in ax.flat:
        style(a)
    ax[0, 0].plot(t, [r[1]["Qlen(wm)"] for r in rows], color=SERIES[0], linewidth=2, label="mainline")
    ax[0, 0].plot(t, [r[1]["Qlen(wr)"] for r in rows], color=SERIES[1], linewidth=2, label="on-ramp")
    ax[0, 0].set_ylabel("queue length  (veh)", color=INK)
    ax[0, 0].legend(frameon=False, fontsize=9, labelcolor=INK)
    for j in range(4):
        ax[0, 1].plot(t, [r[1][f"Spd(v{j+1})"] for r in rows], color=SERIES[j], linewidth=1.8, label=f"section {j+1}")
        ax[1, 0].plot(t, [r[1][f"Dens(c{j+1})"] for r in rows], color=SERIES[j], linewidth=1.8)
        ax[1, 1].plot(t, [r[1][f"Dens(c{j+1})"] * max(r[1][f"Spd(v{j+1})"], 0.0) * 2 for r in rows],
                      color=SERIES[j], linewidth=1.8)
    ax[0, 1].set_ylabel("speed  (km/h)", color=INK); ax[0, 1].legend(frameon=False, fontsize=9, labelcolor=INK, ncol=2)
    ax[1, 0].set_ylabel("density  (veh/km/lane)", color=INK); ax[1, 0].set_xlabel("time  (h)", color=INK)
    ax[1, 1].set_ylabel("flow  (veh/h)", color=INK); ax[1, 1].set_xlabel("time  (h)", color=INK)
    ax[1, 0].axhline(33.5, color=MUTED, linewidth=1, linestyle=(0, (2, 3)))
    ax[1, 0].text(2.6, 34.5, "C_cr", color=MUTED, fontsize=8.5)
    fig.suptitle("No control — circuits/motorway_ramp.net (the paper's Fig. 12)", x=0.02, ha="left",
                 fontsize=10.5, color=INK)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    fig.savefig(OUT / "traffic_no_control.svg", format="svg")
    print("wrote traffic_no_control.svg")


def control(runs):
    fig, ax = plt.subplots(2, 2, figsize=(9.5, 6.4), facecolor=SURFACE)
    for a in ax.flat:
        style(a)
    for k, (label, rows) in enumerate(runs.items()):
        t = [r[0] for r in rows]
        ax[0, 0].plot(t, [r[1]["Qlen(wm)"] for r in rows], color=SERIES[k], linewidth=2, label=label)
        ax[0, 1].plot(t, [r[1]["Qlen(wr)"] for r in rows], color=SERIES[k], linewidth=2)
        ax[1, 0].plot(t, [r[1].get("V(rate)", r[1].get("V(one)", 1.0)) for r in rows], color=SERIES[k], linewidth=2)
        ax[1, 1].plot(t, [r[1]["Dens(c3)"] for r in rows], color=SERIES[k], linewidth=2)
        tts = rows[-1][1]["V(tts)"]
        ax[0, 0].annotate(f"{label}: TTS {tts:.0f} veh·h", (0.02, 0.92 - 0.09 * k), xycoords="axes fraction",
                          color=INK, fontsize=9)
    ax[0, 1].axhline(100, color=MUTED, linewidth=1, linestyle=(0, (2, 3)))
    ax[0, 1].text(2.4, 104, "paper's limit", color=MUTED, fontsize=8.5)
    ax[0, 0].set_ylabel("mainline queue  (veh)", color=INK)
    ax[0, 1].set_ylabel("on-ramp queue  (veh)", color=INK)
    ax[1, 0].set_ylabel("metering rate", color=INK); ax[1, 0].set_xlabel("time  (h)", color=INK)
    ax[1, 0].set_ylim(0, 1.05)
    ax[1, 1].set_ylabel("section 3 density  (veh/km/lane)", color=INK); ax[1, 1].set_xlabel("time  (h)", color=INK)
    ax[0, 0].legend(frameon=False, fontsize=9, labelcolor=INK, loc="center right")
    fig.suptitle("Ramp metering — no control, ALINEA feedback, and the optimised open-loop window",
                 x=0.02, ha="left", fontsize=10.5, color=INK)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    fig.savefig(OUT / "traffic_control.svg", format="svg")
    print("wrote traffic_control.svg")


def main():
    fundamental_diagram()
    rep = "c1,c2,c3,c4,v1,v2,v3,v4,wm,wr,tts"
    nc = parse_tran(run("motorway_ramp.net", "--tran", rep + ",one"))
    fig12(nc)
    al = parse_tran(run("motorway_ramp_alinea.net", "--tran", rep + ",rate"))
    mp = parse_tran(run("motorway_ramp_mpc.net", "--tran", rep + ",rate"))
    control({"no control": nc, "ALINEA": al, "open-loop optimum": mp})


if __name__ == "__main__":
    main()
