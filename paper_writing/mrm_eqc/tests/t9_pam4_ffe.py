#!/usr/bin/env python3
"""t9: the four t5 PAM4 cases ([B] ring cards) with the peak-limited FFE of t6, at 1 mW.

Method: the frozen-temperature method (docs/frozen-temperature-method.md, as t8). Over the
pattern the ring temperature is constant, so self-heating is a laser offset. The sweep runs
with self_heat = 0 and a mean-level lead-in, in terms of delta = laser - resonance at the
mean drive. A point is kept only if it is thermally stable at 1 mW
(0.215 P dL/d(delta) > -1). The best point of each case is then checked with a direct
self_heat = 1 run at the laser that realises it.

Why not t5's own setup with taps added: there the laser is placed by a static model that
heats each level, and the transient keeps the first symbol's temperature. Sweeping toward the
resonance in that setup gave a best point at the grid edge (+80 pm) in a region where the
heating's stability is not checked. That earlier sweep is kept as
t9_pam4_ffe_t5conditions.json and t9b_pam4_ffe_t5conditions_wide.json.

Stage 1: delta scan without FFE, -300..+150 pm. Stage 2: taps pre -0.15..+0.05,
post -0.10..+0.20 (|pre| + |main| + |post| = 1, so the drive stays inside the window),
around each case's stage-1 optimum +-50 pm.

    python t9_pam4_ffe.py            # -> t9_pam4_ffe.json, t9_pam4_best.npz/_meta.json, t9_pam4_eyes.png
    python t9_pam4_ffe.py --plot     # redraw the figure
VACLI = path to va-cli. verilog-a-sim only.
"""
import itertools, json, os, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np
import t5_eyes as t5
import t6_pam8_sweep as t6
import t8_eye_power as t8
from harness import SINGLE, DUAL

VLO, VHI, P, LEAD, K = t5.VLO, t5.VHI, 1.0, 4, 0.215
NAMES = ["single_PAM4_80G", "dual_PAM4_80G", "single_PAM4_106G", "dual_PAM4_106G"]
DELTAS = list(range(-300, 151, 10))
PRES = [-0.15, -0.10, -0.05, 0.0, 0.05]
POSTS = [-0.10, -0.05, 0.0, 0.05, 0.10, 0.15, 0.20]


def cases(pre=0.0, post=0.0, lead=0):
    bits = t5.prbs9(511)
    sym4 = [2 * bits[2 * i] + bits[2 * i + 1] for i in range(255)]
    gray4 = [{0: 0, 1: 1, 3: 2, 2: 3}[s] for s in sym4]
    to_v = lambda y: list(VLO + (VHI - VLO) * (np.asarray(y) + 1.0) / 2.0)
    out = []
    for rate, baud in (("80G", 40e9), ("106G", 53e9)):
        x = [2.0 * g / 3.0 - 1.0 for g in gray4]
        out.append(dict(name=f"single_PAM4_{rate}", card=dict(SINGLE), fsr=8.0, baud=baud, M=4, sym=gray4,
                        v1=to_v(t6.ffe(x, pre, post)), v2=[0.0] * len(x), lo=(VLO, 0), hi=(VHI, 0)))
        msb = [g // 2 for g in sym4]; lsb = [g % 2 for g in sym4]
        out.append(dict(name=f"dual_PAM4_{rate}", card=dict(DUAL), fsr=6.5, baud=baud, M=4,
                        sym=[2 * m + l for m, l in zip(msb, lsb)],
                        v1=to_v(t6.ffe([2.0 * l - 1 for l in lsb], pre, post)),
                        v2=to_v(t6.ffe([2.0 * m - 1 for m in msb], pre, post)),
                        lo=(VLO, VLO), hi=(VHI, VHI)))
    if lead:
        for c in out:
            m1, m2 = float(np.mean(c["v1"])), float(np.mean(c["v2"]))
            c["v1"] = [m1] * lead + list(c["v1"]); c["v2"] = [m2] * lead + list(c["v2"])
            c["sym"] = [0] * lead + list(c["sym"])
    return out


def get(name, pre=0.0, post=0.0, lead=LEAD):
    return next(c for c in cases(pre, post, lead) if c["name"] == name)


def run0(name, pre, post, delta, tag, keep=False):
    """self_heat = 0 run at laser = mean-bias resonance + delta."""
    c = get(name, pre, post); c["card"] = dict(c["card"], self_heat=0)
    lr, _ = t8.mean_ring(c)
    return t6.run_point(tag, c, lr + delta * 1e-3, P, keep=keep)


def stable(name, delta):
    c = get(name); lr, g = t8.mean_ring(c)
    s = float(np.diff(t8.lorentz(np.array([delta - 0.5, delta + 0.5]), lr, g))[0])     # 1/pm
    return K * P * 1e3 * s > -1.0


def main():
    t6.VACLI = os.environ.get("VACLI", t6.VACLI)
    W = max(1, (os.cpu_count() or 2) - 1)
    # stage 1
    jobs = [(n, 0.0, 0.0, d) for n in NAMES for d in DELTAS]
    with ThreadPoolExecutor(W) as ex:
        s1 = list(ex.map(lambda j: dict(case=j[0], pre=0.0, post=0.0, delta_pm=j[3],
                                        **run0(j[0], 0.0, 0.0, j[3], f"t9s1_{j[0]}_{j[3]}")), jobs))
    # stage 2 around each case's stable stage-1 optimum
    jobs = []
    for n in NAMES:
        ok = [r for r in s1 if r["case"] == n and "eye" in r and stable(n, r["delta_pm"])]
        d0 = max(ok, key=lambda r: r["eye"]["norm"])["delta_pm"]
        jobs += [(n, a, b, d) for a, b in itertools.product(PRES, POSTS)
                 for d in range(d0 - 50, d0 + 51, 10) if (a, b) != (0.0, 0.0)]
    with ThreadPoolExecutor(W) as ex:
        s2 = list(ex.map(lambda j: dict(case=j[0], pre=j[1], post=j[2], delta_pm=j[3],
                                        **run0(j[0], j[1], j[2], j[3], f"t9s2_{j[0]}_{j[1]}_{j[2]}_{j[3]}")), jobs))
    res = s1 + s2
    for r in res:
        r["stable_1mW"] = bool(stable(r["case"], r["delta_pm"]))
    print(f"{len(res)} runs, {sum('eye' not in r for r in res)} failed")
    t5ref = {x["case"]: x for x in json.load(open("t5_eyes.json")) if x["sim"] == "va"}
    summary, wav = {}, {}
    for n in NAMES:
        rs = [r for r in res if r["case"] == n and "eye" in r]
        st_ = [r for r in rs if r["stable_1mW"]]
        nf = max((r for r in st_ if r["pre"] == 0 and r["post"] == 0), key=lambda r: r["eye"]["norm"])
        b = max(st_, key=lambda r: r["eye"]["norm"])
        ub = max(rs, key=lambda r: r["eye"]["norm"])
        # direct self-heating check at the laser that realises b
        c = get(n, b["pre"], b["post"]); lr, g = t8.mean_ring(c)
        d_heat = K * P * float(t8.lorentz(np.array([b["delta_pm"]]), lr, g)[0])
        lam = lr + d_heat + b["delta_pm"] * 1e-3
        chk = t8.run(c, lam, P, 1, f"t9chk_{n}")
        summary[n] = dict(t5_norm=t5ref[n]["eye"]["norm"], t5_worst_uW=t5ref[n]["eye"]["worst"] * 1e3,
                          noffe=dict(delta_pm=nf["delta_pm"], eye=nf["eye"]),
                          ffe=dict(pre=b["pre"], post=b["post"], delta_pm=b["delta_pm"], eye=b["eye"]),
                          unconstrained_best=dict(pre=ub["pre"], post=ub["post"], delta_pm=ub["delta_pm"],
                                                  norm=ub["eye"]["norm"], stable=ub["stable_1mW"]),
                          check=dict(laser_nm=lam, heat_shift_pm=1e3 * d_heat, eye=chk.get("eye"),
                                     static_roots=chk.get("static_roots")))
        e0, e1, ec = nf["eye"], b["eye"], chk.get("eye", {})
        print(f"{n:17s} t5 {100 * t5ref[n]['eye']['norm']:5.1f} % | no FFE {100 * e0['norm']:5.1f} % "
              f"(delta {nf['delta_pm']:+d}) | FFE {100 * e1['norm']:5.1f} % pre {b['pre']:+.2f} post {b['post']:+.2f} "
              f"delta {b['delta_pm']:+d}, {e1['worst'] * 1e3:5.1f} uW, OMA {e1['oma'] * 1e3:4.0f}, RLM {e1['rlm']:.2f} | "
              f"direct self-heat {100 * ec.get('norm', float('nan')):5.1f} % ({chk.get('static_roots')} root) | "
              f"unconstrained best {100 * ub['eye']['norm']:.1f} % {'stable' if ub['stable_1mW'] else 'UNSTABLE'}")
        for tag, (a, bb, d) in (("noffe", (0.0, 0.0, nf["delta_pm"])), ("ffe", (b["pre"], b["post"], b["delta_pm"]))):
            r = run0(n, a, bb, d, f"t9w_{n}_{tag}", keep=True)
            wav[f"{n}|{tag}"] = (r["t"], r["P"], r["eye"], 1 / get(n)["baud"], get(n)["sym"])
    json.dump(dict(runs=res, summary=summary), open("t9_pam4_ffe.json", "w"), indent=1)
    np.savez("t9_pam4_best.npz", **{k: np.array(v[0]) for k, v in wav.items()},
             **{k + "|P": np.array(v[1]) for k, v in wav.items()})
    json.dump({k: dict(eye=v[2], T=v[3], sym=list(map(int, v[4]))) for k, v in wav.items()},
              open("t9_pam4_best_meta.json", "w"), indent=1)
    plot()


def plot():
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    meta = json.load(open("t9_pam4_best_meta.json")); z = np.load("t9_pam4_best.npz")
    fig, axs = plt.subplots(2, 4, figsize=(13, 5.8), dpi=200, sharey="col")
    fig.patch.set_facecolor("#fcfcfb")
    for j, n in enumerate(NAMES):
        for i, tag in enumerate(("noffe", "ffe")):
            ax = axs[i, j]; ax.set_facecolor("#fcfcfb")
            m = meta[f"{n}|{tag}"]; t = z[f"{n}|{tag}"]; Pw = z[f"{n}|{tag}|P"] * 1e3
            T, ph, sym = m["T"], m["eye"]["ph"], m["sym"]
            tt = np.linspace(-T, T, 161)
            for k in range(16, len(sym) - 2):
                ax.plot(tt * 1e12, np.interp(k * T + ph + tt, t, Pw), color="#2a78d6" if tag == "noffe" else "#eb6834",
                        lw=0.5, alpha=0.3)
            ax.axvline(0, color="#52514e", lw=0.6, ls=(0, (3, 3)))
            ax.set_xlim(-T * 1e12, T * 1e12)
            e = m["eye"]
            ax.set_title(f"{n.replace('_', ' ')}, {'no FFE' if tag == 'noffe' else 'FFE'}\n"
                         f"eye {e['worst'] * 1e3:.1f} µW ({100 * e['norm']:.0f} % of ideal), RLM {e['rlm']:.2f}",
                         fontsize=7.5, color="#0b0b0b", loc="left")
            for sp in ("top", "right"):
                ax.spines[sp].set_visible(False)
            for sp in ("left", "bottom"):
                ax.spines[sp].set_color("#9a9893")
            ax.tick_params(colors="#52514e", labelsize=7); ax.grid(True, color="#e6e5e1", lw=0.5)
            if i == 1:
                ax.set_xlabel("time from sampling instant (ps)", color="#52514e", fontsize=8)
            if j == 0:
                ax.set_ylabel("through power (µW)", color="#52514e", fontsize=8)
    fig.suptitle("PAM4 on the [B] ring cards, 1 mW, 0.5–3 V, best thermally stable laser position — "
                 "top: no FFE, bottom: best peak-limited 3-tap FFE", fontsize=9, x=0.01, ha="left", color="#0b0b0b")
    fig.tight_layout(); fig.savefig("t9_pam4_eyes.png"); plt.close(fig)


def by_height():
    """Best stable points by absolute eye height (uW), the receiver's view, with and without FFE,
    each checked with a direct self_heat = 1 run -> t9_pam4_height.json. The main sweep ranks
    by % of ideal; a peak-limited FFE trades swing for flatness, so the two rankings differ."""
    t6.VACLI = os.environ.get("VACLI", t6.VACLI)
    d = json.load(open("t9_pam4_ffe.json")); R = [r for r in d["runs"] if "eye" in r and r["stable_1mW"]]
    out = {}
    for n in NAMES:
        rs = [r for r in R if r["case"] == n]
        for tag, pool in (("noffe", [r for r in rs if r["pre"] == 0 and r["post"] == 0]), ("any", rs)):
            b = max(pool, key=lambda r: r["eye"]["worst"])
            c = get(n, b["pre"], b["post"]); lr, g = t8.mean_ring(c)
            lam = lr + K * P * float(t8.lorentz(np.array([b["delta_pm"]]), lr, g)[0]) + b["delta_pm"] * 1e-3
            chk = t8.run(c, lam, P, 1, f"t9h_{n}_{tag}")
            out[f"{n}|{tag}"] = dict(pre=b["pre"], post=b["post"], delta_pm=b["delta_pm"], eye=b["eye"],
                                     direct_eye=chk.get("eye"), static_roots=chk.get("static_roots"))
            print(f"{n:17s} {tag:5s}: {b['eye']['worst'] * 1e3:5.1f} uW ({100 * b['eye']['norm']:.0f} %) pre {b['pre']:+.2f} "
                  f"post {b['post']:+.2f} delta {b['delta_pm']:+d} | direct self-heat "
                  f"{chk['eye']['worst'] * 1e3:5.1f} uW ({chk.get('static_roots')} root)")
    json.dump(out, open("t9_pam4_height.json", "w"), indent=1)


if __name__ == "__main__":
    if "--plot" in sys.argv:
        plot()
    elif "--height" in sys.argv:
        by_height()
    else:
        main()
