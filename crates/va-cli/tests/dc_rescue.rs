//! The DC rescue as a user meets it (2.58.0): every change of method is a warning, and a
//! `.dc` sweep that stops part-way prints the points it had before the error.

use std::process::{Command, Output};

fn va_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .args(args)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .env("VA_LANG", "en")
        .env_remove("VA_MESSAGES_DIR")
        .output()
        .expect("va-cli runs")
}

/// A two-terminal element `I = 1 mA · sqrt(V + 0.6)`: defined down to V = −0.6, so a sweep from
/// 1 V to −1 V in 0.25 V steps solves seven points and then reaches −0.75 V, where every
/// evaluation is NaN and no method can succeed.
const SQRT_MODEL: &str = r#"`include "disciplines.vams"
module sqrtg(p, n);
  inout p, n; electrical p, n;
  analog I(p, n) <+ 1m * sqrt(V(p, n) + 0.6);
endmodule
"#;

const DECK: &str = "* sweep that runs out of the model's domain
V1 a gnd DC 1
X1 a gnd sqrtg
.dc V1 1 -1 -0.25
.end
";

#[test]
fn a_failed_sweep_prints_its_points_and_warns_at_every_change_of_method() {
    let dir = std::env::temp_dir().join(format!("va_dc_rescue_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let disciplines = concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/disciplines.vams");
    std::fs::copy(disciplines, dir.join("disciplines.vams")).unwrap();
    let model = dir.join("sqrtg.va");
    let deck = dir.join("sq.net");
    std::fs::write(&model, SQRT_MODEL).unwrap();
    std::fs::write(&deck, DECK).unwrap();

    let out = va_cli(&[
        "sim",
        deck.to_str().unwrap(),
        "--model",
        model.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    std::fs::remove_dir_all(&dir).ok();

    assert!(!out.status.success(), "the sweep must still fail");
    // The seven points before the failure are printed, the failing one is not.
    let points: Vec<&str> = stdout.lines().filter(|l| l.contains("V1=")).collect();
    assert_eq!(points.len(), 7, "{stdout}");
    assert!(points[6].contains("V1=-0.500000"), "{stdout}");
    assert!(stderr.contains("stopped after 7 point(s)"), "{stderr}");
    assert!(
        stderr.contains("while: `.dc` sweep at V1=-0.75"),
        "{stderr}"
    );

    // Every method is announced as it fails, in order, started from the previous point and
    // then from zero, and the last one says nothing is left.
    let changes: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("warning[W0603]"))
        .collect();
    let order = [
        "trying gmin stepping with node steps capped at 0.5",
        "trying gmin stepping",
        "trying gmin stepping with a line search",
        "trying pseudo-transient continuation",
        "trying nothing: every method has failed",
        "trying every method again, started from zero",
    ];
    let mut at = 0;
    for want in order {
        let found = changes[at..]
            .iter()
            .position(|l| l.ends_with(want))
            .unwrap_or_else(|| panic!("no W0603 `{want}` after line {at}:\n{stderr}"));
        at += found + 1;
    }
    // 5 changes from the previous point, the restart, 5 from zero.
    assert_eq!(changes.len(), 11, "{stderr}");
    assert!(changes.iter().all(|l| l.contains("`.dc` at V1=-0.75")));
    assert!(!stderr.contains('{'), "an unfilled placeholder: {stderr}");
}
