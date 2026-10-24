"""Windows re-run of the verilog-a-sim cases of mrm_tcmt/reference/make_reference.py.

ngspice/OpenVAF are not installed here, so only the va-sim cases run; accuracy is computed
against the *stored* ngspice outputs (ng_tran_tight) and the closed form, exactly as
make_reference.py does. Writes a manifest compatible with check_rerun.py.

usage: python va_rerun.py <reference_dir> <out_dir> <va-cli.exe> [--repeats N] [--skip-tight]
"""
import argparse, gzip, hashlib, json, os, platform, re, shutil, statistics as st, subprocess, sys, time
import numpy as np, psutil

ap = argparse.ArgumentParser()
ap.add_argument("ref"); ap.add_argument("out"); ap.add_argument("vacli")
ap.add_argument("--repeats", type=int, default=5); ap.add_argument("--skip-tight", action="store_true")
a = ap.parse_args()
REF = os.path.abspath(a.ref); OUT = os.path.abspath(a.out); os.makedirs(OUT, exist_ok=True)

CASES = [
    dict(id="va_dc_default", deck="va_dc.net", args=["--report", "tre,tim"], kind="dc"),
    dict(id="va_tran_default", deck="va_tran.net", args=["--tran", "--report", "kk,tre,tim"], kind="tran"),
    dict(id="va_tran_jit", deck="va_tran.net", args=["--tran", "--report", "kk,tre,tim"], env={"VA_JIT": "on"}, kind="tran"),
    dict(id="va_tran_tight", deck="va_tran.net",
         args=["--tran", "--reltol", "1e-7", "--abstol", "1e-10", "--report", "kk,tre,tim"],
         env={"VA_JIT": "on"}, kind="tran", tight=True),
]

work = os.path.join(OUT, "work"); shutil.rmtree(work, ignore_errors=True)
shutil.copytree(os.path.join(REF, "decks"), work)
for f in ("mrm_tcmt.va", "mrm_driver.va"):
    shutil.copy(os.path.join(REF, "..", f), work)
sha = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest()[:16]

def run(cmd, env):
    e = dict(os.environ); e.update(env or {})
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, cwd=work, env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    ps = psutil.Process(p.pid); peak = 0
    import threading
    def poll():
        nonlocal peak
        while p.poll() is None:
            try: peak = max(peak, ps.memory_info().peak_wset)
            except Exception: break
            time.sleep(0.01)
    th = threading.Thread(target=poll); th.start()
    out, err = p.communicate(); wall = time.perf_counter() - t0; th.join()
    return p.returncode, out, err, wall, peak / 2**20

def parse(txt, kind):
    if kind == "dc":
        r = re.findall(r"Vwl=([-\d.e]+): V\(tre\)=([-\d.e]+) V V\(tim\)=([-\d.e]+) V", txt)
        return np.array(r, float), ["Vwl_nm", "V_tre", "V_tim"]
    r = re.findall(r"t=([-\d.e]+)s\s+V\(kk\)=([-\d.e]+) V\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V", txt)
    return np.array(r, float), ["t_s", "V_kk", "V_tre", "V_tim"]

def stats(x_a, kind):
    x = x_a[:, 0]; d = {"points": int(len(x)), "x_first": float(x[0]), "x_last": float(x[-1])}
    if kind == "tran":
        dt = np.diff(x); dt = dt[dt > 0]
        d.update(dt_min=float(dt.min()), dt_median=float(np.median(dt)), dt_max=float(dt.max()),
                 steps_below_1fs=int((dt < 1e-15).sum()))
    return d

ver = subprocess.run([a.vacli, "--help"], capture_output=True, text=True).stdout.splitlines()[:1]
man = {"created": time.strftime("%Y-%m-%d %H:%M:%S %z"),
       "host": {"machine": platform.machine(), "cpus": os.cpu_count(), "cpu": platform.processor(), "os": platform.platform()},
       "tools": {"va-cli": a.vacli},
       "models": {f: sha(os.path.join(work, f)) for f in ("mrm_tcmt.va", "mrm_driver.va")},
       "notes": ["Windows re-run, va-sim cases only; timing = Python perf_counter wall, rss = peak working set",
                 "va-sim timing includes parsing + compiling the Verilog-A each run"],
       "cases": {}}
for c in CASES:
    if a.skip_tight and c.get("tight"):
        continue
    n = 1 if c.get("tight") else a.repeats
    cmd = [a.vacli, "sim", c["deck"], "--model", "."] + c["args"]
    W, R = [], []
    for _ in range(n):
        rc, out, err, w, m = run(cmd, c.get("env"))
        if rc:
            print(c["id"], "FAILED", out[-500:], err[-500:]); sys.exit(1)
        W.append(w); R.append(m)
    arr, cols = parse(out, c["kind"])
    with gzip.open(os.path.join(OUT, c["id"] + ".stdout.gz"), "wt") as g: g.write(out)
    csv = c["id"] + ".csv.gz"
    with gzip.open(os.path.join(OUT, csv), "wt") as g:
        np.savetxt(g, arr, delimiter=",", header=",".join(cols), comments="", fmt="%.15e")
    rec = {"simulator": "va-sim", "deck": c["deck"], "repeats": n,
           "command": " ".join([f"{k}={v}" for k, v in (c.get("env") or {}).items()] + ["va-cli"] + cmd[1:]),
           "wall_s_median": st.median(W), "wall_s_min": min(W), "wall_s_max": max(W), "rss_mb_peak": max(R),
           "output_csv": csv, "output_sha256_16": hashlib.sha256(arr.tobytes()).hexdigest()[:16],
           "stderr_tail": err.strip().splitlines()[-3:]}
    rec.update(stats(arr, c["kind"]))
    man["cases"][c["id"]] = rec
    print(f"{c['id']:16s} {rec['points']:7d} pts  wall {rec['wall_s_median']:7.3f} s (min {rec['wall_s_min']:.3f})  "
          f"rss {rec['rss_mb_peak']:5.1f} MB", flush=True)

# accuracy, same definitions as make_reference.py, references from the STORED outputs
def load(d, i):
    with gzip.open(os.path.join(d, i + ".csv.gz"), "rt") as g:
        return np.loadtxt(g, delimiter=",", skiprows=1)
cl = 2.99792458e8
def lorentz(lam):
    w0 = 2 * np.pi * cl / 1550e-9; gi = w0 / 4e4; gc = w0 / 4.4e4
    dl = 10 / np.pi * np.sin(np.pi * (lam - 1550) / 10)
    dw = 2 * np.pi * cl * dl * 1e-9 / (lam * 1550 * 1e-18)
    return np.abs((gi - gc - 1j * dw) / (gi + gc - 1j * dw)) ** 2
acc = {}
if "va_dc_default" in man["cases"]:
    d = load(OUT, "va_dc_default"); P = d[:, 1] ** 2 + d[:, 2] ** 2
    acc["va_dc_default"] = {"vs": "closed-form Lorentzian", "max_abs_dP": float(np.max(np.abs(P - lorentz(d[:, 0]))))}
ref = load(os.path.join(REF, "outputs"), "ng_tran_tight"); tg = np.arange(0.1e-9, 10.15e-9, 0.05e-12)
Pr = np.interp(tg, ref[:, 0], ref[:, 2] ** 2 + ref[:, 3] ** 2); Pm = Pr.mean(); Vr = np.interp(tg, ref[:, 0], ref[:, 1])
for i in man["cases"]:
    if "tran" in i:
        d = load(OUT, i); P = np.interp(tg, d[:, 0], d[:, 2] ** 2 + d[:, 3] ** 2); V = np.interp(tg, d[:, 0], d[:, 1])
        acc[i] = {"vs": "stored ng_tran_tight", "rms_dP_over_Pmean": float(np.sqrt(np.mean((P - Pr) ** 2)) / Pm),
                  "max_dP_over_Pmean": float(np.max(np.abs(P - Pr)) / Pm), "rms_dV_kk": float(np.sqrt(np.mean((V - Vr) ** 2)))}
man["accuracy"] = acc
json.dump(man, open(os.path.join(OUT, "manifest.json"), "w"), indent=2)
shutil.rmtree(work, ignore_errors=True)
print(json.dumps(acc, indent=1))
