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
    /// A model stamped a NaN or ±inf into the system, so the linear solve was never attempted.
    ///
    /// Distinct from [`CoreError::Singular`] on purpose (2026-09-11): a zero pivot and a
    /// `0.0/0.0` both come out of LU as `NaN`, but they call for opposite next steps — a
    /// singular matrix is a *topology* problem (a floating node, a loop of voltage sources), a
    /// non-finite stamp is an *evaluation* problem inside one model at one trial point. The
    /// commonest cause is a probe used as a divisor or a `log`/`sqrt`/`pow` argument: Newton
    /// starts from the zero vector, so on the first iteration every probe reads 0.
    #[error(
        "a model produced a non-finite (NaN or inf) {} — the equations were not singular; a contribution divides by, or takes log/sqrt/pow of, a quantity outside its domain at this trial point (on Newton's first iteration every probe reads 0.0: guard such a probe in the model, e.g. `(x > 0.0) ? x : x_default`)",
        non_finite_where(*row, *col)
    )]
    NonFinite {
        /// The system row (global unknown index) the bad value landed on.
        row: usize,
        /// `Some(column)` when the value is a Jacobian entry, `None` for the residual.
        col: Option<usize>,
    },
}

/// The "where" clause of [`CoreError::NonFinite`]'s message.
fn non_finite_where(row: usize, col: Option<usize>) -> String {
    match col {
        Some(c) => format!("Jacobian entry at row {row}, column {c}"),
        None => format!("residual at row {row}"),
    }
}

/// Row and column of the first non-finite entry in a dense row-major `n × n` system `a`
/// with right-hand side `b`, as [`CoreError::NonFinite`]; `Ok(())` when everything is finite.
///
/// The residual is scanned first so a bad residual is named as such even when the same row's
/// Jacobian entries are also bad.
///
/// # Errors
///
/// [`CoreError::NonFinite`] naming the first offending entry.
pub fn check_finite(a: &[f64], b: &[f64], n: usize) -> Result<(), CoreError> {
    if let Some(row) = b.iter().position(|v| !v.is_finite()) {
        return Err(CoreError::NonFinite { row, col: None });
    }
    if let Some(k) = a.iter().position(|v| !v.is_finite()) {
        return Err(CoreError::NonFinite {
            row: k / n,
            col: Some(k % n),
        });
    }
    Ok(())
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
    /// Wraps any [`va_abi::ModelInstance`] and claims **every** one of its unknowns is a
    /// junction potential, so a test can put a circuit back under the blanket step limiting
    /// that `unknown_is_junction` replaced (§ junction limiting). Used to demonstrate that the
    /// fix is load-bearing: the same linear circuit converges normally and fails through this
    /// wrapper.
    pub struct JunctionOverride<'a> {
        pub inner: &'a dyn va_abi::ModelInstance,
    }

    impl va_abi::ModelInstance for JunctionOverride<'_> {
        fn unknowns(&self) -> &[usize] {
            self.inner.unknowns()
        }
        fn unknown_kind(&self, i: usize) -> va_abi::UnknownKind {
            self.inner.unknown_kind(i)
        }
        fn state_len(&self) -> usize {
            self.inner.state_len()
        }
        fn unknown_is_junction(&self, _i: usize) -> bool {
            true
        }
        fn load(
            &self,
            x: &[f64],
            ctx: &va_abi::AnalysisCtx,
            state: &mut va_abi::ModelState,
            sink: &mut dyn va_abi::stamps::StampSink,
        ) {
            self.inner.load(x, ctx, state, sink)
        }
    }

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
        fn state_len(&self) -> usize {
            self.inner.state_len()
        }
        fn unknown_abstol(&self, i: usize) -> Option<f64> {
            self.overrides
                .iter()
                .find(|&&(local, _)| local == i)
                .map(|&(_, abstol)| abstol)
                .or_else(|| self.inner.unknown_abstol(i))
        }
        fn load(
            &self,
            x: &[f64],
            ctx: &va_abi::AnalysisCtx,
            state: &mut va_abi::ModelState,
            sink: &mut dyn va_abi::stamps::StampSink,
        ) {
            self.inner.load(x, ctx, state, sink)
        }
    }
}
