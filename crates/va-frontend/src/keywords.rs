//! The Verilog-A/AMS reserved-word set (LRM Annex D).
//!
//! Every word in [`RESERVED_WORDS`] is reserved: it can never be a user identifier. The
//! [`crate::lexer`] surfaces the structural keywords the grammar consumes directly
//! (`module`, `analog`, `if`, …) as their own [`crate::lexer::Token`] variants; every other
//! reserved word — the math/analog built-ins, gate primitives, and constructs outside the v0
//! subset — is carried generically as [`crate::lexer::Token::Keyword`] with a [`Keyword`]
//! payload. Built-ins (`exp`, `ddt`, `idt`, …) are then routed to ordinary call expressions
//! by the parser, so elaboration keeps classifying them by name.
//!
//! # Case sensitivity (LRM §2)
//!
//! Reserved words are recognised **only in lowercase**. `Exp` and `EXP` are ordinary
//! identifiers; only `exp` is a keyword. [`Keyword::from_ident`] matches exactly.
//!
//! # Note on the count
//!
//! The source document's prose states 166 reserved words, but its table lists 169 distinct
//! lowercase words. We recognise all 169 listed words, plus `aliasparam` and `genvar`: the
//! Annex D table omits both, but each is a real grammar production (`aliasparam_declaration`,
//! `genvar_declaration`) seen in the compact-model corpus (`genvar i;` guards a hand-unrolled
//! generate loop in more than one zoo model). A lexer that reserves a superset is conservative
//! (it only forbids a few extra identifiers) and none of the surplus words collide with the
//! model zoo.

/// A Verilog-A/AMS reserved word carried generically by the lexer.
///
/// Constructed only from the [`RESERVED_WORDS`] table via [`Keyword::from_ident`], so a
/// `Keyword` always refers to a real reserved word with a `'static` spelling.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Keyword(&'static str);

impl Keyword {
    /// The canonical lowercase spelling of the reserved word.
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// Look an identifier slice up against the reserved-word table.
    ///
    /// Returns the matching [`Keyword`] when `s` is a reserved word, or `None` for an
    /// ordinary user identifier. Matching is exact and case-sensitive (LRM §2): reserved
    /// words are recognised only in lowercase.
    ///
    /// Words with a dedicated [`crate::lexer::Token`] variant (e.g. `module`) are also in the
    /// table, but the lexer matches them as their own token before this lookup runs, so they
    /// are never produced as a [`Keyword`] in practice.
    pub fn from_ident(s: &str) -> Option<Keyword> {
        RESERVED_WORDS
            .iter()
            .copied()
            .find(|&w| w == s)
            .map(Keyword)
    }
}

impl std::fmt::Display for Keyword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Every reserved word in the Verilog-A/AMS keyword list (LRM Annex D), in document order.
///
/// See the module docs for the 166-vs-169 count caveat. Words that also have a dedicated
/// [`crate::lexer::Token`] variant (`analog`, `begin`, `else`, `end`, `endmodule`,
/// `exclude`, `from`, `ground`, `if`, `inf`, `inout`, `input`, `integer`, `module`,
/// `output`, `parameter`, `real`) appear here for completeness but are tokenized directly.
pub const RESERVED_WORDS: [&str; 171] = [
    "abs",
    "abstol",
    "access",
    "acos",
    "acosh",
    "ac_stim",
    "aliasparam",
    "always",
    "analog",
    "analysis",
    "and",
    "asin",
    "asinh",
    "assign",
    "atan",
    "atan2",
    "atanh",
    "begin",
    "bound_step",
    "branch",
    "buf",
    "bufif0",
    "bufif1",
    "case",
    "casex",
    "casez",
    "cmos",
    "cos",
    "cosh",
    "cross",
    "ddt",
    "ddt_nature",
    "deassign",
    "default",
    "defparam",
    "delay",
    "disable",
    "discipline",
    "discontinuity",
    "edge",
    "else",
    "end",
    "endcase",
    "enddiscipline",
    "endfunction",
    "endmodule",
    "endnature",
    "endprimitive",
    "endspecify",
    "endtable",
    "endtask",
    "event",
    "exclude",
    "exp",
    "final_step",
    "flicker_noise",
    "flow",
    "for",
    "force",
    "forever",
    "fork",
    "from",
    "function",
    "generate",
    "genvar",
    "ground",
    "highz0",
    "highz1",
    "hypot",
    "idt",
    "idt_nature",
    "if",
    "ifnone",
    "inf",
    "initial",
    "initial_step",
    "inout",
    "input",
    "integer",
    "join",
    "laplace_nd",
    "laplace_np",
    "laplace_zd",
    "laplace_zp",
    "large",
    "last_crossing",
    "ln",
    "log",
    "macromodule",
    "max",
    "medium",
    "min",
    "module",
    "nand",
    "nature",
    "negedge",
    "nmos",
    "noise_table",
    "nor",
    "not",
    "notif0",
    "notif1",
    "or",
    "output",
    "parameter",
    "pmos",
    "posedge",
    "potential",
    "pow",
    "primitive",
    "pull0",
    "pull1",
    "pulldown",
    "pullup",
    "rcmos",
    "real",
    "realtime",
    "reg",
    "release",
    "repeat",
    "rnmos",
    "rpmos",
    "rtran",
    "rtranif0",
    "rtranif1",
    "scalared",
    "sin",
    "sinh",
    "slew",
    "small",
    "specify",
    "specparam",
    "sqrt",
    "strong0",
    "strong1",
    "supply0",
    "supply1",
    "table",
    "tan",
    "tanh",
    "task",
    "temperature",
    "time",
    "timer",
    "tran",
    "tranif0",
    "tranif1",
    "transition",
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "units",
    "vectored",
    "vt",
    "wait",
    "wand",
    "weak0",
    "weak1",
    "while",
    "white_noise",
    "wire",
    "wor",
    "xnor",
    "xor",
    "zi_nd",
    "zi_np",
    "zi_zd",
    "zi_zp",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for w in RESERVED_WORDS {
            assert!(seen.insert(w), "duplicate reserved word `{w}`");
        }
        assert_eq!(seen.len(), RESERVED_WORDS.len());
    }

    #[test]
    fn from_ident_is_case_sensitive() {
        assert_eq!(Keyword::from_ident("exp").map(|k| k.as_str()), Some("exp"));
        assert!(Keyword::from_ident("Exp").is_none());
        assert!(Keyword::from_ident("EXP").is_none());
        assert!(Keyword::from_ident("expx").is_none());
        // An ordinary user identifier is not reserved.
        assert!(Keyword::from_ident("anode").is_none());
    }
}
