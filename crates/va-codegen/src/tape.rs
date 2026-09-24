//! Flat instruction tapes for the expressions a `load()` evaluates on every call — Stage 3 of
//! `docs/proposals/evaluator-tree-walk.md`.
//!
//! [`crate::ad::eval`] walks an expression tree recursively: at every node it looks the node up
//! in the module's arena, matches on its kind, recurses into its operands and wraps the result in
//! a `Result`. A [`Tape`] does that work once, when the model is built: each root expression — a
//! statement's right-hand side or condition, a contribution term — is lowered to a post-order
//! list of instructions, and [`eval_root`] runs the list on a value stack.
//!
//! **Bit-identical by construction.** An instruction applies the very function `eval` does for
//! that node kind (`ad::apply_unary`, `ad::apply_binary`, `ad::pure_call1`, `ad::pure_call2`),
//! to operands evaluated in the same order (post-order: left before right, first argument
//! before second). Nodes resolved when the tape is built are the ones `eval` would compute from
//! values fixed for the compiled model's life — a constant, a parameter, `$param_given`,
//! `$port_connected` — and produce the same `Dual::constant`. Every other node kind (probes,
//! `?:`, which evaluates only its taken branch, user functions, `ddx`, events, analog
//! operators, and any built-in not in the pure-maths set) becomes a single
//! [`Instr::Tree`] instruction that calls `eval` on that subtree, so the tape never needs to
//! know their semantics.
//!
//! **Limitation:** a tape exists only for the roots [`Tapes::compile`] collects from the lowered
//! statements and contribution terms; anything else still goes through `eval`.

use crate::ad::{self, Ctx, Dual};
use crate::lower::{Contribution, Lowered, LoweredStmt};
use crate::CodegenError;
use va_ir::{BinOp, Builtin, Expr, ExprId, Module, UnOp, VarId};

/// One tape instruction. Each pushes one value; the operators pop their operands first.
#[derive(Clone, Copy, Debug)]
pub enum Instr {
    /// `Dual::constant(value)` — a literal, or a value fixed for the compiled model's life.
    Const(f64),
    /// A local variable read (`Ctx::get_var`).
    Var(VarId),
    /// `ad::eval` on this subtree — every node kind the tape does not lower.
    Tree(ExprId),
    /// Pops one operand.
    Unary(UnOp),
    /// Pops the right operand, then the left.
    Binary(BinOp),
    /// A one-argument pure maths built-in (`ad::pure_call1`). Pops one.
    Call1(Builtin),
    /// A two-argument pure maths built-in (`ad::pure_call2`). Pops the second, then the first.
    Call2(Builtin),
}

/// One root expression, lowered.
#[derive(Clone, Debug, Default)]
pub struct Tape {
    code: Vec<Instr>,
    /// The expression node each instruction stands for — read only by the `walk-stats` counters.
    #[cfg_attr(not(feature = "walk-stats"), allow(dead_code))]
    sites: Vec<ExprId>,
}

/// Every tape a compiled model has, looked up by root `ExprId`.
#[derive(Debug, Default)]
pub struct Tapes {
    /// `index[e]` is the position in `tapes` of root `e`'s tape, or `u32::MAX` for none.
    index: Vec<u32>,
    tapes: Vec<Tape>,
    /// The deepest value stack any tape needs, to size [`Ctx::tape_stack`] once.
    pub max_depth: usize,
}

/// No tapes: what a hand-built [`Ctx`] uses, so every expression goes through `eval`.
pub static NO_TAPES: Tapes = Tapes {
    index: Vec::new(),
    tapes: Vec::new(),
    max_depth: 0,
};

impl Tapes {
    /// Compile a tape for every root expression the lowered statements evaluate (right-hand
    /// sides, conditions, selectors and labels, loop counts, `bound_step`), and every
    /// contribution term and charge coefficient. `params` are the compiled model's parameter
    /// values, fixed for its life.
    pub fn compile(module: &Module, params: &[f64], lowered: &Lowered) -> Tapes {
        let mut roots = Vec::new();
        collect_roots(&lowered.stmts, &mut roots);
        let mut out = Tapes {
            index: vec![u32::MAX; module.exprs.len()],
            tapes: Vec::new(),
            max_depth: 0,
        };
        for root in roots {
            let r = root.0 as usize;
            if r >= out.index.len() || out.index[r] != u32::MAX {
                continue;
            }
            let mut tape = Tape::default();
            let mut depth = 0;
            let mut max = 0;
            emit(module, params, root, &mut tape, &mut depth, &mut max);
            out.max_depth = out.max_depth.max(max);
            out.index[r] = out.tapes.len() as u32;
            out.tapes.push(tape);
        }
        out
    }

    fn get(&self, root: ExprId) -> Option<&Tape> {
        let i = *self.index.get(root.0 as usize)?;
        self.tapes.get(i as usize)
    }
}

/// Evaluate a root expression: its tape if it has one, else `ad::eval`. The same value either
/// way, bit for bit (see the module doc).
///
/// # Errors
///
/// Whatever `ad::eval` would return for the same expression, in the same order.
pub fn eval_root(ctx: &Ctx, root: ExprId) -> Result<Dual, CodegenError> {
    match ctx.tapes.get(root) {
        Some(tape) => run(ctx, tape),
        None => ad::eval(ctx, root),
    }
}

/// Whether `builtin` is one of the one-argument pure maths built-ins — asked of
/// `ad::pure_call1` itself, so the tape and `eval` cannot disagree about the set.
fn is_pure_call1(builtin: Builtin) -> bool {
    ad::pure_call1(builtin, Dual::constant(1.0)).is_some()
}

/// The two-argument counterpart of [`is_pure_call1`].
fn is_pure_call2(builtin: Builtin) -> bool {
    ad::pure_call2(builtin, Dual::constant(1.0), Dual::constant(1.0)).is_some()
}

/// Lower `e` post-order into `tape`, tracking the value-stack depth.
fn emit(
    module: &Module,
    params: &[f64],
    e: ExprId,
    tape: &mut Tape,
    depth: &mut usize,
    max: &mut usize,
) {
    let push = |tape: &mut Tape, ins: Instr, pops: usize, depth: &mut usize, max: &mut usize| {
        *depth = *depth + 1 - pops;
        *max = (*max).max(*depth);
        tape.code.push(ins);
        tape.sites.push(e);
    };
    match module.expr(e) {
        Expr::Const(c) => push(tape, Instr::Const(*c), 0, depth, max),
        Expr::Param(p) => match params.get(p.0 as usize) {
            Some(v) => push(tape, Instr::Const(*v), 0, depth, max),
            // Out of range: `eval` reports it, so let `eval` see it.
            None => push(tape, Instr::Tree(e), 0, depth, max),
        },
        Expr::ParamGiven(p) => {
            let v = if module.param_is_given(*p) { 1.0 } else { 0.0 };
            push(tape, Instr::Const(v), 0, depth, max);
        }
        Expr::PortConnected(i) => {
            let v = if module.port_is_connected(*i as usize) {
                1.0
            } else {
                0.0
            };
            push(tape, Instr::Const(v), 0, depth, max);
        }
        Expr::Var(v) => push(tape, Instr::Var(*v), 0, depth, max),
        Expr::Unary(op, a) => {
            emit(module, params, *a, tape, depth, max);
            push(tape, Instr::Unary(*op), 1, depth, max);
        }
        Expr::Binary(op, a, b) => {
            emit(module, params, *a, tape, depth, max);
            emit(module, params, *b, tape, depth, max);
            push(tape, Instr::Binary(*op), 2, depth, max);
        }
        Expr::Call(builtin, args) if is_pure_call1(*builtin) && !args.is_empty() => {
            emit(module, params, args[0], tape, depth, max);
            push(tape, Instr::Call1(*builtin), 1, depth, max);
        }
        Expr::Call(builtin, args) if is_pure_call2(*builtin) && args.len() >= 2 => {
            emit(module, params, args[0], tape, depth, max);
            emit(module, params, args[1], tape, depth, max);
            push(tape, Instr::Call2(*builtin), 2, depth, max);
        }
        Expr::Call(..)
        | Expr::CallUser(..)
        | Expr::Select(..)
        | Expr::Ddx(..)
        | Expr::Probe(_)
        | Expr::EventFired(_) => push(tape, Instr::Tree(e), 0, depth, max),
    }
}

/// Run a tape on the context's value stack.
fn run(ctx: &Ctx, tape: &Tape) -> Result<Dual, CodegenError> {
    let mut stack = ctx.tape_stack.borrow_mut();
    let base = stack.len();
    let result = run_on(ctx, tape, &mut stack);
    // On an error the stack may hold this run's partial values; leave it as it was found.
    stack.truncate(base);
    result
}

fn run_on(ctx: &Ctx, tape: &Tape, stack: &mut Vec<Dual>) -> Result<Dual, CodegenError> {
    let underflow = || ad::internal_error("tape stack underflow");
    // The tree-walk counters (`--features walk-stats` only): every instruction is one visited
    // node, except `Tree`, whose nodes `eval` counts itself.
    #[cfg(feature = "walk-stats")]
    for (ins, site) in tape.code.iter().zip(&tape.sites) {
        if !matches!(ins, Instr::Tree(_)) {
            crate::counters::expr_visit(ctx.hoistable, site.0 as usize);
        }
    }
    for ins in &tape.code {
        match *ins {
            Instr::Const(c) => stack.push(Dual::constant(c)),
            Instr::Var(v) => stack.push(ctx.get_var(v)?),
            Instr::Tree(e) => stack.push(ad::eval(ctx, e)?),
            Instr::Unary(op) => {
                let a = stack.pop().ok_or_else(underflow)?;
                stack.push(ad::apply_unary(op, a));
            }
            Instr::Binary(op) => {
                let b = stack.pop().ok_or_else(underflow)?;
                let a = stack.pop().ok_or_else(underflow)?;
                stack.push(ad::apply_binary(op, a, b));
            }
            Instr::Call1(builtin) => {
                let a = stack.pop().ok_or_else(underflow)?;
                stack.push(ad::pure_call1(builtin, a).ok_or_else(underflow)?);
            }
            Instr::Call2(builtin) => {
                let b = stack.pop().ok_or_else(underflow)?;
                let a = stack.pop().ok_or_else(underflow)?;
                stack.push(ad::pure_call2(builtin, a, b).ok_or_else(underflow)?);
            }
        }
    }
    stack.pop().ok_or_else(underflow)
}

/// Every root expression a lowered statement list evaluates, and every contribution term.
fn collect_roots(stmts: &[LoweredStmt], out: &mut Vec<ExprId>) {
    for s in stmts {
        match s {
            LoweredStmt::Assign { rhs, .. } => out.push(*rhs),
            LoweredStmt::BoundStep(e) => out.push(*e),
            LoweredStmt::Contribute(c) => contribution_roots(c, out),
            LoweredStmt::If { cond, then_, else_ } => {
                out.push(*cond);
                collect_roots(then_, out);
                collect_roots(else_, out);
            }
            LoweredStmt::Case {
                selector,
                arms,
                default,
            } => {
                out.push(*selector);
                for arm in arms {
                    out.extend(arm.labels.iter().copied());
                    collect_roots(&arm.body, out);
                }
                collect_roots(default, out);
            }
            LoweredStmt::While { cond, body } => {
                out.push(*cond);
                collect_roots(body, out);
            }
            LoweredStmt::For {
                init,
                cond,
                step,
                body,
            } => {
                collect_roots(init, out);
                out.push(*cond);
                collect_roots(step, out);
                collect_roots(body, out);
            }
            LoweredStmt::Repeat { count, body } => {
                out.push(*count);
                collect_roots(body, out);
            }
        }
    }
}

fn contribution_roots(c: &Contribution, out: &mut Vec<ExprId>) {
    out.extend(c.resistive.iter().map(|t| t.expr));
    for t in &c.charge {
        out.push(t.expr);
        out.extend(t.coeffs.iter().map(|(e, _)| *e));
    }
}
