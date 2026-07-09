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
//! A [`NatureDecl`]'s `access` name, once bound as a discipline's `potential`/`flow` nature,
//! widens `crate::parser::Parser`'s recognized access-function name set beyond the hardcoded
//! `V`/`I`/`Temp`/`Pwr` baseline (§ module preamble discipline/nature parsing). Its `abstol`
//! (§ nature-metadata wiring, added 2026-07-09) round-trips one hop further: [`resolve_abstol`]
//! resolves a net's discipline to its **potential** nature's `abstol`, and
//! `crate::elaborate::Elaborator` records the result on the matching `va_ir::NodeDecl::abstol`,
//! which `va-core::mna::classify_abstol` reads to give that node its own Newton convergence
//! tolerance instead of the solver's single global default (`va-core::newton::NewtonConfig::
//! abstol`). `units`/`idt_nature`/`ddt_nature` remain parsed-but-unused metadata — like
//! [`crate::ast::Range`]'s inclusive/exclusive flags, which are parsed but dropped by
//! elaboration too. There is no equivalent wiring for the *flow* nature's `abstol` (e.g.
//! `Current`'s `1e-12`): only a `Node`-kind unknown (a KCL potential) has a natural
//! `NodeDecl`-shaped home for one — a branch-current unknown stays on the global default.

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

/// Resolve a net's discipline (by name — e.g. `"electrical"`, `"thermal"`, or a user-defined
/// discipline name) to its **potential** nature's `abstol` (§ nature-metadata wiring), for
/// `crate::elaborate::Elaborator` to record on the matching `va_ir::NodeDecl`.
///
/// Never errors — every link in the chain is optional (a discipline block, its `potential`
/// attribute, that nature's own `abstol`), so a missing one just means "no metadata to give
/// this node," not a malformed program. `None` in any of these cases:
/// - `name` was never declared as a `discipline...enddiscipline` block (the common case for
///   `models/` files with no `` `include "disciplines.vams" `` on their include path, or a
///   discipline this project doesn't model beyond `electrical`/`thermal` conservation);
/// - the discipline has no `potential Nature;` attribute (a signal-flow-only discipline, e.g.
///   `disciplines.vams`'s own `discipline current; flow Current; enddiscipline`);
/// - the named potential nature was never itself declared as a `nature...endnature` block;
/// - that nature's own `abstol` is absent or wasn't a plain numeric literal
///   ([`NatureDecl::abstol`]'s own documented limitation).
pub fn resolve_abstol(
    name: &str,
    disciplines: &std::collections::HashMap<String, DisciplineDecl>,
    natures: &std::collections::HashMap<String, NatureDecl>,
) -> Option<f64> {
    let nature_name = disciplines.get(name)?.potential.as_deref()?;
    natures.get(nature_name)?.abstol
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn electrical_fixture() -> (HashMap<String, DisciplineDecl>, HashMap<String, NatureDecl>) {
        let mut disciplines = HashMap::new();
        disciplines.insert(
            "electrical".to_string(),
            DisciplineDecl {
                name: "electrical".to_string(),
                potential: Some("Voltage".to_string()),
                flow: Some("Current".to_string()),
                domain: None,
            },
        );
        let mut natures = HashMap::new();
        natures.insert(
            "Voltage".to_string(),
            NatureDecl {
                name: "Voltage".to_string(),
                abstol: Some(1e-6),
                ..Default::default()
            },
        );
        (disciplines, natures)
    }

    #[test]
    fn resolve_abstol_follows_discipline_to_its_potential_nature() {
        let (disciplines, natures) = electrical_fixture();
        assert_eq!(
            resolve_abstol("electrical", &disciplines, &natures),
            Some(1e-6)
        );
    }

    #[test]
    fn resolve_abstol_is_none_for_an_undeclared_discipline() {
        let (disciplines, natures) = electrical_fixture();
        assert_eq!(resolve_abstol("optical", &disciplines, &natures), None);
    }

    #[test]
    fn resolve_abstol_is_none_when_the_discipline_has_no_potential_nature() {
        let mut disciplines = HashMap::new();
        disciplines.insert(
            "current".to_string(),
            DisciplineDecl {
                name: "current".to_string(),
                potential: None,
                flow: Some("Current".to_string()),
                domain: None,
            },
        );
        let natures = HashMap::new();
        assert_eq!(resolve_abstol("current", &disciplines, &natures), None);
    }

    #[test]
    fn resolve_abstol_is_none_when_the_potential_nature_was_never_declared() {
        let mut disciplines = HashMap::new();
        disciplines.insert(
            "electrical".to_string(),
            DisciplineDecl {
                name: "electrical".to_string(),
                potential: Some("Voltage".to_string()),
                flow: None,
                domain: None,
            },
        );
        let natures = HashMap::new();
        assert_eq!(resolve_abstol("electrical", &disciplines, &natures), None);
    }

    #[test]
    fn resolve_abstol_is_none_when_the_nature_has_no_abstol() {
        let (disciplines, mut natures) = electrical_fixture();
        natures.insert(
            "Voltage".to_string(),
            NatureDecl {
                name: "Voltage".to_string(),
                abstol: None,
                ..Default::default()
            },
        );
        assert_eq!(resolve_abstol("electrical", &disciplines, &natures), None);
    }
}
