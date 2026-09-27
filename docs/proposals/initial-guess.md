# Proposal: start the DC solve from the `.bench` logic (`.nodeset`)

**Status:** measured 2026-09-27, **awaiting the user's decision**. The recommendation in §5 is
to keep `.nodeset` as a general SPICE card, to leave `gen_iscas.py --nodeset` as an opt-in and
not commit nodeset decks, and to point the next measurement at the rows that go singular
rather than at the starting point. The implementation (§2) is committed as 1.30.0, because it
does nothing unless a deck has a `.nodeset` card (every committed deck is bit-identical, as
deck-diff checked) and is simple to remove if the user decides against it. No nodeset deck is
committed.
**Affects:** `va-netlist` (`Netlist::nodesets`), `va-cli` (Newton's starting point), and the
tools `gen_iscas.py --nodeset` and `xtask ladder --nodeset`. No interface change.
**Follows:** `dc-rescue.md` and `adaptive-gmin.md`. 1.27.0+5's release entry noted "not tried
yet: starting Newton from the `.bench` logic's node voltages instead of zero, which might avoid
the rescue altogether". This document tries it.
**Evidence:** release build, i7-1185G7, 8 threads, mains power. Iteration counts are
deterministic; wall times are single runs.

## 1. The question

Every ISCAS'85 circuit from c432 up needs the DC rescue. Plain Newton from `x = 0` fails in
2–3 iterations, and the capped adaptive `gmin` ladder then takes 175–365 Newton iterations
(`dc-rescue.md`). A converging plain solve takes about 20. The circuits are combinational
logic, so every net's DC value is known in advance from the `.bench` file: 0 or VDD. If Newton
started there, would the rescue still be needed?

## 2. What was built

- **`.nodeset V(<net>)=<value> ...`** (SPICE's card), parsed into `Netlist::nodesets`. `va-cli`
  starts the `.op` solve, and the operating point that precedes a transient, from those values;
  every other unknown starts at 0. With no nodeset the call is exactly the old one
  (`operating_point_continued` with no start *is* `operating_point_with_events`), so every
  existing deck is bit-identical. A nodeset naming a net the deck does not have is an error; a
  malformed pair is skipped with a note.
  **Limitation, stated:** SPICE first solves with each nodeset node *held* at its value, then
  releases it. This implements only the second half: the starting guess.
- **`gen_iscas.py ... --nodeset`** evaluates the `.bench` logic for the deck's starting vector
  and writes every net pin (driver and every load pin) at 0 or 1.8 V, plus `vdd`. A cell's
  internal nodes, such as a NAND's stack node, are left to Newton. The rest of the deck is
  unchanged: regenerating without `--nodeset` reproduces the committed decks byte for byte.
- **`xtask ladder <deck> --current [--nodeset]`** replays the rescue from zero or from the
  nodesets, counting assemblies. With `--nodeset` it also runs warm-start variants: capped plain
  Newton from the hint, with a shunt from every node to ground ramped down tenfold per stage
  from 1e-6 S or from 1e-9 S, or with no shunt at all.

## 3. Measured

`.op` for the committed vector of each circuit, from zero and from the logic nodesets:

| circuit | unknowns | assemblies, zero start | assemblies, nodeset | `va-cli` wall, zero / nodeset | answers |
|---|---:|---:|---:|---:|---|
| c432 | 15 416 | 228 (3 + 225) | 228 (3 + 225) | 10 / 13 s | identical |
| c499 | 28 492 | 181 (3 + 178) | 181 (3 + 178) | 19 / 18 s | identical |
| c880 | 32 366 | 177 (3 + 174) | 177 (3 + 174) | 21 / 23 s | identical |
| c1355 | 39 892 | 191 (3 + 188) | 191 (3 + 188) | 26 / 26 s | identical |
| c1908 | 62 055 | 180 (2 + 178) | 180 (2 + 178) | 39 / 40 s | identical |
| c2670 | 93 252 | 240 (2 + 238) | 240 (2 + 238) | 79 / 79 s | identical |
| c3540 | 134 246 | 367 (2 + 365) | 367 (2 + 365) | 185 / 198 s | identical |
| c5315 | 197 274 | 325 (3 + 322) | 325 (3 + 322) | 246 / 244 s | identical |
| c6288 | 171 714 | 227 (2 + 225) | 225 (2 + 223) | 143 / 142 s | identical |
| c7552 | 268 676 | — | — | — | not measured |

"Identical" means every printed value (8 136 to 103 962 per circuit) matches to all printed
digits, and the nodeset run passes `check_iscas.py` (every signal, every primary output).
**c7552 was not measured:** the run was stopped by Claude Code for low system memory while it was
on c7552, and was not restarted.

**The hint changes nothing, not even the iteration count.** Plain Newton fails in the same 2–3
iterations from the logic levels as from zero. The rescue's first stage then shunts 1e-3 S
from every node, which pulls the solution far from the hint, and the remaining stages follow
the same path as before.

**Why plain Newton fails** (c432, `ladder --nodeset`): the error is `singular matrix during
linear solve`, at the **second** assembly, whatever the start. The warm-start variants fail the
same way:

| from the logic nodesets, capped Newton | outcome |
|---|---|
| no shunt | singular at assembly 2 |
| shunt ramp from 1e-6 S | singular at assembly 2 (first stage) |
| shunt ramp from 1e-9 S | singular at assembly 2 (first stage) |
| `VA_BTF=off` (faer only), each of the above | the same |

So the starting point is not what plain Newton lacks. Starting from the right logic levels,
with every node shunted by 1e-6 S, the Jacobian after one Newton step is still singular to the
solver. The 1e-3 S shunt of the rescue's first stage is large enough to prevent that, and
1e-6 S is not. The singular pivot is not in the block-triangular solver (with BTF off, faer
fails identically). It is either on rows the shunt does not touch (constraint rows: the PSP103
instances' internal branch rows, `...nm04.b0`, are `UnknownKind::Branch`) or tied to an
extreme scale in the node rows. Which one was **not** determined: naming the rows needs the
factorization's pivots, and assembling c432 densely to look would take 15 416² doubles
(1.9 GB) at a time when memory was short.

## 4. What this rules out, and what it does not

- **Ruled out:** a logic-level starting guess as a way to avoid the rescue. It costs a nodeset
  line per net and buys nothing, on nine circuits out of nine.
- **Ruled out (on c432):** a warm rescue, i.e. the same ladder started lower because the hint is
  already good. It fails at its first stage.
- **Not ruled out:** holding the hint (SPICE's first nodeset phase, with nodes clamped), or
  hints on a cell's internal nodes (the NAND stack node). Neither was tried; both are only
  worth trying once the singular rows are named, because a singular Jacobian at assembly 2 is
  not obviously a starting-point problem.

## 5. Recommendation (the decision is the user's)

1. **Ship `.nodeset` as a general card** (1.30.0). It is standard SPICE, it costs nothing when
   absent (bit-identical), and it is what a multi-stable circuit such as a latch or a memory
   cell needs in order to land on the intended operating point. That use is a different one
   from the one measured here, and the measurement does not argue against it.
2. **Keep `gen_iscas.py --nodeset` opt-in; do not commit nodeset decks.** On this suite they
   change nothing.
3. **No change to the DC rescue.**
4. **Next measurement:** name the rows whose pivots vanish at c432's second plain assembly. This
   means a probe on the sparse factorization's pivots (not a dense assembly), recording each
   row's unknown label and kind. If they are the PSP103 internal branch rows, the question
   becomes why a constraint row goes singular. If they are node rows at extreme scale, it
   becomes a pivot-tolerance question.

## 6. Limitations

- One vector per circuit (the committed deck's). Iteration counts are deterministic for that
  vector; other vectors were not tried.
- c7552 not measured (§3).
- The warm-start variants were run on c432 only.
- `ladder`'s replay mirrors `dc::with_gmin_rescue` by value (`xtask/src/ladder.rs`'s own
  limitation); the `va-cli` wall times in §3 are the real path.
