//! `xtask` — project dev automation, invoked as `cargo xtask <subcommand>`.
//!
//! Subcommands:
//! - `validate`   — run `va-harness` over the model zoo and compare to `golden/`.
//! - `gen-golden` — (re)generate golden outputs from ngspice, if installed.

use anyhow::{bail, Result};

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("validate") => validate(),
        Some("gen-golden") => gen_golden(),
        Some("--help") | Some("-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            bail!("unknown xtask `{other}`");
        }
    }
}

fn print_usage() {
    eprintln!(
        "cargo xtask <subcommand>\n\n\
         SUBCOMMANDS:\n    \
         validate      Run va-harness over the model zoo vs golden/\n    \
         gen-golden    (Re)generate golden outputs from ngspice, if installed"
    );
}

/// Run the validation harness over every zoo circuit and report pass/fail.
fn validate() -> Result<()> {
    eprintln!("[xtask] validate: running va-harness over the model zoo vs golden/ …");
    // Skeleton: enumerate circuits/, drive va-cli's pipeline via va-harness, compare to
    // golden/, and aggregate verdicts. Implemented alongside the T6 harness milestone.
    todo!("xtask: drive va-harness over circuits/ and aggregate verdicts")
}

/// (Re)generate golden reference outputs by invoking ngspice, if it is on PATH.
fn gen_golden() -> Result<()> {
    eprintln!("[xtask] gen-golden: regenerating golden/ from ngspice …");
    // Skeleton: for each circuit, run ngspice in batch mode and write its output into
    // golden/. No-op with a clear message if ngspice is not installed.
    todo!("xtask: shell out to ngspice and capture golden outputs")
}
