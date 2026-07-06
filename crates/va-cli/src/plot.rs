//! Waveform plotting, via `plotters`' SVG backend only (`CLAUDE.md` §5: pure-Rust, no
//! native-link deps — the bitmap backend pulls in font-rasterization deps for no benefit
//! here). Decision recorded in `docs/roadmap.md`'s Quarto-tutorials conventions.
//!
//! Transient only in v0: a DC operating point is a single point, not a waveform, and plotting
//! one isn't useful — [`plot_transient`] is the only entry point.

use anyhow::{Context, Result};
use plotters::prelude::*;
use va_netlist::Netlist;
use va_transient::integrator::Waveform;

/// A small fixed palette, cycled by node index. Plain `RGBColor`s rather than `Palette99`
/// (fewer moving parts, no dependency on exactly which palette plotters ships) — plenty for
/// the handful of nodes any circuit in this project's zoo has.
const PALETTE: [RGBColor; 6] = [RED, BLUE, GREEN, MAGENTA, CYAN, BLACK];

/// Render every node's voltage over time as an SVG line chart at `path`.
///
/// # Errors
///
/// Returns an error if `wf` has no accepted points, or if drawing/writing the SVG fails.
pub fn plot_transient(path: &str, net: &Netlist, wf: &Waveform) -> Result<()> {
    let t_min = *wf.t.first().context("waveform has no points to plot")?;
    let t_max = *wf.t.last().context("waveform has no points to plot")?;

    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in &wf.x {
        for &v in x.iter().take(net.node_order.len()) {
            y_min = y_min.min(v);
            y_max = y_max.max(v);
        }
    }
    // A flat signal (e.g. a single-node circuit sitting at 0 V throughout) would otherwise
    // collapse the y-axis to a zero-height range.
    if y_max <= y_min {
        y_min -= 1.0;
        y_max += 1.0;
    }
    let pad = 0.05 * (y_max - y_min);
    y_min -= pad;
    y_max += pad;

    let root = SVGBackend::new(path, (960, 540)).into_drawing_area();
    root.fill(&WHITE)
        .with_context(|| format!("initializing SVG canvas at {path}"))?;

    let mut chart = ChartBuilder::on(&root)
        .caption("Transient analysis", ("sans-serif", 24))
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(55)
        .build_cartesian_2d(t_min..t_max, y_min..y_max)
        .context("building the chart coordinate system")?;

    chart
        .configure_mesh()
        .x_desc("Time (s)")
        .y_desc("Voltage (V)")
        .draw()
        .context("drawing the chart mesh")?;

    for (i, name) in net.node_order.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        chart
            .draw_series(LineSeries::new(
                wf.t.iter().zip(&wf.x).map(|(&t, x)| (t, x[i])),
                &color,
            ))
            .with_context(|| format!("drawing V({name})"))?
            .label(format!("V({name})"))
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
    }

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()
        .context("drawing the legend")?;

    root.present()
        .with_context(|| format!("writing SVG to {path}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_netlist::parser::parse;

    #[test]
    fn plots_the_rc_step_waveform_to_a_real_file() {
        let net = parse(include_str!("../../../circuits/rc_step.net")).expect("parse rc_step");
        let wf = Waveform {
            t: vec![0.0, 1e-3, 2e-3],
            x: vec![
                vec![5.0, 0.0, 0.0],
                vec![5.0, 3.16, -0.5],
                vec![5.0, 4.32, -0.2],
            ],
            crossings: Vec::new(),
        };

        let dir = std::env::temp_dir().join("va-cli-plot-test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("rc_step.svg");
        let path_str = path.to_str().expect("utf8 path");

        plot_transient(path_str, &net, &wf).expect("plots without error");

        let contents = std::fs::read_to_string(&path).expect("reads back the SVG");
        assert!(contents.starts_with("<?xml") || contents.contains("<svg"));
        assert!(contents.contains("V(in)"));
        assert!(contents.contains("V(out)"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_waveform_is_an_error_not_a_panic() {
        let net = parse(include_str!("../../../circuits/rc_step.net")).expect("parse rc_step");
        let empty = Waveform {
            t: Vec::new(),
            x: Vec::new(),
            crossings: Vec::new(),
        };
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-empty.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();
        assert!(plot_transient(&path, &net, &empty).is_err());
    }
}
