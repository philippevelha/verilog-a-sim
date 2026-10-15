//! `idt(x, ic, assert)` in transient (LRM 4.5.4, 2.41.0): while `assert` is non-zero the output
//! is held at `ic`, and the integral restarts from `ic` when it returns to zero. 2.36.0-2.40.x
//! refused a reset that could fire (`R0411`); before that, any third argument was an internal
//! error.

use std::process::Command;

/// `(t, value)` of `quantity` for `deck` over `model`.
fn run(tag: &str, model: &str, deck: &str, quantity: &str) -> (Vec<(f64, f64)>, String) {
    let dir = std::env::temp_dir().join(format!("va-cli-idtr-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.va"), model).expect("write model");
    std::fs::write(dir.join("d.net"), deck).expect("write deck");
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .current_dir(&dir)
        .args([
            "sim", "d.net", "--model", "m.va", "--tran", "--report", quantity,
        ])
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let key = format!("V({quantity})=");
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
                .split(&key)
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

/// The value at the last point at or before `t`.
fn at(points: &[(f64, f64)], t: f64) -> f64 {
    points
        .iter()
        .rfind(|(pt, _)| *pt <= t)
        .map(|(_, v)| *v)
        .unwrap_or(f64::NAN)
}

const PULSED: &str =
    "module r(inp, rst, out); input inp, rst; output out; electrical inp, rst, out;
  parameter real ic = 0;
  analog V(out) <+ idt(1e6 * V(inp), ic, V(rst) > 0.5);
endmodule
";

/// A reset pulse from 1.0 to 1.2 us on a 1 V/us integral: the output reaches 1 V, drops to `ic`
/// where the reset rises (its 1 ns edge crosses 0.5 V at 1.0005 us), holds through the pulse,
/// and ramps again from `ic` where it falls (1.2015 us).
#[test]
fn a_pulsed_reset_holds_the_output_at_ic_and_restarts_from_it() {
    for ic in [0.0, 0.5] {
        let (p, text) = run(
            "pulse",
            PULSED,
            &format!(
                "* r\nV1 in 0 DC 1\nV2 rst 0 PULSE(0 1 1u 1n 1n 0.2u 10u)\n\
                 X1 in rst out r ic={ic}\nR1 out 0 1k\n.tran 0.05u 3u\n"
            ),
            "out",
        );
        assert!(p.len() > 20, "{text}");
        // Before the reset: `ic` plus the integral.
        assert!(
            (at(&p, 0.5e-6) - (ic + 0.5)).abs() < 1e-6,
            "ic={ic}: {text}"
        );
        // During it: held at `ic`.
        for t in [1.05e-6, 1.1e-6, 1.2e-6] {
            assert!(
                (at(&p, t) - ic).abs() < 1e-9,
                "ic={ic}, t={t:e}: {}",
                at(&p, t)
            );
        }
        // After: restarted from `ic` at the release, 1.2015 us, located to tstep/1000.
        let end = at(&p, 3e-6);
        let want = ic + (3e-6 - 1.2015e-6) * 1e6;
        assert!((end - want).abs() < 1e-3, "ic={ic}: end {end}, want {want}");
    }
}

/// A reset driven by the output itself, the relaxation oscillator: `idt(x, 0, V(saw) > 1)` on
/// 1 V/us is a sawtooth of period 1 us. The reset releases itself where it applies, so the
/// integral restarts at once rather than one step later, and each wrap is located to tstep/1000.
#[test]
fn a_self_resetting_integrator_is_a_sawtooth_with_the_right_period() {
    let model = "module s(inp, saw); input inp; output saw; electrical inp, saw;
  analog V(saw) <+ idt(1e6 * V(inp), 0, V(saw) > 1);
endmodule
";
    let (p, text) = run(
        "saw",
        model,
        "* s\nV1 in 0 DC 1\nX1 in saw s\nR2 saw 0 1k\n.tran 0.05u 4.5u\n",
        "saw",
    );
    let resets: Vec<f64> = p
        .windows(2)
        .filter(|w| w[1].1 < w[0].1 - 0.5)
        .map(|w| w[1].0)
        .collect();
    assert_eq!(resets.len(), 4, "{resets:?}\n{text}");
    for (k, t) in resets.iter().enumerate() {
        let want = (k + 1) as f64 * 1e-6;
        assert!(
            (t - want).abs() < 1e-9 * (k + 1) as f64,
            "reset {k} at {t:e}, want {want:e}"
        );
    }
    assert!(
        p.iter().all(|&(_, v)| (-1e-9..=1.0 + 1e-3).contains(&v)),
        "{text}"
    );
}
