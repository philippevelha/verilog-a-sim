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
//! `ddt(q)` term stamps `q` and its gradient into the charge channel.
//!
//! # Limitations (v0)
//!
//! - One global unknown per IR node, supplied as `terminals`. The v0 frontend emits modules
//!   whose nodes are exactly their ports, so `terminals` is the port→global map; modules with
//!   internal unknowns are out of scope.
//! - Only flow contributions; no `if`/`else`, no loops/`case`, no user-defined analog
//!   functions (see [`lower`]). Local-variable assignments *are* supported: statements execute
//!   in source order, each `Stmt::Assign` binding into [`ad::Ctx::vars`] for later statements
//!   (including later assignments — a variable can be reassigned) to read via [`ad::Ctx::set_var`]/
//!   the [`ad::eval`] `Expr::Var` case.
//! - `$vt`/`$temperature` evaluate at the fixed ambient point ([`VT`], [`TEMP`]); `$vt(T)`
//!   evaluates the thermal voltage at the given absolute temperature `T`, carrying `T`'s
//!   gradient (e.g. a self-heating thermal node).

#![forbid(unsafe_code)]

pub mod ad;
pub mod lower;

use ad::{eval, Ctx, Dual};
use lower::{Contribution, Lowered, LoweredStmt};
use std::cell::RefCell;
use thiserror::Error;
use va_abi::{ModelInstance, StampSink};
use va_ir::Module;

/// Thermal voltage `kT/q` at ~300 K, in volts. Matches `va_abi::reference::diode::VT_300K`
/// so a generated diode reproduces the reference diode's stamps.
pub const VT: f64 = 0.025_852;

/// Ambient temperature for `$temperature`, in kelvin.
pub const TEMP: f64 = 300.0;

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

/// Compile an elaborated IR module into a loadable model instance bound to `terminals`
/// (the global unknown indices the instance's nodes map onto, in node order).
///
/// # Errors
///
/// Returns [`CodegenError`] if `terminals` is the wrong length or the analog block contains
/// a construct outside the v0 subset (validated eagerly so [`ModelInstance::load`] cannot
/// fail).
pub fn build_instance(
    module: &Module,
    terminals: &[usize],
) -> Result<Box<dyn ModelInstance>, CodegenError> {
    if terminals.len() != module.nodes.len() {
        return Err(CodegenError::TerminalCount {
            expected: module.nodes.len(),
            got: terminals.len(),
        });
    }

    let lowered = lower::lower(module)?;
    let params: Vec<f64> = module.params.iter().map(|p| p.default).collect();

    let model = GeneratedModel {
        module: module.clone(),
        terminals: terminals.to_vec(),
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
        Ctx {
            module: &self.module,
            params: &self.params,
            x,
            terminals: &self.terminals,
            vt: self.vt,
            temp: self.temp,
            vars: RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Real, load-time execution: walks `stmts` in source order. An assignment evaluates its
    /// right-hand side and binds it into `ctx`'s variable environment (`ad::Ctx::set_var`) so
    /// later statements can read it; a contribution stamps directly via `sink`; an `if`/`else`
    /// evaluates its condition and recurses into *only* the arm it selects — same "only the
    /// taken branch is ever evaluated" rule the ternary `Expr::Select` follows in `ad::eval`,
    /// and the reason this can't share a traversal with [`Self::validate_stmts`], which must
    /// visit both arms instead (see `lower`'s module doc comment).
    ///
    /// Aborts the whole walk on the first error — an assignment or condition that fails
    /// post-validation (which "cannot happen") would otherwise leave later statements reading a
    /// stale or missing variable binding, which is worse than stopping early and leaving
    /// whatever was already stamped as-is.
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

    /// Stamp one contribution's resistive and charge channels.
    fn stamp(&self, ctx: &Ctx, c: &Contribution, sink: &mut dyn StampSink) {
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
}

impl ModelInstance for GeneratedModel {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
        let ctx = self.ctx(x);
        // Post-validation this cannot fail; `run` already stops early rather than stamping
        // from a corrupted variable environment if it somehow does (see `run`'s doc comment).
        let _ = self.run(&ctx, &self.lowered.stmts, sink);
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
        let inst = build_instance(&varactor_like_ir(), &[0, 1]).unwrap();
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
        let inst = build_instance(&varactor_like_ir(), &[0, 1]).unwrap();
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

        let inst = build_instance(&m, &[0, 1]).unwrap();
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
            build_instance(&m, &[0, 1]),
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
        let inst = build_instance(&piecewise_ir(g_pos, g_neg), &[0, 1]).unwrap();

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
                build_instance(&m, &[0, 1]),
                Err(CodegenError::Unsupported(_))
            ),
            "an error in the untaken-at-x=0 arm must still be caught eagerly"
        );
    }

    #[test]
    fn resistor_matches_reference_stamp() {
        // 1 kΩ from node 0 to ground (index 1 is out of range of a dim-1 system).
        let inst = build_instance(&resistor_ir(), &[0, 1]).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(&[2.0], &mut sink);
        // Same hand-checked values as va_abi's resistor_stamp_by_hand.
        assert!((sink.residual[0] - 2e-3).abs() < 1e-15);
        assert!((sink.jac(0, 0) - 1e-3).abs() < 1e-18);
        assert_eq!(sink.charge[0], 0.0);
    }

    #[test]
    fn capacitor_stamps_only_charge() {
        let inst = build_instance(&capacitor_ir(), &[0, 1]).unwrap();
        let mut sink = DenseStamp::new(1);
        inst.load(&[3.0], &mut sink);
        // Q = C*V = 1pF * 3V = 3e-12; dQ/dV = C = 1e-12. No resistive current.
        assert!((sink.charge[0] - 3e-12).abs() < 1e-24);
        assert!((sink.dcharge[0] - 1e-12).abs() < 1e-27);
        assert_eq!(sink.residual[0], 0.0);
    }

    #[test]
    fn diode_current_and_conductance() {
        let inst = build_instance(&diode_ir(), &[0, 1]).unwrap();
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
        let inst = build_instance(&diode_ir(), &[0, 1]).unwrap();

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
        match build_instance(&resistor_ir(), &[0]) {
            Err(CodegenError::TerminalCount {
                expected: 2,
                got: 1,
            }) => {}
            _ => panic!("expected a TerminalCount error"),
        }
    }
}
