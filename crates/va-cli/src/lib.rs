//! T6 — `va-cli` library: the pipeline wiring, exposed so `va-harness` can drive it.
//!
//! The binary (`main.rs`) is a thin argument-parsing wrapper over [`run_sim`]. Keeping the
//! pipeline in a library lets the validation harness call it directly instead of shelling
//! out to the executable.
//!
//! # What v0 wires
//!
//! `va-netlist` parses the deck; each device becomes a [`va_abi::ModelInstance`]; `va-core`
//! solves the DC operating point. A `--model <m.va>` is compiled through the real
//! `va-frontend` → `va-codegen` pipeline and used for every device whose model name matches
//! the compiled module (e.g. `resistor` devices against `resistor.va`), with the device's
//! scalar value overriding the model's first parameter. Devices with no matching compiled
//! model fall back to the hand-written reference primitives in `va-abi`.
//!
//! Only DC (`.op`) is implemented; transient/AC decks are rejected with a clear message.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use va_abi::reference::{diode::VT_300K, Capacitor, Diode, Resistor, VSource};
use va_abi::ModelInstance;
use va_core::dc::operating_point;
use va_core::newton::NewtonConfig;
use va_ir::{Module, NodeId};
use va_netlist::{AnalysisCard, Device, Netlist};

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
/// Wires `va-frontend` → `va-codegen` → `va-netlist` → `va-core`. Prints the DC operating
/// point (node voltages and source currents) to stdout.
///
/// # Errors
///
/// Returns an error if a file cannot be read, the deck or model cannot be parsed, an
/// unsupported analysis is requested, a device names an unknown model, or the solve diverges.
pub fn run_sim(netlist: &str, model: Option<&str>, analysis: Analysis) -> Result<()> {
    let deck =
        std::fs::read_to_string(netlist).with_context(|| format!("reading netlist {netlist}"))?;
    let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {netlist}"))?;

    gate_analysis(&net, analysis)?;

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

    let op = solve_dc(&net, &compiled)?;
    report(&net, &op.x);
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

/// Reject analyses v0 does not implement (anything but DC).
fn gate_analysis(net: &Netlist, analysis: Analysis) -> Result<()> {
    if matches!(net.analysis, AnalysisCard::Tran | AnalysisCard::Ac) {
        bail!(
            "deck requests {:?} analysis; only DC (`.op`) is implemented in v0",
            net.analysis
        );
    }
    if analysis != Analysis::Dc {
        bail!("{analysis:?} analysis is not implemented in v0; only DC is supported");
    }
    Ok(())
}

/// Build every device instance and solve the DC operating point. `compiled` is every module
/// compiled from the `--model` file (possibly several, if it defines a subcircuit alongside a
/// top module — § module instantiation); a device is matched against whichever one shares its
/// model name.
fn solve_dc(net: &Netlist, compiled: &[Module]) -> Result<va_core::dc::OperatingPoint> {
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
    let dim = next_unknown;

    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    operating_point(&refs, dim, NewtonConfig::default()).context("DC operating-point solve failed")
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
}
