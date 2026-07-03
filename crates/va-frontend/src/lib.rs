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
}

/// The result of compiling a source file: one elaborated [`va_ir::Module`] per `module` the
/// file defines, each already flattened against every sibling module in the file as its
/// submodule library (§ module instantiation) — so any [`ast::Item::Instance`] anywhere in the
/// file resolves, regardless of which module ends up used as a device model.
pub struct CompiledDesign {
    /// One elaborated module per source `module`, in source order.
    pub modules: Vec<va_ir::Module>,
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
    let expanded = preprocess::preprocess(source, include_dirs)?;
    let tokens = lexer::lex(&expanded)?;
    let asts = parser::parse(&tokens)?;
    let mut modules = Vec::with_capacity(asts.len());
    for ast in &asts {
        modules.push(elaborate::elaborate_with_library(ast, &asts)?);
    }
    Ok(CompiledDesign { modules })
}

#[cfg(test)]
mod tests {
    #[test]
    fn elaborates_resistor_model() {
        let src = include_str!("../../../models/resistor.va");
        let design = super::compile(src).expect("resistor.va should elaborate");
        assert_eq!(design.modules.len(), 1);
        assert_eq!(design.modules[0].name, "resistor");
    }
}
