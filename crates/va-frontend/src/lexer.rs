//! Lexer: source text → token stream.

use crate::FrontendError;

/// A lexical token. Fleshed out during T1; `logos` is the intended backing derive.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// An identifier or keyword.
    Ident(String),
    /// A numeric literal.
    Number(f64),
    /// A punctuation/operator lexeme, stored verbatim (e.g. "<+", "(", ";").
    Punct(String),
}

/// Tokenize `source`.
///
/// # Errors
///
/// Returns [`FrontendError::Lex`] on an untokenizable character.
pub fn lex(_source: &str) -> Result<Vec<Token>, FrontendError> {
    todo!("T1: implement the Verilog-A lexer (logos-backed)")
}
