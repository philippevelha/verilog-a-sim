//! `cargo xtask ladder <deck> [--model <m>] [--current] [--nodeset]` — what the DC rescue costs
//! on a deck (from zero, or from its `.nodeset` hints), and what
//! alternative rescue schedules would cost instead (option C of `docs/proposals/btf-solver.md`,
//! `docs/proposals/dc-rescue.md`).
//!
//! It replays the rescue `va_core::dc` runs — plain Newton, then the adaptive `gmin` ladder with
//! the node-step cap, then without it (the order since 1.23.0, adaptive since 1.24.0; damping,
//! the last tier, is not replayed) — as a sequence of `va_core::newton::solve` calls, and then
//! each variant, counting **assemblies** (one per Newton iteration) with a wrapper on the first
//! instance, which loads exactly once per assembly. Each variant's answer is compared with the
//! current rescue's. `.op` decks only: a `.dc` sweep's rescued point is not isolated here.
//!
//! **Limitations, stated:** the replay mirrors `dc::with_gmin_rescue`'s constants by value
//! (30 steps, 150 iterations, a 0.5 V cap), so it drifts if they change; wall times are single
//! runs.

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use va_abi::ModelInstance;
use va_core::newton::{solve_with_events_from, NewtonConfig};

/// Counts `load` calls on the one instance it wraps; forwards every `ModelInstance` method.
struct Counted<'a> {
    inner: &'a dyn ModelInstance,
    loads: &'a AtomicUsize,
}

impl ModelInstance for Counted<'_> {
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
        self.loads.fetch_add(1, Ordering::Relaxed);
        self.inner.load(x, ctx, state, sink);
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

/// One rescue schedule: the configurations tried in order until one converges.
struct Schedule {
    name: String,
    tiers: Vec<NewtonConfig>,
}

/// `dc::with_gmin_rescue`'s values (see the module limitations).
const STEPS: usize = 30;
const ITERS: usize = 150;
const CAP: f64 = 0.5;

fn ladder(steps: usize, cap: bool) -> NewtonConfig {
    NewtonConfig {
        gmin_steps: steps,
        max_iters: ITERS,
        max_node_step: if cap { CAP } else { f64::INFINITY },
        ..NewtonConfig::default()
    }
}

/// Entry point for `cargo xtask ladder`.
pub fn ladder_cmd(args: &[String]) -> Result<()> {
    let deck = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .context("expected a deck: cargo xtask ladder <deck> [--model <m>]")?;
    let model = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let (net, compiled) = va_cli::load(deck, model).with_context(|| format!("loading {deck}"))?;
    // `--current`: only the rescue as `va_core::dc` runs it, not the alternative schedules.
    // `--nodeset`: every schedule starts from the deck's `.nodeset` hints instead of zero —
    // `docs/proposals/initial-guess.md`, which compares the two.
    let current_only = args.iter().any(|a| a == "--current");
    let from_nodeset = args.iter().any(|a| a == "--nodeset");
    let (boxed, dim) = va_cli::instances(&net, &compiled)?;
    let loads = AtomicUsize::new(0);
    let first = Counted {
        inner: boxed[0].as_ref(),
        loads: &loads,
    };
    let mut insts: Vec<&dyn ModelInstance> = vec![&first];
    insts.extend(boxed[1..].iter().map(|b| b.as_ref()));

    let plain = NewtonConfig::default();
    let adaptive = |cfg: NewtonConfig| NewtonConfig {
        gmin_adaptive: true,
        ..cfg
    };
    let mut schedules = vec![Schedule {
        name: "current: plain, adaptive cap, adaptive".into(),
        tiers: vec![
            plain,
            adaptive(ladder(STEPS, true)),
            adaptive(ladder(STEPS, false)),
        ],
    }];
    schedules.push(Schedule {
        name: "1.23.0: plain, ladder 30 + cap, ladder 30".into(),
        tiers: vec![plain, ladder(STEPS, true), ladder(STEPS, false)],
    });
    schedules.push(Schedule {
        name: "before 1.23.0: plain, ladder 30, + cap".into(),
        tiers: vec![plain, ladder(STEPS, false), ladder(STEPS, true)],
    });
    for steps in [5, 10, 15, 20] {
        schedules.push(Schedule {
            name: format!("1.23.0 tiers, {steps} equal steps"),
            tiers: vec![plain, ladder(steps, true), ladder(steps, false)],
        });
    }

    if current_only {
        schedules.truncate(1);
    }
    let start: Option<Vec<f64>> = if from_nodeset {
        anyhow::ensure!(
            !net.nodesets.is_empty(),
            "--nodeset: {deck} has no `.nodeset` card"
        );
        let mut x = vec![0.0; dim];
        for (name, v) in &net.nodesets {
            let i = *net
                .nodes
                .get(name)
                .with_context(|| format!("`.nodeset`: no net `{name}`"))?;
            x[i] = *v;
        }
        Some(x)
    } else {
        None
    };

    println!(
        "{deck}: {dim} unknowns, {} instances, starting from {}",
        insts.len(),
        if from_nodeset { "the nodesets" } else { "zero" }
    );
    let mut reference: Option<Vec<f64>> = None;
    for s in &schedules {
        let t0 = Instant::now();
        let mut outcome = String::from("FAILED every tier");
        let mut per_tier = Vec::new();
        let mut answer = None;
        for (k, cfg) in s.tiers.iter().enumerate() {
            let before = loads.load(Ordering::Relaxed);
            let r = solve_with_events_from(
                &insts,
                dim,
                *cfg,
                &va_abi::FiredEvents::default(),
                start.as_deref(),
            );
            per_tier.push(loads.load(Ordering::Relaxed) - before);
            match r {
                Ok(x) => {
                    outcome = format!("solved by tier {k}");
                    answer = Some(x);
                    break;
                }
                Err(e) if k == 0 => println!("    plain Newton: {e}"),
                Err(_) => {}
            }
        }
        let total: usize = per_tier.iter().sum();
        let agree = match (&reference, &answer) {
            (Some(r), Some(x)) => {
                let d = r
                    .iter()
                    .zip(x)
                    .fold(0.0_f64, |m, (a, b)| m.max((a - b).abs()));
                format!("; max |x - current| {d:.1e}")
            }
            _ => String::new(),
        };
        println!(
            "  {:44} {outcome}; assemblies {total} ({}); {:.1} s{agree}",
            s.name,
            per_tier
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" + "),
            t0.elapsed().as_secs_f64(),
        );
        if reference.is_none() {
            reference = answer;
        }
    }
    if let Some(x0) = start.as_deref() {
        warm_starts(&insts, dim, x0, &loads, reference.as_deref());
    }
    Ok(())
}

/// With a good starting point, does the rescue still need to start from a 1e-3 S shunt? Each
/// variant starts from `x0` and runs capped plain Newton (the rescue's first tier's settings)
/// with a shunt of `g` from every `Node` row to ground, `g` falling tenfold per stage from `g0`
/// to 1e-12 S, then unshunted. The shunt is `va_abi`'s reference resistor, one per node, so no
/// stamp is written here. `g0 = 0` is capped plain Newton alone. An experiment for
/// `docs/proposals/initial-guess.md`, not a schedule `va_core::dc` runs.
fn warm_starts(
    insts: &[&dyn ModelInstance],
    dim: usize,
    x0: &[f64],
    loads: &AtomicUsize,
    reference: Option<&[f64]>,
) {
    let mut is_node = vec![true; dim];
    let mut seen = vec![false; dim];
    for inst in insts {
        for (i, &u) in inst.unknowns().iter().enumerate() {
            if u < dim {
                seen[u] = true;
                if matches!(inst.unknown_kind(i), va_abi::UnknownKind::Branch) {
                    is_node[u] = false;
                }
            }
        }
    }
    let nodes: Vec<usize> = (0..dim).filter(|&u| seen[u] && is_node[u]).collect();
    let capped = NewtonConfig {
        max_iters: ITERS,
        max_node_step: CAP,
        ..NewtonConfig::default()
    };
    for g0 in [0.0, 1e-6, 1e-9] {
        let t0 = Instant::now();
        let before = loads.load(Ordering::Relaxed);
        let mut stages = Vec::new();
        let mut g = g0;
        while g >= 1e-12 * 0.999 {
            stages.push(g);
            g /= 10.0;
        }
        stages.push(0.0);
        let mut x = x0.to_vec();
        let mut failed = None;
        for &g in &stages {
            let shunts: Vec<va_abi::reference::Resistor> = if g > 0.0 {
                nodes
                    .iter()
                    .map(|&u| {
                        va_abi::reference::Resistor::new(u, va_abi::reference::GROUND, 1.0 / g)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut all: Vec<&dyn ModelInstance> = insts.to_vec();
            all.extend(shunts.iter().map(|r| r as &dyn ModelInstance));
            match solve_with_events_from(
                &all,
                dim,
                capped,
                &va_abi::FiredEvents::default(),
                Some(&x),
            ) {
                Ok(next) => x = next,
                Err(e) => {
                    failed = Some(format!("failed at g = {g:.0e} S: {e}"));
                    break;
                }
            }
        }
        let agree = match (reference, &failed) {
            (Some(r), None) => {
                let d = r
                    .iter()
                    .zip(&x)
                    .fold(0.0_f64, |m, (a, b)| m.max((a - b).abs()));
                format!("; max |x - current| {d:.1e}")
            }
            _ => String::new(),
        };
        println!(
            "  warm start, shunt from {g0:.0e} S ({} stages): {}; assemblies {}; {:.1} s{agree}",
            stages.len(),
            failed.as_deref().unwrap_or("solved"),
            loads.load(Ordering::Relaxed) - before,
            t0.elapsed().as_secs_f64(),
        );
    }
}
