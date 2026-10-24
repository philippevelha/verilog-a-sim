#!/usr/bin/env python3
"""ngspice tolerance matrix for mrm_pin (uA vs mA ladder scaling x reltol), and verilog-a-sim
against the same reference.

Data: ngspice-47 + OpenVAF on Linux, 2026-10-24, decks and logs in uA/ and mA/ (the mA model is
mrm_pin.va with `SC = 1e3). Quantity: terminal current, DC I-V (-1..2 V) and the 10 Gb/s clock of
tb_fig7.cir (6 ns). References: DC - uA at reltol 1e-9; f7 - mA at reltol 1e-9 (181 006 points),
because uA at 1e-9 aborts at t = 0.1 ns ("Timestep too small"). Every comparison prints how much
of the reference interval the run covers: an aborted run must not be compared over its stub.

    python compare.py              # ngspice tables only
    python compare.py --va         # also run f7_va.net in verilog-a-sim (VACLI = path to va-cli)
"""
import gzip, os, re, subprocess, sys, time
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))


def ld(path, col):
    with gzip.open(os.path.join(HERE, path), "rt") as f:
        d = np.loadtxt(f, skiprows=1)
    return d[:, 0], d[:, col]


def err(x, y, xr, yr):
    m = xr <= x[-1]
    yi = np.interp(xr[m], x, y)
    return (x[-1] - xr[0]) / (xr[-1] - xr[0]), np.max(np.abs(yi - yr[m])) / np.max(np.abs(yr)), \
        np.sqrt(np.mean((yi - yr[m]) ** 2) / np.mean(yr[m] ** 2))


for kind, col, ref, lab in (("dc", 1, "uA/dc_uA_1e-9.dat.gz", "DC I-V, -1..2 V"),
                            ("f7", 2, "mA/f7_mA_1e-9.dat.gz", "10 Gb/s current, 6 ns")):
    xr, yr = ld(ref, col)
    print(f"{lab}: reference {ref} ({len(xr)} points); max |dI|/max|I|, RMS")
    for sc in ("uA", "mA"):
        for rt in ("def", "1e-5", "1e-9"):
            x, y = ld(f"{sc}/{kind}_{sc}_{rt}.dat.gz", col)
            cov, mx, rms = err(x, y, xr, yr)
            note = "" if cov > 0.999 else f"   <- covers only {100 * cov:.1f} % (run aborted)"
            print(f"  ngspice {sc} reltol {rt:5s}: max {100 * mx:7.4f} %  RMS {100 * rms:7.4f} %  {len(x):6d} pts{note}")

if "--va" in sys.argv:
    vacli = os.environ.get("VACLI", "va-cli")
    work = os.path.join(HERE, "va_work"); os.makedirs(work, exist_ok=True)
    root = os.path.dirname(os.path.dirname(HERE))                # mrm_injection/
    import shutil
    shutil.copy(os.path.join(root, "mrm_pin.va"), work)
    for h in ("disciplines.vams", "constants.vams"):
        shutil.copy(os.path.join(root, "vasim", h), work)
    shutil.copy(os.path.join(HERE, "f7_va.net"), work)
    xr, yr = ld("mA/f7_mA_1e-9.dat.gz", 2)
    for args in ([], ["--reltol", "1e-4"], ["--reltol", "1e-5"]):
        t0 = time.perf_counter()
        out = subprocess.run([vacli, "sim", "f7_va.net", "--model", ".", "--tran", "--report", "Va", *args],
                             cwd=work, capture_output=True, text=True, env=dict(os.environ, VA_JIT="on")).stdout
        w = time.perf_counter() - t0
        a = np.array(re.findall(r"t=([-+\d.e]+)s\s+I\(Va\)=([-+\d.e]+) A", out), float)
        # verilog-a-sim reports I(Va) into the source's + terminal; the ngspice deck takes -i(Va)
        cov, mx, rms = err(a[:, 0], -a[:, 1] * 1e3, xr, yr)
        print(f"  verilog-a-sim {' '.join(args) or 'defaults':14s}: max {100 * mx:7.4f} %  RMS {100 * rms:7.4f} %  "
              f"{len(a):6d} pts  wall {w:.1f} s  covers {100 * cov:.1f} %")
    shutil.rmtree(work, ignore_errors=True)
