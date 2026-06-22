//! Elaboration: surface [`crate::ast::ModuleAst`] → frozen [`va_ir::Module`] (Interface α).
//!
//! Resolves node/branch/parameter/variable names to arena indices and lowers expressions and
//! statements into the IR. This is the only place the frontend touches `va-ir`.

use crate::ast::ModuleAst;
use crate::FrontendError;

/// Elaborate a parsed module into the IR.
///
/// # Errors
///
/// Returns [`FrontendError::Elaborate`] on unresolved names or unsupported constructs.
pub fn elaborate(_ast: &ModuleAst) -> Result<va_ir::Module, FrontendError> {
    todo!("T1: resolve names and lower the AST into va_ir::Module")
}
