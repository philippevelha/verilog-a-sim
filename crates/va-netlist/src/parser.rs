//! Line-oriented netlist parser.
//!
//! Reads a SPICE-flavored deck: one element or dot-card per line, whitespace-separated
//! tokens, `*` full-line comments. The supported element letters are `R` (resistor), `C`
//! (capacitor), `D` (a two-terminal model-referencing device, e.g. a diode), `M` (a
//! three-terminal model-referencing device, e.g. a MOSFET — `` M<name> d g s model ``, no
//! separate body/bulk terminal in v0, § ladder rung 5), `Q` (a three-terminal model-referencing
//! device, a BJT — `` Q<name> c b e model ``, SPICE's own collector/base/emitter order, no
//! substrate terminal in v0, § ladder rung 6), and `V` (voltage source). Net `0`/`gnd`
//! is the reference node; every other net gets a dense unknown index in first-seen order.
//!
//! # Limitations
//!
//! - Inductors, controlled sources, subcircuits (`X`), and `.model` cards are not parsed.
//! - A `V` source accepts `DC <value>` or `SIN(off amp freq …)`. The latter's offset becomes
//!   the DC value (what a DC operating point needs) *and* its full `(offset, amplitude, freq)`
//!   is retained as [`crate::Device::waveform`] for a transient run to reproduce the actual
//!   time dependence — `va_abi::ModelInstance::load` has no time parameter (Interface β has no
//!   room for one — see `docs/bridges/interface-beta-abi.md` §7), so a transient consumer
//!   reconstructs a fresh, differently-valued source each step instead
//!   (`va_transient::integrator::run_dynamic`). SPICE's optional trailing `SIN` parameters
//!   (delay, damping, phase) are not parsed.
//! - `.dc <source> <start> <stop> <step>` (§ ladder rung 2) sweeps one voltage source's DC
//!   value, solving a fresh operating point at each step ([`crate::DcSweep`]) — only a linear
//!   sweep of a single source, no nested/multi-source sweeps and no `.dc` with no arguments
//!   (a source-list sweep) as some SPICE dialects also accept.

use crate::{AnalysisCard, DcSweep, Device, Netlist, NetlistError, Waveform};
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
    let toks: Vec<&str> = body.split_whitespace().collect();
    let name = toks.first().copied().unwrap_or("");
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
    // `.tran <tstep> <tstop>` — the two SPICE-standard positional values transient needs.
    // Anything past them (a start time, `UIC`, …) isn't parsed in v0.
    if card == AnalysisCard::Tran {
        if let (Some(tstep), Some(tstop)) = (
            toks.get(1).and_then(|v| parse_value(v)),
            toks.get(2).and_then(|v| parse_value(v)),
        ) {
            net.tran = Some((tstep, tstop));
        }
    }
    // `.dc <source> <start> <stop> <step>` (§ ladder rung 2) — the SPICE-standard positional
    // sweep spec. `source` names a device (validated against `net.devices` by `va-cli`, not
    // here — this pass hasn't necessarily seen every device line yet, source order isn't
    // guaranteed).
    if card == AnalysisCard::Dc {
        if let (Some(source), Some(start), Some(stop), Some(step)) = (
            toks.get(1).map(|s| s.to_string()),
            toks.get(2).and_then(|v| parse_value(v)),
            toks.get(3).and_then(|v| parse_value(v)),
            toks.get(4).and_then(|v| parse_value(v)),
        ) {
            net.dc = Some(DcSweep {
                source,
                start,
                stop,
                step,
            });
        }
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

    // Minimum token count for the element kind about to be parsed (two terminals for most,
    // three for `M`).
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
                waveform: None,
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
                waveform: None,
            })
        }
        // `M<name> d g s model` — a three-terminal model-referencing device (e.g. a MOSFET, §
        // ladder rung 5). No body/bulk terminal in v0, unlike SPICE's usual four-terminal `M`
        // line — a stated simplification, not an oversight (mirrors `va-abi::reference::Bjt`'s
        // own no-body-effect scope for the analogous three-terminal BJT).
        'M' => {
            need(5)?;
            let d = intern(net, toks[1]);
            let g = intern(net, toks[2]);
            let s = intern(net, toks[3]);
            // The fifth token names the model (e.g. `mosfet`).
            Ok(Device {
                name,
                model: toks[4].to_string(),
                terminals: vec![d, g, s],
                value: None,
                waveform: None,
            })
        }
        'V' => {
            need(3)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            let value = parse_source_value(&toks[3..]);
            let waveform = parse_source_waveform(&toks[3..]);
            Ok(Device {
                name,
                model: "vsource".to_string(),
                terminals: vec![p, n],
                value: Some(value),
                waveform,
            })
        }
        // `Q<name> c b e model` — a three-terminal model-referencing device (a BJT, § ladder rung
        // 6), SPICE's own collector/base/emitter terminal order. No substrate terminal in v0,
        // unlike SPICE's optional four-terminal `Q` line — mirrors `M`'s own no-body-terminal
        // simplification for the analogous three-terminal MOSFET.
        'Q' => {
            need(5)?;
            let c = intern(net, toks[1]);
            let b = intern(net, toks[2]);
            let e = intern(net, toks[3]);
            // The fifth token names the model (e.g. `bjt`).
            Ok(Device {
                name,
                model: toks[4].to_string(),
                terminals: vec![c, b, e],
                value: None,
                waveform: None,
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

/// Parse a `SIN(offset amplitude freq …)` source's full waveform, or `None` for anything else
/// (`DC <value>`, a bare number, or a malformed `SIN(...)` missing one of the first three
/// values — the DC-only fallback in [`parse_source_value`] already covers that case).
fn parse_source_waveform(rest: &[&str]) -> Option<Waveform> {
    let first = rest.first()?;
    if !first.to_ascii_uppercase().starts_with("SIN") {
        return None;
    }
    let joined = rest.join(" ");
    let inner = joined.split(['(', ')']).nth(1)?;
    let nums: Vec<f64> = inner.split_whitespace().filter_map(parse_value).collect();
    match nums.as_slice() {
        [offset, amplitude, freq, ..] => Some(Waveform::Sin {
            offset: *offset,
            amplitude: *amplitude,
            freq: *freq,
        }),
        _ => None,
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
