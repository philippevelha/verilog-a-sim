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
//! - A branch gets *either* flow or potential contributions, never both (see `lower`'s
//!   Limitations); no loops/`case`, no user-defined analog functions (see [`lower`]).
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
use lower::{Contribution, Lowered, LoweredStmt};
use std::cell::RefCell;
use std::collections::HashMap;
use thiserror::Error;
use va_abi::{ModelInstance, StampSink, UnknownKind};
use va_ir::Module;

/// Thermal voltage `kT/q` at ~300 K, in volts. Matches `va_abi::reference::diode::VT_300K`
/// so a generated diode reproduces the reference diode's stamps.
pub const VT: f64 = 0.025_852;

/// Ambient temperature for `$temperature`, in kelvin.
pub const TEMP: f64 = 300.0;

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
/// own nodes, in node order (unchanged from before potential contributions existed). If the
/// module has one or more branches with a potential contribution, each needs its own auxiliary
/// branch-current unknown too; `build_instance` allocates those itself from `next_unknown`
/// (incrementing it once per such branch, in ascending `BranchId` order — see
/// [`lower::Lowered::branch_currents`]), so the caller's own next-free-index counter (e.g.
/// `va-cli`'s device-building loop) stays in sync without having to pre-compute how many extra
/// unknowns a module will need.
///
/// # Errors
///
/// Returns [`CodegenError`] if `terminals` is the wrong length or the analog block contains
/// a construct outside the v0 subset (validated eagerly so [`ModelInstance::load`] cannot
/// fail).
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
    for _ in &lowered.branch_currents {
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
    fn ctx<'a>(&'a self, x: &'a [f64]) -> Ctx<'a> {
        let branch_current_slots = self
            .lowered
            .branch_currents
            .iter()
            .map(|bc| (bc.branch.0, bc.local_slot))
            .collect();
        Ctx {
            module: &self.module,
            params: &self.params,
            x,
            terminals: &self.terminals,
            vt: self.vt,
            temp: self.temp,
            vars: RefCell::new(HashMap::new()),
            branch_current_slots,
            mixed_branch_potential_used: RefCell::new(std::collections::HashSet::new()),
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
        for stmt in stmts {
            match stmt {
                LoweredStmt::Assign { lhs, rhs } => {
                    let d = eval(ctx, *rhs)?;
                    ctx.set_var(*lhs, d);
                }
                LoweredStmt::Contribute(c) => self.stamp(ctx, c, sink),
                LoweredStmt::If { cond, then_, else_ } => {
                    let taken = if eval(ctx, *cond)?.value != 0.0 {
                        then_
                    } else {
                        else_
                    };
                    self.run(ctx, taken, sink)?;
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
                    self.run(ctx, taken, sink)?;
                }
                LoweredStmt::While { cond, body } => {
                    let mut iters = 0usize;
                    while eval(ctx, *cond)?.value != 0.0 {
                        self.run(ctx, body, sink)?;
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
                    self.run(ctx, init, sink)?;
                    let mut iters = 0usize;
                    while eval(ctx, *cond)?.value != 0.0 {
                        self.run(ctx, body, sink)?;
                        self.run(ctx, step, sink)?;
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
                        self.run(ctx, body, sink)?;
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
        let ctx = self.ctx(&[]);
        Self::validate_stmts(&ctx, &self.lowered.stmts)
    }

    fn validate_stmts(ctx: &Ctx, stmts: &[LoweredStmt]) -> Result<(), CodegenError> {
        for stmt in stmts {
            match stmt {
                LoweredStmt::Assign { lhs, rhs } => {
                    let d = eval(ctx, *rhs)?;
                    ctx.set_var(*lhs, d);
                }
                LoweredStmt::Contribute(c) => {
                    for term in c.resistive.iter().chain(c.charge.iter()) {
                        eval(ctx, term.expr)?;
                    }
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
                    sink.residual(gp, i.value);
                    sink.residual(gn, -i.value);
                    for (slot, &dg) in i.grad.iter().enumerate() {
                        if dg != 0.0 {
                            let gk = self.terminals[slot];
                            sink.jacobian(gp, gk, dg);
                            sink.jacobian(gn, gk, -dg);
                        }
                    }
                }

                if !c.charge.is_empty() {
                    let Ok(q) = Self::sum_terms(ctx, &c.charge) else {
                        return;
                    };
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
                }

                if !c.charge.is_empty() {
                    let Ok(q) = Self::sum_terms(ctx, &c.charge) else {
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
            }
        }
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

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let ctx = self.ctx(x);
        self.stamp_branch_currents(x, sink);
        // Post-validation this cannot fail; `run` already stops early rather than stamping
        // from a corrupted variable environment if it somehow does (see `run`'s doc comment).
        let _ = self.run(&ctx, &self.lowered.stmts, sink);
        self.finalize_mixed_branch_currents(&ctx, sink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::stamps::DenseStamp;
    use va_ir::{
        Access, AccessKind, Branch, BranchId, Builtin, Discipline, Expr, Module, NodeDecl, NodeId,
        Param, Stmt, VarDecl, VarId,
    };

    /// Build the resistor IR: `I(p,n) <+ V(p,n) / R`, R defaulting to 1 kΩ.
    fn resistor_ir() -> Module {
        let mut m = Module::new("resistor");
        m.nodes = vec![
            NodeDecl {
                name: "p".into(),
                discipline: Discipline::Electrical,
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
            },
            NodeDecl {
                name: "c".into(),
                discipline: Discipline::Electrical,
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[v], &mut sink);

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
            inst.load(&[v], &mut s);
            s.charge[0]
        };

        let v = 0.6;
        let h = 1e-6;
        let fd = (charge_at(v + h) - charge_at(v - h)) / (2.0 * h);

        let mut sink = DenseStamp::new(1);
        inst.load(&[v], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[0.0], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[1.0], &mut sink);
        assert!((sink.residual[0] - g_pos).abs() / g_pos < 1e-12);
        assert!((sink.jac(0, 0) - g_pos).abs() / g_pos < 1e-12);

        // V(p,n) = -1 V: the `else` arm, conductance g_neg -- a different value *and* a
        // different Jacobian, proving the selected branch's own gradient is what's stamped,
        // not the other arm's.
        let mut sink = DenseStamp::new(1);
        inst.load(&[-1.0], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[2.0], &mut sink);
        // Same hand-checked values as va_abi's resistor_stamp_by_hand.
        assert!((sink.residual[0] - 2e-3).abs() < 1e-15);
        assert!((sink.jac(0, 0) - 1e-3).abs() < 1e-18);
        assert_eq!(sink.charge[0], 0.0);
    }

    #[test]
    fn capacitor_stamps_only_charge() {
        let inst = build_instance(&capacitor_ir(), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(&[3.0], &mut sink);
        // Q = C*V = 1pF * 3V = 3e-12; dQ/dV = C = 1e-12. No resistive current.
        assert!((sink.charge[0] - 3e-12).abs() < 1e-24);
        assert!((sink.dcharge[0] - 1e-12).abs() < 1e-27);
        assert_eq!(sink.residual[0], 0.0);
    }

    #[test]
    fn diode_current_and_conductance() {
        let inst = build_instance(&diode_ir(), &[0, 1], &mut 2).unwrap();
        let vd = 0.6;
        let mut sink = DenseStamp::new(1);
        inst.load(&[vd], &mut sink);

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
            inst.load(&[vd], &mut s);
            s.residual[0]
        };

        let vd = 0.6;
        let h = 1e-6;
        let fd = (residual_at(vd + h) - residual_at(vd - h)) / (2.0 * h);

        let mut sink = DenseStamp::new(1);
        inst.load(&[vd], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[vp, vn, ib], &mut sink);

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
            inst.load(&[vp, vn, ib], &mut s);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[0.0, 0.0, ib], &mut sink);

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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[vp, vn, stray_ib], &mut sink);

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
        inst.load(&[vp, vn, ib], &mut sink);

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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[1.0, 0.0], &mut sink);
        assert!((sink.residual[0] - g1).abs() / g1 < 1e-12);
        assert!((sink.jac(0, 0) - g1).abs() / g1 < 1e-12);

        // sel=99.0 matches nothing -- falls through to `default`.
        let inst = build_instance(&case_ir(99.0, g0, g1, gdef), &[0, 1], &mut 2).unwrap();
        let mut sink = DenseStamp::new(2);
        inst.load(&[1.0, 0.0], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[v, 0.0], &mut sink);
        // acc after 3 iterations = 3*v; I = 3*g*v.
        let expected_i = n * g * v;
        assert!((sink.residual[0] - expected_i).abs() / expected_i < 1e-12);
        let expected_g = n * g;
        assert!((sink.jac(0, 0) - expected_g).abs() / expected_g < 1e-12);

        // §5: the gradient accumulated *through* the loop must match a central finite difference.
        let residual_at = |v: f64| {
            let mut s = DenseStamp::new(2);
            inst.load(&[v, 0.0], &mut s);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[v, 0.0], &mut sink);
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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[0.0, 0.0], &mut sink);

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
            },
            NodeDecl {
                name: "n".into(),
                discipline: Discipline::Electrical,
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
        inst.load(&[0.0, 0.0], &mut sink);
        // The post-loop contribution never ran, so the residual is untouched.
        assert_eq!(sink.residual[0], 0.0);
    }
}
