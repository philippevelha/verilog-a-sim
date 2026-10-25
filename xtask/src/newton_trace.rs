//! `cargo xtask newton-trace <deck> [--model <m>] [--iters N] [--show K] [--cap V] [--solve] [--sparse] [--shunt G]` — plain Newton on a
//! deck's DC operating point, one line per iteration, naming the unknowns that move the most.
//!
//! Written to name the rows behind After-2.0 item 1 of `docs/roadmap.md` (why plain Newton fails
//! on ISCAS'85 and every circuit from c432 up needs the `gmin` rescue). It mirrors the plain tier
//! of `va_core::dc`: a zero start, a full Newton step with the same junction limiting
//! (`va_core::convergence::limit_junction` on the unknowns `mna::classify_junctions` marks), no
//! damping, no shunt. It assembles **densely** (`va_core::mna::assemble`), so it is for small
//! decks only: one cell, not a whole benchmark (c432 densely is 1.9 GB).
//!
//! For the unknown with the largest step it also prints that unknown's Jacobian column and row,
//! by name, which is what says how weakly the equations hold it.
//!
//! **Limitations:** the dense solve is `linsolve::solve_dense`, not the sparse/BTF path the DC
//! solve uses on a deck this size, so a failure that is the sparse solver's own would not show.
//! The point of the tool is the Newton trajectory, which is the same.

use anyhow::{Context, Result};
use va_abi::ModelInstance;
use va_core::convergence::{default_vcrit, limit_junction, VT_NOMINAL};

pub fn newton_trace(args: &[String]) -> Result<()> {
    let mut deck = None;
    let mut model = None;
    let mut iters = 12usize;
    let mut show = 3usize;
    let mut cap = f64::INFINITY;
    let mut real = false;
    let mut sparse = false;
    let mut shunt = 0.0_f64;
    let mut accept = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model" => model = Some(it.next().context("--model needs a path")?.clone()),
            "--iters" => iters = it.next().context("--iters needs N")?.parse()?,
            "--show" => show = it.next().context("--show needs K")?.parse()?,
            "--cap" => cap = it.next().context("--cap needs volts")?.parse()?,
            "--solve" => real = true,
            "--sparse" => sparse = true,
            "--shunt" => shunt = it.next().context("--shunt needs siemens")?.parse()?,
            "--accept-backward" => accept = true,
            other => deck = Some(other.to_string()),
        }
    }
    let deck = deck.context("usage: cargo xtask newton-trace <deck> [--model <m>] [--iters N]")?;
    let (net, modules) = va_cli::load(&deck, model.as_deref())?;
    let (owned, dim) = va_cli::instances(&net, &modules)?;
    let inst: Vec<&dyn ModelInstance> = owned.iter().map(|b| b.as_ref()).collect();
    let mut name = vec![String::new(); dim];
    for q in va_cli::quantities(&net, &modules)? {
        if q.index < dim {
            name[q.index] = q.label.clone();
        }
    }
    for (i, n) in name.iter_mut().enumerate() {
        if n.is_empty() {
            *n = format!("#{i}");
        }
    }
    if real {
        // `--solve`: the real sparse Newton (`va_core::newton`), plain tier settings plus the cap,
        // with its own per-iteration `[logfull]` lines: for a deck too large to trace densely.
        let cfg = va_core::newton::NewtonConfig {
            max_iters: iters,
            max_node_step: cap,
            log_full: true,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let r = va_core::newton::solve_with_events_from(
            &inst,
            dim,
            cfg,
            &va_abi::FiredEvents::default(),
            None,
        );
        println!(
            "[newton-trace] --solve, cap {cap} V, max {iters} iterations: {} in {:.1} s",
            match &r {
                Ok(_) => "converged".to_string(),
                Err(e) => format!("failed: {e}"),
            },
            t.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    let junction = va_core::mna::classify_junctions(&inst, dim);
    if sparse {
        return sparse_trace(&inst, dim, &name, &junction, iters, cap, shunt, accept);
    }
    let kinds = va_core::mna::classify_unknowns(&inst, dim);
    let ctx = va_abi::ANALYSIS_DC;
    let vcrit = default_vcrit(VT_NOMINAL);
    println!("[newton-trace] {deck}: {dim} unknowns");

    let mut x = vec![0.0; dim];
    for k in 0..iters {
        let sys = va_core::mna::assemble(&inst, &x, &ctx, dim);
        if let Some(r) = sys.residual.iter().position(|v| !v.is_finite()) {
            println!("iter {k}: non-finite residual at row {r} {}", name[r]);
            break;
        }
        let (rmax, rrow) = sys
            .residual
            .iter()
            .enumerate()
            .fold(
                (0.0_f64, 0),
                |m, (i, v)| if v.abs() > m.0 { (v.abs(), i) } else { m },
            );
        let neg: Vec<f64> = sys.residual.iter().map(|v| -v).collect();
        let dx = match va_core::linsolve::solve_dense(&sys.jacobian, &neg, dim) {
            Ok(d) => d,
            Err(e) => {
                println!(
                    "iter {k}: linear solve failed: {e}; max residual {rmax:.3e} at {}",
                    name[rrow]
                );
                break;
            }
        };
        let mut order: Vec<usize> = (0..dim).collect();
        order.sort_by(|&a, &b| dx[b].abs().total_cmp(&dx[a].abs()));
        let top: Vec<String> = order
            .iter()
            .take(show)
            .map(|&i| {
                format!(
                    "{} {:+.3e} (x={:.4e}, {:?})",
                    name[i], dx[i], x[i], kinds[i]
                )
            })
            .collect();
        println!(
            "iter {k}: residual {rmax:.3e} at {} | largest steps: {}",
            name[rrow],
            top.join("; ")
        );
        if k < 3 || k + 1 == iters {
            let w = order[0];
            let col: Vec<String> = (0..dim)
                .filter(|&r| sys.jacobian[r * dim + w] != 0.0)
                .map(|r| format!("{}={:.3e}", name[r], sys.jacobian[r * dim + w]))
                .collect();
            let row: Vec<String> = (0..dim)
                .filter(|&c| sys.jacobian[w * dim + c] != 0.0)
                .map(|c| format!("{}={:.3e}", name[c], sys.jacobian[w * dim + c]))
                .collect();
            println!("    column of {}: {}", name[w], col.join(", "));
            println!(
                "    row    of {}: {} | residual {:.3e}",
                name[w],
                row.join(", "),
                sys.residual[w]
            );
        }
        for i in 0..dim {
            // `--cap`: the rescue's node-step cap (`NewtonConfig::max_node_step`), on node rows.
            let step = if kinds[i] == va_abi::UnknownKind::Node {
                dx[i].clamp(-cap, cap)
            } else {
                dx[i]
            };
            x[i] = if junction[i] {
                limit_junction(x[i] + step, x[i], VT_NOMINAL, vcrit)
            } else {
                x[i] + step
            };
        }
    }
    Ok(())
}

/// `--sparse`: the same capped plain Newton on the sparse path (`va_core::sparse`, BTF then
/// `faer`), for decks too large to trace densely. When a linear solve fails, the Jacobian it was
/// given is examined for the usual causes of an exactly singular matrix, each named: rows and
/// columns whose entries are all zero, and pairs of identical rows (a constraint stated twice).
#[allow(clippy::too_many_arguments)]
fn sparse_trace(
    inst: &[&dyn ModelInstance],
    dim: usize,
    name: &[String],
    junction: &[bool],
    iters: usize,
    cap: f64,
    shunt: f64,
    accept: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use va_core::sparse::{assemble_into, SparseLu, SparseSystem};
    let kinds = va_core::mna::classify_unknowns(inst, dim);
    let vcrit = default_vcrit(VT_NOMINAL);
    let ctx = va_abi::ANALYSIS_DC;
    let fired = va_abi::FiredEvents::default();
    let mut sys = SparseSystem::new(dim);
    let mut lu = SparseLu::with_btf();
    let mut x = vec![0.0; dim];
    for k in 0..iters {
        assemble_into(inst, &x, &ctx, &fired, &mut sys);
        // `--shunt G`: a fixed conductance to ground on every node row (the rescue's own
        // `SparseSystem::shunt_gmin`), held for the whole run rather than stepped away.
        sys.shunt_gmin(&x, shunt, &kinds);
        let f = sys.residual_values().to_vec();
        let rmax = f.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let neg: Vec<f64> = f.iter().map(|v| -v).collect();
        let j = sys.jacobian();
        // `--accept-backward`: when `SparseLu` rejects a solve, accept faer's answer anyway if
        // its normwise backward error |Ax - b| / (|A||x| + |b|) is at rounding level: the
        // criterion that asks whether the solve is as good as floating point allows, rather
        // than whether the residual is small in absolute terms.
        let solved = lu.solve(j, &neg).or_else(|e| {
            if !accept {
                return Err(e);
            }
            match backward_stable(j, &neg, dim) {
                Some((x, eta)) if eta < 1e-12 => {
                    println!(
                        "    (accepted on backward error {eta:.2e}, max |dx| {:.2e})",
                        x.iter().fold(0.0_f64, |m, v| m.max(v.abs()))
                    );
                    Ok(x)
                }
                Some((_, eta)) => {
                    println!("    (backward error {eta:.2e}: rejected)");
                    Err(e)
                }
                None => Err(e),
            }
        });
        match solved {
            Ok(dx) => {
                let (w, _) = dx.iter().enumerate().fold((0, 0.0_f64), |m, (i, v)| {
                    if v.abs() > m.1 {
                        (i, v.abs())
                    } else {
                        m
                    }
                });
                println!(
                    "iter {k}: residual {rmax:.3e} | largest step {} {:+.3e}",
                    name[w], dx[w]
                );
                if rmax <= 1e-12 {
                    println!("converged");
                    if let Some(path) = std::env::var_os("VA_TRACE_X") {
                        let text: String = (0..dim)
                            .map(|i| {
                                format!(
                                    "{}	{:e}
",
                                    name[i], x[i]
                                )
                            })
                            .collect();
                        std::fs::write(&path, text)?;
                    }
                    return Ok(());
                }
                for i in 0..dim {
                    let step = if kinds[i] == va_abi::UnknownKind::Node {
                        dx[i].clamp(-cap, cap)
                    } else {
                        dx[i]
                    };
                    x[i] = if junction[i] {
                        limit_junction(x[i] + step, x[i], VT_NOMINAL, vcrit)
                    } else {
                        x[i] + step
                    };
                }
            }
            Err(e) => {
                println!("iter {k}: residual {rmax:.3e} | linear solve failed: {e}");
                let mut rows: Vec<Vec<(usize, f64)>> = vec![Vec::new(); dim];
                for ((r, c), v) in j.pattern().entries().zip(j.values()) {
                    if *v != 0.0 {
                        rows[r].push((c, *v));
                    }
                }
                let mut colmax = vec![0.0_f64; dim];
                for row in &rows {
                    for &(c, v) in row {
                        colmax[c] = colmax[c].max(v.abs());
                    }
                }
                let zr: Vec<&str> = (0..dim)
                    .filter(|&r| rows[r].is_empty())
                    .map(|r| name[r].as_str())
                    .collect();
                let zc: Vec<&str> = (0..dim)
                    .filter(|&c| colmax[c] == 0.0)
                    .map(|c| name[c].as_str())
                    .collect();
                println!(
                    "  all-zero rows ({}): {}",
                    zr.len(),
                    zr.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                );
                println!(
                    "  all-zero columns ({}): {}",
                    zc.len(),
                    zc.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                );
                let mut seen: HashMap<Vec<(usize, u64)>, usize> = HashMap::new();
                let mut dups = 0;
                for (r, row) in rows.iter().enumerate() {
                    if row.is_empty() {
                        continue;
                    }
                    let mut key: Vec<(usize, u64)> =
                        row.iter().map(|&(c, v)| (c, v.to_bits())).collect();
                    key.sort();
                    if let Some(&first) = seen.get(&key) {
                        dups += 1;
                        if dups <= 20 {
                            println!(
                                "  identical rows: {} = {} ({} entries)",
                                name[first],
                                name[r],
                                row.len()
                            );
                        }
                    } else {
                        seen.insert(key, r);
                    }
                }
                println!("  identical row pairs: {dups}");
                block_report(dim, &rows, name);
                // Factor the same matrix with faer directly, outside `SparseLu`'s checks: does
                // the factorization itself fail, or does it return an answer the backward-error
                // check then rejects? And which rows does that answer miss?
                let trip: Vec<faer::sparse::Triplet<usize, usize, f64>> = j
                    .pattern()
                    .entries()
                    .zip(j.values())
                    .map(|((r, c), v)| faer::sparse::Triplet::new(r, c, *v))
                    .collect();
                match faer::sparse::SparseColMat::<usize, f64>::try_new_from_triplets(
                    dim, dim, &trip,
                ) {
                    Err(e) => println!("  faer: matrix build failed: {e:?}"),
                    Ok(a) => match a.sp_lu() {
                        Err(e) => println!("  faer: LU failed: {e:?}"),
                        Ok(flu) => {
                            let rhs = faer::Mat::from_fn(dim, 1, |i, _| neg[i]);
                            let sol = faer::linalg::solvers::Solve::solve(&flu, &rhs);
                            let xs: Vec<f64> = (0..dim).map(|i| sol[(i, 0)]).collect();
                            let ax = j.mul_vec(&xs);
                            let mut res: Vec<(usize, f64)> =
                                (0..dim).map(|i| (i, (ax[i] - neg[i]).abs())).collect();
                            res.sort_by(|a, b| b.1.total_cmp(&a.1));
                            let xmax = xs.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
                            let nonfinite = xs.iter().filter(|v| !v.is_finite()).count();
                            println!(
                                "  faer: LU ok; max |x| {xmax:.3e}, {nonfinite} non-finite; worst rows of |Ax - b|:"
                            );
                            for &(i, r) in res.iter().take(8) {
                                let row: Vec<String> = rows[i]
                                    .iter()
                                    .map(|&(c, v)| format!("{}={:.2e}", name[c], v))
                                    .collect();
                                println!(
                                    "    {} |r|={r:.3e} b={:.3e} x={:.3e}: {}",
                                    name[i],
                                    neg[i],
                                    xs[i],
                                    row.join(" ")
                                );
                            }
                            let mut big: Vec<usize> = (0..dim).collect();
                            big.sort_by(|&p, &q| xs[q].abs().total_cmp(&xs[p].abs()));
                            let bl: Vec<String> = big
                                .iter()
                                .take(6)
                                .map(|&i| format!("{}={:.3e}", name[i], xs[i]))
                                .collect();
                            println!("  largest |dx| from faer: {}", bl.join(", "));
                            // `VA_TRACE_DUMP=<file>`: every unknown's name and faer's step, one per
                            // line, for analysis outside this tool.
                            if let Some(path) = std::env::var_os("VA_TRACE_DUMP") {
                                let text: String = (0..dim)
                                    .map(|i| {
                                        format!(
                                            "{}	{:e}
",
                                            name[i], xs[i]
                                        )
                                    })
                                    .collect();
                                std::fs::write(&path, text)?;
                            }
                            // Is the matrix near-singular, or the factorization unreliable?
                            let (mut vmax, mut vmin) = (0.0_f64, f64::INFINITY);
                            for v in j.values() {
                                if *v != 0.0 {
                                    vmax = vmax.max(v.abs());
                                    vmin = vmin.min(v.abs());
                                }
                            }
                            let rnorm = res[0].1;
                            let jv = j.mul_vec(&xs.iter().map(|v| v / xmax).collect::<Vec<_>>());
                            let jvmax = jv.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
                            println!(
                                "  |entries| in [{vmin:.2e}, {vmax:.2e}]; |J v|/|v| = {jvmax:.3e} for v = x/|x|; |Ax-b| = {rnorm:.3e}"
                            );
                            // One step of iterative refinement with the same factors.
                            let r: Vec<f64> = (0..dim).map(|i| neg[i] - ax[i]).collect();
                            let rr = faer::Mat::from_fn(dim, 1, |i, _| r[i]);
                            let d = faer::linalg::solvers::Solve::solve(&flu, &rr);
                            let x2: Vec<f64> = (0..dim).map(|i| xs[i] + d[(i, 0)]).collect();
                            let ax2 = j.mul_vec(&x2);
                            let r2 = (0..dim)
                                .map(|i| (ax2[i] - neg[i]).abs())
                                .fold(0.0_f64, f64::max);
                            let x2max = x2.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
                            println!("  after one refinement step: |Ax-b| = {r2:.3e}, max |x| = {x2max:.3e}");
                        }
                    },
                }
                // The rows the last applied step moved furthest from zero, for context.
                let mut order: Vec<usize> = (0..dim).collect();
                order.sort_by(|&a, &b| x[b].abs().total_cmp(&x[a].abs()));
                let far: Vec<String> = order
                    .iter()
                    .take(5)
                    .map(|&i| format!("{}={:.3e}", name[i], x[i]))
                    .collect();
                println!("  largest |x|: {}", far.join(", "));
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Split the failing Jacobian (its numerically nonzero entries, `rows[r] = [(col, value)]`) into
/// block triangular form and print the worst-conditioned diagonal blocks, each unknown named.
/// The near-singular direction of the whole matrix lives in one or more of these blocks, since
/// a block triangular matrix is singular exactly when a diagonal block is.
fn block_report(dim: usize, rows: &[Vec<(usize, f64)>], name: &[String]) {
    let mut col_rows: Vec<Vec<usize>> = vec![Vec::new(); dim];
    for (r, row) in rows.iter().enumerate() {
        for &(c, _) in row {
            col_rows[c].push(r);
        }
    }
    let Some(row_of_col) = crate::btf::max_transversal(dim, &col_rows) else {
        println!("  BTF: structurally singular (no maximum transversal)");
        return;
    };
    let mut col_of_row = vec![usize::MAX; dim];
    for (c, &r) in row_of_col.iter().enumerate() {
        col_of_row[r] = c;
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); dim];
    for (r, row) in rows.iter().enumerate() {
        for &(c, _) in row {
            let i = col_of_row[r];
            if i != c {
                adj[i].push(c);
            }
        }
    }
    let comp = crate::btf::tarjan(dim, &adj);
    let nb = comp.iter().copied().max().map_or(0, |m| m + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (c, &b) in comp.iter().enumerate() {
        members[b].push(c);
    }
    let mut report: Vec<(f64, usize)> = Vec::new();
    for (b, cols) in members.iter().enumerate() {
        let m = cols.len();
        if m > 400 {
            println!("  BTF block {b}: {m} unknowns, too large for a dense SVD here; skipped");
            continue;
        }
        let pos: std::collections::HashMap<usize, usize> =
            cols.iter().enumerate().map(|(i, &c)| (c, i)).collect();
        let a = faer::Mat::<f64>::from_fn(m, m, |i, j| {
            let r = row_of_col[cols[i]];
            rows[r]
                .iter()
                .find(|&&(c, _)| pos.get(&c) == Some(&j))
                .map_or(0.0, |&(_, v)| v)
        });
        let Ok(sv) = a.singular_values() else {
            continue;
        };
        let (smax, smin) = (
            sv.first().copied().unwrap_or(0.0),
            sv.last().copied().unwrap_or(0.0),
        );
        let rc = if smax > 0.0 { smin / smax } else { 0.0 };
        report.push((rc, b));
    }
    report.sort_by(|a, b| a.0.total_cmp(&b.0));
    let sizes: Vec<usize> = members.iter().map(|m| m.len()).collect();
    println!(
        "  BTF: {nb} blocks, largest {}; worst reciprocal condition numbers:",
        sizes.iter().max().unwrap_or(&0)
    );
    for &(rc, b) in report.iter().take(4) {
        let who: Vec<&str> = members[b]
            .iter()
            .take(14)
            .map(|&c| name[c].as_str())
            .collect();
        println!(
            "    block {b} ({} unknowns): sigma_min/sigma_max = {rc:.3e}: {}{}",
            members[b].len(),
            who.join(", "),
            if members[b].len() > 14 { ", ..." } else { "" }
        );
    }
}

/// faer's sparse LU solve of `a x = b`, outside `SparseLu`'s acceptance test, with its normwise
/// backward error `max|Ax - b| / (max(|A| |x|) + max|b|)` (the infinity-norm Rigal-Gaches form).
fn backward_stable(
    a: va_core::sparse::SparseMatrix<'_>,
    b: &[f64],
    dim: usize,
) -> Option<(Vec<f64>, f64)> {
    let trip: Vec<faer::sparse::Triplet<usize, usize, f64>> = a
        .pattern()
        .entries()
        .zip(a.values())
        .map(|((r, c), v)| faer::sparse::Triplet::new(r, c, *v))
        .collect();
    let m =
        faer::sparse::SparseColMat::<usize, f64>::try_new_from_triplets(dim, dim, &trip).ok()?;
    let lu = m.sp_lu().ok()?;
    let rhs = faer::Mat::from_fn(dim, 1, |i, _| b[i]);
    let sol = faer::linalg::solvers::Solve::solve(&lu, &rhs);
    let x: Vec<f64> = (0..dim).map(|i| sol[(i, 0)]).collect();
    if !x.iter().all(|v| v.is_finite()) {
        return None;
    }
    let ax = a.mul_vec(&x);
    let mut absax = vec![0.0_f64; dim];
    for ((r, c), v) in a.pattern().entries().zip(a.values()) {
        absax[r] += v.abs() * x[c].abs();
    }
    let r = (0..dim)
        .map(|i| (ax[i] - b[i]).abs())
        .fold(0.0_f64, f64::max);
    let scale = absax.iter().fold(0.0_f64, |m, v| m.max(*v))
        + b.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    Some((x, if scale > 0.0 { r / scale } else { 0.0 }))
}
