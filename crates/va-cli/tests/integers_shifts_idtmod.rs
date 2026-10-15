//! 2.36.0: the integer-division warning (`W0302`), `$rtoi`/`$itor`, the arithmetic shifts
//! `<<<`/`>>>`, `idtmod`, and `idt`'s third and fourth arguments — through the binary.

use std::process::Command;

/// Run `va-cli` with `args` in a fresh directory holding `files`; stdout and stderr, joined.
fn run(tag: &str, files: &[(&str, &str)], args: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("va-cli-ish-{}-{tag}", std::process::id()));
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
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The `.op` value printed for `V(node)`.
fn op_value(out: &str, node: &str) -> f64 {
    let key = format!("V({node}) =");
    out.lines()
        .find_map(|l| l.trim_start().strip_prefix(&key))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no {key} in:\n{out}"))
}

/// `.op` of a module driving each of `nodes` with one expression: `V(node) <+ expr`.
fn op(tag: &str, decls: &str, prelude: &str, drives: &[(&str, &str)]) -> String {
    let ports: Vec<&str> = drives.iter().map(|(n, _)| *n).collect();
    let model = format!(
        "module m({p}); inout {p}; electrical {p};\n{decls}\nanalog begin\n{prelude}\n{body}\nend\nendmodule\n",
        p = ports.join(", "),
        body = drives
            .iter()
            .map(|(n, e)| format!("  V({n}) <+ {e};"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let deck = format!(
        "* op\nX1 {} m\n{}.op\n",
        ports.join(" "),
        ports
            .iter()
            .enumerate()
            .map(|(k, n)| format!("R{k} {n} 0 1k\n"))
            .collect::<String>()
    );
    run(
        tag,
        &[("m.va", &model), ("d.net", &deck)],
        &["sim", "d.net", "--model", "m.va"],
    )
}

#[test]
fn dividing_two_integers_warns_and_says_what_each_reading_gives() {
    let src = "module a(p); inout p; electrical p;
  integer i; real x, y; parameter integer n = 7;
  analog begin
    i = 7; x = 7;
    y = i / 2;
    y = n / 2;
    y = 7 / 2;
    y = i / 2.0;
    y = x / 2;
    y = i / 2k;
    y = int(i / 2);
    y = $rtoi(n / 2);
    I(p) <+ V(p) * y;
  end
endmodule
";
    let out = run("w0302", &[("a.va", src)], &["check", "a.va"]);
    let lines: Vec<&str> = out.lines().filter(|l| l.contains("W0302")).collect();
    // Lines 5-7 divide two integers; 8-10 have a real operand (`2.0`, `x`, the scale factor
    // in `2k`); 11-12 are the explicit truncation the warning recommends.
    let at: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.split("at a.va:").nth(1)?.split(':').next())
        .collect();
    assert_eq!(at, ["5", "6", "7"], "{out}");
    assert!(
        lines[0].contains("7 / 2 = 3") && lines[0].contains("3.5"),
        "{}",
        lines[0]
    );
}

#[test]
fn rtoi_truncates_and_itor_converts() {
    let out = op(
        "conv",
        "parameter real p = $rtoi(-7.9);",
        "",
        &[
            ("a", "$rtoi(3.7)"),
            ("b", "$rtoi(-3.7)"),
            ("c", "$itor(5)"),
            ("d", "p"),
        ],
    );
    for (node, want) in [("a", 3.0), ("b", -3.0), ("c", 5.0), ("d", -7.0)] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}

#[test]
fn arithmetic_shifts_fill_with_the_sign() {
    let out = op(
        "shift",
        "integer i; parameter integer p = -8 >>> 1;",
        "  i = -8;",
        &[
            ("a", "i >>> 1"),
            ("b", "5 >>> 1"),
            ("c", "-7 >>> 1"),
            ("d", "3 <<< 2"),
            ("e", "p"),
            ("f", "1 + 16 >>> 2"),
        ],
    );
    // `+` binds tighter than a shift: `1 + 16 >>> 2` is `17 >>> 2`.
    for (node, want) in [
        ("a", -4.0),
        ("b", 2.0),
        ("c", -4.0),
        ("d", 12.0),
        ("e", -4.0),
        ("f", 4.0),
    ] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}

/// `idtmod(1e6 * V(inp), 0, 1, -0.5)` on 1 V: a sawtooth of slope 1/µs in `[-0.5, 0.5)`,
/// wrapping at 0.5, 1.5, 2.5 µs. The wraps are breakpoints, so the run does not underflow.
#[test]
fn idtmod_wraps_its_integral_into_the_window() {
    let model = "module vco(inp, out); input inp; output out; electrical inp, out;
  analog V(out) <+ idtmod(1e6 * V(inp), 0.0, 1.0, -0.5);
endmodule
";
    let out = run(
        "idtmod",
        &[
            ("m.va", model),
            (
                "d.net",
                "* vco\nV1 in 0 DC 1\nX1 in out vco\nR1 out 0 1k\n.tran 0.05u 2.6u\n",
            ),
        ],
        &[
            "sim", "d.net", "--model", "m.va", "--tran", "--report", "out",
        ],
    );
    let points: Vec<(f64, f64)> = out
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
    assert!(points.len() > 20, "{out}");
    for &(t, v) in &points {
        assert!((-0.5..=0.5).contains(&v), "V(out) = {v} at {t:e}");
        // Away from a wrap, the value is the unwrapped ramp folded into the window.
        let phase = t * 1e6 + 0.5;
        let frac = phase - phase.floor();
        if frac > 1e-3 && frac < 1.0 - 1e-3 {
            assert!(
                (v - (frac - 0.5)).abs() < 1e-6,
                "V(out) = {v} at {t:e}, want {}",
                frac - 0.5
            );
        }
    }
    let wraps = points
        .windows(2)
        .filter(|w| (w[1].1 - w[0].1).abs() > 0.5)
        .count();
    assert_eq!(wraps, 3, "{out}");
}

#[test]
fn idtmod_operating_point_is_ic_folded_into_the_window_and_without_a_modulus_it_is_idt() {
    let out = op(
        "idtmod-op",
        "",
        "",
        &[
            // `ic = 1.3` folded into `[0, 1)` is 0.3; into `[-0.5, 0.5)`, 0.3 as well.
            ("a", "idtmod(0, 1.3, 1)"),
            ("b", "idtmod(0, 1.3, 1, -0.5)"),
            // No modulus: `idt(0, 0.7)`.
            ("c", "idtmod(0, 0.7)"),
        ],
    );
    for (node, want) in [("a", 0.3), ("b", 0.3), ("c", 0.7)] {
        assert!(
            (op_value(&out, node) - want).abs() < 1e-12,
            "V({node}): {out}"
        );
    }
}

#[test]
fn idt_takes_a_never_resetting_assert_and_an_abstol_and_refuses_a_real_reset() {
    // Before 2.36.0 any third argument was an internal error (`E0001`).
    let out = op("idt3", "", "", &[("a", "idt(0, 0.5, 0, 1e-9)")]);
    assert!((op_value(&out, "a") - 0.5).abs() < 1e-12, "{out}");
    let out = op("idt-reset", "", "", &[("a", "idt(0, 0.5, V(a) > 1)")]);
    assert!(
        out.contains("R0411"),
        "a reset that can fire is refused: {out}"
    );
    assert!(!out.contains("E0001"), "{out}");
}

/// `>>` is a logical shift of Verilog's 32-bit `integer`, read back as signed (2.37.0; it was a
/// 64-bit shift, so `-8 >> 1` gave 2^63 - 4). Checked at run time and in a parameter default,
/// which take different paths (`ad::shift_right_32`, `elaborate::shift_right_32`).
#[test]
fn logical_right_shift_is_32_bit() {
    let out = op(
        "shr32",
        "integer i, k; parameter integer p = -8 >> 1;",
        "  i = -8; k = -1;",
        &[
            ("a", "i >> 1"),
            ("b", "i >> 0"),
            ("c", "16 >> 2"),
            ("d", "i >> 32"),
            ("e", "16 >> k"),
            ("f", "p"),
            ("g", "-1 >> 31"),
        ],
    );
    for (node, want) in [
        ("a", 2147483644.0),
        ("b", -8.0),
        ("c", 4.0),
        ("d", 0.0),
        ("e", 0.0),
        ("f", 2147483644.0),
        ("g", 1.0),
    ] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}
