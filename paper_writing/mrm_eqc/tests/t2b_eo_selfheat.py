#!/usr/bin/env python3
"""Hypothesis test for [B] Fig. 8: detuning offsets measured from the LOW-POWER (cold)
resonance, with the measurement laser heating the ring (Eq. 12).  Large-signal model, AC at
the self-consistent DC point; laser = cold resonance - offset; one laser power for all curves.
Scans P_in, then reports the best P for Fig. 8(b) and applies it unchanged to Fig. 8(a)."""
import json, numpy as np
from harness import run, SINGLE
from t2_eo_response import features

def circ(vr, lamL, P):
    p = dict(SINGLE); p.update(FSR=8.0, loss_mod=1)
    return [("V", "s", "src", "0", f"DC {-vr} AC 1"), ("R", "s", "src", "a", 50.0), ("V", "b", "b", "0", "DC 0"),
            ("V", "re", "inre", "0", f"DC {np.sqrt(P):.8f}"), ("V", "im", "inim", "0", "DC 0"),
            ("V", "wl", "wl", "0", f"DC {lamL:.7f}"), ("V", "h", "hp", "0", "DC 0"),
            ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"], "mrm_eqc_ls", p)]

ref = json.load(open("../ref_data/fig8_features.json"))
lam0 = SINGLE["lam0"]
b_cases = [("40pm", 0.0, -40.0, "green"), ("70pm", 0.0, -70.0, "red"), ("100pm", 0.0, -100.0, "blue")]
a_cases = [("0V", 0.0, -40.0, "green"), ("-2V", 2.0, -40.0, "red"), ("-4V", 4.0, -40.0, "blue")]   # fixed laser
ana = ("ac", 40, 1e9, 5e10)

def score(sim, cases, fig, P, tagp):
    out = []
    for lab, vr, off, key in cases:
        r = run(sim, f"t2b_{fig}_{lab}_{tagp}", circ(vr, lam0 + off * 1e-3, P), ana, ["X1.pmon"])
        pk, fpk, f3 = features(r["x"], r["y"]["X1.pmon"])
        rf = ref[fig][key]
        out.append(dict(case=lab, peak=pk, fpk=fpk, f3=f3, paper=rf, time=r["time"], points=r["points"],
                        f=r["x"].tolist(), dB=(20*np.log10(np.abs(r["y"]["X1.pmon"])/np.abs(r["y"]["X1.pmon"][0]))).tolist()))
    err = np.sqrt(np.mean([(o["peak"] - o["paper"]["peak_dB"]) ** 2 for o in out]))
    return err, out

if __name__ == "__main__":
    scan = {}
    for P in (0.2, 0.35, 0.5, 0.6, 0.7, 0.85, 1.0):
        e, o = score("ngspice", b_cases, "8b", P, f"P{P}")
        scan[P] = e
        print(f"P_in {P:4.2f} mW: RMS peak error vs Fig. 8b {e:.2f} dB | " +
              " ".join(f"{x['case']}: {x['peak']:+.2f} dB/f3 {x['f3']:.0f}" for x in o), flush=True)
    Pbest = min(scan, key=scan.get)
    final = {}
    for sim in ("ngspice", "va"):
        for fig, cases in (("8b", b_cases), ("8a", a_cases)):
            e, o = score(sim, cases, fig, Pbest, f"best_{sim}")
            final[f"{fig}_{sim}"] = dict(rms_peak_err=e, cases=o)
            for x in o:
                print(f"P={Pbest} mW {sim:7s} Fig.{fig} {x['case']:6s}: peak {x['peak']:+.2f} dB @{x['fpk']:4.1f} GHz, f3dB {x['f3']:5.1f} | "
                      f"paper {x['paper']['peak_dB']:+.2f} @{x['paper']['f_peak_GHz']:4.1f}, f3dB {x['paper']['f3dB_GHz']}")
    json.dump(dict(scan=scan, P_best=Pbest, final=final), open("t2b_eo_selfheat.json", "w"), indent=1)
