//! Small-signal AC analysis: linearize about the DC point, sweep frequency.

use crate::AcNoiseError;
use std::f64::consts::PI;
use va_abi::stamps::StampSink;
use va_abi::ModelInstance;
pub use va_core::sparse::Solver;
use va_core::sparse::{Pattern, SparseLu, SparseMatrix, SparseSystem};

/// One complex value as an (real, imag) pair. Kept dependency-free; a `num-complex` type can
/// replace this if the workspace adds it.
pub type Complex = (f64, f64);

/// How an [`AcSweep`] spaces its frequency points — SPICE's three sweep types.
///
/// The `points` count means something different for each, exactly as it does in a SPICE
/// `.ac` card: a *density* for the two logarithmic types, a *total* for the linear one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AcSweepKind {
    /// `dec`: logarithmic, `points` per decade (a factor of 10).
    #[default]
    Dec,
    /// `oct`: logarithmic, `points` per octave (a factor of 2).
    Oct,
    /// `lin`: linear, `points` in total across `[fstart, fstop]`.
    Lin,
}

/// AC sweep specification.
#[derive(Clone, Copy, Debug)]
pub struct AcSweep {
    /// Start frequency (Hz).
    pub fstart: f64,
    /// Stop frequency (Hz).
    pub fstop: f64,
    /// Point count, interpreted per [`Self::kind`]: per decade, per octave, or in total.
    pub points: usize,
    /// Spacing rule.
    pub kind: AcSweepKind,
}

impl AcSweep {
    /// The `dec` sweep this type used to be the only form of — `points` per decade.
    pub fn dec(fstart: f64, fstop: f64, points_per_decade: usize) -> Self {
        AcSweep {
            fstart,
            fstop,
            points: points_per_decade,
            kind: AcSweepKind::Dec,
        }
    }
}

impl AcSweep {
    /// The frequency points this sweep visits, from `fstart` up to and including `fstop`,
    /// spaced per [`Self::kind`] (SPICE's `dec`/`oct`/`lin` conventions). `fstop` is always
    /// the exact last point. Empty if `fstart`/`fstop`/`points` are non-positive or
    /// `fstop < fstart`.
    pub fn frequencies(&self) -> Vec<f64> {
        if self.fstart <= 0.0 || self.fstop < self.fstart || self.points == 0 {
            return Vec::new();
        }
        if self.fstop == self.fstart {
            return vec![self.fstart];
        }
        // `lin` is a *total* count over the closed interval, so it is generated directly
        // rather than by the ratio walk the logarithmic types share. A single requested point
        // degenerates to `fstart` alone, matching the `fstop == fstart` case above.
        if self.kind == AcSweepKind::Lin {
            if self.points == 1 {
                return vec![self.fstart];
            }
            let step = (self.fstop - self.fstart) / (self.points - 1) as f64;
            let mut freqs: Vec<f64> = (0..self.points)
                .map(|i| self.fstart + step * i as f64)
                .collect();
            // Same exactness guarantee the logarithmic path gives below: the last point is
            // `fstop` itself, not `fstart + (n-1)*step` with its accumulated rounding.
            if let Some(last) = freqs.last_mut() {
                *last = self.fstop;
            }
            return freqs;
        }
        let per = match self.kind {
            AcSweepKind::Oct => 2f64,
            _ => 10f64,
        };
        let ratio = libm::pow(per, 1.0 / self.points as f64);
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
///
/// The one exception is [`Self::excitation`]: a model's own `ac_stim` is a *constant* complex
/// term, so unlike the residual it survives linearization — it is the small-signal system's
/// right-hand side rather than part of its matrix.
#[derive(Clone, Debug)]
pub struct Linearization {
    dim: usize,
    /// Conductance matrix `G = ∂residual/∂x`, dense row-major `dim × dim`.
    pub g: Vec<f64>,
    /// Charge-Jacobian matrix `C = ∂charge/∂x`, dense row-major `dim × dim`.
    pub c: Vec<f64>,
    /// Each row's model-supplied `ac_stim`, in [`va_abi::StampSink::excitation`]'s
    /// residual-side sign convention — so forming the system's right-hand side **negates** it.
    /// All-zero unless some compiled model in `instances` calls `ac_stim`.
    pub excitation: Vec<Complex>,
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

    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        if row < self.dim {
            self.excitation[row].0 += re;
            self.excitation[row].1 += im;
        }
    }
}

/// Linearize `instances` about operating point `x_dc`, for the analysis `ctx` names, returning
/// the dense `dim × dim` (row-major) `G` and `C` such that the small-signal system at angular
/// frequency `ω` is `(G + jω·C)·X(ω) = B(ω)` — plus any `ac_stim` excitation the models
/// themselves contribute to `B`.
///
/// `ctx` decides which analysis a compiled model believes it is being evaluated for: pass
/// [`va_abi::AnalysisCtx::ac`] for an AC sweep and [`va_abi::AnalysisCtx::noise`] for a noise
/// run, so that a model's `analysis("ac")`/`analysis("noise")` branches answer correctly. It
/// carries no frequency, and this function is deliberately called **once**, outside any
/// frequency loop: `G` and `C` are frequency-independent by construction. A frequency-dependent
/// small-signal response (`laplace_*`, `zi_*`) would need per-frequency re-linearization, which
/// this signature does not provide and does not pretend to.
pub fn linearize(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    ctx: &va_abi::AnalysisCtx,
    dim: usize,
) -> Linearization {
    let mut lin = Linearization {
        dim,
        g: vec![0.0; dim * dim],
        c: vec![0.0; dim * dim],
        excitation: vec![(0.0, 0.0); dim],
    };
    for inst in instances {
        inst.load(x_dc, ctx, &mut va_abi::ModelState::stateless(), &mut lin);
    }
    lin
}

/// [`Linearization`]'s sparse counterpart: `G` and `C` on one shared pattern — exactly the
/// Jacobian / charge-Jacobian pair [`SparseSystem`] already holds — and the model excitation kept
/// here, since `SparseSystem` does not record it. Used above
/// [`va_core::sparse::SPARSE_THRESHOLD`] unknowns (`docs/proposals/sparse-solve.md`, Step 4).
pub(crate) struct SparseLinearization {
    sys: SparseSystem,
    excitation: Vec<Complex>,
}

impl SparseLinearization {
    fn new(dim: usize) -> Self {
        Self {
            sys: SparseSystem::new(dim),
            excitation: vec![(0.0, 0.0); dim],
        }
    }

    /// Clear, linearize every instance at `x_dc` for `ctx`, and fold the pattern. Returns whether
    /// the pattern grew — always on the first call with any off-diagonal entry, afterwards only
    /// if a model stamped an entry it had not before.
    fn load(
        &mut self,
        instances: &[&dyn ModelInstance],
        x_dc: &[f64],
        ctx: &va_abi::AnalysisCtx,
    ) -> bool {
        self.sys.clear();
        self.excitation.iter_mut().for_each(|e| *e = (0.0, 0.0));
        for inst in instances {
            inst.load(x_dc, ctx, &mut va_abi::ModelState::stateless(), self);
        }
        self.sys.finish()
    }
}

impl StampSink for SparseLinearization {
    fn residual(&mut self, _row: usize, _value: f64) {}

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        StampSink::jacobian(&mut self.sys, row, col, value);
    }

    fn charge(&mut self, _row: usize, _value: f64) {}

    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        StampSink::dcharge(&mut self.sys, row, col, value);
    }

    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        if let Some(e) = self.excitation.get_mut(row) {
            e.0 += re;
            e.1 += im;
        }
    }
}

/// The real `2n × 2n` embedding of `A = G + jω·C` — or of its plain transpose — on the sparse
/// path: the same blocks [`solve_block_embedded`] fills densely, so the two paths solve the
/// identical real system and differ only in how it is factored.
///
/// Built once per `G`/`C` pattern. Each entry of that pattern is mapped to the four slots it
/// fills in the embedded pattern up front, so a frequency point is a pass over the values and a
/// numeric factorization: the embedded pattern does not depend on `ω`, and [`SparseLu`] keeps
/// its symbolic factorization for the whole sweep.
pub(crate) struct SparseEmbedding {
    n: usize,
    pattern: Pattern,
    /// For entry `k` of the `n × n` pattern, the embedded slots of `G`, `−ω·C`, `ω·C`, `G`.
    slots: Vec<[usize; 4]>,
    values: Vec<f64>,
    lu: SparseLu,
}

impl SparseEmbedding {
    fn new(base: &Pattern, transpose: bool) -> Self {
        let n = base.dim();
        // `transpose` swaps each entry's row and column as the blocks are laid out, which is
        // what the dense `transpose` flag does: the plain transpose, not the conjugate one.
        let ij: Vec<(usize, usize)> = base
            .entries()
            .map(|(r, c)| if transpose { (c, r) } else { (r, c) })
            .collect();
        let pattern = Pattern::new(
            2 * n,
            ij.iter()
                .flat_map(|&(i, j)| [(i, j), (i, n + j), (n + i, j), (n + i, n + j)]),
        );
        let slot = |r, c| {
            pattern
                .slot(r, c)
                .expect("every embedded entry was inserted into the pattern just above")
        };
        let slots = ij
            .iter()
            .map(|&(i, j)| {
                [
                    slot(i, j),
                    slot(i, n + j),
                    slot(n + i, j),
                    slot(n + i, n + j),
                ]
            })
            .collect();
        let values = vec![0.0; pattern.nnz()];
        Self {
            n,
            pattern,
            slots,
            values,
            lu: SparseLu::new(),
        }
    }

    /// Solve the embedded system at `omega`, `g` and `c` being the values of the base pattern
    /// this embedding was built from (in its slot order).
    fn solve(
        &mut self,
        g: &[f64],
        c: &[f64],
        omega: f64,
        rhs: &[Complex],
    ) -> Result<Vec<Complex>, AcNoiseError> {
        let n = self.n;
        self.values.iter_mut().for_each(|v| *v = 0.0);
        // Each base entry is distinct, so each embedded slot is written once: assignment, as the
        // dense embedding does, rather than accumulation.
        for (s, (&gk, &ck)) in self.slots.iter().zip(g.iter().zip(c)) {
            self.values[s[0]] = gk;
            self.values[s[1]] = -omega * ck;
            self.values[s[2]] = omega * ck;
            self.values[s[3]] = gk;
        }
        let mut b = vec![0.0; 2 * n];
        for (i, &(re, im)) in rhs.iter().enumerate() {
            b[i] = re;
            b[n + i] = im;
        }
        let a =
            SparseMatrix::new(&self.pattern, &self.values).ok_or(va_core::CoreError::Singular)?;
        let sol = self.lu.solve(a, &b)?;
        Ok((0..n).map(|i| (sol[i], sol[n + i])).collect())
    }
}

/// A linearized small-signal system, dense or sparse, ready to be solved at any frequency —
/// shared by [`run_with`] and [`crate::noise::run_with`] so the dense/sparse choice is made in one
/// place, as the embedding convention is.
pub(crate) enum SmallSignal {
    /// The dense path: exactly what AC and noise did before Step 4.
    Dense {
        lin: Linearization,
        dim: usize,
        transpose: bool,
    },
    /// The sparse path: `G`/`C` on one pattern and the real embedding built from it.
    Sparse {
        lin: Box<SparseLinearization>,
        embedding: Box<SparseEmbedding>,
        transpose: bool,
    },
}

impl SmallSignal {
    /// Linearize `instances` at `x_dc` for `ctx`, on the path `solver` picks for `dim` unknowns.
    /// `transpose` asks for `Aᵀ` (the noise adjoint) instead of `A`.
    pub(crate) fn linearize(
        instances: &[&dyn ModelInstance],
        x_dc: &[f64],
        ctx: &va_abi::AnalysisCtx,
        dim: usize,
        solver: Solver,
        transpose: bool,
    ) -> Self {
        if solver.uses_sparse(dim) {
            let mut lin = SparseLinearization::new(dim);
            lin.load(instances, x_dc, ctx);
            let embedding = Box::new(SparseEmbedding::new(lin.sys.pattern(), transpose));
            SmallSignal::Sparse {
                lin: Box::new(lin),
                embedding,
                transpose,
            }
        } else {
            SmallSignal::Dense {
                lin: linearize(instances, x_dc, ctx, dim),
                dim,
                transpose,
            }
        }
    }

    /// Re-linearize for a new `ctx` — a frequency-dependent circuit's next point. The sparse
    /// path keeps its pattern and embedding, and so its symbolic factorization, unless the
    /// pattern grew.
    pub(crate) fn relinearize(
        &mut self,
        instances: &[&dyn ModelInstance],
        x_dc: &[f64],
        ctx: &va_abi::AnalysisCtx,
    ) {
        match self {
            SmallSignal::Dense { lin, dim, .. } => *lin = linearize(instances, x_dc, ctx, *dim),
            SmallSignal::Sparse {
                lin,
                embedding,
                transpose,
            } => {
                if lin.load(instances, x_dc, ctx) {
                    **embedding = SparseEmbedding::new(lin.sys.pattern(), *transpose);
                }
            }
        }
    }

    /// The model-supplied `ac_stim`, in [`Linearization::excitation`]'s convention.
    pub(crate) fn excitation(&self) -> &[Complex] {
        match self {
            SmallSignal::Dense { lin, .. } => &lin.excitation,
            SmallSignal::Sparse { lin, .. } => &lin.excitation,
        }
    }

    /// Solve `A·X = rhs` (or `Aᵀ·X = rhs`) at angular frequency `omega`.
    pub(crate) fn solve(
        &mut self,
        omega: f64,
        rhs: &[Complex],
    ) -> Result<Vec<Complex>, AcNoiseError> {
        match self {
            SmallSignal::Dense {
                lin,
                dim,
                transpose,
            } => solve_block_embedded(&lin.g, &lin.c, *dim, omega, rhs, *transpose),
            SmallSignal::Sparse { lin, embedding, .. } => embedding.solve(
                lin.sys.jacobian().values(),
                lin.sys.dcharge().values(),
                omega,
                rhs,
            ),
        }
    }
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
/// consistent with `CLAUDE.md` §5's pure-Rust/`faer`-only numerics rule. That is the path below
/// [`va_core::sparse::SPARSE_THRESHOLD`] unknowns; from it the same embedding is factored with
/// sparse LU instead (see [`run_with`]).
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
    run_with(instances, x_dc, dim, sweep, excitation, Solver::Auto)
}

/// [`run`] with an explicit [`Solver`]. Below [`va_core::sparse::SPARSE_THRESHOLD`] unknowns
/// (under [`Solver::Auto`]) the dense real embedding solves every point, exactly as before
/// Step 4; from it, the same embedding is factored sparse, with one symbolic factorization for
/// the whole sweep.
///
/// # Errors
///
/// As [`run`].
pub fn run_with(
    instances: &[&dyn ModelInstance],
    x_dc: &[f64],
    dim: usize,
    sweep: AcSweep,
    excitation: &[Complex],
    solver: Solver,
) -> Result<AcResponse, AcNoiseError> {
    debug_assert_eq!(excitation.len(), dim, "excitation must cover every unknown");
    let freqs = sweep.frequencies();

    // Does any instance's own `G`/`C` move with frequency (a `laplace_*` transfer function)?
    // Almost never — so the default path linearizes **once**, which is both what every
    // ordinary circuit needs and bit-for-bit what this function did before Tier C.
    let per_point = instances.iter().any(|i| i.is_frequency_dependent());

    let mut x = Vec::with_capacity(freqs.len());
    if per_point {
        // O(points) linearizations. Paid only by a circuit that actually contains a filter,
        // which is why `is_frequency_dependent` is opt-in (§ `va_abi::ModelInstance`).
        let mut small: Option<SmallSignal> = None;
        for &f in &freqs {
            let ctx = va_abi::AnalysisCtx::ac_at(f);
            let small = match &mut small {
                Some(s) => {
                    s.relinearize(instances, x_dc, &ctx);
                    s
                }
                None => small.insert(SmallSignal::linearize(
                    instances, x_dc, &ctx, dim, solver, false,
                )),
            };
            let rhs = combine_excitation(excitation, small.excitation());
            x.push(small.solve(2.0 * PI * f, &rhs)?);
        }
    } else {
        let mut small = SmallSignal::linearize(
            instances,
            x_dc,
            &va_abi::AnalysisCtx::ac(),
            dim,
            solver,
            false,
        );
        let rhs = combine_excitation(excitation, small.excitation());
        for &f in &freqs {
            x.push(small.solve(2.0 * PI * f, &rhs)?);
        }
    }
    Ok(AcResponse { f: freqs, x })
}

/// Sum the netlist's own `AC mag phase` sources with any model-supplied `ac_stim` into the
/// system's right-hand side.
///
/// The netlist's contribution arrives already on the right-hand side; a model's `ac_stim`
/// arrives in `residual`'s sign convention (§ [`va_abi::StampSink::excitation`]), so it crosses
/// the equals sign here.
fn combine_excitation(netlist: &[Complex], model: &[Complex]) -> Vec<Complex> {
    netlist
        .iter()
        .zip(model)
        .map(|(&(bre, bim), &(ere, eim))| (bre - ere, bim - eim))
        .collect()
}

/// Solve `A·X = rhs` (or `Aᵀ·X = rhs` when `transpose`) for `A = G + jω·C`, via the real
/// `2n × 2n` block embedding:
///
/// ```text
/// [ G       -ω·C ] [ Re(X) ]   [ Re(B) ]
/// [ ω·C      G   ] [ Im(X) ] = [ Im(B) ]
/// ```
///
/// `transpose` transposes `G` and `C` as it fills the blocks, which embeds `Aᵀ` rather than `A`
/// — note this is the plain transpose, **not** the conjugate transpose, which is what an adjoint
/// noise analysis actually wants ([`crate::noise::run`]'s own doc comment derives why).
///
/// Shared by [`ac::run`](run) and [`crate::noise::run`] so there is exactly one place where the
/// complex-to-real embedding convention lives; a sign error here would otherwise have to be
/// found twice. [`SparseEmbedding`], next to it, lays out the same four blocks on the sparse
/// path.
pub(crate) fn solve_block_embedded(
    g: &[f64],
    c: &[f64],
    dim: usize,
    omega: f64,
    rhs: &[Complex],
    transpose: bool,
) -> Result<Vec<Complex>, AcNoiseError> {
    let n = dim;
    let m = 2 * n;
    let mut a = vec![0.0; m * m];
    for i in 0..n {
        for j in 0..n {
            let src = if transpose { j * n + i } else { i * n + j };
            let gij = g[src];
            let cij = c[src];
            a[i * m + j] = gij;
            a[i * m + (n + j)] = -omega * cij;
            a[(n + i) * m + j] = omega * cij;
            a[(n + i) * m + (n + j)] = gij;
        }
    }
    let mut b = vec![0.0; m];
    for (i, &(re, im)) in rhs.iter().enumerate() {
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
    libm::atan2(im, re)
}

#[cfg(test)]
mod tests {

    /// Each sweep type's *own* contract, checked against values that can be written down by
    /// hand rather than recomputed from the implementation.
    #[test]
    fn each_sweep_type_spaces_its_points_its_own_way() {
        // `dec`: a density. 1 Hz to 1 kHz at 1 point/decade is 1, 10, 100, 1000.
        let f = AcSweep::dec(1.0, 1000.0, 1).frequencies();
        assert_eq!(f.len(), 4);
        for (got, want) in f.iter().zip([1.0, 10.0, 100.0, 1000.0]) {
            assert!((got - want).abs() < 1e-9, "dec grid {f:?}");
        }

        // `oct`: the same shape, but per factor of 2. 1 Hz to 8 Hz at 1/octave is 1, 2, 4, 8.
        let f = AcSweep {
            fstart: 1.0,
            fstop: 8.0,
            points: 1,
            kind: AcSweepKind::Oct,
        }
        .frequencies();
        assert_eq!(f.len(), 4);
        for (got, want) in f.iter().zip([1.0, 2.0, 4.0, 8.0]) {
            assert!((got - want).abs() < 1e-9, "oct grid {f:?}");
        }

        // `lin`: a *total*, not a density. 5 points over 0..100 are evenly spaced.
        let f = AcSweep {
            fstart: 0.0_f64.max(20.0),
            fstop: 100.0,
            points: 5,
            kind: AcSweepKind::Lin,
        }
        .frequencies();
        assert_eq!(f, vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    /// `fstop` is the exact last point of every grid, not a value approached by accumulated
    /// multiplication or addition -- the property a golden comparison against another
    /// simulator's frequency column depends on.
    #[test]
    fn every_sweep_type_ends_exactly_on_fstop() {
        let cases = [
            AcSweep::dec(1.0, 1e6, 7),
            AcSweep {
                fstart: 3.0,
                fstop: 9999.0,
                points: 3,
                kind: AcSweepKind::Oct,
            },
            AcSweep {
                fstart: 1.0,
                fstop: 1e5,
                points: 37,
                kind: AcSweepKind::Lin,
            },
        ];
        for c in cases {
            let f = c.frequencies();
            assert_eq!(
                *f.last().expect("a non-empty grid"),
                c.fstop,
                "{:?} sweep must end exactly on fstop",
                c.kind
            );
            assert_eq!(f[0], c.fstart, "and start exactly on fstart");
            assert!(
                f.windows(2).all(|w| w[1] > w[0]),
                "{:?} grid must be strictly increasing",
                c.kind
            );
        }
    }

    /// Degenerate requests are answered, not panicked on: a single linear point, and a count
    /// of zero.
    #[test]
    fn degenerate_sweeps_are_handled() {
        let one = AcSweep {
            fstart: 50.0,
            fstop: 500.0,
            points: 1,
            kind: AcSweepKind::Lin,
        };
        assert_eq!(one.frequencies(), vec![50.0]);
        let none = AcSweep {
            fstart: 50.0,
            fstop: 500.0,
            points: 0,
            kind: AcSweepKind::Lin,
        };
        assert!(none.frequencies().is_empty());
    }
    use super::*;
    use std::f64::consts::PI;
    use va_abi::reference::{Capacitor, Resistor, VSource, GROUND};

    #[test]
    fn frequencies_cover_start_to_stop_at_the_requested_density() {
        let sweep = AcSweep {
            fstart: 1.0,
            fstop: 100.0,
            points: 2,
            kind: Default::default(),
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
            points: 10,
            kind: Default::default(),
        };
        assert_eq!(sweep.frequencies(), vec![1e3]);
    }

    #[test]
    fn frequencies_empty_on_a_degenerate_sweep() {
        let sweep = AcSweep {
            fstart: 0.0,
            fstop: 100.0,
            points: 10,
            kind: Default::default(),
        };
        assert!(sweep.frequencies().is_empty());
    }

    /// A model's own `ac_stim` drives the circuit through [`StampSink::excitation`], with no
    /// netlist `AC` source anywhere — the path a behavioral Verilog-A model takes.
    ///
    /// The sign is the point of this test, and it is easy to get backwards. `I(p,n) <+ ac_stim`
    /// pushes current **out of** `p`, so at DC an ideal 1 A stimulus across a 1 kΩ resistor to
    /// ground holds that node at **−1000 V**, not +1000: the nodal equation is `V/R + I = 0`.
    /// The excitation arrives in `residual`'s sign convention and [`run`] moves it across the
    /// equals sign, so getting this right is what makes a compiled model's stimulus agree with a
    /// netlist source's.
    #[test]
    fn a_model_supplied_ac_stim_drives_the_circuit_with_the_residual_sign_convention() {
        /// A 1 A stimulus in parallel with a resistor — the `I(p,n) <+ V(p,n)/R + ac_stim(1,0)`
        /// a compiled model produces, written directly against Interface β.
        struct StimResistor {
            terminals: [usize; 2],
            r: f64,
        }

        impl ModelInstance for StimResistor {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn load(
                &self,
                x: &[f64],
                ctx: &va_abi::AnalysisCtx,
                st: &mut va_abi::ModelState,
                sink: &mut dyn StampSink,
            ) {
                let [p, n] = self.terminals;
                Resistor::new(p, n, self.r).load(x, ctx, st, sink);
                if ctx.kind == va_abi::AnalysisKind::Ac {
                    sink.excitation(p, 1.0, 0.0);
                    sink.excitation(n, -1.0, 0.0);
                }
            }
        }

        let (r, c) = (1000.0, 1e-9);
        let stim = StimResistor {
            terminals: [0, GROUND],
            r,
        };
        let cap = Capacitor::new(0, GROUND, c);
        let insts: [&dyn ModelInstance; 2] = [&stim, &cap];

        let sweep = AcSweep {
            fstart: 1e3,
            fstop: 1e6,
            points: 4,
            kind: Default::default(),
        };
        // No netlist excitation at all: everything driving this circuit comes from the model.
        let resp = run(&insts, &[0.0], 1, sweep, &[(0.0, 0.0)]).expect("sweeps");

        // A 1 A source into R‖C: V(jω) = −1 · R/(1 + jωRC).
        for (&f, x) in resp.f.iter().zip(&resp.x) {
            let wrc = 2.0 * PI * f * r * c;
            let expected_mag = r / (1.0 + wrc * wrc).sqrt();
            // Negative real gain rotated by −atan(ωRC): phase = π − atan(ωRC), wrapped.
            let expected_phase = {
                let p = PI - libm::atan(wrc);
                if p > PI {
                    p - 2.0 * PI
                } else {
                    p
                }
            };
            assert!(
                (magnitude(x[0]) - expected_mag).abs() / expected_mag < 1e-9,
                "at {f} Hz: |V| {} vs {expected_mag}",
                magnitude(x[0])
            );
            assert!(
                (phase(x[0]) - expected_phase).abs() < 1e-9,
                "at {f} Hz: phase {} vs {expected_phase}",
                phase(x[0])
            );
        }

        // And it really is analysis-gated: the same instance excites nothing in a DC load.
        let mut dc = va_abi::stamps::DenseStamp::new(1);
        stim.load(
            &[0.0],
            &va_abi::ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut dc,
        );
        assert_eq!(dc.excitation[0], (0.0, 0.0));
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
            points: 5,
            kind: Default::default(),
        };
        // 1V-AC excitation on the source's own branch row; zero everywhere else.
        let excitation = [(0.0, 0.0), (0.0, 0.0), (1.0, 0.0)];

        let response = run(&insts, &x_dc, 3, sweep, &excitation).expect("solves at every point");

        for (&f, x) in response.f.iter().zip(&response.x) {
            let omega = 2.0 * PI * f;
            let wrc = omega * r * cap;
            let expected_mag = 1.0 / (1.0 + wrc * wrc).sqrt();
            let expected_phase = -libm::atan(wrc);

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

    /// Dense and sparse agree at every frequency, to rounding. They factor the identical real
    /// embedding, so the tolerance is tight.
    fn assert_same_response(dense: &AcResponse, sparse: &AcResponse) {
        assert_eq!(dense.f, sparse.f);
        for (k, (xd, xs)) in dense.x.iter().zip(&sparse.x).enumerate() {
            let scale = xd.iter().fold(1e-30_f64, |m, &v| m.max(magnitude(v)));
            for (i, (&d, &s)) in xd.iter().zip(xs).enumerate() {
                let diff = magnitude((d.0 - s.0, d.1 - s.1));
                assert!(
                    diff <= 1e-12 * scale,
                    "f={} x[{i}]: dense {d:?}, sparse {s:?}",
                    dense.f[k]
                );
            }
        }
    }

    #[test]
    fn sparse_ac_matches_dense_on_the_rc_lowpass() {
        let vs = VSource::new(0, GROUND, 2, 5.0);
        let res = Resistor::new(0, 1, 1000.0);
        let capacitor = Capacitor::new(1, GROUND, 1e-6);
        let insts: [&dyn ModelInstance; 3] = [&vs, &res, &capacitor];
        let sweep = AcSweep::dec(1.0, 1e6, 5);
        let exc = [(0.0, 0.0), (0.0, 0.0), (1.0, 0.0)];
        let x_dc = [5.0, 5.0, 0.0];
        let dense = run_with(&insts, &x_dc, 3, sweep, &exc, Solver::Dense).expect("dense");
        let sparse = run_with(&insts, &x_dc, 3, sweep, &exc, Solver::Sparse).expect("sparse");
        assert_same_response(&dense, &sparse);
    }

    /// A transconductance makes `G` non-symmetric and a coupling capacitor puts `C` off the
    /// diagonal: every block of the embedding is exercised, in both orientations.
    #[test]
    fn sparse_ac_matches_dense_on_a_nonsymmetric_system() {
        use va_abi::reference::controlled::Vccs;
        // 0 = in, 1 = mid, 2 = out, 3 = source branch.
        let vs = VSource::new(0, GROUND, 3, 0.0);
        let r1 = Resistor::new(0, 1, 1e3);
        let c1 = Capacitor::new(1, GROUND, 1e-9);
        let gm = Vccs::new(2, GROUND, 1, GROUND, 5e-3);
        let r2 = Resistor::new(2, GROUND, 2e3);
        let cc = Capacitor::new(1, 2, 2e-10);
        let insts: [&dyn ModelInstance; 6] = [&vs, &r1, &c1, &gm, &r2, &cc];
        let sweep = AcSweep::dec(10.0, 1e8, 4);
        let exc = [(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (1.0, 0.0)];
        let x_dc = [0.0; 4];
        let dense = run_with(&insts, &x_dc, 4, sweep, &exc, Solver::Dense).expect("dense");
        let sparse = run_with(&insts, &x_dc, 4, sweep, &exc, Solver::Sparse).expect("sparse");
        assert_same_response(&dense, &sparse);
    }

    /// The per-frequency path: a conductance that moves with frequency forces re-linearization
    /// at every point. The sparse path keeps its pattern and embedding across points (the
    /// pattern does not grow), and still matches dense.
    #[test]
    fn sparse_ac_matches_dense_when_the_circuit_is_frequency_dependent() {
        struct Rising {
            terminals: [usize; 2],
        }
        impl ModelInstance for Rising {
            fn unknowns(&self) -> &[usize] {
                &self.terminals
            }
            fn is_frequency_dependent(&self) -> bool {
                true
            }
            fn load(
                &self,
                _x: &[f64],
                ctx: &va_abi::AnalysisCtx,
                _st: &mut va_abi::ModelState,
                sink: &mut dyn StampSink,
            ) {
                let g = 1e-3 * (1.0 + ctx.freq / 1e3);
                let [p, n] = self.terminals;
                sink.jacobian(p, p, g);
                sink.jacobian(p, n, -g);
                sink.jacobian(n, p, -g);
                sink.jacobian(n, n, g);
            }
        }
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1e3);
        let load = Rising {
            terminals: [1, GROUND],
        };
        let c = Capacitor::new(1, GROUND, 1e-7);
        let insts: [&dyn ModelInstance; 4] = [&vs, &r, &load, &c];
        let sweep = AcSweep::dec(1.0, 1e6, 3);
        let exc = [(0.0, 0.0), (0.0, 0.0), (1.0, 0.0)];
        let x_dc = [1.0, 0.5, 0.0];
        let dense = run_with(&insts, &x_dc, 3, sweep, &exc, Solver::Dense).expect("dense");
        let sparse = run_with(&insts, &x_dc, 3, sweep, &exc, Solver::Sparse).expect("sparse");
        assert_same_response(&dense, &sparse);
    }

    /// Above the threshold `Auto` is the sparse path: a 500-section RC ladder (501 unknowns)
    /// under `Auto` is bit-for-bit the forced-sparse sweep. Agreement with dense is the tests
    /// above; comparing at this size too would factor a dense 1002 × 1002 embedding per point in
    /// a debug build.
    #[test]
    fn auto_above_the_threshold_is_the_sparse_path_in_ac() {
        let n = 500;
        let vs = VSource::new(0, GROUND, n, 0.0);
        let rs: Vec<Resistor> = (0..n - 1).map(|i| Resistor::new(i, i + 1, 100.0)).collect();
        let cs: Vec<Capacitor> = (1..n).map(|i| Capacitor::new(i, GROUND, 1e-9)).collect();
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        insts.extend(rs.iter().map(|r| r as &dyn ModelInstance));
        insts.extend(cs.iter().map(|c| c as &dyn ModelInstance));
        let dim = n + 1;
        assert!(Solver::Auto.uses_sparse(dim));
        let mut exc = vec![(0.0, 0.0); dim];
        exc[n] = (1.0, 0.0);
        let x_dc = vec![0.0; dim];
        let sweep = AcSweep::dec(1e3, 1e6, 2);
        let auto = run(&insts, &x_dc, dim, sweep, &exc).expect("auto");
        let sparse = run_with(&insts, &x_dc, dim, sweep, &exc, Solver::Sparse).expect("sparse");
        assert_eq!(auto.x, sparse.x);
    }
}
