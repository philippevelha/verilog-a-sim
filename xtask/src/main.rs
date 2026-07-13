//! `xtask` — project dev automation, invoked as `cargo xtask <subcommand>`.
//!
//! Subcommands:
//! - `validate`   — run `va-harness` over the model zoo and compare to `golden/`.
//! - `gen-golden` — (re)generate golden outputs from QSPICE, if installed.
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
         gen-golden          (Re)generate golden outputs from QSPICE, if installed\n    \
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
/// not failed — most of `golden/` (see [`gen_golden`]) is still empty, and that's a legitimate
/// "nothing captured yet," not a build error.
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

/// The circuits `gen_golden` can currently regenerate for real: pure `R`/`C`/`V` decks with no
/// custom Verilog-A model and no temperature-sensitive nonlinearity, so QSPICE's own built-in
/// primitives reproduce this project's answer with zero translation and zero ambiguity —
/// confirmed empirically, not assumed (`circuits/divider.net` run through QSPICE unmodified
/// gives `V(mid)=0.5` exactly, bit-for-bit matching the analytic/computed value).
///
/// `circuits/mos_dc.net`/`diode_iv.net` are deliberately **not** here yet: both need a custom
/// `.va` model (`models/mosfet.va`/`diode.va`) translated into an equivalent QSPICE-native
/// `.model` card to cross-check against at all, and — found while investigating exactly that —
/// QSPICE's default simulation temperature (27°C = 300.15 K) differs from this project's own
/// fixed convention (`va_codegen::TEMP = 300.0`, `VT = 0.025_852`) by 0.15 K. For a linear
/// circuit that's irrelevant; for `diode.va`'s exponential I–V law it isn't — a forced 0.5 V
/// diode measured `2.50974869898304e-6` A from this project's own model (fixed 300 K) against
/// `2.48560822992004e-6` A from QSPICE's default-temperature native diode model, a ~0.85%
/// relative difference — comfortably past `va_harness::tol::DC_REL` (`1e-4`). Forcing QSPICE's
/// `.temp` to exactly 300 K does **not** fix this: SPICE diode models rescale `IS` relative to
/// their own nominal temperature (`TNOM`, defaulting to `27°C`) whenever `.temp` differs from
/// it, so overriding `.temp` away from QSPICE's implicit `TNOM=27°C` moves the answer *further*
/// away, not closer (confirmed empirically: implied `Vt` at `.temp 26.85` was `0.0258827`, not
/// the expected `0.0258520`). Closing this needs a real decision — likely aligning this
/// project's own fixed thermal-voltage constants to the 300.15 K SPICE-standard convention,
/// which touches `va-codegen`'s `VT`/`TEMP` constants and every test that hardcodes a value
/// derived from them — not attempted here.
const QSPICE_NATIVE_CIRCUITS: &[&str] = &["circuits/divider.net"];

/// (Re)generate golden reference outputs by invoking QSPICE, if it is installed.
///
/// # Errors
///
/// If QSPICE can't be found ([`find_qspice`]), or a circuit in [`QSPICE_NATIVE_CIRCUITS`] fails
/// to run or its `.qraw` output fails to parse.
fn gen_golden() -> Result<()> {
    eprintln!("[xtask] gen-golden: regenerating golden/ from QSPICE …");
    let qspice = find_qspice().context(
        "QSPICE64.exe not found — set QSPICE_PATH, add it to PATH, or install it to the \
         standard location (C:\\Program Files\\QSPICE\\QSPICE64.exe): https://qspice.com",
    )?;
    eprintln!("[xtask]   using {}", qspice.display());
    let root = workspace_root()?;
    let golden_dir = root.join("golden");
    let tmp = std::env::temp_dir().join("va_xtask_gen_golden");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).context("creating a scratch dir for the QSPICE run")?;

    let mut generated = 0u32;
    for &circuit in QSPICE_NATIVE_CIRCUITS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();

        let raw = run_qspice_op(&qspice, &deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit}"))?;
        let golden = golden_dc_from_qraw(&raw, &net.node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} node(s))",
            golden_path.display(),
            golden.node_order.len()
        );
        generated += 1;
    }
    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!(
        "[xtask] gen-golden: {generated} circuit(s) regenerated from QSPICE (of {} known — the \
         rest need a native-model translation and/or a temperature-convention fix, see \
         QSPICE_NATIVE_CIRCUITS's doc comment)",
        DC_CIRCUITS.len() + SWEEP_CIRCUITS.len()
    );
    Ok(())
}

/// Run `deck` (a `.op`/no-sweep-`.dc` netlist, already confirmed QSPICE-native — see
/// [`QSPICE_NATIVE_CIRCUITS`]) through QSPICE in `workdir` and parse its `.qraw` output.
fn run_qspice_op(qspice: &Path, deck: &str, workdir: &Path, stem: &str) -> Result<QspiceRaw> {
    let cir_path = workdir.join(format!("{stem}.cir"));
    std::fs::write(&cir_path, deck).context("writing scratch .cir")?;
    let status = Command::new(qspice)
        .arg(
            cir_path
                .file_name()
                .context("scratch .cir has no filename")?,
        )
        .current_dir(workdir)
        .status()
        .context("launching QSPICE64.exe")?;
    if !status.success() {
        bail!("QSPICE exited with {status}");
    }
    let qraw_path = workdir.join(format!("{stem}.qraw"));
    let bytes = std::fs::read(&qraw_path)
        .with_context(|| format!("QSPICE did not produce {}", qraw_path.display()))?;
    parse_qraw(&bytes)
}

/// One QSPICE `.qraw` file's contents, restricted to the single-operating-point case (`No.
/// Points: 1` — a `.qraw` for a sweep or transient run has more, and isn't handled here).
struct QspiceRaw {
    /// Variable names in declared order, e.g. `"V(in)"`, `"I(V1)"` (as QSPICE spells them).
    variables: Vec<String>,
    /// One value per `variables` entry, same order.
    values: Vec<f64>,
}

/// Parse a `.qraw` file: an ASCII/UTF-8 header (`Title:`/`Plotname:`/`No. Variables:`/a
/// `Variables:` block listing `<index>\t<name>\t<unit>` per line/`Binary:`), followed by one
/// little-endian `f64` per variable — confirmed empirically against a real QSPICE `.op` run
/// (`circuits/divider.net`: `V(in)=1`, `V(mid)=0.5`, matching this project's own analytic
/// answer exactly), the same ASCII-header-then-binary-payload shape ngspice's own `.raw` format
/// uses. Only `No. Points: 1` is supported — see [`QspiceRaw`].
///
/// # Errors
///
/// If the header is missing/malformed, declares zero or an unparseable variable count, or the
/// binary payload is shorter than the header promises.
fn parse_qraw(bytes: &[u8]) -> Result<QspiceRaw> {
    const MARKER: &[u8] = b"Binary:\n";
    let marker_at = bytes
        .windows(MARKER.len())
        .position(|w| w == MARKER)
        .context("no `Binary:` marker in .qraw — not a QSPICE raw file, or an unsupported one")?;
    let payload_start = marker_at + MARKER.len();
    let header =
        std::str::from_utf8(&bytes[..marker_at]).context(".qraw header is not valid UTF-8")?;

    let mut n_vars = None;
    let mut n_points = None;
    let mut variables = Vec::new();
    let mut in_variables_block = false;
    for line in header.lines() {
        if let Some(rest) = line.strip_prefix("No. Variables:") {
            n_vars = Some(
                rest.trim()
                    .parse::<usize>()
                    .context("unparseable `No. Variables:`")?,
            );
        } else if let Some(rest) = line.strip_prefix("No. Points:") {
            n_points = Some(
                rest.trim()
                    .parse::<usize>()
                    .context("unparseable `No. Points:`")?,
            );
        } else if line.trim() == "Variables:" {
            in_variables_block = true;
        } else if in_variables_block {
            // `\t<index>\t<name>\t<unit>`.
            if let Some(name) = line.split('\t').nth(2) {
                variables.push(name.to_string());
            }
        }
    }
    let n_vars = n_vars.context("`.qraw` header has no `No. Variables:` line")?;
    match n_points {
        Some(1) => {}
        Some(n) => bail!("`.qraw` has {n} point(s); only a single operating point is supported"),
        None => bail!("`.qraw` header has no `No. Points:` line"),
    }
    if variables.len() != n_vars {
        bail!(
            "`.qraw` header declares {n_vars} variable(s) but the `Variables:` block lists {}",
            variables.len()
        );
    }

    let payload = &bytes[payload_start..];
    if payload.len() < n_vars * 8 {
        bail!(
            "`.qraw` binary payload is {} byte(s), too short for {n_vars} f64 value(s)",
            payload.len()
        );
    }
    let values = (0..n_vars)
        .map(|i| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&payload[i * 8..i * 8 + 8]);
            f64::from_le_bytes(b)
        })
        .collect();
    Ok(QspiceRaw { variables, values })
}

/// Map a parsed `.qraw` operating point onto this project's own `node_order`, by looking up
/// each node's `"V(<name>)"` label — QSPICE's own variable ordering isn't assumed to match
/// `node_order`'s (it happened to, for `divider.net`, but nothing guarantees that in general).
fn golden_dc_from_qraw(
    raw: &QspiceRaw,
    node_order: &[String],
) -> Result<va_harness::golden::GoldenDc> {
    let values = node_order
        .iter()
        .map(|name| {
            let label = format!("V({name})");
            raw.variables
                .iter()
                .position(|v| v.eq_ignore_ascii_case(&label))
                .map(|i| raw.values[i])
                .with_context(|| format!("QSPICE output has no `{label}` variable"))
        })
        .collect::<Result<Vec<f64>>>()?;
    Ok(va_harness::golden::GoldenDc {
        node_order: node_order.to_vec(),
        values,
    })
}

/// Locate `QSPICE64.exe`: `QSPICE_PATH` env var first (an exact file path), then `PATH`, then
/// the standard Windows install location. QSPICE is Windows-only, matching this project's own
/// dev environment.
fn find_qspice() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QSPICE_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("QSPICE64.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let standard = PathBuf::from(r"C:\Program Files\QSPICE\QSPICE64.exe");
    standard.is_file().then_some(standard)
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
    fn validate_passes_with_a_mix_of_real_golden_and_skips() {
        // The project's actual current state: `golden/divider.golden` is real QSPICE output
        // (§ ladder rung 1, the first circuit `gen_golden` can regenerate for real); the rest
        // of `golden/` is still empty. `validate` must both check the one real reference (and
        // pass it) and treat the rest as "nothing captured yet," not a failure.
        validate().expect("validate should pass: one real PASS plus the rest merely skipped");
    }

    #[test]
    fn find_qspice_finds_the_real_install_on_this_machine() {
        // QSPICE is genuinely installed in this dev environment (confirmed manually via its own
        // CLI, not assumed) — a real regression check on the standard-install-location
        // fallback, not just a "does it compile" test.
        assert!(find_qspice().is_some());
    }

    /// Build a synthetic `.qraw` byte buffer with the same shape a real QSPICE `.op` run
    /// produces (confirmed against an actual run of `circuits/divider.net`) — lets
    /// `parse_qraw`'s logic be tested hermetically, without invoking QSPICE itself.
    fn synthetic_qraw(vars: &[(&str, &str)], values: &[f64]) -> Vec<u8> {
        let mut header = String::new();
        header.push_str("Title: * synthetic\n");
        header.push_str("Date: Mon Jan  1 00:00:00 2026\n");
        header.push_str("Plotname: Operating Point\n");
        header.push_str("Flags: real\n");
        header.push_str(&format!("No. Variables: {}\n", vars.len()));
        header.push_str("No. Points: 1                    \n");
        header.push_str("Command: QSPICE64, Build test\n");
        header.push_str("Variables:\n");
        for (i, (name, unit)) in vars.iter().enumerate() {
            header.push_str(&format!("\t{i}\t{name}\t{unit}\n"));
        }
        header.push_str("Binary:\n");
        let mut bytes = header.into_bytes();
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parse_qraw_reads_a_divider_style_fixture() {
        let bytes = synthetic_qraw(
            &[
                ("V(in)", "voltage"),
                ("V(mid)", "voltage"),
                ("I(V1)", "current"),
            ],
            &[1.0, 0.5, -0.0005],
        );
        let raw = parse_qraw(&bytes).expect("parse");
        assert_eq!(raw.variables, vec!["V(in)", "V(mid)", "I(V1)"]);
        assert_eq!(raw.values, vec![1.0, 0.5, -0.0005]);
    }

    #[test]
    fn parse_qraw_rejects_a_multi_point_file() {
        // Corrupt just the header text's `No. Points:` line, not the whole (non-UTF-8) buffer —
        // the binary payload's raw float bytes aren't valid UTF-8 to round-trip through String.
        let bytes = synthetic_qraw(&[("V(in)", "voltage")], &[1.0]);
        let marker = b"Binary:\n";
        let split_at = bytes
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap()
            + marker.len();
        let header = std::str::from_utf8(&bytes[..split_at]).unwrap();
        let mut fixed = header
            .replacen("No. Points: 1", "No. Points: 2", 1)
            .into_bytes();
        fixed.extend_from_slice(&bytes[split_at..]);
        fixed.extend_from_slice(&1.0f64.to_le_bytes()); // pad so length alone isn't the failure
        assert!(parse_qraw(&fixed).is_err());
    }

    #[test]
    fn parse_qraw_rejects_a_missing_binary_marker() {
        assert!(parse_qraw(b"Title: no binary marker here\n").is_err());
    }

    #[test]
    fn parse_qraw_rejects_a_truncated_payload() {
        let mut bytes = synthetic_qraw(&[("V(in)", "voltage"), ("V(mid)", "voltage")], &[1.0, 0.5]);
        bytes.truncate(bytes.len() - 4); // half of the second value missing
        assert!(parse_qraw(&bytes).is_err());
    }

    #[test]
    fn golden_dc_from_qraw_looks_up_by_name_regardless_of_order() {
        let raw = QspiceRaw {
            variables: vec![
                "I(V1)".to_string(),
                "V(mid)".to_string(),
                "V(in)".to_string(),
            ],
            values: vec![-0.0005, 0.5, 1.0],
        };
        let node_order = vec!["in".to_string(), "mid".to_string()];
        let golden = golden_dc_from_qraw(&raw, &node_order).expect("map");
        assert_eq!(golden.values, vec![1.0, 0.5]);
    }

    #[test]
    fn golden_dc_from_qraw_errors_on_a_missing_node() {
        let raw = QspiceRaw {
            variables: vec!["V(in)".to_string()],
            values: vec![1.0],
        };
        let node_order = vec!["in".to_string(), "mid".to_string()];
        assert!(golden_dc_from_qraw(&raw, &node_order).is_err());
    }
}
