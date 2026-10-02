"""The Fig. 5 single-neuron deck with the neuron built from transistors, one block at a time.

The neuron is the paper's (Xuan et al., IJCNN 2026, Figs. 2-3), refractory off, in the form the
MNIST decks use: one D flip-flop, reset `rs = por OR (m0 AND clk low) OR (q0 AND clk)`. Each block
can be behavioural (the models/snn/ parts, as in circuits/snn/fig5_neuron.net) or transistor-level
in the technology `tech.py` names; `--blocks` picks which are transistors:

  switches  the leak switch (V_m to C_L on qclk), C_L's discharge (clk) and the reset switch
            (V_m to V_reset on rs). The leak switch is a transmission gate, not the paper's single
            NMOS (MN9): V_m reaches ~2.4 V, and an NMOS with 3.3 V on its gate and its bulk at ground
            cannot pass much above ~2.2 V, so the charge sharing that sets beta would be wrong near
            threshold. The discharge (to ground) and the reset (to 1.0 V) are NMOS, as in Fig. 2.
  comparator  V_m against V_th: a five-transistor differential comparator (NMOS input pair, the
            only kind with headroom at V_th = 2.4 V on a 3.3 V supply; PMOS mirror load; tail from a
            global bias V_bn = 0.8 V, ~4.4 uA) and two inverters for a full-swing logic output. Not
            the paper's Schmitt trigger, whose threshold is set indirectly through V_bias1 and needs
            sizes the paper does not give; here V_th is an explicit reference, and there is no
            hysteresis (the flip-flop only reads the comparator at the end of a high phase, where
            V_m is settled).
  dff       the D flip-flop and the reset logic. The flip-flop is the master-slave structure the
            behavioural one (circuits/snn/dff.inc) mimics: a transmission gate on clk/clkb onto a
            10 fF hold capacitor, buffered by two inverters (m0), then the same on qclk/qclkb (q0,
            the spike). The reset is rs = (m0 AND clk low) OR (q0 AND clk) -- neuron_logic.va's,
            refractory off; its power-on term is dropped (por is 0 in these decks) -- as one CMOS
            AND-OR-INVERT gate and an inverter, with clkb as "clk low".

  input     the input stage (block A): holds the source line at V_ref and delivers I_in / K into
            V_m while clk is high. Our design, not Fig. 2's push-pull conveyor (dead zone at
            I_in = 0): a biased PMOS conveyor under a two-stage op-amp, mirrors scaled by 1/K,
            cascoded outputs, a trimmed bias sink, steering onto V_m or a follower of V_m -- see
            input_stage.py, whose module docstring and constants carry the reasoning and the
            measured DC accuracy. Its biases (vbp, vbpc, vbnc, vbs) are global sources.

The sizes are ours: the paper gives none. The synapses are the access-transistor 2T2R cells of
`fig5_access.net` (same W/L). Complementary clocks clkb/qclkb are global sources (clock generation
is outside the neuron).

    python circuits/snn/transistor/gen_neuron.py                 # every block: fig5_tx.net
    python circuits/snn/transistor/gen_neuron.py --blocks switches,comparator \\
        --out circuits/snn/transistor/fig5_tx_cmp.net               # a rung of the ladder
    va-cli sim circuits/snn/transistor/fig5_tx.net --model models/snn \\
        --model models/sg13g2/psp103/psp103.va --tran --report clk,wl1,vm,spk

The committed decks are the ladder the README's table measures: fig5_tx_none (no block),
fig5_tx_sw, fig5_tx_cmp, fig5_tx_dff (each adding one block), fig5_tx_in (the input stage
alone) and fig5_tx (all four).
"""
import argparse
import os
import re

import input_stage
import tech

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "fig5_neuron.net")
BLOCKS = ["input", "switches", "comparator", "dff"]

# Device sizes (m). Ours: the paper gives none.
SW_N = (2e-6, 0.45e-6)  # NMOS switch
SW_P = (4e-6, 0.40e-6)  # PMOS half of a transmission gate
ACCESS = (10e-6, 0.45e-6)  # the 2T2R access transistor, as fig5_access.net
CMP_IN = (4e-6, 1e-6)  # comparator input pair (NMOS)
CMP_LOAD = (4e-6, 1e-6)  # comparator mirror load (PMOS)
CMP_TAIL = (2e-6, 1e-6)  # comparator tail (NMOS, gate at V_bn)
INV_N = (1e-6, 0.45e-6)  # inverter NMOS
INV_P = (2e-6, 0.40e-6)  # inverter PMOS
TG_N = (1e-6, 0.45e-6)  # flip-flop transmission gate NMOS
TG_P = (2e-6, 0.40e-6)  # flip-flop transmission gate PMOS


def power_up(line):
    """A DC source as a 1 ns ramp from 0 V. Under UIC every node starts at 0, and a gate or bulk on
    a source that is already at its value at t = 0 must charge PSP103's internal gate and bulk
    nodes through tens of ohms into femtofarads: a real transient faster than the step controller's
    floor (it ended the run at t = 5e-16 s, for a PMOS bulk on vdd and for the comparator's gates on
    vth and vbn). A real chip's supplies and references power up too; the first decision is at
    ~125 ns, so the ramp is invisible in every sampled value."""
    m = re.match(r"^(V\w+\s+\S+\s+\S+\s+)DC\s+([-\d.eE+]+)\s*$", line)
    if not m or float(m.group(2)) == 0.0:
        return line
    return f"{m.group(1)}PWL(0 0 1n {m.group(2)})"


def complement(name, node, delay, width, period, tstop, vdd=3.3, edge=1e-9):
    """The complement of PULSE(0 vdd delay edge edge width period) as a PWL source that starts at
    0 V and powers up to vdd over the first `edge` (see power_up), then falls when the pulse rises
    and rises when it falls, until `tstop`."""
    pts = [(0.0, 0.0), (edge, vdd)]
    t = delay
    while t < tstop:
        pts += [(t, vdd), (t + edge, 0.0), (t + edge + width, 0.0), (t + 2 * edge + width, vdd)]
        t += period
    return f"{name} {node} gnd PWL(" + " ".join(f"{a * 1e9:.6g}n {v:g}" for a, v in pts) + ")"


def inverter(t, name, a, y):
    """A CMOS inverter `a` -> `y` on the vdd/gnd rails."""
    return [t.nmos(name + "n", y, a, "gnd", "gnd", *INV_N),
            t.pmos(name + "p", y, a, "vdd", "vdd", *INV_P)]


def neuron(t, blocks, k):
    """The neuron's lines, inline (so `vm` and `spk` keep their names)."""
    out = ["* ===== neuron, blocks as transistors: " + (", ".join(sorted(blocks)) or "none")]
    if "input" in blocks:
        out.extend(input_stage.lines(t, float(k), out="vm"))
    else:
        out.append("* input stage (block A + clk gate): behavioural")
        out.append(f"Xin   sl vref vm clk sl_stage k={k}")
    out.append("* membrane and leak (block B): beta = C_M / (C_M + C_L) from charge sharing")
    out.append("CM    vm  gnd 998f IC=0")
    out.append("CL    cl  gnd 660f IC=0")
    if "switches" in blocks:
        out.append("* leak: transmission gate V_m <-> C_L on qclk; discharge C_L on clk")
        out.append(t.nmos("Mlkn", "vm", "qclk", "cl", "gnd", *SW_N))
        out.append(t.pmos("Mlkp", "vm", "qclkb", "cl", "vdd", *SW_P))
        out.append(t.nmos("Mdis", "cl", "clk", "gnd", "gnd", *SW_N))
    else:
        out.append("XSlk  vm  cl  qclk vswitch")
        out.append("XSdis cl  gnd clk vswitch")
    if "comparator" in blocks:
        out.append("* comparator (block C): NMOS-input 5T comparator against V_th, two inverters")
        out.append(t.nmos("Mc1", "cd1", "vm", "ctail", "gnd", *CMP_IN))
        out.append(t.nmos("Mc2", "co1", "vth", "ctail", "gnd", *CMP_IN))
        out.append(t.pmos("Mc3", "cd1", "cd1", "vdd", "vdd", *CMP_LOAD))
        out.append(t.pmos("Mc4", "co1", "cd1", "vdd", "vdd", *CMP_LOAD))
        out.append(t.nmos("Mc5", "ctail", "vbn", "gnd", "gnd", *CMP_TAIL))
        out.extend(inverter(t, "Mci1", "co1", "cmpb"))
        out.extend(inverter(t, "Mci2", "cmpb", "cmp"))
    else:
        out.append("* comparator (block C): behavioural")
        out.append("Xcmp  vm vth cmp comparator")
    if "dff" in blocks:
        out.append("* D flip-flop (block D): master TG on clk -> hold cap -> 2 inverters (m0);")
        out.append("* slave TG on qclk -> hold cap -> 2 inverters (q0, the spike)")
        out.append(t.nmos("Mfm1", "cmp", "clk", "fmc", "gnd", *TG_N))
        out.append(t.pmos("Mfm2", "cmp", "clkb", "fmc", "vdd", *TG_P))
        out.append("Cfm   fmc gnd 10f IC=0")
        out.extend(inverter(t, "Mfmi1", "fmc", "fmb"))
        out.extend(inverter(t, "Mfmi2", "fmb", "m0"))
        out.append(t.nmos("Mfs1", "m0", "qclk", "fsc", "gnd", *TG_N))
        out.append(t.pmos("Mfs2", "m0", "qclkb", "fsc", "vdd", *TG_P))
        out.append("Cfs   fsc gnd 10f IC=0")
        out.extend(inverter(t, "Mfsi1", "fsc", "fsb"))
        out.extend(inverter(t, "Mfsi2", "fsb", "q0"))
        out.append("Espk  spk gnd q0 gnd 1")
        out.append("* reset logic: rsb = NOT((m0 AND clkb) OR (q0 AND clk)), rs = NOT rsb")
        out.append(t.nmos("Mra1", "rsb", "m0", "ra", "gnd", *INV_N))
        out.append(t.nmos("Mra2", "ra", "clkb", "gnd", "gnd", *INV_N))
        out.append(t.nmos("Mrb1", "rsb", "q0", "rb", "gnd", *INV_N))
        out.append(t.nmos("Mrb2", "rb", "clk", "gnd", "gnd", *INV_N))
        out.append(t.pmos("Mrp1", "rp", "m0", "vdd", "vdd", *INV_P))
        out.append(t.pmos("Mrp2", "rp", "clkb", "vdd", "vdd", *INV_P))
        out.append(t.pmos("Mrp3", "rsb", "q0", "rp", "vdd", *INV_P))
        out.append(t.pmos("Mrp4", "rsb", "clk", "rp", "vdd", *INV_P))
        out.extend(inverter(t, "Mri", "rsb", "rs"))
    else:
        out.append("* D flip-flop (block D): behavioural")
        out.append("Xff0  cmp q0 m0 clk qclk dff")
        out.append("Espk  spk gnd q0 gnd 1")
        out.append("* reset logic: behavioural (refractory off)")
        out.append("Xlog  q0 gnd gnd gnd gnd m0 gnd clk por rs hz neuron_logic refr=0")
    if "switches" in blocks:
        out.append("* reset switch: NMOS V_m -> V_reset on rs")
        out.append(t.nmos("Mrst", "vm", "rs", "vres", "gnd", *SW_N))
    else:
        out.append("XSrst vm  vres rs vswitch")
    out.append("* ===== end neuron")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--blocks", default=",".join(BLOCKS),
                    help="comma-separated transistor blocks: " + ", ".join(BLOCKS) + " (or none)")
    ap.add_argument("--tech", default="sg13g2_hv")
    ap.add_argument("--out", default=os.path.join(HERE, "fig5_tx.net"))
    a = ap.parse_args()
    blocks = set() if a.blocks in ("", "none") else set(a.blocks.split(","))
    unknown = blocks - set(BLOCKS)
    if unknown:
        raise SystemExit(f"unknown block(s): {', '.join(sorted(unknown))}")
    t = tech.get(a.tech)

    src = open(SRC).read().splitlines()
    k = next(re.search(r"k=([\d.]+)", l).group(1) for l in src if l.startswith("Xin "))
    out = [
        "* GENERATED by circuits/snn/transistor/gen_neuron.py -- do not edit.",
        "* circuits/snn/fig5_neuron.net with 2T2R access transistors and the neuron's",
        f"* {', '.join(sorted(blocks)) or 'no'} block(s) as {t.name} transistors; the rest behavioural.",
        "* The neuron is the MNIST decks' form (refractory off, one flip-flop).",
        "*",
        "*   va-cli sim circuits/snn/transistor/fig5_tx.net --model models/snn \\",
        "*       --model models/sg13g2/psp103/psp103.va --tran --report clk,wl1,vm,spk",
        "*   python circuits/snn/check_snn.py fig5 <that output>",
        ".include tech.inc",
        ".include ../dff.inc",
    ]
    skip_neuron = False
    for line in src:
        if line.startswith(".include"):
            continue
        if line.startswith("* --- the neuron's per-deck settings"):
            skip_neuron = True
            continue
        if skip_neuron and (line.startswith("Xin ") or line.startswith("Xlog ")):
            continue
        m = re.match(r"^(XA\w+)\s+(\S+)\s+(\S+)\s+(\S+)\s+vswitch\b", line)
        if m:
            inst, cell, sl, wl = m.groups()
            out.append(t.nmos(inst[1:], cell, wl, sl, "gnd", *ACCESS))
            continue
        if line.startswith("Vqclk "):
            out.append(line)
            # Complementary clocks, and the supply, all starting from 0 V. A PMOS gate or bulk on
            # a source that is already at 3.3 V at t = 0 has to charge PSP103's internal gate and
            # bulk nodes from the cold (UIC) start through tens of ohms into femtofarads: a real
            # transient faster than the step controller's floor, which ended the run at
            # t = 5e-16 s. So the supply ramps up over 1 ns, as a real one does, and the
            # complements are written to start low: qclkb is a pulse, high exactly when qclk is
            # low from the first falling edge of qclk on (low before it, for the first 122 ns,
            # with V_m and C_L both still at 0). clkb is the exact complement of clk after a 1 ns
            # power-up (complement()): written as a pulse starting low too, it left clk and clkb
            # both low for the first 124 ns, which turned on the PMOS half of every gate switched
            # by clk -- the input stage's steering put 0.26 V onto V_m before the first window.
            out.append("Vqclkb qclkb gnd PULSE(0 3.3 122n 1n 1n 129n 250n)")
            out.append(complement("Vclkb", "clkb", 124.5e-9, 124e-9, 250e-9, 3.75e-6))
            out.append(power_up("Vdd   vdd  gnd  DC 3.3"))
            out.append(power_up("Vth   vth  gnd  DC 2.4"))
            out.append(power_up("Vbn   vbn  gnd  DC 0.8"))
            if "input" in blocks:
                for name, v in input_stage.BIAS.items():
                    out.append(power_up(f"V{name:<5} {name:<5} gnd DC {v}"))
            continue
        if line.startswith(".tran"):
            out.extend(neuron(t, blocks, k))
        out.append(power_up(line))
    with open(a.out, "w", newline="\n") as fh:
        fh.write("\n".join(out) + "\n")
    t.write_inc(os.path.join(os.path.dirname(a.out), "tech.inc"))
    print(f"wrote {a.out}")


if __name__ == "__main__":
    main()
