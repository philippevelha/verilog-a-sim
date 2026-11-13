//! `I(<port>)` and the branch self-probe `I(br)` report the total current (2.56.0): conduction
//! plus displacement, in transient and AC, and exactly the conduction current at an operating
//! point. Checked through the binary against closed forms. Until 2.56.0 both probes dropped every
//! `ddt` term, so a capacitor's probed current was zero outside an operating point.

use std::process::Command;

fn run(tag: &str, files: &[(&str, &str)], args: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("va-cli-probe-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (name, text) in files {
        std::fs::write(dir.join(name), text).expect("write file");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .current_dir(&dir)
        .args(args)
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "va-cli failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// `R || C` read through the port probe.
const PORT: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real cj = 1e-6;
  analog begin
    I(a, c) <+ V(a, c) / r;
    I(a, c) <+ ddt(cj * V(a, c));
    V(out, c) <+ I(<a>);
  end
endmodule
";

/// The same device read through a self-probe of its named branch.
const SELF: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real cj = 1e-6;
  branch (a, c) br;
  analog begin
    I(br) <+ V(br) / r;
    I(br) <+ ddt(cj * V(br));
    V(out, c) <+ I(br);
  end
endmodule
";

/// The conduction path guarded by a condition on `ddt` (refused as E0374 until 2.56.0). On a
/// rising ramp the condition holds, so the probe is the same `V/r + cj dV/dt`.
const GUARDED: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real cj = 1e-6;
  real q;
  analog begin
    q = ddt(cj * V(a, c));
    if (q >= 0.0) I(a, c) <+ V(a, c) / r;
    I(a, c) <+ q;
    V(out, c) <+ I(<a>);
  end
endmodule
";

/// The charge carried to the branch through a variable, then probed: the probe reads `q` as a
/// value, which needs its assignment emitted (2.56.0; E0413 before).
const VIA_VAR: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real cj = 1e-6;
  real q, ib;
  analog begin
    q = ddt(cj * V(a, c));
    I(a, c) <+ V(a, c) / r + q;
    ib = I(<a>);
    V(out, c) <+ ib;
  end
endmodule
";

/// A `ddt` held in a variable and read as an ordinary value, no probe involved: `V(out)` is
/// `2 cj dV/dt` = 2.0 on the ramp, plus `V/r` so the node is driven. E0413 until 2.56.0.
const READ_AS_VALUE: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real cj = 1e-6;
  real q, w;
  analog begin
    q = ddt(cj * V(a, c));
    w = 2.0 * q;
    I(a, c) <+ V(a, c) / r;
    V(out, c) <+ w;
  end
endmodule
";

/// A bias-dependent capacitance, `(c0 + c1 V) dV/dt` (the product rule).
const NLCAP: &str = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real r = 1000; parameter real c0 = 1e-6; parameter real c1 = 1e-6;
  analog begin
    I(a, c) <+ V(a, c) / r;
    I(a, c) <+ (c0 + c1 * V(a, c)) * ddt(V(a, c));
    V(out, c) <+ I(<a>);
  end
endmodule
";

/// `(time, V(a), V(out))` per printed transient point.
fn tran_points(stdout: &str) -> Vec<(f64, f64, f64)> {
    let val = |l: &str, key: &str| -> Option<f64> {
        l.split(key).nth(1)?.split_whitespace().next()?.parse().ok()
    };
    stdout
        .lines()
        .filter_map(|l| {
            let t: f64 = l
                .trim_start()
                .strip_prefix("t=")?
                .split('s')
                .next()?
                .parse()
                .ok()?;
            Some((t, val(l, "V(a)=")?, val(l, "V(out)=")?))
        })
        .collect()
}

/// A 0 -> 1 V ramp in 1 us: dV/dt = 1e6 V/s.
const RAMP: &str = "* ramp\nV1 a gnd PWL(0 0 1u 1)\nM1 a gnd out rc\n.tran 10n 1u\n.end\n";

fn ramp(tag: &str, model: &str) -> Vec<(f64, f64, f64)> {
    let out = run(
        tag,
        &[("m.va", model), ("d.net", RAMP)],
        &[
            "sim", "d.net", "--model", "m.va", "--tran", "--report", "a,out",
        ],
    );
    let pts = tran_points(&out);
    assert!(pts.len() > 20, "too few transient points:\n{out}");
    pts
}

#[test]
fn both_probes_carry_the_displacement_current_in_transient() {
    for (tag, model) in [
        ("port", PORT),
        ("self", SELF),
        ("guarded", GUARDED),
        ("via_var", VIA_VAR),
    ] {
        for (t, v, out) in ramp(tag, model) {
            if t == 0.0 {
                continue;
            }
            // V/r + cj*dV/dt = V/1000 + 1.0. Conduction alone (the pre-2.56.0 answer) is
            // V/1000, about 1000x smaller, so this cannot pass by accident.
            let want = v / 1000.0 + 1.0;
            assert!(
                (out - want).abs() < 2e-6,
                "{tag}: t={t:e} V(out)={out} want {want}"
            );
        }
    }
}

#[test]
fn a_bias_dependent_charge_is_probed_with_the_product_rule() {
    for (t, v, out) in ramp("nlcap", NLCAP) {
        if t == 0.0 {
            continue;
        }
        // V/r + (c0 + c1 V) dV/dt = V/1000 + 1 + V. Folding c1 into the charge (Q = (c0+c1 V) V)
        // would give 1 + 2V instead.
        let want = v / 1000.0 + 1.0 + v;
        assert!(
            (out - want).abs() < 2e-6,
            "t={t:e} V(out)={out} want {want}"
        );
    }
}

#[test]
fn both_probes_are_the_admittance_in_ac() {
    let deck = "* ac\nV1 a gnd DC 0 AC 1\nM1 a gnd out rc\n.ac dec 5 10 10meg\n.end\n";
    for (tag, model) in [("port_ac", PORT), ("self_ac", SELF), ("var_ac", VIA_VAR)] {
        let out = run(
            tag,
            &[("m.va", model), ("d.net", deck)],
            &["sim", "d.net", "--model", "m.va", "--ac", "--report", "out"],
        );
        let mut n = 0;
        for l in out.lines().filter(|l| l.trim_start().starts_with("f=")) {
            let f: f64 = l
                .split("f=")
                .nth(1)
                .unwrap()
                .split("Hz")
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let rest = l.split("V(out)=").nth(1).unwrap();
            let mag: f64 = rest.split(" V").next().unwrap().parse().unwrap();
            let ph: f64 = rest
                .split('∠')
                .nth(1)
                .unwrap()
                .split('°')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            // Y(jw) = 1/r + jw cj: flat at 1 mS below 159 Hz, 62.8 S at 10 MHz.
            let (g, b) = (1e-3, 2.0 * std::f64::consts::PI * f * 1e-6);
            let want_mag = (g * g + b * b).sqrt();
            let want_ph = libm::atan2(b, g).to_degrees();
            assert!(
                (mag / want_mag - 1.0).abs() < 1e-5,
                "{tag}: f={f:e} |Y|={mag} want {want_mag}"
            );
            assert!(
                (ph - want_ph).abs() < 0.01,
                "{tag}: f={f:e} phase {ph} want {want_ph}"
            );
            n += 1;
        }
        assert_eq!(n, 31, "{tag}: expected 31 AC points:\n{out}");
    }
}

#[test]
fn at_an_operating_point_both_probes_are_the_conduction_current() {
    let deck = "* op\nV1 a gnd DC 0.5\nM1 a gnd out rc\n.op\n.end\n";
    for (tag, model) in [("port_op", PORT), ("self_op", SELF)] {
        let out = run(
            tag,
            &[("m.va", model), ("d.net", deck)],
            &["sim", "d.net", "--model", "m.va", "--report", "out"],
        );
        let v: f64 = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("V(out) = "))
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or_else(|| panic!("{tag}: no V(out) in:\n{out}"));
        assert!((v - 0.5e-3).abs() < 1e-12, "{tag}: V(out)={v}");
    }
}

#[test]
fn a_ddt_held_in_a_variable_can_be_read_as_a_value() {
    for (t, _, out) in ramp("read_as_value", READ_AS_VALUE) {
        if t == 0.0 {
            continue;
        }
        assert!((out - 2.0).abs() < 2e-6, "t={t:e} V(out)={out} want 2.0");
    }
}

#[test]
fn a_ddt_read_as_a_value_under_a_solution_dependent_guard_is_still_refused() {
    // The history of a `ddt` evaluated only when `V(a,c) > 0.5` would not advance on the other
    // steps, so its rate would be stale (the reason R0407 refuses the `<+` spelling). Until
    // 2.56.0 this ran and `V(out)` read 0.0 throughout; it is refused now (R0412).
    let model = "`include \"disciplines.vams\"
module rc(a, c, out); inout a, c, out; electrical a, c, out;
  parameter real cj = 1e-6;
  real q, w;
  analog begin
    if (V(a, c) > 0.5) q = ddt(cj * V(a, c));
    w = 2.0 * q;
    I(a, c) <+ V(a, c) / 1000.0;
    V(out, c) <+ w;
  end
endmodule
";
    let dir = std::env::temp_dir().join(format!("va-cli-probe-{}-guard", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.va"), model).expect("write");
    std::fs::write(dir.join("d.net"), RAMP).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .current_dir(&dir)
        .args(["sim", "d.net", "--model", "m.va", "--tran"])
        .output()
        .expect("va-cli runs");
    std::fs::remove_dir_all(&dir).ok();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected a refusal, got a run:
{text}"
    );
    assert!(
        text.contains("refused[R0412]") && text.contains("`q`"),
        "expected R0412 naming `q`:
{text}"
    );
}
