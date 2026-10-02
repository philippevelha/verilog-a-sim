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
  * the output is **steered**: to `out` while clk is high, to `dump` otherwise, so the mirrors
    stay biased and the line stays held between integration windows;
  * `dump` is held at V(out) by a **follower** (a PMOS-input five-transistor OTA in unity
    feedback). Held at V_ref instead, the output node's capacitance C_o (~24 fF even with narrow
    cascodes: their drains, the steering switches', and the clock feedthrough onto them) shared
    charge with C_M at every window start -- in the Fig. 5 deck a 25-40 mV step toward V_ref, an
    extra leak of ~2.5 % of (V_ref - V_m) per cycle. The follower only has to be right at window
    start, where V_m is always 0-1.6 V (after the leak or the reset), the range a PMOS input pair
    covers; a 65 mV following error under its +-13 uA load costs ~1.5 mV.

Sizes are ours. Long channels (L = 2 um) in the mirrors keep channel-length modulation small; the
output devices still see V_m move from ~0.4 to ~2.4 V, which is the residual error to measure.
"""

# (W, L) in metres
OA_IN = (4e-6, 1e-6)      # op-amp input pair, NMOS
OA_LOAD = (4e-6, 1e-6)    # op-amp first-stage mirror, PMOS
OA_TAIL = (2e-6, 1e-6)    # op-amp tail, NMOS, gate vbn
# Second stage, PMOS. Sized so its current density matches the first-stage load's when the
# inputs are equal: (W/L)_6 / (W/L)_4 = 2 (W/L)_7 / (W/L)_5 = 2. At (8 um, 0.4 um), a ratio of 5,
# the op-amp had a 1.5 mV systematic offset: the line sat at 1.6515 V, 1.6 % off V_s = 0.1 V.
OA_P2 = (8e-6, 1e-6)
OA_N2 = (2e-6, 1e-6)      # second-stage current source, NMOS, gate vbn
CONV = (10e-6, 0.4e-6)    # the conveyor PMOS (Fig. 2's MP1)
IB = (4e-6, 2e-6)         # bias current source into the line, PMOS, gate vbp
MIR_N = (12e-6, 2e-6)      # NMOS mirror (diode and copy)
MIR_P = (24e-6, 2e-6)      # PMOS mirror input (diode); the output device is W/K
SUB = (12e-6, 2e-6)        # the I_b/K sink, NMOS, gate vbs (W scaled by 1/K too)
# The output cascodes are narrower than the mirrors they cascode: their drains, and the steering
# switches', are the capacitance C_o of the output node `o`. During a window C_o sits in parallel
# with C_M (the integration rate drops by C_o / C_M), and at each window start it shares charge
# with C_M from V_ref, where it waited between windows (an extra leak toward V_ref of the same
# fraction). At the mirrors' widths (24 um / 12 um, before the 1/K) C_o was ~34 fF, 3.4 % of C_M,
# measured as a 40 mV step in V_m at each window start in the Fig. 5 deck.
CAS_P = (8e-6, 0.4e-6)    # output cascode, PMOS, gate vbpc (W scaled by 1/K)
CAS_N = (4e-6, 0.45e-6)   # sink cascode, NMOS, gate vbnc (W scaled by 1/K)
# The dump follower: a PMOS-input five-transistor OTA in unity feedback, tail from vbp at twice
# Mib's width (~40 uA, so it can absorb the +-13 uA the output delivers between windows).
# Small: its input gate hangs on V_m all the time, so its capacitance adds to C_M (at 8 um x 1 um
# the integration rate dropped by a further 1 %).
BUF_IN = (2e-6, 0.5e-6)   # PMOS input pair
BUF_LOAD = (4e-6, 1e-6)   # NMOS mirror load
BUF_TAIL = (8e-6, 2e-6)   # PMOS tail, gate vbp
STEER_N = (2e-6, 0.45e-6)
STEER_P = (4e-6, 0.40e-6)
CC = "200f"               # Miller capacitor

# The design point (scan_input_stage.py, then scan_point.py), with the biases as global sources.
# Cascode biases: chosen on a 5x4 grid (at the earlier, wider cascodes), then re-checked at five
# points for the narrow ones, on the worst |I_out| at I_in = 0 over V_m 0.45-2.4 V (0.24 uA) and
# the gain's flatness. VBS is the trim for that point (I_out = 0 at I_in = 0, V_m = 1.5 V).
BIAS = {"vbp": 1.8, "vbpc": 1.7, "vbnc": 1.2, "vbs": 0.8703}
# The output mirror is sized for K * GAIN_TRIM, not K: with W/K the measured gain is 1.443 at
# I_in = +10 uA and 1.464 at -10 uA (the mirrors' finite output resistance and the conveyor's
# current-dependent V_DS), 1.1 % above 1/K = 1.4368 on average. With the trim it is 1.429 / 1.448,
# flat to 0.1 % over V_m; the +/-0.7 % that remains is curvature in I_in (a second-order term).
#
# During a window the output node's capacitance C_o (~12 fF: the cascode drains and the steering
# switches) sits in parallel with C_M, so V_m integrates I_out / (C_M + C_o): the mirror is made
# stronger by the same factor, C_O_TRIM = 1 + C_o / C_M, measured from the integration slope in
# the Fig. 5 deck (1.8 % below the behavioural stage's, of which 0.5 % is the gain above). An
# estimate for C_M = 998 fF; it would change with C_M or the output devices.
C_O_TRIM = 1.0125
GAIN_TRIM = 1.0112 / C_O_TRIM


def fingers(w, wmax=10e-6):
    """Fingers a device of total width `w` needs to stay within the PDK's width per finger."""
    return max(1, -(-int(round(w * 1e9)) // int(round(wmax * 1e9))))


def lines(t, k, p="is", gain_trim=GAIN_TRIM, out="out", dump=None):
    """The input stage between nets sl, vref, `out` (V_m), `dump`, clk, clkb, with biases vbn, vbp,
    vbs, vbpc, vbnc and supply vdd. `p` prefixes every device and internal net, so two stages can
    coexist. With `dump` None the output waits between windows on an internal node that a
    follower holds at V(out) (see the module docstring); given a net, it waits there."""
    n = lambda s: f"{p}_{s}"  # noqa: E731 -- internal net
    k = k * gain_trim  # the device ratio; the sink is trimmed by vbs, so it scales the same way
    w_out = MIR_P[0] / k
    w_sub = SUB[0] / k
    body = [
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
    ]
    if dump is None:
        dump = n("dump")
        body += [
            "* dump follower: holds the waiting output at V(out), so the output node has nothing",
            "* to share with C_M when it is switched onto it (PMOS-input OTA, unity feedback)",
            t.pmos(n("Mb1"), n("b1"), out, n("bt"), "vdd", *BUF_IN),
            t.pmos(n("Mb2"), dump, dump, n("bt"), "vdd", *BUF_IN),
            t.nmos(n("Mb3"), n("b1"), n("b1"), "gnd", "gnd", *BUF_LOAD),
            t.nmos(n("Mb4"), dump, n("b1"), "gnd", "gnd", *BUF_LOAD),
            t.pmos(n("Mb5"), n("bt"), "vbp", "vdd", "vdd", *BUF_TAIL),
        ]
    return body + [
        "* output steering: to out while clk is high, to dump otherwise",
        t.nmos(n("Ms1"), n("o"), "clk", out, "gnd", *STEER_N),
        t.pmos(n("Ms2"), n("o"), "clkb", out, "vdd", *STEER_P),
        t.nmos(n("Ms3"), n("o"), "clkb", dump, "gnd", *STEER_N),
        t.pmos(n("Ms4"), n("o"), "clk", dump, "vdd", *STEER_P),
        "* ===== end input stage",
    ]
