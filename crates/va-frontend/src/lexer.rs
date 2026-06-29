//! Lexer: source text → token stream.
//!
//! Backed by [`logos`]. The lexer skips whitespace and comments, recognises the subset of
//! Verilog-A lexemes the bring-up model zoo needs (single-module compact models), and maps
//! numeric literals — including scientific notation and SI scale suffixes — to `f64`.
//!
//! # Scope (§1)
//!
//! This covers the declared subset: module/analog structure, parameter and net
//! declarations, `<+` contributions, branch accesses, comparison/boolean operators for
//! `if`/`else`, system functions (`$vt`, `$temperature`), and `` `include `` directives.
//! Built-in math names (`exp`, `ln`, `ddt`, `idt`, …) and access functions (`V`, `I`) are
//! **not** keywords — they lex as [`Token::Ident`] and are classified later, during
//! elaboration, so they remain usable as ordinary identifiers where the grammar allows.
//!
//! # Limitations
//!
//! - `` `include ``/`` `define `` are emitted as [`Token::Directive`] tokens, not expanded:
//!   v0 has no preprocessor, and the standard disciplines/constants are built into
//!   elaboration. A later stage may consume or ignore them.
//! - Numeric literals require a leading digit (`0.5`, not `.5`), matching common Verilog-A
//!   usage. Sized/based integer literals (`4'b0101`) are out of scope.

use crate::FrontendError;
use logos::Logos;

/// A lexical token.
///
/// Keywords, operators, and punctuation each have their own variant so the parser can match
/// on shape rather than re-comparing strings. Identifiers, numbers, strings, system
/// functions, and directives carry their payload.
#[derive(Clone, Debug, PartialEq, Logos)]
#[logos(skip r"[ \t\r\n\f]+")] // whitespace
#[logos(skip r"//[^\n]*")] // line comment
#[logos(skip r"/\*[^*]*\*+([^/*][^*]*\*+)*/")] // block comment
pub enum Token {
    // --- literals & names -------------------------------------------------------------
    /// An identifier (also covers built-in math/access names, classified later).
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    /// A numeric literal, already scaled to its `f64` value.
    #[regex(r"[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?[TGMKkmunpfa]?", parse_number)]
    Number(f64),

    /// A double-quoted string literal, with the surrounding quotes stripped.
    #[regex(r#""[^"]*""#, |lex| { let s = lex.slice(); s[1..s.len() - 1].to_string() })]
    Str(String),

    /// A system function/task name, with the leading `$` stripped (e.g. `vt`, `temperature`).
    #[regex(r"\$[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice()[1..].to_string())]
    SysFunc(String),

    /// A compiler directive name, with the leading backtick stripped (e.g. `include`).
    #[regex(r"`[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice()[1..].to_string())]
    Directive(String),

    // --- keywords ---------------------------------------------------------------------
    /// `module`.
    #[token("module")]
    Module,
    /// `endmodule`.
    #[token("endmodule")]
    EndModule,
    /// `analog`.
    #[token("analog")]
    Analog,
    /// `begin`.
    #[token("begin")]
    Begin,
    /// `end`.
    #[token("end")]
    End,
    /// `parameter`.
    #[token("parameter")]
    Parameter,
    /// `localparam`.
    #[token("localparam")]
    LocalParam,
    /// `real`.
    #[token("real")]
    Real,
    /// `integer`.
    #[token("integer")]
    Integer,
    /// `input`.
    #[token("input")]
    Input,
    /// `output`.
    #[token("output")]
    Output,
    /// `inout`.
    #[token("inout")]
    Inout,
    /// `electrical`.
    #[token("electrical")]
    Electrical,
    /// `thermal`.
    #[token("thermal")]
    Thermal,
    /// `ground`.
    #[token("ground")]
    Ground,
    /// `if`.
    #[token("if")]
    If,
    /// `else`.
    #[token("else")]
    Else,
    /// `from`.
    #[token("from")]
    From,
    /// `exclude`.
    #[token("exclude")]
    Exclude,
    /// `inf` (used in parameter ranges).
    #[token("inf")]
    Inf,

    // --- operators --------------------------------------------------------------------
    /// `<+`, the analog contribution operator.
    #[token("<+")]
    Contribute,
    /// `=`.
    #[token("=")]
    Assign,
    /// `+`.
    #[token("+")]
    Plus,
    /// `-`.
    #[token("-")]
    Minus,
    /// `*`.
    #[token("*")]
    Star,
    /// `**`, exponentiation.
    #[token("**")]
    StarStar,
    /// `/`.
    #[token("/")]
    Slash,
    /// `==`.
    #[token("==")]
    EqEq,
    /// `!=`.
    #[token("!=")]
    NotEq,
    /// `<=`.
    #[token("<=")]
    Le,
    /// `<`.
    #[token("<")]
    Lt,
    /// `>=`.
    #[token(">=")]
    Ge,
    /// `>`.
    #[token(">")]
    Gt,
    /// `!`.
    #[token("!")]
    Not,
    /// `&&`.
    #[token("&&")]
    AndAnd,
    /// `||`.
    #[token("||")]
    OrOr,

    // --- punctuation ------------------------------------------------------------------
    /// `(`.
    #[token("(")]
    LParen,
    /// `)`.
    #[token(")")]
    RParen,
    /// `,`.
    #[token(",")]
    Comma,
    /// `;`.
    #[token(";")]
    Semicolon,
    /// `:`.
    #[token(":")]
    Colon,
    /// `.`.
    #[token(".")]
    Dot,
}

/// Parse a numeric-literal slice into its scaled `f64` value.
///
/// Handles an optional trailing SI scale suffix (`T G M K k m u n p f a`); note `M`
/// (mega, `1e6`) and `m` (milli, `1e-3`) are case-sensitive. Returns `None` (a lex error)
/// if the digit portion does not parse as a float.
fn parse_number(lex: &logos::Lexer<Token>) -> Option<f64> {
    let s = lex.slice();
    let (digits, scale) = match s.as_bytes()[s.len() - 1] {
        b'T' => (&s[..s.len() - 1], 1e12),
        b'G' => (&s[..s.len() - 1], 1e9),
        b'M' => (&s[..s.len() - 1], 1e6),
        b'K' | b'k' => (&s[..s.len() - 1], 1e3),
        b'm' => (&s[..s.len() - 1], 1e-3),
        b'u' => (&s[..s.len() - 1], 1e-6),
        b'n' => (&s[..s.len() - 1], 1e-9),
        b'p' => (&s[..s.len() - 1], 1e-12),
        b'f' => (&s[..s.len() - 1], 1e-15),
        b'a' => (&s[..s.len() - 1], 1e-18),
        _ => (s, 1.0),
    };
    digits.parse::<f64>().ok().map(|v| v * scale)
}

/// Tokenize `source` into a flat token vector.
///
/// Whitespace and comments are discarded. Directives are preserved as [`Token::Directive`]
/// (plus their string argument as [`Token::Str`]) for a later stage to consume or ignore.
///
/// # Errors
///
/// Returns [`FrontendError::Lex`] at the first character the lexer cannot tokenize, with the
/// byte offset of the offending span.
pub fn lex(source: &str) -> Result<Vec<Token>, FrontendError> {
    let mut tokens = Vec::new();
    let mut lexer = Token::lexer(source);
    while let Some(result) = lexer.next() {
        match result {
            Ok(token) => tokens.push(token),
            Err(()) => {
                let span = lexer.span();
                return Err(FrontendError::Lex {
                    offset: span.start,
                    message: format!("unexpected input {:?}", lexer.slice()),
                });
            }
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_ok(src: &str) -> Vec<Token> {
        lex(src).expect("should lex")
    }

    #[test]
    fn contribution_is_one_token() {
        // `<+` must beat `<` and `<=`; longest-match handling.
        assert_eq!(
            lex_ok("I <+ V"),
            vec![
                Token::Ident("I".into()),
                Token::Contribute,
                Token::Ident("V".into()),
            ]
        );
    }

    #[test]
    fn keywords_versus_identifiers() {
        // `module` is a keyword; `modules` (longer) is an identifier.
        assert_eq!(
            lex_ok("module modules"),
            vec![Token::Module, Token::Ident("modules".into()),]
        );
    }

    #[test]
    fn numbers_scientific_and_scaled() {
        assert_eq!(lex_ok("1000.0"), vec![Token::Number(1000.0)]);
        assert_eq!(lex_ok("1e-14"), vec![Token::Number(1e-14)]);
        assert_eq!(lex_ok("0"), vec![Token::Number(0.0)]);
        // SI scale suffixes, case-sensitive (M = mega, m = milli).
        assert_eq!(lex_ok("2k"), vec![Token::Number(2000.0)]);
        assert_eq!(lex_ok("1p"), vec![Token::Number(1e-12)]);
        assert_eq!(lex_ok("5m"), vec![Token::Number(5e-3)]);
    }

    #[test]
    fn system_function_and_directive() {
        assert_eq!(lex_ok("$vt"), vec![Token::SysFunc("vt".into())]);
        assert_eq!(
            lex_ok(r#"`include "disciplines.vams""#),
            vec![
                Token::Directive("include".into()),
                Token::Str("disciplines.vams".into()),
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            lex_ok("R // a line comment\n+ /* block */ C"),
            vec![
                Token::Ident("R".into()),
                Token::Plus,
                Token::Ident("C".into()),
            ]
        );
    }

    #[test]
    fn resistor_contribution_statement() {
        let toks = lex_ok("I(p, n) <+ V(p, n) / R;");
        assert_eq!(
            toks,
            vec![
                Token::Ident("I".into()),
                Token::LParen,
                Token::Ident("p".into()),
                Token::Comma,
                Token::Ident("n".into()),
                Token::RParen,
                Token::Contribute,
                Token::Ident("V".into()),
                Token::LParen,
                Token::Ident("p".into()),
                Token::Comma,
                Token::Ident("n".into()),
                Token::RParen,
                Token::Slash,
                Token::Ident("R".into()),
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn whole_models_lex_without_error() {
        for src in [
            include_str!("../../../models/resistor.va"),
            include_str!("../../../models/capacitor.va"),
            include_str!("../../../models/diode.va"),
        ] {
            assert!(lex(src).is_ok(), "model should lex cleanly");
        }
    }

    #[test]
    fn unexpected_character_reports_offset() {
        let err = lex("R = #").unwrap_err();
        match err {
            FrontendError::Lex { offset, .. } => assert_eq!(offset, 4),
            other => panic!("expected lex error, got {other:?}"),
        }
    }
}
