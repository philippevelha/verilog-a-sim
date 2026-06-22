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

/// A parsed circuit: a node-name→index map and the device records to instantiate.
#[derive(Clone, Debug, Default)]
pub struct Netlist {
    /// Map from net name to global unknown index. `0` (`gnd`) is the reference node.
    pub nodes: HashMap<String, usize>,
    /// Parsed device records (name, model, terminals, value), pre-instantiation.
    pub devices: Vec<Device>,
}

/// A single parsed device line, before it is turned into a [`va_abi::ModelInstance`].
#[derive(Clone, Debug)]
pub struct Device {
    /// Instance name (e.g. `R1`).
    pub name: String,
    /// Model / primitive kind (e.g. `resistor`, `diode`, or a Verilog-A model name).
    pub model: String,
    /// Terminal node indices in port order.
    pub terminals: Vec<usize>,
    /// Primary scalar value (resistance, capacitance, …), if the line carries one.
    pub value: Option<f64>,
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T6: parse circuits/divider.net into two resistor devices and three nodes"]
    fn parses_divider() {
        let deck = include_str!("../../../circuits/divider.net");
        let net = super::parser::parse(deck).expect("divider.net should parse");
        assert_eq!(net.devices.len(), 2);
    }
}
