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
/// rungs 1/5). `model` is `None` for a circuit solved entirely by `va-abi`'s reference
/// primitives.
const DC_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/divider.net", None),
    ("circuits/mos_dc.net", Some("models/mosfet.va")),
];

/// The `.dc`-sweep circuits `validate`/`gen-golden` know how to drive (§ ladder rung 2).
const SWEEP_CIRCUITS: &[(&str, Option<&str>)] =
    &[("circuits/diode_iv.net", Some("models/diode.va"))];

/// The `.tran` transient circuits `validate`/`gen-golden` know how to drive (§ ladder rungs
/// 3/4). Rung 6 (the ring oscillator) isn't here yet — it has no `circuits/*.net` deck at all;
/// its demo builds `va-abi::Bjt` instances directly in a `va-transient` test, since `va-netlist`
/// has no BJT netlist element (a real, scoped gap, not an oversight).
const TRAN_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/rc_step.net", None),
    ("circuits/rectifier.net", Some("models/diode.va")),
];

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

/// Like [`validate_dc_circuits`], for every known `.tran` transient circuit (§ ladder rungs 3/4).
fn validate_tran_circuits(root: &Path) -> Result<Tally> {
    let mut tally = Tally::default();
    for &(circuit, model) in TRAN_CIRCUITS {
        let (circuit_path, model_path, golden_path) = circuit_paths(root, circuit, model)?;
        if !golden_path.is_file() {
            eprintln!(
                "[xtask]   skip {circuit}: no golden reference at {}",
                golden_path.display()
            );
            tally.skipped += 1;
            continue;
        }

        let golden = va_harness::golden::GoldenTran::read(&golden_path)
            .with_context(|| format!("reading golden reference for {circuit}"))?;
        let got = va_harness::tran::run_tran(
            circuit_path.to_str().context("non-UTF8 circuit path")?,
            model_path
                .as_deref()
                .map(|p| p.to_str().context("non-UTF8 model path"))
                .transpose()?,
        )
        .with_context(|| format!("solving {circuit}"))?;
        let verdict = va_harness::tran::compare_tran(&got, &golden)
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

/// Run the validation harness over every known circuit (DC, `.dc`-sweep, and `.tran`) and report
/// pass/fail/skip.
///
/// # Errors
///
/// If any circuit that *does* have a golden reference fails to solve, or diverges from it beyond
/// `va_harness::tol::DC_REL`/`TRAN_RMS`.
fn validate() -> Result<()> {
    eprintln!("[xtask] validate: running va-harness over the model zoo vs golden/ …");
    let root = workspace_root()?;

    let mut tally = validate_dc_circuits(&root)?;
    tally.merge(validate_sweep_circuits(&root)?);
    tally.merge(validate_tran_circuits(&root)?);

    eprintln!(
        "[xtask] validate: {} checked, {} failed, {} skipped (no golden)",
        tally.checked, tally.failed, tally.skipped
    );
    if tally.failed > 0 {
        bail!("{} circuit(s) failed golden comparison", tally.failed);
    }
    Ok(())
}

/// The circuits `gen_golden` can regenerate with **zero translation**: pure `R`/`C`/`V` decks
/// with no custom Verilog-A model and no temperature-sensitive nonlinearity, so QSPICE's own
/// built-in primitives reproduce this project's answer with zero ambiguity — confirmed
/// empirically, not assumed (`circuits/divider.net` run through QSPICE unmodified gives
/// `V(mid)=0.5` exactly, bit-for-bit matching the analytic/computed value).
///
/// `circuits/mos_dc.net`/`diode_iv.net` need a custom `.va` model (`models/mosfet.va`/
/// `diode.va`) translated into an equivalent QSPICE-native `.model` card instead — see
/// [`QSPICE_MODEL_TRANSLATIONS`]/[`QSPICE_SWEEP_MODEL_TRANSLATIONS`]. The temperature-convention
/// mismatch that used to block both (QSPICE's default 300.15 K `TNOM` vs. this project's old
/// fixed 300 K constants — a forced 0.5 V diode measured `2.50974869898304e-6` A at 300 K against
/// `2.48560822992004e-6` A from QSPICE's native diode at its own default temperature, ~0.85%
/// relative difference, comfortably past `va_harness::tol::DC_REL`'s `1e-4`; forcing QSPICE's
/// `.temp` to exactly 300 K made it *worse*, not better, since SPICE rescales `IS` relative to
/// `TNOM` whenever `.temp` differs from it) is now closed: `va_codegen::TEMP`/`VT` (and every
/// reference-model copy) were moved to the 300.15 K/QSPICE-matching convention.
const QSPICE_NATIVE_CIRCUITS: &[&str] = &["circuits/divider.net"];

/// QSPICE-native `.model` card translations for the single-`.op`-point circuits (§ ladder rung 5)
/// that reference a custom `.va` model QSPICE has no idea how to load. Hand-translated from the
/// `.va` model's own default parameters — kept in sync manually, not derived from the `.va`
/// source, so a parameter default changed in the `.va` file must be mirrored here too.
///
/// `models/mosfet.va`'s Level-1 (Shichman-Hodges) equations are exactly SPICE's own `NMOS
/// LEVEL=1` equations (`Id_sat = KP/2 * (W/L) * Vov^2 * (1+LAMBDA*Vds)`, `Id_triode = KP*(W/L) *
/// (Vov*Vds - Vds^2/2) * (1+LAMBDA*Vds)`), so the parameter names carry over one-to-one.
const QSPICE_MODEL_TRANSLATIONS: &[(&str, &str)] = &[(
    "circuits/mos_dc.net",
    ".model mosfet NMOS(LEVEL=1 VTO=0.7 KP=200u LAMBDA=0.01 W=10u L=1u)",
)];

/// Like [`QSPICE_MODEL_TRANSLATIONS`], for the `.dc`-sweep circuits (§ ladder rung 2).
///
/// `models/diode.va`'s `I = Is*(exp(V/(N*$vt)) - 1)` is exactly SPICE's own diode `D` model with
/// no series resistance/junction capacitance/breakdown — `IS`/`N` carry over one-to-one.
const QSPICE_SWEEP_MODEL_TRANSLATIONS: &[(&str, &str)] =
    &[("circuits/diode_iv.net", ".model diode D(IS=1e-14 N=1)")];

/// Like [`QSPICE_NATIVE_CIRCUITS`], for the `.tran` transient circuits (§ ladder rung 3):
/// `circuits/rc_step.net` is a pure `R`/`C`/`V` deck, needing zero translation, just multi-point
/// `.qraw` parsing ([`golden_tran_from_qraw`]) instead of the single-`.op`-point path.
const QSPICE_NATIVE_TRAN_CIRCUITS: &[&str] = &["circuits/rc_step.net"];

/// Like [`QSPICE_SWEEP_MODEL_TRANSLATIONS`], for the `.tran` transient circuits (§ ladder rung 4)
/// that reference a custom `.va` model.
const QSPICE_TRAN_MODEL_TRANSLATIONS: &[(&str, &str)] =
    &[("circuits/rectifier.net", ".model diode D(IS=1e-14 N=1)")];

/// Rewrite `deck` into a QSPICE-runnable deck: insert `model_card`, and widen any 3-terminal
/// `M<name> d g s model` device line (this project's own simplified form, § `va-netlist`'s
/// module doc) into QSPICE's native 4-terminal `M<name> d g s b model` by tying the body to the
/// source — matches `models/mosfet.va`'s own no-body-effect scope (source is its only
/// reference), so body=source is the physically-faithful translation, not just a syntactic one.
///
/// `model_card` is inserted as the deck's **second** line, not prepended as the first: like
/// every SPICE dialect, QSPICE unconditionally treats a deck's first line as its title, whatever
/// its content — prepending `model_card` outright made it swallow the `.model` card as the title
/// string instead of a real directive, silently falling back to a built-in default model
/// (confirmed empirically: QSPICE printed `Didn't find a model for "MOSFET" -- defaults assumed`
/// and solved to `V(d)=4.96` instead of the analytic ~3.255 V).
fn translate_for_qspice(deck: &str, model_card: &str) -> String {
    let mut lines = deck.lines();
    let mut out = String::new();
    if let Some(title) = lines.next() {
        out.push_str(title);
        out.push('\n');
    }
    out.push_str(model_card);
    out.push('\n');
    for line in lines {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let is_m_device = matches!(toks.first(), Some(t) if t.starts_with(['M', 'm']))
            && !line.trim_start().starts_with('*');
        if is_m_device && toks.len() == 5 {
            // `M1 d g s model` -> `M1 d g s s model` (body tied to source).
            out.push_str(&format!(
                "{} {} {} {} {} {}\n",
                toks[0], toks[1], toks[2], toks[3], toks[3], toks[4]
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Force a `.tran` deck to cold-start from the zero vector, matching this project's own
/// `va-transient` convention (`va-cli::solve_transient`'s doc comment: no `.ic`/`UIC` support,
/// so a transient run always starts from `x=0`). QSPICE, like standard SPICE, otherwise computes
/// the DC operating point first and starts the transient integration from *there* instead —
/// confirmed empirically, not assumed: an unmodified `circuits/rc_step.net` run through QSPICE
/// reported `V(out)` already at its settled ~5 V for the *entire* 5 ms window, not climbing the
/// RC charging curve from 0, and `cargo xtask validate` genuinely failed against it (caught the
/// same way the earlier title-line bug was, by sanity-checking the regenerated golden against
/// the netlist's own hand-derived expectation rather than trusting a clean `gen-golden` exit).
/// Seeds every reactive (`C`/`L`) element's own `IC=0` device parameter and appends `UIC` to the
/// `.tran` card — SPICE's standard mechanism for skipping the initial operating-point solve.
fn cold_start_tran_deck(deck: &str) -> String {
    let mut out = String::new();
    for line in deck.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let is_reactive = matches!(toks.first(), Some(t) if t.starts_with(['C', 'c', 'L', 'l']))
            && !line.trim_start().starts_with('*');
        let is_tran_card = line.trim_start().to_ascii_lowercase().starts_with(".tran");
        out.push_str(line);
        if is_reactive && !line.to_ascii_uppercase().contains("IC=") {
            out.push_str(" IC=0");
        } else if is_tran_card && !line.to_ascii_uppercase().contains("UIC") {
            out.push_str(" UIC");
        }
        out.push('\n');
    }
    out
}

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

    for &(circuit, model_card) in QSPICE_MODEL_TRANSLATIONS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();
        let native_deck = translate_for_qspice(&deck, model_card);

        let raw = run_qspice_op(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (native translation)"))?;
        let golden = golden_dc_from_qraw(&raw, &net.node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} node(s), native-model translation)",
            golden_path.display(),
            golden.node_order.len()
        );
        generated += 1;
    }

    for &(circuit, model_card) in QSPICE_SWEEP_MODEL_TRANSLATIONS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
        let dc = net
            .dc
            .as_ref()
            .with_context(|| format!("{circuit} has no `.dc` sweep card"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();
        let native_deck = translate_for_qspice(&deck, model_card);

        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (native translation)"))?;
        let golden = golden_sweep_from_qraw(&raw, &dc.source, &net.node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} point(s), native-model translation)",
            golden_path.display(),
            golden.points.len()
        );
        generated += 1;
    }

    for &circuit in QSPICE_NATIVE_TRAN_CIRCUITS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();

        let native_deck = cold_start_tran_deck(&deck);
        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit}"))?;
        let golden = golden_tran_from_qraw(&raw, &net.node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} point(s))",
            golden_path.display(),
            golden.points.len()
        );
        generated += 1;
    }

    for &(circuit, model_card) in QSPICE_TRAN_MODEL_TRANSLATIONS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();
        let native_deck = cold_start_tran_deck(&translate_for_qspice(&deck, model_card));

        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (native translation)"))?;
        let golden = golden_tran_from_qraw(&raw, &net.node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} point(s), native-model translation)",
            golden_path.display(),
            golden.points.len()
        );
        generated += 1;
    }
    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!(
        "[xtask] gen-golden: {generated} circuit(s) regenerated from QSPICE (of {} known)",
        DC_CIRCUITS.len() + SWEEP_CIRCUITS.len() + TRAN_CIRCUITS.len()
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

/// Like [`run_qspice_op`], for any deck whose `.qraw` has more than one point — a `.dc` sweep
/// (§ ladder rung 2) or a `.tran` transient run (§ ladder rungs 3/4): both are point-major
/// multi-point `.qraw` files with no format difference `parse_qraw_sweep` needs to know about
/// (see [`golden_sweep_from_qraw`] vs. [`golden_tran_from_qraw`] for where the two are told
/// apart, by which variable each keys its rows on).
fn run_qspice_sweep(
    qspice: &Path,
    deck: &str,
    workdir: &Path,
    stem: &str,
) -> Result<QspiceRawSweep> {
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
    parse_qraw_sweep(&bytes)
}

/// One QSPICE `.qraw` file's contents, restricted to the single-operating-point case (`No.
/// Points: 1` — a `.qraw` for a sweep or transient run has more; see [`QspiceRawSweep`]).
struct QspiceRaw {
    /// Variable names in declared order, e.g. `"V(in)"`, `"I(V1)"` (as QSPICE spells them).
    variables: Vec<String>,
    /// One value per `variables` entry, same order.
    values: Vec<f64>,
}

/// One QSPICE `.qraw` file's contents for a multi-point run (§ ladder rung 2's `.dc` sweep).
struct QspiceRawSweep {
    /// Variable names in declared order, same spelling as [`QspiceRaw::variables`] — the first
    /// is always the swept quantity itself (its bare source name, e.g. `"V1"`, not `"V(V1)"`;
    /// confirmed against a real QSPICE `.dc` run of `circuits/diode_iv.net`).
    variables: Vec<String>,
    /// One row of `variables.len()` values per sweep point, in point-major order (confirmed
    /// empirically: a real QSPICE `.dc V1 0 0.6 0.1` run's binary payload is laid out as 7
    /// consecutive 6-value rows, not 6 consecutive 7-value columns).
    points: Vec<Vec<f64>>,
}

/// Shared `.qraw` header parse: variable names plus the declared point count and the raw binary
/// payload slice. Both [`parse_qraw`] and [`parse_qraw_sweep`] build on this, differing only in
/// how many points they accept and how they slice the payload.
///
/// An ASCII/UTF-8 header (`Title:`/`Plotname:`/`No. Variables:`/a `Variables:` block listing
/// `<index>\t<name>\t<unit>` per line/`Binary:`), followed by one little-endian `f64` per
/// variable per point — confirmed empirically against real QSPICE runs (a single-point `.op` of
/// `circuits/divider.net`; a 7-point `.dc` of a translated `circuits/diode_iv.net`), the same
/// ASCII-header-then-binary-payload shape ngspice's own `.raw` format uses.
///
/// # Errors
///
/// If the header is missing/malformed, or declares zero or an unparseable variable count.
fn parse_qraw_header(bytes: &[u8]) -> Result<(Vec<String>, usize, &[u8])> {
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
    let n_points = n_points.context("`.qraw` header has no `No. Points:` line")?;
    if variables.len() != n_vars {
        bail!(
            "`.qraw` header declares {n_vars} variable(s) but the `Variables:` block lists {}",
            variables.len()
        );
    }
    Ok((variables, n_points, &bytes[payload_start..]))
}

/// Parse a single-operating-point `.qraw` file. Only `No. Points: 1` is supported — see
/// [`parse_qraw_sweep`] for a multi-point `.dc` sweep.
///
/// # Errors
///
/// If [`parse_qraw_header`] fails, the file has other than one point, or the binary payload is
/// shorter than the header promises.
fn parse_qraw(bytes: &[u8]) -> Result<QspiceRaw> {
    let (variables, n_points, payload) = parse_qraw_header(bytes)?;
    match n_points {
        1 => {}
        n => bail!("`.qraw` has {n} point(s); only a single operating point is supported"),
    }
    let n_vars = variables.len();
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

/// Parse a multi-point (`.dc` sweep) `.qraw` file — point-major payload layout, see
/// [`QspiceRawSweep`].
///
/// # Errors
///
/// If [`parse_qraw_header`] fails, the file has zero points, or the binary payload is shorter
/// than the header promises.
fn parse_qraw_sweep(bytes: &[u8]) -> Result<QspiceRawSweep> {
    let (variables, n_points, payload) = parse_qraw_header(bytes)?;
    if n_points == 0 {
        bail!("`.qraw` declares 0 points");
    }
    let n_vars = variables.len();
    if payload.len() < n_vars * n_points * 8 {
        bail!(
            "`.qraw` binary payload is {} byte(s), too short for {n_points} point(s) of \
             {n_vars} f64 value(s) each",
            payload.len()
        );
    }
    let points = (0..n_points)
        .map(|p| {
            (0..n_vars)
                .map(|v| {
                    let base = (p * n_vars + v) * 8;
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&payload[base..base + 8]);
                    f64::from_le_bytes(b)
                })
                .collect()
        })
        .collect();
    Ok(QspiceRawSweep { variables, points })
}

/// Look up each of `node_order`'s `"V(<name>)"` labels in `variables`/`row` — shared by
/// [`golden_dc_from_qraw`] and [`golden_sweep_from_qraw`]. QSPICE's own variable ordering isn't
/// assumed to match `node_order`'s (it happened to, for `divider.net`, but nothing guarantees
/// that in general).
fn node_values_from_row(
    variables: &[String],
    row: &[f64],
    node_order: &[String],
) -> Result<Vec<f64>> {
    node_order
        .iter()
        .map(|name| {
            let label = format!("V({name})");
            variables
                .iter()
                .position(|v| v.eq_ignore_ascii_case(&label))
                .map(|i| row[i])
                .with_context(|| format!("QSPICE output has no `{label}` variable"))
        })
        .collect()
}

/// Map a parsed `.qraw` operating point onto this project's own `node_order`.
fn golden_dc_from_qraw(
    raw: &QspiceRaw,
    node_order: &[String],
) -> Result<va_harness::golden::GoldenDc> {
    let values = node_values_from_row(&raw.variables, &raw.values, node_order)?;
    Ok(va_harness::golden::GoldenDc {
        node_order: node_order.to_vec(),
        values,
    })
}

/// Map a parsed `.qraw` sweep onto this project's own `node_order`, keyed by `source`'s own
/// swept value in each row (QSPICE labels that column with the bare source name, e.g. `"V1"`,
/// not `"V(V1)"` — confirmed empirically, see [`QspiceRawSweep::variables`]).
fn golden_sweep_from_qraw(
    raw: &QspiceRawSweep,
    source: &str,
    node_order: &[String],
) -> Result<va_harness::golden::GoldenSweep> {
    let source_idx = raw
        .variables
        .iter()
        .position(|v| v.eq_ignore_ascii_case(source))
        .with_context(|| format!("QSPICE output has no `{source}` swept-value variable"))?;
    let points = raw
        .points
        .iter()
        .map(|row| {
            let node_values = node_values_from_row(&raw.variables, row, node_order)?;
            Ok((row[source_idx], node_values))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(va_harness::golden::GoldenSweep::from_sweep(
        source, node_order, &points,
    ))
}

/// Map a parsed `.qraw` transient run onto this project's own `node_order`, keyed by the `Time`
/// variable QSPICE always includes for a `.tran` run (confirmed empirically against a real run
/// of `circuits/rc_step.net`) — the transient analogue of [`golden_sweep_from_qraw`], which keys
/// off a swept source's own name instead since a `.dc` sweep's independent variable is a device
/// parameter, not time.
fn golden_tran_from_qraw(
    raw: &QspiceRawSweep,
    node_order: &[String],
) -> Result<va_harness::golden::GoldenTran> {
    let time_idx = raw
        .variables
        .iter()
        .position(|v| v.eq_ignore_ascii_case("time"))
        .context("QSPICE output has no `Time` variable")?;
    let points = raw
        .points
        .iter()
        .map(|row| {
            let node_values = node_values_from_row(&raw.variables, row, node_order)?;
            Ok((row[time_idx], node_values))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(va_harness::golden::GoldenTran {
        node_order: node_order.to_vec(),
        points,
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
    fn validate_passes_with_all_known_circuits_golden() {
        // The project's actual current state: every circuit in `DC_CIRCUITS`/`SWEEP_CIRCUITS`/
        // `TRAN_CIRCUITS` now has real, committed QSPICE golden (rungs 1/2/3/4/5) — `divider.net`
        // from `QSPICE_NATIVE_CIRCUITS`/`rc_step.net` from `QSPICE_NATIVE_TRAN_CIRCUITS`
        // unmodified; `mos_dc.net`/`diode_iv.net`/`rectifier.net` from a native-model
        // translation. Nothing is skipped anymore; `validate` must pass all five for real.
        validate().expect("validate should pass: all five known circuits have real golden");
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

    /// Build a synthetic multi-point (`.dc` sweep) `.qraw` byte buffer, point-major — the same
    /// shape a real QSPICE `.dc` run produces (confirmed against an actual translated run of
    /// `circuits/diode_iv.net`).
    fn synthetic_qraw_sweep(vars: &[&str], rows: &[[f64; 2]]) -> Vec<u8> {
        let mut header = String::new();
        header.push_str("Title: * synthetic sweep\n");
        header.push_str("Plotname: DC Transfer Characteristic\n");
        header.push_str("Flags: real\n");
        header.push_str(&format!("No. Variables: {}\n", vars.len()));
        header.push_str(&format!("No. Points: {}                    \n", rows.len()));
        header.push_str("Variables:\n");
        for (i, name) in vars.iter().enumerate() {
            header.push_str(&format!("\t{i}\t{name}\tvoltage\n"));
        }
        header.push_str("Binary:\n");
        let mut bytes = header.into_bytes();
        for row in rows {
            for v in row {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn parse_qraw_sweep_reads_point_major_rows() {
        let bytes = synthetic_qraw_sweep(&["V1", "V(in)"], &[[0.0, 0.0], [0.1, 0.1], [0.2, 0.2]]);
        let raw = parse_qraw_sweep(&bytes).expect("parse");
        assert_eq!(raw.variables, vec!["V1", "V(in)"]);
        assert_eq!(
            raw.points,
            vec![vec![0.0, 0.0], vec![0.1, 0.1], vec![0.2, 0.2],]
        );
    }

    #[test]
    fn parse_qraw_sweep_rejects_a_truncated_payload() {
        let mut bytes = synthetic_qraw_sweep(&["V1", "V(in)"], &[[0.0, 0.0], [0.1, 0.1]]);
        bytes.truncate(bytes.len() - 4); // half of the last row's second value missing
        assert!(parse_qraw_sweep(&bytes).is_err());
    }

    #[test]
    fn golden_sweep_from_qraw_maps_source_and_nodes_by_name() {
        let raw = QspiceRawSweep {
            variables: vec!["V(in)".to_string(), "V1".to_string()],
            points: vec![vec![0.0, 0.0], vec![0.1, 0.1]],
        };
        let node_order = vec!["in".to_string()];
        let golden = golden_sweep_from_qraw(&raw, "V1", &node_order).expect("map");
        assert_eq!(golden.source, "V1");
        assert_eq!(golden.node_order, vec!["in".to_string()]);
        assert_eq!(golden.points, vec![(0.0, vec![0.0]), (0.1, vec![0.1])]);
    }

    #[test]
    fn golden_sweep_from_qraw_errors_on_a_missing_source() {
        let raw = QspiceRawSweep {
            variables: vec!["V(in)".to_string()],
            points: vec![vec![0.0]],
        };
        let node_order = vec!["in".to_string()];
        assert!(golden_sweep_from_qraw(&raw, "V1", &node_order).is_err());
    }

    #[test]
    fn translate_for_qspice_keeps_the_title_as_the_first_line() {
        // SPICE (and QSPICE) unconditionally treats a deck's first line as its title. Prepending
        // the `.model` card outright (the original, broken version of this function) made QSPICE
        // read it as the title string instead of a real directive and silently fall back to a
        // built-in default model — confirmed empirically (`Didn't find a model for "MOSFET" --
        // defaults assumed`, solving to `V(d)=4.96` instead of the analytic ~3.255 V).
        let deck = "* a title comment\nVDD vdd gnd DC 5.0\n.end\n";
        let out = translate_for_qspice(deck, ".model foo BAR(baz=1)");
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("* a title comment"));
        assert_eq!(lines.next(), Some(".model foo BAR(baz=1)"));
    }

    #[test]
    fn translate_for_qspice_widens_a_three_terminal_m_line() {
        let deck = "* title\nM1  d   g   gnd mosfet\n.op\n.end\n";
        let out = translate_for_qspice(deck, ".model mosfet NMOS(LEVEL=1)");
        assert!(
            out.lines().any(|l| l == "M1 d g gnd gnd mosfet"),
            "expected a body=source-widened M line, got:\n{out}"
        );
    }
}
