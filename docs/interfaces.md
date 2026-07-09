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
// Function { name: String, args: Vec<VarId>, arg_dirs: Vec<ArgDir>, ret: VarId, body: Vec<Stmt> }
// ArgDir { Input, Output, Inout }  // LRM `input`/`output`/`inout` on a function argument
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

> **Not a §6 change (at the time — see the 2026-07-09 revision below): discipline/nature
> declarations.** `discipline...enddiscipline`/`nature...endnature` blocks are now genuinely
> parsed (`docs/token-reference.md` §1.5, §2.17, §2.25) into a small `va-frontend`-internal
> table (`disciplines::{NatureDecl, DisciplineDecl}`), instead of discarded as an opaque token
> span. This didn't touch Interface α either, *as of this note*: net *declarations* still only
> accept the `electrical`/`thermal` keyword tokens (unchanged `ast::Discipline`/
> `va_ir::Discipline`), so `Module`/`NodeDecl` were exactly as before — `va_ir::Discipline::Other`
> still existed as a forward-looking placeholder, still never constructed. The only real effect
> was parser-internal: a parsed discipline's bound nature `access` names widen
> `Parser::known_access` beyond the hardcoded `V`/`I`/`Temp`/`Pwr` baseline, additively. (This
> stopped being fully accurate once `NodeDecl` itself gained a field sourced from this same
> metadata — see the 2026-07-09 revision.)

> **Revision (§6 change, 2026-07-06):** added `Function::arg_dirs: Vec<ArgDir>` (`ArgDir` a new
> three-variant enum: `Input`/`Output`/`Inout`), same length and order as `Function::args`,
> recording the LRM's `input`/`output`/`inout` direction on each analog function argument —
> previously parsed by `va-frontend` (`ast::FuncArg::dir`) but discarded during elaboration,
> which bound every argument as a plain input with no way to write a result back to the caller.
> Real compact models use `output`/`inout` arguments for a function that computes several
> results at once (`mvsg_cmc_*.va`'s `calc_iq`/`calc_capt`); `va-codegen`'s `call_function`
> reads this to decide whether to bind the caller's actual-argument value in before running the
> body (`Input`/`Inout`) and/or write the parameter's final binding back into the caller's own
> variable after (`Output`/`Inout` — enforced to be a plain `Expr::Var`, per the LRM's own
> restriction on output/inout actual arguments). Additive and backward compatible: every existing
> `Function` construction site needed only `arg_dirs: vec![ArgDir::Input; args.len()]` added,
> preserving its exact prior behavior.

> **Revision (§6 change, 2026-07-09):** added `NodeDecl.abstol: Option<f64>` (§ nature-metadata
> wiring, `docs/roadmap.md` backlog item 5) — the node's discipline's **potential** nature's
> `abstol`, if a parsed `discipline...enddiscipline`/`nature...endnature` preamble resolves one
> (`va_frontend::disciplines::resolve_abstol`), else `None`. This is the change the
> "not a §6 change" note above predates: a parsed discipline's metadata now reaches `Module`
> itself, not just `Parser::known_access`. Additive and backward compatible: every existing
> `NodeDecl { name, discipline }` construction site needed only `abstol: None` added, preserving
> its exact prior behavior; `va-frontend`'s public entry points stayed source-compatible too —
> `elaborate`/`elaborate_with_library` are now thin wrappers over the new
> `elaborate_with_library_and_disciplines`, passing empty tables. `None` is `va-core`'s signal to
> fall back to its own configured default (see Interface β's matching `unknown_abstol` revision,
> below) — there is deliberately no equivalent field for a discipline's *flow* nature (e.g.
> `Current`'s `abstol`): only a `Node`-kind unknown (a KCL potential) has a natural per-`NodeDecl`
> home for one.

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
pub enum UnknownKind {
    Node,   // a KCL current-sum row; safe for `gmin` to shunt to ground
    Branch, // a constraint row (e.g. a source's V(p)-V(n)=value); never shunt this
}

pub trait ModelInstance {
    /// Global unknown indices this instance contributes to (nodes + internal unknowns).
    fn unknowns(&self) -> &[usize];
    /// Structural kind of `unknowns()[i]`. Default `UnknownKind::Node`.
    fn unknown_kind(&self, i: usize) -> UnknownKind { UnknownKind::Node }
    /// Per-unknown abstol override for `unknowns()[i]`. Default `None` (solver's own default).
    fn unknown_abstol(&self, i: usize) -> Option<f64> { None }
    /// Evaluate at solution vector `x`; emit residual + Jacobian (+ charge in transient).
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
}
```

`va-abi` ships **working** `resistor`, `capacitor`, and `diode` reference models against this
trait at bootstrap, so `va-core` has something real to solve on commit #1.

> **Revision (§6 change, 2026-07-04):** added `UnknownKind` and `ModelInstance::unknown_kind`,
> a **default trait method** (`docs/bridges/interface-beta-abi.md` §8's own recommendation for
> an optional addition), so every existing implementor — `va-abi::reference`'s `Resistor`/
> `Capacitor`/`Diode`, and every `va-codegen`-generated model — kept compiling unchanged.
> `va_abi::reference::VSource` overrides it for its branch-current unknown (`Branch`, everything
> else `Node`). This unblocks `va-core`'s `gmin`-stepping convergence aid
> (`crate::mna::classify_unknowns`/`System::shunt_gmin`, `NewtonConfig::gmin_steps`): it needs
> to know which rows are KCL sums (safe to shunt a conductance to ground) versus constraint
> rows like a source's `V(p) − V(n) = value` (which shunting would silently corrupt) — see
> `docs/roadmap.md`'s T3.3 for the full account of why this was previously listed as
> blocked on exactly this change.

> **Revision (§6 change, 2026-07-09):** added `ModelInstance::unknown_abstol`, another **default
> trait method** in exactly the same shape as `unknown_kind` above — every existing implementor
> kept compiling unchanged. `va-codegen`'s generated models override it, reading the matching
> `va_ir::NodeDecl::abstol` (Interface α's paired revision, above) for any of their node-kind
> unknowns; every hand-written `va-abi::reference` device (none compiled from Verilog-A, so none
> has discipline metadata) and any auxiliary (branch-current/`idt` accumulator) unknown beyond a
> generated model's own node count keep the default `None`. `va-core::mna::classify_abstol`
> collects this into a per-unknown tolerance vector (mirroring `classify_unknowns`), which
> `newton::solve_from`'s per-unknown convergence check now consults instead of always using
> `NewtonConfig::abstol` — see `docs/roadmap.md` backlog item 5 for the full account and its
> stated v1 limits (no flow-nature/branch-unknown wiring; the residual-norm gate stays global).
