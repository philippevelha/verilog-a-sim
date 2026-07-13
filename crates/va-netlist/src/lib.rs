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
    /// `.dc <source> <start> <stop> <step>` sweep spec, if a DC-sweep card was present and every
    /// value parsed (§ ladder rung 2). `None` for a deck with no `.dc` card, one whose tokens
    /// didn't parse, or a plain `.op` card — those solve a single operating point instead.
    pub dc: Option<DcSweep>,
}

/// A `.dc <source> <start> <stop> <step>` sweep spec (§ ladder rung 2): step `source`'s DC value
/// from `start` to `stop` in increments of `step`, solving a fresh operating point at each. Only
/// a voltage-source device may be named — matching the LRM/SPICE convention that a `.dc` sweep
/// steps a source, not an arbitrary device value; `va-cli` reports a clear error if `source`
/// doesn't resolve to one.
#[derive(Clone, Debug, PartialEq)]
pub struct DcSweep {
    /// The swept device's name (must be a `vsource`).
    pub source: String,
    /// First value.
    pub start: f64,
    /// Last value (inclusive, subject to rounding — see `va-cli`'s point generator).
    pub stop: f64,
    /// Increment between points.
    pub step: f64,
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
    /// carries one. For a source with a [`Self::waveform`], this is still that waveform's
    /// DC/offset value — what a DC operating-point solve needs regardless of the full
    /// time-domain shape.
    pub value: Option<f64>,
    /// The full time-domain waveform a `V` line specifies, if it's more than a bare DC value
    /// (e.g. `SIN(...)`). `None` for a plain `DC <value>`/bare-number source, or for any
    /// non-source device.
    pub waveform: Option<Waveform>,
}

/// A time-domain source waveform beyond a bare DC value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waveform {
    /// `SIN(offset amplitude freq)`: `v(t) = offset + amplitude·sin(2π·freq·t)`. Delay/damping/
    /// phase (SPICE's optional trailing `SIN` parameters) are not parsed in v0.
    Sin {
        /// DC offset (V).
        offset: f64,
        /// Peak amplitude above/below the offset (V).
        amplitude: f64,
        /// Frequency (Hz).
        freq: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::{parser::parse, AnalysisCard, DcSweep, Waveform};
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

        // `V1 in gnd SIN(0 5 1k)`.
        let v1 = net.devices.iter().find(|d| d.name == "V1").unwrap();
        assert_eq!(v1.value, Some(0.0));
        match v1.waveform {
            Some(Waveform::Sin {
                offset,
                amplitude,
                freq,
            }) => {
                assert!((offset - 0.0).abs() < 1e-12);
                assert!((amplitude - 5.0).abs() < 1e-12);
                assert!((freq - 1000.0).abs() < 1e-9);
            }
            other => panic!("expected a Sin waveform, got {other:?}"),
        }
    }

    #[test]
    fn a_deck_with_no_tran_card_has_no_timing() {
        let net = parse(include_str!("../../../circuits/divider.net")).expect("parse divider");
        assert_eq!(net.tran, None);
    }

    #[test]
    fn parses_mos_dc_devices() {
        let deck = include_str!("../../../circuits/mos_dc.net");
        let net = parse(deck).expect("mos_dc.net should parse");
        // VDD, VG, RD, M1.
        assert_eq!(net.devices.len(), 4);
        assert_eq!(net.analysis, AnalysisCard::Op);

        let m1 = net.devices.iter().find(|d| d.name == "M1").unwrap();
        assert_eq!(m1.model, "mosfet");
        assert_eq!(m1.value, None);
        assert_eq!(m1.terminals.len(), 3, "d, g, s — no body terminal in v0");
        assert_eq!(m1.terminals[2], GROUND, "M1's source is tied to gnd");
    }

    #[test]
    fn parses_diode_iv_sweep_card() {
        let deck = include_str!("../../../circuits/diode_iv.net");
        let net = parse(deck).expect("diode_iv.net should parse");
        assert_eq!(net.analysis, AnalysisCard::Dc);

        let sweep = net.dc.expect("`.dc` sweep card");
        assert_eq!(
            sweep,
            DcSweep {
                source: "V1".to_string(),
                start: 0.0,
                stop: 0.6,
                step: 0.1,
            }
        );

        let d1 = net.devices.iter().find(|d| d.name == "D1").unwrap();
        assert_eq!(d1.model, "diode");
    }

    #[test]
    fn a_deck_with_no_dc_card_has_no_sweep() {
        let net = parse(include_str!("../../../circuits/divider.net")).expect("parse divider");
        assert_eq!(net.dc, None);
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
