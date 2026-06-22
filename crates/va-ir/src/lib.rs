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
//! `<+` contributions, `ddt`/`idt`, `if/else`, analog functions, and ranged parameters.
//! Multi-module hierarchy, generate loops, and the full LRM are explicitly out of scope.

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
    /// Local analog variables referenced by [`VarId`].
    pub vars: Vec<VarDecl>,
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
}
