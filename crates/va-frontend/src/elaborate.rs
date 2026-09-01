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
//!   single-terminal `V(p)` shorthand) **is** unified with the parent's, as of 2026-09-01.
//!   Verilog-A's reference node is global (LRM §3.6.3), so `V(x)` inside a submodule means
//!   `V(x, ground)` against *the* ground. It used not to be, on the reasoning that each
//!   submodule elaborates in its own arena — which was true of the mechanism but wrong about
//!   the language, and silent: the inlined copy was a separate floating node, so every
//!   `V(out) <+ ...` drove something connected to nothing. Found through the photonic corpus,
//!   whose primitives are written entirely in that style, and none of which could be
//!   simulated as a result.

use std::collections::{HashMap, HashSet};

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
        block_scopes: Vec::new(),
        decl_scopes: Vec::new(),
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
    /// Block-local variable scopes, innermost last — real lexical scoping for declarations
    /// inside `begin ... end` (§ block scoping). Empty while lowering anything that is not
    /// inside an analog block, which is the overwhelmingly common case: a module whose analog
    /// block declares nothing locally never allocates a scope here and resolves exactly as it
    /// did before scoping existed.
    ///
    /// [`Elaborator::lookup_var`] searches these innermost-outward and falls back to
    /// [`Self::vars`], so an inner declaration shadows an outer variable *or* a module
    /// parameter for exactly the extent of its own block — and not one statement further.
    block_scopes: Vec<HashMap<String, VarId>>,
    /// The variable-collecting pre-pass's mirror of [`Self::block_scopes`]: which names are
    /// block-locally declared at each depth. It tracks *names only* — the pre-pass never
    /// allocates a [`VarId`] for a block-local declaration, lowering does — which is what keeps
    /// the two passes from having to agree on an allocation order. Its sole job is to stop
    /// [`Self::register_var`] auto-registering a module-scope variable for an assignment whose
    /// target is really a block-local declaration.
    decl_scopes: Vec<std::collections::HashSet<String>>,
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
        self.collect_ground()?;
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

            // Lower the body with the function scope swapped in, then restore. The block-scope
            // stack is swapped too: a function body's `begin ... end` scopes are its own, and
            // must neither see nor outlive the analog block's.
            let saved = std::mem::replace(&mut self.vars, local);
            let saved_blocks = std::mem::take(&mut self.block_scopes);
            let body = self.lower_stmts(&f.body);
            self.vars = saved;
            self.block_scopes = saved_blocks;
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

    /// Resolve every [`Item::Ground`] (§ ground declaration): each named net must already be
    /// declared (by [`Self::collect_nodes`], run just before this), and becomes the module's
    /// global reference node. Runs before [`Self::resolve_ports`]/[`Self::collect_branches`] so
    /// an implicit single-terminal access later in elaboration (`Self::reference_node`) reuses
    /// whichever node an explicit `ground` statement already named, instead of lazily creating
    /// its own separate `"gnd"`-named node. A second (or later) grounded net in the same module
    /// is aliased into the same [`NodeId`] as the first — every net a `ground` declaration
    /// names is electrically the same global reference node (LRM §3.6.4), so this merges them
    /// rather than leaving them as distinct nodes that happen to both read as zero.
    fn collect_ground(&mut self) -> Result<(), FrontendError> {
        for item in &self.ast.items {
            if let Item::Ground { names } = item {
                for name in names {
                    let id = *self.nodes.get(name).ok_or_else(|| {
                        elab(format!(
                            "`ground {name}`: `{name}` is not a previously declared net"
                        ))
                    })?;
                    match self.ground {
                        Some(gnd) => {
                            self.nodes.insert(name.clone(), gnd);
                        }
                        None => self.ground = Some(id),
                    }
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

    /// Lower every element of a `{...}` array literal into the output arena, preserving them as
    /// expressions rather than const-folding.
    ///
    /// Used by the `laplace_*` family, whose coefficients the corpus routinely writes as
    /// parameter expressions (`` {1, `M_TWO_PI*Fgr} ``). Const-folding them here would freeze a
    /// filter's poles at their declared defaults and silently ignore a netlist override.
    fn array_lit_exprs(&mut self, r: ExprRef, what: &str) -> Result<Vec<ExprId>, FrontendError> {
        let elems = match self.ast.expr(r) {
            ExprAst::ArrayLit(elems) => elems.clone(),
            _ => {
                return Err(elab(format!(
                    "{what} must be a `{{...}}` array-literal coefficient/root list"
                )))
            }
        };
        elems.iter().map(|&e| self.lower_expr(e)).collect()
    }

    /// Read, validate and sort a `noise_table()`/`noise_table_log()` argument into
    /// `(frequency Hz, power)` pairs (LRM §4.6.4.3/§4.6.4.4 — the two impose identical
    /// requirements on the table itself and differ only in how it is interpolated later, so one
    /// reader serves both; `what` names the one the author wrote, for the diagnostics).
    ///
    /// Everything the LRM says about this table is checked here, at the one place that has a
    /// source file to name in the diagnostic:
    ///
    /// - the input must be an array literal — the **file-name form** (`noise_table("f.tbl")`) is
    ///   rejected with its own message rather than mis-parsed, since reading a table off disk at
    ///   elaboration is a genuinely different feature (this crate never opens a file except for
    ///   `` `include ``);
    /// - the flattened list holds `(frequency, power)` **pairs**, so an odd length is an error;
    /// - frequencies must be **unique** ("Each frequency value must be unique") and
    ///   non-negative — a duplicate would make the interpolating segment zero-width and a
    ///   negative frequency has no meaning in a noise sweep;
    /// - powers must be non-negative, since a PSD is a power;
    /// - the pairs are **sorted into ascending frequency** ("the simulator shall internally sort
    ///   the pairs … if required"), which is the invariant `va_abi::noise::table_psd_at` reads
    ///   the table under.
    ///
    /// An empty table is allowed through: it is a source with no power at any frequency, which
    /// the noise analysis then drops. Rejecting it would be a stricter rule than the LRM states.
    fn noise_table_points(&self, r: ExprRef, what: &str) -> Result<Vec<(f64, f64)>, FrontendError> {
        if let ExprAst::Str(_) = self.ast.expr(r) {
            return Err(elab(format!(
                "`{what}` with a file-name argument is not supported — give the table \
                 inline as a `{{f1, p1, f2, p2, …}}` array literal"
            )));
        }
        let values = self.array_lit_values(r, &format!("`{what}`'s table"))?;
        if values.len() % 2 != 0 {
            return Err(elab(format!(
                "`{what}`'s table must hold `(frequency, power)` pairs — got {} values (odd)",
                values.len()
            )));
        }
        let mut points: Vec<(f64, f64)> = Vec::with_capacity(values.len() / 2);
        for pair in values.chunks(2) {
            let (f, p) = (pair[0], pair[1]);
            if f < 0.0 || !f.is_finite() {
                return Err(elab(format!(
                    "`{what}` frequency {f} is not a finite, non-negative frequency in Hz"
                )));
            }
            if p < 0.0 || !p.is_finite() {
                return Err(elab(format!(
                    "`{what}` power {p} at {f} Hz is not a finite, non-negative power \
                     spectral density"
                )));
            }
            points.push((f, p));
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(w) = points.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(elab(format!(
                "`{what}` repeats the frequency {} Hz — the LRM requires each frequency in \
                 a table to be unique",
                w[0].0
            )));
        }
        Ok(points)
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
        // A block-local declaration owns this name here; lowering will allocate its `VarId` in
        // the block scope. Auto-registering a module-scope variable for the same assignment
        // would leave a second, dead binding behind — and, worse, one that a later read outside
        // the block could resolve to.
        if self.decl_scopes.iter().any(|sc| sc.contains(name)) {
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

    /// Record an explicit `real`/`integer` declaration inside the analog block as **block
    /// local** for the pre-pass's purposes: its name, at the current block depth.
    ///
    /// # Real block scoping, and the silent wrong answer it replaces
    ///
    /// Verilog-A says a declaration inside `begin ... end` shadows an outer name **only within
    /// that block**. This elaborator used to have one flat, module-wide `name -> VarId` map with
    /// no push/pop at a `Stmt::Block`, so a block-local declaration leaked over the *entire*
    /// analog block — including statements **before** the block, which no scoping rule could
    /// justify. Given
    ///
    /// ```verilog
    /// parameter real k = 1000.0;
    /// analog begin
    ///   begin : inner  real k;  k = 1.0;  end
    ///   g = 1.0 / k;          // must divide by the parameter, 1000.0
    ///   I(p, n) <+ g * V(p, n);
    /// end
    /// ```
    ///
    /// the read of `k` resolved to the block-local variable and the device silently became a
    /// 1-ohm resistor instead of a 1-kilohm one. `external/bsimsoi.va` is the corpus instance: a
    /// `begin : load` block declares `real ... MJSWG;`, hijacking the read of the `MJSWG`
    /// *parameter* some 2200 lines earlier.
    ///
    /// From 2026-08-29 that collision was **rejected** rather than mis-resolved — the
    /// conservative half of the fix, which could only turn a wrong answer into a diagnostic.
    /// Since 2026-08-30 the scoping is real: [`Self::block_scopes`] gives each `begin ... end`
    /// its own `name -> VarId` map, so the declaration shadows the parameter for exactly its own
    /// block and not one statement further. The rejection is gone because there is nothing left
    /// to reject, and so is the weaker "collision with an outer *variable* silently aliases the
    /// two" limitation that sat beside it — both are ordinary shadowing now.
    ///
    /// **This pass allocates no `VarId`.** Lowering does, when it reaches the declaration with
    /// the matching scope pushed. Keeping allocation in one pass is what lets the two walk the
    /// AST independently without having to agree on an allocation order.
    fn declare_local_var(&mut self, name: &str) -> Result<(), FrontendError> {
        if self.genvars.contains(name) {
            return Ok(());
        }
        if let Some(scope) = self.decl_scopes.last_mut() {
            scope.insert(name.to_string());
        }
        Ok(())
    }

    /// Resolve a variable name against the block scopes (innermost outward), then the
    /// module/function scope. The single place shadowing is decided.
    fn lookup_var(&self, name: &str) -> Option<VarId> {
        self.block_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.vars.get(name).copied())
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
                    self.declare_local_var(&entry.name)?;
                }
                Ok(())
            }
            Stmt::Block(body) => {
                // Popped on the error path too, so a rejected declaration deep in one block
                // cannot leave the scope stack skewed for anything that follows.
                self.decl_scopes.push(std::collections::HashSet::new());
                let mut result = Ok(());
                for s in body {
                    result = self.collect_vars_stmt(s);
                    if result.is_err() {
                        break;
                    }
                }
                self.decl_scopes.pop();
                result
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
                // A `begin ... end` is a scope (§ block scoping): declarations inside it shadow
                // outer names for its extent and vanish at its end. Popped on the error path
                // too, so a failure mid-block cannot leave the stack skewed.
                self.block_scopes.push(HashMap::new());
                let mut out = Vec::with_capacity(body.len());
                let mut result = Ok(());
                for s in body {
                    match self.lower_stmt(s) {
                        Ok(st) => out.push(st),
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                self.block_scopes.pop();
                result.map(|()| va_ir::Stmt::Block(out))
            }
            // A declaration introduces a **fresh** variable bound in the innermost block scope
            // (§ block scoping) — fresh even when an outer variable or a module parameter of the
            // same name exists, which is exactly what shadowing means. An entry with an inline
            // `= expr` initializer additionally emits an assignment in its place, the same
            // DC-only "runs where it's written" approximation `@(initial_step)` already uses
            // (§ event control); entries without one still emit nothing.
            Stmt::VarDecl { names } => {
                for entry in names {
                    if self.genvars.contains(&entry.name) {
                        continue;
                    }
                    let id = VarId(self.out.vars.len() as u32);
                    self.out.vars.push(VarDecl {
                        name: entry.name.clone(),
                    });
                    match self.block_scopes.last_mut() {
                        Some(scope) => {
                            scope.insert(entry.name.clone(), id);
                        }
                        // Unreachable through the analog block (every statement there is inside
                        // at least one `Stmt::Block`), but a declaration with nowhere to bind
                        // must not silently vanish.
                        None => {
                            self.vars.insert(entry.name.clone(), id);
                        }
                    }
                }
                let mut inits = Vec::new();
                for entry in names {
                    if let Some(init) = entry.init {
                        let id = self
                            .lookup_var(&entry.name)
                            .expect("just bound by the loop above");
                        let rhs = self.lower_expr(init)?;
                        inits.push(va_ir::Stmt::Assign { lhs: id, rhs });
                    }
                }
                Ok(va_ir::Stmt::Block(inits))
            }
            // `bound_step(max_step);` parses as a bare call statement like a system task
            // (§ `crate::parser`), but unlike one it has a real effect: it caps the next
            // transient timestep. It used to be discarded with the rest of them; it now lowers
            // to its own IR statement, which the transient integrator honours through
            // `va_abi::StampSink`'s bound-step channel (§ `va_ir::Stmt::BoundStep`).
            Stmt::Task { name, args } if name == "bound_step" => {
                let arg = *args.first().ok_or_else(|| {
                    elab("`bound_step` requires a maximum-timestep argument".to_string())
                })?;
                if args.len() > 1 {
                    return Err(elab(
                        "`bound_step` takes exactly one argument (the maximum timestep in \
                         seconds)"
                            .to_string(),
                    ));
                }
                Ok(va_ir::Stmt::BoundStep(self.lower_expr(arg)?))
            }
            // Other system tasks (`$strobe`, `$finish`, …) have no effect on a solve.
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
                    let id = self
                        .lookup_var(lhs)
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
                } else if let Some(v) = self.lookup_var(name) {
                    Expr::Var(v)
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
            // `$abstime` is the absolute simulation time. It used to fold to a constant `0.0`
            // here, correctly, back when DC was the only analysis there was; once `va-transient`
            // landed that fold froze every time-dependent model at t=0. It now survives to the
            // IR and reads the time from the analysis context at load (§ `va_ir::Builtin::
            // Abstime`) — which still yields exactly `0.0` in a static solve, but as an answer
            // rather than an assumption.
            ExprAst::SysFunc { name, args } if name == "abstime" => {
                if !args.is_empty() {
                    return Err(elab("`$abstime` takes no arguments".to_string()));
                }
                Expr::Call(Builtin::Abstime, Vec::new())
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
            // `$rdist_uniform`/`$rdist_normal`/`$rdist_exponential`/`$rdist_poisson`/
            // `$rdist_chi_square`/`$rdist_t`/`$rdist_erlang` (LRM §9.13.2) generate repeatable
            // pseudo-random values from a named distribution, seeded by their first argument.
            // v0 has no time axis and no simulator random-number generator to drive one with —
            // the same "no meaningful DC value" gap `white_noise`/`flicker_noise`/`noise_table`
            // already have (`docs/roadmap.md`'s language-coverage backlog) — so see
            // `Self::fold_rdist` for the fold, which uses each distribution's own mean rather
            // than an arbitrary constant.
            ExprAst::SysFunc { name, args }
                if matches!(
                    name.as_str(),
                    "rdist_uniform"
                        | "rdist_normal"
                        | "rdist_exponential"
                        | "rdist_poisson"
                        | "rdist_chi_square"
                        | "rdist_t"
                        | "rdist_erlang"
                ) =>
            {
                return self.fold_rdist(name, args);
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
            // `analysis("name", …)` queries which analysis is running. It used to fold to a
            // constant here — `true` for the DC phases, `false` for everything else — which was
            // right when DC was the only analysis and became a silent wrong answer the moment
            // transient and AC existed: a DC-init branch fired at every timepoint and a
            // `analysis("tran")` branch never fired at all. The phase *names* are still resolved
            // here, because the LRM requires them to be string literals and this is the only
            // place that can name a source file when one is misspelled; what survives is the
            // resulting bitmask, answered against the real analysis at load time.
            ExprAst::Call { name, args } if name == "analysis" => {
                let mask = self.phase_mask_of(args, "analysis")?;
                let mask = self.out.push_expr(Expr::Const(f64::from(mask)));
                Expr::Call(Builtin::Analysis, vec![mask])
            }
            // `white_noise(pwr[, "name"])` / `flicker_noise(pwr, exp[, "name"])` (LRM §4.5.13)
            // lower to real IR calls rather than folding away (T5.2's compiled-model noise): the
            // *value* of a noise function is zero in every analysis except noise, but that zero
            // is `va-codegen`'s to produce when it evaluates the resistive channel — the
            // arguments have to survive elaboration for the noise channel to have anything to
            // read. Their optional trailing string label is dropped here (this project reports a
            // summed spectrum, not a per-source breakdown), which also keeps every `Expr::Call`
            // argument a real number rather than a string.
            ExprAst::Call { name, args } if name == "white_noise" => {
                let pwr = *args
                    .first()
                    .ok_or_else(|| elab("`white_noise` requires a power argument".to_string()))?;
                let pwr = self.lower_expr(pwr)?;
                Expr::Call(Builtin::WhiteNoise, vec![pwr])
            }
            ExprAst::Call { name, args } if name == "flicker_noise" => {
                let pwr = *args
                    .first()
                    .ok_or_else(|| elab("`flicker_noise` requires a power argument".to_string()))?;
                let exp = *args.get(1).ok_or_else(|| {
                    elab("`flicker_noise` requires a frequency-exponent argument".to_string())
                })?;
                let pwr = self.lower_expr(pwr)?;
                let exp = self.lower_expr(exp)?;
                Expr::Call(Builtin::FlickerNoise, vec![pwr, exp])
            }
            // `noise_table(input[, "name"])` (LRM §4.6.4.3) and `noise_table_log` (§4.6.4.4)
            // lower like the two noise builtins above, with one difference: their `input` is
            // *data*, not an expression to evaluate per bias. The LRM's table is constant by
            // construction (an array parameter or an array assignment pattern), so it is
            // const-folded, validated, and sorted here — once — and travels as a flat,
            // alternating `f, p, f, p, …` argument list (`Builtin::NoiseTable`'s own doc
            // comment explains why that rather than a new `Expr` variant). The two differ only
            // in which interpolation rule the analysis applies between the points, which is
            // carried by the builtin they lower to and nothing else.
            ExprAst::Call { name, args }
                if matches!(name.as_str(), "noise_table" | "noise_table_log") =>
            {
                let input = *args
                    .first()
                    .ok_or_else(|| elab(format!("`{name}` requires a table argument")))?;
                let points = self.noise_table_points(input, name)?;
                let ids = points
                    .into_iter()
                    .flat_map(|(f, p)| [f, p])
                    .map(|v| self.out.push_expr(Expr::Const(v)))
                    .collect();
                let builtin = if name == "noise_table_log" {
                    Builtin::NoiseTableLog
                } else {
                    Builtin::NoiseTable
                };
                Expr::Call(builtin, ids)
            }
            // `ac_stim([analysis_name [, mag [, phase]]])` (LRM §4.5.2) is a small-signal
            // stimulus active only during the named analysis. It used to fold to zero, which is
            // the right *value* in every analysis — but it is right for the wrong reason: an
            // `ac_stim` is a right-hand-side excitation, and folding it away discarded the
            // magnitude and phase that are the whole point, leaving an AC-driven behavioral
            // model silently unexcited.
            //
            // Normalized here to exactly three arguments — a phase bitmask, `mag`, `phase` — so
            // every consumer reads one shape regardless of how many the source wrote. The LRM's
            // defaults are `"ac"`, `1.0` and `0.0`.
            ExprAst::Call { name, args } if name == "ac_stim" => {
                let (phases, rest): (&[ExprRef], &[ExprRef]) = match args.split_first() {
                    Some((first, rest)) if matches!(self.ast.expr(*first), ExprAst::Str(_)) => {
                        (std::slice::from_ref(first), rest)
                    }
                    // No leading string: every argument is numeric and the analysis defaults to
                    // `"ac"`. `ac_stim(1.0, 0.0)` is by far the common spelling.
                    _ => (&[], args),
                };
                let mask = if phases.is_empty() {
                    va_ir::phase_bit("ac").expect("\"ac\" is a listed phase name")
                } else {
                    self.phase_mask_of(phases, "ac_stim")?
                };
                let mask = self.out.push_expr(Expr::Const(f64::from(mask)));
                let mag = match rest.first() {
                    Some(&e) => self.lower_expr(e)?,
                    None => self.out.push_expr(Expr::Const(1.0)),
                };
                let phase = match rest.get(1) {
                    Some(&e) => self.lower_expr(e)?,
                    None => self.out.push_expr(Expr::Const(0.0)),
                };
                Expr::Call(Builtin::AcStim, vec![mask, mag, phase])
            }
            // `bound_step` in *expression* position. The LRM writes it as a statement, and
            // `crate::parser` parses that form into `Stmt::Task`, which the statement lowering
            // turns into `va_ir::Stmt::BoundStep`. Reaching here means it was written where a
            // value is expected, which is not something to invent a value for.
            ExprAst::Call { name, .. } if name == "bound_step" => {
                return Err(elab(
                    "`bound_step` is a statement, not a value: write `bound_step(expr);` on its \
                     own rather than inside an expression"
                        .to_string(),
                ))
            }
            // `transition(value, delay, rise_time, fall_time)` and `slew(value, pos_rate,
            // neg_rate)` both smooth/limit a signal over time — genuinely time-domain
            // (transient) constructs. v0 is DC-only, and both settle to their input value in
            // steady state (there is no rate-of-change or delay history at a fixed operating
            // point), so they fold transparently to `value`; the rest of the arguments are
            // parsed but never evaluated (same treatment as the noise-source builtins above).
            // The synthetic condition `crate::parser` desugars `@(initial_step) stmt` into. It
            // is not user-writable syntax — `initial_step` is an event name, not a function —
            // which is why this arm accepts no arguments and needs no diagnostic for them.
            ExprAst::Call { name, args } if name == "initial_step" && args.is_empty() => {
                Expr::Call(Builtin::InitialStep, Vec::new())
            }
            // `transition(value, delay, rise_time, fall_time)` (LRM §4.5.5) and
            // `slew(value, pos_rate, neg_rate)` (§4.5.6) both smooth a signal over *time*. They
            // used to fold transparently to `value` — correct in a static solve, where both
            // settle to their input, and a silently wrong waveform in transient, where these
            // *are* the dynamics the model was written to express.
            //
            // They now survive to the IR and evaluate against Interface β's state channel
            // (§ `va_ir::Builtin::Transition`). Arity is normalized here, as `ac_stim`'s is, so
            // no consumer re-derives the LRM's defaults: `transition` always carries four
            // arguments (`delay`/`rise`/`fall` defaulting to 0) and `slew` always three, with
            // `neg_rate` defaulting to `pos_rate` — the LRM's own rule that a single stated rate
            // limits both directions symmetrically.
            ExprAst::Call { name, args } if matches!(name.as_str(), "transition" | "slew") => {
                let value = *args
                    .first()
                    .ok_or_else(|| elab(format!("`{name}` requires at least a value argument")))?;
                let value = self.lower_expr(value)?;
                let mut arg_or = |i: usize, default: f64| -> Result<ExprId, FrontendError> {
                    match args.get(i) {
                        Some(&e) => self.lower_expr(e),
                        None => Ok(self.out.push_expr(Expr::Const(default))),
                    }
                };
                if name == "slew" {
                    let pos = arg_or(1, f64::INFINITY)?;
                    // The LRM's default for an omitted `neg_slew_rate` is the negation of the
                    // positive one, i.e. a symmetric limit. Reuse the *same* `ExprId` rather
                    // than a copied constant so a parameterised rate stays one expression.
                    let neg = match args.get(2) {
                        Some(&e) => self.lower_expr(e)?,
                        None => pos,
                    };
                    Expr::Call(Builtin::Slew, vec![value, pos, neg])
                } else {
                    let delay = arg_or(1, 0.0)?;
                    let rise = arg_or(2, 0.0)?;
                    // An omitted `fall_time` equals `rise_time` (LRM §4.5.5) — again the same
                    // `ExprId`, not a duplicate.
                    let fall = match args.get(3) {
                        Some(&e) => self.lower_expr(e)?,
                        None => rise,
                    };
                    Expr::Call(Builtin::Transition, vec![value, delay, rise, fall])
                }
            }
            // `absdelay(value, delay[, max_delay])` (LRM §4.5.9) delays `value` by a fixed
            // time — again genuinely time-domain, and again settles to its undelayed input in
            // DC steady state (no delay history exists at a fixed operating point), so it folds
            // like `transition`/`slew` above: `delay`/`max_delay` are parsed but never evaluated.
            ExprAst::Call { name, args } if name == "absdelay" => {
                // Lowered to `Builtin::Absdelay` rather than folded away (§6 change,
                // 2026-09-01). Folding was correct at DC and silently wrong everywhere else:
                // an optical waveguide's `absdelay(OptE(fwd), length*n_g/c)` *is* its
                // propagation delay, and folding made light cross the guide instantly.
                let value = *args.first().ok_or_else(|| {
                    elab("`absdelay` requires at least a value and a delay argument".to_string())
                })?;
                let delay = *args
                    .get(1)
                    .ok_or_else(|| elab("`absdelay` requires a delay argument".to_string()))?;
                let value = self.lower_expr(value)?;
                let delay = self.lower_expr(delay)?;
                // A third `maxdelay` argument is accepted and dropped: it exists to size a
                // history buffer, which only a time-domain implementation needs (stage 2).
                Expr::Call(Builtin::Absdelay, vec![value, delay])
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
                let builtin = match name.as_str() {
                    "laplace_nd" => Builtin::LaplaceNd,
                    "laplace_np" => Builtin::LaplaceNp,
                    "laplace_zd" => Builtin::LaplaceZd,
                    _ => Builtin::LaplaceZp,
                };
                // Coefficients/roots survive as *lowered expressions*, not const-folded
                // numbers: the corpus writes them as parameter expressions, and a filter whose
                // pole moves with a netlist-overridden parameter is the normal case. The
                // `Const(num_len)` separator is how a flat argument list says where the
                // numerator ends (§ `va_ir::Builtin::LaplaceNd`).
                let num_ids = self.array_lit_exprs(num, "numerator/zero")?;
                let den_ids = self.array_lit_exprs(den, "denominator/pole")?;
                if num_ids.is_empty() || den_ids.is_empty() {
                    return Err(elab(format!(
                        "`{name}` needs at least one numerator/zero and one denominator/pole entry"
                    )));
                }
                // A zero/pole array is flattened `(re, im)` pairs, so an odd count is
                // malformed. `va-codegen` checks this too, but the diagnostic is worth keeping
                // here, where a source file can still be named — the same reason
                // `noise_table`'s own well-formedness checks live at elaboration.
                let numer_is_zeros = matches!(name.as_str(), "laplace_zd" | "laplace_zp");
                let denom_is_poles = matches!(name.as_str(), "laplace_np" | "laplace_zp");
                if (numer_is_zeros && num_ids.len() % 2 != 0)
                    || (denom_is_poles && den_ids.len() % 2 != 0)
                {
                    return Err(elab(format!(
                        "`{name}`'s zero/pole array must hold an even number of values — they are flattened `(real, imaginary)` root pairs"
                    )));
                }
                let value_id = self.lower_expr(value)?;
                let mut ids = Vec::with_capacity(num_ids.len() + den_ids.len() + 2);
                ids.push(value_id);
                ids.push(self.out.push_expr(Expr::Const(num_ids.len() as f64)));
                ids.extend(num_ids);
                ids.extend(den_ids);
                Expr::Call(builtin, ids)
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

    /// Fold a `$rdist_*` call (LRM §9.13.2) to its distribution's mean, per the doc comment on
    /// this function's call site in [`Self::lower_expr`]. Every form takes `seed` first (parsed
    /// but never evaluated — v0 never calls twice, so there is nothing to seed) and an optional
    /// trailing `type_string` last (LRM Table 9-2's `"global"`/`"instance"`; likewise parsed but
    /// never evaluated — a string literal isn't a valid expression to lower at all).
    fn fold_rdist(&mut self, name: &str, args: &[ExprRef]) -> Result<ExprId, FrontendError> {
        let (min_args, mean_idx) = match name {
            "rdist_uniform" => (3, None),   // seed, start, end[, type]
            "rdist_normal" => (3, Some(1)), // seed, mean, standard_deviation[, type]
            "rdist_exponential" | "rdist_poisson" | "rdist_chi_square" | "rdist_t" => (2, Some(1)), // seed, mean/degree_of_freedom[, type]
            "rdist_erlang" => (3, Some(2)), // seed, k_stage, mean[, type]
            _ => unreachable!("guarded by the caller's match arm"),
        };
        if args.len() < min_args || args.len() > min_args + 1 {
            return Err(elab(format!(
                "`${name}` takes {min_args} or {} arguments",
                min_args + 1
            )));
        }
        match name {
            // The uniform distribution's mean is the midpoint of its bounds — no single
            // argument carries it directly, unlike every other form here.
            "rdist_uniform" => {
                let start = self.lower_expr(args[1])?;
                let end = self.lower_expr(args[2])?;
                let sum = self
                    .out
                    .push_expr(Expr::Binary(va_ir::BinOp::Add, start, end));
                let two = self.out.push_expr(Expr::Const(2.0));
                Ok(self
                    .out
                    .push_expr(Expr::Binary(va_ir::BinOp::Div, sum, two)))
            }
            // A Student's t distribution is symmetric about zero (the only case with a
            // well-defined mean, degrees of freedom > 1) — there's no argument to read a center
            // from at all.
            "rdist_t" => Ok(self.out.push_expr(Expr::Const(0.0))),
            _ => self.lower_expr(args[mean_idx.expect("every other rdist_* form names its mean")]),
        }
    }

    /// Whether an `analysis(...)` call is active under v0's DC-only model: true if any
    /// string argument names a DC/operating-point phase. Arguments must be string literals.
    fn phase_mask_of(&self, args: &[ExprRef], who: &str) -> Result<u32, FrontendError> {
        let mut names = Vec::with_capacity(args.len());
        for &a in args {
            match self.ast.expr(a) {
                ExprAst::Str(s) => names.push(s.as_str()),
                _ => {
                    return Err(elab(format!(
                        "`{who}` analysis-name arguments must be string literals"
                    )))
                }
            }
        }
        // An unrecognized phase name is an error, not a mask of zero. Folding it to "matches
        // nothing" would silently disable whatever branch it guards, forever, with no
        // diagnostic — and a misspelling like `"transient"` for `"tran"` is exactly the kind of
        // mistake that produces a plausible-looking but wrong waveform.
        va_ir::phase_mask(names).map_err(|bad| {
            elab(format!(
                "`{who}`: `{bad}` is not a Verilog-A analysis name (expected one of {})",
                va_ir::ANALYSIS_PHASES.join(", ")
            ))
        })
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

        // Which local variables can carry a `ddt` into a contribution's right-hand side. Computed
        // once here, over the analog block lowered so far, and threaded down through the fold so
        // `Self::contains_ddt` can see a `ddt` that arrived through an assignment
        // (§ `Self::ddt_tainted_vars`).
        let tainted = self.ddt_tainted_vars();

        let mut terms = Vec::new();
        self.collect_port_flow_contributions(
            node,
            port,
            &self.out.analog.clone(),
            &[],
            &tainted,
            &mut terms,
        )?;

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

    /// Rebuild `expr` from only those top-level additive terms that contain no `ddt` — directly
    /// or through a `ddt`-tainted variable listed in `tainted` — or `None` if every term did.
    ///
    /// # Why `I(<port>)` drops charge terms
    ///
    /// A port-current probe folds to the sum of the flow contributions already made to branches
    /// touching the port's node. Some of those contributions are charge terms — `HICUM`'s base
    /// node carries `I(br_bci) <+ ddt(qjcx);` alongside its conduction currents. Inlining a
    /// contribution's raw right-hand side therefore *manufactured* a `ddt` nested inside an
    /// ordinary assignment (`IB = I(<b>);`), a shape no model wrote and `va-codegen` rightly
    /// refuses, since `ddt`'s value depends on the whole history of its argument rather than on
    /// the current unknowns. Six real corpus files failed for this reason with a message
    /// blaming a nested `ddt` that does not appear anywhere in their source.
    ///
    /// **This is an approximation, and a documented one**: the probe reports **conduction
    /// current only**, omitting displacement current. It is *exact at a DC operating point*
    /// (where every `ddt` is zero by definition) and an approximation in transient. That is the
    /// same rule [`va_codegen`'s flow-current accumulator] already applies to a branch
    /// self-probe `I(branch)`, so `I(<port>)` is now consistent with it rather than being the
    /// one construct that fails outright. The alternative — evaluating `dQ/dt` — needs the
    /// integrator's per-term time-stepping coefficient exposed through Interface β, which is a
    /// coordinated change, not a fold-local one.
    ///
    /// The real corpus reads are all operating-point outputs inside `` `ifdef CALC_OP `` blocks
    /// (`IB = I(<b>);`), where the conduction current is what is wanted anyway.
    fn resistive_terms_only(&mut self, expr: ExprId, tainted: &HashSet<u32>) -> Option<ExprId> {
        let mut terms = Vec::new();
        self.collect_signed_terms(expr, 1.0, &mut terms);
        let kept: Vec<(f64, ExprId)> = terms
            .into_iter()
            .filter(|&(_, e)| !self.contains_ddt(e, tainted))
            .collect();
        let mut sum = None;
        for (sign, e) in kept {
            let signed = if sign < 0.0 {
                self.out.push_expr(Expr::Unary(va_ir::UnOp::Neg, e))
            } else {
                e
            };
            sum = Some(match sum {
                None => signed,
                Some(acc) => self
                    .out
                    .push_expr(Expr::Binary(va_ir::BinOp::Add, acc, signed)),
            });
        }
        sum
    }

    /// Flatten `expr` into signed additive terms, pushing `-` through subtraction and unary
    /// negation — the same flattening `va_codegen::lower::collect_terms` does, repeated here
    /// because this crate cannot depend on that one (§3's dependency table).
    fn collect_signed_terms(&self, expr: ExprId, sign: f64, out: &mut Vec<(f64, ExprId)>) {
        match self.out.expr(expr) {
            Expr::Binary(va_ir::BinOp::Add, l, r) => {
                let (l, r) = (*l, *r);
                self.collect_signed_terms(l, sign, out);
                self.collect_signed_terms(r, sign, out);
            }
            Expr::Binary(va_ir::BinOp::Sub, l, r) => {
                let (l, r) = (*l, *r);
                self.collect_signed_terms(l, sign, out);
                self.collect_signed_terms(r, -sign, out);
            }
            Expr::Unary(va_ir::UnOp::Neg, e) => {
                let e = *e;
                self.collect_signed_terms(e, -sign, out);
            }
            _ => out.push((sign, expr)),
        }
    }

    /// Whether `expr` reaches a `ddt` — either syntactically, or through a local variable that
    /// [`Self::ddt_tainted_vars`] proved can carry one (`tainted` holds those, by `VarId.0`).
    /// `idt` is deliberately *not* matched: its value is an ordinary read of its accumulator
    /// unknown, evaluable anywhere.
    ///
    /// The variable arm is what makes this scan sound. Without it the scan sees only the shape
    /// written at the `<+` site, so `qd = ddt(cj*V(a,c)); I(a,c) <+ is*V(a,c) + qd;` looks
    /// entirely resistive and the charge term survives into `I(<a>)` — the *direct* spelling
    /// `I(a,c) <+ is*V(a,c) + ddt(cj*V(a,c));` and the through-a-variable spelling of the same
    /// physics would then disagree about what the probe reports.
    fn contains_ddt(&self, expr: ExprId, tainted: &HashSet<u32>) -> bool {
        match self.out.expr(expr) {
            Expr::Call(va_ir::Builtin::Ddt, _) => true,
            Expr::Var(id) => tainted.contains(&id.0),
            Expr::Call(_, args) | Expr::CallUser(_, args) => {
                args.iter().any(|&a| self.contains_ddt(a, tainted))
            }
            Expr::Unary(_, e) | Expr::Ddx(e, _) => self.contains_ddt(*e, tainted),
            Expr::Binary(_, l, r) => {
                self.contains_ddt(*l, tainted) || self.contains_ddt(*r, tainted)
            }
            Expr::Select(c, t, f) => {
                self.contains_ddt(*c, tainted)
                    || self.contains_ddt(*t, tainted)
                    || self.contains_ddt(*f, tainted)
            }
            Expr::Const(_) | Expr::Param(_) | Expr::Probe(_) => false,
        }
    }

    /// The set of local variables (by `VarId.0`) that can carry a `ddt` into an expression: a
    /// variable is tainted if *any* `Stmt::Assign` to it anywhere in the analog block lowered so
    /// far has a right-hand side that syntactically contains a `ddt` or reads an already-tainted
    /// variable. Iterated to a fixed point, so a chain (`q = ddt(x); s = q; t = 2*s;`) taints
    /// every link, not just the first. This mirrors `va_codegen::lower::param_only_vars`, whose
    /// fixed point runs the same way over the same statement set; the logic is repeated here
    /// because this crate cannot depend on that one (§3's dependency table).
    ///
    /// **Deliberately over-approximate, in the safe direction, and not path-sensitive.** Taint is
    /// a property of the variable across the whole block, never of one program point: after
    /// `x = ddt(q); x = 0;` the variable `x` stays tainted even though the second assignment
    /// plainly supersedes the first, and a `ddt` assigned only inside an `if` that never runs
    /// taints the variable just the same. The safe direction is the *over*-approximating one
    /// because being wrong here has asymmetric cost: a falsely-tainted term is dropped from
    /// `I(<port>)`, which understates a conduction current the probe already documents itself as
    /// approximating (see [`Self::resistive_terms_only`]); a falsely-*clean* term smuggles a
    /// charge into a probe documented to report conduction current only, silently contradicting
    /// the direct spelling of the same physics and handing `va-codegen` a `ddt` nested inside an
    /// ordinary assignment. Path-sensitivity would need a real dataflow lattice over the block's
    /// control flow, which buys nothing for the corpus shapes this fold exists to serve.
    fn ddt_tainted_vars(&self) -> HashSet<u32> {
        let mut assigns = Vec::new();
        collect_var_assigns(&self.out.analog, &mut assigns);

        let mut tainted: HashSet<u32> = HashSet::new();
        loop {
            let mut changed = false;
            for &(var, rhs) in &assigns {
                if tainted.contains(&var) {
                    continue;
                }
                if self.contains_ddt(rhs, &tainted) {
                    tainted.insert(var);
                    changed = true;
                }
            }
            if !changed {
                return tainted;
            }
        }
    }

    /// The refusal text for a flow contribution whose enclosing `if` condition `guard` reaches a
    /// `ddt` (§ port-current probe). Names the construct (`I(<port>)`), the cause, and — when the
    /// `ddt` arrived through an assignment rather than being written in the condition itself —
    /// the source-level variable that carries it.
    ///
    /// **Limitation**: [`FrontendError::Elaborate`] carries no source span (no elaboration error
    /// in this crate does), so the message locates the problem by name, not by line.
    fn ddt_guard_message(&self, port: &str, guard: ExprId, tainted: &HashSet<u32>) -> String {
        let via = match self.first_tainted_var(guard, tainted) {
            Some(id) => match self.vars.iter().find(|(_, v)| **v == id) {
                Some((name, _)) => format!("through variable `{name}`"),
                None => "through a local variable".to_string(),
            },
            None => "written in the condition itself".to_string(),
        };
        format!(
            "`I(<{port}>)` can't sum a flow contribution whose enclosing `if` condition depends \
             on `ddt` ({via}). The probe reports conduction current only (§ port-current probe), \
             so a charge term is dropped from a contribution's *value* — but a condition is kept \
             whole or not at all, so keeping this one would carry the `ddt` into the ordinary \
             assignment that reads the probe, which no model wrote and `va-codegen` refuses. \
             Compute the condition from a `ddt`-free expression, or read the branch current \
             directly instead of probing the port."
        )
    }

    /// The first `ddt`-tainted local variable reachable in `expr`, for
    /// [`Self::ddt_guard_message`]. `None` when `expr` reaches a `ddt` only by spelling one out
    /// directly (`if (ddt(q) > 0)`), with no tainted variable involved.
    fn first_tainted_var(&self, expr: ExprId, tainted: &HashSet<u32>) -> Option<VarId> {
        match self.out.expr(expr) {
            Expr::Var(id) if tainted.contains(&id.0) => Some(*id),
            Expr::Call(_, args) | Expr::CallUser(_, args) => args
                .iter()
                .find_map(|&a| self.first_tainted_var(a, tainted)),
            Expr::Unary(_, e) | Expr::Ddx(e, _) => self.first_tainted_var(*e, tainted),
            Expr::Binary(_, l, r) => self
                .first_tainted_var(*l, tainted)
                .or_else(|| self.first_tainted_var(*r, tainted)),
            Expr::Select(c, t, f) => self
                .first_tainted_var(*c, tainted)
                .or_else(|| self.first_tainted_var(*t, tainted))
                .or_else(|| self.first_tainted_var(*f, tainted)),
            Expr::Const(_) | Expr::Param(_) | Expr::Var(_) | Expr::Probe(_) => None,
        }
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
    /// unconditionally earlier in the same block). `tainted` is [`Self::ddt_tainted_vars`]'s
    /// result, passed through to [`Self::resistive_terms_only`]; `port` names the probed port,
    /// for diagnostics only.
    ///
    /// The value and the guards are the *only* two channels through which anything reaches the
    /// folded probe expression, and both are screened for `ddt` — the value by dropping its
    /// charge terms, the guard by refusing outright, since a condition cannot be "partly" kept.
    /// Together they are what actually enforces [`Self::resistive_terms_only`]'s
    /// conduction-current-only claim.
    ///
    /// **Limitation**: a qualifying contribution found inside a `case`/`for`/`while`/`repeat` is
    /// rejected with a clear error rather than silently mis-summed or silently dropped — those
    /// need either a per-arm equality guard (`case`) or genuine loop-carried accumulation
    /// (`for`/`while`/`repeat`), neither of which this fold attempts; no corpus need for either
    /// has surfaced yet (§ port-current probe).
    fn collect_port_flow_contributions(
        &mut self,
        node: NodeId,
        port: &str,
        stmts: &[va_ir::Stmt],
        guards: &[ExprId],
        tainted: &HashSet<u32>,
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
                    // Keep only the contribution's *resistive* terms. A `ddt(...)` term is a
                    // charge, and its time derivative is not a value this pipeline can
                    // evaluate — inlining it here manufactured a `ddt` nested inside an
                    // ordinary assignment that no model actually wrote (see
                    // `Self::resistive_terms_only`).
                    let Some(mut v) = self.resistive_terms_only(*value, tainted) else {
                        // Every term was a charge term: the branch contributes no conduction
                        // current at all, so it adds nothing to the probe under this rule.
                        continue;
                    };
                    for &g in guards.iter().rev() {
                        // A guard is kept whole or not at all — there is no "resistive half" of
                        // a condition — so a condition that reaches a `ddt` would smuggle the
                        // charge back into the probe the term filter just cleaned. Refuse, and
                        // say which construct and which variable, rather than emit an expression
                        // whose eventual rejection names a variable index nobody wrote.
                        if self.contains_ddt(g, tainted) {
                            return Err(elab(self.ddt_guard_message(port, g, tainted)));
                        }
                        let zero = self.out.push_expr(Expr::Const(0.0));
                        v = self.out.push_expr(Expr::Select(g, v, zero));
                    }
                    out.push((sign, v));
                }
                va_ir::Stmt::Contribute { .. } | va_ir::Stmt::Assign { .. } => {}
                va_ir::Stmt::Block(body) => {
                    self.collect_port_flow_contributions(node, port, body, guards, tainted, out)?;
                }
                va_ir::Stmt::If { cond, then_, else_ } => {
                    let mut then_guards = guards.to_vec();
                    then_guards.push(*cond);
                    self.collect_port_flow_contributions(
                        node,
                        port,
                        then_,
                        &then_guards,
                        tainted,
                        out,
                    )?;

                    let not_cond = self.out.push_expr(Expr::Unary(va_ir::UnOp::Not, *cond));
                    let mut else_guards = guards.to_vec();
                    else_guards.push(not_cond);
                    self.collect_port_flow_contributions(
                        node,
                        port,
                        else_,
                        &else_guards,
                        tainted,
                        out,
                    )?;
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
                // `bound_step` makes no contribution to any branch, so it adds no term here.
                va_ir::Stmt::BoundStep(_) => {}
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
            va_ir::Stmt::Contribute { .. }
            | va_ir::Stmt::Assign { .. }
            | va_ir::Stmt::BoundStep(_) => false,
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

        // Verilog-A's reference node is **global** (LRM §3.6.3): a single-terminal access
        // `V(x)` means `V(x, ground)` against *the* ground, not one private to the module it
        // was written in. Each submodule elaborates in its own arena and interns its own
        // implicit `gnd` for that shorthand, so without this the inlined copy would be a
        // separate floating node — and every contribution written `V(out) <+ ...` would drive
        // a node connected to nothing. That is silent: the model builds, and the row is
        // singular or the value simply wrong.
        //
        // Found via the photonic corpus, whose primitives are written entirely in that style
        // (`OptE(cart[0]) <+ ...`), so nothing in that library could be simulated at all.
        if let Some(sub_ground) = Self::ground_node_of(&sub) {
            node_map.entry(sub_ground).or_insert_with(|| {
                // Interning the parent's own reference node, which is idempotent and is the
                // same node any `V(x)` shorthand in the parent already resolves to.
                if let Some(id) = self.ground {
                    id
                } else {
                    let id = self.intern_node("gnd", Discipline::Electrical, None);
                    self.ground = Some(id);
                    id
                }
            });
        }

        self.merge_submodule(inst_name, sub, node_map);
        Ok(())
    }

    /// The module's own implicit reference node, if it interned one.
    ///
    /// Identified by name: [`Elaborator::reference_node`] interns exactly `"gnd"`, and
    /// [`Elaborator::collect_ground`] aliases every explicit `ground` declaration onto that
    /// same node, so the name is the marker rather than a convention this function invents.
    ///
    /// A node that is also a **port** is excluded: a module declaring a port called `gnd` is
    /// wiring it from outside like any other terminal, and stealing it for the global reference
    /// would silently rewire the instance.
    fn ground_node_of(m: &Module) -> Option<NodeId> {
        let is_port = |id: NodeId| m.ports.iter().flatten().any(|&p| p == id);
        m.nodes.iter().enumerate().find_map(|(i, n)| {
            let id = NodeId(i as u32);
            (n.name == "gnd" && !is_port(id)).then_some(id)
        })
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
        va_ir::Stmt::BoundStep(e) => va_ir::Stmt::BoundStep(expr_off[e.0 as usize]),
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

/// Collect every `(VarId.0, rhs)` pair assigned anywhere in an *already-lowered* IR statement
/// list, recursing through every nested construct (including a `for` header's `init`/`step`).
/// The IR-side counterpart of [`collect_assign_targets`], which does the same job on the surface
/// AST; this one feeds [`Elaborator::ddt_tainted_vars`]'s fixed point.
///
/// Control flow is flattened away deliberately: an assignment inside an `if`/`case`/loop is
/// collected exactly like a top-level one, which is what makes the taint set a non-path-sensitive
/// over-approximation (see [`Elaborator::ddt_tainted_vars`] for why that is the safe direction).
fn collect_var_assigns(stmts: &[va_ir::Stmt], out: &mut Vec<(u32, ExprId)>) {
    for stmt in stmts {
        match stmt {
            va_ir::Stmt::Assign { lhs, rhs } => out.push((lhs.0, *rhs)),
            va_ir::Stmt::Block(body) => collect_var_assigns(body, out),
            va_ir::Stmt::If { then_, else_, .. } => {
                collect_var_assigns(then_, out);
                collect_var_assigns(else_, out);
            }
            va_ir::Stmt::While { body, .. } | va_ir::Stmt::Repeat { body, .. } => {
                collect_var_assigns(body, out)
            }
            va_ir::Stmt::For {
                init, step, body, ..
            } => {
                collect_var_assigns(std::slice::from_ref(&**init), out);
                collect_var_assigns(std::slice::from_ref(&**step), out);
                collect_var_assigns(body, out);
            }
            va_ir::Stmt::Case { arms, default, .. } => {
                for arm in arms {
                    collect_var_assigns(&arm.body, out);
                }
                collect_var_assigns(default, out);
            }
            va_ir::Stmt::Contribute { .. } | va_ir::Stmt::BoundStep(_) => {}
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

    /// `models/`, as an include-path root. Every model there `` `include ``s `disciplines.vams`
    /// and `constants.vams` from alongside itself; a test that lexes/parses a model without
    /// resolving those would hit an unexpanded `` `P_K ``/`` `P_Q `` directive rather than the
    /// number the real pipeline sees (`va_cli::load` passes the model file's own directory).
    fn models_dir() -> Vec<std::path::PathBuf> {
        vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))]
    }

    /// Preprocess a `models/*.va` source the way the real pipeline does.
    fn preprocess_model(src: &str) -> String {
        crate::preprocess::preprocess(src, &models_dir()).expect("preprocess model")
    }

    // --- block-scoping test helpers -------------------------------------------------
    //
    // These assert on *resolution* — which `VarId`/`ParamId` a read actually landed on —
    // because the bug block scoping fixes was a wrong answer, not a failure. A test that only
    // checks "it elaborates" cannot see it.

    fn var_id(m: &Module, name: &str) -> VarId {
        VarId(
            m.vars
                .iter()
                .position(|v| v.name == name)
                .unwrap_or_else(|| panic!("no variable named `{name}`")) as u32,
        )
    }

    /// The RHS of the (single) assignment to `id`, searched depth-first through nested blocks.
    fn assign_rhs(m: &Module, id: VarId) -> Option<ExprId> {
        fn walk(stmts: &[va_ir::Stmt], id: VarId) -> Option<ExprId> {
            for st in stmts {
                let found = match st {
                    va_ir::Stmt::Assign { lhs, rhs } if *lhs == id => Some(*rhs),
                    va_ir::Stmt::Block(b) => walk(b, id),
                    va_ir::Stmt::If { then_, else_, .. } => {
                        walk(then_, id).or_else(|| walk(else_, id))
                    }
                    va_ir::Stmt::While { body, .. } | va_ir::Stmt::Repeat { body, .. } => {
                        walk(body, id)
                    }
                    va_ir::Stmt::For { body, .. } => walk(body, id),
                    va_ir::Stmt::Case { arms, default, .. } => arms
                        .iter()
                        .find_map(|a| walk(&a.body, id))
                        .or_else(|| walk(default, id)),
                    _ => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        walk(&m.analog, id)
    }

    /// Every `ExprId` reachable from `root`. The match is **exhaustive on purpose** — a new
    /// `Expr` variant must break this helper loudly rather than let a resolution test quietly
    /// stop looking inside it.
    fn reachable(m: &Module, root: ExprId) -> Vec<ExprId> {
        let mut seen = Vec::new();
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if seen.contains(&e) {
                continue;
            }
            seen.push(e);
            match &m.exprs[e.0 as usize] {
                Expr::Const(_) | Expr::Param(_) | Expr::Var(_) | Expr::Probe(_) => {}
                Expr::Unary(_, a) | Expr::Ddx(a, _) => stack.push(*a),
                Expr::Binary(_, a, b) => {
                    stack.push(*a);
                    stack.push(*b);
                }
                Expr::Select(a, b, c) => {
                    stack.push(*a);
                    stack.push(*b);
                    stack.push(*c);
                }
                Expr::Call(_, args) | Expr::CallUser(_, args) => stack.extend(args.iter().copied()),
            }
        }
        seen
    }

    fn reads_param(m: &Module, root: ExprId, p: ParamId) -> bool {
        reachable(m, root)
            .into_iter()
            .any(|e| matches!(&m.exprs[e.0 as usize], Expr::Param(q) if *q == p))
    }

    fn reads_any_var_named(m: &Module, root: ExprId, name: &str) -> bool {
        reachable(m, root).into_iter().any(
            |e| matches!(&m.exprs[e.0 as usize], Expr::Var(v) if m.vars[v.0 as usize].name == name),
        )
    }

    /// A block-local declaration shadows an outer name **for its own block only** — the case
    /// that used to be a silent wrong answer, and then (2026-08-29) a rejection.
    ///
    /// `self.vars` was one flat module-wide map with no push/pop at a `Stmt::Block`, so the
    /// binding leaked over the whole analog block, statements *before* it included. Here
    /// `g = 1.0 / k` after the block must divide by the **parameter** 1000.0, not the
    /// block-local `k = 1.0`: doing the latter turns a 1-kilohm device into a 1-ohm one with no
    /// diagnostic. `external/bsimsoi.va` is the corpus instance (see
    /// [`Elaborator::declare_local_var`]).
    ///
    /// This asserts the *resolution*, not merely that elaboration succeeds — which is the whole
    /// point, since the bug's signature was succeeding with the wrong answer.
    #[test]
    fn a_block_local_declaration_shadows_only_within_its_block() {
        let m = elaborate_src(
            "module shadow(p, n);
             electrical p, n;
             parameter real k = 1000.0;
             real g;
             analog begin
               begin : inner
                 real k;
                 k = 1.0;
               end
               g = 1.0 / k;
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );

        // Two distinct `k`s exist: the parameter, and the block-local variable.
        let inner_k = m.vars.iter().filter(|v| v.name == "k").count();
        assert_eq!(inner_k, 1, "the block-local `k` is its own variable");
        assert!(
            m.params.iter().any(|p| p.name == "k"),
            "the parameter `k` still exists"
        );

        // `g = 1.0 / k` sits after the block, so its `k` must be the *parameter*.
        let g = var_id(&m, "g");
        let rhs = assign_rhs(&m, g).expect("`g` is assigned at analog-block level");
        let k_param = m.params.iter().position(|p| p.name == "k").unwrap() as u32;
        assert!(
            reads_param(&m, rhs, ParamId(k_param)),
            "the read of `k` after the block must resolve to the parameter (1000.0), not to the              block-local variable — that mis-resolution silently made this a 1-ohm resistor"
        );
        assert!(
            !reads_any_var_named(&m, rhs, "k"),
            "and must not read any variable named `k`"
        );
    }

    /// The same shadowing rule when the `begin ... end` is an **`if`-arm body** rather than a
    /// standalone block — the shape real compact models actually use.
    ///
    /// This did not work when block scoping first landed: `parse_block_or_single` returned a flat
    /// `Vec<Stmt>` and dropped the `begin ... end` boundary, so only a standalone block ever
    /// became a `Stmt::Block`. An arm body therefore had no block node, neither pass pushed a
    /// scope, and the declaration leaked module-wide — the same 1 kΩ-becomes-0.5 Ω silent wrong
    /// answer, one construct over. The parser now preserves the boundary.
    #[test]
    fn a_declaration_in_an_if_arm_body_does_not_leak_past_the_arm() {
        let m = elaborate_src(
            "module armshadow(p, n);
             electrical p, n;
             parameter real k = 1000.0;
             real g;
             analog begin
               if (V(p, n) > 0.0) begin
                 real k;
                 k = 1.0;
               end
               g = 1.0 / k;
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );
        // `g = 1.0 / k` sits after the `if`, so its `k` must be the parameter (1000.0).
        let g = var_id(&m, "g");
        let rhs = assign_rhs(&m, g).expect("`g` is assigned after the arm");
        let k_param = m
            .params
            .iter()
            .position(|p| p.name == "k")
            .expect("parameter `k`") as u32;
        assert!(
            reads_param(&m, rhs, ParamId(k_param)),
            "the read of `k` after the if-arm must resolve to the parameter, not the arm-local              variable — mis-resolving it silently made this a 0.5 ohm device"
        );
        assert!(
            !reads_any_var_named(&m, rhs, "k"),
            "and must not read any variable named `k`"
        );
    }

    /// The same for a loop body, so the fix is not special-cased to `if`.
    #[test]
    fn a_declaration_in_a_loop_body_does_not_leak_past_the_loop() {
        let m = elaborate_src(
            "module loopshadow(p, n);
             electrical p, n;
             parameter real k = 1000.0;
             real g;
             integer c;
             analog begin
               c = 0;
               while (c < 1) begin
                 real k;
                 k = 1.0;
                 c = c + 1;
               end
               g = 1.0 / k;
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );
        let g = var_id(&m, "g");
        let rhs = assign_rhs(&m, g).expect("`g` is assigned after the loop");
        let k_param = m
            .params
            .iter()
            .position(|p| p.name == "k")
            .expect("parameter `k`") as u32;
        assert!(reads_param(&m, rhs, ParamId(k_param)));
        assert!(!reads_any_var_named(&m, rhs, "k"));
    }

    /// The mirror of the test above: *inside* the block, the same name must resolve to the
    /// block-local variable rather than the parameter. Without this, resolving every `k` to the
    /// parameter would pass the test above for the wrong reason — there would be no shadowing
    /// at all, just the old flat map with the arrow pointing the other way.
    #[test]
    fn inside_the_block_the_shadowing_declaration_wins() {
        let m = elaborate_src(
            "module shadow(p, n);
             electrical p, n;
             parameter real k = 1000.0;
             real g;
             analog begin
               begin : inner
                 real k;
                 k = 1.0;
                 g = k;
               end
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );
        let g = var_id(&m, "g");
        let rhs = assign_rhs(&m, g).expect("`g` is assigned inside the block");
        assert!(
            reads_any_var_named(&m, rhs, "k"),
            "inside the block, `k` must resolve to the block-local variable"
        );
    }

    /// Two sibling blocks may each declare the same name; they are different variables, and
    /// neither escapes. This is the property a flat map cannot express at all.
    #[test]
    fn sibling_blocks_declaring_the_same_name_get_distinct_variables() {
        let m = elaborate_src(
            "module siblings(p, n);
             electrical p, n;
             real g;
             analog begin
               begin : first
                 real t;
                 t = 1.0;
                 g = t;
               end
               begin : second
                 real t;
                 t = 2.0;
                 g = g + t;
               end
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );
        assert_eq!(
            m.vars.iter().filter(|v| v.name == "t").count(),
            2,
            "each block's `t` is its own variable"
        );
    }

    /// The control for the test above: the same shape with **no** name collision must still
    /// elaborate, and the outer read must resolve to the parameter. Without this, rejecting
    /// every block-local declaration would pass the test above for the wrong reason.
    #[test]
    fn a_block_local_declaration_without_a_collision_still_elaborates() {
        let m = elaborate_src(
            "module noshadow(p, n);
             electrical p, n;
             parameter real k = 1000.0;
             real g;
             analog begin
               begin : inner
                 real tmp;
                 tmp = 1.0;
               end
               g = 1.0 / k;
               I(p, n) <+ g * V(p, n);
             end
             endmodule
",
        );
        let g_id = m
            .vars
            .iter()
            .position(|v| v.name == "g")
            .expect("`g` is a variable") as u32;
        let rhs = find_assign_rhs(&m.analog, g_id).expect("`g` is assigned");
        let va_ir::Expr::Binary(_, _, divisor) = m.expr(rhs) else {
            panic!("expected `1.0 / k`, got {:?}", m.expr(rhs));
        };
        assert!(
            matches!(m.expr(*divisor), va_ir::Expr::Param(_)),
            "`k` must resolve to the parameter, got {:?}",
            m.expr(*divisor)
        );
    }

    /// Depth-first search for the RHS of the last `Assign` to `var` in `stmts`.
    fn find_assign_rhs(stmts: &[va_ir::Stmt], var: u32) -> Option<va_ir::ExprId> {
        let mut found = None;
        for s in stmts {
            match s {
                va_ir::Stmt::Assign { lhs, rhs } if lhs.0 == var => found = Some(*rhs),
                va_ir::Stmt::Block(body) => {
                    if let Some(r) = find_assign_rhs(body, var) {
                        found = Some(r);
                    }
                }
                _ => {}
            }
        }
        found
    }

    /// Flatten nested [`va_ir::Stmt::Block`]s into the statements they contain, for tests that
    /// care *what* a generate loop unrolled to rather than how it is nested.
    ///
    /// Since block boundaries are preserved (§ block scoping), each unrolled iteration of a
    /// `for ... begin ... end` is its own `Stmt::Block` — which is correct, an iteration is a
    /// scope — so an assertion on the unrolled statements has to look through that nesting.
    fn flatten_blocks(stmts: &[va_ir::Stmt]) -> Vec<&va_ir::Stmt> {
        let mut out = Vec::new();
        for st in stmts {
            match st {
                va_ir::Stmt::Block(inner) => out.extend(flatten_blocks(inner)),
                other => out.push(other),
            }
        }
        out
    }

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
        let m = elaborate_src(&preprocess_model(include_str!(
            "../../../models/resistor.va"
        )));
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

        // Two contributions: Ohm's law, then the thermal-noise source (T5.2). The noise one
        // carries a real `Builtin::WhiteNoise` call rather than folding to zero — its *value* is
        // zero outside a noise analysis, but that is `va-codegen`'s doing, not the frontend's.
        assert_eq!(m.analog.len(), 2);
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
        match &m.analog[1] {
            va_ir::Stmt::Contribute { target, value } => {
                assert_eq!(target.kind, AccessKind::Flow);
                assert!(
                    matches!(m.expr(*value), Expr::Call(va_ir::Builtin::WhiteNoise, args)
                        if args.len() == 1),
                    "expected a white_noise call, got {:?}",
                    m.expr(*value)
                );
            }
            other => panic!("expected a noise contribution, got {other:?}"),
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

    /// Elaborate one `laplace_*` contribution and return its `(builtin, args)`.
    fn laplace_call(call: &str) -> (Builtin, Vec<va_ir::ExprId>, va_ir::Module) {
        let m = elaborate_src(&format!(
            "module t(a, b); electrical a, b; analog I(a, b) <+ {call}; endmodule"
        ));
        let found = m.exprs.iter().find_map(|e| match e {
            Expr::Call(
                b @ (Builtin::LaplaceNd
                | Builtin::LaplaceNp
                | Builtin::LaplaceZd
                | Builtin::LaplaceZp),
                a,
            ) => Some((*b, a.clone())),
            _ => None,
        });
        let (b, a) = found.expect("a laplace call survives elaboration");
        (b, a, m)
    }

    #[test]
    fn laplace_survives_elaboration_with_its_coefficient_lists_intact() {
        // `laplace_*` used to fold to its DC gain `H(0)` at elaboration, so every filter model
        // read flat: a one-pole lowpass and a straight wire produced identical AC responses.
        // It now reaches the IR whole, with `[value, Const(num_len), num…, den…]` — the flat
        // layout a `Const` separator makes readable (§ `va_ir::Builtin::LaplaceNd`).
        let (b, args, m) = laplace_call("laplace_nd(V(a, b), {2, 0}, {2, 1})");
        assert_eq!(b, Builtin::LaplaceNd);
        assert!(matches!(m.expr(args[0]), Expr::Probe(_)));
        assert!(matches!(m.expr(args[1]), Expr::Const(n) if *n == 2.0));
        assert_eq!(args.len(), 2 + 2 + 2, "value + separator + 2 num + 2 den");
        let num: Vec<f64> = args[2..4]
            .iter()
            .map(|&i| match m.expr(i) {
                Expr::Const(v) => *v,
                o => panic!("{o:?}"),
            })
            .collect();
        assert_eq!(num, vec![2.0, 0.0]);
    }

    #[test]
    fn laplace_coefficients_stay_expressions_rather_than_folding() {
        // The corpus writes coefficients as parameter expressions. Const-folding them here
        // would freeze a filter's poles at their declared defaults and silently ignore a
        // netlist override, so they are lowered as expressions and evaluated per instance.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real tau = 1e-6;              analog I(a, b) <+ laplace_nd(V(a, b), {1}, {1, tau}); endmodule",
        );
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                Expr::Call(Builtin::LaplaceNd, a) => Some(a.clone()),
                _ => None,
            })
            .expect("a laplace call survives elaboration");
        // args = [value, Const(1), num0, den0, den1]; `tau` must not have become a Const.
        assert!(
            matches!(m.expr(args[4]), Expr::Param(_)),
            "tau should survive as a parameter reference, got {:?}",
            m.expr(args[4])
        );
    }

    #[test]
    fn a_pole_at_the_origin_is_now_accepted() {
        // An integrator, `H(s) = 1/s`. Elaboration used to *reject* this outright, because the
        // DC-gain fold divided by zero — an artifact of folding, not a property of the filter.
        // With the transfer function evaluated at `s = jω` it is perfectly well defined
        // everywhere except DC, where `va-codegen` drops the term (see its `stamp_laplace`).
        let (b, _, _) = laplace_call("laplace_np(V(a, b), {1}, {0, 0})");
        assert_eq!(b, Builtin::LaplaceNp);

        // Likewise a zero `s^0` denominator coefficient: `H(s) = 1/s` written the other way.
        let (b, _, _) = laplace_call("laplace_nd(V(a, b), {1}, {0, 1})");
        assert_eq!(b, Builtin::LaplaceNd);
    }

    #[test]
    fn laplace_filter_odd_length_root_array_is_an_error() {
        // Zero/pole arrays hold flattened `(re, im)` pairs, so an odd length is malformed. The
        // check stays at elaboration — where a source file can still be named — even though
        // `va-codegen` re-checks it (§ `lower::laplace_term_shape`).
        let src = "module t(a, b); electrical a, b; \
                    analog I(a, b) <+ laplace_zp(V(a, b), {1, 0, 2}, {1, 0}); endmodule";
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
        let m = elaborate_src(&preprocess_model(include_str!("../../../models/diode.va")));
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
    fn abstime_survives_elaboration_instead_of_folding_to_zero() {
        // The time is the *simulator's* to supply, so `$abstime` must reach the IR as a call
        // and read it at load. It used to fold to a constant `0.0` here, which froze every
        // time-dependent model at t=0 the moment transient analysis existed.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ V(a, b) + $abstime; end endmodule",
        );
        assert!(
            m.exprs
                .iter()
                .any(|e| matches!(e, va_ir::Expr::Call(Builtin::Abstime, args) if args.is_empty())),
            "no $abstime call survived: {:?}",
            m.exprs
        );

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
        let m = elaborate_src(&preprocess_model(include_str!(
            "../../../models/resistor.va"
        )));
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
    fn simparam_folds_to_default_and_noise_lowers_to_a_call() {
        // `$simparam("gmin", 1e-9)` folds to its default; `white_noise(...)` lowers to a real
        // `Builtin::WhiteNoise` call carrying its power argument (T5.2 — `va-codegen` needs the
        // argument to reach the noise channel; the *value* being zero outside noise analysis is
        // codegen's job, not a fold here).
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ $simparam("gmin", 1e-9) * V(a, b) + white_noise(1.0, "thermal"); end endmodule"#;
        let m = elaborate_src(src);
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 1e-9)));
        assert!(m
            .analog
            .iter()
            .any(|s| matches!(s, va_ir::Stmt::Contribute { .. })));
        // The call survives, with exactly one argument — its `"thermal"` label dropped, so no
        // string literal reaches the IR.
        let white = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::WhiteNoise, args) => Some(args.clone()),
                _ => None,
            })
            .expect("white_noise lowers to a WhiteNoise call");
        assert_eq!(white.len(), 1, "the string label must be dropped");
        assert!(
            matches!(m.expr(white[0]), va_ir::Expr::Const(v) if *v == 1.0),
            "the power argument must survive elaboration"
        );

        // The default may be any expression, not just a constant.
        let src = r#"module t(a, b); electrical a, b; parameter real g = 1e-3; analog begin I(a, b) <+ $simparam("gmin", g) * V(a, b); end endmodule"#;
        let m = elaborate_src(src);
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Param(_))));

        // `flicker_noise(pwr, exp)` keeps both arguments, in order.
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ flicker_noise(1e-19, 1.0, "1overf"); end endmodule"#;
        let m = elaborate_src(src);
        let flicker = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::FlickerNoise, args) => Some(args.clone()),
                _ => None,
            })
            .expect("flicker_noise lowers to a FlickerNoise call");
        assert_eq!(flicker.len(), 2, "power and exponent, label dropped");
        assert!(matches!(m.expr(flicker[0]), va_ir::Expr::Const(v) if *v == 1e-19));
        assert!(matches!(m.expr(flicker[1]), va_ir::Expr::Const(v) if *v == 1.0));

        // A noise call missing a required argument is a clear error, not a silent zero.
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ flicker_noise(1e-19); end endmodule"#;
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());

        // `noise_table` lowers to its own builtin — never to a `WhiteNoise` call, whose flat
        // shape would be a different (and wrong) spectrum.
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ V(a, b) + noise_table({1.0, 2e-20, 10.0, 4e-20}); end endmodule"#;
        let m = elaborate_src(src);
        assert!(
            !m.exprs
                .iter()
                .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::WhiteNoise, _))),
            "noise_table must not masquerade as a white source"
        );
        assert!(
            m.exprs
                .iter()
                .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::NoiseTable, _))),
            "noise_table lowers to a NoiseTable call"
        );

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

    /// Elaborate a module whose analog block is just `body`, returning the error message.
    fn elaborate_err(src: &str) -> String {
        let toks = lex(src).expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        elaborate(&ast)
            .expect_err("expected an elaboration error")
            .to_string()
    }

    /// The table survives elaboration as flattened, const-folded `f, p, f, p, …` arguments —
    /// the shape `va-codegen` reads (§ `va_ir::Builtin::NoiseTable`).
    #[test]
    fn noise_table_lowers_to_flattened_constant_pairs() {
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ noise_table({1.0, 2e-20, 10.0, 4e-20}, "tbl"); end endmodule"#;
        let m = elaborate_src(src);
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::NoiseTable, args) => Some(args.clone()),
                _ => None,
            })
            .expect("noise_table lowers to a NoiseTable call");
        assert_eq!(args.len(), 4, "two pairs, the trailing label dropped");
        let vals: Vec<f64> = args
            .iter()
            .map(|&id| match m.expr(id) {
                va_ir::Expr::Const(v) => *v,
                other => panic!("table entries must be constants, got {other:?}"),
            })
            .collect();
        assert_eq!(vals, vec![1.0, 2e-20, 10.0, 4e-20]);
    }

    /// The LRM makes sorting the simulator's job, so an out-of-order table is valid input that
    /// must come out ascending — not an error, and not passed through unsorted (which would
    /// silently invert every interpolated segment downstream).
    #[test]
    fn noise_table_is_sorted_into_ascending_frequency_at_elaboration() {
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ noise_table({100.0, 1e-22, 1.0, 1e-20, 10.0, 1e-21}); end endmodule"#;
        let m = elaborate_src(src);
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::NoiseTable, args) => Some(args.clone()),
                _ => None,
            })
            .expect("a NoiseTable call");
        let freqs: Vec<f64> = args
            .chunks(2)
            .map(|pair| match m.expr(pair[0]) {
                va_ir::Expr::Const(v) => *v,
                _ => panic!("constant"),
            })
            .collect();
        assert_eq!(freqs, vec![1.0, 10.0, 100.0]);
    }

    /// `noise_table_log` is a *separate LRM function* (§4.6.4.4), not a flag on `noise_table`,
    /// so it lowers to its own builtin — that variant is the only thing carrying "interpolate
    /// logarithmically" downstream. The table itself is read by the same code, so a table that
    /// is valid for one is valid for the other.
    #[test]
    fn noise_table_log_lowers_to_its_own_builtin() {
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ noise_table_log({1.0, 1.0, 1e6, 1e-6}); end endmodule"#;
        let m = elaborate_src(src);
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::NoiseTableLog, args) => Some(args.clone()),
                _ => None,
            })
            .expect("noise_table_log lowers to a NoiseTableLog call");
        assert_eq!(args.len(), 4);
        assert!(
            !m.exprs
                .iter()
                .any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::NoiseTable, _))),
            "the linear builtin must not stand in for the logarithmic one — they interpolate \
             differently, and confusing them is silently wrong rather than loudly wrong"
        );
    }

    /// It is a *reserved word* now, not just a recognized call name (LRM Annex B lists it beside
    /// `noise_table`). A user variable of that name must therefore be rejected, exactly as one
    /// named `noise_table` already was.
    #[test]
    fn noise_table_log_is_reserved_and_cannot_be_a_user_identifier() {
        assert_eq!(
            crate::keywords::Keyword::from_ident("noise_table_log").map(|k| k.as_str()),
            Some("noise_table_log")
        );
        let src = "module t(a, b); electrical a, b; real noise_table_log; \
                   analog begin I(a, b) <+ V(a, b); end endmodule";
        let toks = lex(src).expect("lex");
        assert!(
            parse(&toks).is_err(),
            "a reserved word cannot be declared as a variable"
        );
    }

    /// Both spellings share one validator, so both reject the same malformed tables — and the
    /// message names whichever one the author actually wrote, rather than always saying
    /// `noise_table`.
    #[test]
    fn noise_table_log_shares_the_validator_and_is_named_in_its_own_diagnostics() {
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ noise_table_log({1.0, 2e-20, 1.0, 4e-20}); end endmodule"#;
        let msg = elaborate_err(src);
        assert!(msg.contains("unique"), "got: {msg}");
        assert!(
            msg.contains("noise_table_log"),
            "the diagnostic should name the function written, got: {msg}"
        );
    }

    /// Every malformed table the LRM rules out is a named elaboration error rather than a
    /// silently-wrong spectrum. Each of these would otherwise "work" and produce nonsense.
    #[test]
    fn malformed_noise_tables_are_rejected_with_their_own_diagnostics() {
        let cases = [
            // An odd number of values cannot be (frequency, power) pairs.
            (r#"noise_table({1.0, 2e-20, 10.0})"#, "pairs"),
            // The LRM: "Each frequency value must be unique."
            (r#"noise_table({1.0, 2e-20, 1.0, 4e-20})"#, "unique"),
            // A PSD is a power; a negative one is not a table this can interpolate.
            (r#"noise_table({1.0, -2e-20, 10.0, 4e-20})"#, "power"),
            (r#"noise_table({-1.0, 2e-20, 10.0, 4e-20})"#, "frequency"),
            // The file-name form is a separate, unimplemented feature — say so, rather than
            // failing with an array-literal message that misdescribes what the author wrote.
            (r#"noise_table("table.tbl")"#, "file-name"),
            // A bare scalar is not a table at all.
            (r#"noise_table(1.0)"#, "array-literal"),
            // No argument at all.
            (r#"noise_table()"#, "requires a table argument"),
        ];
        for (call, needle) in cases {
            let src = format!(
                "module t(a, b); electrical a, b; analog begin I(a, b) <+ {call}; end endmodule"
            );
            let msg = elaborate_err(&src);
            assert!(
                msg.contains(needle),
                "`{call}` should be rejected mentioning `{needle}`, got: {msg}"
            );
        }
    }

    /// An empty table cannot be *written* — `{}` is not an expression this parser accepts (an
    /// array literal needs at least one element), so the empty case never reaches elaboration
    /// from source. It is still handled defensively downstream (`va_abi::noise::table_psd_at`
    /// returns `0.0` for an empty table rather than indexing `first()`/`last()`), because a
    /// table can also be built programmatically by a caller of that ABI.
    #[test]
    fn an_empty_noise_table_cannot_be_written_and_fails_at_the_parser() {
        let src = r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ noise_table({}); end endmodule"#;
        let toks = lex(src).expect("lex");
        assert!(
            parse(&toks).is_err(),
            "`{{}}` is not a writable array literal"
        );
    }

    /// The table may be written in terms of parameters and macro constants — it only has to be
    /// *constant*, which is what const-folding it at elaboration checks. This is the shape
    /// `models/resistor_noise_table.va` uses to write `4kT/R` without hard-coding the number.
    #[test]
    fn a_noise_table_may_be_built_from_parameter_expressions() {
        let src = "module t(a, b); electrical a, b; parameter real R = 1000.0; \
                   analog begin I(a, b) <+ noise_table({1.0, 4.0*1.380649e-23*300.15/R, \
                   1e6, 4.0*1.380649e-23*300.15/R}); end endmodule";
        let m = elaborate_src(src);
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::NoiseTable, args) => Some(args.clone()),
                _ => None,
            })
            .expect("a NoiseTable call");
        let power = match m.expr(args[1]) {
            va_ir::Expr::Const(v) => *v,
            _ => panic!("constant"),
        };
        let want = 4.0 * 1.380_649e-23 * 300.15 / 1000.0;
        assert!((power - want).abs() < 1e-30, "got {power}, want {want}");
    }

    #[test]
    fn rdist_normal_exponential_poisson_chi_square_fold_to_their_mean_argument() {
        for call in [
            "$rdist_normal(1, 2.5, 1.0)",
            "$rdist_exponential(1, 2.5)",
            "$rdist_poisson(1, 2.5)",
            "$rdist_chi_square(1, 2.5)",
        ] {
            let src = format!(
                "module t(a, b); electrical a, b; analog begin I(a, b) <+ {call} * V(a, b); end endmodule"
            );
            let m = elaborate_src(&src);
            assert!(
                m.exprs
                    .iter()
                    .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 2.5)),
                "{call} should fold to a Const(2.5) mean"
            );
        }
    }

    #[test]
    fn rdist_erlang_folds_to_its_mean_argument() {
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ $rdist_erlang(1, 2, 7.0) * V(a, b); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 7.0)));
    }

    #[test]
    fn rdist_uniform_folds_to_the_midpoint_of_its_bounds() {
        // No single argument carries the mean directly, unlike the other rdist_* forms — it's
        // built as (start + end) / 2 in the IR itself.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ $rdist_uniform(1, 2.0, 6.0) * V(a, b); end endmodule",
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::Add, _, _))));
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Binary(va_ir::BinOp::Div, _, _))));
    }

    #[test]
    fn rdist_t_folds_to_zero_and_type_string_is_never_evaluated() {
        let m = elaborate_src(
            r#"module t(a, b); electrical a, b; analog begin I(a, b) <+ $rdist_t(1, 5, "instance") * V(a, b); end endmodule"#,
        );
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, va_ir::Expr::Const(v) if *v == 0.0)));
    }

    #[test]
    fn rdist_wrong_arity_is_an_error() {
        // `$rdist_normal` needs 3 or 4 arguments (seed, mean, standard_deviation[, type]), not 2.
        let src = "module t(a, b); electrical a, b; analog begin I(a, b) <+ $rdist_normal(1, 2.0) * V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex"))
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
    fn transition_survives_elaboration_with_four_normalized_arguments() {
        // `transition` used to fold transparently to `V(a,b)` — right in a static solve, and a
        // silently wrong waveform in transient, where the ramp *is* what the model expresses.
        // It now reaches the IR with arity normalized, so no consumer re-derives the LRM's
        // defaults: an omitted `fall_time` is the *same expression* as `rise_time` (§4.5.5).
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real td = 1n; parameter real tr = 1n; \
             analog begin I(a, b) <+ transition(V(a, b), td, tr); end endmodule",
        );
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(Builtin::Transition, a) => Some(a.clone()),
                _ => None,
            })
            .expect("a transition call survives elaboration");
        assert_eq!(args.len(), 4, "transition must normalize to four arguments");
        assert!(matches!(m.expr(args[0]), va_ir::Expr::Probe(_)));
        assert_eq!(args[2], args[3], "an omitted fall_time reuses rise_time");

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
    fn slew_survives_elaboration_with_a_symmetric_default_rate() {
        // Same un-folding as `transition`. An omitted `neg_slew_rate` is the *same expression*
        // as the positive one (LRM §4.5.6's symmetric default), not a copied constant — so a
        // parameterised rate stays one expression in the arena.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; parameter real rate = 1e6; \
             analog begin I(a, b) <+ slew(V(a, b), rate); end endmodule",
        );
        let args = m
            .exprs
            .iter()
            .find_map(|e| match e {
                va_ir::Expr::Call(Builtin::Slew, a) => Some(a.clone()),
                _ => None,
            })
            .expect("a slew call survives elaboration");
        assert_eq!(args.len(), 3, "slew must normalize to three arguments");
        assert!(matches!(m.expr(args[0]), va_ir::Expr::Probe(_)));
        assert_eq!(args[1], args[2], "an omitted neg rate reuses the pos rate");
    }

    #[test]
    fn absdelay_lowers_to_its_own_builtin() {
        // Until 2026-09-01 `absdelay` was folded to its value argument and its delay was never
        // lowered at all. Right at DC, silently wrong everywhere else — an optical
        // waveguide's propagation delay simply vanished — so it now survives into the IR as
        // its own builtin, carrying the delay (§6 change; `docs/proposals/absdelay.md`).
        let args_of = |src: &str| {
            elaborate_src(src)
                .exprs
                .iter()
                .find_map(|e| match e {
                    va_ir::Expr::Call(Builtin::Absdelay, a) => Some(a.clone()),
                    _ => None,
                })
                .expect("an absdelay call survives elaboration")
        };

        let m = elaborate_src("module t(a, b); electrical a, b; parameter real td = 1n; analog begin I(a, b) <+ absdelay(V(a, b), td); end endmodule");
        let args = args_of("module t(a, b); electrical a, b; parameter real td = 1n; analog begin I(a, b) <+ absdelay(V(a, b), td); end endmodule");
        assert_eq!(args.len(), 2, "absdelay normalizes to (value, delay)");
        assert!(matches!(m.expr(args[0]), va_ir::Expr::Probe(_)));
        // The delay is a real lowered expression now, not a discarded token.
        assert!(matches!(m.expr(args[1]), va_ir::Expr::Param(_)));

        // A third `maxdelay` argument is accepted and dropped: only a time-domain
        // implementation needs it, to size a history buffer (stage 2 of the proposal).
        assert_eq!(args_of("module t(a, b); electrical a, b; analog begin I(a, b) <+ absdelay(V(a, b), 1n, 5n); end endmodule").len(), 2, "maxdelay is dropped, not carried");

        // No value argument at all is still an error.
        let toks = lex(
            "module t(a, b); electrical a, b; analog begin I(a, b) <+ absdelay(); end endmodule",
        )
        .expect("lex");
        let ast = parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }

    #[test]
    fn ac_stim_normalizes_to_a_mask_and_two_arguments() {
        // Whatever the source writes, the IR carries exactly three arguments: a phase bitmask,
        // a magnitude and a phase. The magnitude/phase used to be discarded entirely — the call
        // folded to a bare `0.0` — which left an AC-driven behavioral model unexcited.
        let ac = va_ir::phase_bit("ac").unwrap();
        let stim = |src: &str| -> (u32, f64, f64) {
            let m = elaborate_src(src);
            let args = m
                .exprs
                .iter()
                .find_map(|e| match e {
                    va_ir::Expr::Call(Builtin::AcStim, args) => Some(args.clone()),
                    _ => None,
                })
                .expect("an ac_stim call survives elaboration");
            assert_eq!(args.len(), 3, "ac_stim must normalize to three arguments");
            let konst = |i: usize| match m.expr(args[i]) {
                va_ir::Expr::Const(v) => *v,
                other => panic!("argument {i} is not constant: {other:?}"),
            };
            (konst(0) as u32, konst(1), konst(2))
        };

        let head = "module t(a, b); electrical a, b; analog begin I(a, b) <+ ";
        // Explicit magnitude and phase, analysis defaulting to "ac".
        assert_eq!(
            stim(&format!(
                "{head} ac_stim(2.0, 0.5) + V(a, b); end endmodule"
            )),
            (ac, 2.0, 0.5)
        );
        // LRM defaults: magnitude 1.0, phase 0.0.
        assert_eq!(
            stim(&format!("{head} ac_stim() + V(a, b); end endmodule")),
            (ac, 1.0, 0.0)
        );
        // A leading string names the analysis instead of being mistaken for a magnitude.
        assert_eq!(
            stim(&format!(
                r#"{head} ac_stim("noise", 3.0) + V(a, b); end endmodule"#
            )),
            (va_ir::phase_bit("noise").unwrap(), 3.0, 0.0)
        );
    }

    #[test]
    fn bound_step_lowers_to_its_own_statement_instead_of_a_no_op() {
        // `bound_step(step);` used to be discarded with the system tasks. It now survives as a
        // statement carrying its argument, so the transient controller can honour it.
        let m = elaborate_src(
            "module t(a, b); electrical a, b; \
             analog begin bound_step(1n); I(a, b) <+ V(a, b); end endmodule",
        );
        assert_eq!(m.analog.len(), 2);
        let va_ir::Stmt::BoundStep(e) = m.analog[0] else {
            panic!("expected a bound_step statement, got {:?}", m.analog[0]);
        };
        assert!(matches!(m.expr(e), va_ir::Expr::Const(v) if (*v - 1e-9).abs() < 1e-21));

        // It is a statement, not a value: writing it in expression position is an error rather
        // than a silent zero.
        let src = "module t(a, b); electrical a, b; \
                   analog begin I(a, b) <+ bound_step(1n) + V(a, b); end endmodule";
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
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
                let flat = flatten_blocks(stmts);
                assert_eq!(flat.len(), 3);
                assert!(flat
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
                let flat = flatten_blocks(stmts);
                assert_eq!(flat.len(), 2);
                assert!(flat
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
                let flat = flatten_blocks(stmts);
                assert_eq!(flat.len(), 3);
                assert!(flat.iter().all(|s| matches!(s, va_ir::Stmt::Assign { .. })));
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
    fn port_probe_excludes_charge_terms_and_keeps_the_resistive_half_of_the_same_branch() {
        // `hicumL0_v2p0p0.va`'s real shape: the probed node carries both conduction current and
        // charge on the same branch. Inlining the raw RHS used to manufacture a `ddt` nested in
        // an ordinary assignment (`IB = I(<b>);`) — a shape none of those models actually wrote
        // — and `va-codegen` refused it, blaming a nested `ddt` that appears nowhere in their
        // source. The probe now reports conduction current only (§ `resistive_terms_only`).
        let m = elaborate_src(
            "module dev(a, c); inout a, c; electrical a, c;              parameter real is = 1e-14;              parameter real cj = 1e-12;              analog begin                I(a, c) <+ is * V(a, c) + ddt(cj * V(a, c));                I(a) <+ I(<a>);              end endmodule",
        );
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(
            !expr_contains_ddt(&m, value),
            "the probe must carry no `ddt`; got {:?}",
            m.expr(value)
        );
        // The resistive half must survive: a probe that folded to a bare `0.0` would satisfy
        // the assertion above for entirely the wrong reason.
        assert!(
            matches!(m.expr(value), va_ir::Expr::Binary(va_ir::BinOp::Mul, _, _)),
            "expected the surviving `is * V(a,c)` term, got {:?}",
            m.expr(value)
        );
    }

    #[test]
    fn port_probe_of_a_purely_capacitive_branch_folds_to_zero() {
        // The boundary of the rule above: if *every* term of every touching contribution is a
        // charge term, the port carries no conduction current and the probe is zero — not an
        // error, and not a stray `ddt`.
        let m = elaborate_src(
            "module cap(a, c); inout a, c; electrical a, c;              parameter real cj = 1e-12;              analog begin                I(a, c) <+ ddt(cj * V(a, c));                I(a) <+ I(<a>);              end endmodule",
        );
        let value = match &m.analog[1] {
            va_ir::Stmt::Contribute { value, .. } => *value,
            other => panic!("expected a contribution, got {other:?}"),
        };
        assert!(
            matches!(m.expr(value), va_ir::Expr::Const(c) if *c == 0.0),
            "expected a folded 0.0, got {:?}",
            m.expr(value)
        );
    }

    /// The expression `ib = I(<a>);` lowered to — resolved through [`var_id`]/[`assign_rhs`] by
    /// *name*, so these tests read the probe back the same way the reproducer writes it.
    fn probe_read(m: &Module, var: &str) -> va_ir::ExprId {
        assign_rhs(m, var_id(m, var))
            .unwrap_or_else(|| panic!("no assignment to `{var}` in the elaborated block"))
    }

    /// Whether `expr`'s tree reads any local variable. Used to prove a dropped charge term is
    /// gone *entirely* — not merely stripped of its `ddt` while still reading the variable that
    /// carried it.
    fn expr_contains_var(m: &Module, expr: va_ir::ExprId) -> bool {
        match m.expr(expr) {
            va_ir::Expr::Var(_) => true,
            va_ir::Expr::Call(_, args) | va_ir::Expr::CallUser(_, args) => {
                args.iter().any(|&a| expr_contains_var(m, a))
            }
            va_ir::Expr::Unary(_, e) | va_ir::Expr::Ddx(e, _) => expr_contains_var(m, *e),
            va_ir::Expr::Binary(_, l, r) => expr_contains_var(m, *l) || expr_contains_var(m, *r),
            va_ir::Expr::Select(c, t, f) => {
                expr_contains_var(m, *c) || expr_contains_var(m, *t) || expr_contains_var(m, *f)
            }
            _ => false,
        }
    }

    #[test]
    fn port_probe_excludes_a_charge_term_that_reached_the_branch_through_a_variable() {
        // The same physics as `port_probe_excludes_charge_terms_...` above, spelled with the
        // `ddt` bound to a variable first. A purely syntactic scan of the `<+` right-hand side
        // sees only `is*V(a,c) + qd` and calls it entirely resistive, so the charge survived
        // into the probe — the two spellings of one device disagreed, and the one that carried
        // the charge was then rejected downstream by index (`variable #0 read before
        // assignment`) rather than by name. The variable arm of `Elaborator::contains_ddt`
        // (§ `Elaborator::ddt_tainted_vars`) is what closes that gap.
        let m = elaborate_src(
            "module dev(a, c); inout a, c; electrical a, c; \
             parameter real is = 1e-14; \
             parameter real cj = 1e-12; \
             real qd, ib; \
             analog begin \
               qd = ddt(cj * V(a, c)); \
               I(a, c) <+ is * V(a, c) + qd; \
               ib = I(<a>); \
               I(a) <+ ib; \
             end endmodule",
        );
        let value = probe_read(&m, "ib");
        assert!(
            !expr_contains_ddt(&m, value),
            "the probe must carry no `ddt`; got {:?}",
            m.expr(value)
        );
        // Stronger than "no `ddt`": the `qd` read itself must be gone. A probe that kept
        // `Var(qd)` while the scan merely failed to look inside it would satisfy the assertion
        // above and still be exactly the bug.
        assert!(
            !expr_contains_var(&m, value),
            "the probe must not read the charge variable at all; got {:?}",
            m.expr(value)
        );
        // And the resistive half must survive — a fold to a bare `0.0` would pass both
        // assertions above for entirely the wrong reason.
        assert!(
            matches!(m.expr(value), va_ir::Expr::Binary(va_ir::BinOp::Mul, _, _)),
            "expected the surviving `is * V(a,c)` term, got {:?}",
            m.expr(value)
        );
    }

    #[test]
    fn port_probe_taint_follows_a_chain_of_assignments_to_a_fixed_point() {
        // One assignment of depth is not enough: `qd` carries the `ddt`, `qs` carries `qd`, and
        // it is `qs` that reaches the branch. Only the fixed point in
        // `Elaborator::ddt_tainted_vars` sees the second link.
        let m = elaborate_src(
            "module dev(a, c); inout a, c; electrical a, c; \
             parameter real is = 1e-14; \
             parameter real cj = 1e-12; \
             real qd, qs, ib; \
             analog begin \
               qd = ddt(cj * V(a, c)); \
               qs = 2.0 * qd; \
               I(a, c) <+ is * V(a, c) + qs; \
               ib = I(<a>); \
               I(a) <+ ib; \
             end endmodule",
        );
        let value = probe_read(&m, "ib");
        assert!(
            !expr_contains_ddt(&m, value) && !expr_contains_var(&m, value),
            "the probe must carry neither the `ddt` nor the variable chain that reached it; \
             got {:?}",
            m.expr(value)
        );
        assert!(
            matches!(m.expr(value), va_ir::Expr::Binary(va_ir::BinOp::Mul, _, _)),
            "expected the surviving `is * V(a,c)` term, got {:?}",
            m.expr(value)
        );
    }

    #[test]
    fn port_probe_keeps_a_variable_term_that_never_sees_a_ddt() {
        // The negative control for the taint analysis: the two terms of this contribution are
        // both bare variable reads, and only one of them was ever assigned a `ddt`. Dropping
        // both would make `I(<port>)` useless for every model that names its conduction current
        // before contributing it — the over-approximation must be over `ddt` reachability, not
        // over "is a variable".
        let m = elaborate_src(
            "module dev(a, c); inout a, c; electrical a, c; \
             parameter real is = 1e-14; \
             parameter real cj = 1e-12; \
             real qd, gm, ib; \
             analog begin \
               qd = ddt(cj * V(a, c)); \
               gm = is * V(a, c); \
               I(a, c) <+ gm + qd; \
               ib = I(<a>); \
               I(a) <+ ib; \
             end endmodule",
        );
        let value = probe_read(&m, "ib");
        // The probe is exactly the surviving `gm` read — resolved to the `VarId` `gm` itself
        // names, so this pins *which* variable survived, not merely that one did.
        let gm = var_id(&m, "gm");
        assert!(
            matches!(m.expr(value), va_ir::Expr::Var(v) if *v == gm),
            "expected the probe to be the untainted `gm` read, got {:?}",
            m.expr(value)
        );
    }

    #[test]
    fn port_probe_taint_is_not_path_sensitive_and_a_reassigned_variable_stays_tainted() {
        // Locks in the documented over-approximation (§ `Elaborator::ddt_tainted_vars`): `qd` is
        // plainly `0.0` by the time the contribution reads it, but taint is a property of the
        // variable across the whole block, not of a program point, so the term is still dropped.
        // Deliberate: over-approximating loses a conduction term the probe already documents
        // itself as approximating, whereas under-approximating smuggles a charge into a probe
        // documented to report conduction current only.
        let m = elaborate_src(
            "module dev(a, c); inout a, c; electrical a, c; \
             parameter real is = 1e-14; \
             parameter real cj = 1e-12; \
             real qd, ib; \
             analog begin \
               qd = ddt(cj * V(a, c)); \
               qd = 0.0; \
               I(a, c) <+ is * V(a, c) + qd; \
               ib = I(<a>); \
               I(a) <+ ib; \
             end endmodule",
        );
        let value = probe_read(&m, "ib");
        assert!(
            matches!(m.expr(value), va_ir::Expr::Binary(va_ir::BinOp::Mul, _, _)),
            "expected only the `is * V(a,c)` term to survive, got {:?}",
            m.expr(value)
        );
    }

    #[test]
    fn port_probe_refuses_a_guard_that_depends_on_a_ddt_by_name() {
        // A condition cannot be half-kept the way a sum can, so a guard that reaches a `ddt` is
        // refused rather than folded. The point of the test is the *wording*: the old failure
        // for this shape came out of `va-codegen` as `variable #0 read before assignment`,
        // which names neither the construct nor the cause.
        let err = elaborate_err(
            "module dev(a, c); inout a, c; electrical a, c; \
             parameter real is = 1e-14; \
             parameter real cj = 1e-12; \
             real qd, ib; \
             analog begin \
               qd = ddt(cj * V(a, c)); \
               if (qd > 0.0) I(a, c) <+ is * V(a, c); \
               ib = I(<a>); \
               I(a) <+ ib; \
             end endmodule",
        );
        assert!(
            err.contains("`I(<a>)`") && err.contains("`qd`") && err.contains("ddt"),
            "the refusal must name the construct, the variable, and the cause; got: {err}"
        );
        assert!(
            !err.contains("read before assignment"),
            "the refusal must not fall through to the by-index codegen message; got: {err}"
        );
    }

    /// Whether `expr`'s tree contains a `ddt` call — the test-side mirror of
    /// [`Elaborator::contains_ddt`], which works on the elaborator's in-progress module.
    fn expr_contains_ddt(m: &Module, expr: va_ir::ExprId) -> bool {
        match m.expr(expr) {
            va_ir::Expr::Call(va_ir::Builtin::Ddt, _) => true,
            va_ir::Expr::Call(_, args) | va_ir::Expr::CallUser(_, args) => {
                args.iter().any(|&a| expr_contains_ddt(m, a))
            }
            va_ir::Expr::Unary(_, e) | va_ir::Expr::Ddx(e, _) => expr_contains_ddt(m, *e),
            va_ir::Expr::Binary(_, l, r) => expr_contains_ddt(m, *l) || expr_contains_ddt(m, *r),
            va_ir::Expr::Select(c, t, f) => {
                expr_contains_ddt(m, *c) || expr_contains_ddt(m, *t) || expr_contains_ddt(m, *f)
            }
            _ => false,
        }
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
        // Both generate loops are fully unrolled: one nest per `i`, each holding one Assign per
        // `j`. Since 2026-08-31 each `begin ... end` is preserved as its own scope, so an
        // iteration's statements sit one `Stmt::Block` deeper than the unrolling itself puts
        // them — `flatten_blocks` looks through that, which is why this asserts on *counts of
        // assignments* rather than on the exact nesting.
        match &m.analog[0] {
            va_ir::Stmt::Block(outer) => {
                let per_i = flatten_blocks(outer);
                assert_eq!(per_i.len(), 4, "2 values of i x 2 of j");
                assert!(per_i
                    .iter()
                    .all(|s| matches!(s, va_ir::Stmt::Assign { .. })));
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

    #[test]
    fn ground_aliases_an_explicit_net_to_the_implicit_reference_node() {
        // `V(a, gnd)` (explicit) and `V(a)` (implicit single-terminal) must resolve to exactly
        // the same branch once `gnd` is declared ground — proving `collect_ground` reused
        // `gnd`'s own `NodeId` as the reference node rather than creating a separate one.
        let m = elaborate_src(
            "module t(a); electrical a, gnd; ground gnd; analog begin I(a, gnd) <+ V(a, gnd); I(a) <+ V(a); end endmodule",
        );
        assert_eq!(m.branches.len(), 1);
    }

    #[test]
    fn ground_merges_multiple_named_nets_into_one_reference_node() {
        let m = elaborate_src(
            "module t(a); electrical a, gnd1, gnd2; ground gnd1, gnd2; analog begin I(a, gnd1) <+ V(a, gnd1); I(a, gnd2) <+ V(a, gnd2); end endmodule",
        );
        assert_eq!(m.branches.len(), 1);
    }

    #[test]
    fn ground_of_an_undeclared_net_errors() {
        let src =
            "module t(a); electrical a; ground nope; analog begin I(a) <+ V(a); end endmodule";
        let ast = parse(&lex(src).expect("lex"))
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module");
        assert!(elaborate(&ast).is_err());
    }
}
