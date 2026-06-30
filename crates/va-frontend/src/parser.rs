//! Parser: token stream → surface [`crate::ast::ModuleAst`].
//!
//! A hand-written recursive-descent parser for the §1 subset, with a precedence-climbing
//! expression parser. It parses exactly one `module`. Compiler directives (`` `include ``)
//! are skipped — v0 has no preprocessor.
//!
//! Beyond `if`/`else`, the parser accepts the analog control-flow statements `while`, `for`,
//! `repeat`, and `case`, and `analog function` definitions. These are surfaced in the AST but
//! cannot yet be lowered into the frozen v0 IR (only `if`/`else` and module-level statements
//! are lowered); elaboration rejects them with a clear error. See [`crate::elaborate`].
//!
//! # Limitations
//!
//! - Errors are reported by token index, not source byte offset: the lexer discards spans
//!   in v0 (see [`crate::lexer`]). Carrying spans is a planned improvement.
//! - Parameter ranges accept mixed inclusive/exclusive delimiters — `[`/`]` (inclusive) and
//!   `(`/`)` (exclusive) in any combination, e.g. `from [0:inf)`. The inclusive/exclusive
//!   flags are recorded in the AST but dropped by elaboration (see [`crate::elaborate`]).
//! - Access functions are limited to `V` and `I`; other natures are out of scope for v0.
//! - Analog functions retain argument directions and body only; argument/local *types*
//!   (`real x;`) are parsed and discarded.
//! - Event control `@(event) stmt` is parsed but the trigger is discarded — the controlled
//!   statement runs unconditionally. This matches DC operating-point semantics (`initial_step`
//!   setup runs once regardless); proper event scheduling is a transient-analysis concern.

use crate::ast::{
    Access, AccessKind, AnalogFunction, BinOp, CaseArm, Direction, Discipline, ExprAst, ExprRef,
    FuncArg, Item, ModuleAst, ParamType, Range, Stmt, UnOp,
};
use crate::lexer::Token;
use crate::FrontendError;

/// Parse a token stream into a single module AST.
///
/// # Errors
///
/// Returns [`FrontendError::Parse`] on unexpected or missing tokens.
pub fn parse(tokens: &[Token]) -> Result<ModuleAst, FrontendError> {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        exprs: Vec::new(),
    };
    p.parse_module()
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    exprs: Vec<ExprAst>,
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

    /// Skip directives and top-level `discipline …  enddiscipline` / `nature … endnature`
    /// blocks (e.g. from an expanded `disciplines.vams`) that precede the module. v0 models
    /// disciplines natively, so these declarations are ignored.
    fn skip_preamble(&mut self) {
        loop {
            self.skip_directives();
            if self.at_keyword("discipline") {
                self.skip_block_until("enddiscipline");
            } else if self.at_keyword("nature") {
                self.skip_block_until("endnature");
            } else {
                break;
            }
        }
    }

    /// Consume tokens up to and including the reserved word `end` (e.g. `enddiscipline`).
    fn skip_block_until(&mut self, end: &str) {
        while let Some(tok) = self.peek() {
            let done = matches!(tok, Token::Keyword(kw) if kw.as_str() == end);
            self.pos += 1;
            if done {
                break;
            }
        }
    }

    // --- module --------------------------------------------------------------------

    fn parse_module(&mut self) -> Result<ModuleAst, FrontendError> {
        self.skip_preamble();
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
                let nets = self.ident_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Direction { dir, nets })
            }
            Some(Token::Electrical) | Some(Token::Thermal) => {
                let discipline = match self.bump() {
                    Some(Token::Electrical) => Discipline::Electrical,
                    _ => Discipline::Thermal,
                };
                let nets = self.ident_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Net { discipline, nets })
            }
            Some(Token::Parameter) => self.parse_param(),
            // A bare `real`/`integer` at module scope declares variables (a `parameter`
            // declaration starts with `parameter`, and an `analog function` with `analog`).
            Some(Token::Real) | Some(Token::Integer) => {
                let ty = match self.bump() {
                    Some(Token::Integer) => ParamType::Integer,
                    _ => ParamType::Real,
                };
                let names = self.ident_list()?;
                self.eat(&Token::Semicolon)?;
                Ok(Item::Var { ty, names })
            }
            Some(Token::Keyword(kw)) if kw.as_str() == "branch" => self.parse_branch_decl(),
            // `analog function …` is a function definition; a bare `analog` is the block.
            Some(Token::Analog) if matches!(self.nth(1), Some(Token::Keyword(kw)) if kw.as_str() == "function") => {
                self.parse_function()
            }
            Some(Token::Analog) => {
                self.pos += 1;
                let body = self.parse_block_or_single()?;
                Ok(Item::Analog(Stmt::Block(body)))
            }
            _ => self.err("expected a declaration or `analog` block".to_string()),
        }
    }

    fn parse_param(&mut self) -> Result<Item, FrontendError> {
        self.eat(&Token::Parameter)?;
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
        let mut terminals = vec![self.expect_ident()?];
        if self.at(&Token::Comma) {
            self.pos += 1;
            terminals.push(self.expect_ident()?);
        }
        self.eat(&Token::RParen)?;
        let names = self.ident_list()?;
        self.eat(&Token::Semicolon)?;
        Ok(Item::Branch { terminals, names })
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
            // A block-local variable declaration, `real x, y;` / `integer i;`.
            Some(Token::Real) | Some(Token::Integer) => {
                self.pos += 1; // base type (not retained)
                let names = self.ident_list()?;
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
            Some(Token::Ident(name)) if is_access(name) && self.nth(1) == Some(&Token::LParen) => {
                let target = self.parse_access()?;
                self.eat(&Token::Contribute)?;
                let value = self.parse_expr()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Contribute { target, value })
            }
            Some(Token::Ident(_)) => {
                let lhs = self.expect_ident()?;
                self.eat(&Token::Assign)?;
                let rhs = self.parse_expr()?;
                self.eat(&Token::Semicolon)?;
                Ok(Stmt::Assign { lhs, rhs })
            }
            Some(&Token::Keyword(kw)) => match kw.as_str() {
                "while" => self.parse_while(),
                "repeat" => self.parse_repeat(),
                "for" => self.parse_for(),
                "case" => self.parse_case(),
                other => self.err(format!(
                    "reserved word `{other}` begins a construct outside the v0 subset"
                )),
            },
            _ => self.err("expected a statement".to_string()),
        }
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
        Ok(Stmt::Assign { lhs, rhs })
    }

    /// Parse an access function application `V(a[, b])` / `I(a[, b])`.
    fn parse_access(&mut self) -> Result<Access, FrontendError> {
        let kind = match self.peek() {
            Some(Token::Ident(n)) if n == "V" => AccessKind::Potential,
            Some(Token::Ident(n)) if n == "I" => AccessKind::Flow,
            _ => return self.err("expected an access function `V` or `I`".to_string()),
        };
        self.pos += 1;
        self.eat(&Token::LParen)?;
        let mut args = vec![self.expect_ident()?];
        if self.at(&Token::Comma) {
            self.pos += 1;
            args.push(self.expect_ident()?);
        }
        self.eat(&Token::RParen)?;
        Ok(Access { kind, args })
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
            Some(Token::SysFunc(name)) => {
                let name = name.clone();
                self.pos += 1;
                // v0 system functions ($vt, $temperature) take no arguments; tolerate `()`.
                if self.at(&Token::LParen) {
                    self.pos += 1;
                    self.eat(&Token::RParen)?;
                }
                Ok(self.push(ExprAst::SysFunc(name)))
            }
            Some(Token::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(self.push(ExprAst::Str(s)))
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                if self.nth(1) == Some(&Token::LParen) {
                    if is_access(&name) {
                        let access = self.parse_access()?;
                        Ok(self.push(ExprAst::Probe(access)))
                    } else {
                        self.parse_call(name)
                    }
                } else {
                    self.pos += 1;
                    Ok(self.push(ExprAst::Ident(name)))
                }
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

/// Whether `name` is a branch access function recognised by v0 (`V` or `I`).
fn is_access(name: &str) -> bool {
    name == "V" || name == "I"
}

/// Map an operator token to `(op, left_bp, right_bp)`. Higher binding power binds tighter;
/// `**` is right-associative (`right_bp < left_bp`).
fn binop_binding(t: &Token) -> Option<(BinOp, u8, u8)> {
    Some(match t {
        Token::OrOr => (BinOp::Or, 1, 2),
        Token::AndAnd => (BinOp::And, 3, 4),
        Token::EqEq => (BinOp::Eq, 5, 6),
        Token::NotEq => (BinOp::Ne, 5, 6),
        Token::Lt => (BinOp::Lt, 7, 8),
        Token::Le => (BinOp::Le, 7, 8),
        Token::Gt => (BinOp::Gt, 7, 8),
        Token::Ge => (BinOp::Ge, 7, 8),
        Token::Plus => (BinOp::Add, 9, 10),
        Token::Minus => (BinOp::Sub, 9, 10),
        Token::Star => (BinOp::Mul, 11, 12),
        Token::Slash => (BinOp::Div, 11, 12),
        Token::StarStar => (BinOp::Pow, 14, 13),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_src(src: &str) -> ModuleAst {
        let toks = lex(src).expect("lex");
        parse(&toks).expect("parse")
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
                assert_eq!(target.args, vec!["p", "n"]);
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
            .any(|e| matches!(e, ExprAst::SysFunc(s) if s == "vt")));
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
                Item::Var { ty, names } => Some((*ty, names.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], (ParamType::Real, vec!["q".into(), "v".into()]));
        assert_eq!(vars[1], (ParamType::Integer, vec!["i".into()]));
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
    fn named_block_with_local_var_decls() {
        let m = parse_src(
            "module t(a, b); electrical a, b; analog begin : blk real x, y; x = V(a, b); y = x; I(a, b) <+ y; end endmodule",
        );
        let body = analog_body(&m);
        match &body[0] {
            Stmt::VarDecl { names } => {
                assert_eq!(names, &vec!["x".to_string(), "y".to_string()]);
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
        assert_eq!(terminals, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(names, vec!["br1".to_string(), "br2".to_string()]);
    }

    #[test]
    fn missing_semicolon_is_an_error() {
        let toks = lex("module t(); parameter real X = 1 endmodule").expect("lex");
        assert!(parse(&toks).is_err());
    }
}
