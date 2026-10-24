#!/usr/bin/env python3
"""Large-signal eyes with the [B] parameter cards, both simulators, wall time and peak memory.
  80 Gb/s PAM4 (40 GBd) and 106 Gb/s PAM4 (53 GBd): single segment driven with PAM4;
       dual segment as an optical DAC (two NRZ drivers).
  212 Gb/s PAM8 (70.67 GBd): dual segment, NRZ on the stronger segment + PAM4 on the weaker,
       efficiency ratio 3:2 as in [B] Sec. V (total efficiency kept), no FFE.
Conditions NOT given in [B] (stated choices): 2.5 Vpp per driver from -0.5 V to -3.0 V reverse,
50-ohm source, 8 ps 0-100 % edges, 1 mW laser at the blue-side maximum-OMA wavelength
(found with the static model incl. self-heating), PRBS9-derived symbols.
Dual segment: [B] Fig. 4(b) shows the segment labelled 'LSB' shifting MORE (s=0.642) than
'MSB' (s=0.407); the MSB bit is therefore put on the stronger segment."""
import json, numpy as np, sys
from harness import run, SINGLE, DUAL

c = 2.99792458e8
VLO, VHI = 0.5, 3.0            # reverse bias range (V)

def prbs9(n):
    s = 0x1FF; out = []
    for _ in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1; s = ((s << 1) | b) & 0x1FF; out.append(b)
    return out

def pwl(levels, T, tr=8e-12):
    pts = [(0.0, levels[0])]
    for i in range(1, len(levels)):
        if levels[i] != levels[i - 1]:
            pts += [(i * T, levels[i - 1]), (i * T + tr, levels[i])]
    pts.append((len(levels) * T, levels[-1]))
    return "PWL(" + " ".join(f"{a:.6e} {b:.5f}" for a, b in pts) + ")"

def steady_T(card, fsr, lamL, vr1, vr2, P):
    """static transmission incl. self-heating (blue side -> unique), python twin of the model"""
    lr = card["lam0"] + 1e-3 * (card["s1"] * (card["c1"] * vr1 + card["c2"] * vr1 ** 2) + card["s2"] * (card["c1"] * vr2 + card["c2"] * vr2 ** 2))
    w1 = card["s1"] / (card["s1"] + card["s2"]) if card["nseg"] == 2 else 1.0; w2 = 1 - w1 if card["nseg"] == 2 else 0.0
    f = lambda v: card["tl0"] + card["tl1"] * v + card["tl2"] * v * v
    tl = 1e-12 / (1 / card["tl0"] + w1 * (1 / f(vr1) - 1 / card["tl0"]) + w2 * (1 / f(vr2) - 1 / card["tl0"]))
    gl, ge = 1 / tl, 1 / card["tau_e"]; g = gl + ge
    d = 0.0
    for _ in range(200):
        D = 2 * np.pi * c * (1 / (lamL * 1e-9) - 1 / ((lr + d) * 1e-9)); d = 0.5 * d + 0.5 * 0.215 * P * g * g / (D * D + g * g)
    D = 2 * np.pi * c * (1 / (lamL * 1e-9) - 1 / ((lr + d) * 1e-9))
    return (D * D + (gl - ge) ** 2) / (D * D + g * g)

def best_laser(card, fsr, lo, hi, P=1.0):
    lams = card["lam0"] + np.arange(-0.4, 0.0, 0.0005)
    oma = [abs(steady_T(card, fsr, l, *hi, P) - steady_T(card, fsr, l, *lo, P)) for l in lams]
    return float(lams[int(np.argmax(oma))]), float(max(oma))

def eye(t, P, sym, M, T, skip=16):
    idx = np.arange(skip, len(sym) - 3); k = np.asarray(sym)[idx]; best = None
    for ph in np.arange(0, 3 * T, T / 100):
        s = np.interp(idx * T + ph, t, P)
        means = np.array([s[k == j].mean() for j in range(M)])
        if means[-1] < means[0]: s, means = -s, -means
        e = [s[k == j + 1].min() - s[k == j].max() for j in range(M - 1)]
        if best is None or min(e) > best["worst"]:
            oma = means[-1] - means[0]
            best = dict(worst=float(min(e)), oma=float(abs(oma)), norm=float(min(e) / (abs(oma) / (M - 1))),
                        rlm=float(np.min(np.diff(means)) / (abs(oma) / (M - 1))), ph=float(ph))
    return best

def case_defs():
    bits = prbs9(511)
    sym4 = [2 * bits[2 * i] + bits[2 * i + 1] for i in range(255)]
    gray4 = [{0: 0, 1: 1, 3: 2, 2: 3}[s] for s in sym4]
    sym8 = [4 * bits[3 * i] + 2 * bits[3 * i + 1] + bits[3 * i + 2] for i in range(170)]
    cases = []
    for rate, baud in (("80G", 40e9), ("106G", 53e9)):
        # single: PAM4 levels on one segment
        lv = [VLO + (VHI - VLO) * g / 3 for g in gray4]
        cases.append(dict(name=f"single_PAM4_{rate}", card=SINGLE, fsr=8.0, baud=baud, M=4, sym=gray4,
                          v1=lv, v2=[0.0] * len(lv), lo=(VLO, 0), hi=(VHI, 0)))
        # dual optical DAC: MSB bit -> stronger segment (2), LSB bit -> segment 1
        msb = [g // 2 for g in sym4]; lsb = [g % 2 for g in sym4]
        lvl = [2 * m + l for m, l in zip(msb, lsb)]
        cases.append(dict(name=f"dual_PAM4_{rate}", card=DUAL, fsr=6.5, baud=baud, M=4, sym=lvl,
                          v1=[VLO + (VHI - VLO) * l for l in lsb], v2=[VLO + (VHI - VLO) * m for m in msb],
                          lo=(VLO, VLO), hi=(VHI, VHI)))
    # PAM8: NRZ (MSB) on the 3/5 segment, PAM4 (2 LSBs) on the 2/5 segment
    d38 = dict(DUAL); stot = DUAL["s1"] + DUAL["s2"]; d38.update(s1=0.4 * stot, s2=0.6 * stot)
    msb = [s // 4 for s in sym8]; low = [s % 4 for s in sym8]
    cases.append(dict(name="dual_PAM8_212G", card=d38, fsr=6.5, baud=212e9 / 3, M=8, sym=sym8,
                      v1=[VLO + (VHI - VLO) * l / 3 for l in low], v2=[VLO + (VHI - VLO) * m for m in msb],
                      lo=(VLO, VLO), hi=(VHI, VHI)))
    return cases

if __name__ == "__main__":
    only = sys.argv[1].split(",") if len(sys.argv) > 1 else None
    res = []
    for cdef in case_defs():
        if only and cdef["name"] not in only: continue
        T = 1 / cdef["baud"]
        lamL, oma = best_laser(cdef["card"], cdef["fsr"], cdef["lo"], cdef["hi"])
        p = dict(cdef["card"]); p.update(FSR=cdef["fsr"], loss_mod=1)
        circ = [("V", "a", "sa", "0", pwl([-v for v in cdef["v1"]], T)), ("R", "a", "sa", "a", 50.0),
                ("V", "b", "sb", "0", pwl([-v for v in cdef["v2"]], T)), ("R", "b", "sb", "b", 50.0),
                ("V", "re", "inre", "0", "DC 1.0"), ("V", "im", "inim", "0", "DC 0"),
                ("V", "wl", "wl", "0", f"DC {lamL:.7f}"), ("V", "h", "hp", "0", "DC 0"),
                ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"], "mrm_eqc_ls", p)]
        ana = ("tran", T / 40, len(cdef["sym"]) * T)
        rs = {}
        for sim in ("ngspice", "va"):
            r = run(sim, f"t5_{cdef['name']}", circ, ana, ["X1.pmon"])
            e = eye(r["x"], r["y"]["X1.pmon"], cdef["sym"], cdef["M"], T)
            rs[sim] = r
            res.append(dict(case=cdef["name"], sim=sim, laser_nm=lamL, static_oma_mW=oma, eye=e, points=r["points"],
                            time=r["time"]))
            print(f"{cdef['name']:16s} {sim:7s}: laser {lamL:.4f} nm | worst eye {e['worst']*1e3:6.1f} uW ({100*e['norm']:5.1f} % of ideal), "
                  f"OMA {e['oma']*1e3:5.0f} uW, RLM {e['rlm']:.2f} | {r['points']} pts, wall {r['time']['wall_s']:.2f} s, "
                  f"RSS {r['time']['rss_mb']:.1f} MB", flush=True)
            np.savez(f"work/t5_{cdef['name']}_{sim}.npz", t=r["x"], P=r["y"]["X1.pmon"], sym=cdef["sym"], T=T, M=cdef["M"])
        tg = np.arange(16 * T, len(cdef["sym"]) * T, T / 50)
        a = np.interp(tg, rs["ngspice"]["x"], rs["ngspice"]["y"]["X1.pmon"]); b = np.interp(tg, rs["va"]["x"], rs["va"]["y"]["X1.pmon"])
        dif = float(np.sqrt(np.mean((a - b) ** 2)) / np.mean(a))
        res[-1]["ng_vs_va_rms_rel"] = dif; res[-2]["ng_vs_va_rms_rel"] = dif
        print(f"{'':16s} ngspice vs verilog-a-sim waveform: RMS dP/Pmean {dif:.1e}")
    json.dump(res, open("t5_eyes.json" if not only else "t5_eyes_partial.json", "w"), indent=1)
