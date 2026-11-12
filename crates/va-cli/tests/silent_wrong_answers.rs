//! Constructs that used to run and give a wrong answer without a word, and constructs whose
//! error named the wrong thing (2.33.0; `docs/proposals/openvaf-shared-defects.md` items 3-7
//! and section 3). Each now fails with its own code. Each is checked through the binary, where
//! a user meets it, alongside a near miss that must stay quiet: a check that fires on correct
//! models would be worse than the silence it replaced.

use std::process::Command;

/// Run `va-cli` with `args` in a fresh directory holding `files`; stdout and stderr, joined.
fn run(tag: &str, files: &[(&str, &str)], args: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("va-cli-silent-{}-{tag}", std::process::id()));
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

/// `va-cli check` on a one-module file whose analog block is `analog`, with `decls` before it.
fn check(tag: &str, decls: &str, analog: &str) -> String {
    let src = format!(
        "module m(a, b); inout a, b; electrical a, b; {decls}\nanalog {analog}\nendmodule\n"
    );
    run(tag, &[("m.va", &src)], &["check", "m.va"])
}

#[test]
fn an_invalid_event_trigger_is_an_error_not_an_unconditional_statement() {
    for trigger in ["foo", "x", "posedge x"] {
        let out = check(
            "trigger",
            "real k, x;",
            &format!("begin k = 0; @({trigger}) k = 1; I(a, b) <+ k * V(a, b); end"),
        );
        assert!(out.contains("error[E0239]"), "@({trigger}): {out}");
    }
    // The global events the parser runs unconditionally on purpose are still accepted.
    for trigger in ["initial_instance", "initial_model"] {
        let out = check(
            "global",
            "real k;",
            &format!("begin k = 0; @({trigger}) k = 1; I(a, b) <+ k * V(a, b); end"),
        );
        assert!(
            !out.contains("E0239"),
            "@({trigger}) must stay legal: {out}"
        );
        assert!(out.contains("1/1 files"), "@({trigger}): {out}");
    }
}

#[test]
fn surplus_arguments_to_transition_slew_and_absdelay_are_errors() {
    for call in [
        "transition(V(a), 0, 1n, 1n, 1p, 7)",
        "slew(V(a), 1, -1, 3)",
        "absdelay(V(a), 1n, 2n, 3n)",
    ] {
        let out = check("arity", "", &format!("V(b) <+ {call};"));
        assert!(out.contains("error[E0334]"), "{call}: {out}");
    }
    let out = check("arity-ok", "", "V(b) <+ transition(V(a), 0, 1n, 1n, 1p);");
    assert!(out.contains("1/1 files"), "five arguments are legal: {out}");
}

#[test]
fn an_absdelay_delay_known_at_compile_time_must_be_in_range() {
    for (call, why) in [
        ("absdelay(V(a), -1n)", "negative"),
        ("absdelay(V(a), 2n, 1n)", "above max_delay"),
    ] {
        let out = check("absdelay", "", &format!("V(b) <+ {call};"));
        assert!(out.contains("error[E0396]"), "{why}: {out}");
    }
    let out = check(
        "absdelay-ok",
        "parameter real td = 1n;",
        "V(b) <+ absdelay(V(a), td, 2n);",
    );
    assert!(
        out.contains("1/1 files"),
        "an in-range delay is legal: {out}"
    );
}

#[test]
fn constructs_that_were_misdiagnosed_now_name_themselves() {
    let cases = [
        (
            "string name;",
            "I(a, b) <+ V(a, b);",
            "refused[R0204]",
        ),
        (
            "integer i; real x;",
            "begin x = 0; for (i = 0; i < 4; i = i + 1) begin if (i == 2) break; x = x + 1; end I(a, b) <+ x; end",
            "error[E023A]",
        ),
        (
            "integer i; real x;",
            "begin x = 0; for (i = 0; i < 4; i = i + 1) begin if (i == 2) continue; x = x + 1; end I(a, b) <+ x; end",
            "error[E023A]",
        ),
        (
            "real g[0:1], h[0:1];",
            "begin g[0] = 1; g[1] = 2; h = g; I(a, b) <+ h[1]; end",
            "error[E0397]",
        ),
    ];
    for (decls, analog, code) in cases {
        let out = check("named", decls, analog);
        assert!(out.contains(code), "{decls} {analog}: want {code}: {out}");
    }
    let paramset = "module r(a); inout a; electrical a; parameter real rr = 1k;\n\
                    analog I(a) <+ V(a) / rr;\nendmodule\n\
                    paramset rfast r;\n  parameter real x = 1;\n  .rr = x;\nendparamset\n";
    let out = run("paramset", &[("p.va", paramset)], &["check", "p.va"]);
    assert!(out.contains("error[E023B]"), "{out}");
}

const RANGED: &str = "module rr(p, n); inout p, n; electrical p, n;
parameter real r = 1k from (0:inf);
analog I(p, n) <+ V(p, n) / r;
endmodule
";

#[test]
fn a_deck_value_outside_a_declared_range_is_an_error() {
    let sim = |value: &str| {
        let deck = format!("* range\nV1 a 0 DC 1\nX1 a 0 rr r={value}\n.op\n");
        run(
            "range",
            &[("rr.va", RANGED), ("rr.net", &deck)],
            &["sim", "rr.net", "--model", "rr.va"],
        )
    };
    let out = sim("-5");
    assert!(out.contains("error[E073A]"), "{out}");
    assert!(!out.contains("DC operating point"), "must not solve: {out}");
    let out = sim("2k");
    assert!(!out.contains("E073A"), "an in-range value solves: {out}");
    assert!(out.contains("DC operating point"), "{out}");
}

/// The index is a variable (`i = 7`), so it is only known while the model runs.
const INDEXED: &str = "module oob(a); inout a; electrical a; real g[0:3]; integer i;
parameter integer k = 7;
analog begin
  g[0] = 1; g[1] = 2; g[2] = 3; g[3] = 4; i = k;
  I(a) <+ V(a) * GUARD / 1k;
end
endmodule
";

#[test]
fn a_runtime_index_out_of_range_reports_an_error() {
    let sim = |guard: &str, k: &str| {
        let model = INDEXED
            .replace("GUARD", guard)
            .replace("k = 7", &format!("k = {k}"));
        run(
            "index",
            &[
                ("oob.va", &model),
                ("oob.net", "* index\nV1 a 0 DC 1\nX1 a oob\n.op\n"),
            ],
            &["sim", "oob.net", "--model", "oob.va"],
        )
    };
    let out = sim("g[i]", "7");
    assert!(
        out.contains("index 7 is outside `g`'s declared range [0:3]"),
        "{out}"
    );
    assert!(
        out.contains("error[E0739]"),
        "the run must not end clean: {out}"
    );
    // In range: silent.
    let out = sim("g[i]", "2");
    assert!(!out.contains("$error"), "{out}");
    // Read only when in range: the guard must not run where the read does not.
    let out = sim("(i <= 3 ? g[i] : 0)", "7");
    assert!(
        !out.contains("$error"),
        "a guarded read must stay quiet: {out}"
    );
}

/// `.noise` through an `absdelay` (2.53.1). R1's thermal current sees
/// `Z = R1 / (1 + (R1/r)·exp(-jω·td))`, so with R1/r = 0.5 the output PSD at f = 1/(2·td)
/// is 9 times its value at f = 1/td. Until 2.53.1 the noise analysis linearized once, with no
/// frequency, the delay entered as a wire, and the spectrum was flat.
#[test]
fn noise_through_an_absdelay_follows_the_delay() {
    let va = "`include \"disciplines.vams\"
module dl(p, n); inout p, n; electrical p, n;
              analog I(p, n) <+ absdelay(V(p, n) / 1000.0, 1e-6);
endmodule
";
    let disc = include_str!("../../../models/disciplines.vams");
    let deck = "* noise through a delay
V1 in gnd DC 0 AC 1
R1 in out 500
                X1 out gnd dl
.noise V(out) V1 lin 2 5e5 1e6
.end
";
    let out = run(
        "noise-delay",
        &[("dl.va", va), ("disciplines.vams", disc), ("d.net", deck)],
        &["sim", "d.net", "--model", "dl.va", "--noise"],
    );
    let psd: Vec<f64> = out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("f="))
        .filter_map(|l| l.split("S=").nth(1))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect();
    assert_eq!(psd.len(), 2, "{out}");
    let four_kt_r = 4.0 * 1.380649e-23 * 300.15 * 500.0;
    assert!((psd[0] / (four_kt_r * 4.0) - 1.0).abs() < 1e-5, "{out}");
    assert!((psd[1] / (four_kt_r / 2.25) - 1.0).abs() < 1e-5, "{out}");
}
