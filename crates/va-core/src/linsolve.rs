//! Dense linear solve `J · dx = -f`, backed by `faer` (pure-Rust, per §5).
//!
//! Starts dense; the project roadmap moves to a simple sparse solve once circuits grow.

use crate::CoreError;

/// Solve `a · x = b` where `a` is a dense row-major `n × n` matrix and `b` has length `n`.
/// Returns `x`.
///
/// # Errors
///
/// [`CoreError::Singular`] if `a` is singular to working precision.
pub fn solve_dense(_a: &[f64], _b: &[f64], _n: usize) -> Result<Vec<f64>, CoreError> {
    todo!("T3: LU solve via faer")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T3: 2x2 dense solve via faer matches a hand computation"]
    fn solves_2x2() {}
}
