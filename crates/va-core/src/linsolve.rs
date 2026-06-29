//! Dense linear solve `J · dx = -f`, backed by `faer` (pure-Rust, per §5).
//!
//! Starts dense; the project roadmap moves to a simple sparse solve once circuits grow.

use crate::CoreError;
use faer::prelude::*;
use faer::Mat;

/// Relative tolerance for the post-solve residual sanity check. A solve whose `‖A·x − b‖∞`
/// exceeds this (scaled by `‖b‖`) is treated as singular — this catches the near-singular
/// case partial pivoting would otherwise return as finite garbage.
const RESIDUAL_TOL: f64 = 1e-6;

/// Solve `a · x = b` where `a` is a dense row-major `n × n` matrix and `b` has length `n`.
/// Returns `x`.
///
/// Uses LU with partial pivoting. Singularity is detected two ways: a non-finite solution
/// (a zero pivot propagates `inf`/`NaN`), or a solution that fails to reproduce `b`.
///
/// # Errors
///
/// [`CoreError::Singular`] if `a` is singular to working precision.
pub fn solve_dense(a: &[f64], b: &[f64], n: usize) -> Result<Vec<f64>, CoreError> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }

    let mat = Mat::from_fn(n, n, |i, j| a[i * n + j]);
    let rhs = Mat::from_fn(n, 1, |i, _| b[i]);

    let lu = mat.partial_piv_lu();
    let sol = lu.solve(&rhs);

    let x: Vec<f64> = (0..n).map(|i| *sol.get(i, 0)).collect();
    if !x.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Singular);
    }

    // Verify A·x ≈ b; a near-singular factorization yields large residuals.
    let bmax = b.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let rmax = (0..n)
        .map(|i| {
            let ax: f64 = (0..n).map(|j| a[i * n + j] * x[j]).sum();
            (ax - b[i]).abs()
        })
        .fold(0.0_f64, f64::max);
    if rmax > RESIDUAL_TOL * (1.0 + bmax) {
        return Err(CoreError::Singular);
    }

    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_2x2() {
        // [4 3; 6 3] x = [10; 12]  ->  x = [1; 2].
        let a = [4.0, 3.0, 6.0, 3.0];
        let b = [10.0, 12.0];
        let x = solve_dense(&a, &b, 2).expect("non-singular");
        assert!((x[0] - 1.0).abs() < 1e-12, "x0 = {}", x[0]);
        assert!((x[1] - 2.0).abs() < 1e-12, "x1 = {}", x[1]);
    }

    #[test]
    fn identity_is_passthrough() {
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [7.0, -3.0];
        let x = solve_dense(&a, &b, 2).unwrap();
        assert_eq!(x, vec![7.0, -3.0]);
    }

    #[test]
    fn singular_matrix_is_rejected() {
        // Rows are linearly dependent: [1 2; 2 4].
        let a = [1.0, 2.0, 2.0, 4.0];
        let b = [1.0, 1.0];
        assert!(matches!(solve_dense(&a, &b, 2), Err(CoreError::Singular)));
    }

    #[test]
    fn empty_system_is_trivial() {
        assert_eq!(solve_dense(&[], &[], 0).unwrap(), Vec::<f64>::new());
    }
}
