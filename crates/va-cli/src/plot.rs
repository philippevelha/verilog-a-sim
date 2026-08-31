//! Waveform plotting, via `plotters`' SVG backend only (`CLAUDE.md` §5: pure-Rust, no
//! native-link deps — the bitmap backend pulls in font-rasterization deps for no benefit
//! here). Decision recorded in `docs/roadmap.md`'s Quarto-tutorials conventions.
//!
//! Two entry points: [`plot_transient`] for a waveform against time, and [`plot_sweep`] for a
//! `.dc` sweep against the swept source's value. A DC *operating point* remains unplottable —
//! it is a single point, not a curve — so asking for one is still a clear error rather than an
//! empty image.

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

/// Render every node's voltage across a `.dc` sweep as an SVG line chart at `path`.
///
/// `points` is [`crate::solve_dc_sweep`]'s output: the swept source value paired with the
/// operating point solved there. The x-axis is that value, so the familiar diode I–V or MOS
/// transfer curve comes out the right way round.
///
/// Only **node voltages** are drawn — `net.node_order.len()` entries — deliberately. An
/// operating point's vector continues past them into branch-current unknowns, and a diode
/// sweep puts volts (0…0.6) beside amps (~1e-13) on one linear axis, where the smaller series
/// is invisible. Currents need a second axis to be worth drawing; until there is one, leaving
/// them out is honest and an empty-looking chart is not.
///
/// # Errors
///
/// Returns an error if `points` is empty, or if drawing/writing the SVG fails.
pub fn plot_sweep(
    path: &str,
    net: &Netlist,
    sweep: &va_netlist::DcSweep,
    points: &[(f64, va_core::dc::OperatingPoint)],
) -> Result<()> {
    let x_min = points.first().context("sweep has no points to plot")?.0;
    let x_max = points.last().context("sweep has no points to plot")?.0;
    // A one-point sweep, or `start == stop`, would collapse the x-axis.
    let (x_min, x_max) = if x_max > x_min {
        (x_min, x_max)
    } else {
        (x_min - 1.0, x_max + 1.0)
    };

    let n_nodes = net.node_order.len();
    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (_, op) in points {
        for &v in op.x.iter().take(n_nodes) {
            y_min = y_min.min(v);
            y_max = y_max.max(v);
        }
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
        .with_context(|| format!("initializing SVG canvas at {path}"))?;

    let mut chart = ChartBuilder::on(&root)
        .caption("DC sweep", ("sans-serif", 24))
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(55)
        .build_cartesian_2d(x_min..x_max, y_min..y_max)
        .context("building the chart coordinate system")?;

    chart
        .configure_mesh()
        .x_desc(format!("{} (V)", sweep.source))
        .y_desc("Voltage (V)")
        .draw()
        .context("drawing the chart mesh")?;

    for (i, name) in net.node_order.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        chart
            .draw_series(LineSeries::new(
                points.iter().map(|(v, op)| (*v, op.x[i])),
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

/// Write a Bode plot of an AC sweep to `path` as SVG: magnitude in dB and phase in degrees,
/// stacked, sharing a logarithmic frequency axis.
///
/// Two panels rather than one because the two quantities share no units and no useful scale;
/// a decibel magnitude and a degree phase forced onto one linear axis is the same mistake
/// `va_harness::plot` avoids by keeping branch currents off a voltage axis.
///
/// # Limitations
///
/// Plots node voltages only (the first `net.node_order.len()` unknowns), so a branch-current
/// unknown is not drawn — same rule [`plot_sweep`] follows. A frequency point at or below zero
/// cannot appear on a logarithmic axis and is skipped; a sweep with no usable point left is an
/// error rather than an empty canvas. Magnitude is `20*log10(|H|)`, with an exact zero clamped
/// to a floor rather than plotted at negative infinity.
///
/// # Errors
///
/// Returns an error if the response is empty, if every frequency point is non-positive, or if
/// drawing/writing the SVG fails.
pub fn plot_ac(path: &str, net: &Netlist, resp: &va_acnoise::ac::AcResponse) -> Result<()> {
    let n_nodes = net.node_order.len();
    let usable: Vec<usize> = (0..resp.f.len()).filter(|&i| resp.f[i] > 0.0).collect();
    if usable.is_empty() || n_nodes == 0 {
        anyhow::bail!("AC response has no positive frequency point to plot");
    }

    // -400 dB is far below any physically meaningful response and keeps an exact zero (a node
    // held at ground, say) on the canvas instead of sending the axis to negative infinity.
    const DB_FLOOR: f64 = -400.0;
    let db = |c: va_acnoise::ac::Complex| {
        let mag = (c.0 * c.0 + c.1 * c.1).sqrt();
        if mag > 0.0 {
            (20.0 * mag.log10()).max(DB_FLOOR)
        } else {
            DB_FLOOR
        }
    };
    let deg = |c: va_acnoise::ac::Complex| c.1.atan2(c.0).to_degrees();

    let f_min = resp.f[*usable.first().expect("non-empty")];
    let f_max = resp.f[*usable.last().expect("non-empty")];
    let (f_min, f_max) = if f_max > f_min {
        (f_min, f_max)
    } else {
        (f_min * 0.9, f_max * 1.1)
    };

    let (mut db_min, mut db_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut ph_min, mut ph_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &i in &usable {
        for c in resp.x[i].iter().take(n_nodes) {
            db_min = db_min.min(db(*c));
            db_max = db_max.max(db(*c));
            ph_min = ph_min.min(deg(*c));
            ph_max = ph_max.max(deg(*c));
        }
    }
    let pad = |lo: &mut f64, hi: &mut f64| {
        if *hi <= *lo {
            *lo -= 1.0;
            *hi += 1.0;
        }
        let p = 0.05 * (*hi - *lo);
        *lo -= p;
        *hi += p;
    };
    pad(&mut db_min, &mut db_max);
    pad(&mut ph_min, &mut ph_max);

    let root = SVGBackend::new(path, (960, 640)).into_drawing_area();
    root.fill(&WHITE)
        .with_context(|| format!("initializing SVG canvas at {path}"))?;
    let (top, bottom) = root.split_vertically(320);

    let mut mag_chart = ChartBuilder::on(&top)
        .caption("AC response", ("sans-serif", 24))
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(60)
        .build_cartesian_2d((f_min..f_max).log_scale(), db_min..db_max)
        .context("building the magnitude chart")?;
    mag_chart
        .configure_mesh()
        .x_desc("Frequency (Hz)")
        .y_desc("Magnitude (dB)")
        .draw()
        .context("drawing the magnitude mesh")?;

    let mut ph_chart = ChartBuilder::on(&bottom)
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(60)
        .build_cartesian_2d((f_min..f_max).log_scale(), ph_min..ph_max)
        .context("building the phase chart")?;
    ph_chart
        .configure_mesh()
        .x_desc("Frequency (Hz)")
        .y_desc("Phase (deg)")
        .draw()
        .context("drawing the phase mesh")?;

    for (i, name) in net.node_order.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        mag_chart
            .draw_series(LineSeries::new(
                usable.iter().map(|&k| (resp.f[k], db(resp.x[k][i]))),
                &color,
            ))
            .with_context(|| format!("drawing the magnitude series for {name}"))?
            .label(format!("V({name})"))
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        ph_chart
            .draw_series(LineSeries::new(
                usable.iter().map(|&k| (resp.f[k], deg(resp.x[k][i]))),
                &color,
            ))
            .with_context(|| format!("drawing the phase series for {name}"))?;
    }

    mag_chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()
        .context("drawing the legend")?;

    root.present()
        .with_context(|| format!("writing the SVG to {path}"))?;
    Ok(())
}

/// Write a noise-spectrum plot to `path` as SVG: output-referred PSD against frequency, on a
/// log-log pair of axes, with the input-referred spectrum alongside it when one was computed.
///
/// Log-log because that is the shape noise actually has: thermal noise is flat, flicker noise
/// is a straight line of slope -1 per decade, and a linear axis renders both as a spike at the
/// left edge. Plotting `V^2/Hz` directly rather than converting to `V/sqrt(Hz)` keeps it the
/// same quantity the golden files and QSPICE's `onoise_spectrum` carry, so the picture and the
/// gate are reading the same numbers.
///
/// # Limitations
///
/// Non-positive or non-finite points cannot appear on a logarithmic axis and are skipped — a
/// zero PSD, or the infinity `input_psd` reports where the input has no path to the output.
/// A spectrum with no usable point left is an error rather than an empty canvas. Per-device
/// contributions (`per_instance`) are not drawn: the totals are what the gate scores, and one
/// curve per device would need a legend keyed by an index this layer would have to map back.
///
/// # Errors
///
/// Returns an error if nothing is plottable, or if drawing/writing the SVG fails.
pub fn plot_noise(path: &str, spectrum: &va_acnoise::noise::NoiseSpectrum) -> Result<()> {
    let usable: Vec<usize> = (0..spectrum.f.len())
        .filter(|&i| {
            spectrum.f[i] > 0.0
                && spectrum
                    .psd
                    .get(i)
                    .is_some_and(|p| *p > 0.0 && p.is_finite())
        })
        .collect();
    if usable.is_empty() {
        anyhow::bail!("noise spectrum has no positive, finite point to plot");
    }

    let has_input = !spectrum.input_psd.is_empty();
    let input_usable: Vec<usize> = if has_input {
        usable
            .iter()
            .copied()
            .filter(|&i| {
                spectrum
                    .input_psd
                    .get(i)
                    .is_some_and(|p| *p > 0.0 && p.is_finite())
            })
            .collect()
    } else {
        Vec::new()
    };

    let f_min = spectrum.f[usable[0]];
    let f_max = spectrum.f[*usable.last().expect("non-empty")];
    let (f_min, f_max) = if f_max > f_min {
        (f_min, f_max)
    } else {
        (f_min * 0.9, f_max * 1.1)
    };

    let (mut p_min, mut p_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &i in usable.iter().chain(input_usable.iter()) {
        p_min = p_min.min(spectrum.psd[i]);
        p_max = p_max.max(spectrum.psd[i]);
        if let Some(v) = spectrum.input_psd.get(i) {
            if *v > 0.0 && v.is_finite() {
                p_min = p_min.min(*v);
                p_max = p_max.max(*v);
            }
        }
    }
    // A flat spectrum (a plain resistor) would collapse the y-axis; widen it by a decade
    // either side so the flatness is visible as flatness rather than as a divide-by-zero.
    if p_max <= p_min {
        p_min *= 0.1;
        p_max *= 10.0;
    }

    let root = SVGBackend::new(path, (960, 540)).into_drawing_area();
    root.fill(&WHITE)
        .with_context(|| format!("initializing SVG canvas at {path}"))?;

    let mut chart = ChartBuilder::on(&root)
        .caption("Noise spectrum", ("sans-serif", 24))
        .margin(15)
        .x_label_area_size(35)
        .y_label_area_size(75)
        .build_cartesian_2d((f_min..f_max).log_scale(), (p_min..p_max).log_scale())
        .context("building the chart coordinate system")?;

    chart
        .configure_mesh()
        .x_desc("Frequency (Hz)")
        .y_desc("PSD (V^2/Hz)")
        .draw()
        .context("drawing the chart mesh")?;

    let out_color = PALETTE[0];
    chart
        .draw_series(LineSeries::new(
            usable.iter().map(|&i| (spectrum.f[i], spectrum.psd[i])),
            &out_color,
        ))
        .context("drawing the output-noise series")?
        .label("output-referred")
        .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], out_color));

    if !input_usable.is_empty() {
        let in_color = PALETTE[1 % PALETTE.len()];
        chart
            .draw_series(LineSeries::new(
                input_usable
                    .iter()
                    .map(|&i| (spectrum.f[i], spectrum.input_psd[i])),
                &in_color,
            ))
            .context("drawing the input-referred series")?
            .label("input-referred")
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], in_color));
    }

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()
        .context("drawing the legend")?;

    root.present()
        .with_context(|| format!("writing the SVG to {path}"))?;
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

    #[test]
    fn plots_a_dc_sweep_to_a_real_file() {
        let net = parse(include_str!("../../../circuits/diode_iv.net")).expect("parse diode_iv");
        let sweep = net.dc.clone().expect("diode_iv.net carries a .dc card");
        let n = net.node_order.len();
        let points: Vec<(f64, va_core::dc::OperatingPoint)> = vec![
            (0.0, va_core::dc::OperatingPoint { x: vec![0.0; n] }),
            (0.3, va_core::dc::OperatingPoint { x: vec![0.3; n] }),
            (0.6, va_core::dc::OperatingPoint { x: vec![0.6; n] }),
        ];

        let dir = std::env::temp_dir().join("va-cli-plot-test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("diode_iv_sweep.svg");
        let path_str = path.to_str().expect("utf8 path");

        plot_sweep(path_str, &net, &sweep, &points).expect("plots without error");

        let contents = std::fs::read_to_string(&path).expect("reads back the SVG");
        assert!(contents.starts_with("<?xml") || contents.contains("<svg"));
        assert!(contents.contains("V(in)"), "node series unlabelled");
        assert!(
            contents.contains(&sweep.source),
            "swept source name missing from the x-axis label"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// The Bode plot draws both panels, labels both axes, and names each node series. Uses a
    /// real solved RC response rather than fabricated numbers, so the magnitudes on the canvas
    /// are the ones the analysis actually produced.
    #[test]
    fn ac_plot_draws_magnitude_and_phase_panels() {
        let net = parse(include_str!("../../../circuits/rc_ac.net")).expect("parse rc_ac");
        let resp = crate::solve_ac(&net, &[]).expect("solves");
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-ac.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();
        plot_ac(&path, &net, &resp).expect("renders");
        let contents = std::fs::read_to_string(&path).expect("reads back");
        let _ = std::fs::remove_file(&path);

        assert!(contents.contains("<svg"), "not an SVG");
        assert!(
            contents.contains("Magnitude (dB)"),
            "magnitude panel missing"
        );
        assert!(contents.contains("Phase (deg)"), "phase panel missing");
        assert!(
            contents.contains("Frequency (Hz)"),
            "frequency axis unlabelled"
        );
        assert!(contents.contains("V(out)"), "node series unlabelled");
    }

    /// The noise plot draws both referred spectra and labels the log-log axes. Uses a real
    /// solved spectrum so the curve is the one the analysis produced.
    #[test]
    fn noise_plot_draws_both_referred_spectra() {
        // `diode_noise.net` resolves its diode to the hand-written `va-abi` reference, so no
        // compiled Verilog-A model is needed to get a real spectrum here.
        let net = parse(include_str!("../../../circuits/diode_noise.net")).expect("parse");
        let spectrum = crate::solve_noise(&net, &[]).expect("solves");
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-noise.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();
        plot_noise(&path, &spectrum).expect("renders");
        let contents = std::fs::read_to_string(&path).expect("reads back");
        let _ = std::fs::remove_file(&path);

        assert!(contents.contains("<svg"), "not an SVG");
        assert!(contents.contains("PSD (V^2/Hz)"), "y-axis unlabelled");
        assert!(contents.contains("Frequency (Hz)"), "x-axis unlabelled");
        assert!(
            contents.contains("output-referred"),
            "output series unlabelled"
        );
        assert!(
            contents.contains("input-referred"),
            "input series unlabelled"
        );
    }

    /// Points a logarithmic axis cannot show are skipped rather than drawn, and a spectrum
    /// with none left is an error rather than a blank canvas — including the infinity
    /// `input_psd` reports where the input has no path to the output.
    #[test]
    fn a_noise_plot_skips_what_a_log_axis_cannot_show() {
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-noise-degenerate.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();

        let empty = va_acnoise::noise::NoiseSpectrum::default();
        assert!(plot_noise(&path, &empty).is_err(), "an empty spectrum");

        let all_zero = va_acnoise::noise::NoiseSpectrum {
            f: vec![1.0, 10.0],
            psd: vec![0.0, 0.0],
            ..Default::default()
        };
        assert!(
            plot_noise(&path, &all_zero).is_err(),
            "a zero PSD has no place on a log axis"
        );

        // A usable output spectrum with an unusable input one still plots: the input series is
        // dropped, not the whole figure.
        let mixed = va_acnoise::noise::NoiseSpectrum {
            f: vec![1.0, 10.0, 100.0],
            psd: vec![1e-18, 1e-18, 1e-18],
            input_psd: vec![f64::INFINITY, 1e-16, f64::INFINITY],
            ..Default::default()
        };
        plot_noise(&path, &mixed).expect("renders with a partial input series");
        let _ = std::fs::remove_file(&path);
    }

    /// A response with nothing plottable is an error, not a blank canvas: a logarithmic
    /// frequency axis cannot show a point at or below zero, and a sweep consisting only of
    /// those has nothing left to draw.
    #[test]
    fn an_ac_plot_with_no_positive_frequency_is_an_error() {
        let net = parse(include_str!("../../../circuits/rc_ac.net")).expect("parse rc_ac");
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-ac-empty.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();

        let empty = va_acnoise::ac::AcResponse::default();
        assert!(plot_ac(&path, &net, &empty).is_err(), "empty response");

        let dc_only = va_acnoise::ac::AcResponse {
            f: vec![0.0],
            x: vec![vec![(1.0, 0.0); net.node_order.len()]],
        };
        assert!(
            plot_ac(&path, &net, &dc_only).is_err(),
            "a zero-frequency-only sweep has nothing a log axis can show"
        );
    }

    /// An empty sweep is refused rather than producing a blank/zero-width canvas — the same
    /// contract [`plot_transient`] enforces for an empty waveform.
    #[test]
    fn empty_sweep_is_an_error_not_a_panic() {
        let net = parse(include_str!("../../../circuits/diode_iv.net")).expect("parse diode_iv");
        let sweep = net.dc.clone().expect("diode_iv.net carries a .dc card");
        let path = std::env::temp_dir()
            .join("va-cli-plot-test-empty-sweep.svg")
            .to_str()
            .expect("utf8 path")
            .to_string();
        assert!(plot_sweep(&path, &net, &sweep, &[]).is_err());
    }
}
