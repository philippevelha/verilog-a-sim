# Frozen Interfaces (ratified at kickoff)

These are the **v0 contracts** (CLAUDE.md §4). They are mirrored into the `va-ir` and
`va-abi` crates. Changing either is a coordinated event (§6): open an issue listing every
downstream crate, get owner agreement, and update this file together with the crate in one
PR with stub adapters so the workspace keeps compiling. **Never** silently widen or reshape
them in a feature PR — a broken contract blocks every sibling thesis at once.

> This file holds the **verbatim v0 sketches**. The full semantic specifications — meaning,
> invariants, conventions, worked examples, and evolution rules — live in
> [`bridges/`](bridges/README.md): [Bridge α](bridges/interface-alpha-ir.md) and
> [Bridge β](bridges/interface-beta-abi.md).

## Interface α — elaborated IR (`va-ir`)

Arena/index representation is mandatory (§5). Expressions and statements are stored in
`Vec`s and referenced by index types, never by `&` references or `Box` graphs.

```rust
// va-ir/src/lib.rs  (sketch — flesh out, do not restructure casually)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct NodeId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ParamId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ExprId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct BranchId(pub u32);

pub struct Module {
    pub name: String,
    pub ports: Vec<Vec<NodeId>>,  // one entry per declared port; >1 NodeId for a vector port
    pub nodes: Vec<NodeDecl>,
    pub branches: Vec<Branch>,
    pub params: Vec<Param>,
    pub exprs: Vec<Expr>,         // arena
    pub functions: Vec<Function>, // user-defined analog functions
    pub analog: Vec<Stmt>,        // top-level analog block
}

pub enum Expr {
    Const(f64),
    Param(ParamId),
    Probe(Access),                 // V(b) or I(b)
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Call(Builtin, Vec<ExprId>),    // exp, ln, ddt, idt, $vt, $temperature, ...
    CallUser(FuncId, Vec<ExprId>), // user-defined analog function call
    Select(ExprId, ExprId, ExprId),// ternary cond ? then : else
    Ddx(ExprId, Access),           // ddx(expr, probe): partial derivative w.r.t. probe's node
}

pub enum Stmt {
    Contribute { target: Access, value: ExprId },  // <+
    If { cond: ExprId, then_: Vec<Stmt>, else_: Vec<Stmt> },
    Assign { lhs: VarId, rhs: ExprId },
    Block(Vec<Stmt>),
    While { cond: ExprId, body: Vec<Stmt> },
    For { init: Box<Stmt>, cond: ExprId, step: Box<Stmt>, body: Vec<Stmt> },
    Repeat { count: ExprId, body: Vec<Stmt> },
    Case { selector: ExprId, arms: Vec<CaseArm>, default: Vec<Stmt> },
}

// CaseArm { labels: Vec<ExprId>, body: Vec<Stmt> }
// Function { name: String, args: Vec<VarId>, ret: VarId, body: Vec<Stmt> }
```

The shipped `va-ir` fleshes this out (adds `VarId`, `VarDecl`, `FuncId`, `Discipline`,
`AccessKind`, helper methods) without restructuring the contract.

> **Revision (§6 change, 2026-06-30):** added the analog control-flow statements (`While`,
> `For`, `Repeat`, `Case` + `CaseArm`) and user-defined analog functions (`Module.functions`,
> `Function`, `Expr::CallUser`, `FuncId`). The frontend lowers all of them; `va-codegen` v0
> still rejects them during its own lowering (stub adapters), so the workspace keeps
> compiling. The `Box<Stmt>` in `For` is a finite-size tree node, not a shared graph, so it
> respects the §5 arena rule.

> **Not a §6 change: module instantiation (Annex C.8).** `va-frontend` now supports
> `Item::Instance` (`resistor r1(p, n);`, `#(...)` overrides, named `.port(net)` connections —
> see `docs/token-reference.md` §2.1b). It does **not** appear here because it never touches
> this contract: the elaborator resolves a whole instantiation hierarchy by recursively
> elaborating each referenced submodule and inlining its arenas into the instantiating
> module's own, entirely inside `va-frontend`, before Interface α's boundary. `Module` above is
> still exactly what `va-codegen`/`va-core`/`va-abi` receive — one flat module, no hierarchy
> concept, unchanged in shape. Hierarchy is a `va-frontend`-internal concern, not an IR one.

> **Not a §6 change: discipline/nature declarations.** `discipline...enddiscipline`/
> `nature...endnature` blocks are now genuinely parsed (`docs/token-reference.md` §1.5, §2.17,
> §2.25) into a small `va-frontend`-internal table (`disciplines::{NatureDecl,
> DisciplineDecl}`), instead of discarded as an opaque token span. This doesn't touch Interface
> α either: net *declarations* still only accept the `electrical`/`thermal` keyword tokens
> (unchanged `ast::Discipline`/`va_ir::Discipline`), so `Module`/`NodeDecl` are exactly as
> before — `va_ir::Discipline::Other` still exists as a forward-looking placeholder, still
> never constructed. The only real effect is parser-internal: a parsed discipline's bound
> nature `access` names widen `Parser::known_access` beyond the hardcoded `V`/`I`/`Temp`/`Pwr`
> baseline, additively.

## Interface β — model instance ABI (`va-abi`)

The project's internal "OSDI." `va-core` calls `load`; both `va-codegen`'s generated models
and `va-abi`'s hand-written reference models implement it. DC ignores the charge channel;
the transient integrator consumes it via a companion model.

```rust
// va-abi/src/stamps.rs
pub trait StampSink {
    fn residual(&mut self, row: usize, value: f64);            // current into node `row`
    fn jacobian(&mut self, row: usize, col: usize, value: f64); // dResidual[row]/dx[col]
    fn charge(&mut self, row: usize, value: f64);              // Q at `row`  (transient)
    fn dcharge(&mut self, row: usize, col: usize, value: f64); // dQ[row]/dx[col]
}

// va-abi/src/instance.rs
pub trait ModelInstance {
    /// Global unknown indices this instance contributes to (nodes + internal unknowns).
    fn unknowns(&self) -> &[usize];
    /// Evaluate at solution vector `x`; emit residual + Jacobian (+ charge in transient).
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
}
```

`va-abi` ships **working** `resistor`, `capacitor`, and `diode` reference models against this
trait at bootstrap, so `va-core` has something real to solve on commit #1.
