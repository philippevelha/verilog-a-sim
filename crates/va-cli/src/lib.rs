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
use va_ir::Module;
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
            let module = va_frontend::compile(&src)
                .with_context(|| format!("compiling Verilog-A model {path}"))?;
            eprintln!(
                "[va-cli] compiled Verilog-A model `{}` from {path}",
                module.name
            );
            Some(module)
        }
        None => None,
    };

    let op = solve_dc(&net, compiled.as_ref())?;
    report(&net, &op.x);
    Ok(())
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

/// Build every device instance and solve the DC operating point.
fn solve_dc(net: &Netlist, compiled: Option<&Module>) -> Result<va_core::dc::OperatingPoint> {
    let n_nodes = net.node_order.len();
    let n_vsrc = net.devices.iter().filter(|d| d.model == "vsource").count();
    let dim = n_nodes + n_vsrc;

    // Voltage sources take branch-current unknowns after the node unknowns.
    let mut next_branch = n_nodes;
    let mut instances: Vec<Box<dyn ModelInstance>> = Vec::with_capacity(net.devices.len());
    for dev in &net.devices {
        let inst = build_instance(dev, compiled, &mut next_branch)?;
        instances.push(inst);
    }

    let refs: Vec<&dyn ModelInstance> = instances.iter().map(|b| b.as_ref()).collect();
    operating_point(&refs, dim, NewtonConfig::default()).context("DC operating-point solve failed")
}

/// Turn one parsed [`Device`] into a loadable instance, preferring a matching compiled
/// Verilog-A model and falling back to the reference primitives.
fn build_instance(
    dev: &Device,
    compiled: Option<&Module>,
    next_branch: &mut usize,
) -> Result<Box<dyn ModelInstance>> {
    let p = dev.terminals[0];
    let n = dev.terminals[1];

    if dev.model == "vsource" {
        let branch = *next_branch;
        *next_branch += 1;
        return Ok(Box::new(VSource::new(
            p,
            n,
            branch,
            dev.value.unwrap_or(0.0),
        )));
    }

    // Use the compiled Verilog-A model when its name matches the device's model.
    if let Some(module) = compiled {
        if module.name == dev.model {
            return build_from_model(module, dev.value, &dev.terminals);
        }
    }

    reference_instance(dev)
}

/// Build a device instance from a compiled IR module, overriding the model's first parameter
/// with the device's scalar value (the SPICE convention: an `R`/`C` line's value sets the
/// model's primary parameter).
fn build_from_model(
    module: &Module,
    value: Option<f64>,
    terminals: &[usize],
) -> Result<Box<dyn ModelInstance>> {
    let mut m = module.clone();
    if let (Some(v), Some(param)) = (value, m.params.first_mut()) {
        param.default = v;
    }
    va_codegen::build_instance(&m, terminals)
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
    fn solve_divider(compiled: Option<&Module>) -> va_core::dc::OperatingPoint {
        let deck = include_str!("../../../circuits/divider.net");
        let net = va_netlist::parser::parse(deck).expect("parse divider");
        solve_dc(&net, compiled).expect("solve divider")
    }

    #[test]
    fn divider_solves_with_reference_models() {
        let op = solve_divider(None);
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
        let module = va_frontend::compile(src).expect("compile resistor.va");
        assert_eq!(module.name, "resistor");
        let op = solve_divider(Some(&module));
        assert!((op.x[1] - 0.5).abs() < 1e-9, "V(mid) = {}", op.x[1]);
    }

    #[test]
    fn transient_deck_is_rejected() {
        let deck = include_str!("../../../circuits/rectifier.net");
        let net = va_netlist::parser::parse(deck).expect("parse rectifier");
        assert!(gate_analysis(&net, Analysis::Dc).is_err());
    }
}
