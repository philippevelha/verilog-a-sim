//! T2 — code generation: lower a [`va_ir::Module`] into a [`va_abi::ModelInstance`].
//!
//! This is the highest-risk crate: it differentiates the IR (forward-mode AD over the
//! expression arena, see [`ad`]) to produce exact Jacobians. Per §5 every differentiated
//! operator is checked against a central finite difference — a wrong Jacobian silently kills
//! Newton.
//!
//! The generated instance reproduces, by construction, the same stamps the hand-written
//! reference models in `va-abi` emit: a flow contribution `I(p,n) <+ value` stamps the
//! residual `value` and its gradient as the canonical 2×2 conductance stamp, while a
//! `ddt(q)` term stamps `q` and its gradient into the charge channel. A potential contribution
//! `V(p,n) <+ expr` stamps differently — see [`GeneratedModel::stamp_branch_currents`].
//!
//! # Limitations (v0)
//!
//! - One global unknown per IR node, supplied as the first `module.nodes.len()` entries of
//!   `terminals`; one further global unknown per branch with a potential contribution,
//!   allocated by [`build_instance`] itself from `next_unknown` (see [`lower::Lowered::branch_currents`]).
//!   The v0 frontend emits modules whose nodes are exactly their ports, so the node prefix of
//!   `terminals` is the port→global map; modules with internal node unknowns are out of scope.
//! - A branch gets *either* flow or potential contributions, never both, with one exception —
//!   a "mixed" branch gated behind mutually-exclusive `if`/`else` arms (see `lower`'s
//!   Limitations). A purely flow-defined branch that's also read via a bare `I(...)` probe
//!   somewhere gets its own auxiliary unknown too, same as a potential contribution's branch
//!   current (see [`lower::FlowCurrentAccumulator`]), but only sums the branch's *resistive*
//!   contributions — a `ddt`/charge term contributed to a self-probed branch isn't reflected in
//!   what that probe reads back. A branch that receives *no* contribution anywhere, probed as a
//!   single-terminal (implicit-ground) access, resolves instead via a node-KCL sum over every
//!   other contributing branch touching its non-ground terminal (see [`lower::NodeKclProbe`]) —
//!   a bare two-node probe of an uncontributed branch between two other, non-ground nodes stays
//!   unsupported.
//!   Local-variable assignments and `if`/`else` *are* supported: statements execute in source
//!   order, each `Stmt::Assign` binding into [`ad::Ctx::vars`] for later statements (including
//!   later assignments — a variable can be reassigned) to read via [`ad::Ctx::set_var`]/the
//!   [`ad::eval`] `Expr::Var` case.
//! - `$vt`/`$temperature` evaluate at the fixed ambient point ([`VT`], [`TEMP`]); `$vt(T)`
//!   evaluates the thermal voltage at the given absolute temperature `T`, carrying `T`'s
//!   gradient (e.g. a self-heating thermal node).

#![forbid(unsafe_code)]

pub mod ad;
pub mod lower;

use ad::{eval, Ctx, Dual};
use lower::{Contribution, Lowered, LoweredStmt, NoiseTerm};
use std::cell::RefCell;
use std::collections::HashMap;
use thiserror::Error;
use va_abi::{ModelInstance, StampSink, UnknownKind};
use va_ir::{Builtin, Expr, ExprId, Module};

/// Thermal voltage `kT/q` at [`TEMP`], in volts. Matches `va_abi::reference::diode::VT_NOMINAL`
/// so a generated diode reproduces the reference diode's stamps.
pub const VT: f64 = 0.025_865;

/// Ambient temperature for `$temperature`, in kelvin: 300.15 K (27°C) — SPICE's/QSPICE's own
/// default nominal simulation temperature (`TNOM`), not an arbitrary round 300 K. Chosen to
/// match QSPICE (`CLAUDE.md` §7's oracle) exactly, since a mismatched fixed ambient temperature
/// is otherwise invisible for a linear circuit but produces a real, tolerance-breaking
/// divergence on any exponential model (confirmed empirically against `diode.va`: the two
/// conventions disagreed by ~0.85% at a 0.5 V forced bias — see `docs/roadmap.md`'s T6.3 notes).
pub const TEMP: f64 = 300.15;

/// Safety cap on how many times [`GeneratedModel::run`] will iterate a `while`/`for`/`repeat`
/// loop in a single [`ModelInstance::load`] call, before giving up rather than hanging.
///
/// Real compact models bound these themselves (a `while` convergence loop with an explicit
/// `l_itmax`/`niter<=4`-style cap, a `for` loop over a `nf`-fingers parameter), so this is
/// generous headroom above anything the corpus actually needs, not a tuned-to-the-edge limit.
/// A loop that still hasn't terminated by this point is either a genuinely non-terminating
/// (buggy or `x`-pathological) condition, or a `count`/`cond` this codegen subset evaluated
/// wrong — either way, [`GeneratedModel::run`] stops and reports [`CodegenError::Unsupported`]
/// rather than hang forever. This is the one case [`GeneratedModel::validate`] cannot rule out
/// ahead of time (it never actually iterates a loop — see `lower`'s module doc comment), so
/// unlike every other `CodegenError` this crate raises, it can still surface for the first time
/// from [`ModelInstance::load`], not just from `build_instance`'s eager validation.
pub const MAX_LOOP_ITERATIONS: usize = 1_000_000;

/// Errors raised while lowering/differentiating the IR.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// The IR used a construct this codegen subset does not yet support.
    #[error("unsupported construct: {0}")]
    Unsupported(String),

    /// `terminals` did not provide one global index per IR node.
    #[error("expected {expected} terminals (one per node), got {got}")]
    TerminalCount {
        /// Number of nodes in the module.
        expected: usize,
        /// Number of terminals supplied.
        got: usize,
    },
}

fn loop_iteration_cap_exceeded() -> CodegenError {
    CodegenError::Unsupported(format!(
        "a loop did not terminate within {MAX_LOOP_ITERATIONS} iterations"
    ))
}

/// Compile an elaborated IR module into a loadable model instance. `terminals` must have
/// exactly `module.nodes.len()` entries — the global unknown index for each of the module's
/// own nodes, in node order (unchanged from before potential contributions existed). Every
/// branch with a potential contribution needs its own auxiliary branch-current unknown, and
/// every distinct `idt(...)` call site needs its own accumulator unknown (see
/// [`lower::Lowered::branch_currents`]/[`lower::Lowered::idt_accumulators`]); `build_instance`
/// allocates all of these itself from `next_unknown` (incrementing it once per extra unknown,
/// branch currents first in ascending `BranchId` order, then accumulators in the order their
/// call sites were encountered), so the caller's own next-free-index counter (e.g. `va-cli`'s
/// device-building loop) stays in sync without having to pre-compute how many extra unknowns a
/// module will need.
///
/// # Errors
///
/// Returns [`CodegenError`] if `terminals` is the wrong length or the analog block contains
/// a construct outside the v0 subset (validated eagerly so [`ModelInstance::load`] cannot
/// fail).
/// Build a model instance from an elaborated IR module.
///
/// The generated model is **independent of the integration method**. It was briefly not: a
/// bias-dependent `ddt` coefficient (`c(x)·ddt(q)`) was accepted only under backward Euler,
/// because trapezoidal's rate reconstruction was first order once the timestep varied. That was
/// an integrator defect, fixed in `va-transient` by taking the first step with backward Euler,
/// and gated there by an order-of-convergence study — so there is nothing left for a model to be
/// compiled "for".
pub fn build_instance(
    module: &Module,
    terminals: &[usize],
    next_unknown: &mut usize,
) -> Result<Box<dyn ModelInstance>, CodegenError> {
    if terminals.len() != module.nodes.len() {
        return Err(CodegenError::TerminalCount {
            expected: module.nodes.len(),
            got: terminals.len(),
        });
    }

    let lowered = lower::lower(module)?;
    let mut full = terminals.to_vec();
    // One further global unknown per entry in `branch_currents`, then one more per entry in
    // `idt_accumulators` — `lowered.n_unknowns` is the authoritative total (see `lower::lower`),
    // so this stays correct regardless of how many categories of auxiliary unknown exist.
    while full.len() < lowered.n_unknowns {
        full.push(*next_unknown);
        *next_unknown += 1;
    }
    let params: Vec<f64> = module.params.iter().map(|p| p.default).collect();

    let model = GeneratedModel {
        module: module.clone(),
        terminals: full,
        params,
        lowered,
        vt: VT,
        temp: TEMP,
    };

    // Validate that every term is evaluable, so `load` never hits an `Unsupported` arm. The
    // checks are structural (independent of `x`), so an empty solution vector suffices.
    model.validate()?;

    Ok(Box::new(model))
}

/// A model instance generated from IR. Holds the module (for its arena), the resolved
/// parameter values, the lowered contribution plan, and the global terminal map.
struct GeneratedModel {
    module: Module,
    terminals: Vec<usize>,
    params: Vec<f64>,
    lowered: Lowered,
    vt: f64,
    temp: f64,
}

impl GeneratedModel {
    fn ctx<'a>(
        &'a self,
        x: &'a [f64],
        analysis: &va_abi::AnalysisCtx,
        state_prev: &'a [f64],
        validating: bool,
    ) -> Ctx<'a> {
        // A self-probed flow branch's accumulator slot is merged into the *same* map a potential
        // contribution's branch-current slot lives in — `ad::eval`'s flow-probe read doesn't (and
        // shouldn't) need to know which of the two reasons gave this branch a slot.
        let branch_current_slots = self
            .lowered
            .branch_currents
            .iter()
            .map(|bc| (bc.branch.0, bc.local_slot))
            .chain(
                self.lowered
                    .flow_current_accumulators
                    .iter()
                    .map(|acc| (acc.branch.0, acc.local_slot)),
            )
            .chain(
                self.lowered
                    .node_kcl_probes
                    .iter()
                    .map(|p| (p.branch.0, p.local_slot)),
            )
            .collect();
        let idt_slots = self
            .lowered
            .idt_accumulators
            .iter()
            .map(|acc| (acc.expr_id, acc.local_slot))
            .collect();
        Ctx {
            module: &self.module,
            params: &self.params,
            x,
            terminals: &self.terminals,
            vt: self.vt,
            temp: self.temp,
            analysis: *analysis,
            state_prev,
            // Pre-seeded from the committed state, so a `transition`/`slew` on a control-flow
            // path this evaluation doesn't take leaves its history untouched rather than
            // resetting it (the same rule `va_abi::state`'s consumer contract states).
            state_next: RefCell::new(state_prev.to_vec()),
            state_slots: self
                .lowered
                .stateful_calls
                .iter()
                .map(|c| (c.expr_id, (c.kind, c.base)))
                .collect(),
            bound_step: std::cell::Cell::new(None),
            vars: RefCell::new(HashMap::new()),
            branch_current_slots,
            idt_slots,
            mixed_branch_potential_used: RefCell::new(std::collections::HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating,
        }
    }

    /// Whether the branch whose auxiliary current unknown lives at local slot `local_slot` also
    /// receives a flow contribution somewhere in the module (see [`lower::BranchCurrent::mixed`]).
    fn is_mixed_branch(&self, local_slot: usize) -> bool {
        self.lowered
            .branch_currents
            .iter()
            .find(|bc| bc.local_slot == local_slot)
            .is_some_and(|bc| bc.mixed)
    }

    /// Whether `branch_id` (a `BranchId.0`) is a purely flow-defined branch that's also read via
    /// a bare `I(...)` probe somewhere in the module (see [`lower::FlowCurrentAccumulator`]).
    fn is_self_probed_flow_branch(&self, branch_id: u32) -> bool {
        self.lowered
            .flow_current_accumulators
            .iter()
            .any(|acc| acc.branch.0 == branch_id)
    }

    /// Real, load-time execution: walks `stmts` in source order. An assignment evaluates its
    /// right-hand side and binds it into `ctx`'s variable environment (`ad::Ctx::set_var`) so
    /// later statements can read it; a contribution stamps directly via `sink`; an `if`/`else`
    /// evaluates its condition and recurses into *only* the arm it selects — same "only the
    /// taken branch is ever evaluated" rule the ternary `Expr::Select` follows in `ad::eval`,
    /// and the reason this can't share a traversal with [`Self::validate_stmts`], which must
    /// visit both arms instead (see `lower`'s module doc comment). `case` is the same rule
    /// generalized to however many arms it has. A loop actually iterates here — re-evaluating
    /// its condition/count and re-running its body for real, up to [`MAX_LOOP_ITERATIONS`] —
    /// unlike [`Self::validate_stmts`], which only ever runs a loop body once (see `lower`'s
    /// module doc comment for why that's still sound).
    ///
    /// Aborts the whole walk on the first error — an assignment or condition that fails
    /// post-validation (which "cannot happen", with the sole exception of a loop exceeding
    /// [`MAX_LOOP_ITERATIONS`] — see its doc comment) would otherwise leave later statements
    /// reading a stale or missing variable binding, which is worse than stopping early and
    /// leaving whatever was already stamped as-is.
    fn run(
        &self,
        ctx: &Ctx,
        stmts: &[LoweredStmt],
        sink: &mut dyn StampSink,
    ) -> Result<(), CodegenError> {
        self.walk(ctx, stmts, &mut |me, ctx, c| me.stamp(ctx, c, sink))
    }

    /// [`Self::run`]'s control-flow walk, with what happens at each contribution left to the
    /// caller — so the noise channel ([`Self::noise`]) reaches exactly the same contributions,
    /// under exactly the same taken-branch and loop-iteration semantics, without duplicating any
    /// of that logic. `run` is the stamping caller; nothing else about it changed.
    fn walk<F>(
        &self,
        ctx: &Ctx,
        stmts: &[LoweredStmt],
        on_contribute: &mut F,
    ) -> Result<(), CodegenError>
    where
        F: FnMut(&Self, &Ctx, &Contribution),
    {
        for stmt in stmts {
            match stmt {
                LoweredStmt::Assign { lhs, rhs } => {
                    let d = eval(ctx, *rhs)?;
                    ctx.set_var(*lhs, d);
                }
                LoweredStmt::Contribute(c) => on_contribute(self, ctx, c),
                // Recorded on the context rather than handed to the callback: `walk` is shared
                // with the noise channel, which has no stamp sink to emit into. `load` drains
                // it once the walk is done (see `Ctx::request_bound_step`).
                LoweredStmt::BoundStep(e) => ctx.request_bound_step(eval(ctx, *e)?.value),
                LoweredStmt::If { cond, then_, else_ } => {
                    let taken = if eval(ctx, *cond)?.value != 0.0 {
                        then_
                    } else {
                        else_
                    };
                    self.walk(ctx, taken, on_contribute)?;
                }
                LoweredStmt::Case {
                    selector,
                    arms,
                    default,
                } => {
                    let sel = eval(ctx, *selector)?;
                    let mut taken = default;
                    'arms: for arm in arms {
                        for &label in &arm.labels {
                            if eval(ctx, label)?.value == sel.value {
                                taken = &arm.body;
                                break 'arms;
                            }
                        }
                    }
                    self.walk(ctx, taken, on_contribute)?;
                }
                LoweredStmt::While { cond, body } => {
                    let mut iters = 0usize;
                    while eval(ctx, *cond)?.value != 0.0 {
                        self.walk(ctx, body, on_contribute)?;
                        iters += 1;
                        if iters > MAX_LOOP_ITERATIONS {
                            return Err(loop_iteration_cap_exceeded());
                        }
                    }
                }
                LoweredStmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    self.walk(ctx, init, on_contribute)?;
                    let mut iters = 0usize;
                    while eval(ctx, *cond)?.value != 0.0 {
                        self.walk(ctx, body, on_contribute)?;
                        self.walk(ctx, step, on_contribute)?;
                        iters += 1;
                        if iters > MAX_LOOP_ITERATIONS {
                            return Err(loop_iteration_cap_exceeded());
                        }
                    }
                }
                LoweredStmt::Repeat { count, body } => {
                    let n = eval(ctx, *count)?.value;
                    if n > MAX_LOOP_ITERATIONS as f64 {
                        return Err(loop_iteration_cap_exceeded());
                    }
                    for _ in 0..(n.round().max(0.0) as usize) {
                        self.walk(ctx, body, on_contribute)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluate every statement once, at the all-zero operating point (structural validation):
    /// surfaces any unsupported construct as a `CodegenError` before the instance is handed
    /// out, so [`ModelInstance::load`] never has to.
    ///
    /// Unlike [`Self::run`], this visits **both** arms of every `if`/`else` unconditionally —
    /// an arm the all-zero point doesn't happen to select could still be the one a real
    /// operating point takes later, and `run` must never discover an unsupported construct
    /// there for the first time. Both arms validate against the same accumulating variable
    /// environment (an over-approximation, not full path-sensitive analysis: this is exact
    /// when both arms assign the same variables, as region-selecting `if`/`else` in real
    /// compact models does — `ids`/`gm`-style outputs set in every arm — but a variable
    /// genuinely assigned in only one arm and read after the `if` would not be soundly caught
    /// here, a stated limitation, not a silent one).
    fn validate(&self) -> Result<(), CodegenError> {
        // A DC context, for the same reason the operating point is all-zero: this is a
        // structural dry run, not a real evaluation, and it must reach every construct
        // regardless of which analysis eventually runs. Nothing here depends on the answer —
        // `analysis()` and `$abstime` evaluate to *some* constant either way, and validation
        // only cares that they evaluate at all.
        let ctx = self.ctx(&[], &va_abi::ANALYSIS_DC, &[], true);
        Self::validate_stmts(&ctx, &self.lowered.stmts)?;
        // An `idt` accumulator's argument only ever gets evaluated by
        // `Self::stamp_idt_accumulators` at real `load()` time, never as part of the ordinary
        // statement walk above (the call site that *reads* the accumulator's value never
        // evaluates its argument at all — see `lower::IdtAccumulator`'s doc comment) — so it
        // needs its own explicit validation pass here.
        for acc in &self.lowered.idt_accumulators {
            eval(&ctx, acc.arg)?;
        }
        Ok(())
    }

    fn validate_stmts(ctx: &Ctx, stmts: &[LoweredStmt]) -> Result<(), CodegenError> {
        for stmt in stmts {
            match stmt {
                LoweredStmt::Assign { lhs, rhs } => {
                    let d = eval(ctx, *rhs)?;
                    ctx.set_var(*lhs, d);
                }
                LoweredStmt::Contribute(c) => {
                    for term in &c.resistive {
                        // A noise call still buried inside a resistive term was not recognized
                        // as a top-level additive term, so `lower` could not pull it into the
                        // noise channel and `ad::eval` will quietly evaluate it to zero —
                        // declaring a noise source that silently contributes nothing. Reject it
                        // here instead (§ `lower::noise_term_shape`'s own doc comment on why
                        // scaled/nested shapes aren't guessed at).
                        if lower::contains_noise_call(ctx.module, term.expr) {
                            return Err(CodegenError::Unsupported(
                                "white_noise/flicker_noise must be a top-level additive term of \
                                 a contribution (a scaled or nested noise call would be silently \
                                 dropped, since its PSD scales as the square of any factor \
                                 around it)"
                                    .to_string(),
                            ));
                        }
                        // Same trap, same treatment: an `ac_stim` still buried in a resistive
                        // term was not pulled into the excitation channel, and `ad::eval` gives
                        // it its LRM-mandated zero value — so the stimulus would vanish with no
                        // diagnostic (§ `lower::ac_stim_term_shape`).
                        // A laplace still buried in a resistive term could not be split out,
                        // and `ad::eval` has no real-valued answer for a complex gain — so it
                        // would be a hard error mid-solve rather than a build diagnostic.
                        if lower::contains_laplace_call(ctx.module, term.expr) {
                            return Err(CodegenError::Unsupported(
                                "a laplace_* filter must be a top-level additive term of a                                  contribution (its gain is complex, so there is nowhere in an                                  ordinary expression to put it)"
                                    .to_string(),
                            ));
                        }
                        if lower::contains_ac_stim_call(ctx.module, term.expr) {
                            return Err(CodegenError::Unsupported(
                                "ac_stim must be a top-level additive term of a contribution (a \
                                 scaled or nested ac_stim would be silently dropped, since its \
                                 value is zero in every analysis and only the split-out \
                                 excitation channel carries it)"
                                    .to_string(),
                            ));
                        }
                        // A resistive term whose value depends on a *time derivative* — a
                        // charge rate scaled by a bias-dependent coefficient, `c(x)·ddt(q(x))` —
                        // is a rate, not a charge, so its value belongs in the residual and its
                        // exact Jacobian needs the product rule split across two channels:
                        // `jacobian += grad` and `dcharge += grad_ddt`. Both are stamped, on
                        // every path that can carry one, so nothing needs refusing here.
                        eval(ctx, term.expr)?;
                    }
                    for term in &c.charge {
                        // The charge channel is single: a `ddt` whose argument itself depends on
                        // a time derivative is a second derivative and has nowhere to go.
                        eval(ctx, term.expr)?;
                        if lower::contains_ddt_call(ctx.module, term.expr) {
                            return Err(CodegenError::Unsupported(
                                "ddt of a ddt is a second time derivative, which this project's single 
                                 charge channel cannot express"
                                    .to_string(),
                            ));
                        }
                    }
                    for term in &c.ac_stim {
                        eval(ctx, term.mag)?;
                        eval(ctx, term.phase)?;
                    }
                    for term in &c.laplace {
                        eval(ctx, term.input)?;
                        for &id in term.num.iter().chain(&term.den) {
                            eval(ctx, id)?;
                        }
                    }
                    for term in &c.noise {
                        match *term {
                            NoiseTerm::White { pwr } => {
                                eval(ctx, pwr)?;
                            }
                            NoiseTerm::Flicker { pwr, exp } => {
                                eval(ctx, pwr)?;
                                eval(ctx, exp)?;
                            }
                            NoiseTerm::Table { call, .. } => {
                                // Every table entry, so an unevaluable one fails the build rather
                                // than silently truncating the table at emit time.
                                if let Expr::Call(
                                    Builtin::NoiseTable | Builtin::NoiseTableLog,
                                    args,
                                ) = ctx.module.expr(call)
                                {
                                    for &arg in args {
                                        eval(ctx, arg)?;
                                    }
                                }
                            }
                        }
                    }
                    for term in &c.charge {
                        eval(ctx, term.expr)?;
                        for &(coeff, _) in &term.coeffs {
                            eval(ctx, coeff)?;
                        }
                    }
                }
                // Validated by evaluation, like every other expression: a `bound_step` argument
                // that cannot be evaluated must fail the build rather than be discovered
                // mid-transient. Its *value* is discarded here — this dry run is not a timestep.
                LoweredStmt::BoundStep(e) => {
                    eval(ctx, *e)?;
                }
                LoweredStmt::If { cond, then_, else_ } => {
                    eval(ctx, *cond)?;
                    Self::validate_stmts(ctx, then_)?;
                    Self::validate_stmts(ctx, else_)?;
                }
                LoweredStmt::Case {
                    selector,
                    arms,
                    default,
                } => {
                    eval(ctx, *selector)?;
                    for arm in arms {
                        for &label in &arm.labels {
                            eval(ctx, label)?;
                        }
                        Self::validate_stmts(ctx, &arm.body)?;
                    }
                    Self::validate_stmts(ctx, default)?;
                }
                // Loops never actually iterate here (see `lower`'s module doc comment): running
                // the body once already covers every statement a real iteration could execute,
                // without needing to resolve a real trip count or risk hanging on a runaway
                // `while` condition during eager validation.
                LoweredStmt::While { cond, body } => {
                    eval(ctx, *cond)?;
                    Self::validate_stmts(ctx, body)?;
                }
                LoweredStmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    Self::validate_stmts(ctx, init)?;
                    eval(ctx, *cond)?;
                    Self::validate_stmts(ctx, body)?;
                    Self::validate_stmts(ctx, step)?;
                }
                LoweredStmt::Repeat { count, body } => {
                    eval(ctx, *count)?;
                    Self::validate_stmts(ctx, body)?;
                }
            }
        }
        Ok(())
    }

    /// Sum a list of signed terms into a single dual.
    fn sum_terms(ctx: &Ctx, terms: &[lower::Term]) -> Result<Dual, CodegenError> {
        let mut acc = Dual::constant(0.0, ctx.count());
        for term in terms {
            let d = eval(ctx, term.expr)?;
            acc = acc.add(&d.scale(term.sign));
        }
        Ok(acc)
    }

    /// Sum a list of signed charge terms into a single dual, applying each term's own
    /// parameter-only scaling coefficients (if any) before accumulating — see
    /// [`lower::ChargeTerm`]'s doc comment. Each coefficient's gradient is always exactly zero
    /// (that's what "parameter-only" guarantees), so scaling by its plain value alone is the
    /// *exact* derivative, not an approximation: `d(coeff*q)/dx = coeff*dq/dx` whenever
    /// `dcoeff/dx = 0`, and this still holds applying several such coefficients in sequence.
    fn sum_charge_terms(ctx: &Ctx, terms: &[lower::ChargeTerm]) -> Result<Dual, CodegenError> {
        let mut acc = Dual::constant(0.0, ctx.count());
        for term in terms {
            let mut d = eval(ctx, term.expr)?;
            for &(coeff_expr, is_divisor) in &term.coeffs {
                let coeff = eval(ctx, coeff_expr)?.value;
                d = if is_divisor {
                    d.scale(1.0 / coeff)
                } else {
                    d.scale(coeff)
                };
            }
            acc = acc.add(&d.scale(term.sign));
        }
        Ok(acc)
    }

    /// Stamp one contribution's resistive and charge channels. A flow contribution (`c.branch_slot
    /// == None`) stamps the ordinary two-terminal KCL shape at `p_slot`/`n_slot`, unchanged from
    /// before potential contributions existed — including for a mixed branch's flow arm, which
    /// needs nothing special here (see [`Self::finalize_mixed_branch_currents`] for the other
    /// half). A potential contribution instead *subtracts* its value/gradient from its branch's
    /// own constraint row. For a non-mixed branch that row's structural `V(p)-V(n)` part and the
    /// branch current's KCL injection at `p`/`n` were already stamped unconditionally by
    /// [`Self::stamp_branch_currents`]; for a **mixed** branch (`lower::BranchCurrent::mixed`)
    /// they haven't been — this call might be the first (and possibly only) potential
    /// contribution to run for it this `load()` call, so the structural part is stamped here
    /// instead, lazily, exactly once (`ad::Ctx::mark_potential_used` reports whether it's the
    /// first time).
    fn stamp(&self, ctx: &Ctx, c: &Contribution, sink: &mut dyn StampSink) {
        match c.branch_slot {
            None => {
                let gp = self.terminals[c.p_slot];
                let gn = self.terminals[c.n_slot];

                if !c.resistive.is_empty() {
                    // Post-validation this cannot fail; bail without stamping if it ever does.
                    let Ok(i) = Self::sum_terms(ctx, &c.resistive) else {
                        return;
                    };
                    // See `lower::FlowCurrentAccumulator`'s doc comment: a purely flow-defined
                    // branch that's also read via a bare `I(...)` probe elsewhere needs this
                    // contribution's resistive total folded into a running per-branch sum, so
                    // `Self::stamp_flow_current_accumulators` can pin that branch's own auxiliary
                    // unknown to it once every contribution to the branch has run. A no-op for
                    // every other (the overwhelming majority of) flow branches.
                    if self.is_self_probed_flow_branch(c.branch.0) {
                        ctx.add_flow_current(c.branch.0, &i);
                    }
                    sink.residual(gp, i.value);
                    sink.residual(gn, -i.value);
                    for (slot, &dg) in i.grad.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.jacobian(gp, gk, dg);
                            sink.jacobian(gn, gk, -dg);
                        }
                    }
                    // The other half of the product rule for a bias-dependent charge
                    // coefficient, `c(x)·ddt(q(x))` (§ `Integration`). The term's *value* is
                    // already in the residual above — reconstructed from committed history by
                    // `Builtin::Ddt` — so it stamps **no** `charge`, which is what keeps it out
                    // of the companion offset. What is missing from the Jacobian is
                    // `c·∂q/∂x`, carried in `grad_ddt`; the integrator supplies the `coeff` that
                    // multiplies it, giving exactly `(dq/dt)·∂c/∂x + c·coeff·∂q/∂x`. Non-zero
                    // only downstream of a nested `ddt`, so this loop is a no-op for every
                    // ordinary resistive term.
                    for (slot, &dg) in i.grad_ddt.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.dcharge(gp, gk, dg);
                            sink.dcharge(gn, gk, -dg);
                        }
                    }
                }

                if !c.charge.is_empty() {
                    let Ok(q) = Self::sum_charge_terms(ctx, &c.charge) else {
                        return;
                    };
                    // A charge argument that itself carries charge is a second time derivative,
                    // which this sink's single charge channel cannot express. `Dual::into_ddt`
                    // catches the nested-expression spelling; this catches the one that reaches
                    // here through `lower`'s statement-level extraction of the *outer* `ddt`.
                    debug_assert!(
                        !q.carries_charge(),
                        "ddt of a ddt reached the charge channel; validate() should have                          rejected this module"
                    );
                    sink.charge(gp, q.value);
                    sink.charge(gn, -q.value);
                    for (slot, &dg) in q.grad.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.dcharge(gp, gk, dg);
                            sink.dcharge(gn, gk, -dg);
                        }
                    }
                }

                // `I(p,n) <+ ac_stim(...)`: a current stimulus, injected with the same sign
                // convention the resistive channel above uses — out of `p`, into `n`.
                if let Some((re, im)) = Self::ac_stim_excitation(ctx, &c.ac_stim) {
                    sink.excitation(gp, re, im);
                    sink.excitation(gn, -re, -im);
                }

                if !c.laplace.is_empty() {
                    self.stamp_laplace(ctx, &c.laplace, gp, gn, None, sink);
                }
            }
            Some(local_slot) => {
                if self.is_mixed_branch(local_slot) && ctx.mark_potential_used(local_slot) {
                    Self::stamp_branch_current_structural(
                        self.terminals[c.p_slot],
                        self.terminals[c.n_slot],
                        self.terminals[local_slot],
                        ctx.x,
                        sink,
                    );
                }
                let gb = self.terminals[local_slot];

                if !c.resistive.is_empty() {
                    let Ok(i) = Self::sum_terms(ctx, &c.resistive) else {
                        return;
                    };
                    sink.residual(gb, -i.value);
                    for (slot, &dg) in i.grad.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.jacobian(gb, gk, -dg);
                        }
                    }
                    // Product-rule charge sensitivity, same as the two-terminal path above.
                    for (slot, &dg) in i.grad_ddt.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.dcharge(gb, gk, -dg);
                        }
                    }
                }

                if !c.charge.is_empty() {
                    let Ok(q) = Self::sum_charge_terms(ctx, &c.charge) else {
                        return;
                    };
                    sink.charge(gb, -q.value);
                    for (slot, &dg) in q.grad.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.dcharge(gb, gk, -dg);
                        }
                    }
                }

                // `V(p,n) <+ ac_stim(...)`: a voltage stimulus. The constraint row reads
                // `V(p) − V(n) − expr = 0`, so the stimulus enters it negated — the same sign
                // the resistive channel just above carries, for the same reason.
                if let Some((re, im)) = Self::ac_stim_excitation(ctx, &c.ac_stim) {
                    sink.excitation(gb, -re, -im);
                }

                if !c.laplace.is_empty() {
                    self.stamp_laplace(ctx, &c.laplace, gb, gb, Some(gb), sink);
                }
            }
        }
    }

    /// Stamp a contribution's `laplace_*` terms (Tier C).
    ///
    /// `gp`/`gn` are the global rows a *flow* contribution injects into; `gb` is `Some(row)` for
    /// a potential contribution's constraint row, which takes the opposite sign — the same split
    /// the resistive channel just above uses.
    ///
    /// **The whole design rests on one identity.** At a single frequency `ω`, a complex
    /// admittance `H = a + jb` is exactly representable by the *real* pair `G = a`, `C = b/ω`,
    /// because the assembler forms `G + jω·C = a + jb`. So no complex stamp channel is needed:
    /// the existing `jacobian`/`dcharge` pair already spans the complex plane at any given
    /// frequency. What Tier C added was only telling the model *which* frequency
    /// (`AnalysisCtx::freq`) and re-linearizing per point.
    ///
    /// `ddt` is the special case `H(s) = s`, and the general rule reproduces it: `a = 0`,
    /// `b = ω`, so `C += grad` — precisely what the charge channel already stamps.
    ///
    /// Outside AC (and at `ω = 0`, which no `AcSweep` produces but a caller could) the transfer
    /// function collapses to its **real DC gain `H(0)`**, stamped resistively. That is exactly
    /// what this construct folded to at elaboration before Tier C, which is why every existing
    /// gate is unmoved — and it is also the honest transient answer only in the sense that it is
    /// the *previous* answer: a time-domain Laplace filter is a convolution, and remains
    /// unimplemented (`docs/proposals/frequency-domain.md` §2).
    fn stamp_laplace(
        &self,
        ctx: &Ctx,
        terms: &[lower::LaplaceTerm],
        gp: usize,
        gn: usize,
        gb: Option<usize>,
        sink: &mut dyn StampSink,
    ) {
        for term in terms {
            let Ok(u) = eval(ctx, term.input) else {
                continue;
            };
            let Some(num) = Self::eval_coeffs(ctx, &term.num) else {
                continue;
            };
            let Some(den) = Self::eval_coeffs(ctx, &term.den) else {
                continue;
            };

            let omega = 2.0 * std::f64::consts::PI * ctx.analysis.freq;
            let ac = ctx.analysis.kind == va_abi::AnalysisKind::Ac && omega != 0.0;
            let h = ad::laplace_at(
                if ac { omega } else { 0.0 },
                &num,
                term.num_is_roots,
                &den,
                term.den_is_roots,
            );
            if !h.0.is_finite() || !h.1.is_finite() {
                continue; // a denominator vanishing at this frequency: skip rather than poison
            }

            let (re, im) = (term.sign * h.0, term.sign * h.1);
            for (slot, &dg) in u.grad.iter().enumerate() {
                if dg == 0.0 {
                    continue;
                }
                let gk = self.terminals[slot];
                match gb {
                    None => {
                        sink.jacobian(gp, gk, re * dg);
                        sink.jacobian(gn, gk, -re * dg);
                        if ac {
                            sink.dcharge(gp, gk, im / omega * dg);
                            sink.dcharge(gn, gk, -im / omega * dg);
                        }
                    }
                    Some(gb) => {
                        sink.jacobian(gb, gk, -re * dg);
                        if ac {
                            sink.dcharge(gb, gk, -im / omega * dg);
                        }
                    }
                }
            }
            // The residual only matters where a *value* does: AC keeps derivatives alone.
            if !ac {
                match gb {
                    None => {
                        sink.residual(gp, re * u.value);
                        sink.residual(gn, -re * u.value);
                    }
                    Some(gb) => sink.residual(gb, -re * u.value),
                }
            }
        }
    }

    /// Evaluate a coefficient/root list to plain numbers, or `None` if any entry fails.
    ///
    /// Only the *values* are used: a filter's coefficients come from parameters, not from the
    /// solution vector, so their gradients are meaningless here. A coefficient that genuinely
    /// depended on `x` would be a nonlinear filter, which is not what `laplace_*` describes.
    fn eval_coeffs(ctx: &Ctx, ids: &[va_ir::ExprId]) -> Option<Vec<f64>> {
        ids.iter()
            .map(|&id| eval(ctx, id).ok().map(|d| d.value))
            .collect()
    }

    /// Sum a contribution's `ac_stim` terms into one complex excitation `(re, im)`, or `None`
    /// if none of them is active in the analysis `ctx` names.
    ///
    /// Each term contributes `sign · mag · e^{j·phase}`. The magnitude and phase expressions are
    /// evaluated at the operating point — so a bias-dependent stimulus comes out right — but only
    /// their **values** are used, never their gradients: an excitation is an independent applied
    /// quantity, so it belongs on the right-hand side, not in `G`.
    ///
    /// A term whose phase set does not include the running analysis contributes nothing, which
    /// is exactly the LRM's rule (§4.5.2) and the reason `ac_stim` can be written unconditionally
    /// in a model that also has to produce a DC operating point.
    fn ac_stim_excitation(ctx: &Ctx, terms: &[lower::AcStimTerm]) -> Option<(f64, f64)> {
        let mut acc: Option<(f64, f64)> = None;
        for term in terms {
            if !ad::phase_mask_active(&ctx.analysis, term.phase_mask) {
                continue;
            }
            // Post-validation these cannot fail; skipping a term that somehow does mirrors how
            // every other channel here bails rather than stamping from a corrupt evaluation.
            let (Ok(mag), Ok(phase)) = (eval(ctx, term.mag), eval(ctx, term.phase)) else {
                continue;
            };
            let a = term.sign * mag.value;
            let (re, im) = (a * phase.value.cos(), a * phase.value.sin());
            let (are, aim) = acc.unwrap_or((0.0, 0.0));
            acc = Some((are + re, aim + im));
        }
        acc
    }

    /// Stamp the constraint row's structural `V(p)-V(n)` term and the branch current's ordinary
    /// two-terminal KCL injection, exactly like `va_abi::reference::VSource` stamps its own
    /// branch current. `gp`/`gn`/`gb` are already-resolved global unknown indices.
    fn stamp_branch_current_structural(
        gp: usize,
        gn: usize,
        gb: usize,
        x: &[f64],
        sink: &mut dyn StampSink,
    ) {
        let vp = x.get(gp).copied().unwrap_or(0.0);
        let vn = x.get(gn).copied().unwrap_or(0.0);
        let ib = x.get(gb).copied().unwrap_or(0.0);

        sink.residual(gb, vp - vn);
        sink.jacobian(gb, gp, 1.0);
        sink.jacobian(gb, gn, -1.0);

        sink.residual(gp, ib);
        sink.residual(gn, -ib);
        sink.jacobian(gp, gb, 1.0);
        sink.jacobian(gn, gb, -1.0);
    }

    /// Stamp the structural part of every **non-mixed** branch (`lower::BranchCurrent::mixed ==
    /// false`) unconditionally and exactly once per [`ModelInstance::load`] call — see
    /// [`lower::BranchCurrent`]'s doc comment for why this can't just live inside [`Self::stamp`]
    /// (it must happen regardless of which, if any, `if`/`else` arm actually contributes to the
    /// branch this call). A **mixed** branch instead gets this lazily, from [`Self::stamp`]
    /// itself, only if a potential contribution actually runs for it this call — see
    /// [`Self::finalize_mixed_branch_currents`] for what happens when one doesn't.
    fn stamp_branch_currents(&self, x: &[f64], sink: &mut dyn StampSink) {
        for bc in &self.lowered.branch_currents {
            if !bc.mixed {
                Self::stamp_branch_current_structural(
                    self.terminals[bc.p_slot],
                    self.terminals[bc.n_slot],
                    self.terminals[bc.local_slot],
                    x,
                    sink,
                );
            }
        }
    }

    /// After the statement walk finishes, resolve every **mixed** branch whose constraint row
    /// [`Self::stamp`] never claimed this call (no potential contribution ran for it — the
    /// branch's flow arm ran instead, or, in principle, neither did): its auxiliary current is
    /// otherwise a free unknown with no equation of its own this call, which would leave the
    /// system singular, so it's pinned to zero instead (`residual(gb, x[gb])`,
    /// `jacobian(gb, gb, 1.0)`) — sound because a flow-mode call already injects the branch's
    /// real current directly into `p`/`n` itself, with no need for this auxiliary unknown to
    /// carry anything.
    fn finalize_mixed_branch_currents(&self, ctx: &Ctx, sink: &mut dyn StampSink) {
        for bc in &self.lowered.branch_currents {
            if bc.mixed
                && !ctx
                    .mixed_branch_potential_used
                    .borrow()
                    .contains(&bc.local_slot)
            {
                let gb = self.terminals[bc.local_slot];
                sink.residual(gb, ctx.x.get(gb).copied().unwrap_or(0.0));
                sink.jacobian(gb, gb, 1.0);
            }
        }
    }

    /// Stamp every `idt` accumulator's own row: residual `-arg` (so Newton drives the row's
    /// charge-channel derivative — the accumulator's own `d/dt` — to equal `arg`) and charge
    /// equal to the accumulator's own current value (`dcharge/d(accumulator) = 1`) — see
    /// `lower::IdtAccumulator`'s doc comment for why this is the right encoding of `ddt(Y) = arg`.
    /// Runs unconditionally, once per `load()`/`validate()` call, independent of whichever
    /// `if`/`case` arm actually reaches this call site's `idt(...)` expression that call (same
    /// "always stamp the structural part" character as [`Self::stamp_branch_currents`]) — and
    /// only *after* [`Self::run`]/`Self::validate_stmts` finish, since `arg` may itself read a
    /// local variable only bound by the statement walk (real compact models routinely compute an
    /// `idt` argument from variables assigned earlier in the same analog block, e.g. PSP102's
    /// NQS `Tnorm`/`fk1`).
    fn stamp_idt_accumulators(&self, ctx: &Ctx, sink: &mut dyn StampSink) {
        for acc in &self.lowered.idt_accumulators {
            let g = self.terminals[acc.local_slot];
            // Post-validation this cannot fail; skip stamping if it somehow does, matching
            // `Self::stamp`'s own "cannot fail, bail without stamping if it somehow does" pattern.
            let Ok(d) = eval(ctx, acc.arg) else {
                continue;
            };
            sink.residual(g, -d.value);
            for (slot, &dg) in d.grad.iter().enumerate() {
                if dg != 0.0 {
                    let gk = self.terminals[slot];
                    sink.jacobian(g, gk, -dg);
                }
            }
            // `idt`'s integrand may itself carry a bias-dependent charge coefficient
            // (`idt(c(x)*ddt(q))`), whose `c·∂q/∂x` lives only in `grad_ddt` — see the
            // accumulator path above and § `Integration`. Stamped alongside this row's own
            // `dcharge(g, g, 1.0)`, which is the accumulator's *own* state, not the integrand's.
            for (slot, &dg) in d.grad_ddt.iter().enumerate() {
                if dg != 0.0 {
                    let gk = self.terminals[slot];
                    sink.dcharge(g, gk, -dg);
                }
            }
            sink.charge(g, ctx.x.get(g).copied().unwrap_or(0.0));
            sink.dcharge(g, g, 1.0);
        }
    }

    /// Pin every self-probed flow branch's own accumulator unknown to the branch's total
    /// resistive contribution this call (`residual = unknown - total`, `jacobian(self,self) =
    /// 1 - d(total)/d(self)` — the last term only nonzero for `diode_basic.va`'s genuinely
    /// self-referential case, where the branch's own current feeds into its own defining
    /// contribution; Newton resolves the fixed point exactly like any other implicit equation).
    /// See `lower::FlowCurrentAccumulator`'s doc comment. Runs unconditionally, once per
    /// `load()`/`validate()` call, after [`Self::run`]/`Self::validate_stmts` — every contribution
    /// to the branch must already have run so `ctx.flow_current_totals` holds the branch's real
    /// total, not a partial one. A branch none of whose contributing statements ran this call
    /// (e.g. they all sit in an untaken `if`/`case` arm) has no entry, treated as a total of zero,
    /// same as the branch's own node injection would produce.
    fn stamp_flow_current_accumulators(&self, ctx: &Ctx, sink: &mut dyn StampSink) {
        for acc in &self.lowered.flow_current_accumulators {
            let g = self.terminals[acc.local_slot];
            let total = ctx
                .flow_current_totals
                .borrow()
                .get(&acc.branch.0)
                .cloned()
                .unwrap_or_else(|| Dual::constant(0.0, ctx.count()));
            sink.residual(g, ctx.x.get(g).copied().unwrap_or(0.0) - total.value);
            sink.jacobian(g, g, 1.0);
            for (slot, &dg) in total.grad.iter().enumerate() {
                if dg != 0.0 {
                    let gk = self.terminals[slot];
                    sink.jacobian(g, gk, -dg);
                }
            }
            // The product-rule half, exactly as the ordinary resistive path stamps it
            // (§ `Integration`): a contribution folded into this accumulator can carry a
            // bias-dependent charge coefficient, and its `c·∂q/∂x` lives only in `grad_ddt`.
            // Dropping it here would leave this row's Jacobian zero where the truth is not —
            // the same silent-Newton-degradation this crate refuses elsewhere. Non-zero only
            // downstream of a nested `ddt`, so a no-op for every ordinary accumulator.
            for (slot, &dg) in total.grad_ddt.iter().enumerate() {
                if dg != 0.0 {
                    let gk = self.terminals[slot];
                    sink.dcharge(g, gk, -dg);
                }
            }
        }
    }

    /// Stamp every [`lower::NodeKclProbe`]'s own row: a purely linear relation to the current
    /// slots of every other contributing branch touching its non-ground terminal (`residual =
    /// unknown + Σ ± other_branch_current`, `jacobian(self,self) = 1`, `jacobian(self,other_slot)
    /// = ±1` per term) — see [`lower::NodeKclProbe`]'s doc comment for the sign convention. Unlike
    /// every other accumulator kind, this needs no expression evaluation at all: every slot it
    /// references is already fully resolved by the time this runs, whether from
    /// [`Self::stamp_branch_currents`]'s unconditional structural stamp or
    /// [`Self::stamp_flow_current_accumulators`]'s post-`run` resolution — so it only ever reads
    /// `x` directly, the same as [`Self::stamp_branch_currents`].
    fn stamp_node_kcl_probes(&self, x: &[f64], sink: &mut dyn StampSink) {
        for probe in &self.lowered.node_kcl_probes {
            let g = self.terminals[probe.local_slot];
            let mut residual = x.get(g).copied().unwrap_or(0.0);
            sink.jacobian(g, g, 1.0);
            for &(slot, sign) in &probe.terms {
                let gk = self.terminals[slot];
                residual += sign * x.get(gk).copied().unwrap_or(0.0);
                sink.jacobian(g, gk, sign);
            }
            sink.residual(g, residual);
        }
    }

    /// Read a `Builtin::NoiseTable` call's flattened arguments back into `(frequency, power)`
    /// pairs for Interface β's noise channel.
    ///
    /// The arguments are constants by construction (`va-frontend` const-folds, validates and
    /// sorts the table at elaboration — § `va_ir::Builtin::NoiseTable`), but they are evaluated
    /// through the ordinary `eval` path anyway rather than pattern-matched as `Expr::Const`: an
    /// IR built by some other producer may legitimately have a parameter reference there, and
    /// evaluating it costs one arena read. `None` if any entry fails to evaluate, so a broken
    /// table declares no source at all rather than a half-read one.
    fn noise_table_points(&self, ctx: &Ctx<'_>, call: ExprId) -> Option<Vec<(f64, f64)>> {
        let Expr::Call(Builtin::NoiseTable | Builtin::NoiseTableLog, args) = self.module.expr(call)
        else {
            return None;
        };
        let mut points = Vec::with_capacity(args.len() / 2);
        for pair in args.chunks(2) {
            let [f, p] = pair else { return None };
            let (f, p) = (eval(ctx, *f).ok()?, eval(ctx, *p).ok()?);
            points.push((f.value, p.value));
        }
        Some(points)
    }
}

impl ModelInstance for GeneratedModel {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    /// A branch's own auxiliary current unknown is a constraint row (`V(p)-V(n) = expr`), never
    /// safe for `va-core`'s `gmin` homotopy to shunt — everything else is an ordinary KCL row.
    fn unknown_kind(&self, i: usize) -> UnknownKind {
        if i >= self.module.nodes.len() {
            UnknownKind::Branch
        } else {
            UnknownKind::Node
        }
    }

    /// A node-kind unknown reports its `NodeDecl`'s own resolved `abstol` (§ nature-metadata
    /// wiring), if any; an auxiliary (branch-current/`idt` accumulator) unknown beyond
    /// `module.nodes.len()` has no `NodeDecl` to read one from and reports `None`, exactly
    /// mirroring `unknown_kind`'s `Node`/`Branch` split above.
    fn unknown_abstol(&self, i: usize) -> Option<f64> {
        self.module.nodes.get(i).and_then(|n| n.abstol)
    }

    /// Emit this model's own noise sources (T5.2) — Interface β's noise channel, fed from the
    /// `white_noise`/`flicker_noise`/`noise_table` calls `lower` split out of each contribution.
    ///
    /// Each source is placed across the branch its containing `<+` targets, which is exactly the
    /// LRM's rule: a noise function written in a branch contribution *is* a source in parallel
    /// with that branch. Its argument expressions are evaluated at the operating point `x`, so a
    /// bias-dependent PSD (SPICE's `KF·I^AF`, a diode's `2q·Id`) comes out right; only the value
    /// is used, never the gradient, since a noise source is an independent stochastic quantity
    /// rather than a function of the solution vector.
    ///
    /// Walks the same control flow `load` does ([`Self::walk`]), so a source declared inside an
    /// `if` arm is emitted only when that arm is the one taken at this operating point.
    ///
    /// `ctx.temp` is ignored: a Verilog-A model writes its own temperature dependence into the
    /// PSD expression (typically via `$temperature`/`$vt`, which read the temperature this model
    /// was compiled at — see [`ad::Ctx::temp`]), rather than having a `4kT` applied for it the
    /// way `va-abi`'s hand-written [`va_abi::reference::Resistor`] does. Its `kind` is passed
    /// through so a source whose PSD expression calls `analysis()` sees the truth.
    fn noise(&self, x: &[f64], actx: &va_abi::AnalysisCtx, sink: &mut dyn va_abi::NoiseSink) {
        // No state: a noise analysis linearizes about a fixed operating point and has no
        // accepted-timepoint sequence, so a `transition`/`slew` inside a PSD expression sees
        // `is_initial_step` and settles to its input — the same steady-state reading DC gets.
        let ctx = self.ctx(x, actx, &[], false);
        // Post-validation the evaluations below cannot fail; a failure mid-walk simply stops
        // emitting further sources, exactly as `load` stops stamping.
        let _ = self.walk(&ctx, &self.lowered.stmts, &mut |me, ctx, c| {
            if c.noise.is_empty() {
                return;
            }
            let (gp, gn) = (me.terminals[c.p_slot], me.terminals[c.n_slot]);
            for term in &c.noise {
                match *term {
                    NoiseTerm::White { pwr } => {
                        if let Ok(p) = eval(ctx, pwr) {
                            sink.white_current(gp, gn, p.value);
                        }
                    }
                    NoiseTerm::Flicker { pwr, exp } => {
                        if let (Ok(p), Ok(e)) = (eval(ctx, pwr), eval(ctx, exp)) {
                            sink.flicker_current(gp, gn, p.value, e.value);
                        }
                    }
                    NoiseTerm::Table { call, interp } => {
                        if let Some(points) = me.noise_table_points(ctx, call) {
                            sink.table_current(gp, gn, &points, interp);
                        }
                    }
                }
            }
        });
    }

    fn load(
        &self,
        x: &[f64],
        actx: &va_abi::AnalysisCtx,
        state: &mut va_abi::ModelState,
        sink: &mut dyn StampSink,
    ) {
        let ctx = self.ctx(x, actx, state.committed(), false);
        self.stamp_branch_currents(x, sink);
        // Post-validation this cannot fail; `run` already stops early rather than stamping
        // from a corrupted variable environment if it somehow does (see `run`'s doc comment).
        let _ = self.run(&ctx, &self.lowered.stmts, sink);
        self.finalize_mixed_branch_currents(&ctx, sink);
        // After `run`, not before: an `idt` accumulator's argument may read a local variable the
        // statement walk just bound (see `Self::stamp_idt_accumulators`'s doc comment).
        self.stamp_idt_accumulators(&ctx, sink);
        // Also after `run`: `ctx.flow_current_totals` only holds a branch's real total once every
        // contribution to it has run (see `Self::stamp_flow_current_accumulators`'s doc comment).
        self.stamp_flow_current_accumulators(&ctx, sink);
        // After every other accumulator kind: a `NodeKclProbe`'s row only ever reads other
        // slots, but a slot it reads may itself be a `FlowCurrentAccumulator` forced into
        // existence solely to serve it (see `lower::NodeKclProbe`'s doc comment) — order doesn't
        // change its *own* stamp (it only reads `x`), but keeping it last mirrors the dependency.
        self.stamp_node_kcl_probes(x, sink);
        // Last of all: any `bound_step(...)` the statement walk reached, and only in transient —
        // no other analysis has a timestep to bound, so forwarding it elsewhere would be
        // meaningless rather than merely harmless (§ `va_ir::Stmt::BoundStep`).
        if actx.kind == va_abi::AnalysisKind::Transient {
            if let Some(dt) = ctx.bound_step.get() {
                sink.bound_step(dt);
            }
            // A model mid-transition asks for steps fine enough to resolve its own ramp. Without
            // this the ramp would be sampled at whatever points the LTE controller happened to
            // choose — which, for a signal the controller sees as smooth, can be far too few.
            if let Some(dt) = self.transition_step_bound(&ctx) {
                sink.bound_step(dt);
            }
        }
        // Publish this evaluation's state proposal. Only committed if the consumer decides this
        // was an accepted timepoint (§ `va_abi::state`).
        for (slot, v) in ctx.state_next.borrow().iter().enumerate() {
            state.set(slot, *v);
        }
    }

    fn state_len(&self) -> usize {
        self.lowered.state_len
    }

    /// A compiled model is frequency-dependent exactly when its source contains a `laplace_*`
    /// filter: that is the only construct whose `G`/`C` themselves move with `ω`. Reporting it
    /// is what makes `va_acnoise::ac::run` re-linearize per point — a cost no ordinary model
    /// should pay, which is why this is opt-in rather than assumed.
    fn is_frequency_dependent(&self) -> bool {
        self.lowered.has_laplace
    }
}

impl GeneratedModel {
    /// The tightest step bound any **in-flight** `transition` wants, or `None` if none is
    /// ramping right now.
    ///
    /// `rise/8` resolves a ramp into roughly eight points, which is enough for the piecewise-
    /// linear shape to survive comparison against an analytic reference without forcing tiny
    /// steps through the long flat stretches on either side. A transition that has reached its
    /// target asks for nothing, so the controller is free to grow the step straight back.
    fn transition_step_bound(&self, ctx: &Ctx) -> Option<f64> {
        const POINTS_PER_RAMP: f64 = 8.0;
        let next = ctx.state_next.borrow();
        let mut bound: Option<f64> = None;
        for call in &self.lowered.stateful_calls {
            if call.kind != lower::StatefulKind::Transition {
                continue;
            }
            let (y, target, rate) = (
                next.get(call.base + 1).copied().unwrap_or(0.0),
                next.get(call.base + 2).copied().unwrap_or(0.0),
                next.get(call.base + 3).copied().unwrap_or(0.0),
            );
            // Still travelling, and at a finite rate: ask for a step that samples the ramp.
            if y != target && rate.is_finite() && rate > 0.0 {
                let span = (target - y).abs() / rate;
                let want = span / POINTS_PER_RAMP;
                if want.is_finite() && want > 0.0 {
                    bound = Some(bound.map_or(want, |b: f64| b.min(want)));
                }
            }
        }
        bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::stamps::DenseStamp;
    use va_abi::ANALYSIS_DC;
    use va_ir::{
        Access, AccessKind, Branch, BranchId, Builtin, Discipline, Expr, ExprId, FuncId, Function,
        Module, NodeDecl, NodeId, Param, Stmt, VarDecl, VarId,
    };

    /// Build the resistor IR: `I(p,n) <+ V(p,n) / R`, R defaulting to 1 kΩ.
    fn resistor_ir() -> Module {
        let mut m = Module::new("resistor");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "R".into(),
            default: 1000.0,
            min: Some(0.0),
            max: None,
        }];

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let r = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let i = m.push_expr(Expr::Binary(va_ir::BinOp::Div, v, r));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i,
        }];
        m
    }

    /// Build the capacitor IR: `I(p,n) <+ ddt(C * V(p,n))`, C defaulting to 1 pF.
    fn capacitor_ir() -> Module {
        let mut m = Module::new("capacitor");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "C".into(),
            default: 1e-12,
            min: Some(0.0),
            max: None,
        }];

        let c = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let cv = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c, v));
        let ddt = m.push_expr(Expr::Call(Builtin::Ddt, vec![cv]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: ddt,
        }];
        m
    }

    /// Build the diode IR: `I(a,c) <+ Is * (exp(V(a,c) / (N * $vt)) - 1)`.
    fn diode_ir() -> Module {
        let mut m = Module::new("diode");
        m.nodes = vec![
            NodeDecl {
                name: "a".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "c".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "Is".into(),
                default: 1e-14,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "N".into(),
                default: 1.0,
                min: Some(0.0),
                max: None,
            },
        ];

        let vd = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let n = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let vt = m.push_expr(Expr::Call(Builtin::Vt, vec![]));
        let nvt = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, n, vt));
        let arg = m.push_expr(Expr::Binary(va_ir::BinOp::Div, vd, nvt));
        let e = m.push_expr(Expr::Call(Builtin::Exp, vec![arg]));
        let one = m.push_expr(Expr::Const(1.0));
        let em1 = m.push_expr(Expr::Binary(va_ir::BinOp::Sub, e, one));
        let is = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let i = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, is, em1));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i,
        }];
        m
    }

    /// Build a varactor-like IR (mirrors `external/varactor.va`'s real shape, with `v0`/`v1`
    /// fixed at `0`/`1` for a simpler expected-value formula): two local variables assigned in
    /// sequence, the second reading the first, then a contribution reading the second —
    /// `real v, q; v = V(p,n); q = c0*v + c1*ln(cosh(v)); I(p,n) <+ ddt(q);`. This is exactly
    /// the shape `va-codegen` rejected before local-variable assignment support (`Stmt::Assign`
    /// lowering) existed.
    fn varactor_like_ir() -> Module {
        let mut m = Module::new("varactor_like");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "c0".into(),
                default: 1e-12,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "c1".into(),
                default: 0.5e-12,
                min: Some(0.0),
                max: None,
            },
        ];
        m.vars = vec![VarDecl { name: "v".into() }, VarDecl { name: "q".into() }];
        let (v_id, q_id) = (VarId(0), VarId(1));

        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let c1 = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let v_read = m.push_expr(Expr::Var(v_id));
        let c0v = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, v_read));
        let cosh_v = m.push_expr(Expr::Call(Builtin::Cosh, vec![v_read]));
        let ln_cosh = m.push_expr(Expr::Call(Builtin::Ln, vec![cosh_v]));
        let c1_ln = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c1, ln_cosh));
        let q_expr = m.push_expr(Expr::Binary(va_ir::BinOp::Add, c0v, c1_ln));
        let q_read = m.push_expr(Expr::Var(q_id));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q_read]));

        m.analog = vec![
            Stmt::Assign {
                lhs: v_id,
                rhs: vprobe,
            },
            Stmt::Assign {
                lhs: q_id,
                rhs: q_expr,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: ddt_q,
            },
        ];
        m
    }

    #[test]
    fn local_variables_compute_a_nonlinear_charge() {
        let inst = build_instance(&varactor_like_ir(), &[0, 1], &mut 2).unwrap();
        let v = 0.6;
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[v],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        let (c0, c1) = (1e-12, 0.5e-12);
        let q_expected = c0 * v + c1 * v.cosh().ln();
        // d/dv[c0*v + c1*ln(cosh(v))] = c0 + c1*tanh(v).
        let dqdv_expected = c0 + c1 * v.tanh();
        assert!(
            (sink.charge[0] - q_expected).abs() / q_expected.abs() < 1e-9,
            "charge: {} vs {}",
            sink.charge[0],
            q_expected
        );
        assert!(
            (sink.dcharge[0] - dqdv_expected).abs() / dqdv_expected.abs() < 1e-9,
            "dcharge: {} vs {}",
            sink.dcharge[0],
            dqdv_expected
        );
    }

    /// The §5 milestone for this construct: the AD Jacobian threaded *through* two sequential
    /// local-variable assignments must still match a central finite difference.
    #[test]
    fn ad_through_local_variables_matches_finite_difference() {
        let inst = build_instance(&varactor_like_ir(), &[0, 1], &mut 2).unwrap();
        let charge_at = |v: f64| {
            let mut s = DenseStamp::new(1);
            inst.load(
                &[v],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.charge[0]
        };

        let v = 0.6;
        let h = 1e-6;
        let fd = (charge_at(v + h) - charge_at(v - h)) / (2.0 * h);

        let mut sink = DenseStamp::new(1);
        inst.load(
            &[v],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let analytic = sink.dcharge[0];

        let rel = (analytic - fd).abs() / fd.abs();
        assert!(rel < 1e-6, "analytic {analytic} vs fd {fd} (rel {rel})");
    }

    #[test]
    fn reassignment_overwrites_the_previous_binding() {
        // real x; x = 1; x = x + 1; I(p,n) <+ x;  -- must read 2, the second assignment, not 1.
        let mut m = Module::new("reassign");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        let x_id = VarId(0);
        m.vars = vec![VarDecl { name: "x".into() }];
        let one = m.push_expr(Expr::Const(1.0));
        let x_read = m.push_expr(Expr::Var(x_id));
        let x_plus_one = m.push_expr(Expr::Binary(va_ir::BinOp::Add, x_read, one));
        let x_read_again = m.push_expr(Expr::Var(x_id));
        m.analog = vec![
            Stmt::Assign {
                lhs: x_id,
                rhs: one,
            },
            Stmt::Assign {
                lhs: x_id,
                rhs: x_plus_one,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: x_read_again,
            },
        ];

        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[0], 2.0);
    }

    #[test]
    fn reading_an_unassigned_variable_is_rejected() {
        // I(p,n) <+ x, with no assignment to x anywhere -- caught eagerly by build_instance's
        // own validate() pass, the same way every other unsupported construct is.
        let mut m = Module::new("unassigned");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }];
        let x_read = m.push_expr(Expr::Var(VarId(0)));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: x_read,
        }];

        assert!(matches!(
            build_instance(&m, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    /// Build a two-terminal device with an asymmetric (piecewise-linear) conductance:
    /// `if (V(p,n) > 0) I(p,n) <+ g_pos*V(p,n); else I(p,n) <+ g_neg*V(p,n);` — a real,
    /// common region-selection pattern (e.g. a crude clamp/rectifier-like element), not a
    /// contrived one.
    fn piecewise_ir(g_pos: f64, g_neg: f64) -> Module {
        let mut m = Module::new("piecewise");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "g_pos".into(),
                default: g_pos,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "g_neg".into(),
                default: g_neg,
                min: Some(0.0),
                max: None,
            },
        ];

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, v, zero));

        let v_then = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let g_pos_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let i_pos = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, g_pos_e, v_then));
        let then_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_pos,
        }];

        let v_else = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let g_neg_e = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let i_neg = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, g_neg_e, v_else));
        let else_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_neg,
        }];

        m.analog = vec![Stmt::If { cond, then_, else_ }];
        m
    }

    #[test]
    fn if_else_selects_the_conductance_for_the_operating_point() {
        let (g_pos, g_neg) = (1e-3, 5e-3);
        let inst = build_instance(&piecewise_ir(g_pos, g_neg), &[0, 1], &mut 2).unwrap();

        // V(p,n) = +1 V: the `then` arm, conductance g_pos.
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[1.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert!((sink.residual[0] - g_pos).abs() / g_pos < 1e-12);
        assert!((sink.jac(0, 0) - g_pos).abs() / g_pos < 1e-12);

        // V(p,n) = -1 V: the `else` arm, conductance g_neg -- a different value *and* a
        // different Jacobian, proving the selected branch's own gradient is what's stamped,
        // not the other arm's.
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[-1.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert!((sink.residual[0] + g_neg).abs() / g_neg < 1e-12);
        assert!((sink.jac(0, 0) - g_neg).abs() / g_neg < 1e-12);
    }

    #[test]
    fn validate_catches_an_error_in_the_arm_not_selected_at_the_all_zero_point() {
        // At x=0 (validate's own operating point), V(p,n) = 0, so `V(p,n) > 0` is false and
        // the `else` arm is what a naive "validate only the taken branch" scheme would check.
        // Put the broken construct in `then` instead -- build_instance must still reject it.
        let mut m = Module::new("bad_then");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }];

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, v, zero));

        // `then`: reads `x`, which is never assigned anywhere -- the broken arm.
        let x_read = m.push_expr(Expr::Var(VarId(0)));
        let then_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: x_read,
        }];
        // `else`: perfectly fine on its own.
        let one = m.push_expr(Expr::Const(1.0));
        let else_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: one,
        }];

        m.analog = vec![Stmt::If { cond, then_, else_ }];

        assert!(
            matches!(
                build_instance(&m, &[0, 1], &mut 2),
                Err(CodegenError::Unsupported(_))
            ),
            "an error in the untaken-at-x=0 arm must still be caught eagerly"
        );
    }

    #[test]
    fn resistor_matches_reference_stamp() {
        // 1 kΩ from node 0 to ground (index 1 is out of range of a dim-1 system).
        let inst = build_instance(&resistor_ir(), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[2.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // Same hand-checked values as va_abi's resistor_stamp_by_hand.
        assert!((sink.residual[0] - 2e-3).abs() < 1e-15);
        assert!((sink.jac(0, 0) - 1e-3).abs() < 1e-18);
        assert_eq!(sink.charge[0], 0.0);
    }

    #[test]
    fn unknown_abstol_reads_the_node_decls_own_value() {
        // § nature-metadata wiring: node 0 ("p") carries a resolved abstol, node 1 ("n") does
        // not — `unknown_abstol` must read each unknown's own `NodeDecl`, not a blanket value.
        let mut m = resistor_ir();
        m.nodes[0].abstol = Some(1e-6);
        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        assert_eq!(inst.unknown_abstol(0), Some(1e-6));
        assert_eq!(inst.unknown_abstol(1), None);
        // An index beyond `module.nodes.len()` (an auxiliary unknown) has no `NodeDecl` to
        // read one from — mirrors `unknown_kind`'s `Node`/`Branch` boundary at the same index.
        assert_eq!(inst.unknown_abstol(2), None);
    }

    #[test]
    fn capacitor_stamps_only_charge() {
        let inst = build_instance(&capacitor_ir(), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[3.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // Q = C*V = 1pF * 3V = 3e-12; dQ/dV = C = 1e-12. No resistive current.
        assert!((sink.charge[0] - 3e-12).abs() < 1e-24);
        assert!((sink.dcharge[0] - 1e-12).abs() < 1e-27);
        assert_eq!(sink.residual[0], 0.0);
    }

    /// `V(p,n) <+ k*idt(V(p,n));` -- `psp102`'s NQS `V(SPLINE1) <+ vnorm_inv*idt(...)` shape in
    /// miniature: `idt`'s argument is the very branch voltage the potential contribution
    /// constrains, so this exercises both halves of `IdtAccumulator` at once: the accumulator's
    /// own row (`ddt(Y) = arg`) and the constraint row reading `Y` back through a coefficient.
    fn idt_ir(k: f64) -> Module {
        let mut m = Module::new("idt_ir");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "k".into(),
            default: k,
            min: None,
            max: None,
        }];

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let k_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let idt_call = m.push_expr(Expr::Call(Builtin::Idt, vec![vpn]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, k_e, idt_call));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value,
        }];
        m
    }

    #[test]
    fn idt_accumulator_integrates_its_argument_and_reads_back_through_a_coefficient() {
        let mut next = 2;
        let k = 3.0;
        let inst = build_instance(&idt_ir(k), &[0, 1], &mut next).unwrap();
        // node p=0, n=1; branch-current slot -> global 2; idt accumulator slot -> global 3.
        assert_eq!(next, 4);

        let (vp, vn, y) = (0.7, 0.0, 1.25);
        let mut sink = DenseStamp::new(4);
        inst.load(
            &[vp, vn, 0.0, y],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // The accumulator's own row (global 3): residual = -(arg) = -(V(p,n)); jacobian w.r.t.
        // p/n = -1/+1; charge = Y itself; dcharge/dY = 1.
        assert!((sink.residual[3] - -(vp - vn)).abs() < 1e-12);
        assert!((sink.jac(3, 0) - -1.0).abs() < 1e-12);
        assert!((sink.jac(3, 1) - 1.0).abs() < 1e-12);
        assert!((sink.charge[3] - y).abs() < 1e-12);
        assert!((sink.dcharge[3 * 4 + 3] - 1.0).abs() < 1e-12);

        // The branch's constraint row (global 2): structural `V(p)-V(n)` minus `k*idt(...)`'s
        // value (`k*Y`, since `idt`'s own gradient w.r.t. `V(p,n)` is zero -- its value comes
        // only from the accumulator unknown, never its argument).
        let expected_residual = (vp - vn) - k * y;
        assert!((sink.residual[2] - expected_residual).abs() < 1e-12);
        assert!((sink.jac(2, 0) - 1.0).abs() < 1e-12);
        assert!((sink.jac(2, 1) - -1.0).abs() < 1e-12);
        assert!((sink.jac(2, 3) - -k).abs() < 1e-12);
    }

    /// `idt(arg, ic)`'s second (initial-condition) argument must not fail to lower or load --
    /// it's accepted syntactically (`lower::IdtAccumulator`'s doc comment) even though this v0
    /// codegen doesn't apply it as a real initial value.
    #[test]
    fn idt_with_initial_condition_argument_builds_and_loads() {
        let mut m = Module::new("idt_with_ic");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let ic = m.push_expr(Expr::Const(0.5));
        let idt_call = m.push_expr(Expr::Call(Builtin::Idt, vec![vpn, ic]));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value: idt_call,
        }];

        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(4);
        // p == n == 0, so the constraint row's structural `V(p)-V(n)` term drops out, isolating
        // `idt`'s own contribution: it reads back as the accumulator's raw value (0.0 here,
        // since the codegen doesn't seed it from `ic`) -- building and loading without erroring
        // is the main point of this test.
        inst.load(
            &[0.0, 0.0, 0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[2], 0.0);
    }

    /// Two distinct `idt(...)` call sites -- even with syntactically identical arguments -- get
    /// two independent accumulators, not one shared between them.
    #[test]
    fn two_distinct_idt_calls_get_independent_accumulators() {
        let mut m = Module::new("two_idt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let idt_a = m.push_expr(Expr::Call(Builtin::Idt, vec![vpn]));
        let idt_b = m.push_expr(Expr::Call(Builtin::Idt, vec![vpn]));
        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Add, idt_a, idt_b));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value: sum,
        }];

        let mut next = 2;
        let inst = build_instance(&m, &[0, 1], &mut next).unwrap();
        // branch-current slot -> 2; two distinct idt accumulators -> 3 and 4.
        assert_eq!(next, 5);

        let (y_a, y_b) = (2.0, 5.0);
        let mut sink = DenseStamp::new(5);
        inst.load(
            &[0.0, 0.0, 0.0, y_a, y_b],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // The constraint row reads `idt_a + idt_b` = y_a + y_b, not `2*` either one.
        assert!((sink.residual[2] - -(y_a + y_b)).abs() < 1e-12);
        // Each accumulator's own row is independent: both driven by the same argument (V(p,n)
        // = 0 here), but each is its own row with its own charge slot.
        assert!((sink.charge[3] - y_a).abs() < 1e-12);
        assert!((sink.charge[4] - y_b).abs() < 1e-12);
    }

    /// `asmhemt.va`'s `idisi = I(di,si);` shape: a purely flow-defined branch (`I(p,n) <+
    /// V(p,n)/R;`) read back afterward, purely to reuse the value elsewhere (here, contributed
    /// into a second branch's potential constraint, so the read's value is externally observable
    /// through `DenseStamp` without needing a third mechanism).
    #[test]
    fn self_probed_flow_branch_reads_back_its_total_contributed_current() {
        let mut m = Module::new("self_probed_flow");
        for name in ["p", "n", "out", "gnd"] {
            m.nodes.push(NodeDecl {
                name: name.into(),
                discipline: Discipline::Electrical,
                abstol: None,
            });
        }
        m.ports = vec![
            vec![NodeId(0)],
            vec![NodeId(1)],
            vec![NodeId(2)],
            vec![NodeId(3)],
        ];
        m.branches = vec![
            Branch {
                p: NodeId(0),
                n: NodeId(1),
            }, // branch 0: the self-probed resistor
            Branch {
                p: NodeId(2),
                n: NodeId(3),
            }, // branch 1: exposes `idisi` via a potential contribution
        ];
        m.params = vec![Param {
            name: "R".into(),
            default: 1000.0,
            min: Some(0.0),
            max: None,
        }];
        m.vars = vec![VarDecl {
            name: "idisi".into(),
        }];
        let idisi = VarId(0);

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let r = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let ipn = m.push_expr(Expr::Binary(va_ir::BinOp::Div, vpn, r));
        let ipn_probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(0),
        }));
        let idisi_read = m.push_expr(Expr::Var(idisi));

        m.analog = vec![
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: ipn,
            },
            Stmt::Assign {
                lhs: idisi,
                rhs: ipn_probe,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Potential,
                    branch: BranchId(1),
                },
                value: idisi_read,
            },
        ];

        let mut next = 4;
        let inst = build_instance(&m, &[0, 1, 2, 3], &mut next).unwrap();
        // branch-current slot (branch 1's constraint) -> 4; flow-current accumulator (branch 0)
        // -> 5.
        assert_eq!(next, 6);

        let (vp, vn, vout, vgnd) = (0.6, 0.1, 0.0, 0.0);
        let r_val = 1000.0;
        let mut sink = DenseStamp::new(6);
        inst.load(
            &[vp, vn, vout, vgnd, 0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // The accumulator's own row (slot 5): residual = x[5] - (vp-vn)/R; jacobian w.r.t. p/n.
        let total = (vp - vn) / r_val;
        assert!((sink.residual[5] - -total).abs() < 1e-9);
        assert!((sink.jac(5, 5) - 1.0).abs() < 1e-12);
        assert!((sink.jac(5, 0) - -(1.0 / r_val)).abs() < 1e-12);
        assert!((sink.jac(5, 1) - (1.0 / r_val)).abs() < 1e-12);

        // Branch 1's constraint row (slot 4) reads `idisi` (= the accumulator) back correctly:
        // residual = (vout-vgnd) - x[5].
        assert!((sink.residual[4] - (vout - vgnd)).abs() < 1e-12);
        assert!((sink.jac(4, 5) - -1.0).abs() < 1e-12);

        // The branch's own node injection is completely unaffected: still the ordinary resistive
        // stamp, exactly as if it were never self-probed at all.
        assert!((sink.residual[0] - total).abs() < 1e-9);
        assert!((sink.residual[1] - -total).abs() < 1e-9);
    }

    /// `verilogaLib-master/ohmmeter.va`'s real shape: `V(dutm,iprobe) <+ 0;` (an ideal-ammeter
    /// wire, branch 0) then `r_val = V(dutp,dutm) / I(iprobe);` — a *single-terminal* probe
    /// (branch 1, `(iprobe, gnd)`) that never itself receives any contribution at all, distinct
    /// from branch 0 even though they share node `iprobe`. Its value can only come from a
    /// node-KCL sum at `iprobe` over branch 0's own auxiliary current — see
    /// [`lower::NodeKclProbe`]. Exposes the probe's read the same way the other flow-probe tests
    /// do here, by contributing it into a third branch's (`out`, `gnd`) potential constraint.
    #[test]
    fn single_terminal_ground_probe_resolves_via_node_kcl_sum() {
        let mut m = Module::new("ohmmeter_like");
        for name in ["dutp", "dutm", "iprobe", "gnd", "out"] {
            m.nodes.push(NodeDecl {
                name: name.into(),
                discipline: Discipline::Electrical,
                abstol: None,
            });
        }
        m.ports = vec![
            vec![NodeId(0)],
            vec![NodeId(1)],
            vec![NodeId(2)],
            vec![NodeId(3)],
            vec![NodeId(4)],
        ];
        m.branches = vec![
            Branch {
                p: NodeId(1),
                n: NodeId(2),
            }, // branch 0: `V(dutm,iprobe) <+ 0;`, the ideal-ammeter wire
            Branch {
                p: NodeId(2),
                n: NodeId(3),
            }, // branch 1: `I(iprobe)` — a bare single-terminal probe, no contribution ever
            Branch {
                p: NodeId(4),
                n: NodeId(3),
            }, // branch 2: exposes the probe's read via a potential contribution
        ];

        let zero = m.push_expr(Expr::Const(0.0));
        let iprobe_read = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(1),
        }));

        m.analog = vec![
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Potential,
                    branch: BranchId(0),
                },
                value: zero,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Potential,
                    branch: BranchId(2),
                },
                value: iprobe_read,
            },
        ];

        let mut next = 5;
        let inst = build_instance(&m, &[0, 1, 2, 3, 4], &mut next).unwrap();
        // branch-current slots (branches 0 and 2, ascending) -> 5, 6; the node-KCL probe's own
        // accumulator (branch 1) -> 7. No `FlowCurrentAccumulator` is needed: branch 0 already
        // has a `BranchCurrent` slot to read from.
        assert_eq!(next, 8);

        let ib0 = 0.02;
        let mut sink = DenseStamp::new(8);
        inst.load(
            &[0.6, 0.5, 0.5, 0.0, 0.0, ib0, 0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // The probe's own row (slot 7): residual = x[7] - x[5] (branch 0's shared terminal is
        // its `n`, so its sign is `-1`), independent of whatever `x[7]` starts at.
        assert!((sink.residual[7] - -ib0).abs() < 1e-12);
        assert!((sink.jac(7, 7) - 1.0).abs() < 1e-12);
        assert!((sink.jac(7, 5) - -1.0).abs() < 1e-12);

        // Branch 0's own constraint/injection stamps are completely unaffected by also being
        // read through the new probe.
        assert!((sink.residual[5] - (0.5 - 0.5)).abs() < 1e-12);
        assert!((sink.jac(5, 1) - 1.0).abs() < 1e-12);
        assert!((sink.jac(5, 2) - -1.0).abs() < 1e-12);

        // Branch 2's constraint row (slot 6) reads the probe back correctly: residual =
        // (vout-vgnd) - x[7].
        assert!((sink.residual[6] - (0.0 - 0.0)).abs() < 1e-12);
        assert!((sink.jac(6, 7) - -1.0).abs() < 1e-12);
    }

    /// `diode_basic.va`'s real idiom: `Id = I(anode,cathode);` read *before* the contribution
    /// that defines the branch, then fed back into that very contribution (`Id` scales a
    /// series-resistance term) -- a genuine implicit equation, not just a sequential re-read.
    /// `Ib`'s own row must carry the correct self-referential Jacobian entry
    /// (`d(residual)/d(Ib) = 1 - d(total)/d(Ib)`) for Newton to actually resolve the fixed point.
    #[test]
    fn self_referential_flow_probe_feeds_back_into_its_own_contribution() {
        let mut m = Module::new("self_referential_flow");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "Rs".into(),
            default: 5.0,
            min: Some(0.0),
            max: None,
        }];
        m.vars = vec![VarDecl { name: "id".into() }];
        let id = VarId(0);

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let ipn_probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(0),
        }));
        let rs = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let id_read = m.push_expr(Expr::Var(id));
        let rs_id = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, rs, id_read));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Sub, vpn, rs_id));

        m.analog = vec![
            Stmt::Assign {
                lhs: id,
                rhs: ipn_probe,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value,
            },
        ];

        let mut next = 2;
        let inst = build_instance(&m, &[0, 1], &mut next).unwrap();
        assert_eq!(next, 3); // flow-current accumulator -> slot 2

        let (vp, vn, rs_val, ib) = (0.6, 0.1, 5.0, 0.05);
        let mut sink = DenseStamp::new(3);
        inst.load(
            &[vp, vn, ib],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // total = (vp-vn) - Rs*ib; residual[2] = ib - total; jacobian(2,2) = 1 - (-Rs) = 1+Rs.
        let total = (vp - vn) - rs_val * ib;
        assert!((sink.residual[2] - (ib - total)).abs() < 1e-9);
        assert!((sink.jac(2, 2) - (1.0 + rs_val)).abs() < 1e-12);
        assert!((sink.jac(2, 0) - -1.0).abs() < 1e-12);
        assert!((sink.jac(2, 1) - 1.0).abs() < 1e-12);

        // The node injection uses the same self-referential total, including its Jacobian entry
        // at the accumulator's own slot -- Newton needs this to converge on the fixed point.
        assert!((sink.residual[0] - total).abs() < 1e-9);
        assert!((sink.jac(0, 2) - -rs_val).abs() < 1e-12);
    }

    #[test]
    fn diode_current_and_conductance() {
        let inst = build_instance(&diode_ir(), &[0, 1], &mut 2).unwrap();
        let vd = 0.6;
        let mut sink = DenseStamp::new(1);
        inst.load(
            &[vd],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        let is = 1e-14;
        let nvt = 1.0 * VT;
        let i_expected = is * ((vd / nvt).exp() - 1.0);
        let g_expected = (is / nvt) * (vd / nvt).exp();
        assert!((sink.residual[0] - i_expected).abs() / i_expected.abs() < 1e-12);
        assert!((sink.jac(0, 0) - g_expected).abs() / g_expected.abs() < 1e-12);
    }

    /// The §5 milestone: the AD Jacobian must match a central finite difference.
    #[test]
    fn ad_matches_finite_difference() {
        let inst = build_instance(&diode_ir(), &[0, 1], &mut 2).unwrap();

        let residual_at = |vd: f64| {
            let mut s = DenseStamp::new(1);
            inst.load(
                &[vd],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.residual[0]
        };

        let vd = 0.6;
        let h = 1e-6;
        let fd = (residual_at(vd + h) - residual_at(vd - h)) / (2.0 * h);

        let mut sink = DenseStamp::new(1);
        inst.load(
            &[vd],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let analytic = sink.jac(0, 0);

        let rel = (fd - analytic).abs() / analytic.abs();
        assert!(rel < 1e-5, "rel error {rel} (fd={fd}, analytic={analytic})");
    }

    #[test]
    fn wrong_terminal_count_is_rejected() {
        match build_instance(&resistor_ir(), &[0], &mut 1) {
            Err(CodegenError::TerminalCount {
                expected: 2,
                got: 1,
            }) => {}
            _ => panic!("expected a TerminalCount error"),
        }
    }

    /// `V(p,n) <+ I(p,n) * R;` — the "voltage in terms of own current" series-resistance idiom
    /// that recurs across several real compact models (`diode.va`, `jfet.va`, `mosvar.va`: a
    /// bulk/access resistance modeled as a potential contribution reading the branch's own
    /// flow). Needs a self-referencing flow probe, which only resolves because this branch also
    /// receives a potential contribution (see `lower`'s module doc comment).
    fn series_resistor_ir(r: f64) -> Module {
        let mut m = Module::new("series_r");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "r".into(),
            default: r,
            min: Some(0.0),
            max: None,
        }];

        let i_probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(0),
        }));
        let r_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let rhs = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, i_probe, r_e));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value: rhs,
        }];
        m
    }

    #[test]
    fn potential_contribution_with_self_flow_probe_stamps_constraint_and_kcl() {
        let r = 1_000.0;
        let mut next_unknown = 2usize;
        let inst = build_instance(&series_resistor_ir(r), &[0, 1], &mut next_unknown).unwrap();
        // One auxiliary branch-current unknown allocated past the two node slots.
        assert_eq!(next_unknown, 3);

        let (vp, vn, ib) = (5.0, 2.0, 1e-3);
        let mut sink = DenseStamp::new(3);
        inst.load(
            &[vp, vn, ib],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // Constraint row (global index 2): V(p) - V(n) - I(p,n)*R = 0.
        assert!((sink.residual[2] - (vp - vn - ib * r)).abs() < 1e-9);
        assert!((sink.jac(2, 0) - 1.0).abs() < 1e-12);
        assert!((sink.jac(2, 1) + 1.0).abs() < 1e-12);
        // d(residual)/d(Ib) = -R -- the self-referencing flow probe's own diagonal term.
        assert!((sink.jac(2, 2) + r).abs() < 1e-9);

        // The branch current's own two-terminal KCL injection at p/n.
        assert!((sink.residual[0] - ib).abs() < 1e-15);
        assert!((sink.residual[1] + ib).abs() < 1e-15);
        assert!((sink.jac(0, 2) - 1.0).abs() < 1e-12);
        assert!((sink.jac(1, 2) + 1.0).abs() < 1e-12);

        // §5: the self-referencing flow-probe gradient must match a central finite difference.
        let residual_at = |ib: f64| {
            let mut s = DenseStamp::new(3);
            inst.load(
                &[vp, vn, ib],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.residual[2]
        };
        let h = 1e-6;
        let fd = (residual_at(ib + h) - residual_at(ib - h)) / (2.0 * h);
        let analytic = sink.jac(2, 2);
        assert!(
            (analytic - fd).abs() < 1e-6,
            "analytic {analytic} vs fd {fd}"
        );
    }

    /// `V(p,n) <+ L * ddt(I(p,n));` — an ideal inductor spelled as a potential contribution
    /// (`external/varistor.va`'s series-inductance branch). The `ddt` term must land in the
    /// *constraint row's* charge channel, not at `p`/`n` — a different routing than a flow
    /// contribution's `ddt` (which stamps at the node rows).
    fn inductor_like_ir(l: f64) -> Module {
        let mut m = Module::new("inductor_like");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "l".into(),
            default: l,
            min: Some(0.0),
            max: None,
        }];

        let i_probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(0),
        }));
        let l_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let arg = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, l_e, i_probe));
        let ddt = m.push_expr(Expr::Call(Builtin::Ddt, vec![arg]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value: ddt,
        }];
        m
    }

    #[test]
    fn ddt_inside_a_potential_contribution_stamps_the_constraint_rows_charge_channel() {
        let l = 2.5e-9;
        let mut next_unknown = 2usize;
        let inst = build_instance(&inductor_like_ir(l), &[0, 1], &mut next_unknown).unwrap();

        let ib = 0.4;
        let mut sink = DenseStamp::new(3);
        inst.load(
            &[0.0, 0.0, ib],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // The constraint row is `V(p)-V(n) - L*ddt(I(p,n)) = 0`; the structural `V(p)-V(n)`
        // part is stamped separately (`stamp_branch_currents`), so the remaining `-L*I(p,n)`
        // is what must land in the charge channel here, at the branch's own row (index 2), so
        // that its time-derivative subtracts `L*dI/dt` from the row the way the LRM's inductor
        // idiom (`V <+ L*ddt(I)`) requires.
        assert!((sink.charge[2] + l * ib).abs() < 1e-18);
        assert!((sink.dcharge[2 * 3 + 2] + l).abs() < 1e-18);
        // No charge at the ordinary node rows -- this isn't a node-charge stamp.
        assert_eq!(sink.charge[0], 0.0);
        assert_eq!(sink.charge[1], 0.0);
    }

    /// The real `` `collapsibleR `` idiom (`generalMacrosAndDefines.va`, `diode_cmc.va`): a
    /// branch that behaves as an ordinary resistor when a *parameter* clears some threshold,
    /// and collapses to a forced short otherwise -- `if (rt > 1.0) I(b) <+ V(b)/rt; else
    /// V(b) <+ 0.0;`. The same branch gets a flow contribution in one arm and a potential
    /// contribution in the other, mutually exclusively.
    fn collapsible_r_ir(rt: f64) -> Module {
        let mut m = Module::new("collapsible_r");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "rt".into(),
            default: rt,
            min: Some(0.0),
            max: None,
        }];

        let rt_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let one = m.push_expr(Expr::Const(1.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, rt_e, one));

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let rt_e2 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let i_over_rt = m.push_expr(Expr::Binary(va_ir::BinOp::Div, v, rt_e2));
        let then_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_over_rt,
        }];

        let zero = m.push_expr(Expr::Const(0.0));
        let else_ = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            },
            value: zero,
        }];

        m.analog = vec![Stmt::If { cond, then_, else_ }];
        m
    }

    #[test]
    fn collapsible_r_behaves_as_an_ordinary_resistor_above_threshold() {
        // rt = 2000 > 1.0 -- the flow (ordinary-resistor) arm runs.
        let rt = 2000.0;
        let mut next_unknown = 2usize;
        let inst = build_instance(&collapsible_r_ir(rt), &[0, 1], &mut next_unknown).unwrap();
        assert_eq!(next_unknown, 3); // the branch still gets an aux slot -- it might need one

        let (vp, vn, stray_ib) = (5.0, 2.0, 42.0);
        let mut sink = DenseStamp::new(3);
        inst.load(
            &[vp, vn, stray_ib],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // Ordinary resistor KCL at the nodes: I = (Vp-Vn)/rt.
        let expected_i = (vp - vn) / rt;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-12);
        assert!((sink.residual[1] + expected_i).abs() / expected_i < 1e-12);
        // No stray KCL injection from the (unused this call) auxiliary branch current.
        assert!((sink.jac(0, 2)).abs() < 1e-15);
        assert!((sink.jac(1, 2)).abs() < 1e-15);

        // The auxiliary current is otherwise a free unknown this call -- pinned to zero rather
        // than left unconstrained (a singular system).
        assert!((sink.residual[2] - stray_ib).abs() < 1e-12);
        assert!((sink.jac(2, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn collapsible_r_forces_a_short_below_threshold() {
        // rt = 0.5 <= 1.0 -- the potential (forced-short) arm runs: V(p,n) <+ 0.
        let rt = 0.5;
        let mut next_unknown = 2usize;
        let inst = build_instance(&collapsible_r_ir(rt), &[0, 1], &mut next_unknown).unwrap();

        let (vp, vn, ib) = (5.0, 2.0, 1e-3);
        let mut sink = DenseStamp::new(3);
        inst.load(
            &[vp, vn, ib],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // Constraint row: V(p) - V(n) - 0 = 0.
        assert!((sink.residual[2] - (vp - vn)).abs() < 1e-12);
        assert!((sink.jac(2, 0) - 1.0).abs() < 1e-12);
        assert!((sink.jac(2, 1) + 1.0).abs() < 1e-12);

        // The branch current's own KCL injection at p/n (the short actually carries current).
        assert!((sink.residual[0] - ib).abs() < 1e-15);
        assert!((sink.residual[1] + ib).abs() < 1e-15);
        assert!((sink.jac(0, 2) - 1.0).abs() < 1e-12);
        assert!((sink.jac(1, 2) + 1.0).abs() < 1e-12);
    }

    /// `case (sel) 0: I<+g0*V; 1,2: I<+g1*V; default: I<+gdef*V;` -- a model-selection idiom
    /// (`angelov.va`'s `case(Idsmod)`, `bsim4.va`'s `case(geo)`), structurally an n-ary `if`.
    fn case_ir(sel: f64, g0: f64, g1: f64, gdef: f64) -> Module {
        let mut m = Module::new("case_mod");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "sel".into(),
                default: sel,
                min: None,
                max: None,
            },
            Param {
                name: "g0".into(),
                default: g0,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "g1".into(),
                default: g1,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "gdef".into(),
                default: gdef,
                min: Some(0.0),
                max: None,
            },
        ];

        let sel_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));

        let contribute_with = |m: &mut Module, param_idx: u32| {
            let v = m.push_expr(Expr::Probe(Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            }));
            let g = m.push_expr(Expr::Param(va_ir::ParamId(param_idx)));
            let i = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, g, v));
            vec![Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: i,
            }]
        };

        let label0 = m.push_expr(Expr::Const(0.0));
        let label1a = m.push_expr(Expr::Const(1.0));
        let label1b = m.push_expr(Expr::Const(2.0));
        let arm0_body = contribute_with(&mut m, 1);
        let arm1_body = contribute_with(&mut m, 2);
        let default_body = contribute_with(&mut m, 3);

        m.analog = vec![Stmt::Case {
            selector: sel_e,
            arms: vec![
                va_ir::CaseArm {
                    labels: vec![label0],
                    body: arm0_body,
                },
                va_ir::CaseArm {
                    labels: vec![label1a, label1b],
                    body: arm1_body,
                },
            ],
            default: default_body,
        }];
        m
    }

    #[test]
    fn case_selects_the_matching_arm_including_a_multi_label_arm() {
        let (g0, g1, gdef) = (1e-3, 2e-3, 5e-3);

        // sel=2.0 matches arm1's *second* label (1,2: ...) -- proves multi-label arms work.
        let inst = build_instance(&case_ir(2.0, g0, g1, gdef), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[1.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert!((sink.residual[0] - g1).abs() / g1 < 1e-12);
        assert!((sink.jac(0, 0) - g1).abs() / g1 < 1e-12);

        // sel=99.0 matches nothing -- falls through to `default`.
        let inst = build_instance(&case_ir(99.0, g0, g1, gdef), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[1.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert!((sink.residual[0] - gdef).abs() / gdef < 1e-12);
        assert!((sink.jac(0, 0) - gdef).abs() / gdef < 1e-12);
    }

    /// `real acc; acc=0; repeat(n) acc=acc+V(p,n); I(p,n)<+acc*g;` -- accumulates `n` copies of
    /// the branch voltage, mirroring the real `for`-loop finger-accumulation idiom
    /// (`bsim4.va`'s `for (i=0;i<BSIM4nf;i=i+1) Inv_sa=Inv_sa+T0;`) but through `repeat`.
    fn repeat_accumulate_ir(n: f64, g: f64) -> Module {
        let mut m = Module::new("repeat_accum");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "acc".into() }];
        m.params = vec![
            Param {
                name: "n".into(),
                default: n,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "g".into(),
                default: g,
                min: Some(0.0),
                max: None,
            },
        ];

        let zero = m.push_expr(Expr::Const(0.0));
        let acc_init = Stmt::Assign {
            lhs: VarId(0),
            rhs: zero,
        };

        let acc_read = m.push_expr(Expr::Var(VarId(0)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Add, acc_read, v));
        let acc_update = Stmt::Assign {
            lhs: VarId(0),
            rhs: sum,
        };

        let n_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let repeat_stmt = Stmt::Repeat {
            count: n_e,
            body: vec![acc_update],
        };

        let acc_final = m.push_expr(Expr::Var(VarId(0)));
        let g_e = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let i_expr = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, acc_final, g_e));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_expr,
        };

        m.analog = vec![acc_init, repeat_stmt, contribute];
        m
    }

    #[test]
    fn repeat_accumulates_n_copies_of_the_branch_voltage() {
        let (n, g, v) = (3.0, 1e-3, 2.0);
        let inst = build_instance(&repeat_accumulate_ir(n, g), &[0, 1], &mut 2).unwrap();

        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // acc after 3 iterations = 3*v; I = 3*g*v.
        let expected_i = n * g * v;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-12);
        let expected_g = n * g;
        assert!((sink.jac(0, 0) - expected_g).abs() / expected_g < 1e-12);

        // §5: the gradient accumulated *through* the loop must match a central finite difference.
        let residual_at = |v: f64| {
            let mut s = DenseStamp::new(2);
            inst.load(
                &[v, 0.0],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.residual[0]
        };
        let h = 1e-6;
        let fd = (residual_at(v + h) - residual_at(v - h)) / (2.0 * h);
        assert!(
            (sink.jac(0, 0) - fd).abs() < 1e-6,
            "{} vs {}",
            sink.jac(0, 0),
            fd
        );
    }

    /// Same accumulation as [`repeat_accumulate_ir`], but through an explicit `for
    /// (i=0;i<n;i=i+1)` loop with its own counter variable, to exercise `init`/`cond`/`step`.
    fn for_accumulate_ir(n: f64, g: f64) -> Module {
        let mut m = Module::new("for_accum");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        // VarId(0) = acc, VarId(1) = the loop counter `i`.
        m.vars = vec![VarDecl { name: "acc".into() }, VarDecl { name: "i".into() }];
        m.params = vec![
            Param {
                name: "n".into(),
                default: n,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "g".into(),
                default: g,
                min: Some(0.0),
                max: None,
            },
        ];

        let zero = m.push_expr(Expr::Const(0.0));
        let acc_init = Stmt::Assign {
            lhs: VarId(0),
            rhs: zero,
        };

        let i_init_zero = m.push_expr(Expr::Const(0.0));
        let init = Stmt::Assign {
            lhs: VarId(1),
            rhs: i_init_zero,
        };

        let i_read = m.push_expr(Expr::Var(VarId(1)));
        let n_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Lt, i_read, n_e));

        let i_read2 = m.push_expr(Expr::Var(VarId(1)));
        let one = m.push_expr(Expr::Const(1.0));
        let i_next = m.push_expr(Expr::Binary(va_ir::BinOp::Add, i_read2, one));
        let step = Stmt::Assign {
            lhs: VarId(1),
            rhs: i_next,
        };

        let acc_read = m.push_expr(Expr::Var(VarId(0)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Add, acc_read, v));
        let acc_update = Stmt::Assign {
            lhs: VarId(0),
            rhs: sum,
        };

        let for_stmt = Stmt::For {
            init: Box::new(init),
            cond,
            step: Box::new(step),
            body: vec![acc_update],
        };

        let acc_final = m.push_expr(Expr::Var(VarId(0)));
        let g_e = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let i_expr = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, acc_final, g_e));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_expr,
        };

        m.analog = vec![acc_init, for_stmt, contribute];
        m
    }

    #[test]
    fn for_loop_accumulates_n_copies_of_the_branch_voltage() {
        let (n, g, v) = (4.0, 2e-3, 1.5);
        let inst = build_instance(&for_accumulate_ir(n, g), &[0, 1], &mut 2).unwrap();

        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let expected_i = n * g * v;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-12);
        let expected_g = n * g;
        assert!((sink.jac(0, 0) - expected_g).abs() / expected_g < 1e-12);
    }

    /// `real x; x=1.0; while (x>eps) x=x/2; I(p,n)<+x;` -- a pure local-variable computation
    /// (no dependence on the branch voltage at all), mirroring the real bounded-convergence
    /// idiom (`hicumL2*.va`'s `while (abs(d_Q)>=RTOLC*abs(Q_pT) && l_it<=l_itmax) ...`).
    fn halving_while_ir(eps: f64) -> Module {
        let mut m = Module::new("halving_while");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }];
        m.params = vec![Param {
            name: "eps".into(),
            default: eps,
            min: Some(0.0),
            max: None,
        }];

        let one = m.push_expr(Expr::Const(1.0));
        let x_init = Stmt::Assign {
            lhs: VarId(0),
            rhs: one,
        };

        let x_read = m.push_expr(Expr::Var(VarId(0)));
        let eps_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, x_read, eps_e));

        let x_read2 = m.push_expr(Expr::Var(VarId(0)));
        let two = m.push_expr(Expr::Const(2.0));
        let halved = m.push_expr(Expr::Binary(va_ir::BinOp::Div, x_read2, two));
        let x_update = Stmt::Assign {
            lhs: VarId(0),
            rhs: halved,
        };

        let while_stmt = Stmt::While {
            cond,
            body: vec![x_update],
        };

        let x_final = m.push_expr(Expr::Var(VarId(0)));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: x_final,
        };

        m.analog = vec![x_init, while_stmt, contribute];
        m
    }

    #[test]
    fn while_loop_halves_until_below_threshold() {
        let eps = 1e-3;
        let inst = build_instance(&halving_while_ir(eps), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // Replicate the same loop in Rust to get the expected final `x`, rather than hardcoding
        // a magic constant.
        let mut expected = 1.0_f64;
        while expected > eps {
            expected /= 2.0;
        }
        assert!((sink.residual[0] - expected).abs() < 1e-15);
        // The loop must have actually stopped -- one more halving would still be `<= eps`, so if
        // it stopped one iteration too early or late this would catch it.
        assert!(expected <= eps);
        assert!(expected * 2.0 > eps);
    }

    /// A `while` condition that never becomes false. `build_instance` cannot catch this
    /// (`validate` never actually iterates a loop -- see `lower`'s module doc comment), so this
    /// proves the other half of that documented limitation: `load` itself must still not hang
    /// forever, and must leave the system safely (if incompletely) stamped rather than panic.
    #[test]
    fn a_runaway_while_loop_is_bounded_by_the_iteration_cap_not_a_hang() {
        let mut m = Module::new("runaway");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }];

        let zero = m.push_expr(Expr::Const(0.0));
        let x_init = Stmt::Assign {
            lhs: VarId(0),
            rhs: zero,
        };

        // Always true: 1.0 > 0.0.
        let one_e = m.push_expr(Expr::Const(1.0));
        let zero_e = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, one_e, zero_e));

        let x_read = m.push_expr(Expr::Var(VarId(0)));
        let one = m.push_expr(Expr::Const(1.0));
        let x_next = m.push_expr(Expr::Binary(va_ir::BinOp::Add, x_read, one));
        let x_update = Stmt::Assign {
            lhs: VarId(0),
            rhs: x_next,
        };

        let while_stmt = Stmt::While {
            cond,
            body: vec![x_update],
        };

        // Never reached: `run` must abort inside the while loop first.
        let x_final = m.push_expr(Expr::Var(VarId(0)));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: x_final,
        };

        m.analog = vec![x_init, while_stmt, contribute];

        // `build_instance` still succeeds: validation only runs the loop body once.
        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // The post-loop contribution never ran, so the residual is untouched.
        assert_eq!(sink.residual[0], 0.0);
    }

    /// `real function sq(x); sq = x*x; endfunction`, called from `I(p,n) <+ sq(V(p,n))*g;` --
    /// the simplest real analog-function idiom (a small utility factored out of a compact
    /// model's own expressions).
    fn sq_function_ir(g: f64) -> Module {
        let mut m = Module::new("sq_func");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "g".into(),
            default: g,
            min: Some(0.0),
            max: None,
        }];
        // VarId(0) = the function's own argument `x`, VarId(1) = its return variable `sq`.
        m.vars = vec![VarDecl { name: "x".into() }, VarDecl { name: "sq".into() }];

        let x_a = m.push_expr(Expr::Var(VarId(0)));
        let x_b = m.push_expr(Expr::Var(VarId(0)));
        let mul = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, x_a, x_b));
        m.functions.push(Function {
            name: "sq".into(),
            args: vec![VarId(0)],
            arg_dirs: vec![va_ir::ArgDir::Input],
            ret: VarId(1),
            body: vec![Stmt::Assign {
                lhs: VarId(1),
                rhs: mul,
            }],
        });

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![v]));
        let g_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let i_expr = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, call, g_e));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: i_expr,
        }];
        m
    }

    #[test]
    fn user_function_computes_its_value_and_gradient_through_the_call() {
        let (v, g) = (3.0, 1e-3);
        let inst = build_instance(&sq_function_ir(g), &[0, 1], &mut 2).unwrap();

        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // I = g * sq(v) = g*v^2.
        let expected_i = g * v * v;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-12);
        // dI/dv = g * 2v (the chain rule through the function call).
        let expected_grad = g * 2.0 * v;
        assert!((sink.jac(0, 0) - expected_grad).abs() / expected_grad < 1e-12);

        // §5: cross-check against a central finite difference.
        let residual_at = |v: f64| {
            let mut s = DenseStamp::new(2);
            inst.load(
                &[v, 0.0],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.residual[0]
        };
        let h = 1e-6;
        let fd = (residual_at(v + h) - residual_at(v - h)) / (2.0 * h);
        assert!(
            (sink.jac(0, 0) - fd).abs() < 1e-6,
            "{} vs {}",
            sink.jac(0, 0),
            fd
        );
    }

    /// `mvsg_cmc_*.va`'s real `calc_iq`/`calc_capt` shape in miniature: `analog function real
    /// calc_sq_and_cube; output cubeout; input x; begin cubeout=x*x*x;
    /// calc_sq_and_cube=x*x; end endfunction`, called as
    /// `sq_result = calc_sq_and_cube(cube_result, V(p,n));` -- `cube_result` is never assigned
    /// before the call (exactly `mvsg_cmc_1.1.1.va`'s `qgsrs`/`cofsmt` pattern: a pure
    /// write-only result, read only through the call's own write-back, never via the outer
    /// assignment at all).
    fn output_arg_function_ir() -> Module {
        let mut m = Module::new("output_arg_func");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        // Function scope: VarId(0)=cubeout (output), VarId(1)=x (input), VarId(2)=the
        // function's own return variable.
        // Module scope: VarId(3)=sq_result, VarId(4)=cube_result.
        m.vars = vec![
            VarDecl {
                name: "cubeout".into(),
            },
            VarDecl { name: "x".into() },
            VarDecl {
                name: "calc_sq_and_cube".into(),
            },
            VarDecl {
                name: "sq_result".into(),
            },
            VarDecl {
                name: "cube_result".into(),
            },
        ];
        let (cubeout, x, ret, sq_result, cube_result) =
            (VarId(0), VarId(1), VarId(2), VarId(3), VarId(4));

        let x1 = m.push_expr(Expr::Var(x));
        let x2 = m.push_expr(Expr::Var(x));
        let x3 = m.push_expr(Expr::Var(x));
        let xx = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, x1, x2));
        let xxx = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, xx, x3));
        let x4 = m.push_expr(Expr::Var(x));
        let x5 = m.push_expr(Expr::Var(x));
        let xsq = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, x4, x5));
        m.functions.push(Function {
            name: "calc_sq_and_cube".into(),
            args: vec![cubeout, x],
            arg_dirs: vec![va_ir::ArgDir::Output, va_ir::ArgDir::Input],
            ret,
            body: vec![
                Stmt::Assign {
                    lhs: cubeout,
                    rhs: xxx,
                },
                Stmt::Assign { lhs: ret, rhs: xsq },
            ],
        });

        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let cube_result_ref = m.push_expr(Expr::Var(cube_result));
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![cube_result_ref, vpn]));
        let sq_read = m.push_expr(Expr::Var(sq_result));
        let cube_read = m.push_expr(Expr::Var(cube_result));
        let total = m.push_expr(Expr::Binary(va_ir::BinOp::Add, sq_read, cube_read));

        m.analog = vec![
            Stmt::Assign {
                lhs: sq_result,
                rhs: call,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: total,
            },
        ];
        m
    }

    #[test]
    fn output_argument_write_back_and_the_ordinary_return_value_both_work() {
        let inst = build_instance(&output_arg_function_ir(), &[0, 1], &mut 2).unwrap();
        let v = 2.0;
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // I = sq_result + cube_result = v^2 + v^3.
        let expected_i = v * v + v * v * v;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-9);
        // dI/dv = 2v + 3v^2, through *both* the ordinary return value and the output argument's
        // write-back -- both must carry a correct gradient, not just a correct value.
        let expected_grad = 2.0 * v + 3.0 * v * v;
        assert!((sink.jac(0, 0) - expected_grad).abs() / expected_grad < 1e-9);

        let h = 1e-6;
        let residual_at = |v: f64| {
            let mut s = DenseStamp::new(2);
            inst.load(
                &[v, 0.0],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.residual[0]
        };
        let fd = (residual_at(v + h) - residual_at(v - h)) / (2.0 * h);
        assert!(
            (sink.jac(0, 0) - fd).abs() < 1e-6,
            "{} vs {}",
            sink.jac(0, 0),
            fd
        );
    }

    /// `inout` reads the caller's current value in *and* writes the final value back:
    /// `analog function real bump; inout counter; input delta; begin counter=counter+delta;
    /// bump=counter; end endfunction`, called from a module that first sets its own `counter`
    /// variable to a known value, calls `bump(counter, delta)` (discarding the ordinary return
    /// value), then contributes `counter` -- now updated by the call's write-back -- afterward.
    fn inout_arg_function_ir(delta: f64) -> Module {
        let mut m = Module::new("inout_arg_func");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "delta".into(),
            default: delta,
            min: None,
            max: None,
        }];
        // Function scope: VarId(0)=counter (inout), VarId(1)=delta_in (input), VarId(2)=ret.
        // Module scope: VarId(3)=counter_outer (the caller's own variable), VarId(4)=discarded
        // return-value binding.
        m.vars = vec![
            VarDecl {
                name: "counter".into(),
            },
            VarDecl {
                name: "delta_in".into(),
            },
            VarDecl {
                name: "bump".into(),
            },
            VarDecl {
                name: "counter_outer".into(),
            },
            VarDecl {
                name: "scratch".into(),
            },
        ];
        let (counter, delta_in, ret, counter_outer, scratch) =
            (VarId(0), VarId(1), VarId(2), VarId(3), VarId(4));

        let counter_read = m.push_expr(Expr::Var(counter));
        let delta_read = m.push_expr(Expr::Var(delta_in));
        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Add, counter_read, delta_read));
        let counter_read2 = m.push_expr(Expr::Var(counter));
        m.functions.push(Function {
            name: "bump".into(),
            args: vec![counter, delta_in],
            arg_dirs: vec![va_ir::ArgDir::Inout, va_ir::ArgDir::Input],
            ret,
            body: vec![
                Stmt::Assign {
                    lhs: counter,
                    rhs: sum,
                },
                Stmt::Assign {
                    lhs: ret,
                    rhs: counter_read2,
                },
            ],
        });

        let initial = m.push_expr(Expr::Const(10.0));
        let delta_param = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let counter_outer_ref = m.push_expr(Expr::Var(counter_outer));
        let call = m.push_expr(Expr::CallUser(
            FuncId(0),
            vec![counter_outer_ref, delta_param],
        ));
        let counter_outer_read = m.push_expr(Expr::Var(counter_outer));

        m.analog = vec![
            Stmt::Assign {
                lhs: counter_outer,
                rhs: initial,
            },
            Stmt::Assign {
                lhs: scratch,
                rhs: call,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: counter_outer_read,
            },
        ];
        m
    }

    #[test]
    fn inout_argument_reads_the_initial_value_in_and_writes_the_final_value_back() {
        let delta = 1.5;
        let inst = build_instance(&inout_arg_function_ir(delta), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.0, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        // counter_outer starts at 10.0, `bump` adds `delta` and writes the sum back.
        assert!((sink.residual[0] - (10.0 + delta)).abs() < 1e-9);
        // The contributed value came from `counter_outer` (a constant, not a node voltage), so
        // it carries no gradient at all.
        assert_eq!(sink.jac(0, 0), 0.0);
    }

    /// The LRM restricts an `output`/`inout` actual argument to a plain variable (there must be
    /// somewhere to write the result back to); passing an arbitrary expression instead must be
    /// rejected, not silently ignored or miscomputed.
    #[test]
    fn output_argument_that_is_not_a_plain_variable_is_rejected() {
        let mut m = Module::new("bad_output_arg");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "out".into() }, VarDecl { name: "f".into() }];
        let (out, ret) = (VarId(0), VarId(1));

        let one = m.push_expr(Expr::Const(1.0));
        m.functions.push(Function {
            name: "f".into(),
            args: vec![out],
            arg_dirs: vec![va_ir::ArgDir::Output],
            ret,
            body: vec![
                Stmt::Assign { lhs: out, rhs: one },
                Stmt::Assign { lhs: ret, rhs: one },
            ],
        });

        // The actual argument is `2*V(p,n)`, not a plain variable.
        let vpn = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let two = m.push_expr(Expr::Const(2.0));
        let bad_arg = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, two, vpn));
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![bad_arg]));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: call,
        }];

        assert!(matches!(
            build_instance(&m, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    /// A function whose body region-selects like a real compact model's utility routine would:
    /// `if (x>0) ret=<bad, unassigned read>; else ret=x;`. At the all-zero validate point
    /// `x=0`, a real call would take the `else` arm -- proving `build_instance` still catches
    /// the broken `then` arm requires the same "validate every arm unconditionally" split
    /// already used for the top-level analog block, now applied *inside* a function call too.
    #[test]
    fn validate_catches_an_error_in_a_functions_own_untaken_arm() {
        let mut m = Module::new("func_soundness");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        // VarId(0) = arg `x`, VarId(1) = ret `myfunc`, VarId(2) = never assigned anywhere.
        m.vars = vec![
            VarDecl { name: "x".into() },
            VarDecl {
                name: "myfunc".into(),
            },
            VarDecl {
                name: "unassigned".into(),
            },
        ];

        let x_read = m.push_expr(Expr::Var(VarId(0)));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, x_read, zero));
        let bad_read = m.push_expr(Expr::Var(VarId(2)));
        let then_body = vec![Stmt::Assign {
            lhs: VarId(1),
            rhs: bad_read,
        }];
        let x_read2 = m.push_expr(Expr::Var(VarId(0)));
        let else_body = vec![Stmt::Assign {
            lhs: VarId(1),
            rhs: x_read2,
        }];

        m.functions.push(Function {
            name: "myfunc".into(),
            args: vec![VarId(0)],
            arg_dirs: vec![va_ir::ArgDir::Input],
            ret: VarId(1),
            body: vec![Stmt::If {
                cond,
                then_: then_body,
                else_: else_body,
            }],
        });

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![v]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: call,
        }];

        assert!(matches!(
            build_instance(&m, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    #[test]
    fn a_function_called_with_the_wrong_argument_count_is_rejected() {
        let mut m = Module::new("wrong_arity");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }, VarDecl { name: "sq".into() }];

        let x_a = m.push_expr(Expr::Var(VarId(0)));
        let x_b = m.push_expr(Expr::Var(VarId(0)));
        let mul = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, x_a, x_b));
        m.functions.push(Function {
            name: "sq".into(),
            args: vec![VarId(0)],
            arg_dirs: vec![va_ir::ArgDir::Input],
            ret: VarId(1),
            body: vec![Stmt::Assign {
                lhs: VarId(1),
                rhs: mul,
            }],
        });

        // Called with zero arguments instead of the one `sq` declares.
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: call,
        }];

        assert!(matches!(
            build_instance(&m, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    #[test]
    fn a_contribution_inside_a_function_body_is_rejected() {
        let mut m = Module::new("contribute_in_function");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.vars = vec![VarDecl { name: "x".into() }, VarDecl { name: "bad".into() }];

        let zero = m.push_expr(Expr::Const(0.0));
        m.functions.push(Function {
            name: "bad".into(),
            args: vec![VarId(0)],
            arg_dirs: vec![va_ir::ArgDir::Input],
            ret: VarId(1),
            body: vec![Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: zero,
            }],
        });

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let call = m.push_expr(Expr::CallUser(FuncId(0), vec![v]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: call,
        }];

        assert!(matches!(
            build_instance(&m, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    /// Which of the three real "coefficient-scaled `ddt`" syntactic shapes to build.
    #[derive(Clone, Copy)]
    enum ScaledDdtShape {
        MulBefore,
        MulAfter,
        Div,
    }

    /// `I(p,n) <+ coeff*ddt(c0*V(p,n));` (or the `ddt(..)*coeff`/`ddt(..)/coeff` variants) --
    /// the real "polarity/multiplicity-scaled charge term" idiom (`bsim4.va`'s `I(gi,si) <+
    /// BSIM4type * ddt(qgate);`), with both `c0` and `coeff` as parameters (so `coeff` is
    /// provably parameter-only and the fold into the charge channel is exact).
    fn scaled_ddt_ir(c0: f64, coeff: f64, shape: ScaledDdtShape) -> Module {
        let mut m = Module::new("scaled_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "c0".into(),
                default: c0,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "coeff".into(),
                default: coeff,
                min: Some(0.0),
                max: None,
            },
        ];

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0_e, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let coeff_e = m.push_expr(Expr::Param(va_ir::ParamId(1)));

        let value = match shape {
            ScaledDdtShape::MulBefore => {
                m.push_expr(Expr::Binary(va_ir::BinOp::Mul, coeff_e, ddt_q))
            }
            ScaledDdtShape::MulAfter => {
                m.push_expr(Expr::Binary(va_ir::BinOp::Mul, ddt_q, coeff_e))
            }
            ScaledDdtShape::Div => m.push_expr(Expr::Binary(va_ir::BinOp::Div, ddt_q, coeff_e)),
        };

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];
        m
    }

    #[test]
    fn scaled_ddt_folds_into_the_charge_channel_all_three_shapes() {
        let (c0, coeff, v) = (1e-12, 2.5, 3.0);

        for (shape, expected_q, expected_dq) in [
            (ScaledDdtShape::MulBefore, coeff * c0 * v, coeff * c0),
            (ScaledDdtShape::MulAfter, coeff * c0 * v, coeff * c0),
            (ScaledDdtShape::Div, c0 * v / coeff, c0 / coeff),
        ] {
            let inst = build_instance(&scaled_ddt_ir(c0, coeff, shape), &[0, 1], &mut 2).unwrap();
            let mut sink = DenseStamp::new(2);
            inst.load(
                &[v, 0.0],
                &ANALYSIS_DC,
                &mut va_abi::ModelState::stateless(),
                &mut sink,
            );
            assert!((sink.charge[0] - expected_q).abs() / expected_q < 1e-9);
            assert!((sink.dcharge[0] - expected_dq).abs() / expected_dq < 1e-9);
            // No charge should leak onto the resistive residual channel.
            assert_eq!(sink.residual[0], 0.0);

            // §5 (charge-channel analogue): the charge Jacobian must match a central finite
            // difference on the charge value itself.
            let charge_at = |v: f64| {
                let mut s = DenseStamp::new(2);
                inst.load(
                    &[v, 0.0],
                    &ANALYSIS_DC,
                    &mut va_abi::ModelState::stateless(),
                    &mut s,
                );
                s.charge[0]
            };
            let h = 1e-6;
            let fd = (charge_at(v + h) - charge_at(v - h)) / (2.0 * h);
            assert!(
                (sink.dcharge[0] - fd).abs() < 1e-6 * expected_dq.abs(),
                "dcharge {} vs fd {}",
                sink.dcharge[0],
                fd
            );
        }
    }

    /// `I(p,n) <+ V(p,n) * ddt(c0*V(p,n));` -- the coefficient is the branch voltage itself, not
    /// a parameter, so `coeff(x)*dQ/dt != d(coeff*Q)/dt` in general: folding it into the charge
    /// channel the way [`scaled_ddt_ir`] does would silently produce the wrong Jacobian. Must
    /// stay rejected exactly like an unscaled nested `ddt` always has.
    #[test]
    fn a_non_parameter_coefficient_scaling_ddt_is_still_rejected() {
        let mut m = Module::new("bad_scaled_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "c0".into(),
            default: 1e-12,
            min: Some(0.0),
            max: None,
        }];

        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let v2 = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0_e, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, v2, ddt_q));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];

        assert_product_rule_is_carried(&m, &[0, 1]);
    }

    /// **The §5 gate for the product-rule path**: with a bias-dependent charge coefficient, the
    /// *assembled* Jacobian the integrator forms — `jacobian + coeff·dcharge`, exactly the
    /// combination `va_transient::newton_step` builds — must match a central finite difference
    /// of the assembled residual.
    ///
    /// This is the test that decides whether the split across two channels is right. Checking
    /// `jacobian` alone would pass while the whole feature was wrong: the missing `c·∂q/∂x` term
    /// lives entirely in `dcharge`, which is precisely what used to be dropped (and why the
    /// construct was refused). The model is
    ///
    /// ```verilog
    /// I(p,n) <+ V(p,n) * ddt(c0 * V(p,n));
    /// ```
    ///
    /// whose coefficient `V(p,n)` is genuinely bias-dependent, so `is_param_only` refuses to
    /// fold it into the plain charge channel and it must take the product-rule path.
    #[test]
    fn the_product_rule_jacobian_matches_a_central_finite_difference() {
        let mut m = Module::new("bias_dependent_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "c0".into(),
            default: 2e-9,
            min: None,
            max: None,
        }];
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let v2 = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, v2, ddt_q));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];

        let inst = build_instance(&m, &[0, 1], &mut 2).expect("builds");

        // A backward-Euler step: coeff = 1/h, no previous-rate weight. The committed history is
        // a nonzero previous charge, so `dq/dt` is not trivially proportional to `q` — a test
        // with `q_prev = 0` would pass even if the history term were mishandled.
        let h = 1e-6_f64;
        let coeff = 1.0 / h;
        let ctx = va_abi::AnalysisCtx::transient(1e-3)
            .with_ddt(coeff, 0.0)
            .with_initial_step(false);
        let committed = vec![7e-10_f64; inst.state_len()];

        // The assembled residual at row 0, as `va_transient` forms it: residual + coeff*charge.
        // (`offset` is a per-row constant, independent of `x`, so it cancels in the difference.)
        let assembled = |x0: f64| -> f64 {
            let mut next = vec![0.0; committed.len()];
            let mut st = va_abi::ModelState::new(&committed, &mut next);
            let mut sink = DenseStamp::new(1);
            inst.load(&[x0], &ctx, &mut st, &mut sink);
            sink.residual[0] + coeff * sink.charge[0]
        };

        let x0 = 0.35_f64;
        let mut next = vec![0.0; committed.len()];
        let mut st = va_abi::ModelState::new(&committed, &mut next);
        let mut sink = DenseStamp::new(1);
        inst.load(&[x0], &ctx, &mut st, &mut sink);
        let analytic = sink.jacobian[0] + coeff * sink.dcharge[0];

        let d = 1e-6_f64;
        let fd = (assembled(x0 + d) - assembled(x0 - d)) / (2.0 * d);

        let scale = analytic.abs().max(fd.abs()).max(1.0);
        assert!(
            (analytic - fd).abs() / scale < 1e-6,
            "assembled Jacobian {analytic} disagrees with central difference {fd}"
        );

        // And the half that would be silently missing without the `dcharge` stamp is not
        // negligible — otherwise the test above could pass with `grad_ddt` dropped entirely.
        assert!(
            (coeff * sink.dcharge[0]).abs() > 0.1 * analytic.abs(),
            "the charge-channel half must be a real part of the answer, not rounding"
        );
    }

    /// Assembled-Jacobian finite-difference check over a whole `dim`-unknown system: compares
    /// `jacobian + coeff·dcharge` against a central difference of `residual + coeff·charge`,
    /// which is exactly the combination `va_transient::newton_step` forms. Any stamping path
    /// that receives a `grad_ddt`-carrying `Dual` and ignores it shows up here as an entry that
    /// is zero where the difference is not.
    fn assert_assembled_jacobian_matches_fd(
        inst: &dyn va_abi::ModelInstance,
        dim: usize,
        x0: &[f64],
        coeff: f64,
    ) {
        let ctx = va_abi::AnalysisCtx::transient(1e-3)
            .with_ddt(coeff, 0.0)
            .with_initial_step(false);
        let committed = vec![4e-10_f64; inst.state_len()];

        let assembled = |x: &[f64]| -> Vec<f64> {
            let mut next = vec![0.0; committed.len()];
            let mut st = va_abi::ModelState::new(&committed, &mut next);
            let mut sink = DenseStamp::new(dim);
            inst.load(x, &ctx, &mut st, &mut sink);
            (0..dim)
                .map(|r| sink.residual[r] + coeff * sink.charge[r])
                .collect()
        };

        let mut next = vec![0.0; committed.len()];
        let mut st = va_abi::ModelState::new(&committed, &mut next);
        let mut sink = DenseStamp::new(dim);
        inst.load(x0, &ctx, &mut st, &mut sink);

        let d = 1e-6_f64;
        for col in 0..dim {
            let (mut xp, mut xm) = (x0.to_vec(), x0.to_vec());
            xp[col] += d;
            xm[col] -= d;
            let (fp, fm) = (assembled(&xp), assembled(&xm));
            for row in 0..dim {
                let fd = (fp[row] - fm[row]) / (2.0 * d);
                let analytic =
                    sink.jacobian[row * dim + col] + coeff * sink.dcharge[row * dim + col];
                let scale = analytic.abs().max(fd.abs()).max(1.0);
                assert!(
                    (analytic - fd).abs() / scale < 1e-6,
                    "row {row} col {col}: analytic {analytic} vs central difference {fd}"
                );
            }
        }
    }

    /// Two nodes plus a bias-dependent charge coefficient on branch 0, as
    /// `the_product_rule_jacobian_matches_a_central_finite_difference` builds it — factored out
    /// so the accumulator shapes below can reuse it.
    fn bias_dependent_ddt_module(name: &str) -> (Module, ExprId) {
        let mut m = Module::new(name);
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "c0".into(),
            default: 2e-9,
            min: None,
            max: None,
        }];
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let v2 = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, v2, ddt_q));
        (m, value)
    }

    /// A contribution carrying a bias-dependent charge coefficient, re-read through a bare
    /// `I(branch)` probe so it is routed through `stamp_flow_current_accumulators`.
    ///
    /// That path stamped `total.grad` only. Lifting the blanket refusal on a nested `ddt` in a
    /// resistive term (2026-08-30) newly exposed it: the model now *builds*, and its accumulator
    /// row's Jacobian was zero where the truth is not — the same silent-Newton-degradation the
    /// refusal had been preventing, moved one path over.
    #[test]
    fn the_flow_current_accumulator_carries_the_product_rule_term() {
        let (mut m, value) = bias_dependent_ddt_module("acc_product_rule");
        m.nodes.push(NodeDecl {
            name: "o".into(),
            discipline: Discipline::Electrical,
            abstol: None,
        });
        m.ports.push(vec![NodeId(2)]);
        m.branches.push(Branch {
            p: NodeId(2),
            n: NodeId(1),
        });
        // `I(o,n) <+ I(p,n);` — the bare flow probe that creates the accumulator.
        let probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Flow,
            branch: BranchId(0),
        }));
        m.analog = vec![
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value,
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(1),
                },
                value: probe,
            },
        ];

        let mut next_unknown = 3;
        let inst = build_instance(&m, &[0, 1, 2], &mut next_unknown).expect("builds");
        // The accumulator claims its own unknown during the build, so the solution vector is
        // only the right length once `next_unknown` has settled.
        let mut x0 = vec![0.0; next_unknown];
        x0[0] = 0.35;
        x0[2] = 0.1;
        assert_assembled_jacobian_matches_fd(inst.as_ref(), next_unknown, &x0, 1e6);
    }

    /// The same coefficient inside an `idt` integrand — `stamp_idt_accumulators` stamped
    /// `d.grad` only, and was exposed by the same change for the same reason.
    #[test]
    fn the_idt_accumulator_carries_the_product_rule_term() {
        let (mut m, value) = bias_dependent_ddt_module("idt_product_rule");
        let idt = m.push_expr(Expr::Call(Builtin::Idt, vec![value]));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: idt,
        }];

        let mut next_unknown = 2;
        let inst = build_instance(&m, &[0, 1], &mut next_unknown).expect("builds");
        let mut x0 = vec![0.0; next_unknown];
        x0[0] = 0.35;
        if next_unknown > 2 {
            x0[2] = 0.05;
        }
        assert_assembled_jacobian_matches_fd(inst.as_ref(), next_unknown, &x0, 1e6);
    }

    /// A bias-dependent charge coefficient (`c(x)·ddt(q)`, a `ddt` nested inside a resistive
    /// term) builds and takes the product-rule path.
    ///
    /// These shapes used to be rejected outright, and the tests that pin them were the negative
    /// controls proving `lower::is_param_only`'s safety check bit. That check still bites — it is
    /// what keeps such a term *out* of the plain charge channel, where folding it in would be
    /// wrong — but the term is no longer refused: it takes the product-rule path instead, which
    /// is method-independent (§ `build_instance`).
    ///
    /// Asserts the term actually reaches a channel, not merely that the build succeeds: a build
    /// that quietly dropped the charge would otherwise pass, which is the exact failure mode the
    /// original refusal existed to prevent.
    fn assert_product_rule_is_carried(m: &Module, terminals: &[usize]) {
        let mut next = terminals.len();
        let inst = build_instance(m, terminals, &mut next).expect("builds");

        let ctx = va_abi::AnalysisCtx::transient(1e-3)
            .with_ddt(1e6, 0.0)
            .with_initial_step(false);
        let committed = vec![4e-10_f64; inst.state_len()];
        let mut nxt = vec![0.0; committed.len()];
        let mut st = va_abi::ModelState::new(&committed, &mut nxt);
        let mut sink = DenseStamp::new(next);
        let mut x = vec![0.0; next];
        x[0] = 0.35;
        inst.load(&x, &ctx, &mut st, &mut sink);
        assert!(
            sink.dcharge.iter().any(|v| *v != 0.0),
            "the bias-dependent charge coefficient reached no channel — dcharge is all zero"
        );
    }

    /// `I(p,n) <+ ddt(c0*V(p,n))*coeff1*coeff2;` -- `ekv26.va`'s real `ddt(qjd)*TYPE*M` shape,
    /// parsing as `(ddt(qjd)*TYPE)*M`: two parameter-only coefficients nested two multiplications
    /// deep, outside what the single-level `charge_term_shape` used to inspect.
    #[test]
    fn doubly_nested_multiplication_ddt_folds_into_the_charge_channel() {
        let mut m = Module::new("doubly_scaled_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "c0".into(),
                default: 1e-12,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "type_".into(),
                default: -1.0,
                min: None,
                max: None,
            },
            Param {
                name: "m".into(),
                default: 4.0,
                min: Some(0.0),
                max: None,
            },
        ];

        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let type_ = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let mult = m.push_expr(Expr::Param(va_ir::ParamId(2)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, vprobe));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let ddt_type = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, ddt_q, type_));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, ddt_type, mult));

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];

        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        let (c0v, type_v, mv, v) = (1e-12, -1.0, 4.0, 0.7);
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        let expected_q = type_v * mv * c0v * v;
        let expected_dq = type_v * mv * c0v;
        assert!((sink.charge[0] - expected_q).abs() / expected_q.abs() < 1e-9);
        assert!((sink.dcharge[0] - expected_dq).abs() / expected_dq.abs() < 1e-9);
        assert_eq!(sink.residual[0], 0.0);
    }

    /// Build `I(p,n) <+ sign * (a*ddt(c1*V(p,n)) - b*ddt(c2*V(p,n))) * qon;` — `ekv3.va`'s real
    /// gate-charge shape (`I(d,g) <+ SIGN_M * (d_gt_s*ddt(QD) + s_gt_d*ddt(QS)) * QON;`), with
    /// the inner `+` turned into a `-` so the distributed signs are actually discriminated
    /// rather than both defaulting to `+1`.
    ///
    /// Params: 0 = `c1`, 1 = `c2`, 2 = `a`, 3 = `b`, 4 = `sign`, 5 = `qon`.
    fn scaled_sum_of_ddts_ir() -> Module {
        let mut m = Module::new("scaled_sum_of_ddts");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        let named = |name: &str, default: f64| Param {
            name: name.into(),
            default,
            min: None,
            max: None,
        };
        m.params = vec![
            named("c1", 3e-12),
            named("c2", 5e-12),
            named("a", 2.0),
            named("b", 7.0),
            named("sign", -1.0),
            named("qon", 0.25),
        ];

        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let mul = |m: &mut Module, l, r| m.push_expr(Expr::Binary(va_ir::BinOp::Mul, l, r));
        let param = |m: &mut Module, i| m.push_expr(Expr::Param(va_ir::ParamId(i)));

        let c1 = param(&mut m, 0);
        let q1 = mul(&mut m, c1, vprobe);
        let ddt1 = m.push_expr(Expr::Call(Builtin::Ddt, vec![q1]));
        let a = param(&mut m, 2);
        let left = mul(&mut m, a, ddt1);

        let c2 = param(&mut m, 1);
        let q2 = mul(&mut m, c2, vprobe);
        let ddt2 = m.push_expr(Expr::Call(Builtin::Ddt, vec![q2]));
        let b = param(&mut m, 3);
        let right = mul(&mut m, b, ddt2);

        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Sub, left, right));
        let sign = param(&mut m, 4);
        let scaled = mul(&mut m, sign, sum);
        let qon = param(&mut m, 5);
        let value = mul(&mut m, scaled, qon);

        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];
        m
    }

    /// A coefficient scaling a parenthesised *sum* of `ddt`s must distribute over both of them.
    /// Before this was recognized, `charge_term_shape` saw a `Mul` whose operands were an `Add`
    /// (not a `ddt` shape) and `QON` (not a `ddt` shape), fell back to the resistive channel, and
    /// `ad::eval` rejected the still-nested `ddt` — which is exactly why `external/ekv3.va`
    /// failed `va-cli check --codegen`.
    #[test]
    fn a_coefficient_distributes_over_a_parenthesised_sum_of_ddts() {
        let m = scaled_sum_of_ddts_ir();
        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();

        let (c1, c2, a, b, sign, qon) = (3e-12, 5e-12, 2.0, 7.0, -1.0, 0.25);
        let v = 0.7;
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // sign*qon*(a*c1 - b*c2)*V, i.e. the hand-distributed form of the contribution.
        let gain = sign * qon * (a * c1 - b * c2);
        let expected_q = gain * v;
        assert!(
            (sink.charge[0] - expected_q).abs() / expected_q.abs() < 1e-9,
            "charge {} != {expected_q}",
            sink.charge[0]
        );
        assert!(
            (sink.dcharge[0] - gain).abs() / gain.abs() < 1e-9,
            "dcharge {} != {gain}",
            sink.dcharge[0]
        );
        // Nothing may leak into the residual: the whole contribution is charge.
        assert_eq!(sink.residual[0], 0.0);
        // Node `n` is the mirror of node `p`.
        assert!((sink.charge[1] + expected_q).abs() / expected_q.abs() < 1e-9);
    }

    /// The negative control for the test above: distribution applies only when *every* term of
    /// the sum is a charge shape. `(g*V(p,n) + ddt(c*V(p,n))) * k` mixes a resistive term with a
    /// charge one, and splitting it would mean synthesising a `g*V(p,n)*k` expression this crate
    /// has no way to write into the IR arena — so it must stay rejected rather than silently
    /// dropping the resistive half or the coefficient.
    #[test]
    fn a_coefficient_over_a_mixed_resistive_and_ddt_sum_is_still_rejected() {
        let mut m = scaled_sum_of_ddts_ir();
        // Rebuild the contribution as `(a*V(p,n) - b*ddt(c2*V(p,n))) * qon` — same skeleton, but
        // the left half of the sum is now an ordinary resistive term.
        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let a = m.push_expr(Expr::Param(va_ir::ParamId(2)));
        let left = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, a, vprobe));

        let c2 = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let q2 = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c2, vprobe));
        let ddt2 = m.push_expr(Expr::Call(Builtin::Ddt, vec![q2]));
        let b = m.push_expr(Expr::Param(va_ir::ParamId(3)));
        let right = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, b, ddt2));

        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Sub, left, right));
        let qon = m.push_expr(Expr::Param(va_ir::ParamId(5)));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, sum, qon));
        m.analog = vec![Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        }];

        assert_product_rule_is_carried(&m, &[0, 1]);
    }

    /// `real t0; t0 = ddt(c0*V(p,n)); if (cond>0.0) begin I(p,n)<+V(p,n)*g+t0; end else
    /// I(p,n)<+0.0;` -- `angelov_gan.va`'s real `T0 = ddt(Ldc*I(rf,si)); // Avoid analog operator
    /// in if/else block` idiom: a `ddt` assigned to a local variable in a straight-line
    /// assignment, then read back (as a bare additive term) inside a later `if`'s arm.
    fn ddt_via_local_variable_ir(cond_positive: bool) -> Module {
        let mut m = Module::new("ddt_via_local_var");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "c0".into(),
                default: 1e-12,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "g".into(),
                default: 2.0,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "cond".into(),
                default: if cond_positive { 1.0 } else { -1.0 },
                min: None,
                max: None,
            },
        ];
        m.vars = vec![VarDecl { name: "t0".into() }];
        let t0_id = VarId(0);

        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let g = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let cond = m.push_expr(Expr::Param(va_ir::ParamId(2)));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond_ge = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, cond, zero));

        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, vprobe));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let vg = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, vprobe, g));
        let t0_read = m.push_expr(Expr::Var(t0_id));
        let then_value = m.push_expr(Expr::Binary(va_ir::BinOp::Add, vg, t0_read));
        let else_value = m.push_expr(Expr::Const(0.0));

        m.analog = vec![
            Stmt::Assign {
                lhs: t0_id,
                rhs: ddt_q,
            },
            Stmt::If {
                cond: cond_ge,
                then_: vec![Stmt::Contribute {
                    target: Access {
                        kind: AccessKind::Flow,
                        branch: BranchId(0),
                    },
                    value: then_value,
                }],
                else_: vec![Stmt::Contribute {
                    target: Access {
                        kind: AccessKind::Flow,
                        branch: BranchId(0),
                    },
                    value: else_value,
                }],
            },
        ];
        m
    }

    #[test]
    fn ddt_result_assigned_to_a_local_variable_is_tracked_to_its_contribution() {
        let inst = build_instance(&ddt_via_local_variable_ir(true), &[0, 1], &mut 2).unwrap();
        let (c0, g, v) = (1e-12, 2.0, 0.6);
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );

        // The resistive term (`V(p,n)*g`) and the substituted `ddt(c0*V(p,n))` charge term must
        // both land correctly, exactly as if the source had written `I(p,n) <+ V(p,n)*g +
        // ddt(c0*V(p,n));` directly instead of through `t0`.
        assert!(
            (sink.residual[0] - v * g).abs() < 1e-12,
            "{}",
            sink.residual[0]
        );
        assert!((sink.charge[0] - c0 * v).abs() / (c0 * v) < 1e-9);
        assert!((sink.dcharge[0] - c0).abs() / c0 < 1e-9);
    }

    /// The `else` arm never reads `t0` at all, so it must build and run cleanly even though `t0`
    /// only ever holds a `ddt` shape (never a real assigned value at runtime, since that
    /// assignment is never lowered to an ordinary `LoweredStmt::Assign` -- see `DdtVars`'s doc
    /// comment) -- proving the substitution doesn't leak a spurious requirement onto a branch
    /// that has no use for it.
    #[test]
    fn ddt_via_local_variable_else_arm_does_not_need_it() {
        let inst = build_instance(&ddt_via_local_variable_ir(false), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.6, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[0], 0.0);
        assert_eq!(sink.charge[0], 0.0);
    }

    /// `real t0; t0 = ddt(c0*V(p,n)); if (cond>0.0) begin t0 = 5.0; end I(p,n) <+ t0;` -- `t0`
    /// holds a `ddt` shape *before* the `if`, but only one arm reassigns it to an ordinary value,
    /// and the read is *after* the `if` (not inside either arm). Which arm ran determines what
    /// `t0` actually is, so `lower_stmt` must forget the pre-`if` `ddt` substitution once *any*
    /// arm reassigns `t0` (see `invalidate_ddt_vars`) rather than keep treating the post-`if` read
    /// as the stale `ddt` shape -- the dangerous failure mode this guards against is stamping a
    /// plausible-looking but *wrong* charge value (as if the read had still been `ddt(c0*V(p,n))`)
    /// regardless of which arm actually ran.
    ///
    /// The two cases below land on different sides of a real, pre-existing, `ddt`-unrelated gap:
    /// `GeneratedModel::validate` visits *both* arms of an `if` unconditionally (by design, to
    /// catch an error hiding in the untaken arm — see `lower`'s module doc comment), so it always
    /// leaves `t0` bound by the time it reaches the post-`if` read, regardless of `cond`. When the
    /// reassigning arm is also the one that runs for real, this happens to match — the post-`if`
    /// read correctly sees that arm's real value. When it *isn't* (`t0` never actually gets
    /// assigned at the real operating point, since the `ddt`-shape pre-`if` assignment is
    /// symbolic-only — see `DdtVars`'s doc comment), `load` cannot fail loudly to report it:
    /// [`GeneratedModel::load`] deliberately swallows a load-time error from [`GeneratedModel::run`]
    /// (`let _ = self.run(...)`, on the documented assumption that post-validation this "cannot
    /// happen"), so the sink is simply left unstamped rather than stamped wrong — silent, but
    /// safe. Both outcomes are what this test locks in; neither is a regression this specific fix
    /// introduces.
    fn reassigned_in_one_arm_only_ir(reassigning_arm_runs: bool) -> Module {
        let mut m = Module::new("reassigned_in_one_arm");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![
            Param {
                name: "c0".into(),
                default: 1e-12,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "cond".into(),
                default: if reassigning_arm_runs { 1.0 } else { -1.0 },
                min: None,
                max: None,
            },
        ];
        m.vars = vec![VarDecl { name: "t0".into() }];
        let t0_id = VarId(0);

        let vprobe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0 = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let cond = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond_ge = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, cond, zero));

        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0, vprobe));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let five = m.push_expr(Expr::Const(5.0));
        let t0_read = m.push_expr(Expr::Var(t0_id));

        m.analog = vec![
            Stmt::Assign {
                lhs: t0_id,
                rhs: ddt_q,
            },
            Stmt::If {
                cond: cond_ge,
                then_: vec![Stmt::Assign {
                    lhs: t0_id,
                    rhs: five,
                }],
                else_: vec![],
            },
            Stmt::Contribute {
                target: Access {
                    kind: AccessKind::Flow,
                    branch: BranchId(0),
                },
                value: t0_read,
            },
        ];
        m
    }

    #[test]
    fn reassignment_in_one_arm_invalidates_the_ddt_substitution_after_the_if() {
        // The reassigning arm ran: the post-`if` read must see the real value it set (5.0),
        // treated as an ordinary resistive term -- not the discarded `ddt` shape (which would
        // instead have produced a nonzero `charge[0]` here).
        let inst = build_instance(&reassigned_in_one_arm_only_ir(true), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.6, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[0], 5.0);
        assert_eq!(sink.charge[0], 0.0);

        // The reassigning arm didn't run: `t0` was never actually assigned at runtime, so `run`
        // aborts on the first statement and `load` leaves the sink exactly as it started --
        // stamping nothing is safe; stamping a value as though `t0` still held `ddt(c0*V(p,n))`
        // would not have been.
        let inst = build_instance(&reassigned_in_one_arm_only_ir(false), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[0.6, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert_eq!(sink.residual[0], 0.0);
        assert_eq!(sink.charge[0], 0.0);
    }

    /// The real `devsign`/`ct` idiom (`bsimbulk.va`: `if (TYPE==\`ntype) devsign=1; else
    /// devsign=-1;` then `I(sbulk,si) <+ devsign*ddt(...)`; `asmhemt.va`: `if (V(g)>voff)
    /// ct=ctrap3; else ct=1.0e-9;` then `I(trap1) <+ ct*ddt(V(trap1));`) -- a local variable
    /// scaling a `ddt` term, assigned via `if`/`else` where *every* assigned value is
    /// parameter-only even though the guard condition itself reads the branch voltage. The
    /// guard doesn't matter; only what actually gets assigned does.
    fn if_assigned_coefficient_ddt_ir(c0: f64) -> Module {
        let mut m = Module::new("if_assigned_coeff_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "c0".into(),
            default: c0,
            min: Some(0.0),
            max: None,
        }];
        // VarId(0) = `devsign`.
        m.vars = vec![VarDecl {
            name: "devsign".into(),
        }];

        let v_guard = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, v_guard, zero));
        let one = m.push_expr(Expr::Const(1.0));
        let minus_one = m.push_expr(Expr::Const(-1.0));
        let if_stmt = Stmt::If {
            cond,
            then_: vec![Stmt::Assign {
                lhs: VarId(0),
                rhs: one,
            }],
            else_: vec![Stmt::Assign {
                lhs: VarId(0),
                rhs: minus_one,
            }],
        };

        let devsign = m.push_expr(Expr::Var(VarId(0)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0_e, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, devsign, ddt_q));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        };

        m.analog = vec![if_stmt, contribute];
        m
    }

    #[test]
    fn if_assigned_local_variable_coefficient_scales_ddt_in_both_regions() {
        let c0 = 1e-12;
        let inst = build_instance(&if_assigned_coefficient_ddt_ir(c0), &[0, 1], &mut 2).unwrap();

        // V > 0: devsign = 1.
        let v = 3.0;
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        assert!((sink.charge[0] - c0 * v).abs() / (c0 * v) < 1e-9);
        assert!((sink.dcharge[0] - c0).abs() / c0 < 1e-9);

        // V < 0: devsign = -1.
        let v = -3.0;
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let expected_q = -c0 * v;
        assert!((sink.charge[0] - expected_q).abs() / expected_q.abs() < 1e-9);
        assert!((sink.dcharge[0] + c0).abs() / c0 < 1e-9);
    }

    /// `a = W/L; b = a*2; I(p,n) <+ b*ddt(q);` -- a short parameter-only dependency chain
    /// through two local variables, proving [`lower::param_only_vars`]'s fixed point actually
    /// propagates transitively rather than only recognizing a variable assigned directly from a
    /// bare `Const`/`Param`.
    #[test]
    fn a_transitive_chain_of_parameter_only_variables_scales_ddt() {
        let mut m = Module::new("chained_coeff_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        let (w, l, c0) = (2.0, 4.0, 1e-12);
        m.params = vec![
            Param {
                name: "w".into(),
                default: w,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "l".into(),
                default: l,
                min: Some(0.0),
                max: None,
            },
            Param {
                name: "c0".into(),
                default: c0,
                min: Some(0.0),
                max: None,
            },
        ];
        // VarId(0) = `a`, VarId(1) = `b`.
        m.vars = vec![VarDecl { name: "a".into() }, VarDecl { name: "b".into() }];

        let w_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let l_e = m.push_expr(Expr::Param(va_ir::ParamId(1)));
        let w_over_l = m.push_expr(Expr::Binary(va_ir::BinOp::Div, w_e, l_e));
        let a_assign = Stmt::Assign {
            lhs: VarId(0),
            rhs: w_over_l,
        };

        let a_read = m.push_expr(Expr::Var(VarId(0)));
        let two = m.push_expr(Expr::Const(2.0));
        let a_times_2 = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, a_read, two));
        let b_assign = Stmt::Assign {
            lhs: VarId(1),
            rhs: a_times_2,
        };

        let b_read = m.push_expr(Expr::Var(VarId(1)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let c0_e = m.push_expr(Expr::Param(va_ir::ParamId(2)));
        let q = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, c0_e, v));
        let ddt_q = m.push_expr(Expr::Call(Builtin::Ddt, vec![q]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, b_read, ddt_q));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        };

        m.analog = vec![a_assign, b_assign, contribute];

        let inst = build_instance(&m, &[0, 1], &mut 2).unwrap();
        let v = 3.0;
        let mut sink = DenseStamp::new(2);
        inst.load(
            &[v, 0.0],
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let b_value = (w / l) * 2.0;
        let expected_q = b_value * c0 * v;
        assert!((sink.charge[0] - expected_q).abs() / expected_q < 1e-9);
        let expected_dq = b_value * c0;
        assert!((sink.dcharge[0] - expected_dq).abs() / expected_dq < 1e-9);
    }

    /// A local variable assigned a parameter-only value in one arm but the branch voltage
    /// itself in the other (`if (cond) ct=c0; else ct=V(p,n);`) must still be rejected as a
    /// `ddt` coefficient -- not *every* assignment is parameter-only, so folding it in would be
    /// unsound on whichever path takes the `else` arm.
    #[test]
    fn a_variable_only_sometimes_assigned_a_parameter_only_value_is_still_rejected() {
        let mut m = Module::new("unsound_coeff_ddt");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
                abstol: None,
            },
        ];
        m.ports = vec![vec![NodeId(0)], vec![NodeId(1)]];
        m.branches = vec![Branch {
            p: NodeId(0),
            n: NodeId(1),
        }];
        m.params = vec![Param {
            name: "c0".into(),
            default: 1e-12,
            min: Some(0.0),
            max: None,
        }];
        m.vars = vec![VarDecl { name: "ct".into() }];

        let v_guard = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let zero = m.push_expr(Expr::Const(0.0));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, v_guard, zero));
        let c0_e = m.push_expr(Expr::Param(va_ir::ParamId(0)));
        let v_else = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let if_stmt = Stmt::If {
            cond,
            then_: vec![Stmt::Assign {
                lhs: VarId(0),
                rhs: c0_e,
            }],
            else_: vec![Stmt::Assign {
                lhs: VarId(0),
                rhs: v_else, // not parameter-only!
            }],
        };

        let ct_read = m.push_expr(Expr::Var(VarId(0)));
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let ddt_v = m.push_expr(Expr::Call(Builtin::Ddt, vec![v]));
        let value = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, ct_read, ddt_v));
        let contribute = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value,
        };

        m.analog = vec![if_stmt, contribute];

        assert_product_rule_is_carried(&m, &[0, 1]);
    }

    /// Build a two-node module whose single flow contribution is `V(p,n)/R + <noise>`, where
    /// `<noise>` is supplied by `make_noise` given the module under construction. Shared by the
    /// noise-lowering tests below.
    fn module_with_noise(make_noise: impl FnOnce(&mut Module) -> ExprId) -> Module {
        let mut m = resistor_ir();
        let noise = make_noise(&mut m);
        let Some(Stmt::Contribute { value, .. }) = m.analog.last().cloned() else {
            panic!("resistor_ir's last statement should be the contribution");
        };
        let sum = m.push_expr(Expr::Binary(va_ir::BinOp::Add, value, noise));
        *m.analog.last_mut().unwrap() = Stmt::Contribute {
            target: Access {
                kind: AccessKind::Flow,
                branch: BranchId(0),
            },
            value: sum,
        };
        m
    }

    /// A `white_noise` term reaches Interface β's noise channel with its argument evaluated at
    /// the operating point — and, critically, leaves `load` untouched: the resistive stamp is
    /// bit-identical to the same model without the noise term, which is the LRM's "value is zero
    /// outside noise analysis" rule made concrete.
    #[test]
    fn white_noise_lowers_to_the_noise_channel_without_perturbing_load() {
        let plain = resistor_ir();
        let noisy = module_with_noise(|m| {
            let pwr = m.push_expr(Expr::Const(4.2e-23));
            m.push_expr(Expr::Call(Builtin::WhiteNoise, vec![pwr]))
        });

        let plain_inst = build_instance(&plain, &[0, 1], &mut 2).expect("plain builds");
        let noisy_inst = build_instance(&noisy, &[0, 1], &mut 2).expect("noisy builds");

        // Identical resistive stamps.
        let x = [2.0, 0.0];
        let (mut a, mut b) = (DenseStamp::new(2), DenseStamp::new(2));
        plain_inst.load(
            &x,
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut a,
        );
        noisy_inst.load(
            &x,
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut b,
        );
        assert_eq!(
            a.residual, b.residual,
            "noise must not perturb the residual"
        );
        assert_eq!(
            a.jacobian, b.jacobian,
            "noise must not perturb the Jacobian"
        );

        // The plain model reports no sources; the noisy one reports exactly its declared PSD,
        // across the contribution's own branch.
        let mut sink = va_abi::noise::CollectedNoise::default();
        plain_inst.noise(&x, &ANALYSIS_DC, &mut sink);
        assert!(sink.sources.is_empty(), "{:?}", sink.sources);

        let mut sink = va_abi::noise::CollectedNoise::default();
        noisy_inst.noise(&x, &ANALYSIS_DC, &mut sink);
        assert_eq!(sink.sources.len(), 1);
        let (p, n, source) = &sink.sources[0];
        assert_eq!(
            (*p, *n),
            (0, 1),
            "source sits across the contributed branch"
        );
        assert_eq!(*source, va_abi::noise::NoiseSource::White { psd: 4.2e-23 });
    }

    /// A `flicker_noise` term carries both its numerator and its exponent through, and the
    /// numerator is evaluated at the operating point — here `V(p,n)`, so the recorded
    /// coefficient tracks the bias rather than being frozen at build time.
    #[test]
    fn flicker_noise_carries_a_bias_dependent_coefficient_and_its_exponent() {
        let noisy = module_with_noise(|m| {
            let v = m.push_expr(Expr::Probe(Access {
                kind: AccessKind::Potential,
                branch: BranchId(0),
            }));
            let exp = m.push_expr(Expr::Const(1.0));
            m.push_expr(Expr::Call(Builtin::FlickerNoise, vec![v, exp]))
        });
        let inst = build_instance(&noisy, &[0, 1], &mut 2).expect("builds");

        for bias in [2.0, 5.0] {
            let mut sink = va_abi::noise::CollectedNoise::default();
            inst.noise(&[bias, 0.0], &ANALYSIS_DC, &mut sink);
            assert_eq!(
                sink.sources[0].2,
                va_abi::noise::NoiseSource::Flicker {
                    coeff: bias,
                    exponent: 1.0
                },
                "coefficient must track the operating point"
            );
        }
    }

    /// A `noise_table` term reaches Interface β as a table of pairs — reassembled from the
    /// flattened `Const` arguments — and, like the other two shapes, leaves `load` untouched.
    #[test]
    fn noise_table_lowers_to_a_tabulated_source_without_perturbing_load() {
        let plain = resistor_ir();
        let noisy = module_with_noise(|m| {
            // 1 Hz → 4e-20, 10 Hz → 1e-20: a real slope, so a "flat, take the first power"
            // implementation would be caught.
            let args = [1.0, 4e-20, 10.0, 1e-20]
                .into_iter()
                .map(|v| m.push_expr(Expr::Const(v)))
                .collect();
            m.push_expr(Expr::Call(Builtin::NoiseTable, args))
        });

        let plain_inst = build_instance(&plain, &[0, 1], &mut 2).expect("plain builds");
        let noisy_inst = build_instance(&noisy, &[0, 1], &mut 2).expect("noisy builds");

        let x = [2.0, 0.0];
        let (mut a, mut b) = (DenseStamp::new(2), DenseStamp::new(2));
        plain_inst.load(
            &x,
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut a,
        );
        noisy_inst.load(
            &x,
            &ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut b,
        );
        assert_eq!(
            a.residual, b.residual,
            "a table must not perturb the residual"
        );
        assert_eq!(
            a.jacobian, b.jacobian,
            "a table must not perturb the Jacobian"
        );

        let mut sink = va_abi::noise::CollectedNoise::default();
        noisy_inst.noise(&x, &ANALYSIS_DC, &mut sink);
        assert_eq!(sink.sources.len(), 1);
        let (p, n, source) = &sink.sources[0];
        assert_eq!(
            (*p, *n),
            (0, 1),
            "source sits across the contributed branch"
        );
        assert_eq!(
            *source,
            va_abi::noise::NoiseSource::Table {
                points: vec![(1.0, 4e-20), (10.0, 1e-20)],
                interp: va_abi::noise::TableInterp::Linear,
            }
        );
        // And it evaluates as a table, not as either endpoint: halfway *in frequency*.
        let mid = source.psd_at(5.5);
        assert!((mid - 2.5e-20).abs() < 1e-33, "got {mid}");
    }

    /// The two table builtins differ only in the interpolation rule they hand to Interface β,
    /// and that rule must survive lowering — a `noise_table_log` source emitted as `Linear`
    /// would be wrong by orders of magnitude between its points while looking perfectly
    /// plausible at every knot.
    #[test]
    fn noise_table_log_reaches_the_abi_carrying_the_logarithmic_rule() {
        let build = |b: Builtin| {
            module_with_noise(|m| {
                let args = [1.0, 1.0, 1e6, 1e-6]
                    .into_iter()
                    .map(|v| m.push_expr(Expr::Const(v)))
                    .collect();
                m.push_expr(Expr::Call(b, args))
            })
        };
        let expect_interp = |b: Builtin, want: va_abi::noise::TableInterp| {
            let inst = build_instance(&build(b), &[0, 1], &mut 2).expect("builds");
            let mut sink = va_abi::noise::CollectedNoise::default();
            inst.noise(&[2.0, 0.0], &ANALYSIS_DC, &mut sink);
            let va_abi::noise::NoiseSource::Table { interp, .. } = sink.sources[0].2 else {
                panic!("a tabulated source");
            };
            assert_eq!(interp, want);
            sink.sources[0].2.clone()
        };

        let lin = expect_interp(Builtin::NoiseTable, va_abi::noise::TableInterp::Linear);
        let log = expect_interp(Builtin::NoiseTableLog, va_abi::noise::TableInterp::Log);

        // Same points, same knots, genuinely different curve in between: at 1 kHz the log rule
        // gives the 1/f value 1e-3, the linear rule is still essentially at its 1.0 endpoint.
        assert_eq!(lin.psd_at(1.0), log.psd_at(1.0));
        assert!(
            (log.psd_at(1e3) - 1e-3).abs() < 1e-12,
            "{}",
            log.psd_at(1e3)
        );
        assert!(lin.psd_at(1e3) > 0.99, "{}", lin.psd_at(1e3));
    }

    /// A table whose flattened arguments don't pair up cannot be a `(frequency, power)` list;
    /// `va-frontend` never produces one, so this can only come from a hand-built IR — it is
    /// still rejected rather than silently dropping the trailing value.
    #[test]
    fn an_odd_length_noise_table_is_a_build_error() {
        let odd = module_with_noise(|m| {
            let args = [1.0, 4e-20, 10.0]
                .into_iter()
                .map(|v| m.push_expr(Expr::Const(v)))
                .collect();
            m.push_expr(Expr::Call(Builtin::NoiseTable, args))
        });
        assert!(matches!(
            build_instance(&odd, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    /// A noise call that isn't a bare top-level term can't be pulled into the noise channel, and
    /// would otherwise evaluate quietly to zero — declaring a source that contributes nothing.
    /// That must be a build error, not a silent drop (§ `lower::noise_term_shape`).
    #[test]
    fn a_scaled_noise_call_is_rejected_rather_than_silently_dropped() {
        let scaled = module_with_noise(|m| {
            let pwr = m.push_expr(Expr::Const(4.2e-23));
            let call = m.push_expr(Expr::Call(Builtin::WhiteNoise, vec![pwr]));
            let two = m.push_expr(Expr::Const(2.0));
            m.push_expr(Expr::Binary(va_ir::BinOp::Mul, two, call))
        });
        assert!(matches!(
            build_instance(&scaled, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }

    // -----------------------------------------------------------------------------------
    // Tier A: the analysis-context constructs (§6 change, 2026-08-06).
    // -----------------------------------------------------------------------------------

    /// The `1 kΩ` resistor with one extra additive term in its flow contribution. Same shape as
    /// [`module_with_noise`], reused for the analysis-context builtins so each test differs only
    /// in the term it adds.
    fn resistor_plus(make_term: impl FnOnce(&mut Module) -> ExprId) -> Module {
        module_with_noise(make_term)
    }

    fn phase_bit(name: &str) -> f64 {
        f64::from(va_ir::phase_bit(name).expect("a listed phase name"))
    }

    /// `analysis("tran")` must answer the analysis actually running, not the one elaboration
    /// guessed. This is the fold that used to make a DC-init branch fire at *every* transient
    /// timepoint while the transient branch never fired at all.
    #[test]
    fn analysis_answers_the_running_analysis_rather_than_a_baked_in_constant() {
        // I(p,n) <+ V(p,n)/R + analysis("tran") * 1e-3 — a 1 mA offset only during transient.
        let m = resistor_plus(|m| {
            let mask = m.push_expr(Expr::Const(phase_bit("tran")));
            let call = m.push_expr(Expr::Call(Builtin::Analysis, vec![mask]));
            let amp = m.push_expr(Expr::Const(1e-3));
            m.push_expr(Expr::Binary(va_ir::BinOp::Mul, call, amp))
        });
        let inst = build_instance(&m, &[0, 1], &mut 2).expect("builds");

        let residual_under = |ctx: &va_abi::AnalysisCtx| {
            let mut s = DenseStamp::new(1);
            inst.load(&[1.0], ctx, &mut va_abi::ModelState::stateless(), &mut s);
            s.residual[0]
        };

        // V/R at 1 V across 1 kΩ is 1 mA; the offset doubles it, but only in transient.
        assert!((residual_under(&ANALYSIS_DC) - 1e-3).abs() < 1e-15);
        assert!((residual_under(&va_abi::AnalysisCtx::ac()) - 1e-3).abs() < 1e-15);
        assert!((residual_under(&va_abi::AnalysisCtx::transient(0.0)) - 2e-3).abs() < 1e-15);

        // An any-of mask matches either named analysis, and neither of two unnamed ones.
        let m2 = resistor_plus(|m| {
            let mask = m.push_expr(Expr::Const(phase_bit("dc") + phase_bit("ac")));
            let call = m.push_expr(Expr::Call(Builtin::Analysis, vec![mask]));
            let amp = m.push_expr(Expr::Const(1e-3));
            m.push_expr(Expr::Binary(va_ir::BinOp::Mul, call, amp))
        });
        let inst2 = build_instance(&m2, &[0, 1], &mut 2).expect("builds");
        let r2 = |ctx: &va_abi::AnalysisCtx| {
            let mut s = DenseStamp::new(1);
            inst2.load(&[1.0], ctx, &mut va_abi::ModelState::stateless(), &mut s);
            s.residual[0]
        };
        assert!((r2(&ANALYSIS_DC) - 2e-3).abs() < 1e-15);
        assert!((r2(&va_abi::AnalysisCtx::ac()) - 2e-3).abs() < 1e-15);
        assert!((r2(&va_abi::AnalysisCtx::transient(0.0)) - 1e-3).abs() < 1e-15);
    }

    /// `$abstime` tracks the context's time, and — §5's rule — contributes **zero** to the
    /// Jacobian, confirmed against a central finite difference rather than assumed. Time is not
    /// a function of the solution vector, so a nonzero gradient here would corrupt Newton.
    #[test]
    fn abstime_reads_the_clock_and_has_no_gradient() {
        // I(p,n) <+ V(p,n)/R + 1e-3 * $abstime  (a 1 mA/s current ramp).
        let k = 1e-3;
        let m = resistor_plus(|m| {
            let t = m.push_expr(Expr::Call(Builtin::Abstime, Vec::new()));
            let k = m.push_expr(Expr::Const(k));
            m.push_expr(Expr::Binary(va_ir::BinOp::Mul, k, t))
        });
        let inst = build_instance(&m, &[0, 1], &mut 2).expect("builds");

        let stamp_at = |ctx: &va_abi::AnalysisCtx, v: f64| {
            let mut s = DenseStamp::new(1);
            inst.load(&[v], ctx, &mut va_abi::ModelState::stateless(), &mut s);
            s
        };

        // A static solve reads t = 0: the ramp term vanishes and the model is a plain resistor.
        assert!((stamp_at(&ANALYSIS_DC, 1.0).residual[0] - 1e-3).abs() < 1e-15);
        // In transient it tracks the clock, linearly and independently of bias.
        for t in [0.0, 1e-6, 2.5e-3] {
            let s = stamp_at(&va_abi::AnalysisCtx::transient(t), 1.0);
            assert!(
                (s.residual[0] - (1e-3 + k * t)).abs() < 1e-15,
                "at t={t}: {}",
                s.residual[0]
            );
        }

        // §5: the Jacobian is still exactly the resistor's, at any time — central difference.
        let ctx = va_abi::AnalysisCtx::transient(3.3e-3);
        let h = 1e-6;
        let fd =
            (stamp_at(&ctx, 1.0 + h).residual[0] - stamp_at(&ctx, 1.0 - h).residual[0]) / (2.0 * h);
        let analytic = stamp_at(&ctx, 1.0).jac(0, 0);
        assert!((analytic - 1e-3).abs() < 1e-18, "analytic {analytic}");
        assert!(
            (analytic - fd).abs() < 1e-12,
            "analytic {analytic} vs fd {fd}"
        );
    }

    /// `ac_stim` reaches the excitation channel during AC and is silent everywhere else —
    /// and, like a noise term, it never perturbs `load`'s resistive stamp. That combination is
    /// what lets a model declare its AC drive unconditionally and still solve a DC point.
    #[test]
    fn ac_stim_excites_only_in_ac_and_leaves_the_resistive_stamp_alone() {
        let plain = resistor_ir();
        // I(p,n) <+ V(p,n)/R + ac_stim(2.0, π/2)  — a purely imaginary 2 A stimulus.
        let (mag, ph) = (2.0, std::f64::consts::FRAC_PI_2);
        let stim = resistor_plus(|m| {
            let mask = m.push_expr(Expr::Const(phase_bit("ac")));
            let mag = m.push_expr(Expr::Const(mag));
            let ph = m.push_expr(Expr::Const(ph));
            m.push_expr(Expr::Call(Builtin::AcStim, vec![mask, mag, ph]))
        });

        let plain_inst = build_instance(&plain, &[0, 1], &mut 2).expect("plain builds");
        let stim_inst = build_instance(&stim, &[0, 1], &mut 2).expect("stim builds");

        // Resistive stamps identical in every analysis — the stimulus is not in `G`.
        for ctx in [
            ANALYSIS_DC,
            va_abi::AnalysisCtx::ac(),
            va_abi::AnalysisCtx::transient(1e-6),
        ] {
            let (mut a, mut b) = (DenseStamp::new(1), DenseStamp::new(1));
            plain_inst.load(&[1.0], &ctx, &mut va_abi::ModelState::stateless(), &mut a);
            stim_inst.load(&[1.0], &ctx, &mut va_abi::ModelState::stateless(), &mut b);
            assert_eq!(a.residual, b.residual, "residual differs under {ctx:?}");
            assert_eq!(a.jacobian, b.jacobian, "jacobian differs under {ctx:?}");
        }

        // The excitation itself appears only in AC.
        let excitation = |ctx: &va_abi::AnalysisCtx| {
            let mut s = DenseStamp::new(1);
            stim_inst.load(&[1.0], ctx, &mut va_abi::ModelState::stateless(), &mut s);
            s.excitation[0]
        };
        assert_eq!(excitation(&ANALYSIS_DC), (0.0, 0.0));
        assert_eq!(
            excitation(&va_abi::AnalysisCtx::transient(1e-6)),
            (0.0, 0.0)
        );

        let (re, im) = excitation(&va_abi::AnalysisCtx::ac());
        assert!(re.abs() < 1e-15, "cos(π/2) should vanish, got {re}");
        assert!((im - mag).abs() < 1e-15, "imag {im} vs {mag}");
    }

    /// `bound_step` reaches the timestep channel during transient and nowhere else — there is no
    /// step to bound in a static or small-signal solve.
    #[test]
    fn bound_step_reaches_the_timestep_channel_only_in_transient() {
        let mut m = resistor_ir();
        let dt = m.push_expr(Expr::Const(2.5e-9));
        m.analog.insert(0, Stmt::BoundStep(dt));
        let inst = build_instance(&m, &[0, 1], &mut 2).expect("builds");

        let bound_under = |ctx: &va_abi::AnalysisCtx| {
            let mut s = DenseStamp::new(1);
            inst.load(&[1.0], ctx, &mut va_abi::ModelState::stateless(), &mut s);
            s.bound_step
        };
        assert_eq!(
            bound_under(&va_abi::AnalysisCtx::transient(0.0)),
            Some(2.5e-9)
        );
        assert_eq!(bound_under(&ANALYSIS_DC), None);
        assert_eq!(bound_under(&va_abi::AnalysisCtx::ac()), None);
    }

    /// A `bound_step` guarded by an `if` is a *conditional* request: it applies only when the
    /// operating point selects the arm holding it. That is the whole reason it stays a statement
    /// in the control-flow walk instead of becoming a module-level property.
    #[test]
    fn a_guarded_bound_step_applies_only_when_its_arm_runs() {
        // if (V(p,n) > 0.5) bound_step(1n);
        let mut m = resistor_ir();
        let v = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let half = m.push_expr(Expr::Const(0.5));
        let cond = m.push_expr(Expr::Binary(va_ir::BinOp::Gt, v, half));
        let dt = m.push_expr(Expr::Const(1e-9));
        m.analog.insert(
            0,
            Stmt::If {
                cond,
                then_: vec![Stmt::BoundStep(dt)],
                else_: Vec::new(),
            },
        );
        let inst = build_instance(&m, &[0, 1], &mut 2).expect("builds");

        let bound_at = |v: f64| {
            let mut s = DenseStamp::new(1);
            inst.load(
                &[v],
                &va_abi::AnalysisCtx::transient(0.0),
                &mut va_abi::ModelState::stateless(),
                &mut s,
            );
            s.bound_step
        };
        assert_eq!(bound_at(1.0), Some(1e-9));
        assert_eq!(bound_at(0.1), None);
    }

    /// An `ac_stim` buried where the lowering can't split it out would evaluate to its
    /// LRM-mandated zero and vanish without trace. Rejected at build time instead, exactly as a
    /// scaled noise call is.
    #[test]
    fn a_scaled_ac_stim_is_rejected_rather_than_silently_dropped() {
        let scaled = resistor_plus(|m| {
            let mask = m.push_expr(Expr::Const(phase_bit("ac")));
            let mag = m.push_expr(Expr::Const(1.0));
            let ph = m.push_expr(Expr::Const(0.0));
            let call = m.push_expr(Expr::Call(Builtin::AcStim, vec![mask, mag, ph]));
            let two = m.push_expr(Expr::Const(2.0));
            m.push_expr(Expr::Binary(va_ir::BinOp::Mul, two, call))
        });
        assert!(matches!(
            build_instance(&scaled, &[0, 1], &mut 2),
            Err(CodegenError::Unsupported(_))
        ));
    }
}
