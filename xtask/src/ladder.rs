//! `cargo xtask ladder <deck> [--model <m>]` — what the DC rescue costs on a deck, and what
//! alternative rescue schedules would cost instead (option C of `docs/proposals/btf-solver.md`,
//! `docs/proposals/dc-rescue.md`).
//!
//! It replays the rescue `va_core::dc` runs — plain Newton, then the `gmin` ladder with the
//! node-step cap, then the plain ladder (the order since 1.23.0; damping, the last tier, is not
//! replayed) — as a sequence of `va_core::newton::solve` calls, and then
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
use va_core::newton::{solve, NewtonConfig};

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
    let (boxed, dim) = va_cli::instances(&net, &compiled)?;
    let loads = AtomicUsize::new(0);
    let first = Counted {
        inner: boxed[0].as_ref(),
        loads: &loads,
    };
    let mut insts: Vec<&dyn ModelInstance> = vec![&first];
    insts.extend(boxed[1..].iter().map(|b| b.as_ref()));

    let plain = NewtonConfig::default();
    let mut schedules = vec![Schedule {
        name: "current: plain, ladder 30 + cap, ladder 30".into(),
        tiers: vec![plain, ladder(STEPS, true), ladder(STEPS, false)],
    }];
    schedules.push(Schedule {
        name: "before 1.23.0: plain, ladder 30, + cap".into(),
        tiers: vec![plain, ladder(STEPS, false), ladder(STEPS, true)],
    });
    for steps in [5, 10, 15, 20] {
        schedules.push(Schedule {
            name: format!("current tiers, {steps} steps"),
            tiers: vec![plain, ladder(steps, true), ladder(steps, false)],
        });
    }

    println!("{deck}: {dim} unknowns, {} instances", insts.len());
    let mut reference: Option<Vec<f64>> = None;
    for s in &schedules {
        let t0 = Instant::now();
        let mut outcome = String::from("FAILED every tier");
        let mut per_tier = Vec::new();
        let mut answer = None;
        for (k, cfg) in s.tiers.iter().enumerate() {
            let before = loads.load(Ordering::Relaxed);
            let r = solve(&insts, dim, *cfg);
            per_tier.push(loads.load(Ordering::Relaxed) - before);
            if let Ok(x) = r {
                outcome = format!("solved by tier {k}");
                answer = Some(x);
                break;
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
    Ok(())
}
