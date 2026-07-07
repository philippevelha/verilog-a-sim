//! Lowering: walk a [`va_ir::Module`]'s analog block into an ordered sequence of executable
//! statements — local-variable assignments and contributions (each already flattened and split
//! into resistive/charge terms) — in source order.
//!
//! Order matters once local variables are involved: `real q; q = c0*v + …; I(p,n) <+ ddt(q);`
//! only evaluates correctly if `q`'s assignment runs *before* the contribution that reads it,
//! and it must run again on every [`va_abi::ModelInstance::load`] call (an assigned value
//! depends on `x`, so it can't be precomputed once here at lowering time — this module stays
//! purely structural, same as before local variables were supported; only the shape of the
//! plan it hands back changed, from an unordered `Vec<Contribution>` to an ordered statement
//! sequence). See `crate::ad::Ctx::set_var`/`get_var` for where the actual sequential
//! execution and variable environment live.
//!
//! `if`/`else` (`Stmt::If`) lowers too, but it is genuinely different from the other two
//! statement kinds: which branch runs depends on `x`, so it can't be flattened away here the
//! way a contribution's terms are — [`LoweredStmt::If`] carries *both* arms, each its own
//! lowered statement sequence, and `crate::GeneratedModel` picks one at `load()` time based on
//! the condition's value at the current operating point (same "only the taken branch is ever
//! evaluated" rule the ternary `Expr::Select` already follows in `crate::ad::eval`). One
//! consequence: `crate::GeneratedModel::validate`, which normally evaluates everything once at
//! the all-zero point to catch an unsupported construct before it ever reaches `load`, must
//! visit *both* arms unconditionally here — an arm the all-zero point doesn't happen to select
//! could still be the one a real operating point takes later.
//!
//! A potential (voltage) contribution `V(p,n) <+ expr` lowers too, but stamps somewhere
//! genuinely different from a flow contribution: it's a *constraint* (`V(p)-V(n) = expr`,
//! not a current balance), which needs its own auxiliary branch-current unknown — see
//! [`BranchCurrent`] and [`Lowered::branch_currents`]. `lower` computes, once per module,
//! which branches need one (every branch targeted by at least one potential contribution
//! anywhere in the analog block, `if`/`else` arms included) and assigns each a local terminal
//! slot past the node slots (`module.nodes.len()..`); `crate::GeneratedModel` is what actually
//! stamps the constraint row and the branch's own KCL injection (see
//! `crate::GeneratedModel::stamp_branch_currents`/`stamp`).
//!
//! A branch can receive *both* flow and potential contributions, gated by mutually-exclusive
//! `if`/`else` arms — a real, recurring idiom (the widely-reused `` `collapsibleR `` macro,
//! `diode_cmc.va`'s several collapsible branches): a parameter picks, once, whether the branch
//! behaves as an ordinary current-defined element or collapses to a forced/near-zero-impedance
//! voltage constraint. [`BranchCurrent::mixed`] flags exactly these branches; unlike a
//! branch that only ever gets potential contributions, a mixed branch's constraint row can't be
//! stamped unconditionally up front, because its very shape depends on which kind of
//! contribution this particular `load()` call's control flow actually takes — see
//! `crate::GeneratedModel::stamp`/`finalize_mixed_branch_currents` for how that gets resolved
//! at evaluation time instead of here.
//!
//! `while`/`for`/`repeat` loops and `case` statements lower too, both by generalizing patterns
//! already established above rather than needing anything new. `case` is an n-ary `if`/`else`:
//! [`LoweredStmt::Case`] carries every arm's labels and body plus the default body, and
//! `crate::GeneratedModel::run`/`validate_stmts` extend the existing "run only the selected
//! branch, validate every branch once" split to however many arms there are instead of
//! exactly two. Loops are different in kind, not degree: `while`/`for`/`repeat` need genuine
//! *repeated* execution at `load()` time — real compact models use them for a parameter-bounded
//! accumulation (`for (i=0; i<nf; i=i+1) acc = acc + term;`, one term per transistor finger) or
//! a capped Newton-style sub-iteration inside the analog block itself (`while (abs(d_Q) >= tol
//! && iters <= max) …`), never for anything array-indexed — the frontend's own elaboration pass
//! already expands any array/genvar indexing into an ordinary `if`/`else` chain before this IR
//! ever exists (see `va-frontend::elaborate`'s `unroll_indexed_contribute`/
//! `lower_indexed_var_write`), so a loop body here is just an ordinary statement sequence.
//! `crate::GeneratedModel::run` interprets a loop for real — actually iterating, actually
//! re-evaluating the condition/count against the current variable bindings each time, so the
//! forward-mode AD gradient accumulates correctly across iterations exactly like any other
//! sequence of statements would (AD doesn't know or care that a "loop" produced the sequence).
//! A `while`/`for` loop's trip count isn't knowable in advance (its condition can depend on
//! `x` or on state a preceding iteration computed), so `run` bounds it defensively at a fixed
//! iteration cap — see `crate::MAX_LOOP_ITERATIONS`'s doc comment for what happens if a
//! pathological (or genuinely non-terminating) condition exceeds it. `validate`, in contrast,
//! never actually iterates a loop at all: it only needs to confirm every statement *inside* the
//! body is itself evaluable, which running the body exactly once (same as any other block of
//! statements) already establishes, without needing to resolve a real trip count or risk
//! hanging on a runaway condition during eager validation.
//!
//! `ddt` is recognised as a top-level additive term (`I <+ resistive + ddt(charge)`), optionally
//! negated, *and* optionally scaled by a parameter-only coefficient
//! (`coeff*ddt(charge)`/`ddt(charge)*coeff`/`ddt(charge)/coeff` — a real corpus survey found
//! this "coefficient times a time-derivative" shape in every single one of a batch of
//! previously-blocked real compact models, e.g. `bsim4.va`'s `I(gi,si) <+ BSIM4type *
//! ddt(qgate);`, a polarity-selection parameter scaling a charge term). The coefficient must be
//! **provably parameter-only** ([`is_param_only`]) — built from nothing but `Const`/`Param`,
//! pure arithmetic/builtin combinations of those, and (recursively) other provably parameter-only
//! *local variables* ([`param_only_vars`]) — never a node/branch probe or function call — because
//! `coeff(x) * dQ/dt` only equals `d(coeff*Q)/dt` (letting it fold into the ordinary charge
//! channel) when `coeff` doesn't itself depend on the unknowns `x`; this project's
//! `va_abi::StampSink` charge channel has no way to express the general product-rule case where
//! it does (that would need the whole companion-model discretization, currently owned entirely
//! by `va-transient`'s integrator via a single per-row time-stepping coefficient, to also carry a
//! per-term, model-supplied coefficient — a `va_abi`/`va_transient` interface change, not a
//! `va-codegen`-local one). A local variable counts as parameter-only when **every**
//! `Stmt::Assign` to it anywhere in the analog block assigns a parameter-only expression — real
//! models commonly compute a polarity/sign flag once from a parameter comparison (`if (TYPE ==
//! \`ntype) devsign = 1; else devsign = -1;`, `bsimbulk.va`) or guard it behind an `x`-dependent
//! *condition* while every *assigned value* stays parameter-only (`asmhemt.va`'s `if (V(g) >
//! voff) ct = ctrap3; else ct = 1.0e-9;` — the guard reading a node voltage doesn't matter, only
//! what actually gets assigned does) — this is an eager, non-path-sensitive over-approximation
//! (same character as the `if`/`else`-validation split elsewhere in this crate): it's sound
//! (every accepted variable really is parameter-only on every path that could reach it) but not
//! complete (a variable that's parameter-only on the *specific* path relevant to one `ddt` site
//! but genuinely `x`-dependent on some unrelated path stays rejected). [`charge_term_shape`] now
//! recurses through arbitrarily many nested multiplications/divisions rather than inspecting only
//! the immediate operands of the outermost one — `ekv26.va`'s `ddt(qjd)*TYPE*M` parses as
//! `(ddt(qjd)*TYPE)*M`, two levels deep, and needed exactly this. `ddt` still may not appear
//! nested any *other* way (inside a ternary, as another builtin's argument, etc.) — none of those
//! shapes turned up anywhere in the same survey, so there was nothing concrete to scope a fix
//! against.
//!
//! A `ddt` result assigned to a plain local variable and read back later in a `<+`
//! (`real dqdt; dqdt = ddt(q); I(p,n) <+ dqdt + …;` — seen in the wild specifically to work
//! around this project's still-`if`/`case`-restricted `ddt` placement in some real models, e.g.
//! `angelov_gan.va`'s `T0 = ddt(Ldc * I(rf,si)); // Avoid analog operator in if/else block`) is
//! tracked back to its defining assignment via [`DdtVars`]: a `Stmt::Assign` whose RHS is itself
//! a recognized `ddt` shape never becomes an ordinary [`LoweredStmt::Assign`] (there would be no
//! sound value to assign — evaluating a bare `ddt(...)` outside the charge channel is exactly
//! what this project cannot do) and instead records `lhs -> rhs` for the `Stmt::Contribute` arm
//! to substitute when it later encounters a bare read of that variable as an additive term. This
//! is forward, single-pass, and intentionally *not* a full reaching-definitions analysis: entering
//! any branch/loop body clones the map, and the clone's mutations are discarded on exit rather
//! than merged back, so a variable reassigned inside only one arm of an `if`/`case` (a common
//! pattern — `T0` is reused for unrelated scratch values throughout `angelov_gan.va`) never lets
//! a stale or wrong definition leak to code after the branch. A variable resolved this way is
//! *only* ever substituted at a `<+` site; if it's read as an ordinary value anywhere else while
//! still holding a `ddt` shape, lowering silently drops that read's assignment (no
//! `LoweredStmt::Assign` was ever emitted for it) rather than miscomputing — out of scope because
//! neither corpus file needs it, not because it would be sound to guess a value.
//!
//! `idt` (the time-*integral* operator) is lowered too, but architecturally differently from
//! `ddt`: its value at a given instant depends on the *entire history* of its argument, not just
//! the current unknowns, so it can't be recovered symbolically the way `ddt`'s charge argument
//! is. Instead, each distinct `idt(expr)` call site gets its own auxiliary "accumulator" unknown
//! `Y` (see [`IdtAccumulator`]), enforcing `ddt(Y) = expr` via the ordinary charge-channel
//! machinery, self-contained exactly like a potential contribution's own branch-current unknown —
//! `crate::GeneratedModel::stamp_idt_accumulators` stamps this row unconditionally every `load()`
//! call, independent of whatever control flow does or doesn't reach the specific `idt(...)`
//! expression that call site sits in. Reading `idt(expr)`'s *value* is then just an ordinary read
//! of `Y` (`crate::ad::Ctx::idt_slots`/`crate::ad::eval`'s `Builtin::Idt` case), so — unlike
//! `ddt` — `idt` may appear anywhere in an expression, not only as a top-level contribution term:
//! this is exactly the shape `psp102`'s NQS variants need,
//! `V(SPLINE1) <+ vnorm_inv * idt(-Tnorm * fk1, Qp1_0);`, a coefficient-scaled `idt` nested inside
//! a potential contribution's RHS with no special-casing of the multiplication at all.
//!
//! # Limitations
//!
//! - `idt`'s optional second (initial-condition) argument is accepted syntactically (so a
//!   two-argument call doesn't fail to lower) but not applied: this project already starts every
//!   transient run from the all-zero vector (no `.ic`/`UIC` support at all — `va-cli`'s module
//!   doc comment), so an accumulator's initial value is whatever the DC operating point resolves
//!   it to (in general *not* the declared `ic`), the same honest limitation as every other
//!   reactive state in this codegen, not a special gap in `idt` specifically.
//! - The local-variable `ddt`-indirection tracking above only ever substitutes a *bare* variable
//!   read (`Expr::Var`) that is itself one additive term; a variable read as part of a larger
//!   sub-expression (e.g. `2*dqdt`) is not tracked back to its defining `ddt` call — no corpus
//!   file surveyed needed that shape. `idt` never needs this at all — its value is an ordinary
//!   unknown read, substitutable anywhere, not just at a `<+` site.
//!
//! User-defined analog functions (`Expr::CallUser`) are handled entirely in `crate::ad` instead
//! — a function call is an expression-level construct, so it never needs anything from this
//! module's statement-level extraction of the *analog block* (see `crate::ad::call_function`).

use crate::CodegenError;
use std::collections::{BTreeSet, HashMap, HashSet};
use va_ir::{
    AccessKind, BinOp, BranchId, Builtin, Expr, ExprId, Module, NodeId, Stmt, UnOp, VarId,
};

/// One additive term of a contribution: a signed expression handle.
#[derive(Clone, Copy, Debug)]
pub struct Term {
    /// `+1.0` or `-1.0`, accumulated from `-`/unary-negation while flattening.
    pub sign: f64,
    /// The (already ddt-stripped) expression to evaluate.
    pub expr: ExprId,
}

/// One additive charge-channel term: `ddt(expr)`, optionally scaled by any depth of
/// parameter-only multiplication/division (`coeff*ddt(expr)`, `ddt(expr)*coeff`,
/// `ddt(expr)/coeff`, `coeff1*coeff2*ddt(expr)`, `(ddt(expr)*coeff1)*coeff2`, ... — see this
/// module's doc comment for why each coefficient must be parameter-only).
#[derive(Clone, Debug)]
pub struct ChargeTerm {
    /// `+1.0` or `-1.0`, accumulated from `-`/unary-negation while flattening.
    pub sign: f64,
    /// The `ddt` call's own argument — the quantity whose time-derivative is contributed.
    pub expr: ExprId,
    /// Every scaling factor found wrapping the `ddt`, each paired with whether it divides
    /// (`true`) rather than multiplies (`false`). Empty for a plain, unscaled `ddt(expr)`.
    pub coeffs: Vec<(ExprId, bool)>,
}

/// A single branch contribution, split into resistive and charge channels.
#[derive(Clone, Debug)]
pub struct Contribution {
    /// Which branch this contribution targets — consulted only to accumulate a flow
    /// contribution's resistive total for [`FlowCurrentAccumulator`] (see its doc comment);
    /// otherwise `p_slot`/`n_slot`/`branch_slot` already carry everything stamping needs.
    pub branch: BranchId,
    /// Local node slot of the branch's positive terminal.
    pub p_slot: usize,
    /// Local node slot of the branch's negative terminal.
    pub n_slot: usize,
    /// `Some(slot)` for a potential (voltage) contribution — the local terminal slot of this
    /// branch's own auxiliary current unknown (see [`BranchCurrent`]); `None` for an ordinary
    /// flow (current) contribution, stamped directly at `p_slot`/`n_slot` as before.
    pub branch_slot: Option<usize>,
    /// Static terms summed into the residual/Jacobian.
    pub resistive: Vec<Term>,
    /// `ddt` terms summed into the charge/charge-Jacobian channel.
    pub charge: Vec<ChargeTerm>,
}

/// One branch that receives a potential (voltage) contribution somewhere in the module, and
/// the local terminal slot allocated for its auxiliary branch-current unknown.
///
/// For a **non-mixed** branch (`mixed == false`), `crate::GeneratedModel::stamp_branch_currents`
/// stamps two things for every entry, unconditionally, exactly once per
/// [`crate::GeneratedModel::load`] call regardless of which (if any) `if`/`else` arm actually
/// contributes to it that call: the constraint row itself (`V(p)-V(n) = 0` structurally; each
/// executed `V(...)<+expr` statement subtracts its own `expr` from that same row via
/// `crate::GeneratedModel::stamp`) and the branch current's ordinary two-terminal KCL injection
/// (`+ib` at `p`, `-ib` at `n`). A path that contributes nothing to this branch this call
/// defaults the row to `V(p)-V(n) = 0`, matching the LRM's implicit-zero-contribution rule for
/// an access nothing ever assigns on that path.
///
/// For a **mixed** branch (`mixed == true`, this module's doc comment), that unconditional
/// up-front stamp would be wrong on a call where a flow contribution runs instead: the
/// constraint row's very meaning depends on which kind actually executed. Its structural part
/// is stamped lazily instead, the first time a potential contribution actually runs for it
/// (`crate::GeneratedModel::stamp`); if none does, `crate::GeneratedModel::
/// finalize_mixed_branch_currents` pins the otherwise-unconstrained auxiliary current to zero
/// after the walk finishes, once it's known no potential contribution claimed the row this call.
#[derive(Clone, Copy, Debug)]
pub struct BranchCurrent {
    /// Which branch this auxiliary unknown belongs to.
    pub branch: BranchId,
    /// Local node slot of the branch's positive terminal.
    pub p_slot: usize,
    /// Local node slot of the branch's negative terminal.
    pub n_slot: usize,
    /// Local terminal slot (`>= module.nodes.len()`) allocated for the branch's own current.
    pub local_slot: usize,
    /// Whether this branch also receives a flow contribution somewhere in the module (always
    /// in a different, mutually-exclusive `if`/`else` arm from every potential contribution to
    /// it — see this struct's doc comment).
    pub mixed: bool,
}

/// One executable statement in the codegen v0 subset, in source order.
#[derive(Clone, Debug)]
pub enum LoweredStmt {
    /// `lhs = rhs`: evaluate `rhs` (under whatever variable bindings are in scope so far) and
    /// bind the result to `lhs` for subsequent statements to read.
    Assign {
        /// The assigned variable.
        lhs: VarId,
        /// The expression to evaluate and bind.
        rhs: ExprId,
    },
    /// A flow or potential contribution, already split into resistive/charge terms.
    Contribute(Contribution),
    /// `if (cond) { then_ } else { else_ }`. `crate::GeneratedModel::run` walks only the arm
    /// `cond` selects at the current operating point; `crate::GeneratedModel::validate` walks
    /// both (see this module's doc comment).
    If {
        /// The condition to evaluate; non-zero selects `then_`.
        cond: ExprId,
        /// Statements to run when `cond` is non-zero.
        then_: Vec<LoweredStmt>,
        /// Statements to run when `cond` is zero.
        else_: Vec<LoweredStmt>,
    },
    /// `case (selector) { arms… } [default]`. `crate::GeneratedModel::run` evaluates `selector`
    /// once, then walks only the first arm with a matching label (or `default`, if none match);
    /// `crate::GeneratedModel::validate` walks every arm plus `default` unconditionally, the
    /// same n-ary generalization of [`Self::If`]'s two-arm split.
    Case {
        /// The selector expression, evaluated once.
        selector: ExprId,
        /// Arms in source order; the first with a label equal to `selector`'s value wins.
        arms: Vec<LoweredCaseArm>,
        /// Statements to run when no arm's label matches.
        default: Vec<LoweredStmt>,
    },
    /// `while (cond) { body }`. `crate::GeneratedModel::run` actually iterates (this module's
    /// doc comment); `crate::GeneratedModel::validate` runs `body` exactly once, unconditionally.
    While {
        /// Re-evaluated before every iteration; the loop stops once this is zero.
        cond: ExprId,
        /// Statements executed once per iteration.
        body: Vec<LoweredStmt>,
    },
    /// `for (init; cond; step) { body }`, same execution model as [`Self::While`] plus an
    /// `init` run once before the first condition check and a `step` run after every iteration.
    For {
        /// Run exactly once, before the first `cond` check.
        init: Vec<LoweredStmt>,
        /// Re-evaluated before every iteration; the loop stops once this is zero.
        cond: ExprId,
        /// Run once after every iteration's `body`, before the next `cond` check.
        step: Vec<LoweredStmt>,
        /// Statements executed once per iteration.
        body: Vec<LoweredStmt>,
    },
    /// `repeat (count) { body }`: `count` is evaluated once, then `body` runs that many times
    /// (rounded to the nearest non-negative integer).
    Repeat {
        /// Evaluated once, before the first iteration.
        count: ExprId,
        /// Statements executed once per iteration.
        body: Vec<LoweredStmt>,
    },
}

/// One arm of a [`LoweredStmt::Case`]: label expressions and the body they select.
#[derive(Clone, Debug)]
pub struct LoweredCaseArm {
    /// Label expressions compared against the selector (any match selects this arm's body).
    pub labels: Vec<ExprId>,
    /// Statements executed when a label matches.
    pub body: Vec<LoweredStmt>,
}

/// One `idt` call site. Unlike `ddt`, whose result only ever needs to be *stamped* (the charge
/// channel encodes "this row's residual is the time-derivative of `expr`" without ever computing
/// an actual value for it), `idt(expr)`'s result is a genuine *value* every containing expression
/// needs to read — and, unlike `ddt`'s charge argument, this codegen has no way to recover
/// `expr`'s time-integral symbolically. So `idt` gets its own auxiliary "accumulator" unknown
/// `Y`, enforcing `ddt(Y) = expr` as a self-contained row exactly like a `ddt` charge term would
/// (see `crate::GeneratedModel::stamp_idt_accumulators`) — and `crate::ad::eval` reads `idt`'s
/// *value* as simply `Y`'s current value (see `crate::ad::Ctx::idt_slots`). Because the value is
/// just an ordinary unknown read, `idt` may appear anywhere in an expression, not just as a
/// top-level contribution term the way `ddt` must.
#[derive(Clone, Copy, Debug)]
pub struct IdtAccumulator {
    /// The `idt(...)` call's own `ExprId.0` — how `crate::ad::Ctx::idt_slots` maps a specific
    /// call site back to the unknown its value reads from (the same call written twice is two
    /// independent accumulators, exactly as two `ddt` calls on the same argument are).
    pub expr_id: u32,
    /// `idt`'s first argument — the quantity being integrated.
    pub arg: ExprId,
    /// Local terminal slot (past every node and [`BranchCurrent`] slot) allocated for this
    /// accumulator's own unknown.
    pub local_slot: usize,
}

/// A branch that is **purely flow-defined** (never receives a potential contribution — a
/// [`BranchCurrent`]-carrying branch already has a working auxiliary current unknown, see its
/// doc comment) but is *also* read via a bare `I(...)` probe somewhere in the module — real
/// models do this two ways: purely as a derived/diagnostic quantity read strictly *after* every
/// contribution to the branch (`asmhemt.va`'s `idisi = I(di,si);`, feeding only an `` `OPM ``
/// operating-point-report variable, never anything electrical), or genuinely
/// *self-referentially*, read *before* the contribution that defines it, to compute a value that
/// feeds back into that very contribution (`diode_basic.va`'s `Id = I(anode,cathode);`, used to
/// compute a series-resistance voltage drop that ultimately determines `Id` itself via
/// `Im`/`Qe`/`kfwd` — a real implicit equation, needing simultaneous, not sequential, resolution).
///
/// Ordinarily a flow contribution's value is computed and injected directly into its nodes'
/// KCL rows each time it runs, with nothing kept around to answer a later `I(...)` read (see
/// [`Contribution::branch_slot`]'s `None` case) — that's what makes a bare `I(...)` probe on such
/// a branch fail today. Both real shapes above are handled uniformly by giving the branch its
/// *own* auxiliary unknown too, exactly like [`BranchCurrent`]'s, but with the opposite defining
/// equation: instead of a constraint row forcing `V(p)-V(n)` to equal the contributed value (with
/// the unknown injected into the node KCL rows), this unknown's own row forces *itself* to equal
/// the branch's total **resistive** contribution (`crate::GeneratedModel::stamp_flow_current_accumulators`),
/// while the node KCL injection stays exactly as it already was — completely unaffected, since
/// this accumulator is a pure bookkeeping shadow of a value the branch's own contributions already
/// determine, not a new physical degree of freedom. Every `I(...)` read of this branch (before or
/// after its contribution, anywhere in the module) then simply reads this same unknown via the
/// *existing* flow-probe machinery (`crate::ad::Ctx::branch_current_slots`, populated with this
/// entry exactly like a `BranchCurrent`'s) — Newton resolves the self-referential case exactly
/// like it resolves any other implicit equation, with no special-casing needed at read sites.
///
/// **Limitation:** the defining equation only sums the branch's *resistive* contributions, not
/// any `ddt`/charge term also contributed to it — this project's DC solve already ignores the
/// charge channel entirely (`crate::lower`'s `ddt` handling), so this is consistent there, but a
/// transient run's `I(...)` read of a self-probed branch that also carries a `ddt` term (e.g.
/// `diode_basic.va`'s own `I(anode,cathode) <+ Im+(ddt(Qd));`) will not reflect that charge
/// current's contribution. No corpus file surveyed feeds such a probe back into anything
/// electrical, only into diagnostic output, so this was not worth the added complexity of a
/// second, charge-aware defining equation.
#[derive(Clone, Copy, Debug)]
pub struct FlowCurrentAccumulator {
    /// Which branch this accumulator shadows.
    pub branch: BranchId,
    /// Local terminal slot (past every node, [`BranchCurrent`], and [`IdtAccumulator`] slot)
    /// allocated for this accumulator's own unknown.
    pub local_slot: usize,
}

/// A branch that receives **no** contribution anywhere in the module, but is read via a bare
/// `I(...)` probe with one terminal being the module's implicit ground reference (the node a
/// single-terminal access creates — see `va-frontend::elaborate::Elaborator::reference_node`).
/// Unlike [`FlowCurrentAccumulator`] (a branch whose *own* contribution defines its current),
/// this branch has no contribution of its own to sum: its value can only be recovered from a
/// node-KCL sum at its non-ground terminal, over every *other* contributing branch that touches
/// that same node. `verilogaLib-master/ohmmeter.va` is the corpus case this exists for: its
/// `I(iprobe)` is a single-terminal probe of the branch `(iprobe, gnd)`, entirely distinct from
/// the branch `(dutm, iprobe)` that `V(dutm,iprobe) <+ 0;` actually contributes to — the two
/// share node `iprobe`, and KCL there is exactly what ties the probe's value to that other
/// branch's own current.
///
/// Every contributing branch touching the non-ground terminal already has its own current slot
/// (a [`BranchCurrent`] for a potential contribution) or is given one, forcing a new
/// [`FlowCurrentAccumulator`] into existence if it doesn't have one yet (a flow-only branch that
/// happens not to be independently `I(...)`-probed anywhere else). This accumulator's defining
/// equation is then purely linear in those slots — `Y = -(Σ ± other_branch_current)`, sign `+`
/// if the shared node is that other branch's own `p`, `-` if its `n`, the same convention every
/// branch's own node-KCL injection already uses (see `crate::GeneratedModel::stamp_node_kcl_probes`).
///
/// **Limitations:**
/// - Only a *single-terminal* (implicit-ground) probe is handled: a bare `I(a,b)` probe of an
///   uncontributed branch between two *other*, non-ground nodes is rejected rather than guessing
///   which terminal's local KCL sum to trust — no corpus file surveyed needs it.
/// - A touching branch that is itself a **mixed** [`BranchCurrent`] (`BranchCurrent::mixed`)
///   whose *flow* arm actually ran a given call reads back `0` here — the same pre-existing
///   character every other bare `I(...)` read of a mixed branch already has, since
///   `crate::GeneratedModel::finalize_mixed_branch_currents` pins that branch's auxiliary
///   current to `0` whenever its flow arm (which injects the real current directly into the node
///   KCL rows instead) ran instead of its potential arm.
#[derive(Clone, Debug)]
pub struct NodeKclProbe {
    /// The purely-probed branch itself (e.g. `(iprobe, gnd)`).
    pub branch: BranchId,
    /// Local terminal slot (past every other accumulator kind) allocated for this probe's own
    /// unknown — read exactly like any other branch current, via
    /// `crate::ad::Ctx::branch_current_slots`.
    pub local_slot: usize,
    /// Every other contributing branch touching the non-ground terminal: `(current_slot, sign)`,
    /// `sign` `+1.0` if the shared node is that branch's own `p`, `-1.0` if its `n`.
    pub terms: Vec<(usize, f64)>,
}

/// A lowered, evaluable representation of a module's analog block.
#[derive(Debug, Default)]
pub struct Lowered {
    /// Total number of local unknowns: one per IR node, plus one per entry in
    /// [`Self::branch_currents`], [`Self::idt_accumulators`], [`Self::flow_current_accumulators`],
    /// and [`Self::node_kcl_probes`].
    pub n_unknowns: usize,
    /// Statements in source order (assignments and contributions only — see Limitations).
    pub stmts: Vec<LoweredStmt>,
    /// One entry per branch that receives a potential contribution anywhere in the module, in
    /// ascending [`BranchId`] order (the deterministic order their local terminal slots are
    /// allocated in, past `module.nodes.len()`).
    pub branch_currents: Vec<BranchCurrent>,
    /// One entry per distinct `idt(...)` call site anywhere in the module, in the order
    /// encountered walking the analog block (the deterministic order their local terminal slots
    /// are allocated in, past every [`BranchCurrent`] slot).
    pub idt_accumulators: Vec<IdtAccumulator>,
    /// One entry per purely-flow-defined branch that is also read via a bare `I(...)` probe
    /// somewhere in the module, in ascending [`BranchId`] order (the deterministic order their
    /// local terminal slots are allocated in, past every [`IdtAccumulator`] slot) — plus any
    /// forced into existence solely to give a [`NodeKclProbe`] something to read (see its doc
    /// comment).
    pub flow_current_accumulators: Vec<FlowCurrentAccumulator>,
    /// One entry per purely-probed branch with no contribution anywhere, one terminal of which
    /// is the module's implicit ground reference (see [`NodeKclProbe`]), in ascending
    /// [`BranchId`] order, past every [`FlowCurrentAccumulator`] slot.
    pub node_kcl_probes: Vec<NodeKclProbe>,
}

/// Lower a module's analog block into a [`Lowered`] plan.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset
/// (user-defined functions, malformed `ddt`, or an `idt` called with other than one or two
/// arguments).
pub fn lower(module: &Module) -> Result<Lowered, CodegenError> {
    let (flow_branches, potential_branches) = branch_kinds(&module.analog);
    let param_only = param_only_vars(module, &module.analog);

    let mut branch_currents = Vec::new();
    let mut slot_of_branch = HashMap::new();
    let mut next_slot = module.nodes.len();
    for &id in &potential_branches {
        let br = module.branches[id as usize];
        slot_of_branch.insert(id, next_slot);
        branch_currents.push(BranchCurrent {
            branch: BranchId(id),
            p_slot: br.p.0 as usize,
            n_slot: br.n.0 as usize,
            local_slot: next_slot,
            mixed: flow_branches.contains(&id),
        });
        next_slot += 1;
    }

    let mut idt_calls = Vec::new();
    collect_idt_calls_in_stmts(module, &module.analog, &mut idt_calls);
    let mut idt_accumulators = Vec::new();
    let mut seen_idt = HashSet::new();
    for call in idt_calls {
        if !seen_idt.insert(call.0) {
            continue;
        }
        let Expr::Call(Builtin::Idt, args) = module.expr(call) else {
            unreachable!("collect_idt_calls_in_stmts only ever collects `Idt` call sites");
        };
        if args.is_empty() || args.len() > 2 {
            return Err(unsupported("idt expects one or two arguments"));
        }
        idt_accumulators.push(IdtAccumulator {
            expr_id: call.0,
            arg: args[0],
            local_slot: next_slot,
        });
        next_slot += 1;
    }

    let mut probed_flow_branches = BTreeSet::new();
    collect_flow_probe_branches_in_stmts(module, &module.analog, &mut probed_flow_branches);
    let mut flow_current_accumulators = Vec::new();
    for &id in &probed_flow_branches {
        // Only a *purely* flow-defined branch needs this: one with a potential contribution
        // anywhere already has a working `BranchCurrent` (see its doc comment), and a bare
        // `I(...)` probe already reads that just fine via `crate::ad::Ctx::branch_current_slots`.
        if flow_branches.contains(&id) && !potential_branches.contains(&id) {
            flow_current_accumulators.push(FlowCurrentAccumulator {
                branch: BranchId(id),
                local_slot: next_slot,
            });
            next_slot += 1;
        }
    }

    // A probed branch that receives *no* contribution anywhere (neither flow nor potential) —
    // `ohmmeter.va`'s `I(iprobe)` is exactly this — can't be resolved from its own contribution
    // at all; see `NodeKclProbe`'s doc comment for the node-KCL sum this falls back to.
    let ground = ground_node(module);
    let mut node_kcl_probes = Vec::new();
    for &id in &probed_flow_branches {
        if flow_branches.contains(&id) || potential_branches.contains(&id) {
            continue;
        }
        let br = module.branches[id as usize];
        let probe_node = match ground {
            Some(g) if br.p == g && br.n != g => br.n,
            Some(g) if br.n == g && br.p != g => br.p,
            _ => {
                return Err(unsupported(
                    "a bare `I(...)` probe of a branch that receives no contribution anywhere \
                     is only supported when one terminal is the module's implicit ground \
                     reference",
                ));
            }
        };

        let mut terms = Vec::new();
        for (bidx, other) in module.branches.iter().enumerate() {
            let bidx = bidx as u32;
            if bidx == id {
                continue;
            }
            let sign = if other.p == probe_node {
                1.0
            } else if other.n == probe_node {
                -1.0
            } else {
                continue;
            };
            if !flow_branches.contains(&bidx) && !potential_branches.contains(&bidx) {
                continue; // this branch contributes nothing; treat as zero.
            }
            let slot = if let Some(bc) = branch_currents.iter().find(|bc| bc.branch.0 == bidx) {
                bc.local_slot
            } else if let Some(acc) = flow_current_accumulators
                .iter()
                .find(|acc| acc.branch.0 == bidx)
            {
                acc.local_slot
            } else {
                let slot = next_slot;
                flow_current_accumulators.push(FlowCurrentAccumulator {
                    branch: BranchId(bidx),
                    local_slot: slot,
                });
                next_slot += 1;
                slot
            };
            terms.push((slot, sign));
        }

        node_kcl_probes.push(NodeKclProbe {
            branch: BranchId(id),
            local_slot: next_slot,
            terms,
        });
        next_slot += 1;
    }

    let mut stmts = Vec::new();
    let mut ddt_vars = HashMap::new();
    for stmt in &module.analog {
        lower_stmt(
            module,
            stmt,
            &slot_of_branch,
            &param_only,
            &mut ddt_vars,
            &mut stmts,
        )?;
    }
    Ok(Lowered {
        n_unknowns: next_slot,
        stmts,
        branch_currents,
        idt_accumulators,
        flow_current_accumulators,
        node_kcl_probes,
    })
}

/// The module's implicit global reference node, if a single-terminal access anywhere ever
/// created one (see `va-frontend::elaborate::Elaborator::reference_node`) — identified by name,
/// the same `"gnd"` sentinel convention `va-netlist` uses when wiring nodes across module
/// instances. `None` if the module never has one (no single-terminal access anywhere).
fn ground_node(module: &Module) -> Option<NodeId> {
    module
        .nodes
        .iter()
        .position(|n| n.name.eq_ignore_ascii_case("gnd"))
        .map(|i| NodeId(i as u32))
}

/// Collect every branch targeted by a bare `I(...)` (flow) probe reachable anywhere in `stmts`,
/// the same generic-expression-walk shape as [`collect_idt_calls_in_stmts`]/
/// [`collect_idt_calls_in_expr`] (a flow probe, like `idt`, may appear anywhere in an expression,
/// not just a top-level contribution term).
fn collect_flow_probe_branches_in_stmts(module: &Module, stmts: &[Stmt], out: &mut BTreeSet<u32>) {
    for stmt in stmts {
        collect_flow_probe_branches_in_stmt(module, stmt, out);
    }
}

fn collect_flow_probe_branches_in_stmt(module: &Module, stmt: &Stmt, out: &mut BTreeSet<u32>) {
    match stmt {
        Stmt::Contribute { value, .. } => collect_flow_probe_branches_in_expr(module, *value, out),
        Stmt::Assign { rhs, .. } => collect_flow_probe_branches_in_expr(module, *rhs, out),
        Stmt::Block(body) => collect_flow_probe_branches_in_stmts(module, body, out),
        Stmt::If { cond, then_, else_ } => {
            collect_flow_probe_branches_in_expr(module, *cond, out);
            collect_flow_probe_branches_in_stmts(module, then_, out);
            collect_flow_probe_branches_in_stmts(module, else_, out);
        }
        Stmt::While { cond, body } => {
            collect_flow_probe_branches_in_expr(module, *cond, out);
            collect_flow_probe_branches_in_stmts(module, body, out);
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            collect_flow_probe_branches_in_stmt(module, init, out);
            collect_flow_probe_branches_in_expr(module, *cond, out);
            collect_flow_probe_branches_in_stmt(module, step, out);
            collect_flow_probe_branches_in_stmts(module, body, out);
        }
        Stmt::Repeat { count, body } => {
            collect_flow_probe_branches_in_expr(module, *count, out);
            collect_flow_probe_branches_in_stmts(module, body, out);
        }
        Stmt::Case {
            selector,
            arms,
            default,
        } => {
            collect_flow_probe_branches_in_expr(module, *selector, out);
            for arm in arms {
                for &label in &arm.labels {
                    collect_flow_probe_branches_in_expr(module, label, out);
                }
                collect_flow_probe_branches_in_stmts(module, &arm.body, out);
            }
            collect_flow_probe_branches_in_stmts(module, default, out);
        }
    }
}

fn collect_flow_probe_branches_in_expr(module: &Module, expr: ExprId, out: &mut BTreeSet<u32>) {
    match module.expr(expr) {
        Expr::Probe(access) if access.kind == AccessKind::Flow => {
            out.insert(access.branch.0);
        }
        Expr::Const(_) | Expr::Param(_) | Expr::Var(_) | Expr::Probe(_) => {}
        Expr::Unary(_, e) => collect_flow_probe_branches_in_expr(module, *e, out),
        Expr::Binary(_, l, r) => {
            collect_flow_probe_branches_in_expr(module, *l, out);
            collect_flow_probe_branches_in_expr(module, *r, out);
        }
        Expr::Call(_, args) | Expr::CallUser(_, args) => {
            for &a in args {
                collect_flow_probe_branches_in_expr(module, a, out);
            }
        }
        Expr::Select(c, t, e) => {
            collect_flow_probe_branches_in_expr(module, *c, out);
            collect_flow_probe_branches_in_expr(module, *t, out);
            collect_flow_probe_branches_in_expr(module, *e, out);
        }
        Expr::Ddx(e, _) => collect_flow_probe_branches_in_expr(module, *e, out),
    }
}

/// Collect every `idt(...)` call site reachable anywhere in `stmts`, in source order (a given
/// call may be pushed more than once if it's somehow reachable via more than one path — callers
/// dedupe by `ExprId`). Recurses into every nested construct `lower_stmt` itself recurses through,
/// the same shape as [`collect_branch_kinds`]/[`collect_assigns`].
fn collect_idt_calls_in_stmts(module: &Module, stmts: &[Stmt], out: &mut Vec<ExprId>) {
    for stmt in stmts {
        collect_idt_calls_in_stmt(module, stmt, out);
    }
}

fn collect_idt_calls_in_stmt(module: &Module, stmt: &Stmt, out: &mut Vec<ExprId>) {
    match stmt {
        Stmt::Contribute { value, .. } => collect_idt_calls_in_expr(module, *value, out),
        Stmt::Assign { rhs, .. } => collect_idt_calls_in_expr(module, *rhs, out),
        Stmt::Block(body) => collect_idt_calls_in_stmts(module, body, out),
        Stmt::If { cond, then_, else_ } => {
            collect_idt_calls_in_expr(module, *cond, out);
            collect_idt_calls_in_stmts(module, then_, out);
            collect_idt_calls_in_stmts(module, else_, out);
        }
        Stmt::While { cond, body } => {
            collect_idt_calls_in_expr(module, *cond, out);
            collect_idt_calls_in_stmts(module, body, out);
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            collect_idt_calls_in_stmt(module, init, out);
            collect_idt_calls_in_expr(module, *cond, out);
            collect_idt_calls_in_stmt(module, step, out);
            collect_idt_calls_in_stmts(module, body, out);
        }
        Stmt::Repeat { count, body } => {
            collect_idt_calls_in_expr(module, *count, out);
            collect_idt_calls_in_stmts(module, body, out);
        }
        Stmt::Case {
            selector,
            arms,
            default,
        } => {
            collect_idt_calls_in_expr(module, *selector, out);
            for arm in arms {
                for &label in &arm.labels {
                    collect_idt_calls_in_expr(module, label, out);
                }
                collect_idt_calls_in_stmts(module, &arm.body, out);
            }
            collect_idt_calls_in_stmts(module, default, out);
        }
    }
}

/// Walk every sub-expression of `expr` looking for an `idt(...)` call — unlike `ddt`, which is
/// only ever recognized in the specific top-level-additive-term shapes [`charge_term_shape`]
/// inspects, `idt` may appear anywhere at all (see [`IdtAccumulator`]'s doc comment), so this
/// visits every `Expr` variant's sub-expressions generically rather than following a specific
/// contribution shape.
fn collect_idt_calls_in_expr(module: &Module, expr: ExprId, out: &mut Vec<ExprId>) {
    match module.expr(expr) {
        Expr::Call(Builtin::Idt, args) => {
            out.push(expr);
            for &a in args {
                collect_idt_calls_in_expr(module, a, out);
            }
        }
        Expr::Const(_) | Expr::Param(_) | Expr::Var(_) | Expr::Probe(_) => {}
        Expr::Unary(_, e) => collect_idt_calls_in_expr(module, *e, out),
        Expr::Binary(_, l, r) => {
            collect_idt_calls_in_expr(module, *l, out);
            collect_idt_calls_in_expr(module, *r, out);
        }
        Expr::Call(_, args) | Expr::CallUser(_, args) => {
            for &a in args {
                collect_idt_calls_in_expr(module, a, out);
            }
        }
        Expr::Select(c, t, e) => {
            collect_idt_calls_in_expr(module, *c, out);
            collect_idt_calls_in_expr(module, *t, out);
            collect_idt_calls_in_expr(module, *e, out);
        }
        Expr::Ddx(e, _) => collect_idt_calls_in_expr(module, *e, out),
    }
}

/// Collect the set of branch IDs targeted by a flow contribution and the set targeted by a
/// potential contribution, anywhere in `stmts` (recursing into every nested construct —
/// `if`/`else`, `case`, loop bodies/init/step, blocks — the same shapes `lower_stmt` itself
/// recurses through).
fn branch_kinds(stmts: &[Stmt]) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut flow = BTreeSet::new();
    let mut potential = BTreeSet::new();
    collect_branch_kinds(stmts, &mut flow, &mut potential);
    (flow, potential)
}

fn collect_branch_kinds(stmts: &[Stmt], flow: &mut BTreeSet<u32>, potential: &mut BTreeSet<u32>) {
    for stmt in stmts {
        collect_branch_kinds_one(stmt, flow, potential);
    }
}

fn collect_branch_kinds_one(stmt: &Stmt, flow: &mut BTreeSet<u32>, potential: &mut BTreeSet<u32>) {
    match stmt {
        Stmt::Contribute { target, .. } => match target.kind {
            AccessKind::Flow => {
                flow.insert(target.branch.0);
            }
            AccessKind::Potential => {
                potential.insert(target.branch.0);
            }
        },
        Stmt::Block(body) => collect_branch_kinds(body, flow, potential),
        Stmt::If { then_, else_, .. } => {
            collect_branch_kinds(then_, flow, potential);
            collect_branch_kinds(else_, flow, potential);
        }
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } => {
            collect_branch_kinds(body, flow, potential);
        }
        Stmt::For {
            init, step, body, ..
        } => {
            collect_branch_kinds_one(init, flow, potential);
            collect_branch_kinds_one(step, flow, potential);
            collect_branch_kinds(body, flow, potential);
        }
        Stmt::Case { arms, default, .. } => {
            for arm in arms {
                collect_branch_kinds(&arm.body, flow, potential);
            }
            collect_branch_kinds(default, flow, potential);
        }
        Stmt::Assign { .. } => {}
    }
}

/// `ddt_vars` maps a local variable (`VarId.0`) to the RHS expression of its most recent
/// `Stmt::Assign` in the current straight-line scope, *only* when that RHS is itself a
/// recognized (possibly coefficient-scaled) `ddt` shape (see [`charge_term_shape`]) — i.e. the
/// "`` real dqdt; dqdt = ddt(q); I <+ dqdt + …; ``" indirection this module's doc comment
/// documents as a limitation, now handled for the specific shape real models use it in: an
/// unconditional assignment read back later inside a `<+`. Forked (cloned) on entry to any
/// branch/loop body and never merged back — see [`lower_stmt`]'s `Stmt::If`/`Stmt::While`/etc.
/// arms — so a reassignment made only *inside* a branch never leaks a false definition to code
/// after it; this is a sound, path-insensitive-in-the-conservative-direction restriction, not a
/// full reaching-definitions analysis. A variable assigned anything else invalidates (removes)
/// any prior entry, so a variable that's ever reused for an ordinary value (as `T0` commonly is
/// in real models, e.g. `angelov_gan.va`) only resolves through this map at the specific
/// contribution sites that run after its most recent assignment was a `ddt` shape.
type DdtVars = HashMap<u32, ExprId>;

fn lower_stmt(
    module: &Module,
    stmt: &Stmt,
    slot_of_branch: &HashMap<u32, usize>,
    param_only: &HashSet<u32>,
    ddt_vars: &mut DdtVars,
    out: &mut Vec<LoweredStmt>,
) -> Result<(), CodegenError> {
    match stmt {
        Stmt::Contribute { target, value } => {
            let br = module.branches[target.branch.0 as usize];

            let mut terms = Vec::new();
            collect_terms(module, *value, 1.0, &mut terms);

            let mut resistive = Vec::new();
            let mut charge = Vec::new();
            for term in terms {
                // A bare variable read that was last assigned a `ddt` shape substitutes to that
                // shape here, so `real dqdt; dqdt = ddt(q); I <+ dqdt + …;` folds into the charge
                // channel exactly as `I <+ ddt(q) + …;` would.
                let shape_expr = match module.expr(term.expr) {
                    Expr::Var(id) => ddt_vars.get(&id.0).copied().unwrap_or(term.expr),
                    _ => term.expr,
                };
                match charge_term_shape(module, shape_expr, param_only)? {
                    Some((expr, coeffs)) => charge.push(ChargeTerm {
                        sign: term.sign,
                        expr,
                        coeffs,
                    }),
                    None => resistive.push(term),
                }
            }

            let branch_slot = match target.kind {
                AccessKind::Flow => None,
                AccessKind::Potential => Some(slot_of_branch[&target.branch.0]),
            };

            out.push(LoweredStmt::Contribute(Contribution {
                branch: target.branch,
                p_slot: br.p.0 as usize,
                n_slot: br.n.0 as usize,
                branch_slot,
                resistive,
                charge,
            }));
            Ok(())
        }
        Stmt::Assign { lhs, rhs } => {
            // A `ddt`-shape RHS never becomes a `LoweredStmt::Assign`: this project has no way
            // to evaluate a bare `ddt(...)` as an ordinary value (that's exactly why it normally
            // must be a top-level contribution term — see this module's doc comment), so the
            // assignment is tracked symbolically in `ddt_vars` instead and resolved at whatever
            // later contribution reads it (see the `Stmt::Contribute` arm above). Any other RHS
            // invalidates a stale entry, so a variable reused for an ordinary value afterward is
            // read normally, not substituted.
            match charge_term_shape(module, *rhs, param_only)? {
                Some(_) => {
                    ddt_vars.insert(lhs.0, *rhs);
                }
                None => {
                    ddt_vars.remove(&lhs.0);
                    out.push(LoweredStmt::Assign {
                        lhs: *lhs,
                        rhs: *rhs,
                    });
                }
            }
            Ok(())
        }
        Stmt::Block(body) => {
            for s in body {
                lower_stmt(module, s, slot_of_branch, param_only, ddt_vars, out)?;
            }
            Ok(())
        }
        Stmt::If { cond, then_, else_ } => {
            let mut then_lowered = Vec::new();
            let mut then_ddt_vars = ddt_vars.clone();
            for s in then_ {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut then_ddt_vars,
                    &mut then_lowered,
                )?;
            }
            let mut else_lowered = Vec::new();
            let mut else_ddt_vars = ddt_vars.clone();
            for s in else_ {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut else_ddt_vars,
                    &mut else_lowered,
                )?;
            }
            // Neither arm's own reassignments (of a variable pre-existing before the `if`, or a
            // brand-new one local to just one arm) are known to hold after the `if` — which arm
            // ran isn't known here — so forget any variable either arm assigned at all, in the
            // *outer* map that carries forward past this construct (see `DdtVars`'s doc comment).
            invalidate_ddt_vars(ddt_vars, then_);
            invalidate_ddt_vars(ddt_vars, else_);
            out.push(LoweredStmt::If {
                cond: *cond,
                then_: then_lowered,
                else_: else_lowered,
            });
            Ok(())
        }
        Stmt::While { cond, body } => {
            let mut body_lowered = Vec::new();
            let mut body_ddt_vars = ddt_vars.clone();
            for s in body {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut body_ddt_vars,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(ddt_vars, body);
            out.push(LoweredStmt::While {
                cond: *cond,
                body: body_lowered,
            });
            Ok(())
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            let mut loop_ddt_vars = ddt_vars.clone();
            let mut init_lowered = Vec::new();
            lower_stmt(
                module,
                init,
                slot_of_branch,
                param_only,
                &mut loop_ddt_vars,
                &mut init_lowered,
            )?;
            let mut step_lowered = Vec::new();
            lower_stmt(
                module,
                step,
                slot_of_branch,
                param_only,
                &mut loop_ddt_vars,
                &mut step_lowered,
            )?;
            let mut body_lowered = Vec::new();
            for s in body {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut loop_ddt_vars,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(ddt_vars, std::slice::from_ref(&**init));
            invalidate_ddt_vars(ddt_vars, std::slice::from_ref(&**step));
            invalidate_ddt_vars(ddt_vars, body);
            out.push(LoweredStmt::For {
                init: init_lowered,
                cond: *cond,
                step: step_lowered,
                body: body_lowered,
            });
            Ok(())
        }
        Stmt::Repeat { count, body } => {
            let mut body_lowered = Vec::new();
            let mut body_ddt_vars = ddt_vars.clone();
            for s in body {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut body_ddt_vars,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(ddt_vars, body);
            out.push(LoweredStmt::Repeat {
                count: *count,
                body: body_lowered,
            });
            Ok(())
        }
        Stmt::Case {
            selector,
            arms,
            default,
        } => {
            // Every arm (and `default`) is a mutually exclusive alternative to every other, so
            // each must be lowered from the *same* pre-`case` snapshot — not one another's
            // possibly-already-invalidated state — hence cloning from `ddt_vars` up front for
            // every arm before any of them invalidates anything in it.
            let mut lowered_arms = Vec::new();
            for arm in arms {
                let mut body_lowered = Vec::new();
                let mut arm_ddt_vars = ddt_vars.clone();
                for s in &arm.body {
                    lower_stmt(
                        module,
                        s,
                        slot_of_branch,
                        param_only,
                        &mut arm_ddt_vars,
                        &mut body_lowered,
                    )?;
                }
                lowered_arms.push(LoweredCaseArm {
                    labels: arm.labels.clone(),
                    body: body_lowered,
                });
            }
            let mut default_lowered = Vec::new();
            let mut default_ddt_vars = ddt_vars.clone();
            for s in default {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    &mut default_ddt_vars,
                    &mut default_lowered,
                )?;
            }
            for arm in arms {
                invalidate_ddt_vars(ddt_vars, &arm.body);
            }
            invalidate_ddt_vars(ddt_vars, default);
            out.push(LoweredStmt::Case {
                selector: *selector,
                arms: lowered_arms,
                default: default_lowered,
            });
            Ok(())
        }
    }
}

/// Flatten an expression into signed additive terms, pushing `-` through subtraction and
/// unary negation so that top-level `ddt` terms become visible for the charge/resistive split.
fn collect_terms(module: &Module, expr: ExprId, sign: f64, out: &mut Vec<Term>) {
    match module.expr(expr) {
        Expr::Binary(BinOp::Add, l, r) => {
            collect_terms(module, *l, sign, out);
            collect_terms(module, *r, sign, out);
        }
        Expr::Binary(BinOp::Sub, l, r) => {
            collect_terms(module, *l, sign, out);
            collect_terms(module, *r, -sign, out);
        }
        Expr::Unary(UnOp::Neg, e) => {
            collect_terms(module, *e, -sign, out);
        }
        _ => out.push(Term { sign, expr }),
    }
}

/// A recognized `ddt` charge shape: the `ddt` call's own argument, plus every parameter-only
/// scaling factor wrapping it, each paired with whether it divides (`true`) rather than
/// multiplies (`false`). See [`charge_term_shape`].
type ChargeShape = (ExprId, Vec<(ExprId, bool)>);

/// If `expr` is `ddt(arg)` wrapped in any depth of parameter-only multiplication/division
/// (`ddt(arg)`, `coeff*ddt(arg)`, `ddt(arg)*coeff`, `ddt(arg)/coeff`, `(ddt(arg)*coeff1)*coeff2`,
/// `coeff1*coeff2*ddt(arg)`, ... — real models nest at least two multiplications deep, e.g.
/// `ekv26.va`'s `ddt(qjd)*TYPE*M`, parsing as `(ddt(qjd)*TYPE)*M`), return `(arg, coeffs)` where
/// `coeffs` lists every scaling factor found (in the order encountered), each paired with
/// whether it divides (`true`) rather than multiplies (`false`). Returns `Ok(None)` for anything
/// else — including a syntactically-plausible `coeff*ddt(arg)` whose `coeff` fails the
/// parameter-only check ([`is_param_only`] given `param_only`), which falls back to being
/// treated as an ordinary resistive term (and is rejected later, when `ad::eval` actually tries
/// to evaluate the still-nested `ddt` call, by the same `CodegenError::Unsupported` this
/// returned `None` to avoid pre-empting here).
fn charge_term_shape(
    module: &Module,
    expr: ExprId,
    param_only: &HashSet<u32>,
) -> Result<Option<ChargeShape>, CodegenError> {
    if let Some(arg) = ddt_arg(module, expr)? {
        return Ok(Some((arg, Vec::new())));
    }
    match module.expr(expr) {
        Expr::Binary(BinOp::Mul, l, r) => {
            if let Some((arg, mut coeffs)) = charge_term_shape(module, *l, param_only)? {
                if is_param_only(module, *r, param_only) {
                    coeffs.push((*r, false));
                    return Ok(Some((arg, coeffs)));
                }
            }
            if let Some((arg, mut coeffs)) = charge_term_shape(module, *r, param_only)? {
                if is_param_only(module, *l, param_only) {
                    coeffs.push((*l, false));
                    return Ok(Some((arg, coeffs)));
                }
            }
            Ok(None)
        }
        Expr::Binary(BinOp::Div, l, r) => {
            if let Some((arg, mut coeffs)) = charge_term_shape(module, *l, param_only)? {
                if is_param_only(module, *r, param_only) {
                    coeffs.push((*r, true));
                    return Ok(Some((arg, coeffs)));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// `Ok(Some(arg))` if `expr` is `ddt(arg)`; `Ok(None)` if `expr` isn't a `ddt` call at all;
/// `Err` if it is one but with the wrong argument count.
fn ddt_arg(module: &Module, expr: ExprId) -> Result<Option<ExprId>, CodegenError> {
    match module.expr(expr) {
        Expr::Call(Builtin::Ddt, args) if args.len() == 1 => Ok(Some(args[0])),
        Expr::Call(Builtin::Ddt, _) => Err(unsupported("ddt expects exactly one argument")),
        _ => Ok(None),
    }
}

/// Whether `expr` is provably independent of every unknown (node voltage, branch current, and
/// local variable) — built from nothing but `Const`/`Param`, pure arithmetic/builtin
/// combinations of those, and a local variable (by `VarId.0`) present in `param_only` (see
/// [`param_only_vars`]). See this module's doc comment for why [`charge_term_shape`] requires
/// this of a `ddt` scaling coefficient.
fn is_param_only(module: &Module, expr: ExprId, param_only: &HashSet<u32>) -> bool {
    match module.expr(expr) {
        Expr::Const(_) | Expr::Param(_) => true,
        Expr::Var(id) => param_only.contains(&id.0),
        Expr::Unary(_, e) => is_param_only(module, *e, param_only),
        Expr::Binary(_, l, r) => {
            is_param_only(module, *l, param_only) && is_param_only(module, *r, param_only)
        }
        Expr::Call(builtin, args) => {
            !matches!(builtin, Builtin::Ddt | Builtin::Idt)
                && args.iter().all(|&a| is_param_only(module, a, param_only))
        }
        _ => false,
    }
}

/// Compute the set of local variables (by `VarId.0`) that are *provably* parameter-only: every
/// `Stmt::Assign` to them anywhere in `stmts` (recursing into every nested construct, same as
/// [`collect_branch_kinds`]) assigns a [`is_param_only`] expression, checked to a fixed point so
/// a short dependency chain (`a=W/L; b=a*2;`) is still recognised — a variable only counts once
/// every variable *it* depends on has already been confirmed. See this module's doc comment for
/// why this is a sound but incomplete (non-path-sensitive) over-approximation.
fn param_only_vars(module: &Module, stmts: &[Stmt]) -> HashSet<u32> {
    let mut assigns = Vec::new();
    collect_assigns(stmts, &mut assigns);
    let assigned_vars: BTreeSet<u32> = assigns.iter().map(|&(v, _)| v).collect();

    let mut known: HashSet<u32> = HashSet::new();
    loop {
        let mut changed = false;
        for &var in &assigned_vars {
            if known.contains(&var) {
                continue;
            }
            let all_param_only = assigns
                .iter()
                .filter(|&&(v, _)| v == var)
                .all(|&(_, rhs)| is_param_only(module, rhs, &known));
            if all_param_only {
                known.insert(var);
                changed = true;
            }
        }
        if !changed {
            return known;
        }
    }
}

fn collect_assigns(stmts: &[Stmt], out: &mut Vec<(u32, ExprId)>) {
    for stmt in stmts {
        collect_assigns_one(stmt, out);
    }
}

/// Remove from `ddt_vars` every variable assigned anywhere in `stmts` — used when a branch/loop
/// body finishes lowering, so a `ddt`-shape substitution recorded (or overwritten) only inside
/// that body never carries forward past it (see [`DdtVars`]'s doc comment for why this can't
/// simply merge the body's own final state back instead).
fn invalidate_ddt_vars(ddt_vars: &mut DdtVars, stmts: &[Stmt]) {
    let mut assigns = Vec::new();
    collect_assigns(stmts, &mut assigns);
    for (var, _) in assigns {
        ddt_vars.remove(&var);
    }
}

fn collect_assigns_one(stmt: &Stmt, out: &mut Vec<(u32, ExprId)>) {
    match stmt {
        Stmt::Assign { lhs, rhs } => out.push((lhs.0, *rhs)),
        Stmt::Block(body) => collect_assigns(body, out),
        Stmt::If { then_, else_, .. } => {
            collect_assigns(then_, out);
            collect_assigns(else_, out);
        }
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } => collect_assigns(body, out),
        Stmt::For {
            init, step, body, ..
        } => {
            collect_assigns_one(init, out);
            collect_assigns_one(step, out);
            collect_assigns(body, out);
        }
        Stmt::Case { arms, default, .. } => {
            for arm in arms {
                collect_assigns(&arm.body, out);
            }
            collect_assigns(default, out);
        }
        Stmt::Contribute { .. } => {}
    }
}

fn unsupported(msg: &str) -> CodegenError {
    CodegenError::Unsupported(msg.to_string())
}
