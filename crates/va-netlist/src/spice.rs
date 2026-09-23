//! SPICE deck structure, resolved before the line parser runs: `.include`, `+` continuation
//! lines, `.model` cards, `.subckt`/`.ends` definitions and their instances, `.global`, and the
//! ngspice constructs a benchmark deck carries that are not part of the circuit (`.control`
//! blocks, `.options`).
//!
//! The output is what [`crate::parser`] has always read — one element or dot-card per line —
//! plus the `.model` cards, which a device line resolves against, and the notes on what was
//! skipped. Doing it as a separate pass keeps the line parser's own rules, and its error
//! messages, exactly as they were for a deck that uses none of this.
//!
//! # Flattening
//!
//! An `X` line whose model names a `.subckt` is replaced by the subcircuit's body, with:
//!
//! - **ports** mapped to the nets the instance line connects;
//! - **internal nets** renamed `<instance path>.<net>` (ngspice's convention: net `sig3` inside
//!   instance `X5` is `X5.sig3`; nested, `X5.X2.sig3`), so two instances never share one;
//! - **element names** renamed `<letter>.<instance path>.<name>` (again ngspice's: `nm01` inside
//!   `X5` becomes `n.X5.nm01`), which keeps the leading letter that decides the element kind;
//! - `0`, `gnd` and every `.global` net left global.
//!
//! Definitions may come before or after their use, and nest by instantiation to any depth; a
//! subcircuit that instantiates itself, directly or not, is an error.
//!
//! # Limitations
//!
//! - **No subcircuit parameters** (`.subckt name a b w=1`, `params:`, or `name=value` on an
//!   instance of one): refused with an error naming the line, rather than instantiated with the
//!   definition's defaults.
//! - **No `.subckt` defined inside another** (SPICE allows local definitions; none of the decks
//!   here use them), and no `.lib` sections.
//! - `.model` cards apply to `X` and `N` lines only. An `M`/`D`/`Q` line keeps its existing
//!   meaning — a model-referencing primitive with fixed terminals — and does not read cards.
//! - A card's `level=` is dropped: it is SPICE's dispatch to a built-in model, and the card's
//!   type already names the Verilog-A module.

use crate::NetlistError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One logical line after continuation-joining and include expansion, with where it came from.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    /// The line's text, `+` continuations joined, `;` comment stripped, trimmed.
    pub text: String,
    /// 1-based line number in the file it came from (its first physical line).
    pub line: usize,
    /// Where the line came from when that is not the top-level deck itself: an included file,
    /// or a subcircuit instance. Appended to a parse error so it points somewhere findable.
    pub origin: Option<String>,
}

/// A `.model` card: the Verilog-A module it names and its parameters, `level` removed.
#[derive(Clone, Debug)]
pub(crate) struct ModelCard {
    /// The module name as written on the card (matched to a module ignoring case by `va-cli`).
    pub module: String,
    /// `name=value` parameters in the order written.
    pub params: Vec<(String, f64)>,
}

/// A deck with its SPICE structure resolved.
#[derive(Debug, Default)]
pub(crate) struct Expanded {
    /// Element and dot-card lines, subcircuits flattened; no `.model`, `.subckt`, `.include`.
    pub lines: Vec<Line>,
    /// `.model` cards, keyed by lower-cased card name (SPICE names are case-insensitive).
    pub models: HashMap<String, ModelCard>,
    /// What was recognised and not applied (see [`crate::Netlist::notes`]).
    pub notes: Vec<String>,
}

/// How deep `.include` may nest before it is taken to be a cycle.
const MAX_INCLUDE_DEPTH: usize = 16;

/// How deep subcircuit instantiation may nest. Real hierarchies are a handful of levels; this
/// only bounds a runaway, since direct and indirect recursion is caught by name first.
const MAX_SUBCKT_DEPTH: usize = 64;

/// Resolve `deck`'s SPICE structure. `base` is the directory `.include` paths are relative to —
/// the deck file's own directory; `None` (a deck given as a string) makes `.include` an error.
pub(crate) fn expand(deck: &str, base: Option<&Path>) -> Result<Expanded, NetlistError> {
    let mut out = Expanded::default();
    let mut logical = Vec::new();
    read_lines(deck, base, None, 0, &mut logical, &mut out.notes)?;

    // Pass 1: pull out `.model`, `.subckt` bodies and `.global`, wherever they appear.
    let mut subckts: HashMap<String, Subckt> = HashMap::new();
    let mut globals: HashSet<String> = HashSet::new();
    let mut top = Vec::new();
    let mut open: Option<Subckt> = None;
    for line in logical {
        let lower = line.text.to_ascii_lowercase();
        let head = lower.split_whitespace().next().unwrap_or("");
        match head {
            ".subckt" => {
                if let Some(outer) = &open {
                    return Err(err(
                        &line,
                        format!(
                            "`.subckt` inside the definition of `{}`: nested subcircuit \
                             definitions are not supported",
                            outer.name
                        ),
                    ));
                }
                open = Some(Subckt::from_header(&line)?);
            }
            ".ends" => {
                let Some(done) = open.take() else {
                    return Err(err(&line, "`.ends` with no open `.subckt`".to_string()));
                };
                let key = done.name.to_ascii_lowercase();
                if subckts.contains_key(&key) {
                    return Err(err(
                        &line,
                        format!("subcircuit `{}` is defined twice", done.name),
                    ));
                }
                subckts.insert(key, done);
            }
            ".model" => {
                let (name, card) = parse_model_card(&line)?;
                out.models.insert(name.to_ascii_lowercase(), card);
            }
            ".global" => {
                globals.extend(line.text.split_whitespace().skip(1).map(str::to_string));
            }
            _ => match &mut open {
                Some(sub) => sub.body.push(line),
                None => top.push(line),
            },
        }
    }
    if let Some(sub) = open {
        return Err(NetlistError::Parse {
            line: sub.line,
            message: format!("`.subckt {}` is never closed by `.ends`", sub.name),
        });
    }

    // Pass 2: flatten every subcircuit instance.
    let ctx = Flatten {
        subckts: &subckts,
        globals: &globals,
    };
    for line in top {
        ctx.emit(
            &line,
            None,
            &HashMap::new(),
            &mut Vec::new(),
            &mut out.lines,
        )?;
    }
    Ok(out)
}

/// Read `text` into logical lines: strip comments, join `+` continuations, expand `.include`,
/// and skip an ngspice `.control` … `.endc` block with a note.
fn read_lines(
    text: &str,
    base: Option<&Path>,
    file: Option<&str>,
    depth: usize,
    out: &mut Vec<Line>,
    notes: &mut Vec<String>,
) -> Result<(), NetlistError> {
    let origin = file.map(|f| format!("in `{f}`"));
    let mut in_control: Option<usize> = None;
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        // Comments: `*` lines, and the tail after `;`. A comment between a line and its `+`
        // continuation does not break the continuation (model cards comment out entries).
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        let trimmed = trimmed.split(';').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(start) = in_control {
            if lower.starts_with(".endc") {
                in_control = None;
                notes.push(format!(
                    "ignored the ngspice `.control` block at lines {start}–{line_no}{}: it is a \
                     script for ngspice's interpreter, not part of the circuit — write the \
                     analysis as a dot-card (`.tran`, `.op`, …)",
                    file.map(|f| format!(" of `{f}`")).unwrap_or_default()
                ));
            }
            continue;
        }
        if lower.starts_with(".control") {
            in_control = Some(line_no);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('+') {
            let Some(prev) = out.last_mut() else {
                return Err(NetlistError::Parse {
                    line: line_no,
                    message: "a `+` continuation line with no line before it to continue"
                        .to_string(),
                });
            };
            prev.text.push(' ');
            prev.text.push_str(rest.trim());
            continue;
        }
        if lower.starts_with(".include") || lower.starts_with(".inc ") {
            let arg = trimmed
                .split_once(char::is_whitespace)
                .map(|(_, a)| a.trim().trim_matches(['"', '\'']))
                .unwrap_or("");
            let here = Line {
                text: trimmed.to_string(),
                line: line_no,
                origin: origin.clone(),
            };
            if arg.is_empty() {
                return Err(err(&here, "`.include` names no file".to_string()));
            }
            let Some(dir) = base else {
                return Err(err(
                    &here,
                    format!(
                        "`.include {arg}` needs the deck's own location to resolve against; \
                         parse the deck from its file (`va_netlist::parser::parse_file`)"
                    ),
                ));
            };
            if depth >= MAX_INCLUDE_DEPTH {
                return Err(err(
                    &here,
                    format!("`.include` nested more than {MAX_INCLUDE_DEPTH} deep (a cycle?)"),
                ));
            }
            let path: PathBuf = dir.join(arg);
            let body = std::fs::read_to_string(&path).map_err(|e| {
                err(
                    &here,
                    format!("cannot read `.include {arg}` ({}): {e}", path.display()),
                )
            })?;
            let label = path.display().to_string();
            read_lines(&body, path.parent(), Some(&label), depth + 1, out, notes)?;
            continue;
        }
        if lower.starts_with(".option") {
            notes.push(format!(
                "not applied: `{trimmed}` — this simulator uses its own tolerances and `gmin` \
                 handling, not a deck's `.options`"
            ));
            continue;
        }
        out.push(Line {
            text: trimmed.to_string(),
            line: line_no,
            origin: origin.clone(),
        });
    }
    if let Some(start) = in_control {
        return Err(NetlistError::Parse {
            line: start,
            message: "`.control` is never closed by `.endc`".to_string(),
        });
    }
    Ok(())
}

/// A `.subckt` definition.
#[derive(Debug)]
struct Subckt {
    name: String,
    ports: Vec<String>,
    body: Vec<Line>,
    line: usize,
}

impl Subckt {
    fn from_header(line: &Line) -> Result<Self, NetlistError> {
        let toks: Vec<&str> = line.text.split_whitespace().collect();
        let Some(name) = toks.get(1) else {
            return Err(err(line, "`.subckt` names no subcircuit".to_string()));
        };
        let ports: Vec<String> = toks[2..].iter().map(|t| t.to_string()).collect();
        if let Some(p) = ports
            .iter()
            .find(|p| p.contains('=') || p.eq_ignore_ascii_case("params:"))
        {
            return Err(err(
                line,
                format!(
                    "`.subckt {name}` declares a parameter (`{p}`): subcircuit parameters are \
                     not supported"
                ),
            ));
        }
        Ok(Self {
            name: name.to_string(),
            ports,
            body: Vec::new(),
            line: line.line,
        })
    }
}

/// Parse a `.model <name> <type> [(] [name=value]... [)]` card. Spaces around `=` and the
/// optional parentheses are allowed, as SPICE allows them; `level` is dropped (module docs).
fn parse_model_card(line: &Line) -> Result<(String, ModelCard), NetlistError> {
    let mut text = line.text.replace(['(', ')'], " ");
    // `a = 1` and `a =1` to `a=1`.
    while text.contains(" =") || text.contains("= ") {
        text = text.replace(" =", "=").replace("= ", "=");
    }
    let toks: Vec<&str> = text.split_whitespace().collect();
    let (Some(name), Some(module)) = (toks.get(1), toks.get(2)) else {
        return Err(err(
            line,
            "`.model` needs a name and a type (`.model <name> <module> …`)".to_string(),
        ));
    };
    let mut params = Vec::new();
    for tok in &toks[3..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(err(
                line,
                format!("`.model {name}`: expected `name=value`, found `{tok}`"),
            ));
        };
        if key.eq_ignore_ascii_case("level") {
            continue;
        }
        let Some(v) = crate::parser::parse_value(value) else {
            return Err(err(
                line,
                format!("`.model {name}`: bad value `{value}` for `{key}`"),
            ));
        };
        params.push((key.to_string(), v));
    }
    Ok((
        name.to_string(),
        ModelCard {
            module: module.to_string(),
            params,
        },
    ))
}

/// The flattening pass's fixed context.
struct Flatten<'a> {
    subckts: &'a HashMap<String, Subckt>,
    globals: &'a HashSet<String>,
}

impl Flatten<'_> {
    /// Emit `line`, from the body of the instance at `path` (`None` at top level) whose ports
    /// map through `ports`; an `X` line naming a subcircuit is expanded in place.
    fn emit(
        &self,
        line: &Line,
        path: Option<&str>,
        ports: &HashMap<String, String>,
        stack: &mut Vec<String>,
        out: &mut Vec<Line>,
    ) -> Result<(), NetlistError> {
        let toks: Vec<&str> = line.text.split_whitespace().collect();
        let Some(first) = toks.first() else {
            return Ok(());
        };
        if first.starts_with('.') {
            if path.is_some() {
                return Err(err(
                    line,
                    format!("`{first}` inside a subcircuit body is not supported"),
                ));
            }
            out.push(line.clone());
            return Ok(());
        }
        let letter = first.chars().next().unwrap_or(' ').to_ascii_uppercase();

        // An `X` line naming a subcircuit: expand it.
        if letter == 'X' {
            let model_at = model_index(&toks);
            if let Some(sub) =
                model_at.and_then(|i| self.subckts.get(&toks[i].to_ascii_lowercase()))
            {
                let i = model_at.unwrap_or(0);
                return self.instantiate(line, &toks, i, sub, path, ports, stack, out);
            }
        }

        let Some(path) = path else {
            out.push(line.clone());
            return Ok(());
        };
        // A body line: rename the element, its internal nets, and any element it names.
        let (nodes, refs) = slots(letter, &toks);
        let mut renamed: Vec<String> = toks.iter().map(|t| t.to_string()).collect();
        renamed[0] = scoped_element(first, path);
        for i in nodes {
            renamed[i] = self.net(toks[i], path, ports);
        }
        for i in refs {
            renamed[i] = scoped_element(toks[i], path);
        }
        out.push(Line {
            text: renamed.join(" "),
            line: line.line,
            origin: Some(format!(
                "{}instance `{path}`",
                line.origin
                    .as_ref()
                    .map(|o| format!("{o}, "))
                    .unwrap_or_default()
            )),
        });
        Ok(())
    }

    /// Expand instance line `toks` (model at index `model_at`) of subcircuit `sub`.
    #[allow(clippy::too_many_arguments)]
    fn instantiate(
        &self,
        line: &Line,
        toks: &[&str],
        model_at: usize,
        sub: &Subckt,
        path: Option<&str>,
        ports: &HashMap<String, String>,
        stack: &mut Vec<String>,
        out: &mut Vec<Line>,
    ) -> Result<(), NetlistError> {
        let inst = toks[0];
        if toks.len() > model_at + 1 {
            return Err(err(
                line,
                format!(
                    "`{inst}` passes parameters to subcircuit `{}`: subcircuit parameters are \
                     not supported",
                    sub.name
                ),
            ));
        }
        let actual = &toks[1..model_at];
        if actual.len() != sub.ports.len() {
            return Err(err(
                line,
                format!(
                    "`{inst}` connects {} net(s) but subcircuit `{}` has {} port(s) ({})",
                    actual.len(),
                    sub.name,
                    sub.ports.len(),
                    sub.ports.join(" ")
                ),
            ));
        }
        let key = sub.name.to_ascii_lowercase();
        if stack.contains(&key) {
            return Err(err(
                line,
                format!(
                    "subcircuit `{}` instantiates itself (through {})",
                    sub.name,
                    stack.join(" → ")
                ),
            ));
        }
        if stack.len() >= MAX_SUBCKT_DEPTH {
            return Err(err(
                line,
                format!("subcircuits nested more than {MAX_SUBCKT_DEPTH} deep"),
            ));
        }
        let inner_path = match path {
            Some(p) => format!("{p}.{inst}"),
            None => inst.to_string(),
        };
        let inner_ports: HashMap<String, String> = sub
            .ports
            .iter()
            .zip(actual)
            .map(|(port, net)| {
                let net = match path {
                    Some(p) => self.net(net, p, ports),
                    None => net.to_string(),
                };
                (port.clone(), net)
            })
            .collect();
        stack.push(key);
        for body_line in &sub.body {
            self.emit(body_line, Some(&inner_path), &inner_ports, stack, out)?;
        }
        stack.pop();
        Ok(())
    }

    /// The flat name of net `name` seen from inside the instance at `path`.
    fn net(&self, name: &str, path: &str, ports: &HashMap<String, String>) -> String {
        if name == "0" || name.eq_ignore_ascii_case("gnd") || self.globals.contains(name) {
            return name.to_string();
        }
        match ports.get(name) {
            Some(actual) => actual.clone(),
            None => format!("{path}.{name}"),
        }
    }
}

/// An element name as seen from inside the instance at `path`: `<letter>.<path>.<name>`.
fn scoped_element(name: &str, path: &str) -> String {
    let letter: String = name.chars().take(1).collect();
    format!("{letter}.{path}.{name}")
}

/// Index of the model name on an `X`/`N` line: the last token that is not `name=value`.
fn model_index(toks: &[&str]) -> Option<usize> {
    let mut end = toks.len();
    while end > 1 && toks[end - 1].contains('=') {
        end -= 1;
    }
    (end >= 3).then_some(end - 1)
}

/// Which tokens of an element line are nets, and which name other elements, by element letter —
/// the same positions [`crate::parser`] reads them from.
fn slots(letter: char, toks: &[&str]) -> (Vec<usize>, Vec<usize>) {
    let upto = |n: usize| (1..n.min(toks.len())).collect::<Vec<_>>();
    match letter {
        'R' | 'C' | 'L' | 'D' | 'V' | 'I' => (upto(3), Vec::new()),
        'E' | 'G' => (upto(5), Vec::new()),
        'F' | 'H' => (upto(3), if toks.len() > 3 { vec![3] } else { Vec::new() }),
        'K' => (Vec::new(), upto(3)),
        'M' | 'Q' => (upto(4), Vec::new()),
        'X' | 'N' => (model_index(toks).map(upto).unwrap_or_default(), Vec::new()),
        _ => (Vec::new(), Vec::new()),
    }
}

fn err(line: &Line, message: String) -> NetlistError {
    NetlistError::Parse {
        line: line.line,
        message: match &line.origin {
            Some(o) => format!("{message} ({o})"),
            None => message,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{parse, parse_file};
    use crate::{Netlist, NetlistError};
    use va_abi::reference::GROUND;

    fn dev<'a>(net: &'a Netlist, name: &str) -> &'a crate::Device {
        net.devices
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("no device `{name}` in {:?}", net.devices))
    }

    fn err_text(deck: &str) -> String {
        match parse(deck) {
            Err(NetlistError::Parse { message, .. }) => message,
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    /// Ports map to the instance's nets, internal nets and element names are scoped by the
    /// instance path (ngspice's convention), ground stays global, and a definition may follow
    /// its use and instantiate another subcircuit.
    #[test]
    fn subcircuits_flatten_with_scoped_names() {
        let net = parse(
            "V1 in 0 DC 1\n\
             X1 in out gnd pair\n\
             .subckt pair a z g\n\
             R1 a mid 1k\n\
             Xin mid z g leg\n\
             .ends\n\
             .subckt leg p q g\n\
             R1 p q 2k\n\
             C1 q g 1p\n\
             .ends\n\
             .end\n",
        )
        .expect("parses");
        let r_outer = dev(&net, "R.X1.R1");
        assert_eq!(
            r_outer.terminals,
            vec![net.nodes["in"], net.nodes["X1.mid"]]
        );
        let r_inner = dev(&net, "R.X1.Xin.R1");
        assert_eq!(
            r_inner.terminals,
            vec![net.nodes["X1.mid"], net.nodes["out"]]
        );
        assert_eq!(r_inner.value, Some(2e3));
        let c = dev(&net, "C.X1.Xin.C1");
        assert_eq!(c.terminals, vec![net.nodes["out"], GROUND]);
        assert_eq!(net.devices.len(), 4, "{:?}", net.devices);
        // Two instances of one subcircuit share no internal net.
        let two =
            parse(".subckt s a\nR1 a n 1\nR2 n 0 1\n.ends\nX1 p s\nX2 q s\nV1 p 0 1\nV2 q 0 1\n")
                .expect("parses");
        assert_ne!(two.nodes["X1.n"], two.nodes["X2.n"]);
    }

    /// `+` joins a continuation to the line before, across comment lines; a `.model` card's
    /// module and parameters reach an `N` line, `level` is dropped, and the line's own values
    /// replace the card's, compared ignoring case.
    #[test]
    fn model_cards_and_continuations_reach_n_lines() {
        let net = parse(
            ".model nch PSP103VA level=69\n\
             +type=1\n\
             * a commented-out entry does not break the continuation\n\
             +TR = 27.0 L=1u\n\
             N1 d g s b nch l=0.12u w=2u\n\
             V1 d 0 1\n",
        )
        .expect("parses");
        let n = dev(&net, "N1");
        assert_eq!(n.model, "PSP103VA");
        assert!(n.spice_names);
        assert_eq!(n.terminals.len(), 4);
        let get = |k: &str| n.params.iter().find(|(p, _)| p == k).map(|(_, v)| *v);
        assert_eq!(get("type"), Some(1.0));
        assert_eq!(get("TR"), Some(27.0));
        assert_eq!(
            get("L"),
            Some(0.12e-6),
            "the line's `l` replaces the card's `L`"
        );
        assert_eq!(get("w"), Some(2e-6));
        assert!(n
            .params
            .iter()
            .all(|(p, _)| !p.eq_ignore_ascii_case("level")));
        // An `N` line with no card still reads SPICE's names; an `X` line keeps exact ones.
        let plain = parse("N1 a 0 mymod\nX2 a 0 mymod\nV1 a 0 1\n").expect("parses");
        assert!(dev(&plain, "N1").spice_names);
        assert!(!dev(&plain, "X2").spice_names);
    }

    /// An ngspice `.control` block and `.options` are skipped with a note each, not silently.
    #[test]
    fn control_blocks_and_options_are_noted_not_applied() {
        let net = parse(
            ".options gmin=1e-15\nV1 a 0 1\nR1 a 0 1k\n.control\ntran 1p 1n\n.endc\n.op\n.end\n",
        )
        .expect("parses");
        assert_eq!(net.devices.len(), 2);
        assert_eq!(net.notes.len(), 2, "{:?}", net.notes);
        assert!(net.notes.iter().any(|n| n.contains(".control")));
        assert!(net.notes.iter().any(|n| n.contains(".options")));
    }

    /// `.global` nets are not scoped by the instance.
    #[test]
    fn global_nets_are_shared_by_every_instance() {
        let net = parse(".global vdd\n.subckt s a\nR1 a vdd 1\n.ends\nX1 p s\nV1 vdd 0 1\n")
            .expect("parses");
        assert_eq!(dev(&net, "R.X1.R1").terminals[1], net.nodes["vdd"]);
        assert!(!net.nodes.contains_key("X1.vdd"));
    }

    /// `.include` resolves relative to the including file's directory, nested includes
    /// relative to theirs; a deck given as a string cannot include.
    #[test]
    fn include_resolves_relative_to_the_including_file() {
        let dir = std::env::temp_dir().join(format!("va_netlist_include_{}", std::process::id()));
        let sub = dir.join("cards");
        std::fs::create_dir_all(&sub).expect("temp dir");
        std::fs::write(sub.join("a.inc"), ".include b.inc\n").expect("write");
        std::fs::write(sub.join("b.inc"), ".model rc RES r=2k\n").expect("write");
        std::fs::write(
            dir.join("deck.net"),
            ".include cards/a.inc\nX1 p 0 rc\nV1 p 0 1\n",
        )
        .expect("write");
        let net = parse_file(&dir.join("deck.net")).expect("parses");
        let x = dev(&net, "X1");
        assert_eq!(x.model, "RES");
        assert_eq!(x.params, vec![("r".to_string(), 2e3)]);
        std::fs::remove_dir_all(&dir).ok();
        assert!(err_text(".include cards/a.inc\n").contains("parse_file"));
    }

    /// Malformed structure is an error naming what is wrong, never a guess.
    #[test]
    fn malformed_subcircuits_are_errors() {
        let wrong_ports = err_text(".subckt s a b\nR1 a b 1\n.ends\nX1 p s\n");
        assert!(wrong_ports.contains("1 net(s)") && wrong_ports.contains("2 port(s)"));
        let recursive = err_text(".subckt s a\nX1 a s\n.ends\nX1 p s\n");
        assert!(recursive.contains("instantiates itself"), "{recursive}");
        let unclosed = err_text(".subckt s a\nR1 a 0 1\n");
        assert!(unclosed.contains("never closed"), "{unclosed}");
        let params = err_text(".subckt s a w=1\nR1 a 0 1\n.ends\n");
        assert!(params.contains("parameter"), "{params}");
        let passed = err_text(".subckt s a\nR1 a 0 1\n.ends\nX1 p s w=2\n");
        assert!(passed.contains("parameters"), "{passed}");
        let nested = err_text(".subckt s a\n.subckt t b\n.ends\n.ends\n");
        assert!(nested.contains("nested"), "{nested}");
        // An error inside a body names the instance it came from.
        let inner = err_text(".subckt s a\nR1 a 0 notanumber\n.ends\nX7 p s\n");
        assert!(inner.contains("instance `X7`"), "{inner}");
    }
}
