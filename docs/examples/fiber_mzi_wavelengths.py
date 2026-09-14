#!/usr/bin/env python3
"""Fiber MZI thermal phase noise at three wavelengths — the paper's Fig. 3, reproduced.

Runs `circuits/fiber_mzi_noise.net` three times through `va-cli`, with the laser wavelength
and the wavelength-dependent fiber parameters (mode-field radius `w0`, thermo-optic coefficient
`dndT`, the PSD's `lambda0`) set per wavelength, and plots the input-referred spectrum — the
interferometer's phase noise in dB re rad/sqrt(Hz) — against the closed forms the waveguide
tabulates (Wanser + Duan, as in `models/waveguide.va`), plus the differences between
wavelengths the way Bartolo et al. (IEEE JQE 48, 2012) plot them in Fig. 3(b).

Per-wavelength parameters (Fibercore SM1500 [5.3/80], the paper's fiber):
  * dn/dT from the paper's eq. (A1) (Corning HPFS dispersion of the thermo-optic coefficient):
    9.974e-6 (633 nm), 9.527e-6 (1310 nm), 9.488e-6 (1550 nm).
  * w0: the paper's Table II gives 2.35 um at 1319 nm and 2.605 um at 1550 nm; 1310 nm takes
    the 1319 nm value (0.7 % in wavelength). 633 nm is *outside this fiber's single-mode range*
    (V = 7.1 against the 2.405 cutoff; a 5.3 um core is single-mode above ~1.3 um), so its w0
    is the fundamental mode's Marcuse estimate, 1.95 um — the same Marcuse fit reproduces the
    paper's 1319 nm value within 2.5 % — and the curve is "what Wanser's formula gives for the
    LP01 mode of this fiber at 633 nm", not a single-mode measurement anyone could make on it.

Usage, from the repository root (writes docs/examples/fiber_mzi_wavelengths.svg):
    python docs/examples/fiber_mzi_wavelengths.py
"""
import math
import re
import subprocess
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
DECK = ROOT / "circuits" / "fiber_mzi_noise.net"
OUT = ROOT / "docs" / "examples" / "fiber_mzi_wavelengths.svg"

# (label, lambda [m], w0 [m], dn/dT [1/K], categorical slot colour — validated set, fixed order)
CASES = [
    ("633 nm", 633e-9, 1.95e-6, 9.974e-6, "#2a78d6"),
    ("1310 nm", 1310e-9, 2.35e-6, 9.527e-6, "#eb6834"),
    ("1550 nm", 1550e-9, 2.605e-6, 9.488e-6, "#1baf7a"),
]
L_ARM = 40.0  # m, each arm
KB = 1.380649e-23


def wanser(f, lam, w0, dndt, L=L_ARM, T=295.0, kappa=1.37, D=0.82e-6, af=40e-6, n=1.457,
           aL=5.0e-7):
    """One-sided phase-noise PSD, rad^2/Hz — models/waveguide.va's table entry, closed form."""
    kmax4, kmin4 = (2.0 / w0) ** 4, (2.405 / af) ** 4
    wd = (2 * math.pi * f / D) ** 2
    a = 2 * math.pi * KB * T * T * L * (dndt + n * aL) ** 2 / (kappa * lam * lam)
    return a * math.log((kmax4 + wd) / (kmin4 + wd))


def duan(f, lam, L=L_ARM, n=1.457, T=295.0, E0=1.9e10, phi0=1e-2, dcoat=160e-6):
    area = math.pi * (dcoat / 2) ** 2
    return (2 * math.pi * n / lam) ** 2 * 2 * KB * T * L * phi0 / (3 * math.pi * E0 * area) / f


def run(lam, w0, dndt):
    """Run the deck at one wavelength; return (f, Sin) from va-cli's noise report."""
    src = DECK.read_text(encoding="utf-8")
    src = re.sub(r"lambda=1319e-9", f"lambda={lam:.6e}", src)
    src = src.replace("L=40 lambda0=1319e-9 w0=2.35e-6 dndT=9.52e-6",
                      f"L=40 lambda0={lam:.6e} w0={w0:.4e} dndT={dndt:.4e}")
    assert src.count(f"lambda0={lam:.6e}") == 2, "both waveguide lines must be rewritten"
    tmp = OUT.with_suffix(f".{int(lam * 1e9)}.net")
    tmp.write_text(src, encoding="utf-8")
    try:
        out = subprocess.run(
            ["cargo", "run", "-q", "--release", "-p", "va-cli", "--", "sim", str(tmp),
             "--model", str(ROOT / "models"), "--noise"],
            cwd=ROOT, capture_output=True, text=True, check=True).stdout
    finally:
        tmp.unlink(missing_ok=True)
    pts = re.findall(r"f=([0-9.e+-]+)Hz .* Sin=([0-9.e+-]+) V\^2/Hz", out)
    if not pts:
        sys.exit(f"no noise points parsed from va-cli output:\n{out}")
    return [float(a) for a, _ in pts], [float(b) for _, b in pts]


def db(x):
    return 10 * math.log10(x)


def main():
    runs = {}
    for label, lam, w0, dndt, colour in CASES:
        f, sin = run(lam, w0, dndt)
        runs[label] = (f, sin, lam, w0, dndt, colour)
        print(f"{label}: S_in at 1 Hz {db(sin[0]):.2f} dB, 1 kHz {db(sin[30]):.2f} dB, "
              f"100 kHz {db(sin[-1]):.2f} dB re rad/sqrt(Hz)")

    ink, muted, grid = "#0b0b0b", "#52514e", "#e6e5e1"
    fig, (ax, axd) = plt.subplots(2, 1, figsize=(8.2, 7.4), height_ratios=[3, 1.6],
                                  sharex=True, facecolor="#fcfcfb")
    for a in (ax, axd):
        a.set_facecolor("#fcfcfb")
        a.grid(True, which="major", color=grid, linewidth=0.8)
        a.tick_params(colors=muted, labelsize=9)
        for s in a.spines.values():
            s.set_color(grid)

    # (a) spectra: simulation solid, closed form dashed, same hue per wavelength.
    for label, (f, sin, lam, w0, dndt, colour) in runs.items():
        ax.semilogx(f, [db(s) for s in sin], color=colour, linewidth=2, label=f"{label} — va-cli")
        # Closed form drawn *on top* in ink, thin and dashed: it sits within 1 % of the
        # simulation, so in the series colour it would simply vanish under the solid line.
        cf = [db(2 * (wanser(x, lam, w0, dndt) + duan(x, lam))) for x in f]
        ax.semilogx(f, cf, color=ink, linewidth=0.9, linestyle=(0, (4, 4)), zorder=3)
        ax.annotate(label, (f[-1], db(sin[-1])), xytext=(6, 0), textcoords="offset points",
                    color=ink, fontsize=9, va="center")
    ax.axhline(-125.5, color=muted, linewidth=1, linestyle=(0, (1, 2)))
    ax.text(1.3, -125.5 - 1.7, "−125.5 dB: paper's Wanser F(0), 1319 nm, 80 m",
            color=muted, fontsize=8)
    ax.plot([], [], color=ink, linewidth=0.9, linestyle=(0, (4, 4)),
            label="closed form, 2·(Wanser + Duan), 40 m per arm")
    ax.set_ylabel("phase noise  (dB re rad/√Hz)", color=ink)
    ax.set_title("Fiber MZI thermal phase noise vs wavelength (input-referred, "
                 "circuits/fiber_mzi_noise.net)", color=ink, fontsize=10, loc="left")
    ax.legend(frameon=False, fontsize=8.5, labelcolor=ink, loc="upper right")
    ax.set_xlim(1, 2e5)

    # (b) differences relative to 1550 nm, as the paper's Fig. 3(b); the λ-only scaling
    # 20·log10(1550/λ) is the dotted reference — the remainder is w0 and dn/dT.
    f_ref, sin_ref = runs["1550 nm"][0], runs["1550 nm"][1]
    for label, (f, sin, lam, w0, dndt, colour) in runs.items():
        if label == "1550 nm":
            continue
        diff = [db(a) - db(b) for a, b in zip(sin, sin_ref)]
        axd.semilogx(f, diff, color=colour, linewidth=2)
        lam_only = 20 * math.log10(1550e-9 / lam)
        axd.axhline(lam_only, color=colour, linewidth=1, linestyle=(0, (1, 2)))
        axd.annotate(f"{label} − 1550 nm   (λ-only: {lam_only:.2f} dB)", (f[-1], diff[-1]),
                     xytext=(6, 0), textcoords="offset points", color=ink, fontsize=9,
                     va="center")
    axd.set_ylabel("difference  (dB)", color=ink)
    axd.set_xlabel("frequency  (Hz)", color=ink)
    fig.subplots_adjust(right=0.72, hspace=0.08, top=0.94, bottom=0.08, left=0.1)
    fig.savefig(OUT, format="svg")
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
