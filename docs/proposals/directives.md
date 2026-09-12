# Proposal: every compiler directive does what the LRM says, or says why it cannot

**Status: proposed 2026-09-12, a 1.0 blocker by supervisor decision** ("I don't want the
preprocessor to consume with no effect"). Source of truth: Verilog-AMS LRM 2.4 Clause 10 and
Annex C.12 ("The compiler directives of Clause 10 are applicable to both Verilog-AMS HDL and
Verilog-A"), read from `references/VAMS-LRM-2-4.pdf`, not from memory.

**Affects:** `va-frontend` only (`preprocess.rs`, `lexer.rs`, `parser.rs`, `elaborate.rs`).
No Interface α or β change: a directive changes how source text is *read*, and everything it
decides is settled before the IR exists.

## 1. Where things stand

The LRM lists 20 directives (Clause 10, table on p. 263). `Preprocessor::process_line` today:

| Directive | LRM | Today | Corpus uses |
|---|---|---|---|
| `` `define ``, `` `undef ``, `` `include `` | 10.4, 1364 | implemented | many |
| `` `ifdef ``, `` `ifndef ``, `` `elsif ``, `` `else ``, `` `endif `` | 1364 | implemented | many |
| `` `default_discipline `` | **10.2** | consumed, no effect | 0 |
| `` `default_transition `` | **10.3** | consumed, no effect | 0 |
| `` `resetall `` | 1364 | consumed, no effect | 0 |
| `` `line `` | 1364 | consumed, no effect | 0 |
| `` `begin_keywords ``, `` `end_keywords `` | **10.6** | consumed, no effect | 0 |
| `` `timescale `` | 1364 | consumed, no effect | 0 |
| `` `pragma `` | 1364 | consumed, no effect | 0 |
| `` `celldefine ``, `` `endcelldefine `` | 1364 | **not recognised → "undefined macro"** | 0 |
| `` `default_nettype `` | 1364 | **not recognised → "undefined macro"** | 0 |
| `` `unconnected_drive ``, `` `nounconnected_drive `` | 1364 | **not recognised → "undefined macro"** | 0 |
| `` `default_nodetype `` | *not in the LRM* (Annex F change table: an obsolete AMS-1.x spelling of `default_discipline`) | consumed, no effect | 0 |

Zero corpus demand for any of the non-macro directives, so this is planned against the
standard, not against a failing file. That is also why none of it was found earlier: nothing
exercised it.

## 2. What each one means for Verilog-A, and what to do

The dividing line is whether the directive can change **what an analog block computes** or
**what a diagnostic says**. If it can, it is implemented. If the LRM itself gives it no
meaning in Verilog-A, it is recognised and *reported* — never silently dropped — because
"no effect" is then the LRM's own semantics for this simulator, and the user should be told
so rather than left to wonder.

### 2.1 Implement — these change the answer

**`` `default_discipline [discipline_identifier [qualifier]] ``** (LRM 10.2). Applies a
discipline to every net declared *without one* from this point in the text stream on, across
files, until another `` `default_discipline `` or `` `resetall ``; the bare form (no name)
disables the default. This is load-bearing here: a port with no discipline declaration is
currently the elaboration error "port `X` has no discipline declaration", and a model written
against a header that sets `` `default_discipline electrical `` is correct Verilog-A that this
engine rejects. The `qualifier` forms (`integer`, `real`, `reg`, `wreal`, `wire`, `tri`, …)
scope the default to *discrete* net kinds and have no Verilog-A meaning — Annex C.6/C.9
exclude the discrete domain — so a qualified form is an error naming Annex C, not a silent
accept.

*Mechanism.* The preprocessor already carries text-order state (the macro table); it gains
`default_discipline: Option<String>` and emits a synthetic marker line the parser understands
(the same trick a `` `line `` implementation needs, §2.2), so the parser records, per module,
the default in force where the module *starts* (the LRM scopes it to the text stream, which is
what "where the module starts" approximates for a directive that is illegal inside a module
body). Elaboration's "no discipline declaration" arm consults it before erroring. Precedence
rule (10.2, last paragraph): an explicit declaration always wins.

**`` `default_transition transition_time ``** (LRM 10.3, and 4.5.8's definition of
`transition`). Sets the default rise and fall time for every `transition()` that omits them,
from this point on. Today an omitted rise/fall is `0.0` (`elaborate.rs`, the `arg_or(2, 0.0)`
arm), which the LRM describes as "controlled by the simulator" when no directive is present —
acceptable as a default, wrong when a directive *is* present. And it has a second customer:
the Z-domain filters (§`docs/proposals/z-domain-filters.md`) hold their output between samples
and transition it with exactly this time.

*Mechanism.* Preprocessor evaluates the constant expression (it already const-folds `` `ifdef ``
arguments? — no: it will need a small constant evaluator, or emit the expression text into the
marker and let the parser const-eval it, which is the simpler route since the parser has
`const_eval`). Elaboration substitutes it where `transition`'s rise/fall are absent. `resetall`
clears it.

**`` `resetall ``** (1364-2005 §19.6). Resets every directive to its default. Meaningful the
moment the two above exist: it clears the default discipline and transition time (and the
keyword set, below). Not the macro table — 1364 is explicit that `` `resetall `` does not
undefine macros.

**`` `begin_keywords "spec" `` / `` `end_keywords ``** (LRM 10.6). Selects the reserved-word
set for a region: `"VAMS-2.3"` (Annex B — the set this lexer has), `"1364-2005"`, `"1364-2001"`,
`"1364-1995"` (progressively smaller pure-Verilog sets). The LRM's own example is a module
using `sin` as a port name under `"1364-2005"`, which is legal there and a keyword here. "Must
also be supported" is the LRM's phrasing. Zero corpus demand, but it is a *correctness*
directive for legacy digital-style headers included into analog designs.

*Mechanism.* The lexer's reserved set becomes a selectable table (`keywords.rs` gains the
three 1364 lists — they are subsets, so this is data, not logic), and the preprocessor emits
region markers the lexer honours. `end_keywords` restores the previous set (they nest);
`resetall` restores the default. Unknown spec string → error quoting the four legal ones.

**`` `line number "filename" level ``** (1364-2005 §19.7). Overrides the file and line that
subsequent diagnostics report. Every parse/elaboration error today says "at preprocessed line
N" — a number in the *expanded* text that the user has to map back by hand. Implementing
`` `line `` properly means the preprocessor keeps a line map (expanded line → original file and
line), which fixes the "preprocessed line" wart for every diagnostic at once, and `` `line ``
is then a one-arm addition to that map. This is the biggest item and the one with the most
user-visible payoff.

### 2.2 Recognise and report — the LRM gives these no analog meaning

**`` `timescale unit/precision ``** (1364-2005 §19.8). Sets the time unit for *digital* delay
controls (`#10`) and `$time`. Verilog-A has neither: Annex C.7 excludes delay controls, and
`$abstime` (9.17) is in seconds unconditionally. Conformant behaviour is to accept the
directive and let it change nothing — but the user is told: one warning per file, "`` `timescale ``
has no effect in Verilog-A: `$abstime` is in seconds and there are no delay controls".

**`` `celldefine `` / `` `endcelldefine ``** (1364-2005 §19.2). Tags a module as a library
cell for delay-annotation (SDF) and netlisting tools. No simulator semantics at all, in any
Verilog. Recognised, no warning (it is documentation, not an instruction to a simulator).

**`` `default_nettype type | none ``** (1364-2005 §19.3). The net type for *implicitly*
declared nets, and `none` to forbid them. In Verilog-A an implicit net (a name used in an
instance connection without a declaration) takes its discipline from `` `default_discipline ``;
`` `default_nettype `` chooses a *digital* net type (`wire`, `tri`, …) which Annex C excludes.
Plan: `none` is honoured — it is a stricter, meaningful setting: an undeclared net in an
instance connection becomes an error naming the directive (today it is already an error, so
this is a message improvement); any other type is accepted with the same one-per-file warning
shape as `` `timescale ``.

**`` `unconnected_drive pull0|pull1 `` / `` `nounconnected_drive ``** (1364-2005 §19.10).
Drives unconnected *digital input ports* to a logic level. No analog meaning; recognised with
the warning.

**`` `pragma ``** (1364-2005 §19.9). Tool-specific. 1364 requires an implementation to ignore
pragmas it does not understand, so accepting an unknown one *is* the conformant behaviour and
gets no warning. The exception that matters: `` `pragma protect begin_protected `` …
`` `end_protected `` wraps encrypted model text. That text would reach the lexer today and fail
with a garbage-token error. Plan: recognise the envelope and raise a `Refusal` — "encrypted
model text; a clean-room simulator holds no vendor keys" — which is what it is, and the one
`pragma` this project will ever need to understand.

### 2.3 Reject — not in the standard

**`` `default_nodetype ``** appears in the LRM only in Annex F's change table as an obsolete
Verilog-AMS 1.x spelling superseded by `` `default_discipline ``. Accepting it silently is
worse than an error: a model relying on it *expects* a default discipline that this engine
would not apply. Plan: an error, "obsolete directive; write `` `default_discipline ``". Any
other unrecognised backtick word stays what it is today, an undefined macro.

## 3. Order and size

1. `` `line `` and the line map (largest; every diagnostic improves). ~1 day.
2. `` `default_discipline `` with `` `resetall `` — the one that unlocks real models. ~½ day.
3. `` `default_transition `` — needed by the Z-domain filters anyway. ~2 hours.
4. `` `begin_keywords ``/`` `end_keywords `` — keyword tables + region markers. ~½ day.
5. The recognise-and-report group, `` `pragma protect `` refusal, `` `default_nodetype ``
   error — ~2 hours together.

Each lands with a test that a model *exercising* the directive parses/elaborates differently
with and without it (a `` `default_discipline electrical `` module whose ports declare no
discipline; a `transition` with omitted rise time under `` `default_transition 1n `` producing
a 1 ns ramp in transient; `sin` as a port name under `` `begin_keywords "1364-2005" ``; a
`` `line `` directive changing the file:line an error reports), and the `Directive` entry in
`docs/token-reference.md` states the outcome per directive.

## 4. What this does not do

It does not implement the discrete-domain semantics behind the qualifier forms of
`` `default_discipline `` or the net types of `` `default_nettype `` — those are Verilog-AMS
digital constructs Annex C excludes from Verilog-A, and the errors say so.
