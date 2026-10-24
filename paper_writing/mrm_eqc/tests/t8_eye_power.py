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
CL = 2.99792458e8


def mean_ring(c):
    """Cold resonance (nm) and total amplitude decay rate g (1/s) at the mean drive bias."""
    card = c["card"]
    v1, v2 = float(np.mean(c["v1"])), float(np.mean(c["v2"]))
    lr = card["lam0"] + 1e-3 * (card["s1"] * (card["c1"] * v1 + card["c2"] * v1 ** 2)
                                + card["s2"] * (card["c1"] * v2 + card["c2"] * v2 ** 2))
    w1 = card["s1"] / (card["s1"] + card["s2"]); w2 = 1 - w1
    f = lambda v: card["tl0"] + card["tl1"] * v + card["tl2"] * v * v
    tl = 1e-12 / (1 / card["tl0"] + w1 * (1 / f(v1) - 1 / card["tl0"]) + w2 * (1 / f(v2) - 1 / card["tl0"]))
    return lr, 1 / tl + 1 / card["tau_e"]


def lorentz(delta_pm, lres, g):
    """Normalised intracavity power g^2/(D^2+g^2) at laser - resonance = delta (pm)."""
    lres_m = lres * 1e-9
    D = 2 * np.pi * CL * (1 / (lres_m + delta_pm * 1e-12) - 1 / lres_m)
    return g * g / (D * D + g * g)


def power_sweep():
    """Frozen-temperature method (see module docstring of this mode in README 3d):
    over the 2.4 ns pattern the ring temperature is constant (tau_th = 1 us), so a self-heating
    shift d is exactly a laser offset -d. The eye at power P and effective detuning delta
    (laser minus heated resonance, at the mean drive) is therefore the self_heat = 0 eye at that
    delta, with the eye height scaled by P. Power decides which delta can be held: the point is
    thermally stable iff 0.215 P dL/d(delta) > -1 (red side: heating pulls the resonance onto the
    laser). Step 1: one self_heat = 0 scan of delta at 1 mW. Step 2: per power, best stable delta.
    Step 3 (check): direct self_heat = 1 runs at 0.5/1/2 mW at the laser that gives that delta,
    compared with the prediction."""
    c = case(LEAD)
    lam_c = laser_cold(c)
    lr, g = mean_ring(c)
    workers = max(1, (os.cpu_count() or 2) - 1)
    deltas = list(np.arange(-250, 201, 5))                       # pm, laser - resonance (mean bias)
    with ThreadPoolExecutor(workers) as ex:
        scan = list(ex.map(lambda dl: (float(dl), run(c, lr + dl * 1e-3, 1.0, 0, f"t8f_d{dl}")), deltas))
    scan = [(dl, r) for dl, r in scan if "eye" in r]
    dls = np.array([dl for dl, _ in scan])
    dLd = np.gradient(lorentz(dls, lr, g), dls)                   # 1/pm
    res = dict(lam_res_mean_nm=lr, laser_cold_ref_nm=lam_c, delta_ref_pm=1e3 * (lam_c - lr),
               scan=[dict(delta_pm=dl, eye=r["eye"], Pmean_mW=r["Pmean_mW"]) for dl, r in scan],
               powers=[], checks=[])
    for P in POWERS:
        stable = 0.215 * P * 1e3 * dLd > -1.0                     # 0.215 nm/mW -> pm
        cand = [(dl, r) for (dl, r), s in zip(scan, stable) if s]
        dl, r = max(cand, key=lambda x: x[1]["eye"]["norm"])
        dl_best_all = max(scan, key=lambda x: x[1]["eye"]["norm"])[0]
        d_shift = 0.215 * P * float(lorentz(np.array([dl]), lr, g)[0])      # nm, heating at that delta
        res["powers"].append(dict(P=P, delta_pm=dl, eye_norm=r["eye"]["norm"], worst_uW=r["eye"]["worst"] * 1e3 * P,
                                  oma_uW=r["eye"]["oma"] * 1e3 * P, heat_shift_pm=1e3 * d_shift,
                                  laser_nm=lr + d_shift + dl * 1e-3, optimum_stable=bool(dl == dl_best_all),
                                  stable_red_limit_pm=float(dls[stable][dls[stable] > 0].min()) if np.any(~stable) else None))
        print(f"P {P:5.2f} mW: best stable delta {dl:+5.0f} pm -> eye {100 * r['eye']['norm']:5.1f} % of ideal, "
              f"{r['eye']['worst'] * 1e3 * P:6.2f} uW; heating shift {1e3 * d_shift:6.1f} pm; "
              f"{'unconstrained optimum' if dl == dl_best_all else 'optimum (' + format(dl_best_all, '+.0f') + ' pm) unstable'}",
              flush=True)
    # check against direct self_heat = 1 runs (single-root region)
    for P in (0.5, 1.0, 2.0):
        p = next(x for x in res["powers"] if x["P"] == P)
        r = run(c, p["laser_nm"], P, 1, f"t8chk_P{P}")
        res["checks"].append(dict(P=P, laser_nm=p["laser_nm"], predicted_uW=p["worst_uW"],
                                  direct_uW=r["eye"]["worst"] * 1e3, direct_norm=r["eye"]["norm"],
                                  static_roots=r.get("static_roots")))
        print(f"check P {P} mW, self_heat=1 at laser {p['laser_nm']:.4f} nm: direct {r['eye']['worst'] * 1e3:6.2f} uW "
              f"({100 * r['eye']['norm']:.1f} %) vs predicted {p['worst_uW']:6.2f} uW "
              f"({100 * p['eye_norm']:.1f} %); static roots {r.get('static_roots')}", flush=True)
    dl_opt = max(scan, key=lambda x: x[1]["eye"]["norm"])[0]
    slope = float(np.diff(lorentz(np.array([dl_opt - 0.5, dl_opt + 0.5]), lr, g))[0])   # 1/pm
    res["threshold_mW"] = -1.0 / (0.215 * 1e3 * slope) if slope < 0 else None
    print(f"unconstrained optimum delta {dl_opt:+.0f} pm is thermally unstable above "
          f"{res['threshold_mW']:.2f} mW" if res["threshold_mW"] else "optimum is on the stable side")
    json.dump(res, open("t8_power.json", "w"), indent=1)
    plot_power(res)


def plot_power(res):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    ps = res["powers"]; P = [p["P"] for p in ps]
    fig, axs = plt.subplots(1, 3, figsize=(10.5, 3.3), dpi=200)
    fig.patch.set_facecolor("#fcfcfb")
    sc = res["scan"]
    best = max(sc, key=lambda s: s["eye"]["norm"])["eye"]["worst"] * 1e3
    panels = [("worst eye opening (µW)", [p["worst_uW"] for p in ps], [best * x for x in P]),
              ("eye opening (% of ideal)", [100 * p["eye_norm"] for p in ps],
               [100 * max(s["eye"]["norm"] for s in sc)] * len(P)),
              ("operating detuning (pm)\nlaser − heated resonance", [p["delta_pm"] for p in ps], None)]
    Pth = res.get("threshold_mW")
    for k, (ax, (lab, y, ref)) in enumerate(zip(axs, panels)):
        if k == 1:
            ax.set_ylim(0, 45)
        if Pth and k > 0:
            ax.axvline(Pth, color="#52514e", lw=0.8, ls=(0, (2, 2)))
            ax.annotate(f"red-side optimum\nunstable above {Pth:.1f} mW", (Pth, 0.05 if k == 1 else 0.45),
                        xycoords=("data", "axes fraction"), xytext=(4, 0), textcoords="offset points",
                        fontsize=7, color="#52514e")
        ax.set_facecolor("#fcfcfb")
        if ref is not None:
            ax.plot(P, ref, color="#2a78d6", lw=2, ls=(0, (4, 3)), label="without self-heating")
        ax.plot(P, y, color="#eb6834", lw=2, marker="o", ms=4, label="with self-heating (stable point)")
        for ch in res["checks"] if ref is not None and lab.startswith("worst") else []:
            ax.plot(ch["P"], ch["direct_uW"], ls="none", marker="s", ms=7, mfc="none", mec="#0b0b0b",
                    label="direct self-heating run" if ch is res["checks"][0] else None)
        ax.set_xscale("log"); ax.set_xlabel("laser power (mW)", color="#52514e")
        ax.set_title(lab, fontsize=9, color="#0b0b0b", loc="left")
        for sp in ("top", "right"):
            ax.spines[sp].set_visible(False)
        for sp in ("left", "bottom"):
            ax.spines[sp].set_color("#9a9893")
        ax.tick_params(colors="#52514e", labelsize=8); ax.grid(True, color="#e6e5e1", lw=0.5)
    axs[0].legend(fontsize=7, frameon=False)
    fig.suptitle("212 Gb/s PAM8, ring $\\tau_e$ = 15 ps ($Q_L$ ≈ 5500), FFE pre −0.15 / post −0.10, 0.5–3 V",
                 fontsize=9, x=0.01, ha="left", color="#0b0b0b")
    fig.tight_layout(); fig.savefig("t8_power.png"); plt.close(fig)


if __name__ == "__main__":
    t6.VACLI = os.environ.get("VACLI", t6.VACLI)
    if "--eye" in sys.argv:
        eye_figure()
    if "--plot" in sys.argv:                     # redraw t8_power.png from t8_power.json
        plot_power(json.load(open("t8_power.json")))
    if "--power" in sys.argv:
        power_sweep()
