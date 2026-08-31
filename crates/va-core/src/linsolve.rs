//! Linear solve `J · dx = -f`, backed by `faer` (pure-Rust, per §5).
//!
//! [`solve_dense`] is what `newton`/`dc` actually call — the production path. [`solve_sparse`]
//! is a **prototype**, added 2026-08-31 to answer the sparse-solve backlog question
//! (`docs/roadmap.md`'s T3 section, staffed since the 2026-07-04 note): not wired into any
//! solver loop yet, it exists so `cargo xtask bench-linsolve` can measure dense vs. sparse
//! head-to-head on real assembled MNA systems and report where (if anywhere) dense stops being
//! the right default. See that doc section for the measured crossover.

use crate::CoreError;
use faer::prelude::*;
use faer::sparse::linalg::solvers::{Lu as SparseLu, SymbolicLu as SparseSymbolicLu};
use faer::sparse::{SparseColMat, Triplet};
use faer::Mat;

/// Relative tolerance for the post-solve residual sanity check. A solve whose `‖A·x − b‖∞`
/// exceeds this (scaled by `‖b‖`) is treated as singular — this catches the near-singular
/// case partial pivoting would otherwise return as finite garbage.
const RESIDUAL_TOL: f64 = 1e-6;

/// Whether `x` solves `a · x = b` (dense row-major `a`, `n × n`) to within [`RESIDUAL_TOL`].
///
/// Shared by [`solve_dense`] and [`solve_sparse`] so both solvers apply *the same* singularity
/// check to the same inputs — the whole point of comparing them is that they agree, including
/// on what counts as "failed."
fn residual_ok(a: &[f64], b: &[f64], n: usize, x: &[f64]) -> bool {
    let bmax = b.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let rmax = (0..n)
        .map(|i| {
            let ax: f64 = (0..n).map(|j| a[i * n + j] * x[j]).sum();
            (ax - b[i]).abs()
        })
        .fold(0.0_f64, f64::max);
    rmax <= RESIDUAL_TOL * (1.0 + bmax)
}

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
    if !residual_ok(a, b, n, &x) {
        return Err(CoreError::Singular);
    }

    Ok(x)
}

/// Count of nonzero entries in dense row-major `a` (`n × n`).
///
/// The raw material for a fill-fraction measurement (`nnz(a, n) as f64 / (n * n) as f64`) —
/// used by `cargo xtask bench-linsolve` to report how sparse a real assembled MNA Jacobian
/// actually is at a given circuit size.
///
/// Exact-zero equality, not a tolerance: every stamp in this codebase writes a real nonzero
/// physical quantity (a conductance, a unit coefficient, …) into a touched entry and leaves an
/// untouched entry at [`crate::mna::System::new`]'s zero-initialized value — there is no
/// near-cancellation case here to guard against, unlike [`RESIDUAL_TOL`]'s singularity check.
pub fn nnz(a: &[f64], n: usize) -> usize {
    debug_assert_eq!(a.len(), n * n);
    a.iter().filter(|&&v| v != 0.0).count()
}

/// Solve `a · x = b` — same dense row-major `a`/`b`/`n` shape as [`solve_dense`] — via `faer`'s
/// pure-Rust sparse LU (`faer::sparse::linalg::solvers`) instead of its dense LU.
///
/// **Prototype, not production**: `a`'s exact-zero entries are dropped when building the CSC
/// triplet representation `faer`'s sparse solver wants, so this pays the `O(n²)` cost of
/// scanning the dense buffer *before* the sparse solve even starts — a real sparse path would
/// assemble triplets directly from [`va_abi::ModelInstance::load`] and never touch a dense
/// buffer at all. This function exists to answer one question — does `faer`'s sparse solver
/// beat dense on real MNA systems, and at what size — not to be the fast path itself; see
/// `docs/roadmap.md`'s T3 section for that answer and `cargo xtask bench-linsolve` for how to
/// reproduce it.
///
/// # Errors
///
/// [`CoreError::Singular`] if the sparsity pattern is symbolically singular, the numeric
/// factorization fails (a zero pivot with no admissible replacement — including one `faer`
/// itself surfaces as a panic rather than an `Err`, caught at this function's boundary; see the
/// comment at the `catch_unwind` call below), or — mirroring [`solve_dense`]'s own check — the
/// returned `x` fails to reproduce `b` to [`RESIDUAL_TOL`].
pub fn solve_sparse(a: &[f64], b: &[f64], n: usize) -> Result<Vec<f64>, CoreError> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }

    let triplets: Vec<Triplet<usize, usize, f64>> = (0..n)
        .flat_map(|i| (0..n).map(move |j| (i, j)))
        .filter(|&(i, j)| a[i * n + j] != 0.0)
        .map(|(i, j)| Triplet::new(i, j, a[i * n + j]))
        .collect();

    // `faer`'s sparse LU factorization panics (rather than returning `Err`) on at least one
    // genuinely-singular input: a column whose structural pivot candidate has collapsed to an
    // exact numeric zero (`faer-0.22.6/src/sparse/linalg/lu.rs:1426`, `panic!()` with no
    // message) — confirmed empirically by this module's own `sparse_singular_matrix_is_rejected`
    // test, not merely suspected from reading the source. `CLAUDE.md` §5 forbids *this* crate
    // panicking on bad input; since the panic originates one layer down, in a dependency, the
    // only way to keep that promise is to catch it at the boundary and fold it into the same
    // `CoreError::Singular` a graceful rejection would have produced — a caller cannot tell the
    // difference between "rejected gracefully" and "rejected via an internal panic," and should
    // not have to.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mat: SparseColMat<usize, f64> = SparseColMat::try_new_from_triplets(n, n, &triplets)
            .map_err(|_| CoreError::Singular)?;
        let symbolic =
            SparseSymbolicLu::try_new(mat.symbolic()).map_err(|_| CoreError::Singular)?;
        let lu = SparseLu::try_new_with_symbolic(symbolic, mat.as_ref())
            .map_err(|_| CoreError::Singular)?;

        let rhs = Mat::from_fn(n, 1, |i, _| b[i]);
        let sol = lu.solve(&rhs);
        Ok::<Vec<f64>, CoreError>((0..n).map(|i| *sol.get(i, 0)).collect())
    }));

    let x = match outcome {
        Ok(result) => result?,
        Err(_) => return Err(CoreError::Singular),
    };

    if !x.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Singular);
    }

    if !residual_ok(a, b, n, &x) {
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

    #[test]
    fn sparse_solves_2x2() {
        let a = [4.0, 3.0, 6.0, 3.0];
        let b = [10.0, 12.0];
        let x = solve_sparse(&a, &b, 2).expect("non-singular");
        assert!((x[0] - 1.0).abs() < 1e-9, "x0 = {}", x[0]);
        assert!((x[1] - 2.0).abs() < 1e-9, "x1 = {}", x[1]);
    }

    #[test]
    fn sparse_identity_is_passthrough() {
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [7.0, -3.0];
        let x = solve_sparse(&a, &b, 2).unwrap();
        assert!((x[0] - 7.0).abs() < 1e-12);
        assert!((x[1] - (-3.0)).abs() < 1e-12);
    }

    #[test]
    fn sparse_singular_matrix_is_rejected() {
        // Same linearly-dependent rows as `singular_matrix_is_rejected`.
        let a = [1.0, 2.0, 2.0, 4.0];
        let b = [1.0, 1.0];
        assert!(matches!(solve_sparse(&a, &b, 2), Err(CoreError::Singular)));
    }

    #[test]
    fn sparse_empty_system_is_trivial() {
        assert_eq!(solve_sparse(&[], &[], 0).unwrap(), Vec::<f64>::new());
    }

    /// A banded tridiagonal-with-shunt system — the shape a resistor-ladder MNA Jacobian
    /// actually has (§ `cargo xtask bench-linsolve`) — solved by both solvers must agree, since
    /// this is the whole premise of comparing them at all.
    #[test]
    fn sparse_agrees_with_dense_on_a_banded_system() {
        let n = 12;
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            a[i * n + i] = 3.0;
            if i > 0 {
                a[i * n + (i - 1)] = -1.0;
            }
            if i + 1 < n {
                a[i * n + (i + 1)] = -1.0;
            }
        }
        let b: Vec<f64> = (0..n).map(|i| 1.0 + i as f64).collect();

        let dense = solve_dense(&a, &b, n).unwrap();
        let sparse = solve_sparse(&a, &b, n).unwrap();
        for i in 0..n {
            assert!(
                (dense[i] - sparse[i]).abs() < 1e-9,
                "row {i}: dense={} sparse={}",
                dense[i],
                sparse[i]
            );
        }
    }

    #[test]
    fn nnz_counts_only_stamped_entries() {
        // Same banded shape as above, n=5: 3 diagonal-band entries per row except the two
        // edge rows (2 each) — 3*3 + 2*2 = 13.
        let n = 5;
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            a[i * n + i] = 3.0;
            if i > 0 {
                a[i * n + (i - 1)] = -1.0;
            }
            if i + 1 < n {
                a[i * n + (i + 1)] = -1.0;
            }
        }
        assert_eq!(nnz(&a, n), 13);
    }

    #[test]
    fn nnz_of_all_zero_is_zero() {
        assert_eq!(nnz(&[0.0; 9], 3), 0);
    }
}
