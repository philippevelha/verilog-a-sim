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
//! `(ddt(qjd)*TYPE)*M`, two levels deep, and needed exactly this. It also **distributes a
//! coefficient over a parenthesised sum of `ddt`s** — `ekv3.va` writes its gate charge as
//! `I(d,g) <+ SIGN_M * (d_gt_s*ddt(QD) + s_gt_d*ddt(QS)) * QON;`, which [`collect_terms`] cannot
//! reach because the sum is nested inside the scaling, not at the top of the contribution. A sum
//! distributes only when *every* one of its terms is itself a charge shape; a mixed
//! `(resistive + ddt(q))*coeff` is left alone, since splitting it would mean synthesising a new
//! `resistive*coeff` expression and this module only reads the IR arena. `ddt` still may not
//! appear nested any *other* way (inside a ternary, as another builtin's argument, etc.) — none
//! of those shapes turned up anywhere in the same survey, so there was nothing concrete to scope
//! a fix against.
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
//! a stale or wrong definition leak to code after the branch.
//!
//! Discarding the clone is sound for *values*, but it used to lose a **charge term** without
//! saying so. `hicumL0_v2p1p0.va` writes its self-heating capacitance as
//! `if (...) I_cth = 0.0; else I_cth = ddt(cth*V(br_sht));` followed by a *separate* `if` whose
//! arm contributes `I(br_sht) <+ I_cth;`. By then the `ddt` binding is gone, so the read lowered
//! as an ordinary resistive term — and it *compiled*, because the other arm's `I_cth = 0.0` had
//! emitted a real assignment. The device's entire thermal capacitance vanished silently. So
//! `invalidate_ddt_vars` now records which variables lost a `ddt` shape (in a set that is
//! deliberately **not** cloned per branch, unlike `DdtVars` itself, so the mark survives
//! outward), an ordinary assignment clears the mark, and a `<+` that reads a still-marked
//! variable is **rejected**. Guessing a missing reactive term is not an option, and neither is
//! dropping one quietly. A variable resolved this way is
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
use va_abi::noise::TableInterp;
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

/// One additive **noise-channel** term: a `white_noise(pwr)` or `flicker_noise(pwr, exp)` call
/// appearing as a top-level additive term of a contribution (T5.2).
///
/// Split out of the resistive channel for exactly the reason [`ChargeTerm`] is: the containing
/// `<+` carries two different physical statements at once, and only one of them belongs in the
/// residual. A noise call's *value* is zero in every analysis except noise (LRM §4.5.13), so
/// leaving it in the resistive channel would be harmless-but-pointless arithmetic; pulling it
/// out is what lets `crate::GeneratedModel::noise` find the arguments at all.
///
/// Unlike a `ChargeTerm`, no `sign` is recorded: a noise source's contribution to the output is
/// weighted by `|Z|²`, so the sign of the term it was written with cannot affect any result
/// (§ `va_acnoise::noise`). Nor are scaling coefficients flattened out the way `ChargeTerm`
/// does — a scaled `2*white_noise(p)` is not a recognized shape (see [`noise_term_shape`]),
/// because the scaling would have to be squared to be applied correctly and silently getting
/// that wrong is worse than rejecting it.
#[derive(Clone, Copy, Debug)]
pub enum NoiseTerm {
    /// `white_noise(pwr)` — the PSD expression.
    White {
        /// The power-spectral-density argument (A²/Hz for a flow contribution).
        pwr: ExprId,
    },
    /// `flicker_noise(pwr, exp)` — the PSD numerator and the frequency exponent.
    Flicker {
        /// The numerator, including any bias dependence the model wrote.
        pwr: ExprId,
        /// The frequency exponent (`1.0` for textbook `1/f`).
        exp: ExprId,
    },
    /// `noise_table({f1, p1, …})`/`noise_table_log(…)` — a tabulated PSD.
    ///
    /// Carries the **call expression itself** rather than an owned list of pairs, so a
    /// `NoiseTerm` stays `Copy` however long the table is: the pairs already live in the module's
    /// expression arena (as alternating `Const` arguments, § `va_ir::Builtin::NoiseTable`), and
    /// re-reading them there at emit time costs one arena index.
    Table {
        /// The `Expr::Call(Builtin::NoiseTable | Builtin::NoiseTableLog, …)` node holding the
        /// flattened pairs.
        call: ExprId,
        /// Which of the LRM's two interpolation rules applies — the *only* difference between
        /// the two builtins, resolved here so nothing downstream has to re-inspect the call.
        interp: TableInterp,
    },
}

/// One `ac_stim(...)` call pulled out of a contribution — a small-signal excitation on the
/// right-hand side of `(G + jω·C)·X = B`, never a term in `G`.
///
/// Split out for the same reason [`NoiseTerm`] is: the call's *value* is zero in every analysis
/// (§ `va_ir::Builtin::AcStim`), so unless its arguments are captured here, nothing downstream
/// could ever recover them. Unlike a noise term it carries a [`Self::sign`], because an
/// excitation combines **linearly** — `−ac_stim(1,0)` is a genuinely opposite stimulus, whereas
/// a noise source's sign is squared away by `|Z|²`.
#[derive(Clone, Copy, Debug)]
pub struct AcStimTerm {
    /// `+1.0`/`−1.0` from the enclosing expression's sums and negations.
    pub sign: f64,
    /// Which analyses this stimulus is active in, as a bitmask over `va_ir::ANALYSIS_PHASES`
    /// (`"ac"` unless the source named something else). Read from the call's already-folded
    /// constant first argument at lowering time, so nothing downstream re-inspects the arena.
    pub phase_mask: u32,
    /// The magnitude argument. Evaluated at the operating point, so a bias-dependent magnitude
    /// comes out right; only its value is used, never its gradient — a stimulus is an
    /// independent quantity, not a function of the solution vector.
    pub mag: ExprId,
    /// The phase argument, in **radians**.
    pub phase: ExprId,
}

/// One `laplace_*` call pulled out of a contribution — a rational transfer function
/// `H(s) = N(s)/D(s)` applied to `input` (Tier C, 2026-08-07).
///
/// The fourth construct to be split out of a contribution rather than evaluated in place, after
/// `ddt` (charge), the noise family, and `ac_stim` — and for a reason peculiar to this one:
/// `H(jω)` is **complex**, and `crate::ad::Dual` carries a real value and a real gradient. There
/// is nowhere in an ordinary expression evaluation to put an imaginary part. Splitting it out
/// lets `crate::GeneratedModel::stamp` place the real part in `G` and the imaginary part in `C`,
/// where the assembled `G + jω·C` reconstitutes it exactly.
#[derive(Clone, Debug)]
pub struct LaplaceTerm {
    /// `+1.0`/`−1.0` from the enclosing expression's sums and negations.
    pub sign: f64,
    /// The filtered input.
    pub input: ExprId,
    /// Whether the numerator list is `(re, im)` **roots** rather than polynomial coefficients.
    pub num_is_roots: bool,
    /// Whether the denominator list is roots.
    pub den_is_roots: bool,
    /// Numerator coefficients (lowest degree first) or flattened `(re, im)` zero pairs.
    pub num: Vec<ExprId>,
    /// Denominator coefficients or flattened `(re, im)` pole pairs.
    pub den: Vec<ExprId>,
}

/// If `expr` is a bare `laplace_*` call, unpack its flattened argument layout
/// (`[input, Const(num_len), num…, den…]`) into a [`LaplaceTerm`] shape.
///
/// Recognizes only the **bare** call, like every other split-out construct. A nested or scaled
/// one is rejected by `crate::GeneratedModel::validate` rather than silently evaluating —
/// `crate::ad::eval` has no real-valued answer for a complex gain, so letting one through would
/// be a hard error at the worst possible moment instead of a build-time diagnostic.
fn laplace_term_shape(
    module: &Module,
    expr: ExprId,
    sign: f64,
) -> Result<Option<LaplaceTerm>, CodegenError> {
    let (builtin, args) = match module.expr(expr) {
        Expr::Call(
            b @ (Builtin::LaplaceNd | Builtin::LaplaceNp | Builtin::LaplaceZd | Builtin::LaplaceZp),
            args,
        ) => (*b, args),
        _ => return Ok(None),
    };
    let num_is_roots = matches!(builtin, Builtin::LaplaceZd | Builtin::LaplaceZp);
    let den_is_roots = matches!(builtin, Builtin::LaplaceNp | Builtin::LaplaceZp);

    // `[input, Const(num_len), …]` — the separator is a `Const` by construction (va-frontend
    // emits it), so anything else means the IR was built by hand.
    let (Some(&input), Some(&len_id)) = (args.first(), args.get(1)) else {
        return Err(unsupported("laplace call is missing its argument list"));
    };
    let Expr::Const(num_len) = module.expr(len_id) else {
        return Err(unsupported(
            "laplace call's numerator-length separator must be a constant",
        ));
    };
    let num_len = *num_len as usize;
    let rest = &args[2..];
    if num_len > rest.len() {
        return Err(unsupported(
            "laplace numerator length exceeds its argument list",
        ));
    }
    let (num, den) = rest.split_at(num_len);
    if num.is_empty() || den.is_empty() {
        return Err(unsupported(
            "laplace needs at least one numerator and one denominator entry",
        ));
    }
    // A root list is flattened `(re, im)` pairs, so an odd count means a malformed IR.
    if (num_is_roots && num.len() % 2 != 0) || (den_is_roots && den.len() % 2 != 0) {
        return Err(unsupported(
            "a laplace zero/pole list must be an even number of (re, im) values",
        ));
    }
    Ok(Some(LaplaceTerm {
        sign,
        input,
        num_is_roots,
        den_is_roots,
        num: num.to_vec(),
        den: den.to_vec(),
    }))
}

/// Whether `expr` contains a `laplace_*` call anywhere in its tree — used to reject one buried
/// where [`laplace_term_shape`] cannot pull it out, the exact counterpart of
/// [`contains_noise_call`].
pub(crate) fn contains_laplace_call(module: &Module, expr: ExprId) -> bool {
    match module.expr(expr) {
        Expr::Call(
            Builtin::LaplaceNd | Builtin::LaplaceNp | Builtin::LaplaceZd | Builtin::LaplaceZp,
            _,
        ) => true,
        Expr::Call(_, args) => args.iter().any(|&a| contains_laplace_call(module, a)),
        Expr::Unary(_, e) => contains_laplace_call(module, *e),
        Expr::Binary(_, l, r) => {
            contains_laplace_call(module, *l) || contains_laplace_call(module, *r)
        }
        _ => false,
    }
}

/// A single branch contribution, split into resistive, charge, noise, and excitation channels.
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
    /// `white_noise`/`flicker_noise` terms emitted into the noise channel (T5.2). Empty for the
    /// overwhelming majority of contributions.
    pub noise: Vec<NoiseTerm>,
    /// `ac_stim` terms emitted into the excitation channel during AC analysis. Empty for the
    /// overwhelming majority of contributions.
    pub ac_stim: Vec<AcStimTerm>,
    /// `laplace_*` transfer functions applied to their inputs. Empty for the overwhelming
    /// majority of contributions.
    pub laplace: Vec<LaplaceTerm>,
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
    /// `bound_step(max_step);` — an upper bound on the next transient timestep, emitted into
    /// `va_abi::StampSink`'s bound-step channel when a transient run evaluates it and dropped
    /// in every other analysis (there is no timestep to bound).
    ///
    /// Kept as a statement through lowering rather than hoisted to a module-level property
    /// because it may sit inside an `if`: whether the bound applies can depend on the operating
    /// point, so it has to be reached by the same control-flow walk everything else is.
    BoundStep(ExprId),
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
    /// One entry per `transition`/`slew` call site, in ascending [`ExprId`] order — see
    /// [`StatefulCall`]. Maps a call site to its base offset in Interface β's per-instance
    /// state channel (`va_abi::ModelState`).
    pub stateful_calls: Vec<StatefulCall>,
    /// Whether any contribution carries a [`LaplaceTerm`] — what
    /// `crate::GeneratedModel::is_frequency_dependent` reports, and hence whether an AC sweep
    /// pays for per-frequency re-linearization.
    pub has_laplace: bool,
    /// Total `f64` slots this model needs on the state channel — the sum of every
    /// [`StatefulCall`]'s width, and what `crate::GeneratedModel::state_len` reports.
    pub state_len: usize,
}

/// Which time-domain construct a [`StatefulCall`] is, and how many state slots it needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatefulKind {
    /// `slew(value, pos_rate, neg_rate)` — 2 slots: `(t_prev, y_prev)`.
    Slew,
    /// `transition(value, delay, rise, fall)` — 5 slots:
    /// `(t_prev, y_prev, target, rate, t_start)`.
    Transition,
    /// `ddt(q)` — 2 slots: `(q_prev, rate_prev)`, the charge and its rate as of the last
    /// accepted timepoint.
    ///
    /// Allocated for **every** `ddt` site, including those whose contribution is stamped through
    /// the charge channel and therefore never needs a numeric rate. The slots are cheap (two
    /// `f64`), and making the allocation unconditional keeps it independent of whether
    /// `charge_term_shape` happened to recognise a given site — a coupling that would otherwise
    /// silently change a model's `state_len` when an unrelated recognizer widened.
    Ddt,
}

impl StatefulKind {
    /// Slots this kind occupies on the state channel.
    pub fn width(self) -> usize {
        match self {
            StatefulKind::Slew => 2,
            StatefulKind::Transition => 5,
            StatefulKind::Ddt => 2,
        }
    }
}

/// One `transition`/`slew` call site and the state slots allocated to it.
///
/// **Per call site, not per construct**: the same `slew(V(a), r)` written twice is two
/// independent limiters with independent history, exactly as two `idt` calls on the same
/// argument are two independent accumulators ([`IdtAccumulator`]). Keying on `ExprId` is what
/// makes that fall out — the arena already gave each written occurrence its own identity.
///
/// Slots are allocated in ascending `ExprId` order, which is a **deterministic** function of the
/// IR alone. That matters more than it looks: the consumer's `committed` buffer is a flat array
/// indexed by these offsets, so two evaluations of the same model must agree on the layout or a
/// limiter would read another one's history.
#[derive(Clone, Copy, Debug)]
pub struct StatefulCall {
    /// The call's own `ExprId.0`.
    pub expr_id: u32,
    /// Which construct, and hence how many slots.
    pub kind: StatefulKind,
    /// First slot index into this instance's own state region.
    pub base: usize,
}

/// Collect every `transition`/`slew` call in the module's expression arena, in ascending
/// `ExprId` order, assigning each its state slots.
///
/// Scans the **arena** rather than walking statements, deliberately: a call reached only from
/// inside an `if` arm still needs a stable slot, because whether that arm runs can change
/// between timepoints and its history must survive the steps where it does not.
/// Whether any lowered contribution anywhere in `stmts` carries a [`LaplaceTerm`].
fn stmts_contain_laplace(stmts: &[LoweredStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LoweredStmt::Contribute(c) => !c.laplace.is_empty(),
        LoweredStmt::If { then_, else_, .. } => {
            stmts_contain_laplace(then_) || stmts_contain_laplace(else_)
        }
        LoweredStmt::While { body, .. } | LoweredStmt::Repeat { body, .. } => {
            stmts_contain_laplace(body)
        }
        LoweredStmt::For {
            init, step, body, ..
        } => {
            stmts_contain_laplace(init)
                || stmts_contain_laplace(step)
                || stmts_contain_laplace(body)
        }
        LoweredStmt::Case { arms, default, .. } => {
            arms.iter().any(|a| stmts_contain_laplace(&a.body)) || stmts_contain_laplace(default)
        }
        LoweredStmt::Assign { .. } | LoweredStmt::BoundStep(_) => false,
    })
}

fn collect_stateful_calls(module: &Module) -> (Vec<StatefulCall>, usize) {
    let mut calls = Vec::new();
    let mut next = 0usize;
    for (i, expr) in module.exprs.iter().enumerate() {
        let kind = match expr {
            Expr::Call(Builtin::Slew, _) => StatefulKind::Slew,
            Expr::Call(Builtin::Transition, _) => StatefulKind::Transition,
            Expr::Call(Builtin::Ddt, _) => StatefulKind::Ddt,
            _ => continue,
        };
        calls.push(StatefulCall {
            expr_id: i as u32,
            kind,
            base: next,
        });
        next += kind.width();
    }
    (calls, next)
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
    // Variables whose `ddt` binding was discarded at the close of a branch/loop, and that no
    // ordinary assignment has superseded since. Unlike `ddt_vars` this is never cloned per
    // branch — a drop that happened inside one arm must stay visible to everything after it.
    let mut dropped_ddt = HashSet::new();
    for stmt in &module.analog {
        lower_stmt(
            module,
            stmt,
            &slot_of_branch,
            &param_only,
            &mut ddt_vars,
            &mut dropped_ddt,
            &mut stmts,
        )?;
    }
    let (stateful_calls, state_len) = collect_stateful_calls(module);
    let has_laplace = stmts_contain_laplace(&stmts);

    Ok(Lowered {
        n_unknowns: next_slot,
        stmts,
        branch_currents,
        idt_accumulators,
        flow_current_accumulators,
        node_kcl_probes,
        stateful_calls,
        state_len,
        has_laplace,
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
        // A `bound_step` argument is an ordinary expression and may probe like any other.
        Stmt::BoundStep(e) => collect_flow_probe_branches_in_expr(module, *e, out),
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
        Stmt::BoundStep(e) => collect_idt_calls_in_expr(module, *e, out),
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
        // `bound_step` targets no branch, so it classifies none.
        Stmt::BoundStep(_) => {}
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
    dropped_ddt: &mut HashSet<u32>,
    out: &mut Vec<LoweredStmt>,
) -> Result<(), CodegenError> {
    match stmt {
        Stmt::Contribute { target, value } => {
            let br = module.branches[target.branch.0 as usize];

            let mut terms = Vec::new();
            collect_terms(module, *value, 1.0, &mut terms);

            let mut resistive = Vec::new();
            let mut charge = Vec::new();
            let mut noise = Vec::new();
            let mut ac_stim = Vec::new();
            let mut laplace = Vec::new();
            for term in terms {
                // A bare variable read that was last assigned a `ddt` shape substitutes to that
                // shape here, so `real dqdt; dqdt = ddt(q); I <+ dqdt + …;` folds into the charge
                // channel exactly as `I <+ ddt(q) + …;` would.
                let shape_expr = match module.expr(term.expr) {
                    Expr::Var(id) => match ddt_vars.get(&id.0) {
                        Some(&shape) => shape,
                        // The variable held a `ddt` shape assigned inside a branch, and that
                        // binding was discarded when the branch closed (`DdtVars` is forward and
                        // single-pass by design). Lowering this as an ordinary resistive read
                        // would compile cleanly and silently omit the charge term entirely —
                        // `hicumL0_v2p1p0.va` loses its whole self-heating capacitance that way.
                        // Refuse instead: a missing reactive term is not something to guess at.
                        None if dropped_ddt.contains(&id.0) => {
                            return Err(unsupported(
                                "a ddt assigned to a variable inside an if/case/loop arm and contributed after that arm is not supported: the charge term would be silently dropped. Move the ddt into the contribution itself, or assign it outside the branch",
                            ))
                        }
                        None => term.expr,
                    },
                    _ => term.expr,
                };
                // Noise first: a `white_noise`/`flicker_noise` call is never also a charge shape,
                // and checking it first keeps the charge path exactly as it was.
                if let Some(nt) = noise_term_shape(module, shape_expr)? {
                    noise.push(nt);
                    continue;
                }
                // Same treatment, same reason: the call's value is zero, so its arguments have
                // to be captured here or they are lost. `sign` is carried through because an
                // excitation adds linearly (see `AcStimTerm`).
                if let Some((phase_mask, mag, phase)) = ac_stim_term_shape(module, shape_expr)? {
                    ac_stim.push(AcStimTerm {
                        sign: term.sign,
                        phase_mask,
                        mag,
                        phase,
                    });
                    continue;
                }
                // Split before the charge check, like the others: a `laplace_*` is never also
                // a `ddt` shape, and `ad::eval` has no real-valued answer for a complex gain.
                if let Some(lt) = laplace_term_shape(module, shape_expr, term.sign)? {
                    laplace.push(lt);
                    continue;
                }
                match charge_term_shape(module, shape_expr, param_only)? {
                    // One term can yield several charge terms: a scaled parenthesised sum
                    // distributes over its `ddt`s (see `charge_term_shape`). Each shape's own
                    // relative sign multiplies the enclosing term's.
                    Some(shapes) => {
                        charge.extend(shapes.into_iter().map(|(sign, expr, coeffs)| ChargeTerm {
                            sign: term.sign * sign,
                            expr,
                            coeffs,
                        }))
                    }
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
                noise,
                ac_stim,
                laplace,
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
                    // A real value assigned here supersedes any `ddt` shape this variable held
                    // in an earlier branch, so a later `<+` read of it is an ordinary resistive
                    // read again, not a dropped charge term.
                    dropped_ddt.remove(&lhs.0);
                    out.push(LoweredStmt::Assign {
                        lhs: *lhs,
                        rhs: *rhs,
                    });
                }
            }
            Ok(())
        }
        Stmt::BoundStep(expr) => {
            out.push(LoweredStmt::BoundStep(*expr));
            Ok(())
        }
        Stmt::Block(body) => {
            for s in body {
                lower_stmt(
                    module,
                    s,
                    slot_of_branch,
                    param_only,
                    ddt_vars,
                    dropped_ddt,
                    out,
                )?;
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
                    dropped_ddt,
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
                    dropped_ddt,
                    &mut else_lowered,
                )?;
            }
            // Neither arm's own reassignments (of a variable pre-existing before the `if`, or a
            // brand-new one local to just one arm) are known to hold after the `if` — which arm
            // ran isn't known here — so forget any variable either arm assigned at all, in the
            // *outer* map that carries forward past this construct (see `DdtVars`'s doc comment).
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, then_);
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, else_);
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
                    dropped_ddt,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, body);
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
                dropped_ddt,
                &mut init_lowered,
            )?;
            let mut step_lowered = Vec::new();
            lower_stmt(
                module,
                step,
                slot_of_branch,
                param_only,
                &mut loop_ddt_vars,
                dropped_ddt,
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
                    dropped_ddt,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(
                module,
                param_only,
                ddt_vars,
                dropped_ddt,
                std::slice::from_ref(&**init),
            );
            invalidate_ddt_vars(
                module,
                param_only,
                ddt_vars,
                dropped_ddt,
                std::slice::from_ref(&**step),
            );
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, body);
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
                    dropped_ddt,
                    &mut body_lowered,
                )?;
            }
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, body);
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
                        dropped_ddt,
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
                    dropped_ddt,
                    &mut default_lowered,
                )?;
            }
            for arm in arms {
                invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, &arm.body);
            }
            invalidate_ddt_vars(module, param_only, ddt_vars, dropped_ddt, default);
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

/// A recognized `ddt` charge shape: a relative sign, the `ddt` call's own argument, and every
/// parameter-only scaling factor wrapping it, each paired with whether it divides (`true`)
/// rather than multiplies (`false`). See [`charge_term_shape`].
///
/// The sign is *relative to the enclosing term*: it accumulates the `-` of a subtraction or a
/// unary negation found **inside** the term (only reachable under a scaling coefficient — a
/// negation at the top of a contribution was already flattened by [`collect_terms`]), and the
/// caller multiplies it by the term's own sign.
type ChargeShape = (f64, ExprId, Vec<(ExprId, bool)>);

/// Recognize `expr` as one or more `ddt` charge terms, or `Ok(None)` if it is not a charge
/// shape at all.
///
/// The accepted shapes are `ddt(arg)` wrapped in any depth of parameter-only
/// multiplication/division (`coeff*ddt(arg)`, `ddt(arg)*coeff`, `ddt(arg)/coeff`,
/// `(ddt(arg)*coeff1)*coeff2`, `coeff1*coeff2*ddt(arg)`, ... — real models nest at least two
/// multiplications deep, e.g. `ekv26.va`'s `ddt(qjd)*TYPE*M`, parsing as `(ddt(qjd)*TYPE)*M`),
/// **and any sum, difference or negation of such shapes**. The last of those is why this returns
/// a `Vec`: a scaled parenthesised sum distributes over its terms.
///
/// Distributing matters because it is the shape real charge models are written in.
/// `external/ekv3.va` contributes its gate charge as
///
/// ```text
/// I(d, g) <+ SIGN_M * (d_gt_s * ddt(QD) + s_gt_d * ddt(QS)) * QON;
/// ```
///
/// — a *sum* of two scaled `ddt` calls, itself scaled on both sides. [`collect_terms`] flattens
/// only the top level of a contribution, so it cannot reach inside the parentheses, and the
/// pre-distribution recognizer saw a `Mul` whose operands were an `Add` (not a `ddt` shape) and
/// `QON` (not a `ddt` shape), returned `None`, and the contribution was rejected downstream.
/// Recognizing the sum here yields `[(+1, QD, [d_gt_s, SIGN_M, QON]), (+1, QS, [s_gt_d, SIGN_M,
/// QON])]`, exactly as if the model had written the distributed form by hand.
///
/// A sum is accepted **only when every one of its terms is itself a charge shape**. A mixed
/// `(resistive + ddt(q)) * coeff` is deliberately *not* split: doing so would require
/// synthesising a new `resistive * coeff` expression, and this module only ever reads the IR
/// arena — it never writes to it. Such a term falls through to the resistive channel and is
/// rejected downstream exactly as before.
///
/// Returns `Ok(None)` for anything else — including a syntactically-plausible `coeff*ddt(arg)`
/// whose `coeff` fails the parameter-only check ([`is_param_only`] given `param_only`), which
/// falls back to being treated as an ordinary resistive term (and is rejected later, when
/// `ad::eval` actually tries to evaluate the still-nested `ddt` call, by the same
/// `CodegenError::Unsupported` this returned `None` to avoid pre-empting here).
fn charge_term_shape(
    module: &Module,
    expr: ExprId,
    param_only: &HashSet<u32>,
) -> Result<Option<Vec<ChargeShape>>, CodegenError> {
    if let Some(arg) = ddt_arg(module, expr)? {
        return Ok(Some(vec![(1.0, arg, Vec::new())]));
    }
    match module.expr(expr) {
        Expr::Binary(BinOp::Mul, l, r) => {
            if let Some(mut shapes) = charge_term_shape(module, *l, param_only)? {
                if is_param_only(module, *r, param_only) {
                    push_coeff(&mut shapes, *r, false);
                    return Ok(Some(shapes));
                }
            }
            if let Some(mut shapes) = charge_term_shape(module, *r, param_only)? {
                if is_param_only(module, *l, param_only) {
                    push_coeff(&mut shapes, *l, false);
                    return Ok(Some(shapes));
                }
            }
            Ok(None)
        }
        Expr::Binary(BinOp::Div, l, r) => {
            if let Some(mut shapes) = charge_term_shape(module, *l, param_only)? {
                if is_param_only(module, *r, param_only) {
                    push_coeff(&mut shapes, *r, true);
                    return Ok(Some(shapes));
                }
            }
            Ok(None)
        }
        // A sum only distributes if *both* halves are charge shapes — see this function's doc
        // comment for why a mixed resistive/charge sum cannot be split here.
        Expr::Binary(op @ (BinOp::Add | BinOp::Sub), l, r) => {
            let (Some(mut left), Some(right)) = (
                charge_term_shape(module, *l, param_only)?,
                charge_term_shape(module, *r, param_only)?,
            ) else {
                return Ok(None);
            };
            let flip = if matches!(op, BinOp::Sub) { -1.0 } else { 1.0 };
            left.extend(right.into_iter().map(|(s, e, c)| (s * flip, e, c)));
            Ok(Some(left))
        }
        Expr::Unary(UnOp::Neg, e) => Ok(charge_term_shape(module, *e, param_only)?.map(|shapes| {
            shapes
                .into_iter()
                .map(|(s, e, c)| (-s, e, c))
                .collect::<Vec<_>>()
        })),
        _ => Ok(None),
    }
}

/// Append one scaling factor to every shape in `shapes`, preserving the innermost-first order
/// [`ChargeTerm::coeffs`] documents.
fn push_coeff(shapes: &mut [ChargeShape], coeff: ExprId, divides: bool) {
    for (_, _, coeffs) in shapes.iter_mut() {
        coeffs.push((coeff, divides));
    }
}

/// If `expr` is a bare `white_noise(pwr)` or `flicker_noise(pwr, exp)` call, return the matching
/// [`NoiseTerm`]; `Ok(None)` for anything else.
///
/// Deliberately recognizes only the **bare** call, unlike [`charge_term_shape`], which flattens
/// any depth of parameter-only scaling around a `ddt`. A noise source's contribution to the
/// output is weighted by `|Z|²`, so a scale factor `k` written around the call would have to be
/// applied as `k²` to the PSD — and a model author writing `2*white_noise(p)` almost certainly
/// means "twice the power," not "four times." Rather than guess, an unrecognized shape falls
/// through to the resistive channel, where the noise call evaluates to its LRM-mandated `0` and
/// the source is simply never declared. That is a silent drop, so [`crate::GeneratedModel`]
/// additionally rejects any noise call left nested in a resistive term (see its `validate`).
fn noise_term_shape(module: &Module, expr: ExprId) -> Result<Option<NoiseTerm>, CodegenError> {
    match module.expr(expr) {
        Expr::Call(Builtin::WhiteNoise, args) if args.len() == 1 => {
            Ok(Some(NoiseTerm::White { pwr: args[0] }))
        }
        Expr::Call(Builtin::WhiteNoise, _) => {
            Err(unsupported("white_noise expects exactly one argument"))
        }
        Expr::Call(Builtin::FlickerNoise, args) if args.len() == 2 => {
            Ok(Some(NoiseTerm::Flicker {
                pwr: args[0],
                exp: args[1],
            }))
        }
        Expr::Call(Builtin::FlickerNoise, _) => Err(unsupported(
            "flicker_noise expects exactly two arguments (power, exponent)",
        )),
        // The table's own well-formedness (pairs, uniqueness, sort order) was settled at
        // elaboration, where the source file could be named in the diagnostic; an odd argument
        // count here would mean the IR was built by hand rather than by `va-frontend`, so it is
        // still checked, just not re-explained.
        Expr::Call(b @ (Builtin::NoiseTable | Builtin::NoiseTableLog), args)
            if args.len() % 2 == 0 =>
        {
            let interp = match b {
                Builtin::NoiseTableLog => TableInterp::Log,
                _ => TableInterp::Linear,
            };
            Ok(Some(NoiseTerm::Table { call: expr, interp }))
        }
        Expr::Call(Builtin::NoiseTable | Builtin::NoiseTableLog, _) => Err(unsupported(
            "noise_table/noise_table_log expects an even number of arguments (alternating \
             frequency and power)",
        )),
        _ => Ok(None),
    }
}

/// If `expr` is a bare `ac_stim(mask, mag, phase)` call, return its `(phase_mask, mag, phase)`;
/// `Ok(None)` for anything else.
///
/// Recognizes only the **bare** call, like [`noise_term_shape`] and unlike
/// [`charge_term_shape`]. Here the restriction is conservatism rather than a correctness trap —
/// an excitation *is* linear, so `2*ac_stim(1,0)` would have a well-defined meaning — but
/// `collect_terms` flattens only sums and negations, so a general scaling coefficient has
/// nowhere to go without the same coefficient-flattening machinery `charge_term_shape` carries.
/// An unrecognized shape is **rejected** by `crate::GeneratedModel::validate` (via
/// [`contains_ac_stim_call`]) rather than silently evaluating to zero and vanishing.
///
/// `va-frontend` normalizes every call to exactly three arguments with the mask already folded
/// to a constant, so an argument count or shape other than that means the IR was built by hand.
fn ac_stim_term_shape(
    module: &Module,
    expr: ExprId,
) -> Result<Option<(u32, ExprId, ExprId)>, CodegenError> {
    match module.expr(expr) {
        Expr::Call(Builtin::AcStim, args) if args.len() == 3 => match module.expr(args[0]) {
            Expr::Const(mask) => Ok(Some((*mask as u32, args[1], args[2]))),
            _ => Err(unsupported(
                "ac_stim's analysis-name argument must fold to a constant phase bitmask",
            )),
        },
        Expr::Call(Builtin::AcStim, _) => Err(unsupported(
            "ac_stim expects exactly three arguments after elaboration \
             (phase bitmask, magnitude, phase)",
        )),
        _ => Ok(None),
    }
}

/// Whether `expr` contains an `ac_stim` call anywhere in its tree — used to reject a stimulus
/// buried where [`ac_stim_term_shape`] cannot pull it out, rather than silently contributing
/// nothing. The exact counterpart of [`contains_noise_call`].
pub(crate) fn contains_ac_stim_call(module: &Module, expr: ExprId) -> bool {
    match module.expr(expr) {
        Expr::Call(Builtin::AcStim, _) => true,
        Expr::Call(_, args) => args.iter().any(|&a| contains_ac_stim_call(module, a)),
        Expr::Unary(_, e) => contains_ac_stim_call(module, *e),
        Expr::Binary(_, l, r) => {
            contains_ac_stim_call(module, *l) || contains_ac_stim_call(module, *r)
        }
        _ => false,
    }
}

/// Whether `expr` contains a `ddt` call anywhere in its tree.
///
/// Structural, not numeric: `Dual::carries_charge` answers the same question by inspecting a
/// gradient at one evaluation point, and a coefficient that happens to be zero there hides a
/// `ddt` that is very much present (`c(x)*ddt(q)` at `x = 0`). The eager validation in
/// `crate::GeneratedModel::validate` must not depend on the probe point, so it asks this
/// instead — the same shape as [`contains_noise_call`] and [`contains_ac_stim_call`].
pub(crate) fn contains_ddt_call(module: &Module, expr: ExprId) -> bool {
    match module.expr(expr) {
        Expr::Call(Builtin::Ddt, _) => true,
        Expr::Call(_, args) | Expr::CallUser(_, args) => {
            args.iter().any(|&a| contains_ddt_call(module, a))
        }
        Expr::Unary(_, e) | Expr::Ddx(e, _) => contains_ddt_call(module, *e),
        Expr::Binary(_, l, r) => contains_ddt_call(module, *l) || contains_ddt_call(module, *r),
        Expr::Select(c, t, f) => {
            contains_ddt_call(module, *c)
                || contains_ddt_call(module, *t)
                || contains_ddt_call(module, *f)
        }
        _ => false,
    }
}

/// Whether `expr` contains a noise call anywhere in its tree — used to reject a noise source
/// buried where [`noise_term_shape`] cannot pull it out (e.g. `2*white_noise(p)` or
/// `sin(white_noise(p))`), rather than silently contributing nothing.
pub(crate) fn contains_noise_call(module: &Module, expr: ExprId) -> bool {
    match module.expr(expr) {
        Expr::Call(
            Builtin::WhiteNoise
            | Builtin::FlickerNoise
            | Builtin::NoiseTable
            | Builtin::NoiseTableLog,
            _,
        ) => true,
        Expr::Call(_, args) => args.iter().any(|&a| contains_noise_call(module, a)),
        Expr::Unary(_, e) => contains_noise_call(module, *e),
        Expr::Binary(_, l, r) => contains_noise_call(module, *l) || contains_noise_call(module, *r),
        _ => false,
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
fn invalidate_ddt_vars(
    module: &Module,
    param_only: &HashSet<u32>,
    ddt_vars: &mut DdtVars,
    dropped: &mut HashSet<u32>,
    stmts: &[Stmt],
) {
    let mut assigns = Vec::new();
    collect_assigns(stmts, &mut assigns);
    for (var, rhs) in assigns {
        ddt_vars.remove(&var);
        // A `ddt` shape assigned *inside* the branch had its binding recorded only in the
        // branch's clone of the map, which is discarded here. If the variable is later read at
        // a `<+`, the charge term it stood for would vanish with no diagnostic — record it so
        // the contribution site can refuse instead. A plain reassignment afterwards clears the
        // mark (see `Stmt::Assign`), because that supersedes the `ddt` with a real value.
        if matches!(charge_term_shape(module, rhs, param_only), Ok(Some(_))) {
            dropped.insert(var);
        }
    }
}

fn collect_assigns_one(stmt: &Stmt, out: &mut Vec<(u32, ExprId)>) {
    match stmt {
        Stmt::Assign { lhs, rhs } => out.push((lhs.0, *rhs)),
        // `bound_step` assigns nothing, so it invalidates no `ddt`-shape binding.
        Stmt::BoundStep(_) => {}
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
