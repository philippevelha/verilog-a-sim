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
//! # Limitations
//!
//! - A branch may receive *either* flow contributions or potential contributions, never both
//!   (anywhere in the module, including mutually-exclusive `if`/`else` arms) — `lower` rejects
//!   the mix outright. Real compact models do sometimes gate between the two per-branch by a
//!   *parameter* (e.g. the widely-reused `` `collapsibleR `` idiom), but that needs the
//!   constraint row itself to change shape depending on which arm ran, which this lowering
//!   doesn't attempt; a module doing that stays [`CodegenError::Unsupported`] for now.
//! - `ddt` is recognised only as a top-level additive term (optionally negated), matching how
//!   compact models are written (`I <+ resistive + ddt(charge)`, `V <+ resistive + ddt(charge)`);
//!   `ddt` nested inside a nonlinear function is rejected later by the AD evaluator.
//! - Loops/`case` and user-defined analog functions in the analog block are not yet lowered.
//!   The IR (Interface α) models these, but codegen v0 rejects them with
//!   [`CodegenError::Unsupported`].

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
/// `crate::GeneratedModel::stamp_branch_currents` stamps two things for every entry,
/// unconditionally, exactly once per [`crate::GeneratedModel::load`] call regardless of which
/// (if any) `if`/`else` arm actually contributes to it that call: the constraint row itself
/// (`V(p)-V(n) = 0` structurally; each executed `V(...)<+expr` statement subtracts its own
/// `expr` from that same row via `crate::GeneratedModel::stamp`) and the branch current's
/// ordinary two-terminal KCL injection (`+ib` at `p`, `-ib` at `n`). A path that contributes
/// nothing to this branch this call defaults the row to `V(p)-V(n) = 0`, matching the LRM's
/// implicit-zero-contribution rule for an access nothing ever assigns on that path.
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
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset (a branch
/// mixing flow and potential contributions, loops/`case`, user-defined functions, malformed
/// `ddt`).
pub fn lower(module: &Module) -> Result<Lowered, CodegenError> {
    let (flow_branches, potential_branches) = branch_kinds(&module.analog);
    if let Some(&bad) = flow_branches.intersection(&potential_branches).next() {
        return Err(unsupported(&format!(
            "branch #{bad} receives both a flow and a potential contribution somewhere in the \
             module; mixing contribution kinds on one branch is not supported in codegen v0"
        )));
    }

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
/// potential contribution, anywhere in `stmts` (recursing into `if`/`else` arms and blocks —
/// same shape `lower_stmt` itself recurses through). Loop/`case` bodies are not walked: any
/// module containing one is rejected by `lower_stmt` regardless, so under-approximating here
/// changes nothing about the overall `lower()` result.
fn branch_kinds(stmts: &[Stmt]) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut flow = BTreeSet::new();
    let mut potential = BTreeSet::new();
    collect_branch_kinds(stmts, &mut flow, &mut potential);
    (flow, potential)
}

fn collect_branch_kinds(stmts: &[Stmt], flow: &mut BTreeSet<u32>, potential: &mut BTreeSet<u32>) {
    for stmt in stmts {
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
            _ => {}
        }
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
        Stmt::While { .. } | Stmt::For { .. } | Stmt::Repeat { .. } | Stmt::Case { .. } => Err(
            unsupported("loops and case statements are not supported in codegen v0"),
        ),
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
