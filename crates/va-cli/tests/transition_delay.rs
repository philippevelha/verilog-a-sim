//! `transition(x, delay, ...)` is a transport delay (2.35.0): every change of `x` comes out
//! `delay` later, however soon the next one follows. Until 2.35.0 a site held one pending
//! change and a second overwrote it, so a pulse shorter than the delay vanished — the defect
//! OpenVAF-Reloaded PRs #44/#46 fixed in their own `transition`. A delayed site warns (`W0402`)
//! that a pulse passes through delayed rather than filtered.

use std::process::Command;

const MODEL: &str = "module tdx(inp, out); input inp; output out; electrical inp, out;
  parameter real td = 2n;
  real level;
  analog begin
    level = V(inp) > 0.5 ? 1.0 : 0.0;
    V(out) <+ transition(level, td, 0.1n, 0.1n);
  end
endmodule
";

/// `(t, V(out))` for `pulse` (a `PULSE(...)` source spec) through `model`, with the run's output.
fn run(tag: &str, model: &str, pulse: &str, tstop: &str) -> (Vec<(f64, f64)>, String) {
    let dir = std::env::temp_dir().join(format!("va-cli-tdelay-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.va"), model).expect("write model");
    std::fs::write(
        dir.join("d.net"),
        format!("* delay\nV1 in 0 {pulse}\nX1 in out tdx\nR1 out 0 1k\n.tran 0.05n {tstop}\n"),
    )
    .expect("write deck");
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .current_dir(&dir)
        .args([
            "sim", "d.net", "--model", "m.va", "--tran", "--report", "out",
        ])
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let points = text
        .lines()
        .filter_map(|l| {
            let t = l
                .trim_start()
                .strip_prefix("t=")?
                .split('s')
                .next()?
                .parse()
                .ok()?;
            let v = l
                .split("V(out)=")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            Some((t, v))
        })
        .collect();
    (points, text)
}

/// The times `V(out)` crosses 0.5 V upward and downward.
fn edges(points: &[(f64, f64)]) -> (Vec<f64>, Vec<f64>) {
    let (mut up, mut down) = (Vec::new(), Vec::new());
    for w in points.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        let at = || t0 + (0.5 - v0) * (t1 - t0) / (v1 - v0);
        if v0 < 0.5 && v1 >= 0.5 {
            up.push(at());
        } else if v0 >= 0.5 && v1 < 0.5 {
            down.push(at());
        }
    }
    (up, down)
}

#[test]
fn a_pulse_shorter_than_the_delay_comes_out_delayed() {
    // A 0.5 ns pulse at 10 ns through a 2 ns delay: out at 12.0-12.5 ns (plus half a ramp).
    let (points, text) = run("short", MODEL, "PULSE(0 1 10n 1p 1p 0.5n 100n)", "20n");
    assert!(text.contains("warning[W0402]"), "{text}");
    let (up, down) = edges(&points);
    assert_eq!(
        (up.len(), down.len()),
        (1, 1),
        "one pulse: {up:?} {down:?}\n{text}"
    );
    assert!((up[0] - 12.05e-9).abs() < 0.02e-9, "rises at {:e}", up[0]);
    assert!(
        (down[0] - 12.55e-9).abs() < 0.02e-9,
        "falls at {:e}",
        down[0]
    );
}

#[test]
fn every_pulse_of_a_train_inside_one_delay_comes_out() {
    // 0.3 ns pulses every 0.6 ns: several edges wait inside the 2 ns delay at once.
    let (points, text) = run("train", MODEL, "PULSE(0 1 10n 1p 1p 0.3n 0.6n)", "14n");
    let (up, _) = edges(&points);
    // The source keeps pulsing until tstop: four pulses have left the delay by 14 ns, with
    // up to seven edges waiting inside it at any moment.
    assert!(up.len() >= 4, "pulses came out: {up:?}\n{text}");
    for (k, t) in up.iter().enumerate() {
        let want = 12.05e-9 + k as f64 * 0.6e-9;
        assert!(
            (t - want).abs() < 0.02e-9,
            "pulse {k} rises at {t:e}, want {want:e}"
        );
    }
}

#[test]
fn a_pulse_longer_than_the_delay_is_unchanged() {
    let (points, _) = run("long", MODEL, "PULSE(0 1 10n 1p 1p 3n 100n)", "20n");
    let (up, down) = edges(&points);
    assert_eq!((up.len(), down.len()), (1, 1));
    assert!((up[0] - 12.05e-9).abs() < 0.02e-9, "rises at {:e}", up[0]);
    assert!(
        (down[0] - 15.05e-9).abs() < 0.02e-9,
        "falls at {:e}",
        down[0]
    );
}

#[test]
fn no_delay_no_warning() {
    for td in ["0", "0.0"] {
        let model = MODEL.replace("td = 2n", &format!("td = {td}"));
        let (points, text) = run("nodelay", &model, "PULSE(0 1 10n 1p 1p 0.5n 100n)", "20n");
        assert!(!text.contains("W0402"), "td = {td}: {text}");
        let (up, _) = edges(&points);
        assert!((up[0] - 10.05e-9).abs() < 0.02e-9, "rises at {:e}", up[0]);
    }
}
