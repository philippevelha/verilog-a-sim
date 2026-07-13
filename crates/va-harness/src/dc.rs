//! Drive a single DC operating point through `va-cli` and compare it against golden (§ ladder
//! rungs 1/5 — the circuits with a plain `.op`/no-sweep `.dc` card; a `.dc` sweep or transient
//! waveform isn't wired to golden yet, see `docs/roadmap.md`'s T6.3 notes).

use crate::golden::GoldenDc;
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
}
