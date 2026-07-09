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

/// Test-only re-exports. The ideal voltage source used to *excite* the reference devices in
/// `va-core`'s own tests now lives in the `va-abi` reference zoo ([`va_abi::reference::VSource`]);
/// without a source every circuit solves to the trivial `x = 0`.
#[cfg(test)]
pub(crate) mod testutil {
    pub use va_abi::reference::VSource;

    /// Wraps any [`va_abi::ModelInstance`], overriding [`va_abi::ModelInstance::unknown_abstol`]
    /// for zero or more of its own local indices (`overrides`, `(local index, abstol)` pairs) —
    /// lets `mna`'s and `newton`'s tests exercise § nature-metadata wiring's per-unknown
    /// convergence tolerance without a real Verilog-A-compiled model (none of the hand-written
    /// `va-abi::reference` devices carry discipline metadata to report). A local index with no
    /// matching entry in `overrides` falls back to `inner`'s own (always `None`, for every
    /// `va-abi::reference` device) — so a multi-unknown instance like `VSource` can have some
    /// of its unknowns overridden and others left at the solver's default in one wrapper,
    /// without double-stamping `inner.load()` via two separate wrapper instances.
    pub struct AbstolOverride<'a> {
        pub inner: &'a dyn va_abi::ModelInstance,
        pub overrides: &'a [(usize, f64)],
    }

    impl va_abi::ModelInstance for AbstolOverride<'_> {
        fn unknowns(&self) -> &[usize] {
            self.inner.unknowns()
        }
        fn unknown_kind(&self, i: usize) -> va_abi::UnknownKind {
            self.inner.unknown_kind(i)
        }
        fn unknown_abstol(&self, i: usize) -> Option<f64> {
            self.overrides
                .iter()
                .find(|&&(local, _)| local == i)
                .map(|&(_, abstol)| abstol)
                .or_else(|| self.inner.unknown_abstol(i))
        }
        fn load(&self, x: &[f64], sink: &mut dyn va_abi::stamps::StampSink) {
            self.inner.load(x, sink)
        }
    }
}
