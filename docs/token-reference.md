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
`crates/va-frontend/src/keywords.rs::RESERVED_WORDS` (182 words, as of the discipline/nature
parsing pass adding `discrete`/`domain`/`continuous` — previously 179). That's expected and correct —
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
which *is* reserved). All eleven were added, then two of them — `vt`/`temperature` — were
**removed again**: unlike every other word in that batch, neither has a grammar production for
its bare (non-`$`) form at all, and a broad corpus scan (`external/`, ~118 files including
BSIM/HiSIM/HICUM/EKV/VBIC/PSP-family industry compact models, not just the small hand-picked
set used earlier) turned up real models declaring a plain `vt` variable, which the reservation
broke for no benefit (see §1.5's `Vt`/`Temperature` entry). Net effect: 169 + 11 − 2 = 178.

Other gaps this document surfaced and that have since been fixed: `transition` (§1.5) used to
parse as an ordinary call expression but fail at elaboration with "unknown function" — confirmed
live at the time by `va-cli check` on `external/verilogaLib-master/comparator_dynamic.va` — now
folds to its `value` argument (the only sound answer under v0's DC-only model). Access functions
were limited to `V`/`I`; `Temp`/`Pwr` (the thermal discipline's standard names from
`disciplines.vams`, §2.17) are now recognized too, fixing a parse failure in a dozen real corpus
models that contribute to a `thermal` branch. `%` (modulus) was entirely unlexed; it's now
`BinOp::Mod` (§1.2), fixing another batch of BSIM-family files that use it for parity checks
(`nf % 2`). `ddx` was entirely unrecognized ("unknown function"); it's now reserved (it's a
genuine Annex B word this table had missed until a corpus scan surfaced it) and lowers to a new
`va_ir::Expr::Ddx` (§1.5) — the only genuinely new *IR* construct among this batch of fixes,
implemented exactly per the LRM's own worked examples rather than approximated, since
`va-codegen`'s forward-mode AD already carries everything `ddx` needs.

---

# Part 1 — Lexer tokens

## 1.1 Non-keyword token kinds

These are `Token` variants defined by regex, not by a fixed spelling — each covers a whole
class of lexemes.

### `Ident(String)`

- **Purpose and Static Nature**: Purely lexical — carries any identifier-shaped lexeme
  (`[a-zA-Z_][a-zA-Z0-9_]*`, or an *escaped* identifier, below) that isn't one of the 172
  reserved words. Whether the identifier it names is itself static (a parameter, a genvar) or
  dynamic (a variable, a net) is decided later, by elaboration, not by the lexer.
- **Declaration and Assignment**: Any parameter, net, variable, branch, function, or genvar name
  is lexed as `Ident`. The two access-function names `V` and `I` are *also* plain `Ident`
  tokens — Verilog-A does not reserve them (LRM §5.5, "nature access functions"; see Part 2 §2.17
  for how the parser recognizes them contextually). An *escaped identifier* (LRM §2.8.1),
  `\name`, is a second lexeme for the same token: it starts with `\` and runs through any
  printable, non-whitespace ASCII character, ending at the first whitespace — matched by the
  regex `\\[!-~]+` (a second `#[regex(...)]` on the same `Ident` variant), stripping the leading
  `\` in its callback. Neither the leading `\` nor the terminating whitespace is part of the
  name, so `\cpu3` lexes identically to the plain identifier `cpu3` (the LRM's own example) —
  genuinely interchangeable from every later pass's point of view, since both produce the same
  `Token::Ident("cpu3")`. One real wrinkle worth knowing: an escaped identifier absorbs *any*
  printable character up to the next whitespace, including ones that are otherwise
  operators/punctuation (`\a+b ` lexes as the single identifier `"a+b"`, not `a`, `+`, `b`) —
  unusual, but exactly the LRM's rule, not a bug. No corpus file surveyed uses one; added as a
  real lexer gap regardless (`docs/roadmap.md`'s language-coverage backlog).
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
  operating point is conventionally t=0; `$mfactor` (the instance multiplicity/`m=` factor)
  folds to `1.0`, its LRM default, since v0 has no netlist-driven instance parameters to override
  it; `$param_given(name)`/`$port_connected(name)` both fold to `0.0`/false — `name` is read
  directly off the AST as a bare parameter/port-name reference (validated against the module's
  own declarations, but never lowered as a value expression), and since v0's pipeline has no
  netlist-driven instantiation at all, no parameter is ever explicitly overridden and no optional
  port is ever connected, making `false` the honest answer in every case rather than an
  approximation; `$limit(access, "fn_name"[, args...])` (a Newton convergence aid, LRM §4.5.14)
  folds transparently to its first argument's value — a converged Newton solve is a fixed point
  of the *unlimited* equations, so the limiter changes only the iteration path toward that point,
  never the point itself, and this project's stateless `ModelInstance::load` ABI has no
  previous-iteration history to limit against in the first place (see `va-core/src/convergence.rs`
  and `docs/roadmap.md`); `$rdist_uniform`/`$rdist_normal`/`$rdist_exponential`/`$rdist_poisson`/
  `$rdist_chi_square`/`$rdist_t`/`$rdist_erlang` (LRM §9.13.2, the repeatable seeded
  random-distribution family) fold to their own distribution's *mean* — `(start+end)/2` for
  `rdist_uniform` (built as a real `Add`/`Div` IR pair, since no single argument carries it), the
  bare `mean`/`degree_of_freedom` argument for every other form except `rdist_t` (`0.0`, the only
  well-defined center for a distribution symmetric about zero) — rather than an arbitrary `0.0`
  the way the noise-source builtins below do, since v0 has no simulator random-number generator
  to actually draw a sample from at all (the same "no meaningful DC value" gap `white_noise`/
  `flicker_noise`/`noise_table` have); `seed` (always first) and an optional trailing
  `type_string` (`"global"`/`"instance"`, LRM Table 9-2) are both parsed but never evaluated
  (`Elaborator::fold_rdist`); anything else reachable as a `Stmt::Task` (`$strobe`, `$finish`, …)
  is parsed but elaborates to a no-op. `$simparam` gets its default-argument fold in *two* places,
  not one: `lower_expr` (the dynamic analog-block path) and the separate, non-mutating
  `const_eval` (the parameter-default/range/genvar-bound constant-folding path) — a real model
  can default a parameter directly from `$simparam` (`external/bsim6.0.va`:
  `parameter real GMIN = $simparam("gmin", 1.0e-15);`), and the two evaluators don't share code,
  so the fold has to be taught to both explicitly.
- **Structural and Analog Usage**: Analog-block only — `$vt`/`$temperature`/`$simparam`/
  `$mfactor`/`$param_given`/`$port_connected`/`$limit`/`$rdist_*` appear in expressions,
  `$strobe`-class calls are statements. `$simparam` is the one exception to "analog-block only":
  it is also legal (and, per the corpus, common) in a parameter's own default expression.
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
  effects are built directly into elaboration. `Preprocessor::resolve_include` tries the literal
  path against each search directory first (as always), then — only if every exact candidate
  misses — retries by *basename alone* against those same directories. This closes a real,
  corpus-confirmed gap distinct from a genuinely absent file: `external/ekv3.va`'s own `` `include
  "ekv3_include/ekv3_definitions.va" `` (and 14 sibling `` `include ``s under the same
  subdirectory) names a vendor subdirectory this corpus snapshot flattened away without rewriting
  the `` `include `` directives themselves — `external/ekv3_definitions.va` (and every other
  target) is still physically present, just directly under `external/`, not
  `external/ekv3_include/`. The fallback is scoped to the already-configured search directories
  (never a new filesystem walk) and tried only after every exact match fails, so it can't reach
  across an unrelated library folder that happens to ship a same-named header (e.g. two vendors'
  own `disciplines.vams`) and never shadows a real exact match.
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

### `Plus` (`+`), `Minus` (`-`), `Star` (`*`), `Slash` (`/`), `Percent` (`%`)

- **Purpose and Static Nature**: Purely structural (arithmetic) — static or dynamic depending
  entirely on their operands; the operator itself carries no timing.
- **Declaration and Assignment**: N/A.
- **Expressions and Evaluation**: Standard left-associative binary arithmetic (`BinOp::Add/Sub/
  Mul/Div/Mod`), plus `Minus`/`Plus` double as unary prefix operators (`UnOp::Neg`; unary `+` is
  a parsed-and-discarded no-op). All const-foldable (used in parameter-range/genvar
  const-evaluation) and runtime-evaluable (used in `<+`/`=` right-hand sides). `%` (modulus)
  takes its sign from the dividend, matching Rust's/C's `%` (`self.const_eval`/`eval_binop` just
  reuse Rust's `%` on `f64` directly); in `va-codegen`'s AD it's zero-gradient like the bitwise
  operators, since it's genuinely discontinuous (jumps at every multiple of the divisor) rather
  than smoothly differentiable.
- **Structural and Analog Usage**: Identical everywhere expressions appear.
- **Comparison with Traditional Constructs**: Same as C/digital Verilog, no surprises — `%` in
  particular matches C's semantics (not Python's, which takes the sign of the *divisor*
  instead). Confirmed needed by a real corpus scan: BSIM4/BSIM6/BSIMBULK's
  `` `define BSIM4NumFingerDiff(...) if ((nf%2) != 0) ... `` macro (an even/odd finger-count
  check) was an outright lex error before this was added.

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

- **Purpose and Static Nature**: Elaboration-only — a purely syntactic marker; it never itself
  reaches the IR.
- **Declaration and Assignment**: Two grammar productions consume it (§ module instantiation,
  Annex C.8), both inside an `Item::Instance`: a named port connection, `.port(net)`
  (`parse_port_conn`), and a `#(...)` parameter override's `.name(expr)` entries
  (`parse_param_override`). Outside those two positions `.` still has no grammar rule and
  remains a parse error via the default "expected a statement"/"expected an expression"
  fallback — there is no struct/record member access in this subset.
- **Expressions and Evaluation**: N/A — a delimiter, not an expression.
- **Structural and Analog Usage**: Module-item level only (inside an instantiation's port-
  connection or parameter-override list).
- **Comparison with Traditional Constructs**: In C, `.` is struct member access; here its two
  uses are closer to Ada/VHDL-style named association (`port => net`) — binding an actual by
  the formal's declared name rather than by position.

### `Hash` (`#`)

- **Purpose and Static Nature**: Elaboration-only. Introduces a module instantiation's
  parameter-override list (§ module instantiation, §2.1b) — the values it carries must be
  const-evaluable in the instantiating module's own scope.
- **Declaration and Assignment**: `module_name # ( .param_name ( expr ) , ... ) inst_name (
  ... ) ;` — `parse_optional_param_overrides`, called from `parse_instance` right after the
  instantiated module's name. Absent entirely when a module is instantiated with no overrides
  (every parameter keeps its own default).
- **Expressions and Evaluation**: Each `.param_name(expr)` is parsed like a named port
  connection's `.port(net)` (both use `Dot`), but with an arbitrary expression instead of a
  `NetArg` in parentheses — const-evaluated by the parent (`Elaborator::const_eval`) at
  elaboration, not left as a runtime expression.
- **Structural and Analog Usage**: Module-item level only, immediately following an
  instantiation's module name.
- **Comparison with Traditional Constructs**: The direct digital-Verilog/SystemVerilog
  analogue — `#(.WIDTH(8))` instance parameterization is the same idiom under the same
  punctuation. No C analogue (C has no notion of a parameterized, reusable structural unit).

## 1.4 Dedicated structural keyword tokens

These 21 words each get their own `Token` variant (matched unconditionally by `logos`, ahead of
the generic `Keyword` fallback) because the grammar dispatches on them directly and repeatedly.
All 21 (`module`, `analog`, `begin`, `end`, `endmodule`, `parameter`, `localparam`, `real`,
`integer`, `genvar`, `input`, `output`, `inout`, `electrical`, `thermal`, `if`, `else`, `from`,
`exclude`, `inf`, `ground`) are now also listed in `RESERVED_WORDS` (`localparam`/`electrical`/
`thermal` were the gap noted above, closed by this pass).

### `Module` / `EndModule`

- **Purpose and Static Nature**: Purely structural — brackets one elaborated unit; carries no
  per-instance runtime state itself. Multiple `module...endmodule` blocks in one source unit
  are now expected, not exceptional (§ module instantiation): a file may define a subcircuit
  alongside the top module that instantiates it.
- **Declaration and Assignment**: `module name ( port_list ) ; ... endmodule` (LRM §6, "Hierar-
  chical structures"; `parse_module`). `parser::parse` loops `parse_module` until the token
  stream is exhausted, returning every module the source defines, in source order (`Vec
  <ModuleAst>`) — no change to `parse_module` itself was needed, since it already drained its
  own expression arena per call.
- **Expressions and Evaluation**: N/A — pure structure.
- **Structural and Analog Usage**: Module-level only; this *is* the module-level scope.
- **Comparison with Traditional Constructs**: A C translation unit is the loose analogue
  (top-level container, potentially defining several things); a digital-Verilog `module`/
  `endmodule` is the direct one, including module *instantiation* (one module containing
  another, `Item::Instance` — § module instantiation) — resolved entirely by
  `crate::elaborate` recursively elaborating and inlining the referenced submodule into the
  instantiating module's own IR arenas, so `va_ir::Module` itself never represents hierarchy;
  one flat module remains the only IR shape Interface α defines.

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
  return-type prefix. Any name in the list may carry *either* its own `[msb:lsb]` array range
  (`real out_val[0:15], tmp;`, § array variables, Part 2 §2.2b) — found directly in the
  `verilogaLib` corpus — *or* an inline `= expr` initializer (`real laser_freq = `P_C /
  wavelength / 1e-9;`, the exact `external/photonic/CwLaser.va` idiom), never both (the LRM's
  `real_identifier` grammar allows a dimension or an initializer, not both — `Parser::
  parse_var_entry` only looks for `=` when there was no `[...]` range).
  **Separately**, `real(expr)`/`integer(expr)` — the same dedicated tokens, but immediately
  followed by `(` — are type-cast *call* expressions, not a declaration at all (e.g.
  `digital = integer((V(in)/vref) * (1 << N));`); the parser disambiguates purely on the
  following `(`, the same way it disambiguates `V`/`I` access-function calls from ordinary
  identifiers.
- **Expressions and Evaluation**: Declaring a scalar name with no initializer introduces it into
  scope with no initial value; it becomes assignable via `=`. The type itself is parsed and then
  *discarded* for a scalar — v0 performs no integer-vs-real type checking or truncation (a
  stated limitation) — but an *array* declaration's range is genuinely load-bearing: it's
  const-evaluated and interns one `VarId` per index, named `"name[k]"`, exactly mirroring how a
  vector net expands (§2.18). An inline initializer lowers to a `Stmt::Assign`, prepended to the
  analog block (module-level) or emitted in place (block-local) — the LRM requires it to run
  before the first analog block executes, and this project has no simulation-phase distinction
  yet, so "prepended/in place, in source order" is the same DC-only approximation `@(initial_
  step)` already uses (§ event control). `real(x)` casts fold to `x` unchanged (every value here
  is already `f64` — a complete no-op); `integer(x)` rounds to nearest (`Builtin::Round`),
  matching Verilog's real-to-integer *assignment* conversion rule — not `int()`'s
  truncate-toward-zero.
- **Structural and Analog Usage**: Module-level (`real x;`, `real out_val[0:15];`) and
  analog-block-local (`real x;` inside `begin...end`) *scalar* declarations are both legal and
  treated identically by elaboration (registering the same kind of `VarId`, via
  `Elaborator::declare_local_var`). An *array* declaration is module-scope only — a block-local
  one is rejected with a specific error, since by the time the analog-block pass runs there's
  nowhere sound left to register an array's worth of nodes into (§2.2b). An explicit declaration
  always introduces a *new* identifier, shadowing a same-named module parameter for the rest of
  its block (ordinary nested-scope rules — `declare_local_var` never checks `params`, unlike
  `register_var`'s auto-registration for a bare, declaration-less assignment target, which treats
  a same-named parameter as already resolvable and registers nothing new); `Ident` resolution
  checks `vars` before `params` so a read inside the shadowing scope sees the local variable, not
  the outer parameter. A real corpus pattern: `external/bsimsoi.va`'s `begin : load ... real
  ... MJSWG; ... end`, shadowing a same-named `` `MPRoo``-macro-declared parameter.
- **Comparison with Traditional Constructs**: A scalar declaration reads exactly like a C
  `double`/`int` declaration, but v0's "declared type is parsed and dropped" behavior means it
  behaves more like a dynamically-typed language's variable declaration (Python's bare `x = 0`)
  than like C's statically-checked one. The array form is closer to a C array declaration
  (`double out_val[16];`), restricted to a compile-time-constant or genvar index — see §2.18's
  vector-net comparison, which applies identically here. `real(x)`/`integer(x)` are C-style
  casts, but as genuine callable functions rather than a `(double)x`-style unary operator —
  closer to C++'s `static_cast<int>(x)` in that sense.

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
  i<N; i=i+1) my_mod inst(...);`). This project does support module instantiation (§ module
  instantiation, `Item::Instance`), but not yet a module-item-level `generate`/`endgenerate`
  wrapping one — `generate` remains analog-block-scoped only (a stated v1 limitation, not a
  hard boundary like the rest of this entry), so a genvar-driven *array* of instances isn't
  expressible yet; a plain `Item::Instance` is always exactly one instance. Its only use here
  is still the "unroll analog code, indexing a signal vector" half of the LRM's genvar story.
  The nearest C/C++ analogue is a compile-time-unrolled loop (`#pragma unroll`, or a C++
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
  scalar node — see Part 2 §2.18). `discipline`/`nature` blocks are now genuinely parsed (§1.5's
  `discipline`/`nature` entries, § module preamble discipline/nature parsing), and a
  user-defined discipline name registered by one of those blocks can also head a net
  declaration (`Parser::parse_item`'s `self.disciplines.contains_key(name)` guard before the
  module-instantiation fallback, dispatching to the shared `Parser::parse_net_item` both forms
  use) — e.g. `discipline optical; ... enddiscipline` then `optical in_r, in_i;` (the exact
  shape in `external/microring_modulator.va`'s optical ports). It elaborates to
  `va_ir::Discipline::Other` (§1 roadmap: `va-core` doesn't model multi-physics conservation
  beyond electrical/thermal yet, so an `Other` node is still a fully usable KCL row, just not
  domain-checked). Separately, the parsing *also* unlocks access-function recognition (§2.17):
  any access name a parsed discipline binds becomes usable on a net regardless of that net's own
  declared discipline, matching how this project already treats access names as purely
  name-based, not type-checked against the declaring net.
- **Expressions and Evaluation**: N/A — pure declaration; the discipline is looked up once
  (`collect_nodes`) and attached to each interned `NodeId`. **`electrical`/`thermal` (and
  `ground`, §1.4) also parse as an ordinary identifier wherever the grammar expects a bare
  *name* rather than the start of a declaration** — `Parser::ident_like_keyword` — a real corpus
  need, not a hypothetical one: `external/ekv3_variables.va` declares `real thermal;` (a plain
  module-level variable literally spelled `thermal`), later read and reassigned as a bare
  identifier throughout `external/ekv3_noise.va`/`ekv3_oppoints.va` — all `` `include ``d into
  the same compilation unit as `external/ekv3.va`. This mirrors the precedent
  `Parser::expect_discipline_or_nature_name` already established for `electrical`/`thermal` as a
  `discipline`/`nature` block's own declared name, generalized to `Parser::expect_ident` (every
  declaration-name position: variable/parameter/net/branch/function/genvar names) and
  `Parser::parse_primary` (expression-atom position, so a later *read* of `thermal` resolves,
  and a bare `thermal = expr;` parses as `Stmt::Assign`, not a declaration). Unambiguous by
  construction: `parse_item`'s dispatch on `Token::Electrical`/`Token::Thermal` to start a net
  declaration happens *before* falling into either of these paths, so a token reached inside
  them is never the start of a declaration. Deliberately narrow — `Real`/`Integer`/`Parameter`/…
  stay fully reserved; no corpus need found for those, and their central grammar role makes the
  collision risk much higher.
- **Structural and Analog Usage**: Module-level declaration; referenced from the analog block
  only indirectly, through `V(...)`/`I(...)` access-function calls naming the net.
- **Comparison with Traditional Constructs**: No C analogue (C has no notion of a physical
  discipline). The closest digital-Verilog concept is a `wire`/`reg`'s bit width — both are a
  net-level type annotation — but "discipline" carries physics (KVL/KCL semantics), not bit
  width.

### `Ground`

- **Purpose and Static Nature**: Structural — declares that an already-declared net is the
  module's global reference node.
- **Declaration and Assignment**: `ground list_of_net_identifiers;` (LRM §3.6.4, Syntax 3-7),
  e.g. `electrical gnd; ground gnd;` — the LRM's own idiom, and every real-world example this
  project has seen. `Item::Ground { names }` (`Parser::parse_ground_item`) parses a
  comma-separated identifier list terminated by `;`. **v1 limitation:** the grammar's optional
  leading `discipline_identifier`/`range` (declaring a net inline as part of the *same*
  statement, rather than referencing an already-declared one) is not parsed — no corpus need
  found for it.
- **Expressions and Evaluation**: N/A — pure declaration, resolved once during elaboration
  (`Elaborator::collect_ground`, run right after `collect_nodes` and before anything that could
  lazily create the implicit reference node). Each named net must already exist; an undeclared
  name is an elaboration error. The *first* grounded net's own already-interned `NodeId` becomes
  `self.ground` directly (so its `NodeDecl.name` stays whatever the net was actually called, not
  a synthetic `"gnd"`); any additional grounded net in the same module is aliased into that same
  `NodeId` (`self.nodes.insert(name, gnd)`) — every net a `ground` declaration names is
  electrically the same global reference node (LRM §3.6.4), so this merges them rather than
  leaving them as distinct nodes that happen to both read as zero. The pre-existing implicit
  path — a bare single-terminal access (`V(a)`) lazily creating a node named `"gnd"`
  (`Elaborator::reference_node`) — is unchanged and now simply reuses whichever `NodeId` an
  explicit `ground` declaration already claimed, when one is present. Like `electrical`/
  `thermal` above, `ground` also parses as an ordinary identifier wherever the grammar expects a
  bare name (`Parser::ident_like_keyword`) — symmetric treatment of the same token class, though
  no corpus file surveyed declares a variable/parameter literally named `ground`.
- **Structural and Analog Usage**: Module-level declaration; referenced from the analog block
  only indirectly, through any single-terminal or explicit-to-ground `V(...)`/`I(...)` access.
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
grammar/elaboration behavior; §1.6 gives the master table covering every one of the 182 words,
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

### `Ddx`

- **Purpose and Static Nature**: Simulation-time analog operator (LRM §4.5.13, "The ddx
  operator") that returns the *symbolic* partial derivative of its first argument with respect
  to the unknown a potential-probe access identifies — `ddx(expr, V(p, n))` is
  `∂expr/∂V(p,n)`, evaluated at the current operating point. It is stateless (unlike
  `ddt`/`idt`), but like them it must be re-evaluated every Newton iteration since the
  derivative's *value* depends on the current solution point even though the differentiation
  itself is exact/symbolic rather than a numeric finite-difference approximation. This project's
  forward-mode AD engine (`va-codegen/src/ad.rs`) already tracks a per-node-slot gradient
  alongside every `Dual` value it computes, so `ddx` costs nothing extra to implement correctly:
  the operator is exactly "read the gradient component already sitting at the probed node,"
  never a separate differentiation pass.
- **Declaration and Assignment**: Called as `ddx(expr, V(p, n))`, exactly two arguments — an
  arbitrary analog expression, and a potential-probe access (`V(p)`/`V(p,n)` or a
  discipline-appropriate equivalent) naming the unknown to differentiate against. The LRM also
  permits `I(branch)` as the second argument in principle, but this codegen's flow probes are
  not independent unknowns (currents are solved-for quantities derived from other unknowns, not
  free variables with their own AD gradient slot), so `ddx(..., I(...))` is rejected with an
  explicit elaboration error rather than silently producing a wrong (always-zero) answer — an
  honest scope caveat per CLAUDE.md §1, not a silent gap.
- **Expressions and Evaluation**: Lowered to `Expr::Ddx(ExprId, Access)` (Interface α, added
  2026-07-02 — see `docs/bridges/interface-alpha-ir.md`). The elaborator validates the second
  argument is syntactically a `Probe(Access)` of `AccessKind::Potential` before constructing the
  node; `va-codegen`'s `eval` evaluates the first argument to a full `Dual`, then reads
  `d.grad[p]` where `p` is the probed branch's positive node's local slot (`0.0` if that node
  never entered the expression's dependency set — a legitimate zero-derivative answer, not a
  missing-value error). Validated directly against the LRM's own worked examples: the §4.5.13
  VCCS example (`ddx(vin, V(pin)) == 1`, `ddx(vin, V(nin)) == -1`, `ddx(vin, V(pout)) == 0`) and
  the LRM's diode conductance example, the latter additionally cross-checked against a central
  finite difference per CLAUDE.md §5's AD-validation rule.
- **Structural and Analog Usage**: Analog-block only, like every other analog operator — never
  legal in a parameter default, genvar loop header, or outside an `analog` block.
- **Comparison with Traditional Constructs**: No C/digital-Verilog equivalent — symbolic
  differentiation with respect to a live simulation unknown (rather than a syntactic
  sub-expression, as in a CAS) is intrinsic to analog device-model construction (e.g. building
  a small-signal transconductance directly from a large-signal current expression) and has no
  discrete-time or general-purpose-language analogue.

### `Vt` / `Temperature` — not reserved (fixed)

- **Purpose and Static Nature**: Simulation-time environment queries, reachable only through
  the `$`-prefixed `SysFunc` token (`$vt`, `$vt(T)`, `$temperature` — see §1.1's `SysFunc`
  entry for the real grammar). `vt`/`temperature` are *not* reserved words in this project,
  even though Annex B lists them: the bare word (no `$`) has no grammar production consuming it
  at all, so reserving it was pure downside with no benefit. This was tried the other way first
  (both were reserved for a time) and reverted once a broad real-model corpus scan turned up
  `real vt; vt = $vt(...);` — caching the thermal-voltage value under its conventional plain
  name — as the single most common reservation conflict in that corpus (confirmed directly in
  `external/igbt3.va`).
- **Declaration and Assignment**: Freely usable as an ordinary parameter/variable/genvar/net
  name, exactly like any other identifier — `real vt, temperature;` declares two ordinary
  variables.
- **Expressions and Evaluation**: A bare `vt`/`temperature` reference resolves like any other
  identifier (parameter, then variable, then error if unknown) — there is no special-casing at
  all; the name is not privileged or shadowed by the `$`-prefixed system functions, which live
  in an entirely separate lexical channel (`Token::SysFunc`, not `Token::Ident`/`Token::Keyword`)
  and never collide with a same-spelled bare identifier.
- **Structural and Analog Usage**: Anywhere an ordinary identifier is legal.
- **Comparison with Traditional Constructs**: This is exactly the situation C/C++ handle by
  putting library names in a separate namespace or requiring a prefix (`std::`, `errno` vs.
  `EINVAL`) rather than reserving the bare word — `$vt` already *is* that prefix, so reserving
  the unprefixed `vt` on top of it added a second layer of protection nothing needed.

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

### `Absdelay`

- **Purpose and Static Nature**: A genuinely time-domain analog operator in full Verilog-AMS
  (LRM §4.5.9): `absdelay(value, delay[, max_delay])` delays `value` by a fixed time `delay`,
  tracking *when* `value` last changed the same way `transition`/`slew` do. v0 is DC-only (no
  time axis to delay through), and the filter settles to its undelayed input in steady state (no
  delay history exists at a fixed operating point), so it folds transparently to its `value`
  argument at elaboration — identical treatment to `Transition`/`Slew` above, in the same
  `Elaborator::lower_expr` arm family. Previously unimplemented: parsed as an ordinary call but
  failed at elaboration with "unknown function `absdelay`" — confirmed live by `va-cli check` on
  `external/fbh_hbt-2_1.va`, which now passes the frontend end to end.
- **Declaration and Assignment**: Called as `absdelay(value, delay[, max_delay])` — `value` is
  required, `delay`/`max_delay` optional/parsed but unused.
- **Expressions and Evaluation**: Only `value` is lowered and returned as-is (no wrapper node);
  `delay`/`max_delay` are read from the AST only to check `value` is present, never evaluated —
  an empty argument list is a hard error.
- **Structural and Analog Usage**: Analog-block only, typically wrapping a `<+` contribution's
  right-hand side.
- **Comparison with Traditional Constructs**: No C/digital-Verilog analogue. Once `va-transient`
  exists, this would need real time-delay handling rather than a constant fold — a stated,
  deliberate v0 simplification, not a permanent design decision.

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

- **Purpose and Static Nature**: Elaboration-only — fully parsed (§ module preamble
  discipline/nature parsing) into a small in-`va-frontend` table
  (`disciplines::{NatureDecl, DisciplineDecl}`), not merely recognized-and-discarded. Runs
  before every `module` in the token stream (`Parser::parse_preamble`, called from
  `parse_module`), so blocks interleaved between modules — or an expanded
  `` `include "disciplines.vams" `` preceding just the first — are all reached the same way.
- **Declaration and Assignment**: `discipline name [;] ... enddiscipline` / `nature name [;]
  ... endnature` (LRM §4). The `;` after the name is optional — both the canonical
  `disciplines.vams` (semicolon) and the real `external/ekv3_natures.va` (no semicolon) shapes
  parse. A nature's body is `units = "...";`/`access = Name;`/`abstol = value;`/
  `idt_nature = Other;`/`ddt_nature = Other;`; a discipline's is `potential Nature;`/
  `flow Nature;`/`domain discrete|continuous;`. An unrecognized attribute keyword inside either
  body is a hard parse error — the LRM's attribute set is fixed, not user-extensible, matching
  every other unknown-construct error in this parser.
- **Expressions and Evaluation**: `units`/`idt_nature`/`ddt_nature` are parsed but remain
  **unused metadata** — no `va-core` unit-checking code consults them yet (like `ast::Range`'s
  inclusive/exclusive flags). `abstol` (§ nature-metadata wiring, added 2026-07-09) is the
  exception: its value is read as a plain (optionally negated) numeric literal when the source
  writes one — a more complex expression (`abstol = 2*1e-6;`) still parses (its tokens are
  consumed via the ordinary expression parser) but its value is dropped rather than rejected —
  and now round-trips all the way into `va-core`. `Parser::parse_with_disciplines` exposes the
  parsed `natures`/`disciplines` tables (dropped by the plain `Parser::parse` most callers use);
  `crate::compile_with_includes` threads them into
  `Elaborator::elaborate_with_library_and_disciplines`, which resolves each net's discipline to
  its **potential** nature's `abstol` (`disciplines::resolve_abstol`) and records it on the
  matching `va_ir::NodeDecl::abstol` (an Interface α change). `va-codegen`'s generated models
  expose that per-node value via `va_abi::ModelInstance::unknown_abstol` (an Interface β
  addition, the same default-method shape as `unknown_kind`); `va-core::mna::classify_abstol`
  collects it, and `newton::solve_from`'s per-unknown convergence check consults it instead of
  always using the solver's single configured default. No wiring exists yet for a discipline's
  *flow* nature (e.g. `Current`'s own `abstol`) — only a `Node`-kind unknown has a natural
  `NodeDecl`-shaped home for one. The other attribute with a *real* effect is a nature's
  `access` name, once a `discipline` block binds that nature as its `potential`/`flow` nature:
  `Parser::register_access` then adds that access name to the recognized set
  (`Parser::known_access`) — additively, on top of the always-on `V`/`I`/`Temp`/`Pwr` baseline,
  which stays recognized regardless of whether any block was ever parsed. See §2.17.
- **Structural and Analog Usage**: Module-preamble-level — before, or interleaved between, the
  modules in a compilation unit; never inside a module or an `analog` block.
- **Comparison with Traditional Constructs**: A discipline/nature pair is the closest thing this
  language has to a C `struct`/units-of-measure system (binding a physical unit and tolerance to
  a signal type) — no C construct maps onto it directly.
- **A real grammar collision worth noting**: the standard header itself declares
  `discipline electrical; ... enddiscipline` and `discipline thermal; ... enddiscipline` —
  i.e. a discipline's own *name* is literally `electrical`/`thermal`, which this project already
  lexes as its own dedicated `Token::Electrical`/`Token::Thermal` (for net declarations), not as
  `Token::Ident`. `Parser::expect_discipline_or_nature_name` accepts either spelling for a
  discipline/nature's declared name specifically because of this — an ordinary `expect_ident`
  would reject the very file that motivated this whole feature.

## 1.6 Master table — every reserved word

Every one of the 182 words in `RESERVED_WORDS`, alphabetically, each addressed against all five
questions. Words with a full write-up above are cross-referenced rather than repeated; the
remaining ~110 words — almost entirely digital-Verilog gate primitives, net-strength/charge
keywords, specify-block/task/event keywords, and signal-processing transform names — get their
first (and, for the ~90 with zero implemented behavior, only) treatment here.

| Token | Purpose & Static Nature | Declaration & Assignment | Expressions & Evaluation | Structural & Analog Usage | Comparison with Traditional Constructs |
|---|---|---|---|---|---|
| `abs` | Dynamic/static dual, see §1.5 Math builtins | `abs(x)` call | Absolute value, both paths | Analog expr / const context | C `fabs()`/`abs()` |
| `absdelay` | Folds to its `value` argument (fixed — see §1.5 `Absdelay`); settles to input at DC | `absdelay(value, delay[, max_delay])` call | Identity on `value`; `delay`/`max_delay` parsed, never evaluated | Analog-block only | No C analogue |
| `abstol` | Parsed into `NatureDecl::abstol` (§1.5 `Discipline`/`Nature`); round-trips into `va_ir::NodeDecl::abstol` and `va-core`'s per-unknown Newton convergence check (§ nature-metadata wiring) | Nature attribute `abstol = expr;` | Read only when `expr` is a plain (optionally negated) numeric literal; a more complex expression still parses (tokens consumed) but the value is dropped | N/A (module preamble) | A nature's absolute-tolerance attribute; no C analogue |
| `access` | Parsed into `NatureDecl::access` (§1.5), widens the recognized access-function set once bound by a `discipline` | Nature attribute `access = fn_name;` | Read as a plain identifier; has a real effect (§2.17) once a `discipline` binds this nature as `potential`/`flow` | N/A (module preamble) | Names the `V`/`I`-style access function for a custom nature; no C analogue |
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
| `continuous` | A discipline's `domain continuous;` attribute value (§1.5), fully parsed — the LRM default, so rarely written explicitly | `domain continuous;` inside a `discipline` body | Parsed into `DisciplineDecl::domain` (`DomainKind::Continuous`), unused metadata | Module preamble | No C analogue |
| `cos` | Dynamic/static dual, §1.5 | `cos(x)` call | Cosine | Analog expr / const context | C `cos()` |
| `cosh` | Dynamic/static dual, §1.5 | `cosh(x)` call | Hyperbolic cosine | Analog expr / const context | C `cosh()` |
| `cross` | Parses as a call (`cross(expr, dir[, ...])`) if written bare; its one real usage, `@(cross(...))`, is discarded wholesale by v0's `skip_balanced_parens` before it's ever parsed as an expression | Zero-crossing event detector (LRM §5.7ish) | Neither path currently evaluates its arguments | Analog-block only (event control) | No C analogue (continuous zero-crossing detection needs the solver's own state) |
| `ddt` | Analog operator with internal state, §1.5 `Ddt`/`Idt` | `ddt(expr)` | Time derivative | Analog-block only | No C/digital-Verilog analogue |
| `ddt_nature` | Parsed into `NatureDecl::ddt_nature` (§1.5), unused metadata | Nature attribute `ddt_nature = other_nature;` | Read as a plain identifier, not yet consulted by anything | N/A (module preamble) | Binds a nature to its time-derivative counterpart; no C analogue |
| `ddx` | Simulation-time symbolic derivative, §1.5 `Ddx` | `ddx(expr, V(p, n))` call | Lowers to `Expr::Ddx`; codegen reads the AD gradient at node `p` | Analog-block only | No C/digital-Verilog analogue |
| `deassign` | Reserved, no grammar production (digital procedural-continuous-assignment release) | N/A | N/A | Digital only | No C analogue |
| `default` | Case-arm keyword, §1.5 | `default[:] body` inside `case...endcase` | Dynamic body | Analog-block only | C `switch`'s `default:` |
| `defparam` | Reserved, no grammar production (digital hierarchical parameter override by path, e.g. `defparam top.sub.R = 2k;`) | N/A | N/A | Structural/hierarchy only; module instantiation exists now (§ module instantiation), but only its own `#(.name(expr))` override syntax at the instantiation site — a separate, deprecated *post-hoc*-by-hierarchical-path override mechanism like `defparam` remains unimplemented | No C analogue |
| `delay` | Reserved, no grammar production (specify-block path delay) | N/A | N/A | Specify-block (timing-check) only | No C analogue |
| `disable` | Reserved, no grammar production (digital named-block/task abort) | N/A | N/A | Digital procedural only | Loosely C's `goto`-out-of-block, but scoped to a named block/task |
| `discipline` | Genuinely parsed into `DisciplineDecl` (§1.5, `Parser::parse_discipline`) | `discipline name [;] ... enddiscipline` | Its `potential`/`flow`/`domain` attributes are all parsed (§1.5) | Module preamble | Closest to a C `struct`/unit-of-measure definition |
| `discontinuity` | Reserved, no grammar production (`discontinuity(order);` hints the solver about a non-smooth point) | N/A | N/A | Would be analog-block only | No C analogue (a numerical-solver hint) |
| `discrete` | A discipline's `domain discrete;` attribute value (§1.5), fully parsed | `domain discrete;` inside a `discipline` body | Parsed into `DisciplineDecl::domain` (`DomainKind::Discrete`), unused metadata | Module preamble | No C analogue |
| `domain` | A discipline attribute keyword (§1.5), fully parsed | `domain discrete\|continuous;` inside a `discipline` body | Parsed into `DisciplineDecl::domain`, unused metadata | Module preamble | No C analogue |
| `edge` | Parses as a call (`edge(expr)`) if written bare; realistically only ever appears inside a discarded `@(...)` | Digital-style edge-detection function | Rejected at elaboration if reached | Analog-block only (event control) | Closest to a rising/falling-edge interrupt trigger; no C analogue |
| `electrical` | Dedicated token, §1.4 | — | — | — | — |
| `else` | Dedicated token, §1.4 | — | — | — | — |
| `end` | Dedicated token, §1.4 | — | — | — | — |
| `endcase` | Case-block terminator, §1.5 | Closes `case...endcase` | N/A | Analog-block only | C `switch`'s closing `}` |
| `enddiscipline` | Genuinely recognized as the block terminator (§1.5, `Parser::parse_discipline`) | Closes `discipline...enddiscipline` | N/A | Module preamble | — |
| `endfunction` | Function-definition terminator, §1.5 | Closes `analog function...endfunction` | N/A | Module-level | C function's closing `}` |
| `endgenerate` | Syntactic bracket only, §1.5 `Generate`/`Endgenerate` | Closes `generate...endgenerate` | N/A | Analog-block only | No C analogue |
| `endmodule` | Dedicated token, §1.4 | — | — | — | — |
| `endnature` | Genuinely recognized as the block terminator (§1.5, `Parser::parse_nature`) | Closes `nature...endnature` | N/A | Module preamble | — |
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
| `flow` | A discipline attribute keyword (§1.5), fully parsed and given real effect | `flow Nature;` inside a `discipline` body | Parsed into `DisciplineDecl::flow`; also calls `Parser::register_access` (§2.17), binding that nature's `access` name as a recognized `Flow`-kind access function | Module preamble | Names the conserved "current-like" quantity of a discipline; no C analogue |
| `for` | Simulation-time (or elaboration-time when genvar-driven), §1.5/Part 2 §2.14 | `for (init; cond; step) body` | Dynamic, or const-evaluated if genvar-driven | Analog-block only | C `for` — with the added genvar-unrolling mode C has no concept of |
| `force` | Reserved, no grammar production (digital procedural force-a-net) | N/A | N/A | Digital procedural only | No C analogue |
| `forever` | Reserved, no grammar production (digital unconditional loop) | N/A | N/A | Digital procedural only | C's `for(;;)`/`while(1)` |
| `fork` | Reserved, no grammar production (digital concurrent-process block, paired with `join`) | N/A | N/A | Digital procedural only | Loosely POSIX threads' fork, but cooperative/simulation-scheduled |
| `from` | Range-clause keyword, §1.4 | `from [lo:hi]`/`(lo:hi)` | Const-evaluated bounds | Module-level (parameter ranges) | No C analogue |
| `function` | Function-definition keyword, §1.5 | `analog function ...` | — | Module-level | C `static` pure function |
| `generate` | Syntactic bracket only, §1.5 | `generate ... endgenerate` | N/A | Analog-block only | No C analogue |
| `genvar` | **Elaboration-only construct**, dedicated token, full treatment in §1.4 | — | — | — | — |
| `ground` | Dedicated token, real grammar production, §1.4 | `ground list_of_net_identifiers;` | Aliases each named (already-declared) net to the module's reference node | Module-level declaration | No general-purpose analogue |
| `highz0` | Reserved, no grammar production (net strength: high-impedance driving 0) | N/A | N/A | Digital net-strength only | No C analogue |
| `highz1` | Reserved, no grammar production (net strength: high-impedance driving 1) | N/A | N/A | Digital net-strength only | No C analogue |
| `hypot` | Dynamic/static dual, §1.5 | `hypot(x, y)` call | `sqrt(x²+y²)` | Analog expr / const context | C99 `hypot()` |
| `idt` | Analog operator with internal state, §1.5 | `idt(expr)` | Time integral | Analog-block only | No C/digital-Verilog analogue |
| `idt_nature` | Same as `ddt_nature` (nature attribute, parsed but unused) | Nature attribute `idt_nature = other_nature;` | Read as a plain identifier, not yet consulted by anything | N/A (module preamble) | No C analogue |
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
| `nature` | Genuinely parsed into `NatureDecl` (§1.5, `Parser::parse_nature`) | `nature name [;] ... endnature` | Its `units`/`access`/`abstol`/`idt_nature`/`ddt_nature` attributes are all parsed (§1.5) | Module preamble | Closest to a C units-of-measure/tolerance struct |
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
| `potential` | A discipline attribute keyword (§1.5), fully parsed and given real effect | `potential Nature;` inside a `discipline` body | Parsed into `DisciplineDecl::potential`; also calls `Parser::register_access` (§2.17), binding that nature's `access` name as a recognized `Potential`-kind access function | Module preamble | Names the conserved "voltage-like" quantity of a discipline; no C analogue |
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
| `units` | Parsed into `NatureDecl::units` (§1.5), unused metadata | Nature attribute `units = "V";` | Read as a string literal via `Parser::expect_string`, not yet consulted by anything | N/A (module preamble) | No C analogue |
| `vectored` | Reserved, no grammar production (net-vector storage-layout hint, pairs with `scalared`) | N/A | N/A | Digital structural only | No C analogue |
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

- **Purpose and Static Nature**: Purely structural; parsed once per module, contributing one
  `ModuleAst` to `parser::parse`'s returned list (a source unit may define several — § module
  instantiation).
- **Declaration and Assignment**: `module name ( port_name, ... ) ; items... endmodule`
  (`parse_module`, looped by `parser::parse` until the token stream is exhausted). Ports are
  bare names here — direction/discipline are separate declarations elsewhere in the item list,
  matched by name at elaboration (`resolve_ports`).
- **Expressions and Evaluation**: N/A.
- **Structural and Analog Usage**: The entire module-level scope, containing every other item,
  at most one `analog` block (a module composed purely of instantiated submodules may have
  none at all), and zero or more `Item::Instance`s (§2.1b).
- **Comparison with Traditional Constructs**: A digital-Verilog `module`, including
  instantiation of other modules (§2.1b) — unlike the rest of this table, no longer a stated
  scope boundary.

## 2.1b Module instantiation (`resistor r1(p, n);`, LRM Annex C.8)

- **Purpose and Static Nature**: The *declaration* (which module, which instance name, which
  parameter overrides, which port connections) is elaboration-only — fully resolved by
  recursively elaborating the referenced submodule and inlining its arenas into the
  instantiating module's own (`Elaborator::collect_instances`/`inline_instance`/
  `merge_submodule`, `crates/va-frontend/src/elaborate.rs`). The inlined analog behavior itself
  is, as always, simulation-time.
- **Declaration and Assignment**: `module_name [ # ( .param_name ( expr ) , ... ) ] inst_name (
  conn, ... ) ;`, where each `conn` is either a bare net (positional, bound to the submodule's
  ports in declaration order) or `.port_name ( net )` (named, bound by name in any order) —
  `Item::Instance { module, name, params, connections }`, `ast::PortConn::{Positional,
  Named}`. A leading bare identifier at item level is unambiguous: every other item production
  starts with a dedicated keyword/type token, so `parse_item` routes any `Ident` straight to
  `parse_instance`. Connections must be all-positional or all-named (parses either way; a mix
  is rejected at elaboration, not by the grammar).
- **Expressions and Evaluation**: A `#(...)` override's `expr` is const-evaluated
  (`const_eval`) in the *instantiating* module's own scope (it may reference the parent's own
  parameters/genvars) before being substituted for the submodule's corresponding parameter
  default — validated against that parameter's declared `from` range exactly as an ordinary
  default is. The submodule is then elaborated as if standalone; its parameters never survive
  as `va_ir::Param`s in the parent — every `Expr::Param` reference inside its copied body
  collapses to `Expr::Const` using the already-resolved value, since a Verilog-A parameter is
  a compile-time constant either way. A port connection's net is resolved in the parent's own
  scope (`resolve_conn_nodes` — a scalar net, a constant/genvar-indexed single vector-net
  element, a bare vector-net name, or an explicit `[msb:lsb]` slice, the last two both
  resolving to the connection's full ascending-index-order node list) and bound element-wise
  (`bind_port_nodes`) to the submodule's corresponding port's own node list — a vector port
  (`electrical [0:1] x, y;`) connects exactly like a scalar one once both sides are just "a node
  list of the same width," which is also why a connection whose resolved width disagrees with
  the port's declared width is a dedicated elaboration error, not a silent partial bind. Every
  other node, branch, variable, and function the submodule declares is copied into the parent's
  arenas, namespaced `"{inst_name}.{name}"` to avoid colliding with a same-named parent
  declaration (impossible anyway, since Verilog-A identifiers can't contain `.`).
- **Structural and Analog Usage**: Module-item level, exactly like a net or parameter
  declaration — never inside an `analog` block. A cycle (a module instantiating itself,
  directly or transitively) and an unknown module/port/parameter-override name are all
  elaboration errors (`FrontendError::Elaborate`), not silently accepted.
- **Comparison with Traditional Constructs**: A digital-Verilog module instantiation, resolved
  the way an aggressively-inlining compiler would rather than the way a hardware elaborator
  keeps hierarchy: `va_ir::Module` never gains a hierarchy concept of its own (Interface α is
  unchanged by this — see `docs/interfaces.md`'s note), so `va-codegen`/`va-core`/`va-abi`
  still only ever see one flat module, exactly as before. **Cross-file instantiation**: at the
  `va_frontend::elaborate` layer, `Elaborator::library` is still just "every module parsed from
  one compilation unit" (`elaborate_with_library`'s caller decides what that unit is — it never
  cared which file an entry came from, so no frontend/Interface α change was needed here). A
  submodule defined in a different `.va` file (a real pattern: `external/photonic/
  Attenuator.va` instantiates `Polar2Cartesian`, declared in the sibling `Polar2Cartesian.va`)
  now resolves at the `va-cli` layer instead: `check_models` groups every file it's about to
  check by its own immediate parent directory and elaborates every module from every
  successfully-parsed file in a group against one library combining all of them
  (`crates/va-cli/src/lib.rs`'s `check_group`) — deliberately scoped to "files sharing one
  directory," not the whole top-level scanned root, since several real corpus files at the same
  nesting depth directly under `external/` declare a module with the same name (a directory-wide
  merge across unrelated vendor releases would risk silently resolving an instantiation against
  the wrong same-named module). `run_sim`'s `--model <path>` (the actual simulation front door,
  as opposed to the `check` diagnostic) still only ever compiles the one given file via
  `compile_with_includes`, so this fix is `check`-only for now — a stated remaining v1 limit.
  **Other stated v1 limits**: no module-item-level `generate`/`endgenerate` around an instance
  (so no genvar-driven *array* of instances — see §1.4 `Genvar`'s and §2.14's comparison notes);
  and a submodule's own implicit ground (from a
  single-terminal `V(p)` shorthand, `Elaborator::reference_node`) is *not* unified with the
  parent's or a sibling instance's ground, since each submodule elaborates in its own arena — a
  model that needs the true circuit reference node from inside a submodule must declare an
  explicit port for it and have the instantiating parent wire that port to real ground, like
  any other port.

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
- **§ 2-D vector net (non-standard extension, added 2026-07-09)**: a *second* dimension —
  `electrical [0:R][0:C] grid;` (prefix) or `electrical grid[0:R][0:C];` (suffix), capped at
  exactly 2 dimensions — is **not** standard Verilog-A. The real LRM's `net_declaration`
  grammar only ever carries one `[msb:lsb]` range; this project implements it anyway as a
  deliberate, clearly labeled extension (see §2.2c) because it's the natural way to express a
  2-D-addressed grid of physical nodes ("tiles"), and CLAUDE.md's scope-creep guardrail is about
  *silently* drifting beyond the LRM, not about a documented, opt-in exception. Contrast §2.2b's
  array-variable 2-D form, which *is* standard grammar.

## 2.2b Array variable declaration & indexed access (`real out_val[0:15]`, `out_val[i]`)

The `real`/`integer` counterpart of §2.2/§2.18's vector nets, added once the real corpus
(`external/verilogaLib-master/adc_16bit_ideal.va`/`dac_16bit_ideal.va`) needed it: `digital =
integer(...); for (i=0; i<N; i=i+1) begin if ((digital >> i) & 1) out_val[i] = high; ... end`.

- **Purpose and Static Nature**: The *declaration* is elaboration-only (interning one `VarId`
  per index, exactly like a vector net interns one `NodeId` per index). An access's index is
  either elaboration-time-constant/genvar-derived (resolves directly to that one `VarId`,
  simulation-time-dynamic only in the ordinary "which value does the variable hold" sense) or a
  genuinely runtime expression (§ dynamic vector-net/array-variable indexing, fixed
  2026-07-02) — the direct real-corpus idiom above (`digital = integer(...); for (i=0; i<N;
  i=i+1) begin ... out_val[i] = ...; end`) uses an ordinary `integer` loop variable, not a
  `genvar`. There is still no runtime-indexable-*storage* concept in the IR itself (unlike full
  Verilog-A/digital Verilog's `reg [7:0] mem [0:255];`) — a runtime index is instead resolved by
  *elaboration-time unrolling into every statically-known candidate*, guarded by an equality
  check against the actual runtime value, which is sound precisely because the array's range is
  always static.
- **Declaration and Assignment**: `real|integer name[msb:lsb], name2, ...;` — only the
  per-identifier suffix form (§2.2's `NetDecl`-style prefix-range form doesn't apply to
  `real`/`integer`, which have no bit-width concept to prefix). `name[index_expr] = rhs;`
  assigns one element (`Stmt::Assign.index: Vec<ExprRef>`, 0–2 entries); `name[index_expr]`
  reads one (`ExprAst::IndexedIdent`) — both share the AST-level index-parsing helper vector-net
  access uses (`Parser::parse_index_list`, capped at 2 entries — see §2.2c for the 2-D form).
- **Expressions and Evaluation**: A constant/genvar index is evaluated by `const_eval_int` and
  bounds-checked against the array's declared `(lo, hi)` (`resolve_var_array_index`, thinly
  wrapping `resolve_array_var_at`, the constant-index tail also used by the runtime-index path
  below). A genuinely runtime index — detected by probing `const_eval(index_expr)` and checking
  whether it errors, *without* propagating that error — routes instead to
  `lower_indexed_var_read`/`lower_indexed_var_write`: a **read** (`out_val[j]` in an expression)
  expands into a nested `Expr::Select` chain, one arm per declared index `k`, each guarded by
  `j == k`, bottoming out at the unconditional `hi` arm; a **write** (`out_val[j] = rhs;`)
  expands the same way into an if/else-if chain of `Stmt::Assign`s. `rhs`/the read index are each
  lowered exactly once and the resulting `ExprId` shared across every arm — not re-lowered per
  arm — so the expansion costs `O(range)` arena nodes, not `O(range)` re-evaluations of a
  possibly-expensive sub-expression. **Limitation**: a runtime index outside the array's declared
  range at simulation time silently resolves to the `hi` arm rather than erroring — there is no
  runtime-error concept in this IR/ABI; every corpus model driving this path bounds its own loop
  to the array's declared range, so the fallback is never actually reached in practice.
- **Structural and Analog Usage**: Declaration is module-level only (`Item::Var`); a block-local
  attempt (`Stmt::VarDecl`) is rejected with a specific error — by the time the analog-block
  pass runs, module-level declarations are already finalized, so there is nowhere sound left to
  register an array's nodes into. Indexed access (read or write) is analog-block only, whether
  behind a genvar-driven `for` (§2.14, constant path) or an ordinary runtime `for`/`while`
  (dynamic path).
- **Comparison with Traditional Constructs**: The constant/genvar path is a C array restricted
  to a `constexpr`/genvar-derived index. The runtime path has no direct C analogue — it isn't a
  runtime-indexed array read at all under the hood, but a compile-time-unrolled `switch`/chained
  `if` over every statically-known index, closer to how a C compiler might *implement* a small,
  bounded-range switch than to how a programmer would *write* one; a real Verilog-A/digital
  `reg`-memory read is a single indexed load, not an unrolled comparison chain. Confirmed live
  by the real corpus once vector *ports* started resolving (§2.18): both
  `dac_16bit_ideal.va`/`adc_16bit_ideal.va` index their vector nets/arrays with a plain
  `integer` loop variable, and both now elaborate cleanly end to end.

## 2.2c 2-D array variables & 2-D vector nets (`tile[0:R][0:C]`, `grid[0:R][0:C]`, added 2026-07-09)

The general 2-D-addressed-"tile" case: `real`/`integer` array variables and (as a documented
non-standard extension, §2.2) vector nets both generalize from 1 to up to 2 declared
dimensions, indexed as `tile[i][j]` / `V(grid[i][j])`. Not general N-D — capped at exactly 2.

- **Purpose and Static Nature**: Same as §2.2/§2.2b — elaboration-only. A 2-D name interns one
  `NodeId`/`VarId` per index *tuple*, row-major, named `"base[i][j]"` (`Elaborator::indexed_key`
  builds the flattened key; `Elaborator::dim_indices` enumerates the tuples at declaration
  time). `va-ir` (Interface α) needs no change — a 2-D vector/array still fully flattens to
  scalar `NodeDecl`/`VarDecl`s, exactly like the 1-D case.
- **Declaration and Assignment**: `real tile[0:R][0:C], scalar;` (LRM-standard — the
  `variable_identifier` production allows a repeated unpacked-dimension list) and
  `electrical [0:R][0:C] grid;` / `electrical grid[0:R][0:C];` (non-standard extension, §2.2).
  Both go through `Parser::parse_dim_list`, which loops the single-dimension
  `parse_bracket_range` primitive, capped at 2 (a 3rd bracket group is a parse error). AST:
  `NetDecl.ranges`/`VarEntry.ranges: Vec<(ExprRef, ExprRef)>` (0, 1, or 2 entries).
- **Expressions and Evaluation**: Indexed access — `NetArg.index`/`Stmt::Assign.index`/
  `ExprAst::IndexedIdent`'s second field — is likewise `Vec<ExprRef>` (0–2 entries), parsed by
  `Parser::parse_index_list`/`parse_net_arg`'s bracket loop. Resolution generalizes §2.2/§2.2b's
  single-index functions to take an index slice (`Elaborator::resolve_vector_node_at`/
  `resolve_array_var_at`: `&[i64]`), bounds-checking each dimension and erroring on a
  dimension-*count* mismatch too (`grid[0]` against a declared-2-D `grid` is rejected, not
  silently treated as a partial/broadcast access).
  - **Dynamic (runtime) indexing**: at most **one** of a name's (up to 2) index positions may be
    a genuinely runtime expression per access — `Elaborator::dynamic_index_pos` scans and
    rejects two simultaneously-dynamic positions on the same name. This mirrors §2.2b's existing
    precedent of rejecting a two-dynamic-*terminal* access rather than building an `O(range²)`
    chain: a 2-D name's own two index positions are a second place that same blowup could occur,
    so the same discipline applies. When exactly one dimension is dynamic, only *that* dimension
    unrolls into the `Select`/`If` chain (`combine_idx` reattaches the other, already-resolved
    dimension to each candidate) — the expansion stays `O(range)`, never `O(range²)`.
- **Structural and Analog Usage**: A 2-D array variable behaves exactly like §2.2b's 1-D form
  (module-scope-only declaration, analog-block-only access). A 2-D vector net has additional
  restrictions a 1-D vector net doesn't: it can never be used as a module port
  (`Elaborator::resolve_ports` rejects a declared vector whose dimension count isn't 1), and a
  port *connection* (`resolve_conn_nodes`) only accepts a fully 2-indexed single node — never
  bare or partially indexed. Slicing (`[lo:hi]`) stays single-dimension-only in both `resolve_
  net_arg` and `resolve_conn_nodes`: it's rejected outright on a declared-2-D vector, and
  rejected if combined with a non-empty index — even though `bus[i][lo:hi]` (an index followed
  by a trailing slice) parses syntactically (`Parser::parse_net_arg` accepts it structurally;
  only elaboration decides whether it's meaningful for the particular declared name).
- **Comparison with Traditional Constructs**: A 2-D array variable is the direct analogue of a
  C 2-D array (`double tile[R][C];`), restricted (like §2.2b's 1-D case) to a
  `constexpr`/genvar-derived index on the constant path, or an unrolled comparison chain on the
  (single-dimension-only) dynamic path. A 2-D vector net has no clean traditional analogue at
  all — it's a non-standard extension purpose-built for a 2-D grid of physical circuit nodes
  ("reticule"-style addressing), closest in spirit to a 2-D array of wires in a hardware
  description language that *does* support multi-dimensional nets (e.g. SystemVerilog's packed/
  unpacked array nets), which Verilog-A itself does not.

## 2.3 Direction declaration

- A port's direction (`input`/`output`/`inout`) may repeat a vector net's `[msb:lsb]` width at
  the direction-declaration site too (again, the LRM's own DAC example: `input [0:width-1] in;`
  alongside the matching `electrical [0:width-1] in;`). This is parsed and discarded — purely
  informational here, since the real range comes from the paired net declaration (§2.2) — so
  real-world vector-port headers now parse instead of failing on the bracket. The port *itself*
  being a vector is fully supported (fixed 2026-07-02): `va_ir::Module::ports` is
  `Vec<Vec<NodeId>>`, one entry per declared port, holding all of a wide port's nodes — see
  §2.18's "Vector ports (fixed)" note for the full account.
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
- **Expressions and Evaluation**: Argument *directions* and the body are retained — all the way
  through to `va-codegen` now (`va_ir::Function::arg_dirs`, one `ArgDir` per `args` entry,
  §6-revised into Interface α; `docs/interfaces.md`); argument/local *type* declarations
  (`real x;`) are parsed and discarded (v0 has no per-variable type distinction, as noted in
  §1.4). A call (`CallUser(FuncId, args)`) binds `args` positionally to the function's own
  private variable scope — it may read module parameters but not module analog variables, and a
  forward reference to a function defined later in the same file resolves as unknown (no
  forward-reference support, consistent with `aliasparam`/parameter handling). An `input`
  argument's caller-side expression is read in as the initial binding, same as any argument
  always was; an `output`/`inout` argument instead (or additionally) writes the function's
  *final* binding back into the caller's own variable once the call returns — a real idiom for a
  function that computes several results at once (`mvsg_cmc_*.va`'s `calc_iq`/`calc_capt`),
  which the LRM restricts to a plain-variable actual argument (never a general expression, since
  there'd be nowhere to write the result), enforced by `va-codegen` rather than at elaboration
  time.
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
  module instances* — module instantiation itself exists now (§ module instantiation), but only
  as a single, ungenerated `Item::Instance`; a module-item-level `generate`/`endgenerate`
  wrapping one (needed for a genvar-indexed *array* of instances) is not yet supported, so
  genvar support still covers only the "unroll analog code, indexing a signal vector" half of
  the LRM's full genvar story (see Part 1 §1.4 `Genvar`'s comparison note).

## 2.15 `generate`/`endgenerate` wrapper

- Covered fully in Part 1 §1.5 (`Generate`/`Endgenerate`). Grammar note: `parse_generate`
  collects statements until `endgenerate` and hands them back as a plain `Vec<Stmt>`, which
  `parse_stmt` wraps in `Stmt::Block` — structurally indistinguishable, after parsing, from a
  `begin...end` block containing the same statements. All of §2.14's real behavior triggers off
  the *inner* `for`'s genvar-ness, never off the presence or absence of this wrapper.

## 2.16 Case statement

- Covered fully in Part 1 §1.5 (`While`/`Repeat`/`For`/`Case`/`Endcase`/`Default`).

## 2.17 Access-function calls: `V(...)`/`I(...)` (electrical), `Temp(...)`/`Pwr(...)` (thermal), and beyond

- **Purpose and Static Nature**: Simulation-time — a probe (`Expr::Probe`, read) or contribution
  target (`Stmt::Contribute`, write) against a specific branch, re-evaluated every solve
  iteration.
- **Declaration and Assignment**: N/A (these are uses, not declarations) — but note none of
  `V`/`I`/`Temp`/`Pwr` are reserved words in this project (LRM §5.5: nature access-function
  names are the *discipline's*, not the language's, keywords). `V`/`I` are the electrical
  discipline's conventional potential/flow names; `Temp`/`Pwr` are the thermal discipline's —
  both pairs come from the standard `disciplines.vams` header nearly every real model includes,
  and stay recognized regardless of whether any discipline/nature block is ever parsed
  (`Parser::known_access`'s always-on baseline, seeded in `parse`). **Beyond that baseline**
  (§ module preamble discipline/nature parsing, § 1.5's `Discipline`/`Nature` entry): any access
  name a genuinely parsed `discipline` block binds — via `potential Nature;`/`flow Nature;`,
  where `Nature`'s own `access = Name;` attribute names the function — is recognized too,
  additively (`Parser::register_access`). Recognition is purely name-based, not checked against
  the discipline of the net the access is actually applied to (consistent with how `V`/`I` were
  already recognized regardless of a net's declared discipline) — so, matching real-world loose
  usage, an access name from one discipline's nature can be, and in practice sometimes is, used
  directly on an `electrical`-declared net. This was originally found and fixed (for just
  `Temp`/`Pwr`) against the broad `external/` corpus scan: about a dozen real models
  (`asmhemt.va`, `epfl_hemt.va`, `fbh_hbt-2_3.va`, `BSIM6.1.1.va`, `bsimbulk*.va`,
  `mvsg_cmc_*.va`, `vbic_1p3.va`, `hisimsotb.va`, …) contribute to a `thermal` branch via
  `Temp(dt) <+ ...;`/`Pwr(rth) <+ ...;`, which previously mis-parsed as a bare assignment target
  immediately followed by `(` — "expected `=`, found `(`".
- **Expressions and Evaluation**: `V(a)`/`Temp(a)` (implicit reference/ground terminal) or
  `V(a, b)`/`Temp(a, b)` (explicit two-terminal branch) or `V(name)`/`Temp(name)` where `name`
  is a `branch`-declared alias (and likewise for `I`/`Pwr`) — all resolve, via
  `resolve_branch`/`resolve_net_arg`, to the same interned `BranchId` a structurally-equivalent
  access would produce; the IR's `AccessKind::Potential`/`Flow` doesn't distinguish *which*
  surface spelling was used, since both pairs mean the same conserved-quantity role regardless
  of discipline. Each argument is a `NetArg` (§2.18), so any terminal may be a vector-net
  element (`V(bus[i], gnd)`).
- **Structural and Analog Usage**: Analog-block only.
- **Comparison with Traditional Constructs**: No general-purpose-language analogue for the
  *access-function* concept itself (a name whose meaning — voltage vs. current, or temperature
  vs. power — is bound to the discipline of the nets it's applied to, not to a fixed type
  signature). The closest structural parallel is operator overloading resolved by argument
  type — or, for the `V`-vs-`Temp` naming split specifically, function overloading by a
  caller-chosen "unit family" rather than by argument type.

## 2.17b Port-current probe (`I(<port>)`, LRM §3.12.1/§5.4.3, added 2026-07-09)

Real, normative Verilog-A grammar — initially miscategorized as "unclear if real" after
`external/hicumL0_v2p0p0.va` and 5 HICUM/L0 siblings failed to parse `IB = I(<b>);`, until
confirmed directly against `references/VAMS-LRM-2-4.pdf`. Distinct from an ordinary `I(a)`
branch access (§2.17): `I(<a>)` accesses the current flowing *into the module* through
declared port `a`, not a branch between two nets.

- **Purpose and Static Nature**: Simulation-time, like any other access function — but its
  *value* is derived, not probed directly from a single branch: the signed sum of every flow
  contribution made elsewhere in the same analog block to a branch touching the port's node.
- **Declaration and Assignment**: `port_probe_function_call ::= nature_access_function ( <
  analog_port_reference > )` — grammatically `name(<port>)`, where `name` is any recognized
  access function (§2.17) and `port` is a bare port identifier. Parsed by `Parser::
  parse_port_probe`, hooked into the same two call sites `Parser::parse_access` already had
  (a contribution target and a primary expression) by checking for `Token::Lt` immediately
  after `Token::LParen`. Two LRM constraints enforced structurally: **flow-only** — `V(<port>)`
  parses (the grammar doesn't distinguish access-function names) but is rejected at
  elaboration with a clear error, since the LRM states only a flow access function is valid
  here; **read-only** — the contribution-target call site rejects a `(<` lookahead as a *parse*
  error outright ("the port access function shall not be used on the left side of `<+`"), so
  `ExprAst::PortProbe` is never constructible as a `Stmt::Contribute` target at all.
- **Expressions and Evaluation**: `Elaborator::lower_port_probe` resolves `port` to its node
  (must be a declared port of *this* module; a vector port is a stated v1 limitation — scalar
  only, no corpus need found yet), then `Elaborator::collect_port_flow_contributions`
  recursively walks the already-lowered prefix of `self.out.analog` for every `Stmt::Contribute`
  of `AccessKind::Flow` whose branch touches that node. **Sign convention** (verified against
  the LRM's own diode worked example — a forward-biased `branch(a,c)` contributes positive
  current from anode `a` to cathode `c`, so the external circuit must *supply* that current at
  `a`): a branch where the port's node is the `p` terminal contributes `+value`; where it's `n`,
  `-value`. A contribution found inside an `if`/`else` is wrapped in a matching `Expr::Select`
  guard (nested one level per enclosing `if`), so a conditionally-made contribution only counts
  when its condition held — closes the exact HICUM idiom this construct was found from
  (`if (rbi >= `MIN_R) I(br_rbi) <+ ...;` before an operating-point block's `IB = I(<b>);`).
  Every term is summed into one `Expr::Binary(Add, ...)` chain; no qualifying contribution at
  all folds to `Expr::Const(0.0)` (an untouched port genuinely has zero current, not an error).
  **Limitation**: a qualifying contribution found inside a `case`/`for`/`while`/`repeat` is
  rejected with a clear "not yet supported" error (`Elaborator::branch_flow_touches` detects
  the case) rather than silently mis-summed or silently dropped — those need either a per-arm
  equality guard or genuine loop-carried accumulation, neither of which this fold attempts.
- **Structural and Analog Usage**: Analog-block only, same as any other access-function use.
  No `va-ir`/`va-abi` change needed at all — this folds entirely within `va-frontend` into
  ordinary `Expr::Binary`/`Expr::Select`/`Expr::Unary` nodes that already existed, mirroring how
  the runtime-indexed vector-net/array-variable fold (§2.18/§2.2b) needed no Interface α change
  either.
- **Comparison with Traditional Constructs**: No general-purpose-language analogue — it's a
  derived quantity computed from KCL over the module's own declared branches, closer to a SPICE
  simulator's post-solve "terminal current" report than to any ordinary language construct. The
  closest Verilog-A-internal parallel is `ddx(expr, probe)` (§4.5.13): both are access-function-
  adjacent operators whose value isn't a literal probe of one fixed unknown but a *derived*
  quantity computed from the surrounding circuit description.

## 2.18 Vector net declaration & indexed access (`bus[i]`)

- **Purpose and Static Nature**: The *declaration* is elaboration-only (interning nodes). An
  *access*'s index is either elaboration-time-constant/genvar-derived (LRM §5.5.2's baseline:
  "The index must be a constant expression, though it may include genvar variables") or a
  genuinely runtime expression (§ dynamic vector-net/array-variable indexing, fixed
  2026-07-02) — the access itself (which node's voltage/current) is always a simulation-time
  read/write regardless of which kind of index selected it.
- **Declaration and Assignment**: `electrical|thermal [msb:lsb] name;` (§2.2) declares the
  vector; `V(name[index_expr])` / `I(name[index_expr])` (or a bare `name[index_expr]` as either
  terminal of a two-terminal access) reads/writes one element. `NetArg { name, index:
  Vec<ExprRef> }` (0–2 entries) is the AST representation shared by both branch-declaration
  terminals and access-function arguments — a second entry addresses a § 2-D vector net (§2.2c,
  non-standard extension); see §2.2c for the dimension-count/dynamic-indexing rules that extend
  to it.
- **Expressions and Evaluation**: A constant/genvar index is evaluated by `const_eval_int`
  (requiring an exactly-integral result, within `1e-9`) and bounds-checked against the vector's
  declared `(lo, hi)` (`resolve_net_arg`, thinly wrapping `resolve_vector_node_at`) — an
  out-of-range or non-integral *constant* index is a hard elaboration error. A genuinely runtime
  index is detected up front (`dynamic_terminal_range`, probing `const_eval` without propagating
  its error) and handled entirely differently, since a `V(...)`/`I(...)` access ultimately
  resolves to one fixed `BranchId` and there is no way for a single branch to "be" a runtime
  choice among several: a probe **read** (`lower_probe_expr`) expands into a nested
  `Expr::Select` chain of `Expr::Probe`s, one arm per declared index of the vector, guarded by
  `index == k`; a **contribution target** (`unroll_indexed_contribute`) — which can't be an
  expression at all, since `Stmt::Contribute`'s target is a fixed `Access` — expands the same way
  into an if/else-if chain of `Stmt::Contribute`s instead. Both are structurally identical to
  §2.2b's array-variable expansion (same shared-`ExprId`, same `hi`-arm-is-the-fallback
  limitation for an out-of-range runtime index). Attempting to access a declared vector net
  *without* an index, or to index a net that was never declared as a vector, are both still
  separately rejected with a specific message.
- **Structural and Analog Usage**: Declaration is module-level; indexed access is analog-block
  only, whether behind a genvar-driven `for` (§2.14, constant path), a plain literal index like
  `V(bus[2])` (needs no genvar at all), or an ordinary runtime `for`/`while` (dynamic path).
- **Comparison with Traditional Constructs**: The constant/genvar path is a C array subscript
  restricted to a `constexpr`/genvar-derived index. The runtime path — like §2.2b's array
  variables — has no direct C analogue: it's a compile-time-unrolled chain over every
  statically-known index rather than a single indexed load, since this project's `NodeId` model
  has no runtime-indexable-storage concept to begin with.
- **Vector ports (fixed)**: a vector *net* also listed in the module's port list now resolves
  fully — `va_ir::Module::ports` is `Vec<Vec<NodeId>>` (an Interface α change, §6), one entry
  per declared port, holding all of a vector port's nodes (ascending index order) rather than
  just one. `resolve_ports` no longer special-cases this at all: it pushes `vec![id]` for a
  scalar port or the vector's full node list for a wide one, uniformly. `va-codegen` didn't
  actually read `Module.ports` in its real lowering path (only `module.nodes.len()` via
  `build_instance`'s `terminals` argument), so this was low-blast-radius — three test fixtures
  needed a one-line update, nothing else. Real corpus files exercise this directly
  (`external/verilogaLib-master/dac_16bit_ideal.va`/`adc_16bit_ideal.va`, both declaring a
  vector I/O port and both indexing it with a plain runtime `integer`, not a `genvar`) — both
  now elaborate cleanly end to end, since the runtime-indexing gap above closed.

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

## 2.25 Preamble discipline/nature block parsing

- Covered fully in Part 1 §1.5 (`Discipline`/`Nature`/`Enddiscipline`/`Endnature`) and §2.17
  (the access-function recognition it feeds). No longer a skip: `Parser::parse_preamble` (was
  `skip_preamble`) genuinely parses each block into `disciplines::{NatureDecl, DisciplineDecl}`,
  registered in `Parser::natures`/`Parser::disciplines`.

## 2.26 Math builtin call names (`floor`, `ceil`, `round`, `int`, `limexp`, and the rest)

- Now-reserved words, covered fully in Part 1 §1.5's "Math builtins" entry and §1.7's fix note.
  The parser-level point worth restating here: a builtin call reaches `call_builtin` through the
  same "an `Ident`/reserved word immediately followed by `(`, and not `V`/`I`, is a call" path
  as a user-defined function call — the parser does not distinguish "known builtin name" from
  "user function name" at all; that classification happens entirely in elaboration
  (`lower_expr`'s `ExprAst::Call` arm checks the user-function table first, falling back to
  `call_builtin`). This is unaffected by whether the name happens to be reserved — reservation
  only changes whether the *bare* (non-call) form is a legal identifier.
