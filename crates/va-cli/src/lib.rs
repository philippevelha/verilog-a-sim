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
//! DC (`.op`) and transient (`.tran <tstep> <tstop>`) are implemented; AC decks are rejected
//! with a clear message. Transient always starts from the zero vector — v0 has no `.ic`/`UIC`
//! support. A `V` source with a bare `DC <value>` combined with that cold start *is* the step
//! response — the only shape a constant source could produce. A `V` source with a `SIN(...)`
//! waveform is genuinely time-varying: since `va_abi::ModelInstance::load` has no time
//! parameter (Interface β has no room for one — `docs/bridges/interface-beta-abi.md` §7), it
//! is reconstructed fresh every step with the value at that step's time baked in
//! (`va_transient::integrator::run_dynamic`), rather than the fixed-`VSource` path every other
//! device uses.

#![forbid(unsafe_code)]

mod plot;

use anyhow::{bail, Context, Result};
use std::f64::consts::PI;
use va_abi::reference::{diode::VT_NOMINAL, Capacitor, Diode, Resistor, VSource};
use va_abi::ModelInstance;
use va_core::dc::operating_point;
use va_core::newton::NewtonConfig;
use va_ir::{Module, NodeId};
use va_netlist::{AnalysisCard, Device, Netlist};
use va_transient::events::EventQueue;
use va_transient::integrator::{Method, TranConfig, Waveform};

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
) -> Result<()> {
    let (net, compiled) = load(netlist, model)?;

    gate_analysis(&net, analysis)?;
    if plot.is_some() && analysis != Analysis::Transient {
        bail!("--plot only supports transient analysis in v0 (pass --tran)");
    }

    if analysis == Analysis::Transient {
        let wf = solve_transient(&net, &compiled)?;
        report_transient(&net, &wf);
        if let Some(path) = plot {
            plot::plot_transient(path, &net, &wf).with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote transient plot to {path}");
        }
    } else if let Some(sweep) = &net.dc {
        let points = solve_dc_sweep(&net, &compiled, sweep)?;
        report_sweep(&net, sweep, &points);
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
/// # Errors
///
/// Only if a directory cannot be read.
pub fn check_models(paths: &[String]) -> Result<()> {
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

    let mut passed = 0usize;
    let mut total = 0usize;
    for group in groups.into_values() {
        total += group.len();
        passed += check_group(&group);
    }
    println!("\n{passed}/{total} files passed the frontend (lex → parse → elaborate)");
    Ok(())
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

/// Run one source file through preprocess → lex → parse, printing a tagged status line on
/// failure. Returns `None` (already reported) if any stage fails, `Some(asts)` (every module
/// the file's own text defines) otherwise. `scan_root` is the top-level directory the file was
/// discovered under (empty if the file was passed directly rather than found via directory
/// scan) — used only to widen `` `include `` resolution, unrelated to [`check_group`]'s
/// cross-file *instantiation* library.
fn parse_file(path: &str, scan_root: &std::path::Path) -> Option<Vec<va_frontend::ast::ModuleAst>> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("  [read ] {path}: {e}");
            return None;
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
    let src = match va_frontend::preprocess::preprocess(&src, &include_dirs) {
        Ok(s) => s,
        Err(e) => {
            println!("  [pp   ] {path}: {e}");
            return None;
        }
    };
    let tokens = match va_frontend::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            println!("  [lex  ] {path}: {e}");
            return None;
        }
    };
    match va_frontend::parser::parse(&tokens) {
        Ok(a) => Some(a),
        Err(e) => {
            println!("  [parse] {path}: {e}");
            None
        }
    }
}

/// Check every file in one directory-grouped library together (§ module instantiation across
/// files, [`check_models`]): parse each file individually — still reporting its own
/// read/preprocess/lex/parse failure on its own line — then elaborate every module from every
/// successfully-parsed file against the *combined* list of all their modules, so an
/// `Item::Instance` naming a module declared in a sibling file resolves
/// (`elaborate_with_library`'s `library` argument doesn't care which file an entry came from).
/// Returns how many files had every one of their own modules elaborate cleanly.
fn check_group(group: &[(String, std::path::PathBuf)]) -> usize {
    let mut library: Vec<va_frontend::ast::ModuleAst> = Vec::new();
    // Each successfully-parsed file's own modules, as a `library` index range — avoids cloning
    // every `ModuleAst` a second time just to report per-file status.
    let mut file_ranges: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    for (file, root) in group {
        if let Some(asts) = parse_file(file, root) {
            let start = library.len();
            library.extend(asts);
            file_ranges.push((file.as_str(), start..library.len()));
        }
    }

    let mut passed = 0usize;
    for (file, range) in file_ranges {
        let mut all_ok = true;
        for ast in &library[range] {
            match va_frontend::elaborate::elaborate_with_library(ast, &library) {
                Ok(m) => {
                    println!(
                        "  [ok   ] {file}: module `{}` ({} nodes, {} params, {} funcs)",
                        m.name,
                        m.nodes.len(),
                        m.params.len(),
                        m.functions.len()
                    );
                }
                Err(e) => {
                    println!("  [elab ] {file}: module `{}`: {e}", ast.name);
                    all_ok = false;
                }
            }
        }
        if all_ok {
            passed += 1;
        }
    }
    passed
}

/// Reject analyses v0 does not implement (AC), and mismatches between what the deck's own
/// dot-card requests and what the caller asked to run.
fn gate_analysis(net: &Netlist, analysis: Analysis) -> Result<()> {
    if net.analysis == AnalysisCard::Ac || analysis == Analysis::Ac {
        bail!("AC analysis is not implemented in v0; only DC and transient are supported");
    }
    if net.analysis == AnalysisCard::Tran && analysis != Analysis::Transient {
        bail!("deck requests transient analysis (`.tran`); pass `--tran` to run it");
    }
    if analysis == Analysis::Transient && net.tran.is_none() {
        bail!(
            "transient analysis requested but the deck has no parseable \
             `.tran <tstep> <tstop>` card"
        );
    }
    Ok(())
}

/// Build every device instance, returning them alongside the total unknown count (`dim`).
/// `compiled` is every module compiled from the `--model` file (possibly several, if it
/// defines a subcircuit alongside a top module — § module instantiation); a device is matched
/// against whichever one shares its model name. Shared by both DC and transient solving —
/// building the instance set doesn't depend on which analysis will run on it.
fn build_instances(
    net: &Netlist,
    compiled: &[Module],
) -> Result<(Vec<Box<dyn ModelInstance>>, usize)> {
    let n_nodes = net.node_order.len();

    // Voltage sources take branch-current unknowns after the node unknowns; a flattened
    // compiled module's internal (non-port) nodes need global unknowns too (§ module
    // instantiation — `va-codegen::build_instance` requires one global index per IR node, not
    // just per port). Both draw from this single shared counter, so `dim` is only known once
    // every instance has claimed what it needs.
    let mut next_unknown = n_nodes;
    let mut instances: Vec<Box<dyn ModelInstance>> = Vec::with_capacity(net.devices.len());
    for dev in &net.devices {
        let inst = build_instance(dev, compiled, &mut next_unknown)?;
        instances.push(inst);
    }
    Ok((instances, next_unknown))
}

/// Build every device instance and solve the DC operating point. `pub` so `va-harness` can get
/// the numeric [`va_core::dc::OperatingPoint`] back directly (§ golden comparison), rather than
/// parsing [`run_sim`]'s printed stdout.
pub fn solve_dc(net: &Netlist, compiled: &[Module]) -> Result<va_core::dc::OperatingPoint> {
    let (instances, dim) = build_instances(net, compiled)?;
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

/// One `vsource` device's `(p, n, branch)` global indices plus the waveform it should be
/// rebuilt from at each transient step (see [`build_instances_split`]).
type TimeVaryingSource = (usize, usize, usize, va_netlist::Waveform);

/// [`build_instances_split`]'s return: fixed device instances, time-varying source specs, and
/// the total unknown count.
type SplitInstances = (Vec<Box<dyn ModelInstance>>, Vec<TimeVaryingSource>, usize);

/// Like [`build_instances`], but splits out any `vsource` device with a `SIN` waveform into
/// its own list rather than baking a fixed DC value into it, since a transient run needs to
/// reconstruct it fresh each step (this module's doc comment). Returns `(fixed, time_varying,
/// dim)`: `time_varying` entries are `(p, n, branch, waveform)` — the same stable global
/// indices a plain DC-valued `VSource` would have claimed, just not yet turned into one. `dim`
/// and every other device's assigned indices are identical to what [`build_instances`] would
/// produce for the same netlist (each vsource device claims exactly one unknown either way).
fn build_instances_split(net: &Netlist, compiled: &[Module]) -> Result<SplitInstances> {
    let mut next_unknown = net.node_order.len();
    let mut fixed: Vec<Box<dyn ModelInstance>> = Vec::with_capacity(net.devices.len());
    let mut time_varying = Vec::new();

    for dev in &net.devices {
        if dev.model == "vsource" {
            if let Some(waveform) = dev.waveform {
                let branch = next_unknown;
                next_unknown += 1;
                time_varying.push((dev.terminals[0], dev.terminals[1], branch, waveform));
                continue;
            }
        }
        fixed.push(build_instance(dev, compiled, &mut next_unknown)?);
    }
    Ok((fixed, time_varying, next_unknown))
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
/// comment). For a deck with no time-varying source this is the ordinary fixed-instance path
/// ([`va_transient::integrator::run`]); a `SIN`-sourced deck instead rebuilds that source fresh
/// every step via [`va_transient::integrator::run_dynamic`]. `pub` so `va-harness` can get the
/// numeric [`Waveform`] back directly (§ golden comparison), the same reason `solve_dc`/
/// `solve_dc_sweep` are — rather than parsing [`run_sim`]'s printed stdout.
pub fn solve_transient(net: &Netlist, compiled: &[Module]) -> Result<Waveform> {
    let (tstep, tstop) = net
        .tran
        .context("transient analysis requires a `.tran <tstep> <tstop>` card")?;
    let cfg = TranConfig {
        tstart: 0.0,
        tstop,
        tstep,
        tstep_min: tstep * 1e-6,
        method: Method::Trapezoidal,
        lte_reltol: 1e-3,
        lte_abstol: 1e-6,
    };

    let (fixed, time_varying, dim) = build_instances_split(net, compiled)?;
    let x0 = vec![0.0; dim];
    let refs: Vec<&dyn ModelInstance> = fixed.iter().map(|b| b.as_ref()).collect();

    if time_varying.is_empty() {
        return va_transient::integrator::run(&refs, dim, x0, cfg)
            .context("transient integration failed");
    }

    va_transient::integrator::run_dynamic(dim, x0, cfg, &EventQueue::new(), &refs, |t| {
        time_varying
            .iter()
            .map(|&(p, n, branch, waveform)| {
                Box::new(VSource::new(p, n, branch, waveform_value(waveform, t)))
                    as Box<dyn ModelInstance>
            })
            .collect()
    })
    .context("transient integration failed")
}

/// Turn one parsed [`Device`] into a loadable instance, preferring a matching compiled
/// Verilog-A model and falling back to the reference primitives.
fn build_instance(
    dev: &Device,
    compiled: &[Module],
    next_unknown: &mut usize,
) -> Result<Box<dyn ModelInstance>> {
    let p = dev.terminals[0];
    let n = dev.terminals[1];

    if dev.model == "vsource" {
        let branch = *next_unknown;
        *next_unknown += 1;
        return Ok(Box::new(VSource::new(
            p,
            n,
            branch,
            dev.value.unwrap_or(0.0),
        )));
    }

    // Use the compiled Verilog-A model when its name matches the device's model.
    if let Some(module) = compiled.iter().find(|m| m.name == dev.model) {
        return build_from_model(module, dev.value, &dev.terminals, next_unknown);
    }

    reference_instance(dev)
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

    #[test]
    fn analysis_default_is_dc() {
        assert_eq!(Analysis::default(), Analysis::Dc);
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
        let passed = check_group(&group);
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
        let passed = check_group(&group);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            passed, 0,
            "top.va's `leg` instance must not resolve with no leg.va present"
        );
    }

    /// End-to-end DC: parse the divider deck, build reference instances, solve.
    /// V(in) = 1 V, V(mid) = Vin·R2/(R1+R2) = 0.5 V.
    fn solve_divider(compiled: &[Module]) -> va_core::dc::OperatingPoint {
        let deck = include_str!("../../../circuits/divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse divider");
        solve_dc(&net, compiled).expect("solve divider")
    }

    /// End-to-end DC sweep (ladder rung 2): compile `models/diode.va` and sweep
    /// `circuits/diode_iv.net`'s `V1` from 0 to 0.6 V, checking every point against the
    /// closed-form Shockley diode law the model itself implements — `Id(V) =
    /// Is*(exp(V/(N*vt))-1)` — not just against the tool's own output.
    #[test]
    fn diode_iv_sweep_solves_through_codegen_pipeline() {
        let src = include_str!("../../../models/diode.va");
        let design = va_frontend::compile(src).expect("compile diode.va");
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

    /// End-to-end DC (ladder rung 5): compile `models/mosfet.va` and solve `circuits/mos_dc.net`
    /// — an NMOS common-source bias point through the real frontend → codegen → core pipeline.
    #[test]
    fn mos_dc_solves_through_codegen_pipeline() {
        let src = include_str!("../../../models/mosfet.va");
        let design = va_frontend::compile(src).expect("compile mosfet.va");
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
        let design = va_frontend::compile(src).expect("compile resistor.va");
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

        let wf = solve_transient(&net, &[]).expect("integrates");
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

        let wf = solve_transient(&net, &[]).expect("integrates");
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
}
