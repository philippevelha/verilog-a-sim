#!/usr/bin/env python3
"""Chromatic dispersion inside the envelope band as a chain of all-pass sections.

A fibre's dispersion is the all-pass response exp(j*b2L*W^2/2): unit magnitude, group delay
linear in the envelope frequency W. It is not causal without its bulk delay and not rational,
so it cannot be written as a Verilog-A filter directly. A lossless all-pass ring is a
first-order complex all-pass,

    H_k(W) = -(1 - j (W - d_k)/g_k) / (1 + j (W - d_k)/g_k),   delay 2 g_k / ((W - d_k)^2 + g_k^2),

and a chain of N of them, with resonances d_k staggered across the band and widths g_k fitted,
gives a group delay that follows tau0 + slope*W inside the band (this is how ring-based
dispersion compensators work: Madsen and Lenz, PTL 10, 1998). Outside the fitted band the
delay falls back to zero: the approximation has a hard bandwidth, reported by `design`.

    poles = design(disp_ps_nm_km, length_m, band_hz, n)
    lines = chain(poles, "X", "in", "out", "nu", nu0)        # netlist lines of mwp_ring
"""
import numpy as np
from scipy.optimize import least_squares

C0 = 2.99792458e8


def slope_s_per_hz(disp, length, nu0_thz):
    """d(group delay)/d(envelope frequency), s/Hz: -D L lambda^2 / c."""
    lam = C0 / (nu0_thz * 1e12)
    return -disp * 1e-6 * length * lam * lam / C0


def delay(poles, f):
    """Group delay (s) of the chain at envelope frequencies f (Hz). poles: rows (d_k, g_k) in Hz."""
    d, g = poles[:, 0][:, None], poles[:, 1][:, None]
    return np.sum(2 * g / ((f[None, :] - d) ** 2 + g ** 2), axis=0) / (2 * np.pi)


def response(poles, f):
    """Field transfer of the chain at envelope frequencies f (Hz)."""
    h = np.ones_like(f, dtype=complex)
    for d, g in poles:
        x = (f - d) / g
        h *= -(1 - 1j * x) / (1 + 1j * x)
    return h


def design(disp, length, band, n, nu0_thz=193.4):
    """Fit n all-pass sections so that the group delay is tau0 + slope*f for |f| <= band/2.
    Returns (poles [(detuning Hz, half-width Hz)], info dict with the rms and peak delay error)."""
    s = slope_s_per_hz(disp, length, nu0_thz)
    f = np.linspace(-band / 2, band / 2, 801)
    d0 = np.linspace(-0.55 * band, 0.55 * band, n)
    g0 = np.full(n, 0.6 * 1.1 * band / n)

    def unpack(p):
        return np.stack([p[:n], np.abs(p[n:2 * n])], axis=1), p[2 * n]

    def resid(p):
        poles, t0 = unpack(p)
        return (delay(poles, f) - (t0 + s * f)) / abs(s * band)

    p0 = np.concatenate([d0, g0, [n / (1.1 * band)]])
    sol = least_squares(resid, p0, x_scale=np.concatenate([np.full(2 * n, band / n), [1 / band]]), xtol=1e-14, ftol=1e-14, gtol=1e-14)
    poles, t0 = unpack(sol.x)
    err = delay(poles, f) - (t0 + s * f)
    return poles, dict(tau0=t0, slope=s, rms=float(np.sqrt(np.mean(err ** 2))), peak=float(np.max(abs(err))), span=abs(s * band))


def chain(poles, prefix, bus_in, bus_out, tag_net, nu0_thz=193.4):
    """Netlist lines: the chain as mwp_ring all-pass instances between two optical buses."""
    lines, prev = [], bus_in
    for k, (d, g) in enumerate(poles):
        nxt = bus_out if k == len(poles) - 1 else f"{prefix}n{k}"
        nu_res = nu0_thz + d * 1e-12
        q1 = 2 * np.pi * nu_res * 1e12 / (2 * 2 * np.pi * g)          # 1/tau = g (rad/s) = w/(2 q1)
        lines.append(f"{prefix}{k} {prev}_re {prev}_im {nxt}_re {nxt}_im {prefix}d{k}_re {prefix}d{k}_im {tag_net} gnd "
                     f"mwp_ring nu_res={nu_res:.12f} q1={q1:.9e}")
        prev = nxt
    return lines


if __name__ == "__main__":
    for n in (4, 8, 12, 16):
        poles, info = design(17.0, 25e3, 32e9, n)
        print(f"n = {n:2d}: delay span {info['span'] * 1e12:.1f} ps, rms error {info['rms'] * 1e12:.3f} ps, "
              f"peak {info['peak'] * 1e12:.3f} ps, bulk delay {info['tau0'] * 1e12:.0f} ps")
