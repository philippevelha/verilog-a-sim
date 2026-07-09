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
//!   declared `[msb:lsb]` range. A vector-typed port resolves to the full node list
//!   (`Module::ports: Vec<Vec<NodeId>>`).
//! - **2-D vector nets / 2-D array variables**: both vector nets and array variables (§ array
//!   variables) may carry a second declared dimension, interning one node/`VarId` per index
//!   *tuple* as `base[i][j]`. For array variables (`real tile[0:R][0:C];`) this is standard LRM
//!   grammar; for vector nets (`electrical [0:R][0:C] grid;`) it is a deliberate, documented
//!   **non-standard extension** — the LRM's `net_declaration` grammar never carries more than
//!   one range. Both are capped at exactly 2 dimensions (not general N-D). A 2-D vector net may
//!   never be used as a port, sliced, or connected/accessed bare or partially indexed (only a
//!   fully 2-indexed element resolves); these restrictions are enforced in `resolve_ports`,
//!   `resolve_net_arg`, and `resolve_conn_nodes`.
//! - Vector nets and array variables (§ array variables) both support a genuinely runtime index
//!   in **at most one** of their (up to 2) dimensions (an ordinary loop variable, not just a
//!   genvar/constant) via elaboration-time unrolling into a `Select`/`If` chain over every
//!   declared value of that one dimension, guarded by an equality check, with the other
//!   dimension (if any) resolved once up front — see
//!   `dynamic_terminal_range`/`lower_probe_expr`/`unroll_indexed_contribute` and
//!   `dynamic_var_index`/`lower_indexed_var_read`/`lower_indexed_var_write`. Both index
//!   positions of the same 2-D access being simultaneously dynamic is rejected (`O(range²)`
//!   chains are deliberately not built — see `dynamic_index_pos`'s doc comment). There is still
//!   no runtime-indexable-*storage* concept in the IR; a runtime index outside the declared
//!   range at simulation time silently resolves to the chain's last arm rather than erroring.
//! - **Module instantiation** (§ module instantiation, LRM Annex C.8): [`Item::Instance`] is
//!   resolved entirely here, not in the IR — [`elaborate_with_library`] recursively elaborates
//!   the referenced submodule (as if it were standalone, with any `#(...)` overrides baked into
//!   its parameter defaults) and `Elaborator::merge_submodule` inlines the result's arenas
//!   into the instantiating module's own, aliasing the submodule's port nodes to whatever node
//!   the parent wired them to. `va_ir::Module` therefore never represents hierarchy — one flat
//!   module is still the only IR shape, matching its own doc comment. Scalar port connections
//!   only (no vector-port fan-out); no module-item-level `generate` around instances. **A
//!   submodule's own implicit ground** (interned by `Elaborator::reference_node` for
//!   single-terminal `V(p)` shorthand) is *not* unified with the parent's or a sibling
//!   instance's ground, since each submodule elaborates in its own arena — a model that needs
//!   the true circuit reference node from inside a submodule must declare an explicit port for
//!   it and have the instantiating parent wire that port to real ground, like any other port.

use std::collections::HashMap;

use crate::ast::{self, ExprAst, ExprRef, Item, ModuleAst, Stmt};
use crate::disciplines::{self, DisciplineDecl, NatureDecl};
use crate::FrontendError;
use va_ir::{
    Access, AccessKind, ArgDir, Branch, BranchId, Builtin, CaseArm, Discipline, Expr, ExprId,
    FuncId, Function, Module, NodeDecl, NodeId, Param, ParamId, VarDecl, VarId,
};

/// Elaborate a parsed module into the IR, with no submodule library — any [`Item::Instance`]
/// it contains fails to resolve. Equivalent to [`elaborate_with_library`] with `ast` as its own
/// sole library entry.
///
/// # Errors
///
/// Returns [`FrontendError::Elaborate`] on unresolved names, non-constant parameter
/// expressions, or constructs outside the v0 subset.
pub fn elaborate(ast: &ModuleAst) -> Result<Module, FrontendError> {
    elaborate_with_library(ast, std::slice::from_ref(ast))
}

/// Elaborate `ast` with `library` (every module parsed from the same compilation unit,
/// including `ast` itself) available to resolve its [`Item::Instance`]s against (§ module
/// instantiation). This is the entry point multi-module callers
/// ([`crate::compile_with_includes`]) use, once per module in a file.
///
/// Equivalent to [`elaborate_with_library_and_disciplines`] with no parsed discipline/nature
/// metadata — every node's [`va_ir::NodeDecl::abstol`] comes out `None`, exactly this
/// function's behavior before § nature-metadata wiring existed.
///
/// # Errors
///
/// As [`elaborate`], plus an unknown instantiated module name or an instantiation cycle.
pub fn elaborate_with_library(
    ast: &ModuleAst,
    library: &[ModuleAst],
) -> Result<Module, FrontendError> {
    elaborate_with_library_and_disciplines(ast, library, &HashMap::new(), &HashMap::new())
}

/// Like [`elaborate_with_library`], but also resolving each net's [`va_ir::NodeDecl::abstol`]
/// from a parsed `discipline...enddiscipline`/`nature...endnature` preamble (§ nature-metadata
/// wiring) — `disciplines`/`natures` are the tables `crate::parser::parse_with_disciplines`
/// returns alongside the parsed modules, file-scoped (§ module preamble discipline/nature
/// parsing), so the same tables apply to every module `library` holds, including any submodule
/// this call recursively elaborates (§ module instantiation).
///
/// # Errors
///
/// As [`elaborate_with_library`].
pub fn elaborate_with_library_and_disciplines(
    ast: &ModuleAst,
    library: &[ModuleAst],
    disciplines: &HashMap<String, DisciplineDecl>,
    natures: &HashMap<String, NatureDecl>,
) -> Result<Module, FrontendError> {
    elaborate_inner(ast, library, &[], &HashMap::new(), disciplines, natures)
}

fn elaborate_inner(
    ast: &ModuleAst,
    library: &[ModuleAst],
    stack: &[String],
    param_overrides: &HashMap<String, f64>,
    disciplines: &HashMap<String, DisciplineDecl>,
    natures: &HashMap<String, NatureDecl>,
) -> Result<Module, FrontendError> {
    let mut e = Elaborator {
        ast,
        library,
        stack,
        param_overrides,
        disciplines,
        natures,
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

/// A single vector-net terminal (of a `V(...)`/`I(...)` access) whose index is present but not
/// compile-time-constant — the input to the § dynamic vector-net/array-variable indexing
/// expansion (`Elaborator::lower_probe_expr`/`unroll_indexed_contribute`). `lo`/`hi` is the
/// dynamic dimension's own declared range, looked up once by
/// [`Elaborator::dynamic_terminal_range`] so callers don't have to re-look it up.
struct DynamicTerminal {
    /// 0 for the first (`p`) terminal, 1 for the second (`n`).
    pos: usize,
    /// Which of the terminal's (1 or 2, § 2-D vector net) index positions is the dynamic one.
    dyn_dim: usize,
    /// The terminal's other, already-constant-resolved dimension, for a § 2-D vector net —
    /// `None` for a 1-D vector net, which has no other dimension.
    other_idx: Option<i64>,
    name: String,
    idx_expr: ExprRef,
    lo: i64,
    hi: i64,
}

/// The `VarId` counterpart of [`DynamicTerminal`] — an array-variable index that is present but
/// not compile-time-constant, one of the (up to 2) declared dimensions.
struct DynamicVarIndex {
    /// Which index position is the dynamic one.
    dyn_dim: usize,
    /// The other, already-constant-resolved dimension, for a § 2-D array variable.
    other_idx: Option<i64>,
    lo: i64,
    hi: i64,
}

/// The flattened storage key for a 1- or 2-D indexed name: `"name[i]"` or `"name[i][j]"`,
/// exactly mirroring how a scalar-indexed 1-D vector/array has always been interned.
fn indexed_key(name: &str, idxs: &[i64]) -> String {
    let mut s = name.to_string();
    for k in idxs {
        s.push_str(&format!("[{k}]"));
    }
    s
}

/// Every declared index tuple, row-major, for 1 or 2 inclusive dimension ranges — the
/// declaration-time expansion `collect_nodes`/`declare_var_entry` both need.
///
/// # Panics
///
/// Panics if `dims` has more than 2 entries — unreachable in practice, since the parser caps
/// declared dimensions at 2 (`Parser::parse_dim_list`).
fn dim_indices(dims: &[(i64, i64)]) -> Vec<Vec<i64>> {
    match dims {
        [] => vec![vec![]],
        [(lo, hi)] => (*lo..=*hi).map(|k| vec![k]).collect(),
        [(lo0, hi0), (lo1, hi1)] => {
            let mut out = Vec::new();
            for i in *lo0..=*hi0 {
                for j in *lo1..=*hi1 {
                    out.push(vec![i, j]);
                }
            }
            out
        }
        _ => unreachable!("declared dimensions are capped at 2 by the parser"),
    }
}

/// Combine a dynamic dimension's concrete candidate value `k` with the other (already-constant)
/// dimension, if any, into a full index tuple — shared by the vector-net probe/contribute
/// expansion and the array-variable read/write expansion.
fn combine_idx(dyn_dim: usize, k: i64, other_idx: Option<i64>) -> Vec<i64> {
    match other_idx {
        None => vec![k],
        Some(o) if dyn_dim == 0 => vec![k, o],
        Some(o) => vec![o, k],
    }
}

struct Elaborator<'a> {
    ast: &'a ModuleAst,
    /// Every module parsed from the same compilation unit (including `ast` itself), used to
    /// resolve [`Item::Instance`] references (§ module instantiation).
    library: &'a [ModuleAst],
    /// Names of modules currently being elaborated further up the instantiation chain (parent,
    /// grandparent, …), for cycle detection — does not include `ast.name` itself.
    stack: &'a [String],
    /// Parameter-name → overridden value, supplied by the instantiating parent's `#(...)` list
    /// (empty when elaborating a top-level module). Consulted by [`Self::collect_params`] in
    /// place of the AST default when present.
    param_overrides: &'a HashMap<String, f64>,
    /// Parsed `discipline...enddiscipline` blocks, keyed by name (§ nature-metadata wiring),
    /// empty when elaborated via [`elaborate`]/[`elaborate_with_library`] (no preamble
    /// available). File-scoped, shared unchanged across every submodule this elaboration
    /// recursively inlines (§ module instantiation).
    disciplines: &'a HashMap<String, DisciplineDecl>,
    /// Parsed `nature...endnature` blocks, keyed by name — the `disciplines`'s bound
    /// `potential`/`flow` names are looked up here to resolve a net's `abstol`.
    natures: &'a HashMap<String, NatureDecl>,
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
    /// Declared vector nets' inclusive `(lo, hi)` index range per dimension, keyed by base name
    /// (§ vector nets). Length 1 is a standard 1-D vector; length 2 is a § 2-D vector net (a
    /// deliberate, documented non-standard extension). A vector net `bus` interns one
    /// [`NodeId`] per index tuple as `bus[k]`/`bus[i][j]`.
    vectors: HashMap<String, Vec<(i64, i64)>>,
    /// Declared array variables' inclusive `(lo, hi)` index range per dimension, keyed by base
    /// name (§ array variables) — the `VarId` counterpart of `vectors` above. Length 1 is
    /// standard 1-D; length 2 is a § 2-D array variable (standard LRM grammar). An array
    /// `out_val` interns one [`VarId`] per index tuple as `out_val[k]`/`out_val[i][j]`. A
    /// compile-time-constant or genvar index resolves directly; a genuinely runtime index in at
    /// most one dimension (§ dynamic vector-net/array-variable indexing) is unrolled into a
    /// `Select`/`If` chain over every declared value of that one dimension, since there is still
    /// no runtime-indexable-storage concept in the IR itself.
    var_arrays: HashMap<String, Vec<(i64, i64)>>,
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
        // Every parent-scope naming environment (nodes, params, genvars, vars, branches,
        // functions) is fully populated by this point, regardless of source order — so
        // instances may freely appear before or after the parent constructs they connect to.
        self.collect_instances()?;
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

    /// Register one variable-declaration entry: a plain scalar, or (if it carries dimension
    /// range(s)) a 1-D or § 2-D array — interning one [`VarId`] per index tuple, named
    /// `"name[k]"`/`"name[i][j]"`, exactly mirroring how [`Self::collect_nodes`] expands a
    /// vector net.
    fn declare_var_entry(&mut self, entry: &ast::VarEntry) -> Result<(), FrontendError> {
        if entry.ranges.is_empty() {
            if let std::collections::hash_map::Entry::Vacant(slot) =
                self.vars.entry(entry.name.clone())
            {
                let id = VarId(self.out.vars.len() as u32);
                self.out.vars.push(VarDecl {
                    name: entry.name.clone(),
                });
                slot.insert(id);
            }
            return Ok(());
        }
        let mut dims = Vec::with_capacity(entry.ranges.len());
        for &(msb, lsb) in &entry.ranges {
            let msb = self.const_eval_int(msb, "array variable range bound")?;
            let lsb = self.const_eval_int(lsb, "array variable range bound")?;
            dims.push(if msb <= lsb { (msb, lsb) } else { (lsb, msb) });
        }
        for idxs in dim_indices(&dims) {
            let key = indexed_key(&entry.name, &idxs);
            if let std::collections::hash_map::Entry::Vacant(slot) = self.vars.entry(key.clone()) {
                let id = VarId(self.out.vars.len() as u32);
                self.out.vars.push(VarDecl { name: key });
                slot.insert(id);
            }
        }
        self.var_arrays.insert(entry.name.clone(), dims);
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
            let mut arg_dirs = Vec::with_capacity(f.args.len());
            for a in &f.args {
                let id = self.new_var(&a.name);
                local.insert(a.name.clone(), id);
                args.push(id);
                arg_dirs.push(match a.dir {
                    ast::Direction::Input => ArgDir::Input,
                    ast::Direction::Output => ArgDir::Output,
                    ast::Direction::Inout => ArgDir::Inout,
                });
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
                arg_dirs,
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
                    // Multi-physics beyond electrical/thermal isn't modeled by `va-core` yet
                    // (§1 roadmap) — the node still exists and can be probed/contributed to,
                    // it's just not checked for domain-specific conservation.
                    ast::Discipline::Custom(_) => Discipline::Other,
                };
                // The discipline's own *name* (as opposed to `disc`, the collapsed `va_ir`
                // enum) is what a parsed `discipline...enddiscipline` block is keyed by — even
                // the dedicated `electrical`/`thermal` keywords have one, since a real
                // `disciplines.vams` declares `discipline electrical; ... enddiscipline` by
                // that same name (§ nature-metadata wiring).
                let disc_name = match discipline {
                    ast::Discipline::Electrical => "electrical",
                    ast::Discipline::Thermal => "thermal",
                    ast::Discipline::Custom(name) => name.as_str(),
                };
                let abstol = disciplines::resolve_abstol(disc_name, self.disciplines, self.natures);
                // Each name carries its own optional dimension range(s) — `electrical [0:w-1]
                // in;` and `electrical in[`W-1:0], out;` both reach here as one `NetDecl` per
                // name, the prefix-vs-suffix distinction already resolved by the parser (§2.2).
                // A second dimension (§ 2-D vector net) is a non-standard extension.
                for net in nets {
                    if net.ranges.is_empty() {
                        self.intern_node(&net.name, disc, abstol);
                        continue;
                    }
                    // A vector net interns one node per index tuple (§ vector nets); a branch
                    // access later selects one by a genvar expression.
                    let mut dims = Vec::with_capacity(net.ranges.len());
                    for &(msb, lsb) in &net.ranges {
                        let msb = self.const_eval_int(msb, "vector net range bound")?;
                        let lsb = self.const_eval_int(lsb, "vector net range bound")?;
                        dims.push(if msb <= lsb { (msb, lsb) } else { (lsb, msb) });
                    }
                    for idxs in dim_indices(&dims) {
                        self.intern_node(&indexed_key(&net.name, &idxs), disc, abstol);
                    }
                    self.vectors.insert(net.name.clone(), dims);
                }
            }
        }
        Ok(())
    }

    fn intern_node(&mut self, name: &str, discipline: Discipline, abstol: Option<f64>) -> NodeId {
        if let Some(id) = self.nodes.get(name) {
            return *id;
        }
        let id = NodeId(self.out.nodes.len() as u32);
        self.out.nodes.push(NodeDecl {
            name: name.to_string(),
            discipline,
            abstol,
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
            if let Some(dims) = self.vectors.get(port) {
                if dims.len() != 1 {
                    return Err(elab(format!(
                        "port `{port}` is a {}-D vector net (§ 2-D vector net extension); only \
                         a 1-D vector net may be used as a port",
                        dims.len()
                    )));
                }
                let (lo, hi) = dims[0];
                let mut ids = Vec::with_capacity((hi - lo + 1) as usize);
                for k in lo..=hi {
                    let key = indexed_key(port, &[k]);
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

    /// Resolve every declared parameter's value: the instantiating parent's `#(...)` override
    /// (via [`Self::param_overrides`]) when present, else the AST default (§ module
    /// instantiation) — either way validated against the declared `from` range, so an override
    /// is held to the same bound as an ordinary default.
    fn collect_params(&mut self) -> Result<(), FrontendError> {
        for item in &self.ast.items {
            match item {
                Item::Param {
                    name,
                    default,
                    range,
                    ..
                } => {
                    let (min, max) = match range {
                        Some(r) => (bound(self.const_eval(r.lo)?), bound(self.const_eval(r.hi)?)),
                        None => (None, None),
                    };
                    let default_val = match self.param_overrides.get(name) {
                        Some(&v) => v,
                        None => self.const_eval(*default)?,
                    };
                    if let Some(min) = min {
                        if default_val < min {
                            return Err(elab(format!(
                                "parameter `{name}` value {default_val} is below its declared minimum {min}"
                            )));
                        }
                    }
                    if let Some(max) = max {
                        if default_val > max {
                            return Err(elab(format!(
                                "parameter `{name}` value {default_val} is above its declared maximum {max}"
                            )));
                        }
                    }
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
            // `$simparam("name", default)` folds to `default` here exactly as `lower_expr`
            // folds it in the analog block (v0 has no simulator-parameter store, so the queried
            // name is never actually looked up) — a parameter default is just as legitimate a
            // place for this idiom as an ordinary expression (`external/bsim6.0.va`:
            // `parameter real GMIN = $simparam("gmin", 1.0e-15);`). Without a default it's still
            // an error, matching the LRM's behavior for an unknown simulator parameter.
            ExprAst::SysFunc { name, args } if name == "simparam" => match args.get(1) {
                Some(&default) => self.const_eval(default),
                None => Err(elab(
                    "$simparam without a default: the parameter is unknown in v0 (no simulator \
                     parameters)"
                        .to_string(),
                )),
            },
            ExprAst::SysFunc { name, .. } => Err(elab(format!(
                "system function `${name}` is not constant in a parameter context"
            ))),
            ExprAst::Str(_) => Err(elab(
                "a string literal is not valid in a parameter context".to_string(),
            )),
            ExprAst::Probe(_) => Err(elab(
                "a branch probe is not constant in a parameter context".to_string(),
            )),
            ExprAst::PortProbe { .. } => Err(elab(
                "a port-current probe is not constant in a parameter context".to_string(),
            )),
            ExprAst::IndexedIdent(name, _) => Err(elab(format!(
                "array variable `{name}` is not constant in a parameter context"
            ))),
            ExprAst::ArrayLit(_) => Err(elab(
                "an array-literal expression is not constant in a parameter context (only \
                 valid as a Laplace/Z-domain filter argument)"
                    .to_string(),
            )),
        }
    }

    /// Extract and const-evaluate every element of a `{...}` array-literal argument — the
    /// Laplace/Z-domain filter builtins' (§4.5.11/§4.5.12) shared entry point for reading a
    /// `num`/`den` coefficient list or a `zero`/`pole` root list. `what` names the argument in
    /// the error message.
    fn array_lit_values(&self, r: ExprRef, what: &str) -> Result<Vec<f64>, FrontendError> {
        match self.ast.expr(r) {
            ExprAst::ArrayLit(elems) => elems.iter().map(|&e| self.const_eval(e)).collect(),
            _ => Err(elab(format!(
                "{what} must be a `{{...}}` array-literal coefficient/root list"
            ))),
        }
    }

    /// [`Self::array_lit_values`], then require at least one coefficient and return only the
    /// `s^0`/`z^0` (constant) one — a `laplace_nd`/`laplace_np`-style numerator or
    /// `laplace_nd`/`laplace_zd`-style denominator only needs this one coefficient for its DC
    /// (s=0) gain fold, since every higher-degree term vanishes at s=0.
    fn array_lit_first(&self, r: ExprRef, what: &str) -> Result<f64, FrontendError> {
        let values = self.array_lit_values(r, what)?;
        values.first().copied().ok_or_else(|| {
            elab(format!(
                "{what} array literal must have at least one coefficient"
            ))
        })
    }

    /// A `zero`/`pole` array literal's product term at Laplace s=0 (LRM §4.5.11.1-3):
    /// `zeta`/`rho` is a flattened list of `(re, im)` root pairs, one pair per zero/pole. Every
    /// root contributes a factor of `(1 - s/root)`, which is exactly `1` at s=0 for *any*
    /// nonzero root (real or complex — no complex arithmetic is ever needed for this fold), or
    /// `0` for a root exactly at the origin `(0, 0)`: the LRM specifies that root's factor is
    /// implemented as `s` instead of `(1 - s/root)` (avoiding the `s/0` singularity), which is
    /// `0` at s=0. So the whole product is `0.0` if any root is exactly the origin, `1.0`
    /// otherwise. Errors if the array's length is odd (roots must come in `(re, im)` pairs).
    fn laplace_root_product_at_origin(&self, r: ExprRef, what: &str) -> Result<f64, FrontendError> {
        let values = self.array_lit_values(r, what)?;
        if values.len() % 2 != 0 {
            return Err(elab(format!(
                "{what} array literal must hold `(re, im)` pairs, one per root — got {} \
                 elements (odd)",
                values.len()
            )));
        }
        let at_origin = values
            .chunks(2)
            .any(|pair| pair[0] == 0.0 && pair[1] == 0.0);
        Ok(if at_origin { 0.0 } else { 1.0 })
    }

    /// A `zero`/`pole` array literal's product term at Z-domain z=1 (LRM §4.5.12.1-3) — the
    /// steady-state point for a discrete-time filter, the same role s=0 plays for a
    /// continuous-time one. Every root contributes a factor of `(1 - z^-1 * root)`, which at
    /// z=1 is `1 - root` — genuinely complex-valued for a root with a nonzero imaginary part,
    /// unlike the Laplace-domain product above (whose `s/root` form instead makes every
    /// non-origin factor the real constant `1` at s=0). A root exactly at the origin `(0, 0)`
    /// contributes a factor of `z` instead (again avoiding a `root/0`-shaped singularity in the
    /// general form), which is `1` at z=1 — not `0`: z=1 is a different point of the z-plane
    /// than the origin z=0, unlike the Laplace case where s=0 *is* the origin a root-at-origin
    /// sits on. The LRM requires a complex root's conjugate to also be present, so the running
    /// product's imaginary part cancels to (near) zero by construction for any well-formed
    /// filter; only the real part is returned. Errors if the array's length is odd.
    fn z_root_product_at_one(&self, r: ExprRef, what: &str) -> Result<f64, FrontendError> {
        let values = self.array_lit_values(r, what)?;
        if values.len() % 2 != 0 {
            return Err(elab(format!(
                "{what} array literal must hold `(re, im)` pairs, one per root — got {} \
                 elements (odd)",
                values.len()
            )));
        }
        let (mut re_acc, mut im_acc) = (1.0f64, 0.0f64);
        for pair in values.chunks(2) {
            let (root_re, root_im) = (pair[0], pair[1]);
            let (factor_re, factor_im) = if root_re == 0.0 && root_im == 0.0 {
                (1.0, 0.0)
            } else {
                (1.0 - root_re, -root_im)
            };
            let new_re = re_acc * factor_re - im_acc * factor_im;
            let new_im = re_acc * factor_im + im_acc * factor_re;
            (re_acc, im_acc) = (new_re, new_im);
        }
        Ok(re_acc)
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

    /// Register an explicit `real`/`integer` local declaration (`Stmt::VarDecl`) as a fresh
    /// variable — unlike [`Self::register_var`]'s auto-registration for a bare
    /// (declaration-less) assignment target, an explicit declaration always introduces a *new*
    /// identifier in this block's scope, per ordinary nested-scope shadowing, even when a
    /// module-level parameter of the same name already exists. A real corpus pattern:
    /// `external/bsimsoi.va`'s `begin : load ... real ... MJSWG; ... end` declares a local
    /// `MJSWG` that shadows a same-named `` `MPRoo``-macro-declared module parameter for the
    /// rest of that block — `register_var`'s "a same-named parameter means no new var needed"
    /// heuristic is correct for its own weaker, no-declaration-required convenience (assigning
    /// to an actual parameter is invalid Verilog-A, so that case is never a real shadow), but
    /// wrong here, where the declaration is explicit and shadowing is exactly the point.
    fn declare_local_var(&mut self, name: &str) {
        if self.genvars.contains(name) {
            return;
        }
        if !self.vars.contains_key(name) {
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
                if index.is_empty() {
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
                    if !entry.ranges.is_empty() {
                        return Err(elab(format!(
                            "array variable `{}` must be declared at module scope, not inside \
                             the analog block (block-local array variables are not yet \
                             supported)",
                            entry.name
                        )));
                    }
                    self.declare_local_var(&entry.name);
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
        self.lower_module_var_inits()?;
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

    /// Lower every module-level `real x = expr;` inline initializer (§ variable declarations)
    /// into a `Stmt::Assign`, prepended to `self.out.analog` in declaration order — the LRM
    /// requires these to run before the first analog block executes, and this project has no
    /// simulation-phase distinction yet (the same approximation `@(initial_step)` already uses,
    /// § event control), so "prepended, in source order" is the closest sound DC reading.
    fn lower_module_var_inits(&mut self) -> Result<(), FrontendError> {
        let ast = self.ast;
        for item in &ast.items {
            if let Item::Var { names, .. } = item {
                for entry in names {
                    if let Some(init) = entry.init {
                        let id = self.vars[&entry.name];
                        let rhs = self.lower_expr(init)?;
                        self.out.analog.push(va_ir::Stmt::Assign { lhs: id, rhs });
                    }
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
            // A declaration introduces a variable (registered in pass 3); an entry with an
            // inline `= expr` initializer additionally emits an assignment in its place, the
            // same DC-only "runs where it's written" approximation `@(initial_step)` already
            // uses (§ event control) — entries without one still emit nothing.
            Stmt::VarDecl { names } => {
                let mut inits = Vec::new();
                for entry in names {
                    if let Some(init) = entry.init {
                        let id = self.vars[&entry.name];
                        let rhs = self.lower_expr(init)?;
                        inits.push(va_ir::Stmt::Assign { lhs: id, rhs });
                    }
                }
                Ok(va_ir::Stmt::Block(inits))
            }
            // System tasks (`$strobe`, `$finish`, …) have no effect on a DC solve.
            Stmt::Task { .. } => Ok(va_ir::Stmt::Block(Vec::new())),
            Stmt::Contribute { target, value } => {
                // Constant/genvar-indexed terminals (or none at all) resolve straight to a
                // single fixed branch, as always. A runtime-indexed vector-net terminal
                // (§ dynamic vector-net/array-variable indexing) has no single `BranchId` to
                // contribute to, so it's unrolled into an if/else-if chain instead — one
                // `Stmt::Contribute` per declared index, guarded by `index == k`.
                match self.dynamic_terminal_range(&target.args)? {
                    None => {
                        let t = self.lower_access(target)?;
                        let v = self.lower_expr(*value)?;
                        Ok(va_ir::Stmt::Contribute {
                            target: t,
                            value: v,
                        })
                    }
                    Some(dyn_term) => {
                        let kind = match target.kind {
                            ast::AccessKind::Potential => AccessKind::Potential,
                            ast::AccessKind::Flow => AccessKind::Flow,
                        };
                        self.unroll_indexed_contribute(kind, dyn_term, &target.args, *value)
                    }
                }
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
                if index.is_empty() {
                    let id = *self
                        .vars
                        .get(lhs)
                        .ok_or_else(|| elab(format!("assignment to unknown variable `{lhs}`")))?;
                    let rhs = self.lower_expr(*rhs)?;
                    Ok(va_ir::Stmt::Assign { lhs: id, rhs })
                } else {
                    // `lhs[index] = rhs;` / `lhs[i][j] = rhs;`: one element of a 1-D or § 2-D
                    // array variable (§ array variables). A runtime index in at most one
                    // dimension (§ dynamic vector-net/array-variable indexing) can't resolve to
                    // a single `VarId`, so it unrolls into an if/else-if chain instead — see
                    // `lower_indexed_var_write`.
                    self.lower_indexed_var_write(lhs, index, *rhs)
                }
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
                // constant it is currently unrolled to — it never becomes a `Var`/`Param`. `vars`
                // is checked *before* `params`: an explicit local declaration (`declare_local_var`)
                // always shadows a same-named module parameter for the rest of its block (ordinary
                // nested-scope shadowing — see `declare_local_var`'s doc comment), so once a local
                // `MJSWG` exists, a read of `MJSWG` must resolve to it, not the outer parameter.
                if let Some(v) = self.genvar_env.get(name) {
                    Expr::Const(*v as f64)
                } else if let Some(v) = self.vars.get(name) {
                    Expr::Var(*v)
                } else if let Some(p) = self.params.get(name) {
                    Expr::Param(*p)
                } else {
                    return Err(elab(format!("unknown identifier `{name}`")));
                }
            }
            // `name[index]`: one element of an array variable (§ array variables). Constant/
            // genvar indices resolve directly; a runtime index (§ dynamic vector-net/array-
            // variable indexing) expands into a `Select` chain — see `lower_indexed_var_read`.
            ExprAst::IndexedIdent(name, index) => return self.lower_indexed_var_read(name, index),
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
            ExprAst::Probe(access) => return self.lower_probe_expr(access),
            ExprAst::PortProbe { kind, port } => return self.lower_port_probe(*kind, port),
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
            // `absdelay(value, delay[, max_delay])` (LRM §4.5.9) delays `value` by a fixed
            // time — again genuinely time-domain, and again settles to its undelayed input in
            // DC steady state (no delay history exists at a fixed operating point), so it folds
            // like `transition`/`slew` above: `delay`/`max_delay` are parsed but never evaluated.
            ExprAst::Call { name, args } if name == "absdelay" => {
                let value = *args.first().ok_or_else(|| {
                    elab("`absdelay` requires at least a value and a delay argument".to_string())
                })?;
                return self.lower_expr(value);
            }
            // `laplace_nd(value, num, den[, tol])` / `laplace_np(value, num, pole[, tol])` /
            // `laplace_zd(value, zero, den[, tol])` / `laplace_zp(value, zero, pole[, tol])`
            // (LRM §4.5.11) — the four forms of a Laplace-domain filter, differing only in
            // whether the numerator/denominator is a `num`/`den` polynomial-in-`s`
            // coefficient list (lowest degree first) or a `zero`/`pole` array (flattened
            // `(re, im)` root pairs). An optional trailing tolerance argument is parsed but
            // never evaluated (same treatment as `absdelay`'s `max_delay` above). Genuinely
            // time-domain (the whole point is the rational transfer function), but each
            // settles to its DC (s=0) steady-state gain at a fixed operating point the same way
            // `transition`/`absdelay` do — a coefficient list contributes its own `s^0`
            // coefficient (`array_lit_first`); a zero/pole array contributes its root-product
            // fold (`laplace_root_product_at_origin`, `0.0` only if a root sits exactly at the
            // origin). A zero-valued denominator (either form) makes the DC gain undefined — an
            // elaboration error, not a silent `inf`/`NaN`. The LRM's null-argument form (`,,`,
            // omitting the zero/numerator entirely) isn't supported — no corpus need found, and
            // it needs a broader "optional call argument" grammar change nothing else uses yet.
            ExprAst::Call { name, args }
                if matches!(
                    name.as_str(),
                    "laplace_nd" | "laplace_np" | "laplace_zd" | "laplace_zp"
                ) =>
            {
                let (value, num, den) = match (args.first(), args.get(1), args.get(2), args.len()) {
                    (Some(&v), Some(&n), Some(&d), 3..=4) => (v, n, d),
                    _ => {
                        return Err(elab(format!(
                            "`{name}` takes three or four arguments: value, then a \
                             numerator/zero argument, then a denominator/pole argument, and an \
                             optional tolerance, e.g. `{name}(sig, {{1}}, {{1, tau}})`"
                        )))
                    }
                };
                let numer_is_zeros = matches!(name.as_str(), "laplace_zd" | "laplace_zp");
                let denom_is_poles = matches!(name.as_str(), "laplace_np" | "laplace_zp");
                let numer0 = if numer_is_zeros {
                    self.laplace_root_product_at_origin(num, "zero")?
                } else {
                    self.array_lit_first(num, "numerator")?
                };
                let denom0 = if denom_is_poles {
                    self.laplace_root_product_at_origin(den, "pole")?
                } else {
                    self.array_lit_first(den, "denominator")?
                };
                if denom0 == 0.0 {
                    return Err(elab(format!(
                        "`{name}`'s denominator is zero at s=0 (a pole at the origin, or an \
                         explicit zero `s^0` coefficient): the DC gain is undefined"
                    )));
                }
                let value_id = self.lower_expr(value)?;
                let gain_id = self.out.push_expr(Expr::Const(numer0 / denom0));
                Expr::Binary(va_ir::BinOp::Mul, value_id, gain_id)
            }
            // `zi_nd(value, num, den, T[, tol[, t0]])` / `zi_np(value, num, pole, T[, ...])` /
            // `zi_zd(value, zero, den, T[, ...])` / `zi_zp(value, zero, pole, T[, ...])` (LRM
            // §4.5.12) — the Z-domain (discrete-time) counterparts of the four Laplace forms
            // above, same num/den-vs-zero/pole split, plus a mandatory sample period `T` (and
            // optional tolerance/start-time) that — like every other time-domain argument this
            // project folds away — is parsed but never evaluated. Settles to its steady-state
            // (z=1) gain: a coefficient list sums *all* its terms at z=1 (`z^-k` is 1 for every
            // k, not just k=0 — unlike the Laplace s=0 case, where every term past the constant
            // vanishes), and a zero/pole array uses `z_root_product_at_one` (its `(1 - root)`
            // term is genuinely complex-valued, unlike the Laplace fold's real-only `0`/`1`).
            ExprAst::Call { name, args }
                if matches!(name.as_str(), "zi_nd" | "zi_np" | "zi_zd" | "zi_zp") =>
            {
                let (value, num, den) = match (args.first(), args.get(1), args.get(2), args.len()) {
                    (Some(&v), Some(&n), Some(&d), 4..=6) => (v, n, d),
                    _ => {
                        return Err(elab(format!(
                            "`{name}` takes four to six arguments: value, then a numerator/zero \
                             argument, then a denominator/pole argument, the sample period T, \
                             and optionally a tolerance and start time, e.g. \
                             `{name}(sig, {{1}}, {{1, tau}}, T)`"
                        )))
                    }
                };
                let numer_is_zeros = matches!(name.as_str(), "zi_zd" | "zi_zp");
                let denom_is_poles = matches!(name.as_str(), "zi_np" | "zi_zp");
                let numer1 = if numer_is_zeros {
                    self.z_root_product_at_one(num, "zero")?
                } else {
                    self.array_lit_values(num, "numerator")?.iter().sum()
                };
                let denom1 = if denom_is_poles {
                    self.z_root_product_at_one(den, "pole")?
                } else {
                    self.array_lit_values(den, "denominator")?.iter().sum()
                };
                if denom1 == 0.0 {
                    return Err(elab(format!(
                        "`{name}`'s denominator is zero at z=1: the steady-state gain is \
                         undefined"
                    )));
                }
                let value_id = self.lower_expr(value)?;
                let gain_id = self.out.push_expr(Expr::Const(numer1 / denom1));
                Expr::Binary(va_ir::BinOp::Mul, value_id, gain_id)
            }
            // An array literal reaching here means it appeared somewhere other than a Laplace/
            // Z-domain filter's numerator/zero/denominator/pole argument (those cases read it
            // directly via `array_lit_first`/`array_lit_values`/the root-product helpers above,
            // never through `lower_expr`) — not a general-purpose value anywhere in the LRM.
            ExprAst::ArrayLit(_) => {
                return Err(elab(
                    "an array-literal expression (`{...}`) is only valid as a Laplace/Z-domain \
                     filter's numerator/zero/denominator/pole argument (§4.5.11/§4.5.12), not \
                     as a general-purpose value"
                        .to_string(),
                ))
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
        if args.len() == 1 && args[0].index.is_empty() {
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
        Ok(self.intern_branch(p, n))
    }

    /// Intern (or look up) the branch between an already-resolved terminal pair. Extracted out
    /// of [`Self::resolve_branch`] so a runtime-indexed vector-net access (§ dynamic vector-net
    /// indexing) can build one branch per candidate index of its expansion chain without
    /// re-deriving `p`/`n` from an `ast::NetArg` each time.
    fn intern_branch(&mut self, p: NodeId, n: NodeId) -> BranchId {
        let key = (p.0, n.0);
        if let Some(id) = self.branches.get(&key) {
            return *id;
        }
        let id = BranchId(self.out.branches.len() as u32);
        self.out.branches.push(Branch { p, n });
        self.branches.insert(key, id);
        id
    }

    /// Which (if any) position of `idxs` (0, 1, or 2 entries) is genuinely dynamic (not
    /// compile-time-constant/genvar). At most one may be — two dynamic positions on the same
    /// name is rejected here rather than expanded into an O(range²) chain, mirroring this file's
    /// existing precedent of rejecting a two-dynamic-*terminal* access rather than building one
    /// (see [`Self::dynamic_terminal_range`]'s doc comment) — now also enforced *within* a
    /// single § 2-D name's two index positions.
    fn dynamic_index_pos(
        &self,
        what: &str,
        idxs: &[ExprRef],
    ) -> Result<Option<usize>, FrontendError> {
        let mut dyn_dim = None;
        for (d, &e) in idxs.iter().enumerate() {
            if self.const_eval(e).is_err() {
                if dyn_dim.is_some() {
                    return Err(elab(format!(
                        "`{what}` has two dynamically-indexed dimensions in the same access; \
                         at most one index position may be a genuinely runtime expression"
                    )));
                }
                dyn_dim = Some(d);
            }
        }
        Ok(dyn_dim)
    }

    /// Resolve one [`ast::NetArg`] terminal to its [`NodeId`]: a plain net name, or one element
    /// of a vector net selected by a compile-time-constant or genvar expression (§ vector
    /// nets), bounds-checked against its declared dimension range(s). A genuinely runtime index
    /// (§ dynamic vector-net/array-variable indexing) is not resolvable to a single `NodeId`
    /// here at all — that case is detected earlier, by [`Self::dynamic_terminal_range`], and
    /// routed to [`Self::lower_probe_expr`]/[`Self::unroll_indexed_contribute`] instead, which
    /// call [`Self::resolve_vector_node_at`] (this method's constant-index tail, factored out)
    /// once per candidate index rather than once for a single statically-known one.
    fn resolve_net_arg(&mut self, arg: &ast::NetArg) -> Result<NodeId, FrontendError> {
        if arg.slice.is_some() {
            if let Some(dims) = self.vectors.get(&arg.name) {
                if dims.len() == 2 {
                    return Err(elab(format!(
                        "`{}[..]`: slicing a 2-D vector net is not supported (slicing is \
                         single-dimension-only); index both dimensions instead, e.g. \
                         `V({}[0][0])`",
                        arg.name, arg.name
                    )));
                }
            }
            if !arg.index.is_empty() {
                return Err(elab(format!(
                    "`{}[..]`: a `[lo:hi]` slice cannot be combined with an index — slicing is \
                     single-dimension-only",
                    arg.name
                )));
            }
            return Err(elab(format!(
                "`{}[..]` is a vector slice; a branch access/declaration needs a single node, \
                 e.g. `V({}[0])` (a slice is only valid as an instance port-connection argument)",
                arg.name, arg.name
            )));
        }
        if arg.index.is_empty() {
            if let Some(dims) = self.vectors.get(&arg.name) {
                let example = if dims.len() == 1 {
                    format!("`V({}[0])`", arg.name)
                } else {
                    format!("`V({}[0][0])`", arg.name)
                };
                return Err(elab(format!(
                    "`{}` is a {}-D vector net; an access must index every dimension, e.g. {}",
                    arg.name,
                    dims.len(),
                    example
                )));
            }
            return self
                .nodes
                .get(&arg.name)
                .copied()
                .ok_or_else(|| elab(format!("unknown net `{}` in branch access", arg.name)));
        }
        let idxs: Vec<i64> = arg
            .index
            .iter()
            .map(|&e| self.const_eval_int(e, "vector index"))
            .collect::<Result<_, _>>()?;
        self.resolve_vector_node_at(&arg.name, &idxs)
    }

    /// Resolve one already-known index tuple `idxs` of a declared vector net `name` to its
    /// [`NodeId`], bounds-checked against the vector's declared dimension range(s) (dimension
    /// count must also match, catching a partial/over-index like `grid[0]` on a declared-2-D
    /// `grid`). The constant-index tail of [`Self::resolve_net_arg`], factored out so a
    /// runtime-indexed access's expansion chain (§ dynamic vector-net/array-variable indexing)
    /// can resolve each concrete candidate index without an `ExprRef` for a literal — there is
    /// none, since a literal loop index doesn't come from the source AST.
    fn resolve_vector_node_at(&self, name: &str, idxs: &[i64]) -> Result<NodeId, FrontendError> {
        let dims = self.vectors.get(name).ok_or_else(|| {
            elab(format!(
                "`{name}` is not a vector net (no bracketed `[msb:lsb]` range declared)"
            ))
        })?;
        if dims.len() != idxs.len() {
            return Err(elab(format!(
                "`{name}` is declared with {} dimension(s) but accessed with {}",
                dims.len(),
                idxs.len()
            )));
        }
        for (d, (&(lo, hi), &idx)) in dims.iter().zip(idxs).enumerate() {
            if idx < lo || idx > hi {
                return Err(elab(format!(
                    "index {idx} is out of `{name}`'s declared dimension {d} range [{lo}:{hi}]"
                )));
            }
        }
        let key = indexed_key(name, idxs);
        self.nodes.get(&key).copied().ok_or_else(|| {
            elab(format!(
                "internal error: vector node `{key}` was not interned"
            ))
        })
    }

    /// Resolve an instance port-connection argument to the ordered node list it wires up — the
    /// connection-only counterpart of [`Self::resolve_net_arg`], which can only ever name one
    /// node. A bare vector-net name or an explicit `[msb:lsb]` slice both resolve to their full,
    /// ascending-index-order node list (`in[0:1]` → `[in[0], in[1]]`, matching how
    /// [`Self::resolve_ports`] already normalizes a vector *port*'s own node list — lowest index
    /// first regardless of which direction the source wrote), ready to zip element-wise against
    /// a same-width vector port's node list. A scalar net or a single `[i]` index still resolves
    /// to a one-element list, unifying the scalar and vector connection paths in
    /// [`Self::inline_instance`]. A § 2-D vector net may only connect as a fully 2-indexed
    /// single node — slicing it, or connecting it bare/partially indexed, is rejected (the same
    /// restrictions [`Self::resolve_net_arg`] applies to an `Access`/`branch` terminal).
    fn resolve_conn_nodes(&mut self, arg: &ast::NetArg) -> Result<Vec<NodeId>, FrontendError> {
        if let Some((msb, lsb)) = arg.slice {
            if let Some(dims) = self.vectors.get(&arg.name) {
                if dims.len() == 2 {
                    return Err(elab(format!(
                        "`{}[..]`: slicing a 2-D vector net is not supported (slicing is \
                         single-dimension-only); index both dimensions instead, e.g. \
                         `{}[0][0]`",
                        arg.name, arg.name
                    )));
                }
            }
            if !arg.index.is_empty() {
                return Err(elab(format!(
                    "`{}[..]`: a `[lo:hi]` slice cannot be combined with an index — slicing is \
                     single-dimension-only",
                    arg.name
                )));
            }
            let msb = self.const_eval_int(msb, "vector slice bound")?;
            let lsb = self.const_eval_int(lsb, "vector slice bound")?;
            let (lo, hi) = if msb <= lsb { (msb, lsb) } else { (lsb, msb) };
            return (lo..=hi)
                .map(|k| self.resolve_vector_node_at(&arg.name, &[k]))
                .collect();
        }
        if !arg.index.is_empty() {
            let idxs: Vec<i64> = arg
                .index
                .iter()
                .map(|&e| self.const_eval_int(e, "vector index"))
                .collect::<Result<_, _>>()?;
            return Ok(vec![self.resolve_vector_node_at(&arg.name, &idxs)?]);
        }
        if let Some(dims) = self.vectors.get(&arg.name) {
            if dims.len() != 1 {
                return Err(elab(format!(
                    "`{}` is a {}-D vector net; a bare (unindexed) port connection is only \
                     supported for a 1-D vector net",
                    arg.name,
                    dims.len()
                )));
            }
            let (lo, hi) = dims[0];
            return (lo..=hi)
                .map(|k| self.resolve_vector_node_at(&arg.name, &[k]))
                .collect();
        }
        Ok(vec![self.nodes.get(&arg.name).copied().ok_or_else(
            || elab(format!("unknown net `{}` in port connection", arg.name)),
        )?])
    }

    /// If `args` has exactly one terminal whose base name is a declared vector net and whose
    /// index expressions include exactly one that is present but not a compile-time constant
    /// (an ordinary runtime variable, e.g. an `integer` loop counter — confirmed needed by
    /// `adc_16bit_ideal.va`/`dac_16bit_ideal.va`'s bit-serialization loops), return it. Returns
    /// `Ok(None)` for the ordinary case (every index constant-resolvable, or no index at all),
    /// which the caller falls through to the existing `resolve_branch`/`lower_access` path for
    /// unchanged. A *second* dynamically-indexed terminal in the same access (`V(a[i], b[j])`
    /// with both `i`/`j` runtime) is left to `resolve_net_arg`'s ordinary error path rather than
    /// expanded into an O(range²) chain here — not evidenced anywhere in the corpus, and
    /// CLAUDE.md's scope discipline argues against building for a case nothing needs yet. A
    /// § 2-D vector net's *own* two index positions being simultaneously dynamic is instead
    /// caught eagerly by [`Self::dynamic_index_pos`], for the same O(range²)-avoidance reason.
    fn dynamic_terminal_range(
        &self,
        args: &[ast::NetArg],
    ) -> Result<Option<DynamicTerminal>, FrontendError> {
        for (pos, arg) in args.iter().enumerate() {
            let Some(dyn_dim) = self.dynamic_index_pos(&arg.name, &arg.index)? else {
                continue;
            };
            let dims = self.vectors.get(&arg.name).ok_or_else(|| {
                elab(format!(
                    "`{}` is not a vector net (no bracketed `[msb:lsb]` range declared)",
                    arg.name
                ))
            })?;
            if dims.len() != arg.index.len() {
                return Err(elab(format!(
                    "`{}` is declared with {} dimension(s) but accessed with {}",
                    arg.name,
                    dims.len(),
                    arg.index.len()
                )));
            }
            let other_idx = if dims.len() == 2 {
                Some(self.const_eval_int(arg.index[1 - dyn_dim], "vector index")?)
            } else {
                None
            };
            let (lo, hi) = dims[dyn_dim];
            return Ok(Some(DynamicTerminal {
                pos,
                dyn_dim,
                other_idx,
                name: arg.name.clone(),
                idx_expr: arg.index[dyn_dim],
                lo,
                hi,
            }));
        }
        Ok(None)
    }

    /// Lower a `V(...)`/`I(...)` probe to an `Expr`. The common case (every terminal's index,
    /// if any, is compile-time-constant/genvar) resolves directly to a single `Expr::Probe` via
    /// `lower_access`. When exactly one terminal is a vector-net access indexed by a genuinely
    /// runtime expression, there is no single `BranchId` to probe — a branch is a fixed `(p, n)`
    /// pair resolved once at elaboration — so this expands into a nested `Expr::Select` chain
    /// instead, one arm per declared index of the vector, guarded by `index == k`, each arm
    /// probing the concrete branch for that index. The LRM requires a vector net's *declared
    /// range* to be static, not that the selecting index be — nothing here contradicts that.
    /// The statement-level sibling of this (a runtime-indexed *contribution target*, which
    /// can't be an expression at all) is [`Self::unroll_indexed_contribute`].
    ///
    /// **Limitation**: the chain's final (unconditional) arm is index `hi`. A runtime index
    /// that falls outside the vector's declared range at simulation time silently resolves to
    /// that arm rather than erroring — there is no runtime-error concept in this IR/ABI. Every
    /// corpus model driving this path bounds its loop to the array's own declared range, so the
    /// fallback arm is never actually reached in practice.
    fn lower_probe_expr(&mut self, access: &ast::Access) -> Result<ExprId, FrontendError> {
        let Some(dyn_term) = self.dynamic_terminal_range(&access.args)? else {
            let a = self.lower_access(access)?;
            return Ok(self.out.push_expr(Expr::Probe(a)));
        };
        let DynamicTerminal {
            pos,
            dyn_dim,
            other_idx,
            name,
            idx_expr,
            lo,
            hi,
        } = dyn_term;
        let kind = match access.kind {
            ast::AccessKind::Potential => AccessKind::Potential,
            ast::AccessKind::Flow => AccessKind::Flow,
        };
        let idx = self.lower_expr(idx_expr)?;
        let other = if access.args.len() >= 2 {
            Some(self.resolve_net_arg(&access.args[1 - pos])?)
        } else {
            None
        };
        let mut chain: Option<ExprId> = None;
        for k in (lo..=hi).rev() {
            let full = combine_idx(dyn_dim, k, other_idx);
            let node_k = self.resolve_vector_node_at(&name, &full)?;
            let (p, n) = if pos == 0 {
                let n = match other {
                    Some(n) => n,
                    None => self.reference_node(),
                };
                (node_k, n)
            } else {
                (
                    other.expect("a dynamically-indexed second terminal implies a first one"),
                    node_k,
                )
            };
            let branch = self.intern_branch(p, n);
            let probe = self.out.push_expr(Expr::Probe(Access { kind, branch }));
            chain = Some(match chain {
                None => probe,
                Some(rest) => {
                    let k_const = self.out.push_expr(Expr::Const(k as f64));
                    let cond = self
                        .out
                        .push_expr(Expr::Binary(va_ir::BinOp::Eq, idx, k_const));
                    self.out.push_expr(Expr::Select(cond, probe, rest))
                }
            });
        }
        Ok(chain.expect("a declared vector net's range is always non-empty"))
    }

    /// Statement-level sibling of [`Self::lower_probe_expr`]: `V(vec[j]) <+ value;` where `j`
    /// is a genuinely runtime expression expands into an if/else-if chain, one
    /// `Stmt::Contribute` per declared index of the vector, guarded by `j == k`. `value` is
    /// lowered once, up front, and the resulting `ExprId` is shared across every arm — safe
    /// because it's a pure arena reference, not a re-evaluation, and (if `value` itself reads
    /// the same runtime index, as `out_val[j]` does in `adc_16bit_ideal.va`) it stays
    /// self-consistent with the guard: the same `j` value that selects an arm here has already
    /// selected the matching arm of `value`'s own `Expr::Select` chain.
    fn unroll_indexed_contribute(
        &mut self,
        kind: AccessKind,
        dyn_term: DynamicTerminal,
        args: &[ast::NetArg],
        value: ExprRef,
    ) -> Result<va_ir::Stmt, FrontendError> {
        let DynamicTerminal {
            pos,
            dyn_dim,
            other_idx,
            name,
            idx_expr,
            lo,
            hi,
        } = dyn_term;
        let name = name.as_str();
        let idx = self.lower_expr(idx_expr)?;
        let value = self.lower_expr(value)?;
        let other = if args.len() >= 2 {
            Some(self.resolve_net_arg(&args[1 - pos])?)
        } else {
            None
        };
        let mut chain: Option<va_ir::Stmt> = None;
        for k in (lo..=hi).rev() {
            let full = combine_idx(dyn_dim, k, other_idx);
            let node_k = self.resolve_vector_node_at(name, &full)?;
            let (p, n) = if pos == 0 {
                let n = match other {
                    Some(n) => n,
                    None => self.reference_node(),
                };
                (node_k, n)
            } else {
                (
                    other.expect("a dynamically-indexed second terminal implies a first one"),
                    node_k,
                )
            };
            let branch = self.intern_branch(p, n);
            let contribute = va_ir::Stmt::Contribute {
                target: Access { kind, branch },
                value,
            };
            chain = Some(match chain {
                None => contribute,
                Some(rest) => {
                    let k_const = self.out.push_expr(Expr::Const(k as f64));
                    let cond = self
                        .out
                        .push_expr(Expr::Binary(va_ir::BinOp::Eq, idx, k_const));
                    va_ir::Stmt::If {
                        cond,
                        then_: vec![contribute],
                        else_: vec![rest],
                    }
                }
            });
        }
        Ok(chain.expect("a declared vector net's range is always non-empty"))
    }

    /// Lower `I(<port>)` (§ port-current probe, LRM §5.4.3) to an `Expr`: the current flowing
    /// into this module through `port`, computed as the signed sum of every flow contribution
    /// already made (elsewhere in this same analog block, at or before this point in source
    /// order — see [`Self::collect_port_flow_contributions`]) to a branch touching the port's
    /// node. `port` must name one of this module's own scalar ports (a vector port is a stated
    /// v1 limitation — the LRM allows one, but no corpus need has surfaced it yet); `kind` must
    /// be [`ast::AccessKind::Flow`] (`V(<port>)` is explicitly invalid per the LRM).
    ///
    /// **Sign convention**: a branch `(p, n)` contributed `I(p, n) <+ value;` sends `value`
    /// current *from `p` to `n`* (the LRM's own convention — confirmed against its diode worked
    /// example, where a forward-biased `branch(a, c)` with positive `V(a,c)` contributes
    /// positive current, matching conventional current flow from anode `a` to cathode `c`). By
    /// conservation, whatever current a branch sends *away* from the probed port's node must be
    /// *supplied from outside* through the port, and whatever a branch sends *into* the node
    /// reduces (or reverses) what's needed from outside — so a branch where the port's node is
    /// `p` contributes `+value`, and one where it's `n` contributes `-value`.
    fn lower_port_probe(
        &mut self,
        kind: ast::AccessKind,
        port: &str,
    ) -> Result<ExprId, FrontendError> {
        if kind != ast::AccessKind::Flow {
            return Err(elab(format!(
                "`V(<{port}>)` is invalid — a port-current probe is a flow access only \
                 (LRM §5.4.3); use `I(<{port}>)`"
            )));
        }
        if !self.ast.ports.iter().any(|p| p == port) {
            return Err(elab(format!(
                "`{port}` is not a declared port of this module; a port-current probe can only \
                 name one of this module's own ports"
            )));
        }
        if let Some(dims) = self.vectors.get(port) {
            return Err(elab(format!(
                "`{port}` is a {}-D vector port; port-current probes are only supported for a \
                 scalar port (v1 limitation)",
                dims.len()
            )));
        }
        let node = *self
            .nodes
            .get(port)
            .ok_or_else(|| elab(format!("port `{port}` has no discipline declaration")))?;

        let mut terms = Vec::new();
        self.collect_port_flow_contributions(node, &self.out.analog.clone(), &[], &mut terms)?;

        let mut sum = None;
        for (sign, value) in terms {
            let signed = if sign < 0.0 {
                self.out.push_expr(Expr::Unary(va_ir::UnOp::Neg, value))
            } else {
                value
            };
            sum = Some(match sum {
                None => signed,
                Some(acc) => self
                    .out
                    .push_expr(Expr::Binary(va_ir::BinOp::Add, acc, signed)),
            });
        }
        Ok(sum.unwrap_or_else(|| self.out.push_expr(Expr::Const(0.0))))
    }

    /// Recursively walk `stmts` (a prefix of `self.out.analog`, already fully lowered) for every
    /// `Stmt::Contribute` of [`AccessKind::Flow`] whose branch touches `node`, collecting
    /// `(sign, value)` pairs into `out` — the constant-additive-fold half of
    /// [`Self::lower_port_probe`]. `guards` accumulates the `If` conditions (as already-lowered
    /// `ExprId`s) enclosing the current position; a qualifying contribution found `n` levels
    /// deep in nested `if`s has its value wrapped in `n` nested `Expr::Select(guard, value, 0)`s,
    /// so a conditionally-made contribution only counts when its condition actually held —
    /// **not** applied to a contribution outside any `if` (`guards` empty), which counts
    /// unconditionally, matching the direct real-corpus idiom (`external/hicumL0_v2p0p0.va`'s
    /// `IB = I(<b>);`, read after every port-touching branch's contribution already ran
    /// unconditionally earlier in the same block).
    ///
    /// **Limitation**: a qualifying contribution found inside a `case`/`for`/`while`/`repeat` is
    /// rejected with a clear error rather than silently mis-summed or silently dropped — those
    /// need either a per-arm equality guard (`case`) or genuine loop-carried accumulation
    /// (`for`/`while`/`repeat`), neither of which this fold attempts; no corpus need for either
    /// has surfaced yet (§ port-current probe).
    fn collect_port_flow_contributions(
        &mut self,
        node: NodeId,
        stmts: &[va_ir::Stmt],
        guards: &[ExprId],
        out: &mut Vec<(f64, ExprId)>,
    ) -> Result<(), FrontendError> {
        for stmt in stmts {
            match stmt {
                va_ir::Stmt::Contribute { target, value } if target.kind == AccessKind::Flow => {
                    let branch = self.out.branches[target.branch.0 as usize];
                    let sign = if branch.p == node {
                        1.0
                    } else if branch.n == node {
                        -1.0
                    } else {
                        continue;
                    };
                    let mut v = *value;
                    for &g in guards.iter().rev() {
                        let zero = self.out.push_expr(Expr::Const(0.0));
                        v = self.out.push_expr(Expr::Select(g, v, zero));
                    }
                    out.push((sign, v));
                }
                va_ir::Stmt::Contribute { .. } | va_ir::Stmt::Assign { .. } => {}
                va_ir::Stmt::Block(body) => {
                    self.collect_port_flow_contributions(node, body, guards, out)?;
                }
                va_ir::Stmt::If { cond, then_, else_ } => {
                    let mut then_guards = guards.to_vec();
                    then_guards.push(*cond);
                    self.collect_port_flow_contributions(node, then_, &then_guards, out)?;

                    let not_cond = self.out.push_expr(Expr::Unary(va_ir::UnOp::Not, *cond));
                    let mut else_guards = guards.to_vec();
                    else_guards.push(not_cond);
                    self.collect_port_flow_contributions(node, else_, &else_guards, out)?;
                }
                va_ir::Stmt::Case { arms, default, .. } => {
                    let hit = arms.iter().any(|a| self.branch_flow_touches(node, &a.body))
                        || self.branch_flow_touches(node, default);
                    if hit {
                        return Err(elab(
                            "a port-current probe (§ port-current probe) can't sum a flow \
                             contribution made inside a `case` arm — not yet supported"
                                .to_string(),
                        ));
                    }
                }
                va_ir::Stmt::For { body, .. }
                | va_ir::Stmt::While { body, .. }
                | va_ir::Stmt::Repeat { body, .. } => {
                    if self.branch_flow_touches(node, body) {
                        return Err(elab(
                            "a port-current probe (§ port-current probe) can't sum a flow \
                             contribution made inside a loop — not yet supported"
                                .to_string(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether `stmts` contains, anywhere (at any nesting depth), a flow contribution to a
    /// branch touching `node` — the presence-only check [`Self::collect_port_flow_contributions`]
    /// uses to detect an unsupported case (a `case`/loop body it can't soundly fold) rather than
    /// silently under-counting.
    fn branch_flow_touches(&self, node: NodeId, stmts: &[va_ir::Stmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            va_ir::Stmt::Contribute { target, .. } if target.kind == AccessKind::Flow => {
                let branch = self.out.branches[target.branch.0 as usize];
                branch.p == node || branch.n == node
            }
            va_ir::Stmt::Contribute { .. } | va_ir::Stmt::Assign { .. } => false,
            va_ir::Stmt::Block(body) => self.branch_flow_touches(node, body),
            va_ir::Stmt::If { then_, else_, .. } => {
                self.branch_flow_touches(node, then_) || self.branch_flow_touches(node, else_)
            }
            va_ir::Stmt::Case { arms, default, .. } => {
                arms.iter().any(|a| self.branch_flow_touches(node, &a.body))
                    || self.branch_flow_touches(node, default)
            }
            va_ir::Stmt::For { body, .. }
            | va_ir::Stmt::While { body, .. }
            | va_ir::Stmt::Repeat { body, .. } => self.branch_flow_touches(node, body),
        })
    }

    /// Resolve one element of an array variable (§ array variables) to its [`VarId`] — the
    /// `VarId` counterpart of [`Self::resolve_net_arg`]'s vector-net indexing. Each entry of
    /// `idxs` must be a compile-time-constant or genvar expression; a genuinely runtime index
    /// (§ dynamic vector-net/array-variable indexing) is not resolvable to a single `VarId`
    /// here — that case is detected earlier and routed to
    /// [`Self::lower_indexed_var_read`]/[`Self::lower_indexed_var_write`] instead, which call
    /// [`Self::resolve_array_var_at`] (this method's constant-index tail, factored out) once
    /// per candidate index.
    fn resolve_var_array_index(
        &mut self,
        name: &str,
        idxs: &[ExprRef],
    ) -> Result<VarId, FrontendError> {
        let idxs: Vec<i64> = idxs
            .iter()
            .map(|&e| self.const_eval_int(e, "array variable index"))
            .collect::<Result<_, _>>()?;
        self.resolve_array_var_at(name, &idxs)
    }

    /// Resolve one already-known index tuple `idxs` of a declared array variable `name` to its
    /// [`VarId`], bounds-checked against the array's declared dimension range(s) (dimension
    /// count must also match, catching a partial/over-index). The constant-index tail of
    /// [`Self::resolve_var_array_index`], factored out for the same reason as
    /// [`Self::resolve_vector_node_at`] is: a runtime-indexed expansion chain needs to resolve
    /// several concrete literal indices, none of which have an `ExprRef` of their own.
    fn resolve_array_var_at(&self, name: &str, idxs: &[i64]) -> Result<VarId, FrontendError> {
        let dims = self.var_arrays.get(name).ok_or_else(|| {
            elab(format!(
                "`{name}` is not an array variable (no bracketed `[msb:lsb]` declaration)"
            ))
        })?;
        if dims.len() != idxs.len() {
            return Err(elab(format!(
                "`{name}` is declared with {} dimension(s) but accessed with {}",
                dims.len(),
                idxs.len()
            )));
        }
        for (d, (&(lo, hi), &idx)) in dims.iter().zip(idxs).enumerate() {
            if idx < lo || idx > hi {
                return Err(elab(format!(
                    "index {idx} is out of `{name}`'s declared dimension {d} range [{lo}:{hi}]"
                )));
            }
        }
        let key = indexed_key(name, idxs);
        self.vars.get(&key).copied().ok_or_else(|| {
            elab(format!(
                "internal error: array variable node `{key}` was not interned"
            ))
        })
    }

    /// Array-variable counterpart of [`Self::dynamic_terminal_range`]: if `idxs` includes
    /// exactly one genuinely runtime (non-constant, non-genvar) entry, return it; two dynamic
    /// entries is rejected by [`Self::dynamic_index_pos`] before this is even reached.
    fn dynamic_var_index(
        &self,
        name: &str,
        idxs: &[ExprRef],
    ) -> Result<Option<DynamicVarIndex>, FrontendError> {
        let Some(dyn_dim) = self.dynamic_index_pos(name, idxs)? else {
            return Ok(None);
        };
        let dims = self.var_arrays.get(name).ok_or_else(|| {
            elab(format!(
                "`{name}` is not an array variable (no bracketed `[msb:lsb]` declaration)"
            ))
        })?;
        if dims.len() != idxs.len() {
            return Err(elab(format!(
                "`{name}` is declared with {} dimension(s) but accessed with {}",
                dims.len(),
                idxs.len()
            )));
        }
        let other_idx = if dims.len() == 2 {
            Some(self.const_eval_int(idxs[1 - dyn_dim], "array variable index")?)
        } else {
            None
        };
        let (lo, hi) = dims[dyn_dim];
        Ok(Some(DynamicVarIndex {
            dyn_dim,
            other_idx,
            lo,
            hi,
        }))
    }

    /// Lower `name[index]` / `name[i][j]` (one element of a 1-D or § 2-D array variable) to an
    /// `Expr`. The common case (every index compile-time-constant/genvar) resolves directly to
    /// the concrete element's `Expr::Var`. When exactly one index is a genuinely runtime
    /// expression (an ordinary `integer` loop counter, say — confirmed needed by
    /// `adc_16bit_ideal.va`/`dac_16bit_ideal.va`), there is no single `VarId` to read at
    /// elaboration time; expand into a nested `Expr::Select` chain instead, one arm per declared
    /// value of that one dimension, guarded by `index == k` — the expression-level sibling of
    /// [`Self::lower_indexed_var_write`]'s statement-level `If` chain, and structurally
    /// identical to [`Self::lower_probe_expr`]'s (same fallback-arm limitation: an
    /// out-of-declared-range runtime index resolves to the `hi` arm rather than erroring, since
    /// there is no runtime-error concept in this IR/ABI).
    fn lower_indexed_var_read(
        &mut self,
        name: &str,
        idxs: &[ExprRef],
    ) -> Result<ExprId, FrontendError> {
        let Some(DynamicVarIndex {
            dyn_dim,
            other_idx,
            lo,
            hi,
        }) = self.dynamic_var_index(name, idxs)?
        else {
            let id = self.resolve_var_array_index(name, idxs)?;
            return Ok(self.out.push_expr(Expr::Var(id)));
        };
        let idx = self.lower_expr(idxs[dyn_dim])?;
        let mut chain: Option<ExprId> = None;
        for k in (lo..=hi).rev() {
            let full = combine_idx(dyn_dim, k, other_idx);
            let id = self.resolve_array_var_at(name, &full)?;
            let read = self.out.push_expr(Expr::Var(id));
            chain = Some(match chain {
                None => read,
                Some(rest) => {
                    let k_const = self.out.push_expr(Expr::Const(k as f64));
                    let cond = self
                        .out
                        .push_expr(Expr::Binary(va_ir::BinOp::Eq, idx, k_const));
                    self.out.push_expr(Expr::Select(cond, read, rest))
                }
            });
        }
        Ok(chain.expect("a declared array variable's range is always non-empty"))
    }

    /// Statement-level sibling of [`Self::lower_indexed_var_read`]: `name[index] = rhs;` /
    /// `name[i][j] = rhs;` where exactly one index is a genuinely runtime expression expands
    /// into an if/else-if chain, one `Stmt::Assign` per declared value of that one dimension,
    /// guarded by `index == k`. `rhs` is lowered once, up front, and shared across every arm
    /// (same reasoning as [`Self::unroll_indexed_contribute`]'s shared `value`). The
    /// every-index-constant case resolves directly to a single `Stmt::Assign`, mirroring
    /// [`Self::lower_indexed_var_read`]'s dual-path shape.
    fn lower_indexed_var_write(
        &mut self,
        name: &str,
        idxs: &[ExprRef],
        rhs: ExprRef,
    ) -> Result<va_ir::Stmt, FrontendError> {
        let Some(DynamicVarIndex {
            dyn_dim,
            other_idx,
            lo,
            hi,
        }) = self.dynamic_var_index(name, idxs)?
        else {
            let id = self.resolve_var_array_index(name, idxs)?;
            let rhs = self.lower_expr(rhs)?;
            return Ok(va_ir::Stmt::Assign { lhs: id, rhs });
        };
        let idx = self.lower_expr(idxs[dyn_dim])?;
        let rhs = self.lower_expr(rhs)?;
        let mut chain: Option<va_ir::Stmt> = None;
        for k in (lo..=hi).rev() {
            let full = combine_idx(dyn_dim, k, other_idx);
            let id = self.resolve_array_var_at(name, &full)?;
            let assign = va_ir::Stmt::Assign { lhs: id, rhs };
            chain = Some(match chain {
                None => assign,
                Some(rest) => {
                    let k_const = self.out.push_expr(Expr::Const(k as f64));
                    let cond = self
                        .out
                        .push_expr(Expr::Binary(va_ir::BinOp::Eq, idx, k_const));
                    va_ir::Stmt::If {
                        cond,
                        then_: vec![assign],
                        else_: vec![rest],
                    }
                }
            });
        }
        Ok(chain.expect("a declared array variable's range is always non-empty"))
    }

    /// The implicit global reference node, created on first single-terminal access.
    fn reference_node(&mut self) -> NodeId {
        if let Some(id) = self.ground {
            return id;
        }
        let id = self.intern_node("gnd", Discipline::Electrical, None);
        self.ground = Some(id);
        id
    }

    // --- pass: module instantiation (§ module instantiation) --------------------------

    /// Resolve every [`Item::Instance`] in this module: recursively elaborate the referenced
    /// submodule and inline it into `self.out` (see [`Self::inline_instance`]).
    fn collect_instances(&mut self) -> Result<(), FrontendError> {
        let mut seen_names = std::collections::HashSet::new();
        for item in &self.ast.items {
            if let Item::Instance {
                module,
                name,
                params,
                connections,
            } = item
            {
                if !seen_names.insert(name.clone()) {
                    return Err(elab(format!("duplicate instance name `{name}`")));
                }
                self.inline_instance(module, name, params, connections)?;
            }
        }
        Ok(())
    }

    /// Elaborate `module_name` (from [`Self::library`]) as an independent module — with
    /// `param_overrides` evaluated in *this* module's scope substituted for its parameter
    /// defaults — then inline the result into `self.out` under the hierarchical namespace
    /// `inst_name` (§ module instantiation).
    fn inline_instance(
        &mut self,
        module_name: &str,
        inst_name: &str,
        param_overrides: &[(String, ExprRef)],
        connections: &[ast::PortConn],
    ) -> Result<(), FrontendError> {
        if module_name == self.ast.name || self.stack.iter().any(|s| s == module_name) {
            let mut chain: Vec<&str> = self.stack.iter().map(String::as_str).collect();
            chain.push(&self.ast.name);
            return Err(elab(format!(
                "instantiation cycle: `{module_name}` (instantiated as `{inst_name}` inside \
                 `{}`) already appears in the elaboration chain: {}",
                self.ast.name,
                chain.join(" -> ")
            )));
        }
        let sub_ast = self
            .library
            .iter()
            .find(|m| m.name == module_name)
            .ok_or_else(|| {
                elab(format!(
                    "instance `{inst_name}` references unknown module `{module_name}` (no sibling \
                 `module {module_name} ... endmodule` in this compilation unit)"
                ))
            })?;

        let mut overrides: HashMap<String, f64> = HashMap::new();
        for (pname, expr) in param_overrides {
            overrides.insert(pname.clone(), self.const_eval(*expr)?);
        }
        for pname in overrides.keys() {
            if !sub_ast
                .items
                .iter()
                .any(|it| matches!(it, Item::Param { name, .. } if name == pname))
            {
                return Err(elab(format!(
                    "instance `{inst_name}` overrides unknown parameter `{pname}` of module \
                     `{module_name}`"
                )));
            }
        }

        let mut child_stack: Vec<String> = self.stack.to_vec();
        child_stack.push(self.ast.name.clone());
        let sub = elaborate_inner(
            sub_ast,
            self.library,
            &child_stack,
            &overrides,
            self.disciplines,
            self.natures,
        )?;

        if connections.len() != sub.ports.len() {
            return Err(elab(format!(
                "instance `{inst_name}` of `{module_name}` connects {} port(s), but the module \
                 declares {}",
                connections.len(),
                sub.ports.len()
            )));
        }

        let all_positional = connections
            .iter()
            .all(|c| matches!(c, ast::PortConn::Positional(_)));
        let all_named = connections
            .iter()
            .all(|c| matches!(c, ast::PortConn::Named { .. }));
        if !all_positional && !all_named {
            return Err(elab(format!(
                "instance `{inst_name}`: cannot mix positional and named port connections"
            )));
        }

        let mut node_map: HashMap<NodeId, NodeId> = HashMap::new();
        if all_positional {
            for (i, conn) in connections.iter().enumerate() {
                let ast::PortConn::Positional(net_arg) = conn else {
                    unreachable!()
                };
                let parent_nodes = self.resolve_conn_nodes(net_arg)?;
                bind_port_nodes(
                    inst_name,
                    module_name,
                    &(i + 1).to_string(),
                    &sub.ports[i],
                    &parent_nodes,
                    &mut node_map,
                )?;
            }
        } else {
            let mut covered = vec![false; sub_ast.ports.len()];
            for conn in connections {
                let ast::PortConn::Named { port, net } = conn else {
                    unreachable!()
                };
                let idx = sub_ast
                    .ports
                    .iter()
                    .position(|p| p == port)
                    .ok_or_else(|| {
                        elab(format!(
                            "instance `{inst_name}` of `{module_name}`: no port named `{port}`"
                        ))
                    })?;
                if covered[idx] {
                    return Err(elab(format!(
                        "instance `{inst_name}` of `{module_name}`: port `{port}` connected \
                         more than once"
                    )));
                }
                covered[idx] = true;
                let parent_nodes = self.resolve_conn_nodes(net)?;
                bind_port_nodes(
                    inst_name,
                    module_name,
                    port,
                    &sub.ports[idx],
                    &parent_nodes,
                    &mut node_map,
                )?;
            }
        }

        self.merge_submodule(inst_name, sub, node_map);
        Ok(())
    }

    /// Inline an already-elaborated submodule's arenas into `self.out`: port nodes alias
    /// whatever parent node `node_map` resolved them to; every other node, branch, var,
    /// function, and expression is copied in with its embedded indices remapped, namespaced
    /// `"{inst_name}.{name}"` where it carries a name (§ module instantiation). IR arenas are
    /// strictly append-only — every `Expr`/`Stmt` only ever references an earlier index — so a
    /// single forward pass per arena, building an old→new index table as it goes, needs no
    /// fixup pass. The submodule's whole inlined analog body is pushed as one
    /// [`va_ir::Stmt::Block`], grouped per instance for readability.
    fn merge_submodule(&mut self, inst_name: &str, sub: Module, node_map: HashMap<NodeId, NodeId>) {
        let mut node_off: Vec<NodeId> = Vec::with_capacity(sub.nodes.len());
        for (i, decl) in sub.nodes.iter().enumerate() {
            let id = NodeId(i as u32);
            if let Some(&parent_id) = node_map.get(&id) {
                node_off.push(parent_id);
            } else {
                let new_id = NodeId(self.out.nodes.len() as u32);
                self.out.nodes.push(NodeDecl {
                    name: format!("{inst_name}.{}", decl.name),
                    discipline: decl.discipline,
                    abstol: decl.abstol,
                });
                node_off.push(new_id);
            }
        }

        let branch_off: Vec<BranchId> = sub
            .branches
            .iter()
            .map(|b| self.intern_branch(node_off[b.p.0 as usize], node_off[b.n.0 as usize]))
            .collect();

        let var_off: Vec<VarId> = sub
            .vars
            .iter()
            .map(|v| self.new_var(&format!("{inst_name}.{}", v.name)))
            .collect();

        let func_base = self.out.functions.len() as u32;
        let func_off: Vec<FuncId> = (0..sub.functions.len())
            .map(|i| FuncId(func_base + i as u32))
            .collect();

        let mut expr_off: Vec<ExprId> = Vec::with_capacity(sub.exprs.len());
        for e in &sub.exprs {
            let remapped = remap_expr(e, &sub, &branch_off, &var_off, &func_off, &expr_off);
            expr_off.push(self.out.push_expr(remapped));
        }

        for f in &sub.functions {
            self.out.functions.push(Function {
                name: format!("{inst_name}.{}", f.name),
                args: f.args.iter().map(|v| var_off[v.0 as usize]).collect(),
                arg_dirs: f.arg_dirs.clone(),
                ret: var_off[f.ret.0 as usize],
                body: f
                    .body
                    .iter()
                    .map(|s| remap_stmt(s, &branch_off, &var_off, &expr_off))
                    .collect(),
            });
        }

        let inlined: Vec<va_ir::Stmt> = sub
            .analog
            .iter()
            .map(|s| remap_stmt(s, &branch_off, &var_off, &expr_off))
            .collect();
        self.out.analog.push(va_ir::Stmt::Block(inlined));
    }
}

// --- free helpers --------------------------------------------------------------------

fn elab(msg: String) -> FrontendError {
    FrontendError::Elaborate(msg)
}

/// Bind a submodule port's node list to a resolved connection's node list, element-wise, in
/// [`Elaborator::inline_instance`] — used for both positional (`port_label` = 1-based port
/// number) and named (`port_label` = port name) connections. Both a scalar port (`sub_nodes.len()
/// == 1`) and a vector port take the same path: `sub_nodes`/`parent_nodes` are already the full
/// ascending-index-order lists ([`Elaborator::resolve_ports`], [`Elaborator::resolve_conn_nodes`]),
/// so a width mismatch here means the connection's own width — not just port count — disagrees
/// with the module's declared port width.
fn bind_port_nodes(
    inst_name: &str,
    module_name: &str,
    port_label: &str,
    sub_nodes: &[NodeId],
    parent_nodes: &[NodeId],
    node_map: &mut HashMap<NodeId, NodeId>,
) -> Result<(), FrontendError> {
    if sub_nodes.len() != parent_nodes.len() {
        return Err(elab(format!(
            "instance `{inst_name}` of `{module_name}`: port `{port_label}` is {}-wide but the \
             connection is {}-wide",
            sub_nodes.len(),
            parent_nodes.len()
        )));
    }
    for (&sub_node, &parent_node) in sub_nodes.iter().zip(parent_nodes.iter()) {
        node_map.insert(sub_node, parent_node);
    }
    Ok(())
}

/// Remap an already-elaborated submodule expression's embedded indices into the parent's
/// arenas (§ module instantiation, [`Elaborator::merge_submodule`]). `Expr::Param` collapses to
/// `Expr::Const` using the submodule's own (override-applied) resolved value: parameters are
/// compile-time constants, so they are never themselves copied into the parent's `params`
/// arena — only their baked-in value survives.
fn remap_expr(
    e: &Expr,
    sub: &Module,
    branch_off: &[BranchId],
    var_off: &[VarId],
    func_off: &[FuncId],
    expr_off: &[ExprId],
) -> Expr {
    match e {
        Expr::Const(v) => Expr::Const(*v),
        Expr::Param(pid) => Expr::Const(sub.params[pid.0 as usize].default),
        Expr::Var(vid) => Expr::Var(var_off[vid.0 as usize]),
        Expr::Probe(a) => Expr::Probe(remap_access(a, branch_off)),
        Expr::Unary(op, a) => Expr::Unary(*op, expr_off[a.0 as usize]),
        Expr::Binary(op, a, b) => Expr::Binary(*op, expr_off[a.0 as usize], expr_off[b.0 as usize]),
        Expr::Call(b, args) => {
            Expr::Call(*b, args.iter().map(|a| expr_off[a.0 as usize]).collect())
        }
        Expr::CallUser(fid, args) => Expr::CallUser(
            func_off[fid.0 as usize],
            args.iter().map(|a| expr_off[a.0 as usize]).collect(),
        ),
        Expr::Select(c, t, f) => Expr::Select(
            expr_off[c.0 as usize],
            expr_off[t.0 as usize],
            expr_off[f.0 as usize],
        ),
        Expr::Ddx(a, acc) => Expr::Ddx(expr_off[a.0 as usize], remap_access(acc, branch_off)),
    }
}

/// Remap an [`Access`]'s [`BranchId`] into the parent's branch arena.
fn remap_access(a: &Access, branch_off: &[BranchId]) -> Access {
    Access {
        kind: a.kind,
        branch: branch_off[a.branch.0 as usize],
    }
}

/// Remap an already-elaborated submodule statement's embedded indices into the parent's
/// arenas, recursing through nested control flow (see [`remap_expr`]).
fn remap_stmt(
    s: &va_ir::Stmt,
    branch_off: &[BranchId],
    var_off: &[VarId],
    expr_off: &[ExprId],
) -> va_ir::Stmt {
    let recurse = |body: &[va_ir::Stmt]| -> Vec<va_ir::Stmt> {
        body.iter()
            .map(|s| remap_stmt(s, branch_off, var_off, expr_off))
            .collect()
    };
    match s {
        va_ir::Stmt::Contribute { target, value } => va_ir::Stmt::Contribute {
            target: remap_access(target, branch_off),
            value: expr_off[value.0 as usize],
        },
        va_ir::Stmt::If { cond, then_, else_ } => va_ir::Stmt::If {
            cond: expr_off[cond.0 as usize],
            then_: recurse(then_),
            else_: recurse(else_),
        },
        va_ir::Stmt::Assign { lhs, rhs } => va_ir::Stmt::Assign {
            lhs: var_off[lhs.0 as usize],
            rhs: expr_off[rhs.0 as usize],
        },
        va_ir::Stmt::Block(body) => va_ir::Stmt::Block(recurse(body)),
        va_ir::Stmt::While { cond, body } => va_ir::Stmt::While {
            cond: expr_off[cond.0 as usize],
            body: recurse(body),
        },
        va_ir::Stmt::For {
            init,
            cond,
            step,
            body,
        } => va_ir::Stmt::For {
            init: Box::new(remap_stmt(init, branch_off, var_off, expr_off)),
            cond: expr_off[cond.0 as usize],
            step: Box::new(remap_stmt(step, branch_off, var_off, expr_off)),
            body: recurse(body),
        },
        va_ir::Stmt::Repeat { count, body } => va_ir::Stmt::Repeat {
            count: expr_off[count.0 as usize],
            body: recurse(body),
        },
        va_ir::Stmt::Case {
            selector,
            arms,
            default,
        } => va_ir::Stmt::Case {
            selector: expr_off[selector.0 as usize],
            arms: arms
                .iter()
                .map(|arm| CaseArm {
                    labels: arm.labels.iter().map(|l| expr_off[l.0 as usize]).collect(),
                    body: recurse(&arm.body),
                })
                .collect(),
            default: recurse(default),
        },
    }
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
    use crate::{
        lexer::lex,
        parser::{parse, parse_with_disciplines},
    };

    fn elaborate_src(src: &str) -> Module {
        let toks = lex(src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let ast = asts.into_iter().next().expect("at least one module");
        elaborate(&ast).expect("elaborate")
    }

    /// Elaborate `top` (by name) from a multi-module source, with every module in `src`
    /// available as its submodule library (§ module instantiation).
    fn elaborate_top(src: &str, top: &str) -> Module {
        let toks = lex(src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let ast = asts
            .iter()
            .find(|m| m.name == top)
            .unwrap_or_else(|| panic!("top module `{top}` present"));
        elaborate_with_library(ast, &asts).expect("elaborate")
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

    // --- § nature-metadata wiring (`abstol`) ----------------------------------------------

    #[test]
    fn discipline_preamble_resolves_a_nets_abstol() {
        let src = "nature Voltage; units = \"V\"; access = V; abstol = 1e-6; endnature \
                   nature Current; units = \"A\"; access = I; abstol = 1e-12; endnature \
                   discipline electrical; potential Voltage; flow Current; enddiscipline \
                   module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let (asts, natures, disciplines) = parse_with_disciplines(&toks).expect("parse");
        let ast = &asts[0];
        let m = elaborate_with_library_and_disciplines(ast, &asts, &disciplines, &natures)
            .expect("elaborate");
        assert_eq!(m.nodes.len(), 2);
        assert_eq!(m.nodes[0].abstol, Some(1e-6));
        assert_eq!(m.nodes[1].abstol, Some(1e-6));
    }

    #[test]
    fn plain_elaborate_still_gives_no_abstol_without_a_preamble() {
        // Regression: `elaborate`/`elaborate_with_library` (no discipline/nature tables
        // supplied) must keep their exact prior behavior — every node's `abstol` stays `None`,
        // even for an ordinary `electrical` net with no preamble at all.
        let m =
            elaborate_src("module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule");
        assert_eq!(m.nodes[0].abstol, None);
        assert_eq!(m.nodes[1].abstol, None);
    }

    #[test]
    fn discipline_preamble_with_no_potential_nature_leaves_abstol_none() {
        // `electrical`'s `flow Current;` is declared, but no `potential` nature — resolving
        // must fail closed (`None`), not panic or fall back to the flow nature's own abstol.
        let src = "nature Current; units = \"A\"; access = I; abstol = 1e-12; endnature \
                   discipline electrical; flow Current; enddiscipline \
                   module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let (asts, natures, disciplines) = parse_with_disciplines(&toks).expect("parse");
        let ast = &asts[0];
        let m = elaborate_with_library_and_disciplines(ast, &asts, &disciplines, &natures)
            .expect("elaborate");
        assert_eq!(m.nodes[0].abstol, None);
    }

    #[test]
    fn module_var_initializer_lowers_to_a_prepended_assign() {
        // `real laser_freq = ...;` (`external/photonic/CwLaser.va`'s idiom) must run before the
        // analog block's own statements, and `amplitude` (no initializer) must not emit
        // anything at all.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             parameter real wavelength = 1550.0; \
             real laser_freq = 3.0e8 / wavelength; \
             real amplitude; \
             analog begin I(a, b) <+ laser_freq; end endmodule",
        );
        assert_eq!(
            m.analog.len(),
            2,
            "one prepended init assign + one contribution"
        );
        let laser_freq = m.vars.iter().position(|v| v.name == "laser_freq").unwrap();
        match &m.analog[0] {
            va_ir::Stmt::Assign { lhs, .. } => assert_eq!(lhs.0 as usize, laser_freq),
            other => panic!("expected the prepended initializer assign, got {other:?}"),
        }
        assert!(matches!(m.analog[1], va_ir::Stmt::Contribute { .. }));
    }

    #[test]
    fn block_local_var_initializer_lowers_to_an_assign() {
        // `real x = 1.0;` inside the analog block itself, not at module scope.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             analog begin real x = 1.0; I(a, b) <+ x; end endmodule",
        );
        // The top-level `analog begin...end` block is flattened (§ lower_analog), so the
        // `VarDecl`'s own lowering (a `Block` wrapping its initializer assigns) and the
        // `Contribute` are two separate top-level entries.
        assert_eq!(m.analog.len(), 2);
        match &m.analog[0] {
            va_ir::Stmt::Block(body) => {
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], va_ir::Stmt::Assign { .. }));
            }
            other => panic!("expected a block wrapping the initializer assign, got {other:?}"),
        }
        assert!(matches!(m.analog[1], va_ir::Stmt::Contribute { .. }));
    }

    #[test]
    fn laplace_nd_folds_to_its_dc_gain() {
        // `laplace_nd(V(a,b), {2, 0}, {2, 1})` (num[0]/den[0] = 2/2 = 1.0) — the exact
        // `external/photonic/PhotoDetector.va`/`TunableFilter.va` shape (a normalized-gain
        // low-pass filter), reduced to a `Binary(Mul, probe, Const(1.0))`.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             analog I(a, b) <+ laplace_nd(V(a, b), {2, 0}, {2, 1}); endmodule",
        );
        match &m.analog[0] {
            va_ir::Stmt::Contribute { value, .. } => match m.expr(*value) {
                Expr::Binary(va_ir::BinOp::Mul, l, r) => {
                    assert!(matches!(m.expr(*l), Expr::Probe(_)));
                    assert!(matches!(m.expr(*r), Expr::Const(g) if (*g - 1.0).abs() < 1e-12));
                }
                other => panic!("expected a Mul-by-gain fold, got {other:?}"),
            },
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn laplace_nd_zero_dc_denominator_is_an_error() {
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ laplace_nd(V(a, b), {1}, {0, 1}); endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
    }

    #[test]
    fn array_literal_outside_laplace_nd_is_an_error() {
        let src = "module t(); parameter real x = {1, 2}; endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
    }

    /// Elaborate a one-contribution module (`analog I(a, b) <+ <call>;`) and return the
    /// filter-fold's baked-in gain constant — the shared assertion helper for every
    /// `laplace_*`/`zi_*` DC/steady-state-gain test below.
    fn filter_gain(call: &str) -> f64 {
        let m = elaborate_src(&format!(
            "module t(a, b); electrical a, b; analog I(a, b) <+ {call}; endmodule"
        ));
        match &m.analog[0] {
            va_ir::Stmt::Contribute { value, .. } => match m.expr(*value) {
                Expr::Binary(va_ir::BinOp::Mul, l, r) => {
                    assert!(matches!(m.expr(*l), Expr::Probe(_)));
                    match m.expr(*r) {
                        Expr::Const(g) => *g,
                        other => panic!("expected a Const gain, got {other:?}"),
                    }
                }
                other => panic!("expected a Mul-by-gain fold, got {other:?}"),
            },
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn laplace_zp_matches_the_lrm_worked_example() {
        // LRM §4.5.11.5's own example: `laplace_zp(V(in), '{-1,0}, '{-1,-1,-1,1})` implements
        // H(s) = (1+s) / [(1+s/(1+j))(1+s/(1-j))]; H(0) = 1/(1*1) = 1 (a real zero at -1, a
        // complex-conjugate pole pair at -1±j — neither at the origin, so every root's factor
        // is 1 at s=0 regardless of it being real or complex).
        let g = filter_gain("laplace_zp(V(a, b), {-1, 0}, {-1, -1, -1, 1})");
        assert!((g - 1.0).abs() < 1e-12, "got {g}");
    }

    #[test]
    fn laplace_np_matches_the_real_corpus_shape() {
        // `external/angelov.va`/`angelov_gan.va`'s exact idiom: `laplace_np(noise, {1},
        // {2*pi*Fgr, 0, -2*pi*Fgr, 0})` — a real numerator constant over two real, nonzero
        // poles; H(0) = num[0] / (1*1) = 1.
        let g = filter_gain("laplace_np(V(a, b), {1}, {6.28e9, 0.0, -6.28e9, 0.0})");
        assert!((g - 1.0).abs() < 1e-12, "got {g}");
    }

    #[test]
    fn laplace_zd_computes_zero_over_denominator_coefficient() {
        // A real, non-origin zero at 1 (numerator fold = 1) over a denominator `{4, 1}`
        // (den[0] = 4): H(0) = 1/4.
        let g = filter_gain("laplace_zd(V(a, b), {1, 0}, {4, 1})");
        assert!((g - 0.25).abs() < 1e-12, "got {g}");
    }

    #[test]
    fn laplace_zero_at_origin_folds_to_zero_gain() {
        // A zero exactly at the origin `(0, 0)` makes that root's factor `s`, which is `0` at
        // s=0 — the whole numerator product collapses to 0, regardless of the (non-origin) pole.
        let g = filter_gain("laplace_zp(V(a, b), {0, 0}, {1, 0})");
        assert_eq!(g, 0.0);
    }

    #[test]
    fn laplace_pole_at_origin_is_an_error() {
        // A pole exactly at the origin makes the denominator's DC value 0 — undefined gain.
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ laplace_np(V(a, b), {1}, {0, 0}); endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
    }

    #[test]
    fn laplace_filter_odd_length_root_array_is_an_error() {
        // Zero/pole arrays must hold `(re, im)` pairs — an odd-length array is malformed.
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ laplace_zp(V(a, b), {1, 0, 2}, {1, 0}); endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
    }

    #[test]
    fn zi_nd_sums_every_coefficient_at_z_equals_one() {
        // Unlike the Laplace s=0 fold (only the constant term survives), every `z^-k` term is
        // 1 at z=1, so the whole coefficient list is summed: num {1,2} -> 3, den {1,1} -> 2.
        let g = filter_gain("zi_nd(V(a, b), {1, 2}, {1, 1}, 1e-9)");
        assert!((g - 1.5).abs() < 1e-12, "got {g}");
    }

    #[test]
    fn zi_zp_real_roots_use_one_minus_root_not_one_minus_s_over_root() {
        // The Z-domain root term at z=1 is `1 - root` (the LRM's `1 - z^-1*root` evaluated at
        // z=1), structurally different from the Laplace fold's `1` for any non-origin root:
        // zero=0.5 -> factor 0.5; pole=0.25 -> factor 0.75; gain = 0.5/0.75.
        let g = filter_gain("zi_zp(V(a, b), {0.5, 0.0}, {0.25, 0.0}, 1e-9)");
        assert!((g - (0.5 / 0.75)).abs() < 1e-9, "got {g}");
    }

    #[test]
    fn zi_zp_complex_conjugate_pair_reduces_to_a_real_gain() {
        // A complex-conjugate zero pair at 0.5 ± 0.3j: product = (0.5 - 0.3j)(0.5 + 0.3j) =
        // 0.5^2 + 0.3^2 = 0.34 (the imaginary parts must cancel exactly). Pole at the origin
        // contributes a factor of 1 (the z=1 special case, not 0 — unlike the Laplace fold).
        let g = filter_gain("zi_zp(V(a, b), {0.5, 0.3, 0.5, -0.3}, {0.0, 0.0}, 1e-9)");
        assert!((g - 0.34).abs() < 1e-9, "got {g}");
    }

    #[test]
    fn zi_filter_zero_dc_denominator_is_an_error() {
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ zi_nd(V(a, b), {1}, {1, -1}, 1e-9); endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
    }

    #[test]
    fn zi_filter_requires_the_sample_period_argument() {
        // `zi_nd` needs at least 4 arguments (value, num, den, T) — only 3 given here.
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ zi_nd(V(a, b), {1}, {1}); endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks).expect("parse").into_iter().next().unwrap();
        assert!(matches!(elaborate(&ast), Err(FrontendError::Elaborate(_))));
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());

        // `$temperature` takes no arguments.
        let src = "module t(a, b); electrical a, b; analog begin I(a, b) <+ $temperature(V(a, b)); end endmodule";
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());

        // Not a bare identifier.
        let src = "module t(a, b); electrical a, b; parameter real vth0 = 0.5; analog begin if ($param_given(vth0 + 1)) I(a, b) <+ V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
    fn block_local_variable_shadows_a_same_named_parameter() {
        // The exact `external/bsimsoi.va` shape: a named block declares a local `real MJSWG;`
        // that shares its name with a module-level parameter (there, macro-declared via
        // `` `MPRoo(MJSWG, ...)` ``) — `MJSWG = ...;` must resolve as an assignment to the new
        // local variable (previously an "assignment to unknown variable" error, since
        // `register_var` saw the same-named parameter and skipped registering it at all), and a
        // later read of `MJSWG` in the same block must read that local variable back, not the
        // outer parameter's constant default (previously silently wrong once the assignment was
        // made to work at all, since `Ident` resolution checked `params` before `vars`).
        let src = "module t(a, b); electrical a, b; \
                    parameter real g = 0.5 from (0:1); \
                    analog begin : load \
                        real g; \
                        g = 0.25; \
                        I(a, b) <+ g; \
                    end endmodule";
        let m = elaborate_src(src);
        assert_eq!(m.params.len(), 1, "the outer parameter `g` still exists");
        assert!(
            m.vars.iter().any(|d| d.name == "g"),
            "a local `g` must also be registered"
        );

        // The contribution's value must read the local variable, not `Expr::Param`.
        let value = m
            .analog
            .iter()
            .find_map(|s| match s {
                va_ir::Stmt::Contribute { value, .. } => Some(*value),
                _ => None,
            })
            .expect("a contribution");
        assert!(
            matches!(m.expr(value), Expr::Var(_)),
            "expected the contribution to read the local `g`, got {:?}",
            m.expr(value)
        );
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn simparam_folds_to_default_in_a_parameter_default_too() {
        // `external/bsim6.0.va`: `parameter real GMIN = $simparam("gmin", 1.0e-15);` — the same
        // fold `$simparam` gets in the analog block must also work in a parameter's own default
        // expression, which is evaluated by the separate, non-mutating `const_eval`.
        let m = elaborate_src(
            r#"module t(a, b); parameter real GMIN = $simparam("gmin", 1.0e-15); electrical a, b; analog begin I(a, b) <+ GMIN * V(a, b); end endmodule"#,
        );
        assert!(m
            .params
            .iter()
            .any(|p| p.name == "GMIN" && p.default == 1.0e-15));

        // Without a default, still an error in a parameter context too.
        let src = r#"module t(a, b); parameter real GMIN = $simparam("gmin"); electrical a, b; analog begin I(a, b) <+ GMIN * V(a, b); end endmodule"#;
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
    fn absdelay_folds_to_its_value_argument() {
        // `absdelay(V(a,b), td)` settles to its undelayed input in DC steady state, same
        // treatment as `transition`/`slew` — no `Call` node survives, and `td` is never lowered.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real td = 1n; \
             analog begin I(a, b) <+ absdelay(V(a, b), td); end endmodule",
        );
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Probe(_))));
        assert!(!m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Call(..))));

        // No value argument at all is an error.
        let src =
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ absdelay(); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());

        // Second argument isn't a probe at all.
        let src = "module t(a); electrical a; analog begin I(a) <+ ddx(V(a), 1.0); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());

        // Second argument is a flow probe, not a potential one — not supported (flow probes
        // aren't independent unknowns in this codegen).
        let src = "module t(a, b); electrical a, b; \
                   analog begin I(a,b) <+ ddx(V(a,b), I(a,b)); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn genvar_step_must_reassign_the_same_genvar() {
        let src = "module t(); genvar i; integer j; \
                   analog begin generate for (i = 0; i < 2; j = j + 1) begin end endgenerate end \
                   endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn genvar_loop_bound_must_be_static() {
        let src = "module t(); electrical p; genvar i; \
                   analog begin generate for (i = 0; V(p) > 0; i = i + 1) begin end endgenerate end \
                   endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn vector_index_out_of_range_is_rejected() {
        let src = "module t(); electrical [1:0] bus; analog begin I(bus[5]) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn accessing_vector_net_without_index_is_rejected() {
        let src = "module t(); electrical [1:0] bus; analog begin I(bus) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn indexing_a_non_vector_net_is_rejected() {
        let src = "module t(); electrical p; analog begin I(p[0]) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn array_variable_runtime_index_expands_to_a_select_chain() {
        // § dynamic vector-net/array-variable indexing: `out_val[j]` where `j` is an ordinary
        // *runtime* `integer` (not a genvar or a constant) has no single `VarId` to read at
        // elaboration time, so it expands into a nested `Expr::Select` chain, one arm per
        // declared index of the array, guarded by `j == k` — the real-corpus idiom this closes
        // (`external/verilogaLib-master/adc_16bit_ideal.va`'s bit-serialization loop).
        let src = "module t(a); electrical a; real out_val[0:3]; integer j; \
                   analog begin j = 3; I(a) <+ out_val[j]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        let m = elaborate(&ast).expect("elaborates");
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(value), va_ir::Expr::Select(_, _, _)));
        // Every one of the array's 4 declared elements is read somewhere in the chain (the
        // chain also reads `j` itself, once, as each arm's comparison target — filter by name
        // rather than raw count).
        let read_names: std::collections::HashSet<_> = m
            .exprs
            .iter()
            .filter_map(|e| match e {
                va_ir::Expr::Var(id) => Some(m.vars[id.0 as usize].name.as_str()),
                _ => None,
            })
            .collect();
        for k in 0..4 {
            assert!(
                read_names.contains(format!("out_val[{k}]").as_str()),
                "missing read of out_val[{k}] in {read_names:?}"
            );
        }
    }

    #[test]
    fn array_variable_runtime_write_expands_to_an_if_chain() {
        // Statement-level sibling of the read case: `out_val[j] = v;` with a runtime `j`
        // expands into an if/else-if chain, one `Stmt::Assign` per declared index.
        let m = elaborate_src(
            "module t(a); electrical a; real out_val[0:3]; integer j; \
             analog begin j = 2; out_val[j] = 5.0; I(a) <+ out_val[0]; end endmodule",
        );
        match &m.analog[1] {
            va_ir::Stmt::If { then_, else_, .. } => {
                assert!(matches!(then_.as_slice(), [va_ir::Stmt::Assign { .. }]));
                assert!(matches!(else_.as_slice(), [va_ir::Stmt::If { .. }]));
            }
            other => panic!("expected an if/else-if chain, got {other:?}"),
        }
    }

    #[test]
    fn vector_net_runtime_index_probe_expands_to_a_select_chain() {
        // The other half of the same real-corpus gap: `V(in[i])`/`V(out[j]) <+ ...` with a
        // runtime index (`external/verilogaLib-master/dac_16bit_ideal.va`,
        // `adc_16bit_ideal.va`). A probe read expands into nested `Select`s of `Probe`s; a
        // contribution target expands into an if/else-if chain of `Contribute`s.
        let m = elaborate_src(
            "module t(a, b); electrical a, b, bus[0:1]; integer i; \
             analog begin i = 1; I(a, b) <+ V(bus[i]); V(bus[i]) <+ V(a, b); end endmodule",
        );
        let read_value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(read_value), va_ir::Expr::Select(_, _, _)));
        match &m.analog[2] {
            va_ir::Stmt::If { then_, else_, .. } => {
                assert!(matches!(then_.as_slice(), [va_ir::Stmt::Contribute { .. }]));
                assert!(matches!(else_.as_slice(), [va_ir::Stmt::Contribute { .. }]));
            }
            other => panic!("expected an if/else-if chain, got {other:?}"),
        }
    }

    // --- § port-current probe (LRM §5.4.3, `I(<port>)`) ----------------------------------

    #[test]
    fn port_probe_sums_unconditional_contributions_with_correct_sign() {
        // The LRM's own diode worked example (§5.4.3): two branches both terminating at `a`
        // (`a` is the `p` terminal of both `i_diode` and `junc_cap`) each contribute positive
        // current, so `I(<a>)` should be their sum.
        let m = elaborate_src(
            "module diode(a, c); inout a, c; electrical a, c; \
             branch (a, c) i_diode, junc_cap; \
             parameter real is = 1e-14; \
             analog begin \
               I(i_diode) <+ is; \
               I(junc_cap) <+ 2.0 * is; \
               I(a) <+ I(<a>); \
             end endmodule",
        );
        let value = match &m.analog[2] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        // `I(<a>)` sums both branches' contributed values (`is` and `2*is`), both `+` since
        // `a` is the `p` terminal of both branches — expect a `Binary(Add, ...)` chain, not a
        // bare probe/const.
        assert!(matches!(
            m.expr(value),
            va_ir::Expr::Binary(va_ir::BinOp::Add, _, _)
        ));
    }

    #[test]
    fn port_probe_negates_contributions_where_the_port_is_the_n_terminal() {
        // `branch (c, a)`: `a` is now the `n` terminal, so a positive contribution should sum
        // into `I(<a>)` as a *negative* term (current arriving at `a` from inside the module
        // reduces what must be supplied from outside).
        let m = elaborate_src(
            "module leg(a, c); inout a, c; electrical a, c; \
             branch (c, a) i_leg; \
             analog begin \
               I(i_leg) <+ 1.0; \
               I(a) <+ I(<a>); \
             end endmodule",
        );
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(
            m.expr(value),
            va_ir::Expr::Unary(va_ir::UnOp::Neg, _)
        ));
    }

    #[test]
    fn port_probe_with_no_contributions_folds_to_zero() {
        let m = elaborate_src(
            "module t(a); inout a; electrical a; \
             analog begin I(a) <+ I(<a>); end endmodule",
        );
        let value = match &m.analog[0] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(value), va_ir::Expr::Const(v) if *v == 0.0));
    }

    #[test]
    fn port_probe_inside_if_wraps_in_a_select_guarded_by_the_condition() {
        // The HICUM-style pattern: a series-resistance branch only contributes when a
        // parameter clears a threshold — `I(<port>)` must only count it when the guard held.
        let m = elaborate_src(
            "module t(a, bi); inout a, bi; electrical a, bi; \
             branch (a, bi) rbx; parameter real r = 10.0; \
             analog begin \
               if (r >= 1.0) begin \
                 I(rbx) <+ V(rbx) / r; \
               end \
               I(a) <+ I(<a>); \
             end endmodule",
        );
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(value), va_ir::Expr::Select(_, _, _)));
    }

    #[test]
    fn v_of_port_probe_is_rejected() {
        let src = "module t(a); inout a; electrical a; \
                   analog begin I(a) <+ V(<a>); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn port_probe_naming_a_non_port_is_rejected() {
        let src = "module t(a); inout a; electrical a, internal; \
                   analog begin I(a) <+ I(<internal>); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn port_probe_as_a_contribution_target_is_a_parse_error() {
        let src = "module t(a); inout a; electrical a; analog begin I(<a>) <+ 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        assert!(parse(&toks).is_err());
    }

    #[test]
    fn port_probe_of_a_flow_contribution_inside_a_case_arm_is_rejected() {
        let src = "module t(a); inout a; electrical a, x; branch (a, x) br; \
                   parameter real sel = 0.0; \
                   analog begin \
                     case (sel) \
                       0: I(br) <+ 1.0; \
                       default: I(br) <+ 0.0; \
                     endcase \
                     I(a) <+ I(<a>); \
                   end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("case"),
                "expected a case-specific message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
        }
    }

    // --- § 2-D array variables / § 2-D vector nets --------------------------------------

    #[test]
    fn two_d_array_variable_write_and_read_with_constant_indices() {
        // `tile[0][1] = 5.0; I(a,b) <+ tile[0][1];` — literal (compile-time-constant) indices
        // resolve to the same `VarId` on both the write and the read, named "tile[0][1]".
        let m = elaborate_src(
            "module t(a, b); electrical a, b; real tile[0:1][0:1]; \
             analog begin tile[0][1] = 5.0; I(a, b) <+ tile[0][1]; end endmodule",
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
        assert_eq!(m.vars[write_id.0 as usize].name, "tile[0][1]");
    }

    #[test]
    fn two_d_array_variable_out_of_range_index_is_rejected() {
        let src = "module t(); real tile[0:1][0:1]; \
                   analog begin tile[2][0] = 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn two_d_array_variable_partial_index_dimension_mismatch_is_rejected() {
        // `tile` is declared with 2 dimensions; indexing it with only 1 must be rejected
        // rather than silently resolving to some other element.
        let src = "module t(); real tile[0:1][0:1]; analog begin tile[0] = 1.0; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("dimension"),
                "expected a dimension-count message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
        }
    }

    #[test]
    fn two_d_array_variable_runtime_index_in_one_dim_expands_to_a_select_chain() {
        // `tile[0][j]` with a runtime `j` and a constant first dimension: only the dynamic
        // dimension unrolls into a `Select` chain (not an O(range²) chain over both).
        let src = "module t(a); electrical a; real tile[0:0][0:3]; integer j; \
                   analog begin j = 2; I(a) <+ tile[0][j]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        let m = elaborate(&ast).expect("elaborates");
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(value), va_ir::Expr::Select(_, _, _)));
        let read_names: std::collections::HashSet<_> = m
            .exprs
            .iter()
            .filter_map(|e| match e {
                va_ir::Expr::Var(id) => Some(m.vars[id.0 as usize].name.as_str()),
                _ => None,
            })
            .collect();
        for k in 0..4 {
            assert!(
                read_names.contains(format!("tile[0][{k}]").as_str()),
                "missing read of tile[0][{k}] in {read_names:?}"
            );
        }
    }

    #[test]
    fn two_d_array_variable_both_dims_dynamic_is_rejected() {
        // Both index positions of the same 2-D access being simultaneously dynamic is
        // rejected rather than expanded into an O(range²) chain (mirrors this file's existing
        // precedent of rejecting two dynamically-indexed *terminals* in one access — see
        // `dynamic_index_pos`'s doc comment).
        let src = "module t(a); electrical a; real tile[0:1][0:1]; integer i, j; \
                   analog begin i = 0; j = 1; I(a) <+ tile[i][j]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn two_d_vector_net_declaration_and_indexed_probe_elaborates() {
        // `electrical [0:1][0:1] grid;` (§ 2-D vector net, a documented non-standard
        // extension) interns 4 nodes, named "grid[i][j]"; a fully 2-indexed probe resolves.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; electrical [0:1][0:1] grid; \
             analog begin I(a, b) <+ V(grid[0][1]); end endmodule",
        );
        assert!(m.nodes.iter().any(|n| n.name == "grid[0][0]"));
        assert!(m.nodes.iter().any(|n| n.name == "grid[1][1]"));
        assert_eq!(
            m.nodes
                .iter()
                .filter(|n| n.name.starts_with("grid["))
                .count(),
            4
        );
    }

    #[test]
    fn two_d_vector_net_bare_or_partial_index_is_rejected() {
        for src in [
            "module t(); electrical [0:1][0:1] grid; analog begin I(grid) <+ 1.0; end endmodule",
            "module t(); electrical [0:1][0:1] grid; analog begin I(grid[0]) <+ 1.0; end endmodule",
        ] {
            let toks = lex(src).expect("lex");
            let ast = parse(&toks)
                .expect("parse")
                .into_iter()
                .next()
                .expect("at least one module");
            assert!(elaborate(&ast).is_err(), "expected rejection for: {src}");
        }
    }

    #[test]
    fn two_d_vector_net_cannot_be_used_as_a_port() {
        let src = "module t(grid); electrical [0:1][0:1] grid; analog begin end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        match elaborate(&ast) {
            Err(FrontendError::Elaborate(msg)) => assert!(
                msg.contains("2-D"),
                "expected a 2-D-vector-net-specific message, got: {msg}"
            ),
            other => panic!("expected an elaboration error, got {other:?}"),
        }
    }

    #[test]
    fn two_d_vector_net_runtime_index_in_one_dim_expands_to_a_select_chain() {
        let src = "module t(a, b); electrical a, b; electrical [0:0][0:1] grid; integer i; \
                   analog begin i = 1; I(a, b) <+ V(grid[0][i]); end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        let m = elaborate(&ast).expect("elaborates");
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(matches!(m.expr(value), va_ir::Expr::Select(_, _, _)));
    }

    #[test]
    fn two_d_vector_net_slice_is_rejected() {
        for src in [
            // A bare slice on a declared-2-D vector net.
            "module t(a, b); electrical a, b; electrical [0:1][0:1] grid; \
             analog begin I(a, b) <+ V(grid[0:1]); end endmodule",
            // An index combined with a trailing slice.
            "module t(a, b); electrical a, b; electrical [0:1][0:1] grid; \
             analog begin I(a, b) <+ V(grid[0][0:1]); end endmodule",
        ] {
            let toks = lex(src).expect("lex");
            let ast = parse(&toks)
                .expect("parse")
                .into_iter()
                .next()
                .expect("at least one module");
            assert!(elaborate(&ast).is_err(), "expected rejection for: {src}");
        }
    }

    #[test]
    fn two_d_reticule_nested_generate_for_addresses_every_tile() {
        // The motivating "2-D reticule" case: two nested genvar-driven generate loops build a
        // 2x2 grid of tile values, each addressed by `tile[i][j]`.
        let m = elaborate_src(
            "module reticule(a); electrical a; real tile[0:1][0:1]; genvar i, j; \
             analog begin \
               for (i = 0; i < 2; i = i + 1) begin \
                 for (j = 0; j < 2; j = j + 1) begin \
                   tile[i][j] = i + j; \
                 end \
               end \
               I(a) <+ tile[0][0] + tile[0][1] + tile[1][0] + tile[1][1]; \
             end endmodule",
        );
        // Both generate loops are fully unrolled: the outer Block holds one inner Block per
        // `i`, each inner Block holding one Assign per `j`.
        match &m.analog[0] {
            va_ir::Stmt::Block(outer) => {
                assert_eq!(outer.len(), 2);
                for inner in outer {
                    match inner {
                        va_ir::Stmt::Block(inner) => {
                            assert_eq!(inner.len(), 2);
                            assert!(inner
                                .iter()
                                .all(|s| matches!(s, va_ir::Stmt::Assign { .. })));
                        }
                        other => panic!("expected an inner unrolled block, got {other:?}"),
                    }
                }
            }
            other => panic!("expected the outer unrolled block, got {other:?}"),
        }
        for (i, j) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            assert!(
                m.vars.iter().any(|v| v.name == format!("tile[{i}][{j}]")),
                "missing tile[{i}][{j}]"
            );
        }
    }

    #[test]
    fn block_local_array_variable_is_rejected() {
        // Array variables must be declared at module scope (§ array variables); a block-local
        // one has nowhere sound to register into and is rejected with a specific message.
        let src = "module t(); analog begin real out_val[0:15]; end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn port_without_discipline_is_rejected() {
        let src = "module t(p); inout p; analog begin end endmodule";
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
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

    // --- module instantiation (§ module instantiation) --------------------------------

    const LEG: &str = "module leg(p, n); electrical p, n; parameter real r = 1000; \
                        analog I(p, n) <+ V(p, n) / r; endmodule ";

    #[test]
    fn instance_flattens_ports_by_position() {
        let src = format!("{LEG} module top(a, b); electrical a, b; leg l1(a, b); endmodule");
        let m = elaborate_top(&src, "top");
        // `leg`'s two ports both alias an already-declared parent node, so no new node is
        // created for the instance — `top` still has exactly its own two declared nodes.
        assert_eq!(m.nodes.len(), 2);
        assert_eq!(m.branches.len(), 1, "leg's (p, n) branch unifies to (a, b)");
        assert!(
            m.params.is_empty(),
            "leg's parameter is baked into a constant, not copied into top's params"
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, Expr::Const(v) if *v == 1000.0)));
    }

    #[test]
    fn instance_shares_a_parent_declared_internal_node() {
        let src = format!(
            "{LEG} module divider(a, b, mid); electrical a, b, mid; \
             leg l1(a, mid); leg l2(mid, b); endmodule"
        );
        let m = elaborate_top(&src, "divider");
        // `mid` is declared directly by `divider`, so both instances alias it rather than
        // each getting their own copy — three nodes total (a, b, mid), two branches.
        assert_eq!(m.nodes.len(), 3);
        assert_eq!(m.branches.len(), 2);
    }

    #[test]
    fn instance_param_override_bakes_in_constant() {
        let src =
            format!("{LEG} module top(a, b); electrical a, b; leg #(.r(2000)) l1(a, b); endmodule");
        let m = elaborate_top(&src, "top");
        assert!(
            m.exprs
                .iter()
                .any(|e| matches!(e, Expr::Const(v) if *v == 2000.0)),
            "override value must be baked in"
        );
        assert!(
            !m.exprs
                .iter()
                .any(|e| matches!(e, Expr::Const(v) if *v == 1000.0)),
            "the un-overridden default must not leak through"
        );
    }

    #[test]
    fn named_connections_are_order_independent() {
        let src =
            format!("{LEG} module top(a, b); electrical a, b; leg l1(.n(b), .p(a)); endmodule");
        let m = elaborate_top(&src, "top");
        assert_eq!(m.nodes.len(), 2);
        assert_eq!(m.branches.len(), 1);
    }

    #[test]
    fn vector_port_instance_connects_whole_bus_and_a_slice() {
        // The exact `external/photonic/Attenuator.va`/`CartesianMultiplier.va` shape: a 2-wide
        // vector port connected once to a same-width bare vector net (`transfer`, no index) and
        // once to a `[msb:lsb]` slice of a wider one (`in[0:1]` out of a 4-wide `in`).
        const MUL: &str = "module mul(x, y); electrical [0:1] x, y; \
                            analog begin V(x[0]) <+ V(y[0]); V(x[1]) <+ V(y[1]); end endmodule ";
        let src = format!(
            "{MUL} module top(); electrical [0:1] transfer; electrical [0:3] in; \
             mul m1(transfer, in[0:1]); endmodule"
        );
        let m = elaborate_top(&src, "top");
        // `transfer[0..1]` and `in[0..1]` are both already-declared parent nodes — the instance
        // must alias `mul`'s vector ports `x`/`y` to them element-wise, not synthesize new
        // `m1.x[..]`/`m1.y[..]` nodes (the 7th node is `m1.gnd`, `mul`'s own implicit reference
        // node for its single-terminal `V(x[0])`-style probes — unrelated to port binding).
        assert_eq!(
            m.nodes.len(),
            7,
            "transfer[0..1] + in[0..3] + m1's own implicit gnd"
        );
        assert!(m
            .nodes
            .iter()
            .all(|n| !n.name.starts_with("m1.x") && !n.name.starts_with("m1.y")));
    }

    #[test]
    fn vector_port_instance_width_mismatch_is_an_error() {
        const MUL: &str = "module mul(x, y); electrical [0:1] x, y; analog begin end endmodule ";
        let src = format!(
            "{MUL} module top(); electrical [0:2] a; electrical [0:1] b; \
             mul m1(a, b); endmodule"
        );
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").unwrap();
        let err = elaborate_with_library(top, &asts).unwrap_err();
        assert!(matches!(err, FrontendError::Elaborate(_)));
    }

    #[test]
    fn unknown_module_reference_errors() {
        let src = "module top(a, b); electrical a, b; nope n1(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let asts = parse(&toks).expect("parse");
        assert!(elaborate_with_library(&asts[0], &asts).is_err());
    }

    #[test]
    fn self_instantiation_is_a_cycle_error() {
        let src = "module top(a, b); electrical a, b; top t1(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let asts = parse(&toks).expect("parse");
        assert!(elaborate_with_library(&asts[0], &asts).is_err());
    }

    #[test]
    fn transitive_instantiation_cycle_errors() {
        let src = "module a_mod(p, n); electrical p, n; b_mod b1(p, n); endmodule \
                   module b_mod(p, n); electrical p, n; a_mod a1(p, n); endmodule";
        let toks = lex(src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let a = asts
            .iter()
            .find(|m| m.name == "a_mod")
            .expect("a_mod present");
        assert!(elaborate_with_library(a, &asts).is_err());
    }

    #[test]
    fn mismatched_port_count_errors() {
        let src = format!("{LEG} module top(a); electrical a; leg l1(a); endmodule");
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").expect("top present");
        assert!(elaborate_with_library(top, &asts).is_err());
    }

    #[test]
    fn unknown_named_port_errors() {
        let src =
            format!("{LEG} module top(a, b); electrical a, b; leg l1(.p(a), .bogus(b)); endmodule");
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").expect("top present");
        assert!(elaborate_with_library(top, &asts).is_err());
    }

    #[test]
    fn mixed_positional_and_named_connections_errors() {
        let src = format!("{LEG} module top(a, b); electrical a, b; leg l1(a, .n(b)); endmodule");
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").expect("top present");
        assert!(elaborate_with_library(top, &asts).is_err());
    }

    #[test]
    fn unknown_param_override_errors() {
        let src = format!(
            "{LEG} module top(a, b); electrical a, b; leg #(.bogus(1.0)) l1(a, b); endmodule"
        );
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").expect("top present");
        assert!(elaborate_with_library(top, &asts).is_err());
    }

    #[test]
    fn duplicate_instance_name_errors() {
        let src = format!(
            "{LEG} module top(a, b); electrical a, b; leg l1(a, b); leg l1(a, b); endmodule"
        );
        let toks = lex(&src).expect("lex");
        let asts = parse(&toks).expect("parse");
        let top = asts.iter().find(|m| m.name == "top").expect("top present");
        assert!(elaborate_with_library(top, &asts).is_err());
    }
}
