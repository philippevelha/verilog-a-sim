//! Interface α — the elaborated Verilog-A intermediate representation.
//!
//! This crate is a **frozen shared contract** (§4, §6). It is a leaf crate with no
//! internal dependencies, so both `va-frontend` (which produces the IR) and `va-codegen`
//! (which consumes it) can depend on it without coupling to each other.
//!
//! # Representation rules (§5)
//!
//! Everything graph-shaped is stored in arena `Vec`s and referenced by `Copy` index
//! newtypes ([`NodeId`], [`ExprId`], …). There are no `Rc`/`RefCell`/`Box`-graph or
//! lifetime-threaded references in the IR. Flesh these types out, but do not restructure
//! them casually — a change here is a coordinated interface event (§6).
//!
//! # Limitations
//!
//! This is the v0 contract. It models a single module with electrical/thermal disciplines,
//! `<+` contributions, `ddt`/`idt`, ranged parameters, the full set of analog control-flow
//! statements (`if`/`else`, `while`, `for`, `repeat`, `case`), and user-defined analog
//! functions ([`Function`], [`Expr::CallUser`]). Multi-module hierarchy, generate loops, and
//! the full LRM are explicitly out of scope.
//!
//! Note that *modeling* a construct in the IR does not imply every back-end consumes it yet:
//! `va-codegen` v0, for example, still rejects loops/case/user-functions during its own
//! lowering. Adding IR nodes is the contract change (§6); back-end support follows per crate.

#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------------------
// Index newtypes (arena handles). See §4 — copied verbatim, do not reshape casually.
// ---------------------------------------------------------------------------------------

/// Index of a circuit node (terminal or internal) within [`Module::nodes`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

/// Index of a parameter within [`Module::params`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ParamId(pub u32);

/// Index of an expression within the [`Module::exprs`] arena.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExprId(pub u32);

/// Index of a branch within [`Module::branches`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BranchId(pub u32);

/// Index of a local analog variable. Used as the assignment target in [`Stmt::Assign`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VarId(pub u32);

/// Index of a user-defined analog function within [`Module::functions`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FuncId(pub u32);

// ---------------------------------------------------------------------------------------
// Module — the top-level elaborated unit.
// ---------------------------------------------------------------------------------------

/// An elaborated Verilog-A module: the unit `va-frontend` hands to `va-codegen`.
#[derive(Clone, Debug, Default)]
pub struct Module {
    /// Module name as written in source.
    pub name: String,
    /// Ports, in declaration order. Each indexes into [`Self::nodes`].
    pub ports: Vec<NodeId>,
    /// All declared nodes (ports + internal nodes).
    pub nodes: Vec<NodeDecl>,
    /// Branches between node pairs.
    pub branches: Vec<Branch>,
    /// Parameters with optional ranges/defaults.
    pub params: Vec<Param>,
    /// Expression arena. [`ExprId`]s index into this `Vec`.
    pub exprs: Vec<Expr>,
    /// Local analog variables referenced by [`VarId`]. Function arguments and locals share
    /// this arena; the owning [`Function`] records which [`VarId`]s are its arguments/return.
    pub vars: Vec<VarDecl>,
    /// User-defined analog functions referenced by [`FuncId`] (see [`Expr::CallUser`]).
    pub functions: Vec<Function>,
    /// The top-level `analog` block, as a flat statement list.
    pub analog: Vec<Stmt>,
}

impl Module {
    /// Create an empty module with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Push an expression into the arena and return its handle.
    pub fn push_expr(&mut self, expr: Expr) -> ExprId {
        let id = ExprId(self.exprs.len() as u32);
        self.exprs.push(expr);
        id
    }

    /// Borrow an expression by handle.
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }
}

// ---------------------------------------------------------------------------------------
// Declarations.
// ---------------------------------------------------------------------------------------

/// A physical discipline attached to a node. Drives which quantities are conserved.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Discipline {
    /// Electrical: potential = voltage, flow = current.
    Electrical,
    /// Thermal: potential = temperature, flow = power.
    Thermal,
    /// Optical, mechanical, etc. — reserved for the multi-physics roadmap (§1).
    Other,
}

/// A node declaration.
#[derive(Clone, Debug)]
pub struct NodeDecl {
    /// Node name as written in source (`gnd` is conventionally the reference).
    pub name: String,
    /// The discipline that governs this node.
    pub discipline: Discipline,
}

/// A branch between two nodes (the `+` and `-` terminals of an `Access`).
#[derive(Clone, Copy, Debug)]
pub struct Branch {
    /// Positive terminal.
    pub p: NodeId,
    /// Negative terminal (often the reference node).
    pub n: NodeId,
}

/// The kind of quantity probed or contributed across/through a branch.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AccessKind {
    /// Potential access — `V(a, b)`.
    Potential,
    /// Flow access — `I(a, b)`.
    Flow,
}

/// A branch access: `V(b)`/`I(b)`. Used both as a probe ([`Expr::Probe`]) and as the
/// target of a contribution ([`Stmt::Contribute`]).
#[derive(Clone, Copy, Debug)]
pub struct Access {
    /// Whether this is a potential or flow access.
    pub kind: AccessKind,
    /// The branch being accessed.
    pub branch: BranchId,
}

/// A parameter with an optional default and inclusive numeric range.
#[derive(Clone, Debug)]
pub struct Param {
    /// Parameter name.
    pub name: String,
    /// Default value (Verilog-A parameters always carry one).
    pub default: f64,
    /// Inclusive lower bound, if a `from`/`exclude` range was declared.
    pub min: Option<f64>,
    /// Inclusive upper bound, if declared.
    pub max: Option<f64>,
}

/// A local analog variable declaration.
#[derive(Clone, Debug)]
pub struct VarDecl {
    /// Variable name.
    pub name: String,
}

// ---------------------------------------------------------------------------------------
// Expressions (arena nodes).
// ---------------------------------------------------------------------------------------

/// An expression arena node. Children are referenced by [`ExprId`], never by reference.
#[derive(Clone, Debug)]
pub enum Expr {
    /// Literal constant.
    Const(f64),
    /// Reference to a parameter's value.
    Param(ParamId),
    /// Reference to a local variable's current value.
    Var(VarId),
    /// A branch probe — `V(b)` or `I(b)`.
    Probe(Access),
    /// Unary operation.
    Unary(UnOp, ExprId),
    /// Binary operation.
    Binary(BinOp, ExprId, ExprId),
    /// Built-in / system function call: `exp`, `ln`, `ddt`, `idt`, `$vt`, `$temperature`…
    Call(Builtin, Vec<ExprId>),
    /// A call to a user-defined analog function ([`Module::functions`]), with one argument
    /// expression per the function's declared inputs.
    CallUser(FuncId, Vec<ExprId>),
}

/// Unary operators.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UnOp {
    /// Arithmetic negation.
    Neg,
    /// Logical negation.
    Not,
}

/// Binary operators.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BinOp {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Exponentiation (`**`).
    Pow,
    /// Less-than comparison.
    Lt,
    /// Less-than-or-equal comparison.
    Le,
    /// Greater-than comparison.
    Gt,
    /// Greater-than-or-equal comparison.
    Ge,
    /// Equality comparison.
    Eq,
}

/// Built-in and system functions recognized by the IR.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Builtin {
    /// `exp(x)`.
    Exp,
    /// `ln(x)`.
    Ln,
    /// `log(x)` (base-10).
    Log,
    /// `sqrt(x)`.
    Sqrt,
    /// `abs(x)`.
    Abs,
    /// `pow(x, y)`.
    Pow,
    /// `ddt(x)` — time derivative (becomes a charge contribution).
    Ddt,
    /// `idt(x)` — time integral.
    Idt,
    /// `$vt` — thermal voltage.
    Vt,
    /// `$temperature` — ambient temperature.
    Temperature,
}

// ---------------------------------------------------------------------------------------
// Statements.
// ---------------------------------------------------------------------------------------

/// An analog statement. The `analog` block is a flat `Vec<Stmt>`; nested control flow uses
/// owned `Vec<Stmt>` children (still arena-friendly — no shared references).
#[derive(Clone, Debug)]
pub enum Stmt {
    /// A `<+` contribution: `target <+ value`.
    Contribute { target: Access, value: ExprId },
    /// `if (cond) { then_ } else { else_ }`.
    If {
        cond: ExprId,
        then_: Vec<Stmt>,
        else_: Vec<Stmt>,
    },
    /// Local variable assignment: `lhs = rhs`.
    Assign { lhs: VarId, rhs: ExprId },
    /// A `begin … end` block.
    Block(Vec<Stmt>),
    /// `while (cond) { body }`.
    While { cond: ExprId, body: Vec<Stmt> },
    /// `for (init; cond; step) { body }`. `init`/`step` are single statements (usually
    /// [`Stmt::Assign`]); boxed so the recursive `Stmt` has a finite size.
    For {
        init: Box<Stmt>,
        cond: ExprId,
        step: Box<Stmt>,
        body: Vec<Stmt>,
    },
    /// `repeat (count) { body }`.
    Repeat { count: ExprId, body: Vec<Stmt> },
    /// `case (selector) { arms… } [default]`. `default` is empty when no default arm exists.
    Case {
        selector: ExprId,
        arms: Vec<CaseArm>,
        default: Vec<Stmt>,
    },
}

/// One arm of a [`Stmt::Case`]: a set of label expressions and the body they select.
#[derive(Clone, Debug)]
pub struct CaseArm {
    /// Label expressions compared against the selector (`1, 2, 3:` lists several).
    pub labels: Vec<ExprId>,
    /// Statements executed when a label matches.
    pub body: Vec<Stmt>,
}

/// A user-defined analog function (`analog function`).
///
/// Verilog-A analog functions are pure and non-recursive. The function name doubles as the
/// return variable in source; here [`Self::ret`] names that variable. Arguments and locals
/// live in [`Module::vars`]; [`Self::args`] lists the arguments in declaration order, bound
/// positionally from a [`Expr::CallUser`]'s argument expressions.
#[derive(Clone, Debug)]
pub struct Function {
    /// Function name as written in source.
    pub name: String,
    /// Argument variables, in declaration order.
    pub args: Vec<VarId>,
    /// The return variable (named after the function in source).
    pub ret: VarId,
    /// Body statements; they assign to `ret`/locals and read `args`.
    pub body: Vec<Stmt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_push_and_fetch() {
        let mut m = Module::new("rc");
        let a = m.push_expr(Expr::Const(1.0));
        let b = m.push_expr(Expr::Const(2.0));
        let sum = m.push_expr(Expr::Binary(BinOp::Add, a, b));
        match m.expr(sum) {
            Expr::Binary(BinOp::Add, x, y) => {
                assert_eq!(*x, a);
                assert_eq!(*y, b);
            }
            _ => panic!("expected a binary add"),
        }
        assert_eq!(m.exprs.len(), 3);
    }

    #[test]
    fn control_flow_and_function_nodes() {
        let mut m = Module::new("ctrl");
        // A user function `sq(x)` with body `sq = x * x`.
        let x = VarId(m.vars.len() as u32);
        m.vars.push(VarDecl { name: "x".into() });
        let sq = VarId(m.vars.len() as u32);
        m.vars.push(VarDecl { name: "sq".into() });
        let xe = m.push_expr(Expr::Var(x));
        let body_rhs = m.push_expr(Expr::Binary(BinOp::Mul, xe, xe));
        m.functions.push(Function {
            name: "sq".into(),
            args: vec![x],
            ret: sq,
            body: vec![Stmt::Assign {
                lhs: sq,
                rhs: body_rhs,
            }],
        });
        let fid = FuncId(0);

        // A `case` whose default calls the function.
        let sel = m.push_expr(Expr::Const(1.0));
        let label = m.push_expr(Expr::Const(0.0));
        let arg = m.push_expr(Expr::Const(2.0));
        let call = m.push_expr(Expr::CallUser(fid, vec![arg]));
        let stmt = Stmt::Case {
            selector: sel,
            arms: vec![CaseArm {
                labels: vec![label],
                body: vec![Stmt::Assign { lhs: sq, rhs: call }],
            }],
            default: Vec::new(),
        };
        m.analog.push(stmt);

        assert_eq!(m.functions.len(), 1);
        assert!(matches!(m.expr(call), Expr::CallUser(FuncId(0), _)));
        assert!(matches!(m.analog[0], Stmt::Case { .. }));
    }
}
