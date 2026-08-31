//! `xtask` — project dev automation, invoked as `cargo xtask <subcommand>`.
//!
//! Subcommands:
//! - `validate`       — run `va-harness` over the model zoo and compare to `golden/`.
//! - `gen-golden`     — (re)generate golden outputs from QSPICE, if installed.
//! - `tutorials`      — render the Quarto developer-tutorial book (`--preview` to live-edit).
//! - `bench-linsolve` — dense-vs-sparse MNA solve benchmark (T3 sparse-solve backlog).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use va_abi::reference::{Resistor, VSource, GROUND};
use va_abi::ModelInstance;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next();
    let rest: Vec<String> = args.collect();
    match cmd.as_deref() {
        Some("validate") => validate(),
        Some("gen-golden") => gen_golden(),
        Some("tutorials") => tutorials(&rest),
        Some("bench-linsolve") => bench_linsolve(),
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
         tutorials [--preview]  Render the Quarto developer-tutorial book (docs/tutorials/)\n    \
         bench-linsolve      Dense-vs-sparse MNA solve benchmark (T3 sparse-solve backlog)"
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
const SWEEP_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/diode_iv.net", Some("models/diode.va")),
    ("circuits/diode_clamp.net", Some("models/diode.va")),
];

/// The `.tran` transient circuits `validate`/`gen-golden` know how to drive (§ ladder rungs
/// 3/4/6). `ring_osc.net`'s `bjt` device has no `.va` model — it resolves to the hand-written
/// `va-abi::reference::Bjt` via `va-cli::reference_instance`, so `model` is `None` here too.
const TRAN_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/rc_step.net", None),
    ("circuits/rc_discharge.net", None),
    ("circuits/rlc_ring.net", None),
    ("circuits/rl_decay.net", None),
    ("circuits/rectifier.net", Some("models/diode.va")),
    ("circuits/ring_osc.net", None),
    ("circuits/abstime_ramp.net", Some("models/abstime_ramp.va")),
];

/// The `.ac` small-signal circuits `validate`/`gen-golden` know how to drive (T5).
/// `rc_ac.net` is a pure `R`/`C`/`V` deck solved by `va-abi`'s reference primitives;
/// `diode_ac.net` linearizes a compiled `models/diode.va` about a real forward bias, so its
/// golden comparison genuinely checks the AD-derived small-signal conductance, not just R/C
/// stamps.
const AC_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/rc_ac.net", None),
    ("circuits/rc_ac_lin.net", None),
    ("circuits/diode_ac.net", Some("models/diode.va")),
    ("circuits/laplace_ac.net", Some("models/laplace_lowpass.va")),
];

/// `.ac` circuits whose device has no QSPICE `.model` equivalent but whose *response* QSPICE can
/// compute from ordinary components — the AC counterpart of
/// [`QSPICE_TRAN_BEHAVIORAL_TRANSLATIONS`], and the same `(circuit, device, replacement)` shape.
///
/// `laplace_ac.net` is the Tier C gate, and it is the strongest oracle of the three tiers:
/// `laplace_nd(V(in), {1}, {1, tau})` **is** a one-pole lowpass, and QSPICE computes the
/// identical response from a real `R` and `C` with `tau = R·C`. Neither side can be tuned toward
/// the other without the discrepancy showing across the whole sweep — a rational function of `s`
/// and two physical components are genuinely different arithmetic.
///
/// The replacement spans two lines, which [`substitute_device_line`] handles: one device line
/// may become a whole subnetwork.
const QSPICE_AC_BEHAVIORAL_TRANSLATIONS: &[(&str, &str, &str)] = &[(
    "circuits/laplace_ac.net",
    "M1",
    // tau = R*C = 1k * 1u = 1 ms, matching models/laplace_lowpass.va's default.
    "R1x in out 1000
C1x out 0 1e-6",
)];

/// The `.noise` circuits `validate`/`gen-golden` know how to drive (T5.2).
///
/// `diode_noise.net`'s `model` is `None` **on purpose**: its `diode` resolves to the
/// hand-written `va-abi::reference::Diode`, so this deck gates Interface β's noise channel
/// independently of the compiler front end — see `circuits/diode_noise.net`'s own header
/// comment. Every other entry is the opposite gate: a compiled Verilog-A model whose own
/// `white_noise()`/`flicker_noise()`/`noise_table()` is the thing under test (T5.3, T5.6). That
/// split is deliberate; a lowering bug that zeroed every compiled noise source would still leave
/// `diode_noise.net` green.
const NOISE_CIRCUITS: &[(&str, Option<&str>)] = &[
    ("circuits/diode_noise.net", None),
    ("circuits/resistor_noise_va.net", Some("models/resistor.va")),
    (
        "circuits/diode_flicker.net",
        Some("models/diode_flicker.va"),
    ),
    (
        "circuits/resistor_noise_table.net",
        Some("models/resistor_noise_table.va"),
    ),
    (
        "circuits/resistor_noise_table_log.net",
        Some("models/resistor_noise_table_log.va"),
    ),
];

/// Like [`QSPICE_NATIVE_CIRCUITS`], for a `.noise` circuit needing no `.model` translation:
/// `resistor_noise_va.net` is pure `R`/`V`, even though *this* project drives it through a
/// compiled `models/resistor.va` (the point of that circuit is the compiled model's own
/// `white_noise()`; QSPICE just needs a plain resistor to compare against).
/// `resistor_noise_table.net` is the same arrangement for `noise_table()` (T5.6): its tabulated
/// PSD is `4kT/R` at every point, so a plain QSPICE resistor is again the exact oracle.
const QSPICE_NATIVE_NOISE_CIRCUITS: &[&str] = &[
    "circuits/resistor_noise_va.net",
    "circuits/resistor_noise_table.net",
    "circuits/resistor_noise_table_log.net",
];

/// Like [`QSPICE_SWEEP_MODEL_TRANSLATIONS`], for the `.noise` circuits that name a custom model
/// (T5.2). Both are one-to-one: a noise analysis linearizes the very same model equations, and
/// derives its shot noise from the very same DC current.
///
/// `diode_flicker.net` additionally carries `KF`/`AF` — SPICE's own flicker parameterization,
/// which `models/diode_flicker.va` deliberately mirrors (`flicker_noise(KF*|Id|^AF, 1.0)`), so
/// the comparison is meaningful across the whole band rather than only where flicker is
/// negligible.
const QSPICE_NOISE_MODEL_TRANSLATIONS: &[(&str, &str)] = &[
    ("circuits/diode_noise.net", ".model diode D(IS=1e-14 N=1)"),
    (
        "circuits/diode_flicker.net",
        ".model diode_flicker D(IS=1e-14 N=1 KF=1e-15 AF=1)",
    ),
];

/// Tally shared across [`validate`]'s DC/sweep/tran passes — distinguishes *three* different
/// outcomes, not two, per `CLAUDE.md` §7's four metrics: a circuit can have no golden yet
/// ([`Self::skipped`]), fail to converge at all ([`Self::not_converged`] — §7's own "convergence:
/// fraction of zoo circuits that reach a solution" metric, T6.4), or converge but land outside
/// golden's tolerance ([`Self::failed`]). `checked` counts every circuit actually attempted
/// (converged or not) — the convergence-fraction denominator.
#[derive(Default)]
struct Tally {
    /// Circuits with a committed golden reference that were actually attempted.
    checked: u32,
    /// Of `checked`, how many converged but landed outside golden's tolerance.
    failed: u32,
    /// Of `checked`, how many the solver failed to reach a solution for at all — a `CoreError`
    /// (non-convergence, a singular Jacobian, …) propagating out of `va-harness`'s own
    /// `run_dc`/`run_dc_sweep`/`run_tran`. Tracked as its own outcome, not folded into `failed`:
    /// "didn't converge" and "converged but wrong" are different failure modes with different
    /// fixes, and CLAUDE.md §7 asks for the convergence fraction specifically, as a number that
    /// only ever needs to go up.
    not_converged: u32,
    /// No committed `golden/<name>.golden` yet — never attempted at all.
    skipped: u32,
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.checked += other.checked;
        self.failed += other.failed;
        self.not_converged += other.not_converged;
        self.skipped += other.skipped;
    }

    /// How many of `checked` reached a solution at all, regardless of whether it matched
    /// golden — `CLAUDE.md` §7's convergence metric.
    fn converged(&self) -> u32 {
        self.checked - self.not_converged
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

/// Attempt `solve(circuit, model)`, recording the outcome in `tally` and printing `NOCONV`
/// rather than propagating the error — a circuit that fails to converge is real, useful
/// information (T6.4's own convergence-fraction metric), and `bail!`-ing the whole `validate`
/// run at the first one would silently hide the verdict on every circuit ordered after it.
/// Returns `None` on a solve failure, `Some(got)` on success (regardless of golden match).
fn try_solve<T>(
    circuit: &str,
    solve: impl FnOnce() -> Result<T, va_harness::HarnessError>,
    tally: &mut Tally,
) -> Option<T> {
    tally.checked += 1;
    match solve() {
        Ok(got) => Some(got),
        Err(e) => {
            eprintln!("[xtask]   NOCONV {circuit}: {e}");
            tally.not_converged += 1;
            None
        }
    }
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
        let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
        let model_str = model_path
            .as_deref()
            .map(|p| p.to_str().context("non-UTF8 model path"))
            .transpose()?;
        let Some(got) = try_solve(
            circuit,
            || va_harness::dc::run_dc(circuit_str, model_str),
            &mut tally,
        ) else {
            continue;
        };
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
        let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
        let model_str = model_path
            .as_deref()
            .map(|p| p.to_str().context("non-UTF8 model path"))
            .transpose()?;
        let Some(got) = try_solve(
            circuit,
            || va_harness::dc::run_dc_sweep(circuit_str, model_str),
            &mut tally,
        ) else {
            continue;
        };
        let verdict = va_harness::dc::compare_dc_sweep(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Like [`validate_dc_circuits`], for every known `.tran` transient circuit (§ ladder rungs
/// 3/4/6).
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
        let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
        let model_str = model_path
            .as_deref()
            .map(|p| p.to_str().context("non-UTF8 model path"))
            .transpose()?;
        let Some(got) = try_solve(
            circuit,
            || va_harness::tran::run_tran(circuit_str, model_str),
            &mut tally,
        ) else {
            continue;
        };
        let verdict = va_harness::tran::compare_tran(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Like [`validate_dc_circuits`], for every known `.ac` small-signal circuit (T5).
fn validate_ac_circuits(root: &Path) -> Result<Tally> {
    let mut tally = Tally::default();
    for &(circuit, model) in AC_CIRCUITS {
        let (circuit_path, model_path, golden_path) = circuit_paths(root, circuit, model)?;
        if !golden_path.is_file() {
            eprintln!(
                "[xtask]   skip {circuit}: no golden reference at {}",
                golden_path.display()
            );
            tally.skipped += 1;
            continue;
        }

        let golden = va_harness::golden::GoldenAc::read(&golden_path)
            .with_context(|| format!("reading golden reference for {circuit}"))?;
        let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
        let model_str = model_path
            .as_deref()
            .map(|p| p.to_str().context("non-UTF8 model path"))
            .transpose()?;
        let Some(got) = try_solve(
            circuit,
            || va_harness::ac::run_ac(circuit_str, model_str),
            &mut tally,
        ) else {
            continue;
        };
        let verdict = va_harness::ac::compare_ac(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_ac_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Like [`validate_dc_circuits`], for every known `.noise` circuit (T5.2).
fn validate_noise_circuits(root: &Path) -> Result<Tally> {
    let mut tally = Tally::default();
    for &(circuit, model) in NOISE_CIRCUITS {
        let (circuit_path, model_path, golden_path) = circuit_paths(root, circuit, model)?;
        if !golden_path.is_file() {
            eprintln!(
                "[xtask]   skip {circuit}: no golden reference at {}",
                golden_path.display()
            );
            tally.skipped += 1;
            continue;
        }

        let golden = va_harness::golden::GoldenNoise::read(&golden_path)
            .with_context(|| format!("reading golden reference for {circuit}"))?;
        let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
        let model_str = model_path
            .as_deref()
            .map(|p| p.to_str().context("non-UTF8 model path"))
            .transpose()?;
        let Some(got) = try_solve(
            circuit,
            || va_harness::noise::run_noise(circuit_str, model_str),
            &mut tally,
        ) else {
            continue;
        };
        let verdict = va_harness::noise::compare_noise(&got, &golden)
            .with_context(|| format!("comparing {circuit} against golden"))?;
        report_verdict(circuit, verdict, &mut tally);
    }
    Ok(tally)
}

/// Print one AC circuit's PASS/FAIL line and fold a golden mismatch into `tally`. Distinct from
/// [`report_verdict`] because §7's AC metric is genuinely two numbers against two bands
/// (magnitude *and* phase — see `va_harness::ac::AcVerdict`), and reporting only one of them
/// would hide half of what was actually checked. Counts as a single failed circuit either way,
/// not one per failing half.
fn report_ac_verdict(circuit: &str, verdict: va_harness::ac::AcVerdict, tally: &mut Tally) {
    let status = if verdict.passed() { "PASS" } else { "FAIL" };
    eprintln!(
        "[xtask]   {status} {circuit}: |mag| error={:.3e} (tol {:.0e}), phase error={:.3e} rad \
         (tol {:.0e})",
        verdict.magnitude.error, verdict.magnitude.tol, verdict.phase.error, verdict.phase.tol
    );
    if !verdict.passed() {
        tally.failed += 1;
    }
}

/// Print one circuit's PASS/FAIL line and fold a golden-mismatch into `tally`. Only called for a
/// circuit that already converged ([`try_solve`] returned `Some`) — `tally.checked` was already
/// incremented there, not here.
fn report_verdict(circuit: &str, verdict: va_harness::Verdict, tally: &mut Tally) {
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

/// Run the validation harness over every known circuit (DC, `.dc`-sweep, `.tran`, and `.ac`) and
/// report pass/fail/skip, plus `CLAUDE.md` §7's fourth metric — the convergence fraction (T6.4) —
/// as its own line, distinct from the golden-comparison pass/fail count.
///
/// # Errors
///
/// If any circuit that *does* have a golden reference fails to converge, or converges but
/// diverges from golden beyond `va_harness::tol::DC_REL`/`TRAN_RMS`. Every known circuit is still
/// attempted and reported first — a single non-convergent circuit no longer aborts the batch
/// before the rest are checked (T6.4's own point: the convergence fraction is only useful if
/// computed over the *whole* zoo, not just however much of it ran before the first failure).
fn validate() -> Result<()> {
    eprintln!("[xtask] validate: running va-harness over the model zoo vs golden/ …");
    let root = workspace_root()?;

    let mut tally = validate_dc_circuits(&root)?;
    tally.merge(validate_sweep_circuits(&root)?);
    tally.merge(validate_tran_circuits(&root)?);
    tally.merge(validate_ac_circuits(&root)?);
    tally.merge(validate_noise_circuits(&root)?);

    eprintln!(
        "[xtask] validate: {} checked, {} failed golden, {} did not converge, {} skipped (no golden)",
        tally.checked, tally.failed, tally.not_converged, tally.skipped
    );
    if tally.checked > 0 {
        eprintln!(
            "[xtask] validate: convergence {}/{} ({:.1}%) — CLAUDE.md §7's convergence metric",
            tally.converged(),
            tally.checked,
            100.0 * tally.converged() as f64 / tally.checked as f64
        );
    }
    if tally.not_converged > 0 || tally.failed > 0 {
        bail!(
            "{} circuit(s) did not converge, {} circuit(s) failed golden comparison",
            tally.not_converged,
            tally.failed
        );
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
const QSPICE_SWEEP_MODEL_TRANSLATIONS: &[(&str, &str)] = &[
    ("circuits/diode_iv.net", ".model diode D(IS=1e-14 N=1)"),
    ("circuits/diode_clamp.net", ".model diode D(IS=1e-14 N=1)"),
];

/// Like [`QSPICE_NATIVE_CIRCUITS`], for the `.tran` transient circuits (§ ladder rung 3):
/// `circuits/rc_step.net` is a pure `R`/`C`/`V` deck, needing zero translation, just multi-point
/// `.qraw` parsing ([`golden_tran_from_qraw`]) instead of the single-`.op`-point path.
const QSPICE_NATIVE_TRAN_CIRCUITS: &[&str] = &[
    "circuits/rc_step.net",
    "circuits/rc_discharge.net",
    "circuits/rlc_ring.net",
    "circuits/rl_decay.net",
];

/// Like [`QSPICE_SWEEP_MODEL_TRANSLATIONS`], for the `.tran` transient circuits (§ ladder rungs
/// 4/6) that reference a custom model. `models/bjt` has no `.va` file (it's the hand-written
/// `va-abi::reference::Bjt`), but the same one-to-one textbook-parameter translation applies:
/// `va-abi::reference::Bjt`'s simplified Ebers-Moll is exactly SPICE's own `NPN` model with
/// `IS`/`BF`/`BR` set and every other parameter (`VAF`, `RB`/`RC`/`RE`, `CJE`/`CJC`, …) left at
/// its SPICE default of "off" — matching this project's own "no Early effect, no ohmic
/// parasitics, no junction capacitance" stated scope exactly.
///
/// The third field is an optional golden-generation-only `.tran` `tstop` override — see
/// [`RING_OSC_GOLDEN_TSTOP`]'s own doc comment for why `ring_osc.net` needs one and
/// `rectifier.net` doesn't.
const QSPICE_TRAN_MODEL_TRANSLATIONS: &[(&str, &str, Option<f64>)] = &[
    (
        "circuits/rectifier.net",
        ".model diode D(IS=1e-14 N=1)",
        None,
    ),
    (
        "circuits/ring_osc.net",
        ".model bjt NPN(IS=1e-15 BF=100 BR=1)",
        Some(RING_OSC_GOLDEN_TSTOP),
    ),
];

/// `.tran` circuits whose Verilog-A device has **no** QSPICE `.model` equivalent, but whose
/// *behavior* QSPICE can express with a behavioral source. Each entry is
/// `(circuit, device-name prefix to replace, replacement line)`; the named device line is
/// substituted outright rather than having a `.model` card appended beside it
/// ([`substitute_device_line`]).
///
/// This is a different translation kind from [`QSPICE_TRAN_MODEL_TRANSLATIONS`], and the
/// difference is the point. A `.model` translation says "the same device, described in QSPICE's
/// own parameter language"; this says "a different description of the same *physics*". For
/// `abstime_ramp.net` that is exactly what is wanted: nothing in QSPICE corresponds to a
/// Verilog-A model reading `$abstime`, but a behavioral current source reading QSPICE's own
/// `time` computes the identical waveform from a description that shares no code with ours.
/// That independence is what makes the comparison evidence (§ `docs/validation.md`).
///
/// **These decks are deliberately not passed through [`cold_start_tran_deck`].** Verified
/// 2026-08-06: `UIC` shifts QSPICE's own `time` variable by a fixed ~1e-7 s (`I(B1) − time`
/// is exactly `+1.0e-7` at every point with `UIC`, and exactly `0.0` without it), which would
/// inject a constant offset into the very quantity under test. Skipping it is safe here only
/// because these decks carry nothing reactive — with no capacitor or inductor to seed, QSPICE's
/// default operating-point solve lands on the same `t = 0` state our own cold start begins
/// from. A future behavioral entry that *does* contain a reactive element would have to
/// reconcile the two, not simply inherit this exemption.
const QSPICE_TRAN_BEHAVIORAL_TRANSLATIONS: &[(&str, &str, &str)] = &[(
    "circuits/abstime_ramp.net",
    "D1",
    // `I(p,n) <+ K*$abstime` with K = 1 A/s. Verilog-A's `I(p,n)` and SPICE's `B n+ n- I=`
    // share a sign convention (current out of the first node), so the terminal order carries
    // over unchanged — no sign fixup, which is what keeps this a translation rather than a tuning.
    "B1 out 0 I=1*time",
)];

/// Like [`QSPICE_NATIVE_CIRCUITS`], for the `.ac` small-signal circuits (T5):
/// `circuits/rc_ac.net` is a pure `R`/`C`/`V` deck needing no model translation, just complex
/// `.qraw` parsing ([`parse_qraw_ac`]). It is still passed through [`rewrite_gnd_to_zero`] — a
/// topology-neutral normalization this project's own net interning already performs, so applying
/// it costs nothing and sidesteps QSPICE's own `gnd`-aliasing quirk without having to reason
/// about which element kinds are affected.
const QSPICE_NATIVE_AC_CIRCUITS: &[&str] = &["circuits/rc_ac.net", "circuits/rc_ac_lin.net"];

/// Like [`QSPICE_SWEEP_MODEL_TRANSLATIONS`], for the `.ac` circuits (T5) that reference a custom
/// `.va` model. The same one-to-one `IS`/`N` diode translation applies — an AC analysis
/// linearizes the very same model equations a DC solve evaluates, so a translation faithful for
/// `circuits/diode_iv.net` is faithful here too.
const QSPICE_AC_MODEL_TRANSLATIONS: &[(&str, &str)] =
    &[("circuits/diode_ac.net", ".model diode D(IS=1e-14 N=1)")];

/// The `.tran` cutoff used *only* when generating rung 6's golden reference — deliberately
/// shorter than `circuits/ring_osc.net`'s own `.tran 100u 0.2` card, which stays at `0.2` s so
/// `va-cli`'s own oscillation-count test (needing several growth cycles to find ≥4 rail-midpoint
/// crossings) keeps working unchanged.
///
/// This circuit's DC equilibrium is genuinely *unstable* (§ `t4-transient/03-events.qmd`): two
/// independent Newton/LTE-driven solvers agree closely while the oscillation is still small and
/// smooth, but diverge sharply once amplitude reaches the point where this simplified Ebers-Moll
/// model's own numerical edge kicks in — chaotic sensitivity to tiny model/solver differences,
/// not a modeling error. Confirmed empirically, not chosen arbitrarily: comparing the real
/// `golden/ring_osc.golden` (a full 0.2 s run) against a real `va-cli` run at successively later
/// cutoffs gave `error≈1.6e-4` to `2.4e-4` (comfortably under `TRAN_RMS`'s `1e-3`) for every
/// cutoff up to `0.10` s, then `1.16e-2` at `0.12` s and `2.24e-2` over the full `0.2` s — a
/// two-order-of-magnitude jump right where the trajectory visibly leaves the smooth growth
/// regime (collector voltages swinging to within noise of `0`/`5` V, base voltages going
/// transiently negative). Comparing only this well-behaved early window is the same principle
/// `va_transient::integrator`'s own hand-built ring-oscillator fixture already follows for
/// choosing its own `tstop` (§ its doc comment) — not a loosened tolerance, a narrower, honestly
/// scoped claim about what a comparison against an unstable system can mean at all.
const RING_OSC_GOLDEN_TSTOP: f64 = 0.1;

/// Rewrite a `.tran <tstep> <tstop> [flags...]` card's own `tstop` to `new_tstop`, leaving
/// `tstep` and any trailing flag (e.g. `UIC`) untouched. Used only for golden generation
/// (§ [`RING_OSC_GOLDEN_TSTOP`]) — never changes the tracked circuit file `va-cli`/`va-harness`
/// actually solve, only the scratch deck handed to QSPICE.
fn truncate_tran_tstop(deck: &str, new_tstop: f64) -> String {
    let mut out = String::new();
    for line in deck.lines() {
        if line.trim_start().to_ascii_lowercase().starts_with(".tran") {
            let toks: Vec<&str> = line.split_whitespace().collect();
            let mut rewritten = vec![
                toks[0].to_string(),
                toks[1].to_string(),
                new_tstop.to_string(),
            ];
            rewritten.extend(toks[3..].iter().map(|s| s.to_string()));
            out.push_str(&rewritten.join(" "));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Rewrite every whole-token `gnd`/`GND` net reference to the literal `0` SPICE ground name.
///
/// QSPICE reliably treats `0` as the reference node for every element kind, but does **not**
/// reliably alias a `gnd`-named net to ground for a `Q` (BJT) element's own terminal, unlike
/// `R`/`V`/`M` — confirmed empirically, not assumed: a DC solve of a single-BJT bias circuit
/// wired entirely through `gnd` reported `V(gnd)=5` (ground must always read `0` by definition)
/// alongside a physically wrong "every node pinned near VCC, zero BJT current" degenerate
/// result; the *identical* circuit with its emitter/source tied to `0` instead gives the correct
/// forward-active bias (`V(b1)=0.662`, matching this project's own cold-start value almost
/// exactly). Applied to every translated deck, not just ones with a `Q` element — the rewrite is
/// topology-neutral (this project's own net interning already treats `0`/`gnd` as synonymous,
/// case-insensitively), so there's no need to track which device kinds are actually affected by
/// QSPICE's own quirk. Comment lines (`*...`) are left untouched.
fn rewrite_gnd_to_zero(deck: &str) -> String {
    let mut out = String::new();
    for line in deck.lines() {
        if line.trim_start().starts_with('*') || line.split_whitespace().next().is_none() {
            out.push_str(line);
        } else {
            let rewritten: Vec<&str> = line
                .split_whitespace()
                .map(|tok| {
                    if tok.eq_ignore_ascii_case("gnd") {
                        "0"
                    } else {
                        tok
                    }
                })
                .collect();
            out.push_str(&rewritten.join(" "));
        }
        out.push('\n');
    }
    out
}

/// Replace the device line whose name is `device` with `replacement`, leaving every other line
/// (including the title and comments) untouched, then normalize `gnd` to `0`.
///
/// Used by [`QSPICE_TRAN_BEHAVIORAL_TRANSLATIONS`] for a device QSPICE has no `.model` for at
/// all. Matching is on the whole first token, case-insensitively, so a device named `D1` does
/// not match `D10`.
///
/// # Errors
///
/// If `device` names no line in `deck`. A silently-unsubstituted deck would run — QSPICE would
/// simply solve the circuit without that device, since the original line references a model it
/// has never heard of — and produce a plausible, entirely wrong golden file. That is exactly the
/// failure mode `docs/…/no-fake-golden-data` exists to prevent, so it is an error, not a warning.
fn substitute_device_line(deck: &str, device: &str, replacement: &str) -> Result<String> {
    let mut out = String::new();
    let mut replaced = 0usize;
    for line in deck.lines() {
        let is_target = line
            .split_whitespace()
            .next()
            .is_some_and(|tok| tok.eq_ignore_ascii_case(device))
            && !line.trim_start().starts_with('*');
        if is_target {
            out.push_str(replacement);
            replaced += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if replaced != 1 {
        bail!("expected exactly one `{device}` device line to substitute, found {replaced}");
    }
    Ok(rewrite_gnd_to_zero(&out))
}

/// Rewrite `deck` into a QSPICE-runnable deck: insert `model_card`, widen any 3-terminal
/// `M<name> d g s model` device line (this project's own simplified form, § `va-netlist`'s
/// module doc) into QSPICE's native 4-terminal `M<name> d g s b model` by tying the body to the
/// source — matches `models/mosfet.va`'s own no-body-effect scope (source is its only
/// reference), so body=source is the physically-faithful translation, not just a syntactic one —
/// and normalize every `gnd` reference to `0` ([`rewrite_gnd_to_zero`]).
///
/// `model_card` is inserted as the deck's **second** line, not prepended as the first: like
/// every SPICE dialect, QSPICE unconditionally treats a deck's first line as its title, whatever
/// its content — prepending `model_card` outright made it swallow the `.model` card as the title
/// string instead of a real directive, silently falling back to a built-in default model
/// (confirmed empirically: QSPICE printed `Didn't find a model for "MOSFET" -- defaults assumed`
/// and solved to `V(d)=4.96` instead of the analytic ~3.255 V).
fn translate_for_qspice(deck: &str, model_card: &str) -> String {
    let deck = rewrite_gnd_to_zero(deck);
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
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();

        let raw = run_qspice_op(&qspice, &deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit}"))?;
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_dc_from_qraw(&raw, &node_order)
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
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();
        let native_deck = translate_for_qspice(&deck, model_card);

        let raw = run_qspice_op(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (native translation)"))?;
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_dc_from_qraw(&raw, &node_order)
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
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_sweep_from_qraw(&raw, &dc.source, &node_order)
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
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();

        let native_deck = cold_start_tran_deck(&deck);
        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit}"))?;
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_tran_from_qraw(&raw, &node_order)
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

    for &(circuit, device, replacement) in QSPICE_TRAN_BEHAVIORAL_TRANSLATIONS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();

        // Deliberately no `cold_start_tran_deck` here — `UIC` offsets QSPICE's own `time`.
        // See `QSPICE_TRAN_BEHAVIORAL_TRANSLATIONS`' doc comment for the measurement.
        let native_deck = substitute_device_line(&deck, device, replacement)
            .with_context(|| format!("translating {circuit} for QSPICE"))?;

        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (behavioral translation)"))?;
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_tran_from_qraw(&raw, &node_order)
            .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

        let golden_path = golden_dir.join(format!("{stem}.golden"));
        std::fs::write(&golden_path, golden.render())
            .with_context(|| format!("writing {}", golden_path.display()))?;
        eprintln!(
            "[xtask]   wrote {} ({} point(s), behavioral-source translation)",
            golden_path.display(),
            golden.points.len()
        );
        generated += 1;
    }

    for &(circuit, model_card, golden_tstop) in QSPICE_TRAN_MODEL_TRANSLATIONS {
        let circuit_path = root.join(circuit);
        let deck =
            std::fs::read_to_string(&circuit_path).with_context(|| format!("reading {circuit}"))?;
        let stem = Path::new(circuit)
            .file_stem()
            .context("circuit path has no file stem")?
            .to_string_lossy()
            .into_owned();
        let mut native_deck = cold_start_tran_deck(&translate_for_qspice(&deck, model_card));
        if let Some(tstop) = golden_tstop {
            native_deck = truncate_tran_tstop(&native_deck, tstop);
        }

        let raw = run_qspice_sweep(&qspice, &native_deck, &tmp, &stem)
            .with_context(|| format!("running QSPICE on {circuit} (native translation)"))?;
        let node_order = golden_node_order(&root, circuit)
            .with_context(|| format!("resolving branch currents for {circuit}"))?;
        let golden = golden_tran_from_qraw(&raw, &node_order)
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
    // `.ac` circuits (T5): no cold-start rewrite (an AC analysis is *defined* as a linearization
    // about the DC operating point — unlike `.tran`, where this project's own zero-vector start
    // had to be forced on QSPICE), just the `gnd` normalization and, where needed, the same
    // native `.model` translation every other analysis uses.
    for &circuit in QSPICE_NATIVE_AC_CIRCUITS {
        let native_deck = rewrite_gnd_to_zero(&read_circuit(&root, circuit)?);
        generated += gen_ac_golden(&qspice, &root, &tmp, circuit, &native_deck)?;
    }
    for &(circuit, model_card) in QSPICE_AC_MODEL_TRANSLATIONS {
        let native_deck = translate_for_qspice(&read_circuit(&root, circuit)?, model_card);
        generated += gen_ac_golden(&qspice, &root, &tmp, circuit, &native_deck)?;
    }
    for &(circuit, device, replacement) in QSPICE_AC_BEHAVIORAL_TRANSLATIONS {
        let native_deck =
            substitute_device_line(&read_circuit(&root, circuit)?, device, replacement)
                .with_context(|| format!("translating {circuit} for QSPICE"))?;
        generated += gen_ac_golden(&qspice, &root, &tmp, circuit, &native_deck)?;
    }

    // `.noise` circuits (T5.2): like `.ac`, no cold-start rewrite — a noise analysis is a
    // linearization about the DC operating point, so QSPICE's "solve DC first" is exactly right.
    for &circuit in QSPICE_NATIVE_NOISE_CIRCUITS {
        let native_deck = rewrite_gnd_to_zero(&read_circuit(&root, circuit)?);
        generated += gen_noise_golden(&qspice, &root, &tmp, circuit, &native_deck)?;
    }
    for &(circuit, model_card) in QSPICE_NOISE_MODEL_TRANSLATIONS {
        let native_deck = translate_for_qspice(&read_circuit(&root, circuit)?, model_card);
        generated += gen_noise_golden(&qspice, &root, &tmp, circuit, &native_deck)?;
    }

    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!(
        "[xtask] gen-golden: {generated} circuit(s) regenerated from QSPICE (of {} known)",
        DC_CIRCUITS.len()
            + SWEEP_CIRCUITS.len()
            + TRAN_CIRCUITS.len()
            + AC_CIRCUITS.len()
            + NOISE_CIRCUITS.len()
    );
    Ok(())
}

/// Read a circuit deck from the workspace root.
fn read_circuit(root: &Path, circuit: &str) -> Result<String> {
    std::fs::read_to_string(root.join(circuit)).with_context(|| format!("reading {circuit}"))
}

/// Run one already-translated `.ac` deck through QSPICE and write its golden file, returning `1`
/// so the caller can fold it into its own generated count. Factored out (unlike the DC/sweep/tran
/// passes above, which each inline their own copy of this shape) because the native and
/// model-translated AC passes differ *only* in how the deck text was prepared.
fn gen_ac_golden(
    qspice: &Path,
    root: &Path,
    tmp: &Path,
    circuit: &str,
    native_deck: &str,
) -> Result<u32> {
    let stem = Path::new(circuit)
        .file_stem()
        .context("circuit path has no file stem")?
        .to_string_lossy()
        .into_owned();

    let raw = run_qspice_ac(qspice, native_deck, tmp, &stem)
        .with_context(|| format!("running QSPICE on {circuit}"))?;
    let node_order = golden_node_order(root, circuit)
        .with_context(|| format!("resolving branch currents for {circuit}"))?;
    let golden = golden_ac_from_qraw(&raw, &node_order)
        .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

    let golden_path = root.join("golden").join(format!("{stem}.golden"));
    std::fs::write(&golden_path, golden.render())
        .with_context(|| format!("writing {}", golden_path.display()))?;
    eprintln!(
        "[xtask]   wrote {} ({} frequency point(s), {} column(s))",
        golden_path.display(),
        golden.points.len(),
        golden.node_order.len()
    );
    Ok(1)
}

/// Run one already-translated `.noise` deck through QSPICE and write its golden file, returning
/// `1` so the caller can fold it into its own generated count — the noise analogue of
/// [`gen_ac_golden`], and factored out for the same reason (the native and model-translated
/// passes differ only in how the deck text was prepared).
///
/// No cold-start rewrite, like `.ac`: a noise analysis is *defined* as a linearization about the
/// DC operating point, so QSPICE's own "solve DC first" is exactly what's wanted.
fn gen_noise_golden(
    qspice: &Path,
    root: &Path,
    tmp: &Path,
    circuit: &str,
    native_deck: &str,
) -> Result<u32> {
    let deck = read_circuit(root, circuit)?;
    let net = va_netlist::parser::parse(&deck).with_context(|| format!("parsing {circuit}"))?;
    let card = net
        .noise
        .as_ref()
        .with_context(|| format!("{circuit} has no `.noise` card"))?;
    let (output, source) = (card.output.clone(), card.source.clone());
    let stem = Path::new(circuit)
        .file_stem()
        .context("circuit path has no file stem")?
        .to_string_lossy()
        .into_owned();

    // A `.noise` `.qraw` is `Flags: real` and point-major — the same shape `.dc`/`.tran`
    // produce, so it reuses `run_qspice_sweep` rather than needing its own reader.
    let raw = run_qspice_sweep(qspice, native_deck, tmp, &stem)
        .with_context(|| format!("running QSPICE on {circuit}"))?;
    let devices = golden_noise_devices(root, circuit)
        .with_context(|| format!("resolving noise contributors for {circuit}"))?;
    let golden = golden_noise_from_qraw(&raw, &output, &source, &devices)
        .with_context(|| format!("mapping QSPICE output to golden for {circuit}"))?;

    let golden_path = root.join("golden").join(format!("{stem}.golden"));
    std::fs::write(&golden_path, golden.render())
        .with_context(|| format!("writing {}", golden_path.display()))?;
    eprintln!(
        "[xtask]   wrote {} ({} frequency point(s), noise PSD at V({output}), {} device(s))",
        golden_path.display(),
        golden.points.len(),
        golden.devices.len()
    );
    Ok(1)
}

/// Like [`run_qspice_sweep`], for an `.ac` deck — same invocation, complex `.qraw` payload
/// ([`parse_qraw_ac`]).
fn run_qspice_ac(qspice: &Path, deck: &str, workdir: &Path, stem: &str) -> Result<QspiceRawAc> {
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
    parse_qraw_ac(&bytes)
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

/// One QSPICE `.qraw` file's contents for an `.ac` run (T5) — the only analysis whose payload is
/// **complex** (`Flags: complex` in the header).
///
/// Its layout is not simply "twice as many f64s": confirmed empirically against a real
/// `.ac dec 2 1 1meg` run of `circuits/rc_ac.net`, a 5-variable 12-point file carries 108 f64s,
/// i.e. **9** per point — the `Frequency` abscissa is stored as a single real value, and only the
/// remaining 4 variables get a `(re, im)` pair each. A naive "every variable is complex" read
/// (10 per point) would silently misalign every value after the first.
struct QspiceRawAc {
    /// Variable names in declared order **excluding** the `Frequency` abscissa, so an index into
    /// this lines up directly with an index into each point's own complex value list.
    variables: Vec<String>,
    /// `(frequency, one complex value per `variables` entry)`, in point-major order.
    points: Vec<(f64, Vec<(f64, f64)>)>,
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

/// Parse an `.ac` (complex) `.qraw` file — see [`QspiceRawAc`] for the layout and why it isn't
/// just [`parse_qraw_sweep`] with doubled values.
///
/// # Errors
///
/// If [`parse_qraw_header`] fails, the file has zero points or no variables beyond the abscissa,
/// or the binary payload is shorter than the header promises.
fn parse_qraw_ac(bytes: &[u8]) -> Result<QspiceRawAc> {
    let (mut variables, n_points, payload) = parse_qraw_header(bytes)?;
    if n_points == 0 {
        bail!("`.qraw` declares 0 points");
    }
    if variables.len() < 2 {
        bail!("`.qraw` declares no AC variables beyond the frequency abscissa");
    }
    // Drop the abscissa (`Frequency`), which is stored as one real value per point.
    variables.remove(0);
    let n_complex = variables.len();
    let per_point = 1 + 2 * n_complex;
    if payload.len() < n_points * per_point * 8 {
        bail!(
            "`.qraw` binary payload is {} byte(s), too short for {n_points} point(s) of 1 real \
             frequency + {n_complex} complex value(s) each",
            payload.len()
        );
    }
    let f64_at = |i: usize| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&payload[i * 8..i * 8 + 8]);
        f64::from_le_bytes(b)
    };
    let points = (0..n_points)
        .map(|p| {
            let base = p * per_point;
            let values = (0..n_complex)
                .map(|v| (f64_at(base + 1 + 2 * v), f64_at(base + 2 + 2 * v)))
                .collect();
            (f64_at(base), values)
        })
        .collect();
    Ok(QspiceRawAc { variables, points })
}

/// Map a parsed `.ac` `.qraw` onto this project's own `node_order`, keyed by each point's own
/// frequency — the AC analogue of [`golden_tran_from_qraw`]/[`golden_sweep_from_qraw`]. Labels
/// resolve exactly as they do for every other analysis ([`node_values_from_row`]'s own
/// `V(<node>)` / literal `I(<device>)` convention), just yielding complex values.
fn golden_ac_from_qraw(
    raw: &QspiceRawAc,
    node_order: &[String],
) -> Result<va_harness::golden::GoldenAc> {
    let points = raw
        .points
        .iter()
        .map(|(freq, row)| {
            // Compare component-wise by reusing the real lookup twice rather than duplicating
            // the label convention: one pass over the real parts, one over the imaginary.
            let re: Vec<f64> = row.iter().map(|&(re, _)| re).collect();
            let im: Vec<f64> = row.iter().map(|&(_, im)| im).collect();
            let re = node_values_from_row(&raw.variables, &re, node_order)?;
            let im = node_values_from_row(&raw.variables, &im, node_order)?;
            Ok((*freq, re.into_iter().zip(im).collect()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(va_harness::golden::GoldenAc {
        node_order: node_order.to_vec(),
        points,
    })
}

/// The `.qraw` variable QSPICE writes the total output noise PSD to.
///
/// A `.noise` run also emits one `V(onoise_<device>)` column per contributing device (and
/// `V(inoise_spectrum)`, the input-referred spectrum), all of which this project ignores: its own
/// [`va_harness::golden::GoldenNoise`] records the summed output spectrum only, matching what
/// `va_acnoise::noise` computes (§ its stated limitations — no per-device breakdown, no
/// input-referral). Confirmed against a real run: the per-device columns sum exactly to this one.
const QSPICE_ONOISE_TOTAL: &str = "V(onoise_spectrum)";

/// The `.qraw` variable QSPICE writes the **input-referred** total to — the output spectrum
/// divided by the squared gain from the `.noise` card's own input source.
const QSPICE_INOISE_TOTAL: &str = "V(inoise_spectrum)";

/// Map a parsed `.noise` `.qraw` onto a [`va_harness::golden::GoldenNoise`], keyed by frequency.
///
/// A noise `.qraw` is `Flags: real` and point-major, so it needs no complex parsing at all —
/// [`parse_qraw_sweep`] reads it unchanged, and only the *column selection* is noise-specific.
/// `devices` names each noise-contributing device in **this project's** own spelling (`R1`,
/// `D1`), resolved by [`golden_noise_devices`]; QSPICE spells the matching column
/// `V(onoise_r1)`, lowercased, so the lookup is case-insensitive like every other one here.
///
/// QSPICE also emits per-*mechanism* sub-columns for a device (`V(onoise_d1.id)`,
/// `V(onoise_d1.1overf)`, `V(onoise_d1.rs)`); only the aggregate `V(onoise_d1)` is read, since
/// this project attributes noise per device rather than per mechanism (§ `va_acnoise::noise`).
fn golden_noise_from_qraw(
    raw: &QspiceRawSweep,
    output: &str,
    source: &str,
    devices: &[String],
) -> Result<va_harness::golden::GoldenNoise> {
    let column = |label: &str| -> Result<usize> {
        raw.variables
            .iter()
            .position(|v| v.eq_ignore_ascii_case(label))
            .with_context(|| format!("QSPICE noise output has no `{label}` variable"))
    };
    let freq_idx = column("Frequency")?;
    let psd_idx = column(QSPICE_ONOISE_TOTAL)?;
    let input_idx = column(QSPICE_INOISE_TOTAL)?;
    let device_idx: Vec<usize> = devices
        .iter()
        .map(|d| column(&format!("V(onoise_{d})")))
        .collect::<Result<_>>()?;
    Ok(va_harness::golden::GoldenNoise {
        output: output.to_string(),
        source: source.to_string(),
        devices: devices.to_vec(),
        points: raw
            .points
            .iter()
            .map(|row| {
                (
                    row[freq_idx],
                    row[psd_idx],
                    row[input_idx],
                    device_idx.iter().map(|&i| row[i]).collect(),
                )
            })
            .collect(),
    })
}

/// The devices that contribute noise in `circuit`, in this project's own naming and in the order
/// its own analysis reports them.
///
/// Resolved by *running* the analysis, because only a solve knows which devices actually
/// contribute — a reverse-biased junction at zero current, or a model with `KF = 0`, emits
/// nothing and correctly gets no column. This mirrors [`golden_node_order`], which likewise
/// loads the circuit through the real pipeline so the golden file's own columns match exactly
/// what `validate` will later compare against.
fn golden_noise_devices(root: &Path, circuit: &str) -> Result<Vec<String>> {
    let circuit_path = root.join(circuit);
    let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
    let model_path = va_model_for(circuit).map(|m| root.join(m));
    let model_str = model_path
        .as_deref()
        .map(|p| p.to_str().context("non-UTF8 model path"))
        .transpose()?;
    let (net, compiled) = va_cli::load(circuit_str, model_str)
        .with_context(|| format!("loading {circuit} for noise-contributor resolution"))?;
    let spectrum = va_cli::solve_noise(&net, &compiled)
        .with_context(|| format!("solving {circuit} for noise-contributor resolution"))?;
    Ok(va_cli::noise_contributors(&net, &spectrum)
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Look up each of `node_order`'s labels in `variables`/`row` — shared by [`golden_dc_from_qraw`]
/// and [`golden_sweep_from_qraw`]. QSPICE's own variable ordering isn't assumed to match
/// `node_order`'s (it happened to, for `divider.net`, but nothing guarantees that in general).
///
/// An entry already shaped `"I(<name>)"` (§ `va_harness::golden`'s branch-current convention) is
/// looked up *literally* — QSPICE spells a device's own current the same way in its own `.qraw`
/// variable list — rather than wrapped as `"V(I(<name>))"`; every other entry is a bare node name,
/// wrapped as `"V(<name>)"` as before.
fn node_values_from_row(
    variables: &[String],
    row: &[f64],
    node_order: &[String],
) -> Result<Vec<f64>> {
    node_order
        .iter()
        .map(|name| {
            let label = if name.starts_with("I(") && name.ends_with(')') {
                name.clone()
            } else {
                format!("V({name})")
            };
            variables
                .iter()
                .position(|v| v.eq_ignore_ascii_case(&label))
                .map(|i| row[i])
                .with_context(|| format!("QSPICE output has no `{label}` variable"))
        })
        .collect()
}

/// The Verilog-A `--model` path (if any) `va-harness`'s own `run_dc`/`run_dc_sweep`/`run_tran`
/// compiles for `circuit` at validate time (§ `DC_CIRCUITS`/`SWEEP_CIRCUITS`/`TRAN_CIRCUITS`) —
/// shared so [`golden_node_order`]'s own branch-current resolution matches exactly what
/// `validate` will later compare against. Distinct from `gen_golden`'s own QSPICE-side model
/// translation (`QSPICE_MODEL_TRANSLATIONS` et al.), which is a QSPICE-native `.model` card
/// string, not a `.va` path — a device like `mos_dc.net`'s `M1` needs the *compiled* model here
/// to resolve at all (it has no hand-written `va-abi` reference, unlike `resistor`/`diode`/`bjt`).
fn va_model_for(circuit: &str) -> Option<&'static str> {
    DC_CIRCUITS
        .iter()
        .chain(SWEEP_CIRCUITS)
        .chain(TRAN_CIRCUITS)
        .chain(AC_CIRCUITS)
        .chain(NOISE_CIRCUITS)
        .find(|(c, _)| *c == circuit)
        .and_then(|&(_, m)| m)
}

/// `circuit`'s own `node_order` widened with an `"I(<device name>)"` entry per `vsource` device
/// (§ `va_harness::golden`'s branch-current convention) — shared by every `gen_golden` circuit
/// kind so a golden file's own comparison also exercises the device actually driving that
/// source, not just whatever node voltage it directly forces. Compiles `circuit`'s own
/// `va_model_for` model (if any) through the real pipeline (`va_cli::load`), exactly as
/// `va-harness`'s own `run_dc`/`run_dc_sweep`/`run_tran` will at validate time, so a device with
/// no hand-written `va-abi` reference (e.g. `mos_dc.net`'s `mosfet`) resolves correctly too.
fn golden_node_order(root: &Path, circuit: &str) -> Result<Vec<String>> {
    let circuit_path = root.join(circuit);
    let circuit_str = circuit_path.to_str().context("non-UTF8 circuit path")?;
    let model_path = va_model_for(circuit).map(|m| root.join(m));
    let model_str = model_path
        .as_deref()
        .map(|p| p.to_str().context("non-UTF8 model path"))
        .transpose()?;
    let (net, compiled) = va_cli::load(circuit_str, model_str)
        .with_context(|| format!("loading {circuit} for branch-current resolution"))?;
    let branch_currents =
        va_cli::branch_currents(&net, &compiled).context("resolving branch currents")?;
    let mut node_order = net.node_order;
    node_order.extend(branch_currents.iter().map(|(name, _)| format!("I({name})")));
    Ok(node_order)
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
    // `node_order` already carries any `I(<name>)` branch-current labels (§ `golden_node_order`),
    // and `points`' own values already line up with it one-for-one — passing an empty
    // `branch_currents` here means `from_sweep`'s own append step is a no-op, not a double-count.
    Ok(va_harness::golden::GoldenSweep::from_sweep(
        source,
        node_order,
        &points,
        &[],
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

/// Node counts `bench_linsolve` sweeps over. Roughly geometric spacing so both ends of the
/// curve are represented — the low end, where dense is obviously fine, and the high end, where
/// `O(n³)` time / `O(n²)` space asymptotics start to bite — without spending the whole run on
/// closely-spaced redundant points. `dim = n_nodes + 1` ([`build_ladder`]'s `+1` is the driving
/// source's own branch-current unknown). Capped at `10_000` (`dim = 10_001`, ~763 MB of dense
/// `f64` storage — the point where dense genuinely starts to hurt on ordinary dev hardware) —
/// the *sparse* solver stays fast enough at every measured size that the whole sweep still
/// finishes in well under a minute on release hardware; dense's own `O(n³)` cost is what sets
/// the practical ceiling. See this module's own doc comment on [`bench_linsolve`] for measured
/// wall time.
const BENCH_NODE_COUNTS: &[usize] = &[
    10, 20, 50, 100, 200, 500, 1000, 1500, 2000, 3000, 4000, 6000, 8000, 10_000,
];

/// Build a leaky resistor-ladder MNA system with `n_nodes` nodes, through real
/// `va_abi::reference` instances — not a hand-fabricated matrix.
///
/// Topology: node `0` is driven by an ideal 5 V source (contributing its own branch-current
/// unknown, `dim = n_nodes + 1`); every node `i` also carries its own 1 kΩ shunt resistor to
/// ground (so the system is well-conditioned regardless of size, and no node is left floating);
/// each adjacent pair `(i, i+1)` is joined by a 100 Ω series resistor. That is a leaky
/// RC-chain/transmission-line shape, deliberately *not* a dense mesh: every real analog netlist
/// this project's own `circuits/` zoo contains (a resistor ladder, an RC chain, a bus of similar
/// cells) has each node touching only a handful of physical neighbors, which is exactly what
/// makes a dense `n × n` Jacobian mostly zeros as `n` grows — the phenomenon this benchmark
/// exists to quantify. Every stamp goes through the real `Resistor`/`VSource`
/// [`ModelInstance::load`], via [`va_core::mna::assemble`] — the same assembly path
/// `newton`/`dc` use, not a synthetic matrix built by hand.
fn build_ladder(n_nodes: usize) -> (VSource, Vec<Resistor>) {
    let branch = n_nodes;
    let vs = VSource::new(0, GROUND, branch, 5.0);
    let mut resistors = Vec::with_capacity(2 * n_nodes);
    for i in 0..n_nodes {
        resistors.push(Resistor::new(i, GROUND, 1_000.0)); // shunt to ground
    }
    for i in 0..n_nodes.saturating_sub(1) {
        resistors.push(Resistor::new(i, i + 1, 100.0)); // series chain
    }
    (vs, resistors)
}

/// One measured row of `bench_linsolve`'s table.
struct BenchRow {
    n_nodes: usize,
    dim: usize,
    nnz: usize,
    dense_bytes: usize,
    dense_time: Duration,
    /// `None` if `solve_sparse` itself failed (see [`va_core::linsolve::solve_sparse`]'s own
    /// `# Errors`) — kept distinct from a slow-but-successful solve.
    sparse_time: Option<Duration>,
    /// `Some(false)` if the sparse and dense solutions disagree beyond a loose sanity tolerance
    /// — would indicate a real correctness bug, not an expected benchmark outcome. `None` when
    /// there is no sparse solution to compare (`sparse_time` was `None`).
    sparse_matches_dense: Option<bool>,
}

/// Build the ladder at `n_nodes`, assemble its MNA system, and time one dense and one sparse
/// solve of the resulting Newton-step system `J·x = -f` (using the assembled residual as `-b`,
/// exactly what `newton::solve` itself would hand a linear solver at this operating point).
fn run_bench_row(n_nodes: usize) -> Result<BenchRow> {
    let (vs, resistors) = build_ladder(n_nodes);
    let dim = n_nodes + 1;
    let mut insts: Vec<&dyn ModelInstance> = Vec::with_capacity(resistors.len() + 1);
    insts.push(&vs);
    for r in &resistors {
        insts.push(r);
    }

    let x = vec![0.0; dim];
    let sys = va_core::mna::assemble(&insts, &x, &va_abi::ANALYSIS_DC, dim);
    let nnz = va_core::linsolve::nnz(&sys.jacobian, dim);
    let dense_bytes = dim * dim * std::mem::size_of::<f64>();
    let b: Vec<f64> = sys.residual.iter().map(|v| -v).collect();

    let t0 = Instant::now();
    let dense_x = va_core::linsolve::solve_dense(&sys.jacobian, &b, dim)
        .with_context(|| format!("dense solve failed at n_nodes={n_nodes} (dim={dim})"))?;
    let dense_time = t0.elapsed();

    let t1 = Instant::now();
    let (sparse_time, sparse_matches_dense) =
        match va_core::linsolve::solve_sparse(&sys.jacobian, &b, dim) {
            Ok(sparse_x) => {
                let elapsed = t1.elapsed();
                let max_diff = dense_x
                    .iter()
                    .zip(sparse_x.iter())
                    .fold(0.0_f64, |m, (a, s)| m.max((a - s).abs()));
                (Some(elapsed), Some(max_diff < 1e-6))
            }
            Err(e) => {
                eprintln!(
                    "[xtask]   bench-linsolve: sparse solve failed at n_nodes={n_nodes}: {e}"
                );
                (None, None)
            }
        };

    Ok(BenchRow {
        n_nodes,
        dim,
        nnz,
        dense_bytes,
        dense_time,
        sparse_time,
        sparse_matches_dense,
    })
}

/// Sparse-solve benchmark answering the `docs/roadmap.md` T3 backlog question: scale a real
/// leaky resistor-ladder MNA system ([`build_ladder`], assembled through
/// [`va_core::mna::assemble`] over real `va_abi::reference::{Resistor, VSource}` instances)
/// across [`BENCH_NODE_COUNTS`], and report — per size — the assembled Jacobian's actual fill
/// fraction, the dense `n×n` buffer's `O(n²)` memory footprint, dense solve wall time, and
/// (where `faer`'s sparse LU is applicable — see [`va_core::linsolve::solve_sparse`]) sparse
/// solve wall time and the resulting speedup.
///
/// Deterministic and re-runnable: fixed node counts, fixed resistances, no randomness — the
/// same run twice gives the same table modulo wall-clock noise. Does not run under `cargo test
/// --workspace` (it is a `main`-only binary path, not a `#[test]`), so it never affects normal
/// CI/test wall time.
///
/// **Re-run with:** `cargo xtask bench-linsolve` (release build recommended for the upper end
/// of [`BENCH_NODE_COUNTS`]: `cargo build --release -p xtask` first, or accept slower debug
/// timings). See `docs/roadmap.md`'s T3 section for the last captured run's numbers, the
/// crossover (or its absence), and the resulting recommendation.
///
/// # Errors
///
/// If a dense solve fails at some `n_nodes` — this ladder is well-conditioned by construction
/// (every node has its own shunt to ground), so that would indicate a real bug, not an expected
/// benchmark outcome.
fn bench_linsolve() -> Result<()> {
    eprintln!(
        "[xtask] bench-linsolve: dense vs. sparse MNA solve over a leaky resistor ladder \
         (va_abi::reference::{{Resistor, VSource}} via va_core::mna::assemble) …"
    );

    // Warm up both solvers once (rayon thread-pool spin-up, allocator first-touch, icache) on a
    // throwaway system before timing anything — without this the very first measured row pays a
    // one-time cost unrelated to `n`, which would read as (and briefly did read as, in an
    // untrimmed run) a spuriously *slower* small-n dense time than a larger one right after it.
    let _ = run_bench_row(5);

    eprintln!(
        "[xtask]   {:>7} {:>6} {:>8} {:>8} {:>10} {:>11} {:>11} {:>9}",
        "n_nodes", "dim", "nnz", "fill", "dense_MB", "dense_ms", "sparse_ms", "speedup"
    );

    let mut measured = 0usize;
    for &n_nodes in BENCH_NODE_COUNTS {
        let row = run_bench_row(n_nodes)?;
        let fill = row.nnz as f64 / (row.dim * row.dim) as f64;
        let dense_mb = row.dense_bytes as f64 / (1024.0 * 1024.0);
        let sparse_ms_str = row
            .sparse_time
            .map(|d| format!("{:.3}", d.as_secs_f64() * 1e3))
            .unwrap_or_else(|| "FAILED".to_string());
        let speedup_str = row
            .sparse_time
            .map(|d| {
                format!(
                    "{:.2}x",
                    row.dense_time.as_secs_f64() / d.as_secs_f64().max(f64::EPSILON)
                )
            })
            .unwrap_or_else(|| "n/a".to_string());
        eprintln!(
            "[xtask]   {:>7} {:>6} {:>8} {:>8.4} {:>10.3} {:>11.3} {:>11} {:>9}",
            row.n_nodes,
            row.dim,
            row.nnz,
            fill,
            dense_mb,
            row.dense_time.as_secs_f64() * 1e3,
            sparse_ms_str,
            speedup_str,
        );
        if row.sparse_matches_dense == Some(false) {
            eprintln!(
                "[xtask]   WARNING: dense/sparse solutions disagree beyond tolerance at \
                 n_nodes={}",
                row.n_nodes
            );
        }
        measured += 1;
    }

    eprintln!(
        "[xtask] bench-linsolve: {measured} size(s) measured; see docs/roadmap.md's T3 section \
         for the write-up (crossover, recommendation)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fast structural sanity check on the ladder builder — the full `bench_linsolve` sweep is
    /// deliberately *not* exercised here (§ its own doc comment: minutes-scale by design, and
    /// `cargo test --workspace` must not slow down for it). This just confirms the topology is
    /// what the doc comment claims, at a size cheap enough to assert on by hand.
    #[test]
    fn build_ladder_has_the_claimed_topology() {
        let (vs, resistors) = build_ladder(3);
        // 3 nodes -> 3 shunt resistors + 2 series resistors = 5.
        assert_eq!(resistors.len(), 5);
        let dim = 3 + 1; // + branch current
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs];
        for r in &resistors {
            insts.push(r);
        }
        let x = vec![0.0; dim];
        let sys = va_core::mna::assemble(&insts, &x, &va_abi::ANALYSIS_DC, dim);
        // Solves without error, and node 0 (directly driven) sits at the source voltage.
        let b: Vec<f64> = sys.residual.iter().map(|v| -v).collect();
        let x = va_core::linsolve::solve_dense(&sys.jacobian, &b, dim).expect("well-conditioned");
        assert!((x[0] - 5.0).abs() < 1e-9, "V(0) = {}", x[0]);
        // Every downstream node sees a nonzero fraction of the drive (leaky divider chain).
        for &v in &x[1..3] {
            assert!(v > 0.0 && v < 5.0, "v = {v}");
        }
    }

    /// `run_bench_row` at a small size: dense and sparse must agree, and the reported fill
    /// fraction must reflect the tridiagonal-plus-branch-row shape (sparse, not dense).
    #[test]
    fn run_bench_row_agrees_dense_vs_sparse_at_a_small_size() {
        let row = run_bench_row(8).expect("well-conditioned ladder solves");
        assert_eq!(row.dim, 9);
        assert_eq!(row.sparse_matches_dense, Some(true));
        // Well under half-full: each node row touches only its neighbors + the shunt + (for
        // node 0) the source branch — nowhere near a dense n*n fill.
        let fill = row.nnz as f64 / (row.dim * row.dim) as f64;
        assert!(fill < 0.5, "fill = {fill}");
    }

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
        // `TRAN_CIRCUITS`/`AC_CIRCUITS` has real, committed QSPICE golden (all six ladder rungs,
        // plus T5's two AC circuits) — `divider.net`/`rc_step.net`/`rc_ac.net` unmodified
        // (`QSPICE_NATIVE_CIRCUITS`/`QSPICE_NATIVE_TRAN_CIRCUITS`/`QSPICE_NATIVE_AC_CIRCUITS`);
        // `mos_dc.net`/`diode_iv.net`/`rectifier.net`/`ring_osc.net`/`diode_ac.net`/
        // `diode_noise.net` from a native-model translation (`ring_osc.net`'s golden additionally
        // truncated to `RING_OSC_GOLDEN_TSTOP`, its own doc comment explains why). Nothing is
        // skipped anymore; `validate` must pass all nine for real.
        validate().expect("validate should pass: all nine known circuits have real golden");
    }

    #[test]
    fn try_solve_tracks_a_non_convergent_circuit_without_erroring() {
        // A resistor between two nets with no path to ground anywhere is singular, not just
        // slow to converge (confirmed empirically: `va_core::CoreError::Singular`, propagated as
        // `HarnessError::Run("... singular matrix during linear solve")`) — a real, if synthetic,
        // non-convergent circuit, not a hand-waved one.
        let dir = std::env::temp_dir().join("va_xtask_try_solve_test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("floating.net");
        std::fs::write(&path, "R1 a b 1000\n.op\n.end\n").expect("write scratch deck");
        let circuit_str = path.to_str().unwrap();

        let mut tally = Tally::default();
        let got = try_solve(
            "floating.net",
            || va_harness::dc::run_dc(circuit_str, None),
            &mut tally,
        );

        assert!(
            got.is_none(),
            "a singular circuit should not report Some(_)"
        );
        assert_eq!(tally.checked, 1);
        assert_eq!(tally.not_converged, 1);
        assert_eq!(
            tally.failed, 0,
            "non-convergence must not also count as a golden mismatch"
        );
        assert_eq!(tally.converged(), 0);
    }

    #[test]
    fn tally_converged_excludes_non_convergent_circuits_from_the_denominator_numerator() {
        let mut tally = Tally {
            checked: 5,
            not_converged: 2,
            ..Tally::default()
        };
        assert_eq!(tally.converged(), 3);
        tally.merge(Tally {
            checked: 1,
            not_converged: 1,
            ..Tally::default()
        });
        assert_eq!(tally.checked, 6);
        assert_eq!(tally.not_converged, 3);
        assert_eq!(tally.converged(), 3);
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

    /// Build a synthetic complex (`.ac`) `.qraw` byte buffer with the layout a real QSPICE AC run
    /// produces (confirmed against an actual run of `circuits/rc_ac.net`): one **real** frequency
    /// value per point, then a `(re, im)` pair per remaining variable.
    fn synthetic_qraw_ac(vars: &[&str], rows: &[(f64, Vec<(f64, f64)>)]) -> Vec<u8> {
        let mut header = String::new();
        header.push_str("Title: * synthetic ac\n");
        header.push_str("Plotname: AC Analysis\n");
        header.push_str("Flags: complex\n");
        header.push_str(&format!("No. Variables: {}\n", vars.len()));
        header.push_str(&format!("No. Points: {}                    \n", rows.len()));
        header.push_str("Variables:\n");
        for (i, name) in vars.iter().enumerate() {
            header.push_str(&format!("\t{i}\t{name}\tvoltage\n"));
        }
        header.push_str("Binary:\n");
        let mut bytes = header.into_bytes();
        for (freq, values) in rows {
            bytes.extend_from_slice(&freq.to_le_bytes());
            for (re, im) in values {
                bytes.extend_from_slice(&re.to_le_bytes());
                bytes.extend_from_slice(&im.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn parse_qraw_ac_reads_a_real_abscissa_and_complex_values() {
        // The whole point of `QspiceRawAc`'s layout note: 2 points × (1 real + 2 complex) = 10
        // f64s, *not* 12. A naive all-complex read would misalign everything after the first
        // value — so this asserts the exact values land where they belong.
        let bytes = synthetic_qraw_ac(
            &["Frequency", "V(out)", "I(V1)"],
            &[
                (1.0, vec![(0.99, -0.006), (-3.9e-8, -6.2e-6)]),
                (10.0, vec![(0.9, -0.06), (-3.9e-7, -6.2e-5)]),
            ],
        );
        let raw = parse_qraw_ac(&bytes).expect("parse");
        // The `Frequency` abscissa is dropped from `variables` so indices line up with values.
        assert_eq!(raw.variables, vec!["V(out)", "I(V1)"]);
        assert_eq!(raw.points.len(), 2);
        assert_eq!(raw.points[0].0, 1.0);
        assert_eq!(raw.points[0].1, vec![(0.99, -0.006), (-3.9e-8, -6.2e-6)]);
        assert_eq!(raw.points[1].0, 10.0);
        assert_eq!(raw.points[1].1, vec![(0.9, -0.06), (-3.9e-7, -6.2e-5)]);
    }

    #[test]
    fn parse_qraw_ac_rejects_a_truncated_payload() {
        let mut bytes = synthetic_qraw_ac(
            &["Frequency", "V(out)"],
            &[(1.0, vec![(0.99, -0.006)]), (10.0, vec![(0.9, -0.06)])],
        );
        bytes.truncate(bytes.len() - 4); // half of the last imaginary part missing
        assert!(parse_qraw_ac(&bytes).is_err());
    }

    #[test]
    fn parse_qraw_ac_rejects_a_file_with_only_an_abscissa() {
        let bytes = synthetic_qraw_ac(&["Frequency"], &[(1.0, vec![])]);
        assert!(parse_qraw_ac(&bytes).is_err());
    }

    #[test]
    fn golden_ac_from_qraw_looks_up_by_name_regardless_of_order() {
        // QSPICE's own variable order needn't match this project's `node_order`, and a branch
        // current is looked up literally as `I(V1)` while a node becomes `V(out)`.
        let raw = QspiceRawAc {
            variables: vec!["I(V1)".to_string(), "V(out)".to_string()],
            points: vec![(1.0, vec![(-3.9e-8, -6.2e-6), (0.99, -0.006)])],
        };
        let node_order = vec!["out".to_string(), "I(V1)".to_string()];
        let golden = golden_ac_from_qraw(&raw, &node_order).expect("map");
        assert_eq!(golden.node_order, node_order);
        assert_eq!(
            golden.points,
            vec![(1.0, vec![(0.99, -0.006), (-3.9e-8, -6.2e-6)])]
        );
    }

    /// A real `.noise` variable list, including the per-*mechanism* sub-columns QSPICE emits for
    /// a device. The aggregate `V(onoise_d1)` is the one taken; picking `V(onoise_d1.id)` by
    /// accident would silently golden a fraction of that device's real noise.
    fn synthetic_noise_qraw() -> QspiceRawSweep {
        QspiceRawSweep {
            variables: vec![
                "Frequency".to_string(),
                "V(onoise_r1)".to_string(),
                "V(onoise_d1.id)".to_string(),
                "V(onoise_d1.1overf)".to_string(),
                "V(onoise_d1)".to_string(),
                "V(onoise_spectrum)".to_string(),
                "V(inoise_spectrum)".to_string(),
            ],
            points: vec![
                vec![
                    10.0, 6.62e-19, 1.0e-18, 3.25e-19, 1.325e-18, 1.9877e-18, 4.975e-17,
                ],
                vec![
                    100.0, 6.62e-19, 1.0e-18, 3.25e-19, 1.325e-18, 1.9877e-18, 4.975e-17,
                ],
            ],
        }
    }

    #[test]
    fn golden_noise_from_qraw_picks_the_totals_and_per_device_columns() {
        let raw = synthetic_noise_qraw();
        let devices = vec!["R1".to_string(), "D1".to_string()];
        let golden = golden_noise_from_qraw(&raw, "a", "V1", &devices).expect("map");
        assert_eq!(golden.output, "a");
        assert_eq!(golden.source, "V1");
        assert_eq!(golden.devices, devices);
        // Per-device columns are the *aggregates*, not the `.id`/`.1overf` sub-mechanisms — and
        // the lookup is case-insensitive, since QSPICE lowercases device names.
        assert_eq!(
            golden.points,
            vec![
                (10.0, 1.9877e-18, 4.975e-17, vec![6.62e-19, 1.325e-18]),
                (100.0, 1.9877e-18, 4.975e-17, vec![6.62e-19, 1.325e-18])
            ]
        );
    }

    #[test]
    fn golden_noise_from_qraw_errors_on_a_device_with_no_column() {
        let raw = synthetic_noise_qraw();
        // `Q9` isn't in this run at all — better to fail than to emit a zero column that would
        // read as "this device is silent".
        let devices = vec!["R1".to_string(), "Q9".to_string()];
        assert!(golden_noise_from_qraw(&raw, "a", "V1", &devices).is_err());
    }

    #[test]
    fn golden_noise_from_qraw_errors_without_a_total_column() {
        let none: [String; 0] = [];
        let raw = QspiceRawSweep {
            variables: vec!["Frequency".to_string(), "V(onoise_r1)".to_string()],
            points: vec![vec![10.0, 6.62e-19]],
        };
        assert!(golden_noise_from_qraw(&raw, "a", "V1", &none).is_err());

        // Present output total, missing input-referred total — also an error, rather than a
        // golden file with a silently-zero second column.
        let raw = QspiceRawSweep {
            variables: vec!["Frequency".to_string(), "V(onoise_spectrum)".to_string()],
            points: vec![vec![10.0, 1.9877e-18]],
        };
        assert!(golden_noise_from_qraw(&raw, "a", "V1", &none).is_err());
    }

    #[test]
    fn golden_ac_from_qraw_errors_on_a_missing_node() {
        let raw = QspiceRawAc {
            variables: vec!["V(out)".to_string()],
            points: vec![(1.0, vec![(0.99, -0.006)])],
        };
        let node_order = vec!["out".to_string(), "missing".to_string()];
        assert!(golden_ac_from_qraw(&raw, &node_order).is_err());
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
        // "gnd" is also normalized to "0" (rewrite_gnd_to_zero) before widening.
        assert!(
            out.lines().any(|l| l == "M1 d g 0 0 mosfet"),
            "expected a body=source-widened, gnd-normalized M line, got:\n{out}"
        );
    }

    #[test]
    fn rewrite_gnd_to_zero_matches_whole_tokens_only_and_skips_comments() {
        let deck = "* gnd in a comment stays untouched\nQ1 c b gnd bjt\nR1 gnd background 1k\n";
        let out = rewrite_gnd_to_zero(deck);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "* gnd in a comment stays untouched");
        assert_eq!(lines[1], "Q1 c b 0 bjt");
        // "background" contains "gnd" as a substring but must not be rewritten.
        assert_eq!(lines[2], "R1 0 background 1k");
    }

    /// The behavioral substitution replaces exactly the named device, leaves the title and
    /// every other line alone, and normalizes `gnd` on the way through.
    #[test]
    fn substitute_device_line_replaces_exactly_the_named_device() {
        let deck = "* D1 in a comment is not a device\n\
                    D1 out gnd abstime_ramp\n\
                    D10 out gnd other\n\
                    R1 out gnd 1000\n\
                    .tran 20u 1m\n.end\n";
        let out = substitute_device_line(deck, "D1", "B1 out 0 I=1*time").expect("substitutes");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "* D1 in a comment is not a device");
        assert_eq!(lines[1], "B1 out 0 I=1*time");
        // `D10` must not match `D1` — whole-token comparison, not a prefix.
        assert_eq!(lines[2], "D10 out 0 other");
        assert_eq!(lines[3], "R1 out 0 1000");
    }

    /// A substitution that silently matched nothing would still produce a *runnable* deck —
    /// QSPICE would solve the circuit without the device, since the untouched line names a
    /// model it has never heard of — and write a plausible, entirely wrong golden file. That is
    /// the `no-fake-golden-data` failure mode, so it must be an error.
    #[test]
    fn substitute_device_line_refuses_to_silently_match_nothing() {
        let deck = "* title\nR1 out gnd 1000\n.tran 20u 1m\n.end\n";
        assert!(substitute_device_line(deck, "D1", "B1 out 0 I=1*time").is_err());
        // And refuses an ambiguous match rather than picking one.
        let dup = "* title\nD1 a 0 m\nD1 b 0 m\n.end\n";
        assert!(substitute_device_line(dup, "D1", "B1 out 0 I=1*time").is_err());
    }

    #[test]
    fn truncate_tran_tstop_replaces_only_the_stop_time() {
        let deck = "* title\nV1 a 0 DC 1\n.tran 100u 0.2 UIC\n.end\n";
        let out = truncate_tran_tstop(deck, 0.1);
        assert!(
            out.lines().any(|l| l == ".tran 100u 0.1 UIC"),
            "expected tstep/UIC preserved, tstop replaced, got:\n{out}"
        );
    }
}
