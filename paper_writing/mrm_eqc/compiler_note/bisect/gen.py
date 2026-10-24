#!/usr/bin/env python3
"""Bisection set for the OpenVAF-reloaded for-loop bug (README section 5).

Each variant is mrm_eqc_rlc_loop2.va with ONE feature removed. A variant whose V(nind) is no
longer 0 in OpenVAF names a feature that the trigger needs. rl_loop.va (../issue/), which has
the loop, the explicit-current inductor and a loop-assigned value feeding L, does NOT trigger
the bug (ngspice-47 + OpenVAF-reloaded 5063fda, 2026-10-24).

    python3 gen.py                 # writes b*_*.va, b*_*.cir (ngspice), b*_*.net (verilog-a-sim)
    sh run_ngspice.sh              # Linux: compile + run all, print max |V(nind)| per variant
"""
import os, re

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = open(os.path.join(HERE, "..", "mrm_eqc_rlc_loop2.va")).read()
CIR = open(os.path.join(HERE, "..", "ng_ac_10GHz.cir")).read()
NET = open(os.path.join(HERE, "..", "va_ac_10GHz.net")).read()

LOOP = """        for (it = 0; it < 2; it = it + 1) begin
            dlsh = 0.0;
        end
"""
PARASITICS = """        I(in1)     <+ ddt(Cpad1 * V(in1));
        I(in1, sb1) <+ V(in1, sb1) / Rsi1;
        I(sb1)      <+ ddt(Csi1 * V(sb1));
"""
SEG2 = """        I(in2)     <+ ddt(Cpad2 * V(in2));
        I(in2, sb2) <+ V(in2, sb2) / Rsi2;
        I(sb2)      <+ ddt(Csi2 * V(sb2));
        I(in2, j2) <+ V(in2, j2) / Rs2;
        I(j2)      <+ ddt(Cj2 * V(j2));
"""
NEWPAR = "    parameter real Rn     = 1.0       from (0:inf);  // ohm, R1 normalisation\n"


def sub(text, old, new):
    assert text.count(old) == 1, old
    return text.replace(old, new)


VARIANTS = {
    "b0_loop2": [],                                   # control: fails in OpenVAF
    "b1_noloop": [(LOOP, "")],                        # control: should pass
    "b2_body_unused": [(LOOP, LOOP.replace("dlsh = 0.0;", "junk = 0.0;")),
                       ("real w1,", "real junk, w1,")],
    "b3_loop_first": [(LOOP, ""), ("    analog begin\n", "    analog begin\n" + LOOP)],
    "b4_const_L": [("Lind = R2 * tl / 2.0;", "Lind = Lpar;"),
                   (NEWPAR, NEWPAR + "    parameter real Lpar = 1.0e-11 from (0:inf);\n")],
    "b5_const_R2_L": [("Lind = R2 * tl / 2.0;", "Lind = Lpar;"),
                      ("R2 = R1 / ((tau_e * tl / 4.0) * (1.0 / (tau * tau) + D * D) - 1.0);", "R2 = R2par;"),
                      (NEWPAR, NEWPAR + "    parameter real Lpar = 1.0e-11 from (0:inf);\n"
                               "    parameter real R2par = 1.0 from (0:inf);\n")],
    "b6_const_g": [("g1 = -2.0 * D / (D * D + 1.0 / (tau * tau)) * dwdv1 / R1 * P_in;", "g1 = gpar;"),
                   (NEWPAR, NEWPAR + "    parameter real gpar = 1.0e-3 from (-inf:inf);\n")],
    "b7_no_parasitics": [(PARASITICS, ""), (SEG2, ""),     # only in1 -Rs1- j1 -Cj1- gnd left
                         ("electrical j1, j2, sb1, sb2, nind, il;", "electrical j1, nind, il;"),
                         ("I(out) <+ -(g1 * V(j1) + ((nseg == 2) ? g2 * V(j2) : 0.0));",
                          "I(out) <+ -(g1 * V(j1));")],
    "b8_no_Cc": [("        I(out) <+ ddt(Cc * V(out));\n", "")],
    "b9_no_ternary": [("I(out) <+ -(g1 * V(j1) + ((nseg == 2) ? g2 * V(j2) : 0.0));",
                           "I(out) <+ -(g1 * V(j1));")],
}

names = []
for name, edits in VARIANTS.items():
    va = sub(SRC, "module mrm_eqc_rlc_loop2(", f"module {name}(")
    for old, new in edits:
        va = sub(va, old, new)
    open(os.path.join(HERE, name + ".va"), "w", newline="\n").write(va)
    cir = re.sub(r"\.model m_1 mrm_eqc_rlc ", f".model m_1 {name} ", CIR)
    cir = re.sub(r"pre_osdi .*\n(pre_osdi .*\n)?", f"pre_osdi {name}.osdi\n", cir)
    cir = re.sub(r"print .*\n", "print vm(out) vm(n1#nind)\n", cir)
    open(os.path.join(HERE, name + ".cir"), "w", newline="\n").write(cir)
    open(os.path.join(HERE, name + ".net"), "w", newline="\n").write(
        sub(NET, " mrm_eqc_rlc_loop ", f" {name} "))
    names.append(name)
print("\n".join(names))
