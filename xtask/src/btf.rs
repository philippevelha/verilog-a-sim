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
    if args.iter().any(|a| a == "--time-block-solve") {
        let nz: Vec<((usize, usize), f64)> = entries
            .iter()
            .zip(&g)
            .filter(|(_, v)| **v != 0.0)
            .map(|(e, v)| (*e, *v))
            .collect();
        time_block_solve(dim, &nz, &op.x)?;
    }
    if args.iter().any(|a| a == "--track-newton") {
        track_newton(&insts, dim)?;
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
    let t0 = std::time::Instant::now();
    let decomposed = decompose(n, entries);
    let btf_time = t0.elapsed();
    let Some(blocks) = decomposed else {
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
    // Dense LU of every diagonal block: Σ s³/3 flops. A proxy for a block solver's
    // factorization work (it ignores the off-block entries, which substitution touches once).
    let work: f64 = sizes.iter().map(|&s| (s as f64).powi(3) / 3.0).sum();
    let multi: usize = sizes.iter().filter(|&&s| s > 1).count();
    let in_multi: usize = sizes.iter().filter(|&&s| s > 1).sum();
    println!(
        "  {:18} {multi} blocks larger than 1 hold {in_multi} unknowns; dense-block LU work Σs³/3 = {work:.2e} flops; BTF itself took {:.1} ms",
        "",
        btf_time.as_secs_f64() * 1e3,
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

// ---------------------------------------------------------------------------------------------
// Step 1 of docs/proposals/btf-solver.md: how often does the nonzero set change during a solve?
// ---------------------------------------------------------------------------------------------

/// Per-assembly Jacobian values, summed over instances, as the tracked solve goes.
#[derive(Default)]
struct Track {
    /// The assembly being collected: `(row, col) -> summed value`.
    current: std::collections::HashMap<(usize, usize), f64>,
    /// Every finished assembly's nonzero set, sorted.
    sets: Vec<Vec<(usize, usize)>>,
}

impl Track {
    fn finish_assembly(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let mut nz: Vec<(usize, usize)> = self
            .current
            .drain()
            .filter(|(_, v)| *v != 0.0)
            .map(|(e, _)| e)
            .collect();
        nz.sort_unstable();
        self.sets.push(nz);
    }
}

/// Forwards every `ModelInstance` method to `inner` — all ten, so the circuit is unchanged —
/// and copies each Jacobian stamp into the shared [`Track`]. Instance 0's `load` starts a new
/// assembly, which is sound because the tracked solve runs with parallel evaluation off, so
/// instances load in order.
struct Tracker<'a> {
    inner: &'a dyn ModelInstance,
    first: bool,
    dim: usize,
    track: &'a std::sync::Mutex<Track>,
}

/// A `StampSink` that forwards to the real sink and records the Jacobian stamps.
struct Tee<'a> {
    sink: &'a mut dyn va_abi::StampSink,
    dim: usize,
    track: &'a mut Track,
}

impl va_abi::StampSink for Tee<'_> {
    fn residual(&mut self, row: usize, value: f64) {
        self.sink.residual(row, value);
    }
    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            *self.track.current.entry((row, col)).or_insert(0.0) += value;
        }
        self.sink.jacobian(row, col, value);
    }
    fn charge(&mut self, row: usize, value: f64) {
        self.sink.charge(row, value);
    }
    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        self.sink.dcharge(row, col, value);
    }
    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        self.sink.excitation(row, re, im);
    }
    fn bound_step(&mut self, dt: f64) {
        self.sink.bound_step(dt);
    }
}

impl ModelInstance for Tracker<'_> {
    fn unknowns(&self) -> &[usize] {
        self.inner.unknowns()
    }
    fn unknown_kind(&self, i: usize) -> va_abi::UnknownKind {
        self.inner.unknown_kind(i)
    }
    fn unknown_is_junction(&self, i: usize) -> bool {
        self.inner.unknown_is_junction(i)
    }
    fn unknown_abstol(&self, i: usize) -> Option<f64> {
        self.inner.unknown_abstol(i)
    }
    fn load(
        &self,
        x: &[f64],
        ctx: &va_abi::AnalysisCtx,
        state: &mut va_abi::ModelState,
        sink: &mut dyn va_abi::StampSink,
    ) {
        let mut track = self.track.lock().unwrap_or_else(|e| e.into_inner());
        if self.first {
            track.finish_assembly();
        }
        let mut tee = Tee {
            sink,
            dim: self.dim,
            track: &mut track,
        };
        self.inner.load(x, ctx, state, &mut tee);
    }
    fn state_len(&self) -> usize {
        self.inner.state_len()
    }
    fn is_frequency_dependent(&self) -> bool {
        self.inner.is_frequency_dependent()
    }
    fn noise(&self, x: &[f64], ctx: &va_abi::AnalysisCtx, sink: &mut dyn va_abi::NoiseSink) {
        self.inner.noise(x, ctx, sink);
    }
    fn event_count(&self) -> usize {
        self.inner.event_count()
    }
    fn events(&self, x: &[f64], ctx: &va_abi::AnalysisCtx, sink: &mut dyn va_abi::EventSink) {
        self.inner.events(x, ctx, sink);
    }
}

/// Re-run the deck's DC solve exactly as `va-cli` does (default Newton configuration, the same
/// rescue ladder) through [`Tracker`]s, and report how the Jacobian's nonzero set moves.
fn track_newton(insts: &[&dyn ModelInstance], dim: usize) -> Result<()> {
    let track = std::sync::Mutex::new(Track::default());
    let wrapped: Vec<Tracker> = insts
        .iter()
        .enumerate()
        .map(|(i, inst)| Tracker {
            inner: *inst,
            first: i == 0,
            dim,
            track: &track,
        })
        .collect();
    let refs: Vec<&dyn ModelInstance> = wrapped.iter().map(|w| w as &dyn ModelInstance).collect();
    let before = va_core::par::mode();
    va_core::par::set_mode(va_core::par::Mode::Never);
    let solved = va_core::dc::operating_point_with_events(
        &refs,
        dim,
        va_core::newton::NewtonConfig::default(),
        None,
    );
    va_core::par::set_mode(before);
    solved.map_err(|e| anyhow::anyhow!("tracked DC solve failed: {e}"))?;
    let mut track = track.into_inner().unwrap_or_else(|e| e.into_inner());
    track.finish_assembly();
    let sets = &track.sets;

    let changes = sets.windows(2).filter(|w| w[0] != w[1]).count();
    let distinct = {
        let mut all: Vec<&Vec<(usize, usize)>> = sets.iter().collect();
        all.sort();
        all.dedup();
        all.len()
    };
    let mut union: std::collections::BTreeSet<(usize, usize)> = Default::default();
    let mut last_growth = 0;
    for (k, set) in sets.iter().enumerate() {
        let before = union.len();
        union.extend(set.iter().copied());
        if union.len() > before {
            last_growth = k;
        }
    }
    let lo = sets.iter().map(Vec::len).min().unwrap_or(0);
    let hi = sets.iter().map(Vec::len).max().unwrap_or(0);
    println!(
        "  Newton tracking: {} assemblies; nonzero set changed between consecutive assemblies \
         {changes} times ({distinct} distinct sets, {lo}-{hi} entries); running union last grew \
         at assembly {last_growth}, holds {} entries",
        sets.len(),
        union.len(),
    );
    if let Some(last) = sets.last() {
        report("final assembly", dim, last);
    }
    let union: Vec<(usize, usize)> = union.into_iter().collect();
    report("union, whole solve", dim, &union);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Step 2: a BTF block solve, prototype, timed against faer on the same matrix.
// ---------------------------------------------------------------------------------------------

/// Symbolic analysis of a BTF block solve: done once per nonzero pattern.
struct BlockPlan {
    n: usize,
    /// Blocks in solve order (a block depends only on blocks before it), each as its columns.
    blocks: Vec<Vec<usize>>,
    /// The row matched to each column by the transversal.
    row_of_col: Vec<usize>,
    /// Column -> (block index, position within the block).
    place: Vec<(usize, usize)>,
    /// For each row: `(col, index into values)` of its entries.
    row_entries: Vec<Vec<(usize, usize)>>,
}

impl BlockPlan {
    fn new(n: usize, entries: &[(usize, usize)]) -> Option<Self> {
        let mut col_rows: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(r, c) in entries {
            col_rows[c].push(r);
        }
        let row_of_col = max_transversal(n, &col_rows)?;
        let mut col_of_row = vec![usize::MAX; n];
        for (c, &r) in row_of_col.iter().enumerate() {
            col_of_row[r] = c;
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(r, c) in entries {
            let i = col_of_row[r];
            if i != c {
                adj[i].push(c);
            }
        }
        let comp = tarjan(n, &adj);
        let nblocks = comp.iter().copied().max().map_or(0, |m| m + 1);
        // Tarjan numbers a component after everything it reaches, so increasing number is a
        // valid solve order: a block's off-block unknowns are all in blocks already solved.
        let mut blocks: Vec<Vec<usize>> = vec![Vec::new(); nblocks];
        for (c, &b) in comp.iter().enumerate() {
            blocks[b].push(c);
        }
        let mut place = vec![(0, 0); n];
        for (b, cols) in blocks.iter().enumerate() {
            for (k, &c) in cols.iter().enumerate() {
                place[c] = (b, k);
            }
        }
        let mut row_entries: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
        for (k, &(r, c)) in entries.iter().enumerate() {
            row_entries[r].push((c, k));
        }
        Some(Self {
            n,
            blocks,
            row_of_col,
            place,
            row_entries,
        })
    }

    /// Numeric phase: factor each block (dense, partial pivoting) and substitute, in block
    /// order. `None` if a block is numerically singular.
    fn solve(&self, values: &[f64], b: &[f64]) -> Option<Vec<f64>> {
        let mut x = vec![0.0; self.n];
        let mut a: Vec<f64> = Vec::new();
        let mut rhs: Vec<f64> = Vec::new();
        for (bi, cols) in self.blocks.iter().enumerate() {
            let m = cols.len();
            a.clear();
            a.resize(m * m, 0.0);
            rhs.clear();
            rhs.resize(m, 0.0);
            for (i, &c) in cols.iter().enumerate() {
                let row = self.row_of_col[c];
                let mut r = b[row];
                for &(col, k) in &self.row_entries[row] {
                    let (cb, j) = self.place[col];
                    if cb == bi {
                        a[i * m + j] += values[k];
                    } else {
                        r -= values[k] * x[col];
                    }
                }
                rhs[i] = r;
            }
            let sol = dense_solve(m, &mut a, &mut rhs)?;
            for (i, &c) in cols.iter().enumerate() {
                x[c] = sol[i];
            }
        }
        Some(x)
    }
}

/// In-place dense LU with partial pivoting on row-major `a` (`m × m`), then the solve.
fn dense_solve(m: usize, a: &mut [f64], b: &mut [f64]) -> Option<Vec<f64>> {
    if m == 1 {
        return (a[0] != 0.0).then(|| vec![b[0] / a[0]]);
    }
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
    let mut x = vec![0.0; m];
    for k in (0..m).rev() {
        let mut s = b[k];
        for j in k + 1..m {
            s -= a[k * m + j] * x[j];
        }
        x[k] = s / a[k * m + k];
    }
    Some(x)
}

/// Time the block solve on the operating point's nonzero DC matrix, and `faer` on the same
/// matrix: symbolic once, numeric (+ the same residual check `SparseLu::solve` makes) as the
/// median of five. Right-hand side `A·x_op`, so the error is against a known answer.
fn time_block_solve(n: usize, nz: &[((usize, usize), f64)], x_op: &[f64]) -> Result<()> {
    let entries: Vec<(usize, usize)> = nz.iter().map(|(e, _)| *e).collect();
    let values: Vec<f64> = nz.iter().map(|(_, v)| *v).collect();
    let mut b = vec![0.0; n];
    for (&(r, c), v) in entries.iter().zip(&values) {
        b[r] += v * x_op[c];
    }
    let t0 = std::time::Instant::now();
    let plan = BlockPlan::new(n, &entries).context("structurally singular")?;
    let t_sym = t0.elapsed();
    let residual_ok = |x: &[f64]| {
        let mut ax = vec![0.0; n];
        for (&(r, c), v) in entries.iter().zip(&values) {
            ax[r] += v * x[c];
        }
        let bmax = b.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let rmax = ax
            .iter()
            .zip(&b)
            .fold(0.0_f64, |m, (a, bb)| m.max((a - bb).abs()));
        rmax <= 1e-6 * bmax.max(1.0)
    };
    let mut times = Vec::new();
    let mut x = Vec::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        x = plan
            .solve(&values, &b)
            .context("a block is numerically singular")?;
        let ok = residual_ok(&x);
        times.push(t.elapsed().as_secs_f64());
        if !ok {
            bail!("block solve failed the residual check");
        }
    }
    times.sort_by(f64::total_cmp);
    let err = x
        .iter()
        .zip(x_op)
        .fold(0.0_f64, |m, (a, t)| m.max((a - t).abs()));
    let largest = plan.blocks.iter().map(Vec::len).max().unwrap_or(0);
    println!(
        "  block solve        {} entries, {} blocks (largest {largest}): symbolic {:.2} ms, \
         numeric + residual check {:.2} ms (median of 5), max |x - x_op| {err:.1e}",
        entries.len(),
        plan.blocks.len(),
        t_sym.as_secs_f64() * 1e3,
        times[2] * 1e3,
    );
    time_lu("faer, same matrix", n, &entries, &values, x_op);
    Ok(())
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

    /// The block solve reproduces a known solution: a 2×2 block with a zero on its diagonal
    /// (so the dense LU must swap rows), singletons, and one-way couplings between blocks.
    #[test]
    fn the_block_solve_matches_a_known_answer() {
        let entries = [(0, 1), (1, 0), (1, 1), (0, 2), (2, 2), (3, 3), (2, 3)];
        let values = [2.0, 3.0, 1.0, 0.5, 4.0, 5.0, -1.0];
        let x_true = [1.0, -2.0, 0.25, 3.0];
        let mut b = [0.0; 4];
        for (&(r, c), v) in entries.iter().zip(&values) {
            b[r] += v * x_true[c];
        }
        let plan = BlockPlan::new(4, &entries).expect("nonsingular");
        let x = plan.solve(&values, &b).expect("solves");
        for (a, t) in x.iter().zip(&x_true) {
            assert!((a - t).abs() < 1e-14, "{x:?}");
        }
    }

    /// An empty column has no transversal.
    #[test]
    fn a_structurally_singular_matrix_is_reported() {
        assert!(decompose(3, &[(0, 0), (1, 1), (2, 1)]).is_none());
    }
}
