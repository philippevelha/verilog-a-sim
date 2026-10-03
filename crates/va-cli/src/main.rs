//! T6 — `va-cli`: the binary front door that wires the whole pipeline together.
//!
//! Usage (the bring-up target):
//!
//! ```text
//! va-cli sim circuits/divider.net --model models/resistor.va
//! ```
//!
//! This binary only parses arguments and dispatches to [`va_cli::run_sim`]; the pipeline
//! itself lives in the library so `va-harness` can drive it directly.

use anyhow::{bail, Context, Result};
use va_cli::{check_models, run_sim, Analysis, Solver};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // A **refusal** — a construct this implementation recognises and declines — is reported
        // in its own labelled shape and swallowed here, rather than returned for the default
        // `Error: {}` rendering. Two reasons, both about the reader: the default rendering
        // flattens the whole block onto one line, and it opens with `Error:`, which reads as
        // "your model is broken" when the truth is "your model is fine and this simulator will
        // not pretend to run it". Anything else is an ordinary error and prints the usual way.
        Err(e) => {
            if !va_cli::report_refusal(&e) {
                eprintln!("Error: {e:?}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// The real entry point; [`main`] only decides how a failure is rendered.
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sim") => cmd_sim(&args[1..]),
        Some("check") => cmd_check(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            bail!("unknown subcommand `{other}`");
        }
    }
}

/// Print the CLI usage banner.
fn print_usage() {
    eprintln!(
        "va-cli — verilog-a-sim front door\n\n\
         USAGE:\n    \
         va-cli sim <netlist.net> [--model <model.va|dir>] [--ac|--tran|--noise] [--plot <out.svg>]\n    \
         va-cli check <model.va|dir> [more…] [--codegen]   Run the frontend (and, with\n                                                      --codegen, va-codegen) over models\n\n\
         FLAGS:\n    \
         -h, --help    Print this help\n    \
         --model <model.va|dir>  A Verilog-A library; repeat it to compile several as one\n                                 (each file's `include resolves in its own library)\n    \
         --plot <out.svg>        Write an SVG plot: transient waveform, .dc sweep, or AC Bode
    \n         --integration be|trap|gear  Integration method for a transient run.
                                 Default trapezoidal (second order)
    
         --report <a,b,...>      Report only these quantities (default: all of them).
                                 Names a net (`mid`), a device current (`V1`), or a full
                                 label (`V(mid)`); an unknown name is an error.
         --solver auto|dense|sparse  Linear algebra. Default auto: dense below 100
                                 unknowns, sparse from 100. Every analysis
                                 (.op, .dc, .tran, .ac, .noise) uses it.
         --threads <n>           Threads evaluating device instances. Default: one per
                                 logical core (or RAYON_NUM_THREADS). Results are
                                 identical at every thread count; only speed changes.
                                 VA_PARALLEL=auto|always|never overrides when the
                                 parallel path is used (default auto: by measured cost).
                                 VA_BTF=off solves DC and transient steps with faer's
                                 sparse LU alone instead of trying the block-triangular
                                 solver first.
         --reltol <x>            Transient step control's relative truncation-error
                                 tolerance. Default 1e-3. Looser = fewer timepoints,
                                 and results NOT re-validated against golden.
         --abstol <x>            The same budget's absolute floor, in each unknown's
                                 own units (V, A, ...). Default 1e-6.
         --newton-reltol <x>     Each transient step's Newton convergence tolerance.
                                 Default 1e-9 (strict). Looser = fewer iterations;
                                 same caveat.
         --bypass                Device bypass in a transient run (SPICE's BYPASS): a
                                 device whose stamps depend only on its own unknowns
                                 reuses its last evaluation while they move less than
                                 the tolerance. Off by default. Faster on large
                                 transistor circuits, slower on cheap ones; results are
                                 NOT bit-identical and not validated against golden.
                                 Every bypassed run is marked ***** BYPASS [ACTIVE] *****
                                 on stdout and stderr. Transient only.
         --bypass-reltol <x>     Relative drift before a bypassed device is evaluated
                                 again. Default 1e-6 (absolute: 1e-3 x the unknown's
                                 nature abstol, e.g. 1e-9 V). Implies --bypass.
         --tasks-once            Print each distinct $strobe/$display/$warning/...
                                 line once (by its text: per instance only if the
                                 model prints %m). The
                                 LRM prints a task every time it runs; this is for
                                 models whose parameter checks repeat at every point.
                                 A closing line counts what was held back.
         --logfull               Trace every DC Newton iteration on stderr (lines
                                 start `[logfull]`): assembly vs linear-solve time,
                                 line-search time, residual, largest step; and per
                                 gmin stage the iteration count and totals, with
                                 counts of hash lookups, per-call map builds and
                                 probe allocations, and whole-run totals. For
                                 debugging convergence and cost; not the transient
                                 integrator's per-timestep iterations."
    );
}

/// The `check` subcommand: run the frontend over models/directories and report what fails.
/// `--codegen` extends each file's verdict past elaboration into `va-codegen`, so the same
/// command reports either the frontend or the frontend+codegen corpus figure.
fn cmd_check(args: &[String]) -> Result<()> {
    let codegen = args.iter().any(|a| a == "--codegen");
    let paths: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    if paths.is_empty() {
        bail!("expected at least one model file or directory");
    }
    check_models(&paths, codegen)
}

/// The `sim` subcommand: run a netlist through the pipeline.
fn cmd_sim(args: &[String]) -> Result<()> {
    let netlist = args.first().context("expected a netlist path")?;
    // `--threads <n>`: how many threads evaluate device instances (`va_core::par`). Without it,
    // rayon's default — `RAYON_NUM_THREADS`, else one per logical core. Speed only: every
    // thread count gives the same bits.
    // `--reltol <x>` / `--abstol <x>` / `--newton-reltol <x>`: the transient tolerances (`va_cli::set_tran_tolerances`).
    // Defaults are what every golden was validated with; looser is faster and unvalidated.
    let tol = |flag: &str, default: f64| -> Result<f64> {
        match parse_flag(args, flag) {
            None => Ok(default),
            Some(v) => v
                .parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x > 0.0)
                .with_context(|| format!("{flag} expects a positive number, got `{v}`")),
        }
    };
    va_cli::set_tran_tolerances(
        tol("--reltol", va_cli::TRAN_LTE_RELTOL_DEFAULT)?,
        tol("--abstol", va_cli::TRAN_LTE_ABSTOL_DEFAULT)?,
        tol(
            "--newton-reltol",
            va_transient::integrator::NEWTON_RELTOL_DEFAULT,
        )?,
    );
    // `--bypass` / `--bypass-reltol <x>`: device bypass (`va_cli::set_bypass`), transient only.
    let bypass_reltol = parse_flag(args, "--bypass-reltol");
    if bypass_reltol.is_some() || args.iter().any(|a| a == "--bypass") {
        let mut cfg = va_transient::integrator::BypassConfig::default();
        if bypass_reltol.is_some() {
            cfg.reltol = tol(
                "--bypass-reltol",
                va_transient::integrator::BYPASS_RELTOL_DEFAULT,
            )?;
        }
        if !args.iter().any(|a| a == "--tran") {
            eprintln!(
                "[va-cli] note: --bypass applies to a transient run (--tran) only; this analysis runs without it."
            );
        }
        va_cli::set_bypass(Some(cfg));
    }
    if let Some(n) = parse_flag(args, "--threads") {
        let n: usize = n
            .parse()
            .ok()
            .filter(|&n| n > 0)
            .with_context(|| format!("--threads expects a positive integer, got `{n}`"))?;
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .context("setting up the evaluation thread pool")?;
    }
    // `VA_PARALLEL=auto|always|never`: when instances are evaluated in parallel
    // (`va_core::par::Mode`). For verification — `always` makes a serial-vs-parallel
    // `xtask deck-diff` exercise the parallel path on every deck — not for tuning.
    if let Some(v) = std::env::var_os("VA_PARALLEL") {
        let mode = match v.to_string_lossy().as_ref() {
            "auto" => va_core::par::Mode::Auto,
            "always" => va_core::par::Mode::Always,
            "never" => va_core::par::Mode::Never,
            other => bail!("VA_PARALLEL expects auto, always or never, got `{other}`"),
        };
        va_core::par::set_mode(mode);
    }
    // `VA_BTF=on|off`: whether DC solves try the block-triangular path before `faer`
    // (`va_cli::set_btf`). On by default; `off` is the `faer`-only path, for comparison.
    if let Some(v) = std::env::var_os("VA_BTF") {
        match v.to_string_lossy().as_ref() {
            "on" => va_cli::set_btf(true),
            "off" => va_cli::set_btf(false),
            other => bail!("VA_BTF expects on or off, got `{other}`"),
        }
    }
    // `--model` is optional: built-in primitives (R/C/D/V) are satisfied by the reference
    // models, so a Verilog-A model is only needed for custom devices.
    // `--model` may be repeated: every library given is compiled as one unit, each file
    // resolving its own `` `include ``s against its own library (a technology's transistor
    // model beside a circuit's behavioural blocks).
    let models: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].clone())
        .collect();
    let model =
        (!models.is_empty()).then(|| models.join(&va_cli::MODEL_PATH_SEPARATOR.to_string()));
    let plot = parse_flag(args, "--plot");
    let analysis = if args.iter().any(|a| a == "--tran") {
        Analysis::Transient
    } else if args.iter().any(|a| a == "--ac") {
        Analysis::Ac
    } else if args.iter().any(|a| a == "--noise") {
        Analysis::Noise
    } else {
        Analysis::Dc
    };

    eprintln!(
        "[va-cli] sim netlist={netlist} model={} analysis={analysis:?}",
        model.as_deref().unwrap_or("<none>")
    );
    // `--integration` selects the integrator's method; generated models are method-independent
    // -- see `va_cli::Integration`.
    let integration = match parse_flag(args, "--integration") {
        None => va_cli::Integration::default(),
        Some(v) => va_cli::Integration::parse(&v).with_context(|| {
            format!("unknown --integration `{v}` (expected `be`, `trap`, or `gear`)")
        })?,
    };

    // `--report a,b,...` narrows the printed quantities. Without it every quantity the
    // circuit computes is reported -- node potentials in their own discipline's access
    // function and units, device branch currents, and a compiled model's internal unknowns.
    let report_only: Vec<String> = parse_flag(args, "--report")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // `--solver` chooses the linear algebra (docs/proposals/sparse-solve.md). `auto`, the
    // default, is dense below 100 unknowns and sparse from there.
    let solver = match parse_flag(args, "--solver").as_deref() {
        None | Some("auto") => Solver::Auto,
        Some("dense") => Solver::Dense,
        Some("sparse") => Solver::Sparse,
        Some(v) => bail!("unknown --solver `{v}` (expected `auto`, `dense`, or `sparse`)"),
    };

    // `--logfull` switches on the per-iteration Newton trace for every DC solve this run does.
    va_cli::set_log_full(args.iter().any(|a| a == "--logfull"));
    // A model's `$strobe`/`$display`/... print on this command's output (`va_cli::tasks`);
    // `--tasks-once` prints each distinct line once (`va_cli::set_tasks_once`).
    va_cli::set_task_output(true);
    va_cli::set_tasks_once(args.iter().any(|a| a == "--tasks-once"));

    let result = run_sim(
        netlist,
        model.as_deref(),
        analysis,
        plot.as_deref(),
        integration,
        &report_only,
        solver,
    );
    // Printed whether the run succeeded or not: a failed solve's counts are the ones most
    // worth having.
    if let Some(totals) = va_cli::log_full_run_totals() {
        eprintln!("{totals}");
    }
    // Whatever the outcome: a model's files hold what it wrote up to here.
    va_cli::close_task_files();
    // Said on both streams, like the lines it stands for, so neither a report nor a log can
    // be read as if the model printed only what is there.
    let suppressed = va_cli::suppressed_task_lines();
    if suppressed > 0 {
        let note =
            format!("[va-cli] --tasks-once: {suppressed} repeated system-task line(s) not printed");
        println!("{note}");
        eprintln!("{note}");
    }
    // `$error` is a run-time error (LRM §9.7.3): the results are printed, and the exit status
    // says the run was not clean.
    let errors = va_cli::task_errors();
    if result.is_ok() && errors > 0 {
        bail!("{errors} $error message(s) reported by the model(s) during the run");
    }
    result
}

/// Pull the value following `flag` out of `args`.
fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}
