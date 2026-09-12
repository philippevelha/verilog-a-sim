//! T1 — the Verilog-A frontend: lexer → parser → AST → elaboration into [`va_ir::Module`].
//!
//! This crate owns the path from source text to the frozen Interface α. Everything here is
//! a compiling stub today; the milestone is to elaborate the resistor/capacitor/diode `.va`
//! models into a [`va_ir::Module`] that `va-codegen` can differentiate.

#![forbid(unsafe_code)]
// The lexer declares ~170 `#[token]` keyword attributes on one variant; the `logos` derive
// expands them recursively, overflowing the default 128 limit.
#![recursion_limit = "512"]

pub mod ast;
pub mod disciplines;
pub mod elaborate;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod preprocess;

use std::path::PathBuf;
use thiserror::Error;

/// Errors produced anywhere in the frontend pipeline.
#[derive(Debug, Error)]
pub enum FrontendError {
    /// The preprocessor hit a bad directive, undefined macro, or include problem.
    #[error("preprocess error: {0}")]
    Preprocess(String),
    /// The lexer hit a character it cannot tokenize.
    #[error("lex error at byte {offset}: {message}")]
    Lex { offset: usize, message: String },
    /// The parser hit an unexpected token.
    #[error("parse error: {0}")]
    Parse(String),
    /// Elaboration could not lower the AST into the IR (e.g. unknown identifier).
    #[error("elaboration error: {0}")]
    Elaborate(String),
    /// A construct this implementation recognises and **deliberately declines** to support.
    ///
    /// Kept distinct from [`FrontendError::Parse`] and [`FrontendError::Elaborate`] because it
    /// means something different to whoever is reading it, and that difference is the first
    /// thing a user debugging a failed run needs. `Parse` says *your source is malformed*. A
    /// refusal says the opposite: the source is valid Verilog-A, and this simulator will not
    /// pretend to run it, because the alternatives — silently approximating the construct, or
    /// discarding it and running the body anyway — produce a **wrong answer** rather than a
    /// missing feature. Carrying that in the type rather than in prose is also what lets
    /// `va-cli` log every refusal in one marked, uniform shape (§ `va_cli::report_refusal`).
    #[error("{0}")]
    Refused(Refusal),
}

/// A recognised construct this implementation declines to support, and the reasoning a user
/// needs in order to do something about it.
///
/// Four fields, because four questions come up every time and answering fewer of them sends the
/// reader to the source: *what* was refused, *where* it was written, *why* it is refused rather
/// than approximated, and what to write *instead*. `tracking` points at the document that says
/// when the limitation might lift.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refusal {
    /// The construct, quoted as the user wrote it where possible — e.g. ``@(final_step("tran"))``.
    pub what: String,
    /// Why it is refused rather than approximated or ignored. This is the field that stops a
    /// refusal reading as an arbitrary gap: it should say what the wrong answer would have been.
    pub why: String,
    /// What to write instead, when there is a supported spelling. `None` when there is not.
    pub instead: Option<String>,
    /// Where it was written, as the parser's human-readable location. `None` when the refusing
    /// pass has no position to report.
    pub at: Option<String>,
    /// Where the limitation is tracked, so a reader can find out whether it is temporary.
    pub tracking: Option<String>,
}

impl Refusal {
    /// A refusal of `what`, because `why`. Location and advice are added with the builders.
    pub fn new(what: impl Into<String>, why: impl Into<String>) -> Self {
        Refusal {
            what: what.into(),
            why: why.into(),
            instead: None,
            at: None,
            tracking: None,
        }
    }

    /// This refusal, naming a supported spelling to use instead.
    pub fn instead(mut self, instead: impl Into<String>) -> Self {
        self.instead = Some(instead.into());
        self
    }

    /// This refusal, carrying the source location it was raised at.
    pub fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }

    /// This refusal, pointing at the document that tracks the limitation.
    pub fn tracking(mut self, tracking: impl Into<String>) -> Self {
        self.tracking = Some(tracking.into());
        self
    }
}

/// Width of the `  why:      ` style label column, so a field's continuation lines hang under
/// its text rather than under its label.
const REFUSAL_INDENT: &str = "            ";

impl std::fmt::Display for Refusal {
    /// One labelled field per line, opening with a stable `refused:` marker.
    ///
    /// The marker is deliberately greppable: a run over a model library produces one of these
    /// per offending file, and "which constructs did this simulator decline, and why" should be
    /// answerable with `grep refused:` over a captured log rather than by reading it.
    ///
    /// A field may itself be several lines — `what` is a list when one run refuses several
    /// constructs at once — so continuation lines are indented to hang under the field's text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hang = |v: &str| v.replace('\n', &format!("\n{REFUSAL_INDENT}"));
        write!(f, "refused: {}", hang(&self.what))?;
        if let Some(at) = &self.at {
            write!(f, "\n  where:    {}", hang(at))?;
        }
        write!(f, "\n  why:      {}", hang(&self.why))?;
        if let Some(instead) = &self.instead {
            write!(f, "\n  instead:  {}", hang(instead))?;
        }
        if let Some(tracking) = &self.tracking {
            write!(f, "\n  tracking: {}", hang(tracking))?;
        }
        Ok(())
    }
}

/// A refusal is an error in its own right, so a caller outside this crate — `va-cli`'s
/// transient-analysis gate, which refuses whole *models* rather than constructs — can raise one
/// directly and have it reported in the same shape as the frontend's.
impl std::error::Error for Refusal {}

/// The result of compiling a source file: one elaborated [`va_ir::Module`] per `module` the
/// file defines, each already flattened against every sibling module in the file as its
/// submodule library (§ module instantiation) — so any [`ast::Item::Instance`] anywhere in the
/// file resolves, regardless of which module ends up used as a device model.
pub struct CompiledDesign {
    /// One elaborated module per source `module`, in source order.
    pub modules: Vec<va_ir::Module>,
    /// Compiler directives the LRM gives no Verilog-A meaning, reported once each
    /// (`preprocess::Preprocessed::warnings`). Nothing in `modules` depends on them; a caller
    /// prints them so the user knows the directive was seen and why it changed nothing.
    pub warnings: Vec<String>,
}

/// Compile Verilog-A `source` into a [`CompiledDesign`], with no `` `include `` search path
/// (unresolved includes are skipped — the standard disciplines are built in).
///
/// The crate's front door: preprocess → lex → parse → elaborate.
pub fn compile(source: &str) -> Result<CompiledDesign, FrontendError> {
    compile_with_includes(source, &[])
}

/// Like [`compile`], but resolving `` `include `` against `include_dirs` (searched in order),
/// so standard headers (`disciplines.vams`, `constants.vams`) and their macros expand.
pub fn compile_with_includes(
    source: &str,
    include_dirs: &[PathBuf],
) -> Result<CompiledDesign, FrontendError> {
    let pre = preprocess::preprocess_full(source, include_dirs).0?;
    let expanded = &pre.text;
    // Lex with spans and hand them to the parser, so a parse error reports a line, a column,
    // and the offending line's text rather than a token index. The line number is a line of
    // `expanded`, not of `source` — see `parser::parse_with_disciplines_located`.
    // Both stages take the directive events: the lexer for `begin_keywords` regions, the
    // parser for each module's `default_discipline`/`default_transition` settings.
    let (tokens, offsets) = lexer::lex_spanned_with(expanded, &pre.directives)?;
    let (asts, natures, disciplines) =
        parser::parse_with_directives(&tokens, Some((expanded, &offsets)), &pre.directives)?;
    let mut modules = Vec::with_capacity(asts.len());
    for ast in &asts {
        modules.push(elaborate::elaborate_with_library_and_disciplines(
            ast,
            &asts,
            &disciplines,
            &natures,
        )?);
    }
    Ok(CompiledDesign {
        modules,
        warnings: pre.warnings,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn elaborates_resistor_model() {
        let src = include_str!("../../../models/resistor.va");
        // Through the include path, as the real pipeline does: the model `include`s
        // `constants.vams` for `` `P_K `` (its thermal-noise term, T5.2).
        let models = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))];
        let design =
            super::compile_with_includes(src, &models).expect("resistor.va should elaborate");
        assert_eq!(design.modules.len(), 1);
        assert_eq!(design.modules[0].name, "resistor");
    }

    /// Real corpus headers (`generalMacrosAndDefines.va`, `simulatorFlags.va`, ...) are meant
    /// only to be `` `include ``d by an actual device file, never compiled standalone, and
    /// contain nothing but `` `define ``s — no `module` at all. Compiling one of these directly
    /// is not an error: it's a degenerate but valid compilation unit with nothing to elaborate.
    #[test]
    fn macro_only_file_compiles_to_zero_modules() {
        let design =
            super::compile("`define GMIN 1.0e-12\n`define MAX(a, b) ((a) > (b) ? (a) : (b))")
                .expect("a macro-only file should compile, just to zero modules");
        assert!(design.modules.is_empty());
    }

    // ---- compiler directives, end to end (docs/proposals/directives.md) ----------------

    /// `` `default_discipline electrical `` (LRM 10.2): a module whose ports declare no
    /// discipline elaborates, with the ports as electrical nodes carrying the discipline's
    /// abstol; without the directive the same module is the "no discipline declaration" error.
    #[test]
    fn default_discipline_gives_undeclared_ports_a_discipline() {
        let body =
            "module r(p, n);\n  inout p, n;\n  analog I(p, n) <+ V(p, n) / 1000.0;\nendmodule\n";
        let models = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models"
        ))];
        let Err(err) = super::compile_with_includes(body, &models) else {
            panic!("undeclared ports must fail without the directive");
        };
        assert!(
            err.to_string().contains("no discipline declaration"),
            "{err}"
        );

        let src = format!("`include \"disciplines.vams\"\n`default_discipline electrical\n{body}");
        let design =
            super::compile_with_includes(&src, &models).expect("default discipline applies");
        let m = &design.modules[0];
        assert_eq!(m.ports.len(), 2);
        assert_eq!(m.nodes[0].discipline, va_ir::Discipline::Electrical);
        assert_eq!(
            m.nodes[0].abstol,
            Some(1e-6),
            "the discipline's own nature metadata comes along"
        );

        // The bare form turns it off again for what follows.
        let src = format!("`default_discipline electrical\n`default_discipline\n{body}");
        assert!(super::compile_with_includes(&src, &models).is_err());
    }

    /// `` `default_transition 100n `` (LRM 10.3 / 4.5.8): a `transition()` that omits its rise
    /// time gets the directive's value; one that states its own gets `rise > 0 ? rise :
    /// default`, because the LRM says a rise time "equal to zero" also means the default.
    #[test]
    fn default_transition_fills_an_omitted_or_zero_rise_time() {
        let src = "`default_transition 100n
module t(p, n);
  electrical p, n;
  analog V(p, n) <+ transition(1.0, 0) + transition(2.0, 0, 5n);
endmodule
";
        let design = super::compile(src).expect("compiles");
        let m = &design.modules[0];
        let rise_args: Vec<&va_ir::Expr> = m
            .exprs
            .iter()
            .filter_map(|e| match e {
                va_ir::Expr::Call(va_ir::Builtin::Transition, args) => {
                    Some(&m.exprs[args[2].0 as usize])
                }
                _ => None,
            })
            .collect();
        assert_eq!(rise_args.len(), 2);
        // Omitted: the directive's value, as a constant.
        assert!(
            matches!(rise_args[0], va_ir::Expr::Const(v) if (v - 1e-7).abs() < 1e-20),
            "{:?}",
            rise_args[0]
        );
        // Written: `written > 0 ? written : default`.
        match rise_args[1] {
            va_ir::Expr::Select(_, written, default) => {
                assert!(
                    matches!(m.exprs[written.0 as usize], va_ir::Expr::Const(v) if (v - 5e-9).abs() < 1e-22)
                );
                assert!(
                    matches!(m.exprs[default.0 as usize], va_ir::Expr::Const(v) if (v - 1e-7).abs() < 1e-20)
                );
            }
            other => panic!("expected a select, got {other:?}"),
        }
        // No directive: an omitted rise time is 0 here, which `va-codegen` turns into the
        // LRM's "negligible but non-zero" ramp at load, where the time scale is known.
        let design = super::compile(
            "module t(p, n); electrical p, n; analog V(p, n) <+ transition(1.0, 0); endmodule",
        )
        .unwrap();
        let m = &design.modules[0];
        assert!(m.exprs.iter().any(|e| matches!(e, va_ir::Expr::Call(va_ir::Builtin::Transition, args) if matches!(m.exprs[args[2].0 as usize], va_ir::Expr::Const(v) if v == 0.0))));
    }

    /// A legacy-Verilog structural module under `` `begin_keywords "1364-2005" `` can name a
    /// port `sin` (the LRM's own example) and wrap an analog module written outside the
    /// region — with `` `default_discipline `` supplying what `electrical`, itself unreserved
    /// there, cannot. The one shape a keyword region is useful for in a Verilog-A file.
    #[test]
    fn a_1364_keyword_region_wraps_an_analog_module() {
        let src = "`default_discipline electrical\n`begin_keywords \"1364-2005\"\nmodule top(sin, flow);\n  inout sin, flow;\n  leg l1(sin, flow);\nendmodule\n`end_keywords\nmodule leg(p, n);\n  electrical p, n;\n  analog I(p, n) <+ V(p, n) / 1000.0;\nendmodule\n";
        let design = super::compile(src).expect("compiles");
        let top = design
            .modules
            .iter()
            .find(|m| m.name == "top")
            .expect("top");
        assert_eq!(top.ports.len(), 2);
        assert_eq!(top.nodes[0].name, "sin");
        // Without the region `sin` is a reserved word and the port list does not parse.
        let plain = src
            .replace("`begin_keywords \"1364-2005\"\n", "")
            .replace("`end_keywords\n", "");
        assert!(super::compile(&plain).is_err());
    }

    /// The no-effect directives reach the caller as warnings, not as silence.
    #[test]
    fn no_effect_directives_surface_as_warnings() {
        let design = super::compile("`timescale 1ns/1ps\nmodule t(p, n); electrical p, n; analog I(p, n) <+ V(p, n); endmodule\n").unwrap();
        assert_eq!(design.warnings.len(), 1);
        assert!(design.warnings[0].starts_with("`timescale has no effect in Verilog-A"));
    }
}
