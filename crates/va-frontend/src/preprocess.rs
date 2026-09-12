//! Verilog-A preprocessor: a text→text pass run before the lexer.
//!
//! Handles `` `define `` (object- and function-like), `` `undef ``,
//! `` `ifdef``/`` `ifndef``/`` `elsif``/`` `else``/`` `endif `` conditional compilation,
//! `` `include `` resolution, and recursive macro expansion. The output is directive-free,
//! macro-expanded source the [`crate::lexer`] can tokenize directly.
//!
//! # Scope & limitations
//!
//! - **Unresolved `` `include `` is skipped** (not an error): the standard disciplines are
//!   built into the frontend, so a model still compiles with no include path. When the header
//!   *is* on the include path it expands for real (its `discipline`/`nature` blocks are then
//!   skipped by the parser). Skipping is *not* silent — every name skipped is recorded and
//!   returned by [`preprocess_reporting`], because a vendor model whose whole body lives in an
//!   absent `` `include `` otherwise preprocesses to a shell that elaborates cleanly and looks
//!   like a pass. See that function's doc comment for how badly this distorted a corpus metric.
//! - **Undefined macro usage is an error.**
//! - Comments and string literals are honoured: directives/macros inside them are ignored.
//! - No `` `__FILE__ ``/`` `__LINE__ ``, token-pasting (`##`), or stringization (`#`).
//! - **Every LRM directive is recognised, and none is dropped silently** (2026-09-12,
//!   `docs/proposals/directives.md`). The ones that change what a module means —
//!   `` `default_discipline ``, `` `default_transition ``, `` `begin_keywords ``/
//!   `` `end_keywords ``, `` `resetall `` — are recorded as [`DirectiveEvent`]s at the output
//!   offset where they take effect, for the lexer (keyword set) and the parser (per-module
//!   [`Settings`]) to apply. The ones the LRM gives no Verilog-A meaning — `` `timescale ``,
//!   `` `celldefine ``/`` `endcelldefine ``, `` `default_nettype ``, `` `unconnected_drive ``/
//!   `` `nounconnected_drive `` — are accepted and *reported* in [`Preprocessed::warnings`]
//!   (once per directive per file), so a user is told rather than left to wonder. `` `pragma ``
//!   is ignored as IEEE 1364-2005 §19.9 requires, except a `protect` envelope, which is a
//!   [`crate::Refusal`]: encrypted model text a clean-room tool cannot read. The obsolete
//!   `` `default_nodetype `` (an AMS-1.x spelling the LRM lists only in its change history) is
//!   an error naming `` `default_discipline ``.

use crate::FrontendError;
use std::collections::HashMap;
use std::path::PathBuf;

/// Preprocess `source`, resolving `` `include `` against `include_dirs` (searched in order),
/// discarding the record of which includes could not be resolved.
///
/// Prefer [`preprocess_reporting`] anywhere the answer is used to judge whether a *file* is
/// well-formed — this wrapper cannot distinguish a model that compiled from one whose body was
/// silently deleted.
///
/// # Errors
///
/// Returns [`FrontendError::Preprocess`] on an undefined macro, malformed directive,
/// unbalanced conditional, or include cycle.
pub fn preprocess(source: &str, include_dirs: &[PathBuf]) -> Result<String, FrontendError> {
    preprocess_reporting(source, include_dirs).0
}

/// Preprocess `source` as [`preprocess`] does, additionally returning **every `` `include ``
/// name that could not be resolved** and was therefore dropped, in the order encountered
/// (de-duplicated).
///
/// # Why this is worth returning
///
/// Skipping an unresolved include is deliberate and load-bearing — the standard headers
/// (`disciplines.vams`, `constants.vams`) are built into the frontend, so requiring them on disk
/// would reject almost every real model. But the same rule quietly turns a *truncated
/// distribution* into an apparent success. Several vendor compact models are a licence header, a
/// `module` line, one `` `include "..._module.include" `` holding the entire body, and
/// `endmodule`. With the body file absent, what reaches the parser is an empty module — which
/// elaborates perfectly.
///
/// Measured on this project's own 150-file corpus (2026-08-29): 16 model files passed
/// `va-cli check` reporting **0 parameters and 0 functions**, among them `bsimcmg.va` — a
/// BSIM-CMG with no parameters is self-evidently not a real pass. Those files differ from the
/// ten that *fail* with "port has no discipline declaration" only in whether their ports happen
/// to be declared inline before the vanished include. One defect, two opposite verdicts, and a
/// corpus coverage number that measured neither. Returning the skipped names is what lets a
/// caller tell the three cases apart.
///
/// # Why the skipped list is returned *beside* the result, not inside the `Ok`
///
/// A file can both drop an include **and** then fail to preprocess — indeed that is the normal
/// shape of a truncated vendor distribution, where the absent body file also held the macro
/// definitions the surviving text goes on to use (`` `IPRoz ``, `` `MAXA ``, …). Returning the
/// list only on success made those two facts unreportable together: the caller saw an error and
/// had no way to learn the error was *caused* by a truncation rather than by a frontend gap. So
/// the list comes back either way, and `Err` means only "this did not preprocess", never "there
/// is nothing more to say about why".
///
/// # Errors
///
/// The `Result` half is [`FrontendError::Preprocess`] on an undefined macro, malformed
/// directive, unbalanced conditional, or include cycle — identical to [`preprocess`].
pub fn preprocess_reporting(
    source: &str,
    include_dirs: &[PathBuf],
) -> (Result<String, FrontendError>, Vec<String>) {
    let (result, skipped) = preprocess_full(source, include_dirs);
    (result.map(|p| p.text), skipped)
}

/// The complete result of preprocessing: the expanded text plus everything the directives
/// decided that later stages must honour. See the module doc for which directive goes where.
#[derive(Debug, Clone, Default)]
pub struct Preprocessed {
    /// Directive-free, macro-expanded source for the lexer.
    pub text: String,
    /// Every directive that changes how following text is read, in output order, at the byte
    /// offset of `text` from which it applies. Replayed by [`settings_at`] for the parser and
    /// by `crate::lexer::lex_spanned_with` for the keyword set.
    pub directives: Vec<DirectiveEvent>,
    /// Directives the LRM gives no Verilog-A meaning, reported once each: "`` `timescale `` has
    /// no effect in Verilog-A: …". A caller prints them; nothing downstream depends on them.
    pub warnings: Vec<String>,
}

/// One directive with text-stream scope, and where in the expanded text it starts to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectiveEvent {
    /// Byte offset into [`Preprocessed::text`]; the directive governs everything from here to
    /// the next event that supersedes it.
    pub offset: usize,
    pub directive: Directive,
}

/// A directive with text-stream scope (LRM Clause 10: "from the point where it is processed,
/// across all files processed, to the point where another compiler directive supersedes it").
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    /// `` `default_discipline [name] `` (LRM 10.2): the discipline for nets declared without
    /// one; `None` is the bare form, which turns the default off again.
    DefaultDiscipline(Option<String>),
    /// `` `default_transition t `` (LRM 10.3): the rise/fall time a `transition()` or Z-domain
    /// filter uses when it omits its own, in seconds.
    DefaultTransition(f64),
    /// `` `begin_keywords "spec" `` (LRM 10.6): the reserved-word set from here on.
    BeginKeywords(KeywordSet),
    /// `` `end_keywords ``: back to the set in force before the matching `begin_keywords`.
    EndKeywords,
    /// `` `resetall `` (IEEE 1364-2005 §19.6): every directive back to its default. Macros are
    /// untouched — 1364 is explicit that `resetall` does not undefine them.
    ResetAll,
}

/// A reserved-word set a `` `begin_keywords `` may select (LRM 10.6). The three IEEE 1364 sets
/// are strict subsets of `Vams23`; `crate::keywords` holds the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordSet {
    /// `"VAMS-2.3"` — Annex B, the default and everything this lexer reserves.
    Vams23,
    /// `"1364-2005"`.
    Ieee2005,
    /// `"1364-2001"`.
    Ieee2001,
    /// `"1364-1995"`.
    Ieee1995,
}

impl KeywordSet {
    /// Parse the LRM's version specifier strings.
    pub fn from_spec(spec: &str) -> Option<KeywordSet> {
        match spec {
            "VAMS-2.3" | "VAMS-2.4" => Some(KeywordSet::Vams23),
            "1364-2005" => Some(KeywordSet::Ieee2005),
            "1364-2001" | "1364-2001-noconfig" => Some(KeywordSet::Ieee2001),
            "1364-1995" => Some(KeywordSet::Ieee1995),
            _ => None,
        }
    }
}

/// What the directives have decided for a point in the text — the state a module inherits
/// where its `module` keyword sits. Every directive that can carry this state is illegal
/// inside a module body (LRM 10.3, 10.6), so "in force at the module's start" is exactly the
/// LRM's scoping, not an approximation of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Discipline applied to a net declared without one, if a `` `default_discipline `` with a
    /// name is in force.
    pub default_discipline: Option<String>,
    /// Default transition time in seconds, if a `` `default_transition `` is in force.
    pub default_transition: Option<f64>,
    /// The reserved-word set in force.
    pub keywords: KeywordSet,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_discipline: None,
            default_transition: None,
            keywords: KeywordSet::Vams23,
        }
    }
}

/// Replay `events` up to (and including) any at `offset`, giving the [`Settings`] in force
/// there. `begin_keywords`/`end_keywords` nest (a stack); `resetall` empties everything.
pub fn settings_at(events: &[DirectiveEvent], offset: usize) -> Settings {
    let mut st = Settings::default();
    let mut kw_stack: Vec<KeywordSet> = Vec::new();
    for ev in events.iter().take_while(|e| e.offset <= offset) {
        match &ev.directive {
            Directive::DefaultDiscipline(d) => st.default_discipline = d.clone(),
            Directive::DefaultTransition(t) => st.default_transition = Some(*t),
            Directive::BeginKeywords(set) => {
                kw_stack.push(st.keywords);
                st.keywords = *set;
            }
            Directive::EndKeywords => {
                st.keywords = kw_stack.pop().unwrap_or(KeywordSet::Vams23);
            }
            Directive::ResetAll => {
                st = Settings::default();
                kw_stack.clear();
            }
        }
    }
    st
}

/// Preprocess `source` and return everything the directives decided, plus the skipped
/// includes (as [`preprocess_reporting`]). This is the entry point the real pipeline uses;
/// [`preprocess`] and [`preprocess_reporting`] keep only the text.
///
/// # Errors
///
/// As [`preprocess`], plus a malformed `` `default_discipline ``/`` `default_transition ``/
/// `` `begin_keywords `` argument, an `` `end_keywords `` with no matching begin, the obsolete
/// `` `default_nodetype ``, and — as a [`crate::Refusal`] — a `` `pragma protect `` envelope.
pub fn preprocess_full(
    source: &str,
    include_dirs: &[PathBuf],
) -> (Result<Preprocessed, FrontendError>, Vec<String>) {
    let mut pp = Preprocessor {
        include_dirs: include_dirs.to_vec(),
        macros: HashMap::new(),
        cond: Vec::new(),
        include_stack: Vec::new(),
        out: String::new(),
        skipped_includes: Vec::new(),
        directives: Vec::new(),
        warnings: Vec::new(),
        warned: Vec::new(),
        keyword_depth: 0,
    };
    let mut result = pp.process_str(source);
    if result.is_ok() && !pp.cond.is_empty() {
        result = Err(pp_err("unterminated `ifdef/`ifndef (missing `endif)"));
    }
    if result.is_ok() && pp.keyword_depth > 0 {
        result = Err(pp_err(
            "unterminated `begin_keywords (missing `end_keywords)",
        ));
    }
    let skipped = std::mem::take(&mut pp.skipped_includes);
    (
        result.map(|()| Preprocessed {
            text: pp.out,
            directives: pp.directives,
            warnings: pp.warnings,
        }),
        skipped,
    )
}

/// A stored macro definition. `params` is `Some` for a function-like macro.
struct Macro {
    params: Option<Vec<String>>,
    body: String,
}

/// One frame of the `` `ifdef `` conditional stack.
struct CondFrame {
    /// Whether the enclosing region was emitting.
    parent_active: bool,
    /// Whether the current branch emits.
    branch_active: bool,
    /// Whether any branch in this if/elsif/else chain has matched.
    any_taken: bool,
}

struct Preprocessor {
    include_dirs: Vec<PathBuf>,
    macros: HashMap<String, Macro>,
    cond: Vec<CondFrame>,
    include_stack: Vec<PathBuf>,
    out: String,
    /// Every `` `include `` name that no `include_dirs` entry could resolve, in encounter order
    /// and de-duplicated. See [`preprocess_reporting`].
    skipped_includes: Vec<String>,
    /// See [`Preprocessed::directives`].
    directives: Vec<DirectiveEvent>,
    /// See [`Preprocessed::warnings`].
    warnings: Vec<String>,
    /// Directive names already warned about, so a file that repeats `` `timescale `` gets one
    /// line, not one per occurrence.
    warned: Vec<&'static str>,
    /// Open `` `begin_keywords `` regions, for the unmatched-`end_keywords` check.
    keyword_depth: usize,
}

impl Preprocessor {
    /// Whether the current region is emitting output.
    fn emitting(&self) -> bool {
        self.cond.last().is_none_or(|f| f.branch_active)
    }

    /// Process a whole source string: strip comments, join line continuations, run each line.
    fn process_str(&mut self, source: &str) -> Result<(), FrontendError> {
        let stripped = strip_comments(source);
        let lines: Vec<&str> = stripped.split('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let mut line = lines[i].to_string();
            i += 1;
            // Join `\`-continued lines into one logical line.
            while line.trim_end().ends_with('\\') {
                let t = line.trim_end();
                line = t[..t.len() - 1].to_string();
                if i < lines.len() {
                    line.push(' ');
                    line.push_str(lines[i]);
                    i += 1;
                } else {
                    break;
                }
            }
            self.process_line(&line)?;
        }
        Ok(())
    }

    fn process_line(&mut self, line: &str) -> Result<(), FrontendError> {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('`') {
            let (name, after) = split_ident(rest);
            // Conditional directives are handled even inside an inactive region (to track
            // nesting); everything else only when emitting.
            match name {
                "ifdef" => return self.do_ifdef(after, false),
                "ifndef" => return self.do_ifdef(after, true),
                "elsif" => return self.do_elsif(after),
                "else" => return self.do_else(),
                "endif" => return self.do_endif(),
                _ => {}
            }
            if !self.emitting() {
                return Ok(());
            }
            match name {
                "define" => return self.do_define(after),
                "undef" => {
                    self.macros.remove(split_ident(after).0);
                    return Ok(());
                }
                "include" => return self.do_include(after),
                // Directives with text-stream scope: recorded at the offset from which they
                // apply, for the lexer and parser to honour (`docs/proposals/directives.md`).
                "default_discipline" => return self.do_default_discipline(after),
                "default_transition" => return self.do_default_transition(after),
                "begin_keywords" => return self.do_begin_keywords(after),
                "end_keywords" => {
                    if self.keyword_depth == 0 {
                        return Err(pp_err("`end_keywords without a matching `begin_keywords"));
                    }
                    self.keyword_depth -= 1;
                    self.record(Directive::EndKeywords);
                    return Ok(());
                }
                "resetall" => {
                    self.keyword_depth = 0;
                    self.record(Directive::ResetAll);
                    return Ok(());
                }
                // Directives the LRM gives no Verilog-A meaning: accepted, and said so.
                "timescale" => {
                    return self.warn_no_effect(
                        "timescale",
                        "Verilog-A has no delay controls (`#`) and `$abstime` is in seconds \
                         regardless (LRM 9.17), so the time unit it sets is never read",
                    )
                }
                "celldefine" | "endcelldefine" => {
                    return self.warn_no_effect(
                        "celldefine",
                        "it tags a module as a library cell for delay-annotation (SDF) tools; a \
                         simulator has nothing to do with the tag",
                    )
                }
                "default_nettype" => {
                    return self.warn_no_effect(
                        "default_nettype",
                        "it names a *digital* net type for implicitly declared nets (`wire`, \
                         `tri`, …), which Annex C excludes from Verilog-A; an undeclared net in \
                         an instance connection is already an error here, which is what \
                         `default_nettype none` asks for",
                    )
                }
                "unconnected_drive" | "nounconnected_drive" => {
                    return self.warn_no_effect(
                        "unconnected_drive",
                        "it drives unconnected *digital* input ports to a logic level; \
                         Verilog-A ports are analog nets",
                    )
                }
                // `pragma`: IEEE 1364-2005 §19.9 requires a tool to ignore pragmas it does not
                // understand, so an unknown one is not even worth a warning. The one this
                // project must understand is `protect`, whose envelope holds encrypted text.
                "pragma" => {
                    let (word, _) = split_ident(after.trim_start());
                    if word == "protect" {
                        return Err(FrontendError::Refused(crate::Refusal::new(
                            "`pragma protect`, an encrypted model envelope",
                            "the text inside the envelope is ciphertext: a clean-room simulator \
                             holds no vendor key, and lexing the bytes would produce an error \
                             naming a garbage token rather than the real reason",
                        )
                        .instead("obtain the unencrypted source, or a model written for open tools")
                        .tracking("docs/proposals/directives.md section 2.2")));
                    }
                    return Ok(());
                }
                // `line` is consumed here for now; honouring it needs the line map of
                // docs/proposals/directives.md section 2.1 (item 5), which every diagnostic then
                // benefits from. Until that lands the directive is parsed and dropped, and the
                // token reference says so.
                "line" => return Ok(()),
                // Not an LRM directive: the obsolete AMS-1.x spelling the LRM lists only in its
                // Annex F change history. Accepting it silently would be worse than an error —
                // a model relying on it expects a default discipline this engine would not apply.
                "default_nodetype" | "default_nodeType" => {
                    return Err(pp_err(
                        "`default_nodetype is not a Verilog-AMS 2.4 directive (an obsolete \
                         AMS-1.x spelling); write `default_discipline <discipline> instead",
                    ))
                }
                // Not a directive: a leading macro usage. Fall through to expansion.
                _ => {}
            }
        }
        if self.emitting() {
            let expanded = self.expand(line, 0)?;
            self.out.push_str(&expanded);
            self.out.push('\n');
        }
        Ok(())
    }

    // --- conditionals ---------------------------------------------------------------

    fn do_ifdef(&mut self, after: &str, negate: bool) -> Result<(), FrontendError> {
        let name = split_ident(after).0;
        let defined = self.macros.contains_key(name);
        let cond = defined ^ negate;
        let parent = self.emitting();
        self.cond.push(CondFrame {
            parent_active: parent,
            branch_active: parent && cond,
            any_taken: cond,
        });
        Ok(())
    }

    fn do_elsif(&mut self, after: &str) -> Result<(), FrontendError> {
        let defined = self.macros.contains_key(split_ident(after).0);
        let frame = self
            .cond
            .last_mut()
            .ok_or_else(|| pp_err("`elsif without `ifdef"))?;
        let take = frame.parent_active && !frame.any_taken && defined;
        frame.branch_active = take;
        frame.any_taken |= take;
        Ok(())
    }

    fn do_else(&mut self) -> Result<(), FrontendError> {
        let frame = self
            .cond
            .last_mut()
            .ok_or_else(|| pp_err("`else without `ifdef"))?;
        frame.branch_active = frame.parent_active && !frame.any_taken;
        frame.any_taken = true;
        Ok(())
    }

    fn do_endif(&mut self) -> Result<(), FrontendError> {
        self.cond
            .pop()
            .ok_or_else(|| pp_err("`endif without `ifdef"))?;
        Ok(())
    }

    // --- define / include -----------------------------------------------------------

    fn do_define(&mut self, after: &str) -> Result<(), FrontendError> {
        let after = after.trim_start();
        let (name, rest) = split_ident(after);
        if name.is_empty() {
            return Err(pp_err("`define missing a macro name"));
        }
        // Function-like only when `(` immediately follows the name (no space).
        let (params, body) = if rest.starts_with('(') {
            let (args, end) = split_args(rest, 0)?;
            (Some(args), rest[end..].trim().to_string())
        } else {
            (None, rest.trim().to_string())
        };
        self.macros.insert(name.to_string(), Macro { params, body });
        Ok(())
    }

    fn do_include(&mut self, after: &str) -> Result<(), FrontendError> {
        let file =
            parse_quoted(after).ok_or_else(|| pp_err("`include expects a quoted file name"))?;
        let Some(path) = self.resolve_include(&file) else {
            // Unresolved include: skipped (standard headers are built in), but recorded — see
            // `preprocess_reporting` for why a silent skip is not good enough.
            if !self.skipped_includes.contains(&file) {
                self.skipped_includes.push(file);
            }
            return Ok(());
        };
        if self.include_stack.contains(&path) {
            return Err(pp_err(&format!("`include cycle on {}", path.display())));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| pp_err(&format!("reading `include {}: {e}", path.display())))?;

        // Resolve nested includes relative to this file's directory too.
        if let Some(dir) = path.parent() {
            self.include_dirs.push(dir.to_path_buf());
        }
        self.include_stack.push(path.clone());
        let result = self.process_str(&content);
        self.include_stack.pop();
        if path.parent().is_some() {
            self.include_dirs.pop();
        }
        result
    }

    // --- directives with text-stream scope ------------------------------------------

    /// Record `directive` as applying from the current end of the output text.
    fn record(&mut self, directive: Directive) {
        self.directives.push(DirectiveEvent {
            offset: self.out.len(),
            directive,
        });
    }

    /// `` `default_discipline [discipline_identifier [qualifier]] `` (LRM 10.2). The bare
    /// form switches the default off. A qualifier (`integer`, `reg`, `wire`, `tri`, …) scopes
    /// the default to a *discrete* net kind, which Annex C excludes from Verilog-A — an error
    /// naming the annex rather than a silently unqualified default.
    fn do_default_discipline(&mut self, after: &str) -> Result<(), FrontendError> {
        let (name, rest) = split_ident(after.trim_start());
        if name.is_empty() {
            self.record(Directive::DefaultDiscipline(None));
            return Ok(());
        }
        let (qualifier, _) = split_ident(rest.trim_start());
        if !qualifier.is_empty() {
            return Err(pp_err(&format!(
                "`default_discipline {name} {qualifier}: a qualifier scopes the default to a \
                 discrete net kind, which Annex C excludes from Verilog-A; write \
                 `default_discipline {name} for every net declared without a discipline"
            )));
        }
        self.record(Directive::DefaultDiscipline(Some(name.to_string())));
        Ok(())
    }

    /// `` `default_transition transition_time `` (LRM 10.3). The argument is a constant
    /// expression in the LRM; here it must reduce, after macro expansion, to a single numeric
    /// literal (SI suffixes allowed) — a stated limitation, since the preprocessor has no
    /// expression evaluator and the parser's runs too late to feed the lexer's region logic.
    fn do_default_transition(&mut self, after: &str) -> Result<(), FrontendError> {
        let text = self.expand(after.trim(), 0)?;
        let text = text.trim();
        let value = crate::lexer::parse_scaled_number(text).ok_or_else(|| {
            pp_err(&format!(
                "`default_transition expects a numeric literal (SI suffix allowed, e.g. \
                 `default_transition 1n), got `{text}`; an expression here is not supported"
            ))
        })?;
        if value < 0.0 || !value.is_finite() {
            return Err(pp_err(&format!(
                "`default_transition {text}: a transition time must be non-negative and finite"
            )));
        }
        self.record(Directive::DefaultTransition(value));
        Ok(())
    }

    /// `` `begin_keywords "version_specifier" `` (LRM 10.6).
    fn do_begin_keywords(&mut self, after: &str) -> Result<(), FrontendError> {
        let spec = parse_quoted(after)
            .ok_or_else(|| pp_err("`begin_keywords expects a quoted version specifier"))?;
        let set = KeywordSet::from_spec(&spec).ok_or_else(|| {
            pp_err(&format!(
                "`begin_keywords \"{spec}\": unknown version specifier; the LRM (10.6) defines \
                 \"VAMS-2.3\", \"1364-2005\", \"1364-2001\" and \"1364-1995\""
            ))
        })?;
        self.keyword_depth += 1;
        self.record(Directive::BeginKeywords(set));
        Ok(())
    }

    /// Accept a directive the LRM gives no Verilog-A meaning, saying so once per file.
    fn warn_no_effect(&mut self, name: &'static str, why: &str) -> Result<(), FrontendError> {
        if !self.warned.contains(&name) {
            self.warned.push(name);
            self.warnings
                .push(format!("`{name} has no effect in Verilog-A: {why}"));
        }
        Ok(())
    }

    fn resolve_include(&self, file: &str) -> Option<PathBuf> {
        for dir in &self.include_dirs {
            let candidate = dir.join(file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let direct = PathBuf::from(file);
        if direct.is_file() {
            return Some(direct);
        }
        // Fallback: an `` `include `` naming a subdirectory path (e.g. a vendor's own
        // `ekv3_include/ekv3_definitions.va`) that doesn't exist relative to any search
        // directory, even though a file with the same *basename* does — real when a corpus
        // flattens a vendor's original directory layout without rewriting its own `include`
        // directives (confirmed against `external/ekv3.va` and its `ekv3_include/*.va`
        // siblings, which this corpus snapshot ships directly under `external/`). Tried only
        // after every exact-path candidate above fails, so an exact match always wins; scoped
        // to the same already-configured search directories, not a new filesystem walk, so it
        // can't reach across an unrelated library folder that happens to ship a same-named
        // header (e.g. two different vendors' own `disciplines.vams`).
        let basename = std::path::Path::new(file).file_name()?;
        for dir in &self.include_dirs {
            let candidate = dir.join(basename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    // --- macro expansion ------------------------------------------------------------

    /// Expand all macro usages in `text`, recursing into bodies/arguments. Strings are copied
    /// verbatim.
    fn expand(&self, text: &str, depth: usize) -> Result<String, FrontendError> {
        if depth > 64 {
            return Err(pp_err("macro expansion too deep (recursive macro?)"));
        }
        let b = text.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i] as char;
            if c == '"' {
                let end = copy_string(text, i, &mut out);
                i = end;
                continue;
            }
            if c == '`' {
                // A macro name immediately follows the backtick (no whitespace).
                let (name, _) = split_ident(&text[i + 1..]);
                if name.is_empty() {
                    return Err(pp_err("stray ` in source"));
                }
                let name_end = i + 1 + name.len();
                let mac = self
                    .macros
                    .get(name)
                    .ok_or_else(|| pp_err(&format!("undefined macro `{name}")))?;
                match &mac.params {
                    Some(params) => {
                        // Function-like: arguments follow (after optional whitespace).
                        let mut k = name_end;
                        while k < b.len() && (b[k] as char).is_whitespace() {
                            k += 1;
                        }
                        if k >= b.len() || b[k] as char != '(' {
                            return Err(pp_err(&format!("macro `{name} expects arguments")));
                        }
                        let (args, end) = split_args(text, k)?;
                        if args.len() != params.len() {
                            return Err(pp_err(&format!(
                                "macro `{name}: expected {} argument(s), got {}",
                                params.len(),
                                args.len()
                            )));
                        }
                        let substituted = substitute_params(&mac.body, params, &args);
                        out.push_str(&self.expand(&substituted, depth + 1)?);
                        i = end;
                    }
                    None => {
                        out.push_str(&self.expand(&mac.body, depth + 1)?);
                        i = name_end;
                    }
                }
                continue;
            }
            out.push(c);
            i += 1;
        }
        Ok(out)
    }
}

// --- free helpers --------------------------------------------------------------------

fn pp_err(msg: &str) -> FrontendError {
    FrontendError::Preprocess(msg.to_string())
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Split a leading identifier off `s` (after skipping leading whitespace), returning
/// `(ident, rest)`. `ident` is empty if `s` does not start with an identifier.
fn split_ident(s: &str) -> (&str, &str) {
    let trimmed = s.trim_start();
    let off = s.len() - trimmed.len();
    let b = trimmed.as_bytes();
    if b.is_empty() || !is_ident_start(b[0] as char) {
        return ("", &s[off..]);
    }
    let mut j = 0;
    while j < b.len() && is_ident_char(b[j] as char) {
        j += 1;
    }
    (&trimmed[..j], &trimmed[j..])
}

/// Parse a parenthesised, comma-separated list starting at byte index `open` (which must be a
/// `(`). Returns the trimmed argument strings and the index just past the matching `)`.
fn split_args(text: &str, open: usize) -> Result<(Vec<String>, usize), FrontendError> {
    let b = text.as_bytes();
    debug_assert_eq!(b[open] as char, '(');
    let mut i = open + 1;
    let mut depth = 1usize;
    let mut args = Vec::new();
    let mut cur = String::new();
    while i < b.len() {
        let c = b[i] as char;
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
                cur.push(c);
            }
            ',' if depth == 1 => {
                args.push(cur.trim().to_string());
                cur.clear();
            }
            '"' => {
                i = copy_string(text, i, &mut cur);
                continue;
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    if depth != 0 {
        return Err(pp_err("unbalanced `(` in macro/argument list"));
    }
    let trimmed = cur.trim();
    if !trimmed.is_empty() || !args.is_empty() {
        args.push(trimmed.to_string());
    }
    Ok((args, i))
}

/// Substitute function-like macro parameters with their argument text (identifier-aware;
/// strings are left untouched).
fn substitute_params(body: &str, params: &[String], args: &[String]) -> String {
    let map: HashMap<&str, &str> = params
        .iter()
        .map(String::as_str)
        .zip(args.iter().map(String::as_str))
        .collect();
    let b = body.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c == '"' {
            i = copy_string(body, i, &mut out);
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < b.len() && is_ident_char(b[i] as char) {
                i += 1;
            }
            let id = &body[start..i];
            out.push_str(map.get(id).copied().unwrap_or(id));
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Copy a double-quoted string literal starting at index `i` (a `"`) into `out`, returning the
/// index just past the closing quote.
fn copy_string(text: &str, i: usize, out: &mut String) -> usize {
    let b = text.as_bytes();
    out.push('"');
    let mut j = i + 1;
    while j < b.len() {
        let c = b[j] as char;
        out.push(c);
        j += 1;
        if c == '\\' && j < b.len() {
            out.push(b[j] as char);
            j += 1;
        } else if c == '"' {
            break;
        }
    }
    j
}

/// Extract the first double-quoted token from `s` (for `` `include "file" ``).
fn parse_quoted(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

/// Remove `//` and `/* */` comments, preserving newlines and string literals.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    let mut in_line = false;
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push('\n');
            }
            continue;
        }
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            } else if c == '\n' {
                out.push('\n');
            }
            continue;
        }
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                in_line = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                in_block = true;
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(src: &str) -> String {
        preprocess(src, &[]).expect("preprocess")
    }

    #[test]
    fn object_macro_expands_recursively() {
        let out = pp("`define A 2\n`define B (`A + 1)\nx = `B;\n");
        assert_eq!(out.split_whitespace().collect::<String>(), "x=(2+1);");
    }

    #[test]
    fn function_macro_substitutes_args() {
        let out = pp("`define SQ(x) ((x)*(x))\ny = `SQ(a + 1);\n");
        assert_eq!(
            out.split_whitespace().collect::<String>(),
            "y=((a+1)*(a+1));"
        );
    }

    #[test]
    fn ifdef_else_endif() {
        assert_eq!(pp("`ifdef FOO\na\n`else\nb\n`endif\n").trim(), "b");
        assert_eq!(
            pp("`define FOO\n`ifdef FOO\na\n`else\nb\n`endif\n").trim(),
            "a"
        );
        assert_eq!(pp("`ifndef FOO\nyes\n`endif\n").trim(), "yes");
    }

    #[test]
    fn nested_conditionals() {
        // Inner directives in an inactive outer branch must not emit or define.
        let out = pp("`ifdef X\n`define Y 1\n`ifdef Y\ninner\n`endif\n`endif\nafter\n");
        assert_eq!(out.trim(), "after");
    }

    #[test]
    fn comments_and_strings_are_respected() {
        // A `define inside a comment is ignored; a backtick inside a string is left alone.
        let out = pp("// `define X 9\n`define X 1\nz = `X;\ns = \"no `X here\";\n");
        assert!(out.contains("z = 1 ;") || out.replace(' ', "").contains("z=1;"));
        assert!(out.contains("\"no `X here\""));
    }

    #[test]
    fn undefined_macro_is_an_error() {
        assert!(preprocess("y = `NOPE;\n", &[]).is_err());
    }

    #[test]
    fn unresolved_include_is_skipped() {
        // No include path → the include is dropped, the rest survives.
        let out = pp("`include \"nope.vams\"\nmodule m; endmodule\n");
        assert!(out.contains("module m"));
    }

    /// The truncated-vendor-distribution shape: the absent `` `include `` held the macro
    /// definitions the surviving text goes on to use, so the file drops an include **and then**
    /// fails to preprocess. Both facts have to come back, or a caller sees only the error and
    /// reads a truncation as a frontend gap — the exact misattribution the skipped list exists
    /// to prevent.
    #[test]
    fn a_failing_preprocess_still_reports_what_it_skipped() {
        let (result, skipped) = preprocess_reporting(
            "`include \"macrodefs_that_never_shipped.include\"
x = `GMIN;
",
            &[],
        );
        let err = result.expect_err("`GMIN is undefined once the include is dropped");
        assert!(err.to_string().contains("GMIN"), "{err}");
        assert_eq!(
            skipped,
            vec!["macrodefs_that_never_shipped.include".to_string()]
        );
    }

    /// Negative control: a failure with nothing skipped reports an empty list, so a caller
    /// cannot mistake "we know of no truncation" for "we did not look".
    #[test]
    fn a_failing_preprocess_with_no_include_reports_nothing_skipped() {
        let (result, skipped) = preprocess_reporting(
            "x = `NOPE;
",
            &[],
        );
        assert!(result.is_err());
        assert!(skipped.is_empty());
    }

    #[test]
    fn include_falls_back_to_basename_when_the_exact_path_is_missing() {
        // A corpus that flattens a vendor's own subdirectory layout (e.g.
        // `external/ekv3.va`'s own `` `include "ekv3_include/ekv3_definitions.va" ``, shipped
        // flat as `external/ekv3_definitions.va`) without rewriting its own `include`
        // directives still has the target file, just not at the literal path.
        let dir = std::env::temp_dir().join("va_frontend_test_include_basename_fallback");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        std::fs::write(dir.join("real_header.va"), "`define A 2\n").expect("write header");

        let src = "`include \"missing_subdir/real_header.va\"\nx = `A;\n";
        let out = preprocess(src, std::slice::from_ref(&dir)).expect("preprocess");
        assert_eq!(out.split_whitespace().collect::<String>(), "x=2;");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_exact_path_wins_over_basename_fallback() {
        // The basename fallback is only tried after every exact-path candidate fails — an
        // exact match must never be shadowed by a same-named file elsewhere on the search
        // path.
        let dir = std::env::temp_dir().join("va_frontend_test_include_exact_wins");
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("real_sub");
        std::fs::create_dir_all(&sub).expect("create test subdir");
        std::fs::write(dir.join("h.va"), "`define A 1\n").expect("write flat header");
        std::fs::write(sub.join("h.va"), "`define A 2\n").expect("write nested header");

        let src = "`include \"real_sub/h.va\"\nx = `A;\n";
        let out = preprocess(src, std::slice::from_ref(&dir)).expect("preprocess");
        assert_eq!(out.split_whitespace().collect::<String>(), "x=2;");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- compiler directives with text-stream scope (docs/proposals/directives.md) ------

    fn full(src: &str) -> Preprocessed {
        preprocess_full(src, &[]).0.expect("preprocesses")
    }

    #[test]
    fn default_discipline_and_transition_are_recorded_where_they_apply() {
        let pre = full("`default_discipline electrical\nmodule a; endmodule\n`default_transition 1n\nmodule b; endmodule\n`default_discipline\nmodule c; endmodule\n");
        let a = pre.text.find("module a").unwrap();
        let b = pre.text.find("module b").unwrap();
        let c = pre.text.find("module c").unwrap();
        let sa = settings_at(&pre.directives, a);
        assert_eq!(sa.default_discipline.as_deref(), Some("electrical"));
        assert_eq!(sa.default_transition, None);
        let sb = settings_at(&pre.directives, b);
        assert_eq!(sb.default_discipline.as_deref(), Some("electrical"));
        assert_eq!(sb.default_transition, Some(1e-9));
        // The bare form switches the default discipline off; the transition time stays.
        let sc = settings_at(&pre.directives, c);
        assert_eq!(sc.default_discipline, None);
        assert_eq!(sc.default_transition, Some(1e-9));
        assert!(pre.warnings.is_empty());
    }

    #[test]
    fn keyword_regions_nest_and_resetall_clears_everything() {
        let pre = full("`default_transition 2u\n`begin_keywords \"1364-2005\"\nA\n`begin_keywords \"1364-1995\"\nB\n`end_keywords\nC\n`end_keywords\nD\n`resetall\nE\n");
        let at = |m: &str| settings_at(&pre.directives, pre.text.find(m).unwrap());
        assert_eq!(at("A").keywords, KeywordSet::Ieee2005);
        assert_eq!(at("B").keywords, KeywordSet::Ieee1995);
        assert_eq!(
            at("C").keywords,
            KeywordSet::Ieee2005,
            "end_keywords restores the outer set"
        );
        assert_eq!(at("D").keywords, KeywordSet::Vams23);
        assert_eq!(at("D").default_transition, Some(2e-6));
        let e = at("E");
        assert_eq!(
            e,
            Settings::default(),
            "resetall: every directive back to its default"
        );
    }

    #[test]
    fn default_transition_takes_a_scaled_literal_and_rejects_the_rest() {
        let t = settings_at(
            &full(
                "`define TT 100n
`default_transition `TT
",
            )
            .directives,
            usize::MAX,
        )
        .default_transition
        .expect("a macro expanding to a literal is fine");
        assert!((t - 1e-7).abs() < 1e-20, "100n = {t}");
        let err = preprocess_full("`default_transition -1n\n", &[])
            .0
            .unwrap_err();
        assert!(err.to_string().contains("non-negative"), "{err}");
        let err = preprocess_full("`default_transition 1n + 2n\n", &[])
            .0
            .unwrap_err();
        assert!(
            err.to_string().contains("expression here is not supported"),
            "{err}"
        );
    }

    #[test]
    fn default_discipline_with_a_digital_qualifier_is_an_error_naming_annex_c() {
        let err = preprocess_full("`default_discipline logic wire\n", &[])
            .0
            .unwrap_err();
        assert!(err.to_string().contains("Annex C"), "{err}");
    }

    #[test]
    fn no_effect_directives_are_reported_once_each_and_the_text_is_untouched() {
        let pre = full("`timescale 1ns/1ps\n`celldefine\nmodule m; endmodule\n`endcelldefine\n`timescale 1ns/1ps\n`default_nettype none\n`unconnected_drive pull1\n`nounconnected_drive\n`pragma some_tool whatever\n");
        assert_eq!(pre.text.trim(), "module m; endmodule");
        let names: Vec<&str> = pre
            .warnings
            .iter()
            .map(|w| w.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "`timescale",
                "`celldefine",
                "`default_nettype",
                "`unconnected_drive"
            ],
            "one warning per directive, in first-seen order; an unknown pragma gets none: {:?}",
            pre.warnings
        );
        assert!(
            pre.warnings[0].contains("is in seconds"),
            "{}",
            pre.warnings[0]
        );
        assert!(pre.directives.is_empty());
    }

    #[test]
    fn pragma_protect_is_a_refusal_and_default_nodetype_and_unmatched_end_keywords_are_errors() {
        match preprocess_full(
            "`pragma protect begin_protected\nXYZ\n`pragma protect end_protected\n",
            &[],
        )
        .0
        {
            Err(FrontendError::Refused(r)) => {
                assert!(r.what.contains("pragma protect"), "{}", r.what);
                assert!(r.why.contains("ciphertext"), "{}", r.why);
            }
            other => panic!("expected a Refusal, got {other:?}"),
        }
        let err = preprocess_full("`default_nodetype electrical\n", &[])
            .0
            .unwrap_err();
        assert!(err.to_string().contains("`default_discipline"), "{err}");
        let err = preprocess_full("`end_keywords\n", &[]).0.unwrap_err();
        assert!(
            err.to_string().contains("matching `begin_keywords"),
            "{err}"
        );
        let err = preprocess_full("`begin_keywords \"1364-2005\"\nmodule m; endmodule\n", &[])
            .0
            .unwrap_err();
        assert!(
            err.to_string().contains("unterminated `begin_keywords"),
            "{err}"
        );
        let err = preprocess_full("`begin_keywords \"SV-2012\"\n`end_keywords\n", &[])
            .0
            .unwrap_err();
        assert!(
            err.to_string().contains("VAMS-2.3"),
            "names the legal specifiers: {err}"
        );
    }
}
