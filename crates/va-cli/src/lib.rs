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
//! DC (`.op`/`.dc`), transient (`.tran <tstep> <tstop>`), and small-signal AC
//! (`.ac dec <points-per-decade> <fstart> <fstop>`, T5) are implemented; noise is not.
//! Transient always starts from the zero vector — v0 has no `.ic`/`UIC`
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
use va_abi::reference::{diode::VT_NOMINAL, Bjt, Capacitor, Diode, Inductor, Resistor, VSource};
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
        Some(path) => {
            let src =
                std::fs::read_to_string(path).with_context(|| format!("reading model {path}"))?;
            // Resolve `include against the model's own directory.
            let include_dirs: Vec<std::path::PathBuf> = std::path::Path::new(path)
                .parent()
                .map(|p| vec![p.to_path_buf()])
                .unwrap_or_default();
            let design = va_frontend::compile_with_includes(&src, &include_dirs)
                .with_context(|| format!("compiling Verilog-A model {path}"))?;
            for module in &design.modules {
                eprintln!(
                    "[va-cli] compiled Verilog-A module `{}` from {path}",
                    module.name
                );
            }
            design.modules
        }
        None => Vec::new(),
    };

    Ok((net, compiled))
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
/// If `plot` is given, also returns an error if it names a transient run (a DC operating point
/// is a single point, not a waveform — plotting one isn't implemented) or if writing the SVG
/// fails.
pub fn run_sim(
    netlist: &str,
    model: Option<&str>,
    analysis: Analysis,
    plot: Option<&str>,
    integration: Integration,
) -> Result<()> {
    let (net, compiled) = load(netlist, model)?;

    gate_analysis(&net, analysis)?;
    // Plottable analyses are the ones that produce a *curve*: a transient waveform, or a `.dc`
    // sweep. A bare DC operating point is a single point — plotting one would be an empty
    // image, so asking is still a clear error rather than a misleading file.
    if plot.is_some()
        && analysis != Analysis::Transient
        && !(analysis == Analysis::Dc && net.dc.is_some())
    {
        bail!(
            "--plot supports a transient run (--tran) or a `.dc` sweep; a DC operating point is              a single point, not a curve"
        );
    }

    if analysis == Analysis::Transient {
        let wf = solve_transient(&net, &compiled, integration)?;
        report_transient(&net, &wf);
        if let Some(path) = plot {
            plot::plot_transient(path, &net, &wf).with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote transient plot to {path}");
        }
    } else if analysis == Analysis::Ac {
        let response = solve_ac(&net, &compiled)?;
        let currents = branch_currents(&net, &compiled)?;
        report_ac(&net, &currents, &response);
    } else if analysis == Analysis::Noise {
        let spectrum = solve_noise(&net, &compiled)?;
        report_noise(&net, &spectrum);
    } else if let Some(sweep) = &net.dc {
        let points = solve_dc_sweep(&net, &compiled, sweep)?;
        report_sweep(&net, sweep, &points);
        if let Some(path) = plot {
            plot::plot_sweep(path, &net, sweep, &points)
                .with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote sweep plot to {path}");
        }
    } else {
        let op = solve_dc(&net, &compiled)?;
        report(&net, &op.x);
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
}

impl std::ops::AddAssign for CheckTally {
    fn add_assign(&mut self, rhs: Self) {
        self.passed += rhs.passed;
        self.passed_incomplete += rhs.passed_incomplete;
        self.failed += rhs.failed;
        self.failed_incomplete += rhs.failed_incomplete;
        self.no_module += rhs.no_module;
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
    /// Every `` `include `` the preprocessor could not resolve and therefore dropped. A
    /// non-empty list means what parsed is **less than the file says it is**; see
    /// [`va_frontend::preprocess::preprocess_reporting`].
    skipped_includes: Vec<String>,
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
    match va_frontend::parser::parse_with_disciplines_located(&tokens, Some((&src, &offsets)))
        .map(|(asts, _, _)| asts)
    {
        Ok(asts) => Ok(ParsedFile {
            asts,
            skipped_includes,
        }),
        Err(e) => {
            println!("  [parse] {path}: {e}{}", skipped_clause(&skipped_includes));
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
            match va_frontend::elaborate::elaborate_with_library(ast, &library) {
                Ok(m) => {
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
                            all_ok = false;
                            continue;
                        }
                    }
                    println!(
                        "  [ok   ] {file}: module `{}` ({} nodes, {} params, {} funcs){}",
                        m.name,
                        m.nodes.len(),
                        m.params.len(),
                        m.functions.len(),
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
type BuiltInstances = (Vec<Box<dyn ModelInstance>>, usize, Vec<(String, usize)>);

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
    for dev in &net.devices {
        let (inst, branch) = build_instance(dev, compiled, &mut next_unknown)?;
        if let Some(branch) = branch {
            currents.push((dev.name.clone(), branch));
        }
        instances.push(inst);
    }
    Ok((instances, next_unknown, currents))
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
    let (_, _, currents) = build_instances(net, compiled)?;
    Ok(currents)
}

/// Build every device instance and solve the DC operating point. `pub` so `va-harness` can get
/// the numeric [`va_core::dc::OperatingPoint`] back directly (§ golden comparison), rather than
/// parsing [`run_sim`]'s printed stdout.
pub fn solve_dc(net: &Netlist, compiled: &[Module]) -> Result<va_core::dc::OperatingPoint> {
    let (instances, dim, _currents) = build_instances(net, compiled)?;
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    operating_point(&refs, dim, NewtonConfig::default()).context("DC operating-point solve failed")
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
    }
}

/// Build every device instance and integrate the transient response over the deck's
/// `.tran <tstep> <tstop>` window.
///
/// Always starts from the zero vector — v0 has no `.ic`/`UIC` support (this module's doc
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
}

impl Integration {
    /// Parse the `--integration` argument. Accepts `be`/`backward-euler` and `trap`/
    /// `trapezoidal`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "be" | "backward-euler" | "backward_euler" => Some(Self::BackwardEuler),
            "trap" | "trapezoidal" => Some(Self::Trapezoidal),
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

    let (instances, dim, _currents) = build_instances(net, compiled)?;
    let x0 = initial_solution(net, dim);
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();

    va_transient::integrator::run(&refs, dim, x0, cfg).context("transient integration failed")
}

/// The transient run's initial solution vector: zero everywhere, then each capacitor's
/// `IC=<volts>` applied as `V(p) = V(n) + ic` (`va_netlist::Device::ic`).
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
fn initial_solution(net: &Netlist, dim: usize) -> Vec<f64> {
    let mut x0 = vec![0.0; dim];
    for dev in &net.devices {
        let Some(ic) = dev.ic else { continue };
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

    let (instances, dim, currents) = build_instances(net, compiled)?;
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    let op = operating_point(&refs, dim, NewtonConfig::default())
        .context("DC operating-point solve failed (AC analysis linearizes about it)")?;
    let excitation = ac_excitation(net, &currents, dim)?;

    let sweep = va_acnoise::ac::AcSweep {
        fstart: card.fstart,
        fstop: card.fstop,
        points_per_decade: card.points_per_decade,
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

    let (instances, dim, currents) = build_instances(net, compiled)?;
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
        bail!(
            "no device in this circuit contributes any noise, so the spectrum would be \
             identically zero — note that Verilog-A `white_noise()`/`flicker_noise()` is not \
             lowered yet, so a `--model`-compiled device is silent (see va_abi::noise)"
        );
    }

    let sweep = va_acnoise::ac::AcSweep {
        fstart: card.fstart,
        fstop: card.fstop,
        points_per_decade: card.points_per_decade,
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
) -> Result<(Box<dyn ModelInstance>, Option<usize>)> {
    let p = dev.terminals[0];
    let n = dev.terminals[1];

    if dev.model == "vsource" {
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
        return Ok((inst, Some(branch)));
    }

    if dev.model == "inductor" {
        // Like a voltage source, an inductor carries its own branch-current unknown: its row
        // is the constitutive law `-(V(p)-V(n)) + d(L*i)/dt = 0`, not a KCL sum. Claiming the
        // index here (rather than in `va-abi`) keeps `dim` assignment in one place, and
        // returning it as a named current means `I(L1)` reaches golden files exactly as a
        // source's own current does.
        let branch = *next_unknown;
        *next_unknown += 1;
        let inst: Box<dyn ModelInstance> =
            Box::new(Inductor::new(p, n, branch, dev.value.unwrap_or(0.0)));
        return Ok((inst, Some(branch)));
    }

    // Use the compiled Verilog-A model when its name matches the device's model.
    if let Some(module) = compiled.iter().find(|m| m.name == dev.model) {
        return Ok((
            build_from_model(module, dev.value, &dev.terminals, next_unknown)?,
            None,
        ));
    }

    Ok((reference_instance(dev)?, None))
}

/// Build a device instance from a compiled IR module, overriding the model's first parameter
/// with the device's scalar value (the SPICE convention: an `R`/`C` line's value sets the
/// model's primary parameter). Each of `module`'s port nodes is assigned the netlist terminal
/// it connects to; any other node (e.g. an internal node a flattened submodule instance
/// introduced, § module instantiation) claims a fresh global unknown from `next_unknown`.
fn build_from_model(
    module: &Module,
    value: Option<f64>,
    terminals: &[usize],
    next_unknown: &mut usize,
) -> Result<Box<dyn ModelInstance>> {
    let mut m = module.clone();
    if let (Some(v), Some(param)) = (value, m.params.first_mut()) {
        param.default = v;
    }

    let port_nodes: Vec<NodeId> = m.ports.iter().flatten().copied().collect();
    if port_nodes.len() != terminals.len() {
        bail!(
            "model `{}` declares {} port node(s), device connects {}",
            m.name,
            port_nodes.len(),
            terminals.len()
        );
    }
    let mut assigned: Vec<Option<usize>> = vec![None; m.nodes.len()];
    for (nid, &g) in port_nodes.iter().zip(terminals) {
        assigned[nid.0 as usize] = Some(g);
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

    va_codegen::build_instance(&m, &full, next_unknown)
        .with_context(|| format!("generating instance for model `{}`", module.name))
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

/// Print the DC operating point: node voltages, then source branch currents.
fn report(net: &Netlist, x: &[f64]) {
    println!("DC operating point:");
    for (i, name) in net.node_order.iter().enumerate() {
        println!("  V({name}) = {:.6} V", x[i]);
    }
    let mut branch = net.node_order.len();
    for dev in &net.devices {
        if dev.model == "vsource" {
            println!("  I({}) = {:.6e} A", dev.name, x[branch]);
            branch += 1;
        }
    }
}

/// Print a `.dc` sweep: one line per swept value, every node's voltage and source current —
/// the same per-point content [`report`] prints for a single operating point, repeated.
fn report_sweep(
    net: &Netlist,
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
        for (i, name) in net.node_order.iter().enumerate() {
            print!(" V({name})={:.6}V", op.x[i]);
        }
        let mut branch = net.node_order.len();
        for dev in &net.devices {
            if dev.model == "vsource" {
                print!(" I({})={:.6e}A", dev.name, op.x[branch]);
                branch += 1;
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
fn report_ac(net: &Netlist, currents: &[(String, usize)], response: &va_acnoise::ac::AcResponse) {
    use va_acnoise::ac::{magnitude, phase};
    println!(
        "AC analysis ({} point(s), f={:e} to {:e} Hz):",
        response.f.len(),
        response.f.first().copied().unwrap_or(0.0),
        response.f.last().copied().unwrap_or(0.0)
    );
    let polar = |z| format!("{:.6e}∠{:.2}°", magnitude(z), phase(z).to_degrees());
    for (f, x) in response.f.iter().zip(&response.x) {
        let mut cols: Vec<String> = net
            .node_order
            .iter()
            .enumerate()
            .map(|(i, name)| format!("V({name})={}", polar(x[i])))
            .collect();
        cols.extend(
            currents
                .iter()
                .map(|(name, idx)| format!("I({name})={}", polar(x[*idx]))),
        );
        println!("  f={f:.6e}Hz  {}", cols.join("  "));
    }
}

/// Print the noise spectrum: one line per frequency, the output PSD in V²/Hz alongside the more
/// commonly-read amplitude density V/√Hz (just its square root — printed because datasheets and
/// noise plots are conventionally in nV/√Hz, not V²/Hz), then the band-integrated RMS total.
fn report_noise(net: &Netlist, spectrum: &va_acnoise::noise::NoiseSpectrum) {
    let card = net.noise.as_ref();
    let output = card.map(|c| c.output.as_str()).unwrap_or("?");
    let source = card.map(|c| c.source.as_str()).unwrap_or("?");
    println!(
        "Noise analysis at V({output}) ({} point(s), f={:e} to {:e} Hz):",
        spectrum.f.len(),
        spectrum.f.first().copied().unwrap_or(0.0),
        spectrum.f.last().copied().unwrap_or(0.0)
    );
    for (i, (f, psd)) in spectrum.f.iter().zip(&spectrum.psd).enumerate() {
        // The input-referred column exists only when the card named a resolvable source.
        let referred = match spectrum.input_psd.get(i) {
            Some(inp) => format!("  Sin={inp:.6e} V^2/Hz"),
            None => String::new(),
        };
        println!(
            "  f={f:.6e}Hz  S={psd:.6e} V^2/Hz  ({:.6e} V/sqrt(Hz)){referred}",
            psd.sqrt()
        );
    }
    println!(
        "  total integrated output noise = {:.6e} V rms",
        spectrum.total
    );
    if !spectrum.input_psd.is_empty() {
        println!(
            "  total integrated input-referred noise (at {source}) = {:.6e} V rms",
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
            println!("    {name:<8} {:.6e} V rms  ({pct:5.1}%)", power.sqrt());
        }
    }
}

/// Print the transient waveform: one line per accepted timepoint, every node's voltage.
fn report_transient(net: &Netlist, wf: &Waveform) {
    println!(
        "Transient analysis ({} points, t=0 to t={:e}s):",
        wf.t.len(),
        wf.t.last().copied().unwrap_or(0.0)
    );
    for (t, x) in wf.t.iter().zip(&wf.x) {
        let cols: Vec<String> = net
            .node_order
            .iter()
            .enumerate()
            .map(|(i, name)| format!("V({name})={:.6}", x[i]))
            .collect();
        println!("  t={t:.6e}s  {}", cols.join("  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use va_abi::reference::GROUND;

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
        let design = va_frontend::compile_with_includes(src, &[]).expect("compiles");
        let mut next = 2usize;
        let msg = match va_codegen::build_instance(&design.modules[0], &[0, GROUND], &mut next) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a loop body must still be refused"),
        };
        assert!(msg.contains("constant for the whole run"), "got: {msg}");
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
    #[test]
    fn ohmmeter_probe_compiles_through_codegen() {
        let src = include_str!("../../../external/verilogaLib-master/ohmmeter.va");
        let include_dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../external"
        ))];
        let design =
            va_frontend::compile_with_includes(src, &include_dirs).expect("compile ohmmeter.va");
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
}
