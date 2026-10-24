<!--
DRAFT for the OpenVAF-reloaded issue tracker. Not yet filed.
Before filing:
  1. Optional: run ../bisect/ on Linux (python3 gen.py && sh run_ngspice.sh) and fill in the
     "Narrowing down" table; drop that section if you file without it.
  2. Check the tracker URL (believed to be github.com/OpenVAF/OpenVAF-Reloaded).
  3. Attach ../mrm_eqc_rlc_loop2.va, ../ng_ac_10GHz.cir and, as the non-reproducing
     contrast, rl_loop.va + rl_loop.cir.
  4. Put the issue URL in paper_writing/sections/09b-mrm.tex (the \todo) and in README section 5.
-->

# A `for` loop in the analog block drops an inductor's `ddt` contribution in AC

## Summary

In the model below, adding a `for` loop to the analog block makes OpenVAF-reloaded drop the reactive (`ddt`) contribution of an inductor. The inductor is modelled with its current as an explicit state. A loop of two iterations whose body only assigns a constant is enough. In ngspice AC analysis the inductor's node then reads exactly 0 V at every frequency. If the loop is removed, or unrolled by hand, the result is correct.

The bug is silent: compilation and simulation give no warning, and the rest of the circuit still produces plausible-looking numbers. In our ring-modulator model the output was up to 27 % off with nothing to flag it.

## Versions

- OpenVAF-reloaded: commit 5063fda (14 Oct 2026, "Updated readme"). `openvaf-r --version` prints only `OpenVAF-reloaded unknown`.
- ngspice-47 with KLU, built with OSDI support
- OS: Linux x86-64

## Reproducer

`mrm_eqc_rlc_loop2.va` (attached) is a linear equivalent-circuit model of a silicon micro-ring modulator: electrical pad/substrate/junction parasitics, then an electro-optic RLC network driven by a transconductance. The inductor runs from `nind` to ground and is written with its current as a state, `V(il)`:

```verilog
I(nind) <+ V(il);
I(il) <+ ddt(Lind * V(il));
I(il) <+ -V(nind);
```

`Lind`, `R2`, `Cc` and the transconductance are computed in the analog block from parameters only. The block contains this loop, which has no effect:

```verilog
dlsh = 0.0;
for (it = 0; it < 2; it = it + 1) begin
    dlsh = 0.0;
end
```

Deck (`ng_ac_10GHz.cir`, attached), AC from 0.1 to 100 GHz in 13 points:

```
openvaf-r mrm_eqc_rlc_loop2.va -o mrm_eqc_rlc_loop2.osdi
ngspice -b ng_ac_10GHz.cir     # .model line -> mrm_eqc_rlc_loop2, pre_osdi -> mrm_eqc_rlc_loop2.osdi
```

Results at 3.16 GHz:

| run | \|V(out)\| | \|V(nind)\| |
|---|---|---|
| OpenVAF, loop of 2 trivial iterations (`loop2`) | 1.267095e-1 | **0 at every frequency** |
| OpenVAF, loop of 60 fixed-point iterations (`loop60`) | 1.267095e-1 | **0 at every frequency** |
| OpenVAF, same model, loop unrolled by macro | 1.301449e-1 | 2.696714e-2 |
| independent simulator (verilog-a-sim), any of the three | 1.301449e-1 | 2.696714e-2 |

Under OpenVAF, the unrolled model agrees with the independent simulator to 3.6e-7 in V(out) and 4.2e-7 in V(nind). With the loop present, V(out) is wrong by up to 27 % across the band. An exact zero at `nind` is what losing `ddt(Lind * V(il))` predicts: the `il` row then reduces to −V(nind) = 0, which turns the inductor into a short to ground.

## What does not trigger it

A loop and an explicit-current inductor are not enough on their own. `rl_loop.va` (attached) is a 1 Ω series resistor into the same three-line inductor, with the same 2-iteration loop assigning a variable that feeds `L`. It gives the correct jωL/(R + jωL) at all five points from 0.1 to 10 GHz, identical to the same model without the loop. Whatever else the trigger needs is somewhere in the larger model.

## Narrowing down

<!-- fill from ../bisect/run_ngspice.sh, or delete this section -->

Each variant is `loop2` with one feature removed; "bug" = V(nind) is 0 at all 13 points.

| variant | change from `loop2` | bug? |
|---|---|---|
| b0_loop2 | none (control) | |
| b1_noloop | loop removed (control) | |
| b2_body_unused | loop body assigns an unused variable instead of `dlsh` | |
| b3_loop_first | loop moved to the top of the analog block | |
| b4_const_L | `Lind` = parameter instead of computed | |
| b5_const_R2_L | `Lind` and `R2` = parameters | |
| b6_const_g | transconductance = parameter | |
| b7_no_parasitics | only `Rs1`/`Cj1` kept on the electrical side | |
| b8_no_Cc | `ddt(Cc * V(out))` removed | |
| b9_no_ternary | segment-2 ternary removed from the `out` source | |

## Workaround

Replace the loop with straight-line code (macro unrolling).
