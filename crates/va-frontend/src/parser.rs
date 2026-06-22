//! Parser: token stream → surface [`crate::ast::ModuleAst`].

use crate::ast::ModuleAst;
use crate::lexer::Token;
use crate::FrontendError;

/// Parse a token stream into a single module AST.
///
/// # Errors
///
/// Returns [`FrontendError::Parse`] on unexpected tokens.
pub fn parse(_tokens: &[Token]) -> Result<ModuleAst, FrontendError> {
    todo!("T1: implement the recursive-descent Verilog-A parser")
}
