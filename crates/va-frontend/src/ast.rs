//! Surface AST: the parser's output, before elaboration into [`va_ir`].
//!
//! The AST is a faithful, sugar-preserving tree of one `module` — [`crate::parser::parse`]
//! returns one [`ModuleAst`] per `module...endmodule` a source unit defines, since a file may
//! define several that reference each other via [`Item::Instance`] (§ module instantiation).
//! Elaboration ([`crate::elaborate`]) resolves names, assigns IR arena indices, recursively
//! inlines any instantiated submodule, and lowers the result into the frozen IR (Interface α).
//!
//! # Representation (§5)
//!
//! Like the IR, expressions are stored in an arena ([`ModuleAst::exprs`]) and referenced by
//! the `Copy` handle [`ExprRef`] — no `Box`-graph or lifetime-threaded trees. Statements
//! nest via owned `Vec<Stmt>`, mirroring [`va_ir::Stmt`].

/// Index of an expression within [`ModuleAst::exprs`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExprRef(pub u32);

/// A parsed Verilog-A module (surface syntax).
#[derive(Clone, Debug, Default)]
pub struct ModuleAst {
    /// Module name.
    pub name: String,
    /// Port names in declaration order.
    pub ports: Vec<String>,
    /// Declarations and the analog block, in source order.
    pub items: Vec<Item>,
    /// Expression arena; [`ExprRef`]s index into this `Vec`.
    pub exprs: Vec<ExprAst>,
}

impl ModuleAst {
    /// Borrow an expression by handle.
    ///
    /// # Panics
    ///
    /// Panics if `r` was not produced for this module (an internal invariant violation),
    /// mirroring [`va_ir::Module::expr`].
    pub fn expr(&self, r: ExprRef) -> &ExprAst {
        &self.exprs[r.0 as usize]
    }
}

/// A port/net direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// `input`.
    Input,
    /// `output`.
    Output,
    /// `inout`.
    Inout,
}

/// A net discipline keyword.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Discipline {
    /// `electrical`.
    Electrical,
    /// `thermal`.
    Thermal,
    /// A user-defined discipline (`discipline foo; ... enddiscipline`) used as a net-type
    /// keyword, e.g. `foo a, b;`. `va-core` doesn't model multi-physics beyond
    /// electrical/thermal yet, so this elaborates to [`va_ir::Discipline::Other`] (§1 roadmap)
    /// — the node still exists and can be probed/contributed to, it just isn't checked for
    /// domain-specific conservation.
    Custom(String),
}

/// A parameter's declared base type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamType {
    /// `real`.
    Real,
    /// `integer`.
    Integer,
}

/// The kind of branch access: `V(...)` (potential) or `I(...)` (flow).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccessKind {
    /// Potential access, `V(a[, b])`.
    Potential,
    /// Flow access, `I(a[, b])`.
    Flow,
}

/// A branch access: an access function applied to one or two net terminals.
#[derive(Clone, Debug, PartialEq)]
pub struct Access {
    /// Potential or flow.
    pub kind: AccessKind,
    /// One net (node-to-reference) or two (explicit branch terminals).
    pub args: Vec<NetArg>,
}

/// A single net terminal in an [`Access`], `branch` declaration, or instance port connection
/// ([`PortConn`]): a plain net name, a vector element selected by one or two bracketed indices
/// (`bus[i]`, or `grid[i][j]` for a § 2-D vector net), or (connections only) a bracketed slice
/// (`bus[3:0]`) wiring a sub-range of a wider vector net to a vector port. Each index, when
/// present, must be a genvar or compile-time-constant expression — resolved at elaboration
/// (§ vector nets) — except in an `Access`/`Stmt::Contribute` terminal, where at most one of
/// `index`'s (up to 2) entries may instead be a genuinely runtime expression (§ dynamic
/// vector-net indexing). `index` and `slice` are mutually exclusive except for the `[i][lo:hi]`
/// shape (an index followed by a trailing slice), which parses but is rejected at elaboration —
/// slicing stays single-dimension-only even for a 2-D vector net (v1 limitation). A `slice` on
/// an `Access`/`branch` terminal is rejected at elaboration regardless (`V`/`I` take single
/// nodes, never a sub-range).
#[derive(Clone, Debug, PartialEq)]
pub struct NetArg {
    /// The net (or vector net) name.
    pub name: String,
    /// 0 (bare name), 1 (`bus[i]`), or 2 (`grid[i][j]`, § 2-D vector net) bracketed index
    /// expressions, outer-to-inner.
    pub index: Vec<ExprRef>,
    /// The bracketed `[msb:lsb]` slice, if this terminal selects a sub-range of a vector net —
    /// only meaningful as an instance port-connection argument, and only for a 1-D vector net
    /// (a slice on a 2-D-declared vector, or combined with a non-empty `index`, is rejected at
    /// elaboration — v1 limitation, § 2-D vector net).
    pub slice: Option<(ExprRef, ExprRef)>,
}

/// One net name in an [`Item::Net`] declaration list, with its own optional vector range(s)
/// (`bus[3:0]`, or `grid[0:R][0:C]` for a § 2-D vector net) — independent of any shared prefix
/// range on the declaration itself.
///
/// **§ 2-D vector net**: a second dimension here is *not* standard Verilog-A — the LRM's
/// `net_declaration` grammar only ever carries one `[msb:lsb]` range. This is a deliberate,
/// documented extension (capped at exactly 2 dimensions, never more), kept clearly labeled
/// wherever it's surfaced (elaboration errors, `docs/token-reference.md`) so it's never mistaken
/// for standard grammar. Contrast [`VarEntry`], whose 2-D form *is* standard.
#[derive(Clone, Debug)]
pub struct NetDecl {
    /// The net name.
    pub name: String,
    /// The declared dimension ranges, outer-to-inner: empty for an ordinary scalar net, one
    /// entry for a standard 1-D vector net, two for a § 2-D vector net (non-standard extension).
    pub ranges: Vec<(ExprRef, ExprRef)>,
}

/// One name in a `real`/`integer` variable declaration list (module-level or block-local),
/// with its own optional array range(s) — the same shape as [`NetDecl`], for the same reason
/// (`real out_val[0:15], tmp;` mixes an array name with a plain scalar one, just like a net
/// declaration can). Unlike [`NetDecl`]'s 2-D form, a second dimension here *is* standard
/// Verilog-A grammar — the LRM's `variable_identifier` production allows a repeated
/// unpacked-dimension list; this implementation caps it at 2 (not general N-D).
#[derive(Clone, Debug)]
pub struct VarEntry {
    /// The variable name.
    pub name: String,
    /// The declared array dimension ranges, outer-to-inner: empty for an ordinary scalar
    /// variable, one entry for a standard 1-D array (`out_val[0:15]`), two for a § 2-D array
    /// variable (`tile[0:R][0:C]`).
    pub ranges: Vec<(ExprRef, ExprRef)>,
    /// An inline initializer, `real x = expr;`. Mutually exclusive with `ranges` — the LRM's
    /// `real_identifier` grammar allows a dimension *or* an initializer, never both, and the
    /// parser only looks for `= expr` when there was no `[...]` dimension. `None` for a
    /// declaration with no initializer.
    pub init: Option<ExprRef>,
}

/// A parameter's `from` value range, e.g. `from (0:inf)` or `from [0:c0)`.
///
/// `exclude` clauses (single values or ranges) are parsed but not represented here — v0 keeps
/// only the `from` range, which sets the parameter's min/max.
#[derive(Clone, Debug)]
pub struct Range {
    /// Lower bound expression.
    pub lo: ExprRef,
    /// Upper bound expression.
    pub hi: ExprRef,
    /// Whether the lower bound is inclusive (`[`) rather than exclusive (`(`).
    pub lo_inclusive: bool,
    /// Whether the upper bound is inclusive (`]`) rather than exclusive (`)`).
    pub hi_inclusive: bool,
}

/// A top-level item inside a module: a declaration or the analog block.
#[derive(Clone, Debug)]
pub enum Item {
    /// A direction declaration, `inout p, n;`.
    Direction {
        /// The direction keyword.
        dir: Direction,
        /// The declared net names.
        nets: Vec<String>,
    },
    /// A discipline declaration, `electrical p, n;`, or a vector net declaration — either a
    /// shared prefix range, `electrical [3:0] bus;`, or a per-identifier suffix range,
    /// `electrical bus[3:0], p;` (both forms appear in real Verilog-A; the parser applies a
    /// prefix range as the default for any name in the list that doesn't specify its own). A
    /// vector name becomes a bus of nodes spanning its range, indexed by a genvar expression in
    /// a branch access (§ vector nets). A *second* dimension (`electrical [0:R][0:C] grid;`,
    /// § 2-D vector net) is a deliberate, documented **non-standard** extension — the LRM's
    /// `net_declaration` grammar never carries more than one range — capped at exactly 2
    /// dimensions. Contrast [`Item::Var`]'s 2-D form, which *is* standard grammar.
    Net {
        /// The discipline keyword.
        discipline: Discipline,
        /// The declared nets, each with its own optional vector range.
        nets: Vec<NetDecl>,
    },
    /// A parameter declaration, `parameter real R = 1000 from (0:inf);`.
    Param {
        /// Declared base type (defaults to [`ParamType::Real`] when omitted).
        ty: ParamType,
        /// Parameter name.
        name: String,
        /// Default-value expression.
        default: ExprRef,
        /// Optional value range.
        range: Option<Range>,
    },
    /// The `analog` block (always normalised to a [`Stmt::Block`]).
    Analog(Stmt),
    /// An analog function definition, `analog function real f; … endfunction`.
    Function(AnalogFunction),
    /// A module-level variable declaration, `real q, v;` / `integer i;`, or an array-variable
    /// declaration, `real out_val[0:15];` (§ array variables — indexed like a vector net, by a
    /// compile-time-constant or genvar expression; there is no runtime-indexed array support).
    /// A *second* dimension (`real tile[0:R][0:C];`, § 2-D array variable) **is** standard
    /// Verilog-A grammar (the LRM's `variable_identifier` allows a repeated unpacked-dimension
    /// list) — this implementation caps it at 2, not general N-D. Contrast [`Item::Net`]'s 2-D
    /// form, which is a non-standard extension.
    Var {
        /// Declared base type (`real`/`integer`).
        ty: ParamType,
        /// Declared variable names, each with its own optional array range.
        names: Vec<VarEntry>,
    },
    /// A named branch declaration, `branch (a, b) br1, br2;` (one terminal = node-to-reference).
    Branch {
        /// The one or two terminal nets.
        terminals: Vec<NetArg>,
        /// The declared branch names (all aliasing the same terminals).
        names: Vec<String>,
    },
    /// An `aliasparam` declaration, `aliasparam alias = existing;` — a second name for an
    /// already-declared parameter. Does not introduce a new parameter: `alias` resolves to
    /// the same value as `existing`.
    AliasParam {
        /// The new name being introduced.
        name: String,
        /// The name of the parameter it aliases.
        target: String,
    },
    /// A `genvar` declaration, `genvar list_of_genvar_identifiers;`. Genvars exist only during
    /// elaboration (§ generate loops): they may only be assigned inside the control header of
    /// a `for` loop they drive, and that loop is fully unrolled — never emitted as a runtime
    /// [`Stmt::For`] — so analog operators (`ddt`/`idt`) are legal inside it.
    Genvar {
        /// The declared genvar names.
        names: Vec<String>,
    },
    /// A module instantiation, `resistor r1(p, n);` or
    /// `divider #(.gain(2.0)) d1(.in(vin), .out(vo));` (LRM Annex C.8). Resolved entirely at
    /// elaboration by recursively elaborating the referenced module and inlining it into the
    /// instantiating module's own IR arenas (§ module instantiation) — there is no IR-level
    /// hierarchy construct; `va_ir::Module` stays a single flat module.
    Instance {
        /// The instantiated module's name.
        module: String,
        /// The instance name — no runtime identity survives elaboration; used only for
        /// diagnostics and as the hierarchical namespace prefix for any of the submodule's
        /// nodes/vars/functions not unified with a parent net via a port connection.
        name: String,
        /// `#(.name(expr), ...)` parameter overrides, empty if absent. Each `expr` is
        /// evaluated in the *instantiating* module's scope (it may reference the parent's own
        /// parameters/genvars) before being substituted for the submodule's corresponding
        /// parameter default.
        params: Vec<(String, ExprRef)>,
        /// Port connections, in source order. Either all [`PortConn::Positional`] (bound to
        /// the submodule's ports in declaration order) or all [`PortConn::Named`] (bound by
        /// port name, in any order) — mixing the two parses fine but is rejected at
        /// elaboration.
        connections: Vec<PortConn>,
    },
}

/// One port connection in an [`Item::Instance`].
#[derive(Clone, Debug)]
pub enum PortConn {
    /// A positional connection: binds to the submodule's ports in declaration order.
    Positional(NetArg),
    /// A named connection, `.port(net)`: binds one net to one submodule port by name.
    Named {
        /// The submodule's port name.
        port: String,
        /// The net wired to it.
        net: NetArg,
    },
}

/// A user-defined analog function (`analog function`).
///
/// The function name doubles as the return variable inside the body, per Verilog-A. Argument
/// *types* (`real x;` declarations inside the body) are consumed but not tracked in v0; only
/// the argument directions and the body statements are retained.
#[derive(Clone, Debug)]
pub struct AnalogFunction {
    /// Function name (and implicit return variable).
    pub name: String,
    /// Declared return base type (defaults to [`ParamType::Real`]).
    pub ret_ty: ParamType,
    /// Formal arguments, in declaration order.
    pub args: Vec<FuncArg>,
    /// Body statements.
    pub body: Vec<Stmt>,
}

/// A formal argument of an [`AnalogFunction`].
#[derive(Clone, Debug)]
pub struct FuncArg {
    /// Argument direction (`input`/`output`/`inout`).
    pub dir: Direction,
    /// Argument name.
    pub name: String,
}

/// One arm of a [`Stmt::Case`].
#[derive(Clone, Debug)]
pub struct CaseArm {
    /// Label expressions; an arm may list several (`1, 2, 3:`).
    pub labels: Vec<ExprRef>,
    /// Statements executed when a label matches.
    pub body: Vec<Stmt>,
}

/// An analog statement.
#[derive(Clone, Debug)]
pub enum Stmt {
    /// A `begin … end` block.
    Block(Vec<Stmt>),
    /// A block-local variable declaration, `real x, y;` / `integer i;`, or array-variable
    /// declaration, `real out_val[0:15];` (§ array variables). Carries no value; it only
    /// introduces variable (or array) names (the base type is not retained).
    VarDecl {
        /// Declared variable names, each with its own optional array range(s).
        names: Vec<VarEntry>,
    },
    /// A system-task call statement, e.g. `$strobe("…", a, b);` or `$finish;`. v0 treats
    /// these as no-ops (no output side effects in a DC solve).
    Task {
        /// Task name with the leading `$` stripped (e.g. `strobe`).
        name: String,
        /// Argument expressions (may include string literals).
        args: Vec<ExprRef>,
    },
    /// A `<+` contribution: `target <+ value;`.
    Contribute {
        /// The access being contributed to.
        target: Access,
        /// The contributed value.
        value: ExprRef,
    },
    /// A procedural assignment: `lhs = rhs;`, or an array-element assignment,
    /// `lhs[index] = rhs;` / `lhs[i][j] = rhs;` (§ array variables / § 2-D array variables).
    Assign {
        /// Assigned variable name.
        lhs: String,
        /// 0 (plain scalar), 1, or 2 array indices, if `lhs` is an array-variable element
        /// rather than a plain scalar. Each must be compile-time-constant or genvar, except at
        /// most one of (up to 2) may instead be a genuinely runtime expression (§ dynamic
        /// array-variable indexing) — checked at elaboration, not here (mirroring
        /// [`NetArg::index`]).
        index: Vec<ExprRef>,
        /// Right-hand-side expression.
        rhs: ExprRef,
    },
    /// `if (cond) then_ [else else_]`. Branch arms are normalised to statement lists.
    If {
        /// Condition expression.
        cond: ExprRef,
        /// `then` arm.
        then_: Vec<Stmt>,
        /// `else` arm (empty when absent).
        else_: Vec<Stmt>,
    },
    /// `while (cond) body`.
    While {
        /// Loop condition.
        cond: ExprRef,
        /// Loop body.
        body: Vec<Stmt>,
    },
    /// `for (init; cond; step) body`. `init`/`step` are single (assignment) statements.
    For {
        /// Initialiser statement.
        init: Box<Stmt>,
        /// Loop condition.
        cond: ExprRef,
        /// Step statement, run after each iteration.
        step: Box<Stmt>,
        /// Loop body.
        body: Vec<Stmt>,
    },
    /// `repeat (count) body`.
    Repeat {
        /// Iteration count expression.
        count: ExprRef,
        /// Loop body.
        body: Vec<Stmt>,
    },
    /// `case (selector) arms… [default: …] endcase`.
    Case {
        /// The value being switched on.
        selector: ExprRef,
        /// Labelled arms, in source order.
        arms: Vec<CaseArm>,
        /// The `default` arm, if present.
        default: Option<Vec<Stmt>>,
    },
}

/// An expression arena node. Children are referenced by [`ExprRef`].
#[derive(Clone, Debug)]
pub enum ExprAst {
    /// Numeric literal (already scaled; `inf` becomes `f64::INFINITY`).
    Number(f64),
    /// A bare identifier: a parameter, variable, or genvar reference (resolved at elaboration).
    Ident(String),
    /// An indexed identifier, `name[index]` or `name[i][j]`: one element of a 1-D or § 2-D
    /// array variable. Always 1 or 2 entries (the parser only builds this variant once it has
    /// seen a `[`). Each index must be compile-time-constant or genvar, except at most one of
    /// (up to 2) may instead be a genuinely runtime expression (§ dynamic array-variable
    /// indexing) — checked at elaboration, not here.
    IndexedIdent(String, Vec<ExprRef>),
    /// A system function call with the `$` stripped, e.g. `$vt`, `$temperature`,
    /// `$simparam("gmin", 0)`. `args` is empty for the zero-argument forms.
    SysFunc {
        /// Name without the leading `$`.
        name: String,
        /// Argument expressions (may include string literals).
        args: Vec<ExprRef>,
    },
    /// A string literal, e.g. a `$strobe`/`analysis` argument. Has no numeric value; valid
    /// only where a string is expected (a system-task or analysis-style argument).
    Str(String),
    /// A branch probe, `V(...)`/`I(...)`.
    Probe(Access),
    /// A function call, e.g. `exp(x)`, `ddt(C*V(p,n))`, `pow(x, y)`.
    Call {
        /// Callee name.
        name: String,
        /// Argument expressions.
        args: Vec<ExprRef>,
    },
    /// Unary operation.
    Unary(UnOp, ExprRef),
    /// Binary operation.
    Binary(BinOp, ExprRef, ExprRef),
    /// Ternary conditional `cond ? then_ : else_`.
    Cond {
        /// The selector; non-zero chooses `then_`.
        cond: ExprRef,
        /// Value when `cond` is non-zero.
        then_: ExprRef,
        /// Value when `cond` is zero.
        else_: ExprRef,
    },
    /// A port-current probe, `I(<port>)` (LRM §5.4.3 "Accessing flow through a port", §3.12.1
    /// "Port Branches") — the current flowing *into the module* through a declared port,
    /// distinct from an ordinary `V(...)`/`I(...)` branch access (which probes/creates a branch
    /// between two nets, never a port's own boundary current). Real, normative Verilog-A
    /// grammar (`port_probe_function_call ::= nature_access_function ( < analog_port_reference
    /// >)`), not an extension — confirmed by the LRM's own diode worked example (a current-limit
    /// warning guarded on this probe's value). Two hard LRM constraints, both enforced at parse
    /// time: **flow-only** (`kind` is always [`AccessKind::Flow`] — `V(<port>)` is explicitly
    /// invalid and rejected while parsing) and **read-only** (the parser never produces this
    /// variant in a contribution-target position; "the port access function shall not be used
    /// on the left side of `<+`").
    PortProbe {
        /// Always [`AccessKind::Flow`] in practice (kept as a field, not a bare `String`, so
        /// the parser's `V`/`I`-uniform dispatch stays uniform; elaboration re-checks it).
        kind: AccessKind,
        /// The probed port's name — must name one of this module's own declared ports.
        port: String,
    },
    /// An array-literal expression, `{expr, expr, ...}` (LRM §4.5.10's Laplace/Z-domain filter
    /// coefficient-list argument syntax, e.g. `laplace_nd(sig, {1}, {1, tau})`). Not a
    /// general-purpose value — elaboration only accepts one as a `laplace_nd` numerator/
    /// denominator argument, const-evaluating each element for the filter's DC (s=0) gain fold;
    /// anywhere else it's an elaboration error. No `va-ir` representation exists for it (and
    /// none is needed): it never survives past elaboration as a runtime value.
    ArrayLit(Vec<ExprRef>),
}

/// Unary operators.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// Arithmetic negation, `-x`.
    Neg,
    /// Logical negation, `!x`.
    Not,
    /// Bitwise NOT, `~x` (distinct from `!x`; operates on the truncated integer value).
    BitNot,
}

/// Binary operators. Richer than [`va_ir::BinOp`]; elaboration maps or rejects the extras.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `%`, modulus (same sign as the dividend, matching Rust's/C's `%`).
    Mod,
    /// `**`.
    Pow,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `&`, bitwise AND (distinct from `&&`).
    BitAnd,
    /// `|`, bitwise OR (distinct from `||`).
    BitOr,
    /// `^`, bitwise XOR.
    BitXor,
    /// `^~`/`~^`, bitwise XNOR.
    BitXnor,
    /// `<<`, left shift.
    Shl,
    /// `>>`, right shift (logical — this project has no signed/unsigned integer distinction).
    Shr,
}
