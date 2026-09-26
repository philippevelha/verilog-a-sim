//! A block-triangular (BTF) solve for sparse DC systems — Step 3 of
//! `docs/proposals/btf-solver.md`.
//!
//! By value, a DC Jacobian of a logic circuit is almost block triangular: a gate's output depends
//! on its inputs, not the reverse, so the matrix permutes into thousands of small diagonal blocks
//! (c432: 12 597 blocks, the largest 34 unknowns) with every other entry used once, in
//! substitution. Factoring those blocks densely costs about a millisecond on c432 where `faer`'s
//! general sparse LU takes about 30 ms (proposal §6.1).
//!
//! **How.** [`BtfSolver`] keeps, for the current [`crate::sparse::Pattern`], the **running union**
//! of every entry that has been numerically nonzero in a solve so far. The blocks come from that
//! union — a maximum transversal, then the strongly connected components of the permuted graph
//! (Duff–Reid; what KLU does first) — and are recomputed only when the union grows. Measured on
//! c432's real DC solve, the nonzero set changes on almost every other Newton iteration, but the
//! union stops growing after the second (proposal §6.1), so the analysis runs once or twice per
//! solve. Each solve then fills every block from the current values, factors it densely with
//! partial pivoting, and substitutes in block order.
//!
//! **When it steps aside** — returning `None`, so [`crate::sparse::SparseLu`] uses `faer` for that
//! solve and the caller sees no difference in what "failed" means:
//! - the union has no full transversal (structurally singular as far as the union knows);
//! - its largest block exceeds [`MAX_BLOCK`] (a circuit with feedback, or a transient companion
//!   matrix — dense factorization of a large block would be slower than `faer`);
//! - a block is numerically singular this time — an entry of the union can be exactly zero in one
//!   iteration and leave a block singular where the whole matrix is not.
//!
//! **Answers.** Exact entries only are dropped (`0.0` in this matrix), so nothing is approximated,
//! but the elimination order differs from `faer`'s and results move in the last digits. The
//! order is fixed by the pattern and the values, never by the thread count.
//!
//! **Limitations, stated:** serial — the blocks on one level of the block DAG could be factored in
//! parallel, but on c432 those levels are mostly single unknowns; a block that grows past
//! [`MAX_BLOCK`] sends the whole matrix to `faer`, not just that block.

use crate::sparse::SparseMatrix;

/// Largest diagonal block the dense factorization takes; a larger one sends the solve to `faer`.
///
/// Dense LU costs `s³/3` per block: at 64 that is ~87 000 flops, well under `faer`'s per-solve
/// overhead on the matrices where BTF helps (c432's largest block is 34). A circuit with a block
/// this large is not the near-triangular case this module exists for.
pub const MAX_BLOCK: usize = 64;

/// The BTF state kept across solves; see the module documentation.
#[derive(Default)]
pub(crate) struct BtfSolver {
    /// Identity of the pattern the union and the plan refer to.
    pattern_id: Option<u64>,
    /// Per slot of that pattern: nonzero in some solve so far.
    union: Vec<bool>,
    /// The block plan for the current union; `None` inside `Some` when BTF does not apply to it.
    plan: Option<Option<Plan>>,
    /// Block plans computed.
    pub(crate) analyses: usize,
    /// Solves answered by the block path — counted by [`crate::sparse::SparseLu`], which runs
    /// the residual check.
    pub(crate) solves: usize,
    /// Solves handed to `faer` — likewise counted there.
    pub(crate) fallbacks: usize,
}

/// The symbolic half: which unknowns form each block, and in which order to solve them.
struct Plan {
    /// Blocks in solve order (a block's off-block unknowns are all in earlier blocks), each as its
    /// columns.
    blocks: Vec<Vec<usize>>,
    /// The row the transversal matched to each column.
    row_of_col: Vec<usize>,
    /// Column → (block, position in the block).
    place: Vec<(usize, usize)>,
    /// Per row: `(column, slot)` of each union entry in it.
    row_entries: Vec<Vec<(usize, usize)>>,
}

impl BtfSolver {
    /// Solve `a · x = b` by blocks, or `None` to have the caller use `faer` instead. Does not
    /// check the residual; the caller does, as for every path.
    pub(crate) fn try_solve(&mut self, a: SparseMatrix<'_>, b: &[f64]) -> Option<Vec<f64>> {
        let values = a.values();
        if self.pattern_id != Some(a.pattern_id()) || self.union.len() != values.len() {
            self.pattern_id = Some(a.pattern_id());
            self.union = vec![false; values.len()];
            self.plan = None;
        }
        let mut grew = false;
        for (u, v) in self.union.iter_mut().zip(values) {
            if *v != 0.0 && !*u {
                *u = true;
                grew = true;
            }
        }
        if grew || self.plan.is_none() {
            self.plan = Some(Plan::new(a, &self.union));
            self.analyses += 1;
        }
        self.plan
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|plan| plan.solve(values, b))
    }
}

impl Plan {
    /// Analyse the union entries of `a`'s pattern. `None` if the union has no full transversal
    /// or a block exceeds [`MAX_BLOCK`].
    fn new(a: SparseMatrix<'_>, union: &[bool]) -> Option<Self> {
        let n = a.dim();
        let entries: Vec<(usize, usize, usize)> = a
            .pattern()
            .entries()
            .enumerate()
            .filter(|(slot, _)| union[*slot])
            .map(|(slot, (r, c))| (r, c, slot))
            .collect();
        let mut col_rows: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(r, c, _) in &entries {
            col_rows[c].push(r);
        }
        let row_of_col = max_transversal(n, &col_rows)?;
        let mut col_of_row = vec![usize::MAX; n];
        for (c, &r) in row_of_col.iter().enumerate() {
            col_of_row[r] = c;
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(r, c, _) in &entries {
            let i = col_of_row[r];
            if i != c {
                adj[i].push(c);
            }
        }
        let comp = tarjan(n, &adj);
        let nblocks = comp.iter().copied().max().map_or(0, |m| m + 1);
        // Tarjan numbers a component after every component it reaches, so increasing number is
        // a valid solve order.
        let mut blocks: Vec<Vec<usize>> = vec![Vec::new(); nblocks];
        for (c, &blk) in comp.iter().enumerate() {
            blocks[blk].push(c);
        }
        if blocks.iter().any(|b| b.len() > MAX_BLOCK) {
            return None;
        }
        let mut place = vec![(0, 0); n];
        for (blk, cols) in blocks.iter().enumerate() {
            for (k, &c) in cols.iter().enumerate() {
                place[c] = (blk, k);
            }
        }
        let mut row_entries: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
        for &(r, c, slot) in &entries {
            row_entries[r].push((c, slot));
        }
        Some(Self {
            blocks,
            row_of_col,
            place,
            row_entries,
        })
    }

    /// The numeric half. `None` if a block is numerically singular.
    fn solve(&self, values: &[f64], b: &[f64]) -> Option<Vec<f64>> {
        let mut x = vec![0.0; self.place.len()];
        let mut m_a: Vec<f64> = Vec::new();
        let mut m_b: Vec<f64> = Vec::new();
        for (blk, cols) in self.blocks.iter().enumerate() {
            let m = cols.len();
            m_a.clear();
            m_a.resize(m * m, 0.0);
            m_b.clear();
            m_b.resize(m, 0.0);
            for (i, &c) in cols.iter().enumerate() {
                let row = self.row_of_col[c];
                let mut rhs = b[row];
                for &(col, slot) in &self.row_entries[row] {
                    let (cb, j) = self.place[col];
                    if cb == blk {
                        m_a[i * m + j] += values[slot];
                    } else {
                        rhs -= values[slot] * x[col];
                    }
                }
                m_b[i] = rhs;
            }
            dense_solve(m, &mut m_a, &mut m_b)?;
            for (i, &c) in cols.iter().enumerate() {
                x[c] = m_b[i];
            }
        }
        Some(x)
    }
}

/// Dense LU with partial pivoting on row-major `a` (`m × m`), solving in place: `b` becomes `x`.
/// `None` on a zero pivot.
fn dense_solve(m: usize, a: &mut [f64], b: &mut [f64]) -> Option<()> {
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

/// A row for each column such that every column has one (augmenting paths by iterative DFS).
/// `None` when no perfect matching exists.
fn max_transversal(n: usize, col_rows: &[Vec<usize>]) -> Option<Vec<usize>> {
    let mut col_of_row = vec![usize::MAX; n];
    let mut row_of_col = vec![usize::MAX; n];
    for c in 0..n {
        if let Some(&r) = col_rows[c].iter().find(|&&r| col_of_row[r] == usize::MAX) {
            col_of_row[r] = c;
            row_of_col[c] = r;
        }
    }
    let mut visited = vec![usize::MAX; n];
    for start in 0..n {
        if row_of_col[start] != usize::MAX {
            continue;
        }
        // Frames of (column, next row index); `via[d]` is the row taken at depth `d`.
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        let mut via: Vec<usize> = Vec::new();
        let mut found = false;
        while let Some(&mut (c, ref mut k)) = stack.last_mut() {
            if *k >= col_rows[c].len() {
                stack.pop();
                via.pop();
                continue;
            }
            let r = col_rows[c][*k];
            *k += 1;
            if visited[r] == start {
                continue;
            }
            visited[r] = start;
            via.push(r);
            if col_of_row[r] == usize::MAX {
                found = true;
                break;
            }
            stack.push((col_of_row[r], 0));
        }
        if !found {
            return None;
        }
        for (d, &(c, _)) in stack.iter().enumerate() {
            let r = via[d];
            col_of_row[r] = c;
            row_of_col[c] = r;
        }
    }
    Some(row_of_col)
}

/// Tarjan's strongly connected components, iterative; each node's component number.
fn tarjan(n: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![UNSEEN; n];
    let mut stack: Vec<usize> = Vec::new();
    let (mut next_index, mut next_comp) = (0usize, 0usize);
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut k)) = call.last_mut() {
            if *k < adj[v].len() {
                let w = adj[v][*k];
                *k += 1;
                if index[w] == UNSEEN {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp[w] = next_comp;
                    if w == v {
                        break;
                    }
                }
                next_comp += 1;
            }
        }
    }
    comp
}

#[cfg(test)]
mod tests {
    use crate::sparse::{Pattern, SparseLu, SparseMatrix};
    use crate::CoreError;

    /// `(pattern, values)` from `(row, col, value)` triples; unlisted diagonal slots hold 0.0.
    fn matrix(n: usize, triples: &[(usize, usize, f64)]) -> (Pattern, Vec<f64>) {
        let p = Pattern::new(n, triples.iter().map(|&(r, c, _)| (r, c)));
        let mut v = vec![0.0; p.nnz()];
        for &(r, c, x) in triples {
            v[p.slot(r, c).expect("stored")] += x;
        }
        (p, v)
    }

    /// A lower-triangular-by-blocks system: a chain of 2×2 blocks, each coupled one way to the
    /// previous one — the shape of a DC inverter chain.
    fn chain(blocks: usize) -> Vec<(usize, usize, f64)> {
        let mut t = Vec::new();
        for k in 0..blocks {
            let (i, j) = (2 * k, 2 * k + 1);
            t.extend([
                (i, i, 3.0 + k as f64 * 0.1),
                (i, j, 1.0),
                (j, i, -0.5),
                (j, j, 2.0),
            ]);
            if k > 0 {
                t.push((i, i - 1, 0.7));
            }
        }
        t
    }

    /// Agrees with `faer` to rounding, and the block path is what answered.
    #[test]
    fn the_block_path_answers_and_agrees_with_faer() {
        let (p, v) = matrix(40, &chain(20));
        let a = SparseMatrix::new(&p, &v).unwrap();
        let b: Vec<f64> = (0..40).map(|i| 1.0 + i as f64 * 0.25).collect();
        let mut btf = SparseLu::with_btf();
        let x = btf.solve(a, &b).expect("solves");
        let y = SparseLu::new().solve(a, &b).expect("solves");
        for (u, w) in x.iter().zip(&y) {
            assert!((u - w).abs() <= 1e-12 * w.abs().max(1.0), "{u} vs {w}");
        }
        let stats = btf.btf_stats().unwrap();
        assert_eq!((stats.analyses, stats.solves, stats.fallbacks), (1, 1, 0));
    }

    /// A block whose diagonal holds a zero needs a row swap; partial pivoting makes it.
    #[test]
    fn a_zero_on_a_block_diagonal_is_pivoted_around() {
        let (p, v) = matrix(
            3,
            &[
                (0, 1, 2.0),
                (1, 0, 3.0),
                (1, 1, 1.0),
                (2, 2, 4.0),
                (2, 0, 1.0),
            ],
        );
        let a = SparseMatrix::new(&p, &v).unwrap();
        let x_true = [1.0, -2.0, 0.5];
        let b = a.mul_vec(&x_true);
        let mut lu = SparseLu::with_btf();
        let x = lu.solve(a, &b).expect("solves");
        for (u, w) in x.iter().zip(&x_true) {
            assert!((u - w).abs() < 1e-14, "{x:?}");
        }
        assert_eq!(lu.btf_stats().unwrap().solves, 1);
    }

    /// A singular matrix is `Singular` exactly as on the `faer`-only path — the block path
    /// fails, hands over, and `faer` reports it.
    #[test]
    fn a_singular_matrix_fails_the_same_way_as_without_btf() {
        let (p, v) = matrix(2, &[(0, 0, 1.0), (0, 1, 2.0), (1, 0, 2.0), (1, 1, 4.0)]);
        let a = SparseMatrix::new(&p, &v).unwrap();
        let mut lu = SparseLu::with_btf();
        assert!(matches!(lu.solve(a, &[1.0, 1.0]), Err(CoreError::Singular)));
        assert!(matches!(
            SparseLu::new().solve(a, &[1.0, 1.0]),
            Err(CoreError::Singular)
        ));
        assert_eq!(lu.btf_stats().unwrap().fallbacks, 1);
    }

    /// A block larger than `MAX_BLOCK` is left to `faer` — and then the answer is `faer`'s to
    /// the bit, since `faer` computed it.
    #[test]
    fn a_block_too_large_goes_to_faer_unchanged() {
        let n = super::MAX_BLOCK + 6;
        let mut t = Vec::new();
        for i in 0..n {
            t.push((i, i, 4.0));
            t.push((i, (i + 1) % n, -1.0)); // a ring: one strongly connected block of n
        }
        let (p, v) = matrix(n, &t);
        let a = SparseMatrix::new(&p, &v).unwrap();
        let b: Vec<f64> = (0..n).map(|i| (i % 5) as f64 - 2.0).collect();
        let mut lu = SparseLu::with_btf();
        let x = lu.solve(a, &b).expect("solves");
        let y = SparseLu::new().solve(a, &b).expect("solves");
        assert_eq!(x, y);
        let stats = lu.btf_stats().unwrap();
        assert_eq!((stats.solves, stats.fallbacks), (0, 1));
    }

    /// The block plan is recomputed when an entry becomes nonzero for the first time, and only
    /// then — a later zero leaves the union, and the plan, as they were.
    #[test]
    fn the_plan_follows_the_union_of_nonzeros() {
        let t = chain(4);
        let (p, mut v) = matrix(8, &t);
        let slot = p.slot(2, 1).unwrap(); // the coupling into block 1
        let b = vec![1.0; 8];
        let mut lu = SparseLu::with_btf();
        v[slot] = 0.0;
        lu.solve(SparseMatrix::new(&p, &v).unwrap(), &b).unwrap();
        v[slot] = 0.7;
        lu.solve(SparseMatrix::new(&p, &v).unwrap(), &b).unwrap();
        v[slot] = 0.0;
        lu.solve(SparseMatrix::new(&p, &v).unwrap(), &b).unwrap();
        let stats = lu.btf_stats().unwrap();
        assert_eq!(
            stats.analyses, 2,
            "first solve, then the entry's first nonzero"
        );
        assert_eq!(stats.solves, 3);
    }

    /// A whole DC solve of a feed-forward circuit large enough for the sparse path gives the
    /// same operating point with the block path on and off — and its system is one the block
    /// path answers (not one it hands to `faer`, which would make the comparison `faer` against
    /// itself). 150 stages, each a transconductance driven by the previous node into a resistor
    /// and a diode to ground: nothing flows backwards, so every block is one unknown.
    #[test]
    fn a_newton_solve_agrees_with_and_without_btf() {
        use crate::newton::{solve, NewtonConfig};
        use crate::testutil::VSource;
        use va_abi::reference::diode::VT_NOMINAL;
        use va_abi::reference::{Diode, Resistor, Vccs, GROUND};
        use va_abi::ModelInstance;

        let stages = 150;
        let dim = stages + 2; // nodes 0..=stages, then the source's branch
        let vs = VSource::new(0, GROUND, dim - 1, 0.8);
        let gm: Vec<Vccs> = (1..=stages)
            .map(|k| Vccs::new(GROUND, k, k - 1, GROUND, 1.2e-3))
            .collect();
        let rs: Vec<Resistor> = (1..=stages)
            .map(|k| Resistor::new(k, GROUND, 1e3))
            .collect();
        let ds: Vec<Diode> = (1..=stages)
            .map(|k| Diode::new(k, GROUND, 1e-14, 1.0, VT_NOMINAL))
            .collect();
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        for k in 0..stages {
            insts.extend([&gm[k] as &dyn ModelInstance, &rs[k], &ds[k]]);
        }
        let on = solve(&insts, dim, NewtonConfig::default()).expect("converges with btf");
        let off = solve(
            &insts,
            dim,
            NewtonConfig {
                btf: false,
                ..NewtonConfig::default()
            },
        )
        .expect("converges without");
        for (u, w) in on.iter().zip(&off) {
            assert!((u - w).abs() <= 1e-9 * w.abs().max(1e-3), "{u} vs {w}");
        }
        // Stage voltages are real, not all ~0: the diodes are exercised.
        assert!(on[stages].abs() > 0.3, "V(last) = {}", on[stages]);

        let mut sys = crate::sparse::SparseSystem::new(dim);
        let fired = va_abi::FiredEvents::default();
        crate::sparse::assemble_into(&insts, &on, &va_abi::ANALYSIS_DC, &fired, &mut sys);
        let neg_f: Vec<f64> = sys.residual_values().iter().map(|v| -v).collect();
        let mut lu = SparseLu::with_btf();
        lu.solve(sys.jacobian(), &neg_f).expect("solves");
        let stats = lu.btf_stats().unwrap();
        assert_eq!(
            (stats.solves, stats.fallbacks),
            (1, 0),
            "the block path answered"
        );
    }
}
