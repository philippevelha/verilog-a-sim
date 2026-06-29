//! T1 — the Verilog-A frontend: lexer → parser → AST → elaboration into [`va_ir::Module`].
//!
//! This crate owns the path from source text to the frozen Interface α. Everything here is
//! a compiling stub today; the milestone is to elaborate the resistor/capacitor/diode `.va`
//! models into a [`va_ir::Module`] that `va-codegen` can differentiate.

#![forbid(unsafe_code)]

pub mod ast;
pub mod elaborate;
pub mod lexer;
pub mod parser;

use thiserror::Error;

/// Errors produced anywhere in the frontend pipeline.
#[derive(Debug, Error)]
pub enum FrontendError {
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

/// Compile Verilog-A `source` into an elaborated [`va_ir::Module`].
///
/// This is the crate's front door: lex → parse → elaborate. Stubbed until T1 lands.
pub fn compile(source: &str) -> Result<va_ir::Module, FrontendError> {
    let tokens = lexer::lex(source)?;
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
