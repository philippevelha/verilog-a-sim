# The bridges

The pipeline is two halves joined by two **bridges** — the frozen interfaces of CLAUDE.md
§4. Everything in this directory expands those contracts from the type sketches in
[`../interfaces.md`](../interfaces.md) into full semantic specifications: not just *what the
types are*, but *what they mean*, *who promises what*, and *what is illegal*.

```
 Verilog-A source                         circuit netlist
        │                                        │
   [va-frontend] ──IR (Bridge α)──► [va-codegen]      [va-netlist]
   lex/parse/elab                   IR → AD → models       │
                                          │                │
                                  ModelInstance (Bridge β)  │
                                          │                │
                                          ▼                ▼
                              ┌──────────────────────────────────┐
                              │            [va-core]             │
                              │   MNA · Newton · linsolve · conv │
                              └──────────────────────────────────┘
```

| Bridge | Crate     | Document                                          | Producer        | Consumer(s)                 |
|--------|-----------|---------------------------------------------------|-----------------|-----------------------------|
| **α**  | `va-ir`   | [interface-alpha-ir.md](interface-alpha-ir.md)    | `va-frontend`   | `va-codegen`                |
| **β**  | `va-abi`  | [interface-beta-abi.md](interface-beta-abi.md)    | `va-codegen`, `va-abi::reference`, `va-netlist` | `va-core`, `va-transient`, `va-acnoise` |

## Why bridges are load-bearing

Each bridge is a **leaf crate with no internal dependencies** (§3). That is the property
that lets independent theses proceed in parallel:

- `va-core` (T3) depends only on Bridge β. It is validated against `va-abi`'s hand-written
  reference models, so the core team never waits on the compiler team (T2).
- `va-frontend` (T1) and `va-codegen` (T2) meet only at Bridge α; neither imports the
  other.

A broken bridge blocks every sibling thesis at once. That is why §6 makes any change a
coordinated event, and why these documents exist: to make the contract precise enough that
"is this a breaking change?" has an unambiguous answer.

## What "defining a bridge" covers

Each bridge document is organised the same way so reviewers can diff intent against code:

1. **Role** — the one job this bridge does, and the seam it sits on.
2. **Producers & consumers** — who writes it, who reads it, what each promises.
3. **The contract** — the types, mirrored from the crate, annotated with meaning.
4. **Semantics & invariants** — the rules that are true of every valid value, stated as
   numbered, checkable obligations.
5. **Conventions** — units, sign conventions, reference/ground handling, ordering.
6. **Worked example** — one device or module traced end to end across the seam.
7. **Edge cases & non-goals** — what is deliberately out of scope, and what is illegal.
8. **Evolution** — how to change it under §6 without silently breaking a sibling.

## Status

These are **draft** specifications of the **v0** contracts. The types they describe are
already frozen and shipped in `va-ir` / `va-abi`; the prose is being written to match.
Where prose and code disagree, the code is authoritative until the discrepancy is resolved
as a §6 change — flag it, do not silently "fix" either side.
