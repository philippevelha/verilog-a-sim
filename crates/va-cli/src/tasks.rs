//! A model's **system tasks** at the command line (2.8.0): where `$strobe`, `$display`,
//! `$write`, `$debug`, `$info`, `$warning`, `$error`, `$fatal`, `$finish` and `$stop` output goes,
//! and when a static analysis acts on them.
//!
//! - The display tasks print their text on **stdout**, where the report goes, one line each
//!   (`$write` without its newline).
//! - The severity tasks print `$<task> at <when>: <text>` on **stdout and stderr** alike, so a
//!   saved report and a log both carry them; every `$error` makes `va-cli sim` exit with status 1
//!   once its results are printed.
//! - `$finish`/`$stop` end the analysis after the accepted point; [`finish_note`] says so.
//!
//! **When**: a transient run reports from its accepted timepoints (`va_transient::integrator::
//! TranConfig::on_task`). A static analysis — an operating point, each `.dc` sweep point, the
//! operating point of `.ac`/`.noise` — reports from **one extra evaluation at the converged
//! solution** ([`at_static_point`]), which is that analysis's accepted solution (LRM §9.4.6).
//! **Stated limitation:** in a static analysis `$debug` therefore prints once, from that
//! evaluation, rather than on every Newton iteration as the LRM asks; the DC solver does not
//! report its iterations' tasks.

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use va_abi::{FileOp, ModelInstance, TaskKind};

static PRINT: AtomicBool = AtomicBool::new(false);
static ERRORS: AtomicUsize = AtomicUsize::new(0);

/// Print model task output from here on (`va-cli sim` turns it on). Off by default, so a
/// library caller — the harness, a test — is not handed output it did not ask for; `$finish`
/// and `$fatal` act either way.
pub fn set_task_output(on: bool) {
    PRINT.store(on, Ordering::Relaxed);
}

/// How many `$error` messages models have reported in this process.
pub fn task_errors() -> usize {
    ERRORS.load(Ordering::Relaxed)
}

static ONCE: std::sync::Mutex<Option<OnceFilter>> = std::sync::Mutex::new(None);

/// The "statement walk stopped" warnings (`va_codegen::WALK_STOPPED`) already printed: each
/// distinct one prints once whatever `--tasks-once` says, because a model that hits one usually
/// hits it at every point, and one line says everything the next thousand would.
static WALK: std::sync::Mutex<Option<OnceFilter>> = std::sync::Mutex::new(None);

/// How many repeats of an already-printed "evaluation stopped early" warning were held back.
pub fn suppressed_walk_warnings() -> usize {
    WALK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map_or(0, |f| f.suppressed)
}

/// Print each distinct task line once (`va-cli sim --tasks-once`, 2.9.0): a line identical to
/// one already printed — same task, same text — is counted instead of printed. Lines are told
/// apart by their text alone, so two instances stay distinct only if the model writes `%m`
/// (BSIM4's checks do not: its four NAND instances share one line). **Not the LRM's behaviour**, which prints a task every time it
/// runs at an accepted point; it exists for vendor models whose parameter checks sit in plain
/// analog code and so repeat at every point (`benchmark/nand/bsim4.net`: 404 identical lines).
/// A line whose values change (a `$strobe` of a node voltage) still prints every time, and every
/// `$error` still counts toward the exit status.
pub fn set_tasks_once(on: bool) {
    *ONCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = on.then(OnceFilter::default);
}

/// How many task lines [`set_tasks_once`] has kept from printing; 0 when it is off.
pub fn suppressed_task_lines() -> usize {
    ONCE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map_or(0, |f| f.suppressed)
}

/// [`set_tasks_once`]'s memory: every line printed so far, and how many repeats it held back.
#[derive(Default, Debug)]
pub(crate) struct OnceFilter {
    seen: std::collections::HashSet<(&'static str, String)>,
    suppressed: usize,
}

impl OnceFilter {
    /// Whether this line should print: `true` the first time `(kind, text)` is seen, `false`
    /// (and counted) on every repeat.
    pub(crate) fn admit(&mut self, kind: TaskKind, text: &str) -> bool {
        if self.seen.insert((name(kind), text.to_string())) {
            true
        } else {
            self.suppressed += 1;
            false
        }
    }
}

/// The transient run's [`va_transient::integrator::TranConfig::on_task`].
pub(crate) fn transient_hook() -> Option<va_transient::integrator::TaskHook> {
    PRINT
        .load(Ordering::Relaxed)
        .then_some(on_transient_task as va_transient::integrator::TaskHook)
}

fn on_transient_task(kind: TaskKind, text: &str, t: f64) {
    emit(kind, text, &format!("t={t:e}"));
}

/// The transient run's [`va_transient::integrator::TranConfig::on_file`], on with task output.
pub(crate) fn file_hook() -> Option<va_transient::integrator::FileHook> {
    PRINT
        .load(Ordering::Relaxed)
        .then_some(on_transient_file as va_transient::integrator::FileHook)
}

fn on_transient_file(op: FileOp, path: Option<&str>, text: &str, _t: f64) {
    file_op(op, path, text);
}

/// Files a model has written to in this process: open handles, and every path opened so far.
#[derive(Default)]
struct FileTable {
    open: std::collections::HashMap<String, std::io::BufWriter<std::fs::File>>,
    seen: std::collections::HashSet<String>,
}

static FILES: std::sync::Mutex<Option<FileTable>> = std::sync::Mutex::new(None);

/// Carry out one file operation (`va_abi::StampSink::file`, LRM §9.5). A path is created (or
/// truncated) the **first** time any instance opens it in this process; every later open of
/// the same path, by another instance or after a `$fclose`, appends — so two devices logging to
/// one file do not erase each other. Relative paths are relative to the working directory. A
/// file that cannot be opened is reported once on stderr and its output dropped.
pub(crate) fn file_op(op: FileOp, path: Option<&str>, text: &str) {
    if !PRINT.load(Ordering::Relaxed) {
        return;
    }
    let Some(path) = path else {
        match op {
            FileOp::Write { newline: false } => eprint!("{text}"),
            FileOp::Write { newline: true } | FileOp::Debug => eprintln!("{text}"),
            FileOp::Flush => {
                let mut files = FILES
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for w in files
                    .get_or_insert_with(FileTable::default)
                    .open
                    .values_mut()
                {
                    let _ = w.flush();
                }
            }
            FileOp::Open { .. } | FileOp::Close => {}
        }
        return;
    };
    let mut files = FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let table = files.get_or_insert_with(FileTable::default);
    let ensure_open = |table: &mut FileTable, append: bool| -> bool {
        if table.open.contains_key(path) {
            return true;
        }
        let truncate = !append && !table.seen.contains(path);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(truncate)
            .append(!truncate)
            .open(path);
        match file {
            Ok(f) => {
                table.seen.insert(path.to_string());
                table
                    .open
                    .insert(path.to_string(), std::io::BufWriter::new(f));
                true
            }
            Err(e) => {
                if table.seen.insert(path.to_string()) {
                    eprintln!("[va-cli] warning: a model's $fopen of `{path}` failed ({e}); its output is dropped");
                }
                false
            }
        }
    };
    match op {
        FileOp::Open { append } => {
            ensure_open(table, append);
        }
        FileOp::Write { .. } | FileOp::Debug if ensure_open(table, true) => {
            if let Some(w) = table.open.get_mut(path) {
                let _ = w.write_all(text.as_bytes());
                if op != (FileOp::Write { newline: false }) {
                    let _ = w.write_all(b"\n");
                }
            }
        }
        FileOp::Write { .. } | FileOp::Debug => {}
        FileOp::Flush => {
            if let Some(w) = table.open.get_mut(path) {
                let _ = w.flush();
            }
        }
        FileOp::Close => {
            if let Some(mut w) = table.open.remove(path) {
                let _ = w.flush();
            }
        }
    }
}

/// Flush and close every file a model left open; `va-cli sim` calls it when the run ends,
/// whatever its outcome (a process-wide table is never dropped on its own).
pub fn close_task_files() {
    let mut files = FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(table) = files.as_mut() {
        for (_, mut w) in table.open.drain() {
            let _ = w.flush();
        }
    }
}

/// The task's name as the model wrote it.
pub(crate) fn name(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Strobe => "$strobe",
        TaskKind::Display => "$display",
        TaskKind::Write => "$write",
        TaskKind::Debug => "$debug",
        TaskKind::Finish(_) => "$finish",
        TaskKind::Stop(_) => "$stop",
        TaskKind::Fatal(_) => "$fatal",
        TaskKind::Error => "$error",
        TaskKind::Warning => "$warning",
        TaskKind::Info => "$info",
    }
}

/// Print one task's output; `when` is the time (`t=…`) or the static point it ran at.
pub(crate) fn emit(kind: TaskKind, text: &str, when: &str) {
    if kind == TaskKind::Error {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    if !PRINT.load(Ordering::Relaxed) {
        return;
    }
    if kind == TaskKind::Warning && text.contains(va_codegen::WALK_STOPPED) {
        let mut walk = WALK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !walk
            .get_or_insert_with(OnceFilter::default)
            .admit(kind, text)
        {
            return;
        }
    }
    // `$finish`/`$stop` print nothing here, so they are left out of the filter's count.
    if !matches!(kind, TaskKind::Finish(_) | TaskKind::Stop(_)) {
        if let Some(filter) = ONCE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            if !filter.admit(kind, text) {
                return;
            }
        }
    }
    match kind {
        TaskKind::Strobe | TaskKind::Display | TaskKind::Debug => println!("{text}"),
        TaskKind::Write => {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
        TaskKind::Info | TaskKind::Warning | TaskKind::Error | TaskKind::Fatal(_) => {
            let line = format!("{} at {when}: {text}", name(kind));
            println!("{line}");
            eprintln!("{line}");
        }
        // Said once the analysis has stopped (`finish_note`), not while it is still running.
        TaskKind::Finish(_) | TaskKind::Stop(_) => {}
    }
}

/// The note printed when a model's `$finish`/`$stop` ended an analysis early, `None` at
/// diagnostic level 0 (LRM Table 9-25: "prints nothing").
pub(crate) fn finish_note(kind: TaskKind, by: &str, when: &str) -> Option<String> {
    let level = match kind {
        TaskKind::Finish(n) | TaskKind::Stop(n) => n,
        _ => return None,
    };
    if level == 0 {
        return None;
    }
    let stop = if matches!(kind, TaskKind::Stop(_)) {
        " ($stop suspends a simulation; this engine has no interactive mode to resume it from, so it ends the analysis as $finish does)"
    } else {
        ""
    };
    Some(format!(
        "[va-cli] {} by {by} at {when}: the analysis ended at that point{stop}",
        name(kind)
    ))
}

/// Print [`finish_note`] on both streams, if there is one.
pub(crate) fn report_finish(kind: TaskKind, by: &str, when: &str) {
    if let Some(note) = finish_note(kind, by, when) {
        println!("{note}");
        eprintln!("{note}");
    }
}

/// A static analysis's tasks: evaluate every instance once at its converged solution `x` and act
/// on what they report — print it ([`emit`]) and return the first `$finish`/`$stop` with the
/// instance that asked. `$fatal` is an error.
///
/// # Errors
///
/// When a model executed `$fatal`.
pub(crate) fn at_static_point(
    instances: &[&dyn ModelInstance],
    x: &[f64],
    dim: usize,
    when: &str,
) -> anyhow::Result<Option<(TaskKind, String)>> {
    let mut sys = va_core::sparse::SparseSystem::new(dim);
    let fired = va_abi::FiredEvents::new(instances);
    va_core::sparse::assemble_into(instances, x, &va_abi::ANALYSIS_DC, &fired, &mut sys);
    for (op, path, text) in sys.take_files() {
        file_op(op, path.as_deref(), &text);
    }
    let mut finish = None;
    for (kind, text) in sys.take_tasks() {
        emit(kind, &text, when);
        match kind {
            TaskKind::Fatal(_) => anyhow::bail!("$fatal at {when}: {text}"),
            TaskKind::Finish(_) | TaskKind::Stop(_) if finish.is_none() => {
                finish = Some((kind, text));
            }
            _ => {}
        }
    }
    Ok(finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--tasks-once` holds back exact repeats only: the same line from another instance (its
    /// `%m` differs), the same text from another task, and a line whose value changed all print.
    #[test]
    fn the_once_filter_holds_back_exact_repeats_only() {
        let mut f = OnceFilter::default();
        assert!(f.admit(TaskKind::Strobe, "X1: RECALCULATION for no K1 or K2"));
        assert!(!f.admit(TaskKind::Strobe, "X1: RECALCULATION for no K1 or K2"));
        assert!(f.admit(TaskKind::Strobe, "X2: RECALCULATION for no K1 or K2"));
        assert!(f.admit(TaskKind::Warning, "X1: RECALCULATION for no K1 or K2"));
        assert!(f.admit(TaskKind::Strobe, "X1: V=0.500"));
        assert!(f.admit(TaskKind::Strobe, "X1: V=0.501"));
        assert!(!f.admit(TaskKind::Strobe, "X1: V=0.500"));
        assert_eq!(f.suppressed, 2);
    }
}
