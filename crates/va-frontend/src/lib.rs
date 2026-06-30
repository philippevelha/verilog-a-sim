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

/// Compile Verilog-A `source` into an elaborated [`va_ir::Module`], with no `` `include ``
/// search path (unresolved includes are skipped — the standard disciplines are built in).
///
/// The crate's front door: preprocess → lex → parse → elaborate.
pub fn compile(source: &str) -> Result<va_ir::Module, FrontendError> {
    compile_with_includes(source, &[])
}

/// Like [`compile`], but resolving `` `include `` against `include_dirs` (searched in order),
/// so standard headers (`disciplines.vams`, `constants.vams`) and their macros expand.
pub fn compile_with_includes(
    source: &str,
    include_dirs: &[PathBuf],
) -> Result<va_ir::Module, FrontendError> {
    let expanded = preprocess::preprocess(source, include_dirs)?;
    let tokens = lexer::lex(&expanded)?;
    let ast = parser::parse(&tokens)?;
    elaborate::elaborate(&ast)
}

#[cfg(test)]
mod tests {
    #[test]
    fn elaborates_resistor_model() {
        let src = include_str!("../../../models/resistor.va");
        let module = super::compile(src).expect("resistor.va should elaborate");
        assert_eq!(module.name, "resistor");
    }
}
