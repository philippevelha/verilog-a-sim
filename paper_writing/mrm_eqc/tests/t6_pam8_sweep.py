#!/usr/bin/env python3
"""t6: can the 212 Gb/s PAM8 eye of [B] be opened by drive conditions [B] does not state?

Sweeps, for the t5 dual-segment PAM8 case (same pattern, segment split, model card and eye
metric as t5_eyes.py): the reverse-bias window (VLO..VHI, both drivers), the laser power, and
the laser detuning around the blue-side maximum-OMA wavelength. verilog-a-sim only (VA_JIT=on),
runs in parallel with --threads 1. No equalisation, 50-ohm sources, 8 ps edges, as in t5.

The thermal state is whatever the DC operating point gives (tau_th = 1 us >> 2.4 ns pattern),
exactly as in t5.

    python t6_pam8_sweep.py [--quick]   # t6:  no FFE           -> t6_pam8_sweep.json
    python t6_pam8_sweep.py --ffe       # t6b: 3-tap FFE        -> t6b_pam8_ffe.json
    python t6_pam8_sweep.py --refine    # t6c: finer, 4th tap   -> t6c_pam8_ffe_refine.json
    python t6_pam8_sweep.py --noselfheat  # t6d                 -> t6d_pam8_ffe_noselfheat.json
    python t6_pam8_sweep.py --detscan   # t6e                   -> t6e_pam8_detscan_selfheat.json
VACLI = path to va-cli. Each mode prints its best points.
"""
import json, os, shutil, subprocess, sys, itertools, re, time
from concurrent.futures import ThreadPoolExecutor
import numpy as np
import t5_eyes as t5
from harness import DUAL, deck_va, ROOT, HERE

VACLI = os.environ.get("VACLI", "va-cli")
NUM = r"([-+\d.eE]+)"


def ffe(x, pre, post, post2=0.0):
    """3/4-tap FFE on a symbol sequence in [-1, 1]:
    y[n] = pre x[n+1] + main x[n] + post x[n-1] + post2 x[n-2],
    main = 1 - |pre| - |post| - |post2|, so |y| <= 1: the equalised drive stays inside the same
    window (a peak-limited driver, the swing is not increased)."""
    main = 1.0 - abs(pre) - abs(post) - abs(post2)
    x = np.asarray(x, float); xp = np.r_[x[1:], x[-1]]
    xm = np.r_[x[0], x[:-1]]; xm2 = np.r_[x[0], x[0], x[:-2]]
    return pre * xp + main * x + post * xm + post2 * xm2


def pam8_case(vlo, vhi, pre=0.0, post=0.0, post2=0.0):
    bits = t5.prbs9(511)
    sym8 = [4 * bits[3 * i] + 2 * bits[3 * i + 1] + bits[3 * i + 2] for i in range(170)]
    d38 = dict(DUAL); stot = DUAL["s1"] + DUAL["s2"]; d38.update(s1=0.4 * stot, s2=0.6 * stot)
    msb = [s // 4 for s in sym8]; low = [s % 4 for s in sym8]
    to_v = lambda y: list(vlo + (vhi - vlo) * (np.asarray(y) + 1.0) / 2.0)
    x1 = [2.0 * l / 3.0 - 1.0 for l in low]; x2 = [2.0 * m - 1.0 for m in msb]
    return dict(card=d38, fsr=6.5, baud=212e9 / 3, M=8, sym=sym8,
                v1=to_v(ffe(x1, pre, post, post2)), v2=to_v(ffe(x2, pre, post, post2)),
                lo=(vlo, vlo), hi=(vhi, vhi))


def run_point(tag, c, lam, P, extra=(), keep=False):
    """extra: netlist lines inserted before .end (e.g. a .nodeset); keep: also return t, P."""
    T = 1 / c["baud"]
    p = dict(c["card"]); p.update(FSR=c["fsr"], loss_mod=1)
    circ = [("V", "a", "sa", "0", t5.pwl([-v for v in c["v1"]], T)), ("R", "a", "sa", "a", 50.0),
            ("V", "b", "sb", "0", t5.pwl([-v for v in c["v2"]], T)), ("R", "b", "sb", "b", 50.0),
            ("V", "re", "inre", "0", f"DC {np.sqrt(P):.10g}"), ("V", "im", "inim", "0", "DC 0"),
            ("V", "wl", "wl", "0", f"DC {lam:.7f}"), ("V", "h", "hp", "0", "DC 0"),
            ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"],
             "mrm_eqc_ls", p)]
    w = os.path.join(HERE, "work", "t6", tag); os.makedirs(w, exist_ok=True)
    shutil.copy(os.path.join(ROOT, "mrm_eqc_ls.va"), w)
    for h in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(ROOT, "vasim", h), w)
    deck = deck_va(circ, ("tran", T / 40, len(c["sym"]) * T))
    if extra:
        lines = deck.rstrip("\n").split("\n")
        deck = "\n".join(lines[:-1] + list(extra) + lines[-1:]) + "\n"   # before .end
    open(os.path.join(w, "deck.net"), "w").write(deck)
    t0 = time.perf_counter()
    r = subprocess.run([VACLI, "sim", "deck.net", "--model", ".", "--tran", "--threads", "1",
                        "--report", "X1.pmon"], cwd=w, capture_output=True, text=True,
                       env=dict(os.environ, VA_JIT="on"))
    wall = time.perf_counter() - t0
    a = np.array(re.findall(r"t=" + NUM + r"s\s+V\(X1\.pmon\)=" + NUM, r.stdout), float)
    if len(a) == 0:
        return dict(error=(r.stdout + r.stderr)[-400:])
    e = t5.eye(a[:, 0], a[:, 1], c["sym"], c["M"], T)
    out = dict(eye=e, points=len(a), wall_s=wall, Pmean_mW=float(a[:, 1].mean()))
    if keep:
        out.update(t=a[:, 0], P=a[:, 1])
    return out


def extra(mode):
    """t6d (--noselfheat): 1 mW, self_heat=0, laser from the cold best-OMA point + detuning, finer
    and wider FFE taps. t6e (--detscan): 1 mW, self_heat=1, the t6c best taps, detuning -100..+300 pm
    from the static heated best-OMA point (the static model heats each level; the transient
    keeps the first symbol's temperature, so the right laser position is far from that estimate)."""
    if mode == "noselfheat":
        grid = list(itertools.product([-0.08, -0.10, -0.12, -0.14, -0.16], [-0.15, -0.20, -0.25, -0.30],
                                      [0.0, -0.04], [10, 20, 30], [(0.5, 3.0), (0.0, 3.0)]))
        sh, Pplace, name = 0, 1e-6, "t6d_pam8_ffe_noselfheat.json"
    else:
        grid = [(-0.12, -0.20, 0.0, d, (0.5, 3.0)) for d in range(-100, 301, 20)]
        sh, Pplace, name = 1, 1.0, "t6e_pam8_detscan_selfheat.json"

    def go(j):
        pre, post, post2, det, (vlo, vhi) = j
        c = pam8_case(vlo, vhi, pre, post, post2); c["card"] = dict(c["card"]); c["card"]["self_heat"] = sh
        lam0, _ = t5.best_laser(c["card"], c["fsr"], c["lo"], c["hi"], Pplace)
        r = run_point(f"{mode}_{vlo}_{pre}_{post}_{post2}_{det}", c, lam0 + det * 1e-3, 1.0)
        return dict(vlo=vlo, vhi=vhi, pre=pre, post=post, post2=post2, det_pm=det, P=1.0, self_heat=sh,
                    lam=lam0 + det * 1e-3, **r)

    with ThreadPoolExecutor(max_workers=max(1, (os.cpu_count() or 2) - 1)) as ex:
        res = list(ex.map(go, grid))
    json.dump(res, open(name, "w"), indent=1)
    ok = sorted([r for r in res if "eye" in r], key=lambda r: -r["eye"]["norm"])
    print(f"{len(res)} runs, {len(res) - len(ok)} failed (operating point; thermal bistability)")
    for r in ok[:8]:
        e = r["eye"]
        print(f"{r['vlo']}-{r['vhi']} pre {r['pre']:+.2f} post {r['post']:+.2f} post2 {r['post2']:+.2f} "
              f"det {r['det_pm']:+d}: eye {100 * e['norm']:5.1f} %, worst {e['worst'] * 1e3:5.2f} uW, "
              f"OMA {e['oma'] * 1e3:4.0f} uW, RLM {e['rlm']:.2f}")


def main():
    for m in ("noselfheat", "detscan"):
        if "--" + m in sys.argv:
            return extra(m)
    quick = "--quick" in sys.argv
    with_ffe = "--ffe" in sys.argv
    refine = "--refine" in sys.argv
    posts2 = [0.0]
    if refine:              # t6c: around the t6b optimum, finer taps, a 4th tap, lower power
        with_ffe = True
        windows = [(0.5, 3.0), (0.0, 3.0)]
        powers = [0.1, 0.25, 0.5]
        detunings_pm = [5, 10, 15, 20]
        pres = [-0.08, -0.10, -0.12, -0.14]
        posts = [-0.10, -0.125, -0.15, -0.175, -0.20]
        posts2 = [0.0, -0.03, -0.06]
    elif with_ffe:            # t6b: the two best windows/powers of t6, plus a 3-tap FFE
        windows = [(0.5, 3.0), (0.0, 3.0)]
        powers = [0.5, 1.0]
        detunings_pm = [-40, -20, 0, 10]
        pres = [0.0, -0.05, -0.10, -0.15, -0.20]
        posts = [0.0, -0.05, -0.10, -0.15, -0.20, -0.25, -0.30, -0.35]
    else:
        windows = [(0.5, 3.0), (0.0, 3.0), (0.0, 4.0), (0.5, 4.5), (1.0, 4.0)] if not quick else [(0.5, 3.0)]
        powers = [0.5, 1.0, 2.0, 4.0] if not quick else [1.0]
        detunings_pm = [-40, -30, -20, -15, -10, -5, 0, 5, 10] if not quick else [0]
        pres, posts = [0.0], [0.0]
    jobs = []
    for (vlo, vhi), P in itertools.product(windows, powers):
        c0 = pam8_case(vlo, vhi)
        lam0, oma = t5.best_laser(c0["card"], c0["fsr"], c0["lo"], c0["hi"], P)
        for dpm, pre, post, post2 in itertools.product(detunings_pm, pres, posts, posts2):
            jobs.append(dict(vlo=vlo, vhi=vhi, P=P, det_pm=dpm, pre=pre, post=post, post2=post2,
                             lam_best=lam0, static_oma=oma, lam=lam0 + dpm * 1e-3,
                             c=pam8_case(vlo, vhi, pre, post, post2)))
    print(f"{len(jobs)} runs", flush=True)

    def go(j):
        tag = f"w{j['vlo']}_{j['vhi']}_P{j['P']}_d{j['det_pm']}_f{j['pre']}_{j['post']}_{j['post2']}"
        r = run_point(tag, j["c"], j["lam"], j["P"])
        out = {k: v for k, v in j.items() if k != "c"}; out.update(r)
        return out

    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=max(1, (os.cpu_count() or 2) - 1)) as ex:
        res = list(ex.map(go, jobs))
    print(f"done in {time.perf_counter() - t0:.0f} s", flush=True)
    name = "t6c_pam8_ffe_refine.json" if refine else "t6b_pam8_ffe.json" if with_ffe else ("t6_pam8_sweep_quick.json" if quick else "t6_pam8_sweep.json")
    json.dump(res, open(name, "w"), indent=1)
    ok = [r for r in res if "eye" in r]
    bad = [r for r in res if "eye" not in r]
    if bad:
        print(f"{len(bad)} runs failed, e.g.: {bad[0]['error'][-200:]}")
    ok.sort(key=lambda r: -r["eye"]["norm"])
    print(f"{'window (V)':>11s} {'P mW':>5s} {'det pm':>6s} {'pre':>5s} {'post':>5s} {'post2':>5s} {'laser nm':>10s} "
          f"{'eye % ideal':>11s} {'worst uW':>8s} {'OMA uW':>7s} {'RLM':>5s}")
    for r in ok[:15]:
        e = r["eye"]
        print(f"{r['vlo']:4.1f}-{r['vhi']:3.1f}   {r['P']:5.1f} {r['det_pm']:6d} {r['pre']:5.2f} {r['post']:5.3f} {r['post2']:5.2f} "
              f"{r['lam']:10.4f} {100 * e['norm']:11.1f} {e['worst'] * 1e3:8.1f} {e['oma'] * 1e3:7.0f} {e['rlm']:5.2f}")


if __name__ == "__main__":
    main()
