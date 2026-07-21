//! Drive a `.tran` transient run ([`run_tran`]/[`compare_tran`], § ladder rungs 3/4/6) through
//! `va-cli` and compare it against golden. Unlike [`crate::dc`]'s DC/sweep comparisons, a
//! transient comparison can't assume row-for-row alignment: this project's own adaptive-timestep
//! integrator and QSPICE's own essentially never land on the same time points, so [`compare_tran`]
//! resamples the freshly-computed run onto the golden reference's own timebase first
//! ([`crate::metrics::resample_linear`]).

use crate::golden::GoldenTran;
use crate::{metrics, tol, HarnessError, Verdict};

/// Solve `circuit`'s `.tran` transient response (optionally through a compiled Verilog-A
/// `model`) and package it as a [`GoldenTran`].
///
/// # Errors
///
/// [`HarnessError::Run`] if the netlist/model can't be read or parsed, the deck has no `.tran`
/// card, or the integration diverges.
pub fn run_tran(circuit: &str, model: Option<&str>) -> Result<GoldenTran, HarnessError> {
    let (net, compiled) =
        va_cli::load(circuit, model).map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let wf = va_cli::solve_transient(&net, &compiled)
        .map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    let branch_currents = va_cli::branch_currents(&net, &compiled)
        .map_err(|e| HarnessError::Run(format!("{e:#}")))?;
    Ok(GoldenTran::from_waveform(
        &net.node_order,
        &wf.t,
        &wf.x,
        &branch_currents,
    ))
}

/// Compare a freshly-computed transient run against its golden reference (§7's transient
/// metric): each node's freshly-computed series is linearly resampled onto the golden's own
/// sample times, then the RMS error is taken over every node's resampled series flattened into
/// one long series (the same "flatten across nodes/points" shape [`crate::dc::compare_dc_sweep`]
/// already uses for a `.dc` sweep).
///
/// `got`'s own first point is always excluded, both from resampling *and* from the golden
/// window compared against. `va_transient::integrator::run`/`run_with_events` build their
/// `Waveform` as `t: vec![cfg.tstart], x: vec![x0.clone()]` before integrating a single step —
/// index 0 is *definitionally* the caller-supplied seed, not a solved sample, and
/// `va-cli::solve_transient` always seeds `x0 = 0` (no `.ic`/`UIC` support, per its own doc
/// comment). That seed misrepresents any node an algebraic/source equation forces (e.g.
/// `circuits/rc_step.net`'s `V(in)`, held at its source value `5` at every genuinely solved step,
/// but `0` in the raw seed) — QSPICE's own `t=0` sample, by contrast, is a real,
/// algebraically-consistent solve (confirmed via `UIC`, § `xtask`'s `cold_start_tran_deck`).
///
/// Two things had to be true together, both found empirically comparing `rc_step.net` (a
/// constant-forced node) against `rectifier.net` (a smoothly-varying `SIN` node) side by side:
/// dropping only `got`'s seed *sample* but still resampling every golden time (including
/// QSPICE's own densely-clustered early adaptive-step samples, far finer than `got`'s own first
/// real step) clamps that whole early region to `got`'s first real value — fine for `rc_step.net`
/// (already settled to its forced value by then) but wrong for `rectifier.net`'s fast-changing
/// early sine, which raised its own error from `7.8e-4` to `3.1e-2`. Restricting the golden
/// window to `t >= got`'s own first real sample time closes that gap too: it's the same "don't
/// ask a coarser series to explain a region it never resolved" principle, applied to the golden
/// side instead of the got side.
///
/// # Errors
///
/// [`HarnessError::NodeOrderMismatch`] if the two don't describe the same nodes in the same
/// order; [`HarnessError::Run`]-shaped inputs (an empty run) surface as an empty-series
/// zero-error comparison, matching [`metrics::rms_error`]'s own empty-input convention, since an
/// empty transient run isn't this function's own error to raise.
pub fn compare_tran(got: &GoldenTran, golden: &GoldenTran) -> Result<Verdict, HarnessError> {
    if got.node_order != golden.node_order {
        return Err(HarnessError::NodeOrderMismatch {
            got: got.node_order.clone(),
            expected: golden.node_order.clone(),
        });
    }
    let got_points = if got.points.len() > 1 {
        &got.points[1..]
    } else {
        &got.points[..]
    };
    let got_times: Vec<f64> = got_points.iter().map(|(t, _)| *t).collect();
    let first_real_t = got_times.first().copied();
    let golden_points: Vec<&(f64, Vec<f64>)> = golden
        .points
        .iter()
        .filter(|(t, _)| first_real_t.is_none_or(|ft| *t >= ft))
        .collect();
    let golden_times: Vec<f64> = golden_points.iter().map(|(t, _)| *t).collect();

    if got_times.is_empty() || golden_times.is_empty() {
        return Ok(Verdict::new(0.0, tol::TRAN_RMS));
    }

    let mut got_flat = Vec::new();
    let mut golden_flat = Vec::new();
    for node_i in 0..golden.node_order.len() {
        let golden_series: Vec<f64> = golden_points.iter().map(|(_, v)| v[node_i]).collect();
        let got_series: Vec<f64> = got_points.iter().map(|(_, v)| v[node_i]).collect();
        let resampled = metrics::resample_linear(&got_times, &got_series, &golden_times);
        got_flat.extend(resampled);
        golden_flat.extend(golden_series);
    }
    let error = metrics::rms_error(&got_flat, &golden_flat)?;
    Ok(Verdict::new(error, tol::TRAN_RMS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_path(rel: &str) -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel
    }

    #[test]
    fn run_tran_solves_the_rc_step() {
        let g = run_tran(&workspace_path("circuits/rc_step.net"), None).expect("solve rc_step");
        assert_eq!(g.node_order, vec!["in", "out", "I(V1)"]);
        assert!(g.points.len() > 1);
        let (t0, v0) = &g.points[0];
        assert_eq!(*t0, 0.0);
        // v0 has no `.ic`/`UIC` support (`va-cli::solve_transient`'s own doc comment): every
        // unknown, including a directly-forced source node and its own branch current, starts
        // from the zero vector, not its own DC value — so `V(in)=0` at `t=0` too, not the
        // source's `DC 5.0`.
        assert_eq!(v0, &vec![0.0, 0.0, 0.0]);
        let (t_last, v_last) = g.points.last().unwrap();
        assert!((t_last - 5e-3).abs() < 1e-9, "t_last = {t_last}");
        // RC=1ms, tstop=5ms=5*RC -> V(out) within ~1% of the 5V rail.
        assert!(
            (v_last[1] - 5.0).abs() < 0.1,
            "V(out) at tstop = {}",
            v_last[1]
        );
    }

    #[test]
    fn compare_tran_ignores_gots_seed_for_an_algebraically_forced_node() {
        // `got`'s "seed" sample (index 0) is `0`, wrong for a node an ideal source forces to `5`
        // immediately — but every genuinely-solved sample (from index 1 onward) already shows
        // the correct forced value. Without dropping the seed, this would fail large (the real
        // bug this test guards: `rc_step.net` measured `1.097e-1` before this fix, `2.3e-5`
        // after).
        let got = GoldenTran {
            node_order: vec!["in".to_string()],
            points: vec![(0.0, vec![0.0]), (1.0, vec![5.0]), (2.0, vec![5.0])],
        };
        let golden = GoldenTran {
            node_order: vec!["in".to_string()],
            points: vec![(0.0, vec![5.0]), (1.0, vec![5.0]), (2.0, vec![5.0])],
        };
        let verdict = compare_tran(&got, &golden).expect("compare");
        assert!(verdict.passed, "error = {}", verdict.error);
    }

    #[test]
    fn compare_tran_excludes_golden_samples_earlier_than_gots_first_real_step() {
        // `golden` has early, densely-clustered samples in a region `got` never resolved (its
        // own first real sample is at t=10) — clamping those early golden samples to `got`'s
        // first real value (100) rather than excluding them would be a large, wrong comparison
        // (the real bug this test guards: dropping only `got`'s seed without also windowing
        // `golden` raised `rectifier.net`'s error from `7.8e-4` to `3.1e-2`).
        let got = GoldenTran {
            node_order: vec!["out".to_string()],
            points: vec![(0.0, vec![0.0]), (10.0, vec![100.0])],
        };
        let golden = GoldenTran {
            node_order: vec!["out".to_string()],
            points: vec![
                (0.0, vec![0.0]),
                (1.0, vec![1.0]),
                (5.0, vec![50.0]),
                (10.0, vec![100.0]),
            ],
        };
        let verdict = compare_tran(&got, &golden).expect("compare");
        assert!(verdict.passed, "error = {}", verdict.error);
    }

    #[test]
    fn compare_tran_passes_for_an_identical_reference() {
        let got = run_tran(&workspace_path("circuits/rc_step.net"), None).expect("solve rc_step");
        let verdict = compare_tran(&got, &got).expect("compare");
        assert!(verdict.passed);
        assert_eq!(verdict.error, 0.0);
    }

    #[test]
    fn compare_tran_passes_against_a_resampled_but_equivalent_waveform() {
        // A coarser, differently-timed but otherwise-identical RC(1ms) charging curve should
        // still pass — this is the whole point of resampling onto a shared timebase rather than
        // requiring the two runs to land on the same time points.
        let rc = |t: f64| 5.0 * (1.0 - (-t / 1e-3_f64).exp());
        let golden_times: Vec<f64> = (0..=50).map(|i| i as f64 * 1e-4).collect();
        let golden = GoldenTran {
            node_order: vec!["out".to_string()],
            points: golden_times.iter().map(|&t| (t, vec![rc(t)])).collect(),
        };
        let got_times: Vec<f64> = (0..=500).map(|i| i as f64 * 1e-5).collect();
        let got = GoldenTran {
            node_order: vec!["out".to_string()],
            points: got_times.iter().map(|&t| (t, vec![rc(t)])).collect(),
        };
        let verdict = compare_tran(&got, &golden).expect("compare");
        assert!(verdict.passed, "error = {}", verdict.error);
    }

    #[test]
    fn compare_tran_fails_for_a_diverged_reference() {
        let got = run_tran(&workspace_path("circuits/rc_step.net"), None).expect("solve rc_step");
        let mut golden = got.clone();
        let last = golden.points.len() - 1;
        golden.points[last].1[1] = 0.0; // V(out) at tstop wildly wrong
        let verdict = compare_tran(&got, &golden).expect("compare");
        assert!(!verdict.passed);
    }

    #[test]
    fn compare_tran_rejects_a_node_order_mismatch() {
        let got = GoldenTran {
            node_order: vec!["in".to_string(), "out".to_string()],
            points: vec![(0.0, vec![5.0, 0.0])],
        };
        let golden = GoldenTran {
            node_order: vec!["out".to_string(), "in".to_string()],
            points: vec![(0.0, vec![0.0, 5.0])],
        };
        assert!(compare_tran(&got, &golden).is_err());
    }

    #[test]
    fn run_tran_errors_on_a_deck_with_no_tran_card() {
        assert!(run_tran(&workspace_path("circuits/divider.net"), None).is_err());
    }
}
