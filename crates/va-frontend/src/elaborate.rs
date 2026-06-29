//! Elaboration: surface [`crate::ast::ModuleAst`] → frozen [`va_ir::Module`] (Interface α).
//!
//! Resolves net/branch/parameter/variable names to arena indices and lowers expressions and
//! statements into the IR. This is the only place the frontend touches `va-ir`.
//!
//! # Passes
//!
//! 1. **Nodes** — intern every net declared with a discipline; resolve the port list.
//! 2. **Parameters** — const-evaluate each default and range bound into `f64`.
//! 3. **Variables** — register every assignment target as a local variable.
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

use std::collections::HashMap;

use crate::ast::{self, ExprAst, ExprRef, Item, ModuleAst, Stmt};
use crate::FrontendError;
use va_ir::{
    Access, AccessKind, Branch, BranchId, Builtin, Discipline, Expr, ExprId, Module, NodeDecl,
    NodeId, Param, ParamId, VarDecl, VarId,
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
        branches: HashMap::new(),
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
    vars: HashMap<String, VarId>,
    branches: HashMap<(u32, u32), BranchId>,
    ground: Option<NodeId>,
}

impl Elaborator<'_> {
    fn run(&mut self) -> Result<(), FrontendError> {
        self.collect_nodes()?;
        self.resolve_ports()?;
        self.collect_params()?;
        self.collect_vars();
        self.lower_analog()?;
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

    fn collect_vars_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign { lhs, .. } => {
                if !self.params.contains_key(lhs) && !self.vars.contains_key(lhs) {
                    let id = VarId(self.out.vars.len() as u32);
                    self.out.vars.push(VarDecl { name: lhs.clone() });
                    self.vars.insert(lhs.clone(), id);
                }
            }
            Stmt::Block(body) => body.iter().for_each(|s| self.collect_vars_stmt(s)),
            Stmt::If { then_, else_, .. } => {
                then_.iter().for_each(|s| self.collect_vars_stmt(s));
                else_.iter().for_each(|s| self.collect_vars_stmt(s));
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
                let builtin = call_builtin(name)?;
                let mut ids = Vec::with_capacity(args.len());
                for &a in args {
                    ids.push(self.lower_expr(a)?);
                }
                Expr::Call(builtin, ids)
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
    Ok(match name {
        "exp" => arg1()?.exp(),
        "ln" => arg1()?.ln(),
        "log" => arg1()?.log10(),
        "sqrt" => arg1()?.sqrt(),
        "abs" => arg1()?.abs(),
        "pow" => match (args.first(), args.get(1)) {
            (Some(x), Some(y)) => x.powf(*y),
            _ => return Err(arity_err()),
        },
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
    fn unknown_identifier_is_rejected() {
        let src = "module t(); electrical a; analog begin I(a) <+ Z; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        let err = elaborate(&ast).unwrap_err();
        assert!(matches!(err, FrontendError::Elaborate(_)));
    }

    #[test]
    fn port_without_discipline_is_rejected() {
        let src = "module t(p); inout p; analog begin end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }
}
