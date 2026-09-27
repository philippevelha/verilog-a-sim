//! `cargo run --release -p xtask -- bench-dense` — what the scalar dense LU (1.31.0, the
//! platform-identical kernel in `va_core::linsolve::solve_dense`) costs against `faer`'s blocked
//! SIMD LU (`solve_dense_faer`, the kernel it replaced), per solve, across sizes and matrix
//! shapes.
//!
//! Three shapes, because the scalar kernel skips a zero multiplier and so depends on sparsity:
//! - **ladder**: a resistor ladder's conductance matrix (tridiagonal plus a source row), the
//!   shape of most small circuits;
//! - **mesh**: a square grid's (5-point Laplacian plus a leak to ground): LU fill-in grows as
//!   `n^1.5`, the hard case for a sparse matrix held densely;
//! - **dense**: every entry nonzero, the scalar kernel's worst case.
//!
//! Each figure is the minimum over five batches of the mean time per solve within a batch
//! (batches sized to ~50 ms). Both kernels solve the same system and their answers are checked
//! against each other. **Limitation:** single machine, single thread; the matrices are built
//! directly, not assembled from devices (the shape is what the kernels see).

use anyhow::{bail, Result};
use std::time::Instant;
use va_core::linsolve::{solve_dense, solve_dense_faer};

const SIZES: &[usize] = &[10, 25, 50, 100, 200, 400, 800, 1600];

fn ladder(n: usize) -> Vec<f64> {
    let mut a = vec![0.0; n * n];
    for i in 0..n {
        a[i * n + i] = 2.0 + 1e-3;
        if i > 0 {
            a[i * n + i - 1] = -1.0;
        }
        if i + 1 < n {
            a[i * n + i + 1] = -1.0;
        }
    }
    a
}

fn mesh(n: usize) -> Vec<f64> {
    let side = (n as f64).sqrt().ceil() as usize;
    let mut a = vec![0.0; n * n];
    for i in 0..n {
        let (r, c) = (i / side, i % side);
        a[i * n + i] = 4.0 + 1e-3;
        let mut link = |j: usize| {
            if j < n {
                a[i * n + j] = -1.0;
            }
        };
        if c > 0 {
            link(i - 1);
        }
        if c + 1 < side {
            link(i + 1);
        }
        if r > 0 {
            link(i - side);
        }
        link(i + side);
    }
    a
}

fn dense(n: usize) -> Vec<f64> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut a: Vec<f64> = (0..n * n)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64) / ((1u64 << 53) as f64) - 0.5
        })
        .collect();
    for i in 0..n {
        a[i * n + i] += n as f64;
    }
    a
}

/// Seconds per solve: the minimum over five batches of each batch's mean.
fn time(solve: &dyn Fn() -> Result<Vec<f64>, va_core::CoreError>) -> Result<f64> {
    let t0 = Instant::now();
    solve()?;
    let once = t0.elapsed().as_secs_f64().max(1e-7);
    let reps = ((0.05 / once) as usize).clamp(1, 100_000);
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(solve()?);
        }
        best = best.min(t.elapsed().as_secs_f64() / reps as f64);
    }
    Ok(best)
}

/// Entry point.
pub fn bench_dense() -> Result<()> {
    println!(
        "{:>6} {:>7} {:>12} {:>12} {:>8} {:>11}",
        "n", "shape", "scalar_ms", "faer_ms", "ratio", "max|dx|"
    );
    for &n in SIZES {
        for (shape, build) in [
            ("ladder", ladder as fn(usize) -> Vec<f64>),
            ("mesh", mesh),
            ("dense", dense),
        ] {
            let a = build(n);
            let b: Vec<f64> = (0..n).map(|i| 1.0 + (i % 7) as f64).collect();
            let x = solve_dense(&a, &b, n)?;
            let y = solve_dense_faer(&a, &b, n)?;
            let dx = x
                .iter()
                .zip(&y)
                .fold(0.0_f64, |m, (p, q)| m.max((p - q).abs()));
            if dx > 1e-8 {
                bail!("{shape} n={n}: the kernels disagree by {dx:e}");
            }
            let scalar = time(&|| solve_dense(&a, &b, n))?;
            let faer = time(&|| solve_dense_faer(&a, &b, n))?;
            println!(
                "{n:>6} {shape:>7} {:>12.4} {:>12.4} {:>8.2} {dx:>11.1e}",
                scalar * 1e3,
                faer * 1e3,
                scalar / faer
            );
        }
    }
    Ok(())
}
