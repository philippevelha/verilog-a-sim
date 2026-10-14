//! Coded diagnostics as a user meets them: the binary's rendering of a coded error, of an
//! uncoded one (`E0000`), and `--explain`. The catalogue's own consistency is checked in
//! `va-diag`; this checks that `va-cli` uses it.

use std::process::{Command, Output};

fn va_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .args(args)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        // English, from the catalogue built into the binary, whatever the machine's locale.
        .env("VA_LANG", "en")
        .env_remove("VA_MESSAGES_DIR")
        .output()
        .expect("va-cli runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A coded error opens with its kind and code, fills its placeholders, points at `--explain`,
/// and fails the run.
#[test]
fn a_coded_error_names_its_code_and_where_to_read_more() {
    let out = va_cli(&["frobnicate"]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(
        err.contains("error[E0701]: unknown subcommand `frobnicate`"),
        "{err}"
    );
    assert!(err.contains("va-cli --explain E0701"), "{err}");
    assert!(!err.contains('{'), "an unfilled placeholder: {err}");

    let err = stderr(&va_cli(&["check", "--codegen"]));
    assert!(err.contains("error[E0702]"), "{err}");
}

/// A frontend error reaches the top level under its own code, located, with what `va-cli` was
/// doing as context rather than as the headline.
#[test]
fn a_frontend_error_is_shown_under_its_own_code() {
    let model = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/errors/elab_unknown_net.va"
    );
    let deck = concat!(env!("CARGO_MANIFEST_DIR"), "/../../circuits/divider.net");
    let out = va_cli(&["sim", deck, "--model", model]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(err.contains("error[E0366] at "), "{err}");
    assert!(
        err.contains("elab_unknown_net.va:8:10: unknown net `q`"),
        "{err}"
    );
    assert!(err.contains("while: elaborating module `r`"), "{err}");
    assert!(!err.contains("E0000"), "{err}");
}

/// A netlist error carries its `E05xx` code, located at `deck:line`, and a deck card that is
/// recognised but not applied is a coded warning.
#[test]
fn a_netlist_error_and_warning_carry_their_codes() {
    let dir = std::env::temp_dir().join(format!("va-cli-netlist-codes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let bad = dir.join("bad.net");
    std::fs::write(&bad, "* deck\nR1 a 0 1k\nK1 L1 L2 1.5\n.op\n").expect("write");
    let err = stderr(&va_cli(&["sim", bad.to_str().unwrap()]));
    assert!(err.contains("error[E0512] at "), "{err}");
    assert!(
        err.contains("bad.net:3: coupling `1.5` is outside [-1, 1]"),
        "{err}"
    );

    let ok = dir.join("ok.net");
    std::fs::write(
        &ok,
        "* deck\nV1 a 0 1\nR1 a 0 1k\n.options reltol=1e-4\n.op\n",
    )
    .expect("write");
    let out = va_cli(&["sim", ok.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("warning[W0503] at "),
        "{}",
        stderr(&out)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A numerical failure carries its `E06xx` code, and a non-finite value names the unknown whose
/// equation went bad rather than a row number.
#[test]
fn a_numerical_failure_names_its_code_and_unknown() {
    let dir = std::env::temp_dir().join(format!("va-cli-numerical-codes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let model = dir.join("bad.va");
    std::fs::write(
        &model,
        "module bad(p, n); inout p, n; electrical p, n;\n\
         analog I(p, n) <+ 1e-3 * ln(V(p, n));\nendmodule\n",
    )
    .expect("write");
    let deck = dir.join("nf.net");
    std::fs::write(&deck, "* ln(0)\nV1 a 0 1\nX1 a 0 bad\n.op\n").expect("write");
    let out = va_cli(&[
        "sim",
        deck.to_str().unwrap(),
        "--model",
        model.to_str().unwrap(),
    ]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(err.contains("error[E0603]: "), "{err}");
    assert!(err.contains("in the equation of `V(a)`"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

/// An error with no code yet is `E0000`, and keeps its own wording and causes in full: the code
/// adds nothing, so it must not take anything away either.
#[test]
fn an_uncoded_error_is_e0000_with_its_own_text() {
    // An unreadable model file: a files-and-command-line error, not yet coded (step 3d).
    let deck = concat!(env!("CARGO_MANIFEST_DIR"), "/../../circuits/divider.net");
    let out = va_cli(&["sim", deck, "--model", "no/such/model.va"]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(err.contains("error[E0000]: "), "{err}");
    assert!(
        err.contains("no/such/model.va"),
        "the original text is kept: {err}"
    );
}

/// `--explain` prints the description and the actions for any defined code, in any case, and
/// rejects a code that does not exist with a coded error of its own.
#[test]
fn explain_prints_meaning_and_actions() {
    let out = va_cli(&["--explain", "e0703"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.starts_with("E0703 (error)"), "{text}");
    assert!(
        text.contains("What it means:") && text.contains("What to do:"),
        "{text}"
    );
    assert!(text.contains("  1. "), "actions are numbered: {text}");

    let out = va_cli(&["--explain", "E9999"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("error[E0703]"), "{}", stderr(&out));

    let out = va_cli(&["--explain"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("error[E0703]"), "{}", stderr(&out));
}

/// Every code the program defines can be explained by the shipped binary, without the
/// `messages/` directory: the catalogue is built in.
#[test]
fn every_defined_code_explains_from_the_built_in_catalogue() {
    for code in va_diag::codes::ALL {
        let out = va_cli(&["--explain", code.as_str()]);
        assert!(out.status.success(), "{code}: {}", stderr(&out));
    }
}
