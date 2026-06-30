//! Lowering: walk a [`va_ir::Module`]'s analog block and turn `<+` contributions into the
//! per-branch residual/charge stamps a generated [`va_abi::ModelInstance`] emits.
//!
//! Each flow contribution `I(p, n) <+ value` is flattened into a signed sum of additive
//! terms. A top-level `ddt(arg)` term routes `arg` to the **charge** channel; every other
//! term is a **resistive** contribution. This split is what lets DC ignore storage while the
//! transient integrator picks the charge channel up via a companion model.
//!
//! # Limitations
//!
//! - Only flow (current) contributions are lowered. Potential (`V(...) <+ …`) contributions
//!   need a branch-current unknown and are out of scope for v0.
//! - `ddt` is recognised only as a top-level additive term (optionally negated), matching how
//!   compact models are written (`I <+ resistive + ddt(charge)`); `ddt` nested inside a
//!   nonlinear function is rejected later by the AD evaluator.
//! - `if`/`else`, local-variable assignments, loops/`case`, and user-defined analog functions
//!   in the analog block are not yet lowered. The IR (Interface α) models these, but codegen
//!   v0 rejects them with [`CodegenError::Unsupported`].

use crate::CodegenError;
use va_ir::{AccessKind, BinOp, Builtin, Expr, ExprId, Module, Stmt, UnOp};

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

/// A lowered, evaluable representation of a module's analog block.
#[derive(Debug, Default)]
pub struct Lowered {
    /// Number of local unknowns (one per IR node).
    pub n_unknowns: usize,
    /// Per-branch contributions, in source order.
    pub contributions: Vec<Contribution>,
}

/// Lower a module's analog block into a [`Lowered`] plan.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset
/// (potential contributions, `if`/`else`, assignments, malformed `ddt`).
pub fn lower(module: &Module) -> Result<Lowered, CodegenError> {
    let mut contributions = Vec::new();
    for stmt in &module.analog {
        lower_stmt(module, stmt, &mut contributions)?;
    }
    Ok(Lowered {
        n_unknowns: module.nodes.len(),
        contributions,
    })
}

fn lower_stmt(
    module: &Module,
    stmt: &Stmt,
    out: &mut Vec<Contribution>,
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

            out.push(Contribution {
                p_slot: br.p.0 as usize,
                n_slot: br.n.0 as usize,
                resistive,
                charge,
            });
            Ok(())
        }
        Stmt::Block(body) => {
            for s in body {
                lower_stmt(module, s, out)?;
            }
            Ok(())
        }
        Stmt::If { .. } => Err(unsupported("if/else is not supported in codegen v0")),
        Stmt::Assign { .. } => Err(unsupported(
            "local variable assignments are not supported in codegen v0",
        )),
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
