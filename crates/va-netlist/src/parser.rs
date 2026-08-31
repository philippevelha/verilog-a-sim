//! Line-oriented netlist parser.
//!
//! Reads a SPICE-flavored deck: one element or dot-card per line, whitespace-separated
//! tokens, `*` full-line comments. The supported element letters are `R` (resistor), `C`
//! (capacitor), `L` (inductor — which, like a voltage source, claims its own branch-current
//! unknown), `D` (a two-terminal model-referencing device, e.g. a diode), `M` (a
//! three-terminal model-referencing device, e.g. a MOSFET — `` M<name> d g s model ``, no
//! separate body/bulk terminal in v0, § ladder rung 5), `Q` (a three-terminal model-referencing
//! device, a BJT — `` Q<name> c b e model ``, SPICE's own collector/base/emitter order, no
//! substrate terminal in v0, § ladder rung 6), and `V` (voltage source). Net `0`/`gnd`
//! is the reference node; every other net gets a dense unknown index in first-seen order.
//!
//! A `C` or `L` line may carry SPICE's per-element initial condition, `` IC=<value> ``
//! ([`va_netlist::Device::ic`]): volts across a capacitor, amps through an inductor — the state
//! each element actually carries — at `tstart`, seeding a
//! **transient** run's initial solution and ignored by every other analysis — SPICE's own `UIC`
//! semantics, in which no DC operating point is solved first. A capacitor with no `IC=` starts
//! at 0 V, which is what this engine did unconditionally before the token was recognized.
//!
//! # Limitations
//!
//! - Controlled sources, subcircuits (`X`), mutual inductance (`K`), and `.model` cards are
//!   not parsed.
//! - A `V` source accepts `DC <value>` or `SIN(off amp freq …)`. The latter's offset becomes
//!   the DC value (what a DC operating point needs) *and* its full `(offset, amplitude, freq)`
//!   is retained as [`crate::Device::waveform`] for a transient run to reproduce the actual
//!   time dependence. A consumer turns that into an ordinary stateless `ModelInstance` reading
//!   the current time off `va_abi::ModelInstance::load`'s analysis context (§6 change,
//!   2026-08-06 — `va_cli::WaveformSource`); the two facts stay consistent because the offset
//!   *is* the waveform's value at `t = 0`, which is what a DC or AC solve reads. SPICE's
//!   optional trailing `SIN` parameters (delay, damping, phase) are not parsed.
//! - `.dc <source> <start> <stop> <step>` (§ ladder rung 2) sweeps one voltage source's DC
//!   value, solving a fresh operating point at each step ([`crate::DcSweep`]) — only a linear
//!   sweep of a single source, no nested/multi-source sweeps and no `.dc` with no arguments
//!   (a source-list sweep) as some SPICE dialects also accept.
//! - `.ac dec <points-per-decade> <fstart> <fstop>` (T5) requests a small-signal AC sweep
//!   ([`crate::AcSweepCard`]); a `V` line's own `AC <magnitude> [phase]` tokens supply the
//!   excitation ([`crate::AcSpec`]). Only the `dec` sweep type is parsed (see
//!   [`crate::AcSweepCard`]'s own limitations), and a source's AC phase defaults to 0°.

use crate::{
    AcSpec, AcSweepCard, AnalysisCard, DcSweep, Device, Netlist, NetlistError, NoiseCard, Waveform,
};
use va_abi::reference::GROUND;

/// Parse a netlist deck into a [`Netlist`].
///
/// # Errors
///
/// Returns [`NetlistError::Parse`] on a malformed line (unknown element letter, too few
/// tokens, or an unparseable value).
pub fn parse(deck: &str) -> Result<Netlist, NetlistError> {
    let mut net = Netlist::default();
    // `PULSE`'s optional trailing parameters default to values SPICE derives from the `.tran`
    // card (one timestep for an omitted rise/fall, the run length for an omitted width or
    // period), and that card may appear *after* the source line. So the raw numbers are held
    // here and resolved once the whole deck has been read, rather than defaulted against
    // timing that may not have been parsed yet.
    let mut pending_pulses: Vec<(usize, Vec<f64>)> = Vec::new();

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
        if let Some(nums) = pulse_numbers(line) {
            pending_pulses.push((net.devices.len(), nums));
        }
        net.devices.push(device);
    }

    resolve_pulse_defaults(&mut net, &pending_pulses);
    Ok(net)
}

/// The raw `PULSE(...)` numbers on a source line, or `None` for any other line.
fn pulse_numbers(line: &str) -> Option<Vec<f64>> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let first = toks.first()?;
    if !first.starts_with(['V', 'v']) {
        return None;
    }
    let upper = line.to_ascii_uppercase();
    let at = upper.find("PULSE")?;
    let inner = line[at..].split(['(', ')']).nth(1)?;
    let nums: Vec<f64> = inner.split_whitespace().filter_map(parse_value).collect();
    (nums.len() >= 2).then_some(nums)
}

/// Fill in each `PULSE`'s omitted trailing parameters, now that the deck's `.tran` timing (if
/// any) has been parsed. SPICE's defaults: `td = 0`, `tr = tf = <one timestep>`, and `pw` and
/// `per` running to the end of the analysis. With no `.tran` card at all there is no timing to
/// derive from, so an omitted rise/fall becomes an ideal (zero-time) edge and the pulse does
/// not repeat — a `PULSE` source in a `.op`/`.dc`/`.ac`-only deck contributes its `v1` value
/// regardless, so none of those defaults are observable there.
fn resolve_pulse_defaults(net: &mut Netlist, pending: &[(usize, Vec<f64>)]) {
    let (tstep, tstop) = net.tran.unwrap_or((0.0, 0.0));
    for (idx, nums) in pending {
        let Some(dev) = net.devices.get_mut(*idx) else {
            continue;
        };
        let get = |i: usize, default: f64| nums.get(i).copied().unwrap_or(default);
        dev.waveform = Some(Waveform::Pulse {
            v1: nums[0],
            v2: nums[1],
            td: get(2, 0.0),
            tr: get(3, tstep),
            tf: get(4, tstep),
            pw: get(5, tstop),
            per: get(6, tstop),
        });
    }
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
        "noise" => AnalysisCard::Noise,
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
    // `.ac dec <points-per-decade> <fstart> <fstop>` (T5) — SPICE's standard positional AC sweep
    // spec. Only `dec` is accepted (§ this module's own doc comment and `crate::AcSweepCard`'s):
    // an `oct`/`lin` card leaves `net.ac` as `None`, so `va-cli` reports "no parseable `.ac`
    // card" rather than silently solving a decade grid the deck never asked for.
    if card == AnalysisCard::Ac {
        let is_dec = toks.get(1).is_some_and(|t| t.eq_ignore_ascii_case("dec"));
        if let (true, Some(ppd), Some(fstart), Some(fstop)) = (
            is_dec,
            toks.get(2).and_then(|v| parse_value(v)),
            toks.get(3).and_then(|v| parse_value(v)),
            toks.get(4).and_then(|v| parse_value(v)),
        ) {
            if ppd >= 1.0 {
                net.ac = Some(AcSweepCard {
                    points_per_decade: ppd as usize,
                    fstart,
                    fstop,
                });
            }
        }
    }
    // `.noise V(<out>) <source> dec <points-per-decade> <fstart> <fstop>` (T5.2) — SPICE's
    // standard positional noise card: an output *probe*, an input source, then the same
    // frequency-grid spec `.ac` takes (and the same `dec`-only limitation).
    if card == AnalysisCard::Noise {
        let is_dec = toks.get(3).is_some_and(|t| t.eq_ignore_ascii_case("dec"));
        if let (Some(output), Some(source), true, Some(ppd), Some(fstart), Some(fstop)) = (
            toks.get(1).and_then(|t| parse_voltage_probe(t)),
            toks.get(2).map(|s| s.to_string()),
            is_dec,
            toks.get(4).and_then(|v| parse_value(v)),
            toks.get(5).and_then(|v| parse_value(v)),
            toks.get(6).and_then(|v| parse_value(v)),
        ) {
            if ppd >= 1.0 {
                net.noise = Some(NoiseCard {
                    output,
                    source,
                    points_per_decade: ppd as usize,
                    fstart,
                    fstop,
                });
            }
        }
    }
}

/// Unwrap a `V(<node>)` output probe into the bare node name, case-insensitively on the `V`.
///
/// `None` for anything not shaped that way — including SPICE's differential `V(a,b)` form, which
/// this project's noise analysis has no representation for (its output is a single unknown
/// index, not a difference of two).
fn parse_voltage_probe(tok: &str) -> Option<String> {
    let rest = tok
        .strip_prefix("V(")
        .or_else(|| tok.strip_prefix("v("))?
        .strip_suffix(')')?;
    if rest.is_empty() || rest.contains(',') {
        return None;
    }
    Some(rest.to_string())
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
        'R' | 'C' | 'L' => {
            need(4)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            let value =
                parse_value(toks[3]).ok_or_else(|| err(format!("bad value `{}`", toks[3])))?;
            let model = match kind {
                'R' => "resistor",
                'C' => "capacitor",
                _ => "inductor",
            };
            // `IC=<value>`, SPICE's per-element initial condition, in the units of the state
            // the element carries: volts across a capacitor, amps through an inductor. A
            // resistor has no state to initialize, so `IC=` there is an error. Spelled as one token (`IC=5`), the form QSPICE and every
            // SPICE dialect write, and the form `xtask`'s golden-deck translator already
            // leaves untouched when it injects `IC=0` into the decks that lack one.
            let mut ic = None;
            for tok in &toks[4..] {
                let Some(rest) = tok.strip_prefix("IC=").or_else(|| tok.strip_prefix("ic=")) else {
                    return Err(err(format!("unexpected token `{tok}` after the value")));
                };
                if kind == 'R' {
                    return Err(err(format!(
                        "`IC=` is only meaningful on a reactive element, not on `{name}`"
                    )));
                }
                ic = Some(
                    parse_value(rest).ok_or_else(|| err(format!("bad `IC=` value `{rest}`")))?,
                );
            }
            Ok(Device {
                name,
                model: model.to_string(),
                terminals: vec![p, n],
                value: Some(value),
                waveform: None,
                ac: None,
                ic,
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
                ac: None,
                ic: None,
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
                ac: None,
                ic: None,
            })
        }
        'V' => {
            need(3)?;
            let p = intern(net, toks[1]);
            let n = intern(net, toks[2]);
            let value = parse_source_value(&toks[3..]);
            let waveform = parse_source_waveform(&toks[3..]);
            let ac = parse_source_ac(&toks[3..]);
            Ok(Device {
                name,
                model: "vsource".to_string(),
                terminals: vec![p, n],
                value: Some(value),
                waveform,
                ac,
                ic: None,
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
                ac: None,
                ic: None,
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
        Some(t)
            if t.to_ascii_uppercase().starts_with("SIN")
                || t.to_ascii_uppercase().starts_with("PULSE") =>
        {
            // The first number inside the parentheses: a `SIN`'s offset, a `PULSE`'s `v1`.
            // Both are the waveform's value at `t = 0`, which is what a DC operating point and
            // an AC linearization see (§ `Waveform`).
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
    let first = rest.first()?.to_ascii_uppercase();
    let joined = rest.join(" ");
    let inner = joined.split(['(', ')']).nth(1)?;
    let nums: Vec<f64> = inner.split_whitespace().filter_map(parse_value).collect();

    if first.starts_with("SIN") {
        return match nums.as_slice() {
            [offset, amplitude, freq, ..] => Some(Waveform::Sin {
                offset: *offset,
                amplitude: *amplitude,
                freq: *freq,
            }),
            _ => None,
        };
    }

    if first.starts_with("PULSE") {
        // A placeholder: `resolve_pulse_defaults` overwrites this once the deck's `.tran`
        // timing is known. Recorded here anyway so the device is marked time-varying from the
        // start, and so a `PULSE` in a deck with no `.tran` card still parses.
        let [v1, v2, ..] = nums.as_slice() else {
            return None;
        };
        return Some(Waveform::Pulse {
            v1: *v1,
            v2: *v2,
            td: 0.0,
            tr: 0.0,
            tf: 0.0,
            pw: 0.0,
            per: 0.0,
        });
    }

    None
}

/// Parse a `V` line's `AC <magnitude> [phase]` tokens, or `None` for a source with no `AC` token.
///
/// Position-independent within the trailing tokens, matching SPICE: `V1 in 0 DC 0.7 AC 1` and
/// `V1 in 0 AC 1 DC 0.7` mean the same thing, and the `AC` token can equally follow a
/// `SIN(...)` waveform. The magnitude must parse; a missing/unparseable one yields `None` (a
/// bare `AC` with no value is not a 1 V default here — SPICE dialects disagree on that, and a
/// silent implicit magnitude would be a surprising way to excite a circuit). The phase is
/// optional and defaults to 0°; a non-numeric token in that position (e.g. the `DC` of a
/// trailing `AC 1 DC 0.7`) is simply not a phase.
fn parse_source_ac(rest: &[&str]) -> Option<AcSpec> {
    let i = rest.iter().position(|t| t.eq_ignore_ascii_case("ac"))?;
    let magnitude = rest.get(i + 1).and_then(|v| parse_value(v))?;
    let phase_deg = rest.get(i + 2).and_then(|v| parse_value(v)).unwrap_or(0.0);
    Some(AcSpec {
        magnitude,
        phase_deg,
    })
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

    #[test]
    fn source_ac_specs() {
        assert_eq!(
            parse_source_ac(&["DC", "0.7", "AC", "1"]),
            Some(AcSpec {
                magnitude: 1.0,
                phase_deg: 0.0
            })
        );
        // Order-independent, and an explicit phase is picked up.
        assert_eq!(
            parse_source_ac(&["AC", "2", "90", "DC", "0.7"]),
            Some(AcSpec {
                magnitude: 2.0,
                phase_deg: 90.0
            })
        );
        // A trailing non-numeric token is not a phase — `DC` here must not become `0.7`'s phase
        // slot or, worse, parse as some number.
        assert_eq!(
            parse_source_ac(&["AC", "1", "DC", "0.7"]),
            Some(AcSpec {
                magnitude: 1.0,
                phase_deg: 0.0
            })
        );
        // No AC token at all, and a bare `AC` with no magnitude, are both "no excitation".
        assert_eq!(parse_source_ac(&["DC", "5"]), None);
        assert_eq!(parse_source_ac(&["AC"]), None);
    }

    #[test]
    fn ac_card_parses_only_a_dec_sweep() {
        let net = parse("* t\nV1 in 0 AC 1\n.ac dec 10 1 1meg\n.end\n").expect("parse");
        assert_eq!(net.analysis, AnalysisCard::Ac);
        assert_eq!(
            net.ac,
            Some(AcSweepCard {
                points_per_decade: 10,
                fstart: 1.0,
                fstop: 1e6,
            })
        );

        // `lin`/`oct` are recognized as *AC analysis* but leave no parseable sweep — the
        // frequency grid `va_acnoise::ac::AcSweep` produces is per-decade only, so promising a
        // linear grid here would be a lie (§ `crate::AcSweepCard`'s own limitations).
        let net = parse("* t\nV1 in 0 AC 1\n.ac lin 100 1 1meg\n.end\n").expect("parse");
        assert_eq!(net.analysis, AnalysisCard::Ac);
        assert_eq!(net.ac, None);
    }

    #[test]
    fn noise_card_parses_probe_source_and_grid() {
        let net = parse("* t\nV1 in 0 DC 0.7\nR1 in a 1k\n.noise V(a) V1 dec 10 10 10meg\n.end\n")
            .expect("parse");
        assert_eq!(net.analysis, AnalysisCard::Noise);
        assert_eq!(
            net.noise,
            Some(NoiseCard {
                output: "a".to_string(),
                source: "V1".to_string(),
                points_per_decade: 10,
                fstart: 10.0,
                fstop: 1e7,
            })
        );
    }

    #[test]
    fn voltage_probes_that_this_analysis_cannot_represent_are_rejected() {
        assert_eq!(parse_voltage_probe("V(out)"), Some("out".to_string()));
        assert_eq!(parse_voltage_probe("v(out)"), Some("out".to_string()));
        // A differential probe has no single output unknown to take — must not silently become
        // `V(a)` or a node literally named "a,b".
        assert_eq!(parse_voltage_probe("V(a,b)"), None);
        assert_eq!(parse_voltage_probe("V()"), None);
        assert_eq!(parse_voltage_probe("out"), None);
        assert_eq!(parse_voltage_probe("I(V1)"), None);
    }

    #[test]
    fn a_deck_with_no_noise_card_has_no_noise_spec() {
        let net = parse("* t\nR1 a 0 1k\n.op\n.end\n").expect("parse");
        assert_eq!(net.noise, None);
        // A `lin` grid is recognized as noise analysis but leaves nothing parseable, exactly as
        // for `.ac`.
        let lin = parse("* t\nR1 a 0 1k\n.noise V(a) V1 lin 100 10 1meg\n.end\n").expect("parse");
        assert_eq!(lin.analysis, AnalysisCard::Noise);
        assert_eq!(lin.noise, None);
    }

    #[test]
    fn a_deck_with_no_ac_card_has_no_ac_sweep() {
        let net = parse("* t\nR1 a 0 1k\n.op\n.end\n").expect("parse");
        assert_eq!(net.ac, None);
        let v = parse("* t\nV1 a 0 DC 5\n.op\n.end\n").expect("parse");
        assert_eq!(v.devices[0].ac, None);
    }
    /// `IC=` is parsed off a capacitor line, and only off a capacitor: a resistor has no state
    /// to initialize, so `IC=` there is a mistake worth naming rather than ignoring.
    #[test]
    fn a_capacitor_carries_an_initial_condition() {
        let net = parse(
            "R1 out gnd 1000
C1 out gnd 1e-6 IC=5
.tran 1e-6 1e-3
.end
",
        )
        .expect("parses");
        let cap = net
            .devices
            .iter()
            .find(|d| d.name == "C1")
            .expect("C1 parsed");
        assert_eq!(cap.ic, Some(5.0));
        let res = net
            .devices
            .iter()
            .find(|d| d.name == "R1")
            .expect("R1 parsed");
        assert_eq!(res.ic, None, "a resistor has no initial condition");

        // Lower-case spelling, and a suffixed value, both of which real decks use.
        let net = parse(
            "C1 a gnd 1e-6 ic=2.5m
.tran 1e-6 1e-3
.end
",
        )
        .expect("parses");
        assert_eq!(net.devices[0].ic, Some(2.5e-3));
    }

    /// A capacitor with no `IC=` keeps `None` rather than `Some(0.0)`: "not specified" and
    /// "specified as zero" are the same *answer* here but not the same *statement*, and the
    /// golden-deck translator keys off which one a line makes.
    #[test]
    fn a_capacitor_without_ic_has_none() {
        let net = parse(
            "C1 a gnd 1e-6
.tran 1e-6 1e-3
.end
",
        )
        .expect("parses");
        assert_eq!(net.devices[0].ic, None);
    }

    /// `IC=` on a resistor, and a malformed value, are refused with a message — not silently
    /// dropped, which would leave a deck's author believing an initial condition was applied.
    #[test]
    fn a_bad_initial_condition_is_rejected() {
        assert!(
            parse(
                "R1 a gnd 1000 IC=5
.end
"
            )
            .is_err(),
            "IC= on a resistor"
        );
        assert!(
            parse(
                "C1 a gnd 1e-6 IC=banana
.end
"
            )
            .is_err(),
            "unparseable IC="
        );
        assert!(
            parse(
                "C1 a gnd 1e-6 5
.end
"
            )
            .is_err(),
            "stray trailing token"
        );

        // An inductor does carry state, in amps, so `IC=` is accepted there too.
        let net = parse("L1 a gnd 1e-3 IC=2e-3\n.tran 1e-6 1e-3\n.end\n")
            .expect("IC= on an inductor parses");
        assert_eq!(net.devices[0].ic, Some(2e-3));
        assert_eq!(net.devices[0].model, "inductor");
    }
}
