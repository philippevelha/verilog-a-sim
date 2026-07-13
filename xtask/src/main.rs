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

/// The single-operating-point DC circuits `validate`/`gen-golden` know how to drive (§ ladder
/// rungs 1/5) — a `.tran` deck (rungs 3/4/6) has no golden format yet (`docs/roadmap.md`'s T6.3
/// notes), so it's deliberately left off this list rather than silently mishandled. `model` is
/// `None` for a circuit solved entirely by `va-abi`'s reference primitives.
const DC_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/divider.net", None),
    ("circuits/mos_dc.net", Some("models/mosfet.va")),
];

/// The `.dc`-sweep circuits `validate`/`gen-golden` know how to drive (§ ladder rung 2).
const SWEEP_CIRCUITS: &[(&str, Option<&str>)] =
    &[("circuits/diode_iv.net", Some("models/diode.va"))];

/// Pass/fail/skip tally, shared across [`validate`]'s DC and sweep circuit passes.
#[derive(Default)]
struct Tally {
    checked: u32,
    failed: u32,
    skipped: u32,
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.checked += other.checked;
        self.failed += other.failed;
        self.skipped += other.skipped;
    }
}

/// Resolve `circuit`'s (and, if given, `model`'s) absolute path plus its expected
/// `golden/<stem>.golden` path — shared by the DC and sweep passes.
fn circuit_paths(
    root: &Path,
    circuit: &str,
    model: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>, PathBuf)> {
    let circuit_path = root.join(circuit);
    let model_path = model.map(|m| root.join(m));
    let stem = Path::new(circuit)
        .file_stem()
        .context("circuit path has no file stem")?
        .to_string_lossy()
        .into_owned();
    let golden_path = root.join("golden").join(format!("{stem}.golden"));
    Ok((circuit_path, model_path, golden_path))
}

/// Run the validation harness over every known single-operating-point DC circuit, reporting
/// pass/fail/skip per circuit. A circuit with no committed `golden/<name>.golden` is *skipped*,
/// not failed — an empty `golden/` (this project's actual state today, absent a local ngspice
/// install — see [`gen_golden`]) is a legitimate "nothing captured yet," not a build error.
fn validate_dc_circuits(root: &Path) -> Result<Tally> {
    let mut tally = Tally::default();
    for &(circuit, model) in DC_CIRCUITS {
        let (circuit_path, model_path, golden_path) = circuit_paths(root, circuit, model)?;
        if !golden_path.is_file() {
            eprintln!(
                "[xtask]   skip {circuit}: no golden reference at {}",
                golden_path.display()
            );
            tally.skipped += 1;
            continue;
        }

        let golden = va_harness::golden::GoldenDc::read(&golden_path)
            .with_context(|| format!("reading golden reference for {circuit}"))?;
        let got = va_harness::dc::run_dc(
            circuit_path.to_str().context("non-UTF8 circuit path")?,
            model_path
                .as_deref()
                .map(|p| p.to_str().context("non-UTF8 model path"))
                .transpose()?,
        )
        .with_context(|| format!("solving {circuit}"))?;
        let verdict = va_harness::dc::compare_dc(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Like [`validate_dc_circuits`], for every known `.dc`-sweep circuit (§ ladder rung 2).
fn validate_sweep_circuits(root: &Path) -> Result<Tally> {
    let mut tally = Tally::default();
    for &(circuit, model) in SWEEP_CIRCUITS {
        let (circuit_path, model_path, golden_path) = circuit_paths(root, circuit, model)?;
        if !golden_path.is_file() {
            eprintln!(
                "[xtask]   skip {circuit}: no golden reference at {}",
                golden_path.display()
            );
            tally.skipped += 1;
            continue;
        }

        let golden = va_harness::golden::GoldenSweep::read(&golden_path)
            .with_context(|| format!("reading golden reference for {circuit}"))?;
        let got = va_harness::dc::run_dc_sweep(
            circuit_path.to_str().context("non-UTF8 circuit path")?,
            model_path
                .as_deref()
                .map(|p| p.to_str().context("non-UTF8 model path"))
                .transpose()?,
        )
        .with_context(|| format!("solving {circuit}"))?;
        let verdict = va_harness::dc::compare_dc_sweep(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Print one circuit's PASS/FAIL line and fold it into `tally`.
fn report_verdict(circuit: &str, verdict: va_harness::Verdict, tally: &mut Tally) {
    tally.checked += 1;
    if verdict.passed {
        eprintln!(
            "[xtask]   PASS {circuit}: error={:.3e} (tol {:.0e})",
            verdict.error, verdict.tol
        );
    } else {
        eprintln!(
            "[xtask]   FAIL {circuit}: error={:.3e} exceeds tol {:.0e}",
            verdict.error, verdict.tol
        );
        tally.failed += 1;
    }
}

/// Run the validation harness over every known circuit (DC and `.dc`-sweep) and report
/// pass/fail/skip.
///
/// # Errors
///
/// If any circuit that *does* have a golden reference fails to solve, or diverges from it
/// beyond `va_harness::tol::DC_REL`.
fn validate() -> Result<()> {
    eprintln!("[xtask] validate: running va-harness over the model zoo vs golden/ …");
    let root = workspace_root()?;

    let mut tally = validate_dc_circuits(&root)?;
    tally.merge(validate_sweep_circuits(&root)?);

    eprintln!(
        "[xtask] validate: {} checked, {} failed, {} skipped (no golden)",
        tally.checked, tally.failed, tally.skipped
    );
    if tally.failed > 0 {
        bail!("{} circuit(s) failed golden comparison", tally.failed);
    }
    Ok(())
}

/// (Re)generate golden reference outputs by invoking ngspice, if it is on PATH.
///
/// # Errors
///
/// Always, in this environment: ngspice isn't installed here (confirmed — there is nothing to
/// shell out to), and even when it is, this subcommand doesn't yet translate a `circuits/*.net`
/// deck into an ngspice-compatible one and invoke it (`docs/roadmap.md`'s T6.3 notes) — a real,
/// but not-yet-written, next step, reported honestly rather than as a silent no-op.
fn gen_golden() -> Result<()> {
    eprintln!("[xtask] gen-golden: regenerating golden/ from ngspice …");
    if which_ngspice().is_none() {
        bail!(
            "ngspice not found on PATH — install it (https://ngspice.sourceforge.io/) to \
             regenerate golden/ references. `cargo xtask validate` still works against \
             whatever's already committed there."
        );
    }
    bail!(
        "ngspice was found, but this subcommand doesn't yet translate a `circuits/*.net` deck \
         into an ngspice-compatible one and invoke it — not implemented yet \
         (docs/roadmap.md's T6.3 notes)"
    );
}

/// Locate `ngspice`/`ngspice.exe` on `PATH`, without relying on the shell to resolve it (so
/// [`gen_golden`] can give an accurate diagnosis either way, not just propagate whatever error
/// `Command::new("ngspice").status()` would raise).
fn which_ngspice() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        "ngspice.exe"
    } else {
        "ngspice"
    };
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(exe_name);
        candidate.is_file().then_some(candidate)
    })
}

/// Absolute path to the workspace root, derived from this crate's manifest dir so every
/// subcommand works regardless of the caller's current directory.
fn workspace_root() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR is `<workspace>/xtask`; the workspace root is its parent.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .context("xtask manifest has no parent directory")
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
    let dir = workspace_root()?.join("docs").join("tutorials");
    if !dir.join("_quarto.yml").is_file() {
        bail!(
            "expected a Quarto project at {} (missing _quarto.yml)",
            dir.display()
        );
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_resolves_to_the_real_workspace() {
        let root = workspace_root().expect("workspace root");
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("circuits").is_dir());
        assert!(root.join("golden").is_dir());
    }

    #[test]
    fn validate_passes_with_no_golden_present() {
        // The project's actual current state: `golden/` has no `.golden` files committed yet
        // (no ngspice available to generate them from) — `validate` must treat that as "nothing
        // to check yet," not a failure.
        validate().expect("validate should pass when every circuit is merely skipped");
    }

    #[test]
    fn gen_golden_reports_a_clear_error_without_ngspice() {
        // This environment has no ngspice installed — confirmed manually, not assumed. Whatever
        // the reason, `gen_golden` must return a real `Err` (never panic) with an actionable
        // message, not silently succeed having done nothing.
        let err =
            gen_golden().expect_err("gen_golden should fail without ngspice or a deck translator");
        assert!(!err.to_string().is_empty());
    }
}
