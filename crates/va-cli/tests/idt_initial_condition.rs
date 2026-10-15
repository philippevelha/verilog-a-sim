//! `idt(x, ic)` (LRM 4.5.4, 2.34.0): with an initial condition, the output in a static analysis
//! is `ic` and a transient run integrates from `ic`. Before 2.34.0 `ic` was parsed and ignored:
//! an integrator with no feedback around it made the operating point singular (`E0602`), and a
//! `UIC` run integrated from 0. Every model here prints `W0401`, which says so.

use std::process::Command;

/// An open integrator: `V(out) = ic + 1e6 · ∫V(inp)`, so 1 V in gives 1 V/µs.
const OPEN: &str = "module integ(inp, out); input inp; output out; electrical inp, out;
  analog V(out) <+ idt(1e6 * V(inp), 0.5);
endmodule
";

/// Run `va-cli sim` on `model` and `deck` with `args`; stdout and stderr, joined.
fn sim(tag: &str, model: &str, deck: &str, args: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("va-cli-idt-ic-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.va"), model).expect("write model");
    std::fs::write(dir.join("d.net"), deck).expect("write deck");
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .current_dir(&dir)
        .args(["sim", "d.net", "--model", "m.va"])
        .args(args)
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The value of `quantity` (`V(out)`) on each printed line that has it.
fn values(out: &str, quantity: &str) -> Vec<f64> {
    let key = format!("{quantity}=");
    out.lines()
        .filter_map(|l| {
            let rest = &l[l.find(&key)? + key.len()..];
            rest.split_whitespace().next()?.parse().ok()
        })
        .collect()
}

#[test]
fn an_open_integrator_with_ic_has_an_operating_point_at_ic() {
    let out = sim(
        "op",
        OPEN,
        "* op\nV1 in 0 DC 1\nX1 in out integ\nR1 out 0 1k\n.op\n",
        &[],
    );
    assert!(out.contains("warning[W0401]"), "{out}");
    assert!(!out.contains("E0602"), "no longer singular: {out}");
    // `.op` prints `V(out) = 0.500000 V`.
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("V(out) ="))
        .unwrap_or_else(|| panic!("no V(out): {out}"));
    let v: f64 = line
        .split('=')
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((v - 0.5).abs() < 1e-9, "V(out) = {v}, want ic = 0.5");
}

#[test]
fn a_transient_integrates_from_ic_with_and_without_uic() {
    for (tag, uic) in [("tran", ""), ("uic", " UIC")] {
        let out = sim(
            tag,
            OPEN,
            &format!("* tran\nV1 in 0 DC 1\nX1 in out integ\nR1 out 0 1k\n.tran 0.1u 1u{uic}\n"),
            &["--tran"],
        );
        let v = values(&out, "V(out)");
        assert!(v.len() > 2, "{tag}: {out}");
        if uic.is_empty() {
            // The run starts from the operating point, which is `ic`.
            assert!((v[0] - 0.5).abs() < 1e-9, "{tag}: t = 0 gives {}", v[0]);
        }
        // 1 V for 1 µs at 1e6/s adds 1 V to `ic`, from either start.
        let last = *v.last().unwrap();
        assert!(
            (last - 1.5).abs() < 1e-6,
            "{tag}: t = 1 µs gives {last}, want 1.5"
        );
    }
}

/// The small-signal answer does not depend on `ic`: a first-order loop `y = ∫w(x − y)` is
/// `w / (jω + w)`, 1/√2 at `ω = w`.
#[test]
fn ic_does_not_change_the_ac_response() {
    let model = "module lp(inp, out); input inp; output out; electrical inp, out;
  parameter real w = 1e6;
  analog V(out) <+ idt(w * (V(inp) - V(out)), 0.25);
endmodule
";
    let out = sim(
        "ac",
        model,
        "* ac\nV1 in 0 DC 1 AC 1\nX1 in out lp\nR1 out 0 1k\n.ac dec 1 15.915494k 159.15494k\n",
        &["--ac"],
    );
    let v = values(&out, "V(out)");
    let last = *v.last().unwrap_or_else(|| panic!("{out}"));
    assert!(
        (last - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6,
        "|H(jw)| = {last}: {out}"
    );
}

/// Without `ic` nothing changed: an integrator inside a feedback loop settles at the operating
/// point through the loop, and there is no warning.
#[test]
fn an_integrator_without_ic_is_unchanged() {
    let model = "module lp(inp, out); input inp; output out; electrical inp, out;
  analog V(out) <+ idt(1e6 * (V(inp) - V(out)));
endmodule
";
    let out = sim(
        "noic",
        model,
        "* noic\nV1 in 0 DC 0.7\nX1 in out lp\nR1 out 0 1k\n.op\n",
        &[],
    );
    assert!(!out.contains("W0401"), "{out}");
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("V(out) ="))
        .unwrap_or_else(|| panic!("no V(out): {out}"));
    let v: f64 = line
        .split('=')
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (v - 0.7).abs() < 1e-9,
        "the loop drives V(out) to V(in): {v}"
    );
}

/// A bias-dependent `ic` (PSP103's NQS splines write `idt(-Tnorm * fk1, Qp1_0)`) sets only the
/// start: the output is `ic` at the operating point and then follows the integral alone, not
/// later changes of `ic`. Here `ic = V(ctl)` steps from 0.2 V to 0.9 V after the start and the
/// integrand is zero, so the output must stay at 0.2 V.
#[test]
fn a_varying_ic_sets_only_the_start() {
    let model = "module hold(ctl, out); input ctl; output out; electrical ctl, out;
  analog V(out) <+ idt(0.0, V(ctl));
endmodule
";
    let out = sim(
        "vary",
        model,
        "* vary\nV1 ctl 0 PULSE(0.2 0.9 0.2u 1n 1n 10u 20u)\nX1 ctl out hold\nR1 out 0 1k\n.tran 0.1u 1u\n",
        &["--tran"],
    );
    let v = values(&out, "V(out)");
    assert!(v.len() > 2, "{out}");
    for (k, x) in v.iter().enumerate() {
        assert!(
            (x - 0.2).abs() < 1e-9,
            "point {k}: V(out) = {x}, want 0.2: {out}"
        );
    }
}
