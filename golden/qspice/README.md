# QSPICE reference decks — the displacement-current set

Hand-written QSPICE decks for the `I(<port>)` / `I(branch)` displacement-current work. Each one
describes the *same physics* as a `models/*.va` + `circuits/*.net` pair, built from QSPICE-native
primitives so the comparison is evidence rather than a restatement of our own lowering.

**Status: verified to run and to discriminate; not yet wired into `xtask gen-golden`.
Parked deliberately (confirmed 2026-09-01), to be gated later — not abandoned.** The six
`circuits/*.net` decks these mirror are therefore exercised by nothing today: they are
coverage-in-waiting, and `docs/validation.md`'s ungated-circuit section says so plainly so they
are neither deleted nor mistaken for a passing gate.

These are the PWL-driven forms, each run against QSPICE on 2026-08-29 with the numbers below. They are kept
here because the numbers are the load-bearing part — the decks themselves must eventually be
produced by `translate_for_qspice` rather than run by hand (see "Open" below).

## The set

| Deck | Exercises | QSPICE result | Conduction-only (the bug) | Margin |
|---|---|---|---|---|
| `portprobe_dc.cir` | negative control: DC has no displacement current | `V(out)` = 1.000e-3 exactly, `I(C1)` = 1e-18 | same | — |
| `portprobe_ramp.cir` | `I(<port>)` over a parallel R‖C | `V(out)` 1.00025 → 1.001 | 0.00025 → 0.001 | **~1000×** |
| `portprobe_sq.cir` | a charge-carrying probe through a nonlinearity | `V(out)` ≈ 1.0005e-3 → 1.002e-3 | ≈ 1e-9 | **~10⁶×** |
| `nlcap_ramp.cir` | `ddt` with a bias-dependent coefficient | `V(out)` 1.247 → 2.0007 | folded-charge error gives → 3.0 | **50 %** |
| `portprobe_ac.cir` | the charge channel reaching AC | \|V(out)\| 1.002e-3 @10 Hz → 62.832 @10 MHz | flat 1e-3 | **~6×10⁴** |

`portprobe_ramp.cir` doubles as the reference for `circuits/selfprobe_ramp.net`: the port probe
and the branch self-probe must recover the identical current, so if only one construct is fixed,
only one of the two gates goes green.

## Facts established while building these

- **`I(V1)` and `I(R1)` work inside a QSPICE `B` source; `I(C1)` does not** ("unknown controlling
  source"). Total terminal current is therefore taken as `-I(V1)`, or `I(Rseries)` where a series
  element exists.
- **A charge-defined capacitor works**: `C1 a 0 Q=1u*V(a,0)+0.5u*V(a,0)*V(a,0)` yields
  `I = (c0 + c1·V)·dV/dt`, which is what makes `nlcap_ramp` an independent oracle for the
  product-rule case rather than a restatement of our own algebra.
- **`UIC` is not needed for these decks, verified rather than assumed.** `gen_golden`'s existing
  behavioural-translation note warns that its no-`UIC` exemption holds "only because these decks
  carry nothing reactive", and that a future reactive entry "would have to reconcile the two".
  Reconciled: with and without `UIC`, `V(out)` is identical here (only timepoint placement moves),
  because the ramp starts at 0 V with the capacitor uncharged, which is the same state our own
  cold start begins from.

## Open

The decks above use `PWL` sources. `circuits/*.net` use `SIN`, because our netlist parser accepts
`DC` and `SIN` only — **`PWL` is not supported** (`va_netlist::parser`). A hand-run QSPICE deck
with `SIN` over this topology returns an identically-zero solution, while the project's own
`rectifier.net` gate drives `SIN` successfully — the difference is that the gate goes through
`cold_start_tran_deck`/`translate_for_qspice` and a hand-rolled `.cir` does not. Wiring these into
that path is the next step, and is expected to resolve it; adding `PWL` to the netlist parser is
the alternative.
