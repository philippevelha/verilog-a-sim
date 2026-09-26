# Proposal: adaptive `gmin` stepping in the DC rescue

**Status:** decided 2026-09-26 — implement it in any case; the default in the DC rescue if the
measurements show it winning everywhere, behind a switch that is off by default if they show it
failing or slower anywhere (agreed in advance). They show it winning everywhere (§3): it is the
rescue's default.
**Affects:** `va-core` only — `newton.rs` (`NewtonConfig::gmin_adaptive`, the adaptive ramp) and
`dc.rs` (the rescue switches it on). No interface change.
**Follows:** `docs/proposals/dc-rescue.md` §3 option (C4), whose ceiling estimate this replaces
with a measurement; the fixed 30-stage ramp (1.3.1).
**Evidence:** `cargo xtask ladder <deck> --model <m>` (release build, i7-1185G7, 8 threads, mains
power; assembly counts are deterministic, wall times single runs).

## 1. The problem

The DC rescue's `gmin` ladder walks the shunt from 1e-3 S to 1e-12 S in 30 equal steps in log
space, then solves unshunted. The steps are equal whatever the circuit does: on c432, 20 of the 25
stages below 3.2e-5 S converge in ≤ 6 Newton iterations each — they could take bigger steps —
while the ones near 1e-4 S take 41–59. Fewer *equal* steps is not the answer: at 10, 15 or 20
stages three of the five rescue decks were not solved at all (`dc-rescue.md` §3, C2).

## 2. Design

The fixed ramp's ends, with the steps between them chosen as the solve goes:

- start at `1e-3` S with a step of `1/30` of the ramp — the fixed schedule's step;
- a stage that converges in at most `ADAPTIVE_FAST_ITERS` Newton iterations **doubles** the next
  step, up to a quarter of the ramp (`ADAPTIVE_MAX_STEP`: no step divides `gmin` by more than
  ~180);
- a stage that **fails** is retried from the last converged point with **half** the step; after
  three halvings in a row (`ADAPTIVE_MIN_STEP_FRACTION` = 1/8 of the starting step) the solve
  fails with that stage's error;
- past the end of the ramp, the unshunted solve — as in the fixed schedule.

The retry is what makes larger steps safe to try: the fixed ramp has no back-off at all — a stage
that fails ends the whole ladder. `NewtonConfig::gmin_adaptive` (default `false`) selects it, so a
caller that asks for `gmin_steps` still gets equal stages; the DC rescue switches it on for all its
ladder tiers.

## 3. Measured

Assemblies (= Newton iterations, all tiers) on the five `.op` decks that need the rescue, current
tier order (plain → capped ladder → ladder), against 1.23.0's fixed ramp:

| deck | 1.23.0 fixed | adaptive, fast ≤ 4 | ≤ 6 | **≤ 8** |
|---|---:|---:|---:|---:|
| chain40 (1 326 unknowns) | 109 | 60 | 58 | **54** |
| chain80 | 109 | 67 | 61 | **62** |
| chain95 | 110 | 68 | 62 | **62** |
| chain160 (5 286) | 110 | 67 | 61 | **62** |
| c432 (15 416) | 293 | 281 | 245 | **228** |

Every deck solved at every threshold; answers against the fixed ramp's: ≤ 2.7e-16 V on the
chains, 6.2e-11 V on c432 (both within the Newton tolerance). The threshold is the one tuning
choice: at 4, c432 barely gains because its tail stages converge in 5 iterations and so almost
never double the step; 8 is the best measured. **Chosen: 8** — 43–50% fewer iterations on the
chains, 22% on c432.

Not measured: thresholds above 8, a larger maximum step, and circuits outside these decks. The
failure path (back-off and retry) is exercised by a scripted unit test, not by any deck here —
none of them failed a stage under the adaptive ramp.

## 4. Gates

`cargo xtask deck-diff --all`, 1.23.0 against the adaptive build: **68 of 72 identical**. The
four that move are the decks whose DC solve goes through the rescue:

- c432 `.op`: 140 of 15 416 printed values differ, 14 of them above 1e-20; the largest change
  is a 3.6e-14 A branch current in its last printed digit. No node voltage moves.
- NAND sweeps `bsimsoi`, `lutsoi` (one rescued point each): ≤ 5.4e-16 and ≤ 9.8e-15 of scale.
  `bsimcmg` and `ekv26` — also rescued — reach the same printed digits; `hisim2` fails as before.
- c17's transient (its starting point is rescued): 13 487 → 13 462 steps, node voltages resampled
  RMS ≤ 4.4e-5 of scale, edges ≤ 7.5 fs.

`validate` 28/28, errors unchanged (no golden deck reaches the rescue). Speed, c432 `.op`, 8
threads, mains power, two interleaved rounds: **14.96 / 14.55 s → 11.66 / 11.28 s (1.3×)**.

## 5. Limitations

- Tuned on five decks of one technology (PSP103 CMOS). The back-off makes a too-large step cost
  a retry, not a failure, but a circuit that needs many retries can take more iterations than the
  fixed ramp would; no deck measured did.
- A stage that fails for a reason a smaller step cannot fix (a model producing a non-finite value)
  is retried three times before the solve fails with that error — a few wasted stages on a solve
  that was failing anyway.
