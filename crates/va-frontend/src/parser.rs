//! Parser: token stream → surface [`crate::ast::ModuleAst`].
//!
//! A hand-written recursive-descent parser for the §1 subset, with a precedence-climbing
//! expression parser. [`parse`] parses every `module...endmodule` in the token stream (a
//! source file may define several modules — e.g. a subcircuit plus a top module — that
//! reference each other via [`Item::Instance`], § module instantiation). Compiler directives
//! (`` `include ``) are skipped — v0 has no preprocessor.
//!
//! Beyond `if`/`else`, the parser accepts the analog control-flow statements `while`, `for`,
//! `repeat`, and `case`, and `analog function` definitions. `genvar` declarations and vector
//! (bus) net declarations (`electrical [msb:lsb] name;`) are accepted too; a branch access may
//! index into a vector net (`V(bus[i])`). None of this carries generate/vector-specific syntax
//! of its own beyond what is described below — a `generate`/`endgenerate` bracket around a
//! `for` is accepted but parses exactly like `begin`/`end` (no semantics attach to the
//! keywords themselves); whether a given `for`'s loop variable is a genvar (and so gets fully
//! unrolled at elaboration instead of becoming a runtime loop) is entirely an elaboration-time
//! decision. See [`crate::elaborate`].
//!
//! # Limitations
//!
//! - Errors are reported by token index, not source byte offset: the lexer discards spans
//!   in v0 (see [`crate::lexer`]). Carrying spans is a planned improvement.
//! - Parameter ranges accept mixed inclusive/exclusive delimiters — `[`/`]` (inclusive) and
//!   `(`/`)` (exclusive) in any combination, e.g. `from [0:inf)`. The inclusive/exclusive
//!   flags are recorded in the AST but dropped by elaboration (see [`crate::elaborate`]).
//! - `V`/`I` (electrical) and `Temp`/`Pwr` (thermal) are always recognized access-function
//!   names, regardless of whether any `discipline`/`nature` block was parsed. Beyond that
//!   baseline, an access name is recognized once a parsed `discipline` block binds it as a
//!   `potential`/`flow` nature (§ module preamble discipline/nature parsing,
//!   `Parser::known_access`) — additively, never removing the baseline. Net *declarations*
//!   still only accept the `electrical`/`thermal` keywords (a stated v1 limitation; see
//!   `docs/roadmap.md`).
//! - Analog functions retain argument directions and body only; argument/local *types*
//!   (`real x;`) are parsed and discarded.
//! - Event control `@(event) stmt` is parsed but the trigger is discarded — the controlled
//!   statement runs unconditionally. This matches DC operating-point semantics (`initial_step`
//!   setup runs once regardless); proper event scheduling is a transient-analysis concern.
//! - Vector nets and array variables carry at most 2 declared dimensions: 1-D is the standard
//!   form; a 2-D array variable (`real tile[0:R][0:C];`) is standard LRM grammar too, but a 2-D
//!   vector net (`electrical [0:R][0:C] grid;`) is a deliberate, documented **non-standard**
//!   extension — the LRM's `net_declaration` grammar never carries more than one range (see
//!   [`NetDecl`]). A 2-D vector net may never be used as a port, sliced, or connected/
//!   accessed bare or partially indexed — only a fully 2-indexed element resolves (checked at
//!   elaboration). `generate`/`genvar` are elaboration-only, module-scoped, and
//!   analog-block-only — there is no structural (module-item-level) generate (so no
//!   genvar-driven *array* of instances — a plain [`Item::Instance`] is always exactly one
//!   instance).
//! - A module instantiation's port connections may be all-positional or all-named
//!   (`.port(net)`), never mixed — the parser accepts either shape uniformly and leaves
//!   rejecting a mix, and any other per-instance validation (port count, vector ports, unknown
//!   parameter overrides), to elaboration (see [`crate::elaborate`]).

use std::collections::HashMap;

use crate::ast::{
    Access, AccessKind, AnalogFunction, BinOp, CaseArm, Direction, Discipline, ExprAst, ExprRef,
    FuncArg, Item, ModuleAst, NetArg, NetDecl, ParamType, PortConn, Range, Stmt, UnOp, VarEntry,
};
use crate::disciplines::{DisciplineDecl, DomainKind, NatureDecl};
use crate::lexer::Token;
use crate::FrontendError;

/// Parse a token stream into every module it defines, in source order. A stream that defines
/// **no** module at all is not an error — it's a valid, if degenerate, compilation unit: real
/// corpus files routinely carry nothing but `` `define ``s (e.g. `generalMacrosAndDefines.va`,
/// meant only to be `` `include ``d by a real device file, never compiled standalone), and the
/// LRM never requires a source file to contain a module. `Self::parse` returns `Ok(vec![])`
/// for one; the caller ends up with a [`crate::CompiledDesign`] with an empty `modules`, which
/// is simply nothing to elaborate or build instances from.
///
/// # Errors
///
/// Returns [`FrontendError::Parse`] on unexpected or missing tokens.
pub fn parse(tokens: &[Token]) -> Result<Vec<ModuleAst>, FrontendError> {
    parse_with_disciplines(tokens).map(|(modules, _, _)| modules)
}

/// [`parse_with_disciplines`]'s return: every parsed module, plus the file-scoped
/// `nature...endnature`/`discipline...enddiscipline` tables (§ module preamble discipline/
/// nature parsing) keyed by name.
pub type ParsedUnit = (
    Vec<ModuleAst>,
    HashMap<String, NatureDecl>,
    HashMap<String, DisciplineDecl>,
);

/// Like [`parse`], but also returning the file-scoped `nature...endnature`/
/// `discipline...enddiscipline` tables the parser built along the way (§ module preamble
/// discipline/nature parsing) — dropped by [`parse`] itself, since most callers never need
/// them, but required by `crate::compile_with_includes` to thread a net's resolved `abstol`
/// (§ nature-metadata wiring) into `crate::elaborate::elaborate_with_library_and_disciplines`.
///
/// # Errors
///
/// As [`parse`].
pub fn parse_with_disciplines(tokens: &[Token]) -> Result<ParsedUnit, FrontendError> {
    // The always-on access-function baseline (§ module preamble discipline/nature parsing):
    // recognized regardless of whether any `discipline`/`nature` block is ever parsed, so a
    // file with no preamble at all still recognizes the standard electrical/thermal names.
    let mut known_access = HashMap::new();
    known_access.insert("V".to_string(), AccessKind::Potential);
    known_access.insert("Temp".to_string(), AccessKind::Potential);
    known_access.insert("I".to_string(), AccessKind::Flow);
    known_access.insert("Pwr".to_string(), AccessKind::Flow);

    let mut p = Parser {
        toks: tokens,
        pos: 0,
        exprs: Vec::new(),
        natures: HashMap::new(),
        disciplines: HashMap::new(),
        known_access,
    };
    let mut modules = Vec::new();
    loop {
        p.skip_directives();
        if p.peek().is_none() {
            break;
        }
        modules.push(p.parse_module()?);
    }
    Ok((modules, p.natures, p.disciplines))
}

/// The dedicated single-word tokens (`electrical`/`thermal`/`ground`) that exist purely to
/// support their own declaration grammar, but whose plain spelling a real corpus model
/// sometimes reuses as an ordinary variable/parameter name (`external/ekv3_variables.va`:
/// `real thermal;`, later read as a bare identifier throughout `ekv3_noise.va`'s analog code
/// — both files are `` `include ``d into the same compilation unit as `external/ekv3.va`).
/// Returns that token's own spelling, for use wherever the grammar expects a bare *name*
/// rather than the start of a declaration (`Parser::expect_ident`, `Parser::parse_primary`) —
/// mirrors the precedent `Parser::expect_discipline_or_nature_name` already established for
/// `electrical`/`thermal` as a `discipline`/`nature` block's own declared name. Deliberately
/// narrow: `Real`/`Integer`/`Parameter`/… stay fully reserved — no corpus need found for them,
/// and their central role in the grammar makes the collision risk much higher.
fn ident_like_keyword(t: &Token) -> Option<&'static str> {
    match t {
        Token::Electrical => Some("electrical"),
        Token::Thermal => Some("thermal"),
        Token::Ground => Some("ground"),
        _ => None,
    }
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    exprs: Vec<ExprAst>,
    /// Parsed `nature ... endnature` blocks, keyed by name (§ module preamble discipline/nature
    /// parsing).
    natures: HashMap<String, NatureDecl>,
    /// Parsed `discipline ... enddiscipline` blocks, keyed by name.
    disciplines: HashMap<String, DisciplineDecl>,
    /// Recognized access-function name → its potential/flow classification. Seeded with the
    /// always-on `V`/`I`/`Temp`/`Pwr` baseline in [`parse`], then extended additively as a
    /// discipline binds one of `self.natures`'s access names as its `potential`/`flow` nature
    /// (`Parser::register_access`). Consulted by `is_access`/`parse_access` — this is a rare
    /// case in this codebase where the parser itself needs a small symbol table: unlike every
    /// other name (params, vars, functions), access-name recognition must happen at *parse*
    /// time, since it decides which `Stmt`/`ExprAst` variant to build in the first place, not
    /// just how a name later resolves at elaboration.
    known_access: HashMap<String, AccessKind>,
}

impl Parser<'_> {
    // --- cursor helpers --------------------------------------------------------------

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn nth(&self, k: usize) -> Option<&Token> {
        self.toks.get(self.pos + k)
    }

    fn bump(&mut self) -> Option<&Token> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn at(&self, t: &Token) -> bool {
        self.peek() == Some(t)
    }

    fn eat(&mut self, t: &Token) -> Result<(), FrontendError> {
        if self.at(t) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(format!("expected {t:?}"))
        }
    }

    /// Whether the cursor is on the reserved word `name` (a [`Token::Keyword`]).
    fn at_keyword(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Token::Keyword(kw)) if kw.as_str() == name)
    }

    /// Consume the reserved word `name`, or error.
    fn eat_keyword(&mut self, name: &str) -> Result<(), FrontendError> {
        if self.at_keyword(name) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(format!("expected `{name}`"))
        }
    }

    /// Consume tokens through the `)` matching an already-consumed `(`, honouring nesting.
    /// Used to skip the contents of an `@(...)` event expression.
    fn skip_balanced_parens(&mut self) -> Result<(), FrontendError> {
        let mut depth = 1usize;
        loop {
            match self.bump() {
                Some(Token::LParen) => depth += 1,
                Some(Token::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
        self.err("unterminated `@(...)` event expression".to_string())
    }

    fn err<T>(&self, what: String) -> Result<T, FrontendError> {
        Err(FrontendError::Parse(format!(
            "at token {}: {}, found {:?}",
            self.pos,
            what,
            self.peek()
        )))
    }

    fn expect_ident(&mut self) -> Result<String, FrontendError> {
        match self.peek() {
            Some(Token::Ident(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            Some(t) if ident_like_keyword(t).is_some() => {
                let s = ident_like_keyword(t).unwrap().to_string();
                self.pos += 1;
                Ok(s)
            }
            Some(Token::Keyword(kw)) => self.err(format!(
                "expected an identifier, but `{}` is a reserved word",
                kw.as_str()
            )),
            _ => self.err("expected an identifier".to_string()),
        }
    }

    /// Parse a non-empty comma-separated identifier list.
    fn ident_list(&mut self) -> Result<Vec<String>, FrontendError> {
        let mut names = vec![self.expect_ident()?];
        while self.at(&Token::Comma) {
            self.pos += 1;
            names.push(self.expect_ident()?);
        }
        Ok(names)
    }

    /// Parse a net terminal: a plain name, `name[i]`, `name[i][j]` (§ 2-D vector net), a slice
    /// `name[msb:lsb]` (only meaningful in a [`PortConn`], see [`NetArg::slice`]; other callers
    /// reject one at elaboration), or an index followed by a trailing slice, `name[i][lo:hi]`
    /// (parses syntactically; semantic validity — e.g. that `name` even has a dimension left to
    /// slice — is checked at elaboration, matching how this parser already defers all
    /// index/slice semantics there). At most 2 bracket groups total, and a slice (if present)
    /// must be the *last* one — `name[lo:hi][i]` is a parse error. Each index/bound, when
    /// present, must be a genvar or compile-time-constant expression, except in an
    /// `Access`/`Stmt::Contribute` terminal an index may instead be a genuinely runtime
    /// expression — checked at elaboration, not here.
    fn parse_net_arg(&mut self) -> Result<NetArg, FrontendError> {
        let name = self.expect_ident()?;
        let mut index = Vec::new();
        let mut slice = None;
        while self.at(&Token::LBracket) {
            if slice.is_some() {
                return self.err(format!(
                    "`{name}[..]`: a `[lo:hi]` slice must be the final bracket group"
                ));
            }
            if index.len() >= 2 {
                return self.err(format!(
                    "`{name}` has more than 2 bracket groups; indexing/slicing is capped at 2 \
                     dimensions"
                ));
            }
            self.pos += 1;
            let first = self.parse_expr()?;
            if self.at(&Token::Colon) {
                self.pos += 1;
                let last = self.parse_expr()?;
                self.eat(&Token::RBracket)?;
                slice = Some((first, last));
            } else {
                self.eat(&Token::RBracket)?;
                index.push(first);
            }
        }
        Ok(NetArg { name, index, slice })
    }

    /// Parse zero or more consecutive `[expr]` index brackets — one element of a 1-D or § 2-D
    /// array variable / vector-net terminal, capped at 2 (not general N-D). Each expression must
    /// be a compile-time-constant or genvar expression, except in an `Access`/`Stmt::Contribute`
    /// context at most one of (up to 2) may instead be a genuinely runtime expression — checked
    /// at elaboration, not here.
    fn parse_index_list(&mut self) -> Result<Vec<ExprRef>, FrontendError> {
        let mut idxs = Vec::new();
        while self.at(&Token::LBracket) {
            if idxs.len() >= 2 {
                return self.err("indexing is capped at 2 dimensions".to_string());
            }
            self.pos += 1;
            idxs.push(self.parse_expr()?);
            self.eat(&Token::RBracket)?;
        }
        Ok(idxs)
    }

    /// Parse an optional `[msb:lsb]` bracket, e.g. a vector net/port's declared width. The
    /// single-dimension primitive [`Self::parse_dim_list`] repeats to build a 1- or 2-D
    /// declaration.
    fn parse_bracket_range(&mut self) -> Result<Option<(ExprRef, ExprRef)>, FrontendError> {
        if !self.at(&Token::LBracket) {
            return Ok(None);
        }
        self.pos += 1;
        let msb = self.parse_expr()?;
        self.eat(&Token::Colon)?;
        let lsb = self.parse_expr()?;
        self.eat(&Token::RBracket)?;
        Ok(Some((msb, lsb)))
    }

    /// Parse zero or more consecutive `[msb:lsb]` declared-dimension brackets, capped at 2. Used
    /// by both `real`/`integer` array-variable declarations (LRM-standard `variable_identifier`
    /// repeated-unpacked-dimension grammar, § 2-D array variable) and, as a deliberate,
    /// documented non-standard extension (§ 2-D vector net — the LRM's `net_declaration` grammar
    /// never carries more than one range), a vector net's declaration range(s).
    fn parse_dim_list(&mut self) -> Result<Vec<(ExprRef, ExprRef)>, FrontendError> {
        let mut ranges = Vec::new();
        while self.at(&Token::LBracket) {
            if ranges.len() >= 2 {
                return self.err("more than 2 declared dimensions; capped at 2".to_string());
            }
            ranges.push(
                self.parse_bracket_range()?
                    .expect("just checked LBracket is present"),
            );
        }
        Ok(ranges)
    }

    /// Parse one entry of a net-declaration list: a name, optionally followed by its own
    /// declared dimension range(s) (§ vector nets / § 2-D vector net); falls back to
    /// `default_ranges` (the declaration's shared prefix range(s), if any) when the name has no
    /// suffix of its own.
    fn parse_net_decl(
        &mut self,
        default_ranges: &[(ExprRef, ExprRef)],
    ) -> Result<NetDecl, FrontendError> {
        let name = self.expect_ident()?;
        let own = self.parse_dim_list()?;
        let ranges = if own.is_empty() {
            default_ranges.to_vec()
        } else {
            own
        };
        Ok(NetDecl { name, ranges })
    }

    /// Parse the name list of a net declaration under `discipline` (the discipline keyword —
    /// built-in or custom — is already consumed by the caller): optional shared declared
    /// dimension range(s), then a comma-separated [`Self::parse_net_decl`] list, then `;`.
    /// Shared by every discipline spelling so `electrical`/`thermal` (dedicated tokens) and a
    /// user-declared discipline name (a plain `Ident` looked up in `self.disciplines`) parse
    /// identically past the keyword.
    fn parse_net_item(&mut self, discipline: Discipline) -> Result<Item, FrontendError> {
        // Dimension range(s) before the name list are a *default*; each name may also carry its
        // own suffix range(s) (`bus[3:0]`), overriding the default for that name only — both
        // forms appear in real Verilog-A (e.g. `electrical [0:w-1] in;` vs. `electrical
        // in[`W-1:0], out;`). Either way, a vector name becomes a bus of nodes, indexed by a
        // genvar expression in a branch access. A second dimension (§ 2-D vector net) is a
        // non-standard extension — see `NetDecl`'s doc comment.
        let default_ranges = self.parse_dim_list()?;
        let mut nets = vec![self.parse_net_decl(&default_ranges)?];
        while self.at(&Token::Comma) {
            self.pos += 1;
            nets.push(self.parse_net_decl(&default_ranges)?);
        }
        self.eat(&Token::Semicolon)?;
        Ok(Item::Net { discipline, nets })
    }

    /// Parse a ground declaration, `ground gnd, vss;` (LRM §3.6.4, Syntax 3-7) — see
    /// `Item::Ground`'s doc comment for the supported subset.
    fn parse_ground_item(&mut self) -> Result<Item, FrontendError> {
        self.pos += 1; // consume `ground`
        let names = self.ident_list()?;
        self.eat(&Token::Semicolon)?;
        Ok(Item::Ground { names })
    }

    /// Parse one entry of a `real`/`integer` variable-declaration list: a name, followed by
    /// either its own declared dimension range(s) (§ array variables / § 2-D array variable) —
    /// e.g. `real out_val[0:15], tile[0:R][0:C], tmp;` — or an inline `= expr` initializer, e.g.
    /// `real laser_freq = `P_C / wavelength / 1e-9;`. The LRM allows one or the other per name,
    /// never both, so an initializer is only looked for when there were no dimensions. Unlike a
    /// net declaration, there is no shared prefix-range form here: a scalar/array `real`/
    /// `integer` never carries a width before the name list, only per-name array dimension(s)
    /// after it.
    fn parse_var_entry(&mut self) -> Result<VarEntry, FrontendError> {
        let name = self.expect_ident()?;
        let ranges = self.parse_dim_list()?;
        let init = if ranges.is_empty() && self.at(&Token::Assign) {
            self.pos += 1;
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(VarEntry { name, ranges, init })
    }

    /// A non-empty comma-separated list of [`Self::parse_var_entry`].
    fn parse_var_entry_list(&mut self) -> Result<Vec<VarEntry>, FrontendError> {
        let mut names = vec![self.parse_var_entry()?];
        while self.at(&Token::Comma) {
            self.pos += 1;
            names.push(self.parse_var_entry()?);
        }
        Ok(names)
    }

    fn push(&mut self, e: ExprAst) -> ExprRef {
        let r = ExprRef(self.exprs.len() as u32);
        self.exprs.push(e);
        r
    }

    /// Consume any run of compiler directives (`` `include "..." `` etc.).
    fn skip_directives(&mut self) {
        while let Some(Token::Directive(_)) = self.peek() {
            self.pos += 1;
            if let Some(Token::Str(_)) = self.peek() {
                self.pos += 1;
            }
        }
    }

    /// Parse directives and top-level `discipline ... enddiscipline` / `nature ...
    /// endnature` blocks (e.g. from an expanded `disciplines.vams`) that precede a module,
    /// registering each in [`Self::natures`]/[`Self::disciplines`] and widening
    /// [`Self::known_access`] as a discipline binds a nature's access name (§ module preamble
    /// discipline/nature parsing). Runs before *every* module in the token stream (called from
    /// [`Self::parse_module`]), so blocks interleaved between modules are reached too.
    fn parse_preamble(&mut self) -> Result<(), FrontendError> {
        loop {
            self.skip_directives();
            if self.at_keyword("discipline") {
                self.parse_discipline()?;
            } else if self.at_keyword("nature") {
                self.parse_nature()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Consume a token if present; a no-op otherwise. Used to tolerate the real-world grammar
    /// variant (seen in `external/ekv3_natures.va`) that omits the `;` after a
    /// `discipline`/`nature` block's name.
    fn eat_optional(&mut self, t: &Token) -> bool {
        if self.at(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Expect a string literal, e.g. a `nature` block's `units = "...";` value.
    fn expect_string(&mut self) -> Result<String, FrontendError> {
        match self.peek() {
            Some(Token::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            _ => self.err("expected a string literal".to_string()),
        }
    }

    /// Parse a `nature` attribute's value when it's expected to be a plain (optionally
    /// negated) numeric literal, e.g. `abstol = 1e-12;`. Returns `Some(value)` only when the
    /// value is *exactly* `[-]NUMBER` immediately followed by `;` — anything else (e.g.
    /// `abstol = 2*1e-6;`) falls back to [`Self::parse_expr`] (consuming the expression's
    /// tokens so the surrounding grammar still parses) and returns `None`. `abstol` is
    /// parsed-but-unused metadata (like `ast::Range`'s inclusive/exclusive flags), so a
    /// non-literal value is dropped rather than rejected.
    fn parse_literal_f64_opt(&mut self) -> Result<Option<f64>, FrontendError> {
        let negate = matches!(self.peek(), Some(Token::Minus));
        let num_at = if negate { 1 } else { 0 };
        if let Some(Token::Number(n)) = self.nth(num_at) {
            if self.nth(num_at + 1) == Some(&Token::Semicolon) {
                let n = *n;
                self.pos += num_at + 1;
                return Ok(Some(if negate { -n } else { n }));
            }
        }
        self.parse_expr()?;
        Ok(None)
    }

    /// Parse a `discipline`/`nature` block's own declared name. Almost always a generic
    /// identifier, but the standard `disciplines.vams` header itself names a discipline
    /// `electrical`/`thermal` — words this project already lexes as its own dedicated
    /// `Token::Electrical`/`Token::Thermal` net-declaration tokens, not `Token::Ident` — so
    /// this must accept those two spellings too.
    fn expect_discipline_or_nature_name(&mut self) -> Result<String, FrontendError> {
        match self.peek() {
            Some(Token::Electrical) => {
                self.pos += 1;
                Ok("electrical".to_string())
            }
            Some(Token::Thermal) => {
                self.pos += 1;
                Ok("thermal".to_string())
            }
            _ => self.expect_ident(),
        }
    }

    /// Parse one `nature ... endnature` block (LRM §4), registering it in [`Self::natures`].
    fn parse_nature(&mut self) -> Result<(), FrontendError> {
        self.eat_keyword("nature")?;
        let name = self.expect_discipline_or_nature_name()?;
        self.eat_optional(&Token::Semicolon);
        let mut decl = NatureDecl {
            name: name.clone(),
            ..Default::default()
        };
        loop {
            if self.at_keyword("endnature") {
                break;
            }
            if self.peek().is_none() {
                return self.err("unexpected end of input inside `nature...endnature`".to_string());
            }
            if self.at_keyword("units") {
                self.pos += 1;
                self.eat(&Token::Assign)?;
                decl.units = Some(self.expect_string()?);
                self.eat(&Token::Semicolon)?;
            } else if self.at_keyword("access") {
                self.pos += 1;
                self.eat(&Token::Assign)?;
                decl.access = Some(self.expect_ident()?);
                self.eat(&Token::Semicolon)?;
            } else if self.at_keyword("abstol") {
                self.pos += 1;
                self.eat(&Token::Assign)?;
                decl.abstol = self.parse_literal_f64_opt()?;
                self.eat(&Token::Semicolon)?;
            } else if self.at_keyword("idt_nature") {
                self.pos += 1;
                self.eat(&Token::Assign)?;
                decl.idt_nature = Some(self.expect_ident()?);
                self.eat(&Token::Semicolon)?;
            } else if self.at_keyword("ddt_nature") {
                self.pos += 1;
                self.eat(&Token::Assign)?;
                decl.ddt_nature = Some(self.expect_ident()?);
                self.eat(&Token::Semicolon)?;
            } else {
                return self.err(format!("unknown `nature` attribute {:?}", self.peek()));
            }
        }
        self.eat_keyword("endnature")?;
        self.natures.insert(name, decl);
        Ok(())
    }

    /// Parse one `discipline ... enddiscipline` block (LRM §4), registering it in
    /// [`Self::disciplines`] and widening [`Self::known_access`] via [`Self::register_access`].
    fn parse_discipline(&mut self) -> Result<(), FrontendError> {
        self.eat_keyword("discipline")?;
        let name = self.expect_discipline_or_nature_name()?;
        self.eat_optional(&Token::Semicolon);
        let mut decl = DisciplineDecl {
            name: name.clone(),
            ..Default::default()
        };
        loop {
            if self.at_keyword("enddiscipline") {
                break;
            }
            if self.peek().is_none() {
                return self.err(
                    "unexpected end of input inside `discipline...enddiscipline`".to_string(),
                );
            }
            if self.at_keyword("potential") {
                self.pos += 1;
                let nature = self.expect_ident()?;
                self.eat(&Token::Semicolon)?;
                self.register_access(&nature, AccessKind::Potential);
                decl.potential = Some(nature);
            } else if self.at_keyword("flow") {
                self.pos += 1;
                let nature = self.expect_ident()?;
                self.eat(&Token::Semicolon)?;
                self.register_access(&nature, AccessKind::Flow);
                decl.flow = Some(nature);
            } else if self.at_keyword("domain") {
                self.pos += 1;
                decl.domain = Some(if self.at_keyword("discrete") {
                    self.pos += 1;
                    DomainKind::Discrete
                } else {
                    self.eat_keyword("continuous")?;
                    DomainKind::Continuous
                });
                self.eat(&Token::Semicolon)?;
            } else {
                return self.err(format!("unknown `discipline` attribute {:?}", self.peek()));
            }
        }
        self.eat_keyword("enddiscipline")?;
        self.disciplines.insert(name, decl);
        Ok(())
    }

    /// Additively widen [`Self::known_access`]: if `nature_name` is an already-parsed nature
    /// with an `access` name, that name becomes recognized as `kind`. A discipline
    /// forward-referencing a not-yet-parsed nature (never seen in the real corpus) is a
    /// silent no-op here, not an error — best-effort, matching the overall additive framing.
    fn register_access(&mut self, nature_name: &str, kind: AccessKind) {
        if let Some(access_name) = self.natures.get(nature_name).and_then(|n| n.access.clone()) {
            self.known_access.insert(access_name, kind);
        }
    }

    // --- module --------------------------------------------------------------------

    fn parse_module(&mut self) -> Result<ModuleAst, FrontendError> {
        self.parse_preamble()?;
        self.eat(&Token::Module)?;
        let name = self.expect_ident()?;

        self.eat(&Token::LParen)?;
        let ports = if self.at(&Token::RParen) {
            Vec::new()
        } else {
            self.ident_list()?
        };
        self.eat(&Token::RParen)?;
        self.eat(&Token::Semicolon)?;

        let mut items = Vec::new();
        loop {
            self.skip_directives();
            if self.at(&Token::EndModule) {
                break;
            }
            if self.peek().is_none() {
                return self.err("unexpected end of input before `endmodule`".to_string());
            }
            items.push(self.parse_item()?);
        }
        self.eat(&Token::EndModule)?;

        Ok(ModuleAst {
            name,
            ports,
            items,
            exprs: std::mem::take(&mut self.exprs),
        })
    }

    fn parse_item(&mut self) -> Result<Item, FrontendError> {
        match self.peek() {
            Some(Token::Input) | Some(Token::Output) | Some(Token::Inout) => {
                let dir = match self.bump() {
                    Some(Token::Input) => Direction::Input,
                    Some(Token::Output) => Direction::Output,
                    _ => Direction::Inout,
                };
                // A vector port repeats its `[msb:lsb]` width here too (e.g. the LRM's own DAC
                // example: `input [0:width-1] in;` alongside `electrical [0:width-1] in;`) — the
                // real vector range comes from the paired discipline declaration (§2.2), so this
                // one is parsed and discarded; it's purely informational at the direction site.
                self.parse_bracket_range()?;
                let nets = self.ident_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Direction { dir, nets })
            }
            Some(Token::Electrical) | Some(Token::Thermal) => {
                let discipline = match self.bump() {
                    Some(Token::Electrical) => Discipline::Electrical,
                    _ => Discipline::Thermal,
                };
                self.parse_net_item(discipline)
            }
            Some(Token::Parameter) | Some(Token::LocalParam) => self.parse_param(),
            // `ground gnd;` (LRM §3.6.4) — see `Item::Ground`'s doc comment for the supported
            // subset of Syntax 3-7.
            Some(Token::Ground) => self.parse_ground_item(),
            // A bare `real`/`integer` at module scope declares variables (a `parameter`
            // declaration starts with `parameter`, and an `analog function` with `analog`).
            Some(Token::Real) | Some(Token::Integer) => {
                let ty = match self.bump() {
                    Some(Token::Integer) => ParamType::Integer,
                    _ => ParamType::Real,
                };
                let names = self.parse_var_entry_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Var { ty, names })
            }
            // `genvar i;` declares a generate-loop index (§ generate loops). Unlike `integer`,
            // a genvar is never a runtime variable: it is only ever bound to a constant while
            // elaboration unrolls the `for` loop it drives (see `crate::elaborate`).
            Some(Token::Genvar) => {
                self.pos += 1;
                let names = self.ident_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Genvar { names })
            }
            Some(Token::Keyword(kw)) if kw.as_str() == "branch" => self.parse_branch_decl(),
            Some(Token::Keyword(kw)) if kw.as_str() == "aliasparam" => self.parse_aliasparam_decl(),
            // `analog function …` is a function definition; a bare `analog` is the block.
            Some(Token::Analog) if matches!(self.nth(1), Some(Token::Keyword(kw)) if kw.as_str() == "function") => {
                self.parse_function()
            }
            Some(Token::Analog) => {
                self.pos += 1;
                let body = self.parse_block_or_single()?;
                Ok(Item::Analog(Stmt::Block(body)))
            }
            // A bare leading identifier is either a net declaration under a user-defined
            // discipline (`discipline optical; ... enddiscipline` earlier in the file, then
            // `optical a, b;` here — the built-in `electrical`/`thermal` spellings are their
            // own dedicated tokens above, but a custom discipline name is just an `Ident`) or a
            // module instantiation, `module_name inst_name(...);` / `module_name #(...)
            // inst_name(...);` (§ module instantiation). Every `discipline` block registers its
            // name in `self.disciplines` (see `parse_discipline`) before any item using it can
            // appear, so that lookup disambiguates the two without lookahead past the name.
            Some(Token::Ident(name)) if self.disciplines.contains_key(name) => {
                let discipline = Discipline::Custom(name.clone());
                self.pos += 1;
                self.parse_net_item(discipline)
            }
            Some(Token::Ident(name)) => {
                let module = name.clone();
                self.parse_instance(module)
            }
            _ => self.err("expected a declaration or `analog` block".to_string()),
        }
    }

    /// Parse a module instantiation, `module #(.p(expr), ...) name(conn, ...);`. `module` is
    /// the already-peeked (but not yet consumed) instantiated-module name.
    fn parse_instance(&mut self, module: String) -> Result<Item, FrontendError> {
        self.pos += 1; // consume the module-name identifier
        let params = self.parse_optional_param_overrides()?;
        let name = self.expect_ident()?;
        self.eat(&Token::LParen)?;
        let connections = self.parse_port_conn_list()?;
        self.eat(&Token::RParen)?;
        self.eat(&Token::Semicolon)?;
        Ok(Item::Instance {
            module,
            name,
            params,
            connections,
        })
    }

    /// Parse an optional `#(.name(expr), ...)` parameter-override list.
    fn parse_optional_param_overrides(&mut self) -> Result<Vec<(String, ExprRef)>, FrontendError> {
        if !self.at(&Token::Hash) {
            return Ok(Vec::new());
        }
        self.pos += 1;
        self.eat(&Token::LParen)?;
        let mut overrides = Vec::new();
        if !self.at(&Token::RParen) {
            overrides.push(self.parse_param_override()?);
            while self.at(&Token::Comma) {
                self.pos += 1;
                overrides.push(self.parse_param_override()?);
            }
        }
        self.eat(&Token::RParen)?;
        Ok(overrides)
    }

    /// Parse one `.name(expr)` entry of a parameter-override list.
    fn parse_param_override(&mut self) -> Result<(String, ExprRef), FrontendError> {
        self.eat(&Token::Dot)?;
        let name = self.expect_ident()?;
        self.eat(&Token::LParen)?;
        let value = self.parse_expr()?;
        self.eat(&Token::RParen)?;
        Ok((name, value))
    }

    /// Parse a (possibly empty) comma-separated list of instance port connections.
    fn parse_port_conn_list(&mut self) -> Result<Vec<PortConn>, FrontendError> {
        let mut conns = Vec::new();
        if self.at(&Token::RParen) {
            return Ok(conns);
        }
        conns.push(self.parse_port_conn()?);
        while self.at(&Token::Comma) {
            self.pos += 1;
            conns.push(self.parse_port_conn()?);
        }
        Ok(conns)
    }

    /// Parse one port connection: `.port(net)` (named) or a bare `net` (positional).
    fn parse_port_conn(&mut self) -> Result<PortConn, FrontendError> {
        if self.at(&Token::Dot) {
            self.pos += 1;
            let port = self.expect_ident()?;
            self.eat(&Token::LParen)?;
            let net = self.parse_net_arg()?;
            self.eat(&Token::RParen)?;
            Ok(PortConn::Named { port, net })
        } else {
            Ok(PortConn::Positional(self.parse_net_arg()?))
        }
    }

    /// Parse a `parameter` or `localparam` declaration. v0 does not model instance-parameter
    /// overrides at all (`va-netlist` has no by-name override path), so a `localparam`'s only
    /// observable difference from `parameter` — that it cannot be overridden — is moot here;
    /// both lower to the same [`Item::Param`].
    fn parse_param(&mut self) -> Result<Item, FrontendError> {
        match self.peek() {
            Some(Token::Parameter) | Some(Token::LocalParam) => self.pos += 1,
            _ => return self.err("expected `parameter` or `localparam`".to_string()),
        }
        let ty = match self.peek() {
            Some(Token::Real) => {
                self.pos += 1;
                ParamType::Real
            }
            Some(Token::Integer) => {
                self.pos += 1;
                ParamType::Integer
            }
            // Type omitted: Verilog-A defaults an untyped parameter to real.
            _ => ParamType::Real,
        };
        let name = self.expect_ident()?;
        self.eat(&Token::Assign)?;
        let default = self.parse_expr()?;

        // An optional `from` range, then zero or more `exclude` clauses. Only the `from`
        // range is retained (it sets min/max); exclusions are parsed and discarded in v0.
        let range = if self.at(&Token::From) {
            self.pos += 1;
            // A bound may open with `[` (inclusive) or `(` (exclusive) and close with `]`
            // (inclusive) or `)` (exclusive), independently — e.g. `from [0:inf)`.
            let lo_inclusive = self.open_bound()?;
            let lo = self.parse_expr()?;
            self.eat(&Token::Colon)?;
            let hi = self.parse_expr()?;
            let hi_inclusive = self.close_bound()?;
            Some(Range {
                lo,
                hi,
                lo_inclusive,
                hi_inclusive,
            })
        } else {
            None
        };
        while self.at(&Token::Exclude) {
            self.pos += 1;
            self.skip_exclude_clause()?;
        }
        self.eat(&Token::Semicolon)?;
        Ok(Item::Param {
            ty,
            name,
            default,
            range,
        })
    }

    /// Parse and discard one `exclude` clause: either a range (`exclude (a:b)` / `[a:b]`) or a
    /// single value (`exclude 0`). v0 does not enforce exclusions.
    fn skip_exclude_clause(&mut self) -> Result<(), FrontendError> {
        if self.at(&Token::LParen) || self.at(&Token::LBracket) {
            self.open_bound()?;
            self.parse_expr()?;
            self.eat(&Token::Colon)?;
            self.parse_expr()?;
            self.close_bound()?;
        } else {
            self.parse_expr()?;
        }
        Ok(())
    }

    /// Parse an analog function definition:
    /// `analog function [real|integer] name; <decls> <stmts> endfunction`.
    ///
    /// Argument *type* declarations (`real x;`) inside the body are consumed but not retained
    /// in v0; only argument directions and body statements are kept (see [`AnalogFunction`]).
    fn parse_function(&mut self) -> Result<Item, FrontendError> {
        self.eat(&Token::Analog)?;
        self.eat_keyword("function")?;
        let ret_ty = match self.peek() {
            Some(Token::Real) => {
                self.pos += 1;
                ParamType::Real
            }
            Some(Token::Integer) => {
                self.pos += 1;
                ParamType::Integer
            }
            // An untyped analog function returns real.
            _ => ParamType::Real,
        };
        let name = self.expect_ident()?;
        self.eat(&Token::Semicolon)?;

        let mut args = Vec::new();
        let mut body = Vec::new();
        loop {
            if self.at_keyword("endfunction") {
                break;
            }
            if self.peek().is_none() {
                return self.err("unexpected end of input before `endfunction`".to_string());
            }
            match self.peek() {
                Some(Token::Input) | Some(Token::Output) | Some(Token::Inout) => {
                    let dir = match self.bump() {
                        Some(Token::Input) => Direction::Input,
                        Some(Token::Output) => Direction::Output,
                        _ => Direction::Inout,
                    };
                    for name in self.ident_list()? {
                        args.push(FuncArg { dir, name });
                    }
                    self.eat(&Token::Semicolon)?;
                }
                // Argument/local type declarations: consumed, types not tracked in v0.
                Some(Token::Real) | Some(Token::Integer) => {
                    self.pos += 1;
                    self.ident_list()?;
                    self.eat(&Token::Semicolon)?;
                }
                _ => body.push(self.parse_stmt()?),
            }
        }
        self.eat_keyword("endfunction")?;
        Ok(Item::Function(AnalogFunction {
            name,
            ret_ty,
            args,
            body,
        }))
    }

    /// Parse a named branch declaration: `branch (a[, b]) name {, name};`.
    fn parse_branch_decl(&mut self) -> Result<Item, FrontendError> {
        self.eat_keyword("branch")?;
        self.eat(&Token::LParen)?;
        let mut terminals = vec![self.parse_net_arg()?];
        if self.at(&Token::Comma) {
            self.pos += 1;
            terminals.push(self.parse_net_arg()?);
        }
        self.eat(&Token::RParen)?;
        let names = self.ident_list()?;
        self.eat(&Token::Semicolon)?;
        Ok(Item::Branch { terminals, names })
    }

    /// Parse an `aliasparam` declaration: `aliasparam name = target;`. The grammar is a fixed
    /// `identifier = identifier`, not a general expression — `target` must name an
    /// already-declared parameter (checked at elaboration).
    fn parse_aliasparam_decl(&mut self) -> Result<Item, FrontendError> {
        self.eat_keyword("aliasparam")?;
        let name = self.expect_ident()?;
        self.eat(&Token::Assign)?;
        let target = self.expect_ident()?;
        self.eat(&Token::Semicolon)?;
        Ok(Item::AliasParam { name, target })
    }

    /// Consume a range's opening delimiter, returning whether it is inclusive (`[`) rather
    /// than exclusive (`(`).
    fn open_bound(&mut self) -> Result<bool, FrontendError> {
        match self.peek() {
            Some(Token::LBracket) => {
                self.pos += 1;
                Ok(true)
            }
            Some(Token::LParen) => {
                self.pos += 1;
                Ok(false)
            }
            _ => self.err("expected `[` or `(` to open a range bound".to_string()),
        }
    }

    /// Consume a range's closing delimiter, returning whether it is inclusive (`]`) rather
    /// than exclusive (`)`).
    fn close_bound(&mut self) -> Result<bool, FrontendError> {
        match self.peek() {
            Some(Token::RBracket) => {
                self.pos += 1;
                Ok(true)
            }
            Some(Token::RParen) => {
                self.pos += 1;
                Ok(false)
            }
            _ => self.err("expected `]` or `)` to close a range bound".to_string()),
        }
    }

    // --- statements ----------------------------------------------------------------

    /// Parse either a `begin … end` block (returning its statements) or a single statement
    /// (returning a one-element list). Normalises both `if` arms and the analog block.
    fn parse_block_or_single(&mut self) -> Result<Vec<Stmt>, FrontendError> {
        if self.at(&Token::Begin) {
            self.pos += 1;
            // An optional `: label` names the block (Verilog-A); the name is discarded.
            if self.at(&Token::Colon) {
                self.pos += 1;
                self.expect_ident()?;
            }
            let mut stmts = Vec::new();
            while !self.at(&Token::End) {
                if self.peek().is_none() {
                    return self.err("unexpected end of input before `end`".to_string());
                }
                stmts.push(self.parse_stmt()?);
            }
            self.eat(&Token::End)?;
            Ok(stmts)
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, FrontendError> {
        match self.peek() {
            // The empty statement, a bare `;` — legal wherever a statement is expected (LRM),
            // and a real idiom besides: `mvsg_cmc_3.2.0.va`'s
            // `if ($port_connected(dt) == 0);` uses one as an `if`'s entire body, deliberately
            // doing nothing on that branch (its optional thermal port is simply left
            // unconnected). Elaborated the same as an empty `begin end` block.
            Some(Token::Semicolon) => {
                self.pos += 1;
                Ok(Stmt::Block(Vec::new()))
            }
            Some(Token::Begin) => Ok(Stmt::Block(self.parse_block_or_single()?)),
            // Event control `@(event) statement`. v0 discards the trigger and runs the
            // controlled statement: correct for a DC operating point, where `initial_step`
            // setup would run once anyway. (Proper event scheduling is a transient concern.)
            Some(Token::At) => {
                self.pos += 1;
                self.eat(&Token::LParen)?;
                self.skip_balanced_parens()?;
                self.parse_stmt()
            }
            // A block-local variable declaration, `real x, y;` / `integer i;`, or array
            // declaration, `real out_val[0:15];` (§ array variables).
            Some(Token::Real) | Some(Token::Integer) => {
                self.pos += 1; // base type (not retained)
                let names = self.parse_var_entry_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::VarDecl { names })
            }
            // A system-task call statement, `$strobe("…", a);` or `$finish;`.
            Some(Token::SysFunc(name)) => {
                let name = name.clone();
                self.pos += 1;
                let args = if self.at(&Token::LParen) {
                    self.parse_call_args()?
                } else {
                    Vec::new()
                };
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Task { name, args })
            }
            Some(Token::If) => {
                self.pos += 1;
                self.eat(&Token::LParen)?;
                let cond = self.parse_expr()?;
                self.eat(&Token::RParen)?;
                let then_ = self.parse_block_or_single()?;
                let else_ = if self.at(&Token::Else) {
                    self.pos += 1;
                    self.parse_block_or_single()?
                } else {
                    Vec::new()
                };
                Ok(Stmt::If { cond, then_, else_ })
            }
            // `V(...) <+ ...` / `I(...) <+ ...` is a contribution; a bare `name = ...` is an
            // assignment. Both start with an identifier, so disambiguate on the lookahead.
            Some(Token::Ident(name))
                if self.is_access(name) && self.nth(1) == Some(&Token::LParen) =>
            {
                // `I(<port>)` (§ port-current probe, LRM §5.4.3) is flow-only and read-only —
                // "shall not be used on the left side of `<+`" — so a `(` immediately followed
                // by `<` here is a parse error, not a contribution target.
                if self.nth(2) == Some(&Token::Lt) {
                    return self.err(format!(
                        "`{name}(<...>)` (a port-current probe) may not be used on the left \
                         side of `<+` — it is read-only (LRM §5.4.3)"
                    ));
                }
                let target = self.parse_access()?;
                self.eat(&Token::Contribute)?;
                let value = self.parse_expr()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Contribute { target, value })
            }
            Some(Token::Ident(_)) => {
                let lhs = self.expect_ident()?;
                // `lhs[index] = rhs;` / `lhs[i][j] = rhs;` assigns one element of a 1-D or § 2-D
                // array variable; each index must be a compile-time-constant or genvar
                // expression, checked at elaboration.
                let index = self.parse_index_list()?;
                self.eat(&Token::Assign)?;
                let rhs = self.parse_expr()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Assign { lhs, index, rhs })
            }
            // `electrical`/`thermal`/`ground` reused as a plain variable name (see
            // `ident_like_keyword`'s doc comment) — an assignment target here, never a
            // declaration start (that only ever happens in `parse_item`).
            Some(t) if ident_like_keyword(t).is_some() => {
                let lhs = self.expect_ident()?;
                let index = self.parse_index_list()?;
                self.eat(&Token::Assign)?;
                let rhs = self.parse_expr()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Assign { lhs, index, rhs })
            }
            Some(&Token::Keyword(kw)) => match kw.as_str() {
                "while" => self.parse_while(),
                "repeat" => self.parse_repeat(),
                "for" => self.parse_for(),
                "case" => self.parse_case(),
                "generate" => Ok(Stmt::Block(self.parse_generate()?)),
                // `bound_step(step);` is a transient-timestep hint, used as a bare statement
                // (like a system-task call) rather than a value — parsed the same way, and
                // elaborated as a no-op (`Stmt::Task`'s existing treatment).
                "bound_step" => {
                    self.pos += 1;
                    let args = if self.at(&Token::LParen) {
                        self.parse_call_args()?
                    } else {
                        Vec::new()
                    };
                    self.eat(&Token::Semicolon)?;
                    Ok(Stmt::Task {
                        name: "bound_step".to_string(),
                        args,
                    })
                }
                other => self.err(format!(
                    "reserved word `{other}` begins a construct outside the v0 subset"
                )),
            },
            _ => self.err("expected a statement".to_string()),
        }
    }

    /// Parse `generate … endgenerate`. The bracket keywords carry no semantics of their own in
    /// v0 — a `generate`-wrapped `for` over a `genvar` is recognised and fully unrolled purely
    /// by elaboration (see `crate::elaborate`), so this just behaves like `begin … end` and
    /// returns its contained statements for the caller to normalise.
    fn parse_generate(&mut self) -> Result<Vec<Stmt>, FrontendError> {
        self.eat_keyword("generate")?;
        let mut stmts = Vec::new();
        while !self.at_keyword("endgenerate") {
            if self.peek().is_none() {
                return self.err("unexpected end of input before `endgenerate`".to_string());
            }
            stmts.push(self.parse_stmt()?);
        }
        self.eat_keyword("endgenerate")?;
        Ok(stmts)
    }

    /// Parse `while (cond) body`.
    fn parse_while(&mut self) -> Result<Stmt, FrontendError> {
        self.eat_keyword("while")?;
        self.eat(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.eat(&Token::RParen)?;
        let body = self.parse_block_or_single()?;
        Ok(Stmt::While { cond, body })
    }

    /// Parse `repeat (count) body`.
    fn parse_repeat(&mut self) -> Result<Stmt, FrontendError> {
        self.eat_keyword("repeat")?;
        self.eat(&Token::LParen)?;
        let count = self.parse_expr()?;
        self.eat(&Token::RParen)?;
        let body = self.parse_block_or_single()?;
        Ok(Stmt::Repeat { count, body })
    }

    /// Parse `for (init; cond; step) body`. `init`/`step` are bare assignments.
    fn parse_for(&mut self) -> Result<Stmt, FrontendError> {
        self.eat_keyword("for")?;
        self.eat(&Token::LParen)?;
        let init = Box::new(self.parse_assignment()?);
        self.eat(&Token::Semicolon)?;
        let cond = self.parse_expr()?;
        self.eat(&Token::Semicolon)?;
        let step = Box::new(self.parse_assignment()?);
        self.eat(&Token::RParen)?;
        let body = self.parse_block_or_single()?;
        Ok(Stmt::For {
            init,
            cond,
            step,
            body,
        })
    }

    /// Parse `case (selector) arm… [default[:] body] endcase`.
    fn parse_case(&mut self) -> Result<Stmt, FrontendError> {
        self.eat_keyword("case")?;
        self.eat(&Token::LParen)?;
        let selector = self.parse_expr()?;
        self.eat(&Token::RParen)?;

        let mut arms = Vec::new();
        let mut default = None;
        loop {
            if self.at_keyword("endcase") {
                break;
            }
            if self.peek().is_none() {
                return self.err("unexpected end of input before `endcase`".to_string());
            }
            if self.at_keyword("default") {
                self.pos += 1;
                // The colon after `default` is optional in Verilog.
                if self.at(&Token::Colon) {
                    self.pos += 1;
                }
                default = Some(self.parse_block_or_single()?);
                continue;
            }
            // `label {, label} : body`.
            let mut labels = vec![self.parse_expr()?];
            while self.at(&Token::Comma) {
                self.pos += 1;
                labels.push(self.parse_expr()?);
            }
            self.eat(&Token::Colon)?;
            let body = self.parse_block_or_single()?;
            arms.push(CaseArm { labels, body });
        }
        self.eat_keyword("endcase")?;
        Ok(Stmt::Case {
            selector,
            arms,
            default,
        })
    }

    /// Parse a bare `lhs = rhs` assignment with no trailing terminator (for `for` headers).
    fn parse_assignment(&mut self) -> Result<Stmt, FrontendError> {
        let lhs = self.expect_ident()?;
        self.eat(&Token::Assign)?;
        let rhs = self.parse_expr()?;
        Ok(Stmt::Assign {
            lhs,
            index: Vec::new(),
            rhs,
        })
    }

    /// Whether `name` is a recognized access-function name (§ module preamble discipline/nature
    /// parsing) — the always-on `V`/`I`/`Temp`/`Pwr` baseline, plus anything a parsed
    /// `discipline` block has additively registered.
    fn is_access(&self, name: &str) -> bool {
        self.known_access.contains_key(name)
    }

    /// Parse an access function application `V(a[, b])` / `I(a[, b])` (or any other name
    /// [`Self::is_access`] recognizes).
    fn parse_access(&mut self) -> Result<Access, FrontendError> {
        let name = match self.peek() {
            Some(Token::Ident(n)) => n.clone(),
            _ => return self.err("expected an access function".to_string()),
        };
        let kind = *self.known_access.get(&name).ok_or_else(|| {
            FrontendError::Parse(format!("`{name}` is not a recognized access function"))
        })?;
        self.pos += 1;
        self.eat(&Token::LParen)?;
        let mut args = vec![self.parse_net_arg()?];
        if self.at(&Token::Comma) {
            self.pos += 1;
            args.push(self.parse_net_arg()?);
        }
        self.eat(&Token::RParen)?;
        Ok(Access { kind, args })
    }

    /// Parse a port-current probe, `name(<port>)` (§ port-current probe, LRM §5.4.3) — an
    /// access-function name applied to a single port identifier delimited by `<`/`>` rather
    /// than a plain net terminal list. `V(<port>)` parses syntactically (the grammar is
    /// otherwise identical to [`Self::parse_access`]'s dispatch) but is rejected at elaboration
    /// — the LRM only defines this form for a flow access function.
    fn parse_port_probe(&mut self) -> Result<(AccessKind, String), FrontendError> {
        let name = match self.peek() {
            Some(Token::Ident(n)) => n.clone(),
            _ => return self.err("expected an access function".to_string()),
        };
        let kind = *self.known_access.get(&name).ok_or_else(|| {
            FrontendError::Parse(format!("`{name}` is not a recognized access function"))
        })?;
        self.pos += 1;
        self.eat(&Token::LParen)?;
        self.eat(&Token::Lt)?;
        let port = self.expect_ident()?;
        self.eat(&Token::Gt)?;
        self.eat(&Token::RParen)?;
        Ok((kind, port))
    }

    // --- expressions (precedence climbing) -----------------------------------------

    fn parse_expr(&mut self) -> Result<ExprRef, FrontendError> {
        let cond = self.parse_bin(0)?;
        // The ternary `?:` binds looser than every binary operator and is right-associative.
        if self.at(&Token::Question) {
            self.pos += 1;
            let then_ = self.parse_expr()?;
            self.eat(&Token::Colon)?;
            let else_ = self.parse_expr()?;
            Ok(self.push(ExprAst::Cond { cond, then_, else_ }))
        } else {
            Ok(cond)
        }
    }

    fn parse_bin(&mut self, min_bp: u8) -> Result<ExprRef, FrontendError> {
        let mut lhs = self.parse_unary()?;
        while let Some((op, lbp, rbp)) = self.peek().and_then(binop_binding) {
            if lbp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_bin(rbp)?;
            lhs = self.push(ExprAst::Binary(op, lhs, rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<ExprRef, FrontendError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                let e = self.parse_unary()?;
                Ok(self.push(ExprAst::Unary(UnOp::Neg, e)))
            }
            Some(Token::Plus) => {
                // Unary plus is a no-op.
                self.pos += 1;
                self.parse_unary()
            }
            Some(Token::Not) => {
                self.pos += 1;
                let e = self.parse_unary()?;
                Ok(self.push(ExprAst::Unary(UnOp::Not, e)))
            }
            Some(Token::Tilde) => {
                self.pos += 1;
                let e = self.parse_unary()?;
                Ok(self.push(ExprAst::Unary(UnOp::BitNot, e)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<ExprRef, FrontendError> {
        match self.peek() {
            Some(Token::Number(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(self.push(ExprAst::Number(n)))
            }
            Some(Token::Inf) => {
                self.pos += 1;
                Ok(self.push(ExprAst::Number(f64::INFINITY)))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let e = self.parse_expr()?;
                self.eat(&Token::RParen)?;
                Ok(e)
            }
            // `{expr, expr, ...}`: an array literal, only meaningful as a `laplace_nd`-style
            // coefficient-list argument (§ Laplace/Z-domain filters) — parsed generically here,
            // like every other expression form, and restricted to that one use at elaboration.
            Some(Token::LBrace) => {
                self.pos += 1;
                let mut elems = vec![self.parse_expr()?];
                while self.at(&Token::Comma) {
                    self.pos += 1;
                    elems.push(self.parse_expr()?);
                }
                self.eat(&Token::RBrace)?;
                Ok(self.push(ExprAst::ArrayLit(elems)))
            }
            Some(Token::SysFunc(name)) => {
                let name = name.clone();
                self.pos += 1;
                // `$vt`/`$temperature` take no args; `$simparam("name", default)` etc. do.
                let args = if self.at(&Token::LParen) {
                    self.parse_call_args()?
                } else {
                    Vec::new()
                };
                Ok(self.push(ExprAst::SysFunc { name, args }))
            }
            Some(Token::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(self.push(ExprAst::Str(s)))
            }
            // `real(expr)` / `integer(expr)`: type-cast call expressions, distinct from `real`/
            // `integer` the declaration keywords — e.g. `digital = integer(v * scale);`. Same
            // dedicated tokens, disambiguated purely by the following `(`, exactly like `V`/`I`
            // access vs. an ordinary identifier below.
            Some(Token::Real) | Some(Token::Integer) if self.nth(1) == Some(&Token::LParen) => {
                let name = match self.peek() {
                    Some(Token::Integer) => "integer".to_string(),
                    _ => "real".to_string(),
                };
                self.parse_call(name)
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                if self.nth(1) == Some(&Token::LParen) {
                    if self.is_access(&name) {
                        if self.nth(2) == Some(&Token::Lt) {
                            let (kind, port) = self.parse_port_probe()?;
                            return Ok(self.push(ExprAst::PortProbe { kind, port }));
                        }
                        let access = self.parse_access()?;
                        Ok(self.push(ExprAst::Probe(access)))
                    } else {
                        self.parse_call(name)
                    }
                } else if self.nth(1) == Some(&Token::LBracket) {
                    // `name[index]` / `name[i][j]`: one element of a 1-D or § 2-D array
                    // variable, not a call — distinguished from a scalar reference purely by the
                    // following `[`, same disambiguation style as the call-vs-reference check
                    // above.
                    self.pos += 1;
                    let index = self.parse_index_list()?;
                    Ok(self.push(ExprAst::IndexedIdent(name, index)))
                } else {
                    self.pos += 1;
                    Ok(self.push(ExprAst::Ident(name)))
                }
            }
            // `electrical`/`thermal`/`ground` in expression-atom position: never a legitimate
            // declaration start here (that dispatch only ever happens in `parse_item`, never
            // mid-expression), so treat it as a bare identifier read — see
            // `ident_like_keyword`'s doc comment.
            Some(t) if ident_like_keyword(t).is_some() => {
                let name = ident_like_keyword(t).unwrap().to_string();
                self.pos += 1;
                Ok(self.push(ExprAst::Ident(name)))
            }
            // A reserved word in expression position must be a built-in function call
            // (`exp(x)`, `ddt(...)`, `pow(x, y)`, …). Elaboration maps the name to a
            // [`va_ir::Builtin`] and rejects any unsupported reserved word.
            Some(Token::Keyword(kw)) => {
                let name = kw.as_str().to_string();
                if self.nth(1) == Some(&Token::LParen) {
                    self.parse_call(name)
                } else {
                    self.err(format!(
                        "reserved word `{name}` is not valid in an expression"
                    ))
                }
            }
            _ => self.err("expected an expression".to_string()),
        }
    }

    fn parse_call(&mut self, name: String) -> Result<ExprRef, FrontendError> {
        self.pos += 1; // name
        let args = self.parse_call_args()?;
        Ok(self.push(ExprAst::Call { name, args }))
    }

    /// Parse a parenthesised, comma-separated argument list `( [expr {, expr}] )`. The cursor
    /// must be on the opening `(`.
    fn parse_call_args(&mut self) -> Result<Vec<ExprRef>, FrontendError> {
        self.eat(&Token::LParen)?;
        let mut args = Vec::new();
        if !self.at(&Token::RParen) {
            args.push(self.parse_expr()?);
            while self.at(&Token::Comma) {
                self.pos += 1;
                args.push(self.parse_expr()?);
            }
        }
        self.eat(&Token::RParen)?;
        Ok(args)
    }
}

/// Map an operator token to `(op, left_bp, right_bp)`. Higher binding power binds tighter;
/// `**` is right-associative (`right_bp < left_bp`).
/// Binding powers follow the standard Verilog operator-precedence table (IEEE 1364 Table 5-4),
/// loosest to tightest: `||` < `&&` < `|` < `^`/`^~` < `&` < `==`/`!=` < relational < shifts <
/// `+`/`-` < `*`/`/`/`%` < unary < `**`. `**` is right-associative (its `rbp` is lower than its
/// `lbp`); every other binary operator here is left-associative.
fn binop_binding(t: &Token) -> Option<(BinOp, u8, u8)> {
    Some(match t {
        Token::OrOr => (BinOp::Or, 1, 2),
        Token::AndAnd => (BinOp::And, 3, 4),
        Token::Pipe => (BinOp::BitOr, 5, 6),
        Token::Caret => (BinOp::BitXor, 7, 8),
        Token::CaretTilde => (BinOp::BitXnor, 7, 8),
        Token::Amp => (BinOp::BitAnd, 9, 10),
        Token::EqEq => (BinOp::Eq, 11, 12),
        Token::NotEq => (BinOp::Ne, 11, 12),
        Token::Lt => (BinOp::Lt, 13, 14),
        Token::Le => (BinOp::Le, 13, 14),
        Token::Gt => (BinOp::Gt, 13, 14),
        Token::Ge => (BinOp::Ge, 13, 14),
        Token::Shl => (BinOp::Shl, 15, 16),
        Token::Shr => (BinOp::Shr, 15, 16),
        Token::Plus => (BinOp::Add, 17, 18),
        Token::Minus => (BinOp::Sub, 17, 18),
        Token::Star => (BinOp::Mul, 19, 20),
        Token::Slash => (BinOp::Div, 19, 20),
        Token::Percent => (BinOp::Mod, 19, 20),
        Token::StarStar => (BinOp::Pow, 23, 22),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_src(src: &str) -> ModuleAst {
        let toks = lex(src).expect("lex");
        parse(&toks)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one module")
    }

    #[test]
    fn resistor_model() {
        let m = parse_src(include_str!("../../../models/resistor.va"));
        assert_eq!(m.name, "resistor");
        assert_eq!(m.ports, vec!["p", "n"]);

        // One parameter R with default 1000.0 and a from-range.
        let param = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param {
                    name,
                    default,
                    range,
                    ..
                } if name == "R" => Some((*default, range.is_some())),
                _ => None,
            })
            .expect("param R");
        assert!(matches!(m.expr(param.0), ExprAst::Number(v) if *v == 1000.0));
        assert!(param.1, "R should carry a range");

        // The analog block contains one contribution: I(p,n) <+ V(p,n)/R.
        let analog = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Analog(Stmt::Block(s)) => Some(s),
                _ => None,
            })
            .expect("analog block");
        assert_eq!(analog.len(), 1);
        match &analog[0] {
            Stmt::Contribute { target, value } => {
                assert_eq!(target.kind, AccessKind::Flow);
                assert_eq!(target.args[0].name, "p");
                assert_eq!(target.args[1].name, "n");
                assert!(target.args.iter().all(|a| a.index.is_empty()));
                assert!(matches!(m.expr(*value), ExprAst::Binary(BinOp::Div, _, _)));
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn capacitor_uses_ddt_call() {
        let m = parse_src(include_str!("../../../models/capacitor.va"));
        assert_eq!(m.name, "capacitor");
        let analog = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Analog(Stmt::Block(s)) => Some(s),
                _ => None,
            })
            .unwrap();
        match &analog[0] {
            Stmt::Contribute { value, .. } => match m.expr(*value) {
                ExprAst::Call { name, args } => {
                    assert_eq!(name, "ddt");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected ddt(...) call, got {other:?}"),
            },
            other => panic!("expected contribution, got {other:?}"),
        }
    }

    #[test]
    fn diode_uses_exp_and_sysfunc() {
        let m = parse_src(include_str!("../../../models/diode.va"));
        assert_eq!(m.name, "diode");
        // Two parameters Is and N.
        let params: Vec<_> = m
            .items
            .iter()
            .filter(|it| matches!(it, Item::Param { .. }))
            .collect();
        assert_eq!(params.len(), 2);
        // $vt appears somewhere in the arena.
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, ExprAst::SysFunc { name, .. } if name == "vt")));
        // exp(...) call present.
        assert!(m
            .exprs
            .iter()
            .any(|e| matches!(e, ExprAst::Call { name, .. } if name == "exp")));
    }

    #[test]
    fn precedence_mul_binds_tighter_than_add() {
        // 1 + 2 * 3  ==>  Add(1, Mul(2, 3))
        let m = parse_src("module t(); parameter real X = 1 + 2 * 3; endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Binary(BinOp::Add, l, r) => {
                assert!(matches!(m.expr(*l), ExprAst::Number(v) if *v == 1.0));
                assert!(matches!(m.expr(*r), ExprAst::Binary(BinOp::Mul, _, _)));
            }
            other => panic!("expected Add at the root, got {other:?}"),
        }
    }

    #[test]
    fn bitwise_and_shift_precedence_and_parsing() {
        // `1 << i & 1` ==> BitAnd(Shl(1, i), 1): shift binds tighter than `&` (Verilog's
        // operator-precedence table, IEEE 1364 Table 5-4), matching the corpus idiom
        // `(digital >> i) & 1`.
        let m = parse_src("module t(); parameter integer X = 1 << i & 1; endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Binary(BinOp::BitAnd, l, r) => {
                assert!(matches!(m.expr(*l), ExprAst::Binary(BinOp::Shl, _, _)));
                assert!(matches!(m.expr(*r), ExprAst::Number(v) if *v == 1.0));
            }
            other => panic!("expected BitAnd at the root, got {other:?}"),
        }

        // `|`, `^`/`^~`, and unary `~` all parse too.
        let m = parse_src("module t(); parameter integer X = (a | b) ^ (c ^~ d) & ~e; endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        assert!(matches!(
            m.expr(default),
            ExprAst::Binary(BinOp::BitXor, ..)
        ));
    }

    #[test]
    fn array_literal_expression_parses() {
        // `laplace_nd(sig, {1}, {1, tau})` — the exact `external/photonic/PhotoDetector.va`
        // idiom (coefficient-list arguments to a Laplace-domain filter builtin).
        let m = parse_src(
            "module t(a, b); electrical a, b; \
             analog I(a, b) <+ laplace_nd(V(a, b), {1}, {1, 2, 3}); endmodule",
        );
        let call = analog_body(&m)
            .into_iter()
            .find_map(|s| match s {
                Stmt::Contribute { value, .. } => Some(value),
                _ => None,
            })
            .unwrap();
        match m.expr(call) {
            ExprAst::Call { name, args } => {
                assert_eq!(name, "laplace_nd");
                assert_eq!(args.len(), 3);
                assert!(matches!(m.expr(args[0]), ExprAst::Probe(_)));
                match m.expr(args[1]) {
                    ExprAst::ArrayLit(elems) => assert_eq!(elems.len(), 1),
                    other => panic!("expected an array literal, got {other:?}"),
                }
                match m.expr(args[2]) {
                    ExprAst::ArrayLit(elems) => assert_eq!(elems.len(), 3),
                    other => panic!("expected an array literal, got {other:?}"),
                }
            }
            other => panic!("expected a call, got {other:?}"),
        }
    }

    #[test]
    fn ternary_parses_and_is_right_associative() {
        // `1 + 2 > 0 ? 10 : 20 ? 30 : 40` ==> Cond(1+2>0, 10, Cond(20, 30, 40)).
        let m =
            parse_src("module t(); parameter real X = 1 + 2 > 0 ? 10 : 20 ? 30 : 40; endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Cond { cond, then_, else_ } => {
                // The condition is the full `1 + 2 > 0` comparison, not just `0`.
                assert!(matches!(m.expr(*cond), ExprAst::Binary(BinOp::Gt, _, _)));
                assert!(matches!(m.expr(*then_), ExprAst::Number(v) if *v == 10.0));
                // Right-associative: the else-branch is itself a ternary.
                assert!(matches!(m.expr(*else_), ExprAst::Cond { .. }));
            }
            other => panic!("expected a ternary, got {other:?}"),
        }
    }

    #[test]
    fn pow_is_right_associative() {
        // 2 ** 3 ** 2  ==>  Pow(2, Pow(3, 2))
        let m = parse_src("module t(); parameter real X = 2 ** 3 ** 2; endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Binary(BinOp::Pow, l, r) => {
                assert!(matches!(m.expr(*l), ExprAst::Number(v) if *v == 2.0));
                assert!(matches!(m.expr(*r), ExprAst::Binary(BinOp::Pow, _, _)));
            }
            other => panic!("expected right-associative Pow, got {other:?}"),
        }
    }

    #[test]
    fn builtin_keyword_parses_as_a_call() {
        // `sqrt` is now a reserved word; followed by `(` it must parse to a call node so
        // elaboration can map it to a built-in (Call name unchanged from the old design).
        let m = parse_src("module t(); parameter real X = sqrt(4.0); endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Call { name, args } => {
                assert_eq!(name, "sqrt");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected sqrt(...) call, got {other:?}"),
        }
    }

    #[test]
    fn real_and_integer_parse_as_cast_calls_when_followed_by_paren() {
        // `real(x)`/`integer(x)` in expression position are type-cast calls, distinct from the
        // `real`/`integer` declaration keywords (a bare `real x, y;` still declares variables —
        // see `module_level_variable_declarations`).
        let m = parse_src("module t(); parameter real X = integer(2.5) + real(1); endmodule");
        let default = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { default, .. } => Some(*default),
                _ => None,
            })
            .unwrap();
        match m.expr(default) {
            ExprAst::Binary(BinOp::Add, l, r) => {
                assert!(matches!(m.expr(*l), ExprAst::Call { name, .. } if name == "integer"));
                assert!(matches!(m.expr(*r), ExprAst::Call { name, .. } if name == "real"));
            }
            other => panic!("expected Add at the root, got {other:?}"),
        }
    }

    /// Pull the analog block's statement list out of a parsed module.
    fn analog_body(m: &ModuleAst) -> Vec<Stmt> {
        m.items
            .iter()
            .find_map(|it| match it {
                Item::Analog(Stmt::Block(s)) => Some(s.clone()),
                _ => None,
            })
            .expect("analog block")
    }

    #[test]
    fn while_loop_parses() {
        let m = parse_src(
            "module t(); electrical a; analog begin while (i < 3) i = i + 1; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::While { body, .. } => assert_eq!(body.len(), 1),
            other => panic!("expected a while loop, got {other:?}"),
        }
    }

    #[test]
    fn repeat_loop_parses() {
        let m =
            parse_src("module t(); electrical a; analog begin repeat (4) i = i + 1; end endmodule");
        assert!(matches!(analog_body(&m)[0], Stmt::Repeat { .. }));
    }

    #[test]
    fn for_loop_parses_init_cond_step() {
        let m = parse_src(
            "module t(); electrical a; analog begin for (i = 0; i < 3; i = i + 1) x = x + i; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::For {
                init, step, body, ..
            } => {
                assert!(matches!(**init, Stmt::Assign { .. }));
                assert!(matches!(**step, Stmt::Assign { .. }));
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected a for loop, got {other:?}"),
        }
    }

    #[test]
    fn vector_net_declaration_parses() {
        // Shared prefix range: `electrical [3:0] bus;`.
        let m = parse_src("module t(); electrical [3:0] bus; endmodule");
        let nets = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Net { nets, .. } => Some(nets.clone()),
                _ => None,
            })
            .expect("net item");
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].name, "bus");
        assert!(
            !nets[0].ranges.is_empty(),
            "a bracketed `[3:0]` should record a range"
        );

        // Per-identifier suffix range, mixed with a plain scalar name in the same declaration:
        // `electrical bus[3:0], p;`.
        let m = parse_src("module t(); electrical bus[3:0], p; endmodule");
        let nets = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Net { nets, .. } => Some(nets.clone()),
                _ => None,
            })
            .expect("net item");
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].name, "bus");
        assert!(!nets[0].ranges.is_empty());
        assert_eq!(nets[1].name, "p");
        assert!(nets[1].ranges.is_empty());
    }

    #[test]
    fn vector_port_direction_bracket_is_accepted() {
        // The LRM's own DAC example: `input [0:width-1] in;` alongside a matching vector net
        // declaration. The bracket is parsed (and discarded — the net declaration carries the
        // real range) so this no longer fails to parse.
        let m = parse_src(
            "module dac(out, in); output out; input [0:7] in; \
             electrical out; electrical [0:7] in; endmodule",
        );
        assert!(m
            .items
            .iter()
            .any(|it| matches!(it, Item::Direction { nets, .. } if nets == &["in".to_string()])));
    }

    #[test]
    fn indexed_access_parses() {
        let m = parse_src(
            "module t(); electrical [3:0] bus; electrical gnd; \
             analog begin I(bus[i], gnd) <+ 1.0; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Contribute { target, .. } => {
                assert_eq!(target.args[0].name, "bus");
                assert_eq!(target.args[0].index.len(), 1);
                assert_eq!(target.args[1].name, "gnd");
                assert!(target.args[1].index.is_empty());
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn port_probe_parses() {
        // `I(<a>)` (§ port-current probe, LRM §5.4.3) — distinct from `I(a)`, no `NetArg`
        // involved at all.
        let m = parse_src(
            "module t(a); inout a; electrical a; \
             analog begin I(a) <+ I(<a>); end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Contribute { value, .. } => match m.expr(*value) {
                ExprAst::PortProbe { kind, port } => {
                    assert_eq!(*kind, AccessKind::Flow);
                    assert_eq!(port, "a");
                }
                other => panic!("expected a PortProbe, got {other:?}"),
            },
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn two_d_array_variable_declaration_parses() {
        let m = parse_src("module t(); real tile[0:3][0:2], scalar; endmodule");
        let names = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Var { names, .. } => Some(names.clone()),
                _ => None,
            })
            .expect("var item");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].name, "tile");
        assert_eq!(names[0].ranges.len(), 2);
        assert_eq!(names[1].name, "scalar");
        assert!(names[1].ranges.is_empty());
    }

    #[test]
    fn two_d_array_indexed_assignment_and_read_parse() {
        let m = parse_src(
            "module t(); real tile[0:3][0:2]; integer i, j; \
             analog begin tile[i][j] = 1.0; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Assign { lhs, index, .. } => {
                assert_eq!(lhs, "tile");
                assert_eq!(index.len(), 2);
            }
            other => panic!("expected an indexed assignment, got {other:?}"),
        }

        let m = parse_src(
            "module t(); real tile[0:3][0:2]; electrical a; integer i, j; \
             analog begin I(a) <+ tile[i][j]; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Contribute { value, .. } => match m.expr(*value) {
                ExprAst::IndexedIdent(name, index) => {
                    assert_eq!(name, "tile");
                    assert_eq!(index.len(), 2);
                }
                other => panic!("expected an IndexedIdent, got {other:?}"),
            },
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn two_d_vector_net_declaration_parses() {
        // Prefix form.
        let m = parse_src("module t(); electrical [0:1][0:2] grid; endmodule");
        let nets = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Net { nets, .. } => Some(nets.clone()),
                _ => None,
            })
            .expect("net item");
        assert_eq!(nets[0].name, "grid");
        assert_eq!(nets[0].ranges.len(), 2);

        // Suffix form.
        let m = parse_src("module t(); electrical grid[0:1][0:2]; endmodule");
        let nets = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Net { nets, .. } => Some(nets.clone()),
                _ => None,
            })
            .expect("net item");
        assert_eq!(nets[0].name, "grid");
        assert_eq!(nets[0].ranges.len(), 2);
    }

    #[test]
    fn two_d_indexed_net_access_parses() {
        let m = parse_src(
            "module t(); electrical [0:1][0:1] grid; electrical gnd; \
             analog begin I(grid[0][1], gnd) <+ 1.0; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Contribute { target, .. } => {
                assert_eq!(target.args[0].name, "grid");
                assert_eq!(target.args[0].index.len(), 2);
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn more_than_two_bracket_groups_is_a_parse_error() {
        for src in [
            "module t(); real tile[0:1][0:1][0:1]; endmodule",
            "module t(); electrical [0:1][0:1][0:1] grid; endmodule",
            "module t(); real tile[0:1][0:1]; integer i,j,k; \
             analog begin tile[i][j][k] = 1.0; end endmodule",
        ] {
            let toks = lex(src).expect("lex");
            assert!(parse(&toks).is_err(), "expected rejection for: {src}");
        }
    }

    #[test]
    fn slice_before_a_trailing_index_bracket_is_a_parse_error() {
        // `bus[0:1][0]` (a slice followed by another bracket) is rejected — a slice must be
        // the final bracket group. The parser doesn't resolve `mul` as a module (that's an
        // elaboration-time concern), so no module needs to be declared for it.
        let src = "module top(); electrical [0:3] bus; \
                   mul m1(bus[0:1][0], bus[2]); endmodule";
        let toks = lex(src).expect("lex");
        assert!(parse(&toks).is_err());
    }

    #[test]
    fn index_then_trailing_slice_parses_syntactically() {
        // `bus[2][0:1]` (an index followed by a trailing slice) is accepted at parse time —
        // whether it's semantically valid for a given declared net is an elaboration-time
        // question (§ 2-D vector net), not a parser one.
        let src = "module top(); electrical [0:3] bus; \
                   mul m1(bus[2][0:1], bus[0]); endmodule";
        let m = parse_src(src);
        match &m.items[1] {
            Item::Instance { connections, .. } => match &connections[0] {
                PortConn::Positional(net) => {
                    assert_eq!(net.index.len(), 1);
                    assert!(net.slice.is_some());
                }
                other => panic!("expected a positional connection, got {other:?}"),
            },
            other => panic!("expected an instance item, got {other:?}"),
        }
    }

    #[test]
    fn generate_endgenerate_wrapper_parses_transparently() {
        // The `generate`/`endgenerate` bracket carries no grammar of its own in v0 — it just
        // exposes the `for` loop inside, exactly as `begin`/`end` would.
        let m = parse_src(
            "module t(); genvar i; \
             analog begin generate for (i = 0; i < 3; i = i + 1) x = i; endgenerate end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                assert!(matches!(stmts[0], Stmt::For { .. }));
            }
            other => panic!("expected the generate wrapper's block, got {other:?}"),
        }
    }

    #[test]
    fn case_with_default_parses() {
        let m = parse_src(
            "module t(); electrical a; analog begin case (sel) 0, 1: x = 1; 2: x = 2; default: x = 0; endcase end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Case { arms, default, .. } => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].labels.len(), 2); // `0, 1:`
                assert!(default.is_some());
            }
            other => panic!("expected a case, got {other:?}"),
        }
    }

    #[test]
    fn analog_function_definition_parses() {
        let m = parse_src(
            "module t(); analog function real sq; input x; real x; sq = x * x; endfunction endmodule",
        );
        let f = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Function(f) => Some(f),
                _ => None,
            })
            .expect("function item");
        assert_eq!(f.name, "sq");
        assert_eq!(f.ret_ty, ParamType::Real);
        assert_eq!(f.args.len(), 1);
        assert_eq!(f.args[0].name, "x");
        assert_eq!(f.args[0].dir, Direction::Input);
        assert_eq!(f.body.len(), 1); // `sq = x * x;` (the `real x;` decl is discarded)
    }

    #[test]
    fn reserved_word_as_identifier_is_rejected() {
        // `time` is a reserved word and may not name a net.
        let toks = lex("module t(); electrical time; analog begin end endmodule").expect("lex");
        assert!(parse(&toks).is_err());
    }

    #[test]
    fn module_level_variable_declarations() {
        let m = parse_src("module t(); real q, v; integer i; endmodule");
        let vars: Vec<_> = m
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Var { ty, names } => {
                    let names: Vec<String> = names.iter().map(|e| e.name.clone()).collect();
                    Some((*ty, names))
                }
                _ => None,
            })
            .collect();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], (ParamType::Real, vec!["q".into(), "v".into()]));
        assert_eq!(vars[1], (ParamType::Integer, vec!["i".into()]));
    }

    #[test]
    fn variable_declaration_with_inline_initializer_parses() {
        // `real laser_freq = `P_C / wavelength / 1e-9;` — the exact
        // `external/photonic/CwLaser.va` idiom. A name with no initializer (`amplitude`) still
        // parses with `init: None`.
        let m = parse_src(
            "module t(); parameter real wavelength = 1550.0; \
             real laser_freq = 3.0e8 / wavelength / 1e-9; real amplitude; endmodule",
        );
        let names = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Var { names, .. } if names[0].name == "laser_freq" => Some(names.clone()),
                _ => None,
            })
            .expect("laser_freq declaration");
        assert_eq!(names.len(), 1);
        assert!(names[0].init.is_some());
        let amplitude = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Var { names, .. } if names[0].name == "amplitude" => Some(names.clone()),
                _ => None,
            })
            .expect("amplitude declaration");
        assert!(amplitude[0].init.is_none());
    }

    #[test]
    fn array_variable_declaration_parses() {
        // `real out_val[0:15], tmp;` — mixed array and scalar in one declaration, matching the
        // real corpus idiom (`external/verilogaLib-master/adc_16bit_ideal.va`).
        let m = parse_src("module t(); real out_val[0:15], tmp; endmodule");
        let names = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Var { names, .. } => Some(names.clone()),
                _ => None,
            })
            .expect("var item");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].name, "out_val");
        assert!(!names[0].ranges.is_empty());
        assert_eq!(names[1].name, "tmp");
        assert!(names[1].ranges.is_empty());
    }

    #[test]
    fn indexed_assignment_and_read_parse() {
        let m = parse_src(
            "module t(); real out_val[0:15]; integer i; \
             analog begin out_val[i] = 1.0; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Assign { lhs, index, .. } => {
                assert_eq!(lhs, "out_val");
                assert_eq!(index.len(), 1);
            }
            other => panic!("expected an indexed assignment, got {other:?}"),
        }

        let m = parse_src(
            "module t(); real out_val[0:15]; electrical a; integer j; \
             analog begin I(a) <+ out_val[j]; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Contribute { value, .. } => {
                assert!(
                    matches!(m.expr(*value), ExprAst::IndexedIdent(name, _) if name == "out_val")
                );
            }
            other => panic!("expected a contribution, got {other:?}"),
        }
    }

    #[test]
    fn genvar_declaration_parses() {
        let m = parse_src("module t(); genvar i; endmodule");
        assert!(m
            .items
            .iter()
            .any(|it| matches!(it, Item::Genvar { names } if names == &["i".to_string()])));
    }

    #[test]
    fn inclusive_and_mixed_range_bounds() {
        // `from [0:inf)` — inclusive lower, exclusive upper (the varactor/diode style).
        let m = parse_src("module t(); parameter real C = 0.5 from [0:inf); endmodule");
        let range = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { range, .. } => range.clone(),
                _ => None,
            })
            .expect("a range");
        assert!(range.lo_inclusive, "[ should be inclusive");
        assert!(!range.hi_inclusive, ") should be exclusive");
    }

    #[test]
    fn exclude_clauses_are_accepted() {
        // Standalone single-value exclusion (`exclude 0`), no `from` range.
        let m = parse_src("module t(); parameter real vj = 1.0 exclude 0; endmodule");
        assert!(m.items.iter().any(|it| matches!(
            it,
            Item::Param { name, range, .. } if name == "vj" && range.is_none()
        )));

        // `from [..] exclude <value>` and a range-form exclusion both parse; the `from`
        // range is retained.
        let m = parse_src(
            "module t(); parameter integer level = 1 from [1:4] exclude 3 exclude (10:20); endmodule",
        );
        let range = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Param { range, .. } => range.clone(),
                _ => None,
            })
            .expect("a from range");
        assert!(range.lo_inclusive && range.hi_inclusive);
    }

    #[test]
    fn localparam_parses_like_parameter() {
        // `localparam` shares `parameter`'s grammar; v0 lowers both to the same `Item::Param`
        // since there is no instance-parameter-override path to distinguish them against.
        let m = parse_src("module t(); localparam real TN = 2; endmodule");
        assert!(m.items.iter().any(
            |it| matches!(it, Item::Param { name, default, .. } if name == "TN" && matches!(m.expr(*default), ExprAst::Number(n) if *n == 2.0))
        ));
    }

    #[test]
    fn event_control_runs_the_controlled_statement() {
        // `@(initial_step) begin … end` — the trigger is discarded; the body survives.
        let m = parse_src(
            "module t(a, b); electrical a, b; analog begin @(initial_step) begin x = 1.0; end I(a, b) <+ x; end endmodule",
        );
        let body = analog_body(&m);
        // The event-controlled block becomes a plain block of one assignment.
        match &body[0] {
            Stmt::Block(inner) => assert!(matches!(inner[0], Stmt::Assign { .. })),
            other => panic!("expected the controlled block, got {other:?}"),
        }
        assert!(matches!(body[1], Stmt::Contribute { .. }));
    }

    #[test]
    fn event_control_with_nested_parens_in_trigger() {
        // `@(cross(V(a,b) - 1.0, +1))` — nested parens in the event are skipped.
        let m = parse_src(
            "module t(a, b); electrical a, b; analog begin @(cross(V(a, b) - 1.0, 1)) x = 1.0; I(a, b) <+ x; end endmodule",
        );
        let body = analog_body(&m);
        assert!(matches!(body[0], Stmt::Assign { .. }));
    }

    #[test]
    fn system_task_statement_with_string_arg() {
        let m = parse_src(
            r#"module t(a, b); electrical a, b; analog begin $strobe("v=%E", V(a, b)); I(a, b) <+ 0.0; end endmodule"#,
        );
        let body = analog_body(&m);
        match &body[0] {
            Stmt::Task { name, args } => {
                assert_eq!(name, "strobe");
                assert_eq!(args.len(), 2);
                // First argument is the format string.
                assert!(matches!(m.expr(args[0]), ExprAst::Str(s) if s == "v=%E"));
            }
            other => panic!("expected a system-task statement, got {other:?}"),
        }
    }

    #[test]
    fn bound_step_statement_parses() {
        // `bound_step(step);` is a bare statement (a transient-timestep hint), not a value —
        // parsed the same way as a system-task call.
        let m = parse_src(
            "module t(a, b); electrical a, b; analog begin bound_step(1n); I(a, b) <+ 0.0; end endmodule",
        );
        match &analog_body(&m)[0] {
            Stmt::Task { name, args } => {
                assert_eq!(name, "bound_step");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected a bound_step statement, got {other:?}"),
        }
    }

    #[test]
    fn named_block_with_local_var_decls() {
        let m = parse_src(
            "module t(a, b); electrical a, b; analog begin : blk real x, y; x = V(a, b); y = x; I(a, b) <+ y; end endmodule",
        );
        let body = analog_body(&m);
        match &body[0] {
            Stmt::VarDecl { names } => {
                let names: Vec<String> = names.iter().map(|e| e.name.clone()).collect();
                assert_eq!(names, vec!["x".to_string(), "y".to_string()]);
            }
            other => panic!("expected a local declaration, got {other:?}"),
        }
        // The `: blk` label is consumed; the rest of the block parses normally.
        assert!(matches!(body[1], Stmt::Assign { .. }));
        assert!(matches!(body.last(), Some(Stmt::Contribute { .. })));
    }

    #[test]
    fn branch_declaration_parses() {
        let m = parse_src(
            "module t(a, b); electrical a, b; branch (a, b) br1, br2; analog begin I(br1) <+ V(br1); end endmodule",
        );
        let (terminals, names) = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Branch { terminals, names } => Some((terminals.clone(), names.clone())),
                _ => None,
            })
            .expect("branch item");
        let terminal_names: Vec<_> = terminals.iter().map(|t| t.name.clone()).collect();
        assert_eq!(terminal_names, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(names, vec!["br1".to_string(), "br2".to_string()]);
    }

    #[test]
    fn missing_semicolon_is_an_error() {
        let toks = lex("module t(); parameter real X = 1 endmodule").expect("lex");
        assert!(parse(&toks).is_err());
    }

    /// `mvsg_cmc_3.2.0.va`'s real idiom: `if (cond);` — the empty statement as an `if`'s entire
    /// body, deliberately doing nothing on that branch. Legal wherever a statement is expected
    /// (LRM); lowers to an empty `Stmt::Block`.
    #[test]
    fn empty_statement_is_a_valid_no_op() {
        let m = parse_src(
            "module t(); electrical p, n; \
             analog begin \
                 if (V(p, n) == 0.0); \
                 I(p, n) <+ V(p, n); \
             end endmodule",
        );
        let analog = m
            .items
            .iter()
            .find_map(|item| match item {
                Item::Analog(stmt) => Some(stmt),
                _ => None,
            })
            .expect("an analog block");
        let Stmt::Block(body) = analog else {
            panic!("expected the analog block's top-level Block");
        };
        let Stmt::If { then_, .. } = &body[0] else {
            panic!("expected an If statement");
        };
        assert_eq!(then_.len(), 1);
        assert!(matches!(&then_[0], Stmt::Block(inner) if inner.is_empty()));
    }

    /// A source file with no `module` at all is not an error — real corpus headers
    /// (`generalMacrosAndDefines.va`, meant only to be `` `include ``d, never compiled
    /// standalone) carry nothing but `` `define ``s, which `` crate::preprocess `` fully expands
    /// away, leaving zero tokens by the time `parse` ever sees them (see
    /// [`crate::tests::macro_only_file_compiles_to_zero_modules`] for that full,
    /// preprocessor-inclusive path).
    #[test]
    fn zero_modules_is_not_an_error() {
        assert!(parse(&[]).expect("parse").is_empty());
    }

    #[test]
    fn instance_with_positional_ports_parses() {
        let m = parse_src("module top(a, b); electrical a, b; resistor r1(a, b); endmodule");
        match &m.items[1] {
            Item::Instance {
                module,
                name,
                params,
                connections,
            } => {
                assert_eq!(module, "resistor");
                assert_eq!(name, "r1");
                assert!(params.is_empty());
                assert_eq!(connections.len(), 2);
                for (conn, expected) in connections.iter().zip(["a", "b"]) {
                    match conn {
                        PortConn::Positional(net) => assert_eq!(net.name, expected),
                        other => panic!("expected a positional connection, got {other:?}"),
                    }
                }
            }
            other => panic!("expected an instance item, got {other:?}"),
        }
    }

    #[test]
    fn instance_connection_slice_parses() {
        // `in[0:1]` as a connection argument — the exact
        // `external/photonic/Attenuator.va` idiom (`CartesianMultiplier1(transfer, in[0:1],
        // out[0:1]);`): a range slice, distinct from a single `[i]` index.
        let m = parse_src(
            "module top(); electrical [0:1] transfer; electrical [0:3] in, out; \
             mul m1(transfer, in[0:1], out[0:1]); endmodule",
        );
        // items[0] = `transfer`'s net decl, items[1] = `in, out`'s (one shared declaration),
        // items[2] = the instance.
        match &m.items[2] {
            Item::Instance { connections, .. } => {
                assert_eq!(connections.len(), 3);
                match &connections[0] {
                    PortConn::Positional(net) => {
                        assert_eq!(net.name, "transfer");
                        assert!(net.index.is_empty() && net.slice.is_none());
                    }
                    other => panic!("expected a positional connection, got {other:?}"),
                }
                for (conn, expected_name) in connections[1..].iter().zip(["in", "out"]) {
                    match conn {
                        PortConn::Positional(net) => {
                            assert_eq!(net.name, expected_name);
                            assert!(net.slice.is_some());
                            assert!(net.index.is_empty());
                        }
                        other => panic!("expected a positional connection, got {other:?}"),
                    }
                }
            }
            other => panic!("expected an instance item, got {other:?}"),
        }
    }

    #[test]
    fn instance_with_named_ports_and_param_override_parses() {
        let m = parse_src(
            "module top(vin, vout, gnd); electrical vin, vout, gnd; \
             divider #(.gain(2.0)) d1(.out(vout), .in(vin), .gnd(gnd)); endmodule",
        );
        match &m.items[1] {
            Item::Instance {
                module,
                name,
                params,
                connections,
            } => {
                assert_eq!(module, "divider");
                assert_eq!(name, "d1");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].0, "gain");
                assert_eq!(connections.len(), 3);
                let ports: Vec<&str> = connections
                    .iter()
                    .map(|c| match c {
                        PortConn::Named { port, .. } => port.as_str(),
                        other => panic!("expected a named connection, got {other:?}"),
                    })
                    .collect();
                // Named connections may appear out of the submodule's declared port order.
                assert_eq!(ports, vec!["out", "in", "gnd"]);
            }
            other => panic!("expected an instance item, got {other:?}"),
        }
    }

    #[test]
    fn two_modules_in_one_file_both_parse() {
        let toks = lex(
            "module leg(p, n); electrical p, n; parameter real r = 1000; \
             analog I(p, n) <+ V(p, n) / r; endmodule \
             module top(a, b); electrical a, b; leg l1(a, b); endmodule",
        )
        .expect("lex");
        let modules = parse(&toks).expect("parse");
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].name, "leg");
        assert_eq!(modules[1].name, "top");
    }

    #[test]
    fn mixed_positional_and_named_connections_parse_but_flagged_later() {
        // The grammar itself doesn't distinguish; rejecting a mix is elaboration's job.
        let m = parse_src(
            "module top(a, b, c); electrical a, b, c; \
             three_port t1(a, .p2(b), .p3(c)); endmodule",
        );
        match &m.items[1] {
            Item::Instance { connections, .. } => {
                assert!(matches!(connections[0], PortConn::Positional(_)));
                assert!(matches!(connections[1], PortConn::Named { .. }));
            }
            other => panic!("expected an instance item, got {other:?}"),
        }
    }

    // --- discipline/nature preamble parsing (§ module preamble discipline/nature parsing) ---

    /// Build a bare `Parser` over `toks`, seeded with the same always-on access baseline
    /// [`parse`] uses, for tests that need to inspect [`Parser::natures`]/
    /// [`Parser::disciplines`]/[`Parser::known_access`] directly (not reachable through the
    /// public [`parse`]/[`parse_src`] API, which only returns the finished `ModuleAst`s).
    fn make_parser(toks: &[Token]) -> Parser<'_> {
        let mut known_access = HashMap::new();
        known_access.insert("V".to_string(), AccessKind::Potential);
        known_access.insert("Temp".to_string(), AccessKind::Potential);
        known_access.insert("I".to_string(), AccessKind::Flow);
        known_access.insert("Pwr".to_string(), AccessKind::Flow);
        Parser {
            toks,
            pos: 0,
            exprs: Vec::new(),
            natures: HashMap::new(),
            disciplines: HashMap::new(),
            known_access,
        }
    }

    #[test]
    fn discipline_and_nature_parse_accellera_style() {
        // The exact `external/disciplines.vams` shape: a `;` after every discipline/nature name.
        let src = "nature Current; units = \"A\"; access = I; idt_nature = Charge; \
                    abstol = 1e-12; endnature \
                    nature Voltage; units = \"V\"; access = V; endnature \
                    discipline electrical; potential Voltage; flow Current; enddiscipline \
                    module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let mut p = make_parser(&toks);
        p.parse_preamble().expect("parse preamble");
        assert_eq!(p.known_access.get("V"), Some(&AccessKind::Potential));
        assert_eq!(p.known_access.get("I"), Some(&AccessKind::Flow));
        let current = p.natures.get("Current").expect("Current nature parsed");
        assert_eq!(current.access.as_deref(), Some("I"));
        assert_eq!(current.abstol, Some(1e-12));
        assert_eq!(current.idt_nature.as_deref(), Some("Charge"));
        let electrical = p
            .disciplines
            .get("electrical")
            .expect("electrical discipline parsed");
        assert_eq!(electrical.potential.as_deref(), Some("Voltage"));
        assert_eq!(electrical.flow.as_deref(), Some("Current"));
        // The module that follows must still parse fine, `V`/`I` intact.
        let m = p.parse_module().expect("parse module");
        assert_eq!(m.name, "t");
    }

    #[test]
    fn discipline_and_nature_parse_no_semicolon_style() {
        // The exact `external/ekv3_natures.va` shape: no `;` after the discipline/nature name.
        let src = "nature Current\n units = \"A\";\n access = I;\n endnature \
                    nature Voltage\n units = \"V\";\n access = V;\n endnature \
                    discipline electrical\n potential Voltage;\n flow Current;\n enddiscipline \
                    module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule";
        let toks = lex(src).expect("lex");
        let mut p = make_parser(&toks);
        p.parse_preamble().expect("parse preamble");
        assert!(p.natures.contains_key("Current"));
        assert!(p.disciplines.contains_key("electrical"));
        let m = p.parse_module().expect("parse module");
        assert_eq!(m.name, "t");
    }

    #[test]
    fn custom_access_name_recognized_after_discipline_block() {
        // Before this discipline/nature block is parsed, `MMF` is not a recognized access
        // function at all — `MMF(a, b) <+ ...;` would previously mis-parse.
        let m = parse_src(
            "nature Magneto_Motive_Force; units = \"A*turn\"; access = MMF; endnature \
             discipline magnetic; potential Magneto_Motive_Force; enddiscipline \
             module t(a, b); electrical a, b; analog MMF(a, b) <+ 0.0; endmodule",
        );
        assert_eq!(m.name, "t");
        assert!(matches!(analog_body(&m)[0], Stmt::Contribute { .. }));
    }

    #[test]
    fn custom_discipline_used_as_a_net_type_keyword() {
        // `optical a, b;` after `discipline optical; ... enddiscipline` — a bare leading
        // identifier that names a declared custom discipline is a net declaration, not a
        // module instantiation (`external/microring_modulator.va`'s optical ports).
        let m = parse_src(
            "nature Opt_field; units = \"sqrt(W)\"; access = E; endnature \
             discipline optical; potential Opt_field; enddiscipline \
             module t(a, b); optical a, b; analog E(a) <+ E(b); endmodule",
        );
        assert_eq!(m.name, "t");
        let nets = m
            .items
            .iter()
            .find_map(|item| match item {
                Item::Net { discipline, nets } => Some((discipline, nets)),
                _ => None,
            })
            .expect("a net declaration item");
        assert_eq!(nets.0, &Discipline::Custom("optical".to_string()));
        assert_eq!(
            nets.1.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn bare_v_i_recognized_with_no_discipline_block_at_all() {
        // No preamble whatsoever: the always-on baseline alone must still be enough.
        let m = parse_src("module t(a, b); electrical a, b; analog I(a, b) <+ V(a, b); endmodule");
        assert!(matches!(analog_body(&m)[0], Stmt::Contribute { .. }));
    }

    #[test]
    fn domain_discrete_and_continuous_parse() {
        let toks = lex("discipline logic; domain discrete; enddiscipline \
             discipline wire_like; domain continuous; enddiscipline \
             module t(); endmodule")
        .expect("lex");
        let mut p = make_parser(&toks);
        p.parse_preamble().expect("parse preamble");
        assert_eq!(
            p.disciplines.get("logic").and_then(|d| d.domain),
            Some(DomainKind::Discrete)
        );
        assert_eq!(
            p.disciplines.get("wire_like").and_then(|d| d.domain),
            Some(DomainKind::Continuous)
        );
    }

    #[test]
    fn nature_abstol_non_literal_is_dropped_not_an_error() {
        let toks = lex("nature Current; access = I; abstol = 2*1e-6; endnature \
             module t(); endmodule")
        .expect("lex");
        let mut p = make_parser(&toks);
        p.parse_preamble().expect("parse preamble");
        assert_eq!(p.natures.get("Current").and_then(|n| n.abstol), None);
    }

    #[test]
    fn unknown_discipline_attribute_is_a_parse_error() {
        let toks =
            lex("discipline electrical; bogus_attr Voltage; enddiscipline module t(); endmodule")
                .expect("lex");
        let mut p = make_parser(&toks);
        assert!(p.parse_preamble().is_err());
    }

    #[test]
    fn thermal_electrical_ground_reused_as_a_plain_variable_name() {
        // `external/ekv3_variables.va`: `real thermal;`, later read/assigned as a bare
        // identifier throughout `ekv3_noise.va`/`ekv3_oppoints.va` — all three `` `include ``d
        // into the same compilation unit as `external/ekv3.va`.
        let m = parse_src(
            "module t(a, b); electrical a, b; real thermal, electrical, ground; \
             analog begin thermal = 1.0; ground = thermal * 2.0; \
             I(a, b) <+ ground + electrical; end endmodule",
        );
        let names: Vec<&str> = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Var { names, .. } => Some(names.iter().map(|e| e.name.as_str()).collect()),
                _ => None,
            })
            .expect("var decl");
        assert_eq!(names, vec!["thermal", "electrical", "ground"]);

        let analog = m
            .items
            .iter()
            .find_map(|it| match it {
                Item::Analog(Stmt::Block(s)) => Some(s),
                _ => None,
            })
            .expect("analog block");
        assert!(matches!(
            &analog[0],
            Stmt::Assign { lhs, .. } if lhs == "thermal"
        ));
        assert!(matches!(
            &analog[1],
            Stmt::Assign { lhs, .. } if lhs == "ground"
        ));
        match &analog[1] {
            Stmt::Assign { rhs, .. } => {
                assert!(matches!(
                    m.expr(*rhs),
                    ExprAst::Binary(BinOp::Mul, l, _) if matches!(m.expr(*l), ExprAst::Ident(n) if n == "thermal")
                ));
            }
            _ => unreachable!(),
        }
    }
}
