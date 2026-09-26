"""Turn an ISCAS'85 `.bench` netlist into a transistor-level PSP103 deck in the c7552 style.

Usage (from the repository root):
  python circuits/benchmark/iscas85/gen_iscas.py <circuit.bench> <out.net> op <vector>
  python circuits/benchmark/iscas85/gen_iscas.py <circuit.bench> <out.net> tran [<tstop>]

`<vector>` is one 0/1 digit per primary input, in the `.bench` file's INPUT order. `<tstop>` is
the transient's stop time as a SPICE value (default `12n`); the inputs' first edges all fall in
the first 0.8 ns, so a short run still switches every input once.

**Rule (the user's, 2026-09-23): use the cells we already have, and compose any gate the
library lacks from them** — a buffer is two inverters (see README.md).

The gate cells are copied verbatim from the c7552 deck in
external/benchmarkExt/iscas85_benchmark_circuit/ (read at generation time; the deck written is
self-contained apart from the PSP103 model cards it `.include`s). That library stops at four
inputs and has no XOR, so the gates it lacks are **composed** from its cells, as subcircuits:
BUF = NOT + NOT, NAND3/4 = AND3/4 + NOT, XOR2 = XNOR2 + NOT, AND8 = 2 x AND4 + AND2,
AND9 = 3 x AND3 + AND3; and for the rest of the suite (2026-09-26) AND5 = AND4 + AND2,
NAND5 = AND4 + NAND2, NAND8 = 2 x AND4 + NAND2, OR5 = OR4 + OR2, NOR8 = 2 x OR4 + NOR2. The
logic is exact; the timing of a composed gate is not that of a single cell. Nets are RC
subcircuits with c7552's element values, as a star from the driver pin to each load pin.
"""
import os
import re
import sys

C7552 = os.path.join('external', 'benchmarkExt', 'iscas85_benchmark_circuit',
                     'iscas85_benchmark_circuit.sp')
CARDS = '../../../external/benchmarkExt/iscas85_benchmark_circuit/Modelcards'
VDD = 1.8
R_WIRE, C_PIN = '2.224404', '2.080806f'

# gate type and fan-in -> (subckt name, the c7552 cells it needs, composed body or None)
COMPOSED = {
    ('buff', 1): ('buf1c', ['not1'], ['X1 a vdd vss m not1', 'X2 m vdd vss z not1'], 'a'),
    ('buf', 1): ('buf1c', ['not1'], ['X1 a vdd vss m not1', 'X2 m vdd vss z not1'], 'a'),
    ('nand', 3): ('nand3c', ['and3', 'not1'],
                  ['X1 a b c vdd vss m and3', 'X2 m vdd vss z not1'], 'a b c'),
    ('nand', 4): ('nand4c', ['and4', 'not1'],
                  ['X1 a b c d vdd vss m and4', 'X2 m vdd vss z not1'], 'a b c d'),
    ('xor', 2): ('xor2c', ['xnr2v0x1', 'not1'],
                 ['X1 a b vdd vss m xnr2v0x1', 'X2 m vdd vss z not1'], 'a b'),
    ('and', 8): ('and8c', ['and4', 'and2'],
                 ['X1 a b c d vdd vss m1 and4', 'X2 e f g h vdd vss m2 and4',
                  'X3 m1 m2 vdd vss z and2'], 'a b c d e f g h'),
    ('and', 9): ('and9c', ['and3'],
                 ['X1 a b c vdd vss m1 and3', 'X2 d e f vdd vss m2 and3',
                  'X3 g h i vdd vss m3 and3', 'X4 m1 m2 m3 vdd vss z and3'],
                 'a b c d e f g h i'),
    # Added 2026-09-26 for the rest of ISCAS'85 (c499-c7552), fewest cells that are exact.
    ('and', 5): ('and5c', ['and4', 'and2'],
                 ['X1 a b c d vdd vss m and4', 'X2 m e vdd vss z and2'], 'a b c d e'),
    ('nand', 5): ('nand5c', ['and4', 'nand2'],
                  ['X1 a b c d vdd vss m and4', 'X2 m e vdd vss z nand2'], 'a b c d e'),
    ('nand', 8): ('nand8c', ['and4', 'nand2'],
                  ['X1 a b c d vdd vss m1 and4', 'X2 e f g h vdd vss m2 and4',
                   'X3 m1 m2 vdd vss z nand2'], 'a b c d e f g h'),
    ('or', 5): ('or5c', ['or4', 'or2'],
                ['X1 a b c d vdd vss m or4', 'X2 m e vdd vss z or2'], 'a b c d e'),
    ('nor', 8): ('nor8c', ['or4', 'nor2'],
                 ['X1 a b c d vdd vss m1 or4', 'X2 e f g h vdd vss m2 or4',
                  'X3 m1 m2 vdd vss z nor2'], 'a b c d e f g h'),
}
DIRECT = {('not', 1): 'not1', ('nand', 2): 'nand2', ('nor', 2): 'nor2', ('and', 2): 'and2',
          ('and', 3): 'and3', ('and', 4): 'and4', ('nor', 3): 'nor3', ('nor', 4): 'nor4',
          ('or', 2): 'or2', ('or', 3): 'or3', ('or', 4): 'or4'}


def read_bench(path):
    ins, outs, gates = [], [], []
    for line in open(path, encoding='utf-8'):
        line = line.split('#')[0].strip()
        if m := re.match(r'INPUT\((\S+)\)', line):
            ins.append(m.group(1))
        elif m := re.match(r'OUTPUT\((\S+)\)', line):
            outs.append(m.group(1))
        elif m := re.match(r'(\S+)\s*=\s*(\w+)\((.*)\)', line):
            gates.append((m.group(1), m.group(2).lower(),
                          [a.strip() for a in m.group(3).split(',')]))
    return ins, outs, gates


def c7552_cells():
    cells, cur = {}, None
    for line in open(C7552, encoding='utf-8'):
        toks = line.split()
        if toks and toks[0].lower() == '.subckt' and not toks[1].startswith('net'):
            cur = toks[1]
            cells[cur] = [line.rstrip()]
            continue
        if cur:
            cells[cur].append(line.rstrip())
            if toks and toks[0].lower() == '.ends':
                cur = None
    return cells


def net_name(signal):
    """`G118gat` -> `g118`; anything else kept, lower-cased."""
    m = re.fullmatch(r'[Gg]?(\d+)(gat)?', signal)
    return f'g{m.group(1)}' if m else signal.lower()


def evaluate(ins, gates, vector):
    val = dict(zip(ins, vector))
    pending = list(gates)
    while pending:
        rest = []
        for out, kind, args in pending:
            if all(a in val for a in args):
                x = [val[a] for a in args]
                val[out] = {'not': lambda: 1 - x[0], 'buf': lambda: x[0], 'buff': lambda: x[0],
                            'and': lambda: int(all(x)), 'nand': lambda: 1 - int(all(x)),
                            'or': lambda: int(any(x)), 'nor': lambda: 1 - int(any(x)),
                            'xor': lambda: sum(x) % 2, 'xnor': lambda: 1 - sum(x) % 2}[kind]()
            else:
                rest.append((out, kind, args))
        if len(rest) == len(pending):
            raise SystemExit('the netlist has a combinational loop or an undriven signal')
        pending = rest
    return val


def write_deck(bench, out, mode, vector=None, tstop='12n'):
    ins, outs, gates = read_bench(bench)
    cells = c7552_cells()
    name = os.path.splitext(os.path.basename(bench))[0]

    # Which cell each gate uses, and which cells the deck must define.
    use, needed, composed = [], set(), {}
    for g_out, kind, args in gates:
        key = (kind, len(args))
        if key in DIRECT:
            use.append(DIRECT[key])
            needed.add(DIRECT[key])
        elif key in COMPOSED:
            sub, parts, body, ports = COMPOSED[key]
            use.append(sub)
            needed.update(parts)
            composed[sub] = (body, ports)
        else:
            raise SystemExit(f'no cell for {kind} with {len(args)} inputs ({g_out})')

    # Pins: each signal's driver pin is `_0`; every gate input it feeds gets its own pin.
    loads = {}
    for i, (g_out, kind, args) in enumerate(gates):
        for a in args:
            loads.setdefault(a, []).append(i)
    pin = {}
    for sig, users in loads.items():
        for k, i in enumerate(users, start=1):
            pin[(sig, i, k)] = f'{net_name(sig)}_{k}'

    L = [f"* ISCAS'85 {name} at transistor level, PSP103, in the style of the c7552 deck in",
         "* external/benchmarkExt/iscas85_benchmark_circuit/. Generated by gen_iscas.py from",
         f"* {os.path.basename(bench)}: {len(ins)} inputs, {len(outs)} outputs, {len(gates)} gates.",
         "* Cells are c7552's, verbatim; gates it lacks are composed from them (gen_iscas.py).",
         '*', '*   cargo run --release -p va-cli -- sim ' +
         f'circuits/benchmark/iscas85/{os.path.basename(out)} \\',
         '*       --model external/code/psp103/vacode/psp103.va' +
         (' --tran' if mode == 'tran' else ''),
         '', f'.include {CARDS}/psp103_nmos.mod', f'.include {CARDS}/psp103_pmos.mod', '',
         f'Vdd vdd gnd DC {VDD}']
    for i, sig in enumerate(ins):
        if mode == 'op':
            L.append(f'V{net_name(sig)} {net_name(sig)}_0 gnd DC {VDD if vector[i] else 0.0}')
        else:
            per = 2 * (i % 8 + 1)
            L.append(f'V{net_name(sig)} {net_name(sig)}_0 gnd PULSE(0 {VDD} {(i % 8) * 100}p '
                     f'100p 100p {per / 2 - 0.1:.1f}n {per}n)')
    L += ['', '* nets: C at every pin, R from the driver pin to each load pin']
    for sig, users in loads.items():
        n = net_name(sig)
        pins = [f'{n}_{k}' for k in range(1, len(users) + 1)]
        L.append(f'.subckt net{n} {n}_0 {" ".join(pins)} gnd')
        L.append(f'C0 {n}_0 gnd {C_PIN}')
        for k, p in enumerate(pins, start=1):
            L += [f'C{k} {p} gnd {C_PIN}', f'R{k} {n}_0 {p} {R_WIRE}']
        L.append('.ends')
        L.append(f'XW{n} {n}_0 {" ".join(pins)} gnd net{n}')
    L += ['', "* c7552's cells, verbatim"]
    for c in sorted(needed):
        L += cells[c]
    L += ['', '* composed gates']
    for sub, (body, ports) in sorted(composed.items()):
        L += [f'.subckt {sub} {ports} vdd vss z'] + body + ['.ends']
    L += ['', '* the gates']
    for i, ((g_out, kind, args), cell) in enumerate(zip(gates, use)):
        a_pins = [pin[(a, i, loads[a].index(i) + 1)] for a in args]
        L.append(f'XG{net_name(g_out)} {" ".join(a_pins)} vdd gnd {net_name(g_out)}_0 {cell}')
    L += ['', f'.tran 1p {tstop}' if mode == 'tran' else '.op', '.end']
    with open(out, 'w', encoding='utf-8') as f:
        f.write('\n'.join(L) + '\n')
    return ins, outs, gates


if __name__ == '__main__':
    bench, out, mode = sys.argv[1], sys.argv[2], sys.argv[3]
    ins, _, _ = read_bench(bench)
    vec = [int(c) for c in sys.argv[4]] if mode == 'op' else None
    if vec is not None and len(vec) != len(ins):
        raise SystemExit(f'the vector has {len(vec)} digits; the circuit has {len(ins)} inputs')
    tstop = sys.argv[4] if mode == 'tran' and len(sys.argv) > 4 else '12n'
    write_deck(bench, out, mode, vec, tstop)
