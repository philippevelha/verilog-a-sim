//! Drive a DC operating point ([`run_dc`]/[`compare_dc`], § ladder rungs 1/5) or a `.dc` sweep
//! ([`run_dc_sweep`]/[`compare_dc_sweep`], § ladder rung 2) through `va-cli` and compare it
//! against golden. A transient waveform isn't wired to golden yet — see `docs/roadmap.md`'s
//! T6.3 notes.

use crate::golden::{GoldenDc, GoldenSweep};
use crate::{metrics, tol, HarnessError, Verdict};

/// Solve `circuit`'s DC operating point (optionally through a compiled Verilog-A `model`) and
/// package it as a [`GoldenDc`] — the shared shape a freshly-computed result and a committed
/// golden reference both use, so [`compare_dc`] can diff them directly.
///
/// # Errors
///
/// [`HarnessError::Run`] if the netlist/model can't be read or parsed, or the solve diverges.
pub fn run_dc(circuit: &str, model: Option<&str>) -> Result<GoldenDc, HarnessError> {
    let (net, compiled) =
        va_cli::load(circuit, model).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let op = va_cli::solve_dc(&net, &compiled).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    Ok(GoldenDc::from_operating_point(&net.node_order, &op.x))
}

/// Compare a freshly-computed DC operating point against its golden reference (§7's DC metric).
///
/// # Errors
///
/// [`HarnessError::NodeOrderMismatch`] if the two don't describe the same nodes in the same
/// order — comparing their `values` at all would silently diff unrelated quantities.
pub fn compare_dc(got: &GoldenDc, golden: &GoldenDc) -> Result<Verdict, HarnessError> {
    if got.node_order != golden.node_order {
        return Err(HarnessError::NodeOrderMismatch {
            got: got.node_order.clone(),
            expected: golden.node_order.clone(),
        });
    }
    let error = metrics::max_relative_error(&got.values, &golden.values)?;
    Ok(Verdict::new(error, tol::DC_REL))
}

/// Solve `circuit`'s `.dc` sweep (optionally through a compiled Verilog-A `model`) and package
/// it as a [`GoldenSweep`].
///
/// # Errors
///
/// [`HarnessError::Run`] if the netlist/model can't be read or parsed, the deck has no `.dc`
/// sweep card, or the sweep diverges at any point.
pub fn run_dc_sweep(circuit: &str, model: Option<&str>) -> Result<GoldenSweep, HarnessError> {
    let (net, compiled) =
        va_cli::load(circuit, model).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let sweep = net
        .dc
        .clone()
        .ok_or_else(|| HarnessError::Run(format!("{circuit}: no `.dc` sweep card")))?;
    let points = va_cli::solve_dc_sweep(&net, &compiled, &sweep)
        .map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let n = net.node_order.len();
    let rows: Vec<(f64, Vec<f64>)> = points
        .into_iter()
        .map(|(value, op)| (value, op.x[..n].to_vec()))
        .collect();
    Ok(GoldenSweep::from_sweep(
        &sweep.source,
        &net.node_order,
        &rows,
    ))
}

/// Compare a freshly-computed `.dc` sweep against its golden reference (§7's DC metric, applied
/// over every point's node voltages flattened into one long series — the same shape a single
/// operating point's `values` already are, just concatenated across points).
///
/// # Errors
///
/// [`HarnessError::NodeOrderMismatch`] if the two don't describe the same nodes in the same
/// order; [`HarnessError::LengthMismatch`] if they don't have the same number of swept points.
pub fn compare_dc_sweep(got: &GoldenSweep, golden: &GoldenSweep) -> Result<Verdict, HarnessError> {
    if got.node_order != golden.node_order {
        return Err(HarnessError::NodeOrderMismatch {
            got: got.node_order.clone(),
            expected: golden.node_order.clone(),
        });
    }
    if got.points.len() != golden.points.len() {
        return Err(HarnessError::LengthMismatch {
            got: got.points.len(),
            expected: golden.points.len(),
        });
    }
    let flatten = |points: &[(f64, Vec<f64>)]| -> Vec<f64> {
        points.iter().flat_map(|(_, v)| v.iter().copied()).collect()
    };
    let error = metrics::max_relative_error(&flatten(&got.points), &flatten(&golden.points))?;
    Ok(Verdict::new(error, tol::DC_REL))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute path to a workspace file, robust to `cargo test`'s working directory (unlike a
    /// bare relative path passed at runtime — contrast `include_str!`, which resolves at
    /// compile time relative to *this source file* and needs no such trick).
    fn workspace_path(rel: &str) -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel
    }

    #[test]
    fn run_dc_solves_the_divider() {
        let g = run_dc(&workspace_path("circuits/divider.net"), None).expect("solve divider");
        assert_eq!(g.node_order, vec!["in", "mid"]);
        assert!((g.values[0] - 1.0).abs() < 1e-9, "V(in) = {}", g.values[0]);
        assert!((g.values[1] - 0.5).abs() < 1e-9, "V(mid) = {}", g.values[1]);
    }

    #[test]
    fn compare_dc_passes_for_an_identical_reference() {
        let got = run_dc(&workspace_path("circuits/divider.net"), None).expect("solve divider");
        let verdict = compare_dc(&got, &got).expect("compare");
        assert!(verdict.passed);
        assert_eq!(verdict.error, 0.0);
    }

    #[test]
    fn compare_dc_fails_for_a_diverged_reference() {
        let got = run_dc(&workspace_path("circuits/divider.net"), None).expect("solve divider");
        let mut golden = got.clone();
        golden.values[1] = 0.9; // nowhere near the real 0.5
        let verdict = compare_dc(&got, &golden).expect("compare");
        assert!(!verdict.passed);
    }

    #[test]
    fn compare_dc_rejects_a_node_order_mismatch() {
        let got = GoldenDc {
            node_order: vec!["in".to_string(), "mid".to_string()],
            values: vec![1.0, 0.5],
        };
        let golden = GoldenDc {
            node_order: vec!["mid".to_string(), "in".to_string()],
            values: vec![0.5, 1.0],
        };
        assert!(compare_dc(&got, &golden).is_err());
    }

    #[test]
    fn run_dc_sweep_solves_the_diode_iv_curve() {
        // `GoldenSweep` only records node *voltages*, not source branch currents (§
        // `GoldenDc::from_operating_point`'s doc comment) — so the diode's own current, and the
        // Shockley-law cross-check against it, is `va-cli`'s
        // `diode_iv_sweep_solves_through_codegen_pipeline` test's job, not this one's. What this
        // test checks is the plumbing: the right number of points, in order, with `V(in)`
        // tracking the directly-forced source exactly at every one.
        let g = run_dc_sweep(
            &workspace_path("circuits/diode_iv.net"),
            Some(&workspace_path("models/diode.va")),
        )
        .expect("solve diode_iv sweep");
        assert_eq!(g.source, "V1");
        assert_eq!(g.node_order, vec!["in"]);
        assert_eq!(g.points.len(), 7); // 0.0, 0.1, ..., 0.6
        for (v, values) in &g.points {
            assert!(
                (values[0] - v).abs() < 1e-9,
                "V(in) = {} at V1={v}",
                values[0]
            );
        }
    }

    #[test]
    fn compare_dc_sweep_passes_for_an_identical_reference() {
        let got = run_dc_sweep(
            &workspace_path("circuits/diode_iv.net"),
            Some(&workspace_path("models/diode.va")),
        )
        .expect("solve diode_iv sweep");
        let verdict = compare_dc_sweep(&got, &got).expect("compare");
        assert!(verdict.passed);
        assert_eq!(verdict.error, 0.0);
    }

    #[test]
    fn compare_dc_sweep_fails_for_a_diverged_reference() {
        let got = run_dc_sweep(
            &workspace_path("circuits/diode_iv.net"),
            Some(&workspace_path("models/diode.va")),
        )
        .expect("solve diode_iv sweep");
        let mut golden = got.clone();
        golden.points[6].1[0] = 0.0; // last point (V1=0.6) wildly wrong
        let verdict = compare_dc_sweep(&got, &golden).expect("compare");
        assert!(!verdict.passed);
    }

    #[test]
    fn compare_dc_sweep_rejects_a_point_count_mismatch() {
        let a = GoldenSweep {
            source: "V1".to_string(),
            node_order: vec!["in".to_string()],
            points: vec![(0.0, vec![0.0])],
        };
        let b = GoldenSweep {
            source: "V1".to_string(),
            node_order: vec!["in".to_string()],
            points: vec![(0.0, vec![0.0]), (0.1, vec![0.1])],
        };
        assert!(compare_dc_sweep(&a, &b).is_err());
    }

    #[test]
    fn run_dc_sweep_errors_on_a_deck_with_no_dc_card() {
        assert!(run_dc_sweep(&workspace_path("circuits/divider.net"), None).is_err());
    }
}
