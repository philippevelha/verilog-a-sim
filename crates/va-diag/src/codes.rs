//! Every diagnostic code the program can emit.
//!
//! A code is a letter and four digits. The letter is the kind:
//!
//! | Letter | Kind | Meaning |
//! |--------|------|---------|
//! | `E` | error | the input is wrong, or the run failed |
//! | `W` | warning | the run goes on, but something deserves a look |
//! | `R` | refusal | valid Verilog-A this simulator deliberately declines to run (CLAUDE.md §5) |
//! | `C` | communication | information a user may act on |
//!
//! The first two digits are the stage that raises it, the same for every letter:
//!
//! | Range | Stage |
//! |-------|-------|
//! | `00xx` | general; `E0000` is "an error with no code yet" |
//! | `01xx` | preprocessor |
//! | `02xx` | lexer and parser |
//! | `03xx` | elaboration |
//! | `04xx` | code generation |
//! | `05xx` | netlist |
//! | `06xx` | numerical (DC, transient, AC, noise) |
//! | `07xx` | files and the command line |
//!
//! **A code is permanent.** Once released it is never renumbered and never reused for a different
//! problem, because users search for it and other teams' text is keyed to it. A code whose problem
//! can no longer happen is retired: it stays here and in the catalogue, marked as retired.
//!
//! Adding a code: add the constant here and to [`ALL`], and add `messages/en/<CODE>.json`. The
//! `catalogue_is_consistent` test fails until both exist and agree.

use crate::Code;

/// An error that has no code yet: anything not converted to a coded diagnostic, and errors from
/// libraries the program does not wrap. The aim is for this to become rare; each one shown is a
/// gap in the catalogue.
pub const E0000: Code = Code::new("E0000");

/// `va-cli` was given a subcommand it does not know.
pub const E0701: Code = Code::new("E0701");

/// `va-cli check` was given no model file or directory.
pub const E0702: Code = Code::new("E0702");

/// A command-line option or environment variable has a value it does not accept.
pub const E0703: Code = Code::new("E0703");

/// Every code, so the catalogue and the code can be checked against each other.
pub const ALL: &[Code] = &[E0000, E0701, E0702, E0703];
