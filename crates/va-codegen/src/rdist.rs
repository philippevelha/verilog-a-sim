//! The random-number generators behind `$rdist_*` (LRM 9.13.2, 2.42.0).
//!
//! These follow the reference C routines IEEE 1364-2005 §17.9.3 gives for Verilog's `$dist_*`
//! functions (`uniform`, `normal`, `exponential`, `poisson`, `chi_square`, `t`, `erlangian`),
//! which Verilog-AMS's real-valued `$rdist_*` take over: a 32-bit linear congruential seed
//! (`seed = 69069 * seed + 1`, wrapping), whose top 23 bits become a uniform mantissa, and every
//! other distribution built from uniform draws (Marsaglia's polar method for the normal). The seed
//! is the C `long` of those routines, 32 bits here, wrapping exactly as it does there.
//!
//! **Limitation, stated:** written from the standard's description of those routines; agreement
//! bit for bit with another simulator's sequence for the same seed has not been checked against
//! one. What is tested is the distributions' statistics, reproducibility for a given seed, and
//! the seed handling (`va-cli`'s `rdist.rs` test).

use va_ir::RdistKind;

/// The standard's `uniform(seed, start, end)`: a real in `[start, end)` (`[0, 2^31 - 1)` when
/// `start >= end`), advancing `seed`.
pub fn uniform(seed: &mut i32, start: f64, end: f64) -> f64 {
    // 2^-23, the spacing of the mantissa the draw fills.
    const D: f64 = 0.000_000_119_209_289_550_781_25;
    if *seed == 0 {
        *seed = 259_341_593;
    }
    let (a, b) = if start >= end {
        (0.0, 2_147_483_647.0)
    } else {
        (start, end)
    };
    *seed = seed.wrapping_mul(69_069).wrapping_add(1);
    // The top 23 bits of the seed as the mantissa of a float in [1, 2).
    let c = f64::from(f32::from_bits(((*seed as u32) >> 9) | 0x3f80_0000));
    let c = c + c * D;
    (b - a) * (c - 1.0) + a
}

/// The standard's `normal(seed, mean, deviation)`, by Marsaglia's polar method.
pub fn normal(seed: &mut i32, mean: f64, deviation: f64) -> f64 {
    let (mut v1, mut s) = (0.0, 1.0);
    while s >= 1.0 || s == 0.0 {
        v1 = uniform(seed, -1.0, 1.0);
        let v2 = uniform(seed, -1.0, 1.0);
        s = v1 * v1 + v2 * v2;
    }
    let s = v1 * libm::sqrt(-2.0 * libm::log(s) / s);
    s * deviation + mean
}

/// The standard's `exponential(seed, mean)`.
pub fn exponential(seed: &mut i32, mean: f64) -> f64 {
    let n = uniform(seed, 0.0, 1.0);
    if n != 0.0 {
        -libm::log(n) * mean
    } else {
        n
    }
}

/// The standard's `poisson(seed, mean)`: an integer count, returned as a real.
pub fn poisson(seed: &mut i32, mean: f64) -> f64 {
    let p = libm::exp(-mean);
    let mut q = uniform(seed, 0.0, 1.0);
    let mut n = 0u32;
    // Bounded, so a huge mean cannot hang an evaluation: past this the count is a deep tail.
    while p < q && n < 1_000_000 {
        n += 1;
        q *= uniform(seed, 0.0, 1.0);
    }
    f64::from(n)
}

/// The standard's `chi_square(seed, deg_of_free)`.
pub fn chi_square(seed: &mut i32, dof: f64) -> f64 {
    let dof = dof as i64;
    let mut x = if dof % 2 != 0 {
        let n = normal(seed, 0.0, 1.0);
        n * n
    } else {
        0.0
    };
    let mut k = 2;
    while k <= dof {
        x += 2.0 * exponential(seed, 1.0);
        k += 2;
    }
    x
}

/// The standard's `t(seed, deg_of_free)`.
pub fn t(seed: &mut i32, dof: f64) -> f64 {
    let chi2 = chi_square(seed, dof);
    let root = libm::sqrt(chi2 / (dof as i64) as f64);
    normal(seed, 0.0, 1.0) / root
}

/// The standard's `erlangian(seed, k, mean)`.
pub fn erlang(seed: &mut i32, k: f64, mean: f64) -> f64 {
    let k = k as i64;
    let mut x = 1.0;
    for _ in 0..k.max(0) {
        x *= uniform(seed, 0.0, 1.0);
    }
    -mean * libm::log(x) / k as f64
}

/// One draw of `kind` with parameters `p1`, `p2`, advancing `seed`.
pub fn draw(kind: RdistKind, seed: &mut i32, p1: f64, p2: f64) -> f64 {
    match kind {
        RdistKind::Uniform => uniform(seed, p1, p2),
        RdistKind::Normal => normal(seed, p1, p2),
        RdistKind::Exponential => exponential(seed, p1),
        RdistKind::Poisson => poisson(seed, p1),
        RdistKind::ChiSquare => chi_square(seed, p1),
        RdistKind::T => t(seed, p1),
        RdistKind::Erlang => erlang(seed, p1, p2),
    }
}

/// A 32-bit FNV-1a hash of an instance name, mixed into a non-variable seed by the `"instance"`
/// type string so each instance draws its own value.
pub fn instance_hash(name: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean and variance of `n` draws.
    fn moments(n: usize, mut f: impl FnMut() -> f64) -> (f64, f64) {
        let xs: Vec<f64> = (0..n).map(|_| f()).collect();
        let mean = xs.iter().sum::<f64>() / n as f64;
        let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64;
        (mean, var)
    }

    #[test]
    fn each_distribution_has_its_mean_and_variance() {
        let n = 200_000;
        let mut seed = 7;
        // (draw, mean, variance), tolerances a few standard errors wide.
        let cases: [(RdistKind, f64, f64, f64, f64); 7] = [
            (RdistKind::Uniform, 2.0, 6.0, 4.0, 16.0 / 12.0),
            (RdistKind::Normal, 1.5, 0.5, 1.5, 0.25),
            (RdistKind::Exponential, 3.0, 0.0, 3.0, 9.0),
            (RdistKind::Poisson, 4.0, 0.0, 4.0, 4.0),
            (RdistKind::ChiSquare, 5.0, 0.0, 5.0, 10.0),
            (RdistKind::T, 10.0, 0.0, 0.0, 10.0 / 8.0),
            (RdistKind::Erlang, 3.0, 2.0, 2.0, 4.0 / 3.0),
        ];
        for (kind, p1, p2, want_mean, want_var) in cases {
            let (mean, var) = moments(n, || draw(kind, &mut seed, p1, p2));
            let se = (want_var / n as f64).sqrt();
            assert!(
                (mean - want_mean).abs() < 6.0 * se + 1e-3,
                "{kind:?}: mean {mean}, want {want_mean}"
            );
            assert!(
                (var - want_var).abs() < 0.05 * want_var + 1e-3,
                "{kind:?}: var {var}, want {want_var}"
            );
        }
    }

    #[test]
    fn a_seed_gives_one_reproducible_sequence_and_uniform_stays_in_range() {
        let run = |mut seed: i32| {
            (0..100)
                .map(|_| uniform(&mut seed, 0.0, 1.0))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43));
        assert!(run(42).iter().all(|&u| (0.0..=1.0).contains(&u)));
        // A zero seed is replaced, as in the standard, rather than sticking at zero.
        let mut zero = 0;
        uniform(&mut zero, 0.0, 1.0);
        assert_ne!(zero, 0);
    }
}
