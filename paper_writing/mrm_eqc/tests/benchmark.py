#!/usr/bin/env python3
"""Aggregate wall time / peak RSS (from /usr/bin/time, 10 ms resolution) over all test JSONs."""
import json, glob
def walk(o, path, acc):
    if isinstance(o, dict):
        if "wall_s" in o: acc.append((path, o)); return
        for k, v in o.items(): walk(v, path + [str(k)], acc)
    elif isinstance(o, list):
        for v in o: walk(v, path + ([str(v.get("sim", ""))] if isinstance(v, dict) else []), acc)
rows = []
for f in sorted(glob.glob("t[1-5]*.json")):
    acc = []; walk(json.load(open(f)), [], acc)
    for sim in ("ngspice", "va"):
        for mod in ("rlc", "ls"):
            sel = [t for p, t in acc if any(sim == x or x.startswith(sim + "_") or x.endswith("_" + sim) for x in p)
                   and (("rlc" in p) if mod == "rlc" else ("rlc" not in p))]
            if not sel: continue
            rows.append(dict(test=f[:-5], sim=sim, model=mod, runs=len(sel), wall_total_s=round(sum(t["wall_s"] for t in sel), 2),
                             wall_max_s=max(t["wall_s"] for t in sel), rss_peak_mb=round(max(t["rss_mb"] for t in sel), 1)))
print(f"{'test':22s} {'model':5s} {'sim':8s} {'runs':>4s} {'wall tot s':>10s} {'wall max s':>10s} {'peak RSS MB':>11s}")
for r in rows: print(f"{r['test']:22s} {r['model']:5s} {r['sim']:8s} {r['runs']:4d} {r['wall_total_s']:10.2f} {r['wall_max_s']:10.2f} {r['rss_peak_mb']:11.1f}")
json.dump(rows, open("benchmark.json", "w"), indent=1)
