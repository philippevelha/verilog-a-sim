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

    /// Sine. `sin' = cos`.
    pub fn sin(&self) -> Dual {
        self.chain(self.value.sin(), self.value.cos())
    }

    /// Cosine. `cos' = -sin`.
    pub fn cos(&self) -> Dual {
        self.chain(self.value.cos(), -self.value.sin())
    }

    /// Tangent. `tan' = 1 + tan²`.
    pub fn tan(&self) -> Dual {
        let t = self.value.tan();
        self.chain(t, 1.0 + t * t)
    }

    /// Hyperbolic sine. `sinh' = cosh`.
    pub fn sinh(&self) -> Dual {
        self.chain(self.value.sinh(), self.value.cosh())
    }

    /// Hyperbolic cosine. `cosh' = sinh`.
    pub fn cosh(&self) -> Dual {
        self.chain(self.value.cosh(), self.value.sinh())
    }

    /// Hyperbolic tangent. `tanh' = 1 - tanh²`.
    pub fn tanh(&self) -> Dual {
        let t = self.value.tanh();
        self.chain(t, 1.0 - t * t)
    }

    /// Arcsine. `asin'(x) = 1/√(1-x²)`.
    pub fn asin(&self) -> Dual {
        self.chain(
            self.value.asin(),
            1.0 / (1.0 - self.value * self.value).sqrt(),
        )
    }

    /// Arccosine. `acos'(x) = -1/√(1-x²)`.
    pub fn acos(&self) -> Dual {
        self.chain(
            self.value.acos(),
            -1.0 / (1.0 - self.value * self.value).sqrt(),
        )
    }

    /// Arctangent. `atan'(x) = 1/(1+x²)`.
    pub fn atan(&self) -> Dual {
        self.chain(self.value.atan(), 1.0 / (1.0 + self.value * self.value))
    }

    /// Inverse hyperbolic sine. `asinh'(x) = 1/√(x²+1)`.
    pub fn asinh(&self) -> Dual {
        self.chain(
            self.value.asinh(),
            1.0 / (self.value * self.value + 1.0).sqrt(),
        )
    }

    /// Inverse hyperbolic cosine. `acosh'(x) = 1/√(x²-1)`.
    pub fn acosh(&self) -> Dual {
        self.chain(
            self.value.acosh(),
            1.0 / (self.value * self.value - 1.0).sqrt(),
        )
    }

    /// Inverse hyperbolic tangent. `atanh'(x) = 1/(1-x²)`.
    pub fn atanh(&self) -> Dual {
        self.chain(self.value.atanh(), 1.0 / (1.0 - self.value * self.value))
    }

    /// Two-argument arctangent `atan2(self, x)` (self is `y`):
    /// `d atan2 = (x·dy - y·dx) / (x²+y²)`.
    pub fn atan2(&self, x: &Dual) -> Dual {
        let (y, denom) = (self, self.value * self.value + x.value * x.value);
        let grad = (0..self.n())
            .map(|i| (x.value * y.grad[i] - y.value * x.grad[i]) / denom)
            .collect();
        Dual {
            value: y.value.atan2(x.value),
            grad,
        }
    }

    /// Euclidean norm `hypot(self, o) = √(self²+o²)`:
    /// `d hypot = (self·dself + o·do) / hypot`.
    pub fn hypot(&self, o: &Dual) -> Dual {
        let value = self.value.hypot(o.value);
        let grad = (0..self.n())
            .map(|i| (self.value * self.grad[i] + o.value * o.grad[i]) / value)
            .collect();
        Dual { value, grad }
    }

    /// Minimum. The derivative follows the selected argument (subgradient at a tie).
    pub fn min(&self, o: &Dual) -> Dual {
        if self.value <= o.value {
            self.clone()
        } else {
            o.clone()
        }
    }

    /// Maximum. The derivative follows the selected argument (subgradient at a tie).
    pub fn max(&self, o: &Dual) -> Dual {
        if self.value >= o.value {
            self.clone()
        } else {
            o.clone()
        }
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
                BinOp::Ne => Dual::constant(bool_to_f64(a.value != b.value), count),
                BinOp::And => Dual::constant(bool_to_f64(a.value != 0.0 && b.value != 0.0), count),
                BinOp::Or => Dual::constant(bool_to_f64(a.value != 0.0 || b.value != 0.0), count),
            })
        }
        Expr::Call(builtin, args) => eval_call(ctx, *builtin, args),
        Expr::CallUser(..) => Err(unsupported(
            "user-defined analog functions are not supported in codegen v0",
        )),
        // Ternary: evaluate the selector, then only the taken branch (so an unselected,
        // possibly-undefined branch is never touched). The gradient is the taken branch's.
        Expr::Select(cond, then, else_) => {
            if eval(ctx, *cond)?.value != 0.0 {
                eval(ctx, *then)
            } else {
                eval(ctx, *else_)
            }
        }
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
        Builtin::Hypot => arg(0)?.hypot(&arg(1)?),
        Builtin::Atan2 => arg(0)?.atan2(&arg(1)?),
        Builtin::Min => arg(0)?.min(&arg(1)?),
        Builtin::Max => arg(0)?.max(&arg(1)?),
        Builtin::Sin => arg(0)?.sin(),
        Builtin::Cos => arg(0)?.cos(),
        Builtin::Tan => arg(0)?.tan(),
        Builtin::Sinh => arg(0)?.sinh(),
        Builtin::Cosh => arg(0)?.cosh(),
        Builtin::Tanh => arg(0)?.tanh(),
        Builtin::Asin => arg(0)?.asin(),
        Builtin::Acos => arg(0)?.acos(),
        Builtin::Atan => arg(0)?.atan(),
        Builtin::Asinh => arg(0)?.asinh(),
        Builtin::Acosh => arg(0)?.acosh(),
        Builtin::Atanh => arg(0)?.atanh(),
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

    /// A unary-function FD test case: name, the [`Dual`] method, the scalar `f64` function,
    /// and a point to check the derivative at.
    type UnaryCase = (&'static str, fn(&Dual) -> Dual, fn(f64) -> f64, f64);

    #[test]
    fn unary_builtins_match_finite_difference() {
        // §5: every differentiated operator must agree with a central finite difference.
        let h = 1e-6;
        let cases: &[UnaryCase] = &[
            ("sin", Dual::sin, f64::sin, 0.7),
            ("cos", Dual::cos, f64::cos, 0.7),
            ("tan", Dual::tan, f64::tan, 0.5),
            ("sinh", Dual::sinh, f64::sinh, 0.6),
            ("cosh", Dual::cosh, f64::cosh, 0.6),
            ("tanh", Dual::tanh, f64::tanh, 0.6),
            ("asin", Dual::asin, f64::asin, 0.4),
            ("acos", Dual::acos, f64::acos, 0.4),
            ("atan", Dual::atan, f64::atan, 0.4),
            ("asinh", Dual::asinh, f64::asinh, 0.4),
            ("acosh", Dual::acosh, f64::acosh, 1.5),
            ("atanh", Dual::atanh, f64::atanh, 0.4),
        ];
        for (name, dfn, ffn, x0) in cases {
            let analytic = dfn(&Dual::variable(*x0, 0, 1)).grad[0];
            let fd = (ffn(*x0 + h) - ffn(*x0 - h)) / (2.0 * h);
            assert!(
                (analytic - fd).abs() < 1e-5,
                "{name}: analytic {analytic} vs fd {fd}"
            );
        }
    }

    #[test]
    fn select_evaluates_only_the_taken_branch() {
        use va_ir::{Expr, Module};

        // cond != 0 → `then`; the `else` branch (a `Var`, which eval rejects) is never touched,
        // so the call still succeeds.
        let mut m = Module::new("sel");
        let cond = m.push_expr(Expr::Const(1.0));
        let then = m.push_expr(Expr::Const(2.0));
        let bad = m.push_expr(Expr::Var(va_ir::VarId(0))); // eval() would Err on this
        let sel = m.push_expr(Expr::Select(cond, then, bad));
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &[],
            terminals: &[],
            vt: 0.0,
            temp: 0.0,
        };
        assert_eq!(eval(&ctx, sel).unwrap().value, 2.0);

        // cond == 0 → `else`.
        let mut m = Module::new("sel");
        let cond = m.push_expr(Expr::Const(0.0));
        let then = m.push_expr(Expr::Const(2.0));
        let els = m.push_expr(Expr::Const(3.0));
        let sel = m.push_expr(Expr::Select(cond, then, els));
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &[],
            terminals: &[],
            vt: 0.0,
            temp: 0.0,
        };
        assert_eq!(eval(&ctx, sel).unwrap().value, 3.0);
    }

    #[test]
    fn two_arg_builtins_gradients() {
        // hypot(3,4) = 5; d/dx = 3/5, d/dy = 4/5.
        let x = Dual::variable(3.0, 0, 2);
        let y = Dual::variable(4.0, 1, 2);
        let hp = x.hypot(&y);
        assert!((hp.value - 5.0).abs() < 1e-12);
        assert!((hp.grad[0] - 0.6).abs() < 1e-12);
        assert!((hp.grad[1] - 0.8).abs() < 1e-12);

        // atan2(y, x): d/dy = x/(x²+y²), d/dx = -y/(x²+y²).
        let denom = 3.0_f64 * 3.0 + 4.0 * 4.0;
        let at = y.atan2(&x);
        assert!((at.grad[1] - 3.0 / denom).abs() < 1e-12);
        assert!((at.grad[0] + 4.0 / denom).abs() < 1e-12);

        // min/max select the active argument's value and gradient.
        let mn = x.min(&y);
        assert_eq!((mn.value, mn.grad[0], mn.grad[1]), (3.0, 1.0, 0.0));
        let mx = x.max(&y);
        assert_eq!((mx.value, mx.grad[0], mx.grad[1]), (4.0, 0.0, 1.0));
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
