//! Every diagnostic code the program can emit.
//!
//! A code is a letter and four characters. The letter is the kind:
//!
//! | Letter | Kind | Meaning |
//! |--------|------|---------|
//! | `E` | error | the input is wrong, or the run failed |
//! | `W` | warning | the run goes on, but something deserves a look |
//! | `R` | refusal | valid Verilog-A this simulator deliberately declines to run (CLAUDE.md §5) |
//! | `C` | communication | information a user may act on |
//!
//! Two decimal digits for the stage that raises it, the same for every letter, then two
//! hexadecimal digits for the number within the stage: `00` to `99` first, then `9A` to `FF`
//! (upper case). That is 256 codes per stage and kind; 100 were not enough for elaboration.
//! Sorted as text, the codes stay in numeric order (`E0399` < `E039A` < `E03A0`).
//!
//!
//! | Range | Stage |
//! |-------|-------|
//! | `00xx` | general; `E0000` is "an error with no code yet" |
//! | `01xx` | preprocessor |
//! | `02xx` | lexer and parser |
//! | `03xx` | elaboration |
//! | `04xx` | code generation |
//! | `05xx` | netlist |
//! | `06xx` | numerical (DC, transient, AC, noise) |
//! | `07xx` | files and the command line |
//!
//! **A code is permanent.** Once released it is never renumbered and never reused for a different
//! problem, because users search for it and other teams' text is keyed to it. A code whose problem
//! can no longer happen is retired: it stays here and in the catalogue, marked as retired.
//!
//! Adding a code: add the constant here and to [`ALL`], and add `messages/en/<CODE>.json`. The
//! `catalogue_is_consistent` test fails until both exist and agree.

use crate::Code;

// --- every code, sorted; one per messages/en file (keep this line) ---
/// This error has no code yet.
pub const E0000: Code = Code::new("E0000");

/// internal error: {detail}
pub const E0001: Code = Code::new("E0001");

/// undefined macro `{name}
pub const E0101: Code = Code::new("E0101");

/// macro `{name} expects arguments
pub const E0102: Code = Code::new("E0102");

/// macro `{name}: expected {expected} argument(s), got {got}
pub const E0103: Code = Code::new("E0103");

/// macro expansion too deep (recursive macro?)
pub const E0104: Code = Code::new("E0104");

/// stray ` in source
pub const E0105: Code = Code::new("E0105");

/// unbalanced `(` in macro/argument list
pub const E0106: Code = Code::new("E0106");

/// `define missing a macro name
pub const E0107: Code = Code::new("E0107");

/// cannot predefine macro `{name}`: a macro name is a letter or `_` followed by letters, digits, `_` or `$`
pub const E0108: Code = Code::new("E0108");

/// unterminated `ifdef/`ifndef (missing `endif)
pub const E0109: Code = Code::new("E0109");

/// `elsif without `ifdef
pub const E0110: Code = Code::new("E0110");

/// `else without `ifdef
pub const E0111: Code = Code::new("E0111");

/// `endif without `ifdef
pub const E0112: Code = Code::new("E0112");

/// `include expects a quoted file name
pub const E0113: Code = Code::new("E0113");

/// `include cycle on {path}
pub const E0114: Code = Code::new("E0114");

/// reading `include {path}: {reason}
pub const E0115: Code = Code::new("E0115");

/// unterminated `begin_keywords (missing `end_keywords)
pub const E0116: Code = Code::new("E0116");

/// `end_keywords without a matching `begin_keywords
pub const E0117: Code = Code::new("E0117");

/// `begin_keywords expects a quoted version specifier
pub const E0118: Code = Code::new("E0118");

/// `begin_keywords "{spec}": unknown version specifier; the LRM (10.6) defines "VAMS-2.3", "1364-2005", "1364-2001" and "1364-1995"
pub const E0119: Code = Code::new("E0119");

/// `default_nodetype is not a Verilog-AMS 2.4 directive (an obsolete AMS-1.x spelling); write `default_discipline <discipline> instead
pub const E0120: Code = Code::new("E0120");

/// `default_discipline {name} {qualifier}: a qualifier is not allowed in Verilog-A
pub const E0121: Code = Code::new("E0121");

/// `default_transition expects a numeric literal (SI suffix allowed, e.g. `default_transition 1n), got `{text}`; an expression here is not supported
pub const E0122: Code = Code::new("E0122");

/// `default_transition {text}: a transition time must be non-negative and finite
pub const E0123: Code = Code::new("E0123");

/// `line expects a line number, a quoted file name and a level
pub const E0124: Code = Code::new("E0124");

/// `line expects a quoted file name after the line number
pub const E0125: Code = Code::new("E0125");

/// this `/*` comment is never closed by `*/`, so everything after it is commented out
pub const E0126: Code = Code::new("E0126");

/// this string literal is never closed: a string must end with `"` on the line it starts
pub const E0127: Code = Code::new("E0127");

/// unexpected character `{char}`
pub const E0201: Code = Code::new("E0201");

/// expected {expected}, found {found}
pub const E0202: Code = Code::new("E0202");

/// expected `;` after {after}, found {found}
pub const E0203: Code = Code::new("E0203");

/// expected `end` to close the `begin` opened at {opened}, found {found}
pub const E0204: Code = Code::new("E0204");

/// expected `endmodule` to close module `{name}` opened at {opened}, found {found}
pub const E0205: Code = Code::new("E0205");

/// expected a statement, found {found}
pub const E0206: Code = Code::new("E0206");

/// expected an expression, found {found}
pub const E0207: Code = Code::new("E0207");

/// expected an identifier, found {found}
pub const E0208: Code = Code::new("E0208");

/// expected an identifier, but `{word}` is a reserved word
pub const E0209: Code = Code::new("E0209");

/// expected a declaration or an `analog` block, found {found}
pub const E0210: Code = Code::new("E0210");

/// expected a string literal, found {found}
pub const E0211: Code = Code::new("E0211");

/// expected an access function, found {found}
pub const E0212: Code = Code::new("E0212");

/// `{name}` is not a recognized access function
pub const E0213: Code = Code::new("E0213");

/// reserved word `{word}` is not valid in an expression
pub const E0214: Code = Code::new("E0214");

/// `{op}` is not part of Verilog-A: Annex C excludes the case-equality operators, which compare four-state logic values (`x`, `z`) that an analog value cannot hold; use `{instead}`
pub const E0215: Code = Code::new("E0215");

/// `{word}` is not part of Verilog-A: Annex C excludes it, because its don't-care matching works on four-state logic values (`x`, `z`) that an analog value cannot hold; use `case`
pub const E0216: Code = Code::new("E0216");

/// `{discipline} {name}` reads as a net declaration, but no discipline `{discipline}` is declared (declare it in a `discipline` block, or `` `include `` the file that does)
pub const E0217: Code = Code::new("E0217");

/// `{discipline} {name}` reads as a net declaration, but no discipline `{discipline}` is declared; did you mean `{suggestion}`?
pub const E0218: Code = Code::new("E0218");

/// expected an event in the trigger list, found {found}
pub const E0219: Code = Code::new("E0219");

/// empty event trigger
pub const E0220: Code = Code::new("E0220");

/// unterminated `@(...)` event expression
pub const E0221: Code = Code::new("E0221");

/// `{name}[..]`: a `[lo:hi]` slice must be the final bracket group
pub const E0222: Code = Code::new("E0222");

/// `{name}` has more than 2 bracket groups; indexing/slicing is capped at 2 dimensions
pub const E0223: Code = Code::new("E0223");

/// indexing is capped at 2 dimensions
pub const E0224: Code = Code::new("E0224");

/// more than 2 declared dimensions; capped at 2
pub const E0225: Code = Code::new("E0225");

/// `{net_type}` is a discrete-domain net type, which LRM Annex C excludes from Verilog-A, so it may not qualify the port declaration `{dir} {net_type} ...`
pub const E0226: Code = Code::new("E0226");

/// `nature ... : {head}`: `{head}` is not a previously declared nature
pub const E0227: Code = Code::new("E0227");

/// `nature ... : {head}.`: expected `potential` or `flow` after a discipline name
pub const E0228: Code = Code::new("E0228");

/// `nature ... : {head}.{which}`: `{head}` is not a previously declared discipline
pub const E0229: Code = Code::new("E0229");

/// `nature ... : {head}.{which}`: discipline `{head}` binds no `{which}` nature
pub const E0230: Code = Code::new("E0230");

/// `nature ... : {head}.{which}`: `{head}`'s {which} nature `{nature}` is not a previously declared nature
pub const E0231: Code = Code::new("E0231");

/// nature `{name}` derives from `{parent}` and so always inherits its `units` ({units}), which LRM §3.6.1.2 forbids it from defining or changing to "{new_units}"
pub const E0232: Code = Code::new("E0232");

/// nature `{name}` derives from `{parent}` and so always inherits its `access` function ({access}), which LRM §3.6.1.2 forbids it from changing to `{new_access}`
pub const E0233: Code = Code::new("E0233");

/// expected a `nature` attribute or `endnature`, found {found}
pub const E0234: Code = Code::new("E0234");

/// `.{port}(...)`: a port connects to a net, not to a value
pub const E0235: Code = Code::new("E0235");

/// `{name}(<...>)` (a port-current probe) may not be used on the left side of `<+` — it is read-only (LRM §5.4.3)
pub const E0236: Code = Code::new("E0236");

/// expected a `discipline` attribute or `enddiscipline`, found {found}
pub const E0237: Code = Code::new("E0237");

/// `{name}` is declared both as a parameter and as a variable in this module; rename one of them
pub const E0301: Code = Code::new("E0301");

/// variable `{name}` is declared more than once in this module; remove one of the declarations
pub const E0302: Code = Code::new("E0302");

/// analog function `{function}` assigns `{name}`, which is neither its name, one of its arguments, nor declared inside it; declare it with `real {name};`
pub const E0303: Code = Code::new("E0303");

/// `ground {name}`: `{name}` is not a previously declared net
pub const E0304: Code = Code::new("E0304");

/// port `{port}` is a {dims}-D vector net (§ 2-D vector net extension); only a 1-D vector net may be used as a port
pub const E0305: Code = Code::new("E0305");

/// port `{port}` has no discipline declaration (e.g. `electrical {port};`)
pub const E0306: Code = Code::new("E0306");

/// parameter `{name}` is declared more than once in this module; remove or rename one of the declarations
pub const E0307: Code = Code::new("E0307");

/// parameter `{name}` evaluates to {value}, not a finite number (a division by zero in its default expression?)
pub const E0308: Code = Code::new("E0308");

/// parameter `{name}` value {value} is below its declared minimum {min}
pub const E0309: Code = Code::new("E0309");

/// parameter `{name}` value {value} is above its declared maximum {max}
pub const E0310: Code = Code::new("E0310");

/// aliasparam `{name}` reuses the name of a parameter already declared in this module
pub const E0311: Code = Code::new("E0311");

/// aliasparam `{name}` targets unknown parameter `{target}`
pub const E0312: Code = Code::new("E0312");

/// `{name}` is not a compile-time constant in this context
pub const E0313: Code = Code::new("E0313");

/// `$simparam` without a default value cannot be evaluated in a parameter context
pub const E0314: Code = Code::new("E0314");

/// system function `${name}` is not constant in a parameter context
pub const E0315: Code = Code::new("E0315");

/// a string literal is not valid in a parameter context
pub const E0316: Code = Code::new("E0316");

/// a probe is not constant in a parameter context
pub const E0317: Code = Code::new("E0317");

/// array variable `{name}` is not constant in a parameter context
pub const E0318: Code = Code::new("E0318");

/// an array-literal expression is not constant in a parameter context (only valid as a Laplace/Z-domain filter argument)
pub const E0319: Code = Code::new("E0319");

/// the {list} argument must be a `{...}` array-literal coefficient/root list
pub const E0320: Code = Code::new("E0320");

/// `{name}` with a file-name argument is not supported — give the table inline as a `{f1, p1, f2, p2, …}` array literal
pub const E0321: Code = Code::new("E0321");

/// `{name}`'s table must be a `{...}` array-literal coefficient/root list
pub const E0322: Code = Code::new("E0322");

/// `{name}`'s table must hold `(frequency, power)` pairs — got {count} values (odd)
pub const E0323: Code = Code::new("E0323");

/// `{name}` frequency {frequency} is not a finite, non-negative frequency in Hz
pub const E0324: Code = Code::new("E0324");

/// `{name}` power at {frequency} Hz must be a constant expression (parameters, literals and builtin functions): {error}
pub const E0325: Code = Code::new("E0325");

/// `{name}` power {power} at {frequency} Hz is not a finite, non-negative power spectral density
pub const E0326: Code = Code::new("E0326");

/// `{name}` repeats the frequency {frequency} Hz — the LRM requires each frequency in a table to be unique
pub const E0327: Code = Code::new("E0327");

/// {what} must be an integer, got {value}
pub const E0328: Code = Code::new("E0328");

/// `{name}` is a parameter, and a parameter cannot be assigned in the analog block; declare a variable (`real {name}_eff;`) and assign that instead
pub const E0329: Code = Code::new("E0329");

/// variable `{name}` is assigned but never declared; declare it with `real {name};` (or `integer {name};`) at module scope or at the top of the enclosing named block
pub const E0330: Code = Code::new("E0330");

/// array variable `{name}` must be declared at module scope, not inside the analog block (block-local array variables are not yet supported)
pub const E0331: Code = Code::new("E0331");

/// `bound_step` is a statement, not a value: write `bound_step(expr);` on its own rather than inside an expression
pub const E0332: Code = Code::new("E0332");

/// `{name}{signature}` takes {expected} argument(s), but this call passes {got}
pub const E0333: Code = Code::new("E0333");

/// `{name}{signature}` takes {min} to {max} arguments, but this call passes {got}
pub const E0334: Code = Code::new("E0334");

/// `{name}{signature}` takes at least {min} argument(s), but this call passes {got}
pub const E0335: Code = Code::new("E0335");

/// wrong argument count for `{name}` in constant context
pub const E0336: Code = Code::new("E0336");

/// genvar `{name}` may only be assigned by the step of the `for` loop it drives (restricted assignment)
pub const E0337: Code = Code::new("E0337");

/// nested generate loop reuses genvar `{name}`; a genvar's implicit localparam cannot be redeclared while its enclosing loop is still active
pub const E0338: Code = Code::new("E0338");

/// generate loop over genvar `{name}` did not terminate within {limit} iterations
pub const E0339: Code = Code::new("E0339");

/// genvar `{name}` is read outside the `for` loop that drives it; a genvar only has a value inside its own generate loop
pub const E0340: Code = Code::new("E0340");

/// unknown identifier `{name}`
pub const E0341: Code = Code::new("E0341");

/// unknown function `{name}`
pub const E0342: Code = Code::new("E0342");

/// unknown system function `${name}`
pub const E0343: Code = Code::new("E0343");

/// `{name}` is not constant-evaluable
pub const E0344: Code = Code::new("E0344");

/// `$fopen` takes string literals: a file name, and optionally a type, as `$fopen("name", "w")`
pub const E0345: Code = Code::new("E0345");

/// `$fopen` type "{mode}" is not one of the LRM's (Table 9-24): "w", "a", "r" and their "b"/"+" forms
pub const E0346: Code = Code::new("E0346");

/// `$table_model`'s third argument is the control string and must be a string literal, as `"1CC"`
pub const E0347: Code = Code::new("E0347");

/// `$simparam` takes a string parameter name as its first argument (LRM §9.18)
pub const E0348: Code = Code::new("E0348");

/// `{name}`'s argument must be a bare name, as `{name}(x)`
pub const E0349: Code = Code::new("E0349");

/// `$param_given` names `{name}`, which is not a declared parameter of this module
pub const E0350: Code = Code::new("E0350");

/// `$port_connected` names `{name}`, which is not a declared port of this module
pub const E0351: Code = Code::new("E0351");

/// a string literal is only valid as a system-task argument
pub const E0352: Code = Code::new("E0352");

/// `cross`'s direction argument must be -1 (falling), 0 (either) or +1 (rising), got {dir}
pub const E0353: Code = Code::new("E0353");

/// `{name}` needs at least one numerator/zero and one denominator/pole entry
pub const E0354: Code = Code::new("E0354");

/// `{name}`'s zero/pole array must hold an even number of values — they are flattened `(real, imaginary)` root pairs
pub const E0355: Code = Code::new("E0355");

/// an array-literal expression (`{...}`) is only valid as a Laplace/Z-domain filter's numerator/zero/denominator/pole argument (§4.5.11/§4.5.12), not as a general-purpose value
pub const E0356: Code = Code::new("E0356");

/// `ddx(..., I(...))` is not supported: differentiating with respect to a branch current needs flow probes to be independent unknowns, which they are not in this codegen
pub const E0357: Code = Code::new("E0357");

/// `ddx`'s second argument must be a potential-probe access, e.g. `V(p, n)`
pub const E0358: Code = Code::new("E0358");

/// `{name}` analysis-name arguments must be string literals
pub const E0359: Code = Code::new("E0359");

/// `{name}`: `{analysis}` is not a Verilog-A analysis name (expected one of {expected})
pub const E0360: Code = Code::new("E0360");

/// `{func}(...)` is not an access function of discipline `{discipline}`, which net `{net}` is declared with: its access functions are {defined}; write `{expected}(...)` for its {quantity}
pub const E0361: Code = Code::new("E0361");

/// `{name}` has two dynamically-indexed dimensions in the same access; at most one index position may be a genuinely runtime expression
pub const E0362: Code = Code::new("E0362");

/// `{name}[..]`: slicing is single-dimension-only, so a `[lo:hi]` slice cannot be combined with an index or applied to a 2-D vector net
pub const E0363: Code = Code::new("E0363");

/// `{name}[..]` is a vector slice; a branch access/declaration needs a single node, e.g. `V({name}[0])` (a slice is only valid as an instance port-connection argument)
pub const E0364: Code = Code::new("E0364");

/// `{name}` is a {dims}-D vector net; an access must index every dimension, e.g. {example}
pub const E0365: Code = Code::new("E0365");

/// unknown net `{name}`
pub const E0366: Code = Code::new("E0366");

/// `{name}` is indexed, but it is declared without a bracketed `[msb:lsb]` range
pub const E0367: Code = Code::new("E0367");

/// `{name}` is declared with {declared} dimension(s) but accessed with {used}
pub const E0368: Code = Code::new("E0368");

/// index {index} is out of `{name}`'s declared dimension {dim} range [{lo}:{hi}]
pub const E0369: Code = Code::new("E0369");

/// `{name}` is a {dims}-D vector net; a bare (unindexed) port connection is only supported for a 1-D vector net
pub const E0370: Code = Code::new("E0370");

/// `V(<{port}>)` is invalid — a port-current probe is a flow access only (LRM §5.4.3); use `I(<{port}>)`
pub const E0371: Code = Code::new("E0371");

/// `{port}` is not a declared port of this module; a port-current probe can only name one of this module's own ports
pub const E0372: Code = Code::new("E0372");

/// `{port}` is a {dims}-D vector port; port-current probes are only supported for a scalar port (v1 limitation)
pub const E0373: Code = Code::new("E0373");

/// `I(<{port}>)` can't sum a flow contribution whose enclosing `if` condition depends on `ddt` ({via})
pub const E0374: Code = Code::new("E0374");

/// a port-current probe (§ port-current probe) can't sum a flow contribution made inside a {construct} — not yet supported
pub const E0375: Code = Code::new("E0375");

/// `{name}` inside a `{keyword}` loop: LRM §4.5.15 does not allow analog operators in `repeat`, `while` or non-genvar `for` loops
pub const E0376: Code = Code::new("E0376");

/// a monitored event trigger (`@(cross(…))`/`@(above(…))`/`@(timer(…))`) inside a `{keyword}` loop: LRM §5.9 does not allow event control statements in `repeat`, `while` or non-genvar `for` loops
pub const E0377: Code = Code::new("E0377");

/// duplicate instance name `{name}`
pub const E0378: Code = Code::new("E0378");

/// instantiation cycle: `{module}` (instantiated as `{instance}` inside `{parent}`) already appears in the elaboration chain: {chain}
pub const E0379: Code = Code::new("E0379");

/// instance `{instance}` references unknown module `{module}` (no sibling `module {module} ... endmodule` in this compilation unit)
pub const E0380: Code = Code::new("E0380");

/// instance `{instance}` overrides unknown parameter `{param}` of module `{module}`
pub const E0381: Code = Code::new("E0381");

/// instance `{instance}` overrides `{param}`, which module `{module}` declares as a `localparam` — a localparam is a module-internal constant and cannot be overridden
pub const E0382: Code = Code::new("E0382");

/// instance `{instance}` of `{module}` connects {got} port(s), but the module declares {expected}
pub const E0383: Code = Code::new("E0383");

/// instance `{instance}`: cannot mix positional and named port connections
pub const E0384: Code = Code::new("E0384");

/// instance `{instance}` of `{module}`: no port named `{port}`
pub const E0385: Code = Code::new("E0385");

/// instance `{instance}` of `{module}`: port `{port}` connected more than once
pub const E0386: Code = Code::new("E0386");

/// instance `{instance}` of `{module}`: port `{port}` is `{port_discipline}` but is connected to net `{net}`, which is `{net_discipline}`
pub const E0387: Code = Code::new("E0387");

/// instance `{instance}` of `{module}`: port `{port}` is {port_width}-wide but the connection is {net_width}-wide
pub const E0388: Code = Code::new("E0388");

/// `{char}` is not a $table_model extrapolation character; the LRM's Table 9-31 has `C` (constant), `L` (linear) and `E` (error)
pub const E0389: Code = Code::new("E0389");

/// $table_model control string `{control}` has more than the three characters LRM §9.21.2 allows in one dimension
pub const E0390: Code = Code::new("E0390");

/// $table_model cannot read `{file}`; looked in {tried}
pub const E0391: Code = Code::new("E0391");

/// {file}:{line}: `{text}` is not an `x y` pair of numbers
pub const E0392: Code = Code::new("E0392");

/// {file}:{line}: {columns} columns, but only a single dependent column is implemented — the LRM's `;N` selector that would choose among them is refused
pub const E0393: Code = Code::new("E0393");

/// {file}: two points at x = {x} with different values ({y1} and {y2}); a table cannot have two answers at one point (LRM §9.21.1)
pub const E0394: Code = Code::new("E0394");

/// {file}: a $table_model table needs at least two distinct points, found {count}
pub const E0395: Code = Code::new("E0395");

/// bound_step is not allowed inside an analog function body
pub const E0401: Code = Code::new("E0401");

/// a `<+` contribution is not allowed inside an analog function body
pub const E0402: Code = Code::new("E0402");

/// function `{name}`'s output/inout argument #{index} must be a plain variable
pub const E0403: Code = Code::new("E0403");

/// noise_table power {power} at {frequency} Hz is not a finite, non-negative power spectral density with this instance's parameters
pub const E0404: Code = Code::new("E0404");

/// a zi_* sample period must be positive and finite, got {value}
pub const E0405: Code = Code::new("E0405");

/// a zi_* transition time must be non-negative and finite, got {value}
pub const E0406: Code = Code::new("E0406");

/// a {filter} coefficient does not evaluate to a number
pub const E0407: Code = Code::new("E0407");

/// a {filter} coefficient is not finite
pub const E0408: Code = Code::new("E0408");

/// a laplace denominator is identically zero
pub const E0409: Code = Code::new("E0409");

/// {filter} zeros/poles must come in complex-conjugate pairs: their expanded polynomial is not real
pub const E0410: Code = Code::new("E0410");

/// a zi_* denominator has d_0 = 0: the difference equation cannot be solved for the current output sample
pub const E0411: Code = Code::new("E0411");

/// a zi_* filter with {count} more zero(s) at the origin than poles at the origin is an advance of {count} sample(s), which no causal filter can produce (LRM 4.5.12)
pub const E0412: Code = Code::new("E0412");

/// variable `{name}` is read before it is assigned
pub const E0413: Code = Code::new("E0413");

/// cannot read netlist `{path}`: {reason}
pub const E0501: Code = Code::new("E0501");

/// unexpected token `{token}`: a model-referencing device takes only `name=value` parameter overrides after its model name
pub const E0502: Code = Code::new("E0502");

/// bad value `{value}` for parameter `{name}`
pub const E0503: Code = Code::new("E0503");

/// parameter override `{token}` has no name
pub const E0504: Code = Code::new("E0504");

/// `{name}` needs at least {need} tokens, found {got}
pub const E0505: Code = Code::new("E0505");

/// bad value `{value}`
pub const E0506: Code = Code::new("E0506");

/// unexpected token `{token}` after the value
pub const E0507: Code = Code::new("E0507");

/// `IC=` is only meaningful on a reactive element, not on `{name}`
pub const E0508: Code = Code::new("E0508");

/// bad `IC=` value `{value}`
pub const E0509: Code = Code::new("E0509");

/// `{name}` names a model but connects no nodes (expected `X<name> <node>... <model>`)
pub const E0510: Code = Code::new("E0510");

/// bad coupling `{value}`
pub const E0511: Code = Code::new("E0511");

/// coupling `{value}` is outside [-1, 1]
pub const E0512: Code = Code::new("E0512");

/// unsupported element `{name}`
pub const E0513: Code = Code::new("E0513");

/// `.subckt` inside the definition of `{outer}`: nested subcircuit definitions are not supported
pub const E0514: Code = Code::new("E0514");

/// `.ends` with no open `.subckt`
pub const E0515: Code = Code::new("E0515");

/// subcircuit `{name}` is defined twice
pub const E0516: Code = Code::new("E0516");

/// `.subckt {name}` is never closed by `.ends`
pub const E0517: Code = Code::new("E0517");

/// a `+` continuation line with no line before it to continue
pub const E0518: Code = Code::new("E0518");

/// `.include` names no file
pub const E0519: Code = Code::new("E0519");

/// `.include {file}` needs the deck's own location to resolve against
pub const E0520: Code = Code::new("E0520");

/// `.include` nested more than {limit} deep (a cycle?)
pub const E0521: Code = Code::new("E0521");

/// cannot read `.include {file}` ({path}): {reason}
pub const E0522: Code = Code::new("E0522");

/// `.control` is never closed by `.endc`
pub const E0523: Code = Code::new("E0523");

/// `.subckt` names no subcircuit
pub const E0524: Code = Code::new("E0524");

/// `.subckt {name}` declares a parameter (`{param}`): subcircuit parameters are not supported
pub const E0525: Code = Code::new("E0525");

/// `.model` needs a name and a type (`.model <name> <module> …`)
pub const E0526: Code = Code::new("E0526");

/// `.model {name}`: expected `name=value`, found `{token}`
pub const E0527: Code = Code::new("E0527");

/// `.model {name}`: bad value `{value}` for `{key}`
pub const E0528: Code = Code::new("E0528");

/// `{card}` inside a subcircuit body is not supported
pub const E0529: Code = Code::new("E0529");

/// `{instance}` passes parameters to subcircuit `{name}`: subcircuit parameters are not supported
pub const E0530: Code = Code::new("E0530");

/// `{instance}` connects {got} net(s) but subcircuit `{name}` has {expected} port(s) ({ports})
pub const E0531: Code = Code::new("E0531");

/// subcircuit `{name}` instantiates itself (through {chain})
pub const E0532: Code = Code::new("E0532");

/// subcircuits nested more than {limit} deep
pub const E0533: Code = Code::new("E0533");

/// unknown subcommand `{name}`
pub const E0701: Code = Code::new("E0701");

/// `check` needs at least one model file or directory
pub const E0702: Code = Code::new("E0702");

/// invalid value `{value}` for {option}; expected {expected}
pub const E0703: Code = Code::new("E0703");

/// `pragma protect`, an encrypted model envelope
pub const R0101: Code = Code::new("R0101");

/// `@(... {event} ...)`, an event trigger this engine does not schedule
pub const R0201: Code = Code::new("R0201");

/// `@({event} ...)` in a form this engine does not honour
pub const R0202: Code = Code::new("R0202");

/// the reserved word `{word}`
pub const R0203: Code = Code::new("R0203");

/// `$fopen("{name}", "{mode}")`, a file opened for reading
pub const R0301: Code = Code::new("R0301");

/// `$table_model` with an array data source
pub const R0302: Code = Code::new("R0302");

/// `$table_model` with {count} lookup expressions, i.e. a {count}-dimensional table
pub const R0303: Code = Code::new("R0303");

/// `$simparam("{name}")`, with no default value
pub const R0304: Code = Code::new("R0304");

/// `$fopen(...)` used inside an expression
pub const R0305: Code = Code::new("R0305");

/// `$table_model` control string `{control}`, which selects a dependent column
pub const R0306: Code = Code::new("R0306");

/// `$table_model` control string `{control}`, which describes more than one dimension
pub const R0307: Code = Code::new("R0307");

/// `$table_model` interpolation `{char}` (a spline)
pub const R0308: Code = Code::new("R0308");

/// `$table_model` interpolation `{char}` (ignore this column)
pub const R0309: Code = Code::new("R0309");

/// `$table_model` extrapolation `E`
pub const R0310: Code = Code::new("R0310");

/// `ddt` of a value that already depends on a time derivative is a second time derivative
pub const R0401: Code = Code::new("R0401");

/// `{operator}` must be a top-level additive term of a contribution
pub const R0402: Code = Code::new("R0402");

/// white_noise/flicker_noise must be a top-level additive term of a contribution
pub const R0403: Code = Code::new("R0403");

/// ac_stim must be a top-level additive term of a contribution
pub const R0404: Code = Code::new("R0404");

/// flow probe `I(...)` is only supported for a branch that also receives a potential contribution somewhere in the module
pub const R0405: Code = Code::new("R0405");

/// a bare `I(...)` probe of a branch that receives no contribution anywhere is only supported when one terminal is the module's implicit ground reference
pub const R0406: Code = Code::new("R0406");

/// a ddt assigned to a variable inside an if/case/loop arm and contributed after that arm needs a controlling condition that is constant for the whole run (LRM 4.5.15)
pub const R0407: Code = Code::new("R0407");

/// a loop did not terminate within {limit} iterations
pub const R0408: Code = Code::new("R0408");

/// a laplace denominator has d_0 = 0 (a pole at s = 0): the filter integrates its input and has no DC operating point
pub const R0409: Code = Code::new("R0409");

/// a laplace numerator of degree {num_degree} over a denominator of degree {den_degree} is an improper filter
pub const R0410: Code = Code::new("R0410");

/// `timescale has no effect in Verilog-A
pub const W0101: Code = Code::new("W0101");

/// `celldefine has no effect in Verilog-A
pub const W0102: Code = Code::new("W0102");

/// `default_nettype has no effect in Verilog-A
pub const W0103: Code = Code::new("W0103");

/// `unconnected_drive has no effect in Verilog-A
pub const W0104: Code = Code::new("W0104");

/// module `{module}`: port `{port}` has no direction; it is treated as `inout`
pub const W0301: Code = Code::new("W0301");

/// `.nodeset` entry `{entry}` is not `V(<net>)=<value>`; skipped
pub const W0501: Code = Code::new("W0501");

/// ignored the ngspice `.control` block at lines {first}–{last}
pub const W0502: Code = Code::new("W0502");

/// not applied: `{card}`
pub const W0503: Code = Code::new("W0503");

/// Every code, so the catalogue and the code can be checked against each other.
pub const ALL: &[Code] = &[
    E0000, E0001, E0101, E0102, E0103, E0104, E0105, E0106, E0107, E0108, E0109, E0110, E0111,
    E0112, E0113, E0114, E0115, E0116, E0117, E0118, E0119, E0120, E0121, E0122, E0123, E0124,
    E0125, E0126, E0127, E0201, E0202, E0203, E0204, E0205, E0206, E0207, E0208, E0209, E0210,
    E0211, E0212, E0213, E0214, E0215, E0216, E0217, E0218, E0219, E0220, E0221, E0222, E0223,
    E0224, E0225, E0226, E0227, E0228, E0229, E0230, E0231, E0232, E0233, E0234, E0235, E0236,
    E0237, E0301, E0302, E0303, E0304, E0305, E0306, E0307, E0308, E0309, E0310, E0311, E0312,
    E0313, E0314, E0315, E0316, E0317, E0318, E0319, E0320, E0321, E0322, E0323, E0324, E0325,
    E0326, E0327, E0328, E0329, E0330, E0331, E0332, E0333, E0334, E0335, E0336, E0337, E0338,
    E0339, E0340, E0341, E0342, E0343, E0344, E0345, E0346, E0347, E0348, E0349, E0350, E0351,
    E0352, E0353, E0354, E0355, E0356, E0357, E0358, E0359, E0360, E0361, E0362, E0363, E0364,
    E0365, E0366, E0367, E0368, E0369, E0370, E0371, E0372, E0373, E0374, E0375, E0376, E0377,
    E0378, E0379, E0380, E0381, E0382, E0383, E0384, E0385, E0386, E0387, E0388, E0389, E0390,
    E0391, E0392, E0393, E0394, E0395, E0401, E0402, E0403, E0404, E0405, E0406, E0407, E0408,
    E0409, E0410, E0411, E0412, E0413, E0501, E0502, E0503, E0504, E0505, E0506, E0507, E0508,
    E0509, E0510, E0511, E0512, E0513, E0514, E0515, E0516, E0517, E0518, E0519, E0520, E0521,
    E0522, E0523, E0524, E0525, E0526, E0527, E0528, E0529, E0530, E0531, E0532, E0533, E0701,
    E0702, E0703, R0101, R0201, R0202, R0203, R0301, R0302, R0303, R0304, R0305, R0306, R0307,
    R0308, R0309, R0310, R0401, R0402, R0403, R0404, R0405, R0406, R0407, R0408, R0409, R0410,
    W0101, W0102, W0103, W0104, W0301, W0501, W0502, W0503,
];
