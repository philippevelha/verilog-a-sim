//! Surface AST: the parser's output, before elaboration into [`va_ir`].
//!
//! The AST is a faithful, sugar-preserving tree of one `module`. Elaboration ([`crate::elaborate`])
//! resolves names, assigns arena indices, and lowers it into the frozen IR.

/// A parsed Verilog-A module (surface syntax).
#[derive(Clone, Debug, Default)]
pub struct ModuleAst {
    /// Module name.
    pub name: String,
    /// Port names in declaration order.
    pub ports: Vec<String>,
    /// Raw analog-block statements, as source-order items (stubbed representation).
    pub items: Vec<Item>,
}

/// A top-level item inside a module (declaration or analog statement). Stub variant set.
#[derive(Clone, Debug)]
pub enum Item {
    /// A `parameter` declaration with a literal default.
    Param { name: String, default: f64 },
    /// A raw, not-yet-lowered analog statement captured as text (placeholder for T1).
    AnalogStmt(String),
}
