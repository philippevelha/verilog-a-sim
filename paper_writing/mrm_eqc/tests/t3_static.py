#!/usr/bin/env python3
"""Static checks of the large-signal model, both simulators (DC wavelength sweeps, blue -> red):
  Fig. 4(a): single segment, bias 0/1/2/3 V, low power (0.025 mW)    -> resonance and depth
  Fig. 4(b): dual segment, MSB/LSB at 0 or 4 V                        -> resonance and depth
  Fig. 6(a,b): self-heating, 0.025 ... 1 mW (resonance placed at the 0.025 mW dip of Fig. 6a)
  Fig. 7: heater 0 / 0.5 / 1 mW
Resonance positions in Fig. 4 are fitted (c1, c2, s1, s2) -> consistency; depths and the
Fig. 6 shape are independent checks."""
import json, numpy as np
from harness import run, SINGLE, DUAL

def ls_circ(card, fsr, vr1, vr2, P, Vh, extra=None):
    p = dict(card); p.update(FSR=fsr, loss_mod=1); p.update(extra or {})
    return [("V", "a", "a", "0", f"DC {-vr1}"), ("V", "b", "b", "0", f"DC {-vr2}"),
            ("V", "re", "inre", "0", f"DC {np.sqrt(P):.8f}"), ("V", "im", "inim", "0", "DC 0"),
            ("V", "wl", "wl", "0", "DC 1550"), ("V", "h", "hp", "0", f"DC {Vh:.6f}"),
            ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"], "mrm_eqc_ls", p)]

def sweep(sim, tag, circ, l1, l2, step):
    r = run(sim, tag, circ, ("dc", "wl", l1, l2, step), ["X1.pmon"])
    return r

def dip(r, P):
    T = r["y"]["X1.pmon"] / P; i = int(np.argmin(T))
    return float(r["x"][i]), float(10 * np.log10(T[i])), r

if __name__ == "__main__":
    dig = json.load(open("../ref_data/digitised_points.json"))
    out = {}
    for sim in ("ngspice", "va"):
        o = {}
        for lab, vr in (("0V", 0), ("-1V", 1), ("-2V", 2), ("-3V", 3)):
            l, d, r = dip(sweep(sim, f"t3_4a_{lab}", ls_circ(SINGLE, 8.0, vr, 0, 0.025, 0), 1552.6, 1553.6, 0.0005), 0.025)
            o[f"4a_{lab}"] = dict(lam=l, depth=d, paper=dig["fig4a"][lab], time=r["time"], points=r["points"])
        for lab, v1, v2 in (("M0L0", 0, 0), ("M-4L0", 4, 0), ("M0L-4", 0, 4), ("M-4L-4", 4, 4)):
            l, d, r = dip(sweep(sim, f"t3_4b_{lab}", ls_circ(DUAL, 6.5, v1, v2, 0.025, 0), 1550.9, 1551.9, 0.0005), 0.025)
            o[f"4b_{lab}"] = dict(lam=l, depth=d, paper=dig["fig4b"][lab], time=r["time"], points=r["points"])
        # self-heating: Fig. 6 device (resonance near 1551.0 nm); cold resonance put so that the
        # 0.025 mW dip falls at the digitised 1551.0064 nm
        lam6 = dig["fig6a"]["0.025mW"][0] - 0.215 * 0.025
        for P in (0.025, 0.25, 0.5, 0.75, 1.0):
            l, d, r = dip(sweep(sim, f"t3_6_{P}", ls_circ(SINGLE, 8.0, 0, 0, P, 0, dict(lam0=lam6)), 1550.4, 1551.6, 0.0005), P)
            o[f"6_{P}"] = dict(lam=l, depth=d, time=r["time"], points=r["points"],
                               curve=np.c_[r["x"], 10 * np.log10(r["y"]["X1.pmon"] / P)].tolist() if P in (0.025, 1.0) else None)
        for Ph in (0.0, 0.5, 1.0):
            Vh = np.sqrt(Ph * 1e-3 * 1e3)            # Rh = 1 kOhm
            l, d, r = dip(sweep(sim, f"t3_7_{Ph}", ls_circ(SINGLE, 8.0, 0, 0, 0.025, Vh), 1552.6, 1553.6, 0.0005), 0.025)
            o[f"7_{Ph}"] = dict(lam=l, depth=d, time=r["time"], points=r["points"])
        out[sim] = o
        print(f"--- {sim}")
        for k, v in o.items():
            pp = v.get("paper"); s = f"   paper {pp[0]:.4f} nm {pp[1]:6.2f} dB" if pp else ""
            print(f"{k:10s} dip {v['lam']:.4f} nm {v['depth']:7.2f} dB{s}")
    # derived: self-heating slope and heater slope
    for sim in out:
        P = np.array([0.025, 0.25, 0.5, 0.75, 1.0]); L = np.array([out[sim][f"6_{p}"]["lam"] for p in P])
        Ph = np.array([0, 0.5, 1.0]); Lh = np.array([out[sim][f"7_{p}"]["lam"] for p in Ph])
        out[sim]["slope_selfheat_nm_per_mW"] = float(np.polyfit(P, L, 1)[0])
        out[sim]["slope_heater_nm_per_mW"] = float(np.polyfit(Ph, Lh, 1)[0])
        print(f"{sim}: self-heating slope {out[sim]['slope_selfheat_nm_per_mW']:.4f} nm/mW (Fig. 6b 0.2149), "
              f"heater slope {out[sim]['slope_heater_nm_per_mW']:.4f} nm/mW (Fig. 7 points 0.150)")
    json.dump(out, open("t3_static.json", "w"), indent=1)
