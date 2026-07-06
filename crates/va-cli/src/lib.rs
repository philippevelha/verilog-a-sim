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
use va_abi::reference::{diode::VT_300K, Capacitor, Diode, Resistor, VSource};
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
    let deck =
        std::fs::read_to_string(netlist).with_context(|| format!("reading netlist {netlist}"))?;
    let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {netlist}"))?;

    gate_analysis(&net, analysis)?;
    if plot.is_some() && analysis != Analysis::Transient {
        bail!("--plot only supports transient analysis in v0 (pass --tran)");
    }

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

    if analysis == Analysis::Transient {
        let wf = solve_transient(&net, &compiled)?;
        report_transient(&net, &wf);
        if let Some(path) = plot {
            plot::plot_transient(path, &net, &wf).with_context(|| format!("plotting to {path}"))?;
            eprintln!("[va-cli] wrote transient plot to {path}");
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

    let mut passed = 0usize;
    for (file, root) in &files {
        if check_one(file, root) {
            passed += 1;
        }
    }
    println!(
        "\n{passed}/{} files passed the frontend (lex → parse → elaborate)",
        files.len()
    );
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

/// Check a single source file through the frontend stages, printing a tagged status line.
/// `scan_root` is the top-level directory the file was discovered under (empty if the file
/// was passed directly rather than found via directory scan). Returns whether it elaborated
/// cleanly.
fn check_one(path: &str, scan_root: &std::path::Path) -> bool {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("  [read ] {path}: {e}");
            return false;
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
            return false;
        }
    };
    let tokens = match va_frontend::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            println!("  [lex  ] {path}: {e}");
            return false;
        }
    };
    let asts = match va_frontend::parser::parse(&tokens) {
        Ok(a) => a,
        Err(e) => {
            println!("  [parse] {path}: {e}");
            return false;
        }
    };
    // Elaborate every module the file defines (each against the full sibling list, so an
    // `Item::Instance` anywhere in the file resolves), since a file may define a subcircuit
    // plus a top module rather than exactly one (§ module instantiation).
    let mut all_ok = true;
    for ast in &asts {
        match va_frontend::elaborate::elaborate_with_library(ast, &asts) {
            Ok(m) => {
                println!(
                    "  [ok   ] {path}: module `{}` ({} nodes, {} params, {} funcs)",
                    m.name,
                    m.nodes.len(),
                    m.params.len(),
                    m.functions.len()
                );
            }
            Err(e) => {
                println!("  [elab ] {path}: module `{}`: {e}", ast.name);
                all_ok = false;
            }
        }
    }
    all_ok
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

/// Build every device instance and solve the DC operating point.
fn solve_dc(net: &Netlist, compiled: &[Module]) -> Result<va_core::dc::OperatingPoint> {
    let (instances, dim) = build_instances(net, compiled)?;
    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    operating_point(&refs, dim, NewtonConfig::default()).context("DC operating-point solve failed")
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
/// every step via [`va_transient::integrator::run_dynamic`].
fn solve_transient(net: &Netlist, compiled: &[Module]) -> Result<Waveform> {
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

    va_codegen::build_instance(&m, &full)
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
        "diode" => Box::new(Diode::new(p, n, 1e-14, 1.0, VT_300K)),
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

    /// End-to-end DC: parse the divider deck, build reference instances, solve.
    /// V(in) = 1 V, V(mid) = Vin·R2/(R1+R2) = 0.5 V.
    fn solve_divider(compiled: &[Module]) -> va_core::dc::OperatingPoint {
        let deck = include_str!("../../../circuits/divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse divider");
        solve_dc(&net, compiled).expect("solve divider")
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
