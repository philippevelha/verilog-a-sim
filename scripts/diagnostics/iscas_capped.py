"""For each ISCAS'85 .op deck: capped plain Newton accepting backward-stable steps
(`xtask newton-trace --sparse --cap 0.5 --accept-backward`) against va-cli's current rescued
`.op`: iterations, wall time, max |dV| over printed nets, logic disagreements on gate outputs.
Usage: python compare_capped.py c499 c880 ..."""
import pathlib, re, subprocess, sys, time

root = pathlib.Path(__file__).resolve().parents[2]
model = str(root / "benchmark/models/psp103/psp103.va")
xt, va = str(root / "target/release/xtask.exe"), str(root / "target/release/va-cli.exe")
work = root / "target/prof-work"; work.mkdir(parents=True, exist_ok=True)
for c in sys.argv[1:]:
    deck = str(root / f"circuits/benchmark/iscas85/{c}.net")
    xfile = work / f"x_{c}_capped.tsv"
    t0 = time.time()
    r = subprocess.run([xt, "newton-trace", deck, "--model", model, "--iters", "300", "--cap", "0.5",
                        "--sparse", "--accept-backward"], capture_output=True, text=True,
                       env={**__import__("os").environ, "VA_TRACE_X": str(xfile)})
    t_cap = time.time() - t0
    lines = r.stdout.splitlines()
    iters = sum(1 for l in lines if l.startswith("iter "))
    accepted = sum(1 for l in lines if "accepted on backward" in l)
    ok = any(l.strip() == "converged" for l in lines)
    t0 = time.time()
    o = subprocess.run([va, "sim", deck, "--model", model, "--logfull"], capture_output=True, text=True)
    t_op = time.time() - t0
    rescue_iters = sum(int(m) for m in re.findall(r"stage aids=\S+ gmin=\S+ iterations=(\d+)", o.stdout + o.stderr))
    op = {m.group(1): float(m.group(2)) for m in re.finditer(r"^\s+(V\([^)]*\))\s*=\s*([-+\d.eE]+)", o.stdout, re.M)}
    msg = f"{c}: capped {'converged' if ok else 'FAILED'} in {iters} iterations ({accepted} accepted on backward error), {t_cap:.1f} s | va-cli .op exit {o.returncode}, {rescue_iters} iterations over all tiers, {t_op:.1f} s"
    if ok and xfile.exists():
        cap = dict((l.split("\t")[0], float(l.split("\t")[1])) for l in xfile.read_text().splitlines())
        common = [n for n in op if n in cap]
        dmax = max((abs(cap[n] - op[n]), n) for n in common)
        outs = [n for n in common if re.fullmatch(r"V\(g\d+_0\)", n)]
        bad = sum(1 for n in outs if (cap[n] > 0.9) != (op[n] > 0.9))
        msg += f" | {len(common)} nets, max |dV| {dmax[0]:.2e} at {dmax[1]}, logic disagreements {bad}/{len(outs)}"
    print(msg, flush=True)
