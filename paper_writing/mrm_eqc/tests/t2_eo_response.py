#!/usr/bin/env python3
"""EO response of the single-segment MRM vs [B] Fig. 8(a),(b), both models, both simulators.
Drive: 50-ohm source on the pad (VNA port), response = through power, normalised at 1 GHz.
Assumptions (not stated in [B], flagged):
  Fig. 8(a): fixed laser 40 pm on the blue side of the 0 V (heated) resonance; bias 0/2/4 V
             red-shifts the resonance, so the detuning grows with bias.
  Fig. 8(b): detunings 40/70/100 pm from the heated resonance at 0 V bias.
  Laser power 0.1 mW (the normalised response does not depend on it at a given detuning)."""
import json, numpy as np
from harness import run, SINGLE
from t1_ac_crosscheck import op_point

def circ_rlc(vr, det, P):
    p = dict(SINGLE); p.update(op_mode=0, det_pm=det, Vr1=vr, P_in=P)
    return [("V", "s", "src", "0", f"DC {-vr} AC 1"), ("R", "s", "src", "a", 50.0), ("V", "b", "b", "0", "DC 0"),
            ("X", "1", ["a", "b", "out"], "mrm_eqc_rlc", p)]

def circ_ls(vr, det, P):
    lr, dsh, lamL = op_point(SINGLE, vr, 0.0, det, P)
    p = dict(SINGLE); p.update(FSR=8.0, loss_mod=1)
    return [("V", "s", "src", "0", f"DC {-vr} AC 1"), ("R", "s", "src", "a", 50.0), ("V", "b", "b", "0", "DC 0"),
            ("V", "re", "inre", "0", f"DC {np.sqrt(P):.8f}"), ("V", "im", "inim", "0", "DC 0"),
            ("V", "wl", "wl", "0", f"DC {lamL:.7f}"), ("V", "h", "hp", "0", "DC 0"),
            ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"], "mrm_eqc_ls", p)]

def features(f, H):
    y = 20 * np.log10(np.abs(H) / np.abs(H[0]))
    i = int(np.argmax(y)); pk = y[i]
    j = np.where((y < -3) & (f > f[i]))[0]
    f3 = np.interp(-3, y[j[0]:j[0] - 2:-1] if j[0] > 1 else y[j[0]:j[0]+1], f[j[0]:j[0] - 2:-1]) if len(j) else np.nan
    return float(pk), float(f[i] / 1e9), float(f3 / 1e9)

if __name__ == "__main__":
    P = 0.1
    c = 2.99792458e8
    # Fig. 8(a): fixed laser; detuning at bias vr = -40 pm - (resonance shift at vr) from the 0 V point
    shift = lambda vr: SINGLE["c1"] * vr + SINGLE["c2"] * vr ** 2
    cases = [("8a", "0V", 0.0, -40.0), ("8a", "-2V", 2.0, -40.0 - shift(2.0)), ("8a", "-4V", 4.0, -40.0 - shift(4.0)),
             ("8b", "40pm", 0.0, -40.0), ("8b", "70pm", 0.0, -70.0), ("8b", "100pm", 0.0, -100.0)]
    ref = json.load(open("../ref_data/fig8_features.json"))
    keymap = {"0V": "green", "-2V": "red", "-4V": "blue", "40pm": "green", "70pm": "red", "100pm": "blue"}
    res = []
    for fig, lab, vr, det in cases:
        for sim in ("ngspice", "va"):
            ana = ("ac", 40, 1e9, 5e10)
            r1 = run(sim, f"t2_{fig}_{lab}_rlc", circ_rlc(vr, det, P), ana, ["out"])
            r2 = run(sim, f"t2_{fig}_{lab}_ls", circ_ls(vr, det, P), ana, ["X1.pmon"])
            f1 = features(r1["x"], r1["y"]["out"]); f2 = features(r2["x"], r2["y"]["X1.pmon"])
            rf = ref[fig][keymap[lab]]
            res.append(dict(fig=fig, case=lab, Vr=vr, det_pm=det, sim=sim, rlc=f1, ls=f2, paper=rf, f=r2["x"].tolist(),
                            dB_rlc=(20*np.log10(np.abs(r1["y"]["out"])/np.abs(r1["y"]["out"][0]))).tolist(),
                            dB_ls=(20*np.log10(np.abs(r2["y"]["X1.pmon"])/np.abs(r2["y"]["X1.pmon"][0]))).tolist(),
                            time=dict(rlc=r1["time"], ls=r2["time"])))
            print(f"Fig.{fig} {lab:6s} (Vr {vr:.0f} V, det {det:+6.1f} pm) {sim:7s}: RLC peak {f1[0]:+.2f} dB @{f1[1]:4.1f} GHz f3dB {f1[2]:4.1f} | "
                  f"LS peak {f2[0]:+.2f} dB @{f2[1]:4.1f} f3dB {f2[2]:4.1f} | paper peak {rf['peak_dB']:+.2f} @{rf['f_peak_GHz']:4.1f} f3dB {rf['f3dB_GHz']}", flush=True)
    json.dump(res, open("t2_eo_response.json", "w"), indent=1)
