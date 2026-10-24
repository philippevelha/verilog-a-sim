#!/usr/bin/env python3
"""Wang 2016 Eq. (4), the ring's delayed-feedback form, run as Verilog-A in verilog-a-sim (>= 2.44.0,
absdelay in transient) against the TCMT form of mrm_pin.va, on the 10 Gb/s clock of tb_fig7.cir
(tolcheck/f7_va.net: -0.5..1.5 V, 40 ps edges, 6 ns, 1 mW at 1318.813 nm).

mrm_pin_eq4.va is generated from mrm_pin.va by make_eq4.py: the same electrical network, free
carriers and plasma dispersion, with only the ring rewritten. So the comparison isolates the two
ways of writing the ring. eq4_check.py did the same comparison in Python, driven by the TCMT run's
carrier waveform (1.7-2.2 uW RMS); this one runs both forms end to end through the simulator.

    python3 eq4_va_check.py        -> eq4_va_check.json      (VACLI = path to va-cli)
"""
import json, os, re, shutil, subprocess, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
VACLI = os.environ.get("VACLI", "va-cli")


def run(module, work):
    os.makedirs(work, exist_ok=True)
    shutil.copy(os.path.join(ROOT, "mrm_pin.va"), work)
    shutil.copy(os.path.join(HERE, "mrm_pin_eq4.va"), work)
    for h in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(ROOT, "vasim", h), work)
    deck = open(os.path.join(HERE, "tolcheck", "f7_va.net")).read()
    deck = re.sub(r" mrm_pin$", f" {module}", deck, flags=re.M)
    open(os.path.join(work, "deck.net"), "w").write(deck)
    t0 = time.perf_counter()
    out = subprocess.run([VACLI, "sim", "deck.net", "--model", ".", "--tran", "--report", "th_re,th_im"],
                         cwd=work, capture_output=True, text=True, env=dict(os.environ, VA_JIT="on"))
    wall = time.perf_counter() - t0
    a = np.array([[float(x) for x in re.findall(r"=([-+\d.eE]+)", l)]
                  for l in out.stdout.splitlines() if l.startswith("  t=")])
    if len(a) == 0:
        raise SystemExit(f"{module}: no output\n{(out.stdout + out.stderr)[-800:]}")
    return a[:, 0], (a[:, 1] ** 2 + a[:, 2] ** 2) * 1e3, wall      # uW


if __name__ == "__main__":
    work = os.path.join(HERE, "eq4_work")
    t1, p1, w1 = run("mrm_pin", os.path.join(work, "tcmt"))
    t2, p2, w2 = run("mrm_pin_eq4", os.path.join(work, "eq4"))
    sel = t1 > 2e-9                                   # past the start-up, as eq4_check.py
    d = np.interp(t1, t2, p2)[sel] - p1[sel]
    res = dict(points_tcmt=len(t1), points_eq4=len(t2), wall_tcmt_s=w1, wall_eq4_s=w2,
               P_min_uW=float(p1[sel].min()), P_max_uW=float(p1[sel].max()),
               rms_uW=float(np.sqrt(np.mean(d ** 2))), max_uW=float(np.abs(d).max()))
    toml = open(os.path.join(ROOT, "..", "..", "Cargo.toml")).read()
    res["va_cli"] = re.search(r'^version = "([^"]+)"', toml, re.M).group(1)
    json.dump(res, open(os.path.join(HERE, "eq4_va_check.json"), "w"), indent=1)
    print(f"TCMT {len(t1)} points {w1:.1f} s; Eq. (4) {len(t2)} points {w2:.1f} s")
    print(f"through power {res['P_min_uW']:.1f}..{res['P_max_uW']:.1f} uW; Eq. (4) - TCMT after 2 ns: "
          f"RMS {res['rms_uW']:.2f} uW, max {res['max_uW']:.2f} uW")
    shutil.rmtree(work, ignore_errors=True)
