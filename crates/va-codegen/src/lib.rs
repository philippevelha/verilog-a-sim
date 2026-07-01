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
//! - Only flow contributions; no `if`/`else`, no local variables (see [`lower`]).
//! - `$vt`/`$temperature` evaluate at the fixed ambient point ([`VT`], [`TEMP`]); `$vt(T)`
//!   evaluates the thermal voltage at the given absolute temperature `T`, carrying `T`'s
//!   gradient (e.g. a self-heating thermal node).

#![forbid(unsafe_code)]

pub mod ad;
pub mod lower;

use ad::{eval, Ctx, Dual};
use lower::{Contribution, Lowered};
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
        }
    }

    /// Evaluate every term once (structural validation): surfaces any unsupported construct
    /// as a `CodegenError` before the instance is handed out.
    fn validate(&self) -> Result<(), CodegenError> {
        let ctx = self.ctx(&[]);
        for c in &self.lowered.contributions {
            for term in c.resistive.iter().chain(c.charge.iter()) {
                eval(&ctx, term.expr)?;
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
        for c in &self.lowered.contributions {
            self.stamp(&ctx, c, sink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::stamps::DenseStamp;
    use va_ir::{
        Access, AccessKind, Branch, BranchId, Builtin, Discipline, Expr, Module, NodeDecl, NodeId,
        Param, Stmt,
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
        m.ports = vec![NodeId(0), NodeId(1)];
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
        m.ports = vec![NodeId(0), NodeId(1)];
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
        m.ports = vec![NodeId(0), NodeId(1)];
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
