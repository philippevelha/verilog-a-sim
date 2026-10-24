#!/usr/bin/env python3
"""Compare a fresh run of make_reference.py against the stored reference.

    python3 make_reference.py --out rerun          # (add --skip-tight for a quick check)
    python3 check_rerun.py outputs rerun

For every case present in both: point count, x-range and time-step statistics must match
exactly (same deterministic step sequence), the waveform is compared point by point
(identical grid) or after resampling (different grid), and wall time / memory are reported
as ratios (these vary with the machine and are informational only).
"""
import gzip, json, os, sys
import numpy as np

def load(d, f):
    with gzip.open(os.path.join(d, f), "rt") as g:
        return np.loadtxt(g, delimiter=",", skiprows=1)

ref_dir, new_dir = sys.argv[1], sys.argv[2]
R = json.load(open(os.path.join(ref_dir, "manifest.json")))
N = json.load(open(os.path.join(new_dir, "manifest.json")))
if R["models"] != N["models"]:
    print("NOTE: model source hashes differ:", R["models"], "->", N["models"])
bad = 0
print(f"{'case':16s} {'points':>15s} {'steps':>6s} {'max|dV| same grid':>18s} {'RMS dP/Pm':>10s} {'wall x':>7s} {'rss x':>6s}  verdict")
for i, r in R["cases"].items():
    if i not in N["cases"]:
        continue
    n = N["cases"][i]
    a = load(ref_dir, r["output_csv"]); b = load(new_dir, n["output_csv"])
    same_steps = all(r.get(k) == n.get(k) for k in ("points", "x_first", "x_last", "dt_min", "dt_median", "dt_max"))
    if a.shape == b.shape and np.array_equal(a[:, 0], b[:, 0]):
        dmax = float(np.max(np.abs(a[:, 1:] - b[:, 1:]))); rms = float("nan")
    else:
        dmax = float("nan")
        Pa = a[:, -2] ** 2 + a[:, -1] ** 2; Pb = np.interp(a[:, 0], b[:, 0], b[:, -2] ** 2 + b[:, -1] ** 2)
        rms = float(np.sqrt(np.mean((Pa - Pb) ** 2)) / Pa.mean())
    # identical step sequence and output to print precision -> PASS; otherwise flag
    tol = 2e-6 if r["simulator"] == "va-sim" else 1e-12
    ok = same_steps and (dmax <= tol)
    bad += not ok
    print(f"{i:16s} {r['points']:7d}->{n['points']:<7d} {'same' if same_steps else 'DIFF':>6s} "
          f"{dmax:18.2e} {rms:10.2e} {n['wall_s_median']/r['wall_s_median']:7.2f} {n['rss_mb_peak']/r['rss_mb_peak']:6.2f}  "
          f"{'PASS' if ok else 'CHECK'}")
print("all cases reproduce the reference" if bad == 0 else f"{bad} case(s) differ from the reference")
sys.exit(1 if bad else 0)
