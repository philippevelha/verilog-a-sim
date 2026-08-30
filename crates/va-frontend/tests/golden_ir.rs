//! T1.3's validation gate: **the three zoo models elaborate to IR that matches committed
//! golden IR** (`docs/roadmap.md`, Phase T1.3).
//!
//! Until now T1.3's tests asserted IR *structure* — a node count here, a parameter name there.
//! Those catch what they were written to look at and nothing else, so a change that quietly
//! reshaped some other corner of Interface α passed them. A committed snapshot of the whole
//! [`va_ir::Module`] catches any change to any field, including fields nobody thought to
//! assert on and fields that do not exist yet.
//!
//! # Why the snapshot is `{:#?}` and not a hand-written pretty-printer
//!
//! A bespoke printer is easier to read, and it is exactly the wrong tool here: it can only
//! print the fields its author remembered, so a field added to `va_ir::Module` tomorrow would
//! be invisible to it and the gate would silently stop covering it. `Debug` is **exhaustive by
//! construction** — every field of every nested type, or it does not compile. Total coverage
//! beats readability for a gate whose whole job is to notice the change nobody predicted.
//!
//! # These are generated, never hand-written
//!
//! Regenerate with `UPDATE_GOLDEN_IR=1 cargo test -p va-frontend --test golden_ir`, then read
//! the diff. A snapshot is a record of what the code *did*, so a diff is a question ("did I
//! mean to change Interface α?"), never a licence to edit the file until it matches. Note that
//! this is a different kind of artifact from `golden/`, which holds **QSPICE** reference
//! outputs for numerical results — QSPICE cannot produce IR, and nothing here is a physical
//! quantity, which is why these live beside the test instead.

use std::path::{Path, PathBuf};

/// The zoo models T1.3's gate names, and the module each is expected to define.
const ZOO: [(&str, &str); 3] = [
    ("resistor.va", "resistor"),
    ("capacitor.va", "capacitor"),
    ("diode.va", "diode"),
];

fn models_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models"))
}

fn snapshot_path(model: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden_ir")
        .join(format!("{}.ir", model.trim_end_matches(".va")))
}

/// Elaborate one zoo model exactly as the real pipeline does — through the include path, so
/// `` `include "constants.vams" `` resolves for real rather than being silently dropped.
fn elaborate(model: &str, expect_module: &str) -> String {
    let path = models_dir().join(model);
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let design = va_frontend::compile_with_includes(&src, &[models_dir()])
        .unwrap_or_else(|e| panic!("{model} should elaborate: {e}"));
    assert_eq!(
        design.modules.len(),
        1,
        "{model} should define exactly one module"
    );
    assert_eq!(design.modules[0].name, expect_module);
    format!("{:#?}\n", design.modules[0])
}

#[test]
fn zoo_models_match_committed_golden_ir() {
    let update = std::env::var_os("UPDATE_GOLDEN_IR").is_some();
    let mut stale = Vec::new();

    for (model, expect_module) in ZOO {
        let actual = elaborate(model, expect_module);
        let path = snapshot_path(model);

        if update {
            std::fs::write(&path, &actual)
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            continue;
        }

        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing golden IR for {model} ({}): {e}\n\
                 regenerate with `UPDATE_GOLDEN_IR=1 cargo test -p va-frontend --test golden_ir`",
                path.display()
            )
        });
        // Compare line-normalized: a CRLF checkout must not fail a gate about IR shape.
        if expected.replace("\r\n", "\n") != actual.replace("\r\n", "\n") {
            stale.push((model, path, expected, actual));
        }
    }

    if update {
        // Fail loudly rather than silently "passing" a run that asserted nothing.
        panic!(
            "golden IR regenerated; re-run without UPDATE_GOLDEN_IR to verify, and read the diff"
        );
    }

    if let Some((model, path, expected, actual)) = stale.first() {
        let (line, exp_line, act_line) = first_difference(expected, actual);
        panic!(
            "elaborated IR for {model} no longer matches {}\n\
             first difference at line {line}:\n  golden: {exp_line}\n  actual: {act_line}\n\
             {} of {} zoo models differ.\n\
             If this change to Interface \u{3b1} is intended, regenerate with \
             `UPDATE_GOLDEN_IR=1 cargo test -p va-frontend --test golden_ir` and review the diff.",
            path.display(),
            stale.len(),
            ZOO.len(),
        );
    }
}

/// The 1-based line number of the first difference, with both sides' text — a 300-line `{:#?}`
/// dump is unreadable as a raw assert_eq, and the point of failing is to say *what* moved.
fn first_difference(expected: &str, actual: &str) -> (usize, String, String) {
    let (e, a) = (expected.replace("\r\n", "\n"), actual.replace("\r\n", "\n"));
    let (mut el, mut al) = (e.lines(), a.lines());
    let mut n = 0;
    loop {
        n += 1;
        match (el.next(), al.next()) {
            (None, None) => return (n, "<end>".into(), "<end>".into()),
            (e, a) if e != a => {
                return (
                    n,
                    e.unwrap_or("<end of file>").to_string(),
                    a.unwrap_or("<end of file>").to_string(),
                )
            }
            _ => {}
        }
    }
}

/// Negative control for the gate above: the comparison must actually be able to *fail*. A
/// snapshot test that silently passes on any input is worse than no test, and this is the
/// cheapest way to know the plumbing discriminates.
#[test]
fn the_golden_ir_comparison_detects_a_difference() {
    let resistor = elaborate("resistor.va", "resistor");
    let capacitor = elaborate("capacitor.va", "capacitor");
    assert_ne!(
        resistor, capacitor,
        "two different models must not produce identical IR dumps"
    );
    let (line, _, _) = first_difference(&resistor, &capacitor);
    assert!(
        line > 1,
        "the dumps should agree on at least the first line"
    );
}
