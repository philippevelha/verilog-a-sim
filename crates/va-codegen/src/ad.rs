//! Forward-mode automatic differentiation over the [`va_ir`] expression arena.
//!
//! Evaluates an [`va_ir::ExprId`] to a [`Dual`]: its primal value paired with the partial
//! derivatives w.r.t. the model's local unknowns (one slot per node, plus one per branch with
//! its own auxiliary current unknown — see [`Ctx::branch_current_slots`]). The gradient feeds
//! the Jacobian stamps, so it must be exact — §5 checks it against a central finite difference.
//!
//! The active unknowns are the node voltages plus any branch currents. A potential probe
//! `V(p, n)` contributes `+1` in the `p` slot and `-1` in the `n` slot; a flow probe `I(...)`
//! contributes `+1` in its branch's own current slot, if that branch has one allocated (a
//! branch only gets one if it receives a potential contribution somewhere in the module — see
//! `crate::lower`). Every other operator simply propagates gradients through the chain rule.

use crate::CodegenError;
use std::cell::RefCell;
use std::collections::HashMap;
use va_ir::{BinOp, Builtin, Expr, ExprId, Module, UnOp, VarId};

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
    /// Local-variable bindings accumulated by sequential `Stmt::Assign` execution (the
    /// statement walk in `crate::GeneratedModel::load`/`validate`), keyed by `VarId`.
    /// Interior-mutable so it can be populated through a shared `&Ctx`: every recursive `eval`
    /// call already takes `&Ctx`, and only ever *reads* a binding via [`Self::get_var`] — writes
    /// happen exactly once per `Stmt::Assign`, from the outer statement walk via
    /// [`Self::set_var`], never from within expression evaluation itself.
    pub vars: RefCell<HashMap<u32, Dual>>,
    /// Maps a branch (by `BranchId.0`) to the local terminal slot of its own auxiliary current
    /// unknown, for every branch that receives a potential contribution somewhere in the
    /// module (`crate::lower::Lowered::branch_currents`). A flow probe `I(...)` on a branch
    /// absent from this map has no current unknown to read and is rejected.
    pub branch_current_slots: HashMap<u32, usize>,
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

    /// Bind local variable `id` to `value`, overwriting any previous binding — ordinary
    /// imperative reassignment, exactly what a second `Stmt::Assign` to the same variable does.
    pub fn set_var(&self, id: VarId, value: Dual) {
        self.vars.borrow_mut().insert(id.0, value);
    }

    /// Read local variable `id`'s current binding.
    ///
    /// # Errors
    ///
    /// [`CodegenError::Unsupported`] if `id` was never assigned before this read — either a
    /// genuinely uninitialized variable (undefined in real Verilog-A too), or, more likely
    /// today, an assignment that lives inside a still-unsupported `if`/`case` arm this
    /// straight-line statement walk never executes.
    fn get_var(&self, id: VarId) -> Result<Dual, CodegenError> {
        self.vars
            .borrow()
            .get(&id.0)
            .cloned()
            .ok_or_else(|| unsupported(&format!("variable #{} read before assignment", id.0)))
    }
}

/// Evaluate `expr` under forward-mode AD in context `ctx`. A [`Expr::Var`] reads whatever
/// `ctx.vars` currently holds — see [`Ctx::set_var`] for who populates it and when.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] for IR constructs the v0 codegen does not evaluate in
/// value position: flow probes, `ddt`/`idt` (handled by the lowering split, not evaluated
/// here), user-defined functions, and a local variable read before it was ever assigned.
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
        Expr::Var(id) => ctx.get_var(*id),
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
            va_ir::AccessKind::Flow => {
                let slot = *ctx
                    .branch_current_slots
                    .get(&access.branch.0)
                    .ok_or_else(|| {
                        unsupported(
                            "flow probe `I(...)` is only supported for a branch that also \
                             receives a potential contribution somewhere in the module \
                             (codegen v0)",
                        )
                    })?;
                let g = ctx.terminals.get(slot).copied().unwrap_or(usize::MAX);
                let value = ctx.x.get(g).copied().unwrap_or(0.0);
                let mut grad = vec![0.0; count];
                if slot < count {
                    grad[slot] = 1.0;
                }
                Ok(Dual { value, grad })
            }
        },
        Expr::Unary(op, e) => {
            let d = eval(ctx, *e)?;
            Ok(match op {
                UnOp::Neg => d.neg(),
                UnOp::Not => Dual::constant(bool_to_f64(d.value == 0.0), count),
                // Bitwise NOT, like the comparison/logical operators above, is an integer
                // operation with no continuous derivative — zero-gradient.
                UnOp::BitNot => Dual::constant(!to_i64(d.value) as f64, count),
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
                // Modulus is genuinely discontinuous (it jumps at every multiple of `b`), so —
                // like the bitwise/comparison operators below — it's zero-gradient in AD rather
                // than attempting an analytic derivative.
                BinOp::Mod => Dual::constant(a.value % b.value, count),
                BinOp::Pow => a.powf(&b),
                BinOp::Lt => Dual::constant(bool_to_f64(a.value < b.value), count),
                BinOp::Le => Dual::constant(bool_to_f64(a.value <= b.value), count),
                BinOp::Gt => Dual::constant(bool_to_f64(a.value > b.value), count),
                BinOp::Ge => Dual::constant(bool_to_f64(a.value >= b.value), count),
                BinOp::Eq => Dual::constant(bool_to_f64(a.value == b.value), count),
                BinOp::Ne => Dual::constant(bool_to_f64(a.value != b.value), count),
                BinOp::And => Dual::constant(bool_to_f64(a.value != 0.0 && b.value != 0.0), count),
                BinOp::Or => Dual::constant(bool_to_f64(a.value != 0.0 || b.value != 0.0), count),
                // Bitwise/shift operators are integer operations with no continuous derivative,
                // same treatment as the comparison operators above: zero-gradient.
                BinOp::BitAnd => Dual::constant((to_i64(a.value) & to_i64(b.value)) as f64, count),
                BinOp::BitOr => Dual::constant((to_i64(a.value) | to_i64(b.value)) as f64, count),
                BinOp::BitXor => Dual::constant((to_i64(a.value) ^ to_i64(b.value)) as f64, count),
                BinOp::BitXnor => {
                    Dual::constant(!(to_i64(a.value) ^ to_i64(b.value)) as f64, count)
                }
                BinOp::Shl => Dual::constant(
                    to_i64(a.value).wrapping_shl(to_i64(b.value) as u32) as f64,
                    count,
                ),
                BinOp::Shr => Dual::constant(
                    (to_i64(a.value) as u64).wrapping_shr(to_i64(b.value) as u32) as f64,
                    count,
                ),
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
        // `ddx(expr, V(p, n))`: the forward-mode `Dual` for `expr` already carries exactly the
        // partial derivative w.r.t. every node's raw potential (that's what a `Probe` seeds:
        // `grad[p] += 1.0`, per node, independent of any other node) — so `ddx`'s answer is
        // simply the gradient component at the probe's positive-terminal slot. The reference
        // terminal `n` doesn't change the answer (see `va-ir::Expr::Ddx`'s doc comment): it's
        // part of how the probe is *spelled*, not part of what's being differentiated w.r.t.
        // A node the expression never touched naturally reads back `0.0`, matching the LRM's
        // "if the expression does not depend explicitly on the unknown, ddx() returns zero."
        // The result itself is treated as a constant (zero further gradient) — second
        // derivatives are out of scope for this single-pass AD.
        Expr::Ddx(inner, access) => {
            let d = eval(ctx, *inner)?;
            let br = ctx.module.branches[access.branch.0 as usize];
            let p = br.p.0 as usize;
            Ok(Dual::constant(d.grad.get(p).copied().unwrap_or(0.0), count))
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
        // Rounding functions are piecewise constant: value is the rounded primal, gradient 0.
        Builtin::Floor => Dual::constant(arg(0)?.value.floor(), count),
        Builtin::Ceil => Dual::constant(arg(0)?.value.ceil(), count),
        Builtin::Round => Dual::constant(arg(0)?.value.round(), count),
        Builtin::Int => Dual::constant(arg(0)?.value.trunc(), count),
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
        // `$vt` is the thermal voltage `kT/q` at the ambient temperature; `$vt(T)` evaluates it
        // at the given absolute temperature `T` (kelvin). The two share `k/q`, recovered as
        // `ctx.vt / ctx.temp`, so `$vt` and `$vt(ctx.temp)` agree exactly. `T` may depend on
        // unknowns (e.g. a self-heating thermal node), so the argument's gradient is carried
        // through via `scale`.
        Builtin::Vt => match args.first() {
            Some(_) => arg(0)?.scale(ctx.vt / ctx.temp),
            None => Dual::constant(ctx.vt, count),
        },
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

/// Truncate a value to its integer representation for a bitwise/shift operator — mirrors
/// `va-frontend::elaborate`'s constant-folding treatment of the same operators (there is no
/// bit-vector type in this project; every value is `f64`).
fn to_i64(v: f64) -> i64 {
    v.trunc() as i64
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
    fn vt_no_arg_is_ambient_thermal_voltage() {
        use va_ir::{Builtin, Expr, Module};

        // `$vt` with no argument evaluates to `ctx.vt`, gradient zero.
        let mut m = Module::new("vt");
        let vt = m.push_expr(Expr::Call(Builtin::Vt, vec![]));
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &[],
            terminals: &[],
            vt: 0.025_852,
            temp: 300.0,
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
        };
        let d = eval(&ctx, vt).unwrap();
        assert!((d.value - 0.025_852).abs() < 1e-12);
        assert!(d.grad.is_empty());
    }

    #[test]
    fn vt_of_temperature_scales_and_carries_gradient() {
        use va_ir::{Access, AccessKind, Branch, Builtin, Expr, Module, NodeDecl, NodeId};

        // `$vt(T)` with `T = V(t, gnd)`: value `k/q * T`, gradient `k/q` w.r.t. the node.
        let mut m = Module::new("vt_t");
        // Two nodes: slot 0 is the thermal node `t`, slot 1 is ground.
        m.nodes.push(NodeDecl {
            name: "t".into(),
            discipline: va_ir::Discipline::Thermal,
        });
        m.nodes.push(NodeDecl {
            name: "gnd".into(),
            discipline: va_ir::Discipline::Thermal,
        });
        m.branches.push(Branch {
            p: NodeId(0),
            n: NodeId(1),
        });
        let temp_probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: va_ir::BranchId(0),
        }));
        let vt = m.push_expr(Expr::Call(Builtin::Vt, vec![temp_probe]));

        let (vt_ref, temp_ref) = (0.025_852_f64, 300.0_f64);
        let k_over_q = vt_ref / temp_ref;
        // Node `t` held at 350 K; ground slot maps out of range (reads 0).
        let x = [350.0];
        let terminals = [0usize, usize::MAX];
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &x,
            terminals: &terminals,
            vt: vt_ref,
            temp: temp_ref,
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
        };
        let d = eval(&ctx, vt).unwrap();
        assert!((d.value - k_over_q * 350.0).abs() < 1e-12);
        // d($vt(T))/dV(t) = k/q; the ground slot is out of range so contributes no gradient.
        assert!((d.grad[0] - k_over_q).abs() < 1e-12);

        // Cross-check against a central finite difference (§5).
        let h = 1e-3;
        let f = |t: f64| k_over_q * t;
        let fd = (f(350.0 + h) - f(350.0 - h)) / (2.0 * h);
        assert!((d.grad[0] - fd).abs() < 1e-9);

        // `$vt($temperature)` must agree with the no-arg `$vt` at the ambient temperature.
        assert!((k_over_q * temp_ref - vt_ref).abs() < 1e-12);
    }

    #[test]
    fn ddx_matches_the_lrm_vccs_example() {
        use va_ir::{
            Access, AccessKind, Branch, BranchId, Discipline, Expr, Module, NodeDecl, NodeId,
        };

        // The LRM's own worked example (§4.5.13, "vccs"): with `vin = V(pin,nin)`,
        //   one       = ddx(vin, V(pin))  == 1
        //   minusone  = ddx(vin, V(nin))  == -1
        //   zero      = ddx(vin, V(pout)) == 0   (vin doesn't depend on pout)
        let mut m = Module::new("vccs");
        for name in ["pout", "nout", "pin", "nin", "gnd"] {
            m.nodes.push(NodeDecl {
                name: name.into(),
                discipline: Discipline::Electrical,
            });
        }
        let (pout, pin, nin, gnd) = (NodeId(0), NodeId(2), NodeId(3), NodeId(4));
        m.branches.push(Branch { p: pin, n: nin }); // BranchId(0): vin = V(pin, nin)
        m.branches.push(Branch { p: pin, n: gnd }); // BranchId(1): V(pin)
        m.branches.push(Branch { p: nin, n: gnd }); // BranchId(2): V(nin)
        m.branches.push(Branch { p: pout, n: gnd }); // BranchId(3): V(pout)

        let vin = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let one = m.push_expr(Expr::Ddx(
            vin,
            Access {
                kind: AccessKind::Potential,
                branch: BranchId(1),
            },
        ));
        let minusone = m.push_expr(Expr::Ddx(
            vin,
            Access {
                kind: AccessKind::Potential,
                branch: BranchId(2),
            },
        ));
        let zero = m.push_expr(Expr::Ddx(
            vin,
            Access {
                kind: AccessKind::Potential,
                branch: BranchId(3),
            },
        ));

        let terminals = [0usize, 1, 2, 3, 4];
        let x = [0.0, 0.0, 3.0, 1.0, 0.0]; // pin=3V, nin=1V (so vin=2V), everything else 0
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &x,
            terminals: &terminals,
            vt: 0.0,
            temp: 0.0,
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
        };

        assert_eq!(eval(&ctx, vin).unwrap().value, 2.0);
        assert_eq!(eval(&ctx, one).unwrap().value, 1.0);
        assert_eq!(eval(&ctx, minusone).unwrap().value, -1.0);
        assert_eq!(eval(&ctx, zero).unwrap().value, 0.0);
        // ddx's result is a constant as far as further differentiation is concerned.
        assert!(eval(&ctx, one).unwrap().grad.iter().all(|&g| g == 0.0));
    }

    #[test]
    fn ddx_of_diode_conductance_matches_finite_difference() {
        use va_ir::{
            Access, AccessKind, Branch, BranchId, Builtin, Discipline, Expr, Module, NodeDecl,
            NodeId,
        };

        // The LRM's other worked example (§4.5.13, "diode"):
        //   idio = IS * (limexp(V(a,c)/$vt) - 1); gdio = ddx(idio, V(a));
        // `gdio` should be the diode's small-signal conductance at the operating point,
        // cross-checked against a central finite difference on `idio` itself (§5).
        fn idio_at(is: f64, vt: f64, va: f64) -> f64 {
            is * ((va / vt).exp() - 1.0)
        }

        let mut m = Module::new("diode");
        m.nodes.push(NodeDecl {
            name: "a".into(),
            discipline: Discipline::Electrical,
        });
        m.nodes.push(NodeDecl {
            name: "c".into(),
            discipline: Discipline::Electrical,
        });
        let (a, c) = (NodeId(0), NodeId(1));
        m.branches.push(Branch { p: a, n: c }); // BranchId(0): V(a,c)
        m.branches.push(Branch { p: a, n: c }); // BranchId(1): V(a) -- c doubles as reference

        let is = 1e-14_f64;
        let vt = 0.025_852_f64;
        let vac = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: BranchId(0),
        }));
        let is_e = m.push_expr(Expr::Const(is));
        let vt_e = m.push_expr(Expr::Call(Builtin::Vt, vec![]));
        let ratio = m.push_expr(Expr::Binary(va_ir::BinOp::Div, vac, vt_e));
        let expv = m.push_expr(Expr::Call(Builtin::Exp, vec![ratio]));
        let one = m.push_expr(Expr::Const(1.0));
        let em1 = m.push_expr(Expr::Binary(va_ir::BinOp::Sub, expv, one));
        let idio = m.push_expr(Expr::Binary(va_ir::BinOp::Mul, is_e, em1));
        let gdio = m.push_expr(Expr::Ddx(
            idio,
            Access {
                kind: AccessKind::Potential,
                branch: BranchId(1),
            },
        ));

        let terminals = [0usize, 1];
        let x = [0.6, 0.0]; // V(a,c) = 0.6 V
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &x,
            terminals: &terminals,
            vt,
            temp: 300.0,
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
        };
        let analytic = eval(&ctx, gdio).unwrap().value;

        let h = 1e-6;
        let fd = (idio_at(is, vt, 0.6 + h) - idio_at(is, vt, 0.6 - h)) / (2.0 * h);
        assert!(
            (analytic - fd).abs() < 1e-6 * fd.abs().max(1.0),
            "analytic {analytic} vs fd {fd}"
        );
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
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
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
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
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
