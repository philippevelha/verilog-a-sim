# Proposal: a cheaper DC rescue — run the node-step-capped ladder first

**Status:** decided 2026-09-26 and implemented in 1.23.0: cap-first (§4). The other options are
recorded with their measurements.
**Affects:** `va-core` only (`dc.rs`: the order of `with_gmin_rescue`'s tiers). No interface change.
**Follows:** option (C) of `docs/proposals/btf-solver.md` ("fewer solves instead of cheaper
ones"); the rescue tiers of 1.3.1 (`gmin` ladder), 1.10.1 (damping) and 1.12.0 (node-step cap).
**Evidence:** `va-cli --logfull` on c432 (1.21.0) and `cargo xtask ladder <deck> --model <m>`
(new with this document), release build, i7-1185G7, 8 threads, mains power, single runs.

## 1. The problem

Since 1.21.0 made the linear solve cheap, c432's DC operating point (28 s at 8 threads) is almost
all device evaluation: **558 Newton iterations**, each evaluating 910 PSP103 instances. They are
spent like this:

| tier | stages | iterations | outcome |
|---|---:|---:|---|
| plain Newton | 1 | 3 | singular matrix (leakage-only stack nodes) |
| `gmin` ladder, 30 steps | 6 | **265** | stalls at gmin = 3.2e-5: **150 iterations**, no convergence |
| ladder + node-step cap (0.5 V) | 31 | 290 | converges |

**47% of the iterations go to a tier that fails.** `dc::with_gmin_rescue` runs its tiers
cheapest-first so that whatever an earlier tier solves keeps its path and its answer bit for
bit — which is also why c432 pays for the plain ladder every time before reaching the tier that
works.

## 2. Why the plain ladder fails on c432

The stalled stage proposes Newton steps of 31 V to **5.8 × 10⁹ V** on a 1.8 V circuit from its
first iteration — the runaway `dc.rs` documents for the internal nodes of 4-high NMOS stacks,
which are set only by leakage while a lower device is off, so each linearized step divides by a
near-zero conductance. It ends in an exact 2-cycle (residual alternating 3.05e-3 / 3.07e-3, the
largest step 7.4 V every time) that it cannot leave. The cap exists for exactly this and solves it.

## 3. Options measured

`cargo xtask ladder` replays the rescue as a sequence of `newton::solve` calls and counts
assemblies (one per Newton iteration) under alternative schedules, comparing each answer with the
current rescue's. On the five `.op` decks that need the rescue:

| deck | current | **cap first** | fewer steps: 10 / 15 / 20 (cap first) |
|---|---|---|---|
| chain40 (plain ladder solves) | 119 | **109**, answer ±1.6e-16 | 53 / 70 / 85 — 5 steps fails |
| chain80 (plain ladder solves) | 138 | **109**, ±1.6e-16 | **fails at all three** |
| chain95 (cap tier solves) | 153 | **110**, identical | **fails** |
| chain160 (cap tier solves) | 150 | **110**, identical | **fails** |
| c432 (cap tier solves) | 558, 26.8 s | **293, 14.1 s, identical** | 193 / 221 / 229, answers ±1e-9 V |

- **(C1) Cap first** — run the capped ladder before the plain one. Never more expensive on these
  decks, c432 1.9× fewer assemblies, and bit-identical wherever the cap tier already did the work.
  Where the plain ladder used to solve, the answer moves by rounding.
- **(C2) Fewer, larger gmin steps** — **unsafe**: at 10, 15 or 20 steps (instead of 30) three of
  the five decks are not solved at all. Thirty steps is not slack.
- **(C3) Give up on a stalled stage early.** No clean signal in this data: stages that *converge*
  also go 38–56 iterations without a 10× residual improvement before the quadratic finish, and
  the un-capped stage that converged at gmin 6.3e-5 also took 17 steps above 10 V (peak 450 V).
  Only magnitude separates it from the failing stage's 5.8e9 V — one example, no threshold.
  Not recommended without more data.
- **(C4) Adaptive gmin stepping** (grow the step while stages converge quickly, back off and
  retry on failure, as SPICE does). Not measured. Its ceiling on c432: of the capped tier's 290
  iterations, the 25 stages below gmin 3.2e-5 take 123, 20 of them in ≤ 6 iterations — so ~60–80
  iterations (20–27% of what cap-first leaves). C2 shows the chains are sensitive to step size,
  so it needs the retry logic and its own measurement.
- **(C5) Reusing a factorization across iterations** (parallel-assembly §5.2): since 1.21.0 the
  DC solve is ~2 ms of a ~50 ms iteration on c432, so reuse would save little; the cost is
  evaluation, which only fewer iterations reduce.

## 4. Decided: cap first (C1)

New tier order: plain → **ladder + cap** → ladder → ladder + damping. The plain ladder is kept
as the next tier, so a circuit the cap cannot solve but the plain ladder can still solves; the
damped tier stays last.

What it changes, from the deck-diff against 1.21.0 (§5): **71 of 72 decks
identical** (`deck-diff --all`, a scratch build with the order swapped). That includes the four
NAND sweeps whose failing points the plain ladder rescues today (`bsimcmg`, `bsimsoi`, `ekv26`,
`lutsoi` — the capped ladder solves them to the same printed digits), `hisim2` (fails every tier,
before and after), and c432 (its path does not change). The one deck that moves is c17's
transient, whose starting operating point the plain ladder rescues (31 stages): 13 450 → 13 487
steps, node voltages resampled RMS ≤ 5.9e-5 of scale, edges ≤ 12 fs.

**Cost:** circuits the plain ladder used to rescue now take the capped path first, so their
answers move in the last digits (chain40/80: ≤ 1.6e-16 V) — the same kind of shift accepted for
`libm` (1.17.0) and BTF (1.21.0). The "earlier tiers keep their bits" property that the tier order
was built for now protects plain Newton only, which is where almost every circuit is solved.

## 5. Gates

- `cargo xtask deck-diff --all` against 1.21.0, magnitudes reported for every deck that moves.
- `validate` 28/28, errors unchanged (no golden deck needs the rescue).
- The rescue's unit tests in `dc.rs` (tier order and which error surfaces) updated to the new
  order, with the reason.
