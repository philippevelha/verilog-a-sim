//! The error corpus: every file in `tests/fixtures/errors/` is a Verilog-A model with one
//! deliberate mistake, and a header saying what `va-cli check --codegen` must report for it.
//!
//! The binary is run as a user would run it, so what is checked is what a user reads: the
//! stage that rejected the file, the words of the message, the location, and the exit code.
//! The header format is described in `tests/fixtures/errors/README.md`.
//!
//! Two kinds of entry are not plain assertions, and both exist to keep the corpus honest:
//!
//! - `expect: ok` with a `gap:` line records invalid Verilog-A that is **still accepted**. The
//!   test requires it to stay accepted, so the change that finally rejects it has to come here
//!   and promote the file — a gap cannot close (or reopen) without the corpus noticing.
//! - `poor:` records a defect in an otherwise correct diagnostic (wrong line, an internal token
//!   name). It is not asserted, because asserting it would lock the defect in; it is counted
//!   and printed, so the list of messages worth improving lives next to the inputs that show it.
//! - `expect: ok` with a `warning:` line records questionable source that is accepted **with a
//!   warning**; the warning text is asserted, on a `[warn ]` line.
//!
//! Every rejected or warned file also names its diagnostic `code:`, so a change that moves a
//! problem to another code, or drops it back to the uncoded `E0000`, is noticed.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What one corpus file says `check` must report.
#[derive(Debug, Default)]
struct Expectation {
    /// `pp`, `lex`, `parse`, `elab`, `cgen`, `refused`, or `ok`.
    stage: String,
    /// Substrings that must all appear in the output.
    messages: Vec<String>,
    /// `line:col` that must appear in the output, as `:line:col`.
    at: Option<String>,
    /// A known gap: invalid source that is still accepted.
    gap: Option<String>,
    /// A known defect in the message.
    poor: Option<String>,
    /// Text a `[warn ]` line must contain.
    warning: Option<String>,
    /// The diagnostic code the output must carry, as `[CODE]`.
    code: Option<String>,
}

fn read_expectation(path: &Path) -> Expectation {
    let src = std::fs::read_to_string(path).expect("readable corpus file");
    let mut e = Expectation::default();
    for line in src.lines() {
        let Some(rest) = line.strip_prefix("// ") else {
            break;
        };
        let Some((key, value)) = rest.split_once(": ") else {
            continue;
        };
        let value = value.trim().to_string();
        match key {
            "expect" => e.stage = value,
            "message" => e.messages.push(value),
            "at" => e.at = Some(value),
            "gap" => e.gap = Some(value),
            "poor" => e.poor = Some(value),
            "warning" => e.warning = Some(value),
            "code" => e.code = Some(value),
            "fixed" => {}
            other => panic!("{}: unknown header key `{other}`", path.display()),
        }
    }
    assert!(
        !e.stage.is_empty(),
        "{}: missing `// expect:` header",
        path.display()
    );
    e
}

/// The verdict `check` printed: the first per-module line that is not `[ok   ]`, by its stage
/// label, or `refused` when that line carries the refusal marker. `ok` when there is none.
fn verdict(stdout: &str) -> String {
    for line in stdout.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((label, tail)) = rest.split_once(']') else {
            continue;
        };
        let label = label.trim();
        // A warning is not a verdict: the module it is about still passes or fails on its own.
        if label == "ok" || label == "none" || label == "warn" {
            continue;
        }
        return if tail.contains("refused: ") || tail.contains("refused[R") {
            "refused".to_string()
        } else {
            label.to_string()
        };
    }
    "ok".to_string()
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/errors")
}

#[test]
fn every_error_corpus_file_is_reported_as_its_header_says() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .expect("tests/fixtures/errors exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "va"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 30,
        "the corpus went missing: {} files",
        files.len()
    );

    let mut failures = Vec::new();
    let (mut gaps, mut poor) = (Vec::new(), Vec::new());
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let want = read_expectation(path);
        let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
            .arg("check")
            .arg(path)
            .arg("--codegen")
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .output()
            .expect("va-cli runs");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let got = verdict(&stdout);
        let mut problems = Vec::new();
        if got != want.stage {
            problems.push(format!("rejected at `{got}`, expected `{}`", want.stage));
        }
        let want_success = want.stage == "ok";
        if out.status.success() != want_success {
            problems.push(format!(
                "exit status {:?}, expected {}",
                out.status.code(),
                if want_success { "0" } else { "1" }
            ));
        }
        if let Some(code) = &want.code {
            if !stdout.contains(&format!("[{code}]")) {
                problems.push(format!("does not carry the code `{code}`"));
            }
        }
        for m in &want.messages {
            if !stdout.contains(m.as_str()) {
                problems.push(format!("message lacks `{m}`"));
            }
        }
        if let Some(at) = &want.at {
            if !stdout.contains(&format!(":{at}")) {
                problems.push(format!("not located at {at}"));
            }
        }
        if want_success && want.gap.is_none() && want.warning.is_none() {
            problems.push(
                "`expect: ok` needs a `gap:` line saying what is wrong, or a `warning:` line"
                    .to_string(),
            );
        }
        if let Some(w) = &want.warning {
            let warned = stdout
                .lines()
                .any(|l| l.trim_start().starts_with("[warn ]") && l.contains(w.as_str()));
            if !warned {
                problems.push(format!("no `[warn ]` line contains `{w}`"));
            }
        }
        if !problems.is_empty() {
            failures.push(format!(
                "{name}:\n    {}\n  output:\n{stdout}",
                problems.join("\n    ")
            ));
        }
        if let Some(g) = want.gap {
            gaps.push(format!("{name}: {g}"));
        }
        if let Some(p) = want.poor {
            poor.push(format!("{name}: {p}"));
        }
    }

    println!(
        "{} corpus files; {} known gap(s), {} message(s) worth improving",
        files.len(),
        gaps.len(),
        poor.len()
    );
    for g in &gaps {
        println!("  gap   {g}");
    }
    for p in &poor {
        println!("  poor  {p}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} corpus files are not reported as their header says (a `gap` file that is now \
         rejected has been fixed: change its header to the new verdict and rename it):\n\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
