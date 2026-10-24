#!/usr/bin/env python3
"""Check the TCMT optical core of mrm_pin.va against Wang et al. 2016 Eq. (4) -- the dynamic
ring as a sum over round trips -- driven by the SAME carrier waveform.  Eq. (4) is evaluated
as its exact recursion on a grid of one round trip tau_res:
    U_k = sigma a_k exp(j Phi_{k-1}) (1 + U_{k-1}),   T_k = sigma - (kappa^2/sigma) U_k
which sums to Eq. (4) and reduces to Eq. (5) in steady state.  Phi = 2 pi (n_fit + dn(t)) L / lambda_L,
a = a0 exp(-dalpha(t) L / 2);  n_fit puts the 0 V resonance at lambda0 (as the model does).
    python3 eq4_check.py <case_tag.npz>
"""
import sys, numpy as np
f = sys.argv[1] if len(sys.argv) > 1 else "fig13b_pe80_src0.npz"
z = np.load(f); t, P_tcmt, X = z["t"], z["P"], z["X"]
c = 2.99792458e8; QE = 1.602176634e-19
sigma, a0, neff, R, lam0 = 0.9944, 0.9931, 2.5188, 5e-6, 1318.812e-9
tau, Vact, ce, ch, ae, ah, Nfl = 1e-9, 4.408e-12, 6.2e-22, 6.0e-18, 6.0e-18, 4.0e-18, 1e14
lamL = lam0; L = 2 * np.pi * R; tres = neff * L / c
m = np.round(neff * L / lam0); n_fit = m * lam0 / L
dN = tau * 1e-3 * X / QE / Vact
Np = 0.5 * (dN + np.sqrt(dN ** 2 + Nfl ** 2)); Np0 = 0.5 * Nfl
dn = -(ce * (Np - Np0) + ch * (Np ** 0.8 - Np0 ** 0.8)); dal = (ae + ah) * (Np - Np0) * 100.0
tk = np.arange(t[0], t[-1], tres)
dnk = np.interp(tk, t, dn); dalk = np.interp(tk, t, dal)
phi = 2 * np.pi * (n_fit + dnk) * L / lamL
ak = a0 * np.exp(-dalk * L / 2)
kap2 = 1 - sigma ** 2
U = np.zeros(len(tk), complex)
# start from the steady state at t0
e0 = ak[0] * np.exp(1j * phi[0]); U[0] = sigma * e0 / (1 - sigma * e0)
for k in range(1, len(tk)):
    U[k] = sigma * ak[k] * np.exp(1j * phi[k - 1]) * (1 + U[k - 1])
T = sigma - kap2 / sigma * U
P_eq4 = np.abs(T) ** 2 * 1e3
Pt = np.interp(tk, t, P_tcmt)
sel = tk > 2e-9
d = Pt[sel] - P_eq4[sel]
print(f"{f}: {sel.sum()} round trips compared; |T|^2 x 1 mW")
print(f"  RMS difference {np.sqrt(np.mean(d**2)):.2f} uW, max |diff| {np.abs(d).max():.2f} uW, "
      f"signal range {P_eq4[sel].min():.1f}..{P_eq4[sel].max():.1f} uW")
np.savez(f.replace(".npz", "_eq4.npz"), t=tk, P_eq4=P_eq4, P_tcmt=Pt)
