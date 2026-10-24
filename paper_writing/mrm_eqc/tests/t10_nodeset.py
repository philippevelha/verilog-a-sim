#!/usr/bin/env python3
"""t10: the held .nodeset (verilog-a-sim 2.43.0) on the self-heated ring of t8.

mrm_eqc_ls with self_heat = 1, DC drives at the mean level of the t8 pattern, laser red of the cold
resonance at five points where the static heating law has three roots (2-8 mW). For each, .op with
no nodeset, with .nodeset V(tdt) at the hot root, and at the cold root. Before 2.43.0 every run
landed on the cold branch, because a nodeset was only Newton's starting point and the optical field
that would sustain a hot temperature starts at zero (docs/frozen-temperature-method.md). Since
2.43.0 the nodeset is held for a first solve and the hot seed reaches the hot root.

The static roots come from the python twin of the model's heating law (t8.hot_root_K's equation),
so "reaches the hot root" is checked against an independent solution of the same equation.

    python3 t10_nodeset.py        -> t10_nodeset.json        (VACLI = path to va-cli)
"""
import json, os, re, shutil, subprocess
import numpy as np
import t6_pam8_sweep as t6
import t8_eye_power as t8
from harness import deck_va

POINTS = ((2.0, 300), (4.0, 500), (4.0, 650), (8.0, 700), (8.0, 1000))   # (mW, pm red of t8's laser)

c = t8.case()
v1, v2 = float(np.mean(c["v1"])), float(np.mean(c["v2"]))
lam_c = t8.laser_cold(c)


def roots_K(lam, P):
    """Every root of the static heating law at the mean drive, as a temperature rise in K."""
    card = c["card"]; cl = 2.99792458e8
    lr = card["lam0"] + 1e-3 * (card["s1"] * (card["c1"] * v1 + card["c2"] * v1 ** 2)
                                + card["s2"] * (card["c1"] * v2 + card["c2"] * v2 ** 2))
    w1 = card["s1"] / (card["s1"] + card["s2"]); w2 = 1 - w1
    f = lambda v: card["tl0"] + card["tl1"] * v + card["tl2"] * v * v
    tl = 1e-12 / (1 / card["tl0"] + w1 * (1 / f(v1) - 1 / card["tl0"]) + w2 * (1 / f(v2) - 1 / card["tl0"]))
    g = 1 / tl + 1 / card["tau_e"]
    d = np.linspace(0, 0.215 * P * 1.05 + 1e-6, 400001)
    D = 2 * np.pi * cl * (1 / (lam * 1e-9) - 1 / ((lr + d) * 1e-9))
    F = 0.215 * P * g * g / (D * D + g * g) - d
    r = d[np.where(np.diff(np.sign(F)) != 0)[0]]
    return [float(x) / card.get("dlam_dT", 0.08) for x in r]


def op(tag, lam, P, nodeset):
    p = dict(c["card"]); p.update(FSR=c["fsr"], loss_mod=1, self_heat=1)
    circ = [("V", "a", "a", "0", f"DC {-v1:.10g}"), ("V", "b", "b", "0", f"DC {-v2:.10g}"),
            ("V", "re", "inre", "0", f"DC {np.sqrt(P):.10g}"), ("V", "im", "inim", "0", "DC 0"),
            ("V", "wl", "wl", "0", f"DC {lam:.7f}"), ("V", "h", "hp", "0", "DC 0"),
            ("X", "1", ["a", "b", "0", "hp", "0", "inre", "inim", "tre", "tim", "dre", "dim", "wl", "tdt"],
             "mrm_eqc_ls", p)]
    deck = deck_va(circ, ("op",))
    if nodeset is not None:
        deck = deck.replace(".end\n", f".nodeset V(tdt)={nodeset:.6f}\n.end\n")
    w = os.path.join(t6.HERE, "work", "t10", tag); os.makedirs(w, exist_ok=True)
    shutil.copy(os.path.join(t6.ROOT, "mrm_eqc_ls.va"), w)
    for h in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(t6.ROOT, "vasim", h), w)
    open(os.path.join(w, "deck.net"), "w").write(deck)
    o = subprocess.run([t6.VACLI, "sim", "deck.net", "--model", "."], cwd=w, capture_output=True, text=True,
                       env=dict(os.environ, VA_JIT="on"))
    m = re.search(r"V\(tdt\)\s*=\s*" + t6.NUM, o.stdout)
    return float(m.group(1)) if m else None


if __name__ == "__main__":
    # the version of the source tree the binary is assumed to be built from
    toml = open(os.path.join(t6.ROOT, "..", "..", "Cargo.toml")).read()
    ver = re.search(r'^version = "([^"]+)"', toml, re.M).group(1)
    res = []
    for P, det in POINTS:
        lam = lam_c + det * 1e-3
        r = roots_K(lam, P)
        assert len(r) == 3, (P, det, r)
        row = dict(P_mW=P, det_pm=det, laser_nm=lam, static_roots_K=r,
                   none=op(f"P{P}_d{det}_none", lam, P, None),
                   hot=op(f"P{P}_d{det}_hot", lam, P, r[2]),
                   cold=op(f"P{P}_d{det}_cold", lam, P, r[0]))
        res.append(row)
        print(f"P={P} mW det={det} pm: roots {', '.join(f'{x:.4f}' for x in r)} K | no nodeset {row['none']:.4f}"
              f"  hot seed {row['hot']:.4f} (err {1e3 * (row['hot'] - r[2]):+.2f} mK)  cold seed {row['cold']:.4f}")
    json.dump(dict(va_cli=ver, points=res), open("t10_nodeset.json", "w"), indent=1)
