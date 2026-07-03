//! Parsed `discipline...enddiscipline`/`nature...endnature` declarations (LRM §4,
//! "Disciplines and natures").
//!
//! These declarations are file-scoped (they precede, and may be interleaved between, the
//! modules a compilation unit defines) rather than attached to any single [`crate::ast::ModuleAst`],
//! so they get their own small module rather than living in `ast.rs`. Values here are plain
//! fields, never [`crate::ast::ExprRef`] — `crate::parser::Parser`'s expression arena is
//! drained per module (`parse_module`), so an `ExprRef` recorded here would dangle across a
//! module boundary.
//!
//! # Limitations
//!
//! `units`/`abstol`/`idt_nature`/`ddt_nature` are parsed but currently unused metadata — no
//! `va-core` convergence or unit-checking code consults them yet (like [`crate::ast::Range`]'s
//! inclusive/exclusive flags, which are parsed but dropped by elaboration too). Only a
//! [`NatureDecl`]'s `access` name, once bound as a discipline's `potential`/`flow` nature, has
//! a real effect today: it widens `crate::parser::Parser`'s recognized access-function name
//! set beyond the hardcoded `V`/`I`/`Temp`/`Pwr` baseline (§ module preamble discipline/nature
//! parsing).

/// A parsed `nature ... endnature` block.
#[derive(Clone, Debug, Default)]
pub struct NatureDecl {
    /// The nature's name.
    pub name: String,
    /// The `units = "...";` attribute, if present.
    pub units: Option<String>,
    /// The `access = Name;` attribute, if present — the access-function name this nature
    /// exposes (e.g. `"I"`, `"Q"`, `"MMF"`).
    pub access: Option<String>,
    /// The `abstol = value;` attribute, if present *and* a plain (optionally negated) numeric
    /// literal. `None` both when the attribute is absent and when its value isn't a literal —
    /// this is parsed-but-unused metadata, so a non-literal expression is silently dropped
    /// rather than rejected.
    pub abstol: Option<f64>,
    /// The `idt_nature = Other;` attribute, if present — names another [`NatureDecl`].
    pub idt_nature: Option<String>,
    /// The `ddt_nature = Other;` attribute, if present — names another [`NatureDecl`].
    pub ddt_nature: Option<String>,
}

/// A discipline's `domain discrete;`/`domain continuous;` attribute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DomainKind {
    /// `domain discrete;`.
    Discrete,
    /// `domain continuous;` (the LRM default when the attribute is omitted).
    Continuous,
}

/// A parsed `discipline ... enddiscipline` block.
#[derive(Clone, Debug, Default)]
pub struct DisciplineDecl {
    /// The discipline's name.
    pub name: String,
    /// The `potential Nature;` attribute's nature *name*, if present — looked up in
    /// `crate::parser::Parser`'s nature table by name when needed, not resolved to an index
    /// here; this is small, unindexed lookup data, not IR-arena material.
    pub potential: Option<String>,
    /// The `flow Nature;` attribute's nature name, if present.
    pub flow: Option<String>,
    /// The `domain ...;` attribute, if present.
    pub domain: Option<DomainKind>,
}
