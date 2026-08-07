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
use va_ir::{BinOp, Builtin, Expr, ExprId, Function, Module, Stmt, UnOp, VarId};

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
    pub vars: RefCell<HashMap<u32, Dual>>,
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
        // `idt(...)`'s value is a plain read of its own accumulator unknown (see
        // `crate::lower::IdtAccumulator`'s doc comment) — never evaluated through `eval_call`'s
        // ordinary per-builtin dispatch, since its argument is never evaluated to produce this
        // call's *value* at all (only `crate::GeneratedModel::stamp_idt_accumulators` evaluates
        // it, to stamp the accumulator's own row). `expr` is this call's own id, exactly the key
        // `lower::lower` registered it under in `ctx.idt_slots`.
        Expr::Call(Builtin::Idt, _) => {
            let slot = *ctx.idt_slots.get(&expr.0).ok_or_else(|| {
                unsupported(
                    "idt accumulator not registered for this call site (internal codegen error)",
                )
            })?;
            let value = ctx.node_voltage(slot);
            let mut grad = vec![0.0; count];
            if slot < count {
                grad[slot] = 1.0;
            }
            Ok(Dual { value, grad })
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
            Ok(Dual::constant(d.grad.get(p).copied().unwrap_or(0.0), count))
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
        // The three analysis-context builtins. All are constants with respect to `x` — none is
        // a function of the solution vector — so all carry a zero gradient, and `va-codegen`'s
        // finite-difference tests confirm that rather than assuming it.
        //
        // `$abstime` is the absolute simulation time, `0.0` outside transient (the LRM-correct
        // answer for a static solve, not a placeholder).
        Builtin::Abstime => Dual::constant(ctx.analysis.time, count),
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
            Dual::constant(
                if phase_mask_active(&ctx.analysis, mask) {
                    1.0
                } else {
                    0.0
                },
                count,
            )
        }
        // `ac_stim`'s *value* is zero in every analysis including AC — it is a right-hand-side
        // excitation, not a term in `G`. `crate::lower` splits a recognized one out of its
        // contribution into `va_abi::StampSink`'s excitation channel; reaching this arm at all
        // means the call sits somewhere that split could not pull it out of, and
        // `crate::GeneratedModel::validate` rejects that rather than letting it vanish.
        Builtin::AcStim => Dual::constant(0.0, count),
        // `@(initial_step)`'s desugared condition. Pure solver knowledge, no state, no gradient.
        Builtin::InitialStep => Dual::constant(
            if ctx.analysis.is_initial_step {
                1.0
            } else {
                0.0
            },
            count,
        ),
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
                    Dual::constant(hi, count)
                } else if value.value < lo {
                    Dual::constant(lo, count)
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
                let span = if value.value >= y_prev { rise } else { fall };
                target = value.value;
                t_start = t + delay;
                rate = if span > 0.0 {
                    (target - y_prev).abs() / span
                } else {
                    f64::INFINITY // a zero rise/fall time is an instant jump, not a divide error
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
            Dual::constant(y, count)
        }
        // `idt` never reaches here — `eval`'s own `Expr::Call` match intercepts it before this
        // function is even called (see `eval`'s `Builtin::Idt` arm).
        Builtin::Ddt | Builtin::Idt => {
            return Err(unsupported(
                "ddt must appear as a top-level contribution term, not inside an expression",
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
        | Builtin::NoiseTableLog => Dual::constant(0.0, count),
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
            vt: crate::VT,
            temp: crate::TEMP,
            analysis: va_abi::ANALYSIS_DC,
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
        };
        let d = eval(&ctx, vt).unwrap();
        assert!((d.value - crate::VT).abs() < 1e-12);
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
            abstol: None,
        });
        m.nodes.push(NodeDecl {
            name: "gnd".into(),
            discipline: va_ir::Discipline::Thermal,
            abstol: None,
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
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
            branch_current_slots: HashMap::new(),
            idt_slots: HashMap::new(),
            mixed_branch_potential_used: RefCell::new(HashSet::new()),
            flow_current_totals: RefCell::new(HashMap::new()),
            validating: false,
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
                abstol: None,
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
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
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
            abstol: None,
        });
        m.nodes.push(NodeDecl {
            name: "c".into(),
            discipline: Discipline::Electrical,
            abstol: None,
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
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
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
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
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
            state_prev: &[],
            state_next: RefCell::new(Vec::new()),
            state_slots: HashMap::new(),
            bound_step: Cell::new(None),
            vars: RefCell::new(HashMap::new()),
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
