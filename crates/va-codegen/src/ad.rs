//! Forward-mode automatic differentiation over the [`va_ir`] expression arena.
//!
//! Evaluates an [`va_ir::ExprId`] to a value paired with its partial derivatives w.r.t. the
//! branch probes (the unknowns). The result feeds the Jacobian stamps.

/// A value carried with its gradient w.r.t. the active unknowns (a dual number).
#[derive(Clone, Debug)]
pub struct Dual {
    /// The primal value.
    pub value: f64,
    /// Partial derivatives, one per active unknown (same order as the instance's terminals).
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
}

/// Evaluate `expr` under forward-mode AD given the current probe duals.
///
/// Stubbed until T2; will recurse over the arena accumulating value + gradient.
pub fn eval(_module: &va_ir::Module, _expr: va_ir::ExprId, _probes: &[Dual]) -> Dual {
    todo!("T2: forward-mode AD evaluation over the IR arena")
}
