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

/// Integer typing (2.39.0, LRM 4.2.1): a division of two integers truncates toward zero, and a
/// value stored into an integer rounds half away from zero. 2.36.0-2.38.x only warned
/// (`W0302`, now retired) and divided in real arithmetic.
#[test]
fn integers_divide_and_store_as_integers() {
    let out = op(
        "ints",
        "integer n, q, arr[0:1]; real x;
  parameter integer p = 7 / 2; parameter integer r = 2.5; parameter real s = 7 / 2;
  analog function integer half; input v; integer v; half = v / 2; endfunction",
        "  n = 7; x = 7; q = 3.7; arr[1] = -2.5;
  begin : blk
    integer t;
    t = n / 2;
    V(k) <+ t;
  end",
        &[
            ("a", "n / 2"),
            ("b", "-n / 2"),
            ("c", "n / 2.0"),
            ("d", "x / 2"),
            ("e", "p"),
            ("f", "r"),
            ("g", "q"),
            ("h", "arr[1]"),
            ("i", "half(9)"),
            ("j", "s"),
            ("l", "7 / 2 * 2.0"),
            ("m", "n / 2k"),
            ("k", "0"),
        ],
    );
    assert!(!out.contains("W0302"), "the warning is retired: {out}");
    for (node, want) in [
        ("a", 3.0),  // integer / integer truncates
        ("b", -3.0), // toward zero, not down
        ("c", 3.5),  // a real operand makes it real
        ("d", 3.5),
        ("e", 3.0),  // in a parameter default too
        ("f", 3.0),  // an integer parameter rounds 2.5 away from zero
        ("g", 4.0),  // a store into an integer rounds
        ("h", -3.0), // into an integer array element too
        ("i", 4.0),  // an integer function's own arithmetic
        ("j", 3.0),  // 7 / 2 is integer division before it reaches the real parameter
        ("l", 6.0),
        ("m", 0.0035), // `2k` is a real literal
    ] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
    // A block-local `integer` is typed too. `V(k)` gets `t` inside the block and `0` after;
    // the contributions add.
    assert_eq!(op_value(&out, "k"), 3.0, "{out}");
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

/// Every bitwise and shift operator works on Verilog's 32-bit `integer` (2.38.0; 64-bit before,
/// so `1 << 31` was 2147483648 and `1 << 40` was 2^40). Run time and parameter defaults alike.
#[test]
fn bitwise_and_shift_operators_are_32_bit() {
    let out = op(
        "bits32",
        "integer n; parameter integer p = 1 << 31; parameter integer q = ~0;",
        "  n = -1;",
        &[
            ("a", "1 << 31"),
            ("b", "1 << 32"),
            ("c", "3 << n"),
            ("d", "~0"),
            ("e", "4294967295 & 255"),
            ("f", "2147483648 | 0"),
            ("g", "p"),
            ("h", "q"),
            ("i", "-8 >>> 40"),
            ("j", "8 >>> n"),
            ("k", "4294967288 >>> 1"),
            ("l", "(1 << 30) ^ ~0"),
        ],
    );
    for (node, want) in [
        ("a", -2147483648.0),
        ("b", 0.0),
        ("c", 0.0),
        ("d", -1.0),
        ("e", 255.0),
        ("f", -2147483648.0),
        ("g", -2147483648.0),
        ("h", -1.0),
        ("i", -1.0),
        ("j", 0.0),
        ("k", -4.0),
        ("l", -1073741825.0),
    ] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}

/// A value passed into an `integer` function argument is stored into it, so it rounds
/// (2.39.0): `half(9.6)` sees 10 and returns 5. A `real` argument is untouched.
#[test]
fn an_integer_function_argument_rounds_on_entry() {
    let out = op(
        "fnarg",
        "analog function integer half; input v; integer v; half = v / 2; endfunction
  analog function real twice; input w; real w; twice = 2 * w; endfunction",
        "",
        &[
            ("a", "half(9.6)"),
            ("b", "half(-9.6)"),
            ("c", "twice(1.25)"),
        ],
    );
    for (node, want) in [("a", 5.0), ("b", -5.0), ("c", 2.5)] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}

/// 2.40.0: `**` over integers is an integer (truncated: `2 ** -1` is 0), and integer
/// `+ - * / **` and negation wrap at 32 bits, at run time and in parameter defaults.
#[test]
fn integer_power_and_overflow_follow_32_bit_integers() {
    let out = op(
        "pow-wrap",
        "integer n, big; parameter integer p = 2147483647 + 1;",
        "  n = 2; big = 2147483647;",
        &[
            ("a", "2 ** 3"),
            ("b", "n ** -1"),
            ("c", "(-1) ** -1"),
            ("d", "2.0 ** -1"),
            ("e", "big + 1"),
            ("f", "-big - 2"),
            ("g", "65536 * 65536"),
            ("h", "p"),
            ("i", "1 ** -5"),
            ("j", "big + 1.0"),
            ("k", "-(-2147483647 - 1)"),
        ],
    );
    for (node, want) in [
        ("a", 8.0),
        ("b", 0.0),
        ("c", -1.0),
        ("d", 0.5), // a real base keeps it real
        ("e", -2147483648.0),
        ("f", 2147483647.0),
        ("g", 0.0),
        ("h", -2147483648.0),
        ("i", 1.0),
        ("j", 2147483648.0), // a real operand keeps it real
        ("k", -2147483648.0),
    ] {
        assert_eq!(op_value(&out, node), want, "V({node}): {out}");
    }
}

/// A device-line value for a `parameter integer` is rounded half away from zero (2.40.0,
/// through `va_ir::Module::integer_params`), and a real parameter is not.
#[test]
fn a_deck_value_for_an_integer_parameter_rounds() {
    let model = "module m(a, b); inout a, b; electrical a, b;
  parameter integer n = 3; parameter real x = 3;
  analog begin V(a) <+ n; V(b) <+ x; end
endmodule
";
    for (n, want) in [("2.5", 3.0), ("-2.5", -3.0), ("2.4", 2.0)] {
        let out = run(
            "deck-int",
            &[
                ("m.va", model),
                (
                    "d.net",
                    &format!("* d\nX1 a b m n={n} x=2.5\nR1 a 0 1k\nR2 b 0 1k\n.op\n"),
                ),
            ],
            &["sim", "d.net", "--model", "m.va"],
        );
        assert_eq!(op_value(&out, "a"), want, "n={n}: {out}");
        assert_eq!(
            op_value(&out, "b"),
            2.5,
            "a real parameter is not rounded: {out}"
        );
    }
}

/// The 32-bit wrap leaves a non-finite value alone, so an integer division by zero is still
/// reported (`E0603` at run time, `E0308` in a parameter default) instead of wrapping to a
/// finite number.
#[test]
fn an_integer_division_by_zero_still_reports() {
    let out = op(
        "div0",
        "integer n, d;",
        "  n = 7; d = 0;",
        &[("a", "n / d")],
    );
    assert!(out.contains("E0603"), "{out}");
}
