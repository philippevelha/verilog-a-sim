#!/usr/bin/env python3
"""verilog-a-sim against the Spectre results of the translated circuits.

    python scripts/spectre_golden/compare_zoo.py [golden_dir] [deck ...]

golden_dir defaults to golden/spectre/zoo. For each deck directory there, the matching
circuits/ deck is run in verilog-a-sim with the options of its documented command line
(--model, --reltol), reporting the signals Spectre saved, and every signal both have is
compared on Spectre's sweep or time points:

  op, dc     max |ours - spectre| / max(|spectre|)            tolerance 1e-4 (CLAUDE.md section 7)
  ac         max |ours - spectre| / max |spectre|, complex    tolerance 1e-3
  noise      max relative difference of the output PSD        tolerance 1e-3
  tran       RMS difference / peak-to-peak of the Spectre waveform, on Spectre's time points
             (ours interpolated)                              tolerance 1e-3

A device current is compared up to its sign (Spectre reports the current into the + terminal).
Decks whose result is task output in the log (the bubble-cloud sweeps) are not compared here.
Writes compare_zoo.json beside this script and prints one line per deck.
"""
import glob, json, os, re, subprocess, sys, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "paper_writing", "mwp", "spectre"))
import psf                                              # noqa: E402
from make_package import run_line                       # noqa: E402

VACLI = os.environ.get("VACLI", os.path.join(REPO, "target", "release", "va-cli.exe" if os.name == "nt" else "va-cli"))
TOL = {"op": 1e-4, "dc": 1e-4, "ac": 1e-3, "noise": 1e-3, "tran": 1e-3}
NUM = r"([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?|[-+]?(?:inf|nan))"
SKIP = ("bubble_cloud_sweep",)


def deck_of(name):
    p = os.path.join(REPO, "circuits", *name.split("__")) + ".net"
    return p if os.path.isfile(p) else None


def spectre_result(d):
    """(kind, sweep array or None, {trace: array}) from the first analysis file in d."""
    for pat, kind in (("*.tran", "tran"), ("*.ac", "ac"), ("*.noise", "noise"), ("sw*.dc", "dc"), ("*.dc", "op")):
        f = sorted(glob.glob(os.path.join(d, pat)))
        f = [x for x in f if not x.endswith("op.dc")] if kind == "dc" else f
        if f:
            r = psf.read(f[0])
            sw = r.get("_sweep")
            x = np.asarray(r[sw]) if sw else None
            tr = {k: np.asarray(v) for k, v in r.items() if k not in ("_sweep", sw)}
            return kind, x, tr
    return None, None, {}


def run_ours(deck, kind, names, extra, src=None):
    flag = {"tran": ["--tran"], "ac": ["--ac"], "noise": ["--noise"]}.get(kind, [])
    cmd = [VACLI, "sim", deck, *extra, *flag]
    if names and kind != "noise":
        cmd += ["--report", ",".join(names)]
    t0 = time.time()
    p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=3600, cwd=REPO)
    out = p.stdout
    if p.returncode != 0 or "error[" in out + p.stderr:
        raise RuntimeError((out + p.stderr)[-800:])
    xs, ys = [], {}
    if kind == "noise":
        for l in out.splitlines():
            m = re.match(r"\s*f=" + NUM + r"Hz\s+S=" + NUM, l)
            if m:
                xs.append(float(m.group(1))); ys.setdefault("out", []).append(float(m.group(2)))
    elif kind == "ac":
        for l in out.splitlines():
            m = re.match(r"\s*f=" + NUM + r"Hz\s+(.*)", l)
            if m:
                xs.append(float(m.group(1)))
                for lab, mag, ph in re.findall(r"(\S+?)=" + NUM + r" ?\S*∠" + NUM + "°", m.group(2)):
                    ys.setdefault(lab, []).append(float(mag) * np.exp(1j * np.deg2rad(float(ph))))
    elif kind == "op":
        for lab, v in re.findall(r"^\s*(\S+) = " + NUM, out, flags=re.M):
            ys[lab] = [float(v)]
        xs = [0.0]
    else:
        pat = r"\s*t=" + NUM + r"s\s+(.*)" if kind == "tran" else r"\s*" + re.escape(src or "") + r"=" + NUM + r":\s+(.*)"
        for l in out.splitlines():
            if l.startswith("[va-cli]"):
                continue
            m = re.match(pat, l)
            if not m:
                continue
            vals = re.findall(r"(\S+?)=" + NUM, m.group(2))
            if not vals:
                continue
            xs.append(float(m.group(1)))
            for lab, v in vals:
                ys.setdefault(lab, []).append(float(v))
    return np.array(xs), {k: np.array(v) for k, v in ys.items()}, time.time() - t0


def label(ours, sname):
    """Our label for Spectre trace `sname`."""
    if re.search(r":(p|1)$", sname):
        return f"I({sname[:-2]})", True
    for cand in (f"V({sname})", sname):
        if cand in ours:
            return cand, False
    # a non-electrical quantity prints with its own access function: P(pb), Pos(x), ...
    for k in ours:
        if re.fullmatch(r"\w+\(" + re.escape(sname) + r"\)", k):
            return k, False
    return f"V({sname})", False


def compare(kind, xs, s, xo, o):
    if kind in ("op", "dc"):
        so = o if kind == "op" else np.interp(xs, xo, o)
        return float(np.max(abs(so - s)) / max(np.max(abs(s)), 1e-30))
    if kind == "ac":
        so = np.interp(xs, xo, o.real) + 1j * np.interp(xs, xo, o.imag)
        return float(np.max(abs(so - s)) / max(np.max(abs(s)), 1e-30))
    if kind == "noise":
        return float(np.max(abs(np.interp(xs, xo, o) / s - 1)))
    so = np.interp(xs, xo, o)
    # relative to the waveform's swing, or to its size when it hardly moves (a DC source node)
    span = max(np.ptp(s), np.max(abs(s)), 1e-30) if np.ptp(s) < 1e-6 * np.max(abs(s)) else max(np.ptp(s), 1e-30)
    w = np.zeros_like(xs); w[1:] += 0.5 * np.diff(xs); w[:-1] += 0.5 * np.diff(xs)
    return float(np.sqrt(np.sum(w * (so - s) ** 2) / w.sum()) / span)       # time-weighted RMS


def main():
    gdir = sys.argv[1] if len(sys.argv) > 1 and os.path.isdir(sys.argv[1]) else os.path.join(REPO, "golden", "spectre", "zoo")
    only = [a for a in sys.argv[1:] if not os.path.isdir(a)]
    rows = []
    for d in sorted(glob.glob(os.path.join(gdir, "*"))):
        name = os.path.basename(d)
        if not os.path.isdir(d) or (only and name not in only) or any(s in name for s in SKIP):
            continue
        deck = deck_of(name)
        try:
            kind, xs, tr = spectre_result(d)
        except ValueError as e:                             # a run that stopped before writing results
            print(f"{name}: Spectre result unreadable: {e}")
            rows.append(dict(deck=name, error=f"Spectre result unreadable: {e}")); continue
        if deck is None or kind is None:
            print(f"{name}: no deck or no Spectre result"); continue
        models, _ = run_line(deck)
        scs = open(os.path.join(d, name + ".scs"), encoding="utf-8").read()
        if not models:
            # no documented command line: the model files the Spectre netlist itself includes
            models = re.findall(r'^ahdl_include\s+"([^"]+)"', scs, re.M)
        extra = [x for m in models for x in ("--model", m)] or ["--model", "models"]
        doc =" ".join(l for l in open(deck, encoding="utf-8", errors="replace").read().splitlines()[:80] if l.startswith("*"))
        # a number, not the full stop of the sentence it ends (bubble_single: "--reltol 1e-5.")
        m = re.search(r"--reltol\s+((?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)", doc)
        if m:
            extra += ["--reltol", m.group(1)]
        nets = set()
        for mm in re.finditer(r"^\S+ \(([^)]*)\)", scs, re.M):
            nets |= set(mm.group(1).split())
        insts = set(re.findall(r"^([A-Za-z]\w*) \(", scs, re.M))
        tr = {k: v for k, v in tr.items()
              if (k in nets and k != "0") or re.sub(r":(p|1)$", "", k) in insts or kind == "noise" or "." in k}
        names = []
        for k in tr:
            if re.search(r":(p|1)$", k):
                names.append(k[:-2])
            elif kind != "noise":
                names.append(k)
        try:
            mdc = re.search(r"^\.dc\s+(\S+)", open(deck, encoding="utf-8", errors="replace").read(), re.M | re.I)
            xo, o, wall = run_ours(deck, kind, names, extra, mdc.group(1) if mdc else None)
        except Exception as e:                              # noqa: BLE001
            print(f"{name}: verilog-a-sim failed: {str(e)[-300:]}")
            rows.append(dict(deck=name, kind=kind, error=str(e)[-300:])); continue
        worst, wname, n = 0.0, "", 0
        for sname, s in tr.items():
            if kind == "noise":
                if sname != "out":
                    continue
                s = np.real(s) ** 2 if np.real(s).max() > 1e-12 else np.real(s)
                lab, cur = "out", False
            else:
                lab, cur = label(o, sname)
            if lab not in o or len(o[lab]) == 0:
                continue
            ov = o[lab]
            if kind in ("op", "dc", "tran"):
                s = np.real(s)
            if cur:
                alt = -ov
                e = min(compare(kind, xs, s, xo, ov), compare(kind, xs, s, xo, alt))
            else:
                e = compare(kind, xs, s, xo, ov)
            n += 1
            if e > worst:
                worst, wname = e, sname
        ok = n > 0 and worst <= TOL[kind]
        rows.append(dict(deck=name, kind=kind, signals=n, worst=worst, worst_signal=wname, ok=ok, wall_s=round(wall, 1)))
        print(f"{'ok  ' if ok else 'DIFF'} {name}: {kind}, {n} signal(s), worst {worst:.2e} ({wname}), tol {TOL[kind]:g}, ours {wall:.1f} s")
    json.dump(rows, open(os.path.join(HERE, "compare_zoo.json"), "w"), indent=1)
    good = sum(r.get("ok", False) for r in rows)
    print(f"\n{good} of {len(rows)} decks agree with Spectre within tolerance")


if __name__ == "__main__":
    main()
