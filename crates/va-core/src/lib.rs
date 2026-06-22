//! T3 — the numerical core: MNA assembly, Newton iteration, linear solve, convergence, DC.
//!
//! `va-core` is the load-bearing crate. By the §2 invariant it depends on `va-abi`
//! (Interface β) and **nothing else**, so it can be developed and validated against the
//! hand-written reference models without waiting on the compiler half of the pipeline.
//!
//! The solver consumes a slice of [`va_abi::ModelInstance`] objects, assembles their stamps
//! into an MNA system ([`mna`]), and drives Newton ([`newton`]) with a dense linear solve
//! ([`linsolve`]) plus convergence aids ([`convergence`]). [`dc`] wires these into an
//! operating-point / sweep analysis.

#![forbid(unsafe_code)]

pub mod convergence;
pub mod dc;
pub mod linsolve;
pub mod mna;
pub mod newton;

use thiserror::Error;

/// Errors raised by the numerical core.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Newton did not converge within the iteration budget.
    #[error("Newton failed to converge after {iters} iterations (residual {residual:e})")]
    NoConvergence { iters: usize, residual: f64 },
    /// The assembled Jacobian was singular / could not be factored.
    #[error("singular matrix during linear solve")]
    Singular,
}
