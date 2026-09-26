//! `cargo xtask btf <deck> [--model <m>]` — how far a circuit's Jacobian decomposes into
//! **block triangular form** (BTF), the measurement `docs/proposals/parallel-assembly.md` §5.1a
//! asks for before a parallel sparse LU is designed.
//!
//! A matrix permuted to block upper triangular form factors as its diagonal blocks alone, each
//! independently; blocks with no path between them can be factored at the same time. Whether
//! that is worth building depends entirely on the numbers this prints: how many blocks, how big
//! the largest, and how deep the dependency chain between them (the longest chain bounds a
//! parallel schedule however many cores there are).
//!
//! The method is the standard one (Duff & Reid; what KLU does before factoring): a **maximum
//! transversal** — a row permutation putting an entry on every diagonal position — then the
//! **strongly connected components** of the directed graph `i → j` for each off-diagonal entry
//! `(i, j)` of the permuted matrix (Tarjan). Each component is one diagonal block.
//!
//! Printed for three patterns of the same circuit, assembled at its DC operating point:
//! - **structural** — every stored entry, including ones a model stamps as an explicit zero;
//! - **DC nonzero** — only entries numerically nonzero at the operating point: what a solver
//!   that dropped exact zeros would see for `.op`/`.dc`;
//! - **transient nonzero** — entries nonzero in the conductance *or* the capacitance matrix,
//!   the pattern of `G + C/h` that every transient step factors;
//! - the last two again as the **union over four bias points** (the operating point, all-zero,
//!   and two perturbed copies of the operating point), to tell an entry that is zero *at the
//!   operating point* from one that is zero *for this circuit*.
//!
//! **Limitations, stated:** four bias points are evidence, not proof, that an entry is zero
//! everywhere; the depth is in blocks, not in work (a chain of tiny blocks is cheap).

use anyhow::{bail, Context, Result};
use va_abi::ModelInstance;

/// Entry point for `cargo xtask btf`.
pub fn btf(args: &[String]) -> Result<()> {
    let deck = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .context("expected a deck: cargo xtask btf <deck> [--model <m>]")?;
    let model = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let (net, compiled) = va_cli::load(deck, model).with_context(|| format!("loading {deck}"))?;
    eprintln!("[xtask] btf: solving {deck}'s operating point …");
    let op = va_cli::solve_dc(&net, &compiled).context("DC operating point")?;
    let (boxed, dim) = va_cli::instances(&net, &compiled)?;
    let insts: Vec<&dyn ModelInstance> = boxed.iter().map(|b| b.as_ref()).collect();

    let fired = va_abi::FiredEvents::new(&insts);
    let mut sys = va_core::sparse::SparseSystem::new(dim);
    va_core::sparse::assemble_into(&insts, &op.x, &va_abi::ANALYSIS_DC, &fired, &mut sys);
    let entries: Vec<(usize, usize)> = sys.pattern().entries().collect();
    let g = sys.jacobian().values().to_vec();
    let c = sys.dcharge().values().to_vec();
    if g.len() != entries.len() || c.len() != entries.len() {
        bail!("value arrays do not match the pattern");
    }

    println!(
        "{deck}: {dim} unknowns, {} instances, {} stored entries",
        insts.len(),
        entries.len()
    );
    let structural: Vec<(usize, usize)> = entries.clone();
    let dc: Vec<(usize, usize)> = entries
        .iter()
        .zip(&g)
        .filter(|(_, v)| **v != 0.0)
        .map(|(e, _)| *e)
        .collect();
    let tran: Vec<(usize, usize)> = entries
        .iter()
        .zip(g.iter().zip(&c))
        .filter(|(_, (gv, cv))| **gv != 0.0 || **cv != 0.0)
        .map(|(e, _)| *e)
        .collect();
    // Is "nonzero" a property of the operating point or of the circuit? Union over other bias
    // points: all-zero, and the operating point scaled by 0.5 and by 1.1 with a small
    // per-unknown offset (so no two nodes sit at exactly the same voltage). The stored pattern
    // itself can grow at another bias — a model may stamp an entry only in some region — so the
    // union is over entries, keyed by position, not over one fixed pattern's slots.
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<(usize, usize), (bool, bool)> = BTreeMap::new();
    let mut absorb = |sys: &va_core::sparse::SparseSystem| {
        let vals = sys.jacobian().values().iter().zip(sys.dcharge().values());
        for (e, (gv, cv)) in sys.pattern().entries().zip(vals) {
            let slot = seen.entry(e).or_insert((false, false));
            slot.0 |= *gv != 0.0;
            slot.1 |= *gv != 0.0 || *cv != 0.0;
        }
    };
    absorb(&sys);
    let biases: Vec<Vec<f64>> = vec![
        vec![0.0; dim],
        op.x.iter()
            .enumerate()
            .map(|(i, v)| 0.5 * v + 1e-3 * (i % 7) as f64)
            .collect(),
        op.x.iter()
            .enumerate()
            .map(|(i, v)| 1.1 * v - 1e-3 * (i % 5) as f64)
            .collect(),
    ];
    for x in &biases {
        let mut other = va_core::sparse::SparseSystem::new(dim);
        va_core::sparse::assemble_into(&insts, x, &va_abi::ANALYSIS_DC, &fired, &mut other);
        absorb(&other);
    }
    let grown = seen.len() - entries.len();
    println!("  (the stored pattern holds {grown} more entries across the four bias points)");
    let structural_union: Vec<(usize, usize)> = seen.keys().copied().collect();
    let dc_union: Vec<(usize, usize)> = seen.iter().filter(|(_, v)| v.0).map(|(e, _)| *e).collect();
    let tran_union: Vec<(usize, usize)> =
        seen.iter().filter(|(_, v)| v.1).map(|(e, _)| *e).collect();
    for (name, pat) in [
        ("structural", &structural),
        ("struct, 4 biases", &structural_union),
        ("DC nonzero", &dc),
        ("DC, 4 biases", &dc_union),
        ("transient nonzero", &tran),
        ("tran, 4 biases", &tran_union),
    ] {
        report(name, dim, pat);
    }
    if args.iter().any(|a| a == "--time-lu") {
        time_lu("stored pattern", dim, &entries, &g, &op.x);
        let nz: Vec<((usize, usize), f64)> = entries
            .iter()
            .zip(&g)
            .filter(|(_, v)| **v != 0.0)
            .map(|(e, v)| (*e, *v))
            .collect();
        let (e2, v2): (Vec<_>, Vec<_>) = nz.into_iter().unzip();
        time_lu("exact zeros dropped", dim, &e2, &v2, &op.x);
    }
    Ok(())
}

/// Print one pattern's BTF summary.
fn report(name: &str, n: usize, entries: &[(usize, usize)]) {
    let Some(blocks) = decompose(n, entries) else {
        println!(
            "  {name:18} {} entries: structurally singular (no full transversal)",
            entries.len()
        );
        return;
    };
    let mut sizes: Vec<usize> = blocks.sizes.clone();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let singletons = sizes.iter().filter(|&&s| s == 1).count();
    let largest = sizes.first().copied().unwrap_or(0);
    let in_largest = largest as f64 / n.max(1) as f64;
    let top: Vec<String> = sizes.iter().take(8).map(usize::to_string).collect();
    println!(
        "  {name:18} {} entries: {} blocks ({singletons} of size 1); largest {largest} \
         ({:.1}% of unknowns); next {}; block-DAG depth {} (widest level {})",
        entries.len(),
        sizes.len(),
        100.0 * in_largest,
        top.get(1..).map_or(String::new(), |t| t.join(", ")),
        blocks.depth,
        blocks.widest,
    );
}

/// A BTF decomposition's summary.
struct Blocks {
    /// Unknowns per diagonal block.
    sizes: Vec<usize>,
    /// Longest chain of blocks in the dependency DAG (1 = all independent).
    depth: usize,
    /// Most blocks on one level of that DAG — how many could be factored at once.
    widest: usize,
}

/// Maximum transversal, then Tarjan SCC on the permuted graph. `None` if structurally singular.
fn decompose(n: usize, entries: &[(usize, usize)]) -> Option<Blocks> {
    // Column adjacency: for each column, the rows holding an entry.
    let mut col_rows: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(r, c) in entries {
        if r < n && c < n {
            col_rows[c].push(r);
        }
    }
    let row_of_col = max_transversal(n, &col_rows)?;
    // With row `row_of_col[c]` moved to position c, entry (r, c') becomes (pos(r), c'), where
    // pos(r) is the column matched to row r. Edge pos(r) -> c' for every entry, off the diagonal.
    let mut col_of_row = vec![usize::MAX; n];
    for (c, &r) in row_of_col.iter().enumerate() {
        col_of_row[r] = c;
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(r, c) in entries {
        if r < n && c < n {
            let i = col_of_row[r];
            if i != c {
                adj[i].push(c);
            }
        }
    }
    let comp = tarjan(n, &adj);
    let nblocks = comp.iter().copied().max().map_or(0, |m| m + 1);
    let mut sizes = vec![0usize; nblocks];
    for &b in &comp {
        sizes[b] += 1;
    }
    // Condensation DAG and its longest path. Tarjan numbers components in reverse topological
    // order (a component is numbered after every component it reaches), so processing in
    // increasing number sees every successor first.
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); nblocks];
    for (i, out) in adj.iter().enumerate() {
        for &j in out {
            if comp[i] != comp[j] {
                succ[comp[i]].push(comp[j]);
            }
        }
    }
    let mut level = vec![1usize; nblocks];
    for b in 0..nblocks {
        let deepest = succ[b].iter().map(|&s| level[s]).max().unwrap_or(0);
        level[b] = deepest + 1;
    }
    let depth = level.iter().copied().max().unwrap_or(0);
    let mut per_level = vec![0usize; depth + 1];
    for &l in &level {
        per_level[l] += 1;
    }
    let widest = per_level.iter().copied().max().unwrap_or(0);
    Some(Blocks {
        sizes,
        depth,
        widest,
    })
}

/// Row matched to each column so every column has one (augmenting paths, iterative DFS — MC21
/// in spirit). `None` when no perfect matching exists.
fn max_transversal(n: usize, col_rows: &[Vec<usize>]) -> Option<Vec<usize>> {
    let mut col_of_row = vec![usize::MAX; n];
    let mut row_of_col = vec![usize::MAX; n];
    // Cheap pass first: take an unmatched row if the column has one.
    for c in 0..n {
        if let Some(&r) = col_rows[c].iter().find(|&&r| col_of_row[r] == usize::MAX) {
            col_of_row[r] = c;
            row_of_col[c] = r;
        }
    }
    let mut visited = vec![usize::MAX; n]; // row -> stamp of the search that visited it
    for start in 0..n {
        if row_of_col[start] != usize::MAX {
            continue;
        }
        // Iterative DFS over (column, next-row-index) frames.
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        let mut via: Vec<usize> = Vec::new(); // row chosen at each depth
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
        // Augment along the path: column stack[d].0 takes row via[d].
        for (d, &(c, _)) in stack.iter().enumerate() {
            let r = via[d];
            col_of_row[r] = c;
            row_of_col[c] = r;
        }
    }
    Some(row_of_col)
}

/// Tarjan's strongly connected components, iterative. Returns each node's component number.
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

/// Time `faer`'s sparse LU on the DC Jacobian `entries`/`values` (the operating point's): the
/// first solve (symbolic + numeric) and the median of five repeats (numeric + triangular only,
/// the per-Newton-iteration cost). Right-hand side `J·x_op`, so the answer is known.
fn time_lu(name: &str, n: usize, entries: &[(usize, usize)], values: &[f64], x: &[f64]) {
    let pattern = va_core::sparse::Pattern::new(n, entries.iter().copied());
    let mut vals = vec![0.0; pattern.nnz()];
    for (e, v) in entries.iter().zip(values) {
        if let Some(k) = pattern.slot(e.0, e.1) {
            vals[k] += *v;
        }
    }
    let Some(a) = va_core::sparse::SparseMatrix::new(&pattern, &vals) else {
        println!("  LU {name:20} pattern/value mismatch");
        return;
    };
    let b = a.mul_vec(x);
    let mut lu = va_core::sparse::SparseLu::new();
    let t0 = std::time::Instant::now();
    let first = lu.solve(a, &b);
    let t_first = t0.elapsed();
    let mut reps: Vec<f64> = (0..5)
        .map(|_| {
            let t = std::time::Instant::now();
            let _ = lu.solve(a, &b);
            t.elapsed().as_secs_f64()
        })
        .collect();
    reps.sort_by(f64::total_cmp);
    let err = first.as_ref().map_or(f64::NAN, |sol| {
        sol.iter()
            .zip(x)
            .map(|(s, t)| (s - t).abs())
            .fold(0.0, f64::max)
    });
    println!(
        "  LU {name:20} {} entries: first solve {:.3} s, repeat (median of 5) {:.3} s,          max |x - x_op| {err:.1e}{}",
        pattern.nnz(),
        t_first.as_secs_f64(),
        reps[2],
        if first.is_err() { " (FAILED)" } else { "" },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block upper triangular matrix with known blocks: {0,1} coupled both ways, {2} alone,
    /// {3,4} coupled, with 0→2 and 2→3 couplings one way only — three blocks, a chain of three.
    /// Rows are listed out of order so the transversal has to find the diagonal.
    #[test]
    fn a_known_block_structure_is_recovered() {
        let entries = [
            (1, 0),
            (0, 1),
            (0, 0),
            (1, 1), // block {0,1}
            (2, 2), // block {2}
            (3, 3),
            (4, 4),
            (3, 4),
            (4, 3), // block {3,4}
            (0, 2),
            (2, 3), // one-way couplings
        ];
        let b = decompose(5, &entries).expect("nonsingular");
        let mut sizes = b.sizes.clone();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 2, 2]);
        assert_eq!(b.depth, 3);
        assert_eq!(b.widest, 1);
    }

    /// A permutation matrix needs the transversal to move every row; it is n blocks of one,
    /// all independent.
    #[test]
    fn an_anti_diagonal_is_n_independent_blocks() {
        let n = 6;
        let entries: Vec<(usize, usize)> = (0..n).map(|i| (n - 1 - i, i)).collect();
        let b = decompose(n, &entries).expect("nonsingular");
        assert_eq!(b.sizes.len(), n);
        assert_eq!((b.depth, b.widest), (1, n));
    }

    /// An empty column has no transversal.
    #[test]
    fn a_structurally_singular_matrix_is_reported() {
        assert!(decompose(3, &[(0, 0), (1, 1), (2, 1)]).is_none());
    }
}
