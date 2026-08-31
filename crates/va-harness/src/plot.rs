//! Sim-vs-golden overlay plots: the picture behind every `xtask validate` verdict.
//!
//! `xtask validate` prints one number per circuit (`rectifier 6.8e-4, tol 1e-3`). That number
//! answers "did it pass"; it does not answer "*where* does it disagree, and does the shape look
//! right at all". An overlay does, and `docs/roadmap.md`'s tutorial conventions ask for exactly
//! this in every phase's "It works" section.
//!
//! Golden is drawn as grey dots underneath, the simulated curve as a solid line on top, so the
//! eye reads the simulated result as the thing under test rather than as one of two equal
//! traces.
//!
//! # What is drawn, and what is not
//!
//! **Node voltages only.** Both [`GoldenSweep`] and [`GoldenTran`] carry named branch currents
//! appended after the node prefix, and a diode sweep puts volts (0…0.6) beside amps (~1e-13) on
//! one linear axis, where the smaller series is a flat line on zero. Currents need a second
//! axis to be worth drawing; until there is one, omitting them is honest and a chart that looks
//! empty is not. `va_cli::plot` draws the same subset for the same reason.
//!
//! # Errors
//!
//! Every entry point returns [`HarnessError::Run`] for a drawing/IO failure and
//! [`HarnessError::LengthMismatch`] where the two series cannot be put on one x grid — the same
//! condition the corresponding `compare_*` refuses to score.

use plotters::prelude::*;

use crate::golden::{GoldenSweep, GoldenTran};
use crate::HarnessError;

/// Palette for the simulated series, cycled by node index; golden is always grey.
const PALETTE: [RGBColor; 6] = [RED, BLUE, GREEN, MAGENTA, CYAN, BLACK];

/// The grey every golden trace is drawn in — one colour for "the reference", regardless of
/// which node it belongs to, so the legend reads as two groups rather than 2N unrelated lines.
const GOLDEN: RGBColor = RGBColor(120, 120, 120);

/// Wrap any `plotters`/IO failure as [`HarnessError::Run`], matching how `dc.rs` already folds
/// a pipeline failure into a string rather than growing a dependency for it.
fn draw_err(what: &str, e: impl std::fmt::Display) -> HarnessError {
    HarnessError::Run(format!("{what}: {e}"))
}

/// One overlay: a caption, an x-axis label, and the two series sets on a shared x grid.
struct Overlay<'a> {
    caption: &'a str,
    x_desc: String,
    node_order: &'a [String],
    /// `(x, per-node values)` for the simulated run.
    sim: &'a [(f64, Vec<f64>)],
    /// `(x, per-node values)` for the golden reference, already on the same x grid.
    golden: &'a [(f64, Vec<f64>)],
}

/// Draw `ov` to `path`. Shared by both entry points, which differ only in axis labels and in
/// how they get onto a common x grid.
/// Whether a `node_order` entry names a branch current rather than a node voltage.
///
/// The golden format spells those `I(<device>)` (§ `crate::golden`'s branch-current
/// convention), which is not a legal node name, so the prefix test is unambiguous.
fn is_branch_current(name: &str) -> bool {
    name.starts_with("I(")
}

fn render(path: &str, ov: &Overlay) -> Result<(), HarnessError> {
    let (first, last) = match (ov.sim.first(), ov.sim.last()) {
        (Some(f), Some(l)) => (f.0, l.0),
        _ => {
            return Err(HarnessError::LengthMismatch {
                got: 0,
                expected: 1,
            })
        }
    };
    // A single-point run would otherwise collapse the x-axis to zero width.
    let (x_min, x_max) = if last > first {
        (first, last)
    } else {
        (first - 1.0, last + 1.0)
    };

    // Only the node-voltage columns are drawn. A golden file's `node_order` also carries the
    // branch currents `va_cli::branch_currents` resolved (`I(V1)` and friends, § the golden
    // format's branch-current convention), and those are amps: on a shared linear axis with
    // volts a milliamp trace flatlines along the bottom while claiming to be a voltage, which
    // is exactly the reasoning `va_cli::plot::plot_sweep` already states for drawing node
    // voltages only. The gate still checks every column — the figure just doesn't pretend
    // amps and volts share a scale.
    let columns: Vec<usize> = (0..ov.node_order.len())
        .filter(|&i| !is_branch_current(&ov.node_order[i]))
        .collect();
    if columns.is_empty() {
        return Err(HarnessError::Run(
            "nothing to plot: every column is a branch current, and this chart's axis is volts"
                .to_string(),
        ));
    }
    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (_, row) in ov.sim.iter().chain(ov.golden.iter()) {
        for &i in &columns {
            if let Some(&v) = row.get(i) {
                if v.is_finite() {
                    y_min = y_min.min(v);
                    y_max = y_max.max(v);
                }
            }
        }
    }
    if !y_min.is_finite() || !y_max.is_finite() {
        return Err(HarnessError::Run(
            "nothing to plot: no finite node voltages in either series".to_string(),
        ));
    }
    if y_max <= y_min {
        y_min -= 1.0;
        y_max += 1.0;
    }
    let pad = 0.05 * (y_max - y_min);
    y_min -= pad;
    y_max += pad;

    let root = SVGBackend::new(path, (960, 540)).into_drawing_area();
    root.fill(&WHITE)
        .map_err(|e| draw_err("initializing the SVG canvas", e))?;

    let mut chart = ChartBuilder::on(&root)
        .caption(ov.caption, ("sans-serif", 24))
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(55)
        .build_cartesian_2d(x_min..x_max, y_min..y_max)
        .map_err(|e| draw_err("building the chart coordinate system", e))?;

    chart
        .configure_mesh()
        .x_desc(&ov.x_desc)
        .y_desc("Voltage (V)")
        .draw()
        .map_err(|e| draw_err("drawing the chart mesh", e))?;

    // Golden first, so the simulated curve lands on top of it.
    for &i in &columns {
        chart
            .draw_series(ov.golden.iter().filter_map(|(x, row)| {
                row.get(i)
                    .map(|&y| Circle::new((*x, y), 2, GOLDEN.filled()))
            }))
            .map_err(|e| draw_err("drawing a golden series", e))?;
    }
    // One legend entry for the whole golden group, registered once with an empty series.
    chart
        .draw_series(std::iter::empty::<Circle<(f64, f64), i32>>())
        .map_err(|e| draw_err("registering the golden legend entry", e))?
        .label("golden (QSPICE)")
        .legend(|(x, y)| Circle::new((x + 10, y), 2, GOLDEN.filled()));

    for &i in &columns {
        let name = &ov.node_order[i];
        let color = PALETTE[i % PALETTE.len()];
        chart
            .draw_series(LineSeries::new(
                ov.sim
                    .iter()
                    .filter_map(|(x, row)| row.get(i).map(|&y| (*x, y))),
                &color,
            ))
            .map_err(|e| draw_err("drawing a simulated series", e))?
            .label(format!("V({name})"))
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
    }

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()
        .map_err(|e| draw_err("drawing the legend", e))?;

    root.present().map_err(|e| draw_err("writing the SVG", e))?;
    Ok(())
}

/// Overlay a solved `.dc` sweep on its golden reference.
///
/// **No resampling**, deliberately: [`crate::dc::compare_dc_sweep`] already requires both to
/// carry the same point count at the same swept values, so putting them on one x grid is sound
/// for exactly the reason comparing them is. A mismatch is an error here rather than a silently
/// misaligned picture.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if the point counts differ or either sweep is empty;
/// [`HarnessError::Run`] if drawing or writing the SVG fails.
pub fn overlay_sweep(
    path: &str,
    sim: &GoldenSweep,
    golden: &GoldenSweep,
) -> Result<(), HarnessError> {
    if sim.points.len() != golden.points.len() {
        return Err(HarnessError::LengthMismatch {
            got: sim.points.len(),
            expected: golden.points.len(),
        });
    }
    render(
        path,
        &Overlay {
            caption: "DC sweep vs golden",
            x_desc: format!("{} (V)", sim.source),
            node_order: &sim.node_order,
            sim: &sim.points,
            golden: &golden.points,
        },
    )
}

/// Overlay a solved transient run on its golden reference.
///
/// The two runs are on **different time grids** — ours is adaptive, QSPICE's is its own — so the
/// golden series is resampled onto the simulated timebase with
/// [`crate::metrics::resample_linear`], which is the same shared-timebase resample
/// [`crate::tran::compare_tran`] scores against. The picture and the number therefore describe
/// the same comparison; drawing the raw grids instead would show a disagreement the verdict
/// never scored.
///
/// # Errors
///
/// [`HarnessError::LengthMismatch`] if either run is empty; [`HarnessError::Run`] if drawing or
/// writing the SVG fails.
pub fn overlay_tran(path: &str, sim: &GoldenTran, golden: &GoldenTran) -> Result<(), HarnessError> {
    if sim.points.is_empty() || golden.points.is_empty() {
        return Err(HarnessError::LengthMismatch {
            got: sim.points.len(),
            expected: golden.points.len().max(1),
        });
    }
    let sim_t: Vec<f64> = sim.points.iter().map(|(t, _)| *t).collect();
    let gold_t: Vec<f64> = golden.points.iter().map(|(t, _)| *t).collect();

    // Resample each node's golden series onto the simulated timebase, then re-row them.
    let n = sim.node_order.len();
    let mut rows: Vec<(f64, Vec<f64>)> = sim_t.iter().map(|&t| (t, vec![0.0; n])).collect();
    for i in 0..n {
        let series: Vec<f64> = golden
            .points
            .iter()
            .map(|(_, row)| row.get(i).copied().unwrap_or(0.0))
            .collect();
        let resampled = crate::metrics::resample_linear(&gold_t, &series, &sim_t);
        for (row, v) in rows.iter_mut().zip(resampled) {
            row.1[i] = v;
        }
    }

    render(
        path,
        &Overlay {
            caption: "Transient vs golden",
            x_desc: "Time (s)".to_string(),
            node_order: &sim.node_order,
            sim: &sim.points,
            golden: &rows,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn sweep(points: Vec<(f64, Vec<f64>)>) -> GoldenSweep {
        GoldenSweep {
            source: "V1".into(),
            node_order: vec!["in".into(), "out".into()],
            points,
        }
    }

    #[test]
    fn sweep_overlay_writes_a_real_svg_with_both_series_labelled() {
        let sim = sweep(vec![(0.0, vec![0.0, 0.0]), (1.0, vec![1.0, 0.5])]);
        let gold = sweep(vec![(0.0, vec![0.0, 0.0]), (1.0, vec![1.0, 0.49])]);
        let path = tmp("va_harness_overlay_sweep.svg");
        overlay_sweep(&path, &sim, &gold).expect("renders");
        let svg = std::fs::read_to_string(&path).expect("reads back");
        let _ = std::fs::remove_file(&path);
        assert!(svg.contains("<svg"), "not an SVG");
        assert!(svg.contains("golden (QSPICE)"), "golden series unlabelled");
        assert!(svg.contains("V(out)"), "simulated series unlabelled");
    }

    /// A golden file's `node_order` carries branch currents alongside node voltages, and the
    /// overlay's y-axis is volts. Those columns are dropped from the *picture* (not from the
    /// gate, which still scores them): drawing amps against a volt axis puts a milliamp trace
    /// flat along the bottom while the legend calls it a voltage.
    #[test]
    fn an_overlay_leaves_branch_currents_off_a_voltage_axis() {
        let with_current = |points: Vec<(f64, Vec<f64>)>| GoldenSweep {
            source: "V1".into(),
            node_order: vec!["in".into(), "out".into(), "I(V1)".into()],
            points,
        };
        // The current column is ~1000x the voltages: were it drawn, it would set the y-range
        // and squash both real curves into a line.
        let sim = with_current(vec![
            (0.0, vec![0.0, 0.0, 0.0]),
            (1.0, vec![1.0, 0.5, 1000.0]),
        ]);
        let gold = with_current(vec![
            (0.0, vec![0.0, 0.0, 0.0]),
            (1.0, vec![1.0, 0.49, 1000.0]),
        ]);
        let path = tmp("va_harness_overlay_branch_current.svg");
        overlay_sweep(&path, &sim, &gold).expect("renders");
        let svg = std::fs::read_to_string(&path).expect("reads back");
        let _ = std::fs::remove_file(&path);

        assert!(
            svg.contains("V(out)"),
            "voltage series should still be drawn"
        );
        assert!(
            !svg.contains("I(V1)"),
            "a branch current must not be labelled on a voltage axis"
        );
        // The y-axis must still be scaled to the voltages: an axis reaching 1000 would mean
        // the current column had been plotted after all.
        assert!(
            !svg.contains(">1000<") && !svg.contains(">800<"),
            "y-axis was scaled by the branch current, not the voltages"
        );
    }

    /// A mismatched point count is refused rather than drawn misaligned — the same contract
    /// `compare_dc_sweep` enforces before it will score a number.
    #[test]
    fn a_sweep_overlay_refuses_mismatched_point_counts() {
        let sim = sweep(vec![(0.0, vec![0.0, 0.0]), (1.0, vec![1.0, 0.5])]);
        let gold = sweep(vec![(0.0, vec![0.0, 0.0])]);
        let path = tmp("va_harness_overlay_mismatch.svg");
        assert!(matches!(
            overlay_sweep(&path, &sim, &gold),
            Err(HarnessError::LengthMismatch { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }

    /// The golden run is on a coarser time grid than the simulated one; the overlay must
    /// resample it onto the simulated timebase — the grid `compare_tran` scores against —
    /// rather than refuse it or draw it misaligned.
    #[test]
    fn tran_overlay_resamples_golden_onto_the_simulated_timebase() {
        let sim = GoldenTran {
            node_order: vec!["out".into()],
            points: vec![
                (0.0, vec![0.0]),
                (0.5, vec![0.5]),
                (1.0, vec![1.0]),
                (1.5, vec![1.5]),
            ],
        };
        // Two points spanning the same interval: linear resampling must reproduce the ramp.
        let gold = GoldenTran {
            node_order: vec!["out".into()],
            points: vec![(0.0, vec![0.0]), (1.5, vec![1.5])],
        };
        let path = tmp("va_harness_overlay_tran.svg");
        overlay_tran(&path, &sim, &gold).expect("renders");
        let svg = std::fs::read_to_string(&path).expect("reads back");
        let _ = std::fs::remove_file(&path);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("golden (QSPICE)"));
        assert!(svg.contains("V(out)"));
    }

    /// An empty run is refused rather than producing a blank canvas.
    #[test]
    fn an_empty_run_is_refused() {
        let empty = GoldenTran {
            node_order: vec!["out".into()],
            points: vec![],
        };
        let ok = GoldenTran {
            node_order: vec!["out".into()],
            points: vec![(0.0, vec![0.0])],
        };
        let path = tmp("va_harness_overlay_empty.svg");
        assert!(overlay_tran(&path, &empty, &ok).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
