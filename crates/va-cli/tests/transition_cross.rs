//! A `cross` whose crossing falls just before a `transition` ramp ends: the situation of a PLL
//! divider output settling a hair past a phase detector's threshold (the defect
//! OpenVAF-Reloaded PR #48 fixed in its own simulator).
//!
//! Until 2.32.0 `transition` bounded each step to an eighth of what remained of its ramp, so
//! the steps shrank geometrically toward the ramp's end; a crossing in that tail made the step
//! passing it fail its error test at every size, and the run died of a timestep underflow
//! (`E0605`). The ramp below ends at 0.50001 V, so the 0.5 V crossing is 20 fs before it.

use std::process::Command;

const MODEL: &str = "module ramp(out, det); inout out, det; electrical out, det;
  parameter real level = 1.0;
  real target; integer n;
  analog begin
    @(initial_step) begin target = 0; n = 0; end
    @(timer(1e-6, 1e-6)) target = (target > 0) ? 0 : level;
    V(out) <+ transition(target, 0, 1e-9);
    @(cross(V(out) - 0.5, 0)) n = n + 1;
    V(det) <+ n;
  end
endmodule
";

/// The crossings counted by `V(det)` at the end of the run, for a ramp to `level` with `.tran`
/// step `tstep`.
fn crossings(level: &str, tstep: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "va-cli-transition-cross-{}-{level}-{tstep}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let model = dir.join("ramp.va");
    std::fs::write(
        &model,
        MODEL.replace("level = 1.0", &format!("level = {level}")),
    )
    .expect("write model");
    let deck = dir.join("ramp.net");
    std::fs::write(
        &deck,
        format!("* ramp\nX1 out det ramp\nR1 out 0 1k\nR2 det 0 1k\n.tran {tstep} 10.5u\n"),
    )
    .expect("write deck");
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .args([
            "sim",
            deck.to_str().unwrap(),
            "--model",
            model.to_str().unwrap(),
        ])
        .args(["--tran", "--report", "V(det)"])
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "level {level}, tstep {tstep}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let last = stdout
        .lines()
        .rev()
        .find(|l| l.contains("V(det)="))
        .expect("a result");
    last.split("V(det)=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// Ten edges of the 1 µs timer in 10.5 µs, each ramp crossing 0.5 V once: ten crossings,
/// whether the crossing is mid-ramp, 20 fs before the ramp's end, or exactly at it.
#[test]
fn a_crossing_at_the_end_of_a_transition_ramp_does_not_underflow() {
    for (level, tstep) in [
        ("1.0", "1n"),
        ("0.50001", "1n"),
        ("0.5001", "10n"),
        ("0.5", "1n"),
    ] {
        assert_eq!(
            crossings(level, tstep),
            "10.000000",
            "level {level}, tstep {tstep}"
        );
    }
}
