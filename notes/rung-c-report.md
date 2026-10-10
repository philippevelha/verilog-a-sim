# Rung C: fitted access switch with gate charge (2.23.0)

Branch `claude/practical-babbage-dkmxif`, PR philippevelha/verilog-a-sim#3. Date: 2026-10-10.

## What changed
`models/snn/mos_switch.va` now models the charge the gate pushes onto the source and drain
when the word line switches. Each channel terminal carries

    q(V) = cov·V + cch·ns·softplus((V − vt)/ns)

from the gate, using rung A's own vt and ns.

- **Defaults:** `cov` and `cch` default to 0. That is rung A, and its output is byte-identical to before.
- **Fit:** `fit_switch.py` fits the two parameters to PSP103 with a slow gate ramp at V_DS = 0, so
  the terminal currents are pure charge flow. Result: cov = 3.742 fF and cch = 8.472 fF per side.
  PSP103 moves 18.23 fC through each terminal over the full swing; the fit is within 0.29 fC of it
  everywhere, about 0.4 mV of V_m.
- **Decks:** `gen_fig5_access.py --switch fitc` and `gen_xbar_deck.py` / `gen_mnist_tx.py --synapses fitc`
  write the decks with the charged switch. Each crossbar path is written as `rram` + `mos_switch`,
  because the drain-side charge needs the cell node.

## Fig. 5
- Worst |V_m − PSP103| is **0.21 mV**, down from rung A's 2.4 mV, and it no longer always reads high.
- The current pulse in the source line at each word-line edge now matches PSP103's: −68 / +145 µA
  against −74 / +149 µA. Rung A produced no pulse.

## MNIST crossbar
16 hidden-layer-2 neurons, 20 cycles, digit 0, against PSP103. All runs use plain JIT, with no
bypass and no collapse.

| Access device | Unknowns | V_m − PSP103: mean / RMS / worst | Spikes (PSP103: 17) | Wall |
|---|---|---|---|---|
| PSP103 | 35 262 | — | 17 | 2 693 s |
| 10 Ω switch | ~300 | worst 1 767 mV | three neurons differ | 60 s |
| Rung A (folded) | ~300 | +8.0 / 9.2 / 14.7 mV | all 17 | 52 s |
| **Rung C** | 2 494 | **+1.6 / 1.9 / 4.6 mV** | **all 17** | 168 s |

The closest threshold call is neuron h2_5 at step 16. Its margin below threshold is 14.7 mV with
rung C and 16.1 mV with PSP103. Rung A left only 4.7 mV.

## Costs and open points
- **Run time:** about 3× rung A's. The synapse can't be folded into one device any more, so each
  path is now an RRAM cell plus a charged switch, with 2 extra unknowns per synapse.
- **Cell-node kick overshoots:** a high-resistance cell node drops to −0.23 V where PSP103 stops at
  +0.40 V, because the model has no junction capacitance. The kick settles before the neuron
  integrates, so V_m doesn't see it.
- **Remaining bias:** a +1.6 mV mean bias remains, still mostly high. I haven't separated the
  cause. The candidates are:
  - the missing junction capacitance;
  - the V_DS dependence (rung B);
  - the 50/50 charge split between drain and source.
- **Coverage:** one image, 16 of 64 neurons, 20 of 29 cycles.

## Checks at 2.23.0
- `cargo test --workspace`: 963 passed.
- `cargo xtask validate`: 28/28.
- fmt and clippy are clean, and `Cargo.toml` and `release.txt` agree on 2.23.0.
- New test: `the_fitted_switchs_gate_charge_moves_psp103s_charge_and_differentiates_correctly`.

## CI
The `gate (ubuntu-latest)` and `cargo deny` jobs end within seconds on `main` and on this PR, and
their logs return 404. That points to the GitHub Actions setup or the account, not the code.
Someone with access to the repository's Actions settings or the account's billing needs to fix it.

## Next options
1. Find which of the three candidates causes the remaining +1.6 mV bias.
2. Run more images to see whether spike agreement holds beyond digit 0.
