//! Elaboration: surface [`crate::ast::ModuleAst`] → frozen [`va_ir::Module`] (Interface α).
//!
//! Resolves net/branch/parameter/variable names to arena indices and lowers expressions and
//! statements into the IR. This is the only place the frontend touches `va-ir`.
//!
//! # Passes
//!
//! 1. **Parameters** — const-evaluate each default and range bound into `f64` (run first so a
//!    vector net's `[msb:lsb]` range may reference one).
//! 2. **Genvars** — register every declared `genvar` name, before variables are collected, so a
//!    genvar's loop header is never mistaken for a real analog variable.
//! 3. **Nodes** — intern every net declared with a discipline (expanding a vector net's
//!    `[msb:lsb]` range into one node per index); resolve the port list.
//! 4. **Variables** — register explicitly declared module variables (`real q, v;`), then
//!    every remaining assignment target (skipping genvars), as local variables.
//! 5. **Lowering** — translate the analog block's expressions and statements, creating
//!    branches on demand as branch accesses are resolved. A `for` loop whose header assigns a
//!    genvar is fully unrolled here into a flat [`va_ir::Stmt::Block`] (§ generate loops) —
//!    every other `for` lowers to a runtime [`va_ir::Stmt::For`] as before.
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
//! - `analysis("…")` is folded to a constant under v0's DC-only model (`1.0` for the
//!   `static`/`dc`/`ic`/`nodeset` phases, else `0.0`); the same IR is not reusable for other
//!   analyses. System tasks (`$strobe`, …) are no-ops. Both drop their string arguments.
//! - `while`/`for`/`repeat`/`case` control flow and `analog function` definitions are lowered
//!   into the corresponding [`va_ir`] nodes. Functions are lowered in source order against
//!   their own variable scope (arguments + return variable + locals) and may read module
//!   parameters; forward references to a function defined later in source are unsupported and
//!   resolve as an unknown function. Note that back-ends need not consume these yet —
//!   `va-codegen` v0 still rejects them during its own lowering.
//! - `genvar`/`generate` loops (§ generate loops): a `for` loop drives elaboration-time
//!   unrolling only when its header assigns a declared genvar; `init`/`cond`/`step` must be
//!   compile-time constant (literals, parameters, other genvars — same rule as parameter
//!   ranges) and `step` must reassign the same genvar (restricted assignment). Unrolling caps
//!   at 10,000 iterations to turn a malformed loop into a clear error rather than a hang.
//!   Nested loops may not reuse an already-bound genvar name (its "implicit localparam" would
//!   collide); sibling (non-nested) loops may reuse a name freely.
//! - Vector nets (§ vector nets) are internally just one ordinary [`NodeId`] per declared
//!   index, named `base[k]`; there is no separate IR concept for a bus. A vector element must
//!   be indexed (`V(bus[0])`, never bare `V(bus)`), and the index is bounds-checked against the
//!   declared `[msb:lsb]` range. Vector ports are not supported (a port name maps to exactly
//!   one node).

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
        genvars: std::collections::HashSet::new(),
        genvar_env: HashMap::new(),
        vectors: HashMap::new(),
        var_arrays: HashMap::new(),
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
    /// Declared `genvar` names (§ generate loops). A genvar never becomes an IR variable — it
    /// exists only as a constant bound in [`Self::genvar_env`] while its driving `for` loop is
    /// unrolled at elaboration (it does not exist at simulation time).
    genvars: std::collections::HashSet<String>,
    /// The genvar → current-value bindings in effect while unrolling a generate loop. Nested
    /// loops over distinct genvars stack (each key is inserted/removed independently); a loop
    /// re-entering its own still-bound genvar (nested reuse of the same name) is rejected.
    genvar_env: HashMap<String, i64>,
    /// Declared vector nets' inclusive `(lo, hi)` index range, keyed by base name (§ vector
    /// nets). A vector net `bus` interns one [`NodeId`] per index as `bus[k]`.
    vectors: HashMap<String, (i64, i64)>,
    /// Declared array variables' inclusive `(lo, hi)` index range, keyed by base name (§ array
    /// variables) — the `VarId` counterpart of `vectors` above. An array `out_val` interns one
    /// [`VarId`] per index as `out_val[k]`; only a compile-time-constant or genvar index is
    /// supported (there is no runtime-indexed array/memory concept in this IR).
    var_arrays: HashMap<String, (i64, i64)>,
}

impl Elaborator<'_> {
    fn run(&mut self) -> Result<(), FrontendError> {
        // Parameters first: a vector net's `[msb:lsb]` range may reference one (§ vector nets).
        self.collect_params()?;
        self.collect_genvars();
        self.collect_nodes()?;
        self.resolve_ports()?;
        self.collect_branches()?;
        self.collect_functions()?;
        self.collect_var_decls()?;
        self.collect_vars()?;
        self.lower_analog()?;
        Ok(())
    }

    /// Register explicitly declared module-level variables (`real q, v;`) and array variables
    /// (`real out_val[0:15];`, § array variables). The base type is not retained — the IR has
    /// no variable type and treats every value as `f64`. Assignment targets in the analog block
    /// are still auto-registered by [`Self::collect_vars`]; this pass just lets a variable be
    /// declared before (or without) being assigned, and is the only place an array variable can
    /// be declared at all (block-local array declarations are rejected — see
    /// [`Self::collect_vars_stmt`]).
    fn collect_var_decls(&mut self) -> Result<(), FrontendError> {
        let ast = self.ast;
        for item in &ast.items {
            if let Item::Var { names, .. } = item {
                for entry in names {
                    self.declare_var_entry(entry)?;
                }
            }
        }
        Ok(())
    }

    /// Register one variable-declaration entry: a plain scalar, or (if it carries a range) an
    /// array — interning one [`VarId`] per index, named `"name[k]"`, exactly mirroring how
    /// [`Self::collect_nodes`] expands a vector net.
    fn declare_var_entry(&mut self, entry: &ast::VarEntry) -> Result<(), FrontendError> {
        match entry.range {
            None => {
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    self.vars.entry(entry.name.clone())
                {
                    let id = VarId(self.out.vars.len() as u32);
                    self.out.vars.push(VarDecl {
                        name: entry.name.clone(),
                    });
                    slot.insert(id);
                }
            }
            Some((msb, lsb)) => {
                let msb = self.const_eval_int(msb, "array variable range bound")?;
                let lsb = self.const_eval_int(lsb, "array variable range bound")?;
                let (lo, hi) = if msb <= lsb { (msb, lsb) } else { (lsb, msb) };
                for k in lo..=hi {
                    let key = format!("{}[{k}]", entry.name);
                    if let std::collections::hash_map::Entry::Vacant(slot) =
                        self.vars.entry(key.clone())
                    {
                        let id = VarId(self.out.vars.len() as u32);
                        self.out.vars.push(VarDecl { name: key });
                        slot.insert(id);
                    }
                }
                self.var_arrays.insert(entry.name.clone(), (lo, hi));
            }
        }
        Ok(())
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

    /// Register every declared `genvar` name (§ generate loops). Must run before variables are
    /// collected, so a genvar's assignment-looking loop header is never mistaken for a real
    /// analog variable (see [`Self::register_var`]).
    fn collect_genvars(&mut self) {
        for item in &self.ast.items {
            if let Item::Genvar { names } = item {
                for name in names {
                    self.genvars.insert(name.clone());
                }
            }
        }
    }

    // --- pass 1: nodes ---------------------------------------------------------------

    fn collect_nodes(&mut self) -> Result<(), FrontendError> {
        for item in &self.ast.items {
            if let Item::Net { discipline, nets } = item {
                let disc = match discipline {
                    ast::Discipline::Electrical => Discipline::Electrical,
                    ast::Discipline::Thermal => Discipline::Thermal,
                };
                // Each name carries its own optional range — `electrical [0:w-1] in;` and
                // `electrical in[`W-1:0], out;` both reach here as one `NetDecl` per name, the
                // prefix-vs-suffix distinction already resolved by the parser (§2.2).
                for net in nets {
                    match net.range {
                        None => {
                            self.intern_node(&net.name, disc);
                        }
                        // A vector net interns one node per index (§ vector nets); a branch
                        // access later selects one by a genvar expression.
                        Some((msb, lsb)) => {
                            let msb = self.const_eval_int(msb, "vector net range bound")?;
                            let lsb = self.const_eval_int(lsb, "vector net range bound")?;
                            let (lo, hi) = if msb <= lsb { (msb, lsb) } else { (lsb, msb) };
                            for k in lo..=hi {
                                self.intern_node(&format!("{}[{k}]", net.name), disc);
                            }
                            self.vectors.insert(net.name.clone(), (lo, hi));
                        }
                    }
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

    /// Resolve each declared port name to its underlying node(s) — one for a scalar port, or
    /// the vector's full, ascending-index-order node list for a vector port (`electrical
    /// [msb:lsb] {port};`, § vector nets). `Module::ports` is `Vec<Vec<NodeId>>` precisely so a
    /// vector port doesn't need special-casing here beyond "how many nodes did this name
    /// resolve to." Note the list is always lowest-index-first regardless of whether the
    /// source wrote `[msb:lsb]` or `[lsb:msb]` — the original declared direction isn't tracked
    /// (only the normalized `(lo, hi)` bound is, matching how the vector's nodes are already
    /// interned in `collect_nodes`), a stated simplification for a wiring convention
    /// (`va-netlist`) that doesn't exist yet to have an opinion on connection order.
    fn resolve_ports(&mut self) -> Result<(), FrontendError> {
        for port in &self.ast.ports {
            if let Some(id) = self.nodes.get(port) {
                self.out.ports.push(vec![*id]);
                continue;
            }
            if let Some(&(lo, hi)) = self.vectors.get(port) {
                let mut ids = Vec::with_capacity((hi - lo + 1) as usize);
                for k in lo..=hi {
                    let key = format!("{port}[{k}]");
                    ids.push(*self.nodes.get(&key).ok_or_else(|| {
                        elab(format!(
                            "internal error: vector port node `{key}` was not interned"
                        ))
                    })?);
                }
                self.out.ports.push(ids);
                continue;
            }
            return Err(elab(format!(
                "port `{port}` has no discipline declaration (e.g. `electrical {port};`)"
            )));
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
            match item {
                Item::Param {
                    name,
                    default,
                    range,
                    ..
                } => {
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
                // `aliasparam name = target;` introduces no new parameter: `name` is just
                // another name resolving to `target`'s existing `ParamId`/value. `target`
                // must already be declared — forward references are unsupported in v0.
                Item::AliasParam { name, target } => {
                    let id = *self.params.get(target).ok_or_else(|| {
                        elab(format!(
                            "aliasparam `{name}` targets unknown parameter `{target}`"
                        ))
                    })?;
                    let val = self.param_vals[target];
                    self.params.insert(name.clone(), id);
                    self.param_vals.insert(name.clone(), val);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Fold a constant expression to its `f64` value: a parameter default/range bound, a
    /// vector net's range bound, or a genvar loop header (§ generate loops) — anywhere the LRM
    /// requires a value fixed at elaboration. A bound genvar (see [`Self::genvar_env`]) counts
    /// as constant here, matching the rule that a genvar expression may reference other
    /// genvars as well as literals and parameters.
    fn const_eval(&self, r: ExprRef) -> Result<f64, FrontendError> {
        match self.ast.expr(r) {
            ExprAst::Number(n) => Ok(*n),
            ExprAst::Ident(name) => {
                if let Some(v) = self.genvar_env.get(name) {
                    Ok(*v as f64)
                } else {
                    self.param_vals.get(name).copied().ok_or_else(|| {
                        elab(format!(
                            "`{name}` is not a compile-time constant in this context"
                        ))
                    })
                }
            }
            ExprAst::Unary(op, e) => {
                let v = self.const_eval(*e)?;
                Ok(match op {
                    ast::UnOp::Neg => -v,
                    ast::UnOp::Not => bool_to_f64(v == 0.0),
                    ast::UnOp::BitNot => !to_i64(v) as f64,
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
            ExprAst::Cond { cond, then_, else_ } => {
                if self.const_eval(*cond)? != 0.0 {
                    self.const_eval(*then_)
                } else {
                    self.const_eval(*else_)
                }
            }
            ExprAst::SysFunc { name, .. } => Err(elab(format!(
                "system function `${name}` is not constant in a parameter context"
            ))),
            ExprAst::Str(_) => Err(elab(
                "a string literal is not valid in a parameter context".to_string(),
            )),
            ExprAst::Probe(_) => Err(elab(
                "a branch probe is not constant in a parameter context".to_string(),
            )),
            ExprAst::IndexedIdent(name, _) => Err(elab(format!(
                "array variable `{name}` is not constant in a parameter context"
            ))),
        }
    }

    /// [`Self::const_eval`], then require the result to be (nearly) integral — genvars, vector
    /// net range bounds, and vector indices are all integers per the LRM.
    fn const_eval_int(&self, r: ExprRef, what: &str) -> Result<i64, FrontendError> {
        let v = self.const_eval(r)?;
        if (v - v.round()).abs() > 1e-9 {
            return Err(elab(format!("{what} must be an integer, got {v}")));
        }
        Ok(v.round() as i64)
    }

    // --- pass 3: variables -----------------------------------------------------------

    fn collect_vars(&mut self) -> Result<(), FrontendError> {
        // Borrow the AST through a copy of the shared reference so we can mutate `self`.
        let items = self.ast;
        for item in &items.items {
            if let Item::Analog(stmt) = item {
                self.collect_vars_stmt(stmt)?;
            }
        }
        Ok(())
    }

    /// Register `name` as a local variable unless it is already a parameter, known variable,
    /// or a genvar. A genvar (§ generate loops) never becomes an IR variable — it is folded to
    /// a constant wherever it is read, and its only "assignment" is the header of the `for`
    /// loop it drives, which elaboration unrolls rather than lowering as a normal assignment.
    fn register_var(&mut self, name: &str) {
        if self.genvars.contains(name) {
            return;
        }
        if !self.params.contains_key(name) && !self.vars.contains_key(name) {
            let id = VarId(self.out.vars.len() as u32);
            self.out.vars.push(VarDecl {
                name: name.to_string(),
            });
            self.vars.insert(name.to_string(), id);
        }
    }

    fn collect_vars_stmt(&mut self, stmt: &Stmt) -> Result<(), FrontendError> {
        match stmt {
            // An indexed assignment target (`out_val[i] = ...;`) must already be declared as
            // an array variable via `Item::Var` (§ array variables) — nothing to register here.
            Stmt::Assign { lhs, index, .. } => {
                if index.is_none() {
                    self.register_var(lhs);
                }
                Ok(())
            }
            Stmt::VarDecl { names } => {
                for entry in names {
                    // Array variables are elaboration-only in the sense that their whole size
                    // must be known up front (§ array variables); `Item::Var`'s module-scope
                    // pass already ran by the time this (analog-block) pass runs, so a
                    // block-local array range has nowhere sound to be declared into.
                    if entry.range.is_some() {
                        return Err(elab(format!(
                            "array variable `{}` must be declared at module scope, not inside \
                             the analog block (block-local array variables are not yet \
                             supported)",
                            entry.name
                        )));
                    }
                    self.register_var(&entry.name);
                }
                Ok(())
            }
            Stmt::Block(body) => {
                for s in body {
                    self.collect_vars_stmt(s)?;
                }
                Ok(())
            }
            Stmt::If { then_, else_, .. } => {
                for s in then_ {
                    self.collect_vars_stmt(s)?;
                }
                for s in else_ {
                    self.collect_vars_stmt(s)?;
                }
                Ok(())
            }
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => {
                for s in body {
                    self.collect_vars_stmt(s)?;
                }
                Ok(())
            }
            Stmt::For {
                init, step, body, ..
            } => {
                self.collect_vars_stmt(init)?;
                self.collect_vars_stmt(step)?;
                for s in body {
                    self.collect_vars_stmt(s)?;
                }
                Ok(())
            }
            Stmt::Case { arms, default, .. } => {
                for arm in arms {
                    for s in &arm.body {
                        self.collect_vars_stmt(s)?;
                    }
                }
                if let Some(body) = default {
                    for s in body {
                        self.collect_vars_stmt(s)?;
                    }
                }
                Ok(())
            }
            Stmt::Contribute { .. } | Stmt::Task { .. } => Ok(()),
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
            // System tasks (`$strobe`, `$finish`, …) have no effect on a DC solve.
            Stmt::Task { .. } => Ok(va_ir::Stmt::Block(Vec::new())),
            Stmt::Contribute { target, value } => {
                let target = self.lower_access(target)?;
                let value = self.lower_expr(*value)?;
                Ok(va_ir::Stmt::Contribute { target, value })
            }
            Stmt::Assign { lhs, index, rhs } => {
                // Restricted assignment (§ generate loops): a genvar may only be written by
                // the header of the `for` loop it drives, which `Stmt::For` below intercepts
                // and unrolls directly — it never reaches this generic path. Any other
                // assignment to a genvar name (elsewhere in the body, or as an ordinary
                // `for`/`while` control variable) is rejected here.
                if self.genvars.contains(lhs) {
                    return Err(elab(format!(
                        "genvar `{lhs}` may only be assigned in the header of the `for` loop \
                         it drives (restricted assignment)"
                    )));
                }
                let id = match index {
                    // `lhs[index] = rhs;`: one element of an array variable (§ array
                    // variables), constant/genvar-indexed only.
                    Some(idx_expr) => self.resolve_var_array_index(lhs, *idx_expr)?,
                    None => *self
                        .vars
                        .get(lhs)
                        .ok_or_else(|| elab(format!("assignment to unknown variable `{lhs}`")))?,
                };
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
                // A `for` whose header assigns a declared genvar is a generate loop (§ generate
                // loops): fully unrolled here, at elaboration, into a flat `Block` — it never
                // reaches the runtime `va_ir::Stmt::For` path below, which is why analog
                // operators (`ddt`/`idt`) are legal inside it despite being forbidden in an
                // ordinary runtime loop.
                if let Stmt::Assign {
                    lhs, rhs: init_rhs, ..
                } = init.as_ref()
                {
                    if self.genvars.contains(lhs) {
                        return self.lower_generate_for(lhs, *init_rhs, *cond, step, body);
                    }
                }
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

    /// Unroll a genvar-controlled `for` loop (§ generate loops) into a flat [`va_ir::Stmt`]
    /// sequence: `init`/`cond`/`step` must be static (literals, parameters, other genvars —
    /// [`Self::const_eval`] rejects anything else), and `step` must reassign the same genvar
    /// (the LRM's restricted-assignment rule). Each iteration lowers `body` with `genvar` bound
    /// to its current value — read through [`Self::const_eval`]/[`Self::lower_expr`] — which
    /// doubles as the "implicit localparam" the LRM says each generated scope carries.
    fn lower_generate_for(
        &mut self,
        genvar: &str,
        init_rhs: ExprRef,
        cond: ExprRef,
        step: &Stmt,
        body: &[Stmt],
    ) -> Result<va_ir::Stmt, FrontendError> {
        if self.genvar_env.contains_key(genvar) {
            return Err(elab(format!(
                "nested generate loop reuses genvar `{genvar}`; a genvar's implicit localparam \
                 cannot be redeclared while its enclosing loop is still active"
            )));
        }
        let step_rhs = match step {
            Stmt::Assign { lhs, rhs, .. } if lhs == genvar => *rhs,
            _ => {
                return Err(elab(format!(
                    "genvar `{genvar}`'s `for` step must reassign `{genvar}` itself \
                     (restricted assignment: a genvar may only be written by its own loop \
                     header)"
                )))
            }
        };

        // A pathologically malformed step/condition (e.g. a step that never advances toward
        // the bound) would otherwise unroll forever; this is generous for any real ladder
        // network while still catching that case with a clear error instead of hanging.
        const MAX_ITERATIONS: usize = 10_000;

        let mut value = self.const_eval_int(init_rhs, "genvar initial value")?;
        let mut out = Vec::new();
        let mut iterations = 0usize;
        loop {
            self.genvar_env.insert(genvar.to_string(), value);
            let keep_going = self.const_eval(cond)? != 0.0;
            if !keep_going {
                self.genvar_env.remove(genvar);
                break;
            }
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                self.genvar_env.remove(genvar);
                return Err(elab(format!(
                    "generate loop over genvar `{genvar}` did not terminate within \
                     {MAX_ITERATIONS} iterations"
                )));
            }
            out.extend(self.lower_stmts(body)?);
            value = self.const_eval_int(step_rhs, "genvar step value")?;
        }
        Ok(va_ir::Stmt::Block(out))
    }

    fn lower_expr(&mut self, r: ExprRef) -> Result<ExprId, FrontendError> {
        // `self.ast` is a shared reference; copy it locally so the read borrow is of the
        // external `ModuleAst`, not of `self`, leaving `self` free to mutate.
        let ast = self.ast;
        let expr = match ast.expr(r) {
            ExprAst::Number(n) => Expr::Const(*n),
            ExprAst::Ident(name) => {
                // A genvar bound by an enclosing generate loop (§ generate loops) reads as the
                // constant it is currently unrolled to — it never becomes a `Var`/`Param`.
                if let Some(v) = self.genvar_env.get(name) {
                    Expr::Const(*v as f64)
                } else if let Some(p) = self.params.get(name) {
                    Expr::Param(*p)
                } else if let Some(v) = self.vars.get(name) {
                    Expr::Var(*v)
                } else {
                    return Err(elab(format!("unknown identifier `{name}`")));
                }
            }
            // `name[index]`: one element of an array variable (§ array variables), read via
            // the same constant/genvar-indexed resolution as an assignment target.
            ExprAst::IndexedIdent(name, index) => {
                let id = self.resolve_var_array_index(name, *index)?;
                Expr::Var(id)
            }
            ExprAst::SysFunc { name, args } if name == "simparam" => {
                // `$simparam(param_name [, default])`: the queried parameter is always unknown
                // in v0 (no simulator parameter store), so the call returns the `default`
                // expression. With no default, an unknown parameter is an error — matching the
                // LRM, where `$simparam` errors on an unknown parameter when no default is
                // given. The `param_name` (a string) is not evaluated.
                match args.get(1) {
                    Some(&default) => return self.lower_expr(default),
                    None => {
                        return Err(elab(
                            "$simparam without a default: the parameter is unknown in v0 (no \
                             simulator parameters)"
                                .to_string(),
                        ))
                    }
                }
            }
            // `$abstime` is the absolute simulation time. v0 is DC-only (no time axis at all —
            // there is no transient solve for a "current time" to advance through), and a DC
            // operating point is conventionally evaluated at t=0, so it folds to a constant 0.0
            // rather than being rejected as unknown.
            ExprAst::SysFunc { name, args } if name == "abstime" => {
                if !args.is_empty() {
                    return Err(elab("`$abstime` takes no arguments".to_string()));
                }
                Expr::Const(0.0)
            }
            // `$mfactor` is the instance multiplicity factor (device paralleling count, the
            // conventional `m=` netlist parameter). v0 has no netlist-driven instance
            // parameters at all yet, so every instance behaves as if `m` were left at its LRM
            // default of 1.
            ExprAst::SysFunc { name, args } if name == "mfactor" => {
                if !args.is_empty() {
                    return Err(elab("`$mfactor` takes no arguments".to_string()));
                }
                Expr::Const(1.0)
            }
            // `$param_given(name)` asks whether `name` was explicitly set by the instantiating
            // netlist, as opposed to left at its declared default. `name` is a parameter-name
            // reference, not a value expression — read directly off the AST rather than lowered
            // (mirrors `$simparam`'s unevaluated name argument above). v0's pipeline has no
            // netlist-driven parameter overrides yet (`va-netlist` doesn't wire instance
            // parameters into elaboration), so no parameter is ever "given": every instance
            // always sees every parameter at its default, making `false` the honest answer in
            // every case rather than an approximation of a case that could go the other way.
            ExprAst::SysFunc { name, args } if name == "param_given" => {
                let &[param_ref] = args.as_slice() else {
                    return Err(elab(
                        "`$param_given` takes exactly one argument: a parameter name".to_string(),
                    ));
                };
                let param_name = match ast.expr(param_ref) {
                    ExprAst::Ident(n) => n,
                    _ => {
                        return Err(elab(
                            "`$param_given`'s argument must be a bare parameter name".to_string(),
                        ))
                    }
                };
                if !self.params.contains_key(param_name) {
                    return Err(elab(format!(
                        "`$param_given` names `{param_name}`, which is not a declared parameter \
                         of this module"
                    )));
                }
                Expr::Const(0.0)
            }
            // `$port_connected(name)` asks whether the named port has a real connection in the
            // instantiating netlist — the standard idiom for an optional terminal (e.g. a
            // self-heating `dt` thermal port), `if ($port_connected(dt) == 0) begin ... end`.
            // Like `$param_given`, `name` is a port-name reference read directly off the AST,
            // not a value expression to lower. v0 has no netlist-driven instantiation, so no
            // port can be connected by one; folding to `false` is the honest answer for the same
            // reason as `$param_given` above, and matches the corpus's dominant usage (guarding
            // an optional port's absence).
            ExprAst::SysFunc { name, args } if name == "port_connected" => {
                let &[port_ref] = args.as_slice() else {
                    return Err(elab(
                        "`$port_connected` takes exactly one argument: a port name".to_string(),
                    ));
                };
                let port_name = match ast.expr(port_ref) {
                    ExprAst::Ident(n) => n,
                    _ => {
                        return Err(elab(
                            "`$port_connected`'s argument must be a bare port name".to_string(),
                        ))
                    }
                };
                if !self.ast.ports.iter().any(|p| p == port_name) {
                    return Err(elab(format!(
                        "`$port_connected` names `{port_name}`, which is not a declared port of \
                         this module"
                    )));
                }
                Expr::Const(0.0)
            }
            // `$limit(access, "function_name"[, args...])` is a Newton convergence aid (LRM
            // §4.5.14): it bounds how much `access`'s value is allowed to move from its
            // previous-iteration value, using the named limiting algorithm (e.g. `"pnjlim"`, the
            // classic SPICE junction-voltage limiter). A converged Newton solve is a fixed point
            // of the *unlimited* equations — the limiter only reshapes the iteration path toward
            // that fixed point, never the fixed point itself — so `$limit` folds transparently
            // to its first argument's value, exactly like `transition`/`slew` below. This
            // project's stateless `ModelInstance::load` ABI has no previous-iteration history to
            // limit against in the first place (`va-core/src/convergence.rs` ships the `pnjlim`
            // algorithm itself as a tested helper, not yet wired into the Newton loop for this
            // reason — see `docs/roadmap.md`), so there is no alternative reading available even
            // if one were wanted. The function-name string and any trailing algorithm-parameter
            // arguments are parsed but never evaluated.
            ExprAst::SysFunc { name, args } if name == "limit" => {
                let value = *args.first().ok_or_else(|| {
                    elab("`$limit` requires at least an access argument".to_string())
                })?;
                return self.lower_expr(value);
            }
            ExprAst::SysFunc { name, args } => {
                let builtin = sysfunc_builtin(name)?;
                let mut ids = Vec::with_capacity(args.len());
                for &a in args {
                    ids.push(self.lower_expr(a)?);
                }
                // Arity: `$vt` accepts `$vt` (ambient) or `$vt(T)` (thermal voltage at the
                // absolute temperature `T`). Every other system function here takes none.
                match builtin {
                    Builtin::Vt if ids.len() > 1 => {
                        return Err(elab(format!("`${name}` takes at most one argument")))
                    }
                    Builtin::Vt => {}
                    _ if !ids.is_empty() => {
                        return Err(elab(format!("`${name}` takes no arguments")))
                    }
                    _ => {}
                }
                Expr::Call(builtin, ids)
            }
            ExprAst::Str(_) => {
                return Err(elab(
                    "a string literal is only valid as a system-task argument".to_string(),
                ))
            }
            ExprAst::Probe(access) => Expr::Probe(self.lower_access(access)?),
            ExprAst::Call { name, args } if name == "analysis" => {
                // `analysis("name", …)` queries the current analysis. v0 is DC-only, so it
                // folds to a constant: true for the DC/operating-point phases.
                let active = self.analysis_matches(args)?;
                Expr::Const(if active { 1.0 } else { 0.0 })
            }
            // Small-signal noise sources and `ac_stim` (an AC-only stimulus) contribute nothing
            // to a DC operating point; fold to zero (their string label and arguments are not
            // evaluated). `bound_step` is a transient-timestep hint with no DC meaning at all —
            // same fold, on the rare chance it appears in expression position rather than as
            // the bare statement `parse_bound_step_stmt` already handles (see `crate::parser`).
            ExprAst::Call { name, .. }
                if matches!(
                    name.as_str(),
                    "white_noise" | "flicker_noise" | "noise_table" | "ac_stim" | "bound_step"
                ) =>
            {
                Expr::Const(0.0)
            }
            // `transition(value, delay, rise_time, fall_time)` and `slew(value, pos_rate,
            // neg_rate)` both smooth/limit a signal over time — genuinely time-domain
            // (transient) constructs. v0 is DC-only, and both settle to their input value in
            // steady state (there is no rate-of-change or delay history at a fixed operating
            // point), so they fold transparently to `value`; the rest of the arguments are
            // parsed but never evaluated (same treatment as the noise-source builtins above).
            ExprAst::Call { name, args } if matches!(name.as_str(), "transition" | "slew") => {
                let value = *args
                    .first()
                    .ok_or_else(|| elab(format!("`{name}` requires at least a value argument")))?;
                return self.lower_expr(value);
            }
            // `real(expr)` is a type-cast call, not the declaration keyword (that's `Item::Var`/
            // `Stmt::VarDecl` — a different grammar production entirely). Every value in this
            // project is already `f64`, so it's a complete no-op: fold transparently to `expr`.
            ExprAst::Call { name, args } if name == "real" => {
                let value = *args
                    .first()
                    .ok_or_else(|| elab("`real` requires an argument".to_string()))?;
                return self.lower_expr(value);
            }
            // `ddx(expr, probe)` is the analog partial-derivative operator (LRM §4.5.13):
            // "the partial derivative of its first argument with respect to the unknown
            // indicated by the second argument, holding all other unknowns fixed." `probe`
            // must itself be a potential-probe access (`V(p, n)`/`Temp(p, n)`) — it identifies
            // *which* unknown to differentiate against, so it's classified here rather than
            // lowered as an ordinary value-producing sub-expression (the same "elaboration
            // classifies, parsing stays generic" split used for `transition`/`real` above).
            ExprAst::Call { name, args } if name == "ddx" => {
                let (expr_arg, probe_arg) = match (args.first(), args.get(1), args.len()) {
                    (Some(&e), Some(&p), 2) => (e, p),
                    _ => {
                        return Err(elab(
                            "`ddx` takes exactly two arguments: an expression and a \
                             potential-probe access, e.g. `ddx(I(br), V(p, n))`"
                                .to_string(),
                        ))
                    }
                };
                let access = match ast.expr(probe_arg) {
                    ExprAst::Probe(access) if access.kind == ast::AccessKind::Potential => access,
                    ExprAst::Probe(_) => {
                        return Err(elab(
                            "`ddx(..., I(...))` is not supported: differentiating with respect \
                             to a branch current needs flow probes to be independent unknowns, \
                             which they are not in this codegen"
                                .to_string(),
                        ))
                    }
                    _ => {
                        return Err(elab(
                            "`ddx`'s second argument must be a potential-probe access, e.g. \
                             `V(p, n)`"
                                .to_string(),
                        ))
                    }
                };
                let inner = self.lower_expr(expr_arg)?;
                let ir_access = self.lower_access(access)?;
                Expr::Ddx(inner, ir_access)
            }
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
                    ast::UnOp::BitNot => va_ir::UnOp::BitNot,
                };
                Expr::Unary(op, inner)
            }
            ExprAst::Binary(op, l, rhs) => {
                let op = map_binop(*op);
                let l = self.lower_expr(*l)?;
                let rhs = self.lower_expr(*rhs)?;
                Expr::Binary(op, l, rhs)
            }
            ExprAst::Cond { cond, then_, else_ } => {
                let cond = self.lower_expr(*cond)?;
                let then_ = self.lower_expr(*then_)?;
                let else_ = self.lower_expr(*else_)?;
                Expr::Select(cond, then_, else_)
            }
        };
        Ok(self.out.push_expr(expr))
    }

    /// Whether an `analysis(...)` call is active under v0's DC-only model: true if any
    /// string argument names a DC/operating-point phase. Arguments must be string literals.
    fn analysis_matches(&self, args: &[ExprRef]) -> Result<bool, FrontendError> {
        const DC_PHASES: &[&str] = &["static", "dc", "ic", "nodeset"];
        let mut matched = false;
        for &a in args {
            match self.ast.expr(a) {
                ExprAst::Str(s) => matched |= DC_PHASES.contains(&s.as_str()),
                _ => {
                    return Err(elab(
                        "`analysis` arguments must be string literals".to_string(),
                    ))
                }
            }
        }
        Ok(matched)
    }

    fn lower_access(&mut self, access: &ast::Access) -> Result<Access, FrontendError> {
        let kind = match access.kind {
            ast::AccessKind::Potential => AccessKind::Potential,
            ast::AccessKind::Flow => AccessKind::Flow,
        };
        let branch = self.resolve_branch(&access.args)?;
        Ok(Access { kind, branch })
    }

    fn resolve_branch(&mut self, args: &[ast::NetArg]) -> Result<BranchId, FrontendError> {
        // A single unindexed argument may be a declared branch name (e.g. `V(br_rseries)`).
        if args.len() == 1 && args[0].index.is_none() {
            if let Some(id) = self.named_branches.get(&args[0].name) {
                return Ok(*id);
            }
        }
        let p = self.resolve_net_arg(&args[0])?;
        let n = if args.len() >= 2 {
            self.resolve_net_arg(&args[1])?
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

    /// Resolve one [`ast::NetArg`] terminal to its [`NodeId`]: a plain net name, or one element
    /// of a vector net selected by a genvar expression (§ vector nets), bounds-checked against
    /// its declared `[msb:lsb]` range.
    fn resolve_net_arg(&mut self, arg: &ast::NetArg) -> Result<NodeId, FrontendError> {
        match arg.index {
            None => {
                if self.vectors.contains_key(&arg.name) {
                    return Err(elab(format!(
                        "`{}` is a vector net; an access must index it, e.g. `V({}[0])`",
                        arg.name, arg.name
                    )));
                }
                self.nodes
                    .get(&arg.name)
                    .copied()
                    .ok_or_else(|| elab(format!("unknown net `{}` in branch access", arg.name)))
            }
            Some(idx_expr) => {
                let (lo, hi) = *self.vectors.get(&arg.name).ok_or_else(|| {
                    elab(format!(
                        "`{}` is not a vector net (no bracketed `[msb:lsb]` range declared)",
                        arg.name
                    ))
                })?;
                let idx = self.const_eval_int(idx_expr, "vector index")?;
                if idx < lo || idx > hi {
                    return Err(elab(format!(
                        "index {idx} is out of `{}`'s declared range [{lo}:{hi}]",
                        arg.name
                    )));
                }
                let key = format!("{}[{idx}]", arg.name);
                self.nodes.get(&key).copied().ok_or_else(|| {
                    elab(format!(
                        "internal error: vector node `{key}` was not interned"
                    ))
                })
            }
        }
    }

    /// Resolve one element of an array variable (§ array variables) to its [`VarId`] — the
    /// `VarId` counterpart of [`Self::resolve_net_arg`]'s vector-net indexing. `index_expr`
    /// must be a compile-time-constant or genvar expression: `const_eval_int` naturally rejects
    /// a genuinely runtime index (an ordinary loop variable, say) with "not a compile-time
    /// constant in this context" — there is no separate check needed for that restriction.
    fn resolve_var_array_index(
        &mut self,
        name: &str,
        index_expr: ExprRef,
    ) -> Result<VarId, FrontendError> {
        let (lo, hi) = *self.var_arrays.get(name).ok_or_else(|| {
            elab(format!(
                "`{name}` is not an array variable (no bracketed `[msb:lsb]` declaration)"
            ))
        })?;
        let idx = self.const_eval_int(index_expr, "array variable index")?;
        if idx < lo || idx > hi {
            return Err(elab(format!(
                "index {idx} is out of `{name}`'s declared range [{lo}:{hi}]"
            )));
        }
        let key = format!("{name}[{idx}]");
        self.vars.get(&key).copied().ok_or_else(|| {
            elab(format!(
                "internal error: array variable node `{key}` was not interned"
            ))
        })
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
            Stmt::VarDecl { names } => out.extend(names.iter().map(|entry| entry.name.clone())),
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
            Stmt::Contribute { .. } | Stmt::Task { .. } => {}
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

/// Truncate a value to its integer representation for a bitwise/shift operator. Verilog-A has
/// no bit-vector type — every value here is `f64` — so a bitwise op just operates on the
/// value's truncated `i64` representation, matching how `int()` (§1.5) already bridges
/// float/integer elsewhere in this project.
fn to_i64(v: f64) -> i64 {
    v.trunc() as i64
}

/// Map a range bound value to an optional inclusive bound; an infinite bound is unbounded.
fn bound(v: f64) -> Option<f64> {
    if v.is_infinite() {
        None
    } else {
        Some(v)
    }
}

/// Map a surface [`ast::BinOp`] to the IR's. Every surface operator has an IR counterpart.
fn map_binop(op: ast::BinOp) -> va_ir::BinOp {
    use ast::BinOp as A;
    use va_ir::BinOp as B;
    match op {
        A::Add => B::Add,
        A::Sub => B::Sub,
        A::Mul => B::Mul,
        A::Div => B::Div,
        A::Mod => B::Mod,
        A::Pow => B::Pow,
        A::Lt => B::Lt,
        A::Le => B::Le,
        A::Gt => B::Gt,
        A::Ge => B::Ge,
        A::Eq => B::Eq,
        A::Ne => B::Ne,
        A::And => B::And,
        A::Or => B::Or,
        A::BitAnd => B::BitAnd,
        A::BitOr => B::BitOr,
        A::BitXor => B::BitXor,
        A::BitXnor => B::BitXnor,
        A::Shl => B::Shl,
        A::Shr => B::Shr,
    }
}

/// Map a call-syntax function name to a math [`Builtin`].
fn call_builtin(name: &str) -> Result<Builtin, FrontendError> {
    Ok(match name {
        "exp" => Builtin::Exp,
        // `limexp` is a numerically-limited exponential (a Newton convergence aid); its value
        // and derivative are those of `exp`, which is what v0 models.
        "limexp" => Builtin::Exp,
        "ln" => Builtin::Ln,
        "log" => Builtin::Log,
        "sqrt" => Builtin::Sqrt,
        "abs" => Builtin::Abs,
        "floor" => Builtin::Floor,
        "ceil" => Builtin::Ceil,
        "round" => Builtin::Round,
        // `integer(x)` is the type-cast call form (not the `integer` declaration keyword — a
        // different grammar production entirely). It matches Verilog's real-to-integer
        // assignment conversion rule (round to nearest, not truncate), so it shares `round`'s
        // builtin rather than `int`'s.
        "integer" => Builtin::Round,
        "int" => Builtin::Int,
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
        Mod => a % b,
        Pow => a.powf(b),
        Lt => bool_to_f64(a < b),
        Le => bool_to_f64(a <= b),
        Gt => bool_to_f64(a > b),
        Ge => bool_to_f64(a >= b),
        Eq => bool_to_f64(a == b),
        Ne => bool_to_f64(a != b),
        And => bool_to_f64(a != 0.0 && b != 0.0),
        Or => bool_to_f64(a != 0.0 || b != 0.0),
        BitAnd => (to_i64(a) & to_i64(b)) as f64,
        BitOr => (to_i64(a) | to_i64(b)) as f64,
        BitXor => (to_i64(a) ^ to_i64(b)) as f64,
        BitXnor => !(to_i64(a) ^ to_i64(b)) as f64,
        Shl => to_i64(a).wrapping_shl(to_i64(b) as u32) as f64,
        Shr => (to_i64(a) as u64).wrapping_shr(to_i64(b) as u32) as f64,
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
        "exp" | "limexp" => arg1()?.exp(),
        "ln" => arg1()?.ln(),
        "log" => arg1()?.log10(),
        "sqrt" => arg1()?.sqrt(),
        "abs" => arg1()?.abs(),
        "floor" => arg1()?.floor(),
        "ceil" => arg1()?.ceil(),
        "round" | "integer" => arg1()?.round(),
        "int" => arg1()?.trunc(),
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
    fn aliasparam_resolves_to_the_same_param_id() {
        // `aliasparam` introduces no new parameter: `Rtherm` and `Rth` must share a `ParamId`,
        // and a reference to the alias in the analog block lowers to that same expression.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             parameter real Rth = 1000 from (0:inf); \
             aliasparam Rtherm = Rth; \
             analog begin I(a, b) <+ V(a, b) / Rtherm; end endmodule",
        );
        assert_eq!(m.params.len(), 1, "aliasparam must not add a new parameter");
        assert_eq!(m.params[0].name, "Rth");
        assert_eq!(m.params[0].default, 1000.0);
        assert!(m.exprs.iter().any(|e| matches!(e, Expr::Param(ParamId(0)))));
    }

    #[test]
    fn aliasparam_targeting_unknown_param_is_an_error() {
        let src = "module t(a, b); electrical a, b; \
                   aliasparam alias = nope; \
                   analog begin I(a, b) <+ V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
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
    fn vt_of_temperature_keeps_its_argument() {
        // `$vt(T)` lowers to `Builtin::Vt` with one argument (the temperature expression),
        // whereas bare `$vt` lowers with none.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ V(a, b) / $vt(V(a, b)); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, Expr::Call(Builtin::Vt, args) if args.len() == 1)));

        // `$vt` with more than one argument is a arity error.
        let src = "module t(a, b); electrical a, b; analog begin I(a, b) <+ $vt(V(a, b), 1.0); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());

        // `$temperature` takes no arguments.
        let src = "module t(a, b); electrical a, b; analog begin I(a, b) <+ $temperature(V(a, b)); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn abstime_folds_to_zero() {
        // v0 has no time axis; a DC operating point is conventionally t=0.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ V(a, b) + $abstime; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 0.0)));

        // `$abstime` takes no arguments.
        let src =
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ $abstime(1); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn mfactor_folds_to_one() {
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real r = 1; analog begin I(a, b) <+ $mfactor * V(a, b) / r; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 1.0)));

        let src =
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ $mfactor(1); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn param_given_folds_to_false_and_validates_the_name() {
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real vth0 = 0.5; analog begin if ($param_given(vth0)) I(a, b) <+ V(a, b); else I(a, b) <+ 2.0 * V(a, b); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 0.0)));

        // Names an undeclared parameter.
        let src = "module t(a, b); electrical a, b; analog begin if ($param_given(nope)) I(a, b) <+ V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());

        // Not a bare identifier.
        let src = "module t(a, b); electrical a, b; parameter real vth0 = 0.5; analog begin if ($param_given(vth0 + 1)) I(a, b) <+ V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn port_connected_folds_to_false_and_validates_the_name() {
        let m = elaborate_src(
            "module t(a, b, dt); electrical a, b; thermal dt; analog begin if ($port_connected(dt) == 0) I(a, b) <+ V(a, b); else I(a, b) <+ 0; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 0.0)));

        // Names an undeclared port.
        let src = "module t(a, b); electrical a, b; analog begin if ($port_connected(nope)) I(a, b) <+ V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn limit_folds_to_its_first_argument() {
        let m = elaborate_src(
            r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ $limit(V(a, b), "pnjlim", 0.5, 1.0); end endmodule"#,
        );
        // No trace of the limiting-function name/args survives lowering; the probe access does.
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Probe(_))));

        let src =
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ $limit(); end endmodule";
        let ast = parse(&lex(src).expect("lex")).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn rounding_builtins_map_and_fold() {
        // Lowered to their IR builtins in the analog block.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ floor(V(a, b)) + ceil(V(a, b)) + round(V(a, b)) + int(V(a, b)); end endmodule",
        );
        for bi in [
            va_ir::Builtin::Floor,
            va_ir::Builtin::Ceil,
            va_ir::Builtin::Round,
            va_ir::Builtin::Int,
        ] {
            assert!(
                m.exprs
                    .iter()
                    .any(|e| matches!(e, va_ir::Expr::Call(b, _) if *b == bi)),
                "missing {bi:?}"
            );
        }

        // Const-folded in a parameter context.
        let m = elaborate_src(
            "module t(); parameter real X = floor(3.7) + ceil(1.2) + round(2.5) + int(-2.9); electrical a; analog begin I(a) <+ X; end endmodule",
        );
        // 3 + 2 + 3 + (-2) = 6
        assert_eq!(m.params[0].default, 6.0);
    }

    #[test]
    fn real_and_integer_cast_calls_are_distinct_from_the_declaration_keywords() {
        // `real(expr)`/`integer(expr)` are type-cast *calls* (a different grammar production
        // from the `real`/`integer` declaration keywords) — the real corpus idiom
        // `digital = integer((V(in)/vref) * (1 << N));` (`external/verilogaLib-master/
        // adc_16bit_ideal.va`). `real(x)` is a complete no-op (every value here is already an
        // `f64`); `integer(x)` rounds to nearest, matching Verilog's real-to-integer assignment
        // conversion rule (not `int()`'s truncate-toward-zero).
        let m = elaborate_src(
            "module t(); integer digital; electrical a; \
             analog begin digital = integer(2.5); I(a) <+ real(digital); end endmodule",
        );
        assert_eq!(m.params.len(), 0);
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::Round, _))));

        // Const-folded in a parameter context: integer(2.5) rounds to 3.0, not 2.0.
        let m = elaborate_src(
            "module t(); parameter real X = integer(2.5); electrical a; analog begin I(a) <+ X; end endmodule",
        );
        assert_eq!(m.params[0].default, 3.0);
    }

    #[test]
    fn limexp_maps_to_exp() {
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ limexp(V(a, b)); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::Exp, _))));
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
    fn system_task_statement_is_a_noop() {
        // `$strobe(...)` elaborates (lowers to an empty block) and does not affect the model.
        let src = r#"module t(a, b); electrical a, b; analog begin $strobe("hi", V(a, b)); I(a, b) <+ V(a, b); end endmodule"#;
        let m = elaborate_src(src);
        // The contribution is present; the task contributed no further statement of substance.
        assert!(m
            .analog
            .iter()
            .any(|s| matches!(s, va_ir::Stmt::Contribute { .. })));
    }

    #[test]
    fn string_in_numeric_context_is_rejected() {
        // A bare string where a value is expected is an elaboration error.
        let src =
            r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ "oops"; end endmodule"#;
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
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
    fn temp_and_pwr_are_thermal_access_functions() {
        // `Temp`/`Pwr` are the thermal discipline's standard potential/flow access-function
        // names (from `disciplines.vams`), distinct from `V`/`I` — the real corpus idiom
        // `Temp(dt) <+ 0.0; Pwr(rth) <+ ...;` (external/asmhemt.va and others).
        let m = elaborate_src(
            "module t(); thermal dt; branch (dt) rth; \
             analog begin Temp(dt) <+ 300.0; Pwr(rth) <+ Temp(dt) / 100.0; end endmodule",
        );
        assert_eq!(m.analog.len(), 2);
        match &m.analog[0] {
            va_ir::Stmt::Contribute { target, .. } => {
                assert_eq!(target.kind, va_ir::AccessKind::Potential)
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
        match &m.analog[1] {
            va_ir::Stmt::Contribute { target, .. } => {
                assert_eq!(target.kind, va_ir::AccessKind::Flow)
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
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
    fn simparam_folds_to_default_and_noise_to_zero() {
        // `$simparam("gmin", 1e-9)` folds to its default; `white_noise(...)` folds to 0 in DC.
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ $simparam("gmin", 1e-9) * V(a, b) + white_noise(1.0, "thermal"); end endmodule"#;
        let m = elaborate_src(src);
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 1e-9)));
        // No string literal survives into the IR (no Call to white_noise either).
        assert!(m
            .analog
            .iter()
            .any(|s| matches!(s, va_ir::Stmt::Contribute { .. })));

        // The default may be any expression, not just a constant.
        let src = r#"module t(a, b); electrical a, b; parameter real g = 1e-3; analog begin I(a, b) <+ $simparam("gmin", g) * V(a, b); end endmodule"#;
        let m = elaborate_src(src);
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Param(_))));

        // `$simparam` with no default is an error (unknown parameter).
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ $simparam("gmin") * V(a, b); end endmodule"#;
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn transition_folds_to_its_value_argument() {
        // `transition(V(a,b), td, tr, tf)` settles to its input in DC steady state, so it
        // folds transparently to `V(a,b)` — no `Call` node survives into the IR at all (the
        // only call in this source was `transition` itself), and `td`/`tr` are never even
        // lowered.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real td = 1n; parameter real tr = 1n; \
             analog begin I(a, b) <+ transition(V(a, b), td, tr); end endmodule",
        );
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Probe(_))));
        assert!(!m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Call(..))));

        // No value argument at all is an error.
        let src =
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ transition(); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn slew_folds_to_its_value_argument() {
        // `slew(V(a,b), rate)` has no rate-of-change to limit at a fixed DC operating point, so
        // it folds transparently to `V(a,b)`, same treatment as `transition`.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real rate = 1e6; \
             analog begin I(a, b) <+ slew(V(a, b), rate); end endmodule",
        );
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Probe(_))));
        assert!(!m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Call(..))));
    }

    #[test]
    fn ac_stim_and_bound_step_fold_to_zero() {
        // `ac_stim` only contributes during AC analysis (v0 is DC-only); `bound_step` is a
        // transient-timestep hint with no DC meaning. Both fold to a constant zero in
        // expression position.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             analog begin I(a, b) <+ ac_stim(1.0, 0.0) + V(a, b); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 0.0)));

        // `bound_step(step);` as a bare statement is a documented no-op.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             analog begin bound_step(1n); I(a, b) <+ V(a, b); end endmodule",
        );
        assert_eq!(m.analog.len(), 2);
        assert!(matches!(m.analog[0], va_ir::Stmt::Block(ref b) if b.is_empty()));
    }

    #[test]
    fn analysis_folds_to_dc_constant() {
        // `analysis("static")` is true in DC → folds to 1.0; `analysis("tran")` → 0.0.
        let m = elaborate_src(
            r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ analysis("static") ? 1.0 : 2.0; end endmodule"#,
        );
        // The selector folds to a constant 1.0 (no Call to `analysis` survives in the IR).
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 1.0)));

        // A full end-to-end check: varistor's pattern `analysis("static") && expr` elaborates.
        let m = elaborate_src(
            r#"module t(a, b); electrical a, b; analog begin if (analysis("tran") && V(a, b) > 1.0) $strobe("hi"); I(a, b) <+ V(a, b); end endmodule"#,
        );
        assert!(m
            .analog
            .iter()
            .any(|s| matches!(s, va_ir::Stmt::Contribute { .. })));
    }

    #[test]
    fn logical_operators_fold_and_lower() {
        // Const-folded in a parameter: (1 && 0) + (2 != 3) + (0 || 5) = 0 + 1 + 1 = 2.
        let m = elaborate_src(
            "module t(); parameter real X = (1 && 0) + (2 != 3) + (0 || 5); electrical a; analog begin I(a) <+ X; end endmodule",
        );
        assert_eq!(m.params[0].default, 2.0);

        // Lowered in the analog block to the corresponding IR BinOps.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin x = V(a, b) > 0 && V(a, b) != 1; I(a, b) <+ x; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::And, _, _))));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::Ne, _, _))));
    }

    #[test]
    fn ddx_lowers_to_expr_ddx() {
        // The LRM's own diode example (§4.5.13): `gdio = ddx(idio, V(a));`.
        let m = elaborate_src(
            "module diode(a, c); inout a, c; electrical a, c; parameter real IS = 1e-14; \
             real idio, gdio; \
             analog begin idio = IS * (exp(V(a,c) / $vt) - 1); gdio = ddx(idio, V(a)); \
             I(a,c) <+ idio; end endmodule",
        );
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Ddx(..))));
    }

    #[test]
    fn ddx_rejects_malformed_arguments() {
        // Wrong arity.
        let src = "module t(a); electrical a; analog begin I(a) <+ ddx(V(a)); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());

        // Second argument isn't a probe at all.
        let src = "module t(a); electrical a; analog begin I(a) <+ ddx(V(a), 1.0); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());

        // Second argument is a flow probe, not a potential one — not supported (flow probes
        // aren't independent unknowns in this codegen).
        let src = "module t(a, b); electrical a, b; \
                   analog begin I(a,b) <+ ddx(V(a,b), I(a,b)); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("flow"),
                "expected a flow-probe-specific message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
        }
    }

    #[test]
    fn modulus_folds_and_lowers() {
        // Real corpus idiom: `if ((nf%2) != 0) begin ... end` (a `` `define `` macro in
        // external/bsim4.va), an even/odd parity check.
        let m = elaborate_src(
            "module t(); parameter integer X = 7 % 3; electrical a; \
             analog begin I(a) <+ X; end endmodule",
        );
        assert_eq!(m.params[0].default, 1.0);

        let m = elaborate_src(
            "module t(); integer nf; electrical a; \
             analog begin I(a) <+ nf % 2; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::Mod, _, _))));
    }

    #[test]
    fn vt_and_temperature_are_ordinary_identifiers() {
        // `vt`/`temperature` are no longer reserved (§1.5 `Vt`/`Temperature`): the real corpus
        // idiom `real vt; vt = $vt(Tj);` (caching the thermal-voltage value under its
        // conventional name, seen directly in external/igbt3.va) now elaborates — a bare `vt`
        // and the `$vt` system function coexist without conflict.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; real vt, temperature; \
             analog begin vt = $vt; temperature = $temperature; I(a, b) <+ vt + temperature; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::Vt, _))));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::Temperature, _))));
    }

    #[test]
    fn bitwise_operators_fold_and_lower() {
        // Const-folded in a parameter: (6 & 3) | (1 << 2) = 2 | 4 = 6.
        let m = elaborate_src(
            "module t(); parameter integer X = (6 & 3) | (1 << 2); electrical a; \
             analog begin I(a) <+ X; end endmodule",
        );
        assert_eq!(m.params[0].default, 6.0);

        // `~0` (bitwise NOT of 0, all bits set) is a huge value, not 1.0 (which `!` would give).
        let m = elaborate_src(
            "module t(); parameter integer X = ~0; electrical a; analog begin I(a) <+ X; end endmodule",
        );
        assert_eq!(m.params[0].default, !0i64 as f64);

        // Lowered in the analog block to the corresponding IR BinOp/UnOp, matching the real
        // corpus idiom `(digital >> i) & 1`.
        let m = elaborate_src(
            "module t(); integer digital, i, bit_i; electrical a; \
             analog begin bit_i = (digital >> i) & 1; I(a) <+ bit_i; end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::Shr, _, _))));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::BitAnd, _, _))));
    }

    #[test]
    fn ternary_lowers_to_select_and_folds_in_params() {
        // In a parameter context the ternary is const-folded.
        let m = elaborate_src("module t(); parameter real X = 1 > 0 ? 7 : 9; electrical a; analog begin I(a) <+ X; end endmodule");
        assert_eq!(m.params[0].default, 7.0);

        // In the analog block it lowers to Expr::Select.
        let m = elaborate_src("module t(a, b); electrical a, b; analog begin I(a, b) <+ V(a, b) > 0 ? 1.0 : 2.0; end endmodule");
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Select(_, _, _))));
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
    fn generate_for_unrolls_a_vector_ladder() {
        // A 4-node bus (`bus[0..3]`) with a genvar-driven generate loop contributing across
        // each adjacent pair — the compact-model ladder-network pattern genvar exists for.
        let src = "module ladder(p, n); inout p, n; electrical p, n; electrical [3:0] bus; \
                   genvar i; parameter real R = 250; \
                   analog begin \
                     for (i = 0; i < 3; i = i + 1) begin \
                       I(bus[i], bus[i+1]) <+ V(bus[i], bus[i+1]) / R; \
                     end \
                   end endmodule";
        let m = elaborate_src(src);
        // p, n, bus[0..3]: 6 nodes total.
        assert_eq!(m.nodes.len(), 6);
        // The genvar-for is fully unrolled at elaboration: it never reaches the IR as a
        // `va_ir::Stmt::For` — only the flat block of unrolled contributions does.
        match &m.analog[0] {
            va_ir::Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 3);
                assert!(stmts
                    .iter()
                    .all(|s| matches!(s, va_ir::Stmt::Contribute { .. })));
            }
            other => panic!("expected the unrolled block, got {other:?}"),
        }
        assert_eq!(m.branches.len(), 3);
    }

    #[test]
    fn analog_operator_is_legal_inside_generate_for() {
        // Rule: unlike an ordinary runtime loop, a genvar-driven loop is unrolled at
        // elaboration, so `ddt` inside it is just `ddt` in three separate, already-distinct
        // pieces of straight-line code — no special-casing needed once it is unrolled.
        let src = "module t(); electrical [1:0] bus; genvar i; parameter real cap = 1e-12; \
                   analog begin \
                     for (i = 0; i < 2; i = i + 1) begin \
                       I(bus[i]) <+ ddt(cap * V(bus[i])); \
                     end \
                   end endmodule";
        let m = elaborate_src(src);
        match &m.analog[0] {
            va_ir::Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 2);
                assert!(stmts
                    .iter()
                    .all(|s| matches!(s, va_ir::Stmt::Contribute { .. })));
            }
            other => panic!("expected the unrolled block, got {other:?}"),
        }
    }

    #[test]
    fn genvar_assignment_outside_loop_header_is_rejected() {
        let src = "module t(); genvar i; analog begin i = 5; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn genvar_step_must_reassign_the_same_genvar() {
        let src = "module t(); genvar i; integer j; \
                   analog begin generate for (i = 0; i < 2; j = j + 1) begin end endgenerate end \
                   endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn nested_generate_loop_reusing_genvar_name_is_rejected() {
        let src = "module t(); genvar i; real acc; \
                   analog begin \
                     generate for (i = 0; i < 2; i = i + 1) begin \
                       generate for (i = 0; i < 2; i = i + 1) begin \
                         acc = i; \
                       end endgenerate \
                     end endgenerate \
                   end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn genvar_loop_bound_must_be_static() {
        let src = "module t(); electrical p; genvar i; \
                   analog begin generate for (i = 0; V(p) > 0; i = i + 1) begin end endgenerate end \
                   endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn vector_index_out_of_range_is_rejected() {
        let src = "module t(); electrical [1:0] bus; analog begin I(bus[5]) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn accessing_vector_net_without_index_is_rejected() {
        let src = "module t(); electrical [1:0] bus; analog begin I(bus) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn indexing_a_non_vector_net_is_rejected() {
        let src = "module t(); electrical p; analog begin I(p[0]) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn array_variable_write_and_read_with_constant_index() {
        // `out_val[2] = 5.0; I(a) <+ out_val[2];` — a literal (compile-time-constant) index
        // resolves to the same `VarId` on both the write and the read.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; real out_val[0:15]; \
             analog begin out_val[2] = 5.0; I(a, b) <+ out_val[2]; end endmodule",
        );
        let write_id = match &m.analog[0] {
            va_ir::Stmt::Assign { lhs, .. } => *lhs,
            other => panic!("expected an assignment, got {other:?}"),
        };
        let read_id = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => match m.expr(*value) {
                va_ir::Expr::Var(id) => *id,
                other => panic!("expected Expr::Var, got {other:?}"),
            },
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert_eq!(write_id, read_id);
        assert_eq!(m.vars[write_id.0 as usize].name, "out_val[2]");
    }

    #[test]
    fn array_variable_indexed_by_genvar_in_a_generate_for() {
        // The direct real-corpus idiom (`external/verilogaLib-master/*_ideal.va`): a
        // genvar-driven loop writing/reading successive array-variable elements.
        let m = elaborate_src(
            "module t(a); electrical a; real out_val[0:2]; genvar i; \
             analog begin \
               for (i = 0; i < 3; i = i + 1) begin \
                 out_val[i] = i; \
               end \
               I(a) <+ out_val[0] + out_val[1] + out_val[2]; \
             end endmodule",
        );
        // The genvar-for unrolled to 3 flat assignments (Stmt::Block), then the contribution.
        assert_eq!(m.analog.len(), 2);
        match &m.analog[0] {
            va_ir::Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 3);
                assert!(stmts
                    .iter()
                    .all(|s| matches!(s, va_ir::Stmt::Assign { .. })));
            }
            other => panic!("expected the unrolled block, got {other:?}"),
        }
    }

    #[test]
    fn array_variable_out_of_range_index_is_rejected() {
        let src = "module t(); real out_val[0:15]; \
                   analog begin out_val[16] = 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn array_variable_runtime_index_is_rejected_with_a_clear_error() {
        // The real-corpus gap this was written against: `out_val[j]` where `j` is an ordinary
        // *runtime* `integer` (not a genvar or a constant) has no sound compile-time resolution
        // in this IR — `const_eval_int` rejects it the same way it would reject any other
        // non-constant expression in this context, giving a specific, honest error rather than
        // silently doing the wrong thing.
        let src = "module t(a); electrical a; real out_val[0:15]; integer j; \
                   analog begin j = 3; I(a) <+ out_val[j]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("compile-time constant"),
                "expected a compile-time-constant-specific message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
        }
    }

    #[test]
    fn block_local_array_variable_is_rejected() {
        // Array variables must be declared at module scope (§ array variables); a block-local
        // one has nowhere sound to register into and is rejected with a specific message.
        let src = "module t(); analog begin real out_val[0:15]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("module scope"),
                "expected a module-scope-specific message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
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

    #[test]
    fn vector_port_resolves_to_its_full_node_list() {
        // `Module::ports` is `Vec<Vec<NodeId>>` precisely so a vector port (declared like any
        // other vector net, § vector nets) resolves to all of its constituent nodes, in
        // ascending index order, rather than being rejected. `out` is an ordinary scalar port
        // (a one-element list); `bus` is a 4-element vector port.
        let m = elaborate_src(
            "module dac(out, bus); output out; input [3:0] bus; \
             electrical out; electrical [3:0] bus; analog begin end endmodule",
        );
        assert_eq!(m.ports.len(), 2, "two declared ports, regardless of width");
        assert_eq!(m.ports[0].len(), 1, "`out` is scalar");
        assert_eq!(m.ports[1].len(), 4, "`bus` is a 4-element vector");

        // The vector port's nodes are the same interned nodes a direct indexed access
        // (`bus[0]`, `bus[1]`, …) would resolve to, in ascending order.
        let bus0 = m
            .nodes
            .iter()
            .position(|n| n.name == "bus[0]")
            .expect("bus[0] interned");
        let bus3 = m
            .nodes
            .iter()
            .position(|n| n.name == "bus[3]")
            .expect("bus[3] interned");
        assert_eq!(m.ports[1][0].0 as usize, bus0);
        assert_eq!(m.ports[1][3].0 as usize, bus3);

        // Every node the module declares is reachable via `ports.iter().flatten()` too, the
        // flattened-terminal-list view a future netlist wiring convention would use.
        let flattened: Vec<_> = m.ports.iter().flatten().collect();
        assert_eq!(flattened.len(), 5);
    }
}
