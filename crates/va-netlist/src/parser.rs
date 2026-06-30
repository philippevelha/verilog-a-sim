//! Line-oriented netlist parser.
//!
//! Reads a SPICE-flavored deck: one element or dot-card per line, whitespace-separated
//! tokens, `*` full-line comments. The supported element letters are `R` (resistor), `C`
//! (capacitor), `D` (a model-referencing device, e.g. a diode), and `V` (voltage source).
//! Net `0`/`gnd` is the reference node; every other net gets a dense unknown index in
//! first-seen order.
//!
//! # Limitations
//!
//! - Inductors, controlled sources, subcircuits (`X`), and `.model` cards are not parsed.
//! - A `V` source accepts `DC <value>` or `SIN(off amp freq …)`; for the latter the DC
//!   offset becomes the source's DC value (what a DC operating point needs). Other transient
//!   waveforms are not parsed.

use crate::{AnalysisCard, Device, Netlist, NetlistError};
use va_abi::reference::GROUND;

/// Parse a netlist deck into a [`Netlist`].
///
/// # Errors
///
/// Returns [`NetlistError::Parse`] on a malformed line (unknown element letter, too few
/// tokens, or an unparseable value).
pub fn parse(deck: &str) -> Result<Netlist, NetlistError> {
    let mut net = Netlist::default();

    for (idx, raw) in deck.lines().enumerate() {
        let line = raw.trim();
        // Skip blank lines and `*` comments.
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        // Strip a trailing inline comment (`;` …).
        let line = line.split(';').next().unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }

        let line_no = idx + 1;
        if let Some(stripped) = line.strip_prefix('.') {
            parse_card(&mut net, stripped);
            continue;
        }
        let device = parse_device(&mut net, line, line_no)?;
        net.devices.push(device);
    }

    Ok(net)
}

/// Parse a dot-card, recording the analysis it requests. Unrecognized cards are ignored.
fn parse_card(net: &mut Netlist, body: &str) {
    let name = body.split_whitespace().next().unwrap_or("");
    let card = match name.to_ascii_lowercase().as_str() {
        "op" => AnalysisCard::Op,
        "dc" => AnalysisCard::Dc,
        "tran" => AnalysisCard::Tran,
        "ac" => AnalysisCard::Ac,
        _ => return, // `.end`, `.model`, etc. — ignored in v0.
    };
    // The first analysis card wins.
    if net.analysis == AnalysisCard::Unspecified {
        net.analysis = card;
    }
}

/// Parse one element line into a [`Device`], interning its terminal nets.
fn parse_device(net: &mut Netlist, line: &str, line_no: usize) -> Result<Device, NetlistError> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let name = toks[0].to_string();
    let kind = name.chars().next().unwrap_or(' ').to_ascii_uppercase();

    let err = |message: String| NetlistError::Parse {
        line: line_no,
        message,
    };

    // Every supported element has two terminals.
    let need = |n: usize| -> Result<(), NetlistError> {
        if toks.len() < n {
            Err(err(format!(
                "`{name}` needs at least {n} tokens, found {}",
                toks.len()
            )))
        } else {
            Ok(())
        }
    };

    match kind {
        'R' | 'C' => {
            need(4)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            let value =
                parse_value(toks[3]).ok_or_else(|| err(format!("bad value `{}`", toks[3])))?;
            let model = if kind == 'R' { "resistor" } else { "capacitor" };
            Ok(Device {
                name,
                model: model.to_string(),
                terminals: vec![p, n],
                value: Some(value),
            })
        }
        'D' => {
            need(4)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            // The fourth token names the model (e.g. `diode`).
            Ok(Device {
                name,
                model: toks[3].to_string(),
                terminals: vec![p, n],
                value: None,
            })
        }
        'V' => {
            need(3)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            let value = parse_source_value(&toks[3..]);
            Ok(Device {
                name,
                model: "vsource".to_string(),
                terminals: vec![p, n],
                value: Some(value),
            })
        }
        _ => Err(err(format!("unsupported element `{name}`"))),
    }
}

/// Intern a net name to an unknown index. `0`/`gnd` map to the reference sentinel.
fn intern(net: &mut Netlist, name: &str) -> usize {
    if name == "0" || name.eq_ignore_ascii_case("gnd") {
        return GROUND;
    }
    if let Some(&i) = net.nodes.get(name) {
        return i;
    }
    let i = net.node_order.len();
    net.nodes.insert(name.to_string(), i);
    net.node_order.push(name.to_string());
    i
}

/// Resolve a voltage source's DC value from its trailing tokens.
///
/// Accepts `DC <value>`, a bare `<value>`, or `SIN(off amp freq …)` (whose offset is the DC
/// value). Anything unrecognized defaults to `0.0`.
fn parse_source_value(rest: &[&str]) -> f64 {
    match rest.first().copied() {
        None => 0.0,
        Some(t) if t.eq_ignore_ascii_case("dc") => {
            rest.get(1).and_then(|v| parse_value(v)).unwrap_or(0.0)
        }
        Some(t) if t.to_ascii_uppercase().starts_with("SIN") => {
            // The offset is the first number inside the parentheses.
            let joined = rest.join(" ");
            let inner = joined
                .split(['(', ')'])
                .nth(1)
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("0");
            parse_value(inner).unwrap_or(0.0)
        }
        Some(t) => parse_value(t).unwrap_or(0.0),
    }
}

/// Parse a numeric literal with an optional SPICE engineering suffix.
///
/// Recognized suffixes (case-insensitive): `T G MEG K M U N P F A`. Note `MEG` is `1e6`
/// while a bare `M` is milli (`1e-3`), matching SPICE. A trailing unit string after the
/// suffix (e.g. `1kOhm`) is ignored.
fn parse_value(tok: &str) -> Option<f64> {
    let s = tok.trim();
    if s.is_empty() {
        return None;
    }
    // Split the leading numeric part (digits, sign, decimal, exponent) from the suffix.
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E')))
        // Guard against treating an exponent sign as the suffix boundary: only break at a
        // non-exponent character.
        .filter(|&i| {
            let bytes = s.as_bytes();
            !(matches!(bytes[i], b'+' | b'-') && i > 0 && matches!(bytes[i - 1], b'e' | b'E'))
        });

    let (num, suffix) = match split {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let value: f64 = num.parse().ok()?;

    let scale = match suffix.to_ascii_lowercase().as_str() {
        "" => 1.0,
        s if s.starts_with("meg") => 1e6,
        s if s.starts_with('t') => 1e12,
        s if s.starts_with('g') => 1e9,
        s if s.starts_with('k') => 1e3,
        s if s.starts_with('m') => 1e-3,
        s if s.starts_with('u') => 1e-6,
        s if s.starts_with('n') => 1e-9,
        s if s.starts_with('p') => 1e-12,
        s if s.starts_with('f') => 1e-15,
        s if s.starts_with('a') => 1e-18,
        _ => return None,
    };
    Some(value * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_suffixes() {
        assert_eq!(parse_value("1000"), Some(1000.0));
        assert_eq!(parse_value("1.0"), Some(1.0));
        assert_eq!(parse_value("2k"), Some(2000.0));
        assert_eq!(parse_value("1u"), Some(1e-6));
        assert_eq!(parse_value("1e-6"), Some(1e-6));
        assert_eq!(parse_value("5meg"), Some(5e6));
        assert_eq!(parse_value("3m"), Some(3e-3));
    }

    #[test]
    fn source_values() {
        assert_eq!(parse_source_value(&["DC", "1.0"]), 1.0);
        assert_eq!(parse_source_value(&["SIN(0", "5", "1k)"]), 0.0);
        assert_eq!(parse_source_value(&["2.5"]), 2.5);
    }
}
