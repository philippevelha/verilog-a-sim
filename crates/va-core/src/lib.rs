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

/// Test-only model instances. An ideal voltage source is not part of the `va-abi` reference
/// zoo (those are compact device models), but it is needed to *excite* the reference devices
/// in `va-core`'s own tests — without a source every circuit solves to the trivial `x = 0`.
#[cfg(test)]
pub(crate) mod testutil {
    use va_abi::{ModelInstance, StampSink};

    /// An ideal voltage source `V(p) − V(n) = vin`, stamped with the standard MNA
    /// branch-current formulation: an extra unknown `i` (the source current) at global index
    /// `branch`, a constraint row enforcing the voltage, and the current injected into the
    /// terminals.
    pub struct VSource {
        terminals: [usize; 3], // [p, n, branch-current]
        vin: f64,
    }

    impl VSource {
        /// A source of `vin` volts from `p` to `n`, using global unknown `branch` for its
        /// current.
        pub fn new(p: usize, n: usize, branch: usize, vin: f64) -> Self {
            Self {
                terminals: [p, n, branch],
                vin,
            }
        }
    }

    impl ModelInstance for VSource {
        fn unknowns(&self) -> &[usize] {
            &self.terminals
        }

        fn load(&self, x: &[f64], sink: &mut dyn StampSink) {
            let [p, n, b] = self.terminals;
            let vp = x.get(p).copied().unwrap_or(0.0);
            let vn = x.get(n).copied().unwrap_or(0.0);
            let ib = x.get(b).copied().unwrap_or(0.0);

            // Constraint row: V(p) − V(n) − vin = 0.
            sink.residual(b, vp - vn - self.vin);
            sink.jacobian(b, p, 1.0);
            sink.jacobian(b, n, -1.0);

            // The branch current flows out of p and into n.
            sink.residual(p, ib);
            sink.residual(n, -ib);
            sink.jacobian(p, b, 1.0);
            sink.jacobian(n, b, -1.0);
        }
    }
}
