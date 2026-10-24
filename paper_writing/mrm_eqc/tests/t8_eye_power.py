#!/usr/bin/env python3
"""t8: the best lower-Q ring of t7 (tau_e = 15 ps, Q_L 5500, FFE pre -0.15 / post -0.10, 0.5-3 V,
laser 1.0 half-linewidth from the cold best-OMA point): its 212 Gb/s PAM8 eye, and the eye
opening against laser power.

  --eye    the t7b best case exactly (1 mW, self_heat = 0, 170 PAM8 symbols, 154 folded periods)
           -> t8_eye.npz, t8_eye.png
  --power  laser 0.1-8 mW with the same ring, taps and window, self_heat = 0 and 1. With
           self-heating the laser position is re-found at each power (coarse 20 pm scan over the
           expected thermal shift, then 5 pm around the best), as a heater/laser tuning loop
           would. The drives start at their mean level for 4 symbols, so the DC operating point -
           which sets the ring temperature for the whole pattern, tau_th = 1 us >> 2.4 ns - is at
           the average drive, not the first symbol's. The 4 lead-in symbols are inside the 16 the
           eye metric skips.  -> t8_power.json, t8_power.png
verilog-a-sim only (VA_JIT=on).  VACLI = path to va-cli.
"""
import json, os, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np
import t5_eyes as t5
import t6_pam8_sweep as t6
import t7_lowq_sweep as t7

TAU_E, PRE, POST, DET_HW = 15.0, -0.15, -0.10, 1.0
LEAD = 4


def case(lead=0):
    c = t6.pam8_case(t7.VLO, t7.VHI, PRE, POST)
    c["card"] = dict(c["card"]); c["card"].update(tau_e=TAU_E * 1e-12)
    if lead:
        m1, m2 = float(np.mean(c["v1"])), float(np.mean(c["v2"]))
        c["v1"] = [m1] * lead + list(c["v1"]); c["v2"] = [m2] * lead + list(c["v2"])
        c["sym"] = [0] * lead + list(c["sym"])
    return c


def laser_cold(c):
    cc = dict(c); cc["card"] = dict(c["card"], self_heat=0)
    lam0, _ = t5.best_laser(cc["card"], cc["fsr"], cc["lo"], cc["hi"], 1e-6)
    return lam0 + DET_HW * t7.ring(TAU_E)["hwhm_pm"] * 1e-3


def hot_root_K(c, lam, P):
    """Hottest static self-heating solution at the mean drive (the python twin of the model's
    heating law, as t5.steady_T), as a ring temperature rise in K. Where the ring is thermally
    bistable (three solutions) this is the hot branch, the one a laser swept in from the blue
    side stays on; elsewhere it is the only solution. Used as a .nodeset for V(tdt): from a cold
    start the DC solve can land on the cold branch instead (seen at 2 and 4 mW)."""
    card = c["card"]; cl = 2.99792458e8
    v1, v2 = float(np.mean(c["v1"])), float(np.mean(c["v2"]))
    lr = card["lam0"] + 1e-3 * (card["s1"] * (card["c1"] * v1 + card["c2"] * v1 ** 2)
                                + card["s2"] * (card["c1"] * v2 + card["c2"] * v2 ** 2))
    w1 = card["s1"] / (card["s1"] + card["s2"]); w2 = 1 - w1
    f = lambda v: card["tl0"] + card["tl1"] * v + card["tl2"] * v * v
    tl = 1e-12 / (1 / card["tl0"] + w1 * (1 / f(v1) - 1 / card["tl0"]) + w2 * (1 / f(v2) - 1 / card["tl0"]))
    g = 1 / tl + 1 / card["tau_e"]
    d = np.linspace(0, 0.215 * P * 1.05 + 1e-6, 200001)
    D = 2 * np.pi * cl * (1 / (lam * 1e-9) - 1 / ((lr + d) * 1e-9))
    F = 0.215 * P * g * g / (D * D + g * g) - d
    roots = d[np.where(np.diff(np.sign(F)) != 0)[0]]
    dlam_dT = card.get("dlam_dT", 0.08)
    return float(roots.max()) / dlam_dT if len(roots) else 0.0, len(roots)


def run(c, lam, P, sh, tag):
    cc = dict(c); cc["card"] = dict(c["card"], self_heat=sh)
    if not sh:
        return t6.run_point(tag, cc, lam, P)
    TK, nroots = hot_root_K(c, lam, P)
    r = t6.run_point(tag, cc, lam, P, (f".nodeset V(tdt)={TK:.6f}",))
    r["static_roots"] = nroots
    return r


# ------------------------------------------------------------------------------------- eye
def eye_figure():
    import re, matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    c = case()
    lam = laser_cold(c)
    T = 1 / c["baud"]
    # re-run, keeping the waveform (run_point only returns the metric)
    r = run(c, lam, 1.0, 0, "t8_eye")
    w = os.path.join(t6.HERE, "work", "t6", "t8_eye")
    out = __import__("subprocess").run([t6.VACLI, "sim", "deck.net", "--model", ".", "--tran", "--threads", "1",
                                        "--report", "X1.pmon"], cwd=w, capture_output=True, text=True,
                                       env=dict(os.environ, VA_JIT="on")).stdout
    a = np.array(re.findall(r"t=" + t6.NUM + r"s\s+V\(X1\.pmon\)=" + t6.NUM, out), float)
    t, P = a[:, 0], a[:, 1] * 1e3                     # uW
    e = r["eye"]; ph = e["ph"]
    np.savez("t8_eye.npz", t=t, P_uW=P, sym=c["sym"], T=T, ph=ph, laser_nm=lam)
    skip = 16; n = len(c["sym"])
    # fold: 2 UI windows centred on the sampling instant
    fig, ax = plt.subplots(figsize=(6.4, 4.0), dpi=200)
    fig.patch.set_facecolor("#fcfcfb"); ax.set_facecolor("#fcfcfb")
    tt = np.linspace(-T, T, 161)
    for k in range(skip, n - 2):
        ts = k * T + ph
        ax.plot(tt * 1e12, np.interp(ts + tt, t, P), color="#2a78d6", lw=0.6, alpha=0.35)
    # sampled levels and the worst gap, from the same metric as the table
    idx = np.arange(skip, n - 3); s = np.interp(idx * T + ph, t, P); k = np.asarray(c["sym"])[idx]
    for j in range(8):
        ax.plot([0, 0], [s[k == j].min(), s[k == j].max()], color="#0b0b0b", lw=2.0, solid_capstyle="round")
    ax.axvline(0, color="#52514e", lw=0.6, ls=(0, (3, 3)))
    ax.set_xlim(-T * 1e12, T * 1e12)
    ax.set_xlabel("time from sampling instant (ps)", color="#52514e")
    ax.set_ylabel("through-port power (µW)", color="#52514e")
    for sp in ("top", "right"):
        ax.spines[sp].set_visible(False)
    for sp in ("left", "bottom"):
        ax.spines[sp].set_color("#9a9893")
    ax.tick_params(colors="#52514e", labelsize=8)
    ax.grid(True, color="#e6e5e1", lw=0.5)
    ax.set_title(f"212 Gb/s PAM8, ring $\\tau_e$ = 15 ps ($Q_L$ ≈ 5500), 3-tap FFE, 1 mW laser, "
                 f"{n - skip - 2} periods folded\n"
                 f"worst eye {e['worst'] * 1e3:.1f} µW ({100 * e['norm']:.0f} % of ideal), "
                 f"OMA {e['oma'] * 1e3:.0f} µW, RLM {e['rlm']:.2f}\n"
                 f"black bars: spread of each of the 8 levels at the sampling instant",
                 fontsize=8, color="#0b0b0b", loc="left")
    fig.tight_layout(); fig.savefig("t8_eye.png"); plt.close(fig)
    print(f"eye: {100 * e['norm']:.1f} % of ideal, worst {e['worst'] * 1e3:.2f} uW, OMA {e['oma'] * 1e3:.0f} uW, "
          f"RLM {e['rlm']:.2f}, {r['points']} points, {n - skip - 2} periods folded -> t8_eye.png")


# ----------------------------------------------------------------------------------- power
POWERS = [0.1, 0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0]


def power_sweep():
    c = case(LEAD)
    lam_c = laser_cold(c)
    workers = max(1, (os.cpu_count() or 2) - 1)

    def scan(P, offsets_pm, sh, tag):
        jobs = [(P, o) for o in offsets_pm]
        with ThreadPoolExecutor(workers) as ex:
            rs = list(ex.map(lambda j: (j[1], run(c, lam_c + j[1] * 1e-3, j[0], sh, f"{tag}_P{j[0]}_o{j[1]}")), jobs))
        return rs

    res = []
    for P in POWERS:
        r0 = run(c, lam_c, P, 0, f"t8p_sh0_P{P}")
        res.append(dict(P=P, self_heat=0, offset_pm=0.0, lam=lam_c, **r0))
        hi = 300 * P + 100                                  # > 0.215 nm/mW * P with margin
        coarse = scan(P, list(np.arange(-100, hi + 1, 20)), 1, "t8p_c")
        ok = [(o, r) for o, r in coarse if "eye" in r]
        fails = len(coarse) - len(ok)
        o_best = max(ok, key=lambda x: x[1]["eye"]["worst"])[0]
        fine = scan(P, list(np.arange(o_best - 20, o_best + 21, 5)), 1, "t8p_f")
        ok += [(o, r) for o, r in fine if "eye" in r]
        fails += sum("eye" not in r for _, r in fine)
        o, b = max(ok, key=lambda x: x[1]["eye"]["worst"])
        res.append(dict(P=P, self_heat=1, offset_pm=float(o), lam=lam_c + o * 1e-3, failed_runs=fails,
                        scanned=len(coarse) + len(fine), **b))
        e0, e1 = r0["eye"], b["eye"]
        print(f"P {P:5.2f} mW | self_heat 0: eye {e0['worst'] * 1e3:7.2f} uW ({100 * e0['norm']:5.1f} %) | "
              f"self_heat 1: eye {e1['worst'] * 1e3:7.2f} uW ({100 * e1['norm']:5.1f} %), laser +{o:.0f} pm, "
              f"OMA {e1['oma'] * 1e3:5.0f} uW, RLM {e1['rlm']:.2f}, {fails} of {len(coarse) + len(fine)} runs failed",
              flush=True)
    json.dump(res, open("t8_power.json", "w"), indent=1)
    plot_power(res)


def plot_power(res):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    S = {0: ("self-heating off", "#2a78d6"), 1: ("self-heating on, laser re-tuned", "#eb6834")}
    fig, axs = plt.subplots(1, 3, figsize=(10.5, 3.3), dpi=200)
    fig.patch.set_facecolor("#fcfcfb")
    panels = [("worst eye opening (µW)", lambda r: r["eye"]["worst"] * 1e3, (0, 1)),
              ("eye opening (% of ideal)", lambda r: 100 * r["eye"]["norm"], (0, 1)),
              ("laser offset needed (pm)", lambda r: r["offset_pm"], (1,))]
    for ax, (lab, f, shs) in zip(axs, panels):
        ax.set_facecolor("#fcfcfb")
        for sh in shs:
            rs = sorted([r for r in res if r["self_heat"] == sh and "eye" in r], key=lambda r: r["P"])
            ax.plot([r["P"] for r in rs], [f(r) for r in rs], color=S[sh][1], lw=2, marker="o", ms=4,
                    label=S[sh][0])
        ax.set_xscale("log"); ax.set_xlabel("laser power (mW)", color="#52514e")
        ax.set_title(lab, fontsize=9, color="#0b0b0b", loc="left")
        for sp in ("top", "right"):
            ax.spines[sp].set_visible(False)
        for sp in ("left", "bottom"):
            ax.spines[sp].set_color("#9a9893")
        ax.tick_params(colors="#52514e", labelsize=8); ax.grid(True, color="#e6e5e1", lw=0.5)
    axs[0].legend(fontsize=7, frameon=False)
    fig.suptitle("212 Gb/s PAM8, ring τe = 15 ps (Q_L ≈ 5500), FFE pre −0.15 / post −0.10, 0.5–3 V",
                 fontsize=9, x=0.01, ha="left", color="#0b0b0b")
    fig.tight_layout(); fig.savefig("t8_power.png"); plt.close(fig)


if __name__ == "__main__":
    t6.VACLI = os.environ.get("VACLI", t6.VACLI)
    if "--eye" in sys.argv:
        eye_figure()
    if "--power" in sys.argv:
        power_sweep()
