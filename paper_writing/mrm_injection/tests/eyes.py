#!/usr/bin/env python3
"""Optical eyes of Wang et al. 2016 (Figs. 12-15, 8 Gb/s; Fig. 21a, 15 Gb/s) with the paper's drive conditions:
PRBS7, 2 Vpp, 40 ps transitions, laser at the 0 V resonance, ideal voltage source.
Pre-emphasis waveform (Fig. 11): peak levels bias +/- 1 V for `duration`, then steady
levels peak -/+ depth (e.g. 1.7 / 0.9 / -0.3 / 0.5 V for 0.8 V depth, 0.7 V bias).

    python3 eyes.py [--sim ngspice|va]
"""
import json, os, re, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
NG = os.environ.get("NGSPICE", "/opt/ngspice-new/bin/ngspice")
VACLI = os.environ.get("VACLI", "/home/claude/verilog-a-sim/target/release/va-cli")
LAM0 = 1318.812

def prbs7(n):
    s = 0x7F; out = []
    for _ in range(n):
        b = ((s >> 6) ^ (s >> 5)) & 1; s = ((s << 1) | b) & 0x7F; out.append(b)
    return out

def waveform(bits, UI, hi_pk, hi_ss, lo_pk, lo_ss, dur, tr=40e-12):
    """Pre-emphasis NRZ built like the combiner of Fig. 11: main data (swing hi_pk - lo_pk)
    plus a copy delayed by `dur` that takes the level from peak to steady state. Every edge
    is a linear ramp of duration tr; overlapping ramps add (superposition), so dur < tr
    gives a peak that is not reached, as with real hardware.  Asymmetric levels allowed
    (Fig. 19): the delayed copy steps by (hi_pk - hi_ss) on rising and (lo_ss - lo_pk) on
    falling transitions.  dur = 0 -> plain NRZ between hi_pk and lo_pk."""
    def ramp(t, t0):
        return np.clip((t - t0) / tr, 0.0, 1.0)
    edges = []                                    # (time, amplitude)
    for i in range(1, len(bits)):
        if bits[i] != bits[i - 1]:
            t = i * UI
            if bits[i]:
                edges.append((t, hi_pk - lo_ss))
                if dur > 0: edges.append((t + dur, hi_ss - hi_pk))
            else:
                edges.append((t, lo_pk - hi_ss))
                if dur > 0: edges.append((t + dur, lo_ss - lo_pk))
    v0 = (lo_ss if bits[0] == 0 else hi_ss) if dur > 0 else (lo_pk if bits[0] == 0 else hi_pk)
    if dur == 0:   # plain NRZ: swing hi_pk <-> lo_pk
        edges = [(t, (hi_pk - lo_pk) * np.sign(a)) for t, a in edges]
    tb = sorted({0.0, len(bits) * UI} | {e[0] for e in edges} | {e[0] + tr for e in edges})
    tb = np.array([x for x in tb if x <= len(bits) * UI])
    v = v0 + sum(a * ramp(tb, t0) for t0, a in edges)
    return list(zip(tb, v))

CASES = {
    "fig12_nrz":     dict(rate=8e9,  hi_pk=1.7, hi_ss=1.7, lo_pk=-0.3, lo_ss=-0.3, dur=0.0),
    "fig13a_pe40":   dict(rate=8e9,  hi_pk=1.7, hi_ss=0.9, lo_pk=-0.3, lo_ss=0.5, dur=40e-12),
    "fig13b_pe80":   dict(rate=8e9,  hi_pk=1.7, hi_ss=0.9, lo_pk=-0.3, lo_ss=0.5, dur=80e-12),
    "fig14a_d09":    dict(rate=8e9,  hi_pk=1.7, hi_ss=0.8, lo_pk=-0.3, lo_ss=0.6, dur=80e-12),
    "fig14b_d07":    dict(rate=8e9,  hi_pk=1.7, hi_ss=1.0, lo_pk=-0.3, lo_ss=0.4, dur=80e-12),
    "fig15a_b075":   dict(rate=8e9,  hi_pk=1.75, hi_ss=0.95, lo_pk=-0.25, lo_ss=0.55, dur=80e-12),
    "fig15b_b065":   dict(rate=8e9,  hi_pk=1.65, hi_ss=0.85, lo_pk=-0.35, lo_ss=0.45, dur=80e-12),
    # Section IV, Fig. 21(a): 15 Gb/s, pulse "close to the 66.7 ps bit period" (66 ps used here)
    "fig21a_sym":    dict(rate=15e9, hi_pk=1.7, hi_ss=0.9, lo_pk=-0.3, lo_ss=0.5, dur=66e-12),
    "fig21a_asym":   dict(rate=15e9, hi_pk=1.7, hi_ss=0.9, lo_pk=-0.3, lo_ss=0.7, dur=66e-12),
}

def run_ngspice(name, pwl, tstop, extra=""):
    deck = f"""* {name}
.model pin mrm_pin {extra}
Va a 0 PWL({" ".join(f"{a:.6e} {b:.4f}" for a, b in pwl)})
Vin_re in_re 0 1.0
Vin_im in_im 0 0
Vwl wl 0 {LAM0}
Vh hp 0 0
N1 a 0 hp 0 in_re in_im th_re th_im dr_re dr_im wl tdt pin
.control
pre_osdi mrm_pin.osdi
set wr_singlescale
set wr_vecnames
option numdgt=10 reltol=1e-5
tran 0.5p {tstop:.4e} 0 1p
let ia = -i(Va)*1e3
wrdata {name}.dat v(a) ia v(th_re) v(th_im) v(n1#xfc)
quit
.endc
.end
"""
    open(os.path.join(HERE, name + ".cir"), "w").write(deck)
    subprocess.run([NG, "-b", name + ".cir"], cwd=HERE, capture_output=True)
    d = np.loadtxt(os.path.join(HERE, name + ".dat"), skiprows=1)
    return dict(t=d[:, 0], v=d[:, 1], i=d[:, 2], er=d[:, 3], ei=d[:, 4], X=d[:, 5])

VA_RE = re.compile(r"t=([-\d.e]+)s\s+V\(a\)=([-\d.e]+) V\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V\s+V\(X1\.xfc\)=([-\d.e]+) V")

def run_va(name, pwl, tstop, extra=""):
    deck = f"""* {name}
Va a gnd PWL({" ".join(f"{a:.6e} {b:.4f}" for a, b in pwl)})
Vre inre gnd DC 1.0
Vim inim gnd DC 0
Vwl wl gnd DC {LAM0}
Vh hp gnd DC 0
X1 a gnd hp gnd inre inim tre tim dre dim wl tdt mrm_pin {extra}
.tran 1p {tstop:.4e}
.end
"""
    wd = os.path.join(HERE, "va_work"); os.makedirs(wd, exist_ok=True)
    for f in ("disciplines.vams", "constants.vams"):
        subprocess.run(["cp", os.path.join(HERE, "..", "vasim", f), wd])
    subprocess.run(["cp", os.path.join(HERE, "..", "mrm_pin.va"), wd])
    open(os.path.join(wd, name + ".net"), "w").write(deck)
    p = subprocess.run([VACLI, "sim", name + ".net", "--model", ".", "--tran", "--report", "a,tre,tim,X1.xfc"],
                       cwd=wd, capture_output=True, text=True, env=dict(os.environ, VA_JIT="on"))
    a = np.array(VA_RE.findall(p.stdout), float)
    if len(a) == 0:
        sys.exit(p.stdout[-1500:] + p.stderr[-1500:])
    return dict(t=a[:, 0], v=a[:, 1], i=None, er=a[:, 2], ei=a[:, 3], X=a[:, 4])

def scope(t, P, f3db=20e9, dt=0.25e-12):
    """4th-order Bessel-Thomson low-pass (the paper's eyes were taken with a 20 GHz optical
    module). Returns the signal on a uniform grid."""
    from scipy import signal
    tu = np.arange(t[0], t[-1], dt); pu = np.interp(tu, t, P)
    b, a = signal.bessel(4, 2 * np.pi * f3db, analog=True, norm="mag")
    bz, az = signal.bilinear(b, a, 1 / dt)
    return tu, signal.lfilter(bz, az, pu, zi=signal.lfilter_zi(bz, az) * pu[0])[0]

def eye_metrics(t, P, bits, UI, skip=16):
    best = None
    for ph in np.arange(0, 2 * UI, UI / 100):
        idx = np.arange(skip, len(bits) - 2); s = np.interp(idx * UI + ph, t, P); k = np.array(bits)[idx]
        h = s[k == 1].min() - s[k == 0].max()
        if best is None or h > best["eye"]:
            best = dict(eye=float(h), ph=float(ph), p1=float(s[k == 1].mean()), p0=float(s[k == 0].mean()))
    best["er_db"] = 10 * np.log10(best["p1"] / best["p0"])
    return best

if __name__ == "__main__":
    sim = sys.argv[sys.argv.index("--sim") + 1] if "--sim" in sys.argv else "ngspice"
    extra = sys.argv[sys.argv.index("--extra") + 1] if "--extra" in sys.argv else ""
    tag = sys.argv[sys.argv.index("--tag") + 1] if "--tag" in sys.argv else sim
    only = sys.argv[sys.argv.index("--only") + 1].split(",") if "--only" in sys.argv else list(CASES)
    bits = prbs7(127) * 2
    out = {}
    for name, c in CASES.items():
        if name not in only:
            continue
        UI = 1 / c["rate"]
        pwl = waveform(bits, UI, c["hi_pk"], c["hi_ss"], c["lo_pk"], c["lo_ss"], c["dur"])
        r = (run_ngspice if sim == "ngspice" else run_va)(f"{name}_{tag}", pwl, len(bits) * UI, extra)
        P = (r["er"] ** 2 + r["ei"] ** 2) * 1e3
        m = eye_metrics(r["t"], P, bits, UI)
        ts, Ps = scope(r["t"], P)
        ms = eye_metrics(ts, Ps, bits, UI)
        out[name] = dict(raw=m, scope20G=ms)
        np.savez(os.path.join(HERE, f"{name}_{tag}.npz"), t=r["t"], v=r["v"], P=P, X=r["X"], bits=bits, UI=UI)
        print(f"{tag:10s} {name:13s}: eye {m['eye']:7.1f} uW, P1 {m['p1']:6.1f}, P0 {m['p0']:6.1f} uW, ER {m['er_db']:5.2f} dB"
              f" | 20 GHz scope: eye {ms['eye']:7.1f} uW, ER {ms['er_db']:5.2f} dB | {len(r['t'])} points", flush=True)
    json.dump(out, open(os.path.join(HERE, f"eyes_{tag}.json"), "w"), indent=1)
