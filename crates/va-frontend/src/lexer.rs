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
//!
//! # Reserved words
//!
//! All 166-ish Verilog-A/AMS reserved words (LRM Annex D, see [`crate::keywords`]) are
//! recognised — only in lowercase. The structural keywords the grammar consumes directly
//! (`module`, `analog`, `if`, …) have dedicated [`Token`] variants; every other reserved
//! word, including the math/analog built-ins (`exp`, `ln`, `ddt`, `idt`, …), is carried as
//! [`Token::Keyword`]. The parser routes the built-ins to call expressions, so elaboration
//! still classifies them by name. Access functions `V`/`I` are **not** reserved words: they
//! lex as [`Token::Ident`] and are recognised contextually.
//!
//! # Limitations
//!
//! - `` `include ``/`` `define `` are emitted as [`Token::Directive`] tokens, not expanded:
//!   v0 has no preprocessor, and the standard disciplines/constants are built into
//!   elaboration. A later stage may consume or ignore them.
//! - Numeric literals require a leading digit (`0.5`, not `.5`), matching common Verilog-A
//!   usage. Sized/based integer literals (`4'b0101`) are out of scope.
//! - Attribute instances `(* … *)` are skipped entirely (treated like a comment); their
//!   metadata (`units`, `desc`, …) is not retained.

use crate::keywords::Keyword;
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
#[logos(skip r"\(\*[^*]*\*+([^)*][^*]*\*+)*\)")] // (* attribute *) — metadata, discarded
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
    /// `genvar` — a generate-loop index. v0 does not unroll `generate` blocks, so a `genvar`
    /// declaration is lowered like a bare `integer` (see [`crate::parser`]).
    #[token("genvar")]
    Genvar,
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

    // --- reserved words without a dedicated token ------------------------------------
    /// Any Verilog-A/AMS reserved word that the grammar does not consume through one of the
    /// dedicated variants above (LRM Annex D; see [`crate::keywords`]).
    ///
    /// This covers the math/analog built-ins (`exp`, `ln`, `ddt`, `idt`, …) — which the
    /// parser routes to call expressions — plus gate primitives and constructs outside the
    /// v0 subset. The `kw` callback maps the matched lexeme back to its [`Keyword`].
    #[token("abs", kw)]
    #[token("abstol", kw)]
    #[token("access", kw)]
    #[token("acos", kw)]
    #[token("acosh", kw)]
    #[token("ac_stim", kw)]
    #[token("aliasparam", kw)]
    #[token("always", kw)]
    #[token("analysis", kw)]
    #[token("and", kw)]
    #[token("asin", kw)]
    #[token("asinh", kw)]
    #[token("assign", kw)]
    #[token("atan", kw)]
    #[token("atan2", kw)]
    #[token("atanh", kw)]
    #[token("bound_step", kw)]
    #[token("branch", kw)]
    #[token("buf", kw)]
    #[token("bufif0", kw)]
    #[token("bufif1", kw)]
    #[token("case", kw)]
    #[token("casex", kw)]
    #[token("casez", kw)]
    #[token("ceil", kw)]
    #[token("cmos", kw)]
    #[token("continuous", kw)]
    #[token("cos", kw)]
    #[token("cosh", kw)]
    #[token("cross", kw)]
    #[token("ddt", kw)]
    #[token("ddt_nature", kw)]
    #[token("ddx", kw)]
    #[token("deassign", kw)]
    #[token("default", kw)]
    #[token("defparam", kw)]
    #[token("delay", kw)]
    #[token("disable", kw)]
    #[token("discipline", kw)]
    #[token("discontinuity", kw)]
    #[token("discrete", kw)]
    #[token("domain", kw)]
    #[token("edge", kw)]
    #[token("endcase", kw)]
    #[token("enddiscipline", kw)]
    #[token("endfunction", kw)]
    #[token("endgenerate", kw)]
    #[token("endnature", kw)]
    #[token("endprimitive", kw)]
    #[token("endspecify", kw)]
    #[token("endtable", kw)]
    #[token("endtask", kw)]
    #[token("event", kw)]
    #[token("exp", kw)]
    #[token("final_step", kw)]
    #[token("flicker_noise", kw)]
    #[token("floor", kw)]
    #[token("flow", kw)]
    #[token("for", kw)]
    #[token("force", kw)]
    #[token("forever", kw)]
    #[token("fork", kw)]
    #[token("function", kw)]
    #[token("generate", kw)]
    #[token("highz0", kw)]
    #[token("highz1", kw)]
    #[token("hypot", kw)]
    #[token("idt", kw)]
    #[token("idt_nature", kw)]
    #[token("ifnone", kw)]
    #[token("initial", kw)]
    #[token("initial_step", kw)]
    #[token("int", kw)]
    #[token("join", kw)]
    #[token("laplace_nd", kw)]
    #[token("laplace_np", kw)]
    #[token("laplace_zd", kw)]
    #[token("laplace_zp", kw)]
    #[token("large", kw)]
    #[token("last_crossing", kw)]
    #[token("limexp", kw)]
    #[token("ln", kw)]
    #[token("log", kw)]
    #[token("macromodule", kw)]
    #[token("max", kw)]
    #[token("medium", kw)]
    #[token("min", kw)]
    #[token("nand", kw)]
    #[token("nature", kw)]
    #[token("negedge", kw)]
    #[token("nmos", kw)]
    #[token("noise_table", kw)]
    #[token("nor", kw)]
    #[token("not", kw)]
    #[token("notif0", kw)]
    #[token("notif1", kw)]
    #[token("or", kw)]
    #[token("pmos", kw)]
    #[token("posedge", kw)]
    #[token("potential", kw)]
    #[token("pow", kw)]
    #[token("primitive", kw)]
    #[token("pull0", kw)]
    #[token("pull1", kw)]
    #[token("pulldown", kw)]
    #[token("pullup", kw)]
    #[token("rcmos", kw)]
    #[token("realtime", kw)]
    #[token("reg", kw)]
    #[token("release", kw)]
    #[token("repeat", kw)]
    #[token("rnmos", kw)]
    #[token("round", kw)]
    #[token("rpmos", kw)]
    #[token("rtran", kw)]
    #[token("rtranif0", kw)]
    #[token("rtranif1", kw)]
    #[token("scalared", kw)]
    #[token("sin", kw)]
    #[token("sinh", kw)]
    #[token("slew", kw)]
    #[token("small", kw)]
    #[token("specify", kw)]
    #[token("specparam", kw)]
    #[token("sqrt", kw)]
    #[token("strong0", kw)]
    #[token("strong1", kw)]
    #[token("supply0", kw)]
    #[token("supply1", kw)]
    #[token("table", kw)]
    #[token("tan", kw)]
    #[token("tanh", kw)]
    #[token("task", kw)]
    #[token("time", kw)]
    #[token("timer", kw)]
    #[token("tran", kw)]
    #[token("tranif0", kw)]
    #[token("tranif1", kw)]
    #[token("transition", kw)]
    #[token("tri", kw)]
    #[token("tri0", kw)]
    #[token("tri1", kw)]
    #[token("triand", kw)]
    #[token("trior", kw)]
    #[token("trireg", kw)]
    #[token("units", kw)]
    #[token("vectored", kw)]
    #[token("wait", kw)]
    #[token("wand", kw)]
    #[token("weak0", kw)]
    #[token("weak1", kw)]
    #[token("while", kw)]
    #[token("white_noise", kw)]
    #[token("wire", kw)]
    #[token("wor", kw)]
    #[token("xnor", kw)]
    #[token("xor", kw)]
    #[token("zi_nd", kw)]
    #[token("zi_np", kw)]
    #[token("zi_zd", kw)]
    #[token("zi_zp", kw)]
    Keyword(Keyword),

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
    /// `%`, modulus.
    #[token("%")]
    Percent,
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
    /// `<<`, left shift.
    #[token("<<")]
    Shl,
    /// `>>`, right shift.
    #[token(">>")]
    Shr,
    /// `&`, bitwise AND (distinct from `&&`, logical AND).
    #[token("&")]
    Amp,
    /// `|`, bitwise OR (distinct from `||`, logical OR).
    #[token("|")]
    Pipe,
    /// `^`, bitwise XOR.
    #[token("^")]
    Caret,
    /// `^~` or `~^`, bitwise XNOR (both spellings are the same operator).
    #[token("^~")]
    #[token("~^")]
    CaretTilde,
    /// `~`, bitwise NOT (unary; distinct from `!`, logical NOT).
    #[token("~")]
    Tilde,

    // --- punctuation ------------------------------------------------------------------
    /// `(`.
    #[token("(")]
    LParen,
    /// `)`.
    #[token(")")]
    RParen,
    /// `[` — opens an inclusive parameter range bound.
    #[token("[")]
    LBracket,
    /// `]` — closes an inclusive parameter range bound.
    #[token("]")]
    RBracket,
    /// `{` — opens an array-literal expression, `{1, 2, 3}` (a `laplace_nd`-style
    /// coefficient-list argument).
    #[token("{")]
    LBrace,
    /// `}` — closes an array-literal expression.
    #[token("}")]
    RBrace,
    /// `@` — opens an event-control expression, `@(initial_step)`.
    #[token("@")]
    At,
    /// `?` — the ternary conditional operator, `cond ? a : b`.
    #[token("?")]
    Question,
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
    /// `#` — opens an instance parameter-override list, `#(.name(value), ...)` (LRM Annex
    /// C.8's module-instantiation syntax).
    #[token("#")]
    Hash,
}

/// Map a matched reserved-word lexeme to its [`Keyword`].
///
/// The slice is always one of the reserved words declared on [`Token::Keyword`], so the
/// table lookup in [`Keyword::from_ident`] never fails.
fn kw(lex: &logos::Lexer<Token>) -> Keyword {
    Keyword::from_ident(lex.slice()).expect("matched lexeme is a reserved word")
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
    fn domain_discrete_continuous_lex_as_keywords() {
        assert_eq!(
            lex_ok("domain discrete continuous"),
            vec![
                Token::Keyword(crate::keywords::Keyword::from_ident("domain").unwrap()),
                Token::Keyword(crate::keywords::Keyword::from_ident("discrete").unwrap()),
                Token::Keyword(crate::keywords::Keyword::from_ident("continuous").unwrap()),
            ]
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
    fn reserved_words_lex_as_keyword_not_ident() {
        use crate::keywords::Keyword;
        // Math/analog built-ins and other reserved words carry a Keyword payload.
        for word in ["exp", "ddt", "idt", "branch", "analysis", "white_noise"] {
            assert_eq!(
                lex_ok(word),
                vec![Token::Keyword(Keyword::from_ident(word).unwrap())],
                "`{word}` should lex as a reserved word"
            );
        }
    }

    #[test]
    fn genvar_is_its_own_token() {
        assert_eq!(
            lex_ok("genvar i;"),
            vec![Token::Genvar, Token::Ident("i".into()), Token::Semicolon]
        );
    }

    #[test]
    fn every_reserved_word_is_reserved() {
        // No reserved word (dedicated token or generic keyword) may lex as a bare identifier.
        for word in crate::keywords::RESERVED_WORDS {
            let toks = lex_ok(word);
            assert_eq!(toks.len(), 1, "`{word}` should be a single token");
            assert!(
                !matches!(toks[0], Token::Ident(_)),
                "`{word}` must not lex as an identifier"
            );
        }
    }

    #[test]
    fn attributes_are_skipped() {
        // A single attribute, an attribute with a comma/quotes, and a multi-line one.
        assert_eq!(
            lex_ok(r#"(* units="m", desc="length" *) parameter"#),
            vec![Token::Parameter]
        );
        assert_eq!(
            lex_ok("(* desc=\n  \"x*y\" *) real r"),
            vec![Token::Real, Token::Ident("r".into())]
        );
        // A real multiply is untouched (no `(*` adjacency).
        assert_eq!(
            lex_ok("(a * b)"),
            vec![
                Token::LParen,
                Token::Ident("a".into()),
                Token::Star,
                Token::Ident("b".into()),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn at_sign_lexes() {
        assert_eq!(
            lex_ok("@(initial_step)"),
            vec![
                Token::At,
                Token::LParen,
                Token::Keyword(crate::keywords::Keyword::from_ident("initial_step").unwrap()),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn hash_lexes() {
        assert_eq!(
            lex_ok("#(.gain(2.0))"),
            vec![
                Token::Hash,
                Token::LParen,
                Token::Dot,
                Token::Ident("gain".into()),
                Token::LParen,
                Token::Number(2.0),
                Token::RParen,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn brackets_lex() {
        assert_eq!(
            lex_ok("[ 0 : 1 ]"),
            vec![
                Token::LBracket,
                Token::Number(0.0),
                Token::Colon,
                Token::Number(1.0),
                Token::RBracket,
            ]
        );
    }

    #[test]
    fn reserved_words_are_lowercase_only() {
        // Case-sensitive: uppercased spellings are ordinary identifiers (LRM §2).
        assert_eq!(lex_ok("EXP"), vec![Token::Ident("EXP".into())]);
        assert_eq!(lex_ok("Branch"), vec![Token::Ident("Branch".into())]);
        // A longer word that merely starts with a keyword is an identifier.
        assert_eq!(
            lex_ok("expression"),
            vec![Token::Ident("expression".into())]
        );
    }

    #[test]
    fn unexpected_character_reports_offset() {
        // `\` (a lone backslash, not part of an escaped identifier) is still unlexable — `{`
        // used to be this test's example, but it's now a real token (§ array-literal
        // expressions, `{expr, ...}`).
        let err = lex("R = \\").unwrap_err();
        match err {
            FrontendError::Lex { offset, .. } => assert_eq!(offset, 4),
            other => panic!("expected lex error, got {other:?}"),
        }
    }
}
