"""The V_m kick of one word-line edge while sl_stage.va is still integrating, PSP103 against the
fitted switch, on a bench small enough to change one thing at a time.

    python circuits/snn/transistor/edge_kick.py [--va target/release/va-cli] [--nsyn 4]

One source line, held at V_ref by sl_stage.va into CM = 998 fF as in the crossbar's neurons, with
`--nsyn` 2T2R synapses (both cells at level 0) on one word line. The word line rises at 250 ns
and falls at 500 ns, each a 1 ns ramp starting at the instant clk is halfway down, the crossbar's
timing. The kick is V_m's change over the 50 ps after each edge starts: by then sl_stage.va has
stopped passing current, so only the first few percent of the gate swing reach V_m.

Rows: PSP103; rung C and rungs B+C from switch_fit.json; then rungs B+C with PSP103's gate
resistance (~73 Ohm for W/L = 10/0.45 um, ngcon = 2: RSHG * W / (3 NGCON) / (NGCON L) + NF * RGO
+ RINT / (W L)) and a 3 fF junction capacitance on each cell node, separately and together.
These are what the switch would need to reproduce PSP103's kicks; mos_switch.va has neither.
"""
import argparse
import json
import os
import re
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
PSP = ("XA{s} c{s} wl sl gnd hvn_w10000n_l450n_ng1 w=1e-05 l=4.5e-07 nf=1 mult=1 as=3.4e-12 "
       "ad=3.4e-12 ps=2.068e-05 pd=2.068e-05 ngcon=2 delvto=0 factuo=1")


def deck(nsyn, switch, rg=None, cj=None):
    lines = ["* edge kick bench (edge_kick.py)", "Vvp vp gnd PWL(0 0 1n 1.75)",
             "Vvn vn gnd PWL(0 0 1n 1.55)", "Vref vref gnd PWL(0 0 1n 1.65)",
             "Vclk clk gnd PULSE(0 3.3 124.5n 1n 1n 124n 250n)",
             "Vwl wl gnd PWL(0 0 250n 0 251n 3.3 500n 3.3 501n 0)",
             "Xin sl vref vm clk sl_stage k=1.06165769", "CM vm gnd 998f IC=1"]
    if switch is None:
        lines.insert(1, f".include {os.path.join(HERE, 'tech.inc')}")
    gate = "wl" if rg is None else "wlg"
    if rg is not None:
        lines.append(f"RG wl wlg {rg / (2 * nsyn)!r}")  # every gate's own R_g, in parallel
    for j in range(nsyn):
        for br, line in (("p", "vp"), ("n", "vn")):
            s = f"{br}{j}"
            lines.append(f"XG{s} {line} c{s} rram level=0")
            lines.append(PSP.format(s=s) if switch is None
                         else f"XA{s} c{s} sl {gate} mos_switch {switch}")
            if cj is not None:
                lines.append(f"CJ{s} c{s} gnd {cj!r}")
    return "\n".join(lines + [".tran 0.5n 600n UIC", ".end"]) + "\n"


def kicks(va, text, psp):
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "edge.net")
        open(path, "w").write(text)
        models = ["--model", os.path.join(ROOT, "models", "snn")]
        if psp:
            models += ["--model", os.path.join(ROOT, "models", "sg13g2", "psp103", "psp103.va")]
        r = subprocess.run([va, "sim", path, *models, "--tran", "--report", "vm"],
                           capture_output=True, text=True, check=True)
    pts = [(float(t), float(v)) for t, v in re.findall(r"^\s*t=(\S+)s\s+V\(vm\)=(\S+) V",
                                                       r.stdout, re.M)]

    def at(t):
        for (a, x), (b, y) in zip(pts, pts[1:]):
            if a <= t <= b:
                return x + (y - x) * (t - a) / (b - a) if b > a else y
        raise SystemExit(f"t = {t} outside the run")
    return [at(e + 50e-12) - at(e) for e in (250e-9, 500e-9)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--va", default=os.path.join(ROOT, "target", "release", "va-cli"))
    ap.add_argument("--nsyn", type=int, default=4)
    a = ap.parse_args()
    f = json.load(open(os.path.join(HERE, "switch_fit.json")))
    c = f"beta={f['beta']!r} vt={f['vt']!r} ns={f['ns']!r} cov={f['cov']!r} cch={f['cch']!r}"
    bc = (f"beta={f['beta']!r} vt={f['vt_b']!r} ns={f['ns']!r} kds={f['kds']!r} "
          f"cov={f['cov_b']!r} cch={f['cch_b']!r}")
    rows = [("PSP103", None, None, None), ("rung C", c, None, None), ("rungs B+C", bc, None, None),
            ("rungs B+C, R_g 73 Ohm", bc, 73.0, None),
            ("rungs B+C, 3 fF cell node", bc, None, 3e-15),
            ("rungs B+C, R_g 73 Ohm and 3 fF", bc, 73.0, 3e-15)]
    print(f"V_m kick over the first 50 ps of the edge, {a.nsyn} synapses on the word line:")
    ref = None
    for name, sw, rg, cj in rows:
        k = kicks(a.va, deck(a.nsyn, sw, rg, cj), sw is None)
        ref = ref or k
        print(f"  {name:32s} rising {k[0] * 1e6:+8.1f} uV ({k[0] / ref[0] - 1:+6.1%})   "
              f"falling {k[1] * 1e6:+8.1f} uV ({k[1] / ref[1] - 1:+6.1%})")


if __name__ == "__main__":
    main()
