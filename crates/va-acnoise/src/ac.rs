//! Small-signal AC analysis: linearize about the DC point, sweep frequency.

use crate::AcNoiseError;
use std::f64::consts::PI;
use va_abi::stamps::StampSink;
use va_abi::ModelInstance;

/// One complex value as an (real, imag) pair. Kept dependency-free; a `num-complex` type can
/// replace this if the workspace adds it.
pub type Complex = (f64, f64);

/// AC sweep specification (logarithmic decade sweep).
#[derive(Clone, Copy, Debug)]
pub struct AcSweep {
    /// Start frequency (Hz).
    pub fstart: f64,
    /// Stop frequency (Hz).
    pub fstop: f64,
    /// Points per decade.
    pub points_per_decade: usize,
}

impl AcSweep {
    /// The logarithmically-spaced frequency points this sweep visits, from `fstart` up to and
    /// including `fstop` (SPICE `.ac dec` convention). Empty if `fstart`/`fstop`/
    /// `points_per_decade` are non-positive or `fstop < fstart`.
    pub fn frequencies(&self) -> Vec<f64> {
        if self.fstart <= 0.0 || self.fstop < self.fstart || self.points_per_decade == 0 {
            return Vec::new();
        }
        if self.fstop == self.fstart {
            return vec![self.fstart];
        }
        let ratio = 10f64.powf(1.0 / self.points_per_decade as f64);
        let mut freqs = Vec::new();
        let mut f = self.fstart;
        // Stop once a step would overshoot `fstop` by more than half a step (in log space) —
        // avoids both a duplicate near-`fstop` point and silently dropping the last decade.
        while f < self.fstop * ratio.sqrt() {
            freqs.push(f);
            f *= ratio;
        }
        // Guarantee `fstop` itself is always the last point, exactly, regardless of rounding
        // drift accumulated by repeated multiplication: if the loop's own last point already
        // landed within float noise of `fstop`, snap it there instead of appending a
        // near-duplicate; otherwise `fstop` is a genuinely new point.
        match freqs.last_mut() {
            Some(last) if (*last - self.fstop).abs() < self.fstop * 1e-9 => *last = self.fstop,
            _ => freqs.push(self.fstop),
        }
        freqs
    }
}

/// The AC response: frequency points paired with the complex node-voltage vectors.
#[derive(Clone, Debug, Default)]
pub struct AcResponse {
    /// Frequency points (Hz).
    pub f: Vec<f64>,
    /// Complex solution vectors, one row per frequency.
    pub x: Vec<Vec<Complex>>,
}

/// Captures the small-signal conductance (`G = ∂residual/∂x`) and charge-Jacobian
/// (`C = ∂charge/∂x`) matrices a [`ModelInstance::load`] stamps at a fixed operating point,
/// dropping the residual/charge values themselves — irrelevant once linearized, since AC
/// analysis only ever uses their derivatives.
struct Linearization {
    dim: usize,
    g: Vec<f64>,
    c: Vec<f64>,
}

impl StampSink for Linearization {
    fn residual(&mut self, _row: usize, _value: f64) {}

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            self.g[row * self.dim + col] += value;
        }
    }

    fn charge(&mut self, _row: usize, _value: f64) {}

    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            self.c[row * self.dim + col] += value;
        }
    }
}

/// Linearize `instances` about operating point `x_dc`, returning the dense `dim × dim`
/// (row-major) conductance matrix `G` and charge-Jacobian matrix `C` such that the small-signal
/// system at angular frequency `ω` is `(G + jω·C)·X(ω) = B(ω)`.
pub fn linearize(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    dim: usize,
) -> (Vec<f64>, Vec<f64>) {
    let mut lin = Linearization {
        dim,
        g: vec![0.0; dim * dim],
        c: vec![0.0; dim * dim],
    };
    for inst in instances {
        inst.load(x_dc, &mut lin);
    }
    (lin.g, lin.c)
}

/// Run an AC sweep about a precomputed DC operating point `x_dc`.
///
/// `excitation` is the complex small-signal RHS vector (length `dim`), nonzero only at the
/// row(s) an independent AC source owns — e.g. a [`va_abi::reference::VSource`]'s own
/// branch-current row, mirroring how that row's DC constraint (`V(p)-V(n) = value`) is stamped:
/// the row's Jacobian entries already capture `∂/∂x`, so the source's own AC magnitude/phase is
/// purely an RHS term, never a `G`/`C` entry.
///
/// At each frequency this solves the complex linear system `(G + jω·C)·X(ω) = excitation` by
/// embedding it as a real `2·dim × 2·dim` block system (stacking `[Re(X); Im(X)]`) and reusing
/// [`va_core::linsolve::solve_dense`] — this avoids adding a complex-linear-algebra dependency,
/// consistent with `CLAUDE.md` §5's pure-Rust/`faer`-only numerics rule.
///
/// # Errors
///
/// Propagates [`AcNoiseError`] from the underlying real linear solve (one per frequency point).
pub fn run(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    dim: usize,
    sweep: AcSweep,
    excitation: &[Complex],
) -> Result<AcResponse, AcNoiseError> {
    debug_assert_eq!(excitation.len(), dim, "excitation must cover every unknown");
    let (g, c) = linearize(instances, x_dc, dim);
    let freqs = sweep.frequencies();
    let mut x = Vec::with_capacity(freqs.len());
    for &f in &freqs {
        x.push(solve_at(&g, &c, dim, 2.0 * PI * f, excitation)?);
    }
    Ok(AcResponse { f: freqs, x })
}

/// Solve `(G + jω·C)·X = excitation` at one angular frequency `ω`, via the real `2n × 2n`
/// block embedding:
///
/// ```text
/// [ G       -ω·C ] [ Re(X) ]   [ Re(B) ]
/// [ ω·C      G   ] [ Im(X) ] = [ Im(B) ]
/// ```
fn solve_at(
    g: &[f64],
    c: &[f64],
    dim: usize,
    omega: f64,
    excitation: &[Complex],
) -> Result<Vec<Complex>, AcNoiseError> {
    let n = dim;
    let m = 2 * n;
    let mut a = vec![0.0; m * m];
    for i in 0..n {
        for j in 0..n {
            let gij = g[i * n + j];
            let cij = c[i * n + j];
            a[i * m + j] = gij;
            a[i * m + (n + j)] = -omega * cij;
            a[(n + i) * m + j] = omega * cij;
            a[(n + i) * m + (n + j)] = gij;
        }
    }
    let mut b = vec![0.0; m];
    for (i, &(re, im)) in excitation.iter().enumerate() {
        b[i] = re;
        b[n + i] = im;
    }
    let sol = va_core::linsolve::solve_dense(&a, &b, m)?;
    Ok((0..n).map(|i| (sol[i], sol[n + i])).collect())
}

/// Magnitude of a [`Complex`] value.
pub fn magnitude((re, im): Complex) -> f64 {
    (re * re + im * im).sqrt()
}

/// Phase (radians) of a [`Complex`] value.
pub fn phase((re, im): Complex) -> f64 {
    im.atan2(re)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use va_abi::reference::{Capacitor, Resistor, VSource, GROUND};

    #[test]
    fn frequencies_cover_start_to_stop_at_the_requested_density() {
        let sweep = AcSweep {
            fstart: 1.0,
            fstop: 100.0,
            points_per_decade: 2,
        };
        let f = sweep.frequencies();
        assert!((f[0] - 1.0).abs() < 1e-9, "first point: {}", f[0]);
        assert!(
            (*f.last().unwrap() - 100.0).abs() < 1e-9,
            "last point: {}",
            f.last().unwrap()
        );
        // 2 decades at 2 points/decade -> 5 points (inclusive of both ends).
        assert_eq!(f.len(), 5, "{f:?}");
    }

    #[test]
    fn frequencies_single_point_when_start_equals_stop() {
        let sweep = AcSweep {
            fstart: 1e3,
            fstop: 1e3,
            points_per_decade: 10,
        };
        assert_eq!(sweep.frequencies(), vec![1e3]);
    }

    #[test]
    fn frequencies_empty_on_a_degenerate_sweep() {
        let sweep = AcSweep {
            fstart: 0.0,
            fstop: 100.0,
            points_per_decade: 10,
        };
        assert!(sweep.frequencies().is_empty());
    }

    /// RC low-pass: `V(in)` driven by an ideal 1V-AC source through `R` into `C` to ground,
    /// output taken at the `R`-`C` junction. Closed form: `H(jω) = 1 / (1 + jωRC)`, magnitude
    /// `1/sqrt(1+(ωRC)²)`, phase `-atan(ωRC)`.
    #[test]
    fn rc_lowpass_response() {
        let r = 1000.0;
        let cap = 1e-6;
        // Unknowns: 0 = in, 1 = out, 2 = source branch current.
        let vs = VSource::new(0, GROUND, 2, 5.0); // DC operating point is irrelevant (linear).
        let res = Resistor::new(0, 1, r);
        let capacitor = Capacitor::new(1, GROUND, cap);
        let insts: [&dyn ModelInstance; 3] = [&vs, &res, &capacitor];

        let x_dc = [5.0, 5.0, 0.0];
        let sweep = AcSweep {
            fstart: 1.0,
            fstop: 1e6,
            points_per_decade: 5,
        };
        // 1V-AC excitation on the source's own branch row; zero everywhere else.
        let excitation = [(0.0, 0.0), (0.0, 0.0), (1.0, 0.0)];

        let response = run(&insts, &x_dc, 3, sweep, &excitation).expect("solves at every point");

        for (&f, x) in response.f.iter().zip(&response.x) {
            let omega = 2.0 * PI * f;
            let wrc = omega * r * cap;
            let expected_mag = 1.0 / (1.0 + wrc * wrc).sqrt();
            let expected_phase = -wrc.atan();

            let got_mag = magnitude(x[1]);
            let got_phase = phase(x[1]);

            assert!(
                (got_mag - expected_mag).abs() < 1e-6,
                "f={f}: mag got {got_mag}, want {expected_mag}"
            );
            assert!(
                (got_phase - expected_phase).abs() < 1e-6,
                "f={f}: phase got {got_phase}, want {expected_phase}"
            );
        }
    }
}
