//! Surface AST: the parser's output, before elaboration into [`va_ir`].
//!
//! The AST is a faithful, sugar-preserving tree of one `module`. Elaboration
//! ([`crate::elaborate`]) resolves names, assigns IR arena indices, and lowers it into the
//! frozen IR (Interface α).
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Discipline {
    /// `electrical`.
    Electrical,
    /// `thermal`.
    Thermal,
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

/// A branch access: an access function applied to one or two net names.
#[derive(Clone, Debug, PartialEq)]
pub struct Access {
    /// Potential or flow.
    pub kind: AccessKind,
    /// One net (node-to-reference) or two (explicit branch terminals).
    pub args: Vec<String>,
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
    /// A discipline declaration, `electrical p, n;`.
    Net {
        /// The discipline keyword.
        discipline: Discipline,
        /// The declared net names.
        nets: Vec<String>,
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
    /// A module-level variable declaration, `real q, v;` / `integer i;`.
    Var {
        /// Declared base type (`real`/`integer`).
        ty: ParamType,
        /// Declared variable names.
        names: Vec<String>,
    },
    /// A named branch declaration, `branch (a, b) br1, br2;` (one terminal = node-to-reference).
    Branch {
        /// The one or two terminal net names.
        terminals: Vec<String>,
        /// The declared branch names (all aliasing the same terminals).
        names: Vec<String>,
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
    /// A block-local variable declaration, `real x, y;` / `integer i;`. Carries no value; it
    /// only introduces variable names (the base type is not retained).
    VarDecl {
        /// Declared variable names.
        names: Vec<String>,
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
    /// A procedural assignment: `lhs = rhs;`.
    Assign {
        /// Assigned variable name.
        lhs: String,
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
    /// A bare identifier: a parameter or variable reference (resolved at elaboration).
    Ident(String),
    /// A system function reference with the `$` stripped, e.g. `vt`, `temperature`.
    SysFunc(String),
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
}

/// Unary operators.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// Arithmetic negation, `-x`.
    Neg,
    /// Logical negation, `!x`.
    Not,
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
}
