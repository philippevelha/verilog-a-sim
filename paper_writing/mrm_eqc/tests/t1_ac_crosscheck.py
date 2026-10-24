#!/usr/bin/env python3
"""Cross-check 1: [B]'s linear RLC network (mrm_eqc_rlc) against the AC (small-signal)
response of the large-signal model (mrm_eqc_ls) at the same operating point, in ngspice and
verilog-a-sim. Ideal voltage drive on one segment input; output = through-port optical power
(mW per volt). loss_mod=0 in the large-signal model holds tau_l at the bias, as [B]'s g does;
a second large-signal run with loss_mod=1 measures the loss-modulation term [B] omits.
Operating points are given by the detuning from the HEATED resonance (unique), and chosen
outside the thermal-bistability region so the large-signal DC solution is unique."""
import json, numpy as np
from harness import run, SINGLE, DUAL
c = 2.99792458e8
# name, card, FSR, Vr1, Vr2, det_pm (from heated resonance), P_in (mW), driven segment
CASES = [("single", SINGLE, 8.0, 0.0, 0.0, -150.0, 1.0, 1),
         ("single", SINGLE, 8.0, 2.0, 0.0, -100.0, 0.5, 1),
         ("single", SINGLE, 8.0, 4.0, 0.0, -70.0, 0.25, 1),
         ("single", SINGLE, 8.0, 2.0, 0.0, +100.0, 0.05, 1),
         ("dual_MSB", DUAL, 6.5, 2.0, 2.0, -100.0, 0.5, 1),
         ("dual_LSB", DUAL, 6.5, 2.0, 2.0, -100.0, 0.5, 2)]

def tl_of(card, vr1, vr2):
    w1 = card["s1"] / (card["s1"] + card["s2"]) if card["nseg"] == 2 else 1.0
    w2 = card["s2"] / (card["s1"] + card["s2"]) if card["nseg"] == 2 else 0.0
    f = lambda v: card["tl0"] + card["tl1"] * v + card["tl2"] * v * v
    return 1e-12 / (1 / card["tl0"] + w1 * (1 / f(vr1) - 1 / card["tl0"]) + w2 * (1 / f(vr2) - 1 / card["tl0"]))

def op_point(card, vr1, vr2, det_pm, P_in):
    lr = card["lam0"] + 1e-3 * (card["s1"] * (card["c1"] * vr1 + card["c2"] * vr1 ** 2)
                                + card["s2"] * (card["c1"] * vr2 + card["c2"] * vr2 ** 2))
    tau = 1 / (1 / tl_of(card, vr1, vr2) + 1 / card["tau_e"])
    D = -2 * np.pi * c * det_pm * 1e-12 / (lr * 1e-9) ** 2
    dsh = 0.215 * P_in / (1 + (D * tau) ** 2)
    return lr, dsh, lr + dsh + det_pm * 1e-3

if __name__ == "__main__":
    res = []
    for name, card, fsr, vr1, vr2, det, P, seg in CASES:
        lr, dsh, lamL = op_point(card, vr1, vr2, det, P)
        s1 = f"DC {-vr1} AC {1 if seg == 1 else 0}"; s2 = f"DC {-vr2} AC {1 if seg == 2 else 0}"
        rlc = dict(card); rlc.update(op_mode=0, det_pm=det, Vr1=vr1, Vr2=vr2, P_in=P)
        circ_rlc = [("V", "a", "a", "0", s1), ("V", "b", "b", "0", s2), ("X", "1", ["a", "b", "out"], "mrm_eqc_rlc", rlc)]
        def ls_circ(par):
            return [("V", "a", "a", "0", s1), ("V", "b", "b", "0", s2), ("V", "re", "inre", "0", f"DC {np.sqrt(P):.8f}"),
                    ("V", "im", "inim", "0", "DC 0"), ("V", "wl", "wl", "0", f"DC {lamL:.7f}"), ("V", "h", "hp", "0", "DC 0"),
                    ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"], "mrm_eqc_ls", par)]
        ls0 = dict(card); ls0.update(FSR=fsr, loss_mod=0, Vr1_l=vr1, Vr2_l=vr2)
        ls1 = dict(card); ls1.update(FSR=fsr, loss_mod=1)
        ana = ("ac", 20, 1e8, 1e11)
        for sim in ("ngspice", "va"):
            tag = f"t1_{name}_{vr1}_{det}_{P}"
            r1 = run(sim, tag + "_rlc", circ_rlc, ana, ["out"])
            r2 = run(sim, tag + "_ls0", ls_circ(ls0), ana, ["X1.pmon"])
            r3 = run(sim, tag + "_ls1", ls_circ(ls1), ana, ["X1.pmon"])
            H1, H2, H3 = r1["y"]["out"], r2["y"]["X1.pmon"], r3["y"]["X1.pmon"]
            err = float(np.max(np.abs(H1 - H2)) / np.max(np.abs(H2)))
            dlm = float(np.max(np.abs(H3 - H2)) / np.max(np.abs(H2)))
            res.append(dict(case=name, Vr=vr1, det_pm=det, P_in=P, sim=sim, rlc_vs_ls=err, loss_mod_term=dlm,
                            dc_mW_per_V=float(abs(H2[0])), f=r1["x"].tolist(),
                            H_rlc=[[z.real, z.imag] for z in H1], H_ls0=[[z.real, z.imag] for z in H2],
                            H_ls1=[[z.real, z.imag] for z in H3],
                            time=dict(rlc=r1["time"], ls0=r2["time"], ls1=r3["time"]),
                            points=dict(rlc=r1["points"], ls0=r2["points"])))
            print(f"{name:9s} Vr {vr1:3.1f} V det {det:+5.0f} pm P {P:4.2f} mW {sim:7s}: RLC vs large-signal {err:.1e} | "
                  f"loss-modulation term {dlm:5.1%} | DC {abs(H2[0]):.4f} mW/V", flush=True)
    json.dump(res, open("t1_ac_crosscheck.json", "w"), indent=1)
