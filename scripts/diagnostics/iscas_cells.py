"""Tiny decks from c432's own cells: one cell with fixed inputs; capped plain Newton
(`xtask newton-trace --solve --cap 0.5`). Prints only the cases that do not converge.
Usage: python cells.py CELL [CELL ...]"""
import itertools, pathlib, subprocess, sys

root = pathlib.Path(__file__).resolve().parents[2]
src = (root / "circuits/benchmark/iscas85/c432.net").read_text().splitlines()
keep, inside, take = [], False, False
for l in src:
    if l.startswith(".include"):
        keep.append(l.replace("../../../", str(root).replace("\\", "/") + "/"))
    if l.startswith(".subckt"):
        inside, take = True, not l.split()[1].startswith("netg")
    if inside and take:
        keep.append(l)
    if l.startswith(".ends"):
        inside = False
lib = "\n".join(keep)
ports = {l.split()[1]: l.split()[2:] for l in keep if l.startswith(".subckt")}
xt = str(root / "target/release/xtask.exe")
model = str(root / "benchmark/models/psp103/psp103.va")
work = root / "target/prof-work/cells"
work.mkdir(parents=True, exist_ok=True)
for cell in sys.argv[1:]:
    p = ports[cell]
    ins = [x for x in p if x not in ("vdd", "vss", "z", "gnd")]
    bad = 0
    for bits in itertools.product((0, 1), repeat=len(ins)):
        lines = ["* " + cell, lib, "Vdd vdd gnd DC 1.8"]
        lines += [f"V{n} {n} gnd DC {1.8 * b}" for n, b in zip(ins, bits)]
        lines.append("X1 " + " ".join("gnd" if x == "vss" else x for x in p) + f" {cell}")
        lines += [".op", ".end"]
        deck = work / f"{cell}_{''.join(map(str, bits))}.net"
        deck.write_text("\n".join(lines) + "\n")
        r = subprocess.run([xt, "newton-trace", str(deck), "--model", model, "--iters", "150",
                            "--cap", "0.5", "--solve"], capture_output=True, text=True)
        out = [l for l in (r.stdout + r.stderr).splitlines() if l.startswith("[newton-trace] --solve")]
        verdict = out[0] if out else (r.stdout + r.stderr)[-200:]
        if "converged" not in verdict:
            bad += 1
            print(f"{cell} {''.join(map(str, bits))}: {verdict}")
    print(f"{cell}: {2 ** len(ins)} cases, {bad} not converged")
