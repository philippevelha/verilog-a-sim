//! Linear solve `J · dx = -f`: a scalar dense LU of our own ([`solve_dense`], platform-identical
//! bits since 1.31.0) and `faer`'s sparse LU (pure Rust, per §5).
//!
//! [`solve_dense`] is what `newton`/`dc` call below the sparse threshold. [`solve_sparse`]
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
pub(crate) const RESIDUAL_TOL: f64 = 1e-6;

/// Make `faer` factorize and solve sequentially. Every solve in this crate calls it first.
///
/// `faer`'s high-level solvers (`partial_piv_lu`, sparse `Lu`) read a process-global
/// parallelism setting whose default is "rayon, all cores". A parallel factorization splits its
/// work — and so orders its floating-point sums — by the pool size, so the same deck gave
/// different last digits on machines with different core counts (measured on chain160, 5 286
/// unknowns: 159 of 5 292 printed lines differ between `RAYON_NUM_THREADS=1` and `=4`; fixed in
/// 1.16.1). Sequential is also the faster setting on circuit matrices: c432's numeric LU took
/// 16.0 s sequential, 22.5 s 4-way and 36.5 s 8-way (`docs/proposals/parallel-assembly.md` §5.1).
///
/// Set on every call, not once, because the setting is global: anything else in the process
/// that changes it would otherwise silently change our answers. A fixed `Par::rayon(n)` would
/// also be reproducible; "all cores" (`n = 0`, the default) is what must never be used.
///
/// **Limitation:** it is a process-wide setting, so it also makes any other `faer` user in the
/// same process sequential. No such user exists in this workspace.
pub(crate) fn pin_sequential() {
    faer::set_global_parallelism(faer::Par::Seq);
}

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

/// Dense LU with partial pivoting on row-major `a` (`m × m`), solving in place: `b` becomes `x`.
/// `None` on a zero pivot.
///
/// **The same bits on every platform** (1.31.0). Each entry is updated by one multiply and one
/// subtract, in a fixed order, and every sum (the back substitution) runs left to right; Rust
/// never contracts `a - f*b` into a fused multiply-add or reorders a floating-point reduction on
/// its own, so auto-vectorisation of the row update cannot change a result. `faer`'s dense LU,
/// which [`solve_dense`] used before, picks SIMD kernels per architecture, and on arm64 (NEON)
/// they round differently from x86-64's: three decks differed between Linux and macOS in CI,
/// and were identical when rerun on the sparse path (Road to 2.0 item 3, release.txt 1.30.0+4).
///
/// A multiplier that is exactly zero skips its row, which is most of them in an MNA Jacobian.
/// Pivoting takes the largest magnitude in the column, the last one on a tie (`max_by`), which is
/// deterministic. Shared with [`crate::btf`]'s diagonal blocks, which used it first.
pub(crate) fn lu_solve_in_place(m: usize, a: &mut [f64], b: &mut [f64]) -> Option<()> {
    for k in 0..m {
        let p = (k..m).max_by(|&i, &j| a[i * m + k].abs().total_cmp(&a[j * m + k].abs()))?;
        if a[p * m + k] == 0.0 {
            return None;
        }
        if p != k {
            for j in 0..m {
                a.swap(k * m + j, p * m + j);
            }
            b.swap(k, p);
        }
        let piv = a[k * m + k];
        for i in k + 1..m {
            let f = a[i * m + k] / piv;
            if f != 0.0 {
                for j in k..m {
                    a[i * m + j] -= f * a[k * m + j];
                }
                b[i] -= f * b[k];
            }
        }
    }
    for k in (0..m).rev() {
        let mut s = b[k];
        for j in k + 1..m {
            s -= a[k * m + j] * b[j];
        }
        b[k] = s / a[k * m + k];
    }
    Some(())
}

/// Solve `a · x = b` where `a` is a dense row-major `n × n` matrix and `b` has length `n`.
/// Returns `x`.
///
/// Scalar LU with partial pivoting ([`lu_solve_in_place`]) since 1.31.0, so a dense-path answer
/// has the same bits on every platform. Singularity is detected three ways: a zero pivot, a
/// non-finite solution, or a solution that fails to reproduce `b`.
///
/// **Cost, stated:** a scalar `O(n³)` factorisation where `faer` uses blocked SIMD kernels. On
/// a sparse MNA Jacobian the zero-multiplier skip makes it faster; on a genuinely dense matrix it
/// is slower, increasingly so with `n` (`cargo xtask bench-dense`; the figures are in
/// release.txt 1.31.0). The dense path is the default only below 100 unknowns
/// (`sparse::SPARSE_THRESHOLD`); above that it runs only when asked for with `--solver dense`.
///
/// # Errors
///
/// [`CoreError::NonFinite`] if `a` or `b` already holds a NaN/inf — a model evaluation
/// problem, checked before factoring so it is not misreported as singularity — or
/// [`CoreError::Singular`] if `a` is singular to working precision.
pub fn solve_dense(a: &[f64], b: &[f64], n: usize) -> Result<Vec<f64>, CoreError> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }
    crate::check_finite(a, b, n)?;
    let mut lu = a.to_vec();
    let mut x = b.to_vec();
    lu_solve_in_place(n, &mut lu, &mut x).ok_or(CoreError::Singular)?;
    if !x.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Singular);
    }
    if !residual_ok(a, b, n, &x) {
        return Err(CoreError::Singular);
    }
    Ok(x)
}

/// [`solve_dense`] as it was before 1.31.0: `faer`'s blocked, SIMD partial-pivoting LU. Kept
/// only as the reference `cargo xtask bench-dense` times the scalar kernel against. It is not
/// on any solve path, because its last bits depend on the CPU architecture.
///
/// # Errors
///
/// As [`solve_dense`].
pub fn solve_dense_faer(a: &[f64], b: &[f64], n: usize) -> Result<Vec<f64>, CoreError> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }
    crate::check_finite(a, b, n)?;
    pin_sequential();

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
    crate::check_finite(a, b, n)?;
    pin_sequential();

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
    fn a_nan_in_the_system_is_reported_as_non_finite_not_singular() {
        // The matrix is a perfectly well-conditioned identity; only the residual is bad. Before
        // 2026-09-11 LU turned the NaN into a NaN solution and this came back as `Singular`,
        // sending the user to look for a floating node that does not exist.
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [1.0, f64::NAN];
        assert!(matches!(
            solve_dense(&a, &b, 2),
            Err(CoreError::NonFinite { row: 1, col: None })
        ));
        assert!(matches!(
            solve_sparse(&a, &b, 2),
            Err(CoreError::NonFinite { row: 1, col: None })
        ));
        // A Jacobian entry: row-major index 2 of a 2x2 is (1, 0).
        let a_inf = [1.0, 0.0, f64::INFINITY, 1.0];
        assert!(matches!(
            solve_dense(&a_inf, &[1.0, 1.0], 2),
            Err(CoreError::NonFinite {
                row: 1,
                col: Some(0)
            })
        ));
        // And the residual is named first when both are bad.
        assert!(matches!(
            solve_dense(&a_inf, &[f64::NAN, 1.0], 2),
            Err(CoreError::NonFinite { row: 0, col: None })
        ));
    }

    /// A fixed pseudo-random `n × n` system, diagonally weighted so it is well conditioned, with
    /// a sprinkling of exact zeros as an MNA Jacobian has. Integer LCG, so it is the same system
    /// on every platform.
    fn lcg_system(n: usize, seed: u64) -> (Vec<f64>, Vec<f64>) {
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64) / ((1u64 << 53) as f64) - 0.5
        };
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..n {
                let v = next();
                a[i * n + j] = if v.abs() < 0.2 { 0.0 } else { v };
            }
            a[i * n + i] += 0.5 * n as f64 * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        let b: Vec<f64> = (0..n).map(|_| next()).collect();
        (a, b)
    }

    /// The scalar kernel agrees with `faer`'s LU to rounding, across sizes.
    #[test]
    fn the_scalar_lu_agrees_with_faer() {
        for (n, seed) in [(1, 1), (2, 2), (7, 3), (40, 4), (99, 5), (150, 6)] {
            let (a, b) = lcg_system(n, seed);
            let ours = solve_dense(&a, &b, n).expect("scalar solves");
            let theirs = solve_dense_faer(&a, &b, n).expect("faer solves");
            for (x, y) in ours.iter().zip(&theirs) {
                assert!(
                    (x - y).abs() <= 1e-12 * (1.0 + y.abs()),
                    "n={n}: {x} vs {y}"
                );
            }
        }
    }

    /// **The same bits on every platform** (1.31.0). The solution's bit patterns were recorded
    /// on x86-64 (Windows, MSVC) and are asserted exactly; CI runs this on Linux and macOS
    /// arm64 too, which is the unit-level form of Road to 2.0 item 3's cross-platform check.
    /// `faer`'s dense LU fails this kind of test across architectures, which is why it is no
    /// longer on the solve path.
    #[test]
    fn the_scalar_lu_gives_the_same_bits_on_every_platform() {
        let (a, b) = lcg_system(12, 42);
        let x = solve_dense(&a, &b, 12).expect("solves");
        let bits: Vec<u64> = x.iter().map(|v| v.to_bits()).collect();
        assert_eq!(bits, GOLDEN_BITS, "{bits:#x?}");
    }

    const GOLDEN_BITS: [u64; 12] = [
        0x3f883c949dc3b240,
        0xbfb2e0db88e5ae10,
        0x3f75bb59e77c5057,
        0xbf9b2b43c8c5be38,
        0xbfabc45e7f185fee,
        0x3f97ed3bdf1ce2de,
        0x3fa5f82e825355a4,
        0xbf8155c6f85f2383,
        0xbf9b372b043f3dd2,
        0x3fb58934d235292b,
        0x3f98f470a1514750,
        0xbf98b7ded895610b,
    ];

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
