# rl_loop / rl_noloop under OpenVAF-reloaded

Run by the user on Linux on 2026-10-24 with `run_ngspice.sh`. Tools: OpenVAF-reloaded commit 5063fda (14 Oct 2026, "Updated readme"; `--version` prints "OpenVAF-reloaded unknown") and ngspice-47 with KLU (build commit not recorded).

**The minimal model does not reproduce the bug.** `rl_loop` gives the correct answer, identical to `rl_noloop`. ngspice prints phase in radians; it is converted to degrees below.

| f (Hz) | expected \|V(nind)\| | rl_loop | rl_noloop |
|---|---|---|---|
| 1e8 | 0.09950 | 9.950371902100e-2 ∠84.29° | identical |
| 3.16e8 | 0.3015 | 3.015113445778e-1 ∠72.45° | identical |
| 1e9 | 0.7071 | 7.071067811865e-1 ∠45.00° | identical |
| 3.16e9 | 0.9535 | 9.534625892456e-1 ∠17.55° | identical |
| 1e10 | 0.9950 | 9.950371902100e-1 ∠5.71° | identical |

verilog-a-sim (`rl_*.net`) matches the expected values for both variants: on Windows at 2.42.0+15, and on the user's Linux build.
