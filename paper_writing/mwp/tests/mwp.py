#!/usr/bin/env python3
"""Harness for the microwave-photonics library: write a deck, run verilog-a-sim, parse the
output, extract RF metrics.

    r = run(deck_text, "tran", report=["Vs"], tag="t01")   ->  r["x"], r["y"]["I(Vs)"]

Decks are plain verilog-a-sim netlists. Helper `o("a")` expands an optical bus name into its
two nets ("a_re a_im"). Currents are read through 0 V sources (`Vs node gnd DC 0` -> I(Vs)):
va-cli prints currents with 7 significant digits and voltages with 6 decimals, so a current
probe is the only one with the dynamic range an intermodulation measurement needs.

Environment: VACLI = path of the va-cli binary (default: <repo>/target/release/va-cli[.exe]).
"""
import os, re, subprocess, sys, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.dirname(HERE)
LIB = os.path.join(PKG, "lib")
REPO = os.path.dirname(os.path.dirname(PKG))
_exe = "va-cli.exe" if os.name == "nt" else "va-cli"
VACLI = os.environ.get("VACLI", os.path.join(REPO, "target", "release", _exe))
WORK = os.path.join(HERE, "work")

Q = 1.602176634e-19
KB = 1.380649e-23
H = 6.62607015e-34
C0 = 2.99792458e8


def o(name):
    """Optical bus -> its two nets."""
    return f"{name}_re {name}_im"


NUM = r"([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?|[-+]?(?:inf|nan))"


def run(deck, analysis="op", report=None, tag="deck", flags=(), timeout=3600, env=None):
    """Run one deck. analysis: op | dc | tran | ac | noise.
    Returns dict(x, y={label: array}, wall_s, points, stdout). AC values are complex.
    op: y[label] is a one-element array.
    env: extra environment variables for va-cli (e.g. {"VA_ABSDELAY_POINTS": "16384"}).
    For noise: y["S"] (output PSD) and y["Sin"]."""
    os.makedirs(WORK, exist_ok=True)
    path = os.path.join(WORK, tag + ".net")
    with open(path, "w", newline="\n") as f:
        f.write(deck if deck.endswith("\n") else deck + "\n")
    cmd = [VACLI, "sim", path, "--model", LIB]
    cmd += {"op": [], "dc": [], "tran": ["--tran"], "ac": ["--ac"], "noise": ["--noise"]}[analysis]
    if report:
        cmd += ["--report", ",".join(report)]
    cmd += list(flags)
    t0 = time.time()
    e = dict(os.environ); e.update(env or {})
    # output goes to files, so the process can be polled for its peak memory while it runs
    fo, fe = path + ".out", path + ".err"
    peak = 0
    with open(fo, "w", encoding="utf-8") as ho, open(fe, "w", encoding="utf-8") as he:
        proc = subprocess.Popen(cmd, stdout=ho, stderr=he, env=e)
        try:
            import psutil
            ps = psutil.Process(proc.pid)
        except Exception:
            ps = None
        while proc.poll() is None:
            if ps is not None:
                try:
                    mi = ps.memory_info()
                    peak = max(peak, getattr(mi, "peak_wset", 0), mi.rss)
                except Exception:
                    pass
            if time.time() - t0 > timeout:
                proc.kill()
                raise RuntimeError(f"va-cli timed out on {path}")
            time.sleep(0.02)
    wall = time.time() - t0
    out = open(fo, encoding="utf-8").read()
    err = open(fe, encoding="utf-8").read()
    if proc.returncode != 0 or "error[" in out or "error[" in err or "refused" in out:
        raise RuntimeError(f"va-cli failed on {path}:\n" + (out[-1500:] + err[-3000:]))
    xs, ys = [], {}

    def put(label, v):
        ys.setdefault(label, []).append(v)

    if analysis == "noise":
        for line in out.splitlines():
            m = re.match(r"\s*f=" + NUM + r"Hz\s+S=" + NUM + r".*?Sin=" + NUM, line)
            if m:
                xs.append(float(m.group(1))); put("S", float(m.group(2))); put("Sin", float(m.group(3)))
    elif analysis == "ac":
        for line in out.splitlines():
            m = re.match(r"\s*f=" + NUM + r"Hz\s+(.*)", line)
            if not m:
                continue
            xs.append(float(m.group(1)))
            for lab, mag, ph in re.findall(r"([VI]\([^)]*\))=" + NUM + r" [VA]∠" + NUM + "°", m.group(2)):
                put(lab, float(mag) * np.exp(1j * np.deg2rad(float(ph))))
    else:
        if analysis == "op":
            for lab, v in re.findall(r"^\s*([VI]\([^)]*\)) = " + NUM, out, flags=re.M):
                ys[lab] = [float(v)]
            xs = [0.0]
        pat = {"tran": r"\s*t=" + NUM + r"s\s+(.*)", "dc": r"\s*\S+=" + NUM + r":?\s+(.*)",
               "op": r"(?!)()()"}[analysis]
        for line in out.splitlines():
            if line.startswith("[va-cli]"):
                continue
            m = re.match(pat, line)
            if not m or "=" not in m.group(2):
                continue
            vals = re.findall(r"([VI]\([^)]*\))=" + NUM, m.group(2))
            if not vals:
                continue
            xs.append(float(m.group(1)) if m.group(1) else 0.0)
            for lab, v in vals:
                put(lab, float(v))
    y = {k: np.array(v) for k, v in ys.items()}
    return dict(x=np.array(xs), y=y, wall_s=wall, points=len(xs), stdout=out, stderr=err, peak_mb=peak / 2 ** 20)


# ------------------------------------------------------------------ signal metrics
def tones(t, y, freqs, t_start=0.0):
    """Least-squares amplitudes of known tones in a (non-uniformly sampled) waveform.
    Model: y = c0 + sum_k a_k cos(2 pi f_k t) + b_k sin(2 pi f_k t), fitted for t >= t_start.
    Returns (complex array A_k = a_k - j b_k, so y ~ Re(A_k e^{j w t}); dc; rms residual).
    The window should hold an integer number of periods of every tone and of their beats."""
    m = t >= t_start
    tt, yy = t[m], y[m]
    # trapezoid weights: a non-uniform grid must not weight densely sampled stretches more
    w = np.zeros_like(tt)
    w[1:] += 0.5 * np.diff(tt); w[:-1] += 0.5 * np.diff(tt)
    sw = np.sqrt(w / w.sum())
    cols = [np.ones_like(tt)]
    for f in freqs:
        cols += [np.cos(2 * np.pi * f * tt), np.sin(2 * np.pi * f * tt)]
    M = np.stack(cols, axis=1)
    sol, *_ = np.linalg.lstsq(M * sw[:, None], yy * sw, rcond=None)
    res = yy - M @ sol
    amps = np.array([sol[1 + 2 * k] - 1j * sol[2 + 2 * k] for k in range(len(freqs))])
    return amps, sol[0], float(np.sqrt(np.sum(w * res ** 2) / w.sum()))


def freq_est(t, y, f_guess, t_start, n_iter=6):
    """Frequency of the dominant tone near f_guess, from the phase advance between the two
    halves of the record t >= t_start (iterated). Returns (f, amplitude)."""
    m = t >= t_start
    tt, yy = t[m], y[m]
    mid = 0.5 * (tt[0] + tt[-1])
    f = f_guess
    for _ in range(n_iter):
        a1, *_ = tones(tt[tt <= mid] - tt[0], yy[tt <= mid], [f])
        a2, *_ = tones(tt[tt >= mid] - mid, yy[tt >= mid], [f])
        dphi = np.angle(a2[0] / a1[0]) - 2 * np.pi * ((f * (mid - tt[0])) % 1.0)
        dphi = (dphi + np.pi) % (2 * np.pi) - np.pi
        f += dphi / (2 * np.pi * (mid - tt[0]))
    a, *_ = tones(tt, yy, [f])
    return f, abs(a[0])


def resample(t, y, n, t0=None, t1=None):
    """Uniform resampling (linear interpolation) for an FFT."""
    t0 = t[0] if t0 is None else t0
    t1 = t[-1] if t1 is None else t1
    tu = t0 + (t1 - t0) * np.arange(n) / n
    return tu, np.interp(tu, t, y)


def dbm(i_amp, r=50.0):
    """Power (dBm) that a current tone of peak amplitude i_amp delivers to r ohm."""
    return 10 * np.log10(0.5 * np.abs(i_amp) ** 2 * r / 1e-3 + 1e-300)


def pwl(t, v):
    return "PWL(" + " ".join(f"{a:.9e} {b:.9e}" for a, b in zip(t, v)) + ")"


class Report:
    """Collects check lines: name, simulated, reference, tolerance -> pass/fail; writes JSON."""
    def __init__(self, name):
        self.name, self.rows, self.t0 = name, [], time.time()
        print(f"== {name}")

    def check(self, what, sim, ref, tol, rel=True, unit=""):
        err = abs(sim - ref) / abs(ref) if rel else abs(sim - ref)
        ok = bool(err <= tol)
        self.rows.append(dict(what=what, sim=float(np.real(sim)), ref=float(np.real(ref)), err=float(err), tol=tol,
                              rel=rel, unit=unit, ok=ok))
        kind = "rel" if rel else "abs"
        print(f"  [{'ok' if ok else 'FAIL'}] {what}: sim {sim:.6g} ref {ref:.6g} {unit} ({kind} err {err:.2e}, tol {tol:g})")
        return ok

    def note(self, text):
        self.rows.append(dict(note=text)); print("  " + text)

    def done(self, extra=None):
        import json
        ok = all(r.get("ok", True) for r in self.rows)
        d = dict(test=self.name, ok=ok, wall_s=round(time.time() - self.t0, 2), rows=self.rows)
        if extra:
            d["data"] = extra
        with open(os.path.join(HERE, self.name + ".json"), "w") as f:
            json.dump(d, f, indent=1)
        print(f"  -> {'PASS' if ok else 'FAIL'} in {d['wall_s']} s")
        return ok
