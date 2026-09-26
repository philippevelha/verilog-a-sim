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
                    serial(instances, x, ctx, states, sink);
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
        for rec in &recorded {
            rec.replay(sink);
        }
    } else {
        serial(instances, x, ctx, states, sink);
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
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stamp {
    Residual(usize, f64),
    Jacobian(usize, usize, f64),
    Charge(usize, f64),
    Dcharge(usize, usize, f64),
    Excitation(usize, f64, f64),
    BoundStep(f64),
}

/// A [`StampSink`] that stores every call, in order, for [`Recorder::replay`].
///
/// It overrides **every** `StampSink` method, including the two with default bodies
/// (`excitation`, `bound_step`): a method left to its default would be silently dropped from
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
}
