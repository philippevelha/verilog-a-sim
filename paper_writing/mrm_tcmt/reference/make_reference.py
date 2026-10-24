#!/usr/bin/env python3
"""Run every reference case for mrm_tcmt in ngspice (OSDI) and verilog-a-sim, time it,
and store outputs + a manifest.  Re-run any time; compare with check_rerun.py.

Usage:  python3 make_reference.py [--out DIR] [--repeats N] [--skip-tight]
Paths to the tools are taken from the environment (NGSPICE, VACLI, OPENVAF) or the defaults below.
"""
import argparse, gzip, hashlib, json, os, platform, re, shutil, statistics as st, subprocess, sys, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
NGSPICE = os.environ.get("NGSPICE", "/opt/ngspice-new/bin/ngspice")
VACLI   = os.environ.get("VACLI",   "/home/claude/verilog-a-sim/target/release/va-cli")
OPENVAF = os.environ.get("OPENVAF", "/home/claude/arpadbuermen/openvaf/target/release/openvaf-r")
TIME    = "/usr/bin/time"

# ----------------------------------------------------------------------------- cases
# Each case: simulator, deck (in decks/), extra CLI args, env, kind, repeats override
CASES = [
    dict(id="ng_dc_default",    sim="ngspice", deck="ng_dc_default.cir",    kind="dc"),
    dict(id="ng_dc_tight",      sim="ngspice", deck="ng_dc_tight.cir",      kind="dc"),
    dict(id="ng_tran_default",  sim="ngspice", deck="ng_tran_default.cir",  kind="tran"),
    dict(id="ng_tran_tight",    sim="ngspice", deck="ng_tran_tight.cir",    kind="tran", tight=True),
    dict(id="va_dc_default",    sim="va-sim",  deck="va_dc.net",   args=["--report", "tre,tim"], kind="dc"),
    dict(id="va_tran_default",  sim="va-sim",  deck="va_tran.net", args=["--tran", "--report", "kk,tre,tim"], kind="tran"),
    dict(id="va_tran_jit",      sim="va-sim",  deck="va_tran.net", args=["--tran", "--report", "kk,tre,tim"],
         env={"VA_JIT": "on"}, kind="tran"),
    dict(id="va_tran_tight",    sim="va-sim",  deck="va_tran.net",
         args=["--tran", "--reltol", "1e-7", "--abstol", "1e-10", "--report", "kk,tre,tim"],
         env={"VA_JIT": "on"}, kind="tran", tight=True),
]

def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()[:16]

def ver(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.strip().splitlines()
    except Exception as e:
        return [f"unavailable: {e}"]

def run_timed(cmd, cwd, env):
    e = dict(os.environ); e.update(env or {})
    tf = os.path.join(cwd, ".time.txt")
    p = subprocess.run([TIME, "-f", "%e %U %S %M", "-o", tf] + cmd, cwd=cwd, env=e,
                       capture_output=True, text=True)
    w, u, s, m = open(tf).read().split()[-4:]
    os.remove(tf)
    return p, dict(wall=float(w), user=float(u), sys=float(s), rss_mb=int(m) / 1024)

# ----------------------------------------------------------------------------- parsing
def parse_va(txt, kind):
    if kind == "dc":
        rows = re.findall(r"Vwl=([-\d.e]+): V\(tre\)=([-\d.e]+) V V\(tim\)=([-\d.e]+) V", txt)
        return np.array(rows, float), ["Vwl_nm", "V_tre", "V_tim"]
    rows = re.findall(r"t=([-\d.e]+)s\s+V\(kk\)=([-\d.e]+) V\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V", txt)
    return np.array(rows, float), ["t_s", "V_kk", "V_tre", "V_tim"]

def parse_ng(path, kind):
    a = np.loadtxt(path, skiprows=1)
    if kind == "dc":
        return a[:, :3], ["Vwl_nm", "V_tre", "V_tim"]
    return a[:, :4], ["t_s", "V_kk", "V_tre", "V_tim"]

def stats(a, kind):
    x = a[:, 0]
    d = {"points": int(len(x)), "x_first": float(x[0]), "x_last": float(x[-1])}
    if kind == "tran":
        dt = np.diff(x); dt = dt[dt > 0]
        d.update(dt_min=float(dt.min()), dt_median=float(np.median(dt)), dt_max=float(dt.max()),
                 steps_below_1fs=int((dt < 1e-15).sum()))
    P = a[:, -2] ** 2 + a[:, -1] ** 2
    d.update(P_min_mW=float(P.min()), P_max_mW=float(P.max()), P_mean_mW=float(P.mean()))
    return d

# ----------------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(HERE, "outputs"))
    ap.add_argument("--repeats", type=int, default=5)
    ap.add_argument("--skip-tight", action="store_true")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    work = os.path.join(HERE, "work"); shutil.rmtree(work, ignore_errors=True)
    shutil.copytree(os.path.join(HERE, "decks"), work)
    for f in ("mrm_tcmt.va", "mrm_driver.va"):
        shutil.copy(os.path.join(HERE, "..", f), work)
    # ngspice needs compiled OSDI; verilog-a-sim compiles the .va itself. OpenVAF is run in a
    # clean directory: verilog-a-sim's own disciplines.vams (needed in work/) would shadow
    # OpenVAF's built-in one.
    ovdir = os.path.join(HERE, "work_openvaf"); shutil.rmtree(ovdir, ignore_errors=True); os.makedirs(ovdir)
    for f in ("mrm_tcmt.va", "mrm_driver.va"):
        shutil.copy(os.path.join(HERE, "..", f), ovdir)
        subprocess.run([OPENVAF, f], cwd=ovdir, capture_output=True, check=True)
        shutil.copy(os.path.join(ovdir, f.replace(".va", ".osdi")), work)
    shutil.rmtree(ovdir)

    manifest = {
        "created": time.strftime("%Y-%m-%d %H:%M:%S %z"),
        "host": {"machine": platform.machine(), "cpus": os.cpu_count(),
                 "cpu": next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "?")},
        "tools": {"ngspice": ver([NGSPICE, "--version"])[1:2], "openvaf": ver([OPENVAF, "--version"]),
                  "va-cli": VACLI},
        "models": {f: sha(os.path.join(work, f)) for f in ("mrm_tcmt.va", "mrm_driver.va")},
        "notes": ["ngspice runs with .spiceinit 'set num_threads=1' (OpenMP sync overhead otherwise dominates)",
                  "verilog-a-sim timing includes parsing + compiling the Verilog-A each run",
                  "verilog-a-sim prints 6 decimals; ngspice decks use numdgt=15",
                  "timings: median of N repeats; rss = peak resident set size"],
        "cases": {},
    }
    for c in CASES:
        if a.skip_tight and c.get("tight"):
            continue
        n = 1 if c.get("tight") else a.repeats
        if c["sim"] == "ngspice":
            cmd = [NGSPICE, "-b", c["deck"]]
        else:
            cmd = [VACLI, "sim", c["deck"], "--model", "."] + c.get("args", [])
        T = []
        for _ in range(n):
            p, t = run_timed(cmd, work, c.get("env"))
            if p.returncode != 0:
                print(f"{c['id']}: FAILED\n{p.stdout[-500:]}{p.stderr[-500:]}"); sys.exit(1)
            T.append(t)
        if c["sim"] == "ngspice":
            arr, cols = parse_ng(os.path.join(work, c["id"] + ".dat"), c["kind"])
        else:
            arr, cols = parse_va(p.stdout, c["kind"])
            with gzip.open(os.path.join(a.out, c["id"] + ".stdout.gz"), "wt") as g:
                g.write(p.stdout)
        csv = os.path.join(a.out, c["id"] + ".csv.gz")
        with gzip.open(csv, "wt") as g:
            np.savetxt(g, arr, delimiter=",", header=",".join(cols), comments="", fmt="%.15e")
        rec = {"simulator": c["sim"], "deck": c["deck"],
               "command": " ".join(([f"{k}={v}" for k, v in (c.get("env") or {}).items()]) + [os.path.basename(cmd[0])] + cmd[1:]),
               "repeats": n,
               "wall_s_median": st.median(t["wall"] for t in T), "wall_s_min": min(t["wall"] for t in T),
               "wall_s_max": max(t["wall"] for t in T), "user_s_mean": st.mean(t["user"] for t in T),
               "sys_s_mean": st.mean(t["sys"] for t in T), "rss_mb_peak": max(t["rss_mb"] for t in T),
               "output_csv": os.path.basename(csv),
               "output_sha256_16": hashlib.sha256(arr.tobytes()).hexdigest()[:16]}
        rec.update(stats(arr, c["kind"]))
        manifest["cases"][c["id"]] = rec
        print(f"{c['id']:16s} {rec['points']:7d} pts  wall {rec['wall_s_median']:7.3f} s  "
              f"rss {rec['rss_mb_peak']:5.1f} MB", flush=True)

    # accuracy against references
    acc = {}
    c = 2.99792458e8
    def lorentz(lam):
        w0 = 2 * np.pi * c / 1550e-9; gi = w0 / 4e4; gc = w0 / 4.4e4
        dl = 10 / np.pi * np.sin(np.pi * (lam - 1550) / 10)
        dw = 2 * np.pi * c * dl * 1e-9 / (lam * 1550 * 1e-18)
        return np.abs((gi - gc - 1j * dw) / (gi + gc - 1j * dw)) ** 2
    def load(i):
        with gzip.open(os.path.join(a.out, i + ".csv.gz"), "rt") as g:
            return np.loadtxt(g, delimiter=",", skiprows=1)
    for i, r in manifest["cases"].items():
        d = load(i)
        if i.endswith("dc_default") or i.endswith("dc_tight"):
            P = d[:, 1] ** 2 + d[:, 2] ** 2
            acc[i] = {"vs": "closed-form Lorentzian", "max_abs_dP": float(np.max(np.abs(P - lorentz(d[:, 0]))))}
    if "ng_tran_tight" in manifest["cases"]:
        ref = load("ng_tran_tight"); tg = np.arange(0.1e-9, 10.15e-9, 0.05e-12)
        Pr = np.interp(tg, ref[:, 0], ref[:, 2] ** 2 + ref[:, 3] ** 2); Pm = Pr.mean()
        Vr = np.interp(tg, ref[:, 0], ref[:, 1])
        for i, r in manifest["cases"].items():
            if "tran" in i and i != "ng_tran_tight":
                d = load(i); P = np.interp(tg, d[:, 0], d[:, 2] ** 2 + d[:, 3] ** 2); V = np.interp(tg, d[:, 0], d[:, 1])
                acc[i] = {"vs": "ng_tran_tight", "rms_dP_over_Pmean": float(np.sqrt(np.mean((P - Pr) ** 2)) / Pm),
                          "max_dP_over_Pmean": float(np.max(np.abs(P - Pr)) / Pm),
                          "rms_dV_kk": float(np.sqrt(np.mean((V - Vr) ** 2)))}
    manifest["accuracy"] = acc
    json.dump(manifest, open(os.path.join(a.out, "manifest.json"), "w"), indent=2)
    shutil.rmtree(work, ignore_errors=True)
    print("wrote", os.path.join(a.out, "manifest.json"))

if __name__ == "__main__":
    main()
