"""Fit the access switch's rung-A model (models/snn/mos_switch.va) to the SG13G2 PSP103 access
transistor at the bias it lives at in the 2T2R crossbar.

    python circuits/snn/transistor/fit_switch.py [--va target/release/va-cli]

Rung A is a gate-controlled conductance, linear in V_DS:

    g(V_GS) = 1/roff + beta * ns * ln(1 + exp((V_GS - vt) / ns)),   I_DS = g * V_DS

with V_GS measured from the source-line terminal. It is the deep-triode MOS (I = beta * V_ov *
V_DS) with the overdrive smoothed into weak inversion. Why that is enough here
(circuits/snn/transistor/README.md, "A switch fitted to the transistor"): the source line is held
at V_ref = 1.65 V, so V_SB is fixed and the body effect is a constant folded into `vt`; with the
switch on, |V_DS| <= 20 mV, so the triode V_DS^2 term moves R_on by under 1 %; and the edge charge
the transistor kicks onto the cell node settles in ~12 ns, long before the neuron integrates.

What it fits, from a DC gate sweep of the committed access device (bench_access_bias.net's card
and instance line, W/L = 10/0.45 um) with the source at --vs and the drain --vds above it:
  ns    from the steepest weak-inversion slope of ln I(V_GS) (it only shapes the switch's turn-on);
  beta  so that the current at the word line's high level is PSP103's exactly: that is the one
        bias the neuron integrates (the switch is fully on all through the integration window);
  vt    least squares on the relative current error over the rest of strong inversion (I above
        10 % of its value at the high level), which shapes the switch's turn-on.
It then reports R_on against PSP103 at the word line's high level, the largest error over the
fitted range, and R_on at +-20 mV, which is what rung A's linearity assumption costs. It writes
switch_fit.json next to itself.

Limitations: one geometry, the typical corner, one source-line voltage (a different V_ref moves
vt through the body effect: refit), room temperature (the card's). The reference is our own
PSP103 run, not a reference simulator.
"""
import argparse
import json
import math
import os
import re
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
BENCH = os.path.join(HERE, "bench_access_bias.net")


def sweep(va, vs, vds, vgmax, step):
    """PSP103 drain current (A, into the drain) against V_GS, with V_DS = vds."""
    inst = next(l for l in open(BENCH) if l.startswith("XON "))
    inst = re.sub(r"^XON\s+\S+\s+\S+\s+\S+\s+\S+", "XA d g s gnd", inst.strip())
    with tempfile.TemporaryDirectory() as tmp:
        deck = os.path.join(tmp, "sweep.net")
        with open(deck, "w") as fh:
            fh.write("* access NMOS gate sweep (fit_switch.py)\n"
                     f".include {os.path.join(HERE, 'tech.inc')}\n"
                     f"Vd d gnd DC {vs + vds!r}\nVs s gnd DC {vs!r}\nVg g s DC 0\n{inst}\n"
                     f".dc Vg 0 {vgmax!r} {step!r}\n.end\n")
        r = subprocess.run([va, "sim", deck, "--model", os.path.join(ROOT, "models", "snn"),
                            "--model", os.path.join(ROOT, "models", "sg13g2", "psp103",
                                                    "psp103.va"), "--report", "Vd"],
                           capture_output=True, text=True, check=True)
    pts = [(float(a), -float(b)) for a, b in
           re.findall(r"^\s+Vg=(\S+):.*I\(Vd\)=(\S+) A", r.stdout, re.M)]
    if not pts:
        raise SystemExit("no sweep points in va-cli's output:\n" + r.stdout + r.stderr)
    return pts


def softplus(x):
    return x + math.log1p(math.exp(-x)) if x > 0 else math.log1p(math.exp(x))


def model_g(vgs, beta, vt, ns):
    return beta * ns * softplus((vgs - vt) / ns)


def fit(pts, vds, ns):
    top = pts[-1][1]
    strong = [(v, i) for v, i in pts if i > 0.1 * top]

    def beta_for(vt):
        # beta pins the current at the top gate voltage, the one point V_m integrates; vt then
        # minimizes the relative error over the rest of strong inversion
        b = top / (model_g(pts[-1][0], 1.0, vt, ns) * vds)
        return b, sum((model_g(v, b, vt, ns) * vds / i - 1) ** 2 for v, i in strong)

    lo, hi = 0.0, strong[0][0]  # vt lies below the first strong-inversion point
    g = (math.sqrt(5) - 1) / 2
    for _ in range(200):  # golden section on vt
        a, b = hi - g * (hi - lo), lo + g * (hi - lo)
        if beta_for(a)[1] < beta_for(b)[1]:
            hi = b
        else:
            lo = a
    vt = (lo + hi) / 2
    return beta_for(vt)[0], vt, strong


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--va", default=os.path.join(ROOT, "target", "release", "va-cli"))
    ap.add_argument("--vs", type=float, default=1.65, help="source-line voltage (V_ref)")
    ap.add_argument("--vds", type=float, default=0.005, help="drain above source (V)")
    ap.add_argument("--vhigh", type=float, default=3.3, help="word line high level (V)")
    a = ap.parse_args()
    vgmax = a.vhigh - a.vs
    pts = sweep(a.va, a.vs, a.vds, vgmax, 0.01)

    # ns: the steepest weak-inversion slope, d ln I / d V_GS = 1 / ns
    slopes = [(math.log(i2 / i1) / (v2 - v1), v1)
              for (v1, i1), (v2, i2) in zip(pts, pts[1:]) if i1 > 1e-12 and i2 > i1]
    ns = 1.0 / max(slopes)[0]
    beta, vt, strong = fit(pts, a.vds, ns)
    worst = max(abs(model_g(v, beta, vt, ns) * a.vds / i - 1) for v, i in strong)
    i_top = pts[-1][1]
    ron_psp = a.vds / i_top
    ron_fit = 1.0 / model_g(vgmax, beta, vt, ns)

    lin = {}
    for d in (-0.02, 0.02):
        i = sweep(a.va, a.vs, d, vgmax, vgmax)[-1][1]
        lin[d] = d / i
    print(f"PSP103 sweep: V_S = {a.vs} V, V_DS = {a.vds * 1e3:g} mV, V_GS 0..{vgmax:g} V, "
          f"{len(pts)} points ({len(strong)} in strong inversion)")
    print(f"fit:  beta = {beta:.6e} A/V^2   vt = {vt:.6f} V   ns = {ns:.6f} V")
    print(f"R_on at V_GS = {vgmax:g} V: PSP103 {ron_psp:.2f} Ohm, fit {ron_fit:.2f} Ohm "
          f"({(ron_fit / ron_psp - 1) * 100:+.3f} %)")
    print(f"largest |current error| over strong inversion: {worst * 100:.2f} %")
    print("PSP103 R_on at V_DS = -20 / +20 mV: "
          + " / ".join(f"{lin[d]:.2f}" for d in (-0.02, 0.02))
          + f" Ohm (rung A holds it at {ron_fit:.2f})")
    out = {
        "model": "models/snn/mos_switch.va (rung A)",
        "device": "SG13G2 HV NMOS W/L = 10/0.45 um through PSP103, typical corner (tech.inc)",
        "bias": {"vs": a.vs, "vds": a.vds, "vgs_max": vgmax},
        "beta": float(f"{beta:.6e}"), "vt": round(vt, 6), "ns": round(ns, 6),
        "ron_psp103": round(ron_psp, 3), "ron_fit": round(ron_fit, 3),
        "worst_strong_inversion_error": round(worst, 5),
        "ron_psp103_vds_m20mV": round(lin[-0.02], 3), "ron_psp103_vds_p20mV": round(lin[0.02], 3),
    }
    with open(os.path.join(HERE, "switch_fit.json"), "w", newline="\n") as fh:
        json.dump(out, fh, indent=2)
        fh.write("\n")


if __name__ == "__main__":
    main()
