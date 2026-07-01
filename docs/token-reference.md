# Verilog-A/AMS Token & Construct Reference — `va-frontend`

This document explains, for every token the `va-frontend` lexer produces and every construct
the parser recognizes, what it means and how this codebase treats it. It is grounded against
the reference LRMs in `references/` — principally `VAMS-LRM-2-4.pdf` (Accellera Verilog-AMS
LRM v2.4.0), cross-checked against `OVI_VerilogA.pdf` and `veriaref.pdf` — not against
recollection of the language. Where a construct is genuinely out of `va-frontend`'s v0 subset,
that is stated plainly rather than glossed over, per this project's "honest caveat" rule
(`CLAUDE.md` §1).

Each entry follows the same five-part structure:

- **Purpose and Static Nature** — is this resolved at elaboration (compile time), or does it
  have runtime/simulation-time behavior?
- **Declaration and Assignment** — the syntactic rule for where and how it is written.
- **Expressions and Evaluation** — how the parser/elaborator evaluates it, and whether its
  arguments must be static (compile-time constant) or may be dynamic (signal-dependent).
- **Structural and Analog Usage** — module-level/hierarchical use vs. use inside an `analog`
  block.
- **Comparison with Traditional Constructs** — the nearest digital-Verilog or C/C++ analogue,
  and where the analogy breaks down.

## A note on sources and scope

The Verilog-AMS LRM's own Annex B ("List of keywords") lists a larger reserved-word set (around
257 words, including SystemVerilog-configuration keywords like `config`/`liblist`/`connectmodule`
that Annex C.16 explicitly excludes from the Verilog-A subset) than this project's
`crates/va-frontend/src/keywords.rs::RESERVED_WORDS` (180 words). That's expected and correct —
`va-frontend` targets "single-module compact models" (`CLAUDE.md` §1), so words meaningful only
to full Verilog-AMS hierarchy, configuration, and digital timing checks are outside the declared
subset by design, not by oversight. The LRM's own Annex B (VAMS-LRM-2.4, p.380–382, Table B.1)
does list a further eleven words as reserved that the smaller source `keywords.rs`'s "166
reserved words" note was originally keyed to did not (plausibly `OVI_VerilogA.pdf`, the original
Verilog-A-only LRM, predating Verilog-AMS's generate/genvar/localparam additions) —
`aliasparam`/`genvar`/`endgenerate` (added in earlier work on this project), and, as of this
pass, `localparam`/`electrical`/`thermal` (each already had a dedicated `Token` variant — so was
reserved *in effect*, since `logos` matches a dedicated token unconditionally — but was missing
from `RESERVED_WORDS` itself, so the `keywords.rs`-level completeness test
(`every_reserved_word_is_reserved`) didn't exercise it) and the math builtins
`floor`/`ceil`/`round`/`int`/`limexp` (each a real, working call-expression builtin, but
previously unreserved — inconsistent with every other math builtin here, e.g. `exp`/`sqrt`/`ddt`,
which *is* reserved). All eleven are now listed, closing that gap.

A second gap this document surfaced and that has since been fixed: `transition` (§1.5) used to
parse as an ordinary call expression but fail at elaboration with "unknown function" — confirmed
live at the time by `va-cli check` on `external/verilogaLib-master/comparator_dynamic.va`. It now
folds to its `value` argument (the only sound answer under v0's DC-only model — see §1.5's
`Transition` entry for why), and that file now passes the frontend end to end.

---

# Part 1 — Lexer tokens

## 1.1 Non-keyword token kinds

These are `Token` variants defined by regex, not by a fixed spelling — each covers a whole
class of lexemes.

### `Ident(String)`

- **Purpose and Static Nature**: Purely lexical — carries any identifier-shaped lexeme
  (`[a-zA-Z_][a-zA-Z0-9_]*`) that isn't one of the 172 reserved words. Whether the identifier
  it names is itself static (a parameter, a genvar) or dynamic (a variable, a net) is decided
  later, by elaboration, not by the lexer.
- **Declaration and Assignment**: Any parameter, net, variable, branch, function, or genvar name
  is lexed as `Ident`. The two access-function names `V` and `I` are *also* plain `Ident`
  tokens — Verilog-A does not reserve them (LRM §5.5, "nature access functions"; see Part 2 §2.17
  for how the parser recognizes them contextually).
- **Expressions and Evaluation**: In expression position, an `Ident` followed by `(` is either
  an access-function call (`V(...)`/`I(...)`, if the name is `V`/`I`) or an ordinary function
  call (routed to `parse_call`); otherwise it is a bare reference, resolved at elaboration
  against parameters, genvars, then variables, in that order (see `elaborate.rs`'s
  `lower_expr`/`const_eval`).
- **Structural and Analog Usage**: Used identically at module level (parameter/net/branch names)
  and inside `analog` (variable/genvar references, access-function names).
- **Comparison with Traditional Constructs**: Same role as an identifier token in any C-family
  lexer. The one Verilog-A-specific wrinkle: case sensitivity is asymmetric — reserved words
  are recognized *only* lowercase (LRM §2), so `EXP`/`Exp` lex as ordinary `Ident`s while `exp`
  is reserved; C has no such asymmetry (keywords are simply fixed strings, case-sensitive
  throughout, with no separate escape hatch for a capitalized homograph).

### `Number(f64)`

- **Purpose and Static Nature**: Always a compile-time literal. A `Number` is a value, never a
  reference — there's nothing further to resolve at elaboration.
- **Declaration and Assignment**: N/A (it's an expression atom, not a declaration).
- **Expressions and Evaluation**: Regex `[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?[TGMKkmunpfa]?`,
  scaled by `parse_number` for an optional trailing SI suffix (`T`=1e12 … `a`=1e-18; note `M`
  meaning mega and `m` meaning milli is case-sensitive, per SI convention). Requires a leading
  digit (`0.5`, not `.5`) — a stated v0 limitation. Sized/based integer literals (`4'b0101`,
  digital Verilog's bit-vector literal syntax) are out of scope entirely.
- **Structural and Analog Usage**: Identical everywhere a numeric literal can appear — parameter
  defaults/ranges, analog expressions, case labels.
- **Comparison with Traditional Constructs**: Close to a C floating literal, but with
  engineering-notation scale suffixes (`1k`, `10n`) that C has no equivalent for (C requires an
  explicit multiplication, `10e-9`). Verilog's sized literals (`8'hFF`) have no C analogue and
  are unsupported here.

### `Str(String)`

- **Purpose and Static Nature**: A compile-time string literal, quotes stripped. Never has
  simulation-time behavior in this subset — it's only valid where the LRM expects a string
  (a system-task format argument, an `analysis("...")` phase name).
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: `ExprAst::Str` has no numeric value; `lower_expr` rejects it
  everywhere except as a system-task argument (`Stmt::Task`) or inside `analysis(...)`, whose
  arguments the elaborator inspects directly as strings (`analysis_matches`) rather than
  evaluating them as expressions.
- **Structural and Analog Usage**: Analog-block only in practice (`$strobe("...", ...)`,
  `analysis("dc")`) — there's no structural (module-level) use of a bare string in this subset.
- **Comparison with Traditional Constructs**: Same lexical shape as a C string literal, but far
  narrower semantically — no escape-sequence processing, no concatenation operator, and no
  general string *type* (it can't be assigned to a variable or compared) — closer to a
  `printf`-format-argument literal than to a first-class C `string`/`char*`.

### `SysFunc(String)`

- **Purpose and Static Nature**: A system function/task name with the leading `$` stripped.
  Split roughly evenly between elaboration-time (`$vt`, `$temperature`) and would-be
  simulation-time (`$strobe`, which is a documented no-op under v0's DC-only model).
- **Declaration and Assignment**: Never declared — the `$`-prefixed namespace is entirely
  predefined by the LRM (Clause 9, "System tasks and functions").
- **Expressions and Evaluation**: `$vt`/`$vt(T)` and `$temperature` are the only ones
  elaboration actually turns into IR (`Builtin::Vt`, `Builtin::Temperature`); `$simparam` folds
  to its `default` argument (or errors, matching the LRM's behavior for an unknown simulator
  parameter with no default — v0 has no simulator-parameter store at all); `$abstime` (the
  absolute simulation time) folds to a constant `0.0`, since v0 has no time axis at all and a DC
  operating point is conventionally t=0; anything else reachable as a `Stmt::Task` (`$strobe`,
  `$finish`, …) is parsed but elaborates to a no-op.
- **Structural and Analog Usage**: Analog-block only — `$vt`/`$temperature`/`$simparam` appear
  in expressions, `$strobe`-class calls are statements. None of these are meaningful at module
  (structural) scope.
- **Comparison with Traditional Constructs**: The closest C analogue is a compiler
  intrinsic/builtin (`__builtin_...`) or an environment query (`getenv`) — a name that isn't a
  user function but is still called with ordinary call syntax. Digital Verilog's `$display`
  family is the direct ancestor of `$strobe`.

### `Directive(String)`

- **Purpose and Static Nature**: A preprocessor directive name (leading `` ` `` stripped),
  purely a text-level, elaboration-time (in fact pre-elaboration) construct — it never survives
  into the IR.
- **Declaration and Assignment**: `` `include "file" ``, `` `define ``, `` `ifdef ``/`` `else ``/
  `` `endif ``, `` `default_discipline `` (see `crate::preprocess`).
- **Expressions and Evaluation**: Not an expression construct at all; handled by a dedicated
  preprocessing pass before lexing "real" tokens (macro objects/functions expand recursively,
  conditionals are evaluated against the defined-macro set). An unresolved `` `include `` is
  skipped rather than erroring, since the standard `disciplines.vams`/`constants.vams` headers'
  effects are built directly into elaboration.
- **Structural and Analog Usage**: Textual, so it can appear anywhere in source, but in practice
  only before the `module` keyword (headers) or around a macro-guarded declaration.
- **Comparison with Traditional Constructs**: Direct analogue of the C preprocessor (`#include`,
  `#define`, `#ifdef`) — same text-substitution model, same lack of scoping, same "runs before
  the real grammar" phase ordering.

## 1.2 Operators

### `Contribute` (`<+`)

- **Purpose and Static Nature**: Simulation-time — the branch contribution operator (LRM §5.6.1,
  "Direct branch contribution statements") describes a continuous-time relationship the
  simulator must solve for, not a one-shot compile-time computation.
- **Declaration and Assignment**: `branch_lvalue <+ analog_expression ;` — the left-hand side
  must be an access-function application to a branch (`V(...)`/`I(...)`), never a bare variable.
- **Expressions and Evaluation**: The right-hand side may be any expression over signals,
  parameters, and analog operators (`ddt`, `idt`, …); it is lowered to `Stmt::Contribute` and
  becomes a residual/Jacobian stamp in `va-core`'s Newton solve — genuinely dynamic, evaluated
  every iteration.
- **Structural and Analog Usage**: Analog-block only; this is *the* defining analog-block
  construct (LRM: "used in the analog block to describe continuous-time behavior").
- **Comparison with Traditional Constructs**: The LRM itself frames this precisely: contributions
  are *cumulative* — `I(a,b) <+ x; I(a,b) <+ y;` sums to `x + y` on the branch, whereas
  `Assign`'s procedural `=` (below) *replaces* the prior value, exactly like C's `=`. There is
  no digital-Verilog or C equivalent to summation-on-repeated-assignment; the nearest mental
  model is a KCL/KVL constraint accumulator, not an assignment.

### `Assign` (`=`)

- **Purpose and Static Nature**: Simulation-time when the target is an analog variable
  (re-evaluated every Newton iteration); compile-time-only in the narrow sense that
  `genvar_iteration`/`genvar_initialization` also use `=` but restrict its right-hand side to a
  static expression (see Part 2 §2.14).
- **Declaration and Assignment**: `lhs = rhs ;` (procedural assignment, `Stmt::Assign`), or
  bare `lhs = rhs` with no terminator inside a `for`-loop header
  (`parse_assignment`/`Stmt::For.init`/`.step`).
- **Expressions and Evaluation**: The right-hand side is an ordinary dynamic expression;
  elaboration resolves `lhs` against parameters (rejected — parameters aren't assignable),
  genvars (rejected outside a driving loop header — restricted assignment), then variables.
- **Structural and Analog Usage**: Analog-block only (module-level items have no procedural
  assignment; parameters/nets are declared, not assigned).
- **Comparison with Traditional Constructs**: Identical in spirit to C's `=` and digital
  Verilog's blocking assignment — replaces, doesn't accumulate. See `Contribute` above for the
  contrast that actually matters in this language.

### `Plus` (`+`), `Minus` (`-`), `Star` (`*`), `Slash` (`/`)

- **Purpose and Static Nature**: Purely structural (arithmetic) — static or dynamic depending
  entirely on their operands; the operator itself carries no timing.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: Standard left-associative binary arithmetic (`BinOp::Add/Sub/
  Mul/Div`), plus `Minus`/`Plus` double as unary prefix operators (`UnOp::Neg`; unary `+` is a
  parsed-and-discarded no-op). Both const-foldable (used in parameter-range/genvar
  const-evaluation) and runtime-evaluable (used in `<+`/`=` right-hand sides).
- **Structural and Analog Usage**: Identical everywhere expressions appear.
- **Comparison with Traditional Constructs**: Same as C/digital Verilog, no surprises.

### `StarStar` (`**`)

- **Purpose and Static Nature**: Structural; static or dynamic per its operands.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: Exponentiation (`BinOp::Pow`), right-associative and binding
  tighter than unary minus is *not* the case here — precedence follows the LRM's operator table
  (`parser.rs::binop_binding`), tested explicitly (`pow_is_right_associative`).
- **Structural and Analog Usage**: Identical everywhere.
- **Comparison with Traditional Constructs**: C has no exponentiation operator (`pow()` is a
  library call); digital Verilog gained `**` for elaboration-time bit-width computation, which
  Verilog-A repurposes as an ordinary real-valued operator.

### `EqEq` (`==`), `NotEq` (`!=`), `Le` (`<=`), `Lt` (`<`), `Ge` (`>=`), `Gt` (`>`)

- **Purpose and Static Nature**: Structural; static (parameter ranges, genvar conditions) or
  dynamic (a `for`/`while` condition, a ternary condition) per context.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: Yield `1.0`/`0.0` (`bool_to_f64`), never a distinct boolean
  type — consistent with Verilog-A having no `bool`.
- **Structural and Analog Usage**: Identical everywhere; notably also legal (and required to be
  *static*) in a `genvar_expression` loop condition (Part 2 §2.14).
- **Comparison with Traditional Constructs**: Same operators and precedence as C; the
  1.0/0.0-instead-of-a-real-bool representation matches C's historical lack of `_Bool` before
  C99 more than it matches digital Verilog's 4-valued (`0`/`1`/`x`/`z`) logic, which this v0
  subset does not model at all (LRM Annex C.4/C.5: `===`/`!==` case-equality and `x`/`z` values
  are the Verilog-AMS-only, not-in-Verilog-A, part of the language).

### `Not` (`!`), `AndAnd` (`&&`), `OrOr` (`||`)

- **Purpose and Static Nature**: Structural; static or dynamic per operand.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: `!` is `UnOp::Not` (logical negation, `bool_to_f64(x == 0.0)`);
  `&&`/`||` are `BinOp::And`/`Or`, both operands always evaluated (no short-circuiting is
  modeled — there's no side-effecting operand in a pure analog expression for short-circuiting
  to matter to).
- **Structural and Analog Usage**: Identical everywhere.
- **Comparison with Traditional Constructs**: Same operators as C. See `Shl`/`Shr`/`Amp`/`Pipe`/
  `Caret`/`CaretTilde`/`Tilde` below for the separate bitwise family (`&`, `|`, `^`, `^~`/`~^`,
  `~`) and shifts (`<<`, `>>`) — distinct tokens from these logical ones, added in a later pass
  once real corpus code (`(digital >> i) & 1`, an integer-accumulator idiom) needed them.

### `Shl` (`<<`), `Shr` (`>>`), `Amp` (`&`), `Pipe` (`|`), `Caret` (`^`), `CaretTilde` (`^~`/`~^`), and unary `Tilde` (`~`)

- **Purpose and Static Nature**: Structural; static (const-evaluable in a parameter default or
  genvar loop header, e.g. `parameter integer mask = (1 << width) - 1;`) or dynamic per operand,
  exactly like the arithmetic operators.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: There is no bit-vector type in this project — every value is
  `f64` — so each of these truncates its operand(s) to `i64` (`to_i64`, shared with `int()`/
  `floor()`'s float↔integer bridging elsewhere), performs the bitwise/shift operation, and casts
  the result back to `f64`. `>>` is a *logical* (zero-fill) shift — this project has no signed/
  unsigned integer distinction to make an arithmetic shift matter. `^~` and `~^` are accepted as
  two spellings of the same XNOR operator (`BinOp::BitXnor`), matching the LRM. Both the dynamic
  path (`va-codegen`'s AD) and the constant-folding path (`Elaborator::const_eval`/`eval_binop`)
  treat these as **zero-gradient** where AD is concerned — like the comparison operators, a
  bitwise/shift result has no continuous derivative, so `va-codegen` returns `Dual::constant(...)`
  for them rather than attempting to differentiate through a bit pattern.
- **Structural and Analog Usage**: Identical everywhere expressions appear, including parameter
  defaults/ranges and genvar loop headers (both const-evaluated).
- **Comparison with Traditional Constructs**: Same operators, precedence, and (for `>>`)
  logical-shift semantics as C on an unsigned type. Operator precedence follows Verilog's own
  table (IEEE 1364 Table 5-4) rather than C's — notably, in both languages shifts bind *looser*
  than `+`/`-` but *tighter* than relational operators, and `&`/`^`/`|` sit between `&&` and
  `==` (loosest to tightest: `||` < `&&` < `|` < `^`/`^~` < `&` < `==`/`!=` < `<`/`<=`/`>`/`>=` <
  `<<`/`>>` < `+`/`-` < `*`/`/` < unary < `**`) — this project's `binop_binding` table matches it
  exactly now that these are implemented.

## 1.3 Punctuation

### `LParen` (`(`), `RParen` (`)`)

- **Purpose and Static Nature**: Purely structural delimiters.
- **Declaration and Assignment**: Group expressions, wrap call/access-function arguments, wrap
  `if`/`while`/`for`/`case`/`repeat` control expressions, and delimit an `@(...)` event
  controller (whose contents v0 discards wholesale — Part 2 §2.19).
- **Expressions and Evaluation**: `(expr)` simply re-returns the inner `ExprRef` (no IR node of
  its own); everywhere else they are consumed positionally by `eat(&Token::LParen/RParen)`.
- **Structural and Analog Usage**: Both module-level (port lists, parameter/branch-declaration
  terminal lists) and analog-block (calls, control expressions).
- **Comparison with Traditional Constructs**: Identical role to C/digital Verilog parens.

### `LBracket` (`[`), `RBracket` (`]`)

- **Purpose and Static Nature**: Structural, but what they delimit is always compile-time
  static in this subset: a parameter range bound (`from [0:inf)`) or a vector-net declaration's
  width (`electrical [3:0] bus;`), or the bracketed index of a vector-net access (`V(bus[i])`) —
  which must itself const-evaluate to an integer (a genvar expression, in the LRM's terms).
- **Declaration and Assignment**: Three distinct grammar uses, disambiguated by context: (1)
  an *inclusive* range-bound delimiter (`open_bound`/`close_bound`, where `(`/`)` are the
  exclusive alternative), (2) a vector net's `[msb:lsb]` declaration, (3) a `NetArg`'s index
  (`name[index_expr]`).
- **Expressions and Evaluation**: The vector-index form is evaluated by
  `Elaborator::const_eval_int`, which requires an integral result and bounds-checks it against
  the vector's declared `(lo, hi)` range — a hard error, not a runtime out-of-bounds condition,
  since the index must be resolvable at elaboration.
- **Structural and Analog Usage**: Range bounds and vector declarations are module-level;
  indexed access (`V(bus[i])`) is analog-block-only.
- **Comparison with Traditional Constructs**: The vector-net use is the direct Verilog-A
  analogue of a C array subscript, but with a crucial restriction the LRM states explicitly
  (§5.5.2): the index "must be a constant expression, though it may include genvar variables" —
  unlike C, where `a[i]` allows `i` to be any runtime value.

### `At` (`@`)

- **Purpose and Static Nature**: Introduces an event controller. In full Verilog-AMS this is a
  genuinely simulation-time construct (the controlled statement runs when the event triggers);
  v0 flattens this to "runs unconditionally," which is exact for `@(initial_step)` under a
  DC-only analysis and an approximation everywhere else (a stated limitation).
- **Declaration and Assignment**: `@(event_expr) statement`.
- **Expressions and Evaluation**: v0 does not parse `event_expr` as an expression at all — it
  calls `skip_balanced_parens` to discard everything between the matching `(`/`)`, then parses
  the controlled statement and runs it unconditionally.
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: No C equivalent (C has no event/wait model at the
  language level). Closer to digital Verilog's `@(posedge clk)`, except digital Verilog's
  version genuinely gates simulation-time scheduling, which this subset does not model.

### `Question` (`?`)

- **Purpose and Static Nature**: Structural; static (a `genvar_expression`'s ternary form is
  explicitly legal per the LRM's `genvar_expression` grammar) or dynamic per its operands.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: `cond ? then_ : else_`, right-associative, binds looser than
  every binary operator (`parse_expr`, tested by
  `ternary_parses_and_is_right_associative`). Lowers to `Expr::Select`; only the selected branch
  is evaluated (division-by-zero guards like `x != 0 ? 1/x : 0` rely on this).
- **Structural and Analog Usage**: Anywhere an expression can appear, including parameter
  defaults/ranges and genvar loop headers (both const-evaluated).
- **Comparison with Traditional Constructs**: Identical to C's `?:` in both syntax and
  short-circuit-evaluation semantics.

### `Comma` (`,`)

- **Purpose and Static Nature**: Purely structural list separator.
- **Declaration and Assignment**: Separates: port/net/variable/genvar name lists
  (`ident_list`), a branch's one-or-two terminals, a call/access-function's arguments, a
  `case` arm's label list.
- **Expressions and Evaluation**: Never itself evaluated; purely a delimiter consumed by
  `ident_list`/`parse_call_args`/`parse_access`/`parse_net_arg`-adjacent call sites.
- **Structural and Analog Usage**: Both.
- **Comparison with Traditional Constructs**: Same role as C's comma in declarator/argument
  lists (not the C comma *operator*, which this language doesn't have).

### `Semicolon` (`;`)

- **Purpose and Static Nature**: Purely structural statement/declaration terminator.
- **Declaration and Assignment**: Terminates every declaration and every simple statement.
- **Expressions and Evaluation**: N/A.
- **Structural and Analog Usage**: Both.
- **Comparison with Traditional Constructs**: Identical to C's statement-terminating `;`.

### `Colon` (`:`)

- **Purpose and Static Nature**: Purely structural.
- **Declaration and Assignment**: Separates a range's low/high bound (`from [0:inf)`), a vector
  net's msb/lsb (`[3:0]`), a ternary's then/else arm (`cond ? a : b`), a `case` arm's labels
  from its body, and an optional `begin : label` block name.
- **Expressions and Evaluation**: N/A (pure delimiter).
- **Structural and Analog Usage**: Both.
- **Comparison with Traditional Constructs**: The range/ternary uses match Verilog generally
  (`msb:lsb` has no direct C analogue since C arrays don't declare a bit range); the ternary use
  matches C's `?:` exactly.

### `Dot` (`.`)

- **Purpose and Static Nature**: Lexed but not consumed by any v0 grammar production.
- **Declaration and Assignment**: In full Verilog(-AMS), used for named port connections in an
  instantiation (`.p(node_a)`) — module instantiation itself is out of this project's declared
  subset (single-module compact models), so `.` currently has no parser rule that reaches it;
  any occurrence is presently a parse error via the default "expected a statement"/"expected an
  expression" fallback.
- **Expressions and Evaluation**: N/A today.
- **Structural and Analog Usage**: N/A today.
- **Comparison with Traditional Constructs**: In C, `.` is struct member access — no analogue
  exists in this subset (no struct/record type).

## 1.4 Dedicated structural keyword tokens

These 21 words each get their own `Token` variant (matched unconditionally by `logos`, ahead of
the generic `Keyword` fallback) because the grammar dispatches on them directly and repeatedly.
All 21 (`module`, `analog`, `begin`, `end`, `endmodule`, `parameter`, `localparam`, `real`,
`integer`, `genvar`, `input`, `output`, `inout`, `electrical`, `thermal`, `if`, `else`, `from`,
`exclude`, `inf`, `ground`) are now also listed in `RESERVED_WORDS` (`localparam`/`electrical`/
`thermal` were the gap noted above, closed by this pass).

### `Module` / `EndModule`

- **Purpose and Static Nature**: Purely structural — brackets the entire elaborated unit;
  carries no per-instance runtime state itself.
- **Declaration and Assignment**: `module name ( port_list ) ; ... endmodule` (LRM §6, "Hierar-
  chical structures"; `parse_module`). v0 parses exactly one module per source unit — no nested
  or multiple modules.
- **Expressions and Evaluation**: N/A — pure structure.
- **Structural and Analog Usage**: Module-level only; this *is* the module-level scope.
- **Comparison with Traditional Constructs**: A C translation unit is the loose analogue
  (top-level container); a digital-Verilog `module`/`endmodule` is the direct one, except this
  project doesn't support module *instantiation* (one module containing another) — "single-
  module compact models" (`CLAUDE.md` §1) is a hard scope boundary, not a stepping stone.

### `Analog`

- **Purpose and Static Nature**: Structural marker for the one block that runs at
  simulation-time. `analog function` (checked via one-token lookahead against the following
  `function` keyword) is a compile-time-callable subroutine definition instead — same leading
  token, different construct.
- **Declaration and Assignment**: `analog begin ... end` (a bare `analog stmt;` single-statement
  form is also legal, normalized to a one-element block) or `analog function ...
  endfunction` — see Part 2 §2.9.
- **Expressions and Evaluation**: N/A — introduces a statement/definition, not an expression.
- **Structural and Analog Usage**: The keyword itself is module-level (it's an `Item`), but
  everything inside the `analog begin...end` it introduces is the analog-block scope proper —
  this is the boundary between the two.
- **Comparison with Traditional Constructs**: No C equivalent (C has no single privileged
  "runs continuously" block). Loosely analogous to a digital-Verilog `always` block, except
  `analog` runs continuously for the conservative/signal-flow solve rather than being
  event-triggered.

### `Begin` / `End`

- **Purpose and Static Nature**: Purely structural block delimiters; carry no runtime state of
  their own.
- **Declaration and Assignment**: `begin [: label] stmt... end`, wherever the grammar accepts
  either a single statement or a block (`if`/`else` arms, loop bodies, the `analog` block
  itself). An optional `: label` names the block (Verilog-A permits this for disambiguation in
  nested/disabled blocks); v0 parses and discards the label — block naming/disable-by-name is
  out of scope.
- **Expressions and Evaluation**: N/A.
- **Structural and Analog Usage**: Analog-block only (there's no `begin...end` at module/item
  level in this subset).
- **Comparison with Traditional Constructs**: Direct equivalent of C's `{ ... }` — including the
  same "optional if it's a single statement" rule that C's `if`/`while`/`for` bodies follow.

### `Parameter` / `LocalParam`

- **Purpose and Static Nature**: Elaboration-only. A parameter is const-evaluated once,
  up-front (`collect_params`), into a fixed `f64` default plus optional `min`/`max`; it never
  becomes a runtime variable.
- **Declaration and Assignment**: `parameter [real|integer] name = default_expr [from range]
  [exclude ...] ;` (LRM §3.2/Annex A `parameter_declaration`); `localparam` shares the exact
  same grammar. v0 lowers both to the identical `Item::Param`/IR `Param` — the LRM distinction
  (a `localparam` cannot be overridden from an instantiating netlist) is moot here because
  `va-netlist` has no by-name instance-parameter-override path at all yet.
- **Expressions and Evaluation**: `default_expr` and any `from`/`exclude` bounds must be
  compile-time constant — literals, arithmetic, the real math builtins, and (new) genvars are
  all legal; `$vt`, probes (`V(...)`/`I(...)`), and forward references to another parameter are
  rejected by `const_eval` with a clear error.
- **Structural and Analog Usage**: Module-level declaration only; referenced (read-only) from
  inside the analog block via an ordinary identifier lookup (`Expr::Param`).
- **Comparison with Traditional Constructs**: Closest to a C `const` global initialized from a
  compile-time-constant expression — genuinely a compile-time binding, not a variable, even
  though it reads like one syntactically. Digital Verilog's `parameter`/`localparam` distinction
  around instance overrides has no C analogue at all (C has no notion of "this constant can be
  overridden per translation unit that includes it").

### `Real` / `Integer`

- **Purpose and Static Nature**: Both are simulation-time-valued (an assignable analog
  variable) *unless* they appear as a `parameter`'s or `analog function`'s declared base type,
  in which case the type just tags a compile-time constant/return value; the IR itself has no
  variable-type distinction at all — every value is `f64` (`va_ir::VarDecl` carries no type).
- **Declaration and Assignment**: `real name, name, ...;` / `integer name, name, ...;` at module
  scope (`Item::Var`) or block scope (`Stmt::VarDecl`); also the optional base-type prefix on a
  `parameter`/`localparam` (defaults to `real` if omitted) and an `analog function`'s
  return-type prefix.
- **Expressions and Evaluation**: Declaring a name introduces it into scope with no initial
  value; it becomes assignable via `=`. The type itself is parsed and then *discarded* — v0
  performs no integer-vs-real type checking or truncation (a stated `va-ir` limitation).
- **Structural and Analog Usage**: Both module-level (`real x;`) and analog-block-local
  (`real x;` inside `begin...end`) declarations are legal and treated identically by
  elaboration (a block-local declaration just registers the same kind of `VarId`).
- **Comparison with Traditional Constructs**: Reads exactly like a C `double`/`int` declaration,
  but v0's "declared type is parsed and dropped" behavior means it behaves more like a
  dynamically-typed language's variable declaration (Python's bare `x = 0`) than like C's
  statically-checked one — a real, if narrow, gap from full Verilog-A's actual `integer`
  truncation-on-assignment semantics.

### `Genvar`

- **Purpose and Static Nature**: **Elaboration-only, in the strictest sense of any construct in
  this language.** Per LRM §3.5 ("Genvars"): "Genvars are integer-valued variables which compose
  static expressions for instantiating structure behaviorally... The static nature of genvar
  variables is derived from the limitations upon the contexts in which their values can be
  assigned." A genvar never has a runtime value — `va-frontend` fully unrolls the loop it
  drives before the IR is even built, so no `va_ir` node ever represents "a genvar."
- **Declaration and Assignment**: `genvar list_of_genvar_identifiers ;` (LRM Syntax 3-3,
  `genvar_declaration`) — module scope only, lowered to `Item::Genvar`. Per the LRM: "The genvar
  variable `i` can only be assigned within the for-loop control. Assignments to the genvar
  variable `i` can consist only of expressions of static values, e.g., parameters, literals, and
  other genvar variables." `va-frontend` enforces exactly this: any `Stmt::Assign` to a genvar
  name that isn't the `init`/`step` of the `for` loop it drives is rejected
  ("restricted assignment"), and `init`/`cond`/`step` are evaluated by the same `const_eval`
  used for parameter ranges (so a probe or `$vt` there is a hard error, not silently accepted).
- **Expressions and Evaluation**: A genvar's value, once bound by its driving loop, reads as
  `Expr::Const` everywhere it's referenced — in ordinary expressions and as a vector-net index
  alike. LRM §5.5.2 states this precisely for indexing: "The index must be a constant
  expression, though it may include genvar variables." Per LRM §4.5.15, "Analog operators are
  not allowed in the repeat, while and non-genvar for looping statements" — meaning a
  genvar-driven `for` is the *one* loop shape where `ddt`/`idt` are legal inside the body, and
  `va-frontend` gets this for free by unrolling the loop into flat, already-distinct code before
  lowering it (see Part 2 §2.14).
- **Structural and Analog Usage**: Declared at module scope; its only legal *use* is inside the
  analog block, as a `for` loop's control variable. Per LRM §3.6/Annex: "the genvar variable
  `i`... allows... accessing analog signals within behavioral looping constructs" — i.e. it
  exists specifically to let a single piece of source text describe a repeated analog structure
  (a bus of contributions), which the LRM frames as a scope: "Within a generate loop, each
  iteration creates a separate hierarchy scope" with "an implicit localparam" of the genvar's
  name and per-iteration value. `va-frontend` reproduces the *value* half of that faithfully
  (each unrolled iteration sees its own constant), and — since this is a flat, single-module IR
  with no separate hierarchical-instance concept — reproduces the "separate scope" half simply
  by each iteration being distinct, already-substituted code; there is no additional named
  per-instance scope object.
- **Comparison with Traditional Constructs**: Digital Verilog and SystemVerilog use `genvar`
  purely to instantiate an *array of module instances* at elaboration (`generate for (i=0;
  i<N; i=i+1) my_mod inst(...);`) — this project has no module instantiation at all, so its
  only use here is the "unroll analog code, indexing a signal vector" half of the LRM's genvar
  story. The nearest C/C++ analogue is a compile-time-unrolled loop (`#pragma unroll`, or a C++
  `template <int I>` recursion) — an index that exists purely to shape the generated code, never
  present as a runtime value, which is exactly genvar's "static nature."

### `Input` / `Output` / `Inout`

- **Purpose and Static Nature**: Structural — declares a port's direction; carries no runtime
  value of its own (the *node* the port names does).
- **Declaration and Assignment**: `input name, ...;` / `output name, ...;` / `inout name, ...;`
  (LRM §6, port declarations). A port additionally needs a discipline declaration
  (`electrical`/`thermal`) naming the same net to become a resolvable node —
  `resolve_ports` rejects a port with direction but no discipline.
- **Expressions and Evaluation**: N/A — pure declaration.
- **Structural and Analog Usage**: Module-level only.
- **Comparison with Traditional Constructs**: Analogous to a C function's parameter direction
  (though C has no `inout` — closest is a non-`const` pointer/reference parameter). `inout` is
  the default and by far the most common direction for an analog terminal (LRM: "`inout` is
  the default for analog bidirectional ports" — an electrical terminal is inherently
  bidirectional, unlike a digital pin).

### `Electrical` / `Thermal`

- **Purpose and Static Nature**: Structural — a discipline declaration binds a net to a
  physical discipline (LRM §4, "Disciplines and natures"); the discipline governs which
  quantities (`V`=potential/`I`=flow for electrical; temperature/power for thermal) that node's
  branches carry, but the declaration itself has no runtime value.
- **Declaration and Assignment**: `electrical name, ...;` / `thermal name, ...;`, optionally
  preceded by a `[msb:lsb]` vector-width bracket (declaring a bus of nodes rather than one
  scalar node — see Part 2 §2.18). Unlike the general LRM, which lets a user `discipline`
  declaration bind arbitrary natures, v0 hardcodes exactly these two disciplines as built-ins
  (a stated limitation — user `discipline...enddiscipline`/`nature...endnature` blocks are
  parsed only enough to be skipped, never modeled; see §1.5's `discipline`/`nature` entries).
- **Expressions and Evaluation**: N/A — pure declaration; the discipline is looked up once
  (`collect_nodes`) and attached to each interned `NodeId`.
- **Structural and Analog Usage**: Module-level declaration; referenced from the analog block
  only indirectly, through `V(...)`/`I(...)` access-function calls naming the net.
- **Comparison with Traditional Constructs**: No C analogue (C has no notion of a physical
  discipline). The closest digital-Verilog concept is a `wire`/`reg`'s bit width — both are a
  net-level type annotation — but "discipline" carries physics (KVL/KCL semantics), not bit
  width.

### `Ground`

- **Purpose and Static Nature**: Structural — declares that a net is the (or a) reference node.
- **Declaration and Assignment**: `ground name;` per the LRM; note this project's v0 doesn't
  actually parse a distinct `ground` *declaration* item at all (there is no `Token::Ground`
  match arm in `parse_item` today) — the reference node used for a single-terminal access
  (`V(a)` meaning "potential of `a` relative to reference") is instead created implicitly and
  named `"gnd"` the first time it's needed (`Elaborator::reference_node`). `ground` is reserved
  (has a dedicated token) purely to keep the identifier available for this convention and to
  match the LRM's reserved-word list — it currently has no grammar production of its own.
- **Expressions and Evaluation**: N/A today.
- **Structural and Analog Usage**: Would be module-level if implemented.
- **Comparison with Traditional Constructs**: The electrical-circuit notion of "ground" has no
  general-purpose-language analogue; the closest structural parallel is a distinguished
  "origin"/"zero" sentinel value.

### `If` / `Else`

- **Purpose and Static Nature**: Simulation-time in the analog block (the branch taken can
  depend on a signal value, re-evaluated every Newton iteration); note the LRM restriction
  (§4.5.15) that an analog operator (`ddt`, `idt`, …) is only legal inside an `if`/`case`/`?:`
  when the controlling condition is itself a compile-time constant — `va-frontend` does not
  currently enforce this restriction (a gap, not claimed as implemented).
- **Declaration and Assignment**: `if ( cond ) then_stmt [else else_stmt]`, both arms accepting
  either a single statement or a `begin...end` block, normalized to `Stmt::If { cond, then_,
  else_ }` (an absent `else` becomes an empty `else_` list).
- **Expressions and Evaluation**: `cond` is an ordinary dynamic expression, lowered and
  evaluated every solve — this is not const-evaluated (unlike a genvar loop's condition).
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: Identical to C's `if`/`else` in grammar and
  semantics (a non-zero condition selects `then_`), modulo the analog-operator restriction noted
  above, which has no C analogue (C conditionals never interact with a stateful "operator" the
  way `ddt` does).

### `From` / `Exclude` / `Inf`

- **Purpose and Static Nature**: Elaboration-only — all three appear exclusively inside a
  `parameter`/`localparam` range clause, const-evaluated once.
- **Declaration and Assignment**: `from ( lo : hi )` / `from [ lo : hi ]` (mixed
  inclusive/exclusive delimiters in any combination, e.g. `from [0:inf)`), followed by zero or
  more `exclude value` / `exclude (lo:hi)` clauses. `inf` is a literal meaning `f64::INFINITY`,
  used as an open bound (`from [0:inf)`).
- **Expressions and Evaluation**: `from`'s bounds are const-evaluated into `Param::min`/`max`
  (losing the inclusive/exclusive distinction — a stated limitation: both a `[`- and
  `(`-delimited bound collapse to the same `Option<f64>`). `exclude` clauses are parsed (so
  malformed ones are still caught) but their values are discarded — v0 does not enforce
  exclusion ranges at all.
- **Structural and Analog Usage**: Module-level only (parameter declarations).
- **Comparison with Traditional Constructs**: No direct C analogue — closest is a runtime
  assertion/precondition (`assert(0 <= r && r < INFINITY)`), except here the range is
  documentation/validation metadata attached to the parameter declaration itself, evaluated by
  the *tool* rather than by generated runtime code.

## 1.5 Reserved words carried generically (`Token::Keyword`) — implemented subset

Every other reserved word lexes as `Token::Keyword(Keyword)`, a payload the parser inspects by
string (`at_keyword`/`eat_keyword`, or the `Some(&Token::Keyword(kw)) => match kw.as_str()
{...}` dispatch in `parse_stmt`). The words below are the ones with real, working
grammar/elaboration behavior; §1.6 gives the master table covering every one of the 172 words,
including the ones with no implemented behavior at all.

### `Branch`

- **Purpose and Static Nature**: Elaboration-only — resolves a name to a fixed `BranchId`
  (interned once, by its `(p, n)` node pair) so later `V(name)`/`I(name)` accesses in the analog
  block reference the same branch as an equivalent positional `V(a,b)`.
- **Declaration and Assignment**: `branch (a[, b]) name [, name...] ;` (LRM §4.7, "Named
  branches"). One or two terminals; a single terminal implies node-to-reference.
- **Expressions and Evaluation**: The terminals themselves may now be vector-indexed
  (`NetArg`), though resolved with an empty genvar environment at declaration time (branch
  declarations are module-level, outside any loop), so an index there must already be a
  literal or parameter-derived constant.
- **Structural and Analog Usage**: The declaration is module-level; the name it introduces is
  used from the analog block exactly like a positional access.
- **Comparison with Traditional Constructs**: A named alias, closest to a C `#define`/`typedef`
  for a recurring expression — except it's resolved once, structurally, by the elaborator, not
  textually by a preprocessor.

### `Aliasparam`

- **Purpose and Static Nature**: Elaboration-only — introduces no new value at all, just a
  second name resolving to an *already-declared* parameter's existing `ParamId`/value.
- **Declaration and Assignment**: `aliasparam name = target ;` — a fixed `identifier =
  identifier` shape, not a general expression; `target` must already be declared (forward
  references are rejected, matching this project's broader "no forward reference" policy for
  parameters and functions).
- **Expressions and Evaluation**: N/A beyond the name-to-name lookup.
- **Structural and Analog Usage**: Module-level only.
- **Comparison with Traditional Constructs**: Closest to a C reference/alias (`int &b = a;`) or
  a shell `alias` — a second name, zero new storage.

### `Generate` / `Endgenerate`

- **Purpose and Static Nature**: Purely a syntactic bracket — carries no semantics of its own in
  this v0 subset.
- **Declaration and Assignment**: `generate ... endgenerate`, parsed exactly like `begin...end`
  (`parse_generate`): consume statements until `endgenerate`, return them for the caller to wrap
  in a `Stmt::Block`. The LRM's own grammar (`loop_generate_construct ::= for (...)
  generate_block`) treats `generate`/`endgenerate` as one of several ways to spell a
  `generate_block`; this project only implements the `for`-loop form, and doesn't require the
  bracket at all — a bare genvar-driven `for` (no `generate`/`endgenerate` around it) is
  equally legal and behaves identically (see Part 2 §2.14/§2.15).
- **Expressions and Evaluation**: N/A.
- **Structural and Analog Usage**: v0 only supports this inside the analog block (an
  `analog_loop_generate_statement`, per LRM §5.9.3); there is no module-item-level
  `generate`/`endgenerate` (which the full LRM also allows, for generating repeated net/branch
  declarations or instances) — out of scope, since this project has no multi-instance
  hierarchy to generate into.
- **Comparison with Traditional Constructs**: Nothing in C corresponds to "a bracket that means
  nothing on its own but whose contents may be specially interpreted based on what's inside" —
  the nearest parallel is a C preprocessor `#if`/`#endif` pair whose *branch* is what matters,
  not the bracket tokens themselves.

### `Function` / `Endfunction`

- **Purpose and Static Nature**: Structural — brackets a user-defined `analog function`
  definition, which is itself a compile-time-bound callable (resolved once, in source order, to
  a `FuncId`) whose *body* runs at simulation time when called.
- **Declaration and Assignment**: `analog function [real|integer] name ; [direction
  args;] [real|integer locals;] body... endfunction` (Part 2 §2.9 covers the full production).
  `endfunction` closes it.
- **Expressions and Evaluation**: N/A for the keywords themselves; the function's body is an
  ordinary dynamic statement sequence.
- **Structural and Analog Usage**: Declared at module scope (an `Item::Function`), callable
  only from inside the analog block (or from another function, forward references excepted).
- **Comparison with Traditional Constructs**: A C `static` pure function is the closest analogue
  — Verilog-A analog functions are documented as pure and non-recursive, and (per this project's
  v0) the function name doubles as the implicit return variable, unlike C's explicit `return`.

### `While` / `Repeat` / `For` / `Case` / `Endcase` / `Default`

- **Purpose and Static Nature**: Simulation-time control flow *except* when `for`'s header
  assigns a declared genvar, in which case it is fully resolved at elaboration instead (see
  `Genvar` above and Part 2 §2.14). Per LRM §4.5.15, analog operators are illegal inside
  `while`/`repeat`/an ordinary (non-genvar) `for` — a restriction this project does not
  currently enforce for `while`/`repeat`/ordinary `for` (a stated gap), even though it is
  automatically satisfied for the genvar case by unrolling.
- **Declaration and Assignment**: `while (cond) body`; `repeat (count) body`; `for (init; cond;
  step) body`; `case (selector) label,...: body ... [default[:] body] endcase`. `default`'s
  colon is optional, matching general Verilog usage.
- **Expressions and Evaluation**: All conditions/counts/selectors/labels are ordinary dynamic
  expressions (except a genvar-driving `for`'s `init`/`cond`/`step`, which are const-evaluated).
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: `while`/`for`/`case` map directly to C's; `repeat
  (n) body` has no C equivalent (closest: `for (int i = 0; i < n; i++) body`, minus an explicit
  loop variable) but matches digital Verilog's `repeat` exactly. `casex`/`casez` (wildcard-match
  case, meaningful only for Verilog's 4-state values) are explicitly *not* part of Verilog-A at
  all per LRM Annex C.7 ("casex and casez are not supported in Verilog-A") — so their absence
  from this project's grammar (§1.6) is spec-correct, not a gap.

### Math builtins (`abs`, `acos`, `acosh`, `asin`, `asinh`, `atan`, `atan2`, `atanh`, `ceil`, `cos`, `cosh`, `exp`, `floor`, `hypot`, `int`, `limexp`, `ln`, `log`, `max`, `min`, `pow`, `round`, `sin`, `sinh`, `sqrt`, `tan`, `tanh`)

- **Purpose and Static Nature**: Dual — usable both in dynamic analog expressions (evaluated,
  with derivatives taken, every Newton iteration) and in compile-time-constant contexts
  (parameter ranges, genvar loop headers), via the same name resolving to different evaluation
  paths (`call_builtin` for the dynamic `Expr::Call`, `eval_const_call` for `const_eval`'s
  compile-time fold).
- **Declaration and Assignment**: Never declared — predefined; called as `name(args)`, one or
  two real-valued arguments depending on the function (`atan2`/`hypot`/`max`/`min`/`pow` take
  two, the rest take one). `floor`/`ceil`/`round`/`int` implement the four standard rounding
  modes (toward −∞, toward +∞, nearest, toward zero); `limexp` is exactly `exp` under the hood
  in this project (no actual numerical limiting is modeled — a stated simplification, since it's
  documented as a Newton-convergence aid whose *value* and *derivative* this project treats as
  plain `exp`).
- **Expressions and Evaluation**: In dynamic context, `va-codegen`'s automatic differentiation
  is expected to differentiate these (per `CLAUDE.md` §5, validated against finite differences);
  in const context, `eval_const_call` computes the same function numerically once, no
  derivative needed.
- **Structural and Analog Usage**: Analog-block (dynamic use) and module-level parameter/genvar
  declarations (static use) alike.
- **Comparison with Traditional Constructs**: Direct analogues of C's `<math.h>` (`exp`, `log`,
  `sqrt`, `pow`, `hypot`, `floor`, `ceil`, `round`, `sin`/`cos`/`tan` and their inverse/hyperbolic
  forms, `atan2`) plus `min`/`max` (not in C89's `<math.h>`, but common library/language
  extensions since). Unlike C's math functions — never keywords there — every one of these,
  including `floor`/`ceil`/`round`/`int`/`limexp`, is a reserved word here (see §1.6's master
  table; before this pass, those five were implemented but not reserved, letting a user shadow
  the name — a gap this document found and this pass closed).

### `Ddt` / `Idt`

- **Purpose and Static Nature**: Simulation-time analog operators with *internal state* (LRM
  §4.5, "Analog operators") — `ddt(x)` is the time derivative of `x`, `idt(x)` its time integral;
  both require every-iteration re-evaluation to keep that state correct (LRM §4.5.15: "It is
  important to ensure that all analog operators are evaluated every iteration of a simulation to
  ensure that the internal state is maintained").
- **Declaration and Assignment**: Called as `ddt(expr)`/`idt(expr)`, one argument.
- **Expressions and Evaluation**: Lowered to `Expr::Call(Builtin::Ddt/Idt, [expr])`; `va-core`'s
  DC solve treats `ddt` specially via the IR's separate charge channel
  (`ModelInstance::charge`/`dcharge`, `StampSink`), since a DC operating point has no time
  derivative to actually compute — `va-transient` is what gives `ddt`/`idt` their real dynamic
  meaning. Per LRM §4.5.15, these are the operators an ordinary `while`/`repeat`/non-genvar
  `for` may **not** contain — the genvar-`for` unrolling in this project exists specifically so
  that restriction doesn't need special-case enforcement (there's no loop left by the time
  `ddt`/`idt` are lowered).
- **Structural and Analog Usage**: Analog-block only — never legal in a parameter default,
  genvar loop header, or `analog function` body (LRM: analog operators "can only be used inside
  an analog block").
- **Comparison with Traditional Constructs**: No C/digital-Verilog equivalent whatsoever — a
  derivative/integral-with-memory operator is intrinsic to continuous-time analog simulation and
  has no discrete-time or general-purpose-language analogue.

### `Vt` / `Temperature`

- **Purpose and Static Nature**: Simulation-time environment queries, but only reachable through
  the `$`-prefixed `SysFunc` token (`$vt`, `$vt(T)`, `$temperature`) — the *bare* reserved words
  `vt`/`temperature` (no `$`) have no grammar production consuming them at all. They are
  reserved purely to keep the name available/consistent with the `$`-form and to match the
  LRM's Annex B table; a bare `vt`/`temperature` in source is simply a reserved-word-in-expression
  parse error today.
- **Declaration and Assignment**: N/A for the bare form (see `SysFunc` in §1.1 for the real
  grammar).
- **Expressions and Evaluation**: N/A for the bare form.
- **Structural and Analog Usage**: N/A for the bare form.
- **Comparison with Traditional Constructs**: The bare-word reservation without a corresponding
  grammar rule is analogous to a C compiler reserving `__reserved_for_future_use` — present to
  prevent a name collision, not because anything currently consumes it.

### `Analysis`

- **Purpose and Static Nature**: Folded to a compile-time constant under v0's DC-only model —
  genuinely dynamic in full Verilog-AMS (querying which analysis is currently running), but
  since v0 only ever runs a DC solve, `analysis("static"/"dc"/"ic"/"nodeset")` is always `1.0`
  and every other phase name is always `0.0`.
- **Declaration and Assignment**: Called as `analysis("phase"[, "phase", ...])`; each argument
  must be a string literal (`analysis_matches` rejects a non-string argument).
- **Expressions and Evaluation**: Evaluated once, at elaboration, to a fixed `Expr::Const` —
  not re-evaluated per iteration, unlike a genuinely dynamic analog expression, even though its
  *result* still participates in ordinary dynamic expressions around it.
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: Closest to a C preprocessor `#ifdef
  SIMULATING_DC`-style compile-time flag, except it's evaluated by the elaborator rather than a
  textual preprocessor, and the same IR is (per the stated limitation) not reusable across
  analyses — a fresh elaboration would be needed per analysis type in a hypothetical future
  where v0 supports more than DC.

### `White_noise` / `Flicker_noise` / `Noise_table` / `Ac_stim`

- **Purpose and Static Nature**: Simulation-time in full Verilog-AMS — the three noise sources
  feed a noise-analysis PSD computation, and `ac_stim` contributes a stimulus only during AC
  analysis. v0 has neither noise analysis nor AC analysis (`va-acnoise` is the stretch-goal crate
  for both), so all four fold to a compile-time `0.0`: correct, not just convenient, since none
  of them has any effect on a DC operating point regardless.
- **Declaration and Assignment**: Called as `white_noise(pwr[, "name"])` /
  `flicker_noise(pwr, exp[, "name"])` / `noise_table(...)` / `ac_stim([mag[, phase[, type]]])`.
- **Expressions and Evaluation**: Elaborated to `Expr::Const(0.0)` unconditionally — their
  string label and numeric arguments are parsed but never evaluated.
- **Structural and Analog Usage**: Analog-block only (they appear on the right-hand side of a
  `<+` contribution, contributing zero under v0's DC-only model).
- **Comparison with Traditional Constructs**: No general-purpose-language analogue — a
  stochastic-process source or a frequency-domain stimulus is intrinsic to circuit noise/AC
  analysis.

### `Transition` / `Slew`

- **Purpose and Static Nature**: Genuinely time-domain analog operators in full Verilog-AMS —
  `transition` smooths a stepped/discontinuous `value` with finite delay/rise/fall times,
  `slew` rate-limits `value`'s rate of change; both require tracking *when*/*how fast* `value`
  last changed. v0 is DC-only (no time axis to delay/slew through), and both filters settle to
  their input in steady state (there is no rate-of-change or delay history at a fixed operating
  point), so both fold transparently to their `value` argument at elaboration
  (`Elaborator::lower_expr`'s dedicated arm, checked before the generic call path). `transition`
  was previously unimplemented: it parsed as an ordinary call but failed at elaboration with
  "unknown function `transition`" — confirmed live by `va-cli check` on
  `external/verilogaLib-master/comparator_dynamic.va`, which now passes the frontend end to end.
  `slew` got the identical fix in the same pass, by inspection rather than by hitting it in the
  corpus.
- **Declaration and Assignment**: Called as `transition(value, delay[, rise_time[,
  fall_time]])` / `slew(value, pos_rate[, neg_rate])` — `value` is required, the rest optional.
- **Expressions and Evaluation**: Only `value` is lowered and returned as-is (the same `ExprId`
  it would have produced if written bare, with no wrapper node at all); the remaining arguments
  are read from the AST only to check `value` is present, never evaluated — an empty argument
  list is a hard error.
- **Structural and Analog Usage**: Analog-block only, typically wrapping a `<+` contribution's
  right-hand side or an intermediate variable assignment.
- **Comparison with Traditional Constructs**: No C/digital-Verilog analogue (a continuous-time
  slew/delay filter needs a time axis neither has). Once `va-transient` exists, both would need
  real handling there (they aren't just constant-folded away in a time-stepping solve) — this
  DC-only fold is a stated, deliberate simplification, not a permanent design decision.

### `Bound_step`

- **Purpose and Static Nature**: A transient-timestep hint in full Verilog-AMS (requests the
  simulator not step past a given interval, so it can resolve a known-fast event); has no
  meaning at all under v0's DC-only model (there is no timestep to bound), so it's a documented
  no-op rather than an error.
- **Declaration and Assignment**: Used as a bare statement, `bound_step(step);` — like a
  system-task call, not a value (`parse_stmt`'s dedicated `"bound_step"` arm parses it the same
  way `$strobe(...)` is parsed, producing a `Stmt::Task`, which already elaborates to a no-op).
  If it somehow appears in expression position instead, it also folds to `Expr::Const(0.0)`
  (grouped with the noise-source builtins in `lower_expr`) rather than erroring.
- **Expressions and Evaluation**: The step argument is parsed but never evaluated.
- **Structural and Analog Usage**: Analog-block only, transient-specific.
- **Comparison with Traditional Constructs**: No general-purpose analogue — closest is a
  scheduler hint (e.g. a cooperative-multitasking `yield`), except this one bounds a numerical
  integrator's step size rather than yielding control.

### `Discipline` / `Nature` / `Enddiscipline` / `Endnature`

- **Purpose and Static Nature**: Recognized-and-discarded, not modeled at all. v0 hardcodes
  exactly the `electrical`/`thermal` disciplines as built-ins (see the `Electrical`/`Thermal`
  entry above); a user `discipline...enddiscipline`/`nature...endnature` block — the kind an
  expanded `disciplines.vams`/`constants.vams` include produces — is skipped wholesale
  (`skip_preamble`/`skip_block_until`) before the `module` keyword is even reached.
- **Declaration and Assignment**: `discipline name ... enddiscipline` / `nature name ...
  endnature` (LRM §4, defining a discipline's potential/flow natures and a nature's `abstol`/
  `units`/`access`/`ddt_nature`/`idt_nature` attributes) — parsed only as an opaque token span
  to be discarded, never as individual attribute declarations.
- **Expressions and Evaluation**: N/A — the block's *contents* (every reserved word that would
  only ever appear inside one, e.g. `abstol`, `access`, `units`, `potential`, `flow`,
  `ddt_nature`, `idt_nature`, `domain`) are consumed blindly by `skip_block_until`'s
  token-counting loop and never individually inspected.
- **Structural and Analog Usage**: Would be module-preamble-level if modeled; today only ever
  appears (and is only ever skipped) before the `module` keyword.
- **Comparison with Traditional Constructs**: A discipline/nature pair is the closest thing this
  language has to a C `struct`/units-of-measure system (binding a physical unit and tolerance to
  a signal type) — no C construct maps onto it directly.

## 1.6 Master table — every reserved word

Every one of the 172 words in `RESERVED_WORDS`, alphabetically, each addressed against all five
questions. Words with a full write-up above are cross-referenced rather than repeated; the
remaining ~110 words — almost entirely digital-Verilog gate primitives, net-strength/charge
keywords, specify-block/task/event keywords, and signal-processing transform names — get their
first (and, for the ~90 with zero implemented behavior, only) treatment here.

| Token | Purpose & Static Nature | Declaration & Assignment | Expressions & Evaluation | Structural & Analog Usage | Comparison with Traditional Constructs |
|---|---|---|---|---|---|
| `abs` | Dynamic/static dual, see §1.5 Math builtins | `abs(x)` call | Absolute value, both paths | Analog expr / const context | C `fabs()`/`abs()` |
| `abstol` | N/A — only ever inside a skipped `nature` block (§1.5 `Nature`) | Nature attribute `abstol = expr;` | Never individually inspected | N/A (module preamble) | A nature's absolute-tolerance attribute; no C analogue |
| `access` | Same as `abstol` | Nature attribute `access = fn_name;` | Never individually inspected | N/A (module preamble) | Names the `V`/`I`-style access function for a custom nature; no C analogue |
| `acos` | Dynamic/static dual, §1.5 | `acos(x)` call | Inverse cosine | Analog expr / const context | C `acos()` |
| `acosh` | Dynamic/static dual, §1.5 | `acosh(x)` call | Inverse hyperbolic cosine | Analog expr / const context | C99 `acosh()` |
| `ac_stim` | Folds to constant `0.0` (fixed — see §1.5); contributes nothing at DC regardless | `ac_stim(mag[, phase[, type]])` call | Const-folded to `0.0` | Analog-block only (AC analysis) | No analogue — AC-analysis is out of v0's DC-only scope (`CLAUDE.md` §1's "stretch") |
| `aliasparam` | Elaboration-only, see §1.5 `Aliasparam` | `aliasparam name = target;` | Name resolution only | Module-level | C reference/alias |
| `always` | Reserved, no grammar production — v0 has only the single `analog` block, no digital `always` | N/A | N/A | N/A | Digital Verilog's continuously-re-triggered procedural block; no direct C equivalent |
| `analog` | Dedicated token, §1.4 | — | — | — | — |
| `analysis` | Folds to DC constant, §1.5 | `analysis("phase",...)` call | Const-folded once | Analog-block only | Preprocessor-flag-like compile-time query |
| `and` | Reserved, no grammar production (digital gate primitive: `and #delay g(out,a,b);`) | N/A in v0 | N/A | Structural-only, digital gate level; never analog | Verilog `and` gate; loosely C's `&&`/`&`, but with real gate timing that has no C analogue |
| `asin` | Dynamic/static dual, §1.5 | `asin(x)` call | Inverse sine | Analog expr / const context | C `asin()` |
| `asinh` | Dynamic/static dual, §1.5 | `asinh(x)` call | Inverse hyperbolic sine | Analog expr / const context | C99 `asinh()` |
| `assign` | Reserved, no grammar production — this is Verilog's *procedural continuous assignment* statement keyword, distinct from the `=` operator (`Token::Assign`) | `assign net = expr;` (digital continuous assignment) | N/A in v0 | Digital/structural; not modeled | No C analogue (continuous, event-driven re-evaluation of a net) |
| `atan` | Dynamic/static dual, §1.5 | `atan(x)` call | Arctangent | Analog expr / const context | C `atan()` |
| `atan2` | Dynamic/static dual, §1.5 | `atan2(y, x)` call | Two-argument arctangent | Analog expr / const context | C `atan2()` |
| `atanh` | Dynamic/static dual, §1.5 | `atanh(x)` call | Inverse hyperbolic tangent | Analog expr / const context | C99 `atanh()` |
| `begin` | Dedicated token, §1.4 | — | — | — | — |
| `bound_step` | A documented no-op (fixed — see §1.5); has no meaning under v0's DC-only model | `bound_step(step);` as a bare statement (parses like a system-task call) | Step argument parsed, never evaluated | Analog-block only, transient-specific | No analogue — timestep control is a transient-analysis concept `va-transient` doesn't expose to source yet |
| `branch` | Elaboration-only, see §1.5 `Branch` | `branch (a[,b]) name,...;` | Name/pair resolution | Module-level declaration; analog-block use | Named alias for a recurring access-function pair |
| `buf` | Reserved, no grammar production (digital buffer gate primitive) | N/A | N/A | Digital gate level only | No C analogue (has real propagation delay) |
| `bufif0` | Reserved, no grammar production (tristate buffer, active-low enable) | N/A | N/A | Digital gate level only | No C analogue |
| `bufif1` | Reserved, no grammar production (tristate buffer, active-high enable) | N/A | N/A | Digital gate level only | No C analogue |
| `case` | Simulation-time control flow, §1.5 | `case (sel) labels: body ... endcase` | Dynamic selector/labels | Analog-block only | C `switch` (no fallthrough semantics carried over — each arm is its own body) |
| `casex` | **Not part of Verilog-A at all** (LRM Annex C.7: "casex and casez are not supported in Verilog-A") — reserved, no grammar production, correctly so | N/A | N/A | N/A | Digital Verilog's don't-care-match `switch`; no C analogue |
| `ceil` | Dynamic/static dual, §1.5 Math builtins (newly reserved — see §1.7) | `ceil(x)` call | Round toward +∞ | Analog expr / const context | C `ceil()` |
| `casez` | Same as `casex` — explicitly excluded from Verilog-A by the LRM itself | N/A | N/A | N/A | Same as `casex` |
| `cmos` | Reserved, no grammar production (CMOS transmission-gate switch primitive) | N/A | N/A | Digital/switch-level only | No C analogue |
| `cos` | Dynamic/static dual, §1.5 | `cos(x)` call | Cosine | Analog expr / const context | C `cos()` |
| `cosh` | Dynamic/static dual, §1.5 | `cosh(x)` call | Hyperbolic cosine | Analog expr / const context | C `cosh()` |
| `cross` | Parses as a call (`cross(expr, dir[, ...])`) if written bare; its one real usage, `@(cross(...))`, is discarded wholesale by v0's `skip_balanced_parens` before it's ever parsed as an expression | Zero-crossing event detector (LRM §5.7ish) | Neither path currently evaluates its arguments | Analog-block only (event control) | No C analogue (continuous zero-crossing detection needs the solver's own state) |
| `ddt` | Analog operator with internal state, §1.5 `Ddt`/`Idt` | `ddt(expr)` | Time derivative | Analog-block only | No C/digital-Verilog analogue |
| `ddt_nature` | Same as `abstol` (nature attribute, skipped) | Nature attribute `ddt_nature = other_nature;` | Never individually inspected | N/A (module preamble) | Binds a nature to its time-derivative counterpart; no C analogue |
| `deassign` | Reserved, no grammar production (digital procedural-continuous-assignment release) | N/A | N/A | Digital only | No C analogue |
| `default` | Case-arm keyword, §1.5 | `default[:] body` inside `case...endcase` | Dynamic body | Analog-block only | C `switch`'s `default:` |
| `defparam` | Reserved, no grammar production (digital hierarchical parameter override) | N/A | N/A | Structural/hierarchy only; this project has no instantiation hierarchy to override into | No C analogue |
| `delay` | Reserved, no grammar production (specify-block path delay) | N/A | N/A | Specify-block (timing-check) only | No C analogue |
| `disable` | Reserved, no grammar production (digital named-block/task abort) | N/A | N/A | Digital procedural only | Loosely C's `goto`-out-of-block, but scoped to a named block/task |
| `discipline` | Recognized-and-discarded, §1.5 | `discipline name ... enddiscipline` | N/A | Module preamble | Closest to a C `struct`/unit-of-measure definition |
| `discontinuity` | Reserved, no grammar production (`discontinuity(order);` hints the solver about a non-smooth point) | N/A | N/A | Would be analog-block only | No C analogue (a numerical-solver hint) |
| `edge` | Parses as a call (`edge(expr)`) if written bare; realistically only ever appears inside a discarded `@(...)` | Digital-style edge-detection function | Rejected at elaboration if reached | Analog-block only (event control) | Closest to a rising/falling-edge interrupt trigger; no C analogue |
| `electrical` | Dedicated token, §1.4 | — | — | — | — |
| `else` | Dedicated token, §1.4 | — | — | — | — |
| `end` | Dedicated token, §1.4 | — | — | — | — |
| `endcase` | Case-block terminator, §1.5 | Closes `case...endcase` | N/A | Analog-block only | C `switch`'s closing `}` |
| `enddiscipline` | Recognized-and-discarded, §1.5 | Closes `discipline...enddiscipline` | N/A | Module preamble | — |
| `endfunction` | Function-definition terminator, §1.5 | Closes `analog function...endfunction` | N/A | Module-level | C function's closing `}` |
| `endgenerate` | Syntactic bracket only, §1.5 `Generate`/`Endgenerate` | Closes `generate...endgenerate` | N/A | Analog-block only | No C analogue |
| `endmodule` | Dedicated token, §1.4 | — | — | — | — |
| `endnature` | Recognized-and-discarded, §1.5 | Closes `nature...endnature` | N/A | Module preamble | — |
| `endprimitive` | Reserved, no grammar production (closes a UDP — user-defined gate primitive — definition) | N/A | N/A | Digital structural only | No C analogue |
| `endspecify` | Reserved, no grammar production (closes a `specify...endspecify` timing block) | N/A | N/A | Digital timing-check only | No C analogue |
| `endtable` | Reserved, no grammar production (closes a UDP truth-`table...endtable`) | N/A | N/A | Digital structural only | Closest to a C `switch`/lookup-table, but declarative and gate-level |
| `endtask` | Reserved, no grammar production (closes a digital `task...endtask`) | N/A | N/A | Digital procedural only | C function's closing `}`, minus analog-function's purity/no-recursion rules |
| `event` | Reserved, no grammar production (declares a named digital event variable, `event e;`, triggered with `->e;`) | N/A | N/A | Digital procedural only | No C analogue |
| `exclude` | Range-clause keyword, §1.4 | `exclude value` / `exclude (lo:hi)` | Const-evaluated then discarded | Module-level (parameter ranges) | No C analogue (closest: a validated-range precondition, minus the "hole" it punches out) |
| `exp` | Dynamic/static dual, §1.5 | `exp(x)` call | Exponential | Analog expr / const context | C `exp()` |
| `final_step` | Reserved, no grammar production as a bare word outside `@()`; realistically only appears inside the discarded `@(final_step)` | Global analog event: fires once at analysis end | N/A | Analog-block only (event control) | No C analogue (closest: an `atexit()` hook) |
| `flicker_noise` | Folds to constant `0.0`, §1.5 | `flicker_noise(pwr, exp[, "name"])` call | Const-folded to `0.0` | Analog-block only | No general-purpose analogue |
| `floor` | Dynamic/static dual, §1.5 Math builtins (newly reserved — see §1.7) | `floor(x)` call | Round toward −∞ | Analog expr / const context | C `floor()` |
| `flow` | Same as `abstol` (nature attribute, skipped) | Nature attribute naming the flow quantity | Never individually inspected | N/A (module preamble) | Names the conserved "current-like" quantity of a discipline; no C analogue |
| `for` | Simulation-time (or elaboration-time when genvar-driven), §1.5/Part 2 §2.14 | `for (init; cond; step) body` | Dynamic, or const-evaluated if genvar-driven | Analog-block only | C `for` — with the added genvar-unrolling mode C has no concept of |
| `force` | Reserved, no grammar production (digital procedural force-a-net) | N/A | N/A | Digital procedural only | No C analogue |
| `forever` | Reserved, no grammar production (digital unconditional loop) | N/A | N/A | Digital procedural only | C's `for(;;)`/`while(1)` |
| `fork` | Reserved, no grammar production (digital concurrent-process block, paired with `join`) | N/A | N/A | Digital procedural only | Loosely POSIX threads' fork, but cooperative/simulation-scheduled |
| `from` | Range-clause keyword, §1.4 | `from [lo:hi]`/`(lo:hi)` | Const-evaluated bounds | Module-level (parameter ranges) | No C analogue |
| `function` | Function-definition keyword, §1.5 | `analog function ...` | — | Module-level | C `static` pure function |
| `generate` | Syntactic bracket only, §1.5 | `generate ... endgenerate` | N/A | Analog-block only | No C analogue |
| `genvar` | **Elaboration-only construct**, dedicated token, full treatment in §1.4 | — | — | — | — |
| `ground` | Dedicated token (currently unimplemented as a declaration), §1.4 | — | — | — | — |
| `highz0` | Reserved, no grammar production (net strength: high-impedance driving 0) | N/A | N/A | Digital net-strength only | No C analogue |
| `highz1` | Reserved, no grammar production (net strength: high-impedance driving 1) | N/A | N/A | Digital net-strength only | No C analogue |
| `hypot` | Dynamic/static dual, §1.5 | `hypot(x, y)` call | `sqrt(x²+y²)` | Analog expr / const context | C99 `hypot()` |
| `idt` | Analog operator with internal state, §1.5 | `idt(expr)` | Time integral | Analog-block only | No C/digital-Verilog analogue |
| `idt_nature` | Same as `ddt_nature` (nature attribute, skipped) | Nature attribute `idt_nature = other_nature;` | Never individually inspected | N/A (module preamble) | No C analogue |
| `if` | Dedicated token, §1.4 | — | — | — | — |
| `ifnone` | Reserved, no grammar production (specify-block conditional-path fallback) | N/A | N/A | Specify-block (timing-check) only | No C analogue |
| `inf` | Dedicated token, §1.4 | — | — | — | — |
| `initial` | Reserved, no grammar production (digital one-shot-at-time-0 procedural block) | N/A | N/A | Digital procedural only | Closest to running code once before `main()`, e.g. a static initializer |
| `initial_step` | Reserved, no grammar production as a bare word outside `@()`; realistically only appears inside the discarded `@(initial_step)` | Global analog event: fires once at analysis start | N/A | Analog-block only (event control) | No C analogue (closest: a one-time setup routine) |
| `inout` | Dedicated token, §1.4 | — | — | — | — |
| `input` | Dedicated token, §1.4 | — | — | — | — |
| `int` | Dynamic/static dual, §1.5 Math builtins (newly reserved — see §1.7) | `int(x)` call | Truncate toward zero | Analog expr / const context | C's `(int)` cast, but as a genuine callable function |
| `integer` | Dedicated token, §1.4 | — | — | — | — |
| `join` | Reserved, no grammar production (closes a digital `fork...join` block) | N/A | N/A | Digital procedural only | No C analogue |
| `laplace_nd` | Parses as a call (`laplace_nd(in, num[, den])`); elaboration has no builtin → `unknown function` | Laplace-domain transfer-function filter, numerator/denominator coefficient form | Rejected at elaboration today | Analog-block only, signal-flow filter | No C analogue (a continuous-time transfer function) |
| `laplace_np` | Same family as `laplace_nd`, pole/zero form | Laplace-domain filter, pole/zero form | Rejected at elaboration today | Analog-block only | No C analogue |
| `laplace_zd` | Same family, Z-domain numerator/denominator form | Z-domain (discrete) filter | Rejected at elaboration today | Analog-block only | Closest: a digital IIR filter's difference equation, but expressed declaratively |
| `laplace_zp` | Same family, Z-domain pole/zero form | Z-domain (discrete) filter | Rejected at elaboration today | Analog-block only | Same as `laplace_zd` |
| `large` | Reserved, no grammar production (net-strength charge-storage keyword, `trireg`-adjacent) | N/A | N/A | Digital net-strength only | No C analogue |
| `last_crossing` | Parses as a call (`last_crossing(expr, dir)`); elaboration has no builtin → `unknown function` | Returns the simulation time of the last zero-crossing of `expr` | Rejected at elaboration today | Analog-block only | No C analogue |
| `limexp` | Dynamic/static dual, §1.5 Math builtins (newly reserved — see §1.7); folds to plain `exp` | `limexp(x)` call | Exponential (no limiting modeled) | Analog expr / const context | A numerically-limited `exp` Newton-convergence aid; no C analogue |
| `ln` | Dynamic/static dual, §1.5 | `ln(x)` call | Natural log | Analog expr / const context | C `log()` (note the naming swap vs. `log`/`log10` below) |
| `localparam` | Dedicated token, §1.4 | — | — | — | — |
| `log` | Dynamic/static dual, §1.5 | `log(x)` call | Base-10 log | Analog expr / const context | C `log10()` |
| `macromodule` | Reserved, no grammar production (a `module` synonym some tools use for the top-level design unit) | N/A | N/A | Structural, same role as `module` | No C analogue |
| `max` | Dynamic/static dual, §1.5 | `max(x, y)` call | Maximum | Analog expr / const context | C's `fmax()`/a `max` macro |
| `medium` | Reserved, no grammar production (net-strength charge-storage keyword) | N/A | N/A | Digital net-strength only | No C analogue |
| `min` | Dynamic/static dual, §1.5 | `min(x, y)` call | Minimum | Analog expr / const context | C's `fmin()`/a `min` macro |
| `module` | Dedicated token, §1.4 | — | — | — | — |
| `nand` | Reserved, no grammar production (digital gate primitive) | N/A | N/A | Digital gate level only | Loosely C's `!(a && b)`, minus gate timing |
| `nature` | Recognized-and-discarded, §1.5 | `nature name ... endnature` | N/A | Module preamble | Closest to a C units-of-measure/tolerance struct |
| `negedge` | Reserved, no grammar production as a bare word outside `@()`; would appear as `@(negedge sig)`, itself discarded wholesale | Digital falling-edge event trigger | N/A | Digital event control only | No C analogue |
| `nmos` | Reserved, no grammar production (NMOS switch primitive) | N/A | N/A | Digital/switch-level only | No C analogue |
| `noise_table` | Folds to constant `0.0`, §1.5 | `noise_table(table_or_array[, "name"])` call | Const-folded to `0.0` | Analog-block only | No general-purpose analogue |
| `nor` | Reserved, no grammar production (digital gate primitive) | N/A | N/A | Digital gate level only | Loosely C's `!(a \|\| b)`, minus gate timing |
| `not` | Reserved, no grammar production (digital *inverter gate* primitive — distinct from the `!` operator, `Token::Not`) | N/A | N/A | Digital gate level only | Loosely C's `!x` as a value, but as a timed gate instance instead of an operator |
| `notif0` | Reserved, no grammar production (tristate inverter, active-low enable) | N/A | N/A | Digital gate level only | No C analogue |
| `notif1` | Reserved, no grammar production (tristate inverter, active-high enable) | N/A | N/A | Digital gate level only | No C analogue |
| `or` | Reserved, no grammar production (digital gate primitive — distinct from `\|\|`, `Token::OrOr`) | N/A | N/A | Digital gate level only | Loosely C's `\|\|` as a gate instance |
| `output` | Dedicated token, §1.4 | — | — | — | — |
| `parameter` | Dedicated token, §1.4 | — | — | — | — |
| `pmos` | Reserved, no grammar production (PMOS switch primitive) | N/A | N/A | Digital/switch-level only | No C analogue |
| `posedge` | Reserved, no grammar production as a bare word outside `@()`; would appear as `@(posedge sig)`, itself discarded wholesale | Digital rising-edge event trigger | N/A | Digital event control only | No C analogue |
| `potential` | Same as `abstol` (nature attribute, skipped) | Nature attribute naming the potential quantity | Never individually inspected | N/A (module preamble) | Names the conserved "voltage-like" quantity of a discipline; no C analogue |
| `pow` | Dynamic/static dual, §1.5 | `pow(x, y)` call | Power | Analog expr / const context | C `pow()` |
| `primitive` | Reserved, no grammar production (opens a UDP definition, paired with `endprimitive`) | N/A | N/A | Digital structural only | No C analogue |
| `pull0` | Reserved, no grammar production (net strength: resistive pull to 0) | N/A | N/A | Digital net-strength only | No C analogue |
| `pull1` | Reserved, no grammar production (net strength: resistive pull to 1) | N/A | N/A | Digital net-strength only | No C analogue |
| `pulldown` | Reserved, no grammar production (pull-down gate primitive) | N/A | N/A | Digital gate/net level only | No C analogue |
| `pullup` | Reserved, no grammar production (pull-up gate primitive) | N/A | N/A | Digital gate/net level only | No C analogue |
| `rcmos` | Reserved, no grammar production (resistive CMOS transmission-gate switch) | N/A | N/A | Digital/switch-level only | No C analogue |
| `real` | Dedicated token, §1.4 | — | — | — | — |
| `realtime` | Reserved, no grammar production (digital wall-clock-style time variable type) | N/A | N/A | Digital procedural only | Closest to C's `time_t`/`double` timestamp, without simulation-time semantics |
| `reg` | Reserved, no grammar production (digital storage-net type) | N/A | N/A | Digital structural only | Closest to a C variable with implicit "last written value persists" semantics |
| `release` | Reserved, no grammar production (undoes a `force`) | N/A | N/A | Digital procedural only | No C analogue |
| `repeat` | Simulation-time control flow, §1.5 | `repeat (count) body` | Dynamic count | Analog-block only | `for (int i=0;i<n;i++)` minus the explicit loop variable |
| `rnmos` | Reserved, no grammar production (resistive NMOS switch) | N/A | N/A | Digital/switch-level only | No C analogue |
| `round` | Dynamic/static dual, §1.5 Math builtins (newly reserved — see §1.7) | `round(x)` call | Round to nearest | Analog expr / const context | C99 `round()` |
| `rpmos` | Reserved, no grammar production (resistive PMOS switch) | N/A | N/A | Digital/switch-level only | No C analogue |
| `rtran` | Reserved, no grammar production (resistive bidirectional pass switch) | N/A | N/A | Digital/switch-level only | No C analogue |
| `rtranif0` | Reserved, no grammar production (resistive bidirectional pass switch, active-low enable) | N/A | N/A | Digital/switch-level only | No C analogue |
| `rtranif1` | Reserved, no grammar production (resistive bidirectional pass switch, active-high enable) | N/A | N/A | Digital/switch-level only | No C analogue |
| `scalared` | Reserved, no grammar production (net-vector storage-layout hint) | N/A | N/A | Digital structural only | No C analogue |
| `sin` | Dynamic/static dual, §1.5 | `sin(x)` call | Sine | Analog expr / const context | C `sin()` |
| `sinh` | Dynamic/static dual, §1.5 | `sinh(x)` call | Hyperbolic sine | Analog expr / const context | C `sinh()` |
| `slew` | Folds to its `value` argument (fixed — see §1.5 `Transition`/`Slew`); settles to input at DC | `slew(value, pos_rate[, neg_rate])` call | Identity on `value`; rates parsed, never evaluated | Analog-block only | No C analogue |
| `small` | Reserved, no grammar production (net-strength charge-storage keyword) | N/A | N/A | Digital net-strength only | No C analogue |
| `specify` | Reserved, no grammar production (opens a timing-check block, paired with `endspecify`) | N/A | N/A | Digital timing-check only | No C analogue |
| `specparam` | Reserved, no grammar production (a parameter usable only inside a `specify` block) | N/A | N/A | Digital timing-check only | No C analogue |
| `sqrt` | Dynamic/static dual, §1.5 | `sqrt(x)` call | Square root | Analog expr / const context | C `sqrt()` |
| `strong0` | Reserved, no grammar production (net strength: strong drive to 0) | N/A | N/A | Digital net-strength only | No C analogue |
| `strong1` | Reserved, no grammar production (net strength: strong drive to 1) | N/A | N/A | Digital net-strength only | No C analogue |
| `supply0` | Reserved, no grammar production (ground/supply-rail net type) | N/A | N/A | Digital structural only | No C analogue |
| `supply1` | Reserved, no grammar production (power/supply-rail net type) | N/A | N/A | Digital structural only | No C analogue |
| `table` | Reserved, no grammar production (opens a UDP truth-table, paired with `endtable`) | N/A | N/A | Digital structural only | Declarative lookup table; loosely a C `switch`/array lookup |
| `tan` | Dynamic/static dual, §1.5 | `tan(x)` call | Tangent | Analog expr / const context | C `tan()` |
| `tanh` | Dynamic/static dual, §1.5 | `tanh(x)` call | Hyperbolic tangent | Analog expr / const context | C `tanh()` |
| `task` | Reserved, no grammar production (opens a digital `task...endtask` definition) | N/A | N/A | Digital procedural only | Closest to a non-pure C function (may have side effects, consume simulation time) |
| `temperature` | Bare form has no grammar production, §1.5 `Vt`/`Temperature` | — | — | — | — |
| `thermal` | Dedicated token, §1.4 | — | — | — | — |
| `time` | Reserved, no grammar production (digital 64-bit simulation-time variable type) | N/A | N/A | Digital procedural only | Closest to C's `time_t` |
| `timer` | Parses as a call (`timer(start[, period])`) if written bare; realistically only appears inside the discarded `@(timer(...))` | Fires at a specified absolute/periodic simulation time | Rejected at elaboration if reached | Analog-block only (event control) | Closest to a POSIX interval timer/`setitimer` |
| `tran` | Reserved, no grammar production (bidirectional pass-switch primitive) | N/A | N/A | Digital/switch-level only | No C analogue |
| `tranif0` | Reserved, no grammar production (bidirectional pass switch, active-low enable) | N/A | N/A | Digital/switch-level only | No C analogue |
| `tranif1` | Reserved, no grammar production (bidirectional pass switch, active-high enable) | N/A | N/A | Digital/switch-level only | No C analogue |
| `transition` | Folds to its `value` argument (fixed — see §1.5 `Transition`; previously rejected at elaboration, confirmed live at the time by `va-cli check` failing exactly here on `external/verilogaLib-master/comparator_dynamic.va`, which now passes) | `transition(value, delay[, rise[, fall]])` call | Identity on `value`; `delay`/`rise`/`fall` parsed, never evaluated | Analog-block only | No C analogue |
| `tri` | Reserved, no grammar production (default-strength tri-state net type) | N/A | N/A | Digital structural only | No C analogue |
| `tri0` | Reserved, no grammar production (net type: pulls to 0 when undriven) | N/A | N/A | Digital structural only | No C analogue |
| `tri1` | Reserved, no grammar production (net type: pulls to 1 when undriven) | N/A | N/A | Digital structural only | No C analogue |
| `triand` | Reserved, no grammar production (wired-AND tri-state net type) | N/A | N/A | Digital structural only | No C analogue |
| `trior` | Reserved, no grammar production (wired-OR tri-state net type) | N/A | N/A | Digital structural only | No C analogue |
| `trireg` | Reserved, no grammar production (charge-storage net type, paired with `small`/`medium`/`large`) | N/A | N/A | Digital structural only | Closest to a C `static` variable retaining its last value, but modeling analog charge decay |
| `units` | Same as `abstol` (nature attribute, skipped) | Nature attribute `units = "V";` | Never individually inspected | N/A (module preamble) | No C analogue |
| `vectored` | Reserved, no grammar production (net-vector storage-layout hint, pairs with `scalared`) | N/A | N/A | Digital structural only | No C analogue |
| `vt` | Bare form has no grammar production, §1.5 `Vt`/`Temperature` | — | — | — | — |
| `wait` | Reserved, no grammar production (digital procedural block-until-condition) | N/A | N/A | Digital procedural only | Closest to a condition-variable `wait()`, but simulation-scheduled |
| `wand` | Reserved, no grammar production (wired-AND net type) | N/A | N/A | Digital structural only | No C analogue |
| `weak0` | Reserved, no grammar production (net strength: weak drive to 0) | N/A | N/A | Digital net-strength only | No C analogue |
| `weak1` | Reserved, no grammar production (net strength: weak drive to 1) | N/A | N/A | Digital net-strength only | No C analogue |
| `while` | Simulation-time control flow, §1.5 | `while (cond) body` | Dynamic condition | Analog-block only | C `while` |
| `white_noise` | Folds to constant `0.0`, §1.5 | `white_noise(pwr[, "name"])` call | Const-folded to `0.0` | Analog-block only | No general-purpose analogue |
| `wire` | Reserved, no grammar production (default digital net type) | N/A | N/A | Digital structural only | Closest to a C wire/signal — this project always requires an explicit `electrical`/`thermal` discipline instead |
| `wor` | Reserved, no grammar production (wired-OR net type) | N/A | N/A | Digital structural only | No C analogue |
| `xnor` | Reserved, no grammar production (digital gate primitive) | N/A | N/A | Digital gate level only | Loosely C's `!(a ^ b)`, minus gate timing |
| `xor` | Reserved, no grammar production (digital gate primitive — distinct from any bitwise operator, which this subset doesn't implement at all) | N/A | N/A | Digital gate level only | Loosely C's `^`, but as a timed gate instance |
| `zi_nd` | Parses as a call (`zi_nd(in, num, den[, ...])`); elaboration has no builtin → `unknown function` | Z-domain (discrete) IIR filter, numerator/denominator form | Rejected at elaboration today | Analog-block only, signal-flow filter | Closest: a digital IIR filter's difference equation, expressed declaratively |
| `zi_np` | Same family, pole/zero form | Z-domain IIR filter, pole/zero form | Rejected at elaboration today | Analog-block only | Same as `zi_nd` |
| `zi_zd` | Same family as `laplace_zd`/`zi_nd`, Z-domain-input numerator/denominator form | Z-domain IIR filter variant | Rejected at elaboration today | Analog-block only | Same as `zi_nd` |
| `zi_zp` | Same family, Z-domain-input pole/zero form | Z-domain IIR filter variant | Rejected at elaboration today | Analog-block only | Same as `zi_nd` |

## 1.7 `floor`/`ceil`/`round`/`int`/`limexp` — formerly non-reserved (fixed)

`floor`, `ceil`, `round`, `int`, and `limexp` are real, working call-expression builtins
(`call_builtin` maps them to `Builtin::Floor/Ceil/Round/Int`, and `limexp` to `Builtin::Exp` —
`limexp` is documented as a numerically-limited exponential used as a Newton-convergence aid,
whose *value* and *derivative* this project models as plain `exp`). Until this pass, none of the
five was in `RESERVED_WORDS`, even though every other math builtin here (`exp`, `sqrt`, `ddt`,
…) is reserved — a user could declare `real floor;` and shadow the name. All five are now
reserved words with a dedicated `#[token(..., kw)]` entry in the lexer, folded into the "Math
builtins" deep dive in §1.5 (and the master table in §1.6) rather than treated separately here,
since their behavior is now identical in kind to every other math builtin: reserved, callable as
`name(args)`, differentiated dynamically or const-folded statically by the same
`call_builtin`/`eval_const_call` tables. The one remaining asymmetry with C's `<math.h>` (whose
functions are never keywords) is a Verilog-A-wide convention this project follows, not a gap.

---

# Part 2 — Parser constructs

These are grammar productions built from more than one token (or from a token whose behavior
depends on surrounding context), organized by what they do rather than by a single keyword.

## 2.1 Module declaration & port list

- **Purpose and Static Nature**: Purely structural; parsed once, produces the top-level
  `ModuleAst`.
- **Declaration and Assignment**: `module name ( port_name, ... ) ; items... endmodule`
  (`parse_module`). Ports are bare names here — direction/discipline are separate declarations
  elsewhere in the item list, matched by name at elaboration (`resolve_ports`).
- **Expressions and Evaluation**: N/A.
- **Structural and Analog Usage**: The entire module-level scope, containing every other item
  and exactly one `analog` block.
- **Comparison with Traditional Constructs**: A digital-Verilog `module`, minus instantiation of
  other modules.

## 2.2 Net/discipline declaration (with optional vector range)

- **Purpose and Static Nature**: Elaboration-only — resolves a name (or, for a vector net, a
  contiguous run of names) to one or more fixed `NodeId`s.
- **Declaration and Assignment**: `electrical|thermal [ msb : lsb ] name [ [ msb : lsb ] ], name
  [ [ msb : lsb ] ], ... ;` — real Verilog-A uses two different spellings for a vector net's
  width, and this grammar accepts both: a shared **prefix** range before the whole name list
  (`electrical [0:width-1] in;`, the LRM's own DAC example) and a per-name **suffix** range
  (`` electrical in[`W-1:0], out; ``, seen directly in the `verilogaLib` corpus). Each name may
  carry its own suffix range, which overrides the prefix default for that name only — so
  `electrical [3:0] a, b[7:0], c;` declares `a`/`c` as 4-bit vectors and `b` as an 8-bit one.
  When a range (either form) is present, `msb`/`lsb` are const-evaluated (`const_eval_int`) and
  the name gets one interned node per index in `min(msb,lsb) ..= max(msb,lsb)`, named
  `"base[k]"` internally.
- **Expressions and Evaluation**: The range bounds may reference an already-declared parameter
  (the DAC example again: `input [0:width-1] in;`) — which is why `Elaborator::run`
  const-evaluates parameters *before* collecting nodes.
- **Structural and Analog Usage**: Module-level only; the resulting node(s) are read from the
  analog block through §2.17's access-function grammar.
- **Comparison with Traditional Constructs**: The vector form is the Verilog-A analogue of a C
  array declaration (`double bus[4];`), except the "array" here is a bus of physical nodes, not
  storage. The mixed prefix/suffix-with-override grammar has no clean C parallel (closest: a C
  declaration list where each declarator can independently be a pointer/array, `int *a, b[4];`).

## 2.3 Direction declaration

- A port's direction (`input`/`output`/`inout`) may repeat a vector net's `[msb:lsb]` width at
  the direction-declaration site too (again, the LRM's own DAC example: `input [0:width-1] in;`
  alongside the matching `electrical [0:width-1] in;`). This is parsed and discarded — purely
  informational here, since the real range comes from the paired net declaration (§2.2) — so
  real-world vector-port headers now parse instead of failing on the bracket. What still doesn't
  work is the port *itself* being a vector: `va_ir::Module::ports` is one `NodeId` per port, with
  no vector-port representation and no `va-netlist` wiring convention for one, so elaboration
  rejects a vector port with a specific, honest error (`resolve_ports`) rather than silently
  doing the wrong thing — see §2.18's "known gap" note.
- Otherwise covered fully in Part 1 §1.4 (`Input`/`Output`/`Inout`) — the grammar itself is just
  `direction name, ... ;`, with no additional production beyond the token dispatch.

## 2.4 Parameter/localparam declaration

- Covered fully in Part 1 §1.4 (`Parameter`/`LocalParam`); the fuller grammar (optional base
  type, `from`/`exclude` clauses) is described there and in §1.4's `From`/`Exclude`/`Inf` entry.

## 2.5 Genvar declaration

- Covered fully in Part 1 §1.4 (`Genvar`). Grammar: `genvar name, name, ... ;`
  (`Item::Genvar`), module scope only.

## 2.6 Branch declaration

- Covered fully in Part 1 §1.5 (`Branch`). Grammar: `branch ( terminal [, terminal] ) name [,
  name...] ;`, where each `terminal` is now a `NetArg` (§2.18) so a vector element may in
  principle be a branch terminal, though this is only meaningful with a constant (not
  genvar-bound) index at module scope.

## 2.7 Aliasparam declaration

- Covered fully in Part 1 §1.5 (`Aliasparam`). Grammar: `aliasparam name = target ;`, a fixed
  identifier-equals-identifier shape.

## 2.8 Module-level / block-local variable declaration

- Covered fully in Part 1 §1.4 (`Real`/`Integer`). Two grammar sites: `Item::Var` (module
  scope, `real x, y;`) and `Stmt::VarDecl` (block scope, same syntax inside `begin...end`) —
  elaboration treats both identically, registering a `VarId` the first time a name is seen,
  whichever comes first.

## 2.9 Analog function definition

- **Purpose and Static Nature**: The definition itself is elaboration-time (resolved once,
  in source order, to a `Function`/`FuncId`); a *call* to it is simulation-time (its body runs
  with fresh argument bindings each time).
- **Declaration and Assignment**: `analog function [real|integer] name ; [input|output|inout
  name,...;]... [real|integer name,...;]... body-statements... endfunction`. The function name
  doubles as its implicit return variable (assigned inside the body, read by the caller) — a
  Verilog-A-specific convention with no `return` keyword at all.
- **Expressions and Evaluation**: Argument *directions* and the body are retained; argument/
  local *type* declarations (`real x;`) are parsed and discarded (v0 has no per-variable type
  distinction, as noted in §1.4). A call (`CallUser(FuncId, args)`) binds `args` positionally to
  the function's own private variable scope — it may read module parameters but not module
  analog variables, and a forward reference to a function defined later in the same file
  resolves as unknown (no forward-reference support, consistent with `aliasparam`/parameter
  handling).
- **Structural and Analog Usage**: Declared at module scope (an `Item`, sitting alongside
  `Item::Analog`); called only from inside an analog block (or from another already-defined
  function).
- **Comparison with Traditional Constructs**: A C `static` function, but pure (LRM: "Verilog-A
  analog functions are pure and non-recursive") and with the name-doubles-as-return-variable
  convention that has no C parallel (closest: Pascal/Fortran functions, which use the same
  convention).

## 2.10 Contribution statement (`<+`)

- Covered fully in Part 1 §1.2 (`Contribute`).

## 2.11 Procedural assignment (`=`)

- Covered fully in Part 1 §1.2 (`Assign`), including the genvar-restricted-assignment special
  case (cross-referenced from §1.4 `Genvar` and detailed in §2.14 below).

## 2.12 If/else statement

- Covered fully in Part 1 §1.4 (`If`/`Else`).

## 2.13 While / repeat / ordinary for statement

- Covered fully in Part 1 §1.5 (`While`/`Repeat`/`For`/`Case`/`Endcase`/`Default`). The grammar
  for `for` specifically: `for ( assignment ; expr ; assignment ) body` — `init`/`step` are
  parsed via `parse_assignment` (a bare `lhs = rhs`, no terminator), matching the LRM's
  `analog_variable_assignment` production.

## 2.14 Genvar-controlled `for` — elaboration-time unrolling

This is the construct with the most interesting Purpose-and-Static-Nature story in the whole
document, so it gets its own section even though it reuses the exact same `Stmt::For` AST node
as an ordinary loop.

- **Purpose and Static Nature**: Fully elaboration-time. `Elaborator::lower_stmt`'s `Stmt::For`
  arm inspects `init`: if it is `Stmt::Assign { lhs, .. }` and `lhs` was declared with `genvar`,
  the whole loop is diverted to `lower_generate_for`, which never emits a `va_ir::Stmt::For` at
  all — it *executes* the loop during elaboration (bounded at 10,000 iterations, to turn a
  malformed loop into a clear error instead of a hang) and concatenates each iteration's
  already-lowered body into one flat `va_ir::Stmt::Block`. By the time `va-core`/`va-codegen`
  see the IR, the loop is gone — only its unrolled contents remain. This is the direct
  implementation of LRM §3.5's "static nature... derived from the limitations upon the contexts
  in which their values can be assigned," carried to its logical conclusion: since a genvar's
  value can *only* ever be a loop-header-assigned constant, there is nothing left to represent
  at simulation time.
- **Declaration and Assignment**: Same surface grammar as an ordinary `for`
  (`analog_loop_generate_statement ::= for (genvar_initialization; genvar_expression;
  genvar_iteration) analog_statement`, LRM Syntax 5-12) — what makes it a *generate* loop is
  purely that `init` assigns a name previously declared with `genvar`, not any different
  keyword or bracket. `step` is required to reassign that same genvar (`Stmt::Assign { lhs,
  .. } if lhs == genvar`) — anything else is rejected as violating restricted assignment.
- **Expressions and Evaluation**: `init`/`cond`/`step` are evaluated with `const_eval`/
  `const_eval_int` — the same compile-time evaluator used for parameter ranges — so they may
  reference literals, parameters, and other (already-bound, enclosing) genvars, but never a
  probe, a `$vt`, or an ordinary analog variable (LRM: "Assignments to the genvar variable... can
  consist only of expressions of static values"). Inside the loop body, the currently-bound
  genvar reads as `Expr::Const` wherever referenced (`lower_expr`'s `ExprAst::Ident` arm checks
  `genvar_env` before parameters/variables) — this is the mechanism that gives each unrolled
  iteration its own "implicit localparam" value, per LRM §3.6's generate-scope description.
  Analog operators (`ddt`/`idt`) are legal in the body precisely because, by the time they're
  lowered, the loop has already been replaced by straight-line code — there is no special-case
  "allow ddt here" logic at all, which is the whole point of unrolling rather than trying to
  special-case the restriction.
- **Structural and Analog Usage**: Analog-block only; a nested genvar-for reusing an
  already-bound (enclosing) genvar's name is rejected (`genvar_env.contains_key` check) —
  matching LRM §3.6's "nested loop generate constructs cannot use the same genvar identifier"
  rule, since each generate scope's implicit localparam would otherwise collide. Sibling
  (sequential, non-nested) loops may freely reuse a genvar name, since the binding is released
  when its own loop finishes.
- **Comparison with Traditional Constructs**: The nearest general-purpose-language parallel is a
  compile-time-unrolled loop — C++'s `template <int I>` recursion, or a `#pragma unroll` hint —
  where the loop index is baked into the generated code and never exists as a runtime value.
  Digital Verilog/SystemVerilog's `genvar` is more commonly used to instantiate an *array of
  module instances*; this project has no module instantiation, so its genvar support covers only
  the "unroll analog code, indexing a signal vector" half of the LRM's full genvar story (see
  Part 1 §1.4 `Genvar`'s comparison note).

## 2.15 `generate`/`endgenerate` wrapper

- Covered fully in Part 1 §1.5 (`Generate`/`Endgenerate`). Grammar note: `parse_generate`
  collects statements until `endgenerate` and hands them back as a plain `Vec<Stmt>`, which
  `parse_stmt` wraps in `Stmt::Block` — structurally indistinguishable, after parsing, from a
  `begin...end` block containing the same statements. All of §2.14's real behavior triggers off
  the *inner* `for`'s genvar-ness, never off the presence or absence of this wrapper.

## 2.16 Case statement

- Covered fully in Part 1 §1.5 (`While`/`Repeat`/`For`/`Case`/`Endcase`/`Default`).

## 2.17 Access-function calls: `V(...)` / `I(...)`

- **Purpose and Static Nature**: Simulation-time — a probe (`Expr::Probe`, read) or contribution
  target (`Stmt::Contribute`, write) against a specific branch, re-evaluated every solve
  iteration.
- **Declaration and Assignment**: N/A (these are uses, not declarations) — but note `V`/`I` are
  themselves ordinary identifiers, not reserved words (LRM §5.5: nature access-function names
  are the discipline's, not the language's, keywords — `V`/`I` are simply the electrical
  discipline's conventional potential/flow access names). The parser recognizes them
  contextually: `is_access(name)` checks the literal string `"V"`/`"I"` when an `Ident` is
  immediately followed by `(`.
- **Expressions and Evaluation**: `V(a)` / `I(a)` (implicit reference/ground terminal) or
  `V(a, b)` / `I(a, b)` (explicit two-terminal branch) or `V(name)` / `I(name)` where `name` is
  a `branch`-declared alias — all three resolve, via `resolve_branch`/`resolve_net_arg`, to the
  same interned `BranchId` a structurally-equivalent access would produce. Each argument is now
  a `NetArg` (§2.18), so either terminal may be a vector-net element (`V(bus[i], gnd)`).
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: No general-purpose-language analogue for the
  *access-function* concept itself (a name whose meaning — voltage vs. current — is bound to the
  discipline of the nets it's applied to, not to a fixed type signature). The closest structural
  parallel is operator overloading resolved by argument type.

## 2.18 Vector net declaration & indexed access (`bus[i]`)

- **Purpose and Static Nature**: The *declaration* is elaboration-only (interning nodes); an
  *access*'s index must itself be elaboration-time-constant (LRM §5.5.2: "The index must be a
  constant expression, though it may include genvar variables") even though the access itself
  (which node's voltage/current) is a simulation-time read/write.
- **Declaration and Assignment**: `electrical|thermal [msb:lsb] name;` (§2.2) declares the
  vector; `V(name[index_expr])` / `I(name[index_expr])` (or a bare `name[index_expr]` as either
  terminal of a two-terminal access) reads/writes one element. `NetArg { name, index:
  Option<ExprRef> }` is the AST representation shared by both branch-declaration terminals and
  access-function arguments.
- **Expressions and Evaluation**: `index_expr` is evaluated by `const_eval_int` (requiring an
  exactly-integral result, within `1e-9`) and bounds-checked against the vector's declared
  `(lo, hi)` — an out-of-range or non-integral index is a hard elaboration error, not a runtime
  condition. Attempting to access a declared vector net *without* an index, or to index a net
  that was never declared as a vector, are both separately rejected with a specific message
  (`resolve_net_arg`).
- **Structural and Analog Usage**: Declaration is module-level; indexed access is analog-block
  only (typically inside a genvar-driven `for`, per §2.14, though a plain literal index like
  `V(bus[2])` needs no genvar at all).
- **Comparison with Traditional Constructs**: A C array subscript, restricted to a
  compile-time-constant (or genvar-derived) index — closer to a C array indexed only by
  `constexpr`/template-parameter values than to ordinary runtime C indexing.
- **Known gap — vector ports**: a vector *net* works fully as described above; a vector *port*
  (the same net also listed in the module's port list) does not, and gives a specific error
  rather than silently doing the wrong thing (`resolve_ports`: `"port \`{name}\` is a vector
  net; vector ports are not yet supported"`). The blocker is `va_ir::Module::ports: Vec<NodeId>`
  — one node per port, with no representation for "this port is actually N nodes" — and there is
  no `va-netlist` wiring convention for a multi-terminal port connection either. Fixing this
  properly is an Interface α change (§6), not a `va-frontend`-only fix; real corpus files hit
  this directly (`external/verilogaLib-master/dac_16bit_ideal.va`,
  `external/verilogaLib-master/adc_16bit_ideal.va` both declare a vector I/O port).

## 2.19 Event control (`@(...)`)

- Covered fully in Part 1 §1.3 (`At`).

## 2.20 System function/task calls (`$name(...)`)

- Covered fully in Part 1 §1.1 (`SysFunc`).

## 2.21 Expression grammar: precedence, unary/binary operators, ternary

- **Purpose and Static Nature**: Static or dynamic entirely per context (the same
  precedence-climbing parser produces expressions used in both const-evaluated and
  runtime-evaluated positions).
- **Declaration and Assignment**: N/A — this is `parse_expr`/`parse_bin`/`parse_unary`/
  `parse_primary`, a standard operator-precedence (Pratt-style) climb keyed off
  `binop_binding`'s per-operator left/right binding powers, with `**` right-associative and
  every other binary operator left-associative, and `?:` binding looser than all of them and
  right-associative.
- **Expressions and Evaluation**: Builds the `ExprAst` arena node by node (`push`), never
  producing a `Box`-graph — every reference is an `ExprRef` index into `ModuleAst::exprs`, per
  this project's arena-everything house rule (`CLAUDE.md` §5).
- **Structural and Analog Usage**: Identical everywhere an expression can occur.
- **Comparison with Traditional Constructs**: A standard precedence-climbing expression parser,
  the same technique any C-family language parser uses; the arena-of-indices representation
  (rather than a `Box`/`Rc` tree) is a Rust-specific implementation choice, not a language
  semantics difference.

## 2.22 Attribute instances (`(* ... *)`)

- **Purpose and Static Nature**: Purely metadata, entirely discarded — never reaches elaboration
  at all (skipped at the *lexer* level, treated like a comment).
- **Declaration and Assignment**: `(* key="value", key2="value2" *)` preceding a declaration
  (LRM's `attribute_instance`, e.g. annotating a parameter with `desc`/`units`).
- **Expressions and Evaluation**: N/A — skipped by a `logos` regex (`\(\*[^*]*\*+([^)*][^*]*\*+)*\)`)
  before any token is produced.
- **Structural and Analog Usage**: Can precede any declaration; has no runtime meaning in this
  subset regardless.
- **Comparison with Traditional Constructs**: Closest to a C/C++ attribute (`[[nodiscard]]`,
  `__attribute__((...))`) or a doc-comment annotation — metadata for tooling, not for the
  compiler's own semantics.

## 2.23 Compiler directives (`` `include ``, etc.)

- Covered fully in Part 1 §1.1 (`Directive`).

## 2.24 Numeric and string literal grammar

- Covered fully in Part 1 §1.1 (`Number`, `Str`).

## 2.25 Preamble discipline/nature block skipping

- Covered fully in Part 1 §1.5 (`Discipline`/`Nature`/`Enddiscipline`/`Endnature`).

## 2.26 Math builtin call names (`floor`, `ceil`, `round`, `int`, `limexp`, and the rest)

- Now-reserved words, covered fully in Part 1 §1.5's "Math builtins" entry and §1.7's fix note.
  The parser-level point worth restating here: a builtin call reaches `call_builtin` through the
  same "an `Ident`/reserved word immediately followed by `(`, and not `V`/`I`, is a call" path
  as a user-defined function call — the parser does not distinguish "known builtin name" from
  "user function name" at all; that classification happens entirely in elaboration
  (`lower_expr`'s `ExprAst::Call` arm checks the user-function table first, falling back to
  `call_builtin`). This is unaffected by whether the name happens to be reserved — reservation
  only changes whether the *bare* (non-call) form is a legal identifier.
