//! T6 — the circuit-level netlist parser.
//!
//! Reads a SPICE-flavored `.net` deck and resolves it into a node map plus the list of
//! device instances to solve. It depends only on `va-abi` (the instance ABI); the actual
//! Verilog-A-backed instances are supplied by `va-codegen`, while built-in primitives can be
//! satisfied directly by `va-abi`'s reference models.

#![forbid(unsafe_code)]

pub mod parser;

use std::collections::HashMap;
use thiserror::Error;

/// Errors raised while parsing a netlist.
#[derive(Debug, Error)]
pub enum NetlistError {
    /// A line could not be parsed into a known device or directive.
    #[error("netlist parse error on line {line}: {message}")]
    Parse { line: usize, message: String },
    /// A device referenced a model name that was never declared.
    #[error("unknown model `{0}`")]
    UnknownModel(String),
}

/// The analysis a deck requests via its dot-card (`.op`, `.tran`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AnalysisCard {
    /// No analysis card was present.
    #[default]
    Unspecified,
    /// `.op` — DC operating point.
    Op,
    /// `.dc` — DC sweep (treated as an operating point in v0).
    Dc,
    /// `.tran` — transient.
    Tran,
    /// `.ac` — AC small-signal.
    Ac,
}

/// A parsed circuit: the node map, the device records to instantiate, and the analysis card.
///
/// Net `0` and `gnd` are the reference node; they are **not** assigned an unknown index and do
/// not appear in [`Self::nodes`]. Every other net is assigned a dense index `0..nodes.len()`
/// in first-seen order ([`Self::node_order`] preserves that order for reporting). A terminal
/// of [`va_abi::reference::GROUND`] denotes the reference node.
#[derive(Clone, Debug, Default)]
pub struct Netlist {
    /// Map from (non-ground) net name to global unknown index.
    pub nodes: HashMap<String, usize>,
    /// Non-ground net names in index order (`node_order[i]` is the net at index `i`).
    pub node_order: Vec<String>,
    /// Parsed device records (name, model, terminals, value), pre-instantiation.
    pub devices: Vec<Device>,
    /// The analysis requested by the deck's dot-card.
    pub analysis: AnalysisCard,
    /// `.tran <tstep> <tstop>` timing, if a transient card was present and both values parsed.
    /// `None` for a deck with no `.tran` card, or one whose timing tokens didn't parse — a
    /// transient run then has nothing to go on and `va-cli` reports that clearly rather than
    /// guessing a default.
    pub tran: Option<(f64, f64)>,
}

/// A single parsed device line, before it is turned into a [`va_abi::ModelInstance`].
#[derive(Clone, Debug)]
pub struct Device {
    /// Instance name (e.g. `R1`).
    pub name: String,
    /// Model / primitive kind: `resistor`, `capacitor`, `vsource`, or a named model
    /// (e.g. `diode`, or a Verilog-A model name) for device lines that reference one.
    pub model: String,
    /// Terminal node indices in port order ([`va_abi::reference::GROUND`] for the reference).
    pub terminals: Vec<usize>,
    /// Primary scalar value (resistance, capacitance, source DC value, …), if the line
    /// carries one.
    pub value: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::{parser::parse, AnalysisCard};
    use va_abi::reference::GROUND;

    #[test]
    fn parses_divider() {
        let deck = include_str!("../../../circuits/divider.net");
        let net = parse(deck).expect("divider.net should parse");

        // V1, R1, R2.
        assert_eq!(net.devices.len(), 3);
        // Two non-ground nets: in, mid.
        assert_eq!(net.nodes.len(), 2);
        assert_eq!(net.analysis, AnalysisCard::Op);

        // gnd resolves to the reference sentinel, not an unknown index.
        let r2 = net.devices.iter().find(|d| d.name == "R2").unwrap();
        assert_eq!(r2.model, "resistor");
        assert_eq!(r2.value, Some(1000.0));
        assert_eq!(r2.terminals[1], GROUND);

        let v1 = net.devices.iter().find(|d| d.name == "V1").unwrap();
        assert_eq!(v1.model, "vsource");
        assert_eq!(v1.value, Some(1.0));
    }

    #[test]
    fn parses_rectifier_devices_and_card() {
        let deck = include_str!("../../../circuits/rectifier.net");
        let net = parse(deck).expect("rectifier.net should parse");
        // V1, D1, R1, C1.
        assert_eq!(net.devices.len(), 4);
        assert_eq!(net.analysis, AnalysisCard::Tran);

        let d1 = net.devices.iter().find(|d| d.name == "D1").unwrap();
        assert_eq!(d1.model, "diode");
        assert_eq!(d1.value, None);

        let c1 = net.devices.iter().find(|d| d.name == "C1").unwrap();
        assert_eq!(c1.model, "capacitor");
        assert_eq!(c1.value, Some(1e-6));

        // `.tran 10u 5m`.
        let (tstep, tstop) = net.tran.expect("tran timing");
        assert!((tstep - 10e-6).abs() < 1e-15, "tstep = {tstep}");
        assert!((tstop - 5e-3).abs() < 1e-15, "tstop = {tstop}");
    }

    #[test]
    fn a_deck_with_no_tran_card_has_no_timing() {
        let net = parse(include_str!("../../../circuits/divider.net")).expect("parse divider");
        assert_eq!(net.tran, None);
    }

    #[test]
    fn si_suffixes_and_comments() {
        let deck = "* a comment\nR1 a 0 2k\nC1 a 0 1u\n.op\n.end\n";
        let net = parse(deck).expect("should parse");
        let r1 = net.devices.iter().find(|d| d.name == "R1").unwrap();
        assert_eq!(r1.value, Some(2000.0));
        let c1 = net.devices.iter().find(|d| d.name == "C1").unwrap();
        assert_eq!(c1.value, Some(1e-6));
    }
}
