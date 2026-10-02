"""The neuron's input stage (Xuan et al., Fig. 2 block A) at transistor level — our design.

Function (as `models/snn/sl_stage.va`): hold the source line `sl` at V_ref, and deliver the current
the synapses push into it, I_in, scaled by 1/K, into `out` (V_m) while `clk` is high.

**Not the paper's circuit.** Fig. 2 drives the common gate of an NMOS (MN3) and a PMOS (MP1) whose
sources meet at the source line: a push-pull current conveyor, bidirectional. Near I_in = 0 that
structure has a dead zone, and the op-amp output must slew across ~1.4 V (V_ref + V_tn to
V_ref - |V_tp|) each time the current changes sign: with a few uA in the op-amp and any useful
compensation, a large part of the 124 ns integration window. So here:

  * a **bias current** I_b (PMOS source `Mib`, gate at `vbp`) flows into the source line, so the
    PMOS `Mcv` (Fig. 2's MP1) always sinks I_b + I_in > 0 for I_in > -I_b, and the op-amp never
    crosses a dead zone;
  * the op-amp is two-stage (NMOS input pair, PMOS common-source second stage, Miller capacitor),
    gate of `Mcv` at its output: it holds `sl` at V_ref;
  * `Mcv`'s current goes into an NMOS mirror, is turned round by a PMOS mirror whose output device
    is 1/K the size of its input (the scaling), and the NMOS sink `Msub` (gate at `vbs`) removes
    I_b/K. `vbs` is a **trim**: set so the output is zero at I_in = 0, as a calibration would be;
  * both output devices are **cascoded** (gates at `vbpc`, `vbnc`): uncascoded, the output moved
    by ~0.75 uA per volt of V_m, which over a 124 ns window into ~1 pF is an extra leak toward
    ~1.5 V of ~9 % of the distance per cycle -- against the intended beta = 0.6;
  * the output is **steered**: to `out` while clk is high, to `dump` (held at V_ref) otherwise, so
    the mirrors stay biased and the line stays held between integration windows.

Sizes are ours. Long channels (L = 2 um) in the mirrors keep channel-length modulation small; the
output devices still see V_m move from ~0.4 to ~2.4 V, which is the residual error to measure.
"""

# (W, L) in metres
OA_IN = (4e-6, 1e-6)      # op-amp input pair, NMOS
OA_LOAD = (4e-6, 1e-6)    # op-amp first-stage mirror, PMOS
OA_TAIL = (2e-6, 1e-6)    # op-amp tail, NMOS, gate vbn
OA_P2 = (8e-6, 0.4e-6)    # second stage, PMOS
OA_N2 = (2e-6, 1e-6)      # second-stage current source, NMOS, gate vbn
CONV = (10e-6, 0.4e-6)    # the conveyor PMOS (Fig. 2's MP1)
IB = (4e-6, 2e-6)         # bias current source into the line, PMOS, gate vbp
MIR_N = (12e-6, 2e-6)      # NMOS mirror (diode and copy)
MIR_P = (24e-6, 2e-6)      # PMOS mirror input (diode); the output device is W/K
SUB = (12e-6, 2e-6)        # the I_b/K sink, NMOS, gate vbs (W scaled by 1/K too)
CAS_P = (24e-6, 0.4e-6)   # output cascode, PMOS, gate vbpc (W scaled by 1/K)
CAS_N = (12e-6, 0.45e-6)   # sink cascode, NMOS, gate vbnc (W scaled by 1/K)
STEER_N = (2e-6, 0.45e-6)
STEER_P = (4e-6, 0.40e-6)
CC = "200f"               # Miller capacitor


def fingers(w, wmax=10e-6):
    """Fingers a device of total width `w` needs to stay within the PDK's width per finger."""
    return max(1, -(-int(round(w * 1e9)) // int(round(wmax * 1e9))))


def lines(t, k, p="is"):
    """The input stage between nets sl, vref, out (V_m), dump, clk, clkb, with biases vbn, vbp,
    vbs and supply vdd. `p` prefixes every device and internal net, so two stages can coexist."""
    n = lambda s: f"{p}_{s}"  # noqa: E731 -- internal net
    w_out = MIR_P[0] / k
    w_sub = SUB[0] / k
    return [
        f"* ===== input stage (ours, see circuits/snn/transistor/input_stage.py), K = {k}",
        "* op-amp: holds sl at vref (NMOS input pair, PMOS second stage, Miller capacitor)",
        t.nmos(n("M1"), n("a1"), "sl", n("tail"), "gnd", *OA_IN),
        t.nmos(n("M2"), n("o1"), "vref", n("tail"), "gnd", *OA_IN),
        t.pmos(n("M3"), n("a1"), n("a1"), "vdd", "vdd", *OA_LOAD),
        t.pmos(n("M4"), n("o1"), n("a1"), "vdd", "vdd", *OA_LOAD),
        t.nmos(n("M5"), n("tail"), "vbn", "gnd", "gnd", *OA_TAIL),
        t.pmos(n("M6"), n("g"), n("o1"), "vdd", "vdd", *OA_P2),
        t.nmos(n("M7"), n("g"), "vbn", "gnd", "gnd", *OA_N2),
        f"C{p}_cc {n('o1')} {n('g')} {CC}",
        "* bias current into the line, and the conveyor PMOS (Fig. 2's MP1)",
        t.pmos(n("Mib"), "sl", "vbp", "vdd", "vdd", *IB),
        t.pmos(n("Mcv"), n("nd"), n("g"), "sl", "vdd", *CONV),
        "* NMOS mirror, PMOS mirror scaled by 1/K, and the trimmed I_b/K sink",
        t.nmos(n("Mn1"), n("nd"), n("nd"), "gnd", "gnd", *MIR_N, fingers(MIR_N[0])),
        t.nmos(n("Mn2"), n("pd"), n("nd"), "gnd", "gnd", *MIR_N, fingers(MIR_N[0])),
        t.pmos(n("Mp1"), n("pd"), n("pd"), "vdd", "vdd", *MIR_P, fingers(MIR_P[0])),
        t.pmos(n("Mp2"), n("pc"), n("pd"), "vdd", "vdd", w_out, MIR_P[1], fingers(w_out)),
        t.pmos(n("Mp2c"), n("o"), "vbpc", n("pc"), "vdd", CAS_P[0] / k, CAS_P[1],
               fingers(CAS_P[0] / k)),
        t.nmos(n("Msubc"), n("o"), "vbnc", n("sc"), "gnd", CAS_N[0] / k, CAS_N[1],
               fingers(CAS_N[0] / k)),
        t.nmos(n("Msub"), n("sc"), "vbs", "gnd", "gnd", w_sub, SUB[1], fingers(w_sub)),
        "* output steering: to out while clk is high, to dump otherwise",
        t.nmos(n("Ms1"), n("o"), "clk", "out", "gnd", *STEER_N),
        t.pmos(n("Ms2"), n("o"), "clkb", "out", "vdd", *STEER_P),
        t.nmos(n("Ms3"), n("o"), "clkb", "dump", "gnd", *STEER_N),
        t.pmos(n("Ms4"), n("o"), "clk", "dump", "vdd", *STEER_P),
        "* ===== end input stage",
    ]
