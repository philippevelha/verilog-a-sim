//! Forward-mode automatic differentiation over the [`va_ir`] expression arena.
//!
//! Evaluates an [`va_ir::ExprId`] to a [`Dual`]: its primal value paired with the partial
//! derivatives w.r.t. the model's local unknowns (one slot per node, plus one per branch with
//! its own auxiliary current unknown — see [`Ctx::branch_current_slots`]). The gradient feeds
//! the Jacobian stamps, so it must be exact — §5 checks it against a central finite difference.
//!
//! The active unknowns are the node voltages plus any branch currents. A potential probe
//! `V(p, n)` contributes `+1` in the `p` slot and `-1` in the `n` slot; a flow probe `I(...)`
//! contributes `+1` in its branch's own current slot, if that branch has one allocated — either
//! because it receives a potential contribution somewhere in the module, or because it's a
//! purely flow-defined branch that's also read via a bare `I(...)` probe somewhere (see
//! `crate::lower::FlowCurrentAccumulator`). Every other operator simply propagates gradients
//! through the chain rule.

use crate::CodegenError;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use va_ir::{BinOp, Builtin, Expr, ExprId, Function, Module, Stmt, UnOp, VarId};

/// A value carried with its gradient w.r.t. the active unknowns (a dual number), **split into
/// an instantaneous and a time-derivative channel**.
///
/// A `Dual` represents the local affine model
///
/// ```text
/// u  =  value  +  Σ grad[k]·δx[k]  +  Σ grad_ddt[k]·(d/dt)δx[k]
/// ```
///
/// [`Dual::grad`] is the ordinary conductance-like sensitivity that stamps into
/// `va_abi::StampSink::jacobian`; [`Dual::grad_ddt`] is the capacitance-like sensitivity that
/// stamps into `va_abi::StampSink::dcharge`, where the consumer supplies the `d/dt` — the
/// transient integrator's companion coefficient, or `jω` in AC.
///
/// # Why two real channels rather than complex numbers
///
/// Small-signal AC needs `ddt(u) → jωu`, which looks like it demands complex arithmetic. It
/// does not: AC *is* a linearization, so a nonlinearity `f` around the operating point
/// contributes `f'(u₀)·δu` with `f'(u₀)` an ordinary real number. The `jω` never enters the
/// chain rule — it reappears only when the assembler forms `G + jωC`. So both channels stay
/// **real**, and every rule below is the same rule applied twice, because differentiation is
/// linear.
#[derive(Clone, Debug)]
pub struct Dual {
    /// The primal value.
    pub value: f64,
    /// Partial derivatives w.r.t. each local unknown (node slot order) — the **instantaneous**
    /// channel, stamped as Jacobian entries. Read with [`Dual::grad`].
    grad: Grad,
    /// Partial derivatives w.r.t. each local unknown's **time derivative** — the charge channel,
    /// stamped as `dcharge` entries. Non-zero only downstream of a `ddt`. Read with
    /// [`Dual::grad_ddt`].
    grad_ddt: Grad,
}

/// One channel of a [`Dual`]'s gradient.
///
/// # Why this is not just a `Vec<f64>`
///
/// Most of a compact model is **bias-independent**: parameter range checks, temperature
/// scaling, geometry binning, corner interpolation. Measured on the CMC standard models
/// (2026-09-22), 78–87% of the `Dual`s a single `load()` builds depend on no unknown
/// whatsoever — 7816 of BSIM4's 9020. As a dense `Vec<f64>` every one of those cost two heap
/// allocations and `2n` multiply-adds to carry a gradient that was identically zero, on every
/// Newton iteration, for the whole of a simulation.
///
/// [`Grad::Zero`] makes that case free, and it is exact rather than heuristic: a value is
/// structurally independent of the unknowns precisely when both its operands were. Nothing has
/// to prove in advance which statements are bias-independent — the representation finds out as
/// it evaluates, which is also why it needs no invalidation rule when `$temperature` or a
/// parameter override changes.
///
/// A second, smaller benefit falls out: `Zero ⊗ Zero → Zero` never multiplies a zero partial by
/// an infinite coefficient, so the `0 · inf` NaNs that v1.1.1 fixed one rule at a time cannot
/// re-enter through a rule nobody thought about.
#[derive(Debug, Default, PartialEq)]
enum Grad {
    /// Structurally zero: this value depends on no unknown at all. Holds no allocation, and
    /// carries no length — a `Zero` is the zero gradient over *whatever* the ambient unknown
    /// count is, which is what lets a constant be built without being told it.
    #[default]
    Zero,
    /// One entry per local unknown, in slot order. Every `Dense` within one evaluation has the
    /// same length, since the only thing that introduces one is [`Dual::variable`].
    ///
    /// Shared, not owned: copying a `Dual` — which reading a local variable does, every time —
    /// shares the buffer instead of allocating a new one. `Rc<[f64]>`, not `Rc<Vec<f64>>`: the
    /// count and the partials live in **one** allocation, collected straight into it. The `Vec`
    /// form cost two per gradient (the `Rc` box and the buffer), which cancelled what sharing
    /// saved — a sampling profile showed it (1.14.0+1). A gradient is never modified after it
    /// is built, so sharing is safe. Before this, those copies were ~47% of every gradient
    /// allocation a PSP103 evaluation made (`crate::counters`, 2026-09-24).
    Dense(Rc<[f64]>),
}

/// By hand rather than derived, only so a copy of a dense gradient is counted
/// (`crate::counters::grad_clone`). The copy shares the buffer ([`Grad::Dense`]): a
/// reference-count bump, not an allocation.
impl Clone for Grad {
    fn clone(&self) -> Self {
        match self {
            Grad::Zero => Grad::Zero,
            Grad::Dense(v) => {
                crate::counters::grad_clone();
                Grad::Dense(Rc::clone(v))
            }
        }
    }
}

impl Grad {
    /// A dense view over `n` slots, for tests that want to compare a whole channel at once.
    #[cfg(test)]
    fn to_vec(&self, n: usize) -> Vec<f64> {
        (0..n).map(|i| self.at(i)).collect()
    }

    /// The **non-zero** partials, as `(slot, value)`.
    fn iter(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        let dense: &[f64] = match self {
            Grad::Zero => &[],
            Grad::Dense(v) => v,
        };
        dense
            .iter()
            .enumerate()
            .filter(|(_, g)| **g != 0.0)
            .map(|(i, g)| (i, *g))
    }

    /// The partial w.r.t. `slot`, or `0.0` if this channel is zero or `slot` is out of range.
    fn at(&self, slot: usize) -> f64 {
        match self {
            Grad::Zero => 0.0,
            Grad::Dense(v) => v.get(slot).copied().unwrap_or(0.0),
        }
    }

    /// Whether any partial is non-zero. `Zero` answers without looking at anything.
    fn any_nonzero(&self) -> bool {
        match self {
            Grad::Zero => false,
            Grad::Dense(v) => v.iter().any(|&g| g != 0.0),
        }
    }

    /// Another handle on the same buffer, for a by-reference caller of an owned operation:
    /// the extra handle makes the buffer shared, so the operation allocates rather than
    /// writing in place — exactly what a borrowed operand requires. Not counted as a clone:
    /// it is how the by-reference entry points reach the one implementation, not a copy a
    /// model made.
    fn share(&self) -> Grad {
        match self {
            Grad::Zero => Grad::Zero,
            Grad::Dense(v) => Grad::Dense(Rc::clone(v)),
        }
    }

    /// Apply `f` elementwise, on a gradient this call owns: written **in place** when nothing
    /// else holds the buffer, into a new one otherwise — the same `f` on the same elements
    /// either way, so the result is bit-identical; only where it is written differs. A
    /// by-reference caller passes [`Self::share`], which makes the buffer shared.
    ///
    /// **`f(0.0)` must be `0.0`**, which every caller guarantees by mapping a zero partial to
    /// zero explicitly — see [`Dual::chain`] for why that matters at a singularity. `Zero` is
    /// therefore left alone rather than materialized.
    fn map_owned(self, f: impl Fn(f64) -> f64) -> Grad {
        match self {
            Grad::Zero => Grad::Zero,
            Grad::Dense(mut v) => {
                if let Some(buf) = Rc::get_mut(&mut v) {
                    crate::counters::grad_in_place();
                    for g in buf.iter_mut() {
                        *g = f(*g);
                    }
                    Grad::Dense(v)
                } else {
                    crate::counters::grad_alloc();
                    Grad::Dense(v.iter().map(|g| f(*g)).collect())
                }
            }
        }
    }
}

impl Dual {
    /// A constant: no dependence on any unknown, and no allocation. The ambient unknown count
    /// is not needed — see [`Grad::Zero`].
    pub fn constant(value: f64) -> Self {
        Self {
            value,
            grad: Grad::Zero,
            grad_ddt: Grad::Zero,
        }
    }

    /// An independent variable: value `value`, unit derivative in slot `i` of `n`.
    pub fn variable(value: f64, i: usize, n: usize) -> Self {
        crate::counters::grad_alloc();
        let mut grad = vec![0.0; n];
        grad[i] = 1.0;
        Self {
            value,
            grad: Grad::Dense(Rc::from(grad)),
            grad_ddt: Grad::Zero,
        }
    }

    /// The **non-zero** instantaneous partials, as `(slot, value)` — the Jacobian channel.
    ///
    /// Only non-zero entries are yielded, which is what every consumer wants: a stamp of `0.0`
    /// is a stamp not worth making, and every call site used to open with `if dg != 0.0`. A
    /// [`Grad::Zero`] channel yields nothing at all.
    pub fn grad(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.grad.iter()
    }

    /// The **non-zero** time-derivative partials, as `(slot, value)` — the charge channel.
    pub fn grad_ddt(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.grad_ddt.iter()
    }

    /// The instantaneous partial w.r.t. local unknown `slot`, or `0.0` if there is none.
    pub fn grad_at(&self, slot: usize) -> f64 {
        self.grad.at(slot)
    }

    /// The time-derivative partial w.r.t. local unknown `slot`, or `0.0` if there is none.
    pub fn grad_ddt_at(&self, slot: usize) -> f64 {
        self.grad_ddt.at(slot)
    }

    /// The instantaneous channel as a dense `n`-slot vector — tests only. Production code
    /// iterates [`Dual::grad`] instead, which is what makes a [`Grad::Zero`] free.
    #[cfg(test)]
    fn grad_vec(&self, n: usize) -> Vec<f64> {
        self.grad.to_vec(n)
    }

    /// Whether this value depends on any unknown at all.
    pub fn depends_on_unknowns(&self) -> bool {
        self.grad.any_nonzero() || self.grad_ddt.any_nonzero()
    }

    /// Assemble from both channels explicitly — used where a rule builds its gradients by hand.
    fn from_parts(value: f64, grad: Grad, grad_ddt: Grad) -> Dual {
        Dual {
            value,
            grad,
            grad_ddt,
        }
    }

    /// Whether this value depends on any unknown's *time derivative* — i.e. whether it carries
    /// charge.
    pub fn carries_charge(&self) -> bool {
        self.grad_ddt.any_nonzero()
    }

    /// Move the instantaneous channel into the time-derivative channel — the effect of `ddt(·)`
    /// on the *gradients*. `value` is supplied by the caller, since `d/dt` of the primal is an
    /// integrator question, not an algebraic one (see `eval`'s `Builtin::Ddt` arm).
    ///
    /// # Errors
    ///
    /// Returns [`CodegenError::Unsupported`] if `self` already carries charge: that would be a
    /// second time derivative, and `va_abi::StampSink` has exactly one charge channel.
    pub fn into_ddt(self, value: f64) -> Result<Dual, CodegenError> {
        if self.carries_charge() {
            return Err(unsupported(
                "ddt of a value that already depends on a time derivative is a second time \
                 derivative, which this project's single charge channel cannot express",
            ));
        }
        Ok(Dual::from_parts(value, Grad::Zero, self.grad))
    }

    /// Scale value and gradient by a constant `s`.
    pub fn scale(&self, s: f64) -> Dual {
        self.share().scale_owned(s)
    }

    /// [`Self::scale`] consuming `self`, so the gradient can be scaled in place.
    pub fn scale_owned(self, s: f64) -> Dual {
        // A zero partial maps to zero explicitly rather than being multiplied: for an
        // infinite `s` it must still contribute nothing rather than a NaN, exactly as in
        // `chain`.
        let f = |g: f64| if g == 0.0 { 0.0 } else { g * s };
        Dual::from_parts(
            self.value * s,
            self.grad.map_owned(f),
            self.grad_ddt.map_owned(f),
        )
    }

    /// Another handle on the same gradients (see [`Grad::share`]): how each by-reference
    /// operation reaches its by-value implementation without copying, and without writing
    /// into a buffer its caller still holds.
    fn share(&self) -> Dual {
        Dual::from_parts(self.value, self.grad.share(), self.grad_ddt.share())
    }

    /// Sum: `(a + b)' = a' + b'`.
    pub fn add(&self, o: &Dual) -> Dual {
        self.share().add_owned(o.share())
    }

    /// [`Self::add`] consuming both operands, so the result can be written into one of their
    /// gradient buffers instead of a new one ([`zip_owned`]). Bit-identical to [`Self::add`].
    pub fn add_owned(self, o: Dual) -> Dual {
        Dual::from_parts(
            self.value + o.value,
            zip_owned(self.grad, o.grad, |a, b| a + b),
            zip_owned(self.grad_ddt, o.grad_ddt, |a, b| a + b),
        )
    }

    /// Difference: `(a - b)' = a' - b'`.
    pub fn sub(&self, o: &Dual) -> Dual {
        self.share().sub_owned(o.share())
    }

    /// [`Self::sub`] consuming both operands (see [`Self::add_owned`]).
    pub fn sub_owned(self, o: Dual) -> Dual {
        Dual::from_parts(
            self.value - o.value,
            zip_owned(self.grad, o.grad, |a, b| a - b),
            zip_owned(self.grad_ddt, o.grad_ddt, |a, b| a - b),
        )
    }

    /// Product: `(a*b)' = a'b + ab'` — applied to both channels, which is exactly the
    /// linearization `δ(uv) = v₀·δu + u₀·δv` split by which channel each `δ` came from.
    ///
    /// This is where a bias-dependent charge coefficient becomes representable: `c(x)·ddt(q)`
    /// yields `grad_ddt = c₀·∂q/∂x` **and** `grad = (dq/dt)·∂c/∂x` — the two halves of the
    /// product rule — provided the `ddt` carries a real primal value (see [`Dual::into_ddt`]).
    pub fn mul(&self, o: &Dual) -> Dual {
        self.share().mul_owned(o.share())
    }

    /// [`Self::mul`] consuming both operands (see [`Self::add_owned`]).
    pub fn mul_owned(self, o: Dual) -> Dual {
        let (sv, ov) = (self.value, o.value);
        Dual::from_parts(
            sv * ov,
            zip_owned(self.grad, o.grad, |a, b| a * ov + b * sv),
            zip_owned(self.grad_ddt, o.grad_ddt, |a, b| a * ov + b * sv),
        )
    }

    /// Quotient: `(a/b)' = (a'b - ab') / b²`.
    pub fn div(&self, o: &Dual) -> Dual {
        self.share().div_owned(o.share())
    }

    /// [`Self::div`] consuming both operands (see [`Self::add_owned`]).
    pub fn div_owned(self, o: Dual) -> Dual {
        let (sv, ov) = (self.value, o.value);
        let inv = 1.0 / ov;
        let inv2 = inv * inv;
        let rule = |a: f64, b: f64| (a * ov - sv * b) * inv2;
        Dual::from_parts(
            sv * inv,
            zip_owned(self.grad, o.grad, rule),
            zip_owned(self.grad_ddt, o.grad_ddt, rule),
        )
    }

    /// Negation.
    pub fn neg(&self) -> Dual {
        self.scale(-1.0)
    }

    /// [`Self::neg`] consuming `self`, so the gradient is negated in place.
    pub fn neg_owned(self) -> Dual {
        self.scale_owned(-1.0)
    }

    /// Apply a differentiable unary function given its value and derivative at `self.value`.
    ///
    /// The chain rule scales **both** channels by the same `dvalue`: around the operating point
    /// `f(u₀ + δu) ≈ f(u₀) + f'(u₀)·δu`, and it does not matter whether a given part of `δu`
    /// came from `δx` or from `dδx/dt`. This one line is why no nonlinear operator needed
    /// changing.
    fn chain(&self, value: f64, dvalue: f64) -> Dual {
        self.share().chain_owned(value, dvalue)
    }

    /// [`Self::chain`] consuming `self`, so the gradient is scaled in place (see
    /// [`Grad::map_owned`]); the guard and the arithmetic are the same.
    fn chain_owned(self, value: f64, dvalue: f64) -> Dual {
        // A channel whose incoming gradient is exactly `0.0` stays `0.0` rather than being
        // multiplied: the value does not depend on that unknown at all, so its contribution is
        // zero whatever `dvalue` is. Multiplying anyway is wrong at a function's *own*
        // singularity, where `dvalue` is `±inf` and `inf · 0.0` is NaN. That NaN then
        // propagates through every later expression and poisons Jacobian rows the singular
        // subexpression never reached. BSIM4's `T11 = sqrt(jtweff / weffCJ) + 1.0` is the case
        // that found this: both operands are *parameters*, so every channel is 0 and the
        // statement cannot affect the Jacobian at all — yet `sqrt(0.0)` gave it an all-NaN
        // gradient that reached the drain node's row and ended the solve on iteration 1.
        let scale = |g: f64| if g == 0.0 { 0.0 } else { g * dvalue };
        Dual::from_parts(
            value,
            self.grad.map_owned(scale),
            self.grad_ddt.map_owned(scale),
        )
    }

    /// [`Self::chain`] for a caller outside this impl: a value and its derivative w.r.t.
    /// `self`, combined by the chain rule with the zero-channel guard `chain` documents.
    pub fn chain_public(&self, value: f64, dvalue: f64) -> Dual {
        self.chain(value, dvalue)
    }

    /// `exp`.
    pub fn exp(&self) -> Dual {
        self.share().exp_owned()
    }

    /// [`Self::exp`] consuming `self` (see [`Self::chain_owned`]).
    pub fn exp_owned(self) -> Dual {
        let e = self.value.exp();
        self.chain_owned(e, e)
    }

    /// Natural log.
    pub fn ln(&self) -> Dual {
        self.share().ln_owned()
    }

    /// [`Self::ln`] consuming `self`.
    pub fn ln_owned(self) -> Dual {
        let (v, d) = (self.value.ln(), 1.0 / self.value);
        self.chain_owned(v, d)
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
        self.share().sqrt_owned()
    }

    /// [`Self::sqrt`] consuming `self`.
    pub fn sqrt_owned(self) -> Dual {
        let r = self.value.sqrt();
        self.chain_owned(r, 0.5 / r)
    }

    /// Absolute value (derivative `sign(x)`; subgradient `0` at the kink).
    pub fn abs(&self) -> Dual {
        self.share().abs_owned()
    }

    /// [`Self::abs`] consuming `self`.
    pub fn abs_owned(self) -> Dual {
        let (v, d) = (self.value.abs(), self.value.signum());
        self.chain_owned(v, d)
    }

    /// Power `self ** exp`.
    ///
    /// The base's partial is the closed form `v·u^(v-1)`, **not** the logarithmic form
    /// `u^v·(v·u'/u + v'·ln u)` that a general two-operand rule would use. The two agree
    /// wherever both are defined, but the logarithmic one is NaN in two places where the true
    /// derivative is finite:
    ///
    /// - `u = 0` with `v > 1`: `u^v` is `0` and `v·u'/u` is `±inf`, so their product is NaN,
    ///   where `d/du u^1.5 = 1.5·√u = 0`. Newton's first iterate reads every probe as `0.0`,
    ///   so this is not an edge case — it is where every DC solve starts. A compact model
    ///   raising a bias-dependent quantity to a power (BSIM4, BSIM-BULK, PSP103 all do)
    ///   produced a NaN Jacobian entry on iteration 1 and the solve was abandoned before it
    ///   had moved.
    /// - `u < 0` with integer `v`: `ln u` is NaN, where `d/du u² = 2u` is finite. Any
    ///   `pow(x, 2)` over a probe that can go negative was unusable.
    ///
    /// `v·u^(v-1)` is still `±inf` at `u = 0` for `v < 1` (`d/du √u` really is unbounded
    /// there) — that is the function's own singularity, not an artefact of the rule.
    ///
    /// The exponent's partial `u^v·ln u` is formed **only when the exponent actually varies**.
    /// `ln u` is `-inf`/NaN for `u ≤ 0`, and multiplying it by a zero derivative it is not a
    /// term of would poison the result with the same `0·inf` NaN.
    pub fn powf(&self, exp: &Dual) -> Dual {
        self.share().powf_owned(exp.share())
    }

    /// [`Self::powf`] consuming both operands (see [`Self::add_owned`]).
    pub fn powf_owned(self, exp: Dual) -> Dual {
        let (u, v) = (self.value, exp.value);
        let value = u.powf(v);
        let d_du = v * u.powf(v - 1.0);
        let exp_varies = exp.grad.any_nonzero() || exp.grad_ddt.any_nonzero();
        let d_dv = if exp_varies { value * u.ln() } else { 0.0 };
        // A zero derivative contributes nothing, and is skipped rather than multiplied: at a
        // singular `u` the partial is `inf`, and `inf · 0.0` is NaN rather than the 0 that a
        // channel the operand does not depend on must contribute.
        let rule = |sg: f64, eg: f64| {
            let base = if sg == 0.0 { 0.0 } else { d_du * sg };
            let expo = if eg == 0.0 { 0.0 } else { d_dv * eg };
            base + expo
        };
        Dual::from_parts(
            value,
            zip_owned(self.grad, exp.grad, rule),
            zip_owned(self.grad_ddt, exp.grad_ddt, rule),
        )
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
        let rule = |yg: f64, xg: f64| (x.value * yg - y.value * xg) / denom;
        Dual::from_parts(
            y.value.atan2(x.value),
            zip_with(&y.grad, &x.grad, rule),
            zip_with(&y.grad_ddt, &x.grad_ddt, rule),
        )
    }

    /// Euclidean norm `hypot(self, o) = √(self²+o²)`:
    /// `d hypot = (self·dself + o·do) / hypot`.
    pub fn hypot(&self, o: &Dual) -> Dual {
        let value = self.value.hypot(o.value);
        let rule = |sg: f64, og: f64| (self.value * sg + o.value * og) / value;
        Dual::from_parts(
            value,
            zip_with(&self.grad, &o.grad, rule),
            zip_with(&self.grad_ddt, &o.grad_ddt, rule),
        )
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

/// A probe read's gradient over `count` local unknowns: `+1` at `plus`, `−1` at `minus`, zero
/// elsewhere — `V(p, n)` is `(p, Some(n))`, a flow or `idt` read `(slot, None)`. A slot out of
/// range (ground) simply never matches.
///
/// Collected straight into the `Rc<[f64]>`: a mapped range reports its exact length, so this is
/// **one** allocation. Filling a `Vec` and then `Rc::from(vec)`, as before 1.14.0+3, allocated
/// twice and copied — twelve extra allocations per PSP103 evaluation. Each slot's arithmetic is
/// the old code's (`0.0`, `+= 1.0`, `-= 1.0`, in that order), so the result is bit-identical,
/// `p == n` included.
fn probe_grad(count: usize, plus: usize, minus: Option<usize>) -> Grad {
    Grad::Dense(
        (0..count)
            .map(|k| {
                let mut g = 0.0;
                if k == plus {
                    g += 1.0;
                }
                if Some(k) == minus {
                    g -= 1.0;
                }
                g
            })
            .collect(),
    )
}

/// Combine two gradient channels elementwise.
///
/// **`f(0.0, 0.0)` must be `0.0`** — true of every rule that uses this, each of which is linear
/// in the two partials — and that is what makes the `Zero`/`Zero` case answerable without
/// materializing anything. That case is the common one: it is every constant folded against
/// every other constant, 78–87% of the work in a CMC compact model (see [`Grad`]).
fn zip_with(a: &Grad, b: &Grad, f: impl Fn(f64, f64) -> f64) -> Grad {
    zip_owned(a.share(), b.share(), f)
}

/// [`zip_with`] on gradients this call owns — the one implementation, in three cases:
///
/// - **both zero**: zero, no allocation (the common case, see [`Grad`]);
/// - **exactly one zero**: `f` applied to the other operand's partials alone — `f(g, 0.0)` or
///   `f(0.0, g)`, a single pass over one buffer, written in place when that buffer is not
///   shared ([`Grad::map_owned`]);
/// - **both dense**: written into whichever operand's buffer is not shared, else a new one.
///
/// Each rule's own `f` is applied to the same elements in every case, so the result is
/// bit-identical to computing it into a fresh vector; only the buffer differs. `f` is kept
/// rather than reduced to two coefficients `ca·p + cb·q`: some rules round differently in that
/// form (`div`'s `(a·B − A·b)·inv²`), and bit-identity is what lets a change here be checked
/// against every deck exactly.
fn zip_owned(a: Grad, b: Grad, f: impl Fn(f64, f64) -> f64) -> Grad {
    match (a, b) {
        (Grad::Zero, Grad::Zero) => Grad::Zero,
        (x @ Grad::Dense(_), Grad::Zero) => x.map_owned(|g| f(g, 0.0)),
        (Grad::Zero, y @ Grad::Dense(_)) => y.map_owned(|g| f(0.0, g)),
        (Grad::Dense(mut x), Grad::Dense(mut y)) => {
            debug_assert_eq!(x.len(), y.len(), "both duals span the same unknowns");
            if let Some(xb) = Rc::get_mut(&mut x) {
                crate::counters::grad_in_place();
                for (p, &q) in xb.iter_mut().zip(y.iter()) {
                    *p = f(*p, q);
                }
                Grad::Dense(x)
            } else if let Some(yb) = Rc::get_mut(&mut y) {
                crate::counters::grad_in_place();
                for (q, &p) in yb.iter_mut().zip(x.iter()) {
                    *q = f(p, *q);
                }
                Grad::Dense(y)
            } else {
                crate::counters::grad_alloc();
                Grad::Dense(x.iter().zip(y.iter()).map(|(&p, &q)| f(p, q)).collect())
            }
        }
    }
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
    ///
    /// Distinct from `analysis.temp`, deliberately and for now: this is the temperature the
    /// model was *compiled* at (`crate::GeneratedModel::temp`), and it is what `$temperature`
    /// and `$vt` read, exactly as before the analysis context existed. Sourcing them from the
    /// simulator's own temperature instead is a real improvement and a separate change — it
    /// would move every compiled model's answer the moment a caller passes a non-nominal
    /// temperature, which is not something to fold silently into an unrelated one.
    pub temp: f64,
    /// What the simulator says about the evaluation being performed — the analysis kind and
    /// the absolute time, read by `$abstime`, `analysis()` and `ac_stim`.
    pub analysis: va_abi::AnalysisCtx,
    /// Which of this instance's monitored `cross` sites fired at the timepoint being
    /// evaluated — the notification half of Interface β's event channel
    /// (`va_abi::ModelState::event_fired`), indexed by `va_ir::Module::event_sites` position.
    ///
    /// Empty for the overwhelming majority of evaluations: an event fires at one timepoint,
    /// not continuously, and most models declare no events at all.
    pub events_fired: &'a [bool],
    /// This instance's state slots **as committed at the last accepted timepoint** — the
    /// read half of Interface β's state channel (`va_abi::ModelState::get`).
    pub state_prev: &'a [f64],
    /// This evaluation's state proposal, pre-seeded from `state_prev`.
    ///
    /// Held here rather than as a borrowed `&mut va_abi::ModelState` for the same reason
    /// `vars` is a `RefCell`: `eval` takes `&Ctx` all the way down, and a `transition`/`slew`
    /// deep inside an expression must be able to record its new output. `crate::GeneratedModel::
    /// load` drains this into the real `ModelState` once the walk finishes.
    pub state_next: RefCell<Vec<f64>>,
    /// Maps a `transition`/`slew` call site (by its own `ExprId.0`) to its base state slot
    /// (`crate::lower::StatefulCall`).
    pub state_slots: HashMap<u32, (crate::lower::StatefulKind, usize)>,
    /// The tightest `bound_step(...)` request the statement walk has evaluated this
    /// `load()`/`validate()` call, or `None` if none ran.
    ///
    /// Accumulated here rather than emitted straight into the [`va_abi::StampSink`] for the
    /// same reason `flow_current_totals` is: `crate::GeneratedModel::walk` is shared with the
    /// noise channel, which has no stamp sink, so its callback cannot own one. The requests
    /// land here during the walk and `crate::GeneratedModel::load` drains them afterwards.
    /// `Cell` rather than `RefCell` — an `Option<f64>` is `Copy`, so no borrow is needed.
    pub bound_step: Cell<Option<f64>>,
    /// Local-variable bindings accumulated by sequential `Stmt::Assign` execution (the
    /// statement walk in `crate::GeneratedModel::load`/`validate`), keyed by `VarId`.
    /// Interior-mutable so it can be populated through a shared `&Ctx`: every recursive `eval`
    /// call already takes `&Ctx`, and only ever *reads* a binding via [`Self::get_var`] — writes
    /// happen exactly once per `Stmt::Assign`, from the outer statement walk via
    /// [`Self::set_var`], never from within expression evaluation itself.
    /// Indexed by `VarId.0`, not hashed by it: a compact model reads its locals constantly —
    /// BSIM4 declares 1446 and touches them thousands of times per `load()` — and a `HashMap`
    /// charged a SipHash for every one of those reads to look up a slot a plain index finds.
    /// Grown on demand by [`Self::set_var`] rather than pre-sized, so a caller constructing a
    /// `Ctx` by hand does not have to know the module's variable count.
    pub vars: RefCell<Vec<Option<Dual>>>,
    /// Bindings produced by the module's **setup** — the bias-independent prefix of its
    /// statements, evaluated once per instance and reused by every later `load`
    /// (`crate::lower::Lowered::static_prefix`). Indexed by `VarId.0`, like [`Self::vars`],
    /// and read only as a fallback: a variable the current walk has assigned shadows the
    /// setup's value, which is what keeps an ordinary reassignment behaving as it always did.
    ///
    /// Empty for a context that is walking the setup itself, and for `validate`/`noise`/
    /// `events`, which still walk every statement from the top.
    pub static_vars: &'a [Option<Dual>],
    /// `crate::lower::Invariance::hoistable`, indexed by `ExprId.0`, for the tree-walk
    /// counters only (`crate::counters::expr_visit`): whether a visited node is one a hoisting
    /// stage could skip. Empty for a hand-built context, which then counts none as hoistable.
    pub hoistable: &'a [bool],
    /// Maps a branch (by `BranchId.0`) to the local terminal slot of its own auxiliary current
    /// unknown — populated from both `crate::lower::Lowered::branch_currents` (a branch with a
    /// potential contribution) and `crate::lower::Lowered::flow_current_accumulators` (a purely
    /// flow-defined branch also read via a bare `I(...)` probe); a flow probe reads the same way
    /// regardless of which reason gave the branch its slot. A flow probe `I(...)` on a branch
    /// absent from this map has no current unknown to read and is rejected.
    pub branch_current_slots: HashMap<u32, usize>,
    /// Maps an `idt(...)` call site (by its own `ExprId.0`) to the local terminal slot of its
    /// auxiliary accumulator unknown (`crate::lower::Lowered::idt_accumulators`). Consulted by
    /// [`eval`]'s `Builtin::Idt` case to read the call's *value* — see
    /// `crate::lower::IdtAccumulator`'s doc comment for why this is a plain unknown read rather
    /// than anything resembling `ddt`'s charge-channel handling.
    pub idt_slots: HashMap<u32, usize>,
    /// Per-`load()`-call bookkeeping for a branch that mixes flow and potential contributions
    /// (`crate::lower::BranchCurrent::mixed`): the local slots whose constraint-row structural
    /// stamp has already been applied *this call*, because a potential contribution has
    /// already run for them. `crate::GeneratedModel::stamp`/`mark_potential_used` populate and
    /// consult this; a non-mixed branch never touches it (its structural stamp is instead
    /// unconditional, see `crate::lower::BranchCurrent`'s doc comment).
    pub mixed_branch_potential_used: RefCell<HashSet<usize>>,
    /// Running per-branch sum of every flow contribution's resistive total this `load()`/
    /// `validate()` call, keyed by `BranchId.0` — only ever populated for a branch in
    /// `crate::lower::Lowered::flow_current_accumulators` (`crate::GeneratedModel::stamp`
    /// populates it; `crate::GeneratedModel::stamp_flow_current_accumulators` consumes it after
    /// the statement walk finishes). See `crate::lower::FlowCurrentAccumulator`'s doc comment.
    pub flow_current_totals: RefCell<HashMap<u32, Dual>>,
    /// Whether this `Ctx` belongs to `crate::GeneratedModel::validate`'s dry run rather than a
    /// real `crate::GeneratedModel::load` call. Consulted only when evaluating a user-defined
    /// analog function's own internal control flow (see [`call_function`]): validating visits
    /// every `if`/`case` arm unconditionally and never actually iterates a loop, the same
    /// eager-but-sound over-approximation `crate::GeneratedModel::validate_stmts` already
    /// applies to the top-level analog block, for the same reason (an arm/iteration a
    /// particular call doesn't happen to reach could still be the one a different real
    /// operating point's arguments select).
    pub validating: bool,
}

impl Ctx<'_> {
    /// Record that a potential contribution just ran for the branch whose auxiliary current
    /// unknown lives at `local_slot`. Returns `true` the first time this is called for
    /// `local_slot` in this `Ctx`'s lifetime (i.e. this `load()`/`validate()` call) — the signal
    /// `crate::GeneratedModel::stamp` uses to know whether it still owes that branch its
    /// constraint row's structural (`V(p)-V(n)`) stamp.
    pub fn mark_potential_used(&self, local_slot: usize) -> bool {
        crate::counters::ctx_map_lookup();
        self.mixed_branch_potential_used
            .borrow_mut()
            .insert(local_slot)
    }

    /// Read state slot `base + k` as committed at the last accepted timepoint.
    pub fn state_get(&self, base: usize, k: usize) -> f64 {
        self.state_prev.get(base + k).copied().unwrap_or(0.0)
    }

    /// Propose state slot `base + k` for this evaluation.
    pub fn state_set(&self, base: usize, k: usize, v: f64) {
        if let Some(cell) = self.state_next.borrow_mut().get_mut(base + k) {
            *cell = v;
        }
    }

    /// Record a `bound_step(dt)` request, keeping the tightest seen so far.
    ///
    /// Non-positive and non-finite values are dropped here rather than passed on: the LRM gives
    /// them no meaning, and a zero bound reaching the timestep controller would wedge it against
    /// its own floor (`va_abi::StampSink::bound_step` makes the same check, independently —
    /// this one keeps a bad request from displacing a good one recorded earlier).
    pub fn request_bound_step(&self, dt: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.bound_step
                .set(Some(self.bound_step.get().map_or(dt, |cur| cur.min(dt))));
        }
    }

    /// Add `value` into the running resistive total for `branch` (by `BranchId.0`) — a branch
    /// may receive more than one flow contribution (`crate::lower::FlowCurrentAccumulator`'s
    /// `diode_basic.va` example has two), each folded in as it runs.
    pub fn add_flow_current(&self, branch: u32, value: &Dual) {
        // A get and an insert: two lookups.
        crate::counters::ctx_map_lookup();
        crate::counters::ctx_map_lookup();
        let mut totals = self.flow_current_totals.borrow_mut();
        let updated = match totals.get(&branch) {
            Some(existing) => existing.add(value),
            None => value.clone(),
        };
        totals.insert(branch, updated);
    }
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
        let mut vars = self.vars.borrow_mut();
        let i = id.0 as usize;
        if i >= vars.len() {
            vars.resize(i + 1, None);
        }
        vars[i] = Some(value);
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
        let i = id.0 as usize;
        if let Some(bound) = self.vars.borrow().get(i).cloned().flatten() {
            return Ok(bound);
        }
        self.static_vars
            .get(i)
            .cloned()
            .flatten()
            .ok_or_else(|| unsupported(&format!("variable #{} read before assignment", id.0)))
    }
}

/// A minimal complex number for evaluating a `laplace_*` transfer function. Kept local and
/// dependency-free, matching `va_acnoise::ac::Complex`'s own `(re, im)` convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cx(pub f64, pub f64);

impl Cx {
    pub(crate) fn mul(self, o: Cx) -> Cx {
        Cx(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }

    pub(crate) fn div(self, o: Cx) -> Cx {
        let d = o.0 * o.0 + o.1 * o.1;
        Cx(
            (self.0 * o.0 + self.1 * o.1) / d,
            (self.1 * o.0 - self.0 * o.1) / d,
        )
    }
}

/// Expand a flattened `(re, im)` root list into real polynomial coefficients in `s`, lowest
/// degree first, with the LRM's convention that a root `ζ` contributes the factor `(1 − s/ζ)`
/// and a root at the origin the factor `s` — the same convention [`laplace_at`] evaluates in
/// product form.
///
/// This is the one place the codegen expands roots, and it is for the **time-domain**
/// realization (`crate::lower::LaplaceStates`), which needs a polynomial to build its state
/// chain from; the AC path keeps evaluating the product, which is better conditioned. Complex
/// roots must come in conjugate pairs for the product to be real; if it is not (to `1e-9` of
/// the largest coefficient), that is an error rather than a silently discarded imaginary part.
pub fn expand_roots(pairs: &[f64]) -> Result<Vec<f64>, String> {
    let mut poly = vec![Cx(1.0, 0.0)];
    for pair in pairs.chunks_exact(2) {
        let r = Cx(pair[0], pair[1]);
        let factor = if r == Cx(0.0, 0.0) {
            [Cx(0.0, 0.0), Cx(1.0, 0.0)]
        } else {
            [Cx(1.0, 0.0), Cx(-1.0, 0.0).div(r)]
        };
        let mut next = vec![Cx(0.0, 0.0); poly.len() + 1];
        for (i, a) in poly.iter().enumerate() {
            for (j, b) in factor.iter().enumerate() {
                let prod = a.mul(*b);
                next[i + j] = Cx(next[i + j].0 + prod.0, next[i + j].1 + prod.1);
            }
        }
        poly = next;
    }
    let scale = poly
        .iter()
        .map(|c| c.0.abs())
        .fold(0.0_f64, f64::max)
        .max(f64::MIN_POSITIVE);
    if poly.iter().any(|c| c.1.abs() > 1e-9 * scale) {
        return Err(
            "laplace zeros/poles must come in complex-conjugate pairs: their expanded \
             polynomial is not real"
                .to_string(),
        );
    }
    Ok(poly.into_iter().map(|c| c.0).collect())
}

/// A Z-domain filter's polynomials in `z^-1`, lowest power first, plus the net power of `z`
/// its origin roots contribute. See [`zi_realization`].
#[derive(Debug, Clone, PartialEq)]
pub struct ZiRealization {
    /// Numerator coefficients `n_k` of `Σ n_k z^-k`.
    pub num: Vec<f64>,
    /// Denominator coefficients `d_k` of `Σ d_k z^-k`; `d_0 != 0`.
    pub den: Vec<f64>,
    /// Extra delay in samples from denominator origin roots not cancelled by numerator ones —
    /// each is a factor `z`, i.e. `z^-1` on the other side. Never negative: an uncancelled
    /// numerator origin root would be an advance, which [`zi_realization`] refuses.
    pub delay: usize,
}

/// Expand a Z-domain filter's numerator/denominator arguments into real polynomials in `z^-1`
/// (LRM §4.5.12): a coefficient list is already one; a `(re, im)` root list is the product
/// `∏(1 − r_k·z^-1)`, real when complex roots come in conjugate pairs, with a root at the
/// origin contributing the factor `z` instead (the LRM's rule), counted separately.
///
/// # Errors
///
/// An empty list; a root product that is not real (a lone complex root); `d_0 = 0` (the
/// difference equation cannot be solved for the current output — a denominator root product
/// always has `d_0 = 1`, so this is only reachable from a coefficient list); or more origin
/// roots in the numerator than in the denominator (an advance, which no causal filter can do).
pub fn zi_realization(
    num: &[f64],
    num_is_roots: bool,
    den: &[f64],
    den_is_roots: bool,
) -> Result<ZiRealization, String> {
    fn poly(vals: &[f64], is_roots: bool, what: &str) -> Result<(Vec<f64>, usize), String> {
        if vals.is_empty() {
            return Err(format!("a zi_* {what} list is empty"));
        }
        if !is_roots {
            return Ok((vals.to_vec(), 0));
        }
        let mut coeffs = vec![Cx(1.0, 0.0)];
        let mut origin = 0usize;
        for pair in vals.chunks_exact(2) {
            let r = Cx(pair[0], pair[1]);
            if r == Cx(0.0, 0.0) {
                origin += 1;
                continue;
            }
            // multiply by (1 − r·z^-1)
            let mut next = vec![Cx(0.0, 0.0); coeffs.len() + 1];
            for (i, c) in coeffs.iter().enumerate() {
                next[i] = Cx(next[i].0 + c.0, next[i].1 + c.1);
                let m = c.mul(r);
                next[i + 1] = Cx(next[i + 1].0 - m.0, next[i + 1].1 - m.1);
            }
            coeffs = next;
        }
        let scale = coeffs
            .iter()
            .map(|c| c.0.abs())
            .fold(0.0_f64, f64::max)
            .max(f64::MIN_POSITIVE);
        if coeffs.iter().any(|c| c.1.abs() > 1e-9 * scale) {
            return Err(format!(
                "zi_* {what} roots must come in complex-conjugate pairs: their expanded \
                 polynomial is not real"
            ));
        }
        Ok((coeffs.into_iter().map(|c| c.0).collect(), origin))
    }
    let (num, num_origin) = poly(
        num,
        num_is_roots,
        if num_is_roots { "zero" } else { "numerator" },
    )?;
    let (den, den_origin) = poly(
        den,
        den_is_roots,
        if den_is_roots { "pole" } else { "denominator" },
    )?;
    if den[0] == 0.0 {
        return Err(
            "a zi_* denominator has d_0 = 0: the difference equation cannot be solved for the \
             current output sample"
                .to_string(),
        );
    }
    if !num.iter().chain(&den).all(|c| c.is_finite()) {
        return Err("a zi_* coefficient is not finite".to_string());
    }
    if num_origin > den_origin {
        return Err(format!(
            "a zi_* filter with {} more zero(s) at the origin than poles at the origin is an \
             advance of {} sample(s), which no causal filter can produce (LRM 4.5.12: a root \
             at the origin is the factor z)",
            num_origin - den_origin,
            num_origin - den_origin
        ));
    }
    Ok(ZiRealization {
        num,
        den,
        delay: den_origin - num_origin,
    })
}

/// A Z-domain filter's gain at `z = e^{jωT}` (AC), or at `z = 1` when `omega == 0` (its
/// steady-state gain, the DC and noise answer). No zero-order-hold `sinc` factor: this is the
/// discrete filter's own response to a sampled sinusoid, and above the Nyquist frequency
/// `π/T` it aliases exactly as the mathematics does.
pub fn zi_at(omega: f64, period: f64, r: &ZiRealization) -> Cx {
    // z^-1 = e^{-jωT}
    let zinv = Cx((omega * period).cos(), -(omega * period).sin());
    fn poly(zinv: Cx, coeffs: &[f64]) -> Cx {
        let mut acc = Cx(0.0, 0.0);
        for &c in coeffs.iter().rev() {
            acc = acc.mul(zinv);
            acc.0 += c;
        }
        acc
    }
    let mut h = poly(zinv, &r.num).div(poly(zinv, &r.den));
    for _ in 0..r.delay {
        h = h.mul(zinv);
    }
    h
}

/// Evaluate a `laplace_*` transfer function `H(s)` at `s = j·omega`.
///
/// `num`/`den` are already-evaluated real numbers: either polynomial coefficients in `s`
/// (lowest degree first) or flattened `(re, im)` root pairs, per the `*_is_roots` flags.
///
/// **Root forms are evaluated in product form**, `∏(1 − s/ζ)`, never expanded into
/// coefficients: expansion is worse conditioned (the corpus carries a 7-coefficient filter with
/// values near `1e71`) and buys nothing here. A root at the **origin** contributes a factor of
/// `s` rather than `(1 − s/ζ)` — the LRM's own rule, and the one that makes a filter with a zero
/// at the origin report a DC gain of exactly `0`.
pub fn laplace_at(
    omega: f64,
    num: &[f64],
    num_is_roots: bool,
    den: &[f64],
    den_is_roots: bool,
) -> Cx {
    let s = Cx(0.0, omega);

    fn poly(s: Cx, coeffs: &[f64]) -> Cx {
        // Horner from the top down, so `Σ c_k s^k` costs one mul/add per coefficient.
        let mut acc = Cx(0.0, 0.0);
        for &c in coeffs.iter().rev() {
            acc = acc.mul(s);
            acc.0 += c;
        }
        acc
    }

    fn roots(s: Cx, pairs: &[f64]) -> Cx {
        let mut acc = Cx(1.0, 0.0);
        for pair in pairs.chunks_exact(2) {
            let r = Cx(pair[0], pair[1]);
            if r == Cx(0.0, 0.0) {
                // A root at the origin: the factor is `s`, not `1 − s/0`.
                acc = acc.mul(s);
            } else {
                let one_minus = Cx(1.0, 0.0);
                acc = acc.mul(Cx(one_minus.0 - s.div(r).0, one_minus.1 - s.div(r).1));
            }
        }
        acc
    }

    let n = if num_is_roots {
        roots(s, num)
    } else {
        poly(s, num)
    };
    let d = if den_is_roots {
        roots(s, den)
    } else {
        poly(s, den)
    };
    n.div(d)
}

/// Whether the analysis `analysis` describes is named by `mask`, a bitmask over
/// [`va_ir::ANALYSIS_PHASES`].
///
/// **This function is the bridge between the two frozen interfaces**, and `va-codegen` is the
/// only crate that can hold it: Interface α owns the mask encoding (`va-frontend` folds
/// `analysis()`'s string arguments into one at elaboration) and Interface β owns the runtime
/// answer (`va_abi::AnalysisKind::matches_phase`), and the two are leaf crates that cannot see
/// each other. Keeping the join in exactly one place is what stops the bit order from being
/// re-derived — and eventually mis-derived — somewhere else.
///
/// `analysis(...)` is an *any-of* query (LRM §4.5.1), so any single matching bit is enough; an
/// empty mask matches nothing.
pub fn phase_mask_active(analysis: &va_abi::AnalysisCtx, mask: u32) -> bool {
    va_ir::ANALYSIS_PHASES
        .iter()
        .enumerate()
        .any(|(bit, phase)| mask & (1 << bit) != 0 && analysis.kind.matches_phase(phase))
}

/// Evaluate `expr` under forward-mode AD in context `ctx`. A [`Expr::Var`] reads whatever
/// `ctx.vars` currently holds — see [`Ctx::set_var`] for who populates it and when.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] for IR constructs the v0 codegen does not evaluate in
/// value position: a flow probe on a branch with no potential contribution of its own, a bare
/// `ddt` (handled by the lowering split, not evaluated here — unlike `idt`, which *is* evaluated
/// here, as a plain read of its own accumulator unknown), a local variable read before it was
/// ever assigned, and anything [`call_function`] rejects (a `<+` contribution inside a function
/// body, a wrong argument count, or a runaway loop inside one).
pub fn eval(ctx: &Ctx, expr: ExprId) -> Result<Dual, CodegenError> {
    // Compiled in only with `--features walk-stats`: a check here runs on every node of every
    // evaluation (`counters::expr_visit`).
    #[cfg(feature = "walk-stats")]
    crate::counters::expr_visit(ctx.hoistable, expr.0 as usize);
    let count = ctx.count();
    match ctx.module.expr(expr) {
        Expr::Const(c) => Ok(Dual::constant(*c)),
        Expr::Param(p) => {
            let v = ctx
                .params
                .get(p.0 as usize)
                .copied()
                .ok_or_else(|| unsupported("parameter index out of range"))?;
            Ok(Dual::constant(v))
        }
        // `$param_given(p)`: resolved at the instantiation boundary, read here. Constant per
        // instance (the override set cannot change during a solve), hence zero-gradient.
        Expr::ParamGiven(p) => Ok(Dual::constant(if ctx.module.param_is_given(*p) {
            1.0
        } else {
            0.0
        })),
        // `$port_connected(i)`: resolved at the instantiation boundary, read here. Constant
        // per instance, hence zero-gradient.
        Expr::PortConnected(i) => Ok(Dual::constant(
            if ctx.module.port_is_connected(*i as usize) {
                1.0
            } else {
                0.0
            },
        )),
        // `@(cross(...))`'s guard: whether the consumer determined this site fired at the
        // timepoint being evaluated. Held fixed across the Newton iterations of one timepoint
        // (`va_abi::ModelState::event_fired`), so it is a constant here and zero-gradient.
        Expr::EventFired(slot) => Ok(Dual::constant(
            if ctx
                .events_fired
                .get(*slot as usize)
                .copied()
                .unwrap_or(false)
            {
                1.0
            } else {
                0.0
            },
        )),
        Expr::Var(id) => ctx.get_var(*id),
        Expr::Probe(access) => match access.kind {
            va_ir::AccessKind::Potential => {
                let br = ctx.module.branches[access.branch.0 as usize];
                let (p, n) = (br.p.0 as usize, br.n.0 as usize);
                let value = ctx.node_voltage(p) - ctx.node_voltage(n);
                crate::counters::probe_alloc();
                Ok(Dual::from_parts(
                    value,
                    probe_grad(count, p, Some(n)),
                    Grad::Zero,
                ))
            }
            va_ir::AccessKind::Flow => {
                crate::counters::ctx_map_lookup();
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
                crate::counters::probe_alloc();
                Ok(Dual::from_parts(
                    value,
                    probe_grad(count, slot, None),
                    Grad::Zero,
                ))
            }
        },
        Expr::Unary(op, e) => {
            let d = eval(ctx, *e)?;
            Ok(match op {
                UnOp::Neg => d.neg_owned(),
                UnOp::Not => Dual::constant(bool_to_f64(d.value == 0.0)),
                // Bitwise NOT, like the comparison/logical operators above, is an integer
                // operation with no continuous derivative — zero-gradient.
                UnOp::BitNot => Dual::constant(!to_i64(d.value) as f64),
            })
        }
        Expr::Binary(op, l, r) => {
            let a = eval(ctx, *l)?;
            let b = eval(ctx, *r)?;
            Ok(match op {
                // By value: `a` and `b` are this arm's own temporaries, so the result can be
                // written into one of their gradient buffers (`zip_owned`).
                BinOp::Add => a.add_owned(b),
                BinOp::Sub => a.sub_owned(b),
                BinOp::Mul => a.mul_owned(b),
                BinOp::Div => a.div_owned(b),
                // Modulus is genuinely discontinuous (it jumps at every multiple of `b`), so —
                // like the bitwise/comparison operators below — it's zero-gradient in AD rather
                // than attempting an analytic derivative.
                BinOp::Mod => Dual::constant(a.value % b.value),
                BinOp::Pow => a.powf_owned(b),
                BinOp::Lt => Dual::constant(bool_to_f64(a.value < b.value)),
                BinOp::Le => Dual::constant(bool_to_f64(a.value <= b.value)),
                BinOp::Gt => Dual::constant(bool_to_f64(a.value > b.value)),
                BinOp::Ge => Dual::constant(bool_to_f64(a.value >= b.value)),
                BinOp::Eq => Dual::constant(bool_to_f64(a.value == b.value)),
                BinOp::Ne => Dual::constant(bool_to_f64(a.value != b.value)),
                BinOp::And => Dual::constant(bool_to_f64(a.value != 0.0 && b.value != 0.0)),
                BinOp::Or => Dual::constant(bool_to_f64(a.value != 0.0 || b.value != 0.0)),
                // Bitwise/shift operators are integer operations with no continuous derivative,
                // same treatment as the comparison operators above: zero-gradient.
                BinOp::BitAnd => Dual::constant((to_i64(a.value) & to_i64(b.value)) as f64),
                BinOp::BitOr => Dual::constant((to_i64(a.value) | to_i64(b.value)) as f64),
                BinOp::BitXor => Dual::constant((to_i64(a.value) ^ to_i64(b.value)) as f64),
                BinOp::BitXnor => Dual::constant(!(to_i64(a.value) ^ to_i64(b.value)) as f64),
                BinOp::Shl => {
                    Dual::constant(to_i64(a.value).wrapping_shl(to_i64(b.value) as u32) as f64)
                }
                BinOp::Shr => Dual::constant(
                    (to_i64(a.value) as u64).wrapping_shr(to_i64(b.value) as u32) as f64,
                ),
            })
        }
        // `idt(...)`'s value is a plain read of its own accumulator unknown (see
        // `crate::lower::IdtAccumulator`'s doc comment) — never evaluated through `eval_call`'s
        // ordinary per-builtin dispatch, since its argument is never evaluated to produce this
        // call's *value* at all (only `crate::GeneratedModel::stamp_idt_accumulators` evaluates
        // it, to stamp the accumulator's own row). `expr` is this call's own id, exactly the key
        // `lower::lower` registered it under in `ctx.idt_slots`.
        Expr::Call(Builtin::Idt, _) => {
            crate::counters::ctx_map_lookup();
            let slot = *ctx.idt_slots.get(&expr.0).ok_or_else(|| {
                unsupported(
                    "idt accumulator not registered for this call site (internal codegen error)",
                )
            })?;
            let value = ctx.node_voltage(slot);
            crate::counters::probe_alloc();
            Ok(Dual::from_parts(
                value,
                probe_grad(count, slot, None),
                Grad::Zero,
            ))
        }
        Expr::Call(builtin, args) => eval_call(ctx, expr, *builtin, args),
        Expr::CallUser(fid, args) => {
            let func = &ctx.module.functions[fid.0 as usize];
            call_function(ctx, func, args)
        }
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
            Ok(Dual::constant(d.grad_at(p)))
        }
    }
}

/// `site` is the call's own `ExprId` — needed only by the stateful constructs
/// (`transition`/`slew`), which key their state slots on the call site so that the same
/// function written twice keeps two independent histories (§ `crate::lower::StatefulCall`).
fn eval_call(
    ctx: &Ctx,
    site: ExprId,
    builtin: Builtin,
    args: &[ExprId],
) -> Result<Dual, CodegenError> {
    let expr_id = site.0;
    let arg = |i: usize| -> Result<Dual, CodegenError> {
        let id = args
            .get(i)
            .ok_or_else(|| unsupported("built-in called with too few arguments"))?;
        eval(ctx, *id)
    };
    Ok(match builtin {
        Builtin::Exp => arg(0)?.exp_owned(),
        Builtin::Ln => arg(0)?.ln_owned(),
        Builtin::Log => arg(0)?.log10(),
        Builtin::Sqrt => arg(0)?.sqrt_owned(),
        Builtin::Abs => arg(0)?.abs_owned(),
        // Rounding functions are piecewise constant: value is the rounded primal, gradient 0.
        Builtin::Floor => Dual::constant(arg(0)?.value.floor()),
        Builtin::Ceil => Dual::constant(arg(0)?.value.ceil()),
        Builtin::Round => Dual::constant(arg(0)?.value.round()),
        Builtin::Int => Dual::constant(arg(0)?.value.trunc()),
        Builtin::Pow => arg(0)?.powf_owned(arg(1)?),
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
            None => Dual::constant(ctx.vt),
        },
        Builtin::Temperature => Dual::constant(ctx.temp),
        // The three analysis-context builtins. All are constants with respect to `x` — none is
        // a function of the solution vector — so all carry a zero gradient, and `va-codegen`'s
        // finite-difference tests confirm that rather than assuming it.
        //
        // `$abstime` is the absolute simulation time, `0.0` outside transient (the LRM-correct
        // answer for a static solve, not a placeholder).
        Builtin::Abstime => Dual::constant(ctx.analysis.time),
        // `analysis(...)`'s string arguments were folded to a bitmask over
        // `va_ir::ANALYSIS_PHASES` at elaboration; this is where that mask meets the analysis
        // actually running. An any-of query, so any set bit naming the current analysis wins.
        Builtin::Analysis => {
            let mask = match args.first().map(|&a| ctx.module.expr(a)) {
                Some(Expr::Const(m)) => *m as u32,
                _ => {
                    return Err(unsupported(
                        "analysis() expects a single constant phase bitmask argument \
                         (va-frontend folds its string arguments to one)",
                    ))
                }
            };
            Dual::constant(if phase_mask_active(&ctx.analysis, mask) {
                1.0
            } else {
                0.0
            })
        }
        // `ac_stim`'s *value* is zero in every analysis including AC — it is a right-hand-side
        // excitation, not a term in `G`. `crate::lower` splits a recognized one out of its
        // contribution into `va_abi::StampSink`'s excitation channel; reaching this arm at all
        // means the call sits somewhere that split could not pull it out of, and
        // `crate::GeneratedModel::validate` rejects that rather than letting it vanish.
        Builtin::AcStim => Dual::constant(0.0),
        // A `laplace_*` reaching expression evaluation means `crate::lower` could not pull it
        // out of its contribution — it was scaled, nested, or otherwise not a bare top-level
        // term. There is no real-valued answer to give: `H(jω)` is complex, and a `Dual` carries
        // a real value and a real gradient. `crate::GeneratedModel::validate` rejects this at
        // build time so it never surfaces mid-solve.
        Builtin::LaplaceNd | Builtin::LaplaceNp | Builtin::LaplaceZd | Builtin::LaplaceZp => {
            return Err(unsupported(
                "a laplace_* filter must be a top-level additive term of a contribution (its \
                 gain is complex, so there is nowhere in an ordinary expression to put it)",
            ))
        }
        // The Z-domain family too: complex gain in AC, and in transient a sampled history
        // that only the contribution-level stamp (`crate::GeneratedModel::stamp_zi`) carries.
        Builtin::ZiNd | Builtin::ZiNp | Builtin::ZiZd | Builtin::ZiZp => {
            return Err(unsupported(
                "a zi_* filter must be a top-level additive term of a contribution (its \
                 gain is complex, so there is nowhere in an ordinary expression to put it)",
            ))
        }
        // `absdelay` is in the same position and for the same reason: its gain
        // `exp(-jw*tau)` is complex, so a buried one has no real-valued answer either.
        Builtin::Absdelay => {
            return Err(unsupported(
                "absdelay must be a top-level additive term of a contribution (its gain \
                 is complex, so there is nowhere in an ordinary expression to put it)",
            ))
        }
        // `@(initial_step)`'s desugared condition. Pure solver knowledge, no state, no gradient.
        Builtin::InitialStep => Dual::constant(if ctx.analysis.is_initial_step {
            1.0
        } else {
            0.0
        }),
        // `$simparam("name")` (LRM 9.18), read from the analysis context. The argument is the
        // resolved selector, not a value to evaluate -- elaboration already decided that this
        // name is one this simulator knows, so there is no fallback to consider here.
        Builtin::SimParam => {
            let sel = match args.first().map(|&a| ctx.module.expr(a)) {
                Some(va_ir::Expr::Const(v)) => *v,
                _ => {
                    return Err(unsupported(
                        "`$simparam` must carry its resolved parameter selector as a constant                          (an IR built by hand got this wrong; see `va_ir::Builtin::SimParam`)",
                    ))
                }
            };
            let Some(p) = va_ir::SimParam::from_selector(sel) else {
                return Err(unsupported(
                    "`$simparam` carries a parameter selector no `va_ir::SimParam` uses",
                ));
            };
            let sim = &ctx.analysis.sim;
            let value = match p {
                va_ir::SimParam::Gdev => sim.gdev,
                va_ir::SimParam::Iteration => sim.iteration,
                va_ir::SimParam::SourceScaleFactor => sim.source_scale_factor,
                va_ir::SimParam::Abstol => sim.abstol,
                va_ir::SimParam::Reltol => sim.reltol,
            };
            Dual::constant(value)
        }
        // `$mfactor` (LRM 6.3.6), read from the module clone this instance was built from. A
        // number the instantiation fixed, so no gradient and no state -- and deliberately not
        // folded at elaboration, where the instance is not yet known.
        // `$table_model(x, "file" [, "control"])` — LRM §9.21, one dimension, table folded into
        // the arguments at elaboration (`va_ir::Builtin::TableModel` documents the layout).
        //
        // The derivative is the segment's own slope, which is what makes this safe to put in a
        // circuit equation at all: a lookup whose Jacobian did not match its value would not
        // error, it would converge Newton to the wrong answer. At a knot the function has a
        // kink and the slope is one-sided; the right-hand segment is taken, the same
        // subgradient convention `abs` uses at zero.
        Builtin::TableModel => {
            let x = arg(0)?;
            let code = arg(1)?.value as u32;
            let (interp, lo_extrap, hi_extrap) = (code / 100, (code / 10) % 10, code % 10);
            // Pairs start after the lookup expression and the control code.
            let pairs = &args[2..];
            if pairs.len() < 4 || !pairs.len().is_multiple_of(2) {
                return Err(unsupported(
                    "table_model needs at least two (x, y) pairs after its control code",
                ));
            }
            let n = pairs.len() / 2;
            let px = |i: usize| -> Result<f64, CodegenError> { Ok(eval(ctx, pairs[2 * i])?.value) };
            let py =
                |i: usize| -> Result<f64, CodegenError> { Ok(eval(ctx, pairs[2 * i + 1])?.value) };

            let xv = x.value;
            let (x0, xn) = (px(0)?, px(n - 1)?);
            // Which segment, and whether this is an extrapolation off either end. Linear scan
            // rather than a binary search: the tables this is for are tens of points, and the
            // arena reads are the cost here rather than the comparisons.
            let seg = if xv <= x0 {
                None // below
            } else if xv >= xn {
                Some(n - 1) // at or above the top point
            } else {
                let mut k = 0;
                while k + 1 < n && px(k + 1)? <= xv {
                    k += 1;
                }
                Some(k)
            };

            let (value, slope) = match seg {
                // Below the first point: extrapolate by the control's lower rule.
                None => {
                    if lo_extrap == 1 {
                        (py(0)?, 0.0) // constant: the endpoint value
                    } else {
                        let m = (py(1)? - py(0)?) / (px(1)? - px(0)?);
                        (py(0)? + (xv - x0) * m, m)
                    }
                }
                Some(k) if k == n - 1 && xv >= xn => {
                    if hi_extrap == 1 {
                        (py(n - 1)?, 0.0)
                    } else {
                        let m = (py(n - 1)? - py(n - 2)?) / (px(n - 1)? - px(n - 2)?);
                        (py(n - 1)? + (xv - xn) * m, m)
                    }
                }
                Some(k) => {
                    let (xa, xb, ya, yb) = (px(k)?, px(k + 1)?, py(k)?, py(k + 1)?);
                    if interp == 2 {
                        // `D`: closest point. A step function — flat between knots, so the
                        // derivative is zero everywhere it is defined. Newton sees a locally
                        // constant contribution, which is the honest linearization of a lookup
                        // that genuinely does not move.
                        let nearer = if (xv - xa) <= (xb - xv) { ya } else { yb };
                        (nearer, 0.0)
                    } else {
                        let m = (yb - ya) / (xb - xa);
                        (ya + (xv - xa) * m, m)
                    }
                }
            };
            x.chain_public(value, slope)
        }
        Builtin::Mfactor => Dual::constant(ctx.module.multiplicity()),
        // `@(final_step)`'s, likewise. `true` in every static analysis, and in transient only at
        // the last accepted timepoint — which the driver has to solve twice to know.
        Builtin::FinalStep => Dual::constant(if ctx.analysis.is_final_step { 1.0 } else { 0.0 }),
        // `slew(value, pos_rate, neg_rate)` (LRM §4.5.6) — a rate limiter over the *committed*
        // history. `y = clamp(value, y_prev − |neg|·Δt, y_prev + pos·Δt)`.
        //
        // The gradient is the correct piecewise one: while tracking, the output *is* the input
        // and carries its gradient; while rate-limited, the output is pinned to a line through
        // history and is momentarily independent of `x`, so the gradient is zero. Getting this
        // wrong in either direction would give Newton a Jacobian inconsistent with the residual
        // it is solving.
        Builtin::Slew => {
            let value = arg(0)?;
            let (pos, neg) = (arg(1)?.value.abs(), arg(2)?.value.abs());
            crate::counters::ctx_map_lookup();
            let Some(&(_, base)) = ctx.state_slots.get(&expr_id) else {
                return Err(unsupported("slew call site has no state slot allocated"));
            };
            // A static solve, and the first transient point, settle immediately — the
            // LRM-correct steady state and the answer the old const-fold produced.
            let y = if ctx.analysis.is_initial_step {
                value.clone()
            } else {
                let dt = (ctx.analysis.time - ctx.state_get(base, 0)).max(0.0);
                let y_prev = ctx.state_get(base, 1);
                let (lo, hi) = (y_prev - neg * dt, y_prev + pos * dt);
                if value.value > hi {
                    Dual::constant(hi)
                } else if value.value < lo {
                    Dual::constant(lo)
                } else {
                    value.clone()
                }
            };
            ctx.state_set(base, 0, ctx.analysis.time);
            ctx.state_set(base, 1, y.value);
            y
        }
        // `transition(value, delay, rise_time, fall_time)` (LRM §4.5.5) — ramps toward a new
        // target over `rise`/`fall`, after `delay`.
        //
        // **This is an approximation of the LRM's event-scheduled semantics, and the difference
        // is worth stating.** A conforming simulator schedules exact breakpoints at the ramp's
        // corners so the waveform's kinks land on solved timepoints. Here the ramp is advanced
        // by whatever step the LTE controller chose, and the model asks (via `bound_step`, at
        // the call site in `crate::GeneratedModel::load`) for steps small enough to resolve it.
        // The shape is right and the endpoints are right; the corners are rounded by at most one
        // timestep.
        Builtin::Transition => {
            let value = arg(0)?;
            let (delay, rise, fall) = (arg(1)?.value, arg(2)?.value.abs(), arg(3)?.value.abs());
            crate::counters::ctx_map_lookup();
            let Some(&(_, base)) = ctx.state_slots.get(&expr_id) else {
                return Err(unsupported(
                    "transition call site has no state slot allocated",
                ));
            };
            if ctx.analysis.is_initial_step {
                ctx.state_set(base, 0, ctx.analysis.time);
                ctx.state_set(base, 1, value.value);
                ctx.state_set(base, 2, value.value);
                ctx.state_set(base, 3, 0.0);
                ctx.state_set(base, 4, ctx.analysis.time);
                return Ok(value);
            }

            let t = ctx.analysis.time;
            let (t_prev, y_prev) = (ctx.state_get(base, 0), ctx.state_get(base, 1));
            let (mut target, mut rate, mut t_start) = (
                ctx.state_get(base, 2),
                ctx.state_get(base, 3),
                ctx.state_get(base, 4),
            );

            // A changed input starts a new transition: latch the target, the rate implied by
            // this step's full amplitude, and when it may begin.
            if value.value != target {
                let mut span = if value.value >= y_prev { rise } else { fall };
                // LRM 4.5.8: a rise/fall time that is absent *or zero* means the
                // `default_transition` value (elaboration substitutes that when a directive is
                // in force), and with no directive "a negligible, but non-zero, transition time
                // is used" — deliberately, because "forcing a zero-duration transition is
                // undesirable because it could cause convergence problems". Before 2026-09-12
                // this was an instant jump (`rate = ∞`), and a `transition` of a threshold
                // comparison underflowed the timestep at the crossing. "Negligible" is scaled
                // to the deck's own step request: a thousandth of `.tran`'s tstep, which the
                // integrator resolves (~8 points per ramp via `bound_step`) at every time scale.
                if span == 0.0 && ctx.analysis.tstep > 0.0 {
                    span = ctx.analysis.tstep * 1e-3;
                }
                target = value.value;
                t_start = t + delay;
                rate = if span > 0.0 {
                    (target - y_prev).abs() / span
                } else {
                    f64::INFINITY // no time axis at all (not transient): an instant jump
                };
            }

            let y = if t < t_start {
                y_prev // still inside `delay` — hold
            } else {
                let dt = (t - t_prev.max(t_start)).max(0.0);
                let remaining = target - y_prev;
                let step = (rate * dt).min(remaining.abs());
                y_prev + step * remaining.signum()
            };

            ctx.state_set(base, 0, t);
            ctx.state_set(base, 1, y);
            ctx.state_set(base, 2, target);
            ctx.state_set(base, 3, rate);
            ctx.state_set(base, 4, t_start);
            // Zero gradient: the output is pinned to history and a latched target, not to `x`
            // at this instant. Once it reaches the target it stays there until the input
            // changes again, at which point the *next* evaluation re-latches.
            Dual::constant(y)
        }
        // `ddt` evaluated as an ordinary sub-expression, rather than pulled out as a top-level
        // contribution term by `crate::lower`.
        //
        // The gradients are exact and method-independent: `into_ddt` moves the argument's
        // instantaneous sensitivity into the charge channel, where the consumer supplies the
        // `d/dt` (the integrator's companion coefficient, or `jω` in AC).
        //
        // The **primal** is the part that cannot be exact, because `dq/dt` is not an algebraic
        // function of the current unknowns. It is reconstructed from the last accepted
        // timepoint's committed state and the discretization the solver is actually running
        // (`AnalysisCtx::ddt_coeff`/`ddt_prev_rate_weight`):
        //
        // ```text
        // dq/dt = coeff*(q - q_prev) - prev_rate_weight * dq/dt|_prev
        // ```
        //
        // In DC, AC and noise both coefficients are `0.0`, so this is exactly `0.0` — the
        // correct operating-point charge rate, not a fallback. On a transient run's first
        // timepoint `is_initial_step` makes it `0.0` too, and the site seeds its own history.
        //
        // The history is read from the *committed* buffer, so the value is the same for every
        // Newton iteration at this timepoint and identical across a rejected step's retries —
        // which is what keeps `load` a pure function of `(x, ctx, committed-state)`.
        Builtin::Ddt => {
            let q = arg(0)?;
            crate::counters::ctx_map_lookup();
            let Some(&(_, base)) = ctx.state_slots.get(&expr_id) else {
                return Err(unsupported(
                    "ddt call site has no state slots allocated (internal codegen error)",
                ));
            };
            let q_prev = ctx.state_get(base, 0);
            let rate = if ctx.analysis.is_initial_step {
                0.0
            } else {
                let rate_prev = ctx.state_get(base, 1);
                let q_prev2 = ctx.state_get(base, 2);
                // The third term is Gear/BDF2's: a weight on the charge two accepted steps
                // back (§6 change, 2026-09-01). It is `0.0` under every other method, which
                // reduces this to exactly the two-term recursion that predates it — so a
                // model compiled today evaluates bit-identically under backward Euler and
                // trapezoidal to one compiled before the field existed.
                ctx.analysis.ddt_coeff * (q.value - q_prev)
                    - ctx.analysis.ddt_prev_rate_weight * rate_prev
                    + ctx.analysis.ddt_prev2_weight * (q_prev - q_prev2)
            };
            ctx.state_set(base, 0, q.value);
            ctx.state_set(base, 1, rate);
            // Shift the charge history: what was `q_prev` on entry becomes `q_prev2` for the
            // next accepted step. Written unconditionally so the history is already correct
            // whenever a run switches to Gear (it starts on backward Euler by design).
            ctx.state_set(base, 2, q_prev);
            q.into_ddt(rate)?
        }
        // `idt` never reaches here — `eval`'s own `Expr::Call` match intercepts it before this
        // function is even called (see `eval`'s `Builtin::Idt` arm).
        Builtin::Idt => {
            return Err(unsupported(
                "idt must be lowered to its own accumulator unknown, not evaluated here \
                 (internal codegen error)",
            ))
        }
        // LRM §4.5.13: a noise function's *value* is zero in every analysis except noise. This
        // is the arm that makes that true — a model may declare its noise inline in the same
        // `<+` that carries its DC behavior without perturbing any DC/transient/AC answer.
        //
        // A gradient of zero is right as well as convenient: the noise source is an independent
        // stochastic quantity, not a function of the solution vector, so it contributes nothing
        // to the Jacobian either.
        //
        // Reaching here at all means the call was *not* pulled into the noise channel by
        // `lower::noise_term_shape` (which only recognizes a bare top-level term), so the
        // source would be silently dropped. `GeneratedModel::validate` rejects that case up
        // front rather than letting it evaluate quietly to zero here.
        Builtin::WhiteNoise
        | Builtin::FlickerNoise
        | Builtin::NoiseTable
        | Builtin::NoiseTableLog => Dual::constant(0.0),
    })
}

/// Call a user-defined analog function: bind `args` into `func`'s own argument variables, run
/// its body, and return the final binding of its `ret` variable.
///
/// Functions are pure and non-recursive (`va_ir::Function`'s doc comment) and forbid `<+`
/// contributions in their body (an LRM rule; [`exec_stmt`] enforces it), so this needs nothing
/// like `crate::GeneratedModel::stamp`/branch-current bookkeeping — just expression evaluation
/// and the variable environment `ctx` already carries. A function's own arguments/locals/return
/// variable are ordinary globally-unique `VarId`s in `ctx.module.vars` (not a separate stack
/// frame), so nested or repeated calls never alias each other's bindings.
///
/// An `output`/`inout` argument (`func.arg_dirs`) is handled specially — real compact models use
/// this for a function that computes several results at once (`mvsg_cmc_*.va`'s `calc_iq`:
/// `output idsout,qgsout,...; input vgsin,vdsin,...;`, called as
/// `idsrs = calc_iq(idsrs, qgsrs, qgdrs, ..., vgsrs, vdsrs, ...);` — only `idsrs` is bound by the
/// outer assignment; the rest are pure write-only results, never read again by name anywhere in
/// the corpus files surveyed). `Input` binds the caller's evaluated actual argument in as usual;
/// `Output` binds *nothing* in (the parameter starts genuinely unassigned, same as any other
/// local variable would, so the function reading it before writing is correctly rejected, not
/// silently defaulted); `Inout` does both. After the body runs, every `Output`/`Inout` argument's
/// *final* binding is written back into the caller's own variable — which the LRM restricts an
/// output/inout actual argument to being in the first place, enforced here as a plain
/// [`Expr::Var`] check (anything else is rejected: there would be nowhere to write the result).
fn call_function(ctx: &Ctx, func: &Function, args: &[ExprId]) -> Result<Dual, CodegenError> {
    if func.args.len() != args.len() {
        return Err(unsupported(&format!(
            "function `{}` called with {} argument(s), expected {}",
            func.name,
            args.len(),
            func.args.len()
        )));
    }
    for i in 0..func.args.len() {
        let (param, arg_expr, dir) = (func.args[i], args[i], func.arg_dirs[i]);
        if dir != va_ir::ArgDir::Input && !matches!(ctx.module.expr(arg_expr), Expr::Var(_)) {
            return Err(unsupported(&format!(
                "function `{}`'s output/inout argument #{i} must be a plain variable",
                func.name
            )));
        }
        if dir != va_ir::ArgDir::Output {
            let d = eval(ctx, arg_expr)?;
            ctx.set_var(param, d);
        }
    }
    exec_stmts(ctx, &func.body)?;
    let ret = ctx.get_var(func.ret)?;
    for i in 0..func.args.len() {
        let (param, arg_expr, dir) = (func.args[i], args[i], func.arg_dirs[i]);
        if dir != va_ir::ArgDir::Input {
            let Expr::Var(caller_var) = ctx.module.expr(arg_expr) else {
                unreachable!("checked above before the body ran");
            };
            let final_val = ctx.get_var(param)?;
            ctx.set_var(*caller_var, final_val);
        }
    }
    Ok(ret)
}

fn exec_stmts(ctx: &Ctx, stmts: &[Stmt]) -> Result<(), CodegenError> {
    for stmt in stmts {
        exec_stmt(ctx, stmt)?;
    }
    Ok(())
}

/// Execute one statement of a function body. Mirrors `crate::GeneratedModel::run`'s and
/// `crate::GeneratedModel::validate_stmts`'s split for `if`/`case`/loops (`ctx.validating`
/// picks which), for the exact same soundness reason: eager validation must not miss an
/// unsupported construct hiding in an arm/iteration a particular call doesn't happen to take.
fn exec_stmt(ctx: &Ctx, stmt: &Stmt) -> Result<(), CodegenError> {
    match stmt {
        Stmt::Assign { lhs, rhs } => {
            let d = eval(ctx, *rhs)?;
            ctx.set_var(*lhs, d);
            Ok(())
        }
        Stmt::Block(body) => exec_stmts(ctx, body),
        // `bound_step` inside an analog function body is rejected rather than ignored. An
        // analog function is pure — it computes a value from its arguments — and this statement
        // is a request to the simulator, which is a side effect a function has no channel for:
        // `call_function` returns a `Dual`, not a stamp sink. Accepting it silently would
        // discard a timestep bound the model author believes is in force.
        Stmt::BoundStep(_) => Err(unsupported(
            "bound_step is not allowed inside an analog function body (a function is pure; \
             write it in the analog block instead)",
        )),
        Stmt::If { cond, then_, else_ } => {
            if ctx.validating {
                eval(ctx, *cond)?;
                exec_stmts(ctx, then_)?;
                exec_stmts(ctx, else_)
            } else {
                let taken = if eval(ctx, *cond)?.value != 0.0 {
                    then_
                } else {
                    else_
                };
                exec_stmts(ctx, taken)
            }
        }
        Stmt::Case {
            selector,
            arms,
            default,
        } => {
            if ctx.validating {
                eval(ctx, *selector)?;
                for arm in arms {
                    for &label in &arm.labels {
                        eval(ctx, label)?;
                    }
                    exec_stmts(ctx, &arm.body)?;
                }
                exec_stmts(ctx, default)
            } else {
                let sel = eval(ctx, *selector)?;
                let mut taken = default;
                'arms: for arm in arms {
                    for &label in &arm.labels {
                        if eval(ctx, label)?.value == sel.value {
                            taken = &arm.body;
                            break 'arms;
                        }
                    }
                }
                exec_stmts(ctx, taken)
            }
        }
        Stmt::While { cond, body } => {
            if ctx.validating {
                eval(ctx, *cond)?;
                return exec_stmts(ctx, body);
            }
            let mut iters = 0usize;
            while eval(ctx, *cond)?.value != 0.0 {
                exec_stmts(ctx, body)?;
                iters += 1;
                if iters > crate::MAX_LOOP_ITERATIONS {
                    return Err(loop_iteration_cap_exceeded());
                }
            }
            Ok(())
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            if ctx.validating {
                exec_stmt(ctx, init)?;
                eval(ctx, *cond)?;
                exec_stmts(ctx, body)?;
                return exec_stmt(ctx, step);
            }
            exec_stmt(ctx, init)?;
            let mut iters = 0usize;
            while eval(ctx, *cond)?.value != 0.0 {
                exec_stmts(ctx, body)?;
                exec_stmt(ctx, step)?;
                iters += 1;
                if iters > crate::MAX_LOOP_ITERATIONS {
                    return Err(loop_iteration_cap_exceeded());
                }
            }
            Ok(())
        }
        Stmt::Repeat { count, body } => {
            if ctx.validating {
                eval(ctx, *count)?;
                return exec_stmts(ctx, body);
            }
            let n = eval(ctx, *count)?.value;
            if n > crate::MAX_LOOP_ITERATIONS as f64 {
                return Err(loop_iteration_cap_exceeded());
            }
            for _ in 0..(n.round().max(0.0) as usize) {
                exec_stmts(ctx, body)?;
            }
            Ok(())
        }
        Stmt::Contribute { .. } => Err(unsupported(
            "a `<+` contribution is not allowed inside an analog function body",
        )),
    }
}

fn loop_iteration_cap_exceeded() -> CodegenError {
    CodegenError::Unsupported(format!(
        "a loop inside a function did not terminate within {} iterations",
        crate::MAX_LOOP_ITERATIONS
    ))
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
        assert_eq!(f.grad_vec(2), vec![5.0, 3.0]);
    }

    /// `pow` with a constant exponent must use `v·u^(v-1)`, which is finite at `u = 0` for
    /// `v > 1`, rather than the logarithmic form `u^v·v·u'/u`, which is `0·∞` there.
    ///
    /// Newton's first iterate reads every probe as `0.0`, so `u = 0` is not an edge case — it
    /// is where every DC solve starts. With the logarithmic rule, BSIM-BULK 107 and PSP103
    /// both produced a NaN Jacobian entry on iteration 1 and the solve was abandoned before
    /// the unknowns had moved at all.
    #[test]
    fn pow_with_a_constant_exponent_is_differentiable_at_zero() {
        for (v, expect) in [(2.0, 0.0), (1.5, 0.0), (3.0, 0.0)] {
            let u = Dual::variable(0.0, 0, 1);
            let f = u.powf(&Dual::constant(v));
            assert_eq!(f.value, 0.0, "value of 0^{v}");
            assert!(
                f.grad_at(0).is_finite(),
                "d/du u^{v} at 0 is {}",
                f.grad_at(0)
            );
            assert_eq!(f.grad_at(0), expect, "d/du u^{v} at 0");
        }
        // v < 1 keeps the genuine singularity: d/du √u really is unbounded at 0. The rule must
        // report that rather than hide it behind a NaN.
        let f = Dual::variable(0.0, 0, 1).powf(&Dual::constant(0.5));
        assert!(
            f.grad_at(0).is_infinite(),
            "d/du √u at 0 must be ±inf, got {}",
            f.grad_at(0)
        );
    }

    /// `pow(x, k)` over a base that has gone negative must differentiate like the product it
    /// is: `d/dx x² = 2x` is finite at `x = -0.5`, but the logarithmic rule routes through
    /// `ln(-0.5)` and returns NaN.
    #[test]
    fn pow_with_an_integer_exponent_differentiates_a_negative_base() {
        let x = -0.5_f64;
        let f = Dual::variable(x, 0, 1).powf(&Dual::constant(2.0));
        assert!((f.value - 0.25).abs() < 1e-15, "value {}", f.value);
        assert!(
            (f.grad_at(0) - 2.0 * x).abs() < 1e-12,
            "grad {}",
            f.grad_at(0)
        );
        // And it agrees with writing the same thing as `x*x`, which is the invariant that
        // makes the two spellings interchangeable in a model.
        let g = Dual::variable(x, 0, 1).mul(&Dual::variable(x, 0, 1));
        assert_eq!(f.grad_at(0), g.grad_at(0));
    }

    /// A variable exponent still differentiates by the logarithmic rule — the closed form
    /// alone would drop the `u^v·ln u` term and silently return a wrong derivative.
    #[test]
    fn pow_with_a_variable_exponent_keeps_the_logarithmic_term() {
        // f = u^v at (2, 3), both variable: ∂f/∂u = 3·2² = 12, ∂f/∂v = 8·ln 2.
        let u = Dual::variable(2.0, 0, 2);
        let v = Dual::variable(3.0, 1, 2);
        let f = u.powf(&v);
        assert!((f.value - 8.0).abs() < 1e-12);
        assert!((f.grad_at(0) - 12.0).abs() < 1e-10, "d/du {}", f.grad_at(0));
        let expect = 8.0 * 2.0_f64.ln();
        assert!(
            (f.grad_at(1) - expect).abs() < 1e-10,
            "d/dv {}",
            f.grad_at(1)
        );
    }

    /// A zero charge channel must not become NaN and be mistaken for a *second* time
    /// derivative.
    ///
    /// `Dual::carries_charge` is what refuses `ddt` of something that already carries charge,
    /// and it asked "is any partial non-zero" — of a channel that was a dense vector of zeros.
    /// Multiply that by an infinite coefficient and every entry becomes NaN, and `NaN != 0.0`
    /// is `true`, so a value carrying no charge at all reported that it did. That is how
    /// L-UTSOI 102's NQS variant was told it had written a second time derivative it had not
    /// written (corpus 199 -> 200/224 when `Grad::Zero` removed the NaN). The structural rule
    /// that refuses a genuine `ddt(ddt(x))` is in `lower`, and is unaffected.
    #[test]
    fn an_infinite_coefficient_does_not_fake_a_charge_channel() {
        let carries_none = Dual::constant(0.0);
        assert!(!carries_none.carries_charge());
        assert!(!carries_none.scale(f64::INFINITY).carries_charge());
        assert!(!carries_none.scale(f64::NEG_INFINITY).carries_charge());
        // …and a value that genuinely carries charge still says so.
        let q = Dual::variable(1.0, 0, 2).into_ddt(0.0).expect("first ddt");
        assert!(q.carries_charge());
        assert!(q.into_ddt(0.0).is_err(), "a second ddt is still refused");
    }

    /// A singular unary derivative must not reach a channel the operand does not depend on.
    ///
    /// The BSIM4 case: `T11 = sqrt(jtweff / weffCJ) + 1.0` is built entirely from *parameters*,
    /// so every gradient channel of its operand is exactly `0.0` and the statement cannot
    /// affect the Jacobian at all. With `0.0` multiplied by `sqrt`'s `+inf` slope at zero, it
    /// acquired an all-NaN gradient that propagated into the drain node's row and ended the
    /// operating-point solve on the first iteration.
    #[test]
    fn a_singular_slope_does_not_poison_an_independent_channel() {
        // sqrt(0) where the operand is a constant: two channels, both structurally zero.
        let f = Dual::constant(0.0).sqrt();
        assert_eq!(f.value, 0.0);
        assert_eq!(
            f.grad_vec(2),
            vec![0.0, 0.0],
            "a constant's gradient stays zero"
        );

        // ln(0) likewise: value is -inf, but a channel the operand ignores contributes 0.
        let g = Dual::constant(0.0).ln();
        assert!(g.value.is_infinite());
        assert_eq!(g.grad_vec(2), vec![0.0, 0.0]);

        // The channel the operand *does* depend on still reports the true singularity.
        let h = Dual::variable(0.0, 0, 2).sqrt();
        assert!(h.grad_at(0).is_infinite(), "d/dx √x at 0 is unbounded");
        assert_eq!(h.grad_at(1), 0.0, "the untouched channel stays zero");
    }

    #[test]
    fn exp_chain_rule() {
        // f = exp(2*x) at x=0.5: value e, grad 2e.
        let x = Dual::variable(0.5, 0, 1);
        let two = Dual::constant(2.0);
        let f = two.mul(&x).exp();
        let e = 1.0_f64.exp();
        assert!((f.value - e).abs() < 1e-12);
        assert!((f.grad_at(0) - 2.0 * e).abs() < 1e-12);
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
            let analytic = dfn(&Dual::variable(*x0, 0, 1)).grad_at(0);
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
            vt: crate::VT,
            temp: crate::TEMP,
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
        };
        let d = eval(&ctx, vt).unwrap();
        assert!((d.value - crate::VT).abs() < 1e-12);
        assert!(!d.depends_on_unknowns());
    }

    /// Build a one-node module whose only expression is `$table_model(V(n0), …)` over the
    /// given `(x, y)` pairs and control code, and evaluate it at `xv`.
    ///
    /// Mirrors what elaboration produces: the lookup expression, the packed control code, then
    /// the sorted table flattened into `Const` arguments (`va_ir::Builtin::TableModel`).
    #[cfg(test)]
    fn eval_table(pairs: &[(f64, f64)], code: f64, xv: f64) -> Dual {
        use va_ir::{Access, AccessKind, Branch, Builtin, Expr, Module, NodeDecl, NodeId};

        let mut m = Module::new("tbl");
        for name in ["n0", "gnd"] {
            m.nodes.push(NodeDecl {
                name: name.into(),
                discipline: va_ir::Discipline::Electrical,
                abstol: None,
                access: None,
                units: None,
            });
        }
        m.branches.push(Branch {
            p: NodeId(0),
            n: NodeId(1),
        });
        let probe = m.push_expr(Expr::Probe(Access {
            kind: AccessKind::Potential,
            branch: va_ir::BranchId(0),
        }));
        let mut args = vec![probe, m.push_expr(Expr::Const(code))];
        for &(px, py) in pairs {
            args.push(m.push_expr(Expr::Const(px)));
            args.push(m.push_expr(Expr::Const(py)));
        }
        let call = m.push_expr(Expr::Call(Builtin::TableModel, args));

        let x = [xv];
        let terminals = [0usize, usize::MAX];
        let ctx = Ctx {
            module: &m,
            params: &[],
            x: &x,
            terminals: &terminals,
            vt: crate::VT,
            temp: crate::TEMP,
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
        };
        eval(&ctx, call).expect("table_model evaluates")
    }

    /// The table's own points come back exactly, and between them the value is the straight
    /// line joining its neighbours.
    #[test]
    fn table_model_interpolates_linearly_between_its_points() {
        let t = [(0.0, 10.0), (1.0, 20.0), (3.0, 0.0)];
        for &(x, y) in &t {
            let got = eval_table(&t, 122.0, x).value;
            assert!((got - y).abs() < 1e-12, "at knot x={x}: {got} != {y}");
        }
        // Midpoint of each segment.
        assert!((eval_table(&t, 122.0, 0.5).value - 15.0).abs() < 1e-12);
        assert!((eval_table(&t, 122.0, 2.0).value - 10.0).abs() < 1e-12);
    }

    /// §5: the lookup's derivative must agree with a central finite difference.
    ///
    /// This is the rule that makes a table safe to put in a circuit equation. A lookup whose
    /// Jacobian did not match its value would not error — it would converge Newton to the wrong
    /// answer, the failure mode three separate bugs took this week.
    ///
    /// Knots are avoided on purpose: the function has a kink there, so no derivative exists and
    /// a finite difference straddling one is meaningless. That is a property of piecewise-linear
    /// interpolation, not of this implementation, and it is why the one-sided convention is
    /// written down in the evaluator.
    #[test]
    fn table_model_derivative_matches_finite_difference() {
        let t = [(0.0, 10.0), (1.0, 20.0), (3.0, 0.0), (4.0, -5.0)];
        let h = 1e-6;
        for &x in &[0.3, 0.75, 1.5, 2.6, 3.4] {
            let d = eval_table(&t, 122.0, x);
            let fd = (eval_table(&t, 122.0, x + h).value - eval_table(&t, 122.0, x - h).value)
                / (2.0 * h);
            let scale = fd.abs().max(d.grad_at(0).abs()).max(1e-9);
            assert!(
                (d.grad_at(0) - fd).abs() / scale < 1e-6,
                "at x={x}: analytic {} vs FD {fd}",
                d.grad_at(0)
            );
        }
    }

    /// Extrapolation follows the control string at each end independently, which is the whole
    /// reason the LRM lets you give two characters.
    #[test]
    fn table_model_extrapolates_per_end_as_the_control_says() {
        let t = [(0.0, 10.0), (1.0, 20.0)]; // slope +10
                                            // `"1CL"` -> constant below, linear above: 111 would be C/C, 122 L/L, 112 C/L.
        let cl = 112.0;
        assert!(
            (eval_table(&t, cl, -5.0).value - 10.0).abs() < 1e-12,
            "C below"
        );
        assert!(
            (eval_table(&t, cl, -5.0).grad_at(0)).abs() < 1e-12,
            "C is flat"
        );
        assert!(
            (eval_table(&t, cl, 3.0).value - 40.0).abs() < 1e-12,
            "L above"
        );
        assert!(
            (eval_table(&t, cl, 3.0).grad_at(0) - 10.0).abs() < 1e-12,
            "L keeps the slope"
        );
        // And the mirror image, `"1LC"` = 121.
        let lc = 121.0;
        assert!(
            (eval_table(&t, lc, -5.0).value - (-40.0)).abs() < 1e-12,
            "L below"
        );
        assert!(
            (eval_table(&t, lc, 3.0).value - 20.0).abs() < 1e-12,
            "C above"
        );
    }

    /// `D` is closest-point lookup: a step function, flat between knots, so its derivative is
    /// zero wherever one exists. Newton sees a locally constant contribution, which is the
    /// honest linearization of a lookup that genuinely does not move.
    #[test]
    fn table_model_discrete_lookup_is_a_step_with_no_slope() {
        let t = [(0.0, 10.0), (1.0, 20.0)];
        let d = 222.0; // interp 2 = discrete, linear extrapolation both ends
        assert_eq!(eval_table(&t, d, 0.2).value, 10.0, "nearer the low point");
        assert_eq!(eval_table(&t, d, 0.8).value, 20.0, "nearer the high point");
        assert_eq!(eval_table(&t, d, 0.2).grad_at(0), 0.0);
        assert_eq!(eval_table(&t, d, 0.8).grad_at(0), 0.0);
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
            abstol: None,
            access: None,
            units: None,
        });
        m.nodes.push(NodeDecl {
            name: "gnd".into(),
            discipline: va_ir::Discipline::Thermal,
            abstol: None,
            access: None,
            units: None,
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

        let (vt_ref, temp_ref) = (crate::VT, crate::TEMP);
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
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
        };
        let d = eval(&ctx, vt).unwrap();
        assert!((d.value - k_over_q * 350.0).abs() < 1e-12);
        // d($vt(T))/dV(t) = k/q; the ground slot is out of range so contributes no gradient.
        assert!((d.grad_at(0) - k_over_q).abs() < 1e-12);

        // Cross-check against a central finite difference (§5).
        let h = 1e-3;
        let f = |t: f64| k_over_q * t;
        let fd = (f(350.0 + h) - f(350.0 - h)) / (2.0 * h);
        assert!((d.grad_at(0) - fd).abs() < 1e-9);

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
                abstol: None,
                access: None,
                units: None,
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
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
        };

        assert_eq!(eval(&ctx, vin).unwrap().value, 2.0);
        assert_eq!(eval(&ctx, one).unwrap().value, 1.0);
        assert_eq!(eval(&ctx, minusone).unwrap().value, -1.0);
        assert_eq!(eval(&ctx, zero).unwrap().value, 0.0);
        // ddx's result is a constant as far as further differentiation is concerned.
        assert!(!eval(&ctx, one).unwrap().depends_on_unknowns());
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
            abstol: None,
            access: None,
            units: None,
        });
        m.nodes.push(NodeDecl {
            name: "c".into(),
            discipline: Discipline::Electrical,
            abstol: None,
            access: None,
            units: None,
        });
        let (a, c) = (NodeId(0), NodeId(1));
        m.branches.push(Branch { p: a, n: c }); // BranchId(0): V(a,c)
        m.branches.push(Branch { p: a, n: c }); // BranchId(1): V(a) -- c doubles as reference

        let is = 1e-14_f64;
        let vt = crate::VT;
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
            temp: crate::TEMP,
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
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
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
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
            analysis: va_abi::ANALYSIS_DC,
            events_fired: &[],
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(Vec::new()),
            static_vars: &[],
            hoistable: &[],
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
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
        assert!((hp.grad_at(0) - 0.6).abs() < 1e-12);
        assert!((hp.grad_at(1) - 0.8).abs() < 1e-12);

        // atan2(y, x): d/dy = x/(x²+y²), d/dx = -y/(x²+y²).
        let denom = 3.0_f64 * 3.0 + 4.0 * 4.0;
        let at = y.atan2(&x);
        assert!((at.grad_at(1) - 3.0 / denom).abs() < 1e-12);
        assert!((at.grad_at(0) + 4.0 / denom).abs() < 1e-12);

        // min/max select the active argument's value and gradient.
        let mn = x.min(&y);
        assert_eq!((mn.value, mn.grad_at(0), mn.grad_at(1)), (3.0, 1.0, 0.0));
        let mx = x.max(&y);
        assert_eq!((mx.value, mx.grad_at(0), mx.grad_at(1)), (4.0, 0.0, 1.0));
    }

    #[test]
    fn div_matches_finite_difference() {
        // f = 1 / x at x=4: analytic -1/16.
        let x = Dual::variable(4.0, 0, 1);
        let one = Dual::constant(1.0);
        let f = one.div(&x);
        let h = 1e-6;
        let fd = (1.0 / (4.0 + h) - 1.0 / (4.0 - h)) / (2.0 * h);
        assert!(
            (f.grad_at(0) - fd).abs() < 1e-7,
            "{} vs {}",
            f.grad_at(0),
            fd
        );
    }
}

#[cfg(test)]
mod laplace_tests {
    use super::{laplace_at, Cx};

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    /// The DC gains the elaboration-time fold used to compute, now produced by evaluating the
    /// transfer function at `s = 0`. These are the *same numbers* the folded implementation
    /// gave, which is why un-folding moved no existing result — the fold was a special case of
    /// the general rule, not a different rule.
    #[test]
    fn dc_gain_reproduces_the_folds_it_replaced() {
        // laplace_nd, num[0]/den[0] = 2/2.
        let h = laplace_at(0.0, &[2.0, 0.0], false, &[2.0, 1.0], false);
        assert!(close(h.0, 1.0, 1e-12) && h.1.abs() < 1e-12, "{h:?}");

        // LRM §4.5.11.5's worked example: a real zero at -1 over a conjugate pole pair at -1±j.
        // Every non-origin root's factor is 1 at s=0, so H(0) = 1.
        let h = laplace_at(0.0, &[-1.0, 0.0], true, &[-1.0, -1.0, -1.0, 1.0], true);
        assert!(close(h.0, 1.0, 1e-12) && h.1.abs() < 1e-12, "{h:?}");

        // `external/angelov*.va`'s idiom: constant numerator over two real nonzero poles.
        let h = laplace_at(0.0, &[1.0], false, &[6.28e9, 0.0, -6.28e9, 0.0], true);
        assert!(close(h.0, 1.0, 1e-12), "{h:?}");

        // laplace_zd: a real non-origin zero over den[0] = 4.
        let h = laplace_at(0.0, &[1.0, 0.0], true, &[4.0, 1.0], false);
        assert!(close(h.0, 0.25, 1e-12), "{h:?}");

        // A zero exactly at the origin contributes a factor of `s`, which is 0 at DC.
        let h = laplace_at(0.0, &[0.0, 0.0], true, &[1.0, 0.0], true);
        assert_eq!(h, Cx(0.0, 0.0));
    }

    /// The property the DC fold could not express, and the reason Tier C exists: a one-pole
    /// lowpass `1/(1 + sτ)` must roll off. Checked against the closed form at three decades.
    #[test]
    fn a_one_pole_lowpass_rolls_off() {
        let tau = 1e-3;
        for &f in &[1.0, 159.154_943_091_895_35, 1e4] {
            let w = 2.0 * std::f64::consts::PI * f;
            let h = laplace_at(w, &[1.0], false, &[1.0, tau], false);
            // 1/(1 + jωτ)
            let d = 1.0 + (w * tau) * (w * tau);
            assert!(close(h.0, 1.0 / d, 1e-12), "re at {f}: {h:?}");
            assert!(close(h.1, -w * tau / d, 1e-12), "im at {f}: {h:?}");
        }
        // At the corner (ω τ = 1) the magnitude is exactly 1/√2 — the -3 dB point.
        let w = 1.0 / tau;
        let h = laplace_at(w, &[1.0], false, &[1.0, tau], false);
        let mag = (h.0 * h.0 + h.1 * h.1).sqrt();
        assert!(close(mag, std::f64::consts::FRAC_1_SQRT_2, 1e-12), "{mag}");
    }

    /// The coefficient and root forms are two spellings of one filter, so they must agree
    /// numerically at every frequency — a cross-check neither could give alone.
    #[test]
    fn coefficient_and_root_forms_agree() {
        // 1/(1 + s/p) with p = -1000 as a pole array, vs {1, 1/1000} as coefficients... note
        // the root form is (1 - s/ρ), so a pole at ρ = -1000 gives (1 + s/1000).
        let tau = 1e-3;
        for &f in &[10.0, 1e3, 1e5] {
            let w = 2.0 * std::f64::consts::PI * f;
            let by_coeffs = laplace_at(w, &[1.0], false, &[1.0, tau], false);
            let by_roots = laplace_at(w, &[1.0], false, &[-1.0 / tau, 0.0], true);
            assert!(close(by_coeffs.0, by_roots.0, 1e-12), "re at {f}");
            assert!(close(by_coeffs.1, by_roots.1, 1e-12), "im at {f}");
        }
    }

    /// `ddt` is the special case `H(s) = s`, and the general evaluator must reproduce it —
    /// purely imaginary, magnitude `ω`. That is the consistency check tying this channel to the
    /// charge channel it generalizes.
    #[test]
    fn a_differentiator_is_purely_imaginary() {
        let w = 2.0 * std::f64::consts::PI * 500.0;
        // H(s) = s, written as a zero at the origin over a unit denominator.
        let h = laplace_at(w, &[0.0, 0.0], true, &[1.0], false);
        assert!(h.0.abs() < 1e-9, "{h:?}");
        assert!(close(h.1, w, 1e-12), "{h:?}");
    }
}
