//! Elaboration: surface [`crate::ast::ModuleAst`] → frozen [`va_ir::Module`] (Interface α).
//!
//! Resolves net/branch/parameter/variable names to arena indices and lowers expressions and
//! statements into the IR. This is the only place the frontend touches `va-ir`.
//!
//! # Passes
//!
//! 1. **Nodes** — intern every net declared with a discipline; resolve the port list.
//! 2. **Parameters** — const-evaluate each default and range bound into `f64`.
//! 3. **Variables** — register explicitly declared module variables (`real q, v;`), then
//!    every remaining assignment target, as local variables.
//! 4. **Lowering** — translate the analog block's expressions and statements, creating
//!    branches on demand as branch accesses are resolved.
//!
//! # Limitations
//!
//! - A net must carry a discipline declaration (`electrical`/`thermal`) to become a node; a
//!   port declared by direction alone is rejected.
//! - Parameter defaults/ranges must be compile-time constant (literals, arithmetic, and the
//!   real math built-ins). `$vt`, probes, and forward parameter references are non-constant.
//! - Branches are keyed by ordered `(p, n)` node pairs; `V(a,b)` and `V(b,a)` are treated as
//!   distinct branches rather than sign-related aliases.
//! - Range bounds are mapped to `Param::min`/`max` losing the inclusive/exclusive
//!   distinction; an infinite bound becomes `None` (unbounded).
//! - `while`/`for`/`repeat`/`case` control flow and `analog function` definitions are lowered
//!   into the corresponding [`va_ir`] nodes. Functions are lowered in source order against
//!   their own variable scope (arguments + return variable + locals) and may read module
//!   parameters; forward references to a function defined later in source are unsupported and
//!   resolve as an unknown function. Note that back-ends need not consume these yet —
//!   `va-codegen` v0 still rejects them during its own lowering.

use std::collections::HashMap;

use crate::ast::{self, ExprAst, ExprRef, Item, ModuleAst, Stmt};
use crate::FrontendError;
use va_ir::{
    Access, AccessKind, Branch, BranchId, Builtin, CaseArm, Discipline, Expr, ExprId, FuncId,
    Function, Module, NodeDecl, NodeId, Param, ParamId, VarDecl, VarId,
};

/// Elaborate a parsed module into the IR.
///
/// # Errors
///
/// Returns [`FrontendError::Elaborate`] on unresolved names, non-constant parameter
/// expressions, or constructs outside the v0 subset.
pub fn elaborate(ast: &ModuleAst) -> Result<Module, FrontendError> {
    let mut e = Elaborator {
        ast,
        out: Module::new(&ast.name),
        nodes: HashMap::new(),
        params: HashMap::new(),
        param_vals: HashMap::new(),
        vars: HashMap::new(),
        funcs: HashMap::new(),
        branches: HashMap::new(),
        named_branches: HashMap::new(),
        ground: None,
    };
    e.run()?;
    Ok(e.out)
}

struct Elaborator<'a> {
    ast: &'a ModuleAst,
    out: Module,
    nodes: HashMap<String, NodeId>,
    params: HashMap<String, ParamId>,
    param_vals: HashMap<String, f64>,
    /// The variable name → id scope currently in effect. Holds module analog variables while
    /// lowering the analog block, and a function's local scope while lowering that function.
    vars: HashMap<String, VarId>,
    funcs: HashMap<String, FuncId>,
    branches: HashMap<(u32, u32), BranchId>,
    /// Named branches declared with `branch (a, b) name;`, resolved to their [`BranchId`].
    named_branches: HashMap<String, BranchId>,
    ground: Option<NodeId>,
}

impl Elaborator<'_> {
    fn run(&mut self) -> Result<(), FrontendError> {
        self.collect_nodes()?;
        self.resolve_ports()?;
        self.collect_branches()?;
        self.collect_params()?;
        self.collect_functions()?;
        self.collect_var_decls();
        self.collect_vars();
        self.lower_analog()?;
        Ok(())
    }

    /// Register explicitly declared module-level variables (`real q, v;`). The base type is
    /// not retained — the IR has no variable type and treats every value as `f64`. Assignment
    /// targets in the analog block are still auto-registered by [`Self::collect_vars`]; this
    /// pass just lets a variable be declared before (or without) being assigned.
    fn collect_var_decls(&mut self) {
        let ast = self.ast;
        for item in &ast.items {
            if let Item::Var { names, .. } = item {
                for name in names {
                    if let std::collections::hash_map::Entry::Vacant(slot) =
                        self.vars.entry(name.clone())
                    {
                        let id = VarId(self.out.vars.len() as u32);
                        self.out.vars.push(VarDecl { name: name.clone() });
                        slot.insert(id);
                    }
                }
            }
        }
    }

    /// Push a fresh local variable and return its id.
    fn new_var(&mut self, name: &str) -> VarId {
        let id = VarId(self.out.vars.len() as u32);
        self.out.vars.push(VarDecl {
            name: name.to_string(),
        });
        id
    }

    // --- pass: analog functions ------------------------------------------------------

    /// Lower each `analog function` definition into a [`Function`]. Functions are lowered in
    /// source order against their own variable scope (arguments, return variable, and locals);
    /// they may read module parameters but not module analog variables. A call to a function
    /// defined later in source resolves as unknown (forward references are unsupported in v0).
    fn collect_functions(&mut self) -> Result<(), FrontendError> {
        let ast = self.ast;
        for item in &ast.items {
            let f = match item {
                Item::Function(f) => f,
                _ => continue,
            };
            // Build the function-local scope: return variable, arguments, then any locals
            // discovered as assignment targets in the body.
            let mut local: HashMap<String, VarId> = HashMap::new();
            let ret = self.new_var(&f.name);
            local.insert(f.name.clone(), ret);

            let mut args = Vec::with_capacity(f.args.len());
            for a in &f.args {
                let id = self.new_var(&a.name);
                local.insert(a.name.clone(), id);
                args.push(id);
            }

            let mut targets = Vec::new();
            collect_assign_targets(&f.body, &mut targets);
            for name in targets {
                if let std::collections::hash_map::Entry::Vacant(slot) = local.entry(name) {
                    let id = VarId(self.out.vars.len() as u32);
                    self.out.vars.push(VarDecl {
                        name: slot.key().clone(),
                    });
                    slot.insert(id);
                }
            }

            // Lower the body with the function scope swapped in, then restore.
            let saved = std::mem::replace(&mut self.vars, local);
            let body = self.lower_stmts(&f.body);
            self.vars = saved;
            let body = body?;

            let fid = FuncId(self.out.functions.len() as u32);
            self.out.functions.push(Function {
                name: f.name.clone(),
                args,
                ret,
                body,
            });
            self.funcs.insert(f.name.clone(), fid);
        }
        Ok(())
    }

    // --- pass 1: nodes ---------------------------------------------------------------

    fn collect_nodes(&mut self) -> Result<(), FrontendError> {
        for item in &self.ast.items {
            if let Item::Net { discipline, nets } = item {
                let disc = match discipline {
                    ast::Discipline::Electrical => Discipline::Electrical,
                    ast::Discipline::Thermal => Discipline::Thermal,
                };
                for name in nets {
                    self.intern_node(name, disc);
                }
            }
        }
        Ok(())
    }

    fn intern_node(&mut self, name: &str, discipline: Discipline) -> NodeId {
        if let Some(id) = self.nodes.get(name) {
            return *id;
        }
        let id = NodeId(self.out.nodes.len() as u32);
        self.out.nodes.push(NodeDecl {
            name: name.to_string(),
            discipline,
        });
        self.nodes.insert(name.to_string(), id);
        id
    }

    fn resolve_ports(&mut self) -> Result<(), FrontendError> {
        for port in &self.ast.ports {
            match self.nodes.get(port) {
                Some(id) => self.out.ports.push(*id),
                None => {
                    return Err(elab(format!(
                        "port `{port}` has no discipline declaration (e.g. `electrical {port};`)"
                    )))
                }
            }
        }
        Ok(())
    }

    // --- pass: named branches --------------------------------------------------------

    /// Resolve each `branch (a, b) name;` declaration to a [`BranchId`] and register its
    /// name(s). The branch is interned by its terminal node pair, so a named access
    /// `V(name)` and a positional access `V(a, b)` refer to the same branch.
    fn collect_branches(&mut self) -> Result<(), FrontendError> {
        let ast = self.ast;
        for item in &ast.items {
            if let Item::Branch { terminals, names } = item {
                let id = self.resolve_branch(terminals)?;
                for name in names {
                    self.named_branches.insert(name.clone(), id);
                }
            }
        }
        Ok(())
    }

    // --- pass 2: parameters ----------------------------------------------------------

    fn collect_params(&mut self) -> Result<(), FrontendError> {
        for item in &self.ast.items {
            if let Item::Param {
                name,
                default,
                range,
                ..
            } = item
            {
                let default_val = self.const_eval(*default)?;
                let (min, max) = match range {
                    Some(r) => (bound(self.const_eval(r.lo)?), bound(self.const_eval(r.hi)?)),
                    None => (None, None),
                };
                let id = ParamId(self.out.params.len() as u32);
                self.out.params.push(Param {
                    name: name.clone(),
                    default: default_val,
                    min,
                    max,
                });
                self.params.insert(name.clone(), id);
                self.param_vals.insert(name.clone(), default_val);
            }
        }
        Ok(())
    }

    /// Fold a constant expression to its `f64` value. Parameter context only.
    fn const_eval(&self, r: ExprRef) -> Result<f64, FrontendError> {
        match self.ast.expr(r) {
            ExprAst::Number(n) => Ok(*n),
            ExprAst::Ident(name) => self.param_vals.get(name).copied().ok_or_else(|| {
                elab(format!(
                    "`{name}` is not a compile-time constant in this context"
                ))
            }),
            ExprAst::Unary(op, e) => {
                let v = self.const_eval(*e)?;
                Ok(match op {
                    ast::UnOp::Neg => -v,
                    ast::UnOp::Not => bool_to_f64(v == 0.0),
                })
            }
            ExprAst::Binary(op, l, rhs) => {
                let a = self.const_eval(*l)?;
                let b = self.const_eval(*rhs)?;
                Ok(eval_binop(*op, a, b))
            }
            ExprAst::Call { name, args } => {
                let vals: Result<Vec<f64>, _> = args.iter().map(|a| self.const_eval(*a)).collect();
                eval_const_call(name, &vals?)
            }
            ExprAst::SysFunc(name) => Err(elab(format!(
                "system function `${name}` is not constant in a parameter context"
            ))),
            ExprAst::Probe(_) => Err(elab(
                "a branch probe is not constant in a parameter context".to_string(),
            )),
        }
    }

    // --- pass 3: variables -----------------------------------------------------------

    fn collect_vars(&mut self) {
        // Borrow the AST through a copy of the shared reference so we can mutate `self`.
        let items = self.ast;
        for item in &items.items {
            if let Item::Analog(stmt) = item {
                self.collect_vars_stmt(stmt);
            }
        }
    }

    /// Register `name` as a local variable unless it is already a parameter or known variable.
    fn register_var(&mut self, name: &str) {
        if !self.params.contains_key(name) && !self.vars.contains_key(name) {
            let id = VarId(self.out.vars.len() as u32);
            self.out.vars.push(VarDecl {
                name: name.to_string(),
            });
            self.vars.insert(name.to_string(), id);
        }
    }

    fn collect_vars_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign { lhs, .. } => self.register_var(lhs),
            Stmt::VarDecl { names } => {
                for name in names {
                    self.register_var(name);
                }
            }
            Stmt::Block(body) => body.iter().for_each(|s| self.collect_vars_stmt(s)),
            Stmt::If { then_, else_, .. } => {
                then_.iter().for_each(|s| self.collect_vars_stmt(s));
                else_.iter().for_each(|s| self.collect_vars_stmt(s));
            }
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => {
                body.iter().for_each(|s| self.collect_vars_stmt(s));
            }
            Stmt::For {
                init, step, body, ..
            } => {
                self.collect_vars_stmt(init);
                self.collect_vars_stmt(step);
                body.iter().for_each(|s| self.collect_vars_stmt(s));
            }
            Stmt::Case { arms, default, .. } => {
                for arm in arms {
                    arm.body.iter().for_each(|s| self.collect_vars_stmt(s));
                }
                if let Some(body) = default {
                    body.iter().for_each(|s| self.collect_vars_stmt(s));
                }
            }
            Stmt::Contribute { .. } => {}
        }
    }

    // --- pass 4: lowering ------------------------------------------------------------

    fn lower_analog(&mut self) -> Result<(), FrontendError> {
        let ast = self.ast;
        for item in &ast.items {
            if let Item::Analog(stmt) = item {
                // The analog item is always normalised to a top-level block.
                if let Stmt::Block(body) = stmt {
                    for s in body {
                        let lowered = self.lower_stmt(s)?;
                        self.out.analog.push(lowered);
                    }
                } else {
                    let lowered = self.lower_stmt(stmt)?;
                    self.out.analog.push(lowered);
                }
            }
        }
        Ok(())
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<va_ir::Stmt, FrontendError> {
        match stmt {
            Stmt::Block(body) => {
                let mut out = Vec::with_capacity(body.len());
                for s in body {
                    out.push(self.lower_stmt(s)?);
                }
                Ok(va_ir::Stmt::Block(out))
            }
            // A declaration introduces a variable (registered in pass 3) but emits no code.
            Stmt::VarDecl { .. } => Ok(va_ir::Stmt::Block(Vec::new())),
            Stmt::Contribute { target, value } => {
                let target = self.lower_access(target)?;
                let value = self.lower_expr(*value)?;
                Ok(va_ir::Stmt::Contribute { target, value })
            }
            Stmt::Assign { lhs, rhs } => {
                let id = *self
                    .vars
                    .get(lhs)
                    .ok_or_else(|| elab(format!("assignment to unknown variable `{lhs}`")))?;
                let rhs = self.lower_expr(*rhs)?;
                Ok(va_ir::Stmt::Assign { lhs: id, rhs })
            }
            Stmt::If { cond, then_, else_ } => {
                let cond = self.lower_expr(*cond)?;
                let then_ = self.lower_stmts(then_)?;
                let else_ = self.lower_stmts(else_)?;
                Ok(va_ir::Stmt::If { cond, then_, else_ })
            }
            Stmt::While { cond, body } => {
                let cond = self.lower_expr(*cond)?;
                let body = self.lower_stmts(body)?;
                Ok(va_ir::Stmt::While { cond, body })
            }
            Stmt::Repeat { count, body } => {
                let count = self.lower_expr(*count)?;
                let body = self.lower_stmts(body)?;
                Ok(va_ir::Stmt::Repeat { count, body })
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
            } => {
                let init = Box::new(self.lower_stmt(init)?);
                let cond = self.lower_expr(*cond)?;
                let step = Box::new(self.lower_stmt(step)?);
                let body = self.lower_stmts(body)?;
                Ok(va_ir::Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                })
            }
            Stmt::Case {
                selector,
                arms,
                default,
            } => {
                let selector = self.lower_expr(*selector)?;
                let mut ir_arms = Vec::with_capacity(arms.len());
                for arm in arms {
                    let mut labels = Vec::with_capacity(arm.labels.len());
                    for &l in &arm.labels {
                        labels.push(self.lower_expr(l)?);
                    }
                    let body = self.lower_stmts(&arm.body)?;
                    ir_arms.push(CaseArm { labels, body });
                }
                let default = match default {
                    Some(b) => self.lower_stmts(b)?,
                    None => Vec::new(),
                };
                Ok(va_ir::Stmt::Case {
                    selector,
                    arms: ir_arms,
                    default,
                })
            }
        }
    }

    fn lower_stmts(&mut self, stmts: &[Stmt]) -> Result<Vec<va_ir::Stmt>, FrontendError> {
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            out.push(self.lower_stmt(s)?);
        }
        Ok(out)
    }

    fn lower_expr(&mut self, r: ExprRef) -> Result<ExprId, FrontendError> {
        // `self.ast` is a shared reference; copy it locally so the read borrow is of the
        // external `ModuleAst`, not of `self`, leaving `self` free to mutate.
        let ast = self.ast;
        let expr = match ast.expr(r) {
            ExprAst::Number(n) => Expr::Const(*n),
            ExprAst::Ident(name) => {
                if let Some(p) = self.params.get(name) {
                    Expr::Param(*p)
                } else if let Some(v) = self.vars.get(name) {
                    Expr::Var(*v)
                } else {
                    return Err(elab(format!("unknown identifier `{name}`")));
                }
            }
            ExprAst::SysFunc(name) => Expr::Call(sysfunc_builtin(name)?, Vec::new()),
            ExprAst::Probe(access) => Expr::Probe(self.lower_access(access)?),
            ExprAst::Call { name, args } => {
                let mut ids = Vec::with_capacity(args.len());
                for &a in args {
                    ids.push(self.lower_expr(a)?);
                }
                // A user-defined function takes precedence over the built-in table.
                if let Some(fid) = self.funcs.get(name).copied() {
                    Expr::CallUser(fid, ids)
                } else {
                    Expr::Call(call_builtin(name)?, ids)
                }
            }
            ExprAst::Unary(op, e) => {
                let inner = self.lower_expr(*e)?;
                let op = match op {
                    ast::UnOp::Neg => va_ir::UnOp::Neg,
                    ast::UnOp::Not => va_ir::UnOp::Not,
                };
                Expr::Unary(op, inner)
            }
            ExprAst::Binary(op, l, rhs) => {
                let op = map_binop(*op)?;
                let l = self.lower_expr(*l)?;
                let rhs = self.lower_expr(*rhs)?;
                Expr::Binary(op, l, rhs)
            }
        };
        Ok(self.out.push_expr(expr))
    }

    fn lower_access(&mut self, access: &ast::Access) -> Result<Access, FrontendError> {
        let kind = match access.kind {
            ast::AccessKind::Potential => AccessKind::Potential,
            ast::AccessKind::Flow => AccessKind::Flow,
        };
        let branch = self.resolve_branch(&access.args)?;
        Ok(Access { kind, branch })
    }

    fn resolve_branch(&mut self, args: &[String]) -> Result<BranchId, FrontendError> {
        // A single argument may be a declared branch name (e.g. `V(br_rseries)`).
        if args.len() == 1 {
            if let Some(id) = self.named_branches.get(&args[0]) {
                return Ok(*id);
            }
        }
        let p = *self
            .nodes
            .get(&args[0])
            .ok_or_else(|| elab(format!("unknown net `{}` in branch access", args[0])))?;
        let n = if args.len() >= 2 {
            *self
                .nodes
                .get(&args[1])
                .ok_or_else(|| elab(format!("unknown net `{}` in branch access", args[1])))?
        } else {
            self.reference_node()
        };
        let key = (p.0, n.0);
        if let Some(id) = self.branches.get(&key) {
            return Ok(*id);
        }
        let id = BranchId(self.out.branches.len() as u32);
        self.out.branches.push(Branch { p, n });
        self.branches.insert(key, id);
        Ok(id)
    }

    /// The implicit global reference node, created on first single-terminal access.
    fn reference_node(&mut self) -> NodeId {
        if let Some(id) = self.ground {
            return id;
        }
        let id = self.intern_node("gnd", Discipline::Electrical);
        self.ground = Some(id);
        id
    }
}

// --- free helpers --------------------------------------------------------------------

fn elab(msg: String) -> FrontendError {
    FrontendError::Elaborate(msg)
}

/// Collect every assignment-target name in a statement list (recursing through control flow),
/// used to discover a function's local variables before lowering its body.
fn collect_assign_targets(stmts: &[Stmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { lhs, .. } => out.push(lhs.clone()),
            Stmt::VarDecl { names } => out.extend(names.iter().cloned()),
            Stmt::Block(body) => collect_assign_targets(body, out),
            Stmt::If { then_, else_, .. } => {
                collect_assign_targets(then_, out);
                collect_assign_targets(else_, out);
            }
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => {
                collect_assign_targets(body, out)
            }
            Stmt::For {
                init, step, body, ..
            } => {
                collect_assign_targets(std::slice::from_ref(&**init), out);
                collect_assign_targets(std::slice::from_ref(&**step), out);
                collect_assign_targets(body, out);
            }
            Stmt::Case { arms, default, .. } => {
                for arm in arms {
                    collect_assign_targets(&arm.body, out);
                }
                if let Some(body) = default {
                    collect_assign_targets(body, out);
                }
            }
            Stmt::Contribute { .. } => {}
        }
    }
}

fn bool_to_f64(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// Map a range bound value to an optional inclusive bound; an infinite bound is unbounded.
fn bound(v: f64) -> Option<f64> {
    if v.is_infinite() {
        None
    } else {
        Some(v)
    }
}

/// Map a surface [`ast::BinOp`] to the IR's. The IR has no `!=`/`&&`/`||` in v0.
fn map_binop(op: ast::BinOp) -> Result<va_ir::BinOp, FrontendError> {
    use ast::BinOp as A;
    use va_ir::BinOp as B;
    Ok(match op {
        A::Add => B::Add,
        A::Sub => B::Sub,
        A::Mul => B::Mul,
        A::Div => B::Div,
        A::Pow => B::Pow,
        A::Lt => B::Lt,
        A::Le => B::Le,
        A::Gt => B::Gt,
        A::Ge => B::Ge,
        A::Eq => B::Eq,
        A::Ne | A::And | A::Or => {
            return Err(elab(format!(
                "operator `{op:?}` is not supported in the v0 IR"
            )))
        }
    })
}

/// Map a call-syntax function name to a math [`Builtin`].
fn call_builtin(name: &str) -> Result<Builtin, FrontendError> {
    Ok(match name {
        "exp" => Builtin::Exp,
        "ln" => Builtin::Ln,
        "log" => Builtin::Log,
        "sqrt" => Builtin::Sqrt,
        "abs" => Builtin::Abs,
        "pow" => Builtin::Pow,
        "hypot" => Builtin::Hypot,
        "atan2" => Builtin::Atan2,
        "min" => Builtin::Min,
        "max" => Builtin::Max,
        "sin" => Builtin::Sin,
        "cos" => Builtin::Cos,
        "tan" => Builtin::Tan,
        "sinh" => Builtin::Sinh,
        "cosh" => Builtin::Cosh,
        "tanh" => Builtin::Tanh,
        "asin" => Builtin::Asin,
        "acos" => Builtin::Acos,
        "atan" => Builtin::Atan,
        "asinh" => Builtin::Asinh,
        "acosh" => Builtin::Acosh,
        "atanh" => Builtin::Atanh,
        "ddt" => Builtin::Ddt,
        "idt" => Builtin::Idt,
        other => return Err(elab(format!("unknown function `{other}`"))),
    })
}

/// Map a system-function name (no leading `$`) to a [`Builtin`].
fn sysfunc_builtin(name: &str) -> Result<Builtin, FrontendError> {
    Ok(match name {
        "vt" => Builtin::Vt,
        "temperature" => Builtin::Temperature,
        other => return Err(elab(format!("unknown system function `${other}`"))),
    })
}

/// Evaluate a binary operator on two constants (used for parameter folding).
fn eval_binop(op: ast::BinOp, a: f64, b: f64) -> f64 {
    use ast::BinOp::*;
    match op {
        Add => a + b,
        Sub => a - b,
        Mul => a * b,
        Div => a / b,
        Pow => a.powf(b),
        Lt => bool_to_f64(a < b),
        Le => bool_to_f64(a <= b),
        Gt => bool_to_f64(a > b),
        Ge => bool_to_f64(a >= b),
        Eq => bool_to_f64(a == b),
        Ne => bool_to_f64(a != b),
        And => bool_to_f64(a != 0.0 && b != 0.0),
        Or => bool_to_f64(a != 0.0 || b != 0.0),
    }
}

/// Evaluate a real math built-in numerically during constant folding.
fn eval_const_call(name: &str, args: &[f64]) -> Result<f64, FrontendError> {
    let arity_err = || {
        elab(format!(
            "wrong argument count for `{name}` in constant context"
        ))
    };
    let arg1 = || args.first().copied().ok_or_else(arity_err);
    let arg2 = || match (args.first(), args.get(1)) {
        (Some(x), Some(y)) => Ok((*x, *y)),
        _ => Err(arity_err()),
    };
    Ok(match name {
        "exp" => arg1()?.exp(),
        "ln" => arg1()?.ln(),
        "log" => arg1()?.log10(),
        "sqrt" => arg1()?.sqrt(),
        "abs" => arg1()?.abs(),
        "sin" => arg1()?.sin(),
        "cos" => arg1()?.cos(),
        "tan" => arg1()?.tan(),
        "sinh" => arg1()?.sinh(),
        "cosh" => arg1()?.cosh(),
        "tanh" => arg1()?.tanh(),
        "asin" => arg1()?.asin(),
        "acos" => arg1()?.acos(),
        "atan" => arg1()?.atan(),
        "asinh" => arg1()?.asinh(),
        "acosh" => arg1()?.acosh(),
        "atanh" => arg1()?.atanh(),
        "pow" => {
            let (x, y) = arg2()?;
            x.powf(y)
        }
        "atan2" => {
            let (y, x) = arg2()?;
            y.atan2(x)
        }
        "hypot" => {
            let (x, y) = arg2()?;
            x.hypot(y)
        }
        "min" => {
            let (x, y) = arg2()?;
            x.min(y)
        }
        "max" => {
            let (x, y) = arg2()?;
            x.max(y)
        }
        other => return Err(elab(format!("`{other}` is not constant-evaluable"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::lex, parser::parse};

    fn elaborate_src(src: &str) -> Module {
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        elaborate(&ast).expect("elaborate")
    }

    #[test]
    fn resistor_elaborates() {
        let m = elaborate_src(include_str!("../../../models/resistor.va"));
        assert_eq!(m.name, "resistor");
        assert_eq!(m.nodes.len(), 2);
        assert_eq!(m.ports.len(), 2);
        assert_eq!(m.branches.len(), 1);

        assert_eq!(m.params.len(), 1);
        let r = &m.params[0];
        assert_eq!(r.name, "R");
        assert_eq!(r.default, 1000.0);
        assert_eq!(r.min, Some(0.0)); // from (0:inf)
        assert_eq!(r.max, None); // inf → unbounded

        assert_eq!(m.analog.len(), 1);
        match &m.analog[0] {
            va_ir::Stmt::Contribute { target, value } => {
                assert_eq!(target.kind, AccessKind::Flow);
                assert!(matches!(
                    m.expr(*value),
                    Expr::Binary(va_ir::BinOp::Div, _, _)
                ));
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn diode_maps_exp_and_vt() {
        let m = elaborate_src(include_str!("../../../models/diode.va"));
        assert_eq!(m.params.len(), 2);
        // $vt → Builtin::Vt, exp(...) → Builtin::Exp.
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, Expr::Call(Builtin::Vt, _))));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, Expr::Call(Builtin::Exp, _))));
    }

    #[test]
    fn capacitor_maps_ddt() {
        let m = elaborate_src(include_str!("../../../models/capacitor.va"));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, Expr::Call(Builtin::Ddt, _))));
    }

    #[test]
    fn probe_resolves_to_param_and_branch() {
        let m = elaborate_src(include_str!("../../../models/resistor.va"));
        // The divisor of I <+ V/R must be Param(R).
        let div = m
            .exprs
            .iter()
            .find_map(|e| match e {
                Expr::Binary(va_ir::BinOp::Div, _, rhs) => Some(*rhs),
                _ => None,
            })
            .expect("a division");
        assert!(matches!(m.expr(div), Expr::Param(_)));
    }

    #[test]
    fn declared_module_variable_is_registered_and_usable() {
        // `real q, v;` declared at module scope; both are usable in the analog block, and a
        // declared-but-unassigned variable still becomes an IR var.
        let src = "module t(p, n); electrical p, n; real q, v; analog begin v = V(p, n); q = v; I(p, n) <+ q; end endmodule";
        let m = elaborate_src(src);
        let names: Vec<&str> = m.vars.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"q"));
        assert!(names.contains(&"v"));
        // No duplicate registration despite `q`/`v` also being assignment targets.
        assert_eq!(m.vars.iter().filter(|d| d.name == "q").count(), 1);
        assert_eq!(m.vars.iter().filter(|d| d.name == "v").count(), 1);
    }

    #[test]
    fn block_local_variable_is_registered_and_lowered() {
        // A named block with a local `real x;` declaration; `x` becomes an IR variable and
        // the declaration lowers to a no-op (empty block).
        let src = "module t(a, b); electrical a, b; analog begin : blk real x; x = V(a, b); I(a, b) <+ x; end endmodule";
        let m = elaborate_src(src);
        assert!(
            m.vars.iter().any(|d| d.name == "x"),
            "x should be a variable"
        );
        // The block lowers; the declaration contributes an empty block, not a statement error.
        assert!(!m.analog.is_empty());
    }

    #[test]
    fn named_branch_resolves_and_coincides_with_positional() {
        // `V(br)`/`I(br)` and `V(a,b)` all refer to the one declared branch.
        let src = "module t(a, b); electrical a, b; branch (a, b) br; analog begin I(br) <+ V(a, b); end endmodule";
        let m = elaborate_src(src);
        assert_eq!(
            m.branches.len(),
            1,
            "named and positional access share one branch"
        );
        match &m.analog[0] {
            va_ir::Stmt::Contribute { target, value } => {
                assert_eq!(target.kind, AccessKind::Flow);
                // The probe `V(a,b)` resolves to the same branch as the named target `I(br)`.
                assert!(matches!(
                    m.expr(*value),
                    va_ir::Expr::Probe(a) if a.branch == target.branch
                ));
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn unknown_identifier_is_rejected() {
        let src = "module t(); electrical a; analog begin I(a) <+ Z; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        let err = elaborate(&ast).unwrap_err();
        assert!(matches!(err, FrontendError::Elaborate(_)));
    }

    #[test]
    fn for_loop_lowers_to_ir() {
        // `for`/`while`/`repeat`/`case` now lower into the corresponding IR nodes.
        let src = "module t(); electrical a; analog begin for (i = 0; i < 3; i = i + 1) I(a) <+ 1.0; end endmodule";
        let m = elaborate_src(src);
        match &m.analog[0] {
            va_ir::Stmt::For {
                init, step, body, ..
            } => {
                assert!(matches!(**init, va_ir::Stmt::Assign { .. }));
                assert!(matches!(**step, va_ir::Stmt::Assign { .. }));
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected a lowered for-loop, got {other:?}"),
        }
    }

    #[test]
    fn case_lowers_with_arms_and_default() {
        let src = "module t(); parameter real sel = 1.0; electrical a; analog begin case (sel) 0: I(a) <+ 1.0; default: I(a) <+ 0.0; endcase end endmodule";
        let m = elaborate_src(src);
        match &m.analog[0] {
            va_ir::Stmt::Case { arms, default, .. } => {
                assert_eq!(arms.len(), 1);
                assert_eq!(arms[0].labels.len(), 1);
                assert_eq!(default.len(), 1);
            }
            other => panic!("expected a lowered case, got {other:?}"),
        }
    }

    #[test]
    fn analog_function_lowers_and_call_resolves() {
        // The function lowers to a Function node; a call to it lowers to Expr::CallUser.
        let src = "module t(p, n); electrical p, n; analog function real sq; input x; real x; sq = x * x; endfunction analog begin I(p, n) <+ sq(V(p, n)); end endmodule";
        let m = elaborate_src(src);
        assert_eq!(m.functions.len(), 1);
        let f = &m.functions[0];
        assert_eq!(f.name, "sq");
        assert_eq!(f.args.len(), 1);
        // The function body assigns to its return variable.
        assert!(matches!(f.body[0], va_ir::Stmt::Assign { lhs, .. } if lhs == f.ret));
        // The analog block calls it via CallUser.
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::CallUser(va_ir::FuncId(0), _))));
    }

    #[test]
    fn unknown_function_call_is_rejected() {
        // A call to a name that is neither a built-in nor a user function is an error.
        let src =
            "module t(p, n); electrical p, n; analog begin I(p, n) <+ nope(V(p, n)); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn port_without_discipline_is_rejected() {
        let src = "module t(p); inout p; analog begin end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }
}
