//! Parser: token stream → surface [`crate::ast::ModuleAst`].
//!
//! A hand-written recursive-descent parser for the §1 subset, with a precedence-climbing
//! expression parser. It parses exactly one `module`. Compiler directives (`` `include ``)
//! are skipped — v0 has no preprocessor.
//!
//! # Limitations
//!
//! - Errors are reported by token index, not source byte offset: the lexer discards spans
//!   in v0 (see [`crate::lexer`]). Carrying spans is a planned improvement.
//! - Parameter ranges accept only parenthesised `( … : … )` bounds; bracketed inclusive
//!   bounds (`[ … ]`) are not yet lexed and so cannot be parsed.
//! - Access functions are limited to `V` and `I`; other natures are out of scope for v0.

use crate::ast::{
    Access, AccessKind, BinOp, Direction, Discipline, ExprAst, ExprRef, Item, ModuleAst, ParamType,
    Range, Stmt, UnOp,
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

    // --- module --------------------------------------------------------------------

    fn parse_module(&mut self) -> Result<ModuleAst, FrontendError> {
        self.skip_directives();
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

        let range = if self.at(&Token::From) || self.at(&Token::Exclude) {
            let exclude = self.at(&Token::Exclude);
            self.pos += 1;
            // Only parenthesised (exclusive) bounds are lexable in v0.
            self.eat(&Token::LParen)?;
            let lo = self.parse_expr()?;
            self.eat(&Token::Colon)?;
            let hi = self.parse_expr()?;
            self.eat(&Token::RParen)?;
            Some(Range {
                lo,
                hi,
                lo_inclusive: false,
                hi_inclusive: false,
                exclude,
            })
        } else {
            None
        };
        self.eat(&Token::Semicolon)?;
        Ok(Item::Param {
            ty,
            name,
            default,
            range,
        })
    }

    // --- statements ----------------------------------------------------------------

    /// Parse either a `begin … end` block (returning its statements) or a single statement
    /// (returning a one-element list). Normalises both `if` arms and the analog block.
    fn parse_block_or_single(&mut self) -> Result<Vec<Stmt>, FrontendError> {
        if self.at(&Token::Begin) {
            self.pos += 1;
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
            Some(Token::Keyword(kw)) => self.err(format!(
                "reserved word `{}` begins a construct outside the v0 subset",
                kw.as_str()
            )),
            _ => self.err("expected a statement".to_string()),
        }
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
        self.parse_bin(0)
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
        Ok(self.push(ExprAst::Call { name, args }))
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
    fn reserved_word_as_identifier_is_rejected() {
        // `time` is a reserved word and may not name a net.
        let toks = lex("module t(); electrical time; analog begin end endmodule").expect("lex");
        assert!(parse(&toks).is_err());
    }

    #[test]
    fn missing_semicolon_is_an_error() {
        let toks = lex("module t(); parameter real X = 1 endmodule").expect("lex");
        assert!(parse(&toks).is_err());
    }
}
