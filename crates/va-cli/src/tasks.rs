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

use va_abi::{ModelInstance, TaskKind};
use va_ir::Module;

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

/// The transient run's [`va_transient::integrator::TranConfig::on_task`].
pub(crate) fn transient_hook() -> Option<va_transient::integrator::TaskHook> {
    PRINT
        .load(Ordering::Relaxed)
        .then_some(on_transient_task as va_transient::integrator::TaskHook)
}

fn on_transient_task(kind: TaskKind, text: &str, t: f64) {
    emit(kind, text, &format!("t={t:e}"));
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

/// Whether any of `modules` contains a system task, in its analog block or in a function — the
/// test for whether a static analysis needs [`at_static_point`] at all.
pub(crate) fn any_module_has_tasks(modules: &[Module]) -> bool {
    fn walk(stmts: &[va_ir::Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            va_ir::Stmt::Task { .. } => true,
            va_ir::Stmt::Assign { .. }
            | va_ir::Stmt::Contribute { .. }
            | va_ir::Stmt::BoundStep(_) => false,
            va_ir::Stmt::If { then_, else_, .. } => walk(then_) || walk(else_),
            va_ir::Stmt::Block(b)
            | va_ir::Stmt::While { body: b, .. }
            | va_ir::Stmt::Repeat { body: b, .. } => walk(b),
            va_ir::Stmt::For {
                init, step, body, ..
            } => {
                walk(std::slice::from_ref(init.as_ref()))
                    || walk(std::slice::from_ref(step.as_ref()))
                    || walk(body)
            }
            va_ir::Stmt::Case { arms, default, .. } => {
                arms.iter().any(|a| walk(&a.body)) || walk(default)
            }
        })
    }
    modules
        .iter()
        .any(|m| walk(&m.analog) || m.functions.iter().any(|f| walk(&f.body)))
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
