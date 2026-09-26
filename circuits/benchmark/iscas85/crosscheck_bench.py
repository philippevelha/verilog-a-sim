"""Compare two copies of an ISCAS'85 circuit — each a .bench or a gate-level .v — the check behind
the provenance line in each .bench's header: gate counts by type, and the function, both copies
simulated bit-parallel on the same random input vectors (4 096 by default). Inputs and outputs are
matched by declared position, and else by the number in each signal's name (ISCAS signals are
numbers; one copy may say G432, another N432).

Usage (repository root):
  python circuits/benchmark/iscas85/crosscheck_bench.py <copy A> <copy B> [words]

Exits non-zero unless the two compute the same function by one of the two matchings. A padded
variant (extra buffers) is reported, with its gate counts, but still passes if the function
matches.
"""
import random
import re
import sys

OPS = {
    'and': lambda v, m: _red(v, lambda a, b: a & b),
    'nand': lambda v, m: ~_red(v, lambda a, b: a & b) & m,
    'or': lambda v, m: _red(v, lambda a, b: a | b),
    'nor': lambda v, m: ~_red(v, lambda a, b: a | b) & m,
    'xor': lambda v, m: _red(v, lambda a, b: a ^ b),
    'xnor': lambda v, m: ~_red(v, lambda a, b: a ^ b) & m,
    'not': lambda v, m: ~v[0] & m,
    'buf': lambda v, m: v[0],
    'buff': lambda v, m: v[0],
}


def _red(vals, f):
    acc = vals[0]
    for x in vals[1:]:
        acc = f(acc, x)
    return acc


def read_bench(path):
    ins, outs, gates = [], [], []
    for line in open(path, encoding='utf-8', errors='replace'):
        line = line.split('#')[0].strip()
        if not line:
            continue
        m = re.match(r'INPUT\((.+)\)', line)
        if m:
            ins.append(m.group(1).strip())
            continue
        m = re.match(r'OUTPUT\((.+)\)', line)
        if m:
            outs.append(m.group(1).strip())
            continue
        m = re.match(r'(\S+)\s*=\s*(\w+)\((.*)\)', line)
        if m:
            gates.append((m.group(1), m.group(2).lower(), [a.strip() for a in m.group(3).split(',')]))
    return ins, outs, gates


def read_v(path):
    text = re.sub(r'//.*', '', open(path, encoding='utf-8', errors='replace').read())
    stmts = [s.strip() for s in text.replace('\n', ' ').split(';')]
    ins, outs, gates = [], [], []
    for s in stmts:
        m = re.match(r'(input|output)\s+(.*)', s)
        if m:
            names = [n.strip() for n in m.group(2).split(',') if n.strip()]
            (ins if m.group(1) == 'input' else outs).extend(names)
            continue
        m = re.match(r'(and|nand|or|nor|xor|xnor|not|buf)\s+\S+\s*\((.*)\)', s)
        if m:
            args = [a.strip() for a in m.group(2).split(',')]
            gates.append((args[0], m.group(1), args[1:]))
    return ins, outs, gates


def simulate(ins, outs, gates, stim, mask):
    val = dict(zip(ins, stim))
    pending = list(gates)
    while pending:
        rest = []
        for out, kind, args in pending:
            if all(a in val for a in args):
                val[out] = OPS[kind]([val[a] for a in args], mask)
            else:
                rest.append((out, kind, args))
        if len(rest) == len(pending):
            raise SystemExit(f'combinational loop or undriven signal: {rest[0]}')
        pending = rest
    return [val[o] for o in outs]


def counts(gates):
    c = {}
    for _, k, a in gates:
        k = 'buf' if k == 'buff' else k
        c[k] = c.get(k, 0) + 1
    return dict(sorted(c.items()))


def load(path):
    return read_v(path) if path.endswith('.v') else read_bench(path)


def main(pa, pb, words=64):
    a, b = load(pa), load(pb)
    bits = 64 * words
    mask = (1 << bits) - 1
    ca, cb = counts(a[2]), counts(b[2])
    num = lambda s: re.sub(r'\D', '', s)
    rng = random.Random(7552)
    stim = [rng.getrandbits(bits) for _ in a[0]]
    ra = simulate(*a, stim, mask)
    verdicts, ok = [], False
    if len(a[0]) == len(b[0]) and len(a[1]) == len(b[1]):
        if simulate(*b, stim, mask) == ra:
            verdicts.append('same function, I/O matched by position')
            ok = True
    pos = {num(x): i for i, x in enumerate(a[0])}
    if {num(x) for x in b[0]} == set(pos) and {num(x) for x in b[1]} == {num(x) for x in a[1]}:
        rb = dict(zip((num(x) for x in b[1]), simulate(*b, [stim[pos[num(x)]] for x in b[0]], mask)))
        same = rb == dict(zip((num(x) for x in a[1]), ra))
        verdicts.append('same function, I/O matched by name number' if same
                        else 'DIFFERENT function with I/O matched by name number')
        ok = ok or same
    if not verdicts:
        verdicts.append('DIFFERENT function by position, and no shared signal numbering')
    print(f'{pa} vs {pb}: {len(a[0])}/{len(a[1])} and {len(b[0])}/{len(b[1])} I/O; gates '
          f'{sum(ca.values())} vs {sum(cb.values())}'
          f'{"" if ca == cb else f" (by type {ca} vs {cb})"}; {bits} random vectors: '
          + '; '.join(verdicts) + (' -> OK' if ok else ' -> MISMATCH'))
    return ok


if __name__ == '__main__':
    sys.exit(0 if main(sys.argv[1], sys.argv[2], *(int(a) for a in sys.argv[3:])) else 1)
