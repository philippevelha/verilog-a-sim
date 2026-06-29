//! Forward-mode automatic differentiation over the [`va_ir`] expression arena.
//!
//! Evaluates an [`va_ir::ExprId`] to a [`Dual`]: its primal value paired with the partial
//! derivatives w.r.t. the model's local unknowns (one slot per node). The gradient feeds the
//! Jacobian stamps, so it must be exact — §5 checks it against a central finite difference.
//!
//! The active unknowns are the node voltages. A potential probe `V(p, n)` is the only place
//! a derivative is *introduced*: it contributes `+1` in the `p` slot and `-1` in the `n`
//! slot. Every other operator simply propagates gradients through the chain rule.

use crate::CodegenError;
use va_ir::{BinOp, Builtin, Expr, ExprId, Module, UnOp};

/// A value carried with its gradient w.r.t. the active unknowns (a dual number).
#[derive(Clone, Debug)]
pub struct Dual {
    /// The primal value.
    pub value: f64,
    /// Partial derivatives, one per local unknown (node slot order).
    pub grad: Vec<f64>,
}

impl Dual {
    /// A constant with zero gradient over `n` unknowns.
    pub fn constant(value: f64, n: usize) -> Self {
        Self {
            value,
            grad: vec![0.0; n],
        }
    }

    /// An independent variable: value `value`, unit derivative in slot `i` of `n`.
    pub fn variable(value: f64, i: usize, n: usize) -> Self {
        let mut grad = vec![0.0; n];
        grad[i] = 1.0;
        Self { value, grad }
    }

    /// Number of unknowns this dual carries a gradient over.
    fn n(&self) -> usize {
        self.grad.len()
    }

    /// Scale value and gradient by a constant `s`.
    pub fn scale(&self, s: f64) -> Dual {
        Dual {
            value: self.value * s,
            grad: self.grad.iter().map(|g| g * s).collect(),
        }
    }

    /// Sum: `(a + b)' = a' + b'`.
    pub fn add(&self, o: &Dual) -> Dual {
        Dual {
            value: self.value + o.value,
            grad: zip_with(&self.grad, &o.grad, |a, b| a + b),
        }
    }

    /// Difference: `(a - b)' = a' - b'`.
    pub fn sub(&self, o: &Dual) -> Dual {
        Dual {
            value: self.value - o.value,
            grad: zip_with(&self.grad, &o.grad, |a, b| a - b),
        }
    }

    /// Product: `(a*b)' = a'b + ab'`.
    pub fn mul(&self, o: &Dual) -> Dual {
        Dual {
            value: self.value * o.value,
            grad: zip_with(&self.grad, &o.grad, |a, b| a * o.value + b * self.value),
        }
    }

    /// Quotient: `(a/b)' = (a'b - ab') / b²`.
    pub fn div(&self, o: &Dual) -> Dual {
        let inv = 1.0 / o.value;
        let inv2 = inv * inv;
        Dual {
            value: self.value * inv,
            grad: zip_with(&self.grad, &o.grad, |a, b| {
                (a * o.value - self.value * b) * inv2
            }),
        }
    }

    /// Negation.
    pub fn neg(&self) -> Dual {
        self.scale(-1.0)
    }

    /// Apply a differentiable unary function given its value and derivative at `self.value`.
    fn chain(&self, value: f64, dvalue: f64) -> Dual {
        Dual {
            value,
            grad: self.grad.iter().map(|g| g * dvalue).collect(),
        }
    }

    /// `exp`.
    pub fn exp(&self) -> Dual {
        let e = self.value.exp();
        self.chain(e, e)
    }

    /// Natural log.
    pub fn ln(&self) -> Dual {
        self.chain(self.value.ln(), 1.0 / self.value)
    }

    /// Base-10 log.
    pub fn log10(&self) -> Dual {
        self.chain(
            self.value.log10(),
            1.0 / (self.value * std::f64::consts::LN_10),
        )
    }

    /// Square root.
    pub fn sqrt(&self) -> Dual {
        let r = self.value.sqrt();
        self.chain(r, 0.5 / r)
    }

    /// Absolute value (derivative `sign(x)`; subgradient `0` at the kink).
    pub fn abs(&self) -> Dual {
        self.chain(self.value.abs(), self.value.signum())
    }

    /// Power `self ** exp` with a (possibly variable) exponent:
    /// `d/dx u^v = u^v (v' ln u + v u'/u)`.
    pub fn powf(&self, exp: &Dual) -> Dual {
        let value = self.value.powf(exp.value);
        let lnu = self.value.ln();
        let grad = (0..self.n())
            .map(|i| value * (exp.grad[i] * lnu + exp.value * self.grad[i] / self.value))
            .collect();
        Dual { value, grad }
    }
}

fn zip_with(a: &[f64], b: &[f64], f: impl Fn(f64, f64) -> f64) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| f(*x, *y)).collect()
}

/// Evaluation context: everything `eval` needs beyond the expression itself.
pub struct Ctx<'a> {
    /// The IR module owning the expression arena, branches, and parameters.
    pub module: &'a Module,
    /// Parameter values, indexed by `ParamId`.
    pub params: &'a [f64],
    /// The global solution vector; out-of-range indices read as `0.0` (ground).
    pub x: &'a [f64],
    /// Maps a local node slot to its global unknown index.
    pub terminals: &'a [usize],
    /// Thermal voltage for `$vt`.
    pub vt: f64,
    /// Ambient temperature for `$temperature`.
    pub temp: f64,
}

impl Ctx<'_> {
    /// Number of local unknowns (node slots).
    pub fn count(&self) -> usize {
        self.terminals.len()
    }

    /// Read the node voltage at local slot `slot` from the global solution vector.
    fn node_voltage(&self, slot: usize) -> f64 {
        let g = self.terminals.get(slot).copied().unwrap_or(usize::MAX);
        self.x.get(g).copied().unwrap_or(0.0)
    }
}

/// Evaluate `expr` under forward-mode AD in context `ctx`.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] for IR constructs the v0 codegen does not evaluate
/// in value position: local variables, flow probes, and `ddt`/`idt` (which are handled by
/// the lowering split, not evaluated here).
pub fn eval(ctx: &Ctx, expr: ExprId) -> Result<Dual, CodegenError> {
    let count = ctx.count();
    match ctx.module.expr(expr) {
        Expr::Const(c) => Ok(Dual::constant(*c, count)),
        Expr::Param(p) => {
            let v = ctx
                .params
                .get(p.0 as usize)
                .copied()
                .ok_or_else(|| unsupported("parameter index out of range"))?;
            Ok(Dual::constant(v, count))
        }
        Expr::Var(_) => Err(unsupported(
            "local variables are not supported in codegen v0",
        )),
        Expr::Probe(access) => match access.kind {
            va_ir::AccessKind::Potential => {
                let br = ctx.module.branches[access.branch.0 as usize];
                let (p, n) = (br.p.0 as usize, br.n.0 as usize);
                let value = ctx.node_voltage(p) - ctx.node_voltage(n);
                let mut grad = vec![0.0; count];
                if p < count {
                    grad[p] += 1.0;
                }
                if n < count {
                    grad[n] -= 1.0;
                }
                Ok(Dual { value, grad })
            }
            va_ir::AccessKind::Flow => Err(unsupported(
                "flow probes `I(...)` are not supported in codegen v0",
            )),
        },
        Expr::Unary(op, e) => {
            let d = eval(ctx, *e)?;
            Ok(match op {
                UnOp::Neg => d.neg(),
                UnOp::Not => Dual::constant(bool_to_f64(d.value == 0.0), count),
            })
        }
        Expr::Binary(op, l, r) => {
            let a = eval(ctx, *l)?;
            let b = eval(ctx, *r)?;
            Ok(match op {
                BinOp::Add => a.add(&b),
                BinOp::Sub => a.sub(&b),
                BinOp::Mul => a.mul(&b),
                BinOp::Div => a.div(&b),
                BinOp::Pow => a.powf(&b),
                BinOp::Lt => Dual::constant(bool_to_f64(a.value < b.value), count),
                BinOp::Le => Dual::constant(bool_to_f64(a.value <= b.value), count),
                BinOp::Gt => Dual::constant(bool_to_f64(a.value > b.value), count),
                BinOp::Ge => Dual::constant(bool_to_f64(a.value >= b.value), count),
                BinOp::Eq => Dual::constant(bool_to_f64(a.value == b.value), count),
            })
        }
        Expr::Call(builtin, args) => eval_call(ctx, *builtin, args),
    }
}

fn eval_call(ctx: &Ctx, builtin: Builtin, args: &[ExprId]) -> Result<Dual, CodegenError> {
    let count = ctx.count();
    let arg = |i: usize| -> Result<Dual, CodegenError> {
        let id = args
            .get(i)
            .ok_or_else(|| unsupported("built-in called with too few arguments"))?;
        eval(ctx, *id)
    };
    Ok(match builtin {
        Builtin::Exp => arg(0)?.exp(),
        Builtin::Ln => arg(0)?.ln(),
        Builtin::Log => arg(0)?.log10(),
        Builtin::Sqrt => arg(0)?.sqrt(),
        Builtin::Abs => arg(0)?.abs(),
        Builtin::Pow => arg(0)?.powf(&arg(1)?),
        Builtin::Vt => Dual::constant(ctx.vt, count),
        Builtin::Temperature => Dual::constant(ctx.temp, count),
        Builtin::Ddt | Builtin::Idt => {
            return Err(unsupported(
                "ddt/idt must appear as a top-level contribution term, not inside an expression",
            ))
        }
    })
}

fn unsupported(msg: &str) -> CodegenError {
    CodegenError::Unsupported(msg.to_string())
}

fn bool_to_f64(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_rule() {
        // f = x0 * x1 at (3, 5): value 15, grad [5, 3].
        let a = Dual::variable(3.0, 0, 2);
        let b = Dual::variable(5.0, 1, 2);
        let f = a.mul(&b);
        assert_eq!(f.value, 15.0);
        assert_eq!(f.grad, vec![5.0, 3.0]);
    }

    #[test]
    fn exp_chain_rule() {
        // f = exp(2*x) at x=0.5: value e, grad 2e.
        let x = Dual::variable(0.5, 0, 1);
        let two = Dual::constant(2.0, 1);
        let f = two.mul(&x).exp();
        let e = 1.0_f64.exp();
        assert!((f.value - e).abs() < 1e-12);
        assert!((f.grad[0] - 2.0 * e).abs() < 1e-12);
    }

    #[test]
    fn div_matches_finite_difference() {
        // f = 1 / x at x=4: analytic -1/16.
        let x = Dual::variable(4.0, 0, 1);
        let one = Dual::constant(1.0, 1);
        let f = one.div(&x);
        let h = 1e-6;
        let fd = (1.0 / (4.0 + h) - 1.0 / (4.0 - h)) / (2.0 * h);
        assert!((f.grad[0] - fd).abs() < 1e-7, "{} vs {}", f.grad[0], fd);
    }
}
