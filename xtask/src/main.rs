//! `xtask` — project dev automation, invoked as `cargo xtask <subcommand>`.
//!
//! Subcommands:
//! - `validate`   — run `va-harness` over the model zoo and compare to `golden/`.
//! - `gen-golden` — (re)generate golden outputs from ngspice, if installed.
//! - `tutorials`  — render the Quarto developer-tutorial book (`--preview` to live-edit).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next();
    let rest: Vec<String> = args.collect();
    match cmd.as_deref() {
        Some("validate") => validate(),
        Some("gen-golden") => gen_golden(),
        Some("tutorials") => tutorials(&rest),
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
         validate            Run va-harness over the model zoo vs golden/\n    \
         gen-golden          (Re)generate golden outputs from ngspice, if installed\n    \
         tutorials [--preview]  Render the Quarto developer-tutorial book (docs/tutorials/)"
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

/// Render (or preview) the Quarto developer-tutorial book under `docs/tutorials/`.
///
/// Pass `--preview` (or `-p`) for a live-reloading preview while writing; otherwise the book
/// is rendered once into `target/tutorials/`. Requires the Quarto CLI on `PATH`; this is a
/// dev convenience and is not part of the `cargo build`/`test` flow.
fn tutorials(args: &[String]) -> Result<()> {
    let preview = args.iter().any(|a| a == "--preview" || a == "-p");
    if let Some(unknown) = args
        .iter()
        .find(|a| !matches!(a.as_str(), "--preview" | "-p"))
    {
        bail!("tutorials: unknown argument `{unknown}` (expected `--preview`)");
    }

    let dir = tutorials_dir()?;
    let action = if preview { "preview" } else { "render" };
    eprintln!("[xtask] tutorials: quarto {action} {} …", dir.display());

    // Invoke the native `quarto` launcher (resolves to `quarto.exe` on Windows). We
    // deliberately do not target the `quarto.cmd` batch wrapper: launching a `.cmd`
    // directly mangles install paths containing spaces (e.g. `C:\Program Files\Quarto`).
    let status = Command::new("quarto")
        .arg(action)
        .current_dir(&dir)
        .status()
        .with_context(|| {
            "failed to launch `quarto` — is the Quarto CLI installed and on PATH?\n\
             Install it from https://quarto.org/docs/get-started/"
                .to_string()
        })?;

    if !status.success() {
        bail!("quarto {action} failed with {status}");
    }
    if !preview {
        eprintln!("[xtask] tutorials: rendered into target/tutorials/");
    }
    Ok(())
}

/// Absolute path to `docs/tutorials/`, derived from this crate's manifest dir so it works
/// regardless of the caller's current directory.
fn tutorials_dir() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR is `<workspace>/xtask`; the workspace root is its parent.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .context("xtask manifest has no parent directory")?;
    let dir = root.join("docs").join("tutorials");
    if !dir.join("_quarto.yml").is_file() {
        bail!(
            "expected a Quarto project at {} (missing _quarto.yml)",
            dir.display()
        );
    }
    Ok(dir)
}
