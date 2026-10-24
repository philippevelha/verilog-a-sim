#!/usr/bin/env python3
"""One circuit description -> ngspice (OpenVAF/OSDI) deck and verilog-a-sim deck; runs both
under /usr/bin/time and returns the parsed waveforms plus wall time and peak RSS.

Circuit = list of elements:
    ("V", name, n_plus, n_minus, spec)              spec: "DC 0 AC 1", "PWL(...)", "DC 1.0"
    ("R", name, n1, n2, value)
    ("X", name, [nodes], module, {params})          compiled Verilog-A instance
Analysis: ("dc", src, start, stop, step) | ("ac", npd, f1, f2) | ("tran", tstep, tstop)
Outputs:  list of probes: node names "out", or internal "X1.pmon" (instance.node).
"""
import os, re, shutil, subprocess, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
NG = os.environ.get("NGSPICE", "/opt/ngspice-new/bin/ngspice")
VACLI = os.environ.get("VACLI", "/home/claude/verilog-a-sim/target/release/va-cli")
OPENVAF = os.environ.get("OPENVAF", "/home/claude/arpadbuermen/openvaf/target/release/openvaf-r")
MODELS = ["mrm_eqc_rlc.va", "mrm_eqc_ls.va"]

# ------------------------------------------------------------------ parameter cards
SINGLE = dict(nseg=1, Cpad1=38.7e-15, Rs1=192.8, Rsi1=12285.0, Csi1=29.6e-15, Cj1=12.1e-15,
              lam0=1553.0632, tau_e=23.86e-12, tl0=21.32, tl1=0.56, tl2=-5.25e-2,
              c1=26.103, c2=-2.1816, s1=1.0, s2=0.0)
DUAL = dict(nseg=2, Cpad1=38.7e-15, Rs1=183.0, Rsi1=10391.0, Csi1=55.6e-15, Cj1=9.1e-15,      # MSB
            Cpad2=38.7e-15, Rs2=144.1, Rsi2=14775.0, Csi2=25.78e-15, Cj2=8.0e-15,             # LSB
            lam0=1551.3957, tau_e=27.14e-12, tl0=22.89, tl1=0.2825, tl2=-2.875e-2,
            c1=26.103, c2=-2.1816, s1=0.407, s2=0.642)
LS_ONLY = dict(FSR=None)          # FSR differs: single 8 nm, dual 6.5 nm (set by caller)

def workdir(tag):
    w = os.path.join(HERE, "work", tag)
    os.makedirs(w, exist_ok=True)
    return w

_built = {}
def build_osdi(w):
    """OpenVAF in a clean directory (verilog-a-sim's headers must not shadow OpenVAF's)."""
    ov = os.path.join(HERE, "work", "_openvaf")
    if not _built:
        shutil.rmtree(ov, ignore_errors=True); os.makedirs(ov)
        for m in MODELS:
            shutil.copy(os.path.join(ROOT, m), ov)
            subprocess.run([OPENVAF, m], cwd=ov, capture_output=True, check=True)
        _built["ok"] = True
    for m in MODELS:
        shutil.copy(os.path.join(ov, m.replace(".va", ".osdi")), w)

def prep_va(w):
    for m in MODELS:
        shutil.copy(os.path.join(ROOT, m), w)
    for h in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(ROOT, "vasim", h), w)

def fmt(v):
    return f"{v:.10g}" if isinstance(v, (float, int)) else str(v)

def deck_ngspice(circ, ana, probes, datfile, opts="reltol=1e-5"):
    L = ["* generated", ""]
    for e in circ:
        if e[0] == "X":
            _, name, nodes, mod, par = e
            L.append(f".model m_{name} {mod} " + " ".join(f"{k}={fmt(v)}" for k, v in par.items()))
            L.append(f"N{name} " + " ".join("0" if n in ("0", "gnd") else n for n in nodes) + f" m_{name}")
        elif e[0] == "V":
            L.append(f"V{e[1]} {e[2]} {e[3] if e[3] not in ('gnd',) else '0'} {e[4]}")
        elif e[0] == "R":
            L.append(f"R{e[1]} {e[2]} {e[3] if e[3] != 'gnd' else '0'} {fmt(e[4])}")
    L += [".control"] + [f"pre_osdi {m.replace('.va', '.osdi')}" for m in MODELS]
    L += ["set wr_singlescale", "set wr_vecnames", f"option numdgt=12 {opts}"]
    def ng(p):
        if "." in p:
            inst, node = p.split("."); return f"n{inst.lower().lstrip('x')}#{node}"
        return p
    if ana[0] == "dc":
        L.append(f"dc V{ana[1]} {ana[2]} {ana[3]} {ana[4]}")
        L.append(f"wrdata {datfile} " + " ".join(f"v({ng(p)})" for p in probes))
    elif ana[0] == "ac":
        L.append(f"ac dec {ana[1]} {ana[2]} {ana[3]}")
        L.append(f"wrdata {datfile} " + " ".join(f"vm({ng(p)}) vp({ng(p)})" for p in probes))
    elif ana[0] == "tran":
        L.append(f"tran {ana[1]} {ana[2]} 0 {ana[1]}")
        L.append(f"wrdata {datfile} " + " ".join(f"v({ng(p)})" for p in probes))
    L += ["quit", ".endc", ".end"]
    return "\n".join(L) + "\n"

def deck_va(circ, ana):
    L = ["* generated"]
    for e in circ:
        if e[0] == "X":
            _, name, nodes, mod, par = e
            L.append(f"X{name} " + " ".join("gnd" if n in ("0", "gnd") else n for n in nodes) + f" {mod} "
                     + " ".join(f"{k}={fmt(v)}" for k, v in par.items()))
        elif e[0] == "V":
            spec = e[4]
            L.append(f"V{e[1]} {e[2]} {'gnd' if e[3] in ('0', 'gnd') else e[3]} {spec}")
        elif e[0] == "R":
            L.append(f"R{e[1]} {e[2]} {'gnd' if e[3] in ('0', 'gnd') else e[3]} {fmt(e[4])}")
    if ana[0] == "dc":
        L.append(f".dc V{ana[1]} {ana[2]} {ana[3]} {ana[4]}")
    elif ana[0] == "ac":
        L.append(f".ac dec {ana[1]} {ana[2]} {ana[3]}")
    elif ana[0] == "tran":
        L.append(f".tran {ana[1]} {ana[2]}")
    L.append(".end")
    return "\n".join(L) + "\n"

def timed(cmd, cwd, env=None):
    e = dict(os.environ); e.update(env or {})
    tf = os.path.join(cwd, ".time")
    p = subprocess.run(["/usr/bin/time", "-f", "%e %U %S %M", "-o", tf] + cmd, cwd=cwd, env=e,
                       capture_output=True, text=True)
    w, u, s, m = open(tf).read().split()[-4:]
    return p, dict(wall_s=float(w), user_s=float(u), sys_s=float(s), rss_mb=int(m) / 1024)

NUM = r"([-+\d.eE]+)"

def run(sim, tag, circ, ana, probes, va_flags=(), ng_opts="reltol=1e-5"):
    """Returns dict(x=..., y={probe: array (real) or complex}, time=..., points=...)."""
    w = workdir(f"{tag}_{sim}")
    if sim == "ngspice":
        build_osdi(w)
        open(os.path.join(w, ".spiceinit"), "w").write("set num_threads=1\n")
        open(os.path.join(w, "deck.cir"), "w").write(deck_ngspice(circ, ana, probes, "out.dat", ng_opts))
        p, tm = timed([NG, "-b", "deck.cir"], w)
        d = np.loadtxt(os.path.join(w, "out.dat"), skiprows=1, ndmin=2)
        x = d[:, 0]; y = {}
        if ana[0] == "ac":
            for i, pr in enumerate(probes):
                mag, ph = d[:, 1 + 2 * i], d[:, 2 + 2 * i]
                y[pr] = mag * np.exp(1j * ph)
        else:
            for i, pr in enumerate(probes):
                y[pr] = d[:, 1 + i]
    else:
        prep_va(w)
        open(os.path.join(w, "deck.net"), "w").write(deck_va(circ, ana))
        flag = {"dc": [], "ac": ["--ac"], "tran": ["--tran"]}[ana[0]]
        p, tm = timed([VACLI, "sim", "deck.net", "--model", ".", *flag, "--report", ",".join(probes), *va_flags],
                      w, env={"VA_JIT": "on"})
        out = p.stdout
        if "error[" in out or "error[" in p.stderr:
            raise RuntimeError((out + p.stderr)[-2000:])
        xs, ys = [], {pr: [] for pr in probes}
        if ana[0] == "ac":
            for line in out.splitlines():
                m = re.match(r"\s*f=" + NUM + r"Hz\s+(.*)", line)
                if not m: continue
                xs.append(float(m.group(1)))
                for pr in probes:
                    mm = re.search(r"V\(" + re.escape(pr) + r"\)=" + NUM + r" V∠" + NUM + "°", m.group(2))
                    ys[pr].append(float(mm.group(1)) * np.exp(1j * np.deg2rad(float(mm.group(2)))))
        else:
            key = "t=" if ana[0] == "tran" else f"V{ana[1]}="
            for line in out.splitlines():
                m = re.match(r"\s*" + re.escape(key) + NUM + r"s?:?\s+(.*)", line)
                if not m: continue
                vals = []
                for pr in probes:
                    mm = re.search(r"V\(" + re.escape(pr) + r"\)=" + NUM, m.group(2))
                    vals.append(float(mm.group(1)) if mm else np.nan)
                xs.append(float(m.group(1)))
                for pr, v in zip(probes, vals): ys[pr].append(v)
        x = np.array(xs); y = {k: np.array(v) for k, v in ys.items()}
    return dict(x=x, y=y, time=tm, points=len(x), sim=sim)
