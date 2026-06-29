# Prior Art & Clean-Room Policy

`verilog-a-sim` is **clean-room, from-scratch** (`CLAUDE.md` §1) and **Apache-2.0** licensed.
That is a deliberate choice, and it constrains how we are allowed to use existing simulators
and compilers — above all **OpenVAF**, the most complete open Verilog-A compiler. This
document records the prior art we learn *from*, the hard boundary we must not cross, and a
per-thesis policy so every student has a defensible line on day one.

Read alongside [`roadmap.md`](roadmap.md), [`thesis-map.md`](thesis-map.md),
[`interfaces.md`](interfaces.md), and [`validation.md`](validation.md).

---

## TL;DR

- **We do not copy, paste, fork, port, or paraphrase code from GPL-licensed projects** (most
  notably OpenVAF). Doing so would force `verilog-a-sim` to become GPL-3.0, infecting all six
  theses and discarding the Apache-2.0 decision.
- We **may** learn from public **specifications, standards, papers, and architecture docs**,
  and from the *existence proof* that a given approach works.
- For the highest-risk crate (T2, automatic differentiation) we apply stricter clean-room
  discipline: implement from the literature, not from reading a GPL implementation.

The realistic salvage from OpenVAF is **architectural insight and de-risking**, not
shippable code.

---

## The license boundary (why this is not negotiable)

| Project | License | Consequence for us |
|---------|---------|--------------------|
| `verilog-a-sim` (this) | **Apache-2.0** | Permissive; we want it to stay that way. |
| **OpenVAF / OpenVAF-Reloaded** | **GPL-3.0** | Strong copyleft. Any incorporated code makes the *whole* derivative work GPL-3.0. |

GPL-3.0 → Apache-2.0 is a **one-way wall**. OpenVAF's root `LICENSE` is GPL v3 and its
`README` states it plainly; there are **no per-file SPDX headers**, so GPL blankets the entire
workspace — including the generic `lib/*` utility crates, even where they originate from
rust-analyzer. We therefore treat **all** OpenVAF source as off-limits for copying.

The only "reuse OpenVAF code" paths that exist are project-level decisions, **not** something
an individual thesis may do unilaterally:

1. Relicense `verilog-a-sim` to GPL-3.0 (rejected — see §1 of `CLAUDE.md` and the Apache
   choice), or
2. Obtain a separate written license grant from the OpenVAF copyright holders, or
3. Maintain any OpenVAF-derived work as a **separate GPL-3.0 fork**, outside this repo.

Until one of those happens, the boundary below holds.

---

## What is fair game (non-GPL prior art to learn from)

Source the *originals* OpenVAF itself draws on — they give the same insight with no
contamination:

- **The Verilog-A LRM** (Accellera Language Reference Manual) — the authoritative spec for the
  language subset we support.
- **The OSDI specification** — the public ABI for compact-model plugins; informs Interface β
  (`va-abi`) without reading OpenVAF's `osdi` crate.
- **rust-analyzer architecture docs** (MIT/Apache) — the canonical reference for the
  lexer/parser/HIR pattern OpenVAF adopts; safe to read and emulate in spirit.
- **Automatic-differentiation literature** — Griewank & Walther (*Evaluating Derivatives*),
  forward/reverse-mode and source-to-source AD papers — for T2.
- **SPICE/MNA & circuit-simulation texts** — for T3/T4/T5 (nodal analysis, Newton,
  integration, AC/noise).
- **ngspice** — used only as a validation **oracle** (`validation.md`); we never build on it.
- **Permissively-licensed crates** for utilities: `la-arena`/`id-arena`, `index_vec`,
  `fixedbitset`, `faer` (numerics). Prefer these over reimplementing — and never salvage
  OpenVAF's `lib/*` equivalents.

> Rule of thumb: **read the spec, not the source.** If an idea is in a published standard,
> paper, or permissive codebase, learn it there.

---

## OpenVAF, mapped to our architecture

A map of what conceptually corresponds — strictly as orientation for *where each thesis's
problem has been solved before*, never as a code source. (Crate names are OpenVAF's.)

| OpenVAF area | Conceptually maps to | Reference value | Notes |
|--------------|----------------------|-----------------|-------|
| `lexer`, `tokens`, `preprocessor`, `parser`, `syntax` | **T1** `va-frontend` | High | rust-analyzer-style **lossless `rowan` trees** — heavier than our mandated **arena AST** (`CLAUDE.md` §5). Borrow the phase structure, not the representation. |
| `hir`, `hir_def`, `hir_ty`, `hir_lower` | **T1** elaboration → `va-ir` | Medium | Good model for name resolution / type checking / lowering phases. |
| `mir`, **`mir_autodiff`** | **T2** `va-codegen` | **Highest** | Source-to-source AD over an SSA MIR — exactly T2's hard problem, and the **most legally sensitive** to read. See the stricter T2 policy below. |
| `sim_back`, `melange/core` | **T3** `va-core`, **T4** `va-transient` | High | `melange` is OpenVAF's actual circuit backend (assembly + solve). Useful shape for MNA/Newton/integration. |
| `osdi` | Interface β (`va-abi`) | Medium | Prefer the **public OSDI spec**; our `va-abi` is a deliberately simpler internal ABI. |
| `mir_llvm`, `target`, `linker`, `osdi` codegen | — | **None** | LLVM/native codegen — against our pure-Rust, no-native-deps rule (§5). |
| `verilogae/`, `setup.py` | — | None | Python bindings — out of scope. |
| `basedb`, `vfs`, salsa, `rowan` | — | Low | IDE-grade incremental-compilation DB — overkill for a batch simulator. |

Roughly half of OpenVAF (native codegen, Python, incremental DB) is **irrelevant by design**,
independent of licensing.

---

## Per-thesis clean-room policy

The default policy applies to every thesis; T2 carries an additional restriction.

**Default (T1, T3, T4, T5, T6):**

- You **may** read OpenVAF to understand *that* an approach works and to orient yourself in the
  problem space.
- You **must** implement from specs/papers and your own design. Do not transcribe,
  translate, or closely paraphrase OpenVAF source.
- When in doubt, cite the **public** source you actually used (LRM section, paper, OSDI spec,
  rust-analyzer doc) in your design notes and the relevant Quarto tutorial.

**T2 — `va-codegen` (automatic differentiation): stricter.**

- AD is the highest-risk, highest-value crate (`CLAUDE.md` §10) and the part of OpenVAF most
  tempting to copy. Treat `mir_autodiff` as **off-limits even for close reading**.
- Implement forward-mode (and, if pursued, reverse-mode) AD from the **literature** and the
  finite-difference validation contract (`CLAUDE.md` §5). Your dual-number / source-to-source
  design must be derivable from public references alone.
- This protects both the license boundary and the integrity of the thesis: the AD *is* the
  contribution.

---

## If someone still wants OpenVAF's code

That is a supervisor-level decision, not a thesis-level one. The only clean route is a
**separate GPL-3.0 project** that forks OpenVAF directly — kept out of this Apache-2.0
repository. It would be a different effort with a different licensing and pedagogical story
from the clean-room theses described here. Do not blur the two.

---

## Provenance hygiene (practical checklist)

- [ ] New code is written from public specs/papers/permissive crates — not from GPL source.
- [ ] Design notes / tutorials cite the actual public reference used.
- [ ] No file, function, or comment is transcribed or paraphrased from OpenVAF.
- [ ] Utility needs are met by permissive crates.io dependencies, not salvaged `lib/*`.
- [ ] `deny.toml` still passes (no native-link / disallowed-license deps creep in).
