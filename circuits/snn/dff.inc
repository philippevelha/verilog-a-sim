* A master-slave D flip-flop from two sample-and-hold stages, the structure of a transistor DFF:
* the master capacitor tracks `d` while clk is high and holds it from the falling edge; the slave
* copies the master while qclk is high (the next low phase) and holds it through the high phase.
* So `q` is `d` as it was at the end of a high phase, valid for the whole following cycle, and
* `m` (the master, buffered) is that same value during the following low phase only.
* clk and qclk must not overlap (the decks give them a 1.5-2 ns gap).
*
* `m` exists for glitch-free gating: it changes only on clk edges, while the leak switch (qclk)
* is open. `q` changes inside the qclk window, so gating a reset with `q` near a cycle boundary
* re-resets V_m for the nanosecond the slave takes to update (seen, and fixed, while building this).
*
* Hold time: the master switch opens at clk = 2.5 V on the falling edge, before any gate that
* uses `m` (they act below clk = 1.65 V) can move `d`. At the same threshold (the first build),
* the reset began while the master was still open, so it followed the comparator partway down
* and the spike came out at 2.4-2.6 V instead of 3.3 V. The cost: the comparator's verdict is taken
* ~0.3 ns before the 125 ns integration window closes.
*
* Limitations: ideal buffers (E sources) and switches (models/snn/vswitch.va); the 10 fF sample
* capacitors are there to hold a level, not a sized design.
.subckt dff d q m clk qclk
XSm  d  mc clk  vswitch vt=2.5
Cm   mc gnd 10f IC=0
Em   m  gnd mc gnd 1
XSs  m  s  qclk vswitch
Cs   s  gnd 10f IC=0
Eq   q  gnd s gnd 1
.ends
