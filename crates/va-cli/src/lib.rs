//! T6 — `va-cli` library: the pipeline wiring, exposed so `va-harness` can drive it.
//!
//! The binary (`main.rs`) is a thin argument-parsing wrapper over [`run_sim`]. Keeping the
//! pipeline in a library lets the validation harness call it directly instead of shelling
//! out to the executable.
//!
//! # What v0 wires
//!
//! `va-netlist` parses the deck; each device becomes a [`va_abi::ModelInstance`]; `va-core`
//! solves the DC operating point, or `va-transient` integrates a `.tran` deck. A
//! `--model <m.va>` is compiled through the real `va-frontend` → `va-codegen` pipeline and
//! used for every device whose model name matches the compiled module (e.g. `resistor`
//! devices against `resistor.va`), with the device's scalar value overriding the model's
//! first parameter. Devices with no matching compiled model fall back to the hand-written
//! reference primitives in `va-abi`.
//!
//! DC (`.op`/`.dc`), transient (`.tran <tstep> <tstop>`), small-signal AC
//! (`.ac dec <points-per-decade> <fstart> <fstop>`, T5) and noise
//! (`.noise V(<out>) <src> dec …`, T5.2, via [`solve_noise`]) are all implemented.
//! Transient starts from the zero vector except where an element carries a SPICE `IC=`;
//! there is no `.ic` card or circuit-wide `UIC`
//! support. A `V` source with a bare `DC <value>` combined with that cold start *is* the step
//! response — the only shape a constant source could produce. A `V` source with a `SIN(...)`
//! waveform is genuinely time-varying, and becomes a [`WaveformSource`]: an ordinary
//! `ModelInstance` that reads the current time off `va_abi::ModelInstance::load`'s analysis
//! context. Until that context existed (§6 change, 2026-08-06) it could not, and the source had
//! to be re-boxed at every step attempt through a parallel copy of the integrator; that copy is
//! gone and every device now takes the same path.

#![forbid(unsafe_code)]

pub mod plot;

use anyhow::{bail, Context, Result};
use std::f64::consts::PI;
use va_abi::reference::{
    diode::VT_NOMINAL, Bjt, Capacitor, Cccs, Ccvs, Diode, Inductor, Mutual, Resistor, VSource,
    Vccs, Vcvs,
};
use va_abi::ModelInstance;
use va_core::dc::operating_point;
use va_core::newton::NewtonConfig;
use va_ir::{Module, NodeId};
use va_netlist::{AnalysisCard, Device, Netlist};
use va_transient::integrator::{LteEstimator, Method, TranConfig, Waveform};

/// Which analysis to run for a `sim` invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Analysis {
    /// DC operating point / sweep (the default bring-up analysis).
    #[default]
    Dc,
    /// Transient analysis.
    Transient,
    /// AC small-signal analysis.
    Ac,
    /// Small-signal noise analysis (T5.2).
    Noise,
}

/// Analog operators this engine **approximates in a transient run**, paired with what it
/// actually computes instead. Enforced by [`refuse_transient_approximations`].
///
/// Deliberately not a list of everything unimplemented: these are the constructs that
/// produce a *plausible number that is wrong* rather than an error. `transition` and
/// `slew` are absent because they are genuinely evaluated against Interface beta's state
/// channel; the Z-domain family is absent because elaboration rejects it outright, which
/// is already loud. The `laplace_*` family left this table on 2026-09-11 (v0.9.16): a
/// rational filter is now integrated as an ODE on its own state unknowns
/// (`va_codegen::lower::LaplaceStates`), so a transient run computes the filter rather than
/// its DC gain. `absdelay` remains: a pure delay is not an ODE, and its time-domain form
/// (`docs/proposals/absdelay.md` stage 2) is still unimplemented.
const TRANSIENT_APPROXIMATIONS: &[(&str, &str)] = &[(
    "absdelay",
    "folds to its undelayed input (a true delay needs an interpolated history buffer)",
)];

/// Warn, on stderr, when `src` uses an operator this engine approximates in transient.
///
/// Lexes rather than string-searches, so a mention inside a comment or inside a longer
/// identifier does not trigger it — every name here is a reserved word, so it arrives as a
/// `Keyword` token. (`absdelay` only became reserved on 2026-08-31; before that this check
/// would have had to match raw identifiers and would have been the weaker for it.)
///
/// **An error, not a warning** (changed 2026-09-06). The fold is *correct* for DC and AC, where
/// these operators settle to their steady-state value, so those analyses are untouched and stay
/// perfectly sound. A transient run is the case where the fold is simply wrong, and a warning
/// on stderr is not enough: it scrolls past, it does not survive being piped into a file
/// alongside the waveform, and the numbers that follow it look exactly like numbers that were
/// computed. `docs/proposals/absdelay.md` states the principle for this operator family
/// directly - "an error naming the limit, never a quietly wrong waveform".
///
/// The real fix is that proposal's stage 2 (a ring buffer of `(t, value)` on the state channel,
/// with an interpolation weight carried through AD); until it lands, refusing is the honest
/// half of implement-or-refuse.
///
/// # Errors
///
/// If any model source calls an operator in [`TRANSIENT_APPROXIMATIONS`].
fn refuse_transient_approximations(models: &[(String, String)]) -> Result<()> {
    // Every offender, not just the first: a model library hitting two of these should say so
    // once rather than make the reader fix them one run at a time.
    let mut found: Vec<String> = Vec::new();
    for (path, src) in models {
        for name in approximations_in(src) {
            let effect = TRANSIENT_APPROXIMATIONS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, e)| *e)
                .unwrap_or("is approximated");
            found.push(format!("{path} uses `{name}`, which {effect}"));
        }
        for name in step_triggers_in(src) {
            found.push(format!(
                "{path} uses `@({name})`, whose body this engine runs at every timepoint                  instead of only when the event fires"
            ));
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    // Raised as a `Refusal` rather than a bare `bail!`, so this - the one refusal that is about
    // a whole model in a given analysis rather than about a construct the frontend can point at
    // - reaches the user in the same labelled, greppable shape as every other (§ `report_refusal`).
    Err(anyhow::Error::new(
        va_frontend::Refusal::new(
            found.join("\n"),
            "a `.tran` result would be a plausible waveform that is simply wrong, so it is \
             refused rather than printed",
        )
        .instead(
            "run this model as DC (`.op`/`.dc`), AC (`--ac`) or noise (`--noise`), which are \
             unaffected: these constructs are correct in a static solve - an operator settles \
             to exactly this value, and a single solve point is both the first and the last step",
        )
        .tracking("docs/proposals/absdelay.md stage 2"),
    ))
}

/// Step-scoped `@(...)` triggers `src` uses, in table order, each named once.
///
/// These are the triggers `va-frontend` deliberately still discards, running the body
/// unconditionally: the simulator-specific `initial_instance`/`initial_model`. That treatment
/// is **correct in a static solve** — one solve point is both the first and the last step, and
/// setup does run once — and wrong only in transient, where the body re-runs at every
/// timepoint. Hence an analysis-gated refusal here rather than a parse error in the frontend,
/// which is analysis-agnostic. (A *monitored* trigger — `cross`/`above`/`timer`/`absdelta` —
/// never fires in a static solve either, so the frontend rejects those outright and they never
/// reach this check.)
///
/// **`final_step` left this list on 2026-09-07**, when it stopped being discarded: it now
/// desugars to `va_ir::Builtin::FinalStep`, which reads `AnalysisCtx::is_final_step` and is
/// therefore `false` at every transient timepoint except the last. There is nothing left for an
/// analysis-gated refusal to protect against.
///
/// Matches `@` followed by `(` followed by the name, so a bare mention of a trigger word in an
/// expression or a comment does not trigger it. `initial_instance`/`initial_model` are not
/// reserved words and arrive as `Ident`; a reserved one would arrive as a `Keyword`, which is
/// why both token kinds are still read.
fn step_triggers_in(src: &str) -> Vec<&'static str> {
    const STEP_TRIGGERS: [&str; 2] = ["initial_instance", "initial_model"];
    let Ok(tokens) = va_frontend::lexer::lex(src) else {
        return Vec::new(); // A model that does not lex will fail louder elsewhere.
    };
    let mut seen: Vec<&'static str> = Vec::new();
    for w in tokens.windows(3) {
        if !matches!(w[0], va_frontend::lexer::Token::At)
            || !matches!(w[1], va_frontend::lexer::Token::LParen)
        {
            continue;
        }
        let name = match &w[2] {
            va_frontend::lexer::Token::Keyword(kw) => kw.as_str(),
            va_frontend::lexer::Token::Ident(id) => id.as_str(),
            _ => continue,
        };
        if let Some(found) = STEP_TRIGGERS.iter().find(|t| **t == name) {
            if !seen.contains(found) {
                seen.push(found);
            }
        }
    }
    seen
}

/// Every model source `sim` actually compiled, as `(path, source)`.
///
/// Mirrors [`compile_model_path`]'s file-or-directory handling rather than re-deriving it: the
/// previous check read only `model` itself with `std::fs::read_to_string`, so pointing
/// `--model` at a *directory* - the documented way to use a real model library - silently
/// skipped the check entirely, which is precisely the case a photonic library would have hit.
fn model_sources(model: Option<&str>) -> Vec<(String, String)> {
    let Some(path) = model else {
        return Vec::new();
    };
    let p = std::path::Path::new(path);
    let files: Vec<std::path::PathBuf> = if p.is_dir() {
        let Ok(rd) = std::fs::read_dir(p) else {
            return Vec::new();
        };
        rd.filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| {
                matches!(
                    f.extension().and_then(|e| e.to_str()),
                    Some("va") | Some("vams")
                )
            })
            .collect()
    } else {
        vec![p.to_path_buf()]
    };
    files
        .into_iter()
        .filter_map(|f| {
            std::fs::read_to_string(&f)
                .ok()
                .map(|src| (f.display().to_string(), src))
        })
        .collect()
}

/// The [`TRANSIENT_APPROXIMATIONS`] operators `src` actually calls, in table order, each named
/// once. Split out from the warning so the detection is testable without capturing stderr.
fn approximations_in(src: &str) -> Vec<&'static str> {
    let Ok(tokens) = va_frontend::lexer::lex(src) else {
        return Vec::new(); // A model that does not lex will fail louder elsewhere.
    };
    let mut seen: Vec<&'static str> = Vec::new();
    for tok in &tokens {
        if let va_frontend::lexer::Token::Keyword(kw) = tok {
            if let Some((name, _)) = TRANSIENT_APPROXIMATIONS
                .iter()
                .find(|(n, _)| *n == kw.as_str())
            {
                if !seen.contains(name) {
                    seen.push(name);
                }
            }
        }
    }
    seen
}
/// Parse `netlist` and, if `model` is given, compile it through the real frontend → codegen
/// pipeline — the common prelude every driver needs before solving. Split out of [`run_sim`]
/// (which still calls it, unchanged) so a caller that wants the *values* — `va-harness`
/// comparing against golden, not a human reading stdout — doesn't have to re-implement this
/// wiring or shell out to the CLI binary and re-parse its printed output.
///
/// # Errors
///
/// If the netlist or model file cannot be read, or either fails to parse/compile.
pub fn load(netlist: &str, model: Option<&str>) -> Result<(Netlist, Vec<Module>)> {
    let deck =
        std::fs::read_to_string(netlist).with_context(|| format!("reading netlist {netlist}"))?;
    let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {netlist}"))?;

    let compiled = match model {
        Some(path) => compile_model_path(path)?,
        None => Vec::new(),
    };

    Ok((net, compiled))
}

/// Compile `path` into a module library: a single `.va` file, or a **directory** of them.
///
/// A directory is compiled as one unit, which is what makes a real model library usable. The
/// photonic corpus is the motivating case: it is one module per file across 30-odd files, so
/// `Waveguide.va` alone fails with "instance `Polar2Cartesian1` references unknown module" -- it
/// is not self-contained and was never meant to be. Every file's modules are parsed first and
/// then elaborated against the *combined* set, exactly as `check` already does for the corpus
/// scan, so an instance naming a sibling file's module resolves.
///
/// Each file's own `` `include `` resolves against the directory, so a shared
/// `disciplines.vams` defining a custom discipline (the photonic library's `optical`) is found
/// without the caller naming it.
///
/// # Errors
///
/// If a file cannot be read, or any module fails to parse or elaborate. A directory containing
/// a file that is not a model is an error rather than a skip: `sim` is being told *this is the
/// library*, unlike `check`, whose whole job is to survey files of unknown quality.
fn compile_model_path(path: &str) -> Result<Vec<Module>> {
    compile_model_library(path).map(|(modules, _)| modules)
}

/// One source file of a compiled model library: which modules it declares and which it
/// instantiates. This is what lets a per-file check ([`refuse_transient_approximations`]) be
/// applied to the files a deck *uses* rather than to everything the library happens to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LibraryFile {
    /// The path as `compile_model_library` printed it — the same spelling
    /// [`model_sources`] produces, so the two can be matched by string.
    path: String,
    /// Names of the modules declared in this file.
    modules: Vec<String>,
    /// Names of every module instantiated (`Item::Instance`) by any module in this file.
    instantiates: Vec<String>,
}

/// The library files a deck reaches: those declaring a module some device places, plus —
/// transitively — those declaring a module one of *those* instantiates. Elaboration inlines
/// submodules, so the flattened IR no longer says which files fed a placed module; the AST-level
/// instance graph recorded in [`LibraryFile`] does.
///
/// A device whose model is a built-in primitive (`resistor`, `vsource`, …) or unknown reaches
/// nothing here; unknown models are reported when the device is built, not by this walk.
fn files_reached_by(net: &Netlist, files: &[LibraryFile]) -> Vec<String> {
    let mut wanted: Vec<String> = net.devices.iter().map(|d| d.model.clone()).collect();
    let mut reached: Vec<String> = Vec::new();
    let mut i = 0;
    while i < wanted.len() {
        let name = wanted[i].clone();
        i += 1;
        for f in files.iter().filter(|f| f.modules.contains(&name)) {
            if !reached.contains(&f.path) {
                reached.push(f.path.clone());
                for inst in &f.instantiates {
                    if !wanted.contains(inst) {
                        wanted.push(inst.clone());
                    }
                }
            }
        }
    }
    reached
}

/// [`compile_model_path`], also returning which modules each file declared and instantiated.
fn compile_model_library(path: &str) -> Result<(Vec<Module>, Vec<LibraryFile>)> {
    let p = std::path::Path::new(path);
    let files: Vec<std::path::PathBuf> = if p.is_dir() {
        let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(p)
            .with_context(|| format!("reading model directory {path}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| {
                matches!(
                    f.extension().and_then(|e| e.to_str()),
                    Some("va") | Some("vams")
                )
            })
            .collect();
        // Sorted so a library compiles identically on every platform and every run: the
        // elaborated module order reaches `build_instance` and, through it, unknown indices.
        v.sort();
        v
    } else {
        vec![p.to_path_buf()]
    };
    if files.is_empty() {
        bail!("no `.va`/`.vams` model files found in {path}");
    }

    let include_dirs: Vec<std::path::PathBuf> = if p.is_dir() {
        vec![p.to_path_buf()]
    } else {
        p.parent()
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default()
    };

    // Pass 1: parse every file, accumulating modules and the discipline/nature tables their
    // own includes brought in. Later files do not override earlier definitions -- a library
    // whose files disagree about a discipline is a problem to report, not to silently resolve.
    let mut library: Vec<va_frontend::ast::ModuleAst> = Vec::new();
    let mut library_files: Vec<LibraryFile> = Vec::new();
    let mut disciplines = std::collections::HashMap::new();
    let mut natures = std::collections::HashMap::new();
    for f in &files {
        let name = f.display().to_string();
        let src = std::fs::read_to_string(f).with_context(|| format!("reading model {name}"))?;
        let expanded = va_frontend::preprocess::preprocess(&src, &include_dirs)
            .with_context(|| format!("preprocessing {name}"))?;
        let (tokens, offsets) =
            va_frontend::lexer::lex_spanned(&expanded).with_context(|| format!("lexing {name}"))?;
        let (asts, file_natures, file_disciplines) =
            va_frontend::parser::parse_with_disciplines_located(
                &tokens,
                Some((&expanded, &offsets)),
            )
            .with_context(|| format!("parsing {name}"))?;
        for (k, v) in file_disciplines {
            disciplines.entry(k).or_insert(v);
        }
        for (k, v) in file_natures {
            natures.entry(k).or_insert(v);
        }
        library_files.push(LibraryFile {
            path: name,
            modules: asts.iter().map(|a| a.name.clone()).collect(),
            instantiates: asts
                .iter()
                .flat_map(|a| a.items.iter())
                .filter_map(|it| match it {
                    va_frontend::ast::Item::Instance { module, .. } => Some(module.clone()),
                    _ => None,
                })
                .collect(),
        });
        library.extend(asts);
    }

    // Pass 2: elaborate each module against every module in the library.
    let mut modules = Vec::with_capacity(library.len());
    for ast in &library {
        let m = va_frontend::elaborate::elaborate_with_library_and_disciplines(
            ast,
            &library,
            &disciplines,
            &natures,
        )
        .with_context(|| format!("elaborating module `{}`", ast.name))?;
        modules.push(m);
    }
    eprintln!(
        "[va-cli] compiled {} Verilog-A module(s) from {path}",
        modules.len()
    );
    warn_unplaceable_modules(&modules, path);
    warn_mfactor_double_scaling(&modules, path);
    Ok((modules, library_files))
}

/// Print a refusal's labelled fields under a `check` verdict line, on stdout with the rest of
/// the listing, skipping the `refused:` line the verdict already carries.
///
/// Shared by every stage of the listing that can produce one, so `check`'s output does not
/// explain a frontend refusal and leave a codegen refusal as a bare sentence.
fn print_refusal_detail(err: &anyhow::Error) {
    if let Some(block) = refusal_block(err) {
        for line in block.lines().skip(1) {
            println!("        {line}");
        }
    }
}

/// The one-line verdict for `e` in a `check` listing: a refusal's marker line alone, or the
/// whole error otherwise.
///
/// A refusal's full block is printed separately by [`report_refusal`]; repeating all five of its
/// lines inside the per-file verdict would bury the listing that makes `check` readable over a
/// 130-file corpus.
fn refusal_headline(e: &va_frontend::FrontendError) -> String {
    match e {
        va_frontend::FrontendError::Refused(r) => {
            format!("refused: {}", r.what)
        }
        other => other.to_string(),
    }
}

/// Print every refusal carried in `err`'s cause chain, and say whether one was found.
///
/// A **refusal** is this project's name for a construct the implementation recognises and
/// deliberately declines to support, as opposed to malformed input or a numerical failure. The
/// distinction is the first thing someone debugging a failed run needs — "my model is wrong"
/// and "this simulator will not run my correct model" call for completely different next steps —
/// and before this existed the two were indistinguishable in the output: a frontend refusal
/// arrived labelled `parse error` with a debug-formatted token appended, reading exactly like a
/// syntax error in a construct the user had spelled correctly.
///
/// Every refusal therefore reaches the user in **one shape, from whichever layer raised it**,
/// opening with a greppable `refused:` marker and carrying what / where / why / instead /
/// tracking. `va-frontend` supplies those as fields ([`va_frontend::Refusal`]);
/// [`va_codegen::CodegenError::Unsupported`] states its reason inline in the message instead, so
/// its `why` line says where to read it rather than inventing a second one.
///
/// Returns `false` if `err` is an ordinary error, which is the caller's cue to report it the
/// usual way — this function never prints anything for a non-refusal.
pub fn report_refusal(err: &anyhow::Error) -> bool {
    match refusal_block(err) {
        Some(block) => {
            for line in block.lines() {
                eprintln!("[va-cli] {line}");
            }
            true
        }
        None => false,
    }
}

/// The formatted refusal carried anywhere in `err`'s cause chain, or `None` for an ordinary
/// error — the shared half of [`report_refusal`], so a caller writing to stdout (`check`'s
/// per-file listing) emits the identical block rather than a second rendering of it.
///
/// Walks the whole chain, not just the head: `compile_model_path` wraps a refusal in a
/// `parsing <path>` context, and the refusal is what the reader needs to see.
pub fn refusal_block(err: &anyhow::Error) -> Option<String> {
    for cause in err.chain() {
        if let Some(va_frontend::FrontendError::Refused(r)) = cause.downcast_ref() {
            return Some(r.to_string());
        }
        // A `Refusal` raised directly, not wrapped in a `FrontendError` — see
        // `refuse_transient_approximations`.
        if let Some(r) = cause.downcast_ref::<va_frontend::Refusal>() {
            return Some(r.to_string());
        }
        if let Some(va_codegen::CodegenError::Unsupported(msg)) = cause.downcast_ref() {
            return Some(format!(
                "refused: {msg}\n  why:      stated above - `va-codegen` recognises this \
                 construct but cannot lower it into a differentiable model instance, and an \
                 instance with a wrong Jacobian would converge to a wrong answer rather than \
                 fail\n  tracking: docs/roadmap.md, and `docs/token-reference.md` for this \
                 construct's status"
            ));
        }
    }
    None
}

/// Warn about each module that scales a flow contribution by `$mfactor` itself — the
/// double-scaling misuse LRM §6.3.6 requires a simulator to report.
///
/// The simulator already multiplies every flow contribution by the multiplicity, so a model
/// doing it too gets `m²`. A warning rather than a refusal, because that is exactly what the LRM
/// asks for ("the simulator shall issue a warning"), and because the model is still perfectly
/// correct at the default `m = 1` — which is how every deck that never writes `m=` runs it.
///
/// The detection is deliberately narrow (`va_ir::Module::mfactor_scales_a_flow_contribution`):
/// it catches the LRM's own `badres` and leaves its `parares` alone. A model that launders
/// `$mfactor` through a variable first slips past, which costs a warning, not a wrong answer.
fn warn_mfactor_double_scaling(modules: &[va_ir::Module], path: &str) {
    for m in modules
        .iter()
        .filter(|m| m.mfactor_scales_a_flow_contribution())
    {
        eprintln!(
            "[va-cli] warning: {path}: module `{}` multiplies a flow contribution by \
             `$mfactor`, which the simulator already applies (LRM 6.3.6) - an instance with \
             `m=N` would be scaled by N twice.",
            m.name
        );
        eprintln!(
            "[va-cli]   Drop the explicit factor: `I(a,b) <+ V(a,b)/r;` already behaves as N \
             devices in parallel. `$mfactor` is for reading the multiplicity in a *condition* \
             (the LRM's `parares` example), not for scaling the output."
        );
        eprintln!(
            "[va-cli]   Harmless at the default `m=1`, which is how a deck that never writes \
             `m=` runs this model."
        );
    }
}

/// Warn about each compiled module that declares no ports, and so can never be placed by a
/// deck line (§ portless top-level modules).
///
/// In this simulator the *circuit* is always the `.net` deck; a `.va` file supplies component
/// definitions the deck places. Every device line connects at least one node — `va-netlist`
/// rejects an `X` line with none, and every other device letter has a fixed terminal count — so
/// a zero-port module is unplaceable by construction. `ports.is_empty()` is therefore not a
/// heuristic but a decision procedure, which is why nothing else (instances present, no analog
/// block, a `ground` declaration) is consulted: each of those also describes legitimate
/// components, e.g. `models/series_divider.va`, a two-port model built from two instances and
/// with no analog block of its own.
///
/// A warning rather than an error, for the same reason as
/// [`refuse_transient_approximations`]: a file may legitimately hold a circuit module *alongside*
/// the components it instantiates — `external/basic/circuit1.va` does exactly that, and its
/// `vsrc`/`resistor` are perfectly placeable. What is not acceptable is the current silence, in
/// which the module is compiled, never placed, and never mentioned, so the run exits 0 with a
/// confident answer computed entirely without it.
fn warn_unplaceable_modules(modules: &[va_ir::Module], path: &str) {
    for m in modules.iter().filter(|m| m.ports.is_empty()) {
        eprintln!(
            "[va-cli] warning: {path}: module `{}` declares no ports, so no deck line can \
             place it — it was compiled and then ignored.",
            m.name
        );
        eprintln!(
            "[va-cli]   In this simulator the circuit is the `.net` deck; a `.va` file supplies \
             components the deck places, and every device line connects at least one node \
             (`X<name> <node>... <model> [param=value]`)."
        );
        eprintln!(
            "[va-cli]   To simulate `{}`, keep its component modules in `.va` files and rewrite \
             its instance lines as deck lines: `vsrc #(.dc(1)) V1(n, gnd);` becomes \
             `X1 n gnd vsrc dc=1`. See `circuits/divider.net`.",
            m.name
        );
    }
}

/// Run the full pipeline for `netlist` + an optional Verilog-A `model` under `analysis`.
///
/// Wires `va-frontend` → `va-codegen` → `va-netlist` → `va-core`/`va-transient`. Prints the DC
/// operating point (node voltages and source currents), or the transient waveform, to stdout.
///
/// # Errors
///
/// Returns an error if a file cannot be read, the deck or model cannot be parsed, an
/// unsupported analysis is requested, a device names an unknown model, or the solve diverges.
/// If `plot` is given, also returns an error when the requested analysis produces no curve
/// to draw — a bare DC operating point is a single point, not a waveform — or if writing the
/// SVG fails.
pub fn run_sim(
    netlist: &str,
    model: Option<&str>,
    analysis: Analysis,
    plot: Option<&str>,
    integration: Integration,
    report_only: &[String],
) -> Result<()> {
    let deck =
        std::fs::read_to_string(netlist).with_context(|| format!("reading netlist {netlist}"))?;
    let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {netlist}"))?;
    let (compiled, library_files) = match model {
        Some(path) => compile_model_library(path)?,
        None => (Vec::new(), Vec::new()),
    };

    gate_analysis(&net, analysis)?;
    // Plottable analyses are the ones that produce a *curve*: a transient waveform, or a `.dc`
    // sweep. A bare DC operating point is a single point — plotting one would be an empty
    // image, so asking is still a clear error rather than a misleading file.
    if plot.is_some()
        && analysis != Analysis::Transient
        && analysis != Analysis::Ac
        && analysis != Analysis::Noise
        && !(analysis == Analysis::Dc && net.dc.is_some())
    {
        bail!(
            "--plot supports a transient run (--tran), an AC sweep (--ac), a noise sweep \
             (--noise), or a `.dc` sweep; \
             a DC operating point is a single point, not a curve"
        );
    }

    if analysis == Analysis::Transient {
        // Checked *before* solving, so the refusal is not buried under a waveform -- and so no
        // waveform is produced at all. Checked over the files this deck *reaches*, not the
        // whole library: `--model models/` names a directory that also holds `delay_line.va`
        // and `laplace_lowpass.va`, and refusing a circuit that never places them would be a
        // refusal naming the wrong construct (2026-09-11, found by `microring_thermal.net`).
        let reached = files_reached_by(&net, &library_files);
        let sources: Vec<(String, String)> = model_sources(model)
            .into_iter()
            .filter(|(path, _)| reached.contains(path))
            .collect();
        refuse_transient_approximations(&sources)?;
        let wf = solve_transient(&net, &compiled, integration)?;
        // Said only when a request actually went unmet, rather than whenever a tolerance is
        // written: the bracketing step control (§ `cross`) normally honours it, and a blanket
        // warning would cry wolf on every model that asks for one.
        if wf.unresolved_events > 0 {
            eprintln!(
                "[va-cli] warning: {} crossing(s) fired without meeting the time_tol/expr_tol                  their `cross(...)` site asked for — the step control hit its retry limit or                  the minimum timestep. Loosen the tolerance, or lower `.tran`'s step.",
                wf.unresolved_events
            );
        }
        let shown = select_quantities(&quantities(&net, &compiled)?, report_only)?;
        report_transient(&shown, &wf);
        if let Some(path) = plot {
            // The plot shows exactly what the report shows, `--report` included: a chart and a
            // table of the same run disagreeing about which series exist would be worse than
            // either alone. It is also the practical way to plot a cross-domain result, whose
            // series differ by orders of magnitude and share no axis.
            plot::plot_transient(path, &shown, &wf)
                .with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote transient plot to {path}");
        }
    } else if analysis == Analysis::Ac {
        let response = solve_ac(&net, &compiled)?;
        let shown = select_quantities(&quantities(&net, &compiled)?, report_only)?;
        report_ac(&shown, &response);
        if let Some(path) = plot {
            plot::plot_ac(path, &shown, &response)
                .with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote AC plot to {path}");
        }
    } else if analysis == Analysis::Noise {
        let spectrum = solve_noise(&net, &compiled)?;
        // Not `select_quantities`: a `.noise` run reports one output the card itself names,
        // so the table is consulted for that output's *units*, not to choose columns.
        report_noise(&net, &quantities(&net, &compiled)?, &spectrum);
        if let Some(path) = plot {
            plot::plot_noise(path, &spectrum).with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote noise plot to {path}");
        }
    } else if let Some(sweep) = &net.dc {
        let points = solve_dc_sweep(&net, &compiled, sweep)?;
        let shown = select_quantities(&quantities(&net, &compiled)?, report_only)?;
        report_sweep(&shown, sweep, &points);
        if let Some(path) = plot {
            plot::plot_sweep(path, &shown, sweep, &points)
                .with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote sweep plot to {path}");
        }
    } else {
        let op = solve_dc(&net, &compiled)?;
        report(
            &select_quantities(&quantities(&net, &compiled)?, report_only)?,
            &op.x,
        );
    }
    Ok(())
}

/// Run the frontend (lex → parse → elaborate) over each path and print a per-file report of
/// the first failing stage. `paths` may be individual files or directories (scanned for
/// `.va`/`.vams`). This is a diagnostic tool: it always returns `Ok`, reporting status to
/// stdout, and is how we discover which Verilog-A constructs the v0 frontend is missing.
///
/// With `codegen` set, every module that elaborates is additionally pushed through
/// [`va_codegen::build_instance`], so the run measures the **frontend + codegen** figure
/// (T2.2's corpus coverage) rather than the frontend one. That number was previously only
/// obtainable from a one-off hand-written scan, which is precisely how it went stale between
/// roadmap revisions; making it a flag on the same command keeps both figures re-derivable
/// from one command.
///
/// # Errors
///
/// Only if a directory cannot be read.
pub fn check_models(paths: &[String], codegen: bool) -> Result<()> {
    // Each entry pairs a file with the root directory it was scanned from, so nested
    // library folders (e.g. `external/some-lib/`) can still resolve `` `include `` of
    // shared headers (`constants.vams`, `disciplines.vams`) that live at the scanned root.
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for p in paths {
        let path = std::path::Path::new(p);
        if path.is_dir() {
            collect_va_files(path, path, &mut files)
                .with_context(|| format!("scanning directory {p}"))?;
        } else {
            files.push((p.clone(), std::path::PathBuf::new()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // Group by each file's own immediate parent directory: every module across every file in
    // the same directory is elaborated against one combined library, so an `Item::Instance`
    // naming a module declared in a sibling file resolves (§ module instantiation) — matching
    // how a real Verilog-A toolchain treats a whole library folder handed to it together (e.g.
    // `external/photonic/Attenuator.va` instantiating `Polar2Cartesian`, declared in the
    // sibling `Polar2Cartesian.va`). This is *not* extended to the top-level scan root itself
    // sharing one library across unrelated subfolders: several real corpus files at the same
    // nesting depth under `external/` (e.g. two different `hisimsoi_va` releases) declare a
    // module with the same name, so a directory-wide-not-just-folder-wide merge would risk
    // silently resolving an instantiation against the wrong same-named module. Grouping by
    // immediate parent directory only merges files a human actually put together in one
    // folder, which is the one case with an established intent to be used as one library.
    let mut groups: std::collections::BTreeMap<
        std::path::PathBuf,
        Vec<(String, std::path::PathBuf)>,
    > = std::collections::BTreeMap::new();
    for (file, root) in files {
        let parent = std::path::Path::new(&file)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        groups.entry(parent).or_default().push((file, root));
    }

    let mut tally = CheckTally::default();
    let mut total = 0usize;
    for group in groups.into_values() {
        total += group.len();
        tally += check_group(&group, codegen);
    }
    let stages = if codegen {
        "the frontend + codegen (lex → parse → elaborate → build_instance)"
    } else {
        "the frontend (lex → parse → elaborate)"
    };
    let with_module = tally.passed + tally.failed;
    println!(
        "\n{}/{with_module} files declaring a module passed {stages}",
        tally.passed
    );
    println!(
        "  {} further file(s) declare no module at all (macro/nature headers, statement \
         fragments) — not checkable models, and no longer counted as passes",
        tally.no_module
    );
    println!(
        "  of the {} passes, {} are on an incomplete module (an unresolved `include was \
         dropped); {} are self-contained",
        tally.passed,
        tally.passed_incomplete,
        tally.passed - tally.passed_incomplete
    );
    let whole_passed = tally.passed - tally.passed_incomplete;
    let whole_total = whole_passed + (tally.failed - tally.failed_incomplete);
    println!(
        "  {} of the {} failures also dropped an unresolved `include (truncated distributions, not gaps)",
        tally.failed_incomplete, tally.failed
    );
    println!("  => self-contained files declaring a module: {whole_passed}/{whole_total}");
    // Separated from the headline for the same reason `passed_incomplete` is: building is not
    // the same as computing what the source says, and a number that merges the two overstates
    // what the engine actually supports.
    println!(
        "  of the {} passes, {} use an operator approximated in transient (absdelay)",
        tally.passed, tally.passed_approximated
    );
    println!("  {total} file(s) scanned in total");
    Ok(())
}

/// What one `check` run found, split so the headline number cannot silently absorb the three
/// things that are not "a model this frontend can handle".
///
/// This split exists because the single number it replaces was measuring something else. On the
/// 150-file corpus, `114/150 passed the frontend` counted **14 files that declare no module**
/// (they passed because the "did every module elaborate?" loop had nothing to iterate) and
/// **16 more whose entire module body had been deleted** by an unresolved `` `include ``,
/// including a `bsimcmg.va` reporting zero parameters. Those 16 differ from the ten files that
/// *fail* with "port has no discipline declaration" only in whether their ports happen to be
/// declared before the vanished include — one defect, two opposite verdicts. See
/// [`va_frontend::preprocess::preprocess_reporting`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CheckTally {
    /// Files declaring at least one module, every one of which elaborated (and, under
    /// `--codegen`, built).
    passed: usize,
    /// The subset of `passed` whose source was incomplete: at least one `` `include `` could
    /// not be resolved, so what elaborated is less than the file describes.
    passed_incomplete: usize,
    /// Files that declare (or may declare) at least one module, where something failed —
    /// whether at read/preprocess/lex/parse, or on a module that would not elaborate/build.
    failed: usize,
    /// The subset of `failed` that also dropped an unresolved `` `include ``. These are not
    /// frontend gaps — the ten "port `X` has no discipline declaration" corpus failures are
    /// exactly this: the declarations were in a body file the distribution never shipped.
    ///
    /// Counted on **both** failure paths. It used to be reachable only after a successful
    /// parse, because a file that died earlier had its skipped-include list discarded — which
    /// split one defect into two verdicts all over again: a truncated distribution whose absent
    /// `` `include `` broke elaboration was quarantined here, while one whose absent include
    /// broke *preprocessing* (the same file, one macro earlier) was scored as a frontend gap.
    /// `va_frontend::preprocess::preprocess_reporting` now reports skipped includes beside its
    /// error as well as inside its `Ok`, which is what makes this reachable for a `[pp]` line.
    failed_incomplete: usize,
    /// Files that parsed but declare no module — headers and statement-body fragments.
    no_module: usize,
    /// The subset of `passed` that uses an analog operator this engine **approximates rather
    /// than evaluates** (§ [`TRANSIENT_APPROXIMATIONS`]), and so builds into a `ModelInstance`
    /// that computes something other than what the source says in a transient run.
    ///
    /// This exists because "passed" measures *builds*, not *is right*. `absdelay` folds to its
    /// undelayed input and a `laplace_*` filter folds to its DC gain, so a file using either
    /// contributes to the headline number while silently returning the wrong waveform. That is
    /// the same class of overcounting the `passed_incomplete` split above was introduced to
    /// stop — a file scored as coverage for something it does not actually do.
    passed_approximated: usize,
}

impl std::ops::AddAssign for CheckTally {
    fn add_assign(&mut self, rhs: Self) {
        self.passed += rhs.passed;
        self.passed_incomplete += rhs.passed_incomplete;
        self.failed += rhs.failed;
        self.failed_incomplete += rhs.failed_incomplete;
        self.no_module += rhs.no_module;
        self.passed_approximated += rhs.passed_approximated;
    }
}

/// Collect `.va`/`.vams` files under `dir`, recursing into subdirectories so model libraries
/// kept in their own folder are included. Each file is paired with `root` (the top-level
/// directory the scan started from) so its includes can fall back to shared headers kept
/// there, in addition to the file's own directory.
fn collect_va_files(
    dir: &std::path::Path,
    root: &std::path::Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_va_files(&path, root, out)?;
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext == "va" || ext == "vams" {
                out.push((path.to_string_lossy().into_owned(), root.to_path_buf()));
            }
        }
    }
    Ok(())
}

/// What [`parse_file`] recovered from one source file.
struct ParsedFile {
    /// Every module the file's own text defines — possibly empty, which is itself a verdict
    /// (a macro/nature header or a statement-body fragment declares none).
    asts: Vec<va_frontend::ast::ModuleAst>,
    /// The file's parsed `nature`/`discipline` preamble. Kept, not discarded: elaboration needs
    /// the discipline table to resolve a net's `abstol` (§ nature-metadata wiring) and to decide
    /// whether two differently *named* disciplines are compatible at a port connection
    /// (§ port-connection discipline checking). Dropping it here used to make `check` elaborate
    /// with empty tables while `sim` (via `va_frontend::compile_with_includes`) elaborated with
    /// full ones — the same file could then pass one and fail the other.
    natures: std::collections::HashMap<String, va_frontend::disciplines::NatureDecl>,
    /// The file's parsed `discipline...enddiscipline` table; see `natures`.
    disciplines: std::collections::HashMap<String, va_frontend::disciplines::DisciplineDecl>,
    /// Every `` `include `` the preprocessor could not resolve and therefore dropped. A
    /// non-empty list means what parsed is **less than the file says it is**; see
    /// [`va_frontend::preprocess::preprocess_reporting`].
    skipped_includes: Vec<String>,
}

/// Note a module that declares no ports, as a trailing clause on its `[ok]` line
/// (§ portless top-level modules).
///
/// The module really did elaborate, so this is a *note* on a pass, never a new verdict, and it
/// is deliberately not folded into the headline figure: `check` measures "does the frontend
/// handle this file", and a portless structural circuit that elaborates is a genuine frontend
/// success. It is a different axis from `CheckTally::passed_approximated` — that one builds but
/// computes something approximate, this one computes correctly but cannot be reached from a
/// deck.
///
/// Suppressed when an `` `include `` was dropped: a truncated vendor distribution whose port
/// list lived in the absent header elaborates with zero ports too, and calling that a
/// self-contained circuit would point at the wrong thing — the same reason
/// [`skipped_clause`] exists.
fn unplaceable_clause(m: &va_ir::Module, skipped: &[String]) -> String {
    if !m.ports.is_empty() || !skipped.is_empty() {
        return String::new();
    }
    " — declares no ports, so no `.net` deck line can place it; this is a \
     self-contained circuit, not a component. Write it as a deck instead."
        .to_string()
}

/// Render a skipped-include list as a trailing clause for a status line, or `""` if none were
/// skipped. Attached to *failures* as well as passes: the ten corpus files that fail with
/// "port `D` has no discipline declaration" fail only because their whole module body lived in
/// an absent `` `include ``, and without this clause the message points at the wrong thing.
fn skipped_clause(skipped: &[String]) -> String {
    if skipped.is_empty() {
        return String::new();
    }
    format!(
        " [after skipping unresolved `include: {}]",
        skipped.join(", ")
    )
}

/// Run one source file through preprocess → lex → parse, printing a tagged status line on
/// failure. `scan_root` is the top-level directory the file was discovered under (empty if the
/// file was passed directly rather than found via directory scan) — used only to widen
/// `` `include `` resolution, unrelated to [`check_group`]'s cross-file *instantiation* library.
///
/// # Errors
///
/// `Err` means the file did not reach an AST — already reported on its own status line — and
/// carries **the unresolved `` `include ``s found before the failure**. That list is what lets
/// [`check_group`] tell a truncated distribution from a real frontend gap on the failure path,
/// exactly as `skipped_includes` does on the success path. It is empty when the file could not
/// be read at all, or when nothing had been skipped yet.
fn parse_file(path: &str, scan_root: &std::path::Path) -> Result<ParsedFile, Vec<String>> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("  [read ] {path}: {e}");
            return Err(Vec::new());
        }
    };
    // Resolve `include against the file's own directory first, then fall back to the
    // scanned root so nested library folders can still reach shared headers kept there.
    let own_dir = std::path::Path::new(path).parent();
    let mut include_dirs: Vec<std::path::PathBuf> =
        own_dir.map(|p| vec![p.to_path_buf()]).unwrap_or_default();
    if !scan_root.as_os_str().is_empty() && Some(scan_root) != own_dir {
        include_dirs.push(scan_root.to_path_buf());
    }
    let (result, skipped_includes) =
        va_frontend::preprocess::preprocess_reporting(&src, &include_dirs);
    let src = match result {
        Ok(src) => src,
        Err(e) => {
            // The clause matters most here: a vendor distribution whose body `` `include ``
            // never shipped usually fails on a macro that same file defined, so the bare error
            // names a symbol and implies a gap that does not exist.
            println!("  [pp   ] {path}: {e}{}", skipped_clause(&skipped_includes));
            return Err(skipped_includes);
        }
    };
    // Lex with spans so a parse failure below reports a line, a column, and the offending
    // line's text instead of a token index. The line is a line of the *preprocessed* source
    // (unresolved includes have already been dropped), which is why the quoted text matters
    // as much as the number here -- see va_frontend::parser::parse_with_disciplines_located.
    let (tokens, offsets) = match va_frontend::lexer::lex_spanned(&src) {
        Ok(t) => t,
        Err(e) => {
            println!("  [lex  ] {path}: {e}{}", skipped_clause(&skipped_includes));
            return Err(skipped_includes);
        }
    };
    match va_frontend::parser::parse_with_disciplines_located(&tokens, Some((&src, &offsets))) {
        Ok((asts, natures, disciplines)) => Ok(ParsedFile {
            asts,
            natures,
            disciplines,
            skipped_includes,
        }),
        Err(e) => {
            // A refusal gets its full labelled block as well as the one-line verdict, **on
            // stdout with the rest of the listing**: a scan over a model library is exactly the
            // case where "which constructs did this simulator decline, and why" has to survive
            // being redirected to a file, and splitting the verdict from its reason across two
            // streams is how that gets lost.
            println!(
                "  [parse] {path}: {}{}",
                refusal_headline(&e),
                skipped_clause(&skipped_includes)
            );
            // The block's own `refused: <what>` line is skipped -- the verdict above already
            // carries it, and repeating it pushes the `why` a line further from the file name
            // it belongs to.
            print_refusal_detail(&anyhow::Error::new(e));
            Err(skipped_includes)
        }
    }
}

/// Whether `path`'s source can be **proved** to declare no module, without preprocessing or
/// parsing it.
///
/// The proof has two halves, and needs both:
///
/// 1. The byte sequence `module` occurs nowhere in the raw source. Verilog-A's preprocessor has
///    no token-pasting operator (see `va_frontend::preprocess`'s limitations list), so the
///    keyword cannot be assembled from fragments — if those six bytes are absent from the file,
///    no amount of macro expansion can produce them.
/// 2. The file contains no `` `include ``. An included header *could* carry a `module`, and when
///    a file fails at the preprocess stage we cannot know what its includes would have expanded
///    to, so any file with one is left alone.
///
/// Together those make this a proof for the files it accepts rather than a guess, which is why
/// it is allowed to move a file out of the failure count. It is **deliberately one-directional
/// and deliberately crude**: any occurrence of `module` at all counts, including inside a
/// comment, a string, or the word `endmodule`. A wrong "declares no module" would hide a real
/// frontend gap; a wrong "might declare a module" only understates coverage. The asymmetry is
/// the point — when in doubt this returns `false` and the file stays a failing model.
///
/// An unreadable file returns `false` so that `parse_file` reports the read error itself.
fn cannot_declare_a_module(path: &str) -> bool {
    let Ok(src) = std::fs::read_to_string(path) else {
        return false;
    };
    !src.contains("module") && !src.contains("`include")
}

/// Check every file in one directory-grouped library together (§ module instantiation across
/// files, [`check_models`]): parse each file individually — still reporting its own
/// read/preprocess/lex/parse failure on its own line — then elaborate every module from every
/// successfully-parsed file against the *combined* list of all their modules, so an
/// `Item::Instance` naming a module declared in a sibling file resolves
/// (`elaborate_with_library`'s `library` argument doesn't care which file an entry came from).
/// Returns this group's [`CheckTally`]: how many files had every one of their own modules
/// elaborate cleanly (or, with `codegen` set, elaborate *and* build into a
/// [`va_abi::ModelInstance`]), split from the files that declare no module at all and the
/// passes whose module is incomplete because an `` `include `` went unresolved.
fn check_group(group: &[(String, std::path::PathBuf)], codegen: bool) -> CheckTally {
    let mut tally = CheckTally::default();
    let mut library: Vec<va_frontend::ast::ModuleAst> = Vec::new();
    // Merged the same way `library` is: a group is one directory's files elaborated together,
    // so a discipline declared in a shared `disciplines.va` header reaches every file that
    // `` `include ``s it, exactly as its modules do.
    let mut natures: std::collections::HashMap<String, va_frontend::disciplines::NatureDecl> =
        std::collections::HashMap::new();
    let mut disciplines: std::collections::HashMap<
        String,
        va_frontend::disciplines::DisciplineDecl,
    > = std::collections::HashMap::new();
    // Each successfully-parsed file's own modules, as a `library` index range — avoids cloning
    // every `ModuleAst` a second time just to report per-file status.
    let mut file_ranges: Vec<(&str, std::ops::Range<usize>, Vec<String>)> = Vec::new();
    for (file, root) in group {
        // Settle "is this a checkable model at all?" *before* preprocessing or parsing it.
        // Whether an include fragment's body happens to preprocess says nothing about this
        // frontend's coverage, and counting it as a failing model understates coverage exactly
        // as counting it as a pass used to overstate it (see `CheckTally`). This decides only
        // the cases it can *prove*; everything else still goes through the full pipeline.
        if cannot_declare_a_module(file) {
            println!("  [none ] {file}: declares no module (no `module` keyword in the source, and no `` `include `` to introduce one)");
            tally.no_module += 1;
            continue;
        }
        match parse_file(file, root) {
            Ok(parsed) => {
                let start = library.len();
                library.extend(parsed.asts);
                natures.extend(parsed.natures);
                disciplines.extend(parsed.disciplines);
                file_ranges.push((file.as_str(), start..library.len(), parsed.skipped_includes));
            }
            // `parse_file` already printed the reason. A failure that also dropped an
            // unresolved `` `include `` is a *truncated* file, not a gap — the same distinction
            // the post-elaboration path below draws, which until now could not be drawn here at
            // all because the skipped list was discarded on the error path.
            Err(skipped) => {
                tally.failed += 1;
                if !skipped.is_empty() {
                    tally.failed_incomplete += 1;
                }
            }
        }
    }

    for (file, range, skipped) in file_ranges {
        // A file that declares no module is not a checkable model — a macro/nature header or a
        // statement-body fragment meant to be `` `include ``d by something else. It used to be
        // counted as a pass simply because the "did every module elaborate?" loop had nothing
        // to iterate; see `CheckTally`.
        if range.is_empty() {
            println!(
                "  [none ] {file}: declares no module{}",
                skipped_clause(&skipped)
            );
            tally.no_module += 1;
            continue;
        }
        let mut all_ok = true;
        for ast in &library[range] {
            match va_frontend::elaborate::elaborate_with_library_and_disciplines(
                ast,
                &library,
                &disciplines,
                &natures,
            ) {
                Ok(m) => {
                    // Scanning a model library is the case where a double-scaling `$mfactor` is
                    // most worth hearing about, so `check` reports it too rather than leaving it
                    // to the run that eventually places the model (§ `warn_mfactor_double_scaling`).
                    warn_mfactor_double_scaling(std::slice::from_ref(&m), file);
                    // Every node gets its own global unknown, so codegen sees the same shape it
                    // would in a circuit where no terminal happens to be shared or grounded.
                    // `build_instance` allocates its own extra unknowns past `next_unknown`.
                    if codegen {
                        let terminals: Vec<usize> = (0..m.nodes.len()).collect();
                        let mut next_unknown = m.nodes.len();
                        if let Err(e) =
                            va_codegen::build_instance(&m, &terminals, &mut next_unknown)
                        {
                            println!(
                                "  [cgen ] {file}: module `{}`: {e}{}",
                                m.name,
                                skipped_clause(&skipped)
                            );
                            print_refusal_detail(&anyhow::Error::new(e));
                            all_ok = false;
                            continue;
                        }
                    }
                    println!(
                        "  [ok   ] {file}: module `{}` ({} ports, {} nodes, {} params, {} funcs){}{}",
                        m.name,
                        m.ports.len(),
                        m.nodes.len(),
                        m.params.len(),
                        m.functions.len(),
                        unplaceable_clause(&m, &skipped),
                        skipped_clause(&skipped)
                    );
                }
                Err(e) => {
                    // The skipped-include clause is what makes this attributable: ten corpus
                    // files report "port `D` has no discipline declaration" purely because the
                    // declarations were in a `` `include `` that never shipped.
                    println!(
                        "  [elab ] {file}: module `{}`: {e}{}",
                        ast.name,
                        skipped_clause(&skipped)
                    );
                    all_ok = false;
                }
            }
        }
        if !all_ok {
            tally.failed += 1;
            if !skipped.is_empty() {
                tally.failed_incomplete += 1;
            }
            continue;
        }
        tally.passed += 1;
        // A pass whose preprocessing dropped an `` `include `` is a pass on *less source than
        // the file names*. Counting it beside a self-contained model is what let 16 corpus
        // files report "0 params, 0 funcs" and still be scored as coverage.
        if !skipped.is_empty() {
            tally.passed_incomplete += 1;
        }
        // Built, but out of a source whose meaning this engine does not fully honour. Re-read
        // rather than threaded down from `parse_file`: this is a diagnostic command, and the
        // cost of one extra read per passing file is not worth widening a return type for.
        let approximated = std::fs::read_to_string(file)
            .map(|src| approximations_in(&src))
            .unwrap_or_default();
        if !approximated.is_empty() {
            tally.passed_approximated += 1;
            println!(
                "  [approx] {file}: builds, but uses {} (approximated in transient)",
                approximated.join(", ")
            );
        }
    }
    tally
}

/// Reject mismatches between what the deck's own dot-card requests and what the caller asked to
/// run, and analyses the deck doesn't carry the parameters for.
fn gate_analysis(net: &Netlist, analysis: Analysis) -> Result<()> {
    if net.analysis == AnalysisCard::Tran && analysis != Analysis::Transient {
        bail!("deck requests transient analysis (`.tran`); pass `--tran` to run it");
    }
    if net.analysis == AnalysisCard::Ac && analysis != Analysis::Ac {
        bail!("deck requests AC analysis (`.ac`); pass `--ac` to run it");
    }
    if net.analysis == AnalysisCard::Noise && analysis != Analysis::Noise {
        bail!("deck requests noise analysis (`.noise`); pass `--noise` to run it");
    }
    if analysis == Analysis::Transient && net.tran.is_none() {
        bail!(
            "transient analysis requested but the deck has no parseable \
             `.tran <tstep> <tstop>` card"
        );
    }
    if analysis == Analysis::Ac && net.ac.is_none() {
        bail!(
            "AC analysis requested but the deck has no parseable \
             `.ac dec <points-per-decade> <fstart> <fstop>` card (only the `dec` sweep type \
             is supported)"
        );
    }
    if analysis == Analysis::Noise && net.noise.is_none() {
        bail!(
            "noise analysis requested but the deck has no parseable \
             `.noise V(<out>) <source> dec <points-per-decade> <fstart> <fstop>` card (only the \
             `dec` sweep type and a single-node `V(<out>)` probe are supported)"
        );
    }
    Ok(())
}

/// [`build_instances`]'s return: built instances, the total unknown count (`dim`), and every
/// `vsource` device's own name paired with its assigned branch-current global index.
/// Every IR node of one built instance, paired with the global unknown index it was assigned
/// (§ quantity reporting). Produced by `build_from_model`, which *is* where the assignment
/// happens — reporting reads it rather than re-deriving it, because a second implementation of
/// the same index arithmetic would be free to drift, and a label silently attached to the wrong
/// unknown is worse than no label.
type NodeAssignment = Vec<(usize, va_ir::NodeDecl)>;

/// A built device instance: the instance itself, its own branch-current unknown if it claimed
/// one, and its [`NodeAssignment`].
type BuiltDevice = (Box<dyn ModelInstance>, Option<usize>, NodeAssignment);

type BuiltInstances = (
    Vec<Box<dyn ModelInstance>>,
    usize,
    Vec<(String, usize)>,
    Vec<Quantity>,
);

/// One labelled entry of the solution vector (§ quantity reporting).
///
/// A simulation is not necessarily electrical, so a report is not entitled to assume its
/// unknowns are volts and amperes: printing `V(shaft) = 5.2 V` for a mechanical node is not a
/// formatting blemish, it is a false statement about what was computed. Every quantity
/// therefore carries the access-function name and units its own discipline declares
/// (`va_ir::NodeDecl::access`/`units`, resolved from the potential nature), falling back to the
/// electrical spelling only where nothing better is known — which is correct for a deck built
/// from the built-in `R`/`C`/`L`/`V` primitives, since those *are* electrical by definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    /// How the quantity is written, access function and all: `V(in)`, `I(V1)`,
    /// `Omega(M1.shaft)`.
    pub label: String,
    /// The unit string to print after the value, or empty when the discipline declares none.
    pub unit: String,
    /// Position in the solution vector `x`.
    pub index: usize,
    /// The bare name inside the access function (`in`, `V1`, `M1.shaft`), for `--report`
    /// matching — so a user can ask for `mid` without also having to know it is a potential.
    pub name: String,
    /// Whether this is a node *potential* rather than a flow or an auxiliary row. Decides the
    /// number format only: a potential reads naturally in fixed point, while a flow is
    /// routinely microamps and would print as six zeros there. Recorded as a flag rather than
    /// re-derived from the label, which would mean testing for a literal `"V("` and so format
    /// a mechanical node's `Omega(shaft)` as though it were a current.
    pub is_potential: bool,
}

impl Quantity {
    /// Render one value of this quantity, e.g. `V(in) = 0.500000 V`.
    ///
    /// A potential prints in fixed point only while six decimals still carry at least three
    /// significant digits, i.e. `|value| >= 1e-3` (or exactly zero); below that it switches to
    /// scientific, like a flow. Volts are the case fixed point was chosen for, and a
    /// sub-millivolt node is rare there — but a potential is not always volts: a wavelength net
    /// holds `1.5505e-6 m` and an optical power net microwatts, and both printed as
    /// `0.000002` before 2026-09-11, which is not a number anyone can check a model against.
    fn render(&self, value: f64) -> String {
        let unit = if self.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", self.unit)
        };
        if self.is_potential && (value == 0.0 || value.abs() >= 1e-3) {
            format!("{} = {:.6}{unit}", self.label, value)
        } else {
            format!("{} = {:.6e}{unit}", self.label, value)
        }
    }
}

/// Bracket a compound unit so it can safely carry an exponent or a denominator
/// (§ quantity reporting).
///
/// `rads/s` squared per hertz is `(rads/s)^2/Hz`; written unbracketed as `rads/s^2/Hz` it reads
/// as rads per second-squared per hertz, which is a different quantity. A simple unit like `V`
/// needs no brackets and keeps the conventional `V^2/Hz` spelling.
fn bracket_unit(unit: &str) -> String {
    if unit.contains(['/', '*', '-', ' ']) {
        format!("({unit})")
    } else {
        unit.to_string()
    }
}

/// The access-function name and units to print for a node, given whatever its discipline
/// resolved to. `None`/absent means no `discipline...enddiscipline` preamble reached this node —
/// which is the normal case for a deck of built-in primitives, and those are electrical.
fn node_label(decl: Option<&va_ir::NodeDecl>) -> (String, String) {
    let access = decl
        .and_then(|d| d.access.clone())
        .unwrap_or_else(|| "V".to_string());
    let units = decl
        .and_then(|d| d.units.clone())
        .unwrap_or_else(|| "V".to_string());
    (access, units)
}

/// Build every device instance, returning them alongside the total unknown count (`dim`) and
/// every `vsource` device's own name paired with its assigned branch-current global index (§
/// [`branch_currents`]) — the only devices with a directly-addressable MNA branch-current
/// unknown in this codegen today. `compiled` is every module compiled from the `--model` file
/// (possibly several, if it defines a subcircuit alongside a top module — § module
/// instantiation); a device is matched against whichever one shares its model name. Shared by
/// both DC and transient solving — building the instance set doesn't depend on which analysis
/// will run on it.
fn build_instances(net: &Netlist, compiled: &[Module]) -> Result<BuiltInstances> {
    let n_nodes = net.node_order.len();

    // Voltage sources take branch-current unknowns after the node unknowns; a flattened
    // compiled module's internal (non-port) nodes need global unknowns too (§ module
    // instantiation — `va-codegen::build_instance` requires one global index per IR node, not
    // just per port). Both draw from this single shared counter, so `dim` is only known once
    // every instance has claimed what it needs.
    let mut next_unknown = n_nodes;
    let mut instances: Vec<Box<dyn ModelInstance>> = Vec::with_capacity(net.devices.len());
    let mut currents = Vec::new();
    // Phase 1: everything whose construction depends on nothing else. A current-controlled
    // source (`F`/`H`) is deferred, because it needs the *branch row* of the element it senses
    // and that element may be written after it in the deck. Deferring is only safe because
    // branch identity is carried by the `currents` map rather than inferred from device order
    // (§ `report`'s own doc comment, and the bug that established it).
    // Per-netlist-node discipline metadata, filled in by whichever device model declares it.
    // **Limitation:** first writer wins. `Elaborator::check_port_discipline` compares
    // disciplines *within* a module, but nothing compares two deck devices that attach
    // different disciplines to the same net, so a deck-level conflict is silently resolved
    // here rather than reported.
    let mut node_decls: Vec<Option<va_ir::NodeDecl>> = vec![None; n_nodes];
    // Unknowns a compiled model introduced beyond the netlist's own nodes: its internal
    // (non-port) nodes, and the auxiliary branch rows `va-codegen` allocates for a potential
    // contribution. Recorded per device so they can be named `<device>.<node>`.
    let mut internal: Vec<Quantity> = Vec::new();
    for dev in &net.devices {
        if matches!(dev.model.as_str(), "cccs" | "ccvs" | "mutual") {
            continue;
        }
        let before = next_unknown;
        let (inst, branch, assignment) = build_instance(dev, compiled, &mut next_unknown)?;
        let mut assigned_here: Vec<usize> = Vec::new();
        // Auxiliary rows are numbered per device, so `X1.b0` is X1's first regardless of what
        // any earlier device claimed.
        let mut aux = 0usize;
        for (g, decl) in &assignment {
            assigned_here.push(*g);
            if *g < n_nodes {
                // A port node: it *is* one of the deck's nets, so this is where a netlist node
                // learns what discipline governs it.
                if node_decls[*g].is_none() {
                    node_decls[*g] = Some(decl.clone());
                }
            } else if *g >= before {
                let (access, units) = node_label(Some(decl));
                let name = format!("{}.{}", dev.name, decl.name);
                internal.push(Quantity {
                    label: format!("{access}({name})"),
                    unit: units,
                    index: *g,
                    name,
                    is_potential: true,
                });
            }
        }
        // Whatever is left in `[before, next_unknown)` is an auxiliary row `va-codegen` claimed
        // while lowering the model — a branch flow for a potential contribution (`V(p,n) <+
        // ...`), an `idt` accumulator, and so on. These are reported, because they are genuine
        // entries of the solution vector and one of them is often the quantity a user actually
        // wants (a series-RLC model's branch current is an auxiliary row, not a deck net).
        //
        // They are deliberately reported *without* an access function or a unit. Their physical
        // meaning is the model's own business and nothing in Interface α records it: a branch
        // row carries a flow, but an `idt` accumulator carries that flow's time integral, so
        // labelling the pair alike as `I(...)`/`A` would be confidently wrong about half of
        // them. A bare name and no unit says exactly as much as is actually known.
        for g in before..next_unknown {
            // `branch` is already reported by name as this device's own current (`I(V1)`);
            // labelling it again here would print the same unknown twice under two names.
            if assigned_here.contains(&g) || branch == Some(g) {
                continue;
            }
            let k = aux;
            aux += 1;
            let name = format!("{}.b{k}", dev.name);
            internal.push(Quantity {
                label: name.clone(),
                unit: String::new(),
                index: g,
                name,
                is_potential: false,
            });
        }
        if let Some(branch) = branch {
            currents.push((dev.name.clone(), branch));
        }
        instances.push(inst);
    }
    // Phase 2: the current-controlled sources, now that every branch row above has an index.
    for dev in &net.devices {
        if dev.model == "mutual" {
            // A `K` names two inductors and couples their flux. Both the branch rows and the
            // inductances come from those elements, so it can only be built once they exist.
            let (a, b) = match dev.controls.as_slice() {
                [a, b] => (a.as_str(), b.as_str()),
                _ => bail!("`{}` must name exactly two inductors", dev.name),
            };
            let find = |want: &str| -> Result<(usize, f64)> {
                let d = net
                    .devices
                    .iter()
                    .find(|d| d.name == want && d.model == "inductor")
                    .with_context(|| {
                        format!("`{}` couples `{want}`, which is not an inductor", dev.name)
                    })?;
                let branch = currents
                    .iter()
                    .find(|(name, _)| name == want)
                    .map(|(_, i)| *i)
                    .with_context(|| format!("`{want}` has no branch row"))?;
                Ok((branch, d.value.unwrap_or(0.0)))
            };
            let (b1, l1) = find(a)?;
            let (b2, l2) = find(b)?;
            if b1 == b2 {
                bail!("`{}` couples `{a}` to itself", dev.name);
            }
            instances.push(Box::new(Mutual::from_coupling(
                b1,
                b2,
                l1,
                l2,
                dev.value.unwrap_or(0.0),
            )));
            continue;
        }
        if !matches!(dev.model.as_str(), "cccs" | "ccvs") {
            continue;
        }
        let ctl_name = dev
            .controls
            .first()
            .map(String::as_str)
            .with_context(|| format!("`{}` names no controlling element", dev.name))?;
        let ctl = currents
            .iter()
            .find(|(name, _)| name == ctl_name)
            .map(|(_, idx)| *idx)
            .with_context(|| {
                format!(
                    "`{}` is controlled by `{ctl_name}`, which has no branch current to sense                      (only a voltage source, an inductor, or an `E`/`H` carries one)",
                    dev.name
                )
            })?;
        let (p, n) = (dev.terminals[0], dev.terminals[1]);
        let gain = dev.value.unwrap_or(0.0);
        if dev.model == "cccs" {
            instances.push(Box::new(Cccs::new(p, n, ctl, gain)));
        } else {
            let branch = next_unknown;
            next_unknown += 1;
            currents.push((dev.name.clone(), branch));
            instances.push(Box::new(Ccvs::new(p, n, ctl, branch, gain)));
        }
    }
    // Assemble the reportable quantities in solution-vector order: every deck net first (with
    // whatever discipline its attached models declared), then every named device branch
    // current, then whatever a compiled model introduced of its own.
    let mut quantities: Vec<Quantity> = Vec::with_capacity(n_nodes + currents.len());
    for (i, name) in net.node_order.iter().enumerate() {
        let (access, unit) = node_label(node_decls[i].as_ref());
        quantities.push(Quantity {
            label: format!("{access}({name})"),
            unit,
            index: i,
            name: name.clone(),
            is_potential: true,
        });
    }
    for (name, idx) in &currents {
        quantities.push(Quantity {
            label: format!("I({name})"),
            unit: "A".to_string(),
            index: *idx,
            name: name.clone(),
            is_potential: false,
        });
    }
    quantities.extend(internal);

    Ok((instances, next_unknown, currents, quantities))
}

/// Map every `vsource` device's own name to its assigned branch-current global index —
/// structural (independent of any solve): the same `net`/`compiled` always assigns the same
/// indices, in device order, so this can be called once per circuit and reused across every
/// point of a `.dc` sweep or every step of a `.tran` run.
///
/// `pub` so `va-harness` can build a golden reference that also carries named branch currents,
/// not just node voltages (§ rung 2's own honest coverage caveat — `docs/roadmap.md`'s T6.3
/// section: a diode forced by a directly-connected voltage source has a node voltage that
/// trivially matches golden regardless of whether the diode model itself is right; the source's
/// own current is the quantity that actually depends on it).
pub fn branch_currents(net: &Netlist, compiled: &[Module]) -> Result<Vec<(String, usize)>> {
    let (_, _, currents, _) = build_instances(net, compiled)?;
    Ok(currents)
}

/// Every reportable entry of the solution vector, in report order (§ quantity reporting):
/// each deck net, each named device branch current, then whatever a compiled Verilog-A model
/// introduced of its own (internal nodes, auxiliary branch rows). `pub` for the same reason
/// [`branch_currents`] is — so a caller can label results without re-deriving index assignment.
pub fn quantities(net: &Netlist, compiled: &[Module]) -> Result<Vec<Quantity>> {
    let (_, _, _, quantities) = build_instances(net, compiled)?;
    Ok(quantities)
}

/// Narrow `quantities` to those the user asked for with `--report`.
///
/// A selector matches a quantity when it equals either the full label (`V(mid)`) or the bare
/// name inside it (`mid`), case-insensitively — so a user who wants a net does not have to know
/// whether it is reported as a potential or a flow, and someone who wants to disambiguate still
/// can. Unmatched selectors are an error rather than a silent empty column: a typo in a net name
/// would otherwise look exactly like a quantity that was computed and found to be zero.
pub fn select_quantities(all: &[Quantity], selectors: &[String]) -> Result<Vec<Quantity>> {
    if selectors.is_empty() {
        return Ok(all.to_vec());
    }
    let mut out: Vec<Quantity> = Vec::new();
    for sel in selectors {
        let s = sel.trim();
        let hits: Vec<&Quantity> = all
            .iter()
            .filter(|q| q.name.eq_ignore_ascii_case(s) || q.label.eq_ignore_ascii_case(s))
            .collect();
        if hits.is_empty() {
            let mut known: Vec<&str> = all.iter().map(|q| q.label.as_str()).collect();
            known.sort_unstable();
            bail!(
                "--report names `{s}`, which this circuit does not compute (it has: {})",
                known.join(", ")
            );
        }
        for h in hits {
            if !out.iter().any(|q| q.index == h.index) {
                out.push(h.clone());
            }
        }
    }
    Ok(out)
}

/// Build every device instance and solve the DC operating point. `pub` so `va-harness` can get
/// the numeric [`va_core::dc::OperatingPoint`] back directly (§ golden comparison), rather than
/// parsing [`run_sim`]'s printed stdout.
pub fn solve_dc(net: &Netlist, compiled: &[Module]) -> Result<va_core::dc::OperatingPoint> {
    let (instances, dim, _currents, quantities) = build_instances(net, compiled)?;
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    // Events-aware: `above` fires in a static solve when its expression is already past the
    // threshold, and the body it guards changes the equations (§ `@(above)`).
    va_core::dc::operating_point_with_events(&refs, dim, NewtonConfig::default(), None)
        .map(|(op, _)| op)
        .map_err(|e| name_non_finite_row(e.into(), &quantities))
        .context("DC operating-point solve failed")
}

/// If `err` is a [`va_core::CoreError::NonFinite`] (bare, or inside a
/// [`va_transient::TransientError`]), wrap it with the *name* of the unknown whose equation
/// went non-finite — `Popt(drop)` or `X3.b0`, not "row 7". The row is what the core can know;
/// which device's contribution lands there is what the user needs, and the unknown's label
/// (a net in some discipline, a device's branch row, a compiled model's internal unknown) is
/// the nearest thing this layer has to it. Any other error passes through untouched.
fn name_non_finite_row(err: anyhow::Error, quantities: &[Quantity]) -> anyhow::Error {
    let row = match err.downcast_ref::<va_core::CoreError>() {
        Some(va_core::CoreError::NonFinite { row, .. }) => Some(*row),
        _ => match err.downcast_ref::<va_transient::TransientError>() {
            Some(va_transient::TransientError::Core(va_core::CoreError::NonFinite {
                row, ..
            })) => Some(*row),
            _ => None,
        },
    };
    let Some(row) = row else {
        return err;
    };
    match quantities.iter().find(|q| q.index == row) {
        Some(q) => err.context(format!(
            "the non-finite value is in the equation of `{}` (unknown #{row}); look at the device(s) contributing to it",
            q.label
        )),
        None => err.context(format!(
            "the non-finite value is in the equation of unknown #{row}, which no reported quantity names"
        )),
    }
}

/// Solve a `.dc` sweep (§ ladder rung 2): re-solve the whole circuit fresh at each swept value
/// of `sweep.source`, since `va-core::dc::sweep` is agnostic about *what* changed between
/// points and just wants a fresh instance set per point. `sweep.source` must name a `vsource`
/// device; anything else is a clear error rather than a silently-ignored sweep. `pub` for the
/// same reason `solve_dc` is (§ golden comparison) — `va-harness` wants the numeric points back,
/// not `run_sim`'s printed stdout.
pub fn solve_dc_sweep(
    net: &Netlist,
    compiled: &[Module],
    sweep: &va_netlist::DcSweep,
) -> Result<Vec<(f64, va_core::dc::OperatingPoint)>> {
    let src = net
        .devices
        .iter()
        .find(|d| d.name == sweep.source)
        .with_context(|| format!("`.dc` sweeps unknown device `{}`", sweep.source))?;
    if src.model != "vsource" {
        bail!(
            "`.dc` can only sweep a voltage source; `{}` is a `{}`",
            sweep.source,
            src.model
        );
    }

    let points = sweep_points(sweep.start, sweep.stop, sweep.step);
    let mut out = Vec::with_capacity(points.len());
    for value in points {
        let mut swept = net.clone();
        let dev = swept
            .devices
            .iter_mut()
            .find(|d| d.name == sweep.source)
            .expect("just found this device above");
        dev.value = Some(value);
        let op = solve_dc(&swept, compiled)
            .with_context(|| format!("`.dc` sweep at {}={value}", sweep.source))?;
        out.push((value, op));
    }
    Ok(out)
}

/// Generate the swept values `start, start+step, …` up to and including `stop` (within half a
/// step, to absorb float rounding at the endpoint — the SPICE-standard inclusive-range
/// convention). A zero or wrong-signed `step` (one that would never reach `stop`) yields just
/// `start`, rather than looping forever.
fn sweep_points(start: f64, stop: f64, step: f64) -> Vec<f64> {
    if step == 0.0 || (stop - start) * step < 0.0 {
        return vec![start];
    }
    let n = ((stop - start) / step).round().max(0.0) as usize;
    (0..=n).map(|i| start + step * i as f64).collect()
}

/// A `V` source whose value follows a netlist waveform (`SIN(...)`) rather than staying
/// constant — an ideal [`VSource`] whose value is recomputed from the analysis context's
/// current time on every evaluation.
///
/// This is still a **stateless** instance, which is what makes it legal under Interface β:
/// `load` remains a pure function of `(x, ctx)`, returning identical stamps whenever it is
/// called with identical arguments. It may be re-entered freely within a Newton iteration and
/// on a rejected timestep, exactly like every other model.
///
/// Outside transient there is no time axis, so `ctx.time` is `0.0` and the source evaluates to
/// its waveform's value at `t = 0` — the offset, which is precisely the DC value
/// `va_netlist`'s parser already derives for the same device. A DC operating point and an AC
/// linearization therefore see the same source they always did.
struct WaveformSource {
    terminals: [usize; 3], // [p, n, branch-current]
    waveform: va_netlist::Waveform,
}

impl ModelInstance for WaveformSource {
    fn unknowns(&self) -> &[usize] {
        &self.terminals
    }

    fn unknown_kind(&self, i: usize) -> va_abi::UnknownKind {
        // Delegated verbatim from `VSource`: index 2 is this source's own constraint row.
        if i == 2 {
            va_abi::UnknownKind::Branch
        } else {
            va_abi::UnknownKind::Node
        }
    }

    fn load(
        &self,
        x: &[f64],
        ctx: &va_abi::AnalysisCtx,
        state: &mut va_abi::ModelState,
        sink: &mut dyn va_abi::StampSink,
    ) {
        let [p, n, b] = self.terminals;
        VSource::new(p, n, b, waveform_value(self.waveform, ctx.time)).load(x, ctx, state, sink)
    }
}

/// Evaluate a parsed source waveform at time `t`.
fn waveform_value(waveform: va_netlist::Waveform, t: f64) -> f64 {
    match waveform {
        va_netlist::Waveform::Sin {
            offset,
            amplitude,
            freq,
        } => offset + amplitude * (2.0 * PI * freq * t).sin(),
        va_netlist::Waveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => {
            // Before the first edge the source sits at `v1`; a non-positive period means the
            // pulse never repeats, so time past the first cycle stays on the falling tail.
            if t < td {
                return v1;
            }
            let since = if per > 0.0 { (t - td) % per } else { t - td };
            if since < tr {
                // Rising edge. A zero rise time is an ideal step, and is never divided by:
                // `since < 0.0` is impossible, so `since < tr` is false when `tr == 0.0`.
                v1 + (v2 - v1) * (since / tr)
            } else if since < tr + pw {
                v2
            } else if since < tr + pw + tf {
                v2 + (v1 - v2) * ((since - tr - pw) / tf)
            } else {
                v1
            }
        }
    }
}

/// Build every device instance and integrate the transient response over the deck's
/// `.tran <tstep> <tstop>` window.
///
/// Starts from the zero vector unless an element carries a SPICE `IC=` (see the `dev.ic`
/// handling below, added 2026-08-31); there is no `.ic` card or circuit-wide `UIC` flag (this
/// module's doc
/// comment). Every deck takes the same path ([`va_transient::integrator::run`]) whether or not
/// it contains a time-varying source: a `SIN` source is a [`WaveformSource`], which reads the
/// time from the analysis context like any other analysis-dependent model. `pub` so
/// `va-harness` can get the numeric [`Waveform`] back directly (§ golden comparison), the same
/// reason `solve_dc`/`solve_dc_sweep` are — rather than parsing [`run_sim`]'s printed stdout.
/// The time discretization for a transient run, chosen with `va-cli sim --integration <be|trap>`.
///
/// It selects the integrator's method. Generated models are **method-independent** — a
/// bias-dependent `ddt` coefficient was briefly compiled per-method, but that was an integrator
/// defect (fixed by taking the first step with backward Euler) rather than a property of the
/// model, so there is nothing to keep in step any more.
///
/// [`Self::Trapezoidal`] is the default: it is second order, and it is what every committed
/// transient golden was validated against.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Integration {
    /// Second-order trapezoidal. The default.
    #[default]
    Trapezoidal,
    /// Backward Euler — required for a bias-dependent `ddt` coefficient.
    BackwardEuler,
    /// Gear / BDF2, second order and **L-stable** (2026-09-01). Unlike trapezoidal it damps a
    /// stiff mode to zero in one step rather than approaching an amplification factor of -1,
    /// so it does not ring numerically on a stiff transient. That is the only reason to pick
    /// it: it is not more accurate than trapezoidal, being the same order.
    ///
    /// Not the default, and no committed golden was validated under it — see
    /// `docs/validation.md` for the measured comparison on this project's own circuits.
    Gear,
}

impl Integration {
    /// Parse the `--integration` argument. Accepts `be`/`backward-euler` and `trap`/
    /// `trapezoidal`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "be" | "backward-euler" | "backward_euler" => Some(Self::BackwardEuler),
            "trap" | "trapezoidal" => Some(Self::Trapezoidal),
            "gear" | "bdf2" => Some(Self::Gear),
            _ => None,
        }
    }
}

/// Run a transient analysis. `integration` selects the integrator's method — see
/// [`Integration`].
pub fn solve_transient(
    net: &Netlist,
    compiled: &[Module],
    integration: Integration,
) -> Result<Waveform> {
    let (tstep, tstop) = net
        .tran
        .context("transient analysis requires a `.tran <tstep> <tstop>` card")?;
    // Generated models are method-independent (§ `Integration`), so this selects the
    // integrator's method and nothing else. `Trapezoidal` stays the default: it is second order,
    // and it is what every committed transient golden was validated against.
    let method = match integration {
        Integration::BackwardEuler => Method::BackwardEuler,
        Integration::Trapezoidal => Method::Trapezoidal,
        Integration::Gear => Method::Gear,
    };
    let cfg = TranConfig {
        tstart: 0.0,
        tstop,
        tstep,
        tstep_min: tstep * 1e-6,
        method,
        lte_reltol: 1e-3,
        lte_abstol: 1e-6,
        // Divided differences, the rigorous estimator: it reads the local truncation error
        // off a divided difference of past accepted points instead of buying a second Newton
        // solve on every step attempt (~2.5x fewer model evaluations). The transient gates
        // were re-validated under it on 2026-08-31 against the same, unchanged QSPICE golden
        // -- see `va_transient::integrator::LteEstimator` and docs/roadmap.md's T4.2 entry.
        lte_estimator: LteEstimator::DividedDifference,
    };

    let (instances, dim, currents, quantities) = build_instances(net, compiled)?;
    let x0 = initial_solution(net, dim, &currents);
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();

    va_transient::integrator::run(&refs, dim, x0, cfg)
        .map_err(|e| name_non_finite_row(e.into(), &quantities))
        .context("transient integration failed")
}

/// The transient run's initial solution vector: zero everywhere, then each reactive element's
/// `IC=` applied (`va_netlist::Device::ic`) — a capacitor's volts as `V(p) = V(n) + ic`, an
/// inductor's amps straight onto its own branch-current row, whose index `currents` carries.
///
/// Only the seeded entry is set. An inductor's initial current does not back-solve the node
/// voltages that current implies, so the `t = tstart` sample can be inconsistent until the
/// first real solve corrects it — the same unsolved-seed sample `va-harness` already excludes
/// from a golden comparison.
///
/// This is SPICE's `UIC` semantics and nothing more: **no DC operating point is solved first**.
/// A capacitor with no `IC=` starts at 0 V, which is what this engine has always done and what
/// `xtask`'s golden-deck translator reproduces on the QSPICE side by injecting `IC=0` into every
/// reactive element it finds without one.
///
/// # Limitations
///
/// Conditions are applied in device order, each reading whatever `V(n)` holds *at that point*,
/// so a chain of capacitors referenced to each other resolves only if it is written in
/// dependency order; a genuinely floating capacitor between two nodes that no other `IC=`
/// pins leaves the pair under-determined and lands with `V(n) = 0`. A grounded capacitor — the
/// case SPICE decks overwhelmingly write, and the only one in this project's zoo — is exact.
/// Nothing here reconciles two conditions that contradict each other; the last one written
/// wins, rather than the conflict being reported.
fn initial_solution(net: &Netlist, dim: usize, currents: &[(String, usize)]) -> Vec<f64> {
    let mut x0 = vec![0.0; dim];
    for dev in &net.devices {
        let Some(ic) = dev.ic else { continue };
        // An inductor's state is its *current*, which lives on its own branch row rather than
        // across its terminals, so it seeds a different entry entirely (see `Device::ic`'s
        // units). The branch index is the one `build_instances` already handed back.
        if dev.model == "inductor" {
            if let Some(&(_, branch)) = currents.iter().find(|(name, _)| *name == dev.name) {
                if branch < dim {
                    x0[branch] = ic;
                }
            }
            continue;
        }
        let (Some(&p), Some(&n)) = (dev.terminals.first(), dev.terminals.get(1)) else {
            continue;
        };
        // `GROUND` is not a row in the reduced system; a terminal at ground contributes 0.
        let vn = if n < dim { x0[n] } else { 0.0 };
        if p < dim {
            x0[p] = vn + ic;
        }
    }
    x0
}

/// Build the complex small-signal excitation vector (`b` in `(G + jω·C)·X = b`) for `net`'s own
/// AC sources, given every `vsource` device's assigned branch-current index (`currents`, §
/// [`branch_currents`]).
///
/// A source's `AC <magnitude> [phase]` spec becomes a single entry at its **own branch-current
/// row** — the same row its DC constraint (`V(p)-V(n) = value`) is stamped on. That row's
/// Jacobian entries are already captured in `G`, so the stimulus is purely an RHS term (§
/// `va_acnoise::ac::run`'s own doc comment); a source with no `AC` token contributes nothing but
/// still holds its terminals to a zero small-signal difference through that same row, exactly as
/// SPICE does.
///
/// # Errors
///
/// If no source in the deck carries an `AC` spec at all — the resulting system would be
/// homogeneous, solving to an all-zero response at every frequency, which is a silently useless
/// answer rather than a meaningful one.
fn ac_excitation(
    net: &Netlist,
    currents: &[(String, usize)],
    dim: usize,
) -> Result<Vec<va_acnoise::ac::Complex>> {
    let mut excitation = vec![(0.0, 0.0); dim];
    let mut driven = 0usize;
    for (name, branch) in currents {
        let Some(dev) = net.devices.iter().find(|d| &d.name == name) else {
            continue;
        };
        if let Some(ac) = dev.ac {
            let phase = ac.phase_deg.to_radians();
            excitation[*branch] = (ac.magnitude * phase.cos(), ac.magnitude * phase.sin());
            driven += 1;
        }
    }
    if driven == 0 {
        bail!(
            "AC analysis needs at least one source with an `AC <magnitude>` spec; none of this \
             deck's {} voltage source(s) has one (the response would be identically zero)",
            currents.len()
        );
    }
    Ok(excitation)
}

/// Build every device instance, solve the DC operating point, and sweep the small-signal AC
/// response over the deck's `.ac dec <points-per-decade> <fstart> <fstop>` grid (T5).
///
/// The DC solve is not incidental: `va_acnoise::ac::linearize` captures `G`/`C` from each
/// instance's own Jacobian *at that point*, so a nonlinear device's small-signal behavior (a
/// diode's `gd = Is/(N·Vt)·exp(V/(N·Vt))`, say) is only right if the bias it was linearized
/// about is. `pub` so `va-harness` can get the numeric response back directly (§ golden
/// comparison), the same reason `solve_dc`/`solve_dc_sweep`/`solve_transient` are.
///
/// # Errors
///
/// If the deck has no parseable `.ac` card, no AC-excited source ([`ac_excitation`]), the DC
/// operating-point solve diverges, or the complex solve is singular at some frequency.
pub fn solve_ac(net: &Netlist, compiled: &[Module]) -> Result<va_acnoise::ac::AcResponse> {
    let card = net
        .ac
        .context("AC analysis requires an `.ac dec <points-per-decade> <fstart> <fstop>` card")?;

    let (instances, dim, currents, _quantities) = build_instances(net, compiled)?;
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    let op = operating_point(&refs, dim, NewtonConfig::default())
        .context("DC operating-point solve failed (AC analysis linearizes about it)")?;
    let excitation = ac_excitation(net, &currents, dim)?;

    let sweep = va_acnoise::ac::AcSweep {
        fstart: card.fstart,
        fstop: card.fstop,
        points: card.points,
        kind: match card.kind {
            va_netlist::AcSweepKindCard::Dec => va_acnoise::ac::AcSweepKind::Dec,
            va_netlist::AcSweepKindCard::Oct => va_acnoise::ac::AcSweepKind::Oct,
            va_netlist::AcSweepKindCard::Lin => va_acnoise::ac::AcSweepKind::Lin,
        },
    };
    va_acnoise::ac::run(&refs, &op.x, dim, sweep, &excitation).context("AC sweep failed")
}

/// Build every device instance, solve the DC operating point, and sweep the small-signal output
/// noise PSD over the deck's `.noise` grid (T5.2).
///
/// The output node named by the deck's `V(<out>)` probe is resolved to its global unknown index
/// here — a name that isn't a net in this circuit is a clear error rather than a silently
/// mis-probed spectrum — and so is the card's input source, which adds the input-referred
/// spectrum. Noise sources come from Interface β's noise channel
/// (`va_abi::ModelInstance::noise`), so a device that doesn't implement it contributes nothing;
/// notably **every `va-codegen`-compiled model is silent today** (Verilog-A's `white_noise()`/
/// `flicker_noise()` are not lowered), which is why a meaningful noise deck uses the hand-written
/// reference primitives rather than a `--model` compiled one. Rather than let that produce a
/// quietly-zero spectrum, this reports an error when the circuit has no noise sources at all.
///
/// # Errors
///
/// If the deck has no parseable `.noise` card, its output probe names an unknown net, the DC
/// operating-point solve diverges, no device in the circuit contributes any noise, or an adjoint
/// solve is singular at some frequency.
pub fn solve_noise(net: &Netlist, compiled: &[Module]) -> Result<va_acnoise::noise::NoiseSpectrum> {
    let card = net.noise.as_ref().context(
        "noise analysis requires a `.noise V(<out>) <source> dec <ppd> <fstart> <fstop>` card",
    )?;
    let output = *net.nodes.get(&card.output).with_context(|| {
        format!(
            "`.noise` probes V({}), which is not a net in this circuit (nets: {})",
            card.output,
            net.node_order.join(", ")
        )
    })?;

    let (instances, dim, currents, _quantities) = build_instances(net, compiled)?;
    // The `.noise` card's input source, resolved to its own branch-current row — the row an AC
    // stimulus would excite, and therefore (§ `va_acnoise::noise`) the row of the adjoint vector
    // that already holds the forward gain. Only a `vsource` has such a row, so naming anything
    // else is a clear error rather than a silently output-referred-only answer.
    let input = currents
        .iter()
        .find(|(name, _)| *name == card.source)
        .map(|&(_, branch)| branch);
    if input.is_none() {
        bail!(
            "`.noise` names `{}` as its input source, which is not a voltage source in this \
             circuit (sources: {})",
            card.source,
            if currents.is_empty() {
                "none".to_string()
            } else {
                currents
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
    }

    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    let op = operating_point(&refs, dim, NewtonConfig::default())
        .context("DC operating-point solve failed (noise analysis linearizes about it)")?;

    if !has_noise_sources(&refs, &op.x) {
        // The trailing note used to say Verilog-A's noise functions "are not lowered yet, so a
        // `--model`-compiled device is silent". That stopped being true when T5.3/T5.6 landed —
        // `circuits/resistor_noise_va.net` gates a *compiled* model's own `white_noise()`
        // against golden — so the message was telling users something false about why their
        // spectrum was empty, and pointing them away from the real cause.
        bail!(
            "no device in this circuit contributes any noise, so the spectrum would be \
             identically zero. A device is only a noise source if it says so: a resistor and \
             the reference diode contribute thermal/shot noise inherently, and a compiled \
             Verilog-A model contributes exactly what its own `white_noise()`/\
             `flicker_noise()`/`noise_table()` calls declare — a model with none is silent by \
             construction, not by omission in this simulator"
        );
    }

    let sweep = va_acnoise::ac::AcSweep {
        fstart: card.fstart,
        fstop: card.fstop,
        // `.noise` stays `dec`-only: its integrated-total maths assumes logarithmic
        // spacing, so accepting `lin` here would quietly change what the total means.
        points: card.points_per_decade,
        kind: va_acnoise::ac::AcSweepKind::Dec,
    };
    va_acnoise::noise::run_at_nominal_temp(&refs, &op.x, dim, sweep, output, input)
        .context("noise sweep failed")
}

/// Name each contributor in a solved [`va_acnoise::noise::NoiseSpectrum`]'s per-device
/// breakdown, pairing every entry with the netlist device that produced it.
///
/// **The mapping is positional**, and that is sound for one specific reason:
/// [`build_instances`] pushes exactly one instance per `net.devices` entry, in order, so
/// `instances[i]` is always `net.devices[i]`. `va-acnoise` tags each noise source with the index
/// of the instance that emitted it (it has no other identity to work with — a
/// `va_abi::ModelInstance` has no name), and this function turns that index back into a name.
///
/// An index with no corresponding device is skipped rather than guessed at or panicked on: it
/// would mean the 1:1 invariant above had been broken, and silently mislabelling someone else's
/// noise is worse than omitting a row.
pub fn noise_contributors(
    net: &Netlist,
    spectrum: &va_acnoise::noise::NoiseSpectrum,
) -> Vec<(String, Vec<f64>)> {
    spectrum
        .per_instance
        .iter()
        .filter_map(|(idx, series)| {
            net.devices
                .get(*idx)
                .map(|dev| (dev.name.clone(), series.clone()))
        })
        .collect()
}

/// Whether any instance emits at least one noise source at operating point `x` (§
/// [`solve_noise`]'s own "a silently zero spectrum is worse than an error" check).
fn has_noise_sources(instances: &[&dyn ModelInstance], x: &[f64]) -> bool {
    let mut probe = va_abi::noise::CollectedNoise::default();
    for inst in instances {
        inst.noise(x, &va_abi::AnalysisCtx::noise(), &mut probe);
        if !probe.sources.is_empty() {
            return true;
        }
    }
    false
}

/// Turn one parsed [`Device`] into a loadable instance, preferring a matching compiled
/// Verilog-A model and falling back to the reference primitives. Returns the device's own
/// branch-current global index too, if it claimed one (`Some` only for a `vsource` — the only
/// device kind with a directly-addressable MNA branch-current unknown in this codegen; see
/// [`branch_currents`]).
fn build_instance(
    dev: &Device,
    compiled: &[Module],
    next_unknown: &mut usize,
) -> Result<BuiltDevice> {
    // Read lazily rather than up front: every *letter* device has at least two terminals, but
    // an `X` line places a model with whatever port count that model declares, and a
    // one-terminal model is legal (the photonic library's `CwLaser(out)` is one). Indexing
    // both here panicked on exactly that — a panic on valid input, which CLAUDE.md 5 forbids.
    let two_terminals = || -> Result<(usize, usize)> {
        match dev.terminals.as_slice() {
            [p, n, ..] => Ok((*p, *n)),
            _ => bail!(
                "`{}` is a {}-terminal device line, but `{}` needs at least two nodes",
                dev.name,
                dev.terminals.len(),
                dev.model
            ),
        }
    };

    if dev.model == "vsource" {
        let (p, n) = two_terminals()?;
        let branch = *next_unknown;
        *next_unknown += 1;
        // A `SIN(...)` source becomes a time-reading instance; every other source is constant.
        // Both claim exactly one branch-current unknown, so the index assignment — and hence
        // `dim` and every downstream device's indices — is the same either way.
        let inst: Box<dyn ModelInstance> = match dev.waveform {
            Some(waveform) => Box::new(WaveformSource {
                terminals: [p, n, branch],
                waveform,
            }),
            None => Box::new(VSource::new(p, n, branch, dev.value.unwrap_or(0.0))),
        };
        return Ok((inst, Some(branch), Vec::new()));
    }

    if dev.model == "vccs" {
        // No extra unknown: a `G`'s output current is a function of node voltages the solver
        // already carries, so it stamps like a resistor that happens to read a different node
        // pair from the one it drives.
        let (p, n, cp, cn) = (
            dev.terminals[0],
            dev.terminals[1],
            dev.terminals[2],
            dev.terminals[3],
        );
        let inst: Box<dyn ModelInstance> =
            Box::new(Vccs::new(p, n, cp, cn, dev.value.unwrap_or(0.0)));
        return Ok((inst, None, Vec::new()));
    }

    if dev.model == "vcvs" {
        // Like an independent source, an `E` states a constraint rather than contributing a
        // current, so it claims its own branch row.
        let (p, n, cp, cn) = (
            dev.terminals[0],
            dev.terminals[1],
            dev.terminals[2],
            dev.terminals[3],
        );
        let branch = *next_unknown;
        *next_unknown += 1;
        let inst: Box<dyn ModelInstance> =
            Box::new(Vcvs::new(p, n, cp, cn, branch, dev.value.unwrap_or(0.0)));
        return Ok((inst, Some(branch), Vec::new()));
    }

    if dev.model == "inductor" {
        let (p, n) = two_terminals()?;
        // Like a voltage source, an inductor carries its own branch-current unknown: its row
        // is the constitutive law `-(V(p)-V(n)) + d(L*i)/dt = 0`, not a KCL sum. Claiming the
        // index here (rather than in `va-abi`) keeps `dim` assignment in one place, and
        // returning it as a named current means `I(L1)` reaches golden files exactly as a
        // source's own current does.
        let branch = *next_unknown;
        *next_unknown += 1;
        let inst: Box<dyn ModelInstance> =
            Box::new(Inductor::new(p, n, branch, dev.value.unwrap_or(0.0)));
        return Ok((inst, Some(branch), Vec::new()));
    }

    // Use the compiled Verilog-A model when its name matches the device's model.
    if let Some(module) = compiled.iter().find(|m| m.name == dev.model) {
        let (inst, assignment) =
            build_from_model(module, dev.value, &dev.params, &dev.terminals, next_unknown)?;
        return Ok((inst, None, assignment));
    }

    Ok((reference_instance(dev)?, None, Vec::new()))
}

/// Build a device instance from a compiled IR module, applying the device's parameter
/// overrides: the positional scalar `value` sets the model's *first* parameter (the SPICE
/// convention, where an `R`/`C` line's value is its primary parameter), then each `name=value`
/// in `overrides` sets the parameter it names. An override naming a parameter the model does
/// not declare is an error rather than a no-op — see the loop's own comment. Each of `module`'s port nodes is assigned the netlist terminal
/// it connects to; any other node (e.g. an internal node a flattened submodule instance
/// introduced, § module instantiation) claims a fresh global unknown from `next_unknown`.
/// A device line's overrides, with the instance multiplicity separated out: the multiplicity
/// (`None` when the line set none) and the remaining model-parameter overrides.
type SplitOverrides = (Option<f64>, Vec<(String, f64)>);

/// Separate a device line's instance **multiplicity** from its model-parameter overrides.
///
/// Returns the multiplicity (`None` when the line sets none, which reads as the LRM default of
/// `1.0`) and the overrides with that entry removed.
///
/// # Which spelling means what, and why it is not simply `m=`
///
/// SPICE spells multiplicity `m=`, and that is the spelling a user will reach for. But `m` is
/// also an extremely common *model parameter* name: in the corpus it is the junction grading
/// coefficient (`external/diode.va`, `diode_basic.va`, `angelov_gan.va` all declare
/// `parameter real m = 0.5`), a completely different physical quantity. Silently reinterpreting
/// `m=0.5` on such a line as "half a device in parallel" would be a wrong answer of exactly the
/// kind this project refuses to produce.
///
/// So:
///
/// - **`mult=`** is always the multiplicity. Unambiguous, and the spelling to reach for when a
///   model declares its own `m`.
/// - **`m=`** is the multiplicity **only when the model declares no parameter named `m`**.
///   Otherwise it sets that parameter, exactly as it always has — no existing deck changes
///   meaning.
///
/// A model that declares `mult` *and* a line that sets it is genuinely ambiguous, and is refused
/// rather than guessed at.
fn split_multiplicity(module: &Module, overrides: &[(String, f64)]) -> Result<SplitOverrides> {
    let declares = |name: &str| module.params.iter().any(|p| p.name == name);
    let mut multiplicity = None;
    let mut rest = Vec::with_capacity(overrides.len());
    for (name, v) in overrides {
        let is_multiplicity = match name.as_str() {
            "mult" => {
                if declares("mult") {
                    bail!(
                        "model `{}` declares its own parameter `mult`, so `mult=` on a device                          line is ambiguous: it could set that parameter or the instance                          multiplicity. Rename the model's parameter, or set the multiplicity                          with `m=` if the model declares no `m` either.",
                        module.name
                    );
                }
                true
            }
            // The conventional spelling, yielded to a model that declares `m` itself.
            "m" => !declares("m"),
            _ => false,
        };
        if is_multiplicity {
            if !v.is_finite() || *v <= 0.0 {
                bail!(
                    "instance multiplicity must be a positive, finite number, got `{name}={v}`                      (it is a count of identical devices in parallel; LRM 6.3.6)"
                );
            }
            multiplicity = Some(*v);
        } else {
            rest.push((name.clone(), *v));
        }
    }
    Ok((multiplicity, rest))
}

fn build_from_model(
    module: &Module,
    value: Option<f64>,
    overrides: &[(String, f64)],
    terminals: &[usize],
    next_unknown: &mut usize,
) -> Result<(Box<dyn ModelInstance>, NodeAssignment)> {
    let mut m = module.clone();
    // § instance multiplicity. Split out of the override list *before* anything is applied, so
    // the rest of this function keeps seeing only real model parameters.
    let (multiplicity, overrides) = split_multiplicity(module, overrides)?;
    m.mfactor = multiplicity;
    // § `$param_given`: setting a parameter *is* giving it, so every override recorded below
    // also marks givenness on the clone. A deck that writes `Is=1e-15` and a model that asks
    // `$param_given(Is)` must agree, which they did not while the query folded to `false` at
    // elaboration (see `va_ir::Module::given_params`).
    if let (Some(v), Some(param)) = (value, m.params.first_mut()) {
        param.default = v;
        m.mark_param_given(va_ir::ParamId(0));
    }
    // Named overrides are applied after the positional value, so a line that somehow states
    // both has the explicit name win over the implicit position. An unknown name is an error:
    // dropping it silently would leave a deck looking like it set something it did not, and
    // the whole reason to write `Is=1e-12` rather than rely on parameter order is to be sure.
    for (name, v) in &overrides {
        match m
            .params
            .iter_mut()
            .enumerate()
            .find(|(_, p)| p.name == *name)
        {
            Some((i, param)) => {
                let pid = va_ir::ParamId(i as u32);
                if module.param_is_local(pid) {
                    bail!(
                        "model `{}` declares `{name}` as a `localparam`, which a deck cannot                          set — a localparam is a module-internal constant (LRM 3.4.2)",
                        module.name
                    );
                }
                param.default = *v;
                m.mark_param_given(pid);
            }
            None => {
                let mut known: Vec<&str> = m.params.iter().map(|p| p.name.as_str()).collect();
                known.sort_unstable();
                bail!(
                    "model `{}` has no parameter `{name}` (it declares: {})",
                    m.name,
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                );
            }
        }
    }

    let port_nodes: Vec<NodeId> = m.ports.iter().flatten().copied().collect();
    if terminals.len() > port_nodes.len() {
        bail!(
            "model `{}` declares {} port node(s), device connects {}",
            m.name,
            port_nodes.len(),
            terminals.len()
        );
    }
    // § `$port_connected`: a deck line may stop short of the model's trailing ports, which is
    // SPICE's way of leaving an optional terminal (a self-heating `dt`) off — the model then
    // sees `$port_connected(dt) == 0` and takes its own no-external-network branch. Trailing
    // ports are unconnected; they still get a node below, just a floating one.
    //
    // Only for a port the model *queries*, though. An omitted terminal is also exactly what a
    // typo looks like, and silently floating a node the deck meant to wire is the worse
    // failure — so a model that never asks `$port_connected` about the port still gets the
    // wrong-terminal-count error. (The Verilog-A spelling `.dt()` needs no such guard: writing
    // an empty slot is explicit, not an omission.)
    //
    // Granularity is the *declared* port, not the flattened node: a vector port is connected
    // or not as a whole, so a terminal list ending mid-port is an error rather than a
    // half-connected bus.
    let mut covered = 0usize;
    let mut unconnected: Vec<usize> = Vec::new();
    for (i, port) in m.ports.iter().enumerate() {
        let end = covered + port.len();
        if terminals.len() >= end {
            covered = end;
            continue;
        }
        if terminals.len() > covered {
            bail!(
                "model `{}`: device connects {} terminal(s), which ends part-way through                  {}-wide port #{} — a port is connected as a whole or not at all",
                m.name,
                terminals.len(),
                port.len(),
                i + 1
            );
        }
        if !m.queries_port_connected(i) {
            bail!(
                "model `{}` declares {} port node(s), device connects {} — and port #{}                  (`{}`) is not optional: the model never asks `$port_connected` about it",
                m.name,
                port_nodes.len(),
                terminals.len(),
                i + 1,
                port
                    .first()
                    .map(|n| m.nodes[n.0 as usize].name.as_str())
                    .unwrap_or("?")
            );
        }
        unconnected.push(i);
    }
    for i in unconnected {
        m.mark_port_unconnected(i);
    }
    let mut assigned: Vec<Option<usize>> = vec![None; m.nodes.len()];
    for (nid, &g) in port_nodes.iter().zip(terminals) {
        assigned[nid.0 as usize] = Some(g);
    }
    // A module's own implicit reference node is the *circuit's* ground, not an internal net.
    // Verilog-A's reference node is global (LRM §3.6.3), so `V(out) <+ 3.0` inside a model
    // means 3 V with respect to the same ground the deck uses. Left to the loop below it would
    // be handed a fresh unknown connected to nothing, and the contribution would drive a
    // floating node: the model builds and reports 0 V, or a singular row — silently, either
    // way. Identified by name, which is what `va-frontend` interns for the shorthand and what
    // it aliases an explicit `ground` declaration onto; a node that is also a *port* is
    // excluded, since the deck wires that one itself.
    for (i, node) in m.nodes.iter().enumerate() {
        if node.name == "gnd" && assigned[i].is_none() {
            assigned[i] = Some(va_abi::reference::GROUND);
        }
    }
    let full: Vec<usize> = assigned
        .into_iter()
        .map(|slot| {
            slot.unwrap_or_else(|| {
                let g = *next_unknown;
                *next_unknown += 1;
                g
            })
        })
        .collect();

    // Pair every IR node with the global unknown it was just assigned, so reporting can name
    // that entry of the solution vector (§ quantity reporting). Built here rather than
    // re-derived by the reporting code, because `full` above *is* the assignment — a second
    // implementation of it would be free to drift, and a label silently attached to the wrong
    // unknown is worse than no label at all.
    let assignment: NodeAssignment = full
        .iter()
        .zip(&m.nodes)
        .map(|(&g, decl)| (g, decl.clone()))
        .collect();

    let inst = va_codegen::build_instance(&m, &full, next_unknown)
        .with_context(|| format!("generating instance for model `{}`", module.name))?;
    // § instance multiplicity, LRM 6.3.6. The scaling is applied *outside* the instance rather
    // than inside the generated model, which is what makes it one rule for every model and keeps
    // the instance's own internal unknowns per-device (see `va_abi::multiplicity`). At m = 1
    // `wrap` returns this very box, so a deck with no `m=` is bit-identical to one run before
    // multiplicity existed.
    Ok((va_abi::Multiplied::wrap(inst, m.multiplicity()), assignment))
}

/// Build a device instance from the hand-written `va-abi` reference primitives.
fn reference_instance(dev: &Device) -> Result<Box<dyn ModelInstance>> {
    let p = dev.terminals[0];
    let n = dev.terminals[1];
    let value = || {
        dev.value
            .with_context(|| format!("device `{}` needs a value", dev.name))
    };

    let inst: Box<dyn ModelInstance> = match dev.model.as_str() {
        "resistor" => Box::new(Resistor::new(p, n, value()?)),
        "capacitor" => Box::new(Capacitor::new(p, n, value()?)),
        "diode" => Box::new(Diode::new(p, n, 1e-14, 1.0, VT_NOMINAL)),
        // `Q<name> c b e bjt` (§ `va-netlist`'s `'Q'` arm) — terminals are `[c, b, e]`, SPICE's
        // own order, but `Bjt::new` takes `(b, c, e)`; fixed parameters match the ring
        // oscillator's own hand-built fixture (`va_transient::integrator`'s test module).
        "bjt" => {
            let &[c, b, e] = dev.terminals.as_slice() else {
                bail!(
                    "device `{}` (model `bjt`) needs exactly 3 terminals (c, b, e), found {}",
                    dev.name,
                    dev.terminals.len()
                );
            };
            Box::new(Bjt::new(b, c, e, 1e-15, 100.0, 1.0, VT_NOMINAL))
        }
        other => bail!(
            "device `{}` references unknown model `{other}` (no compiled `--model` matched, \
             and it is not a built-in primitive)",
            dev.name
        ),
    };
    Ok(inst)
}

/// Print the DC operating point: node voltages, then every branch current.
///
/// `currents` is [`branch_currents`]' own `(name, global index)` map, the same authority
/// [`report_ac`] uses. It is passed in rather than re-derived here because re-deriving it means
/// assuming *which* devices claim branch rows and in what order — an assumption that was true
/// when only `vsource` did, and became silently wrong once inductors and controlled sources
/// claimed them too. A deck declaring an inductor before its source then printed the
/// inductor's current under the source's name.
fn report(quantities: &[Quantity], x: &[f64]) {
    println!("DC operating point:");
    for q in quantities {
        if let Some(v) = x.get(q.index) {
            println!("  {}", q.render(*v));
        }
    }
}

/// Print a `.dc` sweep: one line per swept value, every node's voltage and source current —
/// the same per-point content [`report`] prints for a single operating point, repeated.
fn report_sweep(
    quantities: &[Quantity],
    sweep: &va_netlist::DcSweep,
    points: &[(f64, va_core::dc::OperatingPoint)],
) {
    println!(
        "DC sweep {} from {} to {} step {} ({} points):",
        sweep.source,
        sweep.start,
        sweep.stop,
        sweep.step,
        points.len()
    );
    for (value, op) in points {
        print!("  {}={value:.6}:", sweep.source);
        for q in quantities {
            if let Some(v) = op.x.get(q.index) {
                print!(" {}", q.render(*v).replace(" = ", "="));
            }
        }
        println!();
    }
}

/// Print the AC sweep: one line per frequency, every node's magnitude and phase, then every
/// source branch current's. Magnitude/phase rather than the raw real/imaginary parts, since that
/// is what a Bode reading of the result actually wants (`va-harness`'s golden comparison keeps
/// the complex values instead — nothing is lost, the two forms are equivalent).
///
/// `currents` is [`branch_currents`]' own `(name, global index)` map rather than this function
/// re-deriving indices by counting `vsource` devices the way [`report`]/[`report_sweep`] do — a
/// compiled Verilog-A model can claim internal unknowns of its own from the same counter (§
/// [`build_instances`]), so "one branch row per source, contiguously after the nodes" is only
/// true for a deck of pure primitives.
fn report_ac(quantities: &[Quantity], response: &va_acnoise::ac::AcResponse) {
    use va_acnoise::ac::{magnitude, phase};
    println!(
        "AC analysis ({} point(s), f={:e} to {:e} Hz):",
        response.f.len(),
        response.f.first().copied().unwrap_or(0.0),
        response.f.last().copied().unwrap_or(0.0)
    );
    for (f, x) in response.f.iter().zip(&response.x) {
        let cols: Vec<String> = quantities
            .iter()
            .filter_map(|q| {
                let z = *x.get(q.index)?;
                // The magnitude of a small-signal response carries the quantity's own unit; the
                // phase is an angle whatever the discipline, so it never takes one.
                let unit = if q.unit.is_empty() {
                    String::new()
                } else {
                    format!(" {}", q.unit)
                };
                Some(format!(
                    "{}={:.6e}{unit}∠{:.2}°",
                    q.label,
                    magnitude(z),
                    phase(z).to_degrees()
                ))
            })
            .collect();
        println!("  f={f:.6e}Hz  {}", cols.join("  "));
    }
}

/// Print the noise spectrum: one line per frequency, the output PSD in V²/Hz alongside the more
/// commonly-read amplitude density V/√Hz (just its square root — printed because datasheets and
/// noise plots are conventionally in nV/√Hz, not V²/Hz), then the band-integrated RMS total.
fn report_noise(
    net: &Netlist,
    quantities: &[Quantity],
    spectrum: &va_acnoise::noise::NoiseSpectrum,
) {
    let card = net.noise.as_ref();
    let output = card.map(|c| c.output.as_str()).unwrap_or("?");
    let source = card.map(|c| c.source.as_str()).unwrap_or("?");
    // A power spectral density is in (whatever the observed quantity is)² per hertz, so the
    // output node's own discipline sets every unit printed below — `V^2/Hz` is only right
    // because the output is usually a voltage. `rads/s^2/Hz` is what a mechanical output
    // would correctly read.
    let out_q = quantities.iter().find(|q| q.name == output);
    let out_label = out_q.map_or_else(|| format!("V({output})"), |q| q.label.clone());
    let u = bracket_unit(out_q.map_or("V", |q| q.unit.as_str()));
    // The input-referred spectrum is referred to the named source, so it carries *that*
    // quantity's unit rather than the output's — the two differ whenever the analysis crosses
    // disciplines (an electrical output driven by a mechanical input, say). Resolved through
    // the source device's own first terminal, falling back to the output's unit when the deck
    // names a source this circuit does not have.
    let src_u = net
        .devices
        .iter()
        .find(|d| d.name == source)
        .and_then(|d| d.terminals.first())
        .and_then(|&t| quantities.iter().find(|q| q.index == t && q.is_potential))
        .map_or_else(|| u.clone(), |q| bracket_unit(&q.unit));
    println!(
        "Noise analysis at {out_label} ({} point(s), f={:e} to {:e} Hz):",
        spectrum.f.len(),
        spectrum.f.first().copied().unwrap_or(0.0),
        spectrum.f.last().copied().unwrap_or(0.0)
    );
    for (i, (f, psd)) in spectrum.f.iter().zip(&spectrum.psd).enumerate() {
        // The input-referred column exists only when the card named a resolvable source.
        let referred = match spectrum.input_psd.get(i) {
            Some(inp) => format!("  Sin={inp:.6e} {src_u}^2/Hz"),
            None => String::new(),
        };
        println!(
            "  f={f:.6e}Hz  S={psd:.6e} {u}^2/Hz  ({:.6e} {u}/sqrt(Hz)){referred}",
            psd.sqrt()
        );
    }
    println!(
        "  total integrated output noise = {:.6e} {u} rms",
        spectrum.total
    );
    if !spectrum.input_psd.is_empty() {
        println!(
            "  total integrated input-referred noise (at {source}) = {:.6e} {src_u} rms",
            spectrum.input_total
        );
    }

    // Per-device breakdown, ordered loudest-first — the actionable form of "where is my noise
    // coming from?". Reported as each device's share of the band-integrated power, which is the
    // question a designer is usually asking; the per-frequency detail is in the table above.
    let contributors = noise_contributors(net, spectrum);
    if !contributors.is_empty() {
        let mut shares: Vec<(String, f64)> = contributors
            .iter()
            .map(|(name, series)| {
                // Integrate this device's own share on the same trapezoidal grid the totals use.
                let power: f64 = spectrum
                    .f
                    .windows(2)
                    .zip(series.windows(2))
                    .map(|(fw, sw)| 0.5 * (sw[0] + sw[1]) * (fw[1] - fw[0]))
                    .sum();
                (name.clone(), power.max(0.0))
            })
            .collect();
        shares.sort_by(|a, b| b.1.total_cmp(&a.1));
        let sum: f64 = shares.iter().map(|(_, p)| p).sum();
        println!("  per-device contribution to the integrated output noise:");
        for (name, power) in shares {
            let pct = if sum > 0.0 { 100.0 * power / sum } else { 0.0 };
            println!("    {name:<8} {:.6e} {u} rms  ({pct:5.1}%)", power.sqrt());
        }
    }
}

/// Print the transient waveform: one line per accepted timepoint, every node's voltage.
fn report_transient(quantities: &[Quantity], wf: &Waveform) {
    println!(
        "Transient analysis ({} points, t=0 to t={:e}s):",
        wf.t.len(),
        wf.t.last().copied().unwrap_or(0.0)
    );
    for (t, x) in wf.t.iter().zip(&wf.x) {
        let cols: Vec<String> = quantities
            .iter()
            .filter_map(|q| x.get(q.index).map(|v| q.render(*v).replace(" = ", "=")))
            .collect();
        println!("  t={t:.6e}s  {}", cols.join("  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::reference::GROUND;

    /// A library directory may hold models a deck never places, and a *transient* refusal
    /// (`absdelay`/`laplace_*` fold) must be about the models the deck uses — 2026-09-11,
    /// found by `circuits/microring_thermal.net` run against `--model models`, which also holds
    /// `laplace_lowpass.va`. Covers the transitive case too: a placed module that instantiates
    /// a sibling file's module reaches that file.
    #[test]
    fn transient_refusal_is_scoped_to_the_files_a_deck_reaches() {
        let dir = std::env::temp_dir().join("va_cli_refusal_scoped_to_reached_files_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, src: &str| {
            let p = dir.join(name);
            std::fs::write(&p, src).unwrap();
            p.display().to_string()
        };
        let plain = write(
            "plain.va",
            "module plain(p, n); electrical p, n; analog I(p, n) <+ V(p, n) / 1000.0; endmodule",
        );
        let wrapper = write(
            "wrapper.va",
            "module wrapper(a, b); electrical a, b; plain inner(a, b); endmodule",
        );
        let folded = write(
            "folded.va",
            "module folded(p, n); electrical p, n; \
             analog I(p, n) <+ absdelay(V(p, n), 1e-6); endmodule",
        );
        let (_, files) = compile_model_library(&dir.display().to_string()).expect("compiles");
        assert_eq!(files.len(), 3);

        // A deck placing only the wrapper reaches wrapper.va and, through its instance,
        // plain.va — and not folded.va.
        let deck =
            va_netlist::parser::parse("X1 a gnd wrapper\nV1 a gnd DC 1\n.tran 1u 10u\n.end\n")
                .unwrap();
        let mut reached = files_reached_by(&deck, &files);
        reached.sort();
        let mut expected = vec![plain.clone(), wrapper.clone()];
        expected.sort();
        assert_eq!(
            reached, expected,
            "wrapper reaches plain transitively, never folded"
        );

        // The refusal, restricted the way `run_sim` restricts it, passes for that deck ...
        let sources = |reached: &[String]| -> Vec<(String, String)> {
            model_sources(Some(&dir.display().to_string()))
                .into_iter()
                .filter(|(p, _)| reached.contains(p))
                .collect()
        };
        refuse_transient_approximations(&sources(&reached))
            .expect("a deck that never places the folded model is not refused");

        // ... and still refuses a deck that does place the folded model.
        let deck2 =
            va_netlist::parser::parse("X1 a gnd folded\nV1 a gnd DC 1\n.tran 1u 10u\n.end\n")
                .unwrap();
        let reached2 = files_reached_by(&deck2, &files);
        assert_eq!(reached2, vec![folded.clone()]);
        let err = refuse_transient_approximations(&sources(&reached2))
            .expect_err("placing the folded model is still refused");
        assert!(err.to_string().contains("absdelay"), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A potential below 1e-3 in magnitude prints in scientific notation: `0.000002 m` is not
    /// a wavelength anyone can check a model against (2026-09-11). Volt-scale values and exact
    /// zero keep the fixed format every existing report uses.
    #[test]
    fn a_small_potential_is_printed_in_scientific_notation() {
        let q = |label: &str, unit: &str, is_potential: bool| Quantity {
            label: label.to_string(),
            unit: unit.to_string(),
            index: 0,
            name: label.to_string(),
            is_potential,
        };
        assert_eq!(q("V(in)", "V", true).render(0.5), "V(in) = 0.500000 V");
        assert_eq!(q("V(in)", "V", true).render(0.0), "V(in) = 0.000000 V");
        assert_eq!(q("V(in)", "V", true).render(-1e-3), "V(in) = -0.001000 V");
        assert_eq!(
            q("Wl(wl)", "m", true).render(1.5505e-6),
            "Wl(wl) = 1.550500e-6 m"
        );
        assert_eq!(
            q("Popt(drop)", "W", true).render(-3e-6),
            "Popt(drop) = -3.000000e-6 W"
        );
        // Flows were always scientific and stay so.
        assert_eq!(q("I(V1)", "A", false).render(0.5), "I(V1) = 5.000000e-1 A");
    }

    /// A model that divides by a probe reads `0/0` on Newton's first (zero-vector) iteration.
    /// That must reach the user as `NonFinite`, naming the unknown whose equation went bad,
    /// not as "singular matrix" (2026-09-11 — `models/microring.va` before its guard).
    #[test]
    fn a_model_dividing_by_a_zero_probe_is_reported_as_non_finite_with_the_unknown_named() {
        let dir = std::env::temp_dir().join("va_cli_non_finite_names_unknown_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("divider.va"),
            "module divider(p, n); electrical p, n; \
             analog I(p, n) <+ 1.0 / V(p, n); endmodule",
        )
        .unwrap();
        let deck_path = dir.join("d.net");
        std::fs::write(&deck_path, "V1 a gnd DC 1\nX1 a gnd divider\n.op\n.end\n").unwrap();

        let (net, compiled) = load(
            &deck_path.display().to_string(),
            Some(&dir.display().to_string()),
        )
        .unwrap();
        let err = solve_dc(&net, &compiled).expect_err("1/V(a) at V(a)=0 is not finite");
        let text = format!("{err:#}");
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(text.contains("non-finite"), "{text}");
        assert!(
            !text.contains("singular matrix"),
            "must not be misreported as singular: {text}"
        );
        assert!(
            text.contains("`V(a)`"),
            "names the unknown whose equation went bad: {text}"
        );
    }

    /// `models/`, as an include-path root — every model there `include`s `disciplines.vams`
    /// and `constants.vams` from alongside itself, exactly as the real pipeline resolves them
    /// (`va_cli::load` passes the model file's own directory). A test compiling a model with
    /// bare `va_frontend::compile` would fail on an undefined `` `P_K ``/`` `P_Q `` macro.
    fn models_dir() -> Vec<std::path::PathBuf> {
        vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))]
    }

    /// Compile a `models/*.va` source the way the real pipeline does.
    fn compile_model(src: &str, what: &str) -> va_frontend::CompiledDesign {
        va_frontend::compile_with_includes(src, &models_dir())
            .unwrap_or_else(|e| panic!("compile {what}: {e}"))
    }

    #[test]
    fn analysis_default_is_dc() {
        assert_eq!(Analysis::default(), Analysis::Dc);
    }

    /// Tier B end to end: a compiled model whose output is `slew`-limited must follow the
    /// closed-form ramp `min(target, rate·t)`, not its input.
    ///
    /// The circuit is deliberately algebraic (a current source into a resistor, nothing
    /// reactive), so every point is an exact solve and any deviation is the state channel's
    /// fault rather than integration error. This is the test that would catch the channel's two
    /// characteristic bugs: committing state on a *rejected* candidate step (the ramp would run
    /// fast, since rejected attempts would advance history) and reading `prev` from the wrong
    /// timepoint (the slope would be wrong).
    #[test]
    fn a_slew_limited_model_follows_the_rate_limit_not_its_input() {
        // I(p,n) <+ slew(K*$abstime, rate) with K = 10 A/s and rate = 1 A/s. The *input* ramps
        // ten times faster than the limiter allows, so the output must follow `rate*t`, not
        // `K*t` — a factor of ten apart, which is what makes this discriminating rather than
        // merely consistent. Into 1 kΩ: V(out) = -1000*I.
        let rate = 1.0; // A/s, the limit
        let design = compile_model(
            "module slewramp(p, n); electrical p, n; \
             parameter real k = 10.0; parameter real rate = 1.0; \
             analog I(p, n) <+ slew(k * $abstime, rate); endmodule",
            "slewramp",
        );
        let module = design.modules.first().expect("one module").clone();

        let mut next = 1usize;
        let dev = va_codegen::build_instance(&module, &[0, GROUND], &mut next).expect("builds");
        let r = va_abi::reference::Resistor::new(0, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 2] = [dev.as_ref(), &r];

        // DC: `$abstime` is 0 and the limiter settles to its input, so the whole thing is 0 V.
        // The static answer is unmoved by Tier B, which is the point of `is_initial_step`.
        let op = operating_point(&insts, 1, NewtonConfig::default()).expect("DC solves");
        assert!(op.x[0].abs() < 1e-12, "DC should be 0 V: {}", op.x[0]);

        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 1e-3,
            tstep: 2e-5,
            tstep_min: 1e-12,
            method: Method::Trapezoidal,
            lte_reltol: 1e-6,
            lte_abstol: 1e-12,
            lte_estimator: LteEstimator::DividedDifference,
        };
        let wf = va_transient::integrator::run(&insts, 1, vec![0.0], cfg).expect("integrates");
        assert!(wf.t.len() > 10, "expected many points: {}", wf.t.len());

        for (&t, x) in wf.t.iter().zip(&wf.x) {
            let expected = -rate * t * 1000.0; // rate-limited, NOT k*t
            assert!(
                (x[0] - expected).abs() < 1e-9,
                "at t={t}: {} vs rate-limited {expected} (unlimited would be {})",
                x[0],
                -10.0 * t * 1000.0
            );
        }
        // And it genuinely moved: at 1 ms the limiter has reached 1 mA, a tenth of its input.
        assert!(
            (wf.x.last().unwrap()[0] + 1.0).abs() < 1e-9,
            "final {}",
            wf.x.last().unwrap()[0]
        );
    }

    /// § `$limit`'s model-declared junctions, end to end and in the "fails one way, succeeds
    /// the other" shape: the *same* model, with and without the `$limit`, must solve and fail
    /// respectively.
    ///
    /// The model deliberately mixes the two kinds of node a compact device has — an exponential
    /// junction (`a`,`c`) and a plain linear pair (`p`,`n`) standing in for the external
    /// terminals behind a series resistance — and the linear pair is driven to 100 V. Without a
    /// declaration `va-codegen` can only guess from the presence of `exp`, and its guess is
    /// per-module: every node is a junction, so `limit_junction`'s logarithmic clamp compresses
    /// the 100 V node's step to roughly `vt·ln(...)` and Newton's iteration budget runs out —
    /// exactly the throttling `va-core`'s own `junction_limiting_no_longer_throttles_a_linear_
    /// circuit` demonstrates. `$limit(V(a,c), …)` names the junction authoritatively, and the
    /// clamp then lands on `a` alone.
    ///
    /// So this pins both directions at once: the declaration is honoured *and* it is honoured
    /// exclusively — the diode half still converges through the clamp it asked for.
    #[test]
    fn a_model_declared_junction_limits_that_node_and_leaves_the_others_alone() {
        // `p`/`n`: 1 kΩ driven at 100 V. `a`/`c`: a diode, fed from the same 100 V rail through
        // an external 100 kΩ, so it sits at a genuine forward operating point.
        const SRC: &str = "module mixed(a, c, p, n); electrical a, c, p, n; \
             parameter real Is = 1e-15; parameter real vt = 0.025852; \
             parameter real R = 1000.0; real vd; \
             analog begin \
               vd = LIMIT; \
               I(a, c) <+ Is * (exp(vd / vt) - 1.0); \
               I(p, n) <+ V(p, n) / R; \
             end endmodule";
        let declared = SRC.replace("LIMIT", r#"$limit(V(a, c), "pnjlim", vt, 0.6145)"#);
        let guessed = SRC.replace("LIMIT", "V(a, c)");

        // Unknown 0 = the 100 V rail, 1 = the diode anode, 2 = the source's branch current.
        let build = |src: &str| {
            let design = compile_model(src, "mixed");
            let module = design.modules.first().expect("one module").clone();
            let mut next = 3usize;
            va_codegen::build_instance(&module, &[1, GROUND, 0, GROUND], &mut next).expect("builds")
        };

        let dev = build(&declared);
        assert!(dev.unknown_is_junction(0), "`a` was declared a junction");
        assert!(
            !dev.unknown_is_junction(2),
            "`p` was not declared one, and is the node the blanket guess used to throttle"
        );

        let vs = va_abi::reference::VSource::new(0, GROUND, 2, 100.0);
        let feed = va_abi::reference::Resistor::new(0, 1, 100e3);
        let insts: [&dyn ModelInstance; 3] = [dev.as_ref(), &vs, &feed];
        let op = operating_point(&insts, 3, NewtonConfig::default())
            .expect("the declared junction leaves the 100 V node unlimited, so this converges");
        assert!(
            (op.x[0] - 100.0).abs() < 1e-9,
            "the rail must reach 100 V: {}",
            op.x[0]
        );
        assert!(
            (0.4..1.0).contains(&op.x[1]),
            "the diode must sit at a forward drop: {}",
            op.x[1]
        );

        // The identical circuit with the declaration removed: every node is guessed to be a
        // junction, and the rail cannot be walked to 100 V inside the iteration budget.
        let plain = build(&guessed);
        assert!(
            plain.unknown_is_junction(2),
            "without a `$limit` the guess is per-module, so `p` is limited too -- if that              stopped being true this test no longer discriminates"
        );
        let insts: [&dyn ModelInstance; 3] = [plain.as_ref(), &vs, &feed];
        let throttled = operating_point(&insts, 3, NewtonConfig::default());
        assert!(
            throttled.is_err(),
            "the blanket guess was expected to throttle this to a non-convergence; it returned              {throttled:?}"
        );
    }

    /// The whole Tier A pipeline, from Verilog-A source to a solved waveform: a model whose
    /// current is a function of `$abstime` must produce a genuine ramp in transient and its
    /// `t = 0` value at a DC operating point.
    ///
    /// This is the end-to-end statement of the fold that used to be wrong. `va-frontend` folded
    /// `$abstime` to `0.0` at elaboration, so this model was a plain resistor at every timepoint
    /// and the waveform below would have been flat.
    #[test]
    fn a_model_reading_abstime_ramps_in_transient_and_reads_zero_in_dc() {
        // I(p,n) <+ V(p,n)/R + k*$abstime — a 1 mA/ms current ramp in parallel with 1 kΩ.
        let design = compile_model(
            "module ramp(p, n); electrical p, n; \
             parameter real r = 1000; parameter real k = 1.0; \
             analog I(p, n) <+ V(p, n) / r + k * $abstime; endmodule",
            "ramp",
        );
        let module = design.modules.first().expect("one module").clone();

        // The ramp source alone from node 0 to ground: V(0) = -k·t·R.
        let (k, r) = (1.0, 1000.0);
        let inst = va_codegen::build_instance(&module, &[0, GROUND], &mut 1).expect("builds");
        let insts: [&dyn ModelInstance; 1] = [inst.as_ref()];

        // DC: `$abstime` reads zero, so the ramp term vanishes and only V/R remains — a lone
        // resistor to ground, which sits at 0 V.
        let op = operating_point(&insts, 1, NewtonConfig::default()).expect("DC solves");
        assert!(op.x[0].abs() < 1e-12, "DC point should be 0 V: {}", op.x[0]);

        // Transient: V(t) = -k·t·R, checked at every accepted point against the closed form.
        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 1e-3,
            tstep: 5e-5,
            tstep_min: 1e-12,
            method: Method::Trapezoidal,
            lte_reltol: 1e-6,
            lte_abstol: 1e-9,
            lte_estimator: LteEstimator::DividedDifference,
        };
        let wf = va_transient::integrator::run(&insts, 1, vec![0.0], cfg).expect("integrates");
        assert!(wf.t.len() > 10, "expected many points: {}", wf.t.len());
        for (&t, x) in wf.t.iter().zip(&wf.x) {
            let expected = -k * t * r;
            assert!(
                (x[0] - expected).abs() < 1e-9,
                "at t={t}: {} vs {expected}",
                x[0]
            );
        }
        // And it genuinely moved, rather than passing by staying at zero.
        assert!(
            (wf.x.last().unwrap()[0] + k * 1e-3 * r).abs() < 1e-9,
            "final {}",
            wf.x.last().unwrap()[0]
        );
    }

    /// `analysis()` selects a different device in DC than in transient, through the real
    /// pipeline. Each half has an independently-known answer, which is how this gets validated
    /// without a single QSPICE construct corresponding to the model as a whole.
    #[test]
    fn a_model_branching_on_analysis_solves_differently_in_dc_and_transient() {
        // A 1 kΩ resistor in DC; the same resistor plus a 1 mA offset in transient.
        let design = compile_model(
            r#"module gated(p, n); electrical p, n;
               analog begin
                 I(p, n) <+ V(p, n) / 1000.0;
                 if (analysis("tran")) I(p, n) <+ 1e-3;
               end
               endmodule"#,
            "gated",
        );
        let module = design.modules.first().expect("one module").clone();

        // Drive it from a 0 V source so the branch current reads the device's own current.
        let build = || {
            let mut next = 1usize;
            let dev = va_codegen::build_instance(&module, &[0, GROUND], &mut next).expect("builds");
            let src = VSource::new(0, GROUND, next, 0.0);
            (dev, src, next + 1)
        };

        let (dev, src, dim) = build();
        let insts: [&dyn ModelInstance; 2] = [dev.as_ref(), &src];

        // DC: the source holds node 0 at 0 V, so the resistor carries nothing.
        let op = operating_point(&insts, dim, NewtonConfig::default()).expect("DC solves");
        assert!(op.x[1].abs() < 1e-12, "DC source current: {}", op.x[1]);

        // Transient: the offset branch now fires, and the source must sink exactly that 1 mA.
        let cfg = TranConfig {
            tstart: 0.0,
            tstop: 1e-5,
            tstep: 1e-6,
            tstep_min: 1e-12,
            method: Method::Trapezoidal,
            lte_reltol: 1e-6,
            lte_abstol: 1e-9,
            lte_estimator: LteEstimator::DividedDifference,
        };
        let wf =
            va_transient::integrator::run(&insts, dim, vec![0.0, 0.0], cfg).expect("integrates");
        for x in wf.x.iter().skip(1) {
            assert!(
                (x[1] + 1e-3).abs() < 1e-9,
                "transient source current should be -1 mA, got {}",
                x[1]
            );
        }
    }

    #[test]
    fn check_group_resolves_cross_file_instantiation() {
        // `check_models`'s directory scan must let `top.va`'s `leg l1(a, b);` instance resolve
        // against `leg`, declared in a *separate* sibling file — the real corpus shape
        // (`external/photonic/Attenuator.va` instantiating `Polar2Cartesian`, declared in the
        // sibling `Polar2Cartesian.va`) plain per-file elaboration can't see.
        let dir = std::env::temp_dir().join("va_cli_check_group_cross_file_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let leg_path = dir.join("leg.va");
        let top_path = dir.join("top.va");
        std::fs::write(
            &leg_path,
            "module leg(p, n); electrical p, n; parameter real r = 1000; \
             analog I(p, n) <+ V(p, n) / r; endmodule",
        )
        .unwrap();
        std::fs::write(
            &top_path,
            "module top(a, b); electrical a, b; leg l1(a, b); endmodule",
        )
        .unwrap();

        let group = vec![
            (leg_path.to_string_lossy().into_owned(), dir.clone()),
            (top_path.to_string_lossy().into_owned(), dir.clone()),
        ];
        let passed = check_group(&group, false).passed;
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            passed, 2,
            "both leg.va and top.va must elaborate cleanly, top.va's instance resolved \
             against leg.va's module"
        );
    }

    #[test]
    fn check_group_does_not_resolve_an_instance_missing_from_its_own_group() {
        // A negative control for `check_group_resolves_cross_file_instantiation`: `top.va`
        // alone (its sibling `leg.va` withheld from the group entirely) must still fail to
        // resolve `leg l1(a, b);`, confirming the positive test's success comes from the shared
        // group and not from some other, broader lookup.
        let dir = std::env::temp_dir().join("va_cli_check_group_missing_sibling_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let top_path = dir.join("top.va");
        std::fs::write(
            &top_path,
            "module top(a, b); electrical a, b; leg l1(a, b); endmodule",
        )
        .unwrap();

        let group = vec![(top_path.to_string_lossy().into_owned(), dir.clone())];
        let passed = check_group(&group, false).passed;
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            passed, 0,
            "top.va's `leg` instance must not resolve with no leg.va present"
        );
    }

    #[test]
    fn check_group_codegen_flag_is_a_strictly_later_stage() {
        // The `--codegen` verdict must be a *superset* of the frontend one: a module that
        // elaborates but that `va-codegen` rejects has to count as passed without the flag and
        // failed with it. Without this, the two corpus figures could silently be the same
        // measurement under two names.
        let dir = std::env::temp_dir().join("va_cli_check_group_codegen_stage_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nested_ddt.va");
        // A *second* time derivative: `va_abi::StampSink` has exactly one charge channel, so
        // `ddt(ddt(x))` cannot be expressed and codegen refuses it (`ad::Dual::into_ddt`),
        // while the frontend elaborates it fine — exactly the frontend/codegen gap this flag
        // exists to measure. (A plain nested `ddt` no longer works as the fixture here: it has
        // been supported since `Dual` gained its charge channel.)
        std::fs::write(
            &path,
            "module m(p, n); electrical p, n; parameter real c = 1e-12; \
             analog I(p, n) <+ ddt(ddt(c * V(p, n))); endmodule",
        )
        .unwrap();

        let group = vec![(path.to_string_lossy().into_owned(), dir.clone())];
        let frontend_only = check_group(&group, false).passed;
        let with_codegen = check_group(&group, true).passed;
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(frontend_only, 1, "the module elaborates");
        assert_eq!(
            with_codegen, 0,
            "but va-codegen rejects the second time derivative"
        );
    }

    // --- an analog operator reaching a contribution through a variable ---------------------
    //
    // `lower`'s `contains_noise_call`/`contains_ac_stim_call`/`contains_ddt_call` establish
    // *silent-drop* safety properties. They were purely syntactic, with no `Expr::Var` arm, so
    // one assignment defeated all three. A taint fixed point over the analog block now closes
    // that; these pin each family, each with its own control.

    fn build_msg(src: &str) -> Result<(), String> {
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let mut next = 2usize;
        match va_codegen::build_instance(&design.modules[0], &[0, GROUND], &mut next) {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// **The most serious of the three, because it was silent.** `2.0*white_noise(...)` written
    /// directly is refused — the PSD scales as the square of any factor around it, so a scaled
    /// noise call cannot be pulled out and would contribute nothing. Written through a variable
    /// it *built*, and the source contributed exactly zero with no diagnostic: measured on a
    /// divider as 4.14e-18 V²/Hz against a true 8.28e-18, the per-device breakdown reporting one
    /// contributor where there were two.
    #[test]
    fn a_noise_source_reaching_a_contribution_through_a_variable_is_refused() {
        let msg = build_msg(
            "module nz(p, n);
             electrical p, n;
             parameter real g = 1e-3;
             real n1;
             analog begin
               n1 = white_noise(4.0e-21 * g, \"thermal\");
               I(p, n) <+ g * V(p, n) + 2.0 * n1;
             end
             endmodule
",
        )
        .expect_err("a scaled noise source must be refused, not silently dropped");
        assert!(msg.contains("top-level additive term"), "got: {msg}");
    }

    /// The control for it: the same source contributed *directly* as a top-level term still
    /// builds. Without this, refusing every `white_noise` would pass the test above.
    #[test]
    fn a_top_level_noise_source_still_builds() {
        build_msg(
            "module nz(p, n);
             electrical p, n;
             parameter real g = 1e-3;
             analog begin
               I(p, n) <+ g * V(p, n) + white_noise(4.0e-21 * g, \"thermal\");
             end
             endmodule
",
        )
        .expect("a top-level noise source is the supported spelling");
    }

    /// Same hole, `ac_stim` family: its value is zero in every analysis and only the split-out
    /// excitation channel carries it, so a nested one contributes nothing.
    #[test]
    fn an_ac_stim_reaching_a_contribution_through_a_variable_is_refused() {
        let msg = build_msg(
            "module stim(p, n);
             electrical p, n;
             real s;
             analog begin
               s = ac_stim(\"ac\", 1.0, 0.0);
               I(p, n) <+ 1e-3 * V(p, n) + 2.0 * s;
             end
             endmodule
",
        )
        .expect_err("a scaled ac_stim must be refused");
        assert!(msg.contains("top-level additive term"), "got: {msg}");
    }

    /// Same hole, `ddt` family: a charge term whose argument itself depends on a time derivative
    /// is a *second* derivative, which this project's single charge channel cannot express. The
    /// direct spelling was rejected; through a variable it built and then hit a `debug_assert`
    /// mid-solve, or in release silently dropped the sensitivity.
    ///
    /// The variable must hold a shape `charge_term_shape` *rejects* — here the coefficient is a
    /// node voltage, so it is not parameter-only — otherwise the assignment is folded into the
    /// charge channel and never becomes an ordinary read at all.
    #[test]
    fn a_second_derivative_reaching_the_charge_channel_through_a_variable_is_refused() {
        let msg = build_msg(
            "module sd(p, n);
             electrical p, n;
             parameter real c0 = 1e-6;
             real x;
             analog begin
               x = V(p, n) * ddt(c0 * V(p, n));
               I(p, n) <+ ddt(c0 * x);
               I(p, n) <+ V(p, n) * 1e-3;
             end
             endmodule
",
        )
        .expect_err("a second time derivative must be refused");
        assert!(msg.contains("second time derivative"), "got: {msg}");
    }

    /// The control for the taint itself: a variable that never touches an analog operator must
    /// not be tainted, so ordinary variable-carried arithmetic keeps working. Without this, a
    /// taint set that marked everything would pass all three tests above.
    #[test]
    fn an_ordinary_variable_is_not_tainted() {
        build_msg(
            "module plain(p, n);
             electrical p, n;
             parameter real g = 1e-3;
             real y;
             analog begin
               y = 2.0 * g;
               I(p, n) <+ y * V(p, n) + white_noise(4.0e-21 * g, \"thermal\");
             end
             endmodule
",
        )
        .expect("an operator-free variable must stay untainted");
    }

    /// `hicumL0_v2p1p0.va`'s self-heating idiom: a `ddt` assigned to a local variable inside
    /// one arm of an `if`, then contributed by a **later, separate** statement.
    ///
    /// `DdtVars` is forward and single-pass, so the `i_cth -> ddt(...)` binding is gone by the
    /// time `I(p,n) <+ i_cth;` is reached. This used to lower as an ordinary resistive read and
    /// *compile*, silently stamping no charge at all — the device's whole thermal capacitance
    /// vanishing with no diagnostic. It was then refused outright. Since the guards here are
    /// **parameter-only**, it is now lowered instead: the assignment is emitted, the variable
    /// carries the rate, and the contribution takes the product-rule path.
    ///
    /// Asserts the four things that distinguish that path from every neighbouring one — a test
    /// that only checked "it builds" would pass on the very bug this replaced:
    ///   * `dcharge == cth`   — the charge sensitivity is present, and is the right size;
    ///   * `charge == 0`      — it took the product-rule path, **not** the charge channel;
    ///   * `jacobian == 0`    — the coefficient here is constant, so there is no `(dq/dt)·∂c/∂x`;
    ///   * `residual` matches the reconstructed rate from real committed history.
    ///
    /// Run under a **transient** context with non-zero `ddt_coeff` and non-zero committed
    /// history on purpose: under DC `ddt_coeff` is zero, so the residual would be legitimately
    /// zero and a broken primal reconstruction would sail through.
    #[test]
    fn a_ddt_assigned_in_an_if_arm_under_parameter_guards_is_contributed_as_a_rate() {
        let src = "module thermal(p, n);
                   electrical p, n;
                   parameter real cth = 1e-9;
                   parameter real flsh = 1;
                   real i_cth;
                   analog begin
                     if (flsh == 0) begin
                       i_cth = 0.0;
                     end else begin
                       i_cth = ddt(cth * V(p, n));
                     end
                     I(p, n) <+ i_cth;
                   end
                   endmodule
";
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let mut next = 2usize;
        let inst = va_codegen::build_instance(&design.modules[0], &[0, GROUND], &mut next)
            .expect("a parameter-guarded escaping rate is supported");

        let (h, cth, v) = (1e-6_f64, 1e-9_f64, 0.4_f64);
        let coeff = 1.0 / h;
        let ctx = va_abi::AnalysisCtx::transient(1e-3)
            .with_ddt(coeff, 0.0)
            .with_initial_step(false);
        let q_prev = 3e-10_f64;
        let committed = vec![q_prev; inst.state_len()];
        let mut nxt = vec![0.0; committed.len()];
        let mut st = va_abi::ModelState::new(&committed, &mut nxt);
        let mut sink = va_abi::stamps::DenseStamp::new(1);
        inst.load(&[v], &ctx, &mut st, &mut sink);

        assert!(
            (sink.dcharge[0] - cth).abs() < 1e-18,
            "expected dcharge = cth = {cth:e}, got {:e} — the thermal capacitance is the whole              point of this shape",
            sink.dcharge[0]
        );
        assert_eq!(
            sink.charge[0], 0.0,
            "an escaping rate must NOT enter the charge channel: its value is already in the              residual, so a non-zero charge here would double-count it through the offset"
        );
        assert!(
            sink.jacobian[0].abs() < 1e-18,
            "the coefficient is a parameter, so there is no (dq/dt)*dc/dx half; got {:e}",
            sink.jacobian[0]
        );
        let expected = coeff * (cth * v - q_prev);
        assert!(
            (sink.residual[0] - expected).abs() < 1e-12 * expected.abs().max(1.0),
            "residual {:e} is not the reconstructed rate {expected:e} from committed history",
            sink.residual[0]
        );
    }

    /// The narrowed refusal must still bite: the same shape with a guard that **depends on the
    /// solution**. There the arm choice can flip between timepoints, so the `ddt` site is not
    /// evaluated every step and its committed history goes stale — an O(1) wrong rate with no
    /// diagnostic. LRM §4.5.15 forbids exactly this.
    #[test]
    fn an_escaping_ddt_under_a_solution_dependent_guard_is_still_refused() {
        let src = "module bad(p, n);
                   electrical p, n;
                   parameter real c0 = 1e-9;
                   real i_c;
                   analog begin
                     if (V(p, n) > 0.5) begin
                       i_c = ddt(c0 * V(p, n));
                     end else begin
                       i_c = 0.0;
                     end
                     I(p, n) <+ i_c;
                   end
                   endmodule
";
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let mut next = 2usize;
        let msg = match va_codegen::build_instance(&design.modules[0], &[0, GROUND], &mut next) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a solution-dependent guard must still be refused"),
        };
        assert!(msg.contains("constant for the whole run"), "got: {msg}");
    }

    /// And inside a loop body, which LRM §4.5.15 forbids outright regardless of the condition.
    ///
    /// **This one is now rejected a stage earlier, by the frontend** (v0.9.14): elaboration
    /// refuses *any* analog operator in a `repeat`/`while`/non-genvar `for`, not only one that
    /// escapes its arm through a variable, so the source never reaches `build_instance`. The
    /// codegen check below still guards the `if`-arm half of the same family — a
    /// solution-dependent guard is not a loop — and hand-built IR that never passed through the
    /// frontend; what changed is which pass gets to this particular shape first.
    #[test]
    fn an_escaping_ddt_inside_a_loop_body_is_still_refused() {
        let src = "module loopy(p, n);
                   electrical p, n;
                   parameter real c0 = 1e-9;
                   integer k;
                   real i_c;
                   analog begin
                     i_c = 0.0;
                     k = 0;
                     while (k < 1) begin
                       i_c = ddt(c0 * V(p, n));
                       k = k + 1;
                     end
                     I(p, n) <+ i_c;
                   end
                   endmodule
";
        let msg = match va_frontend::compile_with_includes(src, &[]) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a loop body must still be refused"),
        };
        assert!(
            msg.contains("`ddt`") && msg.contains("`while` loop") && msg.contains("§4.5.15"),
            "got: {msg}"
        );
    }

    /// The control: the *same* variable-indirection shape with the `ddt` assigned **outside**
    /// any branch still lowers, and still reaches the charge channel. Without this, the check
    /// above could be passing because all `ddt`-via-variable support had been broken.
    #[test]
    fn a_ddt_assigned_outside_any_branch_still_reaches_the_charge_channel() {
        let src = "module cap(p, n);
                   electrical p, n;
                   parameter real cth = 1e-9;
                   real i_cth;
                   analog begin
                     i_cth = ddt(cth * V(p, n));
                     I(p, n) <+ i_cth;
                   end
                   endmodule
";
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let mut next = 2usize;
        let inst = va_codegen::build_instance(&design.modules[0], &[0, GROUND], &mut next)
            .expect("builds");

        let mut sink = va_abi::stamps::DenseStamp::new(2);
        inst.load(
            &[1.0, 0.0],
            &va_abi::ANALYSIS_DC,
            &mut va_abi::ModelState::stateless(),
            &mut sink,
        );
        let cth = 1e-9;
        assert!(
            (sink.charge[0] - cth * 1.0).abs() / cth < 1e-9,
            "Q = cth*V must be stamped, got {}",
            sink.charge[0]
        );
        assert!(
            (sink.dcharge[0] - cth).abs() / cth < 1e-9,
            "and dQ/dV = cth"
        );
    }

    #[test]
    fn a_dropped_include_and_a_module_less_file_are_not_counted_as_clean_passes() {
        // The corpus defect this accounting exists for: a vendor model whose entire body lives
        // in a `` `include `` that was never shipped preprocesses to an empty module, which
        // elaborates perfectly and used to be scored as coverage — `bsimcmg.va` passed
        // `va-cli check` reporting 0 parameters. A file declaring no module at all passed for a
        // different reason: the "did every module elaborate?" loop had nothing to iterate.
        let dir = std::env::temp_dir().join("va_cli_check_tally_honesty_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. A whole, self-contained model.
        std::fs::write(
            dir.join("whole.va"),
            "module whole(p, n); electrical p, n; parameter real r = 1000;              analog I(p, n) <+ V(p, n) / r; endmodule",
        )
        .unwrap();
        // 2. The truncated-vendor-distribution shape: ports declared inline, body `include`d,
        //    include absent. This elaborates cleanly — and says nothing true about coverage.
        std::fs::write(
            dir.join("hollow.va"),
            "module hollow(p, n);
             electrical p, n;
             `include \"body_that_was_never_shipped.include\"
             endmodule
",
        )
        .unwrap();
        // 3. A header that declares no module (the `disciplines.vams`/fragment shape).
        std::fs::write(
            dir.join("header.va"),
            "`define SOME_MACRO 1
",
        )
        .unwrap();

        let group: Vec<(String, std::path::PathBuf)> = ["whole.va", "hollow.va", "header.va"]
            .iter()
            .map(|f| (dir.join(f).to_string_lossy().into_owned(), dir.clone()))
            .collect();
        let tally = check_group(&group, false);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(tally.failed, 0, "none of the three fails outright");
        assert_eq!(tally.no_module, 1, "header.va declares no module");
        assert_eq!(tally.passed, 2, "whole.va and hollow.va both elaborate");
        assert_eq!(
            tally.passed_incomplete, 1,
            "but only hollow.va's pass is on source its own `include left incomplete"
        );
    }

    /// A file that fails to *preprocess* because the `` `include `` holding its macro
    /// definitions never shipped is a truncated distribution, not a frontend gap. Until
    /// `preprocess_reporting` reported skipped includes on its error path, `failed_incomplete`
    /// was unreachable for a `[pp]` failure, so these could never be separated out the way an
    /// `[elab]` failure on the same defect already was — one defect, two verdicts again.
    #[test]
    fn a_preprocess_failure_on_a_dropped_include_is_counted_as_incomplete() {
        let dir = std::env::temp_dir().join("va_cli_pp_failure_incomplete_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The vendor shape: the absent include held `GMIN, which the surviving text then uses.
        std::fs::write(
            dir.join("truncated.va"),
            "`include \"macrodefs_that_never_shipped.include\"
             module truncated(p, n);
             electrical p, n;
             analog I(p, n) <+ `GMIN * V(p, n);
             endmodule
",
        )
        .unwrap();
        // Negative control: the same preprocess failure with no include to blame it on. This is
        // a real defect in the file and must stay in the self-contained denominator.
        std::fs::write(
            dir.join("genuinely_broken.va"),
            "module genuinely_broken(p, n);
             electrical p, n;
             analog I(p, n) <+ `NEVER_DEFINED * V(p, n);
             endmodule
",
        )
        .unwrap();

        let group: Vec<(String, std::path::PathBuf)> = ["truncated.va", "genuinely_broken.va"]
            .iter()
            .map(|f| (dir.join(f).to_string_lossy().into_owned(), dir.clone()))
            .collect();
        let tally = check_group(&group, false);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(tally.passed, 0);
        assert_eq!(tally.no_module, 0, "both files do declare a module");
        assert_eq!(tally.failed, 2, "both fail to preprocess");
        assert_eq!(
            tally.failed_incomplete, 1,
            "only the one whose `include vanished is a truncation rather than a gap"
        );
    }

    /// A statement-body fragment that fails to preprocess or parse is not a failing *model* —
    /// counting it as one understates coverage exactly as counting it as a pass used to
    /// overstate it. `cannot_declare_a_module` moves it out of the denominator, but only where
    /// it can prove the file has no module: no `module` bytes anywhere, and no `` `include ``
    /// that could bring one in.
    #[test]
    fn a_provably_module_less_fragment_is_not_a_failing_model() {
        let dir = std::env::temp_dir().join("va_cli_module_less_fragment_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The `ekv3_*` shape: an include fragment whose token stream opens mid-statement. It
        // cannot parse standalone, and that says nothing about frontend coverage.
        std::fs::write(
            dir.join("fragment.va"),
            "begin\n  real x;\n  x = 1.0;\nend\n",
        )
        .unwrap();
        // Negative control 1: same un-parseable shape, but it *does* contain the keyword, so
        // the proof fails and it stays a failing model.
        std::fs::write(
            dir.join("has_keyword.va"),
            "begin\n  // this fragment belongs to a module\n  real x;\nend\n",
        )
        .unwrap();
        // Negative control 2: no keyword, but an `` `include `` could carry one in — and when a
        // file dies at the preprocess stage we cannot know what it would have expanded to.
        std::fs::write(
            dir.join("has_include.va"),
            "`include \"never_shipped.include\"\nbegin\n  real x;\nend\n",
        )
        .unwrap();

        let group: Vec<(String, std::path::PathBuf)> =
            ["fragment.va", "has_keyword.va", "has_include.va"]
                .iter()
                .map(|f| (dir.join(f).to_string_lossy().into_owned(), dir.clone()))
                .collect();
        let tally = check_group(&group, false);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(tally.passed, 0, "none of the three is a working model");
        assert_eq!(
            tally.no_module, 1,
            "only fragment.va can be proved to declare no module"
        );
        assert_eq!(
            tally.failed, 2,
            "the two controls stay in the failure count — the proof is one-directional"
        );
    }

    /// The proof is deliberately crude in the safe direction: `endmodule` contains `module`, so
    /// a file carrying only that still counts as possibly-a-model. A wrong "declares no module"
    /// would hide a real gap; a wrong "might" only understates coverage.
    #[test]
    fn the_module_less_proof_is_one_directional() {
        let dir = std::env::temp_dir().join("va_cli_module_less_proof_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bare.va"), "real x;\n").unwrap();
        std::fs::write(dir.join("endmodule_only.va"), "real x;\nendmodule\n").unwrap();
        let bare = dir.join("bare.va").to_string_lossy().into_owned();
        let endm = dir.join("endmodule_only.va").to_string_lossy().into_owned();
        let (a, b) = (
            cannot_declare_a_module(&bare),
            cannot_declare_a_module(&endm),
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(a, "no `module` bytes and no `include: provably module-less");
        assert!(
            !b,
            "`endmodule` contains `module` — not proved, so not moved"
        );
    }

    /// End-to-end DC: parse the divider deck, build reference instances, solve.
    /// V(in) = 1 V, V(mid) = Vin·R2/(R1+R2) = 0.5 V.
    fn solve_divider(compiled: &[Module]) -> va_core::dc::OperatingPoint {
        let deck = include_str!("../../../circuits/divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse divider");
        solve_dc(&net, compiled).expect("solve divider")
    }

    /// `branch_currents` maps `divider.net`'s own `V1` to its assigned branch-current global
    /// index — `node_order.len()` (2: `in`, `mid`), since a `vsource`'s branch unknown is the
    /// first one claimed after every node. Solving confirms the real current through the
    /// series 1kΩ+1kΩ divider at `Vin=1V`: `I = 1V / 2000Ω = 0.5mA`, flowing *into* the source
    /// per `VSource`'s own stamp convention (`sink.residual(p, ib)`) — so the solved value is
    /// negative (current flows *out* of the source into the circuit).
    #[test]
    fn branch_currents_maps_the_divider_source_to_its_branch_index() {
        let deck = include_str!("../../../circuits/divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse divider");
        let currents = branch_currents(&net, &[]).expect("branch currents");
        assert_eq!(currents, vec![("V1".to_string(), 2)]);

        let op = solve_dc(&net, &[]).expect("solve divider");
        let i_v1 = op.x[2];
        assert!(
            (i_v1 - (-0.0005)).abs() < 1e-9,
            "I(V1) = {i_v1}, expected -0.5mA"
        );
    }

    /// End-to-end DC sweep (ladder rung 2): compile `models/diode.va` and sweep
    /// `circuits/diode_iv.net`'s `V1` from 0 to 0.6 V, checking every point against the
    /// closed-form Shockley diode law the model itself implements — `Id(V) =
    /// Is*(exp(V/(N*vt))-1)` — not just against the tool's own output.
    #[test]
    fn diode_iv_sweep_solves_through_codegen_pipeline() {
        let src = include_str!("../../../models/diode.va");
        let design = compile_model(src, "diode.va");
        assert_eq!(design.modules.len(), 1);
        assert_eq!(design.modules[0].name, "diode");

        let deck = include_str!("../../../circuits/diode_iv.net");
        let net = va_netlist::parser::parse(deck).expect("parse diode_iv");
        let sweep = net.dc.clone().expect("`.dc` sweep card");
        let points = solve_dc_sweep(&net, &design.modules, &sweep).expect("solve diode_iv sweep");
        assert_eq!(points.len(), 7); // 0.0, 0.1, ..., 0.6

        // diode.va's own defaults: Is = 1e-14 A, N = 1.0; va-codegen's default thermal voltage.
        let is = 1e-14_f64;
        let vt = va_codegen::VT;
        // node_order: ["in"] — V1's own branch-current unknown follows it at index 1.
        let in_idx = 0;
        let branch_idx = 1;
        for (v, op) in &points {
            assert!(
                (op.x[in_idx] - v).abs() < 1e-9,
                "V(in) = {} at V1={v}",
                op.x[in_idx]
            );
            let expected_id = is * ((v / vt).exp() - 1.0);
            // KCL at `in`: id (diode) + ib (source) = 0 (va-abi::VSource's own sign
            // convention — "current flows out of p and into n" internally), so I(V1) = -id.
            let i_v1 = op.x[branch_idx];
            let tol = 1e-9_f64.max(expected_id.abs() * 1e-6);
            assert!(
                (i_v1 - (-expected_id)).abs() < tol,
                "at V1={v}: I(V1)={i_v1}, expected {}",
                -expected_id
            );
        }
    }

    /// The transient-approximation warning fires on the operators this engine folds, stays
    /// quiet on models using none, and is driven by the *lexer* rather than a substring
    /// search -- so the word in a comment, or an identifier merely containing it, is ignored.
    /// That precision is only available because `absdelay` became a reserved word on
    /// A transient run **refuses** a model whose operator this engine only folds, rather than
    /// printing a waveform that looks computed. DC and AC on the same model stay fine, which is
    /// the whole reason this is analysis-scoped rather than a compile-time rejection.
    #[test]
    fn a_transient_run_refuses_an_approximated_operator() {
        let model = concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/delay_line.va");
        let src = std::fs::read_to_string(model).expect("read delay_line.va");
        let models = vec![(model.to_string(), src)];

        let err = refuse_transient_approximations(&models).expect_err("absdelay is folded");
        let msg = format!("{err:#}");
        assert!(msg.contains("absdelay"), "names the operator: {msg}");
        assert!(msg.contains("refused"), "says it refused: {msg}");
        assert!(
            msg.contains("--ac") && msg.contains("unaffected"),
            "points at the analyses that are still correct: {msg}"
        );

        // A model using none of them is not refused.
        let plain = vec![(
            "r.va".to_string(),
            "module r(p,n); electrical p,n; analog I(p,n) <+ V(p,n); endmodule".to_string(),
        )];
        refuse_transient_approximations(&plain).expect("an ordinary model is fine");

        // Nor, since v0.9.16, is a `laplace_*` filter: it is integrated, not folded. Until then
        // this very file was the refused example.
        let lowpass = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/laplace_lowpass.va"
        );
        let src = std::fs::read_to_string(lowpass).expect("read laplace_lowpass.va");
        refuse_transient_approximations(&[(lowpass.to_string(), src)])
            .expect("laplace_nd runs in transient now");
    }

    /// A `laplace_np` filter with a complex-conjugate pole pair, in transient, against the
    /// closed-form step response of the underdamped second-order system it is:
    /// `y = 1 − e^{−at}(cos bt + (a/b) sin bt)` for poles at `−a ± jb`. Exercises the root
    /// expansion, two state unknowns, the `ω0` scaling, and the integrator's LTE control on the
    /// state rows through the real pipeline. The discriminating point is the peak: the folded
    /// `H(0)` this construct used to give is a flat 1.0, the real response overshoots to
    /// `1 + e^{−aπ/b} ≈ 1.73`.
    #[test]
    fn a_laplace_np_filter_follows_the_underdamped_step_response_in_transient() {
        let src = "
`include \"disciplines.vams\"
module resonant(out, in, ref);
  inout out, in, ref;
  electrical out, in, ref;
  parameter real a = 2e3;
  parameter real b = 2e4;
  analog V(out, ref) <+ laplace_np(V(in, ref), {1}, {-a, b, -a, -b});
endmodule
";
        let design = compile_model(src, "resonant");
        let net = va_netlist::parser::parse(
            "V1 in gnd DC 1
M1 out in gnd resonant
.tran 1u 1.5m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("out node");
        let (a, b) = (2e3_f64, 2e4_f64);
        let mut worst = 0.0_f64;
        let mut peak = 0.0_f64;
        for (&t, row) in wf.t.iter().zip(&wf.x) {
            let exact = 1.0 - (-a * t).exp() * ((b * t).cos() + (a / b) * (b * t).sin());
            worst = worst.max((row[out] - exact).abs());
            peak = peak.max(row[out]);
        }
        assert!(worst < 5e-4, "max deviation from the closed form: {worst}");
        let expected_peak = 1.0 + (-a * std::f64::consts::PI / b).exp();
        assert!(
            (peak - expected_peak).abs() < 5e-3,
            "overshoot peak {peak} vs {expected_peak} (a fold to H(0) would give 1.0)"
        );
    }

    /// `@(cross(...))`'s body runs, and only when the event fires.
    ///
    /// The whole chain in one test, which is why it lives here rather than in `va-codegen`
    /// (which may not depend on `va-frontend`, CLAUDE.md §3): the parser turns the trigger into
    /// a guarded body, elaboration registers a `event_sites` entry and an `Expr::EventFired`
    /// guard, `va-codegen` reports the monitored expression through Interface β's event channel
    /// and reads the firing back out of `ModelState`.
    ///
    /// The model is a resistor whose conductance the body changes, so "did the body run" shows
    /// up in the stamp rather than in a flag: 1 mS normally, 1 S when the event fires.
    #[test]
    fn a_cross_body_runs_only_when_the_event_fires() {
        const SRC: &str = "
module xd(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - 2.5, 1)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let m = &design.modules[0];
        assert_eq!(m.event_sites.len(), 1, "one monitored site registered");
        assert!(
            matches!(m.event_sites[0], va_ir::EventSite::Cross { dir: 1, .. }),
            "a rising cross site, as written: {:?}",
            m.event_sites[0]
        );

        let mut next = 2;
        let inst = va_codegen::build_instance(m, &[0, 1], &mut next).expect("builds");
        assert_eq!(
            inst.event_count(),
            1,
            "one event slot reaches Interface beta"
        );

        // The registration reports the monitored *expression*, not the node voltage.
        let mut ev = va_abi::events::RecordingEventSink::new();
        inst.events(&[4.0, 0.0], &va_abi::ANALYSIS_DC, &mut ev);
        assert_eq!(
            ev.value_of(0),
            Some(1.5),
            "reports V(p,n) - 2.5 at V(p,n) = 4"
        );

        // Conductance with the event not fired, and with it fired.
        let g_of = |fired: &[bool]| -> f64 {
            let mut sink = va_abi::stamps::DenseStamp::new(2);
            let mut scratch = vec![0.0; inst.state_len()];
            let mut st = va_abi::ModelState::with_events(&[], &mut scratch, fired);
            inst.load(&[1.0, 0.0], &va_abi::ANALYSIS_DC, &mut st, &mut sink);
            sink.jacobian[0] // dI(p)/dV(p) = g
        };
        let quiet = g_of(&[false]);
        let fired = g_of(&[true]);
        assert!(
            (quiet - 1e-3).abs() < 1e-12,
            "body must not run when nothing fired, got g = {quiet}"
        );
        assert!(
            (fired - 1.0).abs() < 1e-12,
            "body must run when the event fired, got g = {fired}"
        );
    }

    /// A `cross` site inside an instantiated submodule keeps its identity through inlining: the
    /// parent's slot numbering shifts it, and the site list shifts with it. Two instances of
    /// one submodule must get *two* distinct slots, not one shared one — otherwise both
    /// instances' bodies would fire together.
    #[test]
    fn inlined_event_sites_get_distinct_slots() {
        const SRC: &str = "
module leaf(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p) - 1.0, 1)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
module top(a, b);
  inout a, b;
  electrical a, b;
  leaf L1 (a, b);
  leaf L2 (a, b);
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let top = design
            .modules
            .iter()
            .find(|m| m.name == "top")
            .expect("top module");
        assert_eq!(
            top.event_sites.len(),
            2,
            "each instance contributes its own monitored site"
        );
        let slots: Vec<u32> = top
            .exprs
            .iter()
            .filter_map(|e| match *e {
                va_ir::Expr::EventFired(k) => Some(k),
                _ => None,
            })
            .collect();
        assert_eq!(slots, vec![0, 1], "the second instance's guard is remapped");
    }

    /// The end of the chain: an `@(cross(...))` body fires during a **real transient run**,
    /// through the deck → frontend → codegen → Interface β → integrator path.
    ///
    /// This is the test that distinguishes "the pieces compile" from "the feature works". The
    /// model is a 1 mS resistor whose body switches it to 1 S, driven directly across a
    /// sinusoidal source so the monitored expression is source-driven and cannot be perturbed
    /// by the conductance it controls.
    ///
    /// One period of a 5 V sine crosses +2.5 V **twice** — once rising, once falling — so the
    /// discriminator is *when* each direction fires, not whether. Rising must fire at the first
    /// solution of `5·sin(2πft) = 2.5` (t = 1/12 of a period) and falling at the second
    /// (5/12 of a period). A registration that ignored direction would fire both at both times.
    /// A third case, a threshold the signal never reaches, is the control that shows the spike
    /// comes from the event at all.
    #[test]
    fn a_cross_body_fires_during_a_real_transient_run() {
        const PERIOD: f64 = 1e-3; // 1 kHz
        let run = |dir: i32, threshold: f64| -> (f64, Vec<f64>) {
            let src = format!(
                "
module xd(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - {threshold}, {dir})) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd SIN(0 5 1k)
X1 a gnd xd
.tran 2u 1m
.end
",
            )
            .expect("parses");
            let wf =
                solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
            let branch = net.node_order.len(); // I(V1) follows the node unknowns
            let peak =
                wf.x.iter()
                    .map(|row| row[branch].abs())
                    .fold(0.0f64, f64::max);
            (
                peak,
                wf.model_crossings.iter().map(|&(_, _, t)| t).collect(),
            )
        };

        // Rising: 5·sin(2πft) = 2.5 first at ft = 1/12.
        let (rising_peak, rising_times) = run(1, 2.5);
        assert_eq!(
            rising_times.len(),
            1,
            "one rising crossing: {rising_times:?}"
        );
        let expected_rise = PERIOD / 12.0;
        assert!(
            (rising_times[0] - expected_rise).abs() < 0.05 * PERIOD,
            "rising crossing at {:e}, expected ~{expected_rise:e}",
            rising_times[0]
        );
        assert!(
            rising_peak > 1.0,
            "the body must run and switch the conductance: peak |I(V1)| = {rising_peak:e}"
        );

        // Falling: the same level on the way back down, at ft = 5/12.
        let (falling_peak, falling_times) = run(-1, 2.5);
        assert_eq!(
            falling_times.len(),
            1,
            "one falling crossing: {falling_times:?}"
        );
        let expected_fall = 5.0 * PERIOD / 12.0;
        assert!(
            (falling_times[0] - expected_fall).abs() < 0.05 * PERIOD,
            "falling crossing at {:e}, expected ~{expected_fall:e}",
            falling_times[0]
        );
        assert!(falling_peak > 1.0, "the falling body must run too");

        // The two directions fire at genuinely different times — the discrimination that a
        // direction-blind implementation would fail.
        assert!(
            falling_times[0] - rising_times[0] > 0.25 * PERIOD,
            "rising and falling must be distinct events: {:e} vs {:e}",
            rising_times[0],
            falling_times[0]
        );

        // Control: a threshold the 5 V sine never reaches fires nothing, and the model stays at
        // its quiescent 1 mS.
        let (quiet_peak, quiet_times) = run(1, 7.5);
        assert!(
            quiet_times.is_empty(),
            "a threshold above the peak must not fire: {quiet_times:?}"
        );
        assert!(
            quiet_peak < 1e-2,
            "with nothing firing the model stays at 1 mS: peak |I(V1)| = {quiet_peak:e}"
        );
    }

    /// `@(timer(...))` fires at its scheduled times during a real transient run, and only
    /// there.
    ///
    /// A periodic timer is the case that would break a naive implementation two ways: firing
    /// once and never re-arming, or re-registering the same occurrence forever. So this checks
    /// the *count* over a known window and the *spacing* between firings, not merely that
    /// something happened.
    ///
    /// `timer(200u, 200u)` over a 1 ms run fires at 200, 400, 600, 800 and 1000 µs — five
    /// times. The last one counts: `tstop` is itself an accepted timepoint, so an occurrence
    /// landing exactly on it is solved like any other.
    #[test]
    fn a_periodic_timer_fires_at_each_scheduled_time() {
        const SRC: &str = "
module tk(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(timer(200u, 200u)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        assert_eq!(design.modules[0].event_sites.len(), 1, "one timer site");

        let net = va_netlist::parser::parse(
            "V1 a gnd DC 5
X1 a gnd tk
.tran 5u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");

        let times: Vec<f64> = wf.model_crossings.iter().map(|&(_, _, t)| t).collect();
        assert_eq!(
            times.len(),
            5,
            "200us period over a 1ms run fires at 200/400/600/800/1000us: {times:?}"
        );
        for (k, t) in times.iter().enumerate() {
            let expected = 200e-6 * (k as f64 + 1.0);
            assert!(
                (t - expected).abs() < 1e-9,
                "firing {k} at {t:e}, expected {expected:e}"
            );
        }

        // The body really ran: the conductance jumps from 1 mS to 1 S at each firing, so the
        // source current peaks three orders of magnitude above quiescent.
        let branch = net.node_order.len();
        let peak =
            wf.x.iter()
                .map(|row| row[branch].abs())
                .fold(0.0f64, f64::max);
        assert!(
            peak > 1.0,
            "the timer body must run: peak |I(V1)| = {peak:e}"
        );
    }

    /// A timer with no period fires exactly once, and does not re-arm.
    #[test]
    fn a_one_shot_timer_fires_once() {
        const SRC: &str = "
module tk1(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(timer(300u)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 5
X1 a gnd tk1
.tran 5u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        let times: Vec<f64> = wf.model_crossings.iter().map(|&(_, _, t)| t).collect();
        assert_eq!(times.len(), 1, "a period-less timer fires once: {times:?}");
        assert!(
            (times[0] - 300e-6).abs() < 1e-9,
            "at 300us, got {:e}",
            times[0]
        );
    }

    /// `cross`'s `enable` argument is honoured: a disabled site never fires.
    ///
    /// Before 0.9.5 the parser kept only the first two arguments, so a written `enable` — whose
    /// whole job is to switch the event *off* — vanished without trace and the event fired
    /// anyway. That is the failure this pins.
    #[test]
    fn a_disabled_cross_site_never_fires() {
        let run = |enable: &str| -> usize {
            let src = format!(
                "
module xe(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - 2.5, 1, 0, 0, {enable})) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd SIN(0 5 1k)
X1 a gnd xe
.tran 2u 1m
.end
",
            )
            .expect("parses");
            solve_transient(&net, &design.modules, Integration::default())
                .expect("integrates")
                .model_crossings
                .len()
        };
        assert_eq!(run("1"), 1, "enabled: the rising crossing fires");
        assert_eq!(run("0"), 0, "disabled: nothing fires");
    }

    /// Disabling a site must not itself look like a crossing.
    ///
    /// A disabled site stops reporting, and if the consumer read "no report" as a value of
    /// `0.0` it would compare that against whatever the site last reported — firing the event
    /// precisely because it was switched off. The fixture makes that concrete: the monitored
    /// expression sits at a large negative value, so a spurious comparison against zero would
    /// read as a rising crossing.
    #[test]
    fn disabling_a_site_is_not_itself_a_crossing() {
        const SRC: &str = "
module xn(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - 100.0, 1, 0, 0, 0)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd SIN(0 5 1k)
X1 a gnd xn
.tran 2u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        assert!(
            wf.model_crossings.is_empty(),
            "a permanently disabled site fires nothing: {:?}",
            wf.model_crossings
        );
    }

    /// A `cross` tolerance is recorded on the IR so the layer above can say it is not honoured
    /// — and a `timer`'s is not flagged, because an exact landing already satisfies it.
    #[test]
    fn an_unhonoured_cross_tolerance_is_visible_on_the_ir() {
        let sites = |src: &str| {
            va_frontend::compile(src).expect("compiles").modules[0]
                .event_sites
                .clone()
        };

        let with_tol = sites(
            "module a(p, n); inout p, n; electrical p, n;              analog begin @(cross(V(p, n) - 1.0, 1, 1n)) ; I(p, n) <+ V(p, n); end endmodule",
        );
        assert!(
            with_tol[0].has_unhonoured_tolerance(),
            "a cross time_tol must be visible: {:?}",
            with_tol[0]
        );

        let without = sites(
            "module b(p, n); inout p, n; electrical p, n;              analog begin @(cross(V(p, n) - 1.0, 1)) ; I(p, n) <+ V(p, n); end endmodule",
        );
        assert!(!without[0].has_unhonoured_tolerance());

        // A timer's time_tol is met by construction -- the integrator lands exactly -- so it is
        // deliberately not flagged.
        let timer = sites(
            "module c(p, n); inout p, n; electrical p, n;              analog begin @(timer(1u, 1u, 1n)) ; I(p, n) <+ V(p, n); end endmodule",
        );
        assert!(
            !timer[0].has_unhonoured_tolerance(),
            "an exact landing already satisfies a timer tolerance"
        );
    }

    /// `cross`'s `time_tol` is **honoured**: the step control re-takes the step until a
    /// timepoint lands within the requested tolerance of the crossing.
    ///
    /// Measured against the analytic crossing of a 5 V sine through +2.5 V (one twelfth of a
    /// period), and discriminating: the same deck with no tolerance is checked too, and must
    /// land *further* away. Without that control the test would pass on an engine that ignores
    /// the tolerance and merely happens to take small steps.
    #[test]
    fn a_cross_time_tol_is_honoured_by_the_step_control() {
        const PERIOD: f64 = 1e-3;
        let t_star = PERIOD / 12.0; // 5*sin(2*pi*f*t) = 2.5

        // Distance from the analytic crossing to the first accepted timepoint at or after it.
        let overshoot = |tol: &str| -> (f64, usize) {
            let src = format!(
                "
module xt(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - 2.5, 1{tol})) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd SIN(0 5 1k)
X1 a gnd xt
.tran 2u 1m
.end
",
            )
            .expect("parses");
            let wf =
                solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
            let first_after =
                wf.t.iter()
                    .copied()
                    .filter(|&t| t >= t_star)
                    .fold(f64::INFINITY, f64::min);
            (first_after - t_star, wf.unresolved_events)
        };

        let (tight, unresolved) = overshoot(", 10n");
        let (loose, _) = overshoot("");

        assert_eq!(
            unresolved, 0,
            "a 10ns tolerance is reachable and must be met"
        );
        assert!(
            tight <= 10e-9,
            "with time_tol=10ns a timepoint must land within it: overshoot {tight:e}"
        );
        assert!(
            loose > 10.0 * tight,
            "the control must be meaningfully looser, else this test proves nothing:              tolerance-free overshoot {loose:e} vs {tight:e}"
        );
    }

    /// How far the bracketing actually goes, and what happens past that.
    ///
    /// Two facts, measured rather than assumed — my first version of this test asserted an
    /// attosecond tolerance was unreachable and was simply wrong:
    ///
    /// - **1e-18 s is met.** f64 spacing near t = 83 µs is about 1.4e-20 s, so an attosecond is
    ///   ~70 ulps and the retry loop really does converge on it. That is the honest measure of
    ///   how far this step control goes.
    /// - **1e-24 s is not**, being below that spacing, and is *reported* rather than quietly
    ///   missed. The event still fires: an unmeetable tolerance degrades the resolution, not
    ///   the event.
    #[test]
    fn the_bracketing_converges_far_and_reports_when_it_cannot() {
        let run = |tol: &str| -> (usize, usize, f64) {
            let src = format!(
                "
module xu(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(cross(V(p, n) - 2.5, 1, {tol})) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd SIN(0 5 1k)
X1 a gnd xu
.tran 2u 1m
.end
",
            )
            .expect("parses");
            let wf =
                solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
            let t_star = 1e-3 / 12.0;
            let first_after =
                wf.t.iter()
                    .copied()
                    .filter(|&t| t >= t_star)
                    .fold(f64::INFINITY, f64::min);
            (
                wf.unresolved_events,
                wf.model_crossings.len(),
                first_after - t_star,
            )
        };

        let (unresolved, fired, overshoot) = run("1.0e-18");
        assert_eq!(
            unresolved, 0,
            "an attosecond is ~70 ulps here and is reachable"
        );
        assert_eq!(fired, 1);
        assert!(
            overshoot <= 1e-18,
            "the retry loop must actually reach it: overshoot {overshoot:e}"
        );

        let (unresolved, fired, _) = run("1.0e-24");
        assert!(
            unresolved > 0,
            "below f64 spacing the request cannot be met, and must be reported"
        );
        assert_eq!(
            fired, 1,
            "an unmeetable tolerance costs resolution, not the event"
        );
    }

    /// `above` fires where `cross` cannot — on a signal that is *already* past the threshold
    /// and never moves across it.
    ///
    /// This is the LRM's own motivating case (§5.10.2, the sample-and-hold example): "if the
    /// voltage on the smpl port never crosses 2.5V in the positive direction, then the cross()
    /// function of the previous example would never trigger, even if the voltage on the smpl
    /// port is always above 2.5V." A DC deck holds the node at a constant 4 V, so there is no
    /// crossing anywhere in the run — `cross` must stay silent and `above` must fire.
    #[test]
    fn above_fires_on_a_signal_that_never_crosses_where_cross_cannot() {
        let conductance = |event: &str| -> f64 {
            let src = format!(
                "
module ab(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @({event}) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd DC 4
X1 a gnd ab
.op
.end
",
            )
            .expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            // I(V1) = -g * 4, so the conductance is recoverable from the source current.
            op.x[net.node_order.len()].abs() / 4.0
        };

        let with_above = conductance("above(V(p, n) - 2.5)");
        let with_cross = conductance("cross(V(p, n) - 2.5, 1)");

        assert!(
            (with_above - 1.0).abs() < 1e-9,
            "`above` must fire at the operating point: g = {with_above}"
        );
        assert!(
            (with_cross - 1e-3).abs() < 1e-12,
            "`cross` must not fire in a static solve: g = {with_cross}"
        );
    }

    /// `above` fires in transient too, at the first solved timepoint, when its expression starts
    /// out past the threshold — the same signal a `cross` would never see.
    #[test]
    fn above_fires_at_the_start_of_a_transient_run() {
        const SRC: &str = "
module abt(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(above(V(p, n) - 2.5)) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 4
X1 a gnd abt
.tran 10u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        assert_eq!(
            wf.model_crossings.len(),
            1,
            "fires once, at initialization: {:?}",
            wf.model_crossings
        );
        let branch = net.node_order.len();
        let peak =
            wf.x.iter()
                .map(|row| row[branch].abs())
                .fold(0.0f64, f64::max);
        assert!(peak > 1.0, "the body must run: peak |I(V1)| = {peak:e}");
    }

    /// `@(final_step)`'s body runs at the **last** accepted transient timepoint and nowhere
    /// else, and the point recorded there is solved *with* its effect.
    ///
    /// The discriminating shape: the body switches the conductance by three orders of
    /// magnitude, so the assertion fails in both directions that matter — if the body never
    /// runs, the last point looks like all the others; if it runs unconditionally (the
    /// pre-2026-09-07 discard treatment), every point looks like the last one.
    #[test]
    fn final_step_fires_only_at_the_last_transient_timepoint() {
        const SRC: &str = "
module fst(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(final_step) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 4
X1 a gnd fst
.tran 10u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        let branch = net.node_order.len();
        let current = |row: &Vec<f64>| row[branch].abs();
        assert!(
            wf.x.len() > 10,
            "expected a real run, got {} points",
            wf.x.len()
        );
        // Row 0 is the cold-start seed at `tstart`, not a solved step (`I(V1) = 0` there), so
        // the interior points are rows 1..n-1.
        for (t, row) in wf.t.iter().zip(&wf.x).skip(1).take(wf.x.len() - 2) {
            assert!(
                (current(row) - 4e-3).abs() < 1e-9,
                "the body must not run at t = {t:e}: |I(V1)| = {:e}",
                current(row)
            );
        }
        let last = current(wf.x.last().expect("a last point"));
        assert!(
            (last - 4.0).abs() < 1e-6,
            "the body must run at tstop, and its effect must be solved for: |I(V1)| = {last:e}"
        );
    }

    /// The LRM's own two examples are the specification for the double-scaling warning, so they
    /// are the test: `badres` is caught, `parares` is not.
    ///
    /// Warning rather than refusal, per §6.3.6's wording — and the model is genuinely correct at
    /// `m = 1`, which is how every deck that never writes `m=` runs it.
    #[test]
    fn the_lrm_double_scaling_examples_are_told_apart() {
        // LRM 6.3.6's `badres`: the contributed current is multiplied by $mfactor explicitly,
        // and would be again by the simulator.
        let badres = va_frontend::compile(
            "module badres(a, b); inout a, b; electrical a, b;
             parameter real r = 1.0;
             analog begin I(a,b) <+ V(a,b) / r * $mfactor; end endmodule",
        )
        .expect("compiles");
        assert!(
            badres.modules[0].mfactor_scales_a_flow_contribution(),
            "the LRM's `badres` is the double-scaling case and must be caught"
        );

        // LRM 6.3.6's `parares`: $mfactor is read in a condition only, and does not scale the
        // output. No warning.
        let parares = va_frontend::compile(
            "module parares(a, b); inout a, b; electrical a, b;
             parameter real r = 1.0;
             analog begin
               if (r / $mfactor < 1.0e-3) V(a,b) <+ 0.0;
               else I(a,b) <+ V(a,b) / r;
             end endmodule",
        )
        .expect("compiles");
        assert!(
            !parares.modules[0].mfactor_scales_a_flow_contribution(),
            "the LRM's `parares` uses it in a condition and must not be warned about"
        );

        // A model that never mentions it at all is obviously clean -- stated so that a detector
        // which simply returned `true` could not pass this test.
        let plain = va_frontend::compile(
            "module p(a, b); inout a, b; electrical a, b;
             analog I(a,b) <+ V(a,b) / 1000.0; endmodule",
        )
        .expect("compiles");
        assert!(!plain.modules[0].mfactor_scales_a_flow_contribution());
    }

    /// `$simparam("iteration")` really counts Newton iterations **in a transient run** — it is
    /// not stuck at the `0` a folded-at-elaboration answer would give.
    ///
    /// The model is keyed on it so the two possible answers are far apart: at iteration 0 it is
    /// a 1 Ω resistor, from iteration 1 on it is a 1 kΩ one. Since the equations stop changing
    /// after the first iteration, the solve settles on the 1 kΩ circuit — but *only if the
    /// iteration number advances*. If `$simparam("iteration")` were frozen at 0, every
    /// iteration would see 1 Ω and the run would settle on that instead. The two RC time
    /// constants differ by 1000×, so the assertion cannot be satisfied by accident.
    #[test]
    fn simparam_iteration_advances_during_a_transient_solve() {
        const SRC: &str = "
module itr(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    if ($simparam(\"iteration\") < 1.0)
      g = 1.0;
    else
      g = 1e-3;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        // A 1 uF cap charging through the model's resistance from a 1 V step. With g = 1e-3
        // (1 kOhm) tau is 1 ms; with g = 1.0 (1 Ohm) it is 1 us -- already fully charged at the
        // first sample, which is exactly what a frozen iteration number would produce.
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 1
X1 a mid itr
C1 mid gnd 1u
.tran 10u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        let mid = net
            .node_order
            .iter()
            .position(|n| n == "mid")
            .expect("the deck names `mid`");
        let (t_end, x_end) = (
            *wf.t.last().expect("a last time"),
            wf.x.last().expect("a last point")[mid],
        );
        // 1 - exp(-t/tau) at tau = 1 ms. The LTE-controlled run is well inside 1 % of it.
        let expected = 1.0 - (-t_end / 1e-3).exp();
        assert!(
            (x_end - expected).abs() < 1e-2,
            "expected the 1 kOhm time constant (V(mid) ~ {expected:.4} at t = {t_end:e}), got \
             {x_end:.4} -- a value near 1.0 means `$simparam(\"iteration\")` never left 0"
        );
    }

    /// A **known** simulator parameter resolves to the solver's real value; an **unknown** one
    /// falls back to the query's own default; an unknown one with no default is refused.
    ///
    /// The middle case is the one with 21 occurrences in the corpus (`$simparam("gmin", 1e-12)`
    /// and friends), and the one that must not change: this engine has no permanent `gmin`
    /// floor, so it does not claim to know the name, and each model keeps the fallback it stated.
    #[test]
    fn simparam_resolves_known_names_and_falls_back_on_unknown_ones() {
        let conductance = |expr: &str| -> f64 {
            let src = format!(
                "
module sp(p, n);
  inout p, n;
  electrical p, n;
  analog I(p, n) <+ ({expr}) * V(p, n);
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse("V1 a gnd DC 4\nX1 a gnd sp\n.op\n.end\n")
                .expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs() / 4.0
        };
        // Unknown name, with a default: the default is returned. This is the corpus's case.
        assert!(
            (conductance(r#"$simparam("gmin", 1e-3)"#) - 1e-3).abs() < 1e-12,
            "an unknown name must return the model's own fallback"
        );
        // Known name: the solver's real value, not the fallback. `sourceScaleFactor` is 1.0
        // here because this engine never ramps sources -- a true statement about the solve.
        assert!(
            (conductance(r#"$simparam("sourceScaleFactor", 99.0)"#) - 1.0).abs() < 1e-12,
            "a known name must return the solver's value, not the fallback"
        );
        // Known name: `gdev` is 0.0 in an ordinary solve, so this contributes nothing and the
        // node is left floating -- which is itself the observation, so add a fixed shunt.
        assert!(
            (conductance(r#"$simparam("gdev", 7.0) + 1e-3"#) - 1e-3).abs() < 1e-12,
            "`gdev` is 0 with no homotopy running, and must not fall back to 7.0"
        );
    }

    /// An unknown simulator parameter with **no** default is refused, per LRM §9.18, and the
    /// refusal lists the names that would have worked.
    #[test]
    fn an_unknown_simparam_with_no_default_is_refused() {
        let err = va_frontend::compile(
            r#"module sp(p, n); inout p, n; electrical p, n;
               analog I(p, n) <+ $simparam("imelt") * V(p, n); endmodule"#,
        )
        .err()
        .expect("an unknown parameter with no default is an error");
        let block = refusal_block(&anyhow::Error::new(err)).expect("reported as a refusal");
        assert!(block.contains("imelt"), "names what was asked for: {block}");
        assert!(
            block.contains("iteration") && block.contains("sourceScaleFactor"),
            "lists the names this engine does know: {block}"
        );
    }

    /// A resistor model placed once with `m=3` draws exactly what three of it in parallel draw.
    ///
    /// The end-to-end statement of LRM §6.3.6's guarantee, measured through the real pipeline
    /// rather than asserted against arithmetic: the reference deck instantiates the same model
    /// three times on the same nodes, and the two solutions must agree. That is a stronger check
    /// than `I == 3*V/r` because it would still fail if the scaling reached some channel that
    /// parallel copies do not.
    #[test]
    fn an_m_of_three_equals_three_instances_in_parallel() {
        const SRC: &str = "
module res(p, n);
  inout p, n;
  electrical p, n;
  parameter real r = 1000.0;
  analog I(p, n) <+ V(p, n) / r;
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let solve = |deck: &str| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            // The source's branch current: what the whole network draws.
            op.x[net.node_order.len()].abs()
        };
        let multiplied = solve(
            "V1 a gnd DC 4
X1 a gnd res m=3
.op
.end
",
        );
        let parallel = solve(
            "V1 a gnd DC 4
X1 a gnd res
X2 a gnd res
X3 a gnd res
.op
.end
",
        );
        assert!(
            (multiplied - parallel).abs() < 1e-12,
            "m=3 must equal three in parallel: {multiplied:e} vs {parallel:e}"
        );
        // And it must actually be doing something -- a scaling that silently did nothing would
        // pass the comparison above only if the reference deck were also broken, but stating the
        // expected value pins both.
        assert!(
            (multiplied - 3.0 * 4.0 / 1000.0).abs() < 1e-12,
            "expected 3*V/r = 12 mA, got {multiplied:e}"
        );
    }

    /// `$mfactor` reports the multiplicity the deck line set, and the LRM default of `1.0` when
    /// it set none.
    ///
    /// Read in a *condition*, which is the use the LRM's own `parares` example sanctions —
    /// reading it as a factor on a flow contribution is the `badres` double-scaling error, and
    /// is warned about separately.
    #[test]
    fn mfactor_reports_the_deck_lines_multiplicity() {
        const SRC: &str = "
module mf(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    if ($mfactor > 2.0)
      g = 1.0;
    else
      g = 1e-3;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        // Per-device conductance, with the automatic m scaling divided back out, so the number
        // reflects what the *model* chose rather than how many copies there are.
        let per_device_g = |deck: &str, m: f64| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs() / 4.0 / m
        };
        assert!(
            (per_device_g("V1 a gnd DC 4\nX1 a gnd mf\n.op\n.end\n", 1.0) - 1e-3).abs() < 1e-9,
            "no `m=` reads the LRM default of 1"
        );
        assert!(
            (per_device_g("V1 a gnd DC 4\nX1 a gnd mf m=5\n.op\n.end\n", 5.0) - 1.0).abs() < 1e-9,
            "`m=5` must reach `$mfactor`"
        );
    }

    /// `m=` yields to a model that declares its own parameter `m`, and `mult=` is the
    /// unambiguous spelling.
    ///
    /// This is the case that stops multiplicity from silently changing what an existing deck
    /// means: in the corpus `m` is the junction grading coefficient (`external/diode.va` and
    /// friends declare `parameter real m = 0.5`), so reinterpreting `m=0.5` as "half a device"
    /// would be a wrong answer, not a feature.
    #[test]
    fn m_yields_to_a_model_that_declares_its_own_m() {
        const SRC: &str = "
module gm(p, n);
  inout p, n;
  electrical p, n;
  parameter real m = 0.5;
  analog I(p, n) <+ m * V(p, n);
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let current = |deck: &str| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs()
        };
        // `m=0.25` sets the model's own parameter: one device of conductance 0.25.
        let as_param = current("V1 a gnd DC 4\nX1 a gnd gm m=0.25\n.op\n.end\n");
        assert!(
            (as_param - 0.25 * 4.0).abs() < 1e-9,
            "`m=` must set the model's own `m`, not the multiplicity: {as_param:e}"
        );
        // `mult=` is unambiguous, and leaves the model's `m` at its default of 0.5.
        let as_multiplicity = current("V1 a gnd DC 4\nX1 a gnd gm mult=3\n.op\n.end\n");
        assert!(
            (as_multiplicity - 3.0 * 0.5 * 4.0).abs() < 1e-9,
            "`mult=` must set the multiplicity: {as_multiplicity:e}"
        );
    }

    /// A multiplicity that is not a positive, finite count is rejected rather than applied.
    #[test]
    fn a_nonsensical_multiplicity_is_rejected() {
        const SRC: &str = "
module r2(p, n);
  inout p, n;
  electrical p, n;
  analog I(p, n) <+ V(p, n) / 1000.0;
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        for bad in ["m=0", "m=-2"] {
            let net = va_netlist::parser::parse(&format!(
                "V1 a gnd DC 4\nX1 a gnd r2 {bad}\n.op\n.end\n"
            ))
            .expect("parses");
            let err = solve_dc(&net, &design.modules)
                .err()
                .unwrap_or_else(|| panic!("`{bad}` must be rejected"));
            let msg = format!("{err:#}");
            assert!(msg.contains("multiplicity"), "says what is wrong: {msg}");
        }
    }

    /// **Every refusal, from every layer, reaches the user saying what was refused and why.**
    ///
    /// The standing requirement this locks in: a refusal is a construct the implementation
    /// recognises and declines, and a user who hits one needs to be told which construct and
    /// what would have gone wrong — otherwise the only way to find out is to read this source.
    /// The three layers that can refuse are checked together here on purpose, because the
    /// failure mode is one of them drifting into a bare sentence while the others stay
    /// labelled, and no per-layer test would notice that.
    #[test]
    fn every_refusal_says_what_was_refused_and_why() {
        // 1. `va-frontend` — a construct refused at parse time.
        let frontend = va_frontend::compile(
            "module f(p, n); inout p, n; electrical p, n; real k;
             analog begin @(final_step(\"tran\")) k = 1.0; I(p, n) <+ V(p, n); end endmodule",
        )
        .err()
        .expect("an analysis-filtered trigger is refused");
        // 2. `va-codegen` — a construct that elaborates but cannot be lowered.
        let design = va_frontend::compile(
            "module c(p, n); inout p, n; electrical p, n;
             analog begin I(p, n) <+ 1.0 / (1.0 + absdelay(V(p, n), 1u)); end endmodule",
        )
        .expect("this one elaborates; codegen is what refuses it");
        let terminals: Vec<usize> = (0..design.modules[0].nodes.len()).collect();
        let mut next = terminals.len();
        let codegen = va_codegen::build_instance(&design.modules[0], &terminals, &mut next)
            .err()
            .expect("a buried absdelay is refused");
        // 3. `va-cli` — a whole model refused in one analysis.
        let cli = refuse_transient_approximations(&[(
            "m.va".to_string(),
            "module m(p, n); inout p, n; electrical p, n;
             analog I(p, n) <+ absdelay(V(p, n), 1u); endmodule"
                .to_string(),
        )])
        .expect_err("absdelay folds in transient");

        for (layer, err) in [
            ("va-frontend", anyhow::Error::new(frontend)),
            ("va-codegen", anyhow::Error::new(codegen)),
            ("va-cli", cli),
        ] {
            let block = refusal_block(&err)
                .unwrap_or_else(|| panic!("{layer}: a refusal must be recognised as one: {err:#}"));
            assert!(
                block.starts_with("refused: "),
                "{layer}: must open with the greppable marker: {block}"
            );
            assert!(
                block.contains("why:"),
                "{layer}: must say why, not just what: {block}"
            );
            assert!(
                block.contains("tracking:"),
                "{layer}: must point at where the limitation is tracked: {block}"
            );
            // The `what` has to name something the user actually wrote. Each of the three
            // sources above is about `absdelay` or `final_step`; a message naming neither would
            // be the `laplace_*`-for-an-`absdelay` defect that prompted this.
            let names_the_construct = block.contains("absdelay") || block.contains("final_step");
            assert!(
                names_the_construct,
                "{layer}: must name the construct the user wrote: {block}"
            );
        }
    }

    /// A refusal is **not** reported as an ordinary error, and an ordinary error is not reported
    /// as a refusal — the distinction `report_refusal` exists to draw.
    #[test]
    fn an_ordinary_error_is_not_mistaken_for_a_refusal() {
        // A genuine syntax error: malformed source, not a recognised-and-declined construct.
        let malformed = va_frontend::compile("module m(a, b); electrical a b; endmodule")
            .err()
            .expect("this does not parse");
        assert!(
            refusal_block(&anyhow::Error::new(malformed)).is_none(),
            "a parse error is not a refusal: telling a user their correct model was declined, \
             when in fact they mistyped, sends them in exactly the wrong direction"
        );
    }

    /// `@(initial_step or final_step)` runs its body at the end of a transient run and at no
    /// interior timepoint.
    ///
    /// This is the case that exposed the hole 0.9.8 left: before 0.9.9 a compound step trigger
    /// was silently discarded and its body ran at every timepoint, which the middle assertion
    /// below catches.
    ///
    /// **The asymmetry this test documents, which predates the compound form.** Only the
    /// `final_step` half is visible in the waveform. `is_initial_step` is set on exactly one
    /// evaluation — the seed assemble at `tstart`, whose job is to establish state history — and
    /// that evaluation is never *solved*: `x0` is the caller's initial condition, not a Newton
    /// result. So an `@(initial_step)` body that writes a variable feeding a contribution
    /// influences committed state (which is what the flag was added for, in 2026-08-06's state
    /// channel) but changes no recorded point. `final_step` is re-solved and therefore does.
    /// Making the two symmetric would mean re-solving the run's first accepted timepoint as
    /// well, which changes the meaning of a supplied initial condition — a deliberate
    /// non-decision here, recorded in `docs/roadmap.md`'s events section.
    #[test]
    fn an_or_list_of_step_events_does_not_fire_at_every_timepoint() {
        const SRC: &str = "
module bts(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(initial_step or final_step) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 4
X1 a gnd bts
.tran 10u 1m
.end
",
        )
        .expect("parses");
        let wf =
            solve_transient(&net, &design.modules, Integration::default()).expect("integrates");
        let branch = net.node_order.len();
        let current = |row: &Vec<f64>| row[branch].abs();
        assert!(wf.x.len() > 10, "expected a real run, got {}", wf.x.len());
        // Row 0 is the cold-start seed, not a solved step (see this test's doc comment for why
        // the `initial_step` half leaves no mark on it either). Rows 1..n-1 are interior
        // accepted timepoints, where neither half of the trigger fires.
        for (t, row) in wf.t.iter().zip(&wf.x).skip(1).take(wf.x.len() - 2) {
            assert!(
                (current(row) - 4e-3).abs() < 1e-9,
                "neither half fires at t = {t:e}: |I(V1)| = {:e}",
                current(row)
            );
        }
        let last = current(wf.x.last().expect("a last point"));
        assert!(
            (last - 4.0).abs() < 1e-6,
            "the final-step half must fire at tstop: |I(V1)| = {last:e}"
        );
    }

    /// The same model in a **static** solve runs the body: a single operating point is both the
    /// analysis's first step and its last, which is why `AnalysisCtx::is_final_step` is `true`
    /// in DC/AC/noise. This is also what the old analysis-gated refusal in `va-cli` protected —
    /// it was only ever wrong in transient.
    #[test]
    fn final_step_runs_in_a_static_solve() {
        const SRC: &str = "
module fso(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(final_step) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
";
        let design = va_frontend::compile(SRC).expect("compiles");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 4
X1 a gnd fso
.op
.end
",
        )
        .expect("parses");
        let op = solve_dc(&net, &design.modules).expect("solves");
        let g = op.x[net.node_order.len()].abs() / 4.0;
        assert!(
            (g - 1.0).abs() < 1e-9,
            "one solve point is its own final step: g = {g:e}"
        );
    }

    /// `above`'s `enable` still gates it, and an `above` on a *negative* expression does not
    /// fire — the initialization rule is "already positive", not "always".
    #[test]
    fn above_respects_enable_and_the_sign_of_its_expression() {
        let conductance = |args: &str| -> f64 {
            let src = format!(
                "
module abe(p, n);
  inout p, n;
  electrical p, n;
  real g;
  analog begin
    g = 1e-3;
    @(above({args})) g = 1.0;
    I(p, n) <+ g * V(p, n);
  end
endmodule
"
            );
            let design = va_frontend::compile(&src).expect("compiles");
            let net = va_netlist::parser::parse(
                "V1 a gnd DC 4
X1 a gnd abe
.op
.end
",
            )
            .expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs() / 4.0
        };
        // Below the threshold: nothing to be above.
        assert!((conductance("V(p, n) - 100.0") - 1e-3).abs() < 1e-12);
        // Above it, but disabled.
        assert!((conductance("V(p, n) - 2.5, 0, 0, 0") - 1e-3).abs() < 1e-12);
        // Above it and enabled.
        assert!((conductance("V(p, n) - 2.5, 0, 0, 1") - 1.0).abs() < 1e-9);
    }

    /// A step-scoped trigger that is still *discarded* is refused in transient, where its body
    /// would re-run at every timepoint -- but not in a static solve, where running it once is
    /// correct.
    ///
    /// The split matters: `@(initial_model)` in a DC operating point is right, because the
    /// single solve point really is the one and only evaluation. So this is analysis-gated here
    /// rather than a parse error in the frontend. (A *monitored* trigger like `@(cross(...))`
    /// never fires in a static solve either, so the frontend rejects those outright and they
    /// never reach this check.)
    #[test]
    fn a_transient_run_refuses_a_step_scoped_trigger() {
        const MODEL: &str = "
module im(p, n);
  inout p, n;
  electrical p, n;
  real k;
  analog begin
    @(initial_model) k = 1.0;
    I(p, n) <+ V(p, n) / 1000.0;
  end
endmodule
";
        assert_eq!(step_triggers_in(MODEL), vec!["initial_model"]);
        let err = refuse_transient_approximations(&[("im.va".to_string(), MODEL.to_string())])
            .expect_err("`@(initial_model)` re-runs every timepoint");
        let msg = format!("{err:#}");
        assert!(msg.contains("initial_model"), "names the trigger: {msg}");
        assert!(
            msg.contains("every timepoint"),
            "says what actually goes wrong: {msg}"
        );
    }

    /// `@(final_step)` is **not** refused any more (2026-09-07): it is scheduled rather than
    /// discarded, so the reason the refusal existed -- a body running at every timepoint -- is
    /// gone. This is the check that fails if the refusal is ever reinstated by reflex.
    #[test]
    fn a_transient_run_accepts_final_step() {
        const MODEL: &str = "
module fs(p, n);
  inout p, n;
  electrical p, n;
  real k;
  analog begin
    k = 0.0;
    @(final_step) k = 1.0;
    I(p, n) <+ V(p, n) / 1000.0;
  end
endmodule
";
        assert!(
            step_triggers_in(MODEL).is_empty(),
            "`final_step` is implemented, not discarded"
        );
        refuse_transient_approximations(&[("fs.va".to_string(), MODEL.to_string())])
            .expect("nothing to refuse");
    }

    /// Detection is on `@(` + name, so a bare mention of a trigger word elsewhere -- in an
    /// expression, or in a comment -- is not mistaken for an event control.
    #[test]
    fn a_bare_mention_of_a_trigger_word_is_not_an_event_control() {
        const MODEL: &str = "
module m(p, n);
  inout p, n;
  electrical p, n;
  real initial_model;
  analog begin
    // mentions initial_model and initial_instance in prose
    initial_model = 2.0;
    I(p, n) <+ V(p, n) / initial_model;
  end
endmodule
";
        assert!(
            step_triggers_in(MODEL).is_empty(),
            "a variable named `initial_model` is not an `@(initial_model)`"
        );
        refuse_transient_approximations(&[("m.va".to_string(), MODEL.to_string())])
            .expect("nothing to refuse here");
    }

    /// `--model` may name a *directory* -- the documented way to use a real model library --
    /// and the check must see every file in it. The previous implementation read only the path
    /// itself with `read_to_string`, which fails on a directory and was silently skipped, so a
    /// library was exactly the case that escaped the check.
    #[test]
    fn the_approximation_check_sees_a_model_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../models");
        let srcs = model_sources(Some(dir));
        assert!(
            srcs.len() > 1,
            "a directory must yield every model in it, got {}",
            srcs.len()
        );
        assert!(
            srcs.iter().any(|(p, _)| p.contains("laplace_lowpass")),
            "including the one that would be refused"
        );
        refuse_transient_approximations(&srcs)
            .expect_err("the directory contains an approximated model");
    }

    /// 2026-08-31; before that it lexed as a plain identifier.
    #[test]
    fn transient_approximations_are_detected_by_token_not_by_substring() {
        let uses_it = include_str!("../../../tests/fixtures/delay_probe.va");
        assert_eq!(approximations_in(uses_it), vec!["absdelay"]);

        // The zoo's own diode uses none of them.
        assert!(approximations_in(include_str!("../../../models/diode.va")).is_empty());

        // A comment mentioning it, and an identifier containing it, are both ignored: the
        // lexer skips the first and yields an `Ident` (not a `Keyword`) for the second.
        let decoy = "// absdelay is not used here\nmodule m(a);\nelectrical a;\nreal absdelay_count;\nanalog absdelay_count = 1.0;\nendmodule\n";
        assert!(
            approximations_in(decoy).is_empty(),
            "a comment or a longer identifier must not trigger the warning"
        );
    }
    /// `absdelay` in AC, against the closed form. The deck is a 1 k resistor into a delay
    /// line whose own conductance is delayed, so KCL gives exactly
    ///
    /// ```text
    ///   V(out)/V(in) = 1 / (1 + (R1/r)*exp(-j*w*td))
    /// ```
    ///
    /// Checked at several frequencies *and across a phase wrap*: at f = 1/td the delay's phase
    /// has advanced a full turn, so f and f + 1/td must give bit-comparable answers. That wrap
    /// is the part no rational filter can imitate -- a Pade or single-pole approximation of a
    /// delay tracks the first few degrees and then diverges -- which is what makes this a test
    /// of a real delay rather than of "something with phase in it".
    #[test]
    fn absdelay_is_an_exact_delay_in_ac() {
        let src = include_str!("../../../models/delay_line.va");
        let design = compile_model(src, "delay_line.va");
        let net = va_netlist::parser::parse(include_str!("../../../circuits/delay_ac.net"))
            .expect("parse delay_ac");
        let resp = solve_ac(&net, &design.modules).expect("solves");

        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");
        let td = 1e-6_f64; // set on the device line, along with r
        let k = 500.0 / 1000.0; // R1 / r, deliberately not 1 (see the deck's own comment)

        for (i, &f) in resp.f.iter().enumerate() {
            let (re, im) = resp.x[i][out];
            // 1 / (1 + e^{-jwt}), by hand: denominator (1 + cos, -sin), then reciprocal.
            let wt = 2.0 * std::f64::consts::PI * f * td;
            let (dr, di) = (1.0 + k * wt.cos(), -k * wt.sin());
            let d2 = dr * dr + di * di;
            let (want_re, want_im) = (dr / d2, -di / d2);
            let err = ((re - want_re).powi(2) + (im - want_im).powi(2)).sqrt();
            assert!(
                err < 1e-9 * (want_re.hypot(want_im)).max(1.0),
                "at {f:e} Hz: got ({re:.9}, {im:.9}), closed form ({want_re:.9}, {want_im:.9})"
            );
        }

        // The wrap, stated as its own claim: f and f + 1/td are the same point on the delay's
        // unit circle, so the response must repeat. A rational approximation would not.
        let at = |target: f64| {
            let i = resp
                .f
                .iter()
                .position(|&f| (f - target).abs() < 1.0)
                .unwrap_or_else(|| panic!("no sweep point at {target:e} Hz"));
            resp.x[i][out]
        };
        let (a_re, a_im) = at(1e5);
        let (b_re, b_im) = at(1e5 + 1.0 / td);
        assert!(
            (a_re - b_re).abs() < 1e-9 && (a_im - b_im).abs() < 1e-9,
            "the response must repeat every 1/td: ({a_re}, {a_im}) vs ({b_re}, {b_im})"
        );
    }

    /// A two-path interferometer, which is the reason `absdelay` has to be an exact delay.
    ///
    /// Two arms delayed by t1 and t2 give an admittance carrying
    /// `(exp(-jw*t1) + exp(-jw*t2))/2`, so the response is **periodic in frequency** with
    /// spacing `1/(t2 - t1)` -- the free spectral range. The test asserts three things, in
    /// increasing order of how hard they are to fake:
    ///
    /// 1. point-by-point agreement with the closed form;
    /// 2. fringe periodicity across a full free spectral range -- a rational filter can imitate
    ///    one fringe and cannot repeat it forever;
    /// 3. the destructive point, where the arms cancel, the device's admittance vanishes and
    ///    the output rises to the source voltage.
    #[test]
    fn an_interferometer_shows_fringes_at_its_free_spectral_range() {
        let src = include_str!("../../../models/interferometer.va");
        let design = compile_model(src, "interferometer.va");
        let net =
            va_netlist::parser::parse(include_str!("../../../circuits/interferometer_ac.net"))
                .expect("parse interferometer_ac");
        let resp = solve_ac(&net, &design.modules).expect("solves");
        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");

        let (t1, t2, k) = (0.0_f64, 1e-6_f64, 500.0 / 1000.0); // from the deck's device line
        let closed_form = |f: f64| {
            let (w1, w2) = (
                2.0 * std::f64::consts::PI * f * t1,
                2.0 * std::f64::consts::PI * f * t2,
            );
            // Arm sum, averaged: (e^{-jw t1} + e^{-jw t2}) / 2.
            let (ar, ai) = (0.5 * (w1.cos() + w2.cos()), -0.5 * (w1.sin() + w2.sin()));
            // V = 1 / (1 + k * armsum)
            let (dr, di) = (1.0 + k * ar, k * ai);
            let d2 = dr * dr + di * di;
            (dr / d2, -di / d2)
        };

        for (i, &f) in resp.f.iter().enumerate() {
            let (re, im) = resp.x[i][out];
            let (wr, wi) = closed_form(f);
            let err = ((re - wr).powi(2) + (im - wi).powi(2)).sqrt();
            assert!(
                err < 1e-9,
                "at {f:e} Hz: ({re:.9}, {im:.9}) vs ({wr:.9}, {wi:.9})"
            );
        }

        // The comb: one free spectral range apart, the response repeats exactly.
        let fsr = 1.0 / (t2 - t1);
        let at = |target: f64| {
            let i = resp
                .f
                .iter()
                .position(|&f| (f - target).abs() < 1.0)
                .unwrap_or_else(|| panic!("no sweep point at {target:e} Hz"));
            resp.x[i][out]
        };
        for base in [5e4, 3.5e5] {
            let (a_re, a_im) = at(base);
            let (b_re, b_im) = at(base + fsr);
            assert!(
                (a_re - b_re).abs() < 1e-9 && (a_im - b_im).abs() < 1e-9,
                "one FSR apart must repeat: {base:e} -> ({a_re}, {a_im}),                  {:e} -> ({b_re}, {b_im})",
                base + fsr
            );
        }

        // Destructive interference: at w*(t2 - t1) = pi the arms cancel, the device draws no
        // current, and the output must sit at the full source voltage.
        let (re, im) = closed_form(0.5 * fsr);
        assert!(
            (re - 1.0).abs() < 1e-12 && im.abs() < 1e-12,
            "the arms should cancel at half the FSR, giving unity: ({re}, {im})"
        );
    }

    /// Verilog-A's reference node is **global** (LRM 3.6.3): `V(x)` means `V(x, ground)`
    /// against *the* ground, not one private to the module the shorthand was written in.
    ///
    /// Two places got that wrong, and both were silent rather than loud. A submodule
    /// elaborates in its own arena and interns its own implicit `gnd`, which inlining copied
    /// in as a separate floating node; and a top-level model's `gnd` was handed a fresh
    /// unknown by `build_from_model` instead of the circuit's ground. Either way a
    /// contribution written `V(out) <+ 3.0` drove a node connected to nothing, and the model
    /// reported 0 V or a singular row while looking perfectly healthy.
    ///
    /// The fixture mirrors the shape the photonic corpus is written in: a parent driving an
    /// internal net by potential contribution, a child instanced through a *slice* of a wider
    /// vector net, and the child driving its own output the same way.
    #[test]
    fn a_models_implicit_ground_is_the_circuits_ground() {
        let lib = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/veclib2");
        let modules = compile_model_path(lib).expect("compiles");

        // Top level: `V(o) <+ 3.0` against the model's implicit ground must be 3 V against the
        // deck's ground, not 0 V against a floating one.
        let net = va_netlist::parser::parse(
            "X1 out pot
R1 out gnd 1000
.op
.end
",
        )
        .expect("parses");
        let op = solve_dc(&net, &modules).expect("solves");
        let out = net.node_order.iter().position(|n| n == "out").expect("out");
        assert!(
            (op.x[out] - 3.0).abs() < 1e-9,
            "a single-terminal potential contribution must reference circuit ground, got {}",
            op.x[out]
        );

        // And through a submodule instanced across a vector slice.
        let lib = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/veclib");
        let modules = compile_model_path(lib).expect("compiles");
        let deck = "V0 p0 gnd DC 2
V1 p1 gnd DC 0.5
V2 p2 gnd DC 0
V3 p3 gnd DC 0
                    X1 p0 p1 p2 p3 q0 q1 vtop
.op
.end
";
        let net = va_netlist::parser::parse(deck).expect("parses");
        let op = solve_dc(&net, &modules).expect("solves");
        let at = |name: &str| {
            op.x[net
                .node_order
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("node `{name}`"))]
        };
        // The child multiplies its two vector inputs elementwise: t = (3, 5) from the parent's
        // own contributions, p[0:1] = (2, 0.5) from the deck through the slice.
        assert!((at("q0") - 6.0).abs() < 1e-9, "V(q0) = {}", at("q0"));
        assert!((at("q1") - 2.5).abs() < 1e-9, "V(q1) = {}", at("q1"));
    }

    /// A Verilog-A model with an arbitrary port count, instantiated by an `X` line out of a
    /// model *library* loaded from a directory. Both halves are needed and neither existed
    /// before: every other device letter fixes its terminal count (2 for `D`, 3 for `M`/`Q`,
    /// 4 for `E`/`G`), and `--model` took a single file, so a module instancing a sibling
    /// file's module failed with "references unknown module".
    ///
    /// Checked against hand-computed values: each arm is a conductance against a 1 k load.
    #[test]
    fn a_model_library_places_an_arbitrary_port_count() {
        let modules = compile_model_path(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/lib"
        ))
        .expect("compiles the library");
        assert_eq!(modules.len(), 2, "both files' modules are in the library");
        assert!(
            modules.iter().any(|m| m.name == "tee") && modules.iter().any(|m| m.name == "scaler"),
            "the library carries both modules"
        );

        let net = va_netlist::parser::parse(include_str!("../../../circuits/lib_tee.net"))
            .expect("parse lib_tee");
        let dev = net.devices.iter().find(|d| d.name == "X1").expect("X1");
        assert_eq!(dev.model, "tee", "the model name trails the node list");
        assert_eq!(
            dev.terminals.len(),
            4,
            "four nodes, which no other letter allows"
        );

        let op = solve_dc(&net, &modules).expect("solves");
        let at = |name: &str| {
            op.x[net
                .node_order
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("node `{name}`"))]
        };
        assert!((at("b") - 1.0).abs() < 1e-9, "V(b) = {}", at("b"));
        assert!((at("d") - 2.0).abs() < 1e-9, "V(d) = {}", at("d"));
    }

    /// A model that asks `$port_connected` about a trailing port may be placed without it --
    /// SPICE's idiom for an optional terminal, and the reason the query exists at all.
    ///
    /// The fixture is the self-heating shape the corpus is full of: a `dt` thermal port that
    /// the model drives only when something is wired to it, and clamps otherwise. The two
    /// placements must give *different* answers, which is what proves the query is being
    /// answered rather than folded to a constant.
    #[test]
    fn an_optional_trailing_port_may_be_left_off_the_deck() {
        const MODEL: &str = "
module selfheat(p, n, dt);
  inout p, n, dt;
  electrical p, n, dt;
  analog begin
    if ($port_connected(dt)) I(p, n) <+ V(p, n) / 1000.0;
    else I(p, n) <+ V(p, n) / 4000.0;
    I(dt) <+ V(dt) / 1000.0;
  end
endmodule
";
        let design = compile_model(MODEL, "selfheat.va");
        let current = |deck: &str| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs()
        };
        let connected = current(
            "V1 a gnd DC 1.0
X1 a gnd gnd selfheat
.op
.end
",
        );
        let omitted = current(
            "V1 a gnd DC 1.0
X1 a gnd selfheat
.op
.end
",
        );
        assert!(
            (connected - 1e-3).abs() < 1e-9,
            "all three terminals wired means `dt` is connected (1 kOhm), got {connected}"
        );
        assert!(
            (omitted - 2.5e-4).abs() < 1e-9,
            "omitting `dt` must read as unconnected (4 kOhm), got {omitted}"
        );
    }

    /// ...but only for a port the model actually treats as optional. Omitting a terminal is
    /// also what a typo looks like, and silently floating a node the deck meant to wire is the
    /// worse failure, so a model that never asks `$port_connected` still gets the count error.
    #[test]
    fn a_short_deck_line_is_still_an_error_for_a_non_optional_port() {
        const MODEL: &str = "
module plain(p, n, x);
  inout p, n, x;
  electrical p, n, x;
  analog begin
    I(p, n) <+ V(p, n) / 1000.0;
    I(x) <+ V(x) / 1000.0;
  end
endmodule
";
        let design = compile_model(MODEL, "plain.va");
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 1.0
X1 a gnd plain
.op
.end
",
        )
        .expect("parses");
        let err = solve_dc(&net, &design.modules).expect_err("two nodes into a three-port model");
        let msg = format!("{err:#}");
        assert!(msg.contains("plain"), "should name the model: {msg}");
        assert!(
            msg.contains("not optional") && msg.contains("$port_connected"),
            "should say why the omission was not accepted: {msg}"
        );
    }

    /// A `localparam` is a module-internal constant, so a deck cannot set it.
    ///
    /// `parameter` and `localparam` used to lower identically -- harmless while nothing could
    /// override anything, but once device lines gained `name=value` overrides it meant every
    /// `localparam` was silently overridable, which inverts what the keyword is for.
    #[test]
    fn a_deck_cannot_override_a_localparam() {
        const MODEL: &str = "
module lp(p, n);
  inout p, n;
  electrical p, n;
  parameter real r = 1000.0;
  localparam real scale = 2.0;
  analog I(p, n) <+ V(p, n) / (r * scale);
endmodule
";
        let design = compile_model(MODEL, "lp.va");

        // The ordinary parameter is settable...
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 1.0
X1 a gnd lp r=500
.op
.end
",
        )
        .expect("parses");
        solve_dc(&net, &design.modules).expect("an ordinary parameter may be set");

        // ...the localparam is not.
        let net = va_netlist::parser::parse(
            "V1 a gnd DC 1.0
X1 a gnd lp scale=5
.op
.end
",
        )
        .expect("parses");
        let err = solve_dc(&net, &design.modules).expect_err("`scale` is a localparam");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("scale") && msg.contains("localparam"),
            "should name the parameter and say why: {msg}"
        );
    }

    /// Connecting the wrong number of nodes is refused by name and count, not silently
    /// truncated or padded -- the failure mode a positional connection list invites.
    #[test]
    fn an_x_line_with_the_wrong_port_count_is_rejected() {
        let modules = compile_model_path(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/lib"
        ))
        .expect("compiles the library");
        let deck = "V1 a gnd DC 1
X1 a b c tee
R1 b gnd 1k
.op
.end
";
        let net = va_netlist::parser::parse(deck).expect("parses");
        let err = solve_dc(&net, &modules).expect_err("three nodes into a four-port model");
        let msg = format!("{err:#}");
        assert!(msg.contains("tee"), "should name the model: {msg}");
        assert!(
            msg.contains('4') && msg.contains('3'),
            "should give both counts: {msg}"
        );
    }

    /// Controlled sources, against values computable by hand rather than against golden:
    /// a 3 V source across a 2k/1k divider gives V(mid) = 1 V, an E with gain 4 holds
    /// V(eout) = 4 V, and a G pushing 2 mA through 500 ohms gives V(gout) = -1 V.
    #[test]
    fn controlled_sources_amplify_by_their_stated_gain() {
        let deck = include_str!("../../../circuits/vcvs_amp.net");
        let net = va_netlist::parser::parse(deck).expect("parse vcvs_amp");
        let op = solve_dc(&net, &[]).expect("solves");
        let at = |name: &str| {
            let i = net
                .node_order
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("node `{name}`"));
            op.x[i]
        };
        assert!((at("mid") - 1.0).abs() < 1e-9, "V(mid) = {}", at("mid"));
        assert!((at("eout") - 4.0).abs() < 1e-9, "V(eout) = {}", at("eout"));
        assert!((at("gout") + 1.0).abs() < 1e-9, "V(gout) = {}", at("gout"));
    }

    /// Mutual inductance, against the two facts that can be established without an oracle:
    /// the secondary voltage is exactly zero at t=0+ (KCL at that node says
    /// `i_L2 + V(s)/R2 = 0`, and an inductor's current cannot jump), and removing the coupling
    /// leaves the secondary dead for the whole run. `circuits/transformer.net` is deliberately
    /// not gated against QSPICE -- see its own header comment.
    #[test]
    fn coupling_drives_a_secondary_that_is_otherwise_dead() {
        let deck = include_str!("../../../circuits/transformer.net");
        let net = va_netlist::parser::parse(deck).expect("parse transformer");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");
        let s_idx = net
            .node_order
            .iter()
            .position(|n| n == "s")
            .expect("`s` node");

        assert_eq!(
            wf.x[0][s_idx], 0.0,
            "an inductor's current cannot jump, so V(s)(0+) = 0"
        );
        let peak =
            wf.x.iter()
                .map(|r| r[s_idx])
                .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (1.5..1.9).contains(&peak),
            "a k=0.9 transformer with a turns ratio of 2 should push the secondary well past              the 1 V primary step, got {peak}"
        );

        // The same deck without its K card: the secondary is an isolated L-R loop with no
        // source, so it must stay at exactly zero -- which is what makes the assertion above
        // a statement about coupling rather than about wiring.
        let uncoupled: String = deck
            .lines()
            .filter(|l| !l.starts_with("K1"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let net = va_netlist::parser::parse(&uncoupled).expect("parses without K");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");
        assert!(
            wf.x.iter().all(|r| r[s_idx] == 0.0),
            "without coupling the secondary must be dead for the entire run"
        );
    }

    /// Current-controlled sources, against hand-computed values. Both sense the *same*
    /// element deliberately: if either resolved the controlling branch row wrongly, the two
    /// outputs would disagree about a current they must agree on.
    #[test]
    fn current_controlled_sources_sense_a_named_branch() {
        let deck = include_str!("../../../circuits/cccs_mirror.net");
        let net = va_netlist::parser::parse(deck).expect("parse cccs_mirror");
        let op = solve_dc(&net, &[]).expect("solves");
        let at = |name: &str| {
            let i = net
                .node_order
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("node `{name}`"));
            op.x[i]
        };
        // 1 mA through the sensed source: F mirrors it x3 into 200 ohms, H converts it at
        // 2000 ohms transresistance.
        assert!((at("fout") + 0.6).abs() < 1e-9, "V(fout) = {}", at("fout"));
        assert!((at("hout") - 2.0).abs() < 1e-9, "V(hout) = {}", at("hout"));
    }

    /// Naming a controlling element that owns no branch row is a clear error rather than a
    /// silent zero -- a resistor has no branch current to sense, which is exactly why SPICE
    /// decks insert a 0 V source to do the sensing.
    #[test]
    fn a_controller_without_a_branch_row_is_rejected() {
        let deck = "V1 in gnd DC 1
R1 in gnd 1000
F1 o gnd R1 2
R2 o gnd 100
.op
.end
";
        let net = va_netlist::parser::parse(deck).expect("parses");
        let err = solve_dc(&net, &[]).expect_err("R1 has no branch current");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("R1"),
            "should name the controlling element: {msg}"
        );
        assert!(
            msg.contains("branch current"),
            "and say what is missing: {msg}"
        );
    }

    /// Branch-current identity must come from `branch_currents`, not from assuming which
    /// devices claim branch rows and in what order. That assumption held while only `vsource`
    /// claimed one; inductors and controlled sources claim them too, so a deck declaring an
    /// inductor *before* its source used to print the inductor's current under the source's
    /// name -- a confidently mislabelled number rather than a missing one.
    #[test]
    fn branch_currents_are_identified_by_name_not_by_device_order() {
        let deck = "L1 in out 1e-3
V1 in gnd DC 2
R1 out gnd 1000
.op
.end
";
        let net = va_netlist::parser::parse(deck).expect("parses");
        let currents = branch_currents(&net, &[]).expect("resolves");
        let op = solve_dc(&net, &[]).expect("solves");

        let idx = |name: &str| {
            currents
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("no branch current for `{name}`"))
                .1
        };
        // The source sees -2 mA by its own sign convention; the inductor carries +2 mA. If the
        // two were transposed, both assertions would still be about real numbers in the
        // solution vector -- which is exactly why the bug was invisible without checking signs.
        assert!(
            (op.x[idx("V1")] + 2e-3).abs() < 1e-9,
            "I(V1) = {}",
            op.x[idx("V1")]
        );
        assert!(
            (op.x[idx("L1")] - 2e-3).abs() < 1e-9,
            "I(L1) = {}",
            op.x[idx("L1")]
        );
        assert_ne!(idx("V1"), idx("L1"), "each element owns a distinct row");
    }

    /// Per-instance parameter overrides reach the compiled model, and are checked against a
    /// second solve rather than an absolute number: the same deck at the model's own defaults
    /// must give a *different* answer, which is what proves the override was applied at all.
    #[test]
    fn parameter_overrides_reach_the_compiled_model() {
        let src = include_str!("../../../models/diode.va");
        let design = compile_model(src, "diode.va");

        let overridden =
            va_netlist::parser::parse(include_str!("../../../circuits/diode_iv_params.net"))
                .expect("parse diode_iv_params");
        let sweep = overridden.dc.clone().expect("`.dc` card");
        let with = solve_dc_sweep(&overridden, &design.modules, &sweep).expect("solves");

        // The same circuit with the overrides removed, i.e. at the .va file's own defaults.
        let plain_deck = include_str!("../../../circuits/diode_iv_params.net")
            .replace("diode Is=1e-12 N=1.3", "diode");
        let plain = va_netlist::parser::parse(&plain_deck).expect("parse without overrides");
        assert!(
            plain.devices.iter().all(|d| d.params.is_empty()),
            "the comparison deck must genuinely carry no overrides"
        );
        let without = solve_dc_sweep(&plain, &design.modules, &sweep).expect("solves");

        // At the top of the sweep the two are not subtly different. The overridden diode
        // conducts *less*, which is worth stating because it is the opposite of what "a
        // hundredfold larger Is" suggests on its own: N=1.3 shrinks the exponent by more than
        // the larger Is scales the prefactor, and the exponent wins. The assertion is on the
        // size of the gap, not its direction, so it tests that the overrides landed rather
        // than re-deriving the diode equation.
        let branch = overridden.node_order.len(); // I(V1) follows the node unknowns
        let i_with = with.last().expect("points").1.x[branch].abs();
        let i_without = without.last().expect("points").1.x[branch].abs();
        let ratio = (i_with / i_without).max(i_without / i_with);
        assert!(
            ratio > 10.0,
            "overrides should change the answer by orders of magnitude: {i_with} vs {i_without}"
        );
    }

    /// `$param_given` reports what the *deck* did, not a fixed `false`.
    ///
    /// The regression this pins: the query used to fold to `false` at elaboration, justified
    /// by "v0 has no netlist-driven parameter overrides" — a premise that expired when a
    /// device line gained `name=value` overrides. A model branching on `$param_given` then
    /// took the not-given branch even when the deck had plainly given the parameter.
    ///
    /// The fixture makes the two branches differ by 1000x, so a wrong answer cannot hide in a
    /// tolerance: `r` given means a 1 kOhm resistor, `r` not given means 1 MOhm.
    #[test]
    fn param_given_reports_whether_the_deck_set_the_parameter() {
        const MODEL: &str = "
module pg(p, n);
  inout p, n;
  electrical p, n;
  parameter real r = 1000.0;
  real rr;
  analog begin
    if ($param_given(r)) rr = r;
    else rr = 1.0e6;
    I(p, n) <+ V(p, n) / rr;
  end
endmodule
";
        let design = compile_model(MODEL, "pg.va");

        // A 1 V source across the model alone: I(V1) is -1/rr, so the current *is* the answer.
        let current = |deck: &str| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs() // I(V1) follows the node unknowns
        };

        let given = current(
            "V1 a gnd DC 1.0
X1 a gnd pg r=1000
.op
.end
",
        );
        let not_given = current(
            "V1 a gnd DC 1.0
X1 a gnd pg
.op
.end
",
        );

        assert!(
            (given - 1e-3).abs() < 1e-9,
            "`r=1000` on the device line must read as given (1 kOhm), got I = {given}"
        );
        assert!(
            (not_given - 1e-6).abs() < 1e-12,
            "an absent `r` must read as not given (1 MOhm), got I = {not_given}"
        );
        assert!(
            given / not_given > 100.0,
            "the two branches must be distinguishable: {given} vs {not_given}"
        );
    }

    /// An `R`/`C`/`L` line's SPICE positional value sets the model's *first* parameter, so it
    /// gives that parameter just as surely as the named `name=value` form does. Checked against
    /// the same model placed with no value at all, via the general `X` form.
    #[test]
    fn a_positional_value_also_counts_as_given() {
        const MODEL: &str = "
module resistor(p, n);
  inout p, n;
  electrical p, n;
  parameter real r = 1000.0;
  real rr;
  analog begin
    if ($param_given(r)) rr = r;
    else rr = 1.0e6;
    I(p, n) <+ V(p, n) / rr;
  end
endmodule
";
        let design = compile_model(MODEL, "resistor.va");
        let current = |deck: &str| -> f64 {
            let net = va_netlist::parser::parse(deck).expect("parses");
            let op = solve_dc(&net, &design.modules).expect("solves");
            op.x[net.node_order.len()].abs()
        };
        let positional = current(
            "V1 a gnd DC 1.0
R1 a gnd 2000
.op
.end
",
        );
        assert!(
            (positional - 5e-4).abs() < 1e-9,
            "a positional 2000 must be applied *and* reported as given, got I = {positional}"
        );
        let no_value = current(
            "V1 a gnd DC 1.0
X1 a gnd resistor
.op
.end
",
        );
        assert!(
            (no_value - 1e-6).abs() < 1e-12,
            "the same model placed with no value must read as not given, got I = {no_value}"
        );
    }

    /// An override naming a parameter the model does not declare is an error that names both
    /// the offending parameter and what the model does declare -- silently ignoring it would
    /// leave a deck looking like it set something it did not.
    #[test]
    fn an_unknown_parameter_override_is_rejected_by_name() {
        let src = include_str!("../../../models/diode.va");
        let design = compile_model(src, "diode.va");
        let deck =
            include_str!("../../../circuits/diode_iv_params.net").replace("Is=1e-12", "Isat=1e-12");
        let net = va_netlist::parser::parse(&deck).expect("parses -- the name is only checked                                                            against the model, not the grammar");
        let sweep = net.dc.clone().expect("`.dc` card");
        let err = solve_dc_sweep(&net, &design.modules, &sweep)
            .expect_err("`Isat` is not a parameter of models/diode.va");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Isat"),
            "the error should name the bad parameter: {msg}"
        );
        assert!(
            msg.contains("Is"),
            "and list what the model does declare: {msg}"
        );
    }

    /// `PULSE`'s shape, pinned point by point against its own definition: `v1` before the
    /// delay, a linear ramp over `tr`, the `v2` plateau, a linear fall over `tf`, `v1` again,
    /// and the whole thing repeating on `per`. Every boundary is checked from both sides,
    /// since off-by-one-segment errors are the failure mode a mid-segment check would miss.
    #[test]
    fn a_pulse_waveform_follows_its_definition_segment_by_segment() {
        let w = va_netlist::Waveform::Pulse {
            v1: 1.0,
            v2: 5.0,
            td: 10.0,
            tr: 2.0,
            tf: 4.0,
            pw: 8.0,
            per: 100.0,
        };
        let at = |t: f64| waveform_value(w, t);

        assert_eq!(at(0.0), 1.0, "before the delay");
        assert_eq!(at(9.999), 1.0, "still v1 right up to td");
        assert_eq!(
            at(10.0),
            1.0,
            "the ramp starts at td, so it is still v1 there"
        );
        assert!((at(11.0) - 3.0).abs() < 1e-12, "halfway up the rise");
        assert!((at(12.0) - 5.0).abs() < 1e-12, "top of the rise");
        assert!((at(15.0) - 5.0).abs() < 1e-12, "on the plateau");
        assert!((at(20.0) - 5.0).abs() < 1e-12, "plateau ends at td+tr+pw");
        assert!((at(22.0) - 3.0).abs() < 1e-12, "halfway down the fall");
        assert!(
            (at(24.0) - 1.0).abs() < 1e-12,
            "back to v1 at the end of the fall"
        );
        assert_eq!(at(50.0), 1.0, "idle until the period repeats");
        // Second cycle: the same shape, shifted by `per`.
        assert!(
            (at(111.0) - 3.0).abs() < 1e-12,
            "halfway up the second rise"
        );
        assert!(
            (at(122.0) - 3.0).abs() < 1e-12,
            "halfway down the second fall"
        );
    }

    /// A non-positive period means a single pulse: SPICE decks write `per` as 0 (or omit it)
    /// for a one-shot, and treating that as "repeat every 0 seconds" would divide by zero or
    /// spin. Also covers the ideal-edge case `tr = 0`, which must not divide by zero either.
    #[test]
    fn a_pulse_can_be_a_single_shot_with_ideal_edges() {
        let one_shot = va_netlist::Waveform::Pulse {
            v1: 0.0,
            v2: 2.0,
            td: 5.0,
            tr: 0.0,
            tf: 0.0,
            pw: 5.0,
            per: 0.0,
        };
        let at = |t: f64| waveform_value(one_shot, t);
        assert_eq!(at(4.9), 0.0);
        assert_eq!(at(5.0), 2.0, "a zero rise time is an ideal step at td");
        assert_eq!(at(9.9), 2.0);
        assert_eq!(at(10.0), 0.0, "a zero fall time drops instantly");
        assert_eq!(at(1e6), 0.0, "and never repeats");
        assert!(at(1e6).is_finite(), "no division by a zero period");
    }

    /// End to end: the RC's response to a real `PULSE` source, checked against the analytic
    /// charging law rather than against a golden file (`circuits/rc_pulse.net` is deliberately
    /// not gated against QSPICE -- see its own header comment). During the plateau the
    /// capacitor charges toward 5 V with tau = RC, so the *ratio* of successive gaps to 5 V
    /// must decay as exp(-dt/RC) -- a parameter-free check that needs no absolute reference.
    #[test]
    fn a_pulse_driven_rc_charges_with_its_own_time_constant() {
        let deck = include_str!("../../../circuits/rc_pulse.net");
        let net = va_netlist::parser::parse(deck).expect("parse rc_pulse");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");

        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");
        let rc = 1000.0 * 100e-9;

        // Two points well inside the plateau (which runs 120us..520us), far enough from the
        // rising edge that the source really is holding 5 V.
        let sample = |t_query: f64| {
            let i =
                wf.t.iter()
                    .position(|&t| t >= t_query)
                    .expect("a sample at or past the query time");
            (wf.t[i], wf.x[i][out])
        };
        let (t1, v1) = sample(200e-6);
        let (t2, v2) = sample(300e-6);
        let ratio = (5.0 - v2) / (5.0 - v1);
        let expected = (-(t2 - t1) / rc).exp();
        let rel = (ratio - expected).abs() / expected;
        assert!(
            rel < 1e-2,
            "plateau charging ratio {ratio:e} vs exp(-dt/RC) {expected:e} (rel {rel:e})"
        );

        // Before the delay nothing has happened, and by the end of the plateau the capacitor
        // is within a millivolt of the source: 400us of plateau is four time constants.
        assert!(sample(50e-6).1.abs() < 1e-9, "idle before td");
        assert!(
            (sample(515e-6).1 - 5.0).abs() < 0.1,
            "should be near 5 V by the end of the plateau, got {}",
            sample(515e-6).1
        );
        // And between pulses it discharges with the same time constant, toward zero. Checked
        // the same parameter-free way: the ratio of two late samples is exp(-dt/RC), whatever
        // the level happens to be. (At 950us the fall ended 410us ago, so ~4.1 time constants
        // of decay leaves tens of millivolts -- "near zero" would be the wrong assertion.)
        let (t3, v3) = sample(700e-6);
        let (t4, v4) = sample(800e-6);
        let decay = v4 / v3;
        let expected_decay = (-(t4 - t3) / rc).exp();
        let rel = (decay - expected_decay).abs() / expected_decay;
        assert!(
            rel < 1e-2,
            "inter-pulse decay {decay:e} vs exp(-dt/RC) {expected_decay:e} (rel {rel:e})"
        );
        let plateau = sample(515e-6).1;
        assert!(
            v4 < 0.2 * plateau,
            "should have discharged well below the plateau it reached: {v4} vs {plateau}"
        );
    }

    /// An inductor's initial condition is a *current*, seeded on its own branch row rather
    /// than across its terminals. `rl_decay.net` has no source, so as with `rc_discharge.net`
    /// the whole waveform is driven by that seed -- and dropping it would leave the branch
    /// current at zero and every node voltage flat for the entire run.
    #[test]
    fn an_inductor_initial_current_drives_a_source_free_decay() {
        let deck = include_str!("../../../circuits/rl_decay.net");
        let net = va_netlist::parser::parse(deck).expect("parse rl_decay");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");

        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");
        let (r, l, i0) = (10.0_f64, 1e-3_f64, 1e-3_f64);
        let tau = l / r;

        // V(out) = -i(t)*R with i(t) = i0*exp(-t/tau), checked at one and two time constants.
        // The t=0 sample is the caller's own unsolved seed, whose node voltage has not yet
        // been reconciled with the seeded branch current (the sample `va-harness` also
        // excludes from a golden comparison), so it is deliberately not asserted on.
        for &n_tau in &[1.0_f64, 2.0] {
            let i =
                wf.t.iter()
                    .position(|&t| t >= n_tau * tau)
                    .expect("a sample at or past the query time");
            let expected = -i0 * r * (-wf.t[i] / tau).exp();
            let rel = (wf.x[i][out] - expected).abs() / expected.abs();
            assert!(
                rel < 5e-3,
                "at t={} ({n_tau} tau): {} vs analytic {expected} (rel {rel:e})",
                wf.t[i],
                wf.x[i][out]
            );
        }

        // The first *solved* point already reflects the seeded current; a run that ignored
        // IC= would sit at exactly zero for the whole waveform.
        assert!(
            (wf.x[1][out] + i0 * r).abs() < 1e-4,
            "first solved sample should reflect the seeded current, got {}",
            wf.x[1][out]
        );
    }

    /// An inductor is a second-order element, so the strongest cheap check is the closed-form
    /// step response of the series RLC it forms: peak overshoot and ringing frequency both
    /// follow from `zeta` and `w0` alone. A first-order stamp, a missing flux term, or a sign
    /// error on the constitutive row cannot reproduce either.
    #[test]
    fn an_inductor_gives_a_series_rlc_its_textbook_ringing() {
        let deck = include_str!("../../../circuits/rlc_ring.net");
        let net = va_netlist::parser::parse(deck).expect("parse rlc_ring");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");

        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");

        let (r, l, c) = (10.0_f64, 1e-3_f64, 1e-6_f64);
        let w0 = 1.0 / (l * c).sqrt();
        let zeta = 0.5 * r * (c / l).sqrt();

        // Peak overshoot of an underdamped second-order step: 1 + exp(-pi*zeta/sqrt(1-zeta^2)).
        let expected_peak =
            5.0 * (1.0 + (-std::f64::consts::PI * zeta / (1.0 - zeta * zeta).sqrt()).exp());
        let peak =
            wf.x.iter()
                .map(|row| row[out])
                .fold(f64::NEG_INFINITY, f64::max);
        let rel = (peak - expected_peak).abs() / expected_peak;
        assert!(
            rel < 5e-3,
            "overshoot {peak} vs closed form {expected_peak} (rel {rel:e})"
        );

        // Damped ringing period: 2*pi/(w0*sqrt(1-zeta^2)). Measured between the first two
        // upward crossings of the 5 V final value, which is where the waveform is steepest and
        // the crossing time least sensitive to sampling.
        let t_cross: Vec<f64> =
            wf.t.windows(2)
                .zip(wf.x.windows(2))
                .filter(|(_, xs)| xs[0][out] < 5.0 && xs[1][out] >= 5.0)
                .map(|(ts, xs)| {
                    let frac = (5.0 - xs[0][out]) / (xs[1][out] - xs[0][out]);
                    ts[0] + frac * (ts[1] - ts[0])
                })
                .collect();
        assert!(
            t_cross.len() >= 2,
            "expected at least two rising crossings of the final value, got {}",
            t_cross.len()
        );
        let measured_period = t_cross[1] - t_cross[0];
        let expected_period = 2.0 * std::f64::consts::PI / (w0 * (1.0 - zeta * zeta).sqrt());
        let rel = (measured_period - expected_period).abs() / expected_period;
        assert!(
            rel < 5e-3,
            "ringing period {measured_period:e} vs closed form {expected_period:e} (rel {rel:e})"
        );

        // At DC an inductor is a short and a capacitor an open, so the run must settle at the
        // full source voltage rather than at a divider fraction of it.
        let settled = wf.x.last().expect("a last point")[out];
        assert!(
            (settled - 5.0).abs() < 0.15,
            "should ring down toward the 5 V source, ended at {settled}"
        );
    }

    /// The initial condition must actually *drive* the run, not merely parse. `rc_discharge.net`
    /// has no source at all, so if `IC=` were ignored the circuit would sit at 0 V forever and
    /// every sample would be zero -- the strongest available discrimination for this feature.
    /// Checked against the closed form `V(t) = 5*exp(-t/RC)` at three points, not just at t=0.
    #[test]
    fn an_initial_condition_drives_a_source_free_discharge() {
        let deck = include_str!("../../../circuits/rc_discharge.net");
        let net = va_netlist::parser::parse(deck).expect("parse rc_discharge");
        let wf = solve_transient(&net, &[], Integration::Trapezoidal).expect("integrates");

        let out = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("`out` node");
        let rc = 1000.0 * 1e-6;

        assert!(
            (wf.x[0][out] - 5.0).abs() < 1e-9,
            "the run must start at the initial condition, got {}",
            wf.x[0][out]
        );
        for &tau in &[1.0, 2.5, 5.0] {
            let t_query = tau * rc;
            let i =
                wf.t.iter()
                    .position(|&t| t >= t_query)
                    .expect("sample at or past the query time");
            let expected = 5.0 * (-wf.t[i] / rc).exp();
            let rel = (wf.x[i][out] - expected).abs() / expected;
            assert!(
                rel < 1e-3,
                "at t={} ({tau} tau): {} vs analytic {expected} (rel {rel:e})",
                wf.t[i],
                wf.x[i][out]
            );
        }
    }

    /// End-to-end DC sweep of `circuits/diode_clamp.net`: the nonlinear half of ladder rung 2.
    /// Where `diode_iv.net` forces its only node directly (so every node voltage there is a
    /// straight line and only `I(V1)` sees the diode), the series resistor here puts the
    /// exponential *into* a node voltage. Checks the closed form KCL at `mid` holds at every
    /// swept point, and that `V(mid)` genuinely bends: tracking `Vin` below the knee, clamping
    /// well below it once the diode conducts.
    #[test]
    fn diode_clamp_sweep_is_nonlinear_in_a_node_voltage() {
        let src = include_str!("../../../models/diode.va");
        let design = compile_model(src, "diode.va");

        let deck = include_str!("../../../circuits/diode_clamp.net");
        let net = va_netlist::parser::parse(deck).expect("parse diode_clamp");
        let sweep = net.dc.clone().expect("`.dc` sweep card");
        let points = solve_dc_sweep(&net, &design.modules, &sweep).expect("solve clamp sweep");
        assert_eq!(points.len(), 41); // 0.00, 0.05, ..., 2.00

        let mid_idx = net
            .node_order
            .iter()
            .position(|n| n == "mid")
            .expect("`mid` node");

        let is = 1e-14_f64;
        let vt = va_codegen::VT;
        let r = 1000.0_f64;
        for (v, op) in &points {
            let vmid = op.x[mid_idx];
            // KCL at `mid`: the resistor current in equals the diode current out.
            let i_r = (v - vmid) / r;
            let i_d = is * ((vmid / vt).exp() - 1.0);
            let tol = 1e-12_f64.max(i_d.abs() * 1e-6);
            assert!(
                (i_r - i_d).abs() < tol,
                "KCL at mid violated at V1={v}: I(R1)={i_r}, I(D1)={i_d}"
            );
        }

        // Below the knee the diode is off, so R1 drops ~nothing and V(mid) follows V1 ...
        let (v_low, op_low) = &points[4]; // V1 = 0.20
        assert!(
            (op_low.x[mid_idx] - v_low).abs() < 1e-6,
            "V(mid)={} should track V1={v_low} below the knee",
            op_low.x[mid_idx]
        );
        // ... and past it the curve clamps: 2 V in, but nothing like 2 V at `mid`.
        let (v_high, op_high) = points.last().expect("a last point");
        let vmid_high = op_high.x[mid_idx];
        assert!(
            (0.6..0.75).contains(&vmid_high),
            "V(mid)={vmid_high} at V1={v_high} should sit at the diode's knee"
        );
        // The defining property of this circuit, stated as an assertion: the sweep is *not*
        // a straight line. A chord from the first point to the last would predict far more.
        assert!(
            v_high - vmid_high > 1.0,
            "R1 should absorb over a volt at V1={v_high}, got {}",
            v_high - vmid_high
        );
    }

    /// End-to-end DC (ladder rung 5): compile `models/mosfet.va` and solve `circuits/mos_dc.net`
    /// — an NMOS common-source bias point through the real frontend → codegen → core pipeline.
    #[test]
    fn mos_dc_solves_through_codegen_pipeline() {
        let src = include_str!("../../../models/mosfet.va");
        let design = compile_model(src, "mosfet.va");
        assert_eq!(design.modules.len(), 1);
        assert_eq!(design.modules[0].name, "mosfet");

        let deck = include_str!("../../../circuits/mos_dc.net");
        let net = va_netlist::parser::parse(deck).expect("parse mos_dc");
        let op = solve_dc(&net, &design.modules).expect("solve mos_dc");

        // node_order: vdd, g, d (first-seen order; gnd is the reference sentinel).
        let vdd_idx = 0;
        let g_idx = 1;
        let d_idx = 2;
        assert!(
            (op.x[vdd_idx] - 5.0).abs() < 1e-9,
            "V(vdd) = {}",
            op.x[vdd_idx]
        );
        assert!((op.x[g_idx] - 2.0).abs() < 1e-9, "V(g) = {}", op.x[g_idx]);

        // Hand-derived fixed point (see circuits/mos_dc.net's own comment): with Vgs = 2.0 V
        // fixed (vto = 0.7, so Vov = 1.3 V) and the drain node solving
        // `(VDD - Vd)/RD = 0.5*kp*(w/l)*Vov^2*(1 + lambda*Vd)` (Vds = Vd, since the source is
        // tied to gnd), `Vd = 3.31 / 1.0169 = 3.254991...` — well inside saturation
        // (Vd > Vov), confirming the region-selection branch Newton actually lands in.
        let expected_vd = 3.31 / 1.0169;
        assert!(
            (op.x[d_idx] - expected_vd).abs() < 1e-6,
            "V(d) = {}, expected {expected_vd}",
            op.x[d_idx]
        );
    }

    #[test]
    fn divider_solves_with_reference_models() {
        let op = solve_divider(&[]);
        let in_idx = 0; // node_order: in, mid
        let mid_idx = 1;
        assert!(
            (op.x[in_idx] - 1.0).abs() < 1e-9,
            "V(in) = {}",
            op.x[in_idx]
        );
        assert!(
            (op.x[mid_idx] - 0.5).abs() < 1e-9,
            "V(mid) = {}",
            op.x[mid_idx]
        );
    }

    #[test]
    fn divider_solves_through_codegen_pipeline() {
        // Compile the real resistor.va and use the generated model for the R devices.
        let src = include_str!("../../../models/resistor.va");
        let design = compile_model(src, "resistor.va");
        assert_eq!(design.modules.len(), 1);
        assert_eq!(design.modules[0].name, "resistor");
        let op = solve_divider(&design.modules);
        assert!((op.x[1] - 0.5).abs() < 1e-9, "V(mid) = {}", op.x[1]);
    }

    /// § nature-metadata wiring, end to end: with `models/disciplines.vams` on the include
    /// path, `resistor.va`'s two `electrical` nodes pick up a real `abstol` (the LRM-standard
    /// `Voltage` nature's `1e-6`) — and the DC answer is unaffected (a linear divider solves to
    /// the same exact operating point regardless of the Newton convergence tolerance used to
    /// declare it), confirming this is purely a convergence-aid change, not a modeling one.
    #[test]
    fn divider_solves_unchanged_with_disciplines_metadata_resolved() {
        let src = include_str!("../../../models/resistor.va");
        let include_dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))];
        let design = va_frontend::compile_with_includes(src, &include_dirs)
            .expect("compile resistor.va with disciplines.vams resolved");
        assert_eq!(design.modules.len(), 1);
        assert!(
            design.modules[0]
                .nodes
                .iter()
                .all(|n| n.abstol == Some(1e-6)),
            "both of resistor.va's electrical nodes should resolve Voltage's abstol: {:?}",
            design.modules[0].nodes
        );

        let op = solve_divider(&design.modules);
        assert!((op.x[0] - 1.0).abs() < 1e-9, "V(in) = {}", op.x[0]);
        assert!((op.x[1] - 0.5).abs() < 1e-9, "V(mid) = {}", op.x[1]);
    }

    /// End-to-end DC through module instantiation (§ module instantiation): `series_divider`
    /// (two `leg` instances in series, sharing a parent-declared internal node, one connected
    /// positionally and one by name with a parameter override — see `models/series_divider.va`)
    /// is compiled and used as a single 2 kΩ device between the source and the outer divider's
    /// mid node, in series with a plain 1 kΩ resistor. No mocking: this drives the real
    /// frontend → codegen → core pipeline exactly as `divider_solves_through_codegen_pipeline`
    /// does, just with a hierarchical model.
    /// V(mid) = Vin * R2/(R_series + R2) = 1.0 * 1000/(2000 + 1000) = 1/3 V.
    #[test]
    fn hierarchical_divider_solves_through_codegen_pipeline() {
        let src = include_str!("../../../models/series_divider.va");
        let include_dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))];
        let design = va_frontend::compile_with_includes(src, &include_dirs)
            .expect("compile series_divider.va");
        assert_eq!(
            design.modules.len(),
            2,
            "leg.va's `leg` plus `series_divider`"
        );
        assert!(design.modules.iter().any(|m| m.name == "series_divider"));

        let deck = include_str!("../../../circuits/hier_divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse hier_divider");
        let op = solve_dc(&net, &design.modules).expect("solve hier_divider");

        let mid_idx = net
            .node_order
            .iter()
            .position(|n| n == "mid")
            .expect("mid node");
        assert!(
            (op.x[mid_idx] - 1.0 / 3.0).abs() < 1e-9,
            "V(mid) = {}",
            op.x[mid_idx]
        );
    }

    /// `verilogaLib-master/ohmmeter.va`'s `I(iprobe)` — a single-terminal implicit-ground probe
    /// of a branch that receives no contribution of its own anywhere, entirely distinct from the
    /// explicit `V(dutm,iprobe) <+ 0;` branch it shares node `iprobe` with (see
    /// `va_codegen::lower::NodeKclProbe`'s doc comment) — now lowers through the real pipeline
    /// instead of being rejected as unsupported. `ohmmeter` is an instrument model (its ports
    /// don't correspond to any circuit this repo has a netlist for), so this only exercises
    /// frontend → codegen, not a full DC solve.
    ///
    /// The idiom is written out here rather than `include_str!`'d from `external/`: that
    /// directory is gitignored, so the original form of this test made the whole `va-cli` test
    /// target fail to *compile* on any fresh checkout — found by the first CI run, 2026-09-11.
    /// Nothing under `external/` may be a compile-time or run-time dependency of the suite.
    #[test]
    fn ohmmeter_probe_compiles_through_codegen() {
        // The load-bearing lines of the corpus file, in its own shape: an explicit zero-volt
        // branch to `iprobe`, and a bare `I(iprobe)` read of the current that branch carries.
        let src = r#"`include "constants.vams"
`include "disciplines.vams"
module ohmmeter(dutp, dutm, iprobe, r, g);
input dutp, iprobe;
output dutm, r, g;
electrical dutp, dutm, iprobe, r, g;
parameter real max_resistance = 1k;
real r_val, g_val;
analog begin
    V(dutm, iprobe) <+ 0;
    r_val = V(dutp, dutm) / I(iprobe);
    g_val = I(iprobe) / V(dutp, dutm);
    if (r_val > max_resistance) begin
        r_val = max_resistance;
    end
    V(r) <+ r_val;
    V(g) <+ g_val;
end
endmodule
"#;
        let include_dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))];
        let design =
            va_frontend::compile_with_includes(src, &include_dirs).expect("compile ohmmeter");
        assert_eq!(design.modules.len(), 1);
        let module = &design.modules[0];
        assert_eq!(module.name, "ohmmeter");

        let terminals: Vec<usize> = (0..module.nodes.len()).collect();
        let mut next_unknown = module.nodes.len();
        va_codegen::build_instance(module, &terminals, &mut next_unknown)
            .expect("ohmmeter.va's I(iprobe) node-KCL probe should now lower");
    }

    #[test]
    fn transient_deck_is_rejected() {
        let deck = include_str!("../../../circuits/rectifier.net");
        let net = va_netlist::parser::parse(deck).expect("parse rectifier");
        assert!(gate_analysis(&net, Analysis::Dc).is_err());
    }

    #[test]
    fn ac_deck_needs_the_ac_flag_and_an_ac_card() {
        let deck = include_str!("../../../circuits/rc_ac.net");
        let net = va_netlist::parser::parse(deck).expect("parse rc_ac");
        assert_eq!(net.analysis, AnalysisCard::Ac);
        // The deck says `.ac`, so a default DC run must not silently solve something else.
        assert!(gate_analysis(&net, Analysis::Dc).is_err());
        gate_analysis(&net, Analysis::Ac).expect("AC analysis is accepted");

        // Asking for AC on a deck with no `.ac` card is a clear error, not a guessed grid.
        let divider = va_netlist::parser::parse(include_str!("../../../circuits/divider.net"))
            .expect("parse divider");
        assert!(gate_analysis(&divider, Analysis::Ac).is_err());
    }

    /// End-to-end AC (T5) through the real pipeline: parse `rc_ac.net`, build reference
    /// instances, solve the DC point, linearize, sweep. Checked against the closed-form
    /// `H(jω) = 1/(1 + jωRC)` the network itself implements — the same closed form
    /// `va_acnoise::ac`'s own unit test uses, but reached here from a netlist file exactly the
    /// way `va-cli sim circuits/rc_ac.net --ac` does, through `.ac`/`AC 1` deck parsing and the
    /// branch-row excitation vector rather than a hand-built instance list.
    #[test]
    fn rc_ac_solves_through_the_real_pipeline() {
        let deck = include_str!("../../../circuits/rc_ac.net");
        let net = va_netlist::parser::parse(deck).expect("parse rc_ac");
        let response = solve_ac(&net, &[]).expect("AC sweep");

        let in_idx = net.node_order.iter().position(|n| n == "in").unwrap();
        let out_idx = net.node_order.iter().position(|n| n == "out").unwrap();
        assert_eq!(response.f.len(), response.x.len());
        assert!(response.f.len() > 50, "1 Hz..1 MHz at 10/decade");

        let (r, c) = (1000.0, 1e-6);
        for (&f, x) in response.f.iter().zip(&response.x) {
            // The source itself is held at exactly its own 1 V∠0° excitation.
            let (in_re, in_im) = x[in_idx];
            assert!(
                (in_re - 1.0).abs() < 1e-9 && in_im.abs() < 1e-9,
                "V(in) at f={f} = {in_re}+{in_im}j, expected 1+0j"
            );

            let wrc = 2.0 * PI * f * r * c;
            let expected_mag = 1.0 / (1.0 + wrc * wrc).sqrt();
            let expected_phase = -wrc.atan();
            let got_mag = va_acnoise::ac::magnitude(x[out_idx]);
            let got_phase = va_acnoise::ac::phase(x[out_idx]);
            assert!(
                (got_mag - expected_mag).abs() < 1e-9,
                "f={f}: |V(out)| = {got_mag}, expected {expected_mag}"
            );
            assert!(
                (got_phase - expected_phase).abs() < 1e-9,
                "f={f}: ∠V(out) = {got_phase}, expected {expected_phase}"
            );
        }
    }

    /// End-to-end AC through the codegen pipeline at a *nonlinear* operating point (T5):
    /// `circuits/diode_ac.net` compiles `models/diode.va` and biases `D1` at ~0.7 V, so the
    /// answer depends on the diode's own AD-derived small-signal conductance
    /// `gd = Is/(N·Vt)·exp(Vd/(N·Vt))`, not just R/C stamps. Checked against the closed form for
    /// the resulting `R1`-into-`(rd ∥ C1)` network, computed from the *solved* diode voltage
    /// (which is itself an operating-point result, not a hand-assumed 0.7 V — `R1` drops some of
    /// the source's 0.7 V).
    #[test]
    fn diode_ac_solves_through_the_codegen_pipeline_at_its_bias() {
        let src = include_str!("../../../models/diode.va");
        let design = compile_model(src, "diode.va");
        let deck = include_str!("../../../circuits/diode_ac.net");
        let net = va_netlist::parser::parse(deck).expect("parse diode_ac");

        let op = solve_dc(&net, &design.modules).expect("DC bias");
        let a_idx = net.node_order.iter().position(|n| n == "a").unwrap();
        let vd = op.x[a_idx];
        // A forward-biased diode sits in the usual few-hundred-mV band, well under the source's
        // own 0.7 V (R1 drops the rest) — confirms this is a genuinely nonlinear bias point, not
        // a degenerate one where the check below would pass trivially.
        assert!(
            (0.3..0.7).contains(&vd),
            "V(a) = {vd}, expected a real forward bias"
        );

        let response = solve_ac(&net, &design.modules).expect("AC sweep");
        // diode.va's own defaults (Is = 1e-14, N = 1) and va-codegen's thermal voltage.
        let gd = 1e-14 / va_codegen::VT * (vd / va_codegen::VT).exp();
        let (r1, c1) = (1000.0, 1e-7);
        for (&f, x) in response.f.iter().zip(&response.x) {
            // Small-signal divider: V(a)/V(in) = Y_load⁻¹ / (R1 + Y_load⁻¹) with
            // Y_load = gd + jωC1, i.e. V(a)/V(in) = 1 / (1 + R1·(gd + jωC1)).
            let (dre, dim) = (1.0 + r1 * gd, r1 * 2.0 * PI * f * c1);
            let expected_mag = 1.0 / (dre * dre + dim * dim).sqrt();
            let got_mag = va_acnoise::ac::magnitude(x[a_idx]);
            assert!(
                (got_mag - expected_mag).abs() <= 1e-6 * expected_mag.max(1e-12),
                "f={f}: |V(a)| = {got_mag}, expected {expected_mag}"
            );
        }
    }

    #[test]
    fn ac_analysis_needs_an_excited_source() {
        // Same RC network, but the source carries no `AC` token: the system is homogeneous and
        // would solve to an all-zero response at every frequency. That's a clear error, not a
        // silently useless answer.
        let deck = "* no ac source\nV1 in gnd DC 1\nR1 in out 1000\nC1 out gnd 1e-6\n\
                    .ac dec 10 1 1meg\n.end\n";
        let net = va_netlist::parser::parse(deck).expect("parse");
        assert!(solve_ac(&net, &[]).is_err());
    }

    /// End-to-end transient through the real pipeline: parse `rc_step.net`, build reference
    /// instances, integrate. V(out) = Vs·(1 − e^(−t/RC)), RC = 1 ms, matching
    /// `va-transient`'s own analytic RC test but now driven from a netlist file exactly the
    /// way `va-cli sim circuits/rc_step.net --tran` does.
    #[test]
    fn rc_step_solves_through_the_real_pipeline() {
        let deck = include_str!("../../../circuits/rc_step.net");
        let net = va_netlist::parser::parse(deck).expect("parse rc_step");
        assert_eq!(net.analysis, AnalysisCard::Tran);
        gate_analysis(&net, Analysis::Transient).expect("transient analysis is accepted");

        let wf = solve_transient(&net, &[], Integration::default()).expect("integrates");
        let out_idx = net
            .node_order
            .iter()
            .position(|n| n == "out")
            .expect("out node");

        let rc = 1e-3;
        let vs = 5.0;
        // Near t = RC: analytic V(out) = Vs·(1 - e^-1).
        let (t_near_rc, v_near_rc) =
            wf.t.iter()
                .zip(&wf.x)
                .map(|(&t, x)| (t, x[out_idx]))
                .find(|&(t, _)| t >= rc)
                .expect("a sample at or past t=RC");
        let analytic_at_rc = vs * (1.0 - (-t_near_rc / rc).exp());
        assert!(
            (v_near_rc - analytic_at_rc).abs() / vs < 1e-2,
            "V(out)={v_near_rc} at t={t_near_rc} vs analytic {analytic_at_rc}"
        );

        // By t=tstop (5 RC) it should have settled near Vs.
        let v_final = *wf.x.last().unwrap().get(out_idx).unwrap();
        assert!(
            (v_final - vs).abs() / vs < 1e-2,
            "should have settled near Vs: {v_final}"
        );
    }

    /// End-to-end half-wave rectifier through the real pipeline, from `circuits/rectifier.net`
    /// (a 1 kHz/5 V `SIN` source, a diode, and an RC load) — exactly what
    /// `va-cli sim circuits/rectifier.net --tran` runs. Rectification is checked qualitatively
    /// (no golden reference exists yet — that's `va-harness`, still `todo!()`): the diode
    /// should keep `out` from ever following `in`'s negative excursions, and the output should
    /// reach close to the input's peak minus a silicon diode drop.
    #[test]
    fn rectifier_solves_through_the_real_pipeline() {
        let deck = include_str!("../../../circuits/rectifier.net");
        let net = va_netlist::parser::parse(deck).expect("parse rectifier");
        assert_eq!(net.analysis, AnalysisCard::Tran);
        gate_analysis(&net, Analysis::Transient).expect("transient analysis is accepted");

        // Confirm this deck actually exercises the time-varying path being tested.
        let v1 = net.devices.iter().find(|d| d.name == "V1").unwrap();
        assert!(matches!(
            v1.waveform,
            Some(va_netlist::Waveform::Sin { .. })
        ));

        let wf = solve_transient(&net, &[], Integration::default()).expect("integrates");
        let in_idx = net.node_order.iter().position(|n| n == "in").unwrap();
        let out_idx = net.node_order.iter().position(|n| n == "out").unwrap();

        let in_min = wf.x.iter().map(|x| x[in_idx]).fold(f64::INFINITY, f64::min);
        let out_min =
            wf.x.iter()
                .map(|x| x[out_idx])
                .fold(f64::INFINITY, f64::min);
        let out_max =
            wf.x.iter()
                .map(|x| x[out_idx])
                .fold(f64::NEG_INFINITY, f64::max);

        // The source genuinely swings negative (proves the SIN waveform is really driving the
        // circuit, not silently stuck at its DC offset of 0 V).
        assert!(in_min < -4.0, "V(in) should swing well negative: {in_min}");
        // The diode blocks it: `out` never follows, staying close to (well above) zero.
        assert!(
            out_min > -0.1,
            "half-wave rectifier output went negative: {out_min}"
        );
        // The output reaches near the input's peak (5 V) minus a silicon diode drop.
        assert!(
            (3.5..5.0).contains(&out_max),
            "V(out) peak out of range: {out_max}"
        );
    }

    /// End-to-end ring oscillator (§ ladder rung 6) through the real netlist pipeline —
    /// `circuits/ring_osc.net`'s `Q1`/`Q2`/`Q3` lines exercise `va-netlist`'s new `'Q'` element
    /// arm and this module's new `"bjt"` `reference_instance` branch, both added to close this
    /// rung. No `.ic`/`UIC` support was needed after all, despite
    /// `va_transient::integrator::ring_oscillator_sustains_oscillation`'s hand-built fixture
    /// starting from a *perturbed DC operating point*, not `x=0`: cold-starting from `x=0`
    /// charges every stage to nearly the same forward-active bias within ~12.5 µs (all three
    /// stages see an identical `Vbe` from `x=0`), landing close enough to the ring's own
    /// symmetric-but-unstable equilibrium that the stages' deliberately mismatched `R` values
    /// (breaking the exact 3-way symmetry, mirroring the hand-built fixture's own reasoning) are
    /// enough to kick off the same genuine, sustained, growing oscillation — confirmed
    /// empirically by inspecting a full run's node trajectories before writing this assertion,
    /// not assumed from the topology alone. Checked across all three collectors, not just one:
    /// each stage's own swing lags the others' (a real ring oscillator's per-stage phase shift),
    /// so any single node can land on a quiet stretch of its own cycle within this `tstop` while
    /// the ring as a whole keeps oscillating — confirmed empirically (`c1`: 2 rail-midpoint
    /// crossings in this window, `c2`: 6, `c3`: 3; checking only `c1` would have been a flaky,
    /// component-value-specific assertion, not a real regression guard).
    #[test]
    fn ring_osc_sustains_oscillation_through_the_real_pipeline() {
        let deck = include_str!("../../../circuits/ring_osc.net");
        let net = va_netlist::parser::parse(deck).expect("parse ring_osc");
        assert_eq!(net.analysis, AnalysisCard::Tran);
        gate_analysis(&net, Analysis::Transient).expect("transient analysis is accepted");

        let wf = solve_transient(&net, &[], Integration::default()).expect("integrates");
        let collector_idxs: Vec<usize> = ["c1", "c2", "c3"]
            .iter()
            .map(|name| net.node_order.iter().position(|n| n == name).unwrap())
            .collect();

        // Sustained oscillation, not a one-off kick that settles: a genuinely oscillating
        // trajectory crosses the rail midpoint repeatedly; a monotonic settle crosses it at
        // most once. At least one collector must cross it several times.
        let mid = 2.5;
        let max_crossings = collector_idxs
            .iter()
            .map(|&idx| {
                let mut crossings = 0;
                let mut above = wf.x[0][idx] >= mid;
                for x in &wf.x[1..] {
                    let now_above = x[idx] >= mid;
                    if now_above != above {
                        crossings += 1;
                        above = now_above;
                    }
                }
                crossings
            })
            .max()
            .unwrap();
        assert!(
            max_crossings >= 4,
            "expected sustained oscillation (several rail-midpoint crossings on some \
             collector), best was {max_crossings}"
        );

        // Real amplitude, not numerical noise around the midpoint: every stage's collector
        // swings across most of the 0-5 V rail at some point in the run.
        for (&idx, name) in collector_idxs.iter().zip(["c1", "c2", "c3"]) {
            let (v_min, v_max) =
                wf.x.iter()
                    .map(|x| x[idx])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    });
            assert!(
                v_min < 0.5,
                "V({name}) min = {v_min}, expected a low excursion"
            );
            assert!(
                v_max > 4.0,
                "V({name}) max = {v_max}, expected a high excursion"
            );
        }
    }

    // --- § portless top-level modules ---------------------------------------------------

    /// The detection rule is exactly "declares no ports", and it is a decision procedure rather
    /// than a heuristic: `va-netlist`'s minimum placeable arity is 1, so a zero-port module is
    /// unplaceable by construction.
    ///
    /// The control that matters is `models/series_divider.va`: it is built from two `leg`
    /// instances and has no `analog` block of its own, so every *other* candidate signal
    /// ("contains instances", "has no analog block") would false-positive on it — yet it is a
    /// perfectly placeable two-port component that `circuits/hier_divider.net` places today.
    #[test]
    fn only_a_portless_module_is_reported_as_unplaceable() {
        let design = compile_model(
            include_str!("../../../models/series_divider.va"),
            "series_divider.va",
        );
        for m in &design.modules {
            assert!(
                !m.ports.is_empty(),
                "`{}` is a real component and must not be flagged",
                m.name
            );
            assert_eq!(unplaceable_clause(m, &[]), "", "`{}` flagged", m.name);
        }

        // A genuinely portless structural circuit is flagged, and the note says what to do.
        let design = compile_model(
            "module smpl_ckt; electrical n; ground electrical gnd; \
             leg #(.R(1000)) R1(n, gnd); endmodule \
             module leg(p, n); parameter real R = 1; inout p, n; electrical p, n; \
             analog I(p, n) <+ V(p, n) / R; endmodule",
            "portless circuit",
        );
        let top = design
            .modules
            .iter()
            .find(|m| m.name == "smpl_ckt")
            .expect("top module");
        assert!(top.ports.is_empty());
        let note = unplaceable_clause(top, &[]);
        assert!(note.contains("no ports"), "{note}");
        assert!(note.contains("deck"), "{note}");

        // ...but not when an `include` was dropped: a truncated distribution whose port list
        // lived in the absent header also elaborates with zero ports, and calling that a
        // self-contained circuit would point at the wrong thing.
        assert_eq!(
            unplaceable_clause(top, &["disciplines.vams".to_string()]),
            ""
        );
    }

    // --- § quantity reporting ------------------------------------------------------------

    /// Every entry of the solution vector is reported, and each is labelled with the access
    /// function and units its *own* discipline declares — not with an assumed `V`/volts.
    #[test]
    fn quantities_use_each_disciplines_own_access_function_and_units() {
        // A deliberately non-electrical model: potential is angular velocity in rads/s.
        let src = "nature Angular_Velocity units=\"rads/s\"; access=Omega; abstol=1e-6; endnature \
                   nature Angular_Force units=\"N-m\"; access=Tau; abstol=1e-6; endnature \
                   discipline rotational_omega potential Angular_Velocity; \
                   flow Angular_Force; enddiscipline \
                   module damper(shaft, ref); inout shaft, ref; \
                   rotational_omega shaft, ref; parameter real d = 0.1; \
                   analog Tau(shaft, ref) <+ d * Omega(shaft, ref); endmodule";
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        // The discipline metadata must survive into Interface α, or reporting has nothing to
        // read: this is the half of the change that lives in `va-ir`.
        let shaft = design.modules[0]
            .nodes
            .iter()
            .find(|n| n.name == "shaft")
            .expect("shaft node");
        assert_eq!(shaft.access.as_deref(), Some("Omega"));
        assert_eq!(shaft.units.as_deref(), Some("rads/s"));

        let net =
            va_netlist::parser::parse("X1 shaft gnd damper d=0.1\n.op\n.end\n").expect("parses");
        let qs = quantities(&net, &design.modules).expect("quantities");
        let shaft_q = qs
            .iter()
            .find(|q| q.name == "shaft")
            .expect("shaft reported");
        assert_eq!(shaft_q.label, "Omega(shaft)");
        assert_eq!(shaft_q.unit, "rads/s");
        // ...and it renders as such, rather than as volts.
        let line = shaft_q.render(20.0);
        assert!(line.contains("Omega(shaft)"), "{line}");
        assert!(line.contains("rads/s"), "{line}");
        assert!(
            !line.contains(" V"),
            "a mechanical node must not be reported in volts: {line}"
        );
    }

    /// A deck of built-in primitives has no discipline preamble anywhere, and those primitives
    /// really are electrical — so the fallback must stay `V`/volts rather than going blank.
    /// Branch currents are reported alongside the nodes, which is what makes `I(V1)` available
    /// to a transient run.
    #[test]
    fn quantities_cover_nodes_and_branch_currents_of_a_primitive_deck() {
        let net = va_netlist::parser::parse(include_str!("../../../circuits/divider.net"))
            .expect("parse divider");
        let qs = quantities(&net, &[]).expect("quantities");
        let labels: Vec<&str> = qs.iter().map(|q| q.label.as_str()).collect();
        assert!(labels.contains(&"V(in)"), "{labels:?}");
        assert!(labels.contains(&"V(mid)"), "{labels:?}");
        // The source's branch current is a quantity like any other -- this is what
        // `report_transient` gained.
        assert!(labels.contains(&"I(V1)"), "{labels:?}");
        // Each index is distinct: the source's branch row must not be reported twice, once as
        // `I(V1)` and again as an auxiliary row.
        let mut idx: Vec<usize> = qs.iter().map(|q| q.index).collect();
        let n = idx.len();
        idx.sort_unstable();
        idx.dedup();
        assert_eq!(idx.len(), n, "a quantity was reported twice: {labels:?}");
        assert!(qs.iter().all(|q| q.unit == "V" || q.unit == "A"), "{qs:?}");
    }

    /// `--report` selects by bare net name or by full label, and refuses a name the circuit
    /// does not compute rather than silently printing nothing.
    #[test]
    fn select_quantities_matches_by_name_or_label_and_rejects_typos() {
        let net = va_netlist::parser::parse(include_str!("../../../circuits/divider.net"))
            .expect("parse divider");
        let all = quantities(&net, &[]).expect("quantities");

        // No selectors: everything, unchanged.
        assert_eq!(select_quantities(&all, &[]).unwrap(), all);

        let by_name = select_quantities(&all, &["mid".to_string()]).unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].label, "V(mid)");
        // The full label selects the same one, so a user may disambiguate when they want to.
        let by_label = select_quantities(&all, &["V(mid)".to_string()]).unwrap();
        assert_eq!(by_label, by_name);
        // Case-insensitive, and a repeated selector does not duplicate the column.
        let dup = select_quantities(&all, &["MID".to_string(), "mid".to_string()]).unwrap();
        assert_eq!(dup, by_name);

        let err = select_quantities(&all, &["midd".to_string()]).expect_err("typo must error");
        assert!(err.to_string().contains("midd"), "{err}");
    }

    /// A power spectral density is in (observed quantity)² per hertz, so a compound unit has to
    /// be bracketed before it carries the exponent — `(rads/s)^2/Hz`, not `rads/s^2/Hz`, which
    /// reads as rads per second-squared per hertz and is a different quantity. A simple unit
    /// keeps the conventional unbracketed spelling.
    #[test]
    fn compound_units_are_bracketed_before_taking_an_exponent() {
        assert_eq!(bracket_unit("V"), "V");
        assert_eq!(bracket_unit("K"), "K");
        assert_eq!(bracket_unit("rads/s"), "(rads/s)");
        assert_eq!(bracket_unit("N-m"), "(N-m)");
        assert_eq!(bracket_unit(""), "");
    }

    /// The case AC and noise reporting exist for: one solution vector holding two disciplines.
    /// Each quantity must carry its *own* access function and units, since a report that called
    /// the mechanical node a voltage would be stating something false about what was solved.
    #[test]
    fn a_mixed_discipline_circuit_reports_each_quantity_in_its_own_terms() {
        let src = "nature Angular_Velocity units=\"rads/s\"; access=Omega; abstol=1e-6; endnature \
                   nature Angular_Force units=\"N-m\"; access=Tau; abstol=1e-6; endnature \
                   discipline rotational_omega potential Angular_Velocity; \
                   flow Angular_Force; enddiscipline \
                   nature Voltage units=\"V\"; access=V; abstol=1u; endnature \
                   nature Current units=\"A\"; access=I; abstol=1p; endnature \
                   discipline electrical potential Voltage; flow Current; enddiscipline \
                   module motor(shaft, p, n); inout shaft, p, n; \
                   rotational_omega shaft; electrical p, n; \
                   parameter real km = 4.5; parameter real kf = 6.2; \
                   parameter real d = 0.1; parameter real r = 5.0; \
                   analog begin \
                   V(p, n) <+ r * I(p, n) + km * Omega(shaft); \
                   Tau(shaft) <+ d * Omega(shaft) - kf * I(p, n); end endmodule";
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let net =
            va_netlist::parser::parse("V1 drive gnd DC 1\nX1 shaft drive gnd motor\n.op\n.end\n")
                .expect("parses");
        let qs = quantities(&net, &design.modules).expect("quantities");

        let shaft = qs.iter().find(|q| q.name == "shaft").expect("shaft");
        assert_eq!(shaft.label, "Omega(shaft)");
        assert_eq!(shaft.unit, "rads/s");
        let drive = qs.iter().find(|q| q.name == "drive").expect("drive");
        assert_eq!(drive.label, "V(drive)");
        assert_eq!(drive.unit, "V");
        // Both disciplines really are in one solution vector, at distinct indices.
        assert_ne!(shaft.index, drive.index);

        // And the circuit solves to its closed form, so those labels sit on real numbers:
        // I = V/(r + km*kf/d), Omega = kf*I/d.
        let op = solve_dc(&net, &design.modules).expect("solves");
        let i_expected = 1.0 / (5.0 + 4.5 * 6.2 / 0.1);
        let omega_expected = 6.2 * i_expected / 0.1;
        assert!(
            (op.x[shaft.index] - omega_expected).abs() < 1e-9,
            "Omega(shaft) = {}, want {omega_expected}",
            op.x[shaft.index]
        );
    }
}
