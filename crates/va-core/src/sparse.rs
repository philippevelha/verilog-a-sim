//! Sparse MNA assembly and linear solve — Step 1 of `docs/proposals/sparse-solve.md`.
//!
//! **Not wired into any analysis yet.** [`crate::newton`], `va-transient` and `va-acnoise` still
//! assemble into dense buffers and call [`crate::linsolve::solve_dense`]; Steps 2–4 of the
//! proposal switch them over one at a time, above [`SPARSE_THRESHOLD`] unknowns. What this
//! module provides is everything those steps need:
//!
//! - [`SparseSystem`], a [`StampSink`] that stores the Jacobian and the charge Jacobian over one
//!   fixed [`Pattern`] and never allocates a `dim × dim` buffer. Models are unchanged: a sink
//!   only ever receives `(row, col, value)` calls, so how it stores them is its own business
//!   (which is why this needs no Interface β change).
//! - [`SparseLu`], `faer`'s pure-Rust sparse LU with the symbolic factorization computed once
//!   per pattern and reused for every numeric factorization after it.
//! - [`Solver`], the dense/sparse choice, with the threshold as one named constant.
//!
//! **How the pattern is found.** The first assembly into a fresh [`SparseSystem`] has an empty
//! pattern, so every stamp lands in an overflow map; [`SparseSystem::finish`] then builds the
//! pattern from what was stamped. Later assemblies write straight into their slots. A stamp at
//! an entry the pattern has never seen — a Verilog-A contribution inside an `if` that was not
//! taken before, or an `@(above)` body that has just fired — takes the same overflow route, and
//! `finish` grows the pattern to the union. A grown pattern is a new pattern, with a new
//! identity, so [`SparseLu`] redoes the symbolic factorization for it exactly once.
//!
//! **Rules the pattern follows**, each of which the dense code never needed:
//! - an entry stamped with `0.0` is in the pattern (a value that is zero at one operating point
//!   can be nonzero at the next; the dense code's `nnz` treats an exact zero as absent);
//! - every diagonal entry is in the pattern, because [`SparseSystem::shunt_gmin`] writes the
//!   diagonal of every `Node` row and the `gmin` rescue must not fail for want of a slot;
//! - the Jacobian and the charge Jacobian share one pattern (the union of both), so the
//!   transient companion matrix `J + coeff·dQ` is one pass over the values
//!   ([`SparseSystem::companion`]), not a new matrix.
//!
//! **Limitations, stated:** the per-stamp slot lookup is a hash lookup — the simple version the
//! proposal names; a per-instance slot cache is the fast version, to be built only if Step 5's
//! profile says the hash is where the time goes. The sink records `bound_step` (transient,
//! Step 3) the way `va_abi::stamps::DenseStamp` does, and ignores `excitation`, which only AC
//! (Step 4) consumes.

use crate::linsolve::RESIDUAL_TOL;
use crate::CoreError;
use faer::prelude::*;
use faer::sparse::linalg::solvers::{Lu, SymbolicLu};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMatRef};
use faer::Mat;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use va_abi::stamps::StampSink;
use va_abi::{AnalysisCtx, FiredEvents, ModelInstance, ModelState, UnknownKind};

/// The number of unknowns at and above which [`Solver::Auto`] chooses the sparse path.
///
/// 100, decided on 2026-09-23 from Step 5's measurement (`docs/proposals/sparse-solve.md` §7,
/// `docs/validation.md` "The circuit-size limit"). Whole analyses cross over between ~20 and ~50
/// unknowns on an RC ladder and an RC mesh; at ~100 sparse is 1.9–5× faster on the mesh and 4–48×
/// on the ladder, so 100 sits clear of the crossover and its run-to-run noise, and no validation
/// gate (the largest is 8 unknowns) reaches it. It was 500 from 1.6.0 to 1.9.0, a starting value
/// chosen before the sparse path could be measured whole.
pub const SPARSE_THRESHOLD: usize = 100;

/// Which linear solver a circuit uses. Chosen once per circuit, so every analysis of one run
/// uses the same one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Solver {
    /// Dense LU ([`crate::linsolve::solve_dense`]) regardless of size.
    Dense,
    /// Sparse LU ([`SparseLu`]) regardless of size. What lets the golden gates, all far below
    /// the threshold, also be run on the sparse path.
    Sparse,
    /// Dense below [`SPARSE_THRESHOLD`] unknowns, sparse from it.
    #[default]
    Auto,
}

impl Solver {
    /// Whether a circuit of `dim` unknowns is solved with the sparse path.
    pub fn uses_sparse(self, dim: usize) -> bool {
        match self {
            Solver::Dense => false,
            Solver::Sparse => true,
            Solver::Auto => dim >= SPARSE_THRESHOLD,
        }
    }
}

/// Source of [`Pattern`] identities. A pattern is never mutated, only replaced, so a fresh id per
/// construction is exactly "same id ⇔ same pattern", which is all [`SparseLu`]'s cache needs.
static NEXT_PATTERN_ID: AtomicU64 = AtomicU64::new(0);

/// A fixed sparsity pattern for a `dim × dim` matrix: compressed columns with rows sorted
/// within each column, the diagonal always present, and a map from `(row, col)` to the entry's
/// slot in a value array.
#[derive(Debug)]
pub struct Pattern {
    id: u64,
    dim: usize,
    col_ptr: Vec<usize>,
    row_idx: Vec<usize>,
    slots: HashMap<(usize, usize), usize>,
}

impl Pattern {
    /// The pattern holding every diagonal entry plus `entries`. Duplicates are merged and
    /// entries outside `dim × dim` (the ground sentinel, for one) are dropped.
    pub fn new(dim: usize, entries: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut cols: Vec<Vec<usize>> = (0..dim).map(|c| vec![c]).collect();
        for (r, c) in entries {
            if r < dim && c < dim {
                cols[c].push(r);
            }
        }
        let mut col_ptr = Vec::with_capacity(dim + 1);
        let mut row_idx = Vec::new();
        let mut slots = HashMap::new();
        col_ptr.push(0);
        for (c, rows) in cols.iter_mut().enumerate() {
            rows.sort_unstable();
            rows.dedup();
            for &r in rows.iter() {
                slots.insert((r, c), row_idx.len());
                row_idx.push(r);
            }
            col_ptr.push(row_idx.len());
        }
        Self {
            id: NEXT_PATTERN_ID.fetch_add(1, Ordering::Relaxed),
            dim,
            col_ptr,
            row_idx,
            slots,
        }
    }

    /// The matrix dimension.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The number of stored entries, structural zeros included.
    pub fn nnz(&self) -> usize {
        self.row_idx.len()
    }

    /// The slot of entry `(row, col)` in a value array over this pattern, if it is stored.
    pub fn slot(&self, row: usize, col: usize) -> Option<usize> {
        self.slots.get(&(row, col)).copied()
    }

    /// Every stored `(row, col)`, column by column.
    pub fn entries(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.dim).flat_map(move |c| {
            self.row_idx[self.col_ptr[c]..self.col_ptr[c + 1]]
                .iter()
                .map(move |&r| (r, c))
        })
    }

    fn symbolic(&self) -> SymbolicSparseColMatRef<'_, usize> {
        SymbolicSparseColMatRef::new_checked(self.dim, self.dim, &self.col_ptr, None, &self.row_idx)
    }
}

/// A borrowed sparse matrix: a [`Pattern`] and one value per stored entry.
#[derive(Clone, Copy, Debug)]
pub struct SparseMatrix<'a> {
    pattern: &'a Pattern,
    values: &'a [f64],
}

impl<'a> SparseMatrix<'a> {
    /// Pair `values` (one per entry of `pattern`, in slot order) with `pattern`, or `None` if
    /// there is not exactly one value per entry.
    pub fn new(pattern: &'a Pattern, values: &'a [f64]) -> Option<Self> {
        (values.len() == pattern.nnz()).then_some(Self { pattern, values })
    }

    /// The matrix dimension.
    pub fn dim(&self) -> usize {
        self.pattern.dim
    }

    /// Entry `(row, col)`, or `0.0` where the pattern stores nothing.
    pub fn get(&self, row: usize, col: usize) -> f64 {
        self.pattern.slot(row, col).map_or(0.0, |s| self.values[s])
    }

    /// The stored values, one per entry of the pattern, in slot order.
    pub fn values(&self) -> &'a [f64] {
        self.values
    }

    /// `A · x`.
    pub fn mul_vec(&self, x: &[f64]) -> Vec<f64> {
        let mut y = vec![0.0; self.dim()];
        for (c, &xc) in x.iter().enumerate().take(self.dim()) {
            for k in self.pattern.col_ptr[c]..self.pattern.col_ptr[c + 1] {
                y[self.pattern.row_idx[k]] += self.values[k] * xc;
            }
        }
        y
    }
}

/// The assembled MNA system for `dim` unknowns, stored sparse: residual, charge, and the
/// Jacobian and charge Jacobian over one shared [`Pattern`]. The sparse counterpart of
/// [`crate::mna::System`] (DC) and `va_abi::stamps::DenseStamp` (transient).
///
/// Reuse one across Newton iterations and timesteps — [`assemble_into`] clears it, lets every
/// instance stamp, and calls [`Self::finish`] — so the pattern is discovered once.
#[derive(Debug)]
pub struct SparseSystem {
    pattern: Pattern,
    residual: Vec<f64>,
    charge: Vec<f64>,
    jacobian: Vec<f64>,
    dcharge: Vec<f64>,
    /// Stamps at entries the pattern does not hold yet, as `(jacobian, dcharge)` sums. Empty
    /// between [`Self::finish`] and the next stamp outside the pattern.
    overflow: HashMap<(usize, usize), (f64, f64)>,
    /// The tightest `bound_step` request since the last [`Self::clear`], as
    /// `va_abi::stamps::DenseStamp::bound_step` keeps it.
    bound_step: Option<f64>,
}

impl SparseSystem {
    /// An empty system of `dim` unknowns. Its pattern holds only the diagonal until the first
    /// assembly is [`Self::finish`]ed.
    pub fn new(dim: usize) -> Self {
        let pattern = Pattern::new(dim, std::iter::empty());
        let nnz = pattern.nnz();
        Self {
            pattern,
            residual: vec![0.0; dim],
            charge: vec![0.0; dim],
            jacobian: vec![0.0; nnz],
            dcharge: vec![0.0; nnz],
            overflow: HashMap::new(),
            bound_step: None,
        }
    }

    /// Number of unknowns.
    pub fn dim(&self) -> usize {
        self.pattern.dim
    }

    /// The current pattern.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    /// Zero every value, keeping the pattern, before the next assembly.
    pub fn clear(&mut self) {
        self.residual.iter_mut().for_each(|v| *v = 0.0);
        self.charge.iter_mut().for_each(|v| *v = 0.0);
        self.jacobian.iter_mut().for_each(|v| *v = 0.0);
        self.dcharge.iter_mut().for_each(|v| *v = 0.0);
        self.overflow.clear();
        self.bound_step = None;
    }

    /// Fold any stamps that fell outside the pattern into it, growing the pattern to the union.
    /// Returns whether it grew — `true` on the first assembly, and after that only when a model
    /// stamped an entry it had never stamped before.
    pub fn finish(&mut self) -> bool {
        if self.overflow.is_empty() {
            return false;
        }
        let overflow = std::mem::take(&mut self.overflow);
        let grown = Pattern::new(
            self.dim(),
            self.pattern.entries().chain(overflow.keys().copied()),
        );
        let mut jacobian = vec![0.0; grown.nnz()];
        let mut dcharge = vec![0.0; grown.nnz()];
        for (k, (r, c)) in self.pattern.entries().enumerate() {
            // `entries()` walks the old pattern in slot order, so `k` is the old slot.
            let s = grown.slots[&(r, c)];
            jacobian[s] = self.jacobian[k];
            dcharge[s] = self.dcharge[k];
        }
        for ((r, c), (j, q)) in overflow {
            let s = grown.slots[&(r, c)];
            jacobian[s] += j;
            dcharge[s] += q;
        }
        self.pattern = grown;
        self.jacobian = jacobian;
        self.dcharge = dcharge;
        true
    }

    /// The residual `f(x)`, length `dim`.
    pub fn residual_values(&self) -> &[f64] {
        &self.residual
    }

    /// The charge `Q(x)`, length `dim`.
    pub fn charge_values(&self) -> &[f64] {
        &self.charge
    }

    /// The tightest `bound_step` a model asked for since the last [`Self::clear`], if any.
    pub fn bound_step(&self) -> Option<f64> {
        self.bound_step
    }

    /// The Jacobian `∂f/∂x`. Call after [`Self::finish`]: a stamp still in overflow is not in it.
    pub fn jacobian(&self) -> SparseMatrix<'_> {
        SparseMatrix {
            pattern: &self.pattern,
            values: &self.jacobian,
        }
    }

    /// The charge Jacobian `∂Q/∂x`, over the same pattern as [`Self::jacobian`].
    pub fn dcharge(&self) -> SparseMatrix<'_> {
        SparseMatrix {
            pattern: &self.pattern,
            values: &self.dcharge,
        }
    }

    /// Values of the transient companion matrix `J + coeff · ∂Q/∂x`, over [`Self::pattern`] —
    /// one pass over the values, because both live on one pattern. Pair with the pattern via
    /// [`SparseMatrix::new`].
    pub fn companion(&self, coeff: f64) -> Vec<f64> {
        self.jacobian
            .iter()
            .zip(&self.dcharge)
            .map(|(j, q)| j + coeff * q)
            .collect()
    }

    /// Add a `gmin` shunt from ground to every unknown `kinds` marks [`UnknownKind::Node`] —
    /// the same edit, for the same reasons, as [`crate::mna::System::shunt_gmin`]. Every
    /// diagonal is in the pattern, so this never needs a slot that is not there.
    pub fn shunt_gmin(&mut self, x: &[f64], gmin: f64, kinds: &[UnknownKind]) {
        if gmin <= 0.0 {
            return;
        }
        for (i, &kind) in kinds.iter().enumerate() {
            if kind == UnknownKind::Node {
                self.residual[i] += gmin * x[i];
                let s = self.pattern.slots[&(i, i)];
                self.jacobian[s] += gmin;
            }
        }
    }
}

impl StampSink for SparseSystem {
    fn residual(&mut self, row: usize, value: f64) {
        if row < self.dim() {
            self.residual[row] += value;
        }
    }

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim() && col < self.dim() {
            crate::counters::stamp_lookup();
            match self.pattern.slot(row, col) {
                Some(s) => self.jacobian[s] += value,
                None => self.overflow.entry((row, col)).or_default().0 += value,
            }
        }
    }

    fn charge(&mut self, row: usize, value: f64) {
        if row < self.dim() {
            self.charge[row] += value;
        }
    }

    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim() && col < self.dim() {
            crate::counters::stamp_lookup();
            match self.pattern.slot(row, col) {
                Some(s) => self.dcharge[s] += value,
                None => self.overflow.entry((row, col)).or_default().1 += value,
            }
        }
    }

    /// The same rule as `DenseStamp`: keep the tightest meaningful request, and discard a
    /// non-positive or non-finite one, which the LRM gives no meaning.
    fn bound_step(&mut self, dt: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.bound_step = Some(self.bound_step.map_or(dt, |cur| cur.min(dt)));
        }
    }
}

/// Assemble every instance at `x` into `sys`: clear it, let each instance stamp, and
/// [`SparseSystem::finish`]. The sparse counterpart of [`crate::mna::assemble_with_events`],
/// with the same meaning for `ctx` and `fired`. Returns whether the pattern grew.
pub fn assemble_into(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    fired: &FiredEvents,
    sys: &mut SparseSystem,
) -> bool {
    sys.clear();
    for (i, inst) in instances.iter().enumerate() {
        let mut st = ModelState::with_events(&[], &mut [], fired.slice(i));
        inst.load(x, ctx, &mut st, sys);
    }
    sys.finish()
}

/// Sparse LU that keeps the symbolic factorization of the last pattern it saw.
///
/// Within one circuit the pattern is fixed across Newton iterations, timesteps, sweep points and
/// frequencies, so the symbolic analysis (the fill-reducing column ordering and the elimination
/// structure) is done once and only the numeric factorization is repeated. A different pattern —
/// a grown one, or another circuit's — is recognised by its identity and analysed afresh.
#[derive(Default)]
pub struct SparseLu {
    symbolic: Option<(u64, SymbolicLu<usize>)>,
    symbolic_count: usize,
}

impl SparseLu {
    /// A solver with nothing cached.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many symbolic factorizations this solver has computed — once per distinct pattern
    /// when the reuse works.
    pub fn symbolic_factorizations(&self) -> usize {
        self.symbolic_count
    }

    /// Solve `a · x = b`.
    ///
    /// Guarantees what [`crate::linsolve::solve_dense`] guarantees, so the two paths mean the
    /// same thing by "failed": a non-finite input is [`CoreError::NonFinite`] naming the same
    /// entry the dense check would name (the first in row-major order), and a matrix that is
    /// singular — symbolically, numerically, or near enough that `x` fails to reproduce `b` to
    /// the dense path's tolerance — is [`CoreError::Singular`].
    ///
    /// # Errors
    ///
    /// As above. `faer`'s sparse LU panics on at least one singular input
    /// (`faer-0.22.6/src/sparse/linalg/lu.rs:1426`) rather than returning an error; the panic is
    /// caught here and reported as [`CoreError::Singular`], because CLAUDE.md §5 forbids this
    /// crate from panicking on bad input.
    pub fn solve(&mut self, a: SparseMatrix<'_>, b: &[f64]) -> Result<Vec<f64>, CoreError> {
        let n = a.dim();
        debug_assert_eq!(b.len(), n);
        if n == 0 {
            return Ok(Vec::new());
        }
        check_finite(a, b)?;

        let symbolic = match &self.symbolic {
            Some((id, s)) if *id == a.pattern.id => s.clone(),
            _ => {
                let s = catch(|| SymbolicLu::try_new(a.pattern.symbolic()).ok())?;
                self.symbolic = Some((a.pattern.id, s.clone()));
                self.symbolic_count += 1;
                s
            }
        };

        let x = catch(|| {
            let mat = SparseColMatRef::new(a.pattern.symbolic(), a.values);
            let lu = Lu::try_new_with_symbolic(symbolic, mat).ok()?;
            let rhs = Mat::from_fn(n, 1, |i, _| b[i]);
            let sol = lu.solve(&rhs);
            Some((0..n).map(|i| *sol.get(i, 0)).collect::<Vec<f64>>())
        })?;

        if !x.iter().all(|v| v.is_finite()) {
            return Err(CoreError::Singular);
        }
        let ax = a.mul_vec(&x);
        let bmax = b.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let rmax = ax
            .iter()
            .zip(b)
            .map(|(l, r)| (l - r).abs())
            .fold(0.0_f64, f64::max);
        if rmax > RESIDUAL_TOL * (1.0 + bmax) {
            return Err(CoreError::Singular);
        }
        Ok(x)
    }
}

/// Run `f`, turning both a `None` and a panic inside `faer` into [`CoreError::Singular`].
fn catch<T>(f: impl FnOnce() -> Option<T>) -> Result<T, CoreError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Some(v)) => Ok(v),
        Ok(None) | Err(_) => Err(CoreError::Singular),
    }
}

/// [`crate::check_finite`] for a sparse `a`: the residual first, then the Jacobian entry that
/// comes first in **row-major** order, so both paths name the same entry for the same system.
fn check_finite(a: SparseMatrix<'_>, b: &[f64]) -> Result<(), CoreError> {
    if let Some(row) = b.iter().position(|v| !v.is_finite()) {
        return Err(CoreError::NonFinite { row, col: None });
    }
    let first = a
        .pattern
        .entries()
        .zip(a.values)
        .filter(|(_, v)| !v.is_finite())
        .map(|(rc, _)| rc)
        .min();
    match first {
        Some((row, col)) => Err(CoreError::NonFinite {
            row,
            col: Some(col),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linsolve::solve_dense;
    use crate::mna::{assemble, classify_unknowns};
    use va_abi::reference::{Diode, Resistor, VSource, GROUND};
    use va_abi::ANALYSIS_DC;

    /// Assemble `insts` at `x` both ways and return the dense system and the sparse one.
    fn both(
        insts: &[&dyn ModelInstance],
        x: &[f64],
        dim: usize,
    ) -> (crate::mna::System, SparseSystem) {
        let dense = assemble(insts, x, &ANALYSIS_DC, dim);
        let mut sparse = SparseSystem::new(dim);
        assemble_into(insts, x, &ANALYSIS_DC, &FiredEvents::default(), &mut sparse);
        (dense, sparse)
    }

    /// Solve the Newton step `J · dx = -f` both ways and require agreement.
    fn assert_solves_agree(insts: &[&dyn ModelInstance], x: &[f64], dim: usize) {
        let (dense, sparse) = both(insts, x, dim);
        let neg_f: Vec<f64> = dense.residual.iter().map(|v| -v).collect();
        let want = solve_dense(&dense.jacobian, &neg_f, dim).expect("dense solves");
        let got = SparseLu::new()
            .solve(sparse.jacobian(), &neg_f)
            .expect("sparse solves");
        let scale = want.iter().fold(1.0_f64, |m, v| m.max(v.abs()));
        for (i, (w, g)) in want.iter().zip(&got).enumerate() {
            assert!(
                (w - g).abs() <= 1e-12 * scale,
                "row {i}: dense {w}, sparse {g}"
            );
        }
    }

    /// The `bench-linsolve` ladder: a source into a chain of series resistors, each node
    /// shunted to ground. The source's branch row has a zero on the diagonal.
    fn ladder(n: usize) -> (VSource, Vec<Resistor>) {
        let vs = VSource::new(0, GROUND, n, 5.0);
        let mut rs: Vec<Resistor> = (0..n).map(|i| Resistor::new(i, GROUND, 1e3)).collect();
        rs.extend((0..n - 1).map(|i| Resistor::new(i, i + 1, 100.0)));
        (vs, rs)
    }

    #[test]
    fn solver_threshold_is_100_unknowns() {
        assert!(!Solver::Auto.uses_sparse(SPARSE_THRESHOLD - 1));
        assert!(Solver::Auto.uses_sparse(SPARSE_THRESHOLD));
        assert!(Solver::Sparse.uses_sparse(2));
        assert!(!Solver::Dense.uses_sparse(1_000_000));
        assert_eq!(Solver::default(), Solver::Auto);
        assert_eq!(SPARSE_THRESHOLD, 100);
    }

    #[test]
    fn pattern_sorts_merges_drops_ground_and_keeps_the_diagonal() {
        let p = Pattern::new(3, [(2, 0), (0, 2), (2, 0), (GROUND, 1), (1, GROUND)]);
        let entries: Vec<_> = p.entries().collect();
        assert_eq!(entries, vec![(0, 0), (2, 0), (1, 1), (0, 2), (2, 2)]);
        assert_eq!(p.nnz(), 5);
        assert_eq!(p.slot(2, 0), Some(1));
        assert_eq!(p.slot(1, 0), None);
    }

    /// Assembly through the sparse sink holds exactly the matrix the dense assembler builds.
    #[test]
    fn sparse_assembly_matches_dense_entry_by_entry() {
        let (vs, rs) = ladder(6);
        let d = Diode::new(3, GROUND, 1e-14, 1.0, 0.025852);
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs, &d];
        insts.extend(rs.iter().map(|r| r as &dyn ModelInstance));
        let dim = 7;
        let x: Vec<f64> = (0..dim).map(|i| 0.1 * i as f64).collect();
        let (dense, sparse) = both(&insts, &x, dim);
        for r in 0..dim {
            for c in 0..dim {
                assert_eq!(
                    sparse.jacobian().get(r, c),
                    dense.jacobian[r * dim + c],
                    "({r},{c})"
                );
            }
        }
        assert_eq!(sparse.residual_values(), dense.residual.as_slice());
        // Sparse means sparse: far fewer stored entries than dim², diagonal included.
        assert!(sparse.pattern().nnz() < dim * dim / 2);
    }

    #[test]
    fn sparse_and_dense_agree_on_a_ladder_with_a_zero_diagonal_branch_row() {
        let (vs, rs) = ladder(40);
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        insts.extend(rs.iter().map(|r| r as &dyn ModelInstance));
        assert_solves_agree(&insts, &[0.0; 41], 41);
    }

    /// Several ideal sources in a chain: every branch row has a zero diagonal, so the solve only
    /// works if pivoting looks down the column (`docs/proposals/sparse-solve.md` §3.3).
    #[test]
    fn sparse_and_dense_agree_on_a_chain_of_voltage_sources() {
        // Nodes 0..4, branch rows 5..9. V_k holds V(k) - V(k-1) = 1 (V_0 against ground).
        let sources: Vec<VSource> = (0..5)
            .map(|k| VSource::new(k, if k == 0 { GROUND } else { k - 1 }, 5 + k, 1.0))
            .collect();
        let load = Resistor::new(4, GROUND, 1e3);
        let mut insts: Vec<&dyn ModelInstance> = vec![&load];
        insts.extend(sources.iter().map(|s| s as &dyn ModelInstance));
        assert_solves_agree(&insts, &[0.0; 10], 10);
    }

    #[test]
    fn sparse_and_dense_agree_on_a_nonlinear_jacobian() {
        // Source -> 1 kΩ -> diode to ground, linearized at a forward bias.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r = Resistor::new(0, 1, 1e3);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, 0.025852);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        assert_solves_agree(&insts, &[2.0, 0.62, -1.3e-3], 3);
    }

    /// The whole Newton loop, dense-style but with the sparse sink and solve, reaches the same
    /// operating point `dc::solve` does.
    #[test]
    fn a_newton_loop_on_the_sparse_path_reaches_the_dense_operating_point() {
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r = Resistor::new(0, 1, 1e3);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, 0.025852);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        let want = crate::dc::operating_point(&insts, 3, crate::newton::NewtonConfig::default())
            .expect("dense DC solves")
            .x;

        let mut sys = SparseSystem::new(3);
        let mut lu = SparseLu::new();
        let mut x = vec![0.0, 0.7, 0.0];
        for _ in 0..100 {
            assemble_into(&insts, &x, &ANALYSIS_DC, &FiredEvents::default(), &mut sys);
            let neg_f: Vec<f64> = sys.residual_values().iter().map(|v| -v).collect();
            let dx = lu.solve(sys.jacobian(), &neg_f).expect("solves");
            // Crude diode step limit, enough for this one junction.
            for (xi, di) in x.iter_mut().zip(&dx) {
                *xi += di.clamp(-0.1, 0.1);
            }
            if dx.iter().all(|v| v.abs() < 1e-12) {
                break;
            }
        }
        for (i, (w, g)) in want.iter().zip(&x).enumerate() {
            assert!((w - g).abs() < 1e-9, "x[{i}]: dense {w}, sparse loop {g}");
        }
        assert_eq!(
            lu.symbolic_factorizations(),
            1,
            "one pattern, analysed once"
        );
    }

    /// A model stamping `0.0` still owns that entry: at the next operating point it may not be
    /// zero, and a pattern that dropped it would have to be rebuilt every time it flips.
    #[test]
    fn an_entry_stamped_zero_is_in_the_pattern() {
        let mut sys = SparseSystem::new(2);
        StampSink::jacobian(&mut sys, 0, 1, 0.0);
        sys.finish();
        assert!(sys.pattern().slot(0, 1).is_some());
        assert!(sys.pattern().slot(1, 0).is_none());
    }

    /// A stamp outside the known pattern grows it once, keeps every value, and makes the
    /// solver redo the symbolic factorization exactly once for the grown pattern.
    #[test]
    fn a_new_entry_grows_the_pattern_and_refactors_once() {
        let mut sys = SparseSystem::new(2);
        let mut lu = SparseLu::new();
        let stamp = |sys: &mut SparseSystem, coupled: bool| {
            sys.clear();
            StampSink::jacobian(sys, 0, 0, 2.0);
            StampSink::jacobian(sys, 1, 1, 3.0);
            if coupled {
                StampSink::jacobian(sys, 0, 1, 1.0);
            }
            sys.finish()
        };
        // The diagonal is in every pattern from the start, so stamping only the diagonal
        // finds nothing to add.
        assert!(
            !stamp(&mut sys, false),
            "diagonal stamps fit the initial pattern"
        );
        lu.solve(sys.jacobian(), &[2.0, 3.0]).unwrap();
        assert!(!stamp(&mut sys, false), "the same stamps: no growth");
        lu.solve(sys.jacobian(), &[2.0, 3.0]).unwrap();
        assert_eq!(lu.symbolic_factorizations(), 1);

        assert!(stamp(&mut sys, true), "a new entry grows the pattern");
        assert_eq!(sys.jacobian().get(0, 1), 1.0);
        assert_eq!(sys.jacobian().get(0, 0), 2.0);
        // [2 1; 0 3] x = [3; 3] -> x = [1; 1].
        let x = lu.solve(sys.jacobian(), &[3.0, 3.0]).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-15 && (x[1] - 1.0).abs() < 1e-15);
        assert_eq!(lu.symbolic_factorizations(), 2);

        // The grown pattern stays: dropping the coupling again stores a zero there.
        assert!(!stamp(&mut sys, false));
        assert_eq!(sys.jacobian().get(0, 1), 0.0);
        lu.solve(sys.jacobian(), &[2.0, 3.0]).unwrap();
        assert_eq!(lu.symbolic_factorizations(), 2);
    }

    /// Both paths reject a singular matrix. The assertion is on the property — rejected — not on
    /// which variant; the 1.3.2+1 macOS failure is why.
    #[test]
    fn singular_matrices_are_rejected_like_dense() {
        // Numerically singular: [1 2; 2 4].
        let p = Pattern::new(2, [(0, 1), (1, 0)]);
        let values = [1.0, 2.0, 2.0, 4.0]; // slots: (0,0) (1,0) (0,1) (1,1)
        let a = SparseMatrix::new(&p, &values).unwrap();
        assert!(SparseLu::new().solve(a, &[1.0, 1.0]).is_err());
        assert!(solve_dense(&[1.0, 2.0, 2.0, 4.0], &[1.0, 1.0], 2).is_err());

        // A floating node: an all-zero row and column.
        let p = Pattern::new(2, std::iter::empty());
        let values = [1.0, 0.0];
        let a = SparseMatrix::new(&p, &values).unwrap();
        assert!(SparseLu::new().solve(a, &[1.0, 0.0]).is_err());
        assert!(solve_dense(&[1.0, 0.0, 0.0, 0.0], &[1.0, 0.0], 2).is_err());
    }

    /// `NonFinite` names the same entry on both paths: the residual first, then the first bad
    /// Jacobian entry in row-major order — though the sparse values are stored column by column.
    #[test]
    fn non_finite_names_the_same_entry_as_dense() {
        // Bad entries at (0,1) and (1,0). Column order meets (1,0) first; row-major, (0,1).
        let p = Pattern::new(2, [(0, 1), (1, 0)]);
        let values = [1.0, f64::NAN, f64::INFINITY, 1.0];
        let a = SparseMatrix::new(&p, &values).unwrap();
        let dense = [1.0, f64::INFINITY, f64::NAN, 1.0];
        for (sparse, dense) in [
            (
                SparseLu::new().solve(a, &[1.0, 1.0]),
                solve_dense(&dense, &[1.0, 1.0], 2),
            ),
            (
                SparseLu::new().solve(a, &[1.0, f64::NAN]),
                solve_dense(&dense, &[1.0, f64::NAN], 2),
            ),
        ] {
            match (sparse, dense) {
                (
                    Err(CoreError::NonFinite { row: r1, col: c1 }),
                    Err(CoreError::NonFinite { row: r2, col: c2 }),
                ) => assert_eq!((r1, c1), (r2, c2)),
                other => panic!("expected NonFinite from both, got {other:?}"),
            }
        }
    }

    #[test]
    fn gmin_shunt_skips_branch_rows_like_dense() {
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r = Resistor::new(0, 1, 1e3);
        let insts: [&dyn ModelInstance; 2] = [&vs, &r];
        let kinds = classify_unknowns(&insts, 3);
        let x = [2.0, 1.0, 0.0];
        let (mut dense, mut sparse) = both(&insts, &x, 3);
        dense.shunt_gmin(&x, 1e-3, &kinds);
        sparse.shunt_gmin(&x, 1e-3, &kinds);
        for r in 0..3 {
            for c in 0..3 {
                assert_eq!(sparse.jacobian().get(r, c), dense.jacobian[r * 3 + c]);
            }
        }
        assert_eq!(sparse.residual_values(), dense.residual.as_slice());
    }

    /// The charge channel shares the Jacobian's pattern, and the companion matrix is
    /// `J + coeff·dQ` entry by entry — what the transient integrator builds densely today.
    #[test]
    fn charge_shares_the_pattern_and_the_companion_is_j_plus_coeff_dq() {
        let c = va_abi::reference::Capacitor::new(0, 1, 1e-9);
        let r = Resistor::new(1, GROUND, 1e3);
        let insts: [&dyn ModelInstance; 2] = [&c, &r];
        let x = [1.0, 0.25];
        let tran = va_abi::AnalysisCtx::transient(0.0);
        let mut dense = va_abi::stamps::DenseStamp::new(2);
        for inst in insts {
            inst.load(&x, &tran, &mut ModelState::stateless(), &mut dense);
        }
        let mut sparse = SparseSystem::new(2);
        assemble_into(&insts, &x, &tran, &FiredEvents::default(), &mut sparse);

        assert_eq!(sparse.charge_values(), dense.charge.as_slice());
        let coeff = 2.0 / 1e-6;
        let companion = sparse.companion(coeff);
        let comp = SparseMatrix::new(sparse.pattern(), &companion).unwrap();
        for r in 0..2 {
            for c in 0..2 {
                let want = dense.jacobian[r * 2 + c] + coeff * dense.dcharge[r * 2 + c];
                assert_eq!(comp.get(r, c), want, "({r},{c})");
                assert_eq!(sparse.dcharge().get(r, c), dense.dcharge[r * 2 + c]);
            }
        }
    }

    #[test]
    fn bound_step_keeps_the_tightest_meaningful_request_and_clears() {
        let mut sys = SparseSystem::new(1);
        assert_eq!(sys.bound_step(), None);
        for dt in [1e-6, 1e-3, 0.0, -1.0, f64::NAN, 1e-9] {
            StampSink::bound_step(&mut sys, dt);
        }
        assert_eq!(sys.bound_step(), Some(1e-9));
        sys.clear();
        assert_eq!(sys.bound_step(), None, "a bound belongs to one evaluation");
    }
}
