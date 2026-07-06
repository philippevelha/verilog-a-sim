//! Lowering: walk a [`va_ir::Module`]'s analog block into an ordered sequence of executable
//! statements — local-variable assignments and flow contributions (each already flattened and
//! split into resistive/charge terms) — in source order.
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
//! # Limitations
//!
//! - Only flow (current) contributions are lowered. Potential (`V(...) <+ …`) contributions
//!   need a branch-current unknown and are out of scope for v0.
//! - `ddt` is recognised only as a top-level additive term (optionally negated), matching how
//!   compact models are written (`I <+ resistive + ddt(charge)`); `ddt` nested inside a
//!   nonlinear function is rejected later by the AD evaluator.
//! - Loops/`case` and user-defined analog functions in the analog block are not yet lowered.
//!   The IR (Interface α) models these, but codegen v0 rejects them with
//!   [`CodegenError::Unsupported`].

use crate::CodegenError;
use va_ir::{AccessKind, BinOp, Builtin, Expr, ExprId, Module, Stmt, UnOp, VarId};

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
    /// Static terms summed into the residual/Jacobian.
    pub resistive: Vec<Term>,
    /// `ddt` arguments summed into the charge/charge-Jacobian channel.
    pub charge: Vec<Term>,
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
    /// A flow contribution, already split into resistive/charge terms.
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
    /// Number of local unknowns (one per IR node).
    pub n_unknowns: usize,
    /// Statements in source order (assignments and contributions only — see Limitations).
    pub stmts: Vec<LoweredStmt>,
}

/// Lower a module's analog block into a [`Lowered`] plan.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset
/// (potential contributions, loops/`case`, user-defined functions, malformed `ddt`).
pub fn lower(module: &Module) -> Result<Lowered, CodegenError> {
    let mut stmts = Vec::new();
    for stmt in &module.analog {
        lower_stmt(module, stmt, &mut stmts)?;
    }
    Ok(Lowered {
        n_unknowns: module.nodes.len(),
        stmts,
    })
}

fn lower_stmt(
    module: &Module,
    stmt: &Stmt,
    out: &mut Vec<LoweredStmt>,
) -> Result<(), CodegenError> {
    match stmt {
        Stmt::Contribute { target, value } => {
            if target.kind != AccessKind::Flow {
                return Err(unsupported(
                    "potential (voltage) contributions are not supported in codegen v0",
                ));
            }
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

            out.push(LoweredStmt::Contribute(Contribution {
                p_slot: br.p.0 as usize,
                n_slot: br.n.0 as usize,
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
                lower_stmt(module, s, out)?;
            }
            Ok(())
        }
        Stmt::If { cond, then_, else_ } => {
            let mut then_lowered = Vec::new();
            for s in then_ {
                lower_stmt(module, s, &mut then_lowered)?;
            }
            let mut else_lowered = Vec::new();
            for s in else_ {
                lower_stmt(module, s, &mut else_lowered)?;
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
