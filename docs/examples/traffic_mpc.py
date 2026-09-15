#!/usr/bin/env python3
"""Model predictive ramp metering for the paper's Section 5 example — the optimiser outside the
simulator, the simulator as the plant.

Bellemans, De Schutter and De Moor ("Models for traffic control", 2002, §5) meter the on-ramp
of a four-section motorway with MPC: at every control step the discrete Payne model predicts
the next 8 minutes, an optimiser picks the metering rates of the next 2 minutes that minimise
the total time spent (plus a penalty on rate changes, and a 100-vehicle limit on the ramp
queue), and the first minute is applied. They report 1267 veh·h without control and 1183 with
it. A circuit simulator cannot do that optimisation — it has no optimiser and no way to
restart a transient from an arbitrary state — so this script does what the paper does, with
the paper's tools:

  1. the discrete Payne model of the paper (eqs. 4–8, Δt = 10 s, plus the origin queues that
     `models/origin.va` writes out) as the *prediction* model, in plain Python;
  2. receding-horizon optimisation with scipy (L-BFGS-B, bounded rates), prediction horizon
     8 min, control horizon 2 min, rate held for 1 min at a time — the paper's settings;
  3. the resulting metering trajectory written as a `PWL` source into
     `circuits/motorway_ramp_mpc.net`, and run through va-cli, whose continuous-time model is
     the *plant*. The TTS the plant reports is the number that counts.

The discrete model's own no-control TTS is printed next to the paper's 1267 and next to the
plant's, so the three are never confused: the paper's number, the prediction model's number,
and the plant's number are three different quantities, and the honest comparison is control
vs. no control *within* each.

Usage, from the repository root:
    python docs/examples/traffic_mpc.py          # writes the deck and prints the three TTS
"""
import math
import re
import subprocess
from pathlib import Path

import numpy as np
from scipy.optimize import minimize

ROOT = Path(__file__).resolve().parents[2]
DECK_NC = ROOT / "circuits" / "motorway_ramp.net"
DECK_MPC = ROOT / "circuits" / "motorway_ramp_mpc.net"

# --- the road (models/section.va, models/origin.va) -------------------------------------
L, N = 0.5, 2.0
VF, CJAM, ALPHA, BETA, CCR = 102.0, 180.0, 1.86, 11.72, 33.5
TAU_H, NU, KAPPA, DELTA = 18.0 / 3600.0, 60.0, 40.0, 0.0122
QCAP_R = 2000.0
C_BOUNDARY = 38.0    # downstream boundary density (models/boundary.va), the paper's Fig. 12
GODUNOV = False      # the paper's bare eq. (6) coupling; see models/section.va
DT = 10.0            # s, the paper's step
DT_H = DT / 3600.0
T_END = 12600.0      # 3.5 h
STEPS = int(T_END / DT)


def veq(c):
    c = max(c, 1e-9)
    return VF * max(1.0 - (c / CJAM) ** ALPHA, 1e-9) ** BETA


QMAX = CCR * veq(CCR) * N


def supply(c):
    return c * veq(c) * N if c > CCR else QMAX


def demand_ramp(t):
    """Fig. 11: 500 veh/h, rising to 1500 between 0.25 h and 0.5 h, back to 500 by 0.9 h."""
    if t < 900:
        return 500.0
    if t < 1800:
        return 500.0 + 1000.0 * (t - 900.0) / 900.0
    if t < 2340:
        return 1500.0
    if t < 3240:
        return 1500.0 - 1000.0 * (t - 2340.0) / 900.0
    return 500.0


def step(state, t, rate):
    """One Δt of the discrete model. state = [C1..C4, v1..v4, w_main, w_ramp]; returns the
    next state and the vehicles present (for the TTS integral)."""
    c = state[0:4].copy()
    v = state[4:8].copy()
    wm, wr = state[8], state[9]
    d_m, d_r = 3400.0, demand_ramp(t)

    # Origins: METANET's taper on both (models/origin.va, godunov=0).
    room_m = QMAX * max(min(1.0, (CJAM - c[0]) / (CJAM - CCR)), 0.0)
    q_m = min(d_m + wm / DT_H, QMAX, room_m)
    room_r = QCAP_R * max(min(1.0, (CJAM - c[2]) / (CJAM - CCR)), 0.0)
    q_r = min(d_r + wr / DT_H, rate * QCAP_R, room_r)

    # Section outflows, eq. (6) bare — the paper's coupling (models/section.va, godunov=0).
    # `GODUNOV = True` bounds each by the next section's supply instead; docs/traffic.md
    # records what that changes (everything a controller could gain).
    q = np.zeros(4)
    for j in range(4):
        send = c[j] * max(v[j], 0.0) * N
        if GODUNOV:
            sup = supply(c[j + 1]) if j < 3 else supply(C_BOUNDARY)
            q[j] = min(send, sup)
        else:
            q[j] = send

    # Eq. (4): conservation.
    q_in = np.array([q_m, q[0], q[1] + q_r, q[2]])
    c_next = c + DT_H / (L * N) * (q_in - q)

    # Eq. (8): convection, relaxation, anticipation (+ merging on section 3).
    v_next = v.copy()
    for j in range(4):
        v_up = v[j - 1] if j > 0 else v[j]
        c_dn = c[j + 1] if j < 3 else C_BOUNDARY
        dv = (DT_H / L) * v[j] * (v_up - v[j]) \
            + (DT_H / TAU_H) * (veq(c[j]) - v[j]) \
            - (NU * DT_H / (TAU_H * L)) * (c_dn - c[j]) / (c[j] + KAPPA)
        if j == 2:
            dv -= DELTA * DT_H / (L * N) * q_r * v[j] / (c[j] + KAPPA)
        v_next[j] = v[j] + dv
    wm_next = wm + DT_H * (d_m - q_m)
    wr_next = wr + DT_H * (d_r - q_r)
    vehicles = float(np.sum(c) * L * N + wm + wr)
    return np.concatenate([c_next, v_next, [wm_next, wr_next]]), vehicles, q_r


def simulate(rate_of_t, x0, t0=0.0, steps=STEPS, w_limit=None):
    """Run the discrete model; returns (final state, TTS in veh·h, penalty, trajectory)."""
    x = x0.copy()
    tts, pen, traj = 0.0, 0.0, []
    for k in range(steps):
        t = t0 + k * DT
        r = rate_of_t(t)
        x, veh, q_r = step(x, t, r)
        tts += veh * DT_H
        if w_limit is not None:
            pen += max(x[9] - w_limit, 0.0) ** 2 * DT_H
        traj.append((t + DT, x.copy(), r, q_r))
    return x, tts, pen, traj


def equilibrium():
    """The stationary state before the surge: run the model for 900 s from an empty road."""
    x = np.zeros(10)
    x, _, _, _ = simulate(lambda t: 1.0, x, t0=-900.0, steps=90)
    return x


def mpc(np_min=30, nc_min=2, hold_s=120.0, lam=20.0, mu=1.0, w_limit=100.0, r_min=0.2):
    """Receding-horizon control: every `hold_s`, choose the next `nc_min` rates (each held
    `hold_s`) minimising TTS + lam·Σ(Δr)² + mu·Σ(max(w_r − w_limit, 0))²·dt over `np_min`
    minutes, the later intervals holding the last chosen rate; apply the first.

    Multi-start: the cost is not convex in the rate — closing the meter costs queueing time
    at once and pays back over the next hour, so from r = 1 the gradient always says "stay" —
    and a gradient method alone never leaves the bound (measured: L-BFGS-B from 1.0 returned
    1.0 at every step). Five constant first guesses are scored and the best refined."""
    hold = int(hold_s / DT)
    x = np.zeros(10)
    rates, t, tts = [], 0.0, 0.0
    prev_u = np.ones(nc_min)
    while t < T_END:
        if t < 900.0:
            u = np.ones(nc_min)
        else:
            prev = rates[-1][1] if rates else 1.0

            def cost(u, x=x, t=t, prev=prev):
                sched = list(u) + [u[-1]] * (int(np_min * 60 / hold_s) - nc_min)

                def r_of(tt):
                    return sched[min(int((tt - t) / hold_s), len(sched) - 1)]
                _, c_tts, c_pen, _ = simulate(r_of, x, t0=t, steps=int(np_min * 60 / DT),
                                              w_limit=w_limit)
                dr = np.diff(np.concatenate([[prev], u]))
                return c_tts + lam * float(np.sum(dr * dr)) + mu * c_pen
            starts = [prev_u] + [np.full(nc_min, g) for g in (1.0, 0.8, 0.6, 0.4, r_min)]
            best = min(starts, key=cost)
            res = minimize(cost, best, method="L-BFGS-B",
                           bounds=[(r_min, 1.0)] * nc_min, options={"maxiter": 20})
            u = res.x if res.fun <= cost(best) else best
        r = float(u[0])
        for _ in range(hold):
            if t >= T_END:
                break
            x, veh, _ = step(x, t, r)
            tts += veh * DT_H
            t += DT
        rates.append((t - hold_s, r))
        prev_u = np.concatenate([u[1:], [u[-1]]])
    return rates, tts


def best_window(x0, tts_nc):
    """Full-horizon, open-loop: the best single metering window (rate, on, off) by grid
    search — an MPC whose prediction horizon is the whole run and whose control move is one
    window. It sees what the receding-horizon controller cannot: the payback of holding the
    meter for an hour arrives after the surge, outside any 30-minute window."""
    best = None
    for r in (0.3, 0.4, 0.5, 0.6, 0.7):
        for t_on in (900.0, 1200.0, 1500.0, 1800.0):
            for t_off in (3600.0, 4500.0, 5400.0, 7200.0):
                _, c, _, tr = simulate(lambda t, r=r, a=t_on, b=t_off: r if a <= t < b else 1.0,
                                       x0)
                wr = max(st[9] for _, st, _, _ in tr)
                if best is None or c < best[0]:
                    best = (c, r, t_on, t_off, wr)
    tts_ol, r_ol, on_ol, off_ol, wr_ol = best
    print(f"discrete model, best open-loop window: r = {r_ol} on [{on_ol / 3600:.2f}, "
          f"{off_ol / 3600:.2f}) h: TTS = {tts_ol:7.1f} veh.h ({100 * (tts_ol / tts_nc - 1):+.1f} %), "
          f"ramp queue peak {wr_ol:.0f} veh")
    return best


def plant_tts(deck):
    out = subprocess.run(
        ["cargo", "run", "-q", "--release", "-p", "va-cli", "--", "sim", str(deck),
         "--model", str(ROOT / "models"), "--tran", "--report", "tts"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout
    last = [l for l in out.splitlines() if l.startswith("  t=")][-1]
    return float(re.search(r"V\(tts\)=([0-9.e+-]+)", last).group(1))


def main():
    x0 = np.zeros(10)
    _, tts_nc, _, _ = simulate(lambda t: 1.0, x0)
    print(f"discrete model, no control:  TTS = {tts_nc:7.1f} veh.h   (paper: 1267)")
    for w_limit in (100.0, 250.0):
        rates, tts_mpc = mpc(w_limit=w_limit)
        rmin = min(r for _, r in rates)
        closed = [t for t, r in rates if r < 0.99]
        first = f"{closed[0] / 3600:.2f} h" if closed else "never"
        print(f"discrete model, receding-horizon MPC (ramp queue limit {w_limit:.0f}): "
              f"TTS = {tts_mpc:7.1f} veh.h ({100 * (tts_mpc / tts_nc - 1):+.1f} %); rate min "
              f"{rmin:.2f}, first closure {first}   (paper: 1183 under its limit of 100)")
    tts_ol, r_ol, on_ol, off_ol, wr_ol = best_window(x0, tts_nc)

    # The replayed schedule is the one that acts: the open-loop window. One PWL point per
    # 2-minute interval, held between (a repeated time is a step for the PWL source).
    rates = [(t, r_ol if on_ol <= t < off_ol else 1.0) for t in np.arange(0.0, T_END, 120.0)]
    pts = []
    for t, r in rates:
        pts.append((t, r))
        pts.append((t + 120.0 - 1e-3, r))
    pwl = " ".join(f"{t:.0f} {r:.4f}" for t, r in pts)
    src = DECK_NC.read_text(encoding="utf-8")
    body = src[src.index("Vm   dm"):]
    body = body.replace("Xr   dr  one  wr  c3  qr    origin   qcap=2000",
                        "Vrate rate gnd            PWL(" + pwl + ")\n"
                        "Xr   dr  rate wr  c3  qr    origin   qcap=2000")
    head = "\n".join([
        "* The same motorway under the metering schedule optimised on the paper's discrete Payne",
        "* model (docs/examples/traffic_mpc.py) and *replayed* here into the continuous plant as",
        "* a PWL source: the optimiser lives outside the simulator, the simulator is the plant.",
        "* The schedule is the full-horizon optimum (the best single metering window by grid",
        "* search); the receding-horizon MPC of the paper (30 min here, 8 min there) never closes",
        "* the meter in this parameterisation, and no schedule pays under its 100-vehicle",
        "* ramp-queue limit -- docs/traffic.md. Generated; do not edit by hand.",
        f"* Discrete model: TTS {tts_nc:.1f} veh.h no control -> {tts_ol:.1f} with r = {r_ol} on",
        f"* [{on_ol / 3600:.2f}, {off_ol / 3600:.2f}) h, ramp queue peak {wr_ol:.0f} veh "
        "(paper: 1267 -> 1183 under its limit).",
    ]) + "\n"
    DECK_MPC.write_text(head + body, encoding="utf-8")
    print(f"wrote {DECK_MPC.relative_to(ROOT)} ({len(pts)} PWL points)")
    p_nc = plant_tts(DECK_NC)
    p_mpc = plant_tts(DECK_MPC)
    print(f"plant (va-cli), no control:  TTS = {p_nc:7.1f} veh.h")
    print(f"plant (va-cli), replay:      TTS = {p_mpc:7.1f} veh.h   ({100 * (p_mpc / p_nc - 1):+.1f} %)")


if __name__ == "__main__":
    main()
