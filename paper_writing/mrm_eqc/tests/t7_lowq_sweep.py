#!/usr/bin/env python3
"""t7: 212 Gb/s PAM8 with a lower-Q, wider-bandwidth variant of the [B] dual-segment ring.

Only the bus coupling changes (a narrower ring-bus gap): tau_e is swept, everything else stays
the [B] DUAL card - intrinsic loss tau_l(V) (junction doping + waveguide), modulation efficiency
c1, c2 (junction), circumference (FSR 6.5 nm) and all electrical parasitics. The coupling is
reported as the power coupling per round trip, kappa^2 = 2 T_rt / tau_e with
T_rt = lambda^2 / (FSR c), and as the loaded Q = omega tau / 2 (tau = amplitude decay time).

Same PAM8 case, eye metric and peak-limited FFE as t6 (t6_pam8_sweep.py). self_heat = 0: t6d/t6e
showed that self-heating only moves the right laser position (the transient keeps the first
symbol's temperature), so the laser is placed from the cold best-OMA point plus a detuning that
is scaled to each ring's half-linewidth. 1 mW laser, 0.5-3 V window.

    python t7_lowq_sweep.py            # VACLI = path to va-cli  -> t7_lowq_sweep.json
    python t7_lowq_sweep.py --refine   # t7b, around tau_e 15 and 8 ps -> t7b_lowq_refine.json
"""
import itertools, json, os, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np
import t5_eyes as t5
import t6_pam8_sweep as t6

c0 = 2.99792458e8
LAM, FSR = 1551.4e-9, 6.5e-9
T_RT = LAM ** 2 / (FSR * c0)
TAU_E_PS = [27.14, 20.0, 15.0, 12.0, 10.0, 8.0, 6.0]       # 27.14 = [B] Table II (DUAL)
PRES = [0.0, -0.05, -0.10, -0.15]
POSTS = [0.0, -0.10, -0.20, -0.30]
DET_HW = [-1.0, -0.5, 0.0, 0.25, 0.5, 0.75, 1.0, 1.5]       # detuning in half-linewidths
VLO, VHI, P = 0.5, 3.0, 1.0


def ring(tau_e_ps):
    tl = t6.DUAL["tl0"] * 1e-12
    tau = 1 / (1 / tl + 1 / (tau_e_ps * 1e-12))
    w = 2 * np.pi * c0 / LAM
    hw_pm = LAM ** 2 / (2 * np.pi * c0 * tau) * 1e12          # half-width at half-maximum (pm)
    return dict(tau_e_ps=tau_e_ps, kappa2=2 * T_RT / (tau_e_ps * 1e-12), QL=w * tau / 2,
                tau_ps=tau * 1e12, hwhm_pm=hw_pm, f3db_opt_GHz=1 / (2 * np.pi * tau) / 1e9)


def run_jobs(jobs):
    def go(j):
        c = t6.pam8_case(VLO, VHI, j["pre"], j["post"])
        c["card"] = dict(c["card"]); c["card"].update(tau_e=j["tau_e_ps"] * 1e-12, self_heat=0)
        lam0, oma = t5.best_laser(c["card"], c["fsr"], c["lo"], c["hi"], 1e-6)
        det_pm = j["det_hw"] * j["hwhm_pm"]
        r = t6.run_point(f"t7_te{j['tau_e_ps']}_{j['pre']}_{j['post']}_{j['det_hw']}", c,
                         lam0 + det_pm * 1e-3, P)
        return dict(j, det_pm=det_pm, lam=lam0 + det_pm * 1e-3, static_oma=oma, **r)

    with ThreadPoolExecutor(max_workers=max(1, (os.cpu_count() or 2) - 1)) as ex:
        return list(ex.map(go, jobs))


def refine():
    """t7b: finer taps and detuning around the two rings worth quoting (tau_e 15 ps with FFE,
    8 ps without) -> t7b_lowq_refine.json"""
    jobs = []
    for te, pres, posts, dets in (
            (15.0, [-0.10, -0.15, -0.20, -0.25], [-0.05, -0.10, -0.15], [0.6, 0.8, 1.0, 1.2, 1.4]),
            (8.0, [0.0, -0.03, -0.06, -0.09], [0.0, -0.03, -0.06], [0.0, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4])):
        rg = ring(te)
        jobs += [dict(rg, pre=a, post=b, det_hw=d) for a, b, d in itertools.product(pres, posts, dets)]
    res = run_jobs(jobs)
    json.dump(res, open("t7b_lowq_refine.json", "w"), indent=1)
    for te in (15.0, 8.0):
        for ffe in (False, True):
            rs = [r for r in res if r["tau_e_ps"] == te and "eye" in r and ((r["pre"], r["post"]) != (0.0, 0.0)) == ffe]
            if not rs:
                continue
            b = max(rs, key=lambda r: r["eye"]["norm"]); e = b["eye"]
            print(f"tau_e {te} {'FFE' if ffe else 'no FFE'}: {100 * e['norm']:.1f} % pre {b['pre']} post {b['post']} "
                  f"det {b['det_hw']} HW ({b['det_pm']:.0f} pm), worst {e['worst'] * 1e3:.2f} uW, "
                  f"OMA {e['oma'] * 1e3:.0f} uW, RLM {e['rlm']:.2f}")


def main():
    t6.VACLI = os.environ.get("VACLI", t6.VACLI)
    if "--refine" in sys.argv:
        return refine()
    jobs = []
    for te in TAU_E_PS:
        rg = ring(te)
        for pre, post, dh in itertools.product(PRES, POSTS, DET_HW):
            jobs.append(dict(rg, pre=pre, post=post, det_hw=dh))

    res = run_jobs(jobs)
    json.dump(res, open("t7_lowq_sweep.json", "w"), indent=1)
    print(f"{len(res)} runs, {sum('eye' not in r for r in res)} failed")
    print(f"{'tau_e ps':>8s} {'kappa^2':>7s} {'Q_L':>6s} {'pole 1/2pi tau':>14s} | {'no FFE':>7s} | "
          f"{'FFE best':>8s} {'pre':>5s} {'post':>5s} {'det pm':>6s} {'worst uW':>8s} {'OMA uW':>6s} {'RLM':>4s}")
    for te in TAU_E_PS:
        rs = [r for r in res if r["tau_e_ps"] == te and "eye" in r]
        nf = max((r for r in rs if r["pre"] == 0 and r["post"] == 0), key=lambda r: r["eye"]["norm"])
        b = max(rs, key=lambda r: r["eye"]["norm"]); e = b["eye"]
        print(f"{te:8.2f} {b['kappa2']:7.3f} {b['QL']:6.0f} {b['f3db_opt_GHz']:12.1f} G | "
              f"{100 * nf['eye']['norm']:6.1f}% | {100 * e['norm']:7.1f}% {b['pre']:5.2f} {b['post']:5.2f} "
              f"{b['det_pm']:6.1f} {e['worst'] * 1e3:8.2f} {e['oma'] * 1e3:6.0f} {e['rlm']:4.2f}")


if __name__ == "__main__":
    main()
