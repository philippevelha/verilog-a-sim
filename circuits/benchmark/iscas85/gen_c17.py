"""Generate ISCAS'85 c17 in the style of external/benchmarkExt/iscas85_benchmark_circuit (c7552).

Usage (from the repository root): python circuits/benchmark/iscas85/gen_c17.py <out.net> [op|tran] [inputs]
  inputs: five 0/1 digits for inputs 1 2 3 6 7 (op only), default 10101.
"""
import sys

out = sys.argv[1]
mode = sys.argv[2] if len(sys.argv) > 2 else 'op'
bits = sys.argv[3] if len(sys.argv) > 3 else '10101'

INPUTS = [1, 2, 3, 6, 7]
OUTPUTS = [22, 23]
GATES = [  # c17.bench: out = NAND(a, b)
    (10, 1, 3),
    (11, 3, 6),
    (16, 2, 11),
    (19, 11, 7),
    (22, 10, 16),
    (23, 16, 19),
]
VDD = 1.8
R_WIRE, C_PIN = '2.224404', '2.080806f'   # c7552's per-segment wire values

# Pins of each net: _0 is the driver side; _1.. are the gate inputs it fans out to.
loads = {}
for g, a, b in GATES:
    for src in (a, b):
        loads.setdefault(src, []).append(g)
pin_of = {}   # (net, gate) -> pin name
for net, gates in loads.items():
    for k, g in enumerate(gates, start=1):
        pin_of[(net, g)] = f'g{net}_{k}'

L = []
L.append('* ISCAS\'85 c17 -- six NAND2 gates, five inputs, two outputs -- in the style of the')
L.append('* c7552 deck in external/benchmarkExt/iscas85_benchmark_circuit/: its `nand2` subcircuit,')
L.append('* its PSP103 model cards, and RC wire subcircuits with its element values (a star per net')
L.append('* here: driver pin to each load pin; c7552\'s generator also builds trees). Generated.')
L.append('*')
L.append('* c17.bench: 10=NAND(1,3) 11=NAND(3,6) 16=NAND(2,11) 19=NAND(11,7) 22=NAND(10,16) 23=NAND(16,19)')
L.append('*')
L.append('*   cargo run --release -p va-cli -- sim circuits/benchmark/iscas85/c17.net \\')
L.append('*       --model external/code/psp103/vacode/psp103.va' + (' --tran' if mode == 'tran' else ''))
L.append('')
L.append('.include ../../../external/benchmarkExt/iscas85_benchmark_circuit/Modelcards/psp103_nmos.mod')
L.append('.include ../../../external/benchmarkExt/iscas85_benchmark_circuit/Modelcards/psp103_pmos.mod')
L.append('')
L.append(f'Vdd vdd gnd DC {VDD}')
for i, n in enumerate(INPUTS):
    if mode == 'op':
        v = VDD if bits[i] == '1' else 0.0
        L.append(f'V{n} g{n}_0 gnd DC {v}')
    else:
        # A different period per input, so the run walks through many input combinations.
        per = 2 * (i + 1)
        L.append(f'V{n} g{n}_0 gnd PULSE(0 {VDD} {i * 100}p 100p 100p {per / 2 - 0.1:.1f}n {per}n)')
L.append('')
L.append('* the nets: C at every pin, R from the driver pin to each load pin')
for net, gates in sorted(loads.items()):
    pins = [f'g{net}_{k}' for k in range(1, len(gates) + 1)]
    L.append(f'.subckt netg{net} g{net}_0 {" ".join(pins)} gnd')
    L.append(f'C0 g{net}_0 gnd {C_PIN}')
    for k, p in enumerate(pins, start=1):
        L.append(f'C{k} {p} gnd {C_PIN}')
        L.append(f'R{k} g{net}_0 {p} {R_WIRE}')
    L.append('.ends')
    L.append(f'XW{net} g{net}_0 {" ".join(pins)} gnd netg{net}')
L.append('')
L.append('* the gates (c7552\'s nand2, verbatim)')
L.append('.subckt nand2 a b vdd vss z')
L.append('nm01 vdd   a     z     vdd pch  l=0.12u  w=0.77u  as=0.20405p  ad=0.20405p  ps=2.07u   pd=2.07u')
L.append('nm02 vss   a     sig3  vss nch  l=0.12u  w=0.66u  as=0.1749p   ad=0.1749p   ps=1.85u   pd=1.85u')
L.append('nm03 z     b     vdd   vdd pch  l=0.12u  w=0.77u  as=0.20405p  ad=0.20405p  ps=2.07u   pd=2.07u')
L.append('nm04 sig3  b     z     vss nch  l=0.12u  w=0.66u  as=0.1749p   ad=0.1749p   ps=1.85u   pd=1.85u')
L.append('c4  a     vss   0.549f')
L.append('c5  b     vss   0.578f')
L.append('c1  z     vss   0.609f')
L.append('.ends')
for g, a, b in GATES:
    L.append(f'XG{g} {pin_of[(a, g)]} {pin_of[(b, g)]} vdd gnd g{g}_0 nand2')
L.append('')
L.append('.tran 1p 12n' if mode == 'tran' else '.op')
L.append('.end')
open(out, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
