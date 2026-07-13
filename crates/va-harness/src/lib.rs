//! T6 — the validation harness: run the pipeline and compare to committed `golden/` outputs.
//!
//! QSPICE is the oracle (§7). The harness drives `va-cli`'s pipeline, computes the [`metrics`]
//! against golden references, and reports pass/fail against the stated tolerances. No analysis
//! result is trusted until it is green here.

#![forbid(unsafe_code)]

pub mod dc;
pub mod golden;
pub mod metrics;

use thiserror::Error;

/// Errors raised by the harness.
#[derive(Debug, Error)]
pub enum HarnessError {
    /// A golden reference file was missing or unreadable.
    #[error("missing golden reference: {0}")]
    MissingGolden(String),
    /// The series being compared had mismatched lengths / timebases.
    #[error("series length mismatch: got {got}, expected {expected}")]
    LengthMismatch { got: usize, expected: usize },
    /// A `.golden` file's contents didn't parse (§ [`golden`]).
    #[error("golden file parse error on line {line}: {message}")]
    Golden {
        /// 1-indexed line number.
        line: usize,
        /// What went wrong.
        message: String,
    },
    /// The computed and golden results describe different nodes (or the same nodes in a
    /// different order) — comparing their values at all would silently diff unrelated
    /// quantities, so this is always an error rather than an approximation.
    #[error("node order mismatch: got {got:?}, expected {expected:?}")]
    NodeOrderMismatch {
        /// The freshly-computed result's node order.
        got: Vec<String>,
        /// The golden reference's node order.
        expected: Vec<String>,
    },
    /// Running the pipeline itself (`va-cli::load`/`solve_dc`) failed — a bad netlist/model
    /// path, a parse error, or a divergent solve. Wraps the underlying `anyhow::Error`'s full
    /// chain as a string rather than depending on `anyhow` directly (§ `dc.rs`'s doc comment).
    #[error("running the pipeline failed: {0}")]
    Run(String),
}

/// Default tolerances from §7. Tune in `docs/validation.md`.
pub mod tol {
    /// DC: max relative I–V error on the operating point / sweep.
    pub const DC_REL: f64 = 1e-4;
    /// Transient: waveform RMS error after a shared-timebase resample.
    pub const TRAN_RMS: f64 = 1e-3;
}

/// Outcome of comparing one analysis against its golden reference.
#[derive(Clone, Copy, Debug)]
pub struct Verdict {
    /// The measured error metric.
    pub error: f64,
    /// The tolerance it was checked against.
    pub tol: f64,
    /// Whether `error <= tol`.
    pub passed: bool,
}

impl Verdict {
    /// Build a verdict from a measured error and tolerance.
    pub fn new(error: f64, tol: f64) -> Self {
        Self {
            error,
            tol,
            passed: error <= tol,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_passes_within_tolerance() {
        let v = Verdict::new(5e-5, tol::DC_REL);
        assert!(v.passed);
        let v = Verdict::new(5e-3, tol::DC_REL);
        assert!(!v.passed);
    }
}
