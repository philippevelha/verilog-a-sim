//! Parallel device evaluation that gives the same bits as serial evaluation.
//!
//! Every assembly — DC here, transient in `va-transient` — calls `load` once per instance and
//! sums the stamps into one system. For circuits of compiled models, evaluation is almost the
//! whole cost of an iteration and stamping a few percent
//! (`docs/proposals/parallel-assembly.md` §2), so the instances are evaluated in parallel, each
//! into its own [`Recorder`], and the recordings are then **replayed into the system in
//! instance order**. The system receives exactly the calls, in exactly the order, that the
//! serial loop makes, so every entry is summed in the same order and the result is
//! bit-identical whatever the thread count. That rests on two things outside this module:
//! evaluation-path maths that gives the same bits on every thread (`libm`, 1.17.0), and a
//! linear solve that does not depend on the core count (`faer` pinned sequential, 1.16.1).
//!
//! **Why not graph colouring** (EEspice, `references/eespice.pdf`): colouring lets threads stamp
//! straight into the shared matrix, but the order in which an entry's contributions arrive then
//! depends on the schedule, and the answer on the machine. Replay costs a pass over the
//! recorded stamps; that is the price of reproducibility, and it is small next to evaluation.
//!
//! **When it goes parallel.** Thread hand-off costs about 40 µs per assembly on the machine this
//! was measured on, which is more than a circuit of cheap models spends evaluating: the
//! 13-instance reference-model ring oscillator ran 2.2× *slower* in parallel (0.54 vs 0.24 s),
//! while c17's ~30 PSP103 transistors ran 2.0× faster (82 vs 165 s, 8 threads). Instance count
//! does not separate those two; evaluation cost does. So in [`Mode::Auto`] the serial loop is
//! timed now and then, an estimate of evaluation time per instance is kept, and an assembly
//! goes parallel when `instances × estimate` reaches [`PARALLEL_MIN_WORK_NS`]. Both paths give
//! identical results, so this choice — which does depend on timing — affects speed only.
//!
//! **Limitations, stated:** replay is serial — at high core counts it becomes the next limit
//! (proposal §6); a recording is allocated per instance per assembly rather than reused; the
//! cost estimate is one process-wide number, so a process evaluating very different circuits
//! in turn (a test binary) uses the last one measured until the next re-measurement.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::Instant;
use va_abi::stamps::StampSink;
use va_abi::{AnalysisCtx, ModelInstance, ModelState};

use crate::sparse::SparseSystem;

/// When [`load_all`] evaluates in parallel. Process-wide; set with [`set_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Parallel when the pool has more than one thread and the estimated evaluation work of
    /// the assembly reaches [`PARALLEL_MIN_WORK_NS`]. The default.
    Auto,
    /// Parallel whenever the pool has more than one thread and there are two or more
    /// instances — the setting that makes a serial-vs-parallel check exercise the parallel
    /// path on every circuit.
    Always,
    /// Always the serial loop.
    Never,
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// Set the process-wide [`Mode`]. `va-cli` sets it from `VA_PARALLEL` (`auto`, `always`,
/// `never`).
pub fn set_mode(mode: Mode) {
    MODE.store(
        match mode {
            Mode::Auto => 0,
            Mode::Always => 1,
            Mode::Never => 2,
        },
        Ordering::Relaxed,
    );
}

/// The current process-wide [`Mode`].
pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Always,
        2 => Mode::Never,
        _ => Mode::Auto,
    }
}

/// In [`Mode::Auto`], the estimated serial evaluation time of an assembly (ns) from which it
/// is evaluated in parallel. About five times the ~40 µs hand-off measured on the ring
/// oscillator, so that a circuit just over it still gains after paying the hand-off.
pub const PARALLEL_MIN_WORK_NS: u64 = 200_000;

/// How often (in assemblies) [`Mode::Auto`] re-times a serial assembly to refresh its estimate.
const REMEASURE_EVERY: u64 = 256;

static CALLS: AtomicU64 = AtomicU64::new(0);
/// Serial evaluation time per instance, ns; 0 = not measured yet.
static NS_PER_INSTANCE: AtomicU64 = AtomicU64::new(0);

/// Evaluate every instance at `x` and stamp the results into `sink`, as if by
/// `for (i, inst) in instances.iter().enumerate() { inst.load(x, ctx, &mut states[i], sink) }`.
///
/// `states[i]` is instance `i`'s state view; `states.len()` must equal `instances.len()` (a
/// shorter `states` evaluates only that many instances). Serial or parallel on rayon's current
/// pool per [`mode`]; the stamps `sink` receives are the same calls in the same order either
/// way.
pub fn load_all(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    sink: &mut dyn StampSink,
) {
    load_all_into(instances, x, ctx, states, Target::Any(sink));
}

fn load_all_into(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    mut target: Target<'_>,
) {
    let n = instances.len().min(states.len());
    let parallel = n >= 2
        && rayon::current_num_threads() > 1
        && match mode() {
            Mode::Never => false,
            Mode::Always => true,
            Mode::Auto => {
                let call = CALLS.fetch_add(1, Ordering::Relaxed);
                let est = NS_PER_INSTANCE.load(Ordering::Relaxed);
                if est == 0 || call.is_multiple_of(REMEASURE_EVERY) {
                    let t0 = Instant::now();
                    serial(instances, x, ctx, states, target.sink());
                    let per = (t0.elapsed().as_nanos() / n as u128).max(1);
                    NS_PER_INSTANCE
                        .store(u64::try_from(per).unwrap_or(u64::MAX), Ordering::Relaxed);
                    return;
                }
                est.saturating_mul(n as u64) >= PARALLEL_MIN_WORK_NS
            }
        };
    if parallel {
        let recorded: Vec<Recorder> = instances
            .par_iter()
            .zip(states.into_par_iter())
            .map(|(inst, mut st)| {
                let mut rec = Recorder::default();
                inst.load(x, ctx, &mut st, &mut rec);
                rec
            })
            .collect();
        for (i, rec) in recorded.iter().enumerate() {
            target.replay_fresh(i, rec);
        }
    } else {
        serial(instances, x, ctx, states, target.sink());
    }
}

/// **Device bypass** (prototype, branch `proto/device-bypass`, 2026-10-01): reuse an instance's
/// last stamps while its unknowns have not moved, as SPICE's `BYPASS` does.
///
/// Only instances that report [`ModelInstance::is_pure_in_x`] are bypassed — their stamps are a
/// function of `x` alone, so a recording taken at `x0` is a faithful one for any `x` close to
/// it. A recording is replayed **with a first-order correction**: every Jacobian entry
/// `(r, c, g)` adds `g·(x[c] − x0[c])` to residual row `r`, and every `dcharge` entry the same
/// to charge row `r`. The device is then linearised at `x0` rather than frozen there, so the
/// error a bypass leaves is second order in the drift, and the drift is bounded because `x0`
/// is not advanced while an instance is being bypassed: it is re-evaluated, and `x0` reset, as
/// soon as any one of its unknowns leaves `|x − x0| ≤ abstol + reltol·max(|x|, |x0|)`.
///
/// Deterministic: whether an instance is bypassed depends only on `x` and the cache, never on
/// timing or thread count, and the stamps reach the sink in instance order as in
/// [`load_all`]. A run with bypass gives the same bits on every thread count, but **not** the
/// same bits as a run without it.
///
/// **Limitations, stated:** a recording is kept per instance (memory: one stamp list each);
/// the tolerance is one pair for every unknown regardless of its nature; an instance whose
/// Jacobian names a row of `x` outside its own `unknowns()` is never bypassed.
pub struct Bypass {
    reltol: f64,
    /// Per global unknown, in that unknown's own units.
    abstol: Vec<f64>,
    eligible: Vec<bool>,
    entries: Vec<Option<BypassEntry>>,
    /// Loads replayed from a recording.
    pub hits: u64,
    /// Loads of eligible instances that had to be evaluated.
    pub misses: u64,
}

struct BypassEntry {
    /// `(global unknown, x0)` for every unknown of the instance that is a row of `x`.
    x0: Vec<(usize, f64)>,
    rec: Recorder,
    /// The sparse slot of each of `rec`'s stamps, resolved against the pattern whose
    /// [`crate::sparse::Pattern::id`] it carries ([`NO_SLOT`] for a stamp that is not a stored
    /// matrix entry). Filled on the first sparse replay and kept until the pattern changes, so a
    /// replay adds into the matrix directly instead of hashing every `(row, col)` again.
    slots: Option<(u64, Vec<u32>)>,
}

/// A stamp with no cached slot: a residual or charge entry, a stamp outside the system, or one
/// the pattern does not (yet) hold. Replayed through the sink as before.
const NO_SLOT: u32 = u32::MAX;

impl Bypass {
    /// A cache for `instances`, bypassing within `abstol[u] + reltol·max(|x|, |x0|)` on every
    /// unknown `u`. `abstol` has one entry per row of `x`, each in that unknown's own units; an
    /// unknown it does not cover is never treated as unmoved.
    pub fn new(instances: &[&dyn ModelInstance], reltol: f64, abstol: Vec<f64>) -> Self {
        Bypass {
            reltol,
            abstol,
            eligible: instances.iter().map(|i| i.is_pure_in_x()).collect(),
            entries: instances.iter().map(|_| None).collect(),
            hits: 0,
            misses: 0,
        }
    }

    /// The absolute tolerance on each global unknown, in that unknown's own units.
    pub fn abstol(&self) -> &[f64] {
        &self.abstol
    }

    /// How many instances can be bypassed at all.
    pub fn eligible_count(&self) -> usize {
        self.eligible.iter().filter(|&&e| e).count()
    }

    fn fresh(&self, i: usize, x: &[f64]) -> bool {
        let Some(e) = self.entries.get(i).and_then(|e| e.as_ref()) else {
            return false;
        };
        e.x0.iter().all(|&(u, v0)| {
            let v = x[u];
            self.abstol
                .get(u)
                .is_some_and(|&a| (v - v0).abs() <= a + self.reltol * v.abs().max(v0.abs()))
        })
    }

    fn store(&mut self, i: usize, inst: &dyn ModelInstance, x: &[f64], rec: Recorder) {
        let x0: Vec<(usize, f64)> = inst
            .unknowns()
            .iter()
            .filter(|&&u| u < x.len())
            .map(|&u| (u, x[u]))
            .collect();
        let covered = rec.0.iter().all(|s| match *s {
            // A column outside `x` is ground: it never moves, so it needs no correction.
            Stamp::Jacobian(_, c, _) | Stamp::Dcharge(_, c, _) => {
                c >= x.len() || x0.iter().any(|&(u, _)| u == c)
            }
            // A task is a side effect a replay would repeat or skip at the wrong time; a module
            // with one does not claim purity (`va_codegen`), and this keeps a hand-written one
            // that did from being bypassed.
            Stamp::Excitation(..) | Stamp::BoundStep(_) | Stamp::Task(..) | Stamp::File(..) => {
                false
            }
            _ => true,
        });
        if covered {
            self.entries[i] = Some(BypassEntry {
                x0,
                rec,
                slots: None,
            });
        } else {
            self.eligible[i] = false;
            self.entries[i] = None;
        }
    }
}

impl BypassEntry {
    /// [`Self::replay_corrected`] into a sparse system through the cached slots: the same
    /// additions, in the same order, with the same values — so the same bits — without a hash
    /// lookup per Jacobian and charge-Jacobian stamp. The slots are resolved on the first
    /// replay against a pattern and again whenever the pattern has changed since.
    fn replay_corrected_sparse(&mut self, x: &[f64], sys: &mut SparseSystem) {
        let id = sys.pattern().id();
        if self.slots.as_ref().map(|(pid, _)| *pid) != Some(id) {
            let pattern = sys.pattern();
            let slots = self
                .rec
                .0
                .iter()
                .map(|s| match *s {
                    Stamp::Jacobian(r, c, _) | Stamp::Dcharge(r, c, _) => pattern
                        .slot(r, c)
                        .and_then(|k| u32::try_from(k).ok())
                        .unwrap_or(NO_SLOT),
                    _ => NO_SLOT,
                })
                .collect();
            self.slots = Some((id, slots));
        }
        let Some((_, slots)) = self.slots.as_ref() else {
            return;
        };
        let dx = |c: usize| {
            self.x0
                .iter()
                .find(|&&(u, _)| u == c)
                .map_or(0.0, |&(u, v0)| x[u] - v0)
        };
        for (s, &slot) in self.rec.0.iter().zip(slots) {
            match *s {
                Stamp::Jacobian(r, c, g) => {
                    if slot == NO_SLOT {
                        StampSink::jacobian(sys, r, c, g);
                    } else {
                        sys.add_jacobian_at(slot as usize, g);
                    }
                    let d = dx(c);
                    if d != 0.0 {
                        StampSink::residual(sys, r, g * d);
                    }
                }
                Stamp::Dcharge(r, c, g) => {
                    if slot == NO_SLOT {
                        StampSink::dcharge(sys, r, c, g);
                    } else {
                        sys.add_dcharge_at(slot as usize, g);
                    }
                    let d = dx(c);
                    if d != 0.0 {
                        StampSink::charge(sys, r, g * d);
                    }
                }
                Stamp::Residual(r, v) => StampSink::residual(sys, r, v),
                Stamp::Charge(r, v) => StampSink::charge(sys, r, v),
                Stamp::Excitation(r, re, im) => sys.excitation(r, re, im),
                Stamp::BoundStep(dt) => StampSink::bound_step(sys, dt),
                Stamp::Task(kind, ref text) => sys.task(kind, text),
                Stamp::File(op, ref path, ref text) => sys.file(op, path.as_deref(), text),
            }
        }
    }

    fn replay_corrected(&self, x: &[f64], sink: &mut dyn StampSink) {
        let dx = |c: usize| {
            self.x0
                .iter()
                .find(|&&(u, _)| u == c)
                .map_or(0.0, |&(u, v0)| x[u] - v0)
        };
        for s in &self.rec.0 {
            match *s {
                Stamp::Residual(r, v) => sink.residual(r, v),
                Stamp::Jacobian(r, c, g) => {
                    sink.jacobian(r, c, g);
                    let d = dx(c);
                    if d != 0.0 {
                        sink.residual(r, g * d);
                    }
                }
                Stamp::Charge(r, v) => sink.charge(r, v),
                Stamp::Dcharge(r, c, g) => {
                    sink.dcharge(r, c, g);
                    let d = dx(c);
                    if d != 0.0 {
                        sink.charge(r, g * d);
                    }
                }
                Stamp::Excitation(r, re, im) => sink.excitation(r, re, im),
                Stamp::BoundStep(dt) => sink.bound_step(dt),
                Stamp::Task(kind, ref text) => sink.task(kind, text),
                Stamp::File(op, ref path, ref text) => sink.file(op, path.as_deref(), text),
            }
        }
    }
}

/// [`load_all`] with device bypass: every [`Bypass`]-eligible instance whose unknowns are within
/// tolerance of its recording is replayed from it (corrected to first order); every other
/// instance is evaluated, in parallel when the pool has threads and there is enough to do, and
/// an eligible one's fresh recording replaces its old one.
pub fn load_all_bypass(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    sink: &mut dyn StampSink,
    bypass: &mut Bypass,
) {
    bypass_into(instances, x, ctx, states, Target::Any(sink), bypass);
}

/// [`load_all_bypass`] into a sparse system: identical stamps in identical order, with every
/// replayed recording added through its cached slots (`BypassEntry::replay_corrected_sparse`,
/// 2.10.0) and every fresh evaluation through `cache` ([`SlotCache`], 2.11.0) instead of a hash
/// lookup per stamp — the serial part of a bypassed assembly that grew fastest with circuit size
/// (`docs/proposals/device-bypass.md` §6).
pub fn load_all_bypass_sparse(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    sys: &mut SparseSystem,
    bypass: &mut Bypass,
    cache: &mut SlotCache,
) {
    bypass_into(
        instances,
        x,
        ctx,
        states,
        Target::Sparse(sys, cache),
        bypass,
    );
}

/// [`load_all`] into a sparse system, with every parallel evaluation's recording added through
/// `cache` ([`SlotCache`]): the same additions in the same order, so the same bits, without a
/// hash lookup per Jacobian and charge-Jacobian stamp. The serial path (a circuit too cheap to
/// evaluate in parallel) stamps directly, as [`load_all`] does.
pub fn load_all_sparse(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    sys: &mut SparseSystem,
    cache: &mut SlotCache,
) {
    load_all_into(instances, x, ctx, states, Target::Sparse(sys, cache));
}

/// Where an assembly's stamps go: any sink, or a sparse system with the slot cache that lets a
/// recording be added without a hash lookup per stamp.
enum Target<'a> {
    Any(&'a mut dyn StampSink),
    Sparse(&'a mut SparseSystem, &'a mut SlotCache),
}

impl Target<'_> {
    fn sink(&mut self) -> &mut dyn StampSink {
        match self {
            Target::Any(s) => &mut **s,
            Target::Sparse(s, _) => &mut **s,
        }
    }

    /// Add instance `i`'s fresh recording.
    fn replay_fresh(&mut self, i: usize, rec: &Recorder) {
        match self {
            Target::Any(s) => rec.replay(&mut **s),
            Target::Sparse(sys, cache) => cache.replay(i, rec, sys),
        }
    }
}

/// Each instance's stamp layout from its last fresh evaluation — `(row, col, slot)` for every
/// Jacobian and charge-Jacobian stamp, in the order it made them — so the next recording of the
/// same instance is added into a sparse system by position instead of by a hash lookup (2.11.0).
///
/// A model usually stamps the same entries in the same order every time; where it does not (a
/// branch taken differently), the first stamp that differs from the layout is looked up and the
/// layout is rewritten from there on, so the cache is never wrong, only less useful. Valid for one
/// sparse pattern ([`crate::sparse::Pattern::id`]); a new pattern clears it.
#[derive(Default, Debug)]
pub struct SlotCache {
    pattern: u64,
    layouts: Vec<Vec<(usize, usize, u32)>>,
}

impl SlotCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add instance `i`'s recording `rec` into `sys`: exactly what [`Recorder::replay`] does,
    /// through the layout instead of a hash lookup wherever the layout still matches.
    fn replay(&mut self, i: usize, rec: &Recorder, sys: &mut SparseSystem) {
        let id = sys.pattern().id();
        if id != self.pattern {
            self.layouts.iter_mut().for_each(Vec::clear);
            self.pattern = id;
        }
        if self.layouts.len() <= i {
            self.layouts.resize_with(i + 1, Vec::new);
        }
        let layout = &mut self.layouts[i];
        let mut k = 0;
        for s in &rec.0 {
            match *s {
                Stamp::Jacobian(r, c, v) | Stamp::Dcharge(r, c, v) => {
                    let slot = match layout.get(k) {
                        Some(&(lr, lc, slot)) if lr == r && lc == c => slot,
                        _ => {
                            let slot = sys
                                .pattern()
                                .slot(r, c)
                                .and_then(|x| u32::try_from(x).ok())
                                .unwrap_or(NO_SLOT);
                            layout.truncate(k);
                            layout.push((r, c, slot));
                            slot
                        }
                    };
                    k += 1;
                    let jacobian = matches!(s, Stamp::Jacobian(..));
                    match (slot == NO_SLOT, jacobian) {
                        (true, true) => StampSink::jacobian(sys, r, c, v),
                        (true, false) => StampSink::dcharge(sys, r, c, v),
                        (false, true) => sys.add_jacobian_at(slot as usize, v),
                        (false, false) => sys.add_dcharge_at(slot as usize, v),
                    }
                }
                Stamp::Residual(r, v) => StampSink::residual(sys, r, v),
                Stamp::Charge(r, v) => StampSink::charge(sys, r, v),
                Stamp::Excitation(r, re, im) => sys.excitation(r, re, im),
                Stamp::BoundStep(dt) => StampSink::bound_step(sys, dt),
                Stamp::Task(kind, ref text) => sys.task(kind, text),
                Stamp::File(op, ref path, ref text) => sys.file(op, path.as_deref(), text),
            }
        }
        layout.truncate(k);
    }
}

fn bypass_into(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    states: Vec<ModelState<'_>>,
    mut target: Target<'_>,
    bypass: &mut Bypass,
) {
    let n = instances.len().min(states.len());
    let hit: Vec<bool> = (0..n)
        .map(|i| bypass.eligible[i] && bypass.fresh(i, x))
        .collect();
    let work: Vec<(usize, ModelState<'_>)> = states
        .into_iter()
        .take(n)
        .enumerate()
        .filter(|(i, _)| !hit[*i])
        .collect();
    let est = NS_PER_INSTANCE.load(Ordering::Relaxed);
    let parallel = work.len() >= 2
        && rayon::current_num_threads() > 1
        && match mode() {
            Mode::Never => false,
            Mode::Always => true,
            Mode::Auto => est == 0 || est.saturating_mul(work.len() as u64) >= PARALLEL_MIN_WORK_NS,
        };
    let eval = |(i, mut st): (usize, ModelState<'_>)| {
        let mut rec = Recorder::default();
        instances[i].load(x, ctx, &mut st, &mut rec);
        (i, rec)
    };
    let recorded: Vec<(usize, Recorder)> = if parallel {
        work.into_par_iter().map(eval).collect()
    } else {
        work.into_iter().map(eval).collect()
    };
    let mut recorded = recorded.into_iter().peekable();
    for (i, &is_hit) in hit.iter().enumerate() {
        if is_hit {
            bypass.hits += 1;
            if let Some(e) = bypass.entries[i].as_mut() {
                match &mut target {
                    Target::Sparse(sys, _) => e.replay_corrected_sparse(x, sys),
                    Target::Any(sink) => e.replay_corrected(x, &mut **sink),
                }
            }
            continue;
        }
        let Some((j, rec)) = recorded.next_if(|(j, _)| *j == i) else {
            continue;
        };
        debug_assert_eq!(i, j);
        target.replay_fresh(i, &rec);
        if bypass.eligible[i] {
            bypass.misses += 1;
            bypass.store(i, instances[i], x, rec);
        }
    }
}

fn serial(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    ctx: &AnalysisCtx,
    mut states: Vec<ModelState<'_>>,
    sink: &mut dyn StampSink,
) {
    for (inst, st) in instances.iter().zip(states.iter_mut()) {
        inst.load(x, ctx, st, sink);
    }
}

/// One [`StampSink`] call, as recorded.
#[derive(Clone, Debug, PartialEq)]
enum Stamp {
    Residual(usize, f64),
    Jacobian(usize, usize, f64),
    Charge(usize, f64),
    Dcharge(usize, usize, f64),
    Excitation(usize, f64, f64),
    BoundStep(f64),
    Task(va_abi::TaskKind, Box<str>),
    File(va_abi::FileOp, Option<Box<str>>, Box<str>),
}

/// A [`StampSink`] that stores every call, in order, for [`Recorder::replay`].
///
/// It overrides **every** `StampSink` method, including the four with default bodies
/// (`excitation`, `bound_step`, `task`, `file`): a method left to its default would be silently dropped from
/// the parallel path while the serial path honoured it. A method added to `StampSink` must be
/// added here too.
#[derive(Default, Debug)]
pub struct Recorder(Vec<Stamp>);

impl Recorder {
    /// Re-issue every recorded call into `sink`, in the order it was made.
    pub fn replay(&self, sink: &mut dyn StampSink) {
        for s in &self.0 {
            match *s {
                Stamp::Residual(r, v) => sink.residual(r, v),
                Stamp::Jacobian(r, c, v) => sink.jacobian(r, c, v),
                Stamp::Charge(r, v) => sink.charge(r, v),
                Stamp::Dcharge(r, c, v) => sink.dcharge(r, c, v),
                Stamp::Excitation(r, re, im) => sink.excitation(r, re, im),
                Stamp::BoundStep(dt) => sink.bound_step(dt),
                Stamp::Task(kind, ref text) => sink.task(kind, text),
                Stamp::File(op, ref path, ref text) => sink.file(op, path.as_deref(), text),
            }
        }
    }
}

impl StampSink for Recorder {
    fn residual(&mut self, row: usize, value: f64) {
        self.0.push(Stamp::Residual(row, value));
    }
    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        self.0.push(Stamp::Jacobian(row, col, value));
    }
    fn charge(&mut self, row: usize, value: f64) {
        self.0.push(Stamp::Charge(row, value));
    }
    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        self.0.push(Stamp::Dcharge(row, col, value));
    }
    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        self.0.push(Stamp::Excitation(row, re, im));
    }
    fn bound_step(&mut self, dt: f64) {
        self.0.push(Stamp::BoundStep(dt));
    }
    fn task(&mut self, kind: va_abi::TaskKind, text: &str) {
        self.0.push(Stamp::Task(kind, text.into()));
    }
    fn file(&mut self, op: va_abi::FileOp, path: Option<&str>, text: &str) {
        self.0
            .push(Stamp::File(op, path.map(Into::into), text.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::reference::{Capacitor, Diode, Resistor, VSource, GROUND};
    use va_abi::ANALYSIS_DC;

    /// A sink that logs every call verbatim, so two runs can be compared call by call.
    #[derive(Default)]
    struct Log(Vec<Stamp>);
    impl StampSink for Log {
        fn residual(&mut self, row: usize, value: f64) {
            self.0.push(Stamp::Residual(row, value));
        }
        fn jacobian(&mut self, row: usize, col: usize, value: f64) {
            self.0.push(Stamp::Jacobian(row, col, value));
        }
        fn charge(&mut self, row: usize, value: f64) {
            self.0.push(Stamp::Charge(row, value));
        }
        fn dcharge(&mut self, row: usize, col: usize, value: f64) {
            self.0.push(Stamp::Dcharge(row, col, value));
        }
        fn excitation(&mut self, row: usize, re: f64, im: f64) {
            self.0.push(Stamp::Excitation(row, re, im));
        }
        fn bound_step(&mut self, dt: f64) {
            self.0.push(Stamp::BoundStep(dt));
        }
    }

    /// Run `f` with the mode forced to [`Mode::Always`]. The mode is process-wide and tests run
    /// concurrently, so this holds a lock for the duration; `Always` changes only the path
    /// taken, never a result, so a concurrent test that sees it is unaffected.
    fn parallel_always(f: impl FnOnce()) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_mode(Mode::Always);
        f();
        set_mode(Mode::Auto);
    }

    /// An instance that makes every kind of `StampSink` call, including the two with default
    /// bodies — the ones a recorder could forget.
    struct EveryStamp(usize);
    impl ModelInstance for EveryStamp {
        fn unknowns(&self) -> &[usize] {
            &[]
        }
        fn load(&self, x: &[f64], _: &AnalysisCtx, _: &mut ModelState, sink: &mut dyn StampSink) {
            let k = self.0;
            sink.residual(k, x[0] * k as f64);
            sink.jacobian(k, 0, 1.5);
            sink.charge(k, 2.5);
            sink.dcharge(k, 0, 3.5);
            sink.excitation(k, 4.5, -5.5);
            sink.bound_step(1e-9 * (k + 1) as f64);
        }
    }

    /// Replaying a recording issues exactly the calls, in exactly the order, that loading
    /// straight into the sink does — for every `StampSink` method.
    #[test]
    fn a_replayed_recording_is_the_same_calls_as_direct_stamping() {
        let inst = EveryStamp(3);
        let x = [0.25];
        let mut direct = Log::default();
        inst.load(&x, &ANALYSIS_DC, &mut ModelState::stateless(), &mut direct);
        let mut rec = Recorder::default();
        inst.load(&x, &ANALYSIS_DC, &mut ModelState::stateless(), &mut rec);
        let mut replayed = Log::default();
        rec.replay(&mut replayed);
        assert_eq!(direct.0.len(), 6, "the fixture makes one call of each kind");
        assert_eq!(replayed.0, direct.0);
    }

    /// `load_all` in parallel hands the sink the same call sequence as the serial loop, in
    /// pools of 1, 2, 4 and 8 threads, for a circuit that mixes real reference models with the
    /// every-stamp fixture. [`Mode::Always`], so the parallel path is what runs.
    #[test]
    fn load_all_gives_the_serial_call_sequence_at_any_thread_count() {
        let n = 40;
        let resistors: Vec<Resistor> = (0..n)
            .map(|i| Resistor::new(i, i + 1, 1.0 + i as f64))
            .collect();
        let diodes: Vec<Diode> = (0..n)
            .map(|i| Diode::new(i + 1, GROUND, 1e-14, 1.0, 0.025_852))
            .collect();
        let caps: Vec<Capacitor> = (0..n).map(|i| Capacitor::new(i, GROUND, 1e-12)).collect();
        let every: Vec<EveryStamp> = (0..n).map(EveryStamp).collect();
        let vs = VSource::new(0, GROUND, n + 1, 1.0);
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        for i in 0..n {
            insts.push(&resistors[i]);
            insts.push(&diodes[i]);
            insts.push(&caps[i]);
            insts.push(&every[i]);
        }
        let x: Vec<f64> = (0..n + 2).map(|i| 0.6 + 0.001 * i as f64).collect();
        let states = |len: usize| {
            (0..len)
                .map(|_| ModelState::stateless())
                .collect::<Vec<_>>()
        };

        let mut serial = Log::default();
        for inst in &insts {
            inst.load(&x, &ANALYSIS_DC, &mut ModelState::stateless(), &mut serial);
        }
        for threads in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let mut got = Log::default();
            pool.install(|| {
                parallel_always(|| {
                    load_all(&insts, &x, &ANALYSIS_DC, states(insts.len()), &mut got)
                })
            });
            assert_eq!(got.0, serial.0, "{threads} threads");
        }
    }

    /// A reference diode that declares itself pure in `x` (it is: Shockley, no state), so the
    /// bypass has something to bypass without a compiled model.
    struct PureDiode(Diode);
    impl ModelInstance for PureDiode {
        fn unknowns(&self) -> &[usize] {
            self.0.unknowns()
        }
        fn load(
            &self,
            x: &[f64],
            ctx: &AnalysisCtx,
            st: &mut ModelState,
            sink: &mut dyn StampSink,
        ) {
            self.0.load(x, ctx, st, sink)
        }
        fn is_pure_in_x(&self) -> bool {
            true
        }
    }

    /// Dense residual and Jacobian of one load through `f`.
    fn dense(dim: usize, f: impl FnOnce(&mut va_abi::stamps::DenseStamp)) -> (Vec<f64>, Vec<f64>) {
        let mut d = va_abi::stamps::DenseStamp::new(dim);
        f(&mut d);
        (d.residual, d.jacobian)
    }

    /// Device bypass: at the recorded `x` the replay is the evaluation, bit for bit; within
    /// tolerance it is the evaluation linearised at the recording, so its residual error is
    /// second order in the drift; past tolerance the instance is evaluated again; and an
    /// instance that does not declare itself pure is never bypassed.
    #[test]
    fn bypass_replays_exactly_corrects_to_first_order_and_reevaluates_past_tolerance() {
        let pure = PureDiode(Diode::new(0, 1, 1e-14, 1.0, 0.025_852));
        let plain = Diode::new(1, GROUND, 1e-14, 1.0, 0.025_852);
        let insts: [&dyn ModelInstance; 2] = [&pure, &plain];
        let st = || vec![ModelState::stateless(), ModelState::stateless()];
        let fresh = |x: &[f64]| dense(2, |d| load_all(&insts, x, &ANALYSIS_DC, st(), d));
        let mut bp = Bypass::new(&insts, 1e-3, vec![1e-6; 2]);
        assert_eq!(bp.eligible_count(), 1);
        let bypassed = |x: &[f64], bp: &mut Bypass| {
            dense(2, |d| load_all_bypass(&insts, x, &ANALYSIS_DC, st(), d, bp))
        };

        let x0 = [0.65, 0.0];
        assert_eq!(bypassed(&x0, &mut bp), fresh(&x0), "the recording load");
        assert_eq!((bp.hits, bp.misses), (0, 1));
        assert_eq!(
            bypassed(&x0, &mut bp),
            fresh(&x0),
            "a replay at x0 is exact"
        );
        assert_eq!((bp.hits, bp.misses), (1, 1));

        // Within tolerance (0.4 mV on 0.65 V): replayed, linearised at x0.
        let dv = 4e-4;
        let x1 = [0.65 + dv, 0.0];
        let (r_bp, j_bp) = bypassed(&x1, &mut bp);
        assert_eq!(bp.hits, 2, "replayed");
        let (r_fresh, _) = fresh(&x1);
        let (r0, _) = fresh(&x0);
        let err = (r_bp[0] - r_fresh[0]).abs();
        let first_order = (r_fresh[0] - r0[0]).abs();
        assert!(
            err < 1e-2 * first_order,
            "error {err:e} vs first-order change {first_order:e}"
        );
        assert_eq!(j_bp, fresh(&x0).1, "the Jacobian is the recording's");

        // Past tolerance: evaluated again, and exact.
        let x2 = [0.70, 0.0];
        assert_eq!(bypassed(&x2, &mut bp), fresh(&x2));
        assert_eq!(bp.misses, 2);
    }

    /// A device whose stamp layout depends on its operating point: above 0.5 V it adds a
    /// coupling entry `(0, 1)` in the middle of its stamps, below it does not — the case where a
    /// cached layout stops matching part-way through a recording.
    struct Shifty;
    impl ModelInstance for Shifty {
        fn unknowns(&self) -> &[usize] {
            &[0, 1]
        }
        fn load(&self, x: &[f64], _: &AnalysisCtx, _: &mut ModelState, sink: &mut dyn StampSink) {
            sink.jacobian(0, 0, 1.0 + x[0]);
            if x[0] > 0.5 {
                sink.jacobian(0, 1, -0.25 * x[0]);
            }
            sink.dcharge(1, 1, 1e-12 * (1.0 + x[1]));
            sink.jacobian(1, 1, 2.0 + x[1]);
            sink.residual(0, x[0] * x[0]);
        }
    }

    /// `SlotCache` (2.11.0) adds a recording exactly as `Recorder::replay` does: equal bits
    /// whether the layout matches, stops matching part-way (a branch taken differently), comes
    /// back, or the pattern is a new one (every cached slot stale, an entry in overflow).
    #[test]
    fn slot_cache_replays_a_recording_bit_for_bit_whatever_its_layout_does() {
        let shifty = Shifty;
        let diode = Diode::new(1, GROUND, 1e-14, 1.0, 0.025_852);
        let insts: [&dyn ModelInstance; 2] = [&shifty, &diode];
        let mut cache = SlotCache::new();
        let mut cached = SparseSystem::new(2);
        let mut plain = SparseSystem::new(2);
        let mut check = |x: &[f64], cached: &mut SparseSystem, plain: &mut SparseSystem| {
            cached.clear();
            plain.clear();
            for (i, inst) in insts.iter().enumerate() {
                let mut rec = Recorder::default();
                inst.load(x, &ANALYSIS_DC, &mut ModelState::stateless(), &mut rec);
                cache.replay(i, &rec, cached);
                rec.replay(plain);
            }
            cached.finish();
            plain.finish();
            assert_eq!(cached.residual_values(), plain.residual_values(), "x={x:?}");
            assert_eq!(
                SparseSystem::jacobian(cached).values(),
                SparseSystem::jacobian(plain).values(),
                "x={x:?}"
            );
            assert_eq!(
                SparseSystem::dcharge(cached).values(),
                SparseSystem::dcharge(plain).values(),
                "x={x:?}"
            );
        };
        check(&[0.2, 0.6], &mut cached, &mut plain); // grows the pattern (no coupling)
        check(&[0.2, 0.6], &mut cached, &mut plain); // layout matches throughout
        check(&[0.8, 0.6], &mut cached, &mut plain); // coupling appears mid-recording
        check(&[0.8, 0.65], &mut cached, &mut plain); // the rewritten layout matches
        check(&[0.3, 0.65], &mut cached, &mut plain); // and disappears again
        let mut fresh_cached = SparseSystem::new(2);
        let mut fresh_plain = SparseSystem::new(2);
        check(&[0.9, 0.7], &mut fresh_cached, &mut fresh_plain); // a new pattern: stale slots
    }

    /// The slot cache (`BypassEntry::replay_corrected_sparse`, 2.10.0) is an implementation of
    /// the same replay, not an approximation of it: through `load_all_bypass_sparse` every
    /// assembly gives the bits `load_all_bypass` gives through the sink, at the recorded `x`,
    /// within tolerance and past it — and still after the pattern grows, which changes its id
    /// and so every cached slot, the case a stale cache would get wrong.
    #[test]
    fn slot_cached_replay_gives_the_sinks_bits_and_survives_a_grown_pattern() {
        let pure = PureDiode(Diode::new(0, 1, 1e-14, 1.0, 0.025_852));
        let plain = Diode::new(1, GROUND, 1e-14, 1.0, 0.025_852);
        let insts: [&dyn ModelInstance; 2] = [&pure, &plain];
        let st = || vec![ModelState::stateless(), ModelState::stateless()];
        let mut bp_slot = Bypass::new(&insts, 1e-3, vec![1e-6; 2]);
        let mut bp_sink = Bypass::new(&insts, 1e-3, vec![1e-6; 2]);
        let mut sys_slot = SparseSystem::new(2);
        let mut sys_sink = SparseSystem::new(2);
        let mut slot_cache = SlotCache::new();
        let mut assemble = |x: &[f64], bp_slot: &mut Bypass, bp_sink: &mut Bypass| {
            let before = sys_slot.pattern().id();
            sys_slot.clear();
            load_all_bypass_sparse(
                &insts,
                x,
                &ANALYSIS_DC,
                st(),
                &mut sys_slot,
                bp_slot,
                &mut slot_cache,
            );
            sys_slot.finish();
            sys_sink.clear();
            load_all_bypass(&insts, x, &ANALYSIS_DC, st(), &mut sys_sink, bp_sink);
            sys_sink.finish();
            assert_eq!(
                sys_slot.residual_values(),
                sys_sink.residual_values(),
                "x={x:?}"
            );
            assert_eq!(
                sys_slot.jacobian().values(),
                sys_sink.jacobian().values(),
                "x={x:?}"
            );
            (before, sys_slot.pattern().id())
        };
        // The first assembly grows the pattern from the diagonal; the next ones replay against
        // the grown one, whose id differs from the pattern the recording was taken under.
        let (diagonal, grown) = assemble(&[0.65, 0.0], &mut bp_slot, &mut bp_sink);
        assert_ne!(diagonal, grown, "the first finish grew the pattern");
        assemble(&[0.65, 0.0], &mut bp_slot, &mut bp_sink);
        assemble(&[0.65 + 4e-4, 0.0], &mut bp_slot, &mut bp_sink);
        assemble(&[0.70, 0.0], &mut bp_slot, &mut bp_sink);
        assemble(&[0.70 + 1e-4, 0.0], &mut bp_slot, &mut bp_sink);
        assert_eq!(
            (bp_slot.hits, bp_slot.misses),
            (bp_sink.hits, bp_sink.misses)
        );
        assert!(
            bp_slot.hits >= 3,
            "the cached path ran: {} hits",
            bp_slot.hits
        );

        // Slots cached against that pattern, then a different system: a new id and a
        // diagonal-only pattern, so every cached slot is stale and the off-diagonal stamps must
        // go through the sink's overflow. Replayed (a hit) into both, still the same bits.
        let hits_before = bp_slot.hits;
        let x = [0.70 + 1e-4, 0.0];
        let mut fresh_slot = SparseSystem::new(2);
        let mut fresh_sink = SparseSystem::new(2);
        load_all_bypass_sparse(
            &insts,
            &x,
            &ANALYSIS_DC,
            st(),
            &mut fresh_slot,
            &mut bp_slot,
            &mut slot_cache,
        );
        load_all_bypass(
            &insts,
            &x,
            &ANALYSIS_DC,
            st(),
            &mut fresh_sink,
            &mut bp_sink,
        );
        assert_eq!(bp_slot.hits, hits_before + 1, "replayed, not re-evaluated");
        fresh_slot.finish();
        fresh_sink.finish();
        assert_eq!(fresh_slot.residual_values(), fresh_sink.residual_values());
        assert_eq!(
            fresh_slot.jacobian().values(),
            fresh_sink.jacobian().values()
        );
    }
}
