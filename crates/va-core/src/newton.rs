//! Newton–Raphson iteration driver.
//!
//! Each iteration assembles the MNA system at the current `x`, solves `J · dx = −f`, and
//! updates `x += dx` (optionally clamped by [`crate::convergence::limit_junction`] — see
//! `NewtonConfig::limit_junctions`). Convergence is declared when either the residual is below
//! `abstol` or every *applied* update component is within `reltol·|x| + abstol`. For a linear
//! circuit with limiting off this lands in two iterations; for smooth nonlinear devices Newton
//! converges quadratically near the solution.
//!
//! **The `abstol` in the per-unknown "applied update" check is per-unknown**, not always
//! `cfg.abstol`: [`solve`] builds it once via [`crate::mna::classify_abstol`], which lets any
//! [`va_abi::ModelInstance::unknown_abstol`] override its own unknown's tolerance (§
//! nature-metadata wiring — a `va-codegen`-generated model's discipline/nature metadata,
//! ultimately) — every unknown with no override still uses `cfg.abstol`. The residual-norm
//! gate (`residual_norm <= cfg.abstol`, just above) stays a single global scalar — reweighting
//! that `inf_norm` check into a per-row form is a separate design question, out of scope here.
//!
//! [`solve`] also drives an optional outer `gmin`-stepping homotopy (`NewtonConfig::gmin_steps`)
//! around the inner iteration: each stage re-solves with [`crate::mna::System::shunt_gmin`]
//! adding a decreasing conductance ([`crate::convergence::gmin_for_step`]) to every
//! [`va_abi::UnknownKind::Node`] row, warm-starting from the previous stage's solution, ending
//! on an unshunted (`gmin = 0`) solve of the real circuit.
//!
//! **Dense or sparse** (`NewtonConfig::solver`, `docs/proposals/sparse-solve.md` Step 2). Below
//! [`crate::sparse::SPARSE_THRESHOLD`] unknowns — or always, with [`Solver::Dense`] — each
//! iteration assembles into a dense [`mna::System`] and calls [`linsolve::solve_dense`], exactly
//! as before the sparse path existed. Otherwise it assembles into one [`SparseSystem`] kept for
//! the whole solve, every `gmin` stage included, so the pattern is found once and the symbolic
//! factorization done once. The iteration logic around the solve — limiting, damping,
//! convergence tests — is the same code on both paths.

use crate::sparse::{self, Solver, SparseLu, SparseSystem};
use crate::{convergence, linsolve, mna, CoreError};
use va_abi::{ModelInstance, UnknownKind};

/// Tunable Newton iteration controls.
#[derive(Clone, Copy, Debug)]
pub struct NewtonConfig {
    /// Maximum iterations before declaring non-convergence.
    pub max_iters: usize,
    /// Absolute residual tolerance for convergence.
    pub abstol: f64,
    /// Relative update tolerance for convergence.
    pub reltol: f64,
    /// Clamp each iteration's proposed update with [`crate::convergence::limit_junction`],
    /// using [`crate::convergence::VT_NOMINAL`]/[`crate::convergence::default_vcrit`] as a
    /// blanket (not per-device) threshold. Keeps stiff exponential devices (diodes, BJTs) from
    /// overflowing on a cold start; the tradeoff is it can slow convergence on unknowns that
    /// were never exponential to begin with, since `va-core` has no way to tell those apart
    /// from real junction voltages (see `convergence`'s module doc comment). Default `true`.
    pub limit_junctions: bool,
    /// Number of geometric `gmin`-stepping homotopy stages to ramp through before the final,
    /// unshunted solve (see [`crate::convergence::gmin_for_step`]). `0` (the default) disables
    /// `gmin` stepping entirely — a single ordinary solve, identical to every prior release's
    /// behavior. Only ever shunts [`va_abi::UnknownKind::Node`] rows (never a branch-current
    /// constraint row — see [`crate::mna::System::shunt_gmin`]), so it's safe to enable on any
    /// circuit, including ones with ideal sources.
    pub gmin_steps: usize,
    /// Maximum number of times a Newton step may be halved when the full step does not reduce
    /// the residual — a backtracking line search, the third convergence aid alongside junction
    /// limiting and `gmin` stepping.
    ///
    /// `0` (the default) disables damping entirely: every step is taken in full, exactly as
    /// before this existed. With `n > 0`, a step that would *increase* the residual infinity
    /// norm is retried at 1/2, 1/4, ... up to `n` times, and the first scale that improves on
    /// the starting residual is taken; if none does, the smallest tried step is taken anyway so
    /// the iteration still moves rather than stalling.
    ///
    /// This helps where the other two aids do not: junction limiting bounds a step's *size*
    /// per unknown without knowing whether it helps, and `gmin` stepping changes the circuit
    /// rather than the step. Damping is the only one that consults the residual the step
    /// actually produced. It cannot rescue a genuinely singular Jacobian, and it costs one
    /// extra assemble per halving, which is why it is off unless asked for.
    pub max_damping_halvings: usize,
    /// The largest change one Newton step may make to a [`UnknownKind::Node`] unknown, in that
    /// unknown's own units (volts, for an electrical node); larger components of the step are
    /// clamped to it, one by one. Branch rows are never clamped. `f64::INFINITY` (the default)
    /// disables it: every step is taken as solved, exactly as before this existed.
    ///
    /// SPICE's answer to a node set only by leakage — the inside of a transistor stack whose
    /// lower device is off — where the linearized step divides by a near-zero conductance and
    /// proposes tens of volts on a circuit whose supply is under two. Unlike
    /// [`Self::max_damping_halvings`] it costs nothing per iteration (no trial evaluation), and
    /// unlike it, it bounds each unknown separately, so one runaway row does not shrink the
    /// whole step. The converged answer does not depend on it: a clamp only binds on a step
    /// larger than itself, and the convergence test is on the step actually applied. Used by
    /// the DC rescue (`crate::dc`), not by default.
    pub max_node_step: f64,
    /// Dense or sparse linear algebra — see [`Solver`]. Default [`Solver::Auto`]: dense below
    /// [`crate::sparse::SPARSE_THRESHOLD`] unknowns, sparse from it. The answer does not depend
    /// on it beyond rounding (the pivot order differs), so every convergence aid behaves the
    /// same either way.
    pub solver: Solver,
    /// Write a line to stderr for every Newton iteration and for every solve stage (one per
    /// `gmin` step): the time spent assembling (evaluating every instance and stamping), the
    /// time spent in the linear solve, the line search's trial time, the iteration count each
    /// stage took, and on the sparse path the nonzero count and whether the symbolic
    /// factorization was redone. Every line starts `[logfull]`; the format is in
    /// `docs/validation.md` § "Reading a `--logfull` trace". Default `false`.
    ///
    /// A debugging aid, not a result: it does not change the iteration (the timers run either
    /// way; only the printing is switched), but writing one line per iteration costs time of its
    /// own on a solve with many cheap iterations. Covers the DC Newton loop only — every
    /// analysis's operating point, `.dc` sweeps included — not the transient integrator's own
    /// per-timestep Newton loop (`va-transient`).
    pub log_full: bool,
    /// Event counters from outside this crate for [`Self::log_full`]'s lines, as a function
    /// that appends `(name, running total)` pairs — `va-cli` passes one reading
    /// `va_codegen::counters`, which `va-core` cannot depend on. Every line reports each
    /// counter's increase over its iteration (or stage), next to this crate's own
    /// `stamp_lookups` ([`crate::counters`]). `None` (the default) reports `stamp_lookups` only.
    /// Read only when `log_full` is on.
    pub log_counters: Option<CounterSource>,
}

/// A source of named running totals for [`NewtonConfig::log_counters`]: appends
/// `(name, total so far)` pairs to the vector it is given.
pub type CounterSource = fn(&mut Vec<(&'static str, u64)>);

impl Default for NewtonConfig {
    fn default() -> Self {
        Self {
            max_iters: 100,
            abstol: 1e-12,
            reltol: 1e-9,
            max_damping_halvings: 0,
            max_node_step: f64::INFINITY,
            limit_junctions: true,
            gmin_steps: 0,
            solver: Solver::Auto,
            log_full: false,
            log_counters: None,
        }
    }
}

/// What one [`Linear::step`] cost, for [`NewtonConfig::log_full`]: filled in as the step goes,
/// so an iteration whose solve fails still reports the assembly it paid for.
#[derive(Clone, Copy, Debug, Default)]
struct StepCost {
    assemble_ms: f64,
    solve_ms: f64,
    /// Stored entries of the assembled Jacobian; `None` on the dense path, which stores all `dim²`.
    nnz: Option<usize>,
    /// Whether this step's solve computed a new symbolic factorization (the pattern grew).
    new_symbolic: bool,
}

fn ms_since(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The convergence aids a stage runs with, for [`NewtonConfig::log_full`]'s lines: `plain`, or
/// the aids joined with `+` (`ladder`, `cap`, `damp`) — which tier of `crate::dc`'s rescue
/// a stage belongs to reads straight off it.
fn aids_label(cfg: &NewtonConfig) -> String {
    let mut aids = Vec::new();
    if cfg.gmin_steps > 0 {
        aids.push("ladder");
    }
    if cfg.max_node_step.is_finite() {
        aids.push("cap");
    }
    if cfg.max_damping_halvings > 0 {
        aids.push("damp");
    }
    if aids.is_empty() {
        "plain".to_string()
    } else {
        aids.join("+")
    }
}

/// Solve `f(x) = 0` for the given `instances` by Newton iteration, returning the solution
/// vector of length `dim`. The initial guess is the zero vector.
///
/// If `cfg.gmin_steps > 0`, wraps the inner iteration in a `gmin`-stepping homotopy (see this
/// module's doc comment): each stage warm-starts from the previous stage's solution, ending on
/// an unshunted solve of the real circuit. With `cfg.gmin_steps == 0` this is exactly one
/// inner solve from the zero vector — identical to every prior release's behavior.
///
/// # Errors
///
/// [`CoreError::NoConvergence`] if the iteration budget is exhausted at any stage, or
/// [`CoreError::Singular`] if a Jacobian factorization fails.
pub fn solve(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
) -> Result<Vec<f64>, CoreError> {
    solve_with_events(instances, dim, cfg, &va_abi::FiredEvents::default())
}

/// [`solve`], with the events the consumer has determined fired at this operating point.
///
/// See [`crate::mna::assemble_with_events`] for why a static solve has events at all. `solve`
/// passes an empty set, so a caller with no events behaves exactly as before.
///
/// # Errors
///
/// As [`solve`].
pub fn solve_with_events(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    fired: &va_abi::FiredEvents,
) -> Result<Vec<f64>, CoreError> {
    solve_with_events_from(instances, dim, cfg, fired, None)
}

/// [`solve_with_events`], started from `start` instead of the zero vector.
///
/// # What a starting point is for
///
/// Newton converges from wherever it is put, but how *fast*, and whether at all, depends
/// entirely on where that is. A `.dc` sweep solves the same circuit at a sequence of nearby
/// source values, and the answer at one point is an excellent guess for the next — usually
/// within a couple of iterations rather than a dozen from zero. Nothing else about the solve
/// changes: the fixed point of the equations is a property of the equations, not of the path
/// taken to it, so a converged answer is the same answer whichever starting point found it,
/// to within `cfg`'s tolerances.
///
/// `start` shorter or longer than `dim` is a caller error rather than something to paper over,
/// and is rejected. `None` is the zero vector, which is exactly [`solve_with_events`].
///
/// # Errors
///
/// As [`solve`], plus [`CoreError::Singular`]-style rejection of a mis-sized `start` reported
/// as [`CoreError::NoConvergence`] — see the check below.
pub fn solve_with_events_from(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    fired: &va_abi::FiredEvents,
    start: Option<&[f64]>,
) -> Result<Vec<f64>, CoreError> {
    if dim == 0 {
        return Ok(Vec::new());
    }
    // A starting point of the wrong width would silently solve a different problem — padded
    // with zeros it is a guess for a circuit with fewer unknowns, truncated it drops rows.
    if let Some(x0) = start {
        if x0.len() != dim {
            return Err(CoreError::NoConvergence {
                iters: 0,
                residual: f64::NAN,
            });
        }
    }

    // Only classify unknowns when gmin stepping is actually in play — `shunt_gmin` is a no-op
    // at `gmin == 0`, but building the classification is pointless work otherwise.
    let kinds = if cfg.gmin_steps > 0 {
        mna::classify_unknowns(instances, dim)
    } else {
        vec![UnknownKind::Node; dim]
    };
    // Unlike `kinds`, this always runs: every `solve_from` call's per-iteration convergence
    // check needs it, not just `shunt_gmin` (§ nature-metadata wiring's module doc comment).
    let per_abstol = mna::classify_abstol(instances, dim, cfg.abstol);
    // Which unknowns actually sit across an exponential junction. Only these are step-limited:
    // see `ModelInstance::unknown_is_junction` for why applying the clamp to everything is
    // damage rather than caution. Skipped entirely when limiting is switched off.
    let junction = if cfg.limit_junctions {
        mna::classify_junctions(instances, dim)
    } else {
        vec![false; dim]
    };

    let mut x = match start {
        Some(x0) => x0.to_vec(),
        None => vec![0.0; dim],
    };
    // One for the whole solve, across every `gmin` stage: the pattern does not change with the
    // shunt (every diagonal is always in it), so it is found and analysed once.
    let mut linear = Linear::new(cfg.solver, dim);
    // `gmin_for_step(step, 0)` returns `0.0` at `step == 0`, so `gmin_steps == 0` collapses
    // this to exactly one iteration at `gmin = 0` — the original, un-homotopied solve.
    for step in 0..=cfg.gmin_steps {
        let gmin = convergence::gmin_for_step(step, cfg.gmin_steps);
        x = solve_from(
            x,
            instances,
            dim,
            cfg,
            gmin,
            &Classification {
                kinds: &kinds,
                per_abstol: &per_abstol,
                junction: &junction,
            },
            fired,
            &mut linear,
        )?;
    }
    Ok(x)
}

/// The linear algebra one Newton solve uses: nothing to keep for the dense path, the system and
/// the factorization cache for the sparse one.
enum Linear {
    Dense,
    Sparse {
        sys: Box<SparseSystem>,
        lu: SparseLu,
        /// For [`damped_scale`]'s trial points, kept apart from `sys` so a trial point's
        /// assembly cannot disturb the iterate's.
        trial: Box<SparseSystem>,
    },
}

impl Linear {
    fn new(solver: Solver, dim: usize) -> Self {
        if solver.uses_sparse(dim) {
            Linear::Sparse {
                sys: Box::new(SparseSystem::new(dim)),
                lu: SparseLu::new(),
                trial: Box::new(SparseSystem::new(dim)),
            }
        } else {
            Linear::Dense
        }
    }

    /// Assemble at `x` with `gmin` shunted, and return the residual's infinity norm and the
    /// Newton step solving `J · dx = −f`. `cost` receives what the step took (§ `log_full`),
    /// the assembly time included even when the solve then fails.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        instances: &[&dyn ModelInstance],
        x: &[f64],
        ctx: &va_abi::AnalysisCtx,
        dim: usize,
        fired: &va_abi::FiredEvents,
        gmin: f64,
        kinds: &[UnknownKind],
        cost: &mut StepCost,
    ) -> Result<(f64, Vec<f64>), CoreError> {
        *cost = StepCost::default();
        let t0 = std::time::Instant::now();
        match self {
            Linear::Dense => {
                let mut sys = mna::assemble_with_events(instances, x, ctx, dim, fired);
                sys.shunt_gmin(x, gmin, kinds);
                let residual_norm = inf_norm(&sys.residual);
                let neg_f: Vec<f64> = sys.residual.iter().map(|v| -v).collect();
                cost.assemble_ms = ms_since(t0);
                let t1 = std::time::Instant::now();
                let dx = linsolve::solve_dense(&sys.jacobian, &neg_f, dim);
                cost.solve_ms = ms_since(t1);
                Ok((residual_norm, dx?))
            }
            Linear::Sparse { sys, lu, .. } => {
                sparse::assemble_into(instances, x, ctx, fired, sys);
                sys.shunt_gmin(x, gmin, kinds);
                let residual_norm = inf_norm(sys.residual_values());
                let neg_f: Vec<f64> = sys.residual_values().iter().map(|v| -v).collect();
                cost.assemble_ms = ms_since(t0);
                cost.nnz = Some(sys.pattern().nnz());
                let symbolic_before = lu.symbolic_factorizations();
                let t1 = std::time::Instant::now();
                let dx = lu.solve(sys.jacobian(), &neg_f);
                cost.solve_ms = ms_since(t1);
                cost.new_symbolic = lu.symbolic_factorizations() > symbolic_before;
                Ok((residual_norm, dx?))
            }
        }
    }

    /// The residual's infinity norm at trial point `x`, with `gmin` shunted — all
    /// [`damped_scale`] needs, and on the sparse path without a `dim²` buffer.
    fn trial_residual_norm(
        &mut self,
        instances: &[&dyn ModelInstance],
        x: &[f64],
        dim: usize,
        fired: &va_abi::FiredEvents,
        gmin: f64,
        kinds: &[UnknownKind],
    ) -> f64 {
        match self {
            Linear::Dense => {
                let mut sys =
                    mna::assemble_with_events(instances, x, &va_abi::ANALYSIS_DC, dim, fired);
                sys.shunt_gmin(x, gmin, kinds);
                inf_norm(&sys.residual)
            }
            Linear::Sparse { trial, .. } => {
                sparse::assemble_into(instances, x, &va_abi::ANALYSIS_DC, fired, trial);
                trial.shunt_gmin(x, gmin, kinds);
                inf_norm(trial.residual_values())
            }
        }
    }
}

/// The inner Newton iteration, starting from `x0` and shunting `gmin` onto every `Node`-kind
/// row each iteration (see [`mna::System::shunt_gmin`]). [`solve`] is `gmin_steps + 1` calls to
/// this, chained by warm-starting each stage's `x0` from the previous stage's solution.
/// The per-unknown classifications [`solve`] computes once and every iteration reads: which
/// rows `gmin` may shunt, each row's own convergence tolerance, and which rows are junction
/// potentials. Grouped because all three are the same shape (one entry per global unknown),
/// have the same lifetime, and are always passed together.
struct Classification<'a> {
    kinds: &'a [UnknownKind],
    per_abstol: &'a [f64],
    junction: &'a [bool],
}

#[allow(clippy::too_many_arguments)]
fn solve_from(
    mut x: Vec<f64>,
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    gmin: f64,
    class: &Classification<'_>,
    fired: &va_abi::FiredEvents,
    linear: &mut Linear,
) -> Result<Vec<f64>, CoreError> {
    let Classification {
        kinds,
        per_abstol,
        junction,
    } = class;
    let vt = convergence::VT_NOMINAL;
    let vcrit = convergence::default_vcrit(vt);

    // The simulator parameters `$simparam` reports, fixed for this stage: the tolerances come
    // from the caller's config, and `gmin` is this homotopy stage's actual shunt -- so a model
    // asking for it is told the conductance really in the circuit, not a nominal one.
    let sim = va_abi::SimParams::new().with_tolerances(cfg.abstol, cfg.reltol);
    let mut last_residual = f64::INFINITY;
    let mut log = StageLog::new(cfg, gmin, dim, instances.len());
    let mut cost = StepCost::default();
    for iteration in 0..cfg.max_iters {
        log.begin_iteration();
        // This crate solves DC operating points only (`crate::dc`), so the analysis kind is
        // fixed here rather than plumbed in from the caller: an AC or noise run linearizes about
        // a point this same DC solve produced, and asks its own analysis's question later. The
        // *iteration number* is not fixed, which is exactly why `$simparam("iteration")` cannot
        // be answered anywhere earlier than here.
        let ctx = va_abi::ANALYSIS_DC.with_sim(sim.at_iteration(iteration, gmin));
        // Assemble, shunt, and solve J · dx = −f, dense or sparse.
        let (residual_norm, dx) =
            match linear.step(instances, &x, &ctx, dim, fired, gmin, kinds, &mut cost) {
                Ok(v) => v,
                Err(e) => {
                    log.add(&cost, 0.0);
                    log.stage(iteration + 1, &format!("failed: {e}"));
                    return Err(e);
                }
            };

        let trial_start = std::time::Instant::now();
        // Apply the step, optionally damped: `scale` is 1.0 unless the full step made the
        // residual worse, in which case `damped_scale` backtracks (§ `max_damping_halvings`).
        let scale = damped_scale(
            instances,
            &x,
            &dx,
            dim,
            cfg,
            gmin,
            kinds,
            vt,
            vcrit,
            residual_norm,
            junction,
            fired,
            linear,
        );
        let trial_ms = ms_since(trial_start);

        let mut update_small = true;
        let mut max_applied = 0.0_f64;
        for i in 0..dim {
            let vold = x[i];
            let vnew_raw = vold + node_step(scale * dx[i], kinds[i], cfg.max_node_step);
            let vnew = if junction[i] {
                convergence::limit_junction(vnew_raw, vold, vt, vcrit)
            } else {
                vnew_raw
            };
            x[i] = vnew;

            let applied = vnew - vold;
            max_applied = max_applied.max(applied.abs());
            if applied.abs() > cfg.reltol * vnew.abs() + per_abstol[i] {
                update_small = false;
            }
        }

        log.add(&cost, trial_ms);
        log.iteration(
            iteration,
            &cost,
            trial_ms,
            scale,
            residual_norm,
            max_applied,
        );
        if residual_norm <= cfg.abstol || update_small {
            log.stage(iteration + 1, "converged");
            return Ok(x);
        }
        last_residual = residual_norm;
    }

    log.stage(cfg.max_iters, "no convergence");
    Err(CoreError::NoConvergence {
        iters: cfg.max_iters,
        residual: last_residual,
    })
}

/// [`NewtonConfig::log_full`]'s printer for one [`solve_from`] call (one `gmin` stage): a line
/// per iteration, and a closing line with the stage's iteration count and totals. Inert when
/// the flag is off.
struct StageLog {
    on: bool,
    aids: String,
    gmin: f64,
    dim: usize,
    instances: usize,
    start: std::time::Instant,
    assemble_ms: f64,
    solve_ms: f64,
    trial_ms: f64,
    new_symbolic: usize,
    counter_source: Option<CounterSource>,
    /// Counter totals when the stage began, and when the current iteration began.
    stage_counts: Vec<(&'static str, u64)>,
    iteration_counts: Vec<(&'static str, u64)>,
}

impl StageLog {
    fn new(cfg: NewtonConfig, gmin: f64, dim: usize, instances: usize) -> Self {
        Self {
            on: cfg.log_full,
            aids: if cfg.log_full {
                aids_label(&cfg)
            } else {
                String::new()
            },
            gmin,
            dim,
            instances,
            start: std::time::Instant::now(),
            assemble_ms: 0.0,
            solve_ms: 0.0,
            trial_ms: 0.0,
            new_symbolic: 0,
            counter_source: cfg.log_counters,
            stage_counts: Vec::new(),
            iteration_counts: Vec::new(),
        }
        .with_stage_counts()
    }

    fn with_stage_counts(mut self) -> Self {
        if self.on {
            self.stage_counts = self.counts();
        }
        self
    }

    /// Every counter's running total: this crate's, then the caller's.
    fn counts(&self) -> Vec<(&'static str, u64)> {
        let mut counts = vec![("stamp_lookups", crate::counters::stamp_lookups())];
        if let Some(source) = self.counter_source {
            source(&mut counts);
        }
        counts
    }

    fn begin_iteration(&mut self) {
        if self.on {
            self.iteration_counts = self.counts();
        }
    }

    /// ` name=increase` for every counter, against the totals in `since`.
    fn count_increases(&self, since: &[(&'static str, u64)]) -> String {
        self.counts()
            .iter()
            .zip(since)
            .map(|((name, now), (_, then))| format!(" {name}={}", now.saturating_sub(*then)))
            .collect()
    }

    fn add(&mut self, cost: &StepCost, trial_ms: f64) {
        self.assemble_ms += cost.assemble_ms;
        self.solve_ms += cost.solve_ms;
        self.trial_ms += trial_ms;
        self.new_symbolic += usize::from(cost.new_symbolic);
    }

    fn iteration(
        &self,
        iteration: usize,
        cost: &StepCost,
        trial_ms: f64,
        scale: f64,
        residual: f64,
        max_applied: f64,
    ) {
        if !self.on {
            return;
        }
        eprintln!(
            "[logfull] iter  aids={} gmin={:.3e} iter={iteration} assemble_ms={:.3} solve_ms={:.3} trial_ms={:.3} nnz={} new_symbolic={} scale={scale:.3e} residual={residual:.3e} max_step={max_applied:.3e}{}",
            self.aids,
            self.gmin,
            cost.assemble_ms,
            cost.solve_ms,
            trial_ms,
            cost.nnz.map_or_else(|| "dense".to_string(), |n| n.to_string()),
            u8::from(cost.new_symbolic),
            self.count_increases(&self.iteration_counts),
        );
    }

    fn stage(&self, iterations: usize, outcome: &str) {
        if !self.on {
            return;
        }
        eprintln!(
            "[logfull] stage aids={} gmin={:.3e} iterations={iterations} outcome=\"{outcome}\" assemble_ms={:.1} solve_ms={:.1} trial_ms={:.1} wall_ms={:.1} new_symbolic={} unknowns={} instances={}{}",
            self.aids,
            self.gmin,
            self.assemble_ms,
            self.solve_ms,
            self.trial_ms,
            ms_since(self.start),
            self.new_symbolic,
            self.dim,
            self.instances,
            self.count_increases(&self.stage_counts),
        );
    }
}

/// The fraction of `dx` to actually apply: `1.0` when damping is off or when the full step
/// already reduces the residual, otherwise the first of `1/2, 1/4, ...` that does.
///
/// Each trial costs one extra assemble, which is the whole reason this is opt-in. A trial point
/// is built exactly the way the real update is, junction limiting included, so the residual
/// being compared is the one the iteration would actually land on rather than an idealized
/// version of it. If no scale improves on `residual_norm`, the smallest one tried is returned
/// rather than `0.0`: a zero step would stall the iteration into `max_iters` having learned
/// nothing, while a small uphill step still moves and lets the next Jacobian re-aim.
#[allow(clippy::too_many_arguments)]
fn damped_scale(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    dx: &[f64],
    dim: usize,
    cfg: NewtonConfig,
    gmin: f64,
    kinds: &[UnknownKind],
    vt: f64,
    vcrit: f64,
    residual_norm: f64,
    junction: &[bool],
    fired: &va_abi::FiredEvents,
    linear: &mut Linear,
) -> f64 {
    if cfg.max_damping_halvings == 0 {
        return 1.0;
    }
    let mut scale = 1.0;
    for _ in 0..=cfg.max_damping_halvings {
        let candidate: Vec<f64> = (0..dim)
            .map(|i| {
                let raw = x[i] + node_step(scale * dx[i], kinds[i], cfg.max_node_step);
                if junction[i] {
                    convergence::limit_junction(raw, x[i], vt, vcrit)
                } else {
                    raw
                }
            })
            .collect();
        // A non-finite residual (an exponential that overflowed at this trial point) is not an
        // improvement by any reading, and `<` against a NaN is false, so it backtracks.
        if linear.trial_residual_norm(instances, &candidate, dim, fired, gmin, kinds)
            < residual_norm
        {
            return scale;
        }
        scale *= 0.5;
    }
    scale * 2.0
}

/// One component of a Newton step, clamped to `max` when it moves a `Node` unknown
/// ([`NewtonConfig::max_node_step`]). With `max` infinite this is `step` exactly, so the default
/// path's arithmetic is unchanged.
fn node_step(step: f64, kind: UnknownKind, max: f64) -> f64 {
    if max.is_finite() && matches!(kind, UnknownKind::Node) {
        step.clamp(-max, max)
    } else {
        step
    }
}

/// Infinity norm (max absolute component) of a vector.
fn inf_norm(v: &[f64]) -> f64 {
    v.iter().fold(0.0_f64, |m, x| m.max(x.abs()))
}

#[cfg(test)]
mod tests {
    /// `va_abi::SimParams::new()` duplicates `NewtonConfig`'s default tolerances, because
    /// `va-abi` is a leaf crate and may not depend on `va-core` (`CLAUDE.md` §3). Two copies of
    /// one number is exactly the shape that drifts, so this pins them together.
    ///
    /// If this fails, the fix is to change the constant in `va_abi::SimParams::new` to match —
    /// `NewtonConfig` is the authority, and a model asking `$simparam("abstol")` must be told
    /// the tolerance the solve actually uses.
    #[test]
    fn newton_config_and_simparams_agree_on_tolerances() {
        let cfg = super::NewtonConfig::default();
        let sim = va_abi::SimParams::new();
        assert_eq!(
            sim.abstol, cfg.abstol,
            "abstol drifted between the two crates"
        );
        assert_eq!(
            sim.reltol, cfg.reltol,
            "reltol drifted between the two crates"
        );
    }

    use super::*;
    use crate::testutil::VSource;
    use va_abi::reference::diode::VT_NOMINAL;
    use va_abi::reference::{Diode, Resistor, GROUND};

    /// Damping's own demonstration, in the shape `gmin_stepping_converges_a_circuit_plain_
    /// newton_cannot` established: the same circuit is checked to *fail* undamped and
    /// *succeed* damped, in one test, so this cannot pass by being decorative.
    ///
    /// The circuit is a diode in series with a small resistance driven hard, with junction
    /// limiting deliberately **off**. Without limiting, the first Newton step from a cold
    /// start proposes a junction voltage far past the exponential's usable range and the
    /// iteration never recovers. Damping needs no per-device knowledge to fix that -- it just
    /// notices the residual got worse and backs off -- which is exactly the property that
    /// makes it complementary to limiting rather than redundant with it.
    #[test]
    fn log_full_changes_no_number_on_either_path() {
        // § `log_full`: a trace is only trustworthy for debugging if running with it is running
        // the same solve. So: every aid that has its own code path on (the ladder, the node cap,
        // damping's trial assemblies), on the dense and the sparse path, bit-identical answers
        // with and without it -- and the failure path (the log's early return) reports the same
        // error.
        let (vs, r, d) = (
            VSource::new(0, GROUND, 2, 10.0),
            Resistor::new(0, 1, 1.0),
            Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL),
        );
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        for solver in [Solver::Dense, Solver::Sparse] {
            let quiet = NewtonConfig {
                gmin_steps: 8,
                max_damping_halvings: 20,
                max_node_step: 0.5,
                solver,
                ..NewtonConfig::default()
            };
            let traced = NewtonConfig {
                log_full: true,
                ..quiet
            };
            let a = solve(&insts, 3, quiet).expect("the aided diode clamp converges");
            let b = solve(&insts, 3, traced).expect("and does so with the trace on");
            assert_eq!(
                a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "{solver:?}: the trace changed the answer"
            );

            let failing = NewtonConfig {
                limit_junctions: false,
                max_iters: 3,
                solver,
                ..NewtonConfig::default()
            };
            let e1 = solve(&insts, 3, failing).expect_err("3 unaided iterations cannot converge");
            let e2 = solve(
                &insts,
                3,
                NewtonConfig {
                    log_full: true,
                    ..failing
                },
            )
            .expect_err("nor with the trace on");
            assert_eq!(format!("{e1:?}"), format!("{e2:?}"), "{solver:?}");
        }
        assert_eq!(aids_label(&NewtonConfig::default()), "plain");
        assert_eq!(
            aids_label(&NewtonConfig {
                gmin_steps: 30,
                max_node_step: 0.5,
                ..NewtonConfig::default()
            }),
            "ladder+cap"
        );
    }

    #[test]
    fn damping_converges_a_circuit_undamped_newton_cannot() {
        let build = || {
            (
                VSource::new(0, GROUND, 2, 10.0),
                Resistor::new(0, 1, 1.0),
                Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL),
            )
        };
        let (vs, r, d) = build();
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let undamped = NewtonConfig {
            limit_junctions: false,
            max_damping_halvings: 0,
            ..NewtonConfig::default()
        };
        let failed = solve(&insts, 3, undamped);
        assert!(
            failed.is_err(),
            "undamped Newton was expected to fail here; it returned {failed:?} -- if this              circuit became solvable, the test has stopped demonstrating anything and needs a              harder one, not a relaxed assertion"
        );

        let damped = NewtonConfig {
            max_damping_halvings: 20,
            ..undamped
        };
        let x = solve(&insts, 3, damped).expect("damped Newton should converge");

        // And the answer is right, not merely returned: KCL at the junction node says the
        // diode current equals the resistor current, to the solver's own tolerance.
        let vd = x[1];
        let id = 1e-14 * (libm::exp(vd / VT_NOMINAL) - 1.0);
        let ir = (x[0] - vd) / 1.0;
        assert!(
            (id - ir).abs() < 1e-9 * ir.abs().max(1e-6),
            "KCL at the junction: diode {id} vs resistor {ir} (V(d) = {vd})"
        );
        assert!(
            (0.4..1.0).contains(&vd),
            "a forward-biased silicon junction should sit near 0.6-0.8 V, got {vd}"
        );
    }

    /// § `max_node_step`, in the same "fails one way, succeeds the other" shape: the fixture
    /// plain Newton cannot solve without junction limiting converges with only the node-step
    /// cap on, to the same KCL-exact answer — and on a circuit that converges either way, the
    /// cap changes the path, not the answer.
    #[test]
    fn a_node_step_cap_converges_what_plain_newton_cannot_and_keeps_the_answer() {
        let vs = VSource::new(0, GROUND, 2, 10.0);
        let r = Resistor::new(0, 1, 1.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        let plain = NewtonConfig {
            limit_junctions: false,
            ..NewtonConfig::default()
        };
        assert!(
            solve(&insts, 3, plain).is_err(),
            "the fixture must defeat plain Newton"
        );
        let capped = NewtonConfig {
            max_node_step: 0.5,
            ..plain
        };
        let x = solve(&insts, 3, capped).expect("the capped iteration converges");
        let vd = x[1];
        let id = 1e-14 * (libm::exp(vd / VT_NOMINAL) - 1.0);
        let ir = x[0] - vd;
        assert!(
            (id - ir).abs() < 1e-9 * ir.abs().max(1e-6),
            "KCL at the junction: diode {id} vs resistor {ir} (V(d) = {vd})"
        );

        // With junction limiting on, both converge; the cap does not move the answer.
        let limited = NewtonConfig::default();
        let a = solve(&insts, 3, limited).expect("converges");
        let b = solve(
            &insts,
            3,
            NewtonConfig {
                max_node_step: 0.5,
                ..limited
            },
        )
        .expect("converges");
        assert!((a[1] - b[1]).abs() < 1e-9, "{} vs {}", a[1], b[1]);
    }

    /// § junction limiting, in the same "fails one way, succeeds the other" shape the damping
    /// and gmin-stepping demonstrations use, so it cannot pass by being decorative.
    ///
    /// A linear resistor divider at 100 V has no exponential anywhere, so no instance claims a
    /// junction unknown and Newton takes its full step. Wrapped in `JunctionOverride` — which
    /// is exactly the blanket "limit every unknown" behaviour this replaced — the identical
    /// circuit fails, because `limit_junction`'s logarithmic clamp compresses each step to
    /// about `vt*ln(...)` and 100 iterations cannot walk a node to 100 V.
    ///
    /// That was a real bug, not a hypothetical: before `unknown_is_junction` existed this
    /// divider failed to converge above roughly 20 V, and every golden circuit happened to run
    /// at 5 V or less, so nothing caught it.
    /// A starting point changes the *path* Newton takes, never where it lands.
    ///
    /// This is the property a `.dc` sweep's continuation rests on (`va_cli::solve_dc_sweep`):
    /// handing each point the previous point's answer is only legitimate if the fixed point is
    /// a property of the equations rather than of the guess. The circuit is nonlinear on
    /// purpose — a diode in series with a resistor, where a linear one would land in one step
    /// from anywhere and prove nothing — and the guesses span from the cold start the solver
    /// used to be limited to, through a near-miss, to one deliberately on the far side of the
    /// answer.
    #[test]
    fn a_starting_point_changes_the_path_not_the_answer() {
        let dim = 3;
        let src = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1_000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let instances: Vec<&dyn ModelInstance> = vec![&src, &r, &d];

        let cold = solve(&instances, dim, NewtonConfig::default()).expect("cold start solves");

        for start in [
            vec![0.0, 0.0, 0.0],
            vec![1.0, 0.6, -1e-4],
            vec![1.0, 0.55, -5e-4],
            vec![0.9, 0.7, 1e-3],
        ] {
            let warm = solve_with_events_from(
                &instances,
                dim,
                NewtonConfig::default(),
                &va_abi::FiredEvents::default(),
                Some(&start),
            )
            .unwrap_or_else(|e| panic!("start {start:?} failed to converge: {e}"));
            for (k, (c, w)) in cold.iter().zip(&warm).enumerate() {
                let scale = c.abs().max(w.abs()).max(1e-12);
                assert!(
                    (c - w).abs() / scale < 1e-9,
                    "unknown {k}: cold {c} vs warm {w} from start {start:?}"
                );
            }
        }

        // The above would pass just as well if `start` were quietly ignored, so this is what
        // makes the test discriminating: with a one-iteration budget, only a solve that really
        // begins at the answer can reach it. From the origin the same budget must fail.
        let stingy = NewtonConfig {
            max_iters: 1,
            ..NewtonConfig::default()
        };
        let from_answer = solve_with_events_from(
            &instances,
            dim,
            stingy,
            &va_abi::FiredEvents::default(),
            Some(&cold),
        );
        assert!(
            from_answer.is_ok(),
            "starting at the answer must converge within one iteration; it did not, so `start`              is not reaching the iteration: {from_answer:?}"
        );
        assert!(
            solve(&instances, dim, stingy).is_err(),
            "one iteration from the origin was expected to fail on this diode; if it stopped              failing, the check above no longer shows that `start` is used"
        );
    }

    /// A starting point of the wrong width is refused rather than padded or truncated.
    ///
    /// Silently zero-padding it would hand the solver a guess for a circuit with fewer
    /// unknowns; silently truncating would drop rows. Both converge to *something*, which is
    /// the worst outcome — it is the caller's bug and it has to surface as one.
    #[test]
    fn a_mis_sized_starting_point_is_refused() {
        let src = VSource::new(0, GROUND, 1, 1.0);
        let r = Resistor::new(0, GROUND, 1_000.0);
        let instances: Vec<&dyn ModelInstance> = vec![&src, &r];
        for bad in [vec![0.0], vec![0.0; 5]] {
            assert!(
                solve_with_events_from(
                    &instances,
                    2,
                    NewtonConfig::default(),
                    &va_abi::FiredEvents::default(),
                    Some(&bad),
                )
                .is_err(),
                "a {}-wide start for a 2-unknown circuit must be refused",
                bad.len()
            );
        }
        // …and the right width is accepted, so the check is about the width and not about
        // rejecting every `Some`.
        assert!(solve_with_events_from(
            &instances,
            2,
            NewtonConfig::default(),
            &va_abi::FiredEvents::default(),
            Some(&[0.0, 0.0]),
        )
        .is_ok());
    }

    #[test]
    fn junction_limiting_no_longer_throttles_a_linear_circuit() {
        let vs = VSource::new(0, GROUND, 2, 100.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);

        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];
        let x = solve(&insts, 3, NewtonConfig::default())
            .expect("a linear divider must converge at any operating voltage");
        assert!((x[1] - 50.0).abs() < 1e-9, "V(mid) = {}, want 50 V", x[1]);

        // The same circuit, every unknown claiming to be a junction: the old behaviour.
        let (wvs, wr1, wr2) = (
            crate::testutil::JunctionOverride { inner: &vs },
            crate::testutil::JunctionOverride { inner: &r1 },
            crate::testutil::JunctionOverride { inner: &r2 },
        );
        let wrapped: [&dyn ModelInstance; 3] = [&wvs, &wr1, &wr2];
        let throttled = solve(&wrapped, 3, NewtonConfig::default());
        assert!(
            throttled.is_err(),
            "blanket junction limiting was expected to throttle this to a non-convergence; it              returned {throttled:?} -- if that stopped being true, this test no longer              demonstrates why `unknown_is_junction` exists"
        );
    }

    /// The other half: an unknown that *is* a junction still gets limited, and that limiting is
    /// what carries a hard-driven diode to its operating point with damping switched off.
    /// Paired with the test above, this pins both directions — the clamp is applied where it
    /// helps and withheld where it hurts.
    #[test]
    fn a_diode_still_opts_into_junction_limiting() {
        // The reference diode declares its terminals as junction potentials...
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        assert!(d.unknown_is_junction(0), "a diode terminal is a junction");
        // ...a resistor does not.
        let r = Resistor::new(0, 1, 1.0);
        assert!(!r.unknown_is_junction(0), "a resistor terminal is not");

        // Limiting on, damping off: the same circuit the damping test drives, solved by the
        // clamp alone. `classify_junctions` must therefore be picking the diode's claim up.
        let vs = VSource::new(0, GROUND, 2, 10.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        let cfg = NewtonConfig {
            limit_junctions: true,
            max_damping_halvings: 0,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, 3, cfg).expect("junction limiting alone should carry this");
        let vd = x[1];
        assert!(
            (0.4..1.0).contains(&vd),
            "a forward-biased silicon junction should sit near 0.6-0.8 V, got {vd}"
        );
    }

    #[test]
    fn solves_resistor_divider() {
        // Vin = 2 V at node 0; R1 (node0→node1) and R2 (node1→gnd), both 1 kΩ.
        // node1 is the divider midpoint = Vin · R2/(R1+R2) = 1.0 V.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let x = solve(&insts, 3, NewtonConfig::default()).expect("converges");
        assert!((x[0] - 2.0).abs() < 1e-9, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-9, "midpoint = {}", x[1]);
        // Branch current through the source equals the divider current = 1 mA.
        assert!((x[2].abs() - 1e-3).abs() < 1e-12, "i = {}", x[2]);
    }

    #[test]
    fn per_unknown_abstol_override_changes_the_convergence_decision() {
        // The exact `solves_resistor_divider` circuit, but capped to 1 Newton iteration
        // (`limit_junctions: false` for a deterministic, uncla­mped first step — the divider is
        // linear, so that one step already lands exactly on the solution; only the *declared*
        // convergence outcome is what this test is about). At the default (tight, 1e-12)
        // abstol, the first iteration's own jump (0 -> ~2V/1V/1mA) is nowhere near "small", so
        // `update_small` doesn't fire and 1 iteration isn't enough — the residual only settles
        // to ~0 on the *second* pass, exactly `reports_non_convergence`'s shape.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let cfg = NewtonConfig {
            max_iters: 1,
            limit_junctions: false,
            ..NewtonConfig::default()
        };

        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];
        assert!(
            matches!(solve(&insts, 3, cfg), Err(CoreError::NoConvergence { .. })),
            "the default tight abstol should not absorb the first iteration's jump"
        );

        // Every unknown's abstol loosened (§ nature-metadata wiring) past the size of that
        // first jump: `update_small` now holds after the very first iteration, at the exact
        // same 1-iteration budget — the override, not a wider budget, is what changes this.
        let vs_loose = crate::testutil::AbstolOverride {
            inner: &vs,
            overrides: &[(0, 10.0), (2, 10.0)], // node0, this source's own branch current
        };
        let r1_loose = crate::testutil::AbstolOverride {
            inner: &r1,
            overrides: &[(1, 10.0)], // node1, via r1's own local index for it
        };
        let insts_loose: [&dyn ModelInstance; 3] = [&vs_loose, &r1_loose, &r2];
        let x = solve(&insts_loose, 3, cfg).expect("loosened abstol converges within 1 iteration");
        assert!((x[0] - 2.0).abs() < 1e-9, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-9, "midpoint = {}", x[1]);
    }

    #[test]
    fn solves_diode_resistor_clamp() {
        // Vin = 1 V → R = 1 kΩ → diode to ground. Nonlinear: exercises the exp Jacobian.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let x = solve(&insts, 3, NewtonConfig::default()).expect("converges");

        // A forward-biased silicon diode sits around 0.4–0.75 V.
        let vd = x[1];
        assert!(
            (0.4..0.75).contains(&vd),
            "diode voltage out of range: {vd}"
        );
        // KCL at the diode node must balance: (Vin − Vd)/R == diode current.
        let i_r = (x[0] - vd) / 1000.0;
        let i_d = d.current(vd);
        assert!(
            (i_r - i_d).abs() < 1e-9,
            "KCL imbalance: {} vs {}",
            i_r,
            i_d
        );
    }

    #[test]
    fn gmin_stepping_does_not_corrupt_the_vsource_branch() {
        // The exact regression this is here to prevent: a naive "shunt every row" gmin
        // implementation would add a conductance to the VSource's branch-current row too,
        // corrupting its `V(p)-V(n)=value` constraint and giving a wrong answer. With
        // `classify_unknowns` tagging that row `Branch`, the divider must still solve to the
        // same exact midpoint gmin stepping on or off.
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let cfg = NewtonConfig {
            gmin_steps: 8,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, 3, cfg).expect("converges");
        assert!((x[0] - 2.0).abs() < 1e-6, "node0 = {}", x[0]);
        assert!((x[1] - 1.0).abs() < 1e-6, "midpoint = {}", x[1]);
        assert!((x[2].abs() - 1e-3).abs() < 1e-9, "i = {}", x[2]);
    }

    #[test]
    fn gmin_stepping_still_converges_the_diode_clamp() {
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let cfg = NewtonConfig {
            gmin_steps: 8,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, 3, cfg).expect("converges");

        let vd = x[1];
        assert!(
            (0.4..0.75).contains(&vd),
            "diode voltage out of range: {vd}"
        );
        let i_r = (x[0] - vd) / 1000.0;
        let i_d = d.current(vd);
        assert!(
            (i_r - i_d).abs() < 1e-6,
            "KCL imbalance: {} vs {}",
            i_r,
            i_d
        );
    }

    #[test]
    fn gmin_stepping_converges_a_circuit_plain_newton_cannot() {
        // The demo circuit `docs/roadmap.md`'s T3.3 flagged as missing: one that genuinely
        // *needs* gmin stepping, not just tolerates it. 20 diodes in series behind a 10 Ω
        // resistor, driven at 20 V from a cold (zero) start: a real, physically sane operating
        // point exists (~0.81 V/diode, ~0.38 A), but plain Newton's log-ramp junction limiting
        // walks the chain's *internal* node voltages there one node at a time, with no other
        // conductance path to keep them in check, and does not arrive within the default
        // iteration budget. `gmin` stepping's early, well-conditioned stages (a competing shunt
        // conductance to ground at every node) keep the whole chain in range long enough to land
        // near the true operating point before the final, unshunted stage — which then only
        // needs a handful of iterations to finish.
        //
        // **"Cannot" means "not within the default budget", and only that** (corrected in
        // 1.17.0). This test used to claim plain Newton fails at *any* budget, checked at
        // `max_iters: 2000`. That was one ulp: with MinGW's `ln` in `limit_junction` the walk
        // overflowed the factorization (`Singular`); with `libm`'s it arrives. A sweep of
        // 5–40 diodes × 5–160 V (2026-09-26) found that 2000-iteration outcome flips at this one
        // cell and nowhere else — a knife edge, not a property. The default-budget failure is
        // not: plain Newton fails and gmin stepping succeeds over a contiguous region (20–40
        // diodes at 20 V, and 30 diodes from 80 V up), under either maths.
        let n_diodes = 20;
        let branch = n_diodes + 1;
        let dim = branch + 1;
        let vs = VSource::new(0, GROUND, branch, 20.0);
        let r = Resistor::new(0, 1, 10.0);
        let mut diodes = Vec::new();
        for i in 1..n_diodes {
            diodes.push(Diode::new(i, i + 1, 1e-14, 1.0, VT_NOMINAL));
        }
        diodes.push(Diode::new(n_diodes, GROUND, 1e-14, 1.0, VT_NOMINAL));
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs, &r];
        insts.extend(diodes.iter().map(|d| d as &dyn ModelInstance));

        // Asserted as "fails", not as one named variant: which way the walk comes apart is a
        // property of the platform's floating point, not of this solver — `dc.rs`'s sibling
        // pinned `Singular` and went red on macOS while Linux and Windows stayed green.
        assert!(
            solve(&insts, dim, NewtonConfig::default()).is_err(),
            "expected plain Newton to fail within the default iteration budget"
        );

        let cfg_with_gmin = NewtonConfig {
            max_iters: 150,
            gmin_steps: 30,
            ..NewtonConfig::default()
        };
        let x = solve(&insts, dim, cfg_with_gmin).expect("gmin stepping converges");

        // KCL: the same current flows through the resistor, the first diode, and the source
        // branch (a single series loop).
        let i_r = (x[0] - x[1]) / 10.0;
        let vd0 = x[1] - x[2];
        let i_d0 = diodes[0].current(vd0);
        assert!((i_r - i_d0).abs() < 1e-6, "KCL imbalance: {i_r} vs {i_d0}");
        assert!(
            (x[branch].abs() - i_r).abs() < 1e-6,
            "branch current mismatch: {} vs {i_r}",
            x[branch]
        );
    }

    #[test]
    fn reports_non_convergence() {
        // One iteration is not enough for a nonlinear solve from the zero guess.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let r = Resistor::new(0, 1, 1000.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];

        let cfg = NewtonConfig {
            max_iters: 1,
            ..NewtonConfig::default()
        };
        assert!(matches!(
            solve(&insts, 3, cfg),
            Err(CoreError::NoConvergence { .. })
        ));
    }

    /// Solve `insts` with `cfg` on both paths and require the same operating point. Not
    /// bit-identical — the pivot order differs — but far inside every tolerance the solve uses.
    fn assert_paths_agree(insts: &[&dyn ModelInstance], dim: usize, cfg: NewtonConfig) {
        let dense = solve(
            insts,
            dim,
            NewtonConfig {
                solver: Solver::Dense,
                ..cfg
            },
        )
        .expect("dense solves");
        let sparse = solve(
            insts,
            dim,
            NewtonConfig {
                solver: Solver::Sparse,
                ..cfg
            },
        )
        .expect("sparse solves");
        for (i, (d, s)) in dense.iter().zip(&sparse).enumerate() {
            assert!(
                (d - s).abs() <= 1e-9 * d.abs().max(1e-6),
                "x[{i}]: dense {d}, sparse {s}"
            );
        }
    }

    /// A 20-diode chain at 20 V: plain Newton fails on it (`gmin_stepping_rescues_...` above),
    /// so this exercises the sparse path through every `gmin` stage, one pattern throughout.
    #[test]
    fn sparse_newton_matches_dense_through_gmin_stepping() {
        let n_diodes = 20;
        let branch = n_diodes + 1;
        let vs = VSource::new(0, GROUND, branch, 20.0);
        let r = Resistor::new(0, 1, 10.0);
        let mut diodes: Vec<Diode> = (1..n_diodes)
            .map(|i| Diode::new(i, i + 1, 1e-14, 1.0, VT_NOMINAL))
            .collect();
        diodes.push(Diode::new(n_diodes, GROUND, 1e-14, 1.0, VT_NOMINAL));
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs, &r];
        insts.extend(diodes.iter().map(|d| d as &dyn ModelInstance));
        let cfg = NewtonConfig {
            max_iters: 150,
            gmin_steps: 30,
            ..NewtonConfig::default()
        };
        assert_paths_agree(&insts, branch + 1, cfg);
    }

    /// Damping assembles trial points; on the sparse path that uses its own scratch system.
    #[test]
    fn sparse_newton_matches_dense_with_damping() {
        let vs = VSource::new(0, GROUND, 2, 5.0);
        let r = Resistor::new(0, 1, 100.0);
        let d = Diode::new(1, GROUND, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r, &d];
        let cfg = NewtonConfig {
            max_damping_halvings: 8,
            ..NewtonConfig::default()
        };
        assert_paths_agree(&insts, 3, cfg);
    }

    /// Above the threshold `Auto` is the sparse path, and it lands where dense does: a 600-node
    /// ladder with a diode at every node, 601 unknowns. Driven at 0.5 V: at 2 V plain Newton
    /// overflows the junctions on *both* paths (found while writing this), which would test
    /// nothing about the solver.
    #[test]
    fn auto_above_the_threshold_matches_dense() {
        let n = 600;
        let vs = VSource::new(0, GROUND, n, 0.5);
        let mut rs: Vec<Resistor> = (0..n - 1).map(|i| Resistor::new(i, i + 1, 10.0)).collect();
        rs.extend((0..n).map(|i| Resistor::new(i, GROUND, 1e4)));
        let ds: Vec<Diode> = (0..n)
            .map(|i| Diode::new(i, GROUND, 1e-14, 1.0, VT_NOMINAL))
            .collect();
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        insts.extend(rs.iter().map(|r| r as &dyn ModelInstance));
        insts.extend(ds.iter().map(|d| d as &dyn ModelInstance));
        let dim = n + 1;
        assert!(Solver::Auto.uses_sparse(dim));
        let auto = solve(&insts, dim, NewtonConfig::default()).expect("auto solves");
        let dense = solve(
            &insts,
            dim,
            NewtonConfig {
                solver: Solver::Dense,
                ..NewtonConfig::default()
            },
        )
        .expect("dense solves");
        for (i, (d, a)) in dense.iter().zip(&auto).enumerate() {
            assert!(
                (d - a).abs() <= 1e-9 * d.abs().max(1e-6),
                "x[{i}]: dense {d}, auto {a}"
            );
        }
    }

    /// A floating node is a singular matrix on the sparse path too, and the `gmin` rescue in
    /// `dc::operating_point` gets past it the same way.
    #[test]
    fn sparse_path_reports_a_floating_node_and_the_rescue_solves_it() {
        // Node 1 hangs off a diode's cathode with nothing else attached: no DC path.
        let vs = VSource::new(0, GROUND, 2, 1.0);
        let d = Diode::new(0, 1, 1e-14, 1.0, VT_NOMINAL);
        let insts: [&dyn ModelInstance; 2] = [&vs, &d];
        let sparse = NewtonConfig {
            solver: Solver::Sparse,
            ..NewtonConfig::default()
        };
        let dense = NewtonConfig {
            solver: Solver::Dense,
            ..NewtonConfig::default()
        };
        assert_eq!(
            solve(&insts, 3, sparse).is_err(),
            solve(&insts, 3, dense).is_err(),
            "both paths agree on whether plain Newton fails"
        );
        let s = crate::dc::operating_point(&insts, 3, sparse).expect("sparse rescue solves");
        let d = crate::dc::operating_point(&insts, 3, dense).expect("dense rescue solves");
        for (a, b) in s.x.iter().zip(&d.x) {
            assert!((a - b).abs() < 1e-9, "sparse {a}, dense {b}");
        }
    }
}
