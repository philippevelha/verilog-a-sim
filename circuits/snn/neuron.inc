* The analog spiking neuron of Xuan et al., IJCNN 2026 (references/, Figs. 2-3), at the level of
* its functional diagram. Included by fig4_neuron.net and fig5_neuron.net, which supply the
* synapses (on `sl`), the bias and clock nets, and `por`.
*
* All state is on capacitors, as in the paper's circuit: the membrane C_M, the leak capacitor
* C_L, and the flip-flops (two sample capacitors each). The Verilog-A parts (models/snn/) are
* memoryless.
*
* Nets used: sl vref vres clk qclk por (in), vm spk (out).

* --- input stage (block A + the clk switch): source line held at V_ref, I_inj = I_in / K into V_m
*     while clk is high. K is set by each deck on its own copy of this line (see the deck header).

* --- membrane and leak (block B): C_L shares charge with C_M in the low phase (qclk) and is
*     emptied in the high phase (clk), so beta = C_M / (C_M + C_L) comes out of the circuit.
CM    vm  gnd 998f IC=0
XSlk  vm  cl  qclk vswitch
CL    cl  gnd 660f IC=0
XSdis cl  gnd clk vswitch

* --- comparator (block C) and D flip-flop (block D): the flip-flop latches the comparator at the
*     end of the high phase; its output, the spike, is high for the whole next cycle (Eq. 6).
Vth   vth gnd DC 2.4
Xcmp  vm vth cmp comparator
Xff0  cmp q0 m0 clk qclk dff
Espk  spk gnd q0 gnd 1
* q0 delayed by 1..4 cycles (the refractory timer), and a fifth master for the end-of-period reset
Xff1  q0 q1 m1 clk qclk dff
Xff2  q1 q2 m2 clk qclk dff
Xff3  q2 q3 m3 clk qclk dff
Xff4  q3 q4 m4 clk qclk dff
Xff5  q4 q5 m5 clk qclk dff

* --- reset (to V_reset) and refractory hold (to 0), Eqs. (7) and (9)
XSrst vm  vres rs vswitch
XShz  vm  gnd  hz vswitch
