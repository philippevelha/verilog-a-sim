#!/usr/bin/env python3
"""Self-heating shape test vs [B] Fig. 6(a). The Fig. 6 ring has a lower Q than the Fig. 4
ring, so tau_l, tau_e and lambda0 are fitted to the 0.025 mW curve ONLY (tau_l 8.24 ps,
tau_e 10.0 ps, Q_L ~2700); the 1 mW spectrum (asymmetric, thermally dragged) is then a
prediction with dlam_dPin = 0.215 nm/mW (Fig. 6b). Up- and down-sweeps show the hysteresis."""
import json, numpy as np
from harness import run, SINGLE
from t3_static import ls_circ

FIG6 = dict(SINGLE); FIG6.update(lam0=1551.0083 - 0.215 * 0.025, tl0=8.24, tl1=0.0, tl2=0.0, tau_e=10.0e-12)
if __name__ == "__main__":
    ref = {P: np.loadtxt(f"../ref_data/fig6a_{P}.csv", delimiter=",", skiprows=1) for P in ("0.025mW", "1mW")}
    out = {}
    for sim in ("ngspice", "va"):
        for P, lab in ((0.025, "0.025mW"), (1.0, "1mW")):
            for dirn, (a, b, s) in (("up", (1550.4, 1551.6, 0.0005)), ("down", (1551.6, 1550.4, -0.0005))):
                try:
                    r = run(sim, f"t4_{lab}_{dirn}", ls_circ(FIG6, 8.0, 0, 0, P, 0), ("dc", "wl", a, b, s), ["X1.pmon"])
                except Exception as e:
                    print(sim, lab, dirn, "failed:", str(e)[:200]); continue
                x = r["x"]; T = 10 * np.log10(r["y"]["X1.pmon"] / P)
                o = np.argsort(x); x, T = x[o], T[o]
                i = int(np.argmin(T))
                rr = ref[lab]; m = rr[:, 1] < -2.0
                rms = float(np.sqrt(np.mean((np.interp(rr[m, 0], x, T) - rr[m, 1]) ** 2)))
                out[f"{sim}_{lab}_{dirn}"] = dict(dip_nm=float(x[i]), dip_dB=float(T[i]), rms_vs_fig6a_dB=rms,
                                                  time=r["time"], points=r["points"], curve=np.c_[x, T].tolist())
                print(f"{sim:7s} {lab:8s} sweep {dirn:4s}: dip {x[i]:.4f} nm {T[i]:6.2f} dB | RMS vs Fig. 6a {rms:.2f} dB "
                      f"| {r['points']} pts, {r['time']['wall_s']:.2f} s", flush=True)
    json.dump(out, open("t4_selfheat_shape.json", "w"), indent=1)
