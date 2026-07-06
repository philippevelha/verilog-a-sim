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
//! # Limitations
//!
//! - `ddt` is recognised only as a top-level additive term (optionally negated), matching how
//!   compact models are written (`I <+ resistive + ddt(charge)`, `V <+ resistive + ddt(charge)`);
//!   `ddt` nested inside a nonlinear function is rejected later by the AD evaluator.
//! - User-defined analog functions in the analog block are not yet lowered. The IR
//!   (Interface α) models these, but codegen v0 rejects them with [`CodegenError::Unsupported`].

use crate::CodegenError;
use std::collections::{BTreeSet, HashMap};
use va_ir::{AccessKind, BinOp, BranchId, Builtin, Expr, ExprId, Module, Stmt, UnOp, VarId};

/// One additive term of a contribution: a signed expression handle.
#[derive(Clone, Copy, Debug)]
pub struct Term {
    /// `+1.0` or `-1.0`, accumulated from `-`/unary-negation while flattening.
    pub sign: f64,
    /// The (already ddt-stripped) expression to evaluate.
    pub expr: ExprId,
}

/// A single branch contribution, split into resistive and charge channels.
#[derive(Clone, Debug)]
pub struct Contribution {
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
    /// `ddt` arguments summed into the charge/charge-Jacobian channel.
    pub charge: Vec<Term>,
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

/// A lowered, evaluable representation of a module's analog block.
#[derive(Debug, Default)]
pub struct Lowered {
    /// Total number of local unknowns: one per IR node, plus one per entry in
    /// [`Self::branch_currents`].
    pub n_unknowns: usize,
    /// Statements in source order (assignments and contributions only — see Limitations).
    pub stmts: Vec<LoweredStmt>,
    /// One entry per branch that receives a potential contribution anywhere in the module, in
    /// ascending [`BranchId`] order (the deterministic order their local terminal slots are
    /// allocated in, past `module.nodes.len()`).
    pub branch_currents: Vec<BranchCurrent>,
}

/// Lower a module's analog block into a [`Lowered`] plan.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset
/// (user-defined functions, malformed `ddt`).
pub fn lower(module: &Module) -> Result<Lowered, CodegenError> {
    let (flow_branches, potential_branches) = branch_kinds(&module.analog);

    let mut branch_currents = Vec::new();
    let mut slot_of_branch = HashMap::new();
    let mut next_slot = module.nodes.len();
    for id in potential_branches {
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

    let mut stmts = Vec::new();
    for stmt in &module.analog {
        lower_stmt(module, stmt, &slot_of_branch, &mut stmts)?;
    }
    Ok(Lowered {
        n_unknowns: next_slot,
        stmts,
        branch_currents,
    })
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

fn lower_stmt(
    module: &Module,
    stmt: &Stmt,
    slot_of_branch: &HashMap<u32, usize>,
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
                match module.expr(term.expr) {
                    Expr::Call(Builtin::Ddt, args) => {
                        if args.len() != 1 {
                            return Err(unsupported("ddt expects exactly one argument"));
                        }
                        charge.push(Term {
                            sign: term.sign,
                            expr: args[0],
                        });
                    }
                    _ => resistive.push(term),
                }
            }

            let branch_slot = match target.kind {
                AccessKind::Flow => None,
                AccessKind::Potential => Some(slot_of_branch[&target.branch.0]),
            };

            out.push(LoweredStmt::Contribute(Contribution {
                p_slot: br.p.0 as usize,
                n_slot: br.n.0 as usize,
                branch_slot,
                resistive,
                charge,
            }));
            Ok(())
        }
        Stmt::Assign { lhs, rhs } => {
            out.push(LoweredStmt::Assign {
                lhs: *lhs,
                rhs: *rhs,
            });
            Ok(())
        }
        Stmt::Block(body) => {
            for s in body {
                lower_stmt(module, s, slot_of_branch, out)?;
            }
            Ok(())
        }
        Stmt::If { cond, then_, else_ } => {
            let mut then_lowered = Vec::new();
            for s in then_ {
                lower_stmt(module, s, slot_of_branch, &mut then_lowered)?;
            }
            let mut else_lowered = Vec::new();
            for s in else_ {
                lower_stmt(module, s, slot_of_branch, &mut else_lowered)?;
            }
            out.push(LoweredStmt::If {
                cond: *cond,
                then_: then_lowered,
                else_: else_lowered,
            });
            Ok(())
        }
        Stmt::While { cond, body } => {
            let mut body_lowered = Vec::new();
            for s in body {
                lower_stmt(module, s, slot_of_branch, &mut body_lowered)?;
            }
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
            let mut init_lowered = Vec::new();
            lower_stmt(module, init, slot_of_branch, &mut init_lowered)?;
            let mut step_lowered = Vec::new();
            lower_stmt(module, step, slot_of_branch, &mut step_lowered)?;
            let mut body_lowered = Vec::new();
            for s in body {
                lower_stmt(module, s, slot_of_branch, &mut body_lowered)?;
            }
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
            for s in body {
                lower_stmt(module, s, slot_of_branch, &mut body_lowered)?;
            }
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
            let mut lowered_arms = Vec::new();
            for arm in arms {
                let mut body_lowered = Vec::new();
                for s in &arm.body {
                    lower_stmt(module, s, slot_of_branch, &mut body_lowered)?;
                }
                lowered_arms.push(LoweredCaseArm {
                    labels: arm.labels.clone(),
                    body: body_lowered,
                });
            }
            let mut default_lowered = Vec::new();
            for s in default {
                lower_stmt(module, s, slot_of_branch, &mut default_lowered)?;
            }
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

fn unsupported(msg: &str) -> CodegenError {
    CodegenError::Unsupported(msg.to_string())
}
