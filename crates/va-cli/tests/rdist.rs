//! `$rdist_*` draws (LRM 9.13.2, 2.42.0). They folded to the distribution's mean before, so
//! every Monte Carlo draw was the nominal value. The generators' statistics are unit-tested in
//! `va_codegen::rdist`; this checks the seed handling a model sees, through the binary.

use std::process::Command;

/// Run `va-cli sim` on `model` and `deck` with `args`; stdout and stderr, joined.
fn sim(tag: &str, model: &str, deck: &str, args: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("va-cli-rdist-{}-{tag}", std::process::id()));
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

/// The `.op` value printed after `key` (`"V(a) ="`, `"I(V1) ="`).
fn op_value(out: &str, key: &str) -> f64 {
    out.lines()
        .find_map(|l| l.trim_start().strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no {key} in:\n{out}"))
}

/// The values of `quantity` on the printed transient lines.
fn tran_values(out: &str, quantity: &str) -> Vec<f64> {
    let key = format!("{quantity}=");
    out.lines()
        .filter_map(|l| {
            l.split(&key)
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

const MC: &str = "module mcres(p, n); inout p, n; electrical p, n;
  parameter integer s0 = 7;
  integer seed; real r;
  analog begin
    @(initial_step) begin seed = s0; r = $rdist_normal(seed, 1k, 50); end
    I(p, n) <+ V(p, n) / r;
  end
endmodule
";

/// A Monte Carlo resistor: drawn once, in `@(initial_step)`. Different seeds give different
/// resistors near 1 kOhm, the same seed the same one, and the value holds through a transient.
#[test]
fn a_monte_carlo_draw_is_reproducible_and_fixed_for_the_run() {
    let current = |s0: &str| {
        op_value(
            &sim(
                "mc",
                MC,
                &format!("* mc\nV1 a 0 DC 1\nX1 a 0 mcres s0={s0}\n.op\n"),
                &[],
            ),
            "I(V1) =",
        )
    };
    let (a, b, a2) = (current("7"), current("8"), current("7"));
    assert_eq!(a, a2, "same seed, same draw");
    assert_ne!(a, b, "different seeds, different draws");
    for i in [a, b] {
        let r = 1.0 / i.abs();
        assert!((r - 1000.0).abs() < 250.0, "a 1 kOhm +- 50 draw, got {r}");
    }
    let out = sim(
        "mc-tran",
        MC,
        "* mc\nV1 a 0 DC 1\nX1 a 0 mcres s0=7\n.tran 0.1u 0.5u\n",
        &["--tran", "--report", "I(V1)"],
    );
    let i = tran_values(&out, "I(V1)");
    assert!(
        i.len() > 3 && i.iter().all(|&v| v == i[0]),
        "drawn once: {out}"
    );
}

/// A random signal, sampled and held: drawn at each timer tick and smoothed by `transition`.
/// Reproducible run to run, inside `[0, 1]`, and not all the same.
#[test]
fn a_sample_and_hold_draw_changes_at_each_tick_and_is_reproducible() {
    let model = "module sh(out); inout out; electrical out;
  integer seed; real u;
  analog begin
    @(initial_step) begin seed = 11; u = 0.5; end
    @(timer(0, 0.1u)) u = $rdist_uniform(seed, 0, 1);
    V(out) <+ transition(u, 0, 1n);
  end
endmodule
";
    let deck = "* sh\nX1 out sh\nR1 out 0 1k\n.tran 0.05u 2u\n";
    let run = || {
        tran_values(
            &sim("sh", model, deck, &["--tran", "--report", "out"]),
            "V(out)",
        )
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b, "same seed, same waveform");
    assert!(
        a.iter().all(|&v| (-1e-9..=1.0 + 1e-9).contains(&v)),
        "{a:?}"
    );
    let distinct: std::collections::BTreeSet<u64> = a.iter().map(|v| (v * 1e4) as u64).collect();
    assert!(
        distinct.len() > 10,
        "a new value at each tick: {distinct:?}"
    );
    assert!(!sim("sh-warn", model, deck, &["--tran"]).contains("W0403"));
}

/// A seed variable advances at every draw, so 2000 draws in one evaluation walk the sequence:
/// their mean is the distribution's (5 here, standard error 2/sqrt(2000) = 0.045).
#[test]
fn draws_in_a_loop_walk_one_sequence() {
    let model = "module avg(a); inout a; electrical a;
  integer seed, k; real sum;
  analog begin
    seed = 3; sum = 0;
    for (k = 0; k < 2000; k = k + 1) sum = sum + $rdist_normal(seed, 5, 2);
    V(a) <+ sum / 2000;
  end
endmodule
";
    let out = sim("avg", model, "* avg\nX1 a avg\nR1 a 0 1k\n.op\n", &[]);
    let mean = op_value(&out, "V(a) =");
    assert!((mean - 5.0).abs() < 0.2, "mean of 2000 draws: {mean}");
    assert!(
        !out.contains("W0403"),
        "a static run is not warned about: {out}"
    );
}

/// A constant seed draws the same value every time: shared by all instances with `"global"`
/// (the default), one per instance with `"instance"`.
#[test]
fn a_constant_seed_is_global_or_per_instance() {
    let model = |ty: &str| {
        format!(
            "module cs(out); inout out; electrical out;\n  analog V(out) <+ $rdist_normal(23, 0, 1{ty});\nendmodule\n"
        )
    };
    let deck = "* cs\nX1 a cs\nX2 b cs\nR1 a 0 1k\nR2 b 0 1k\n.op\n";
    for (ty, same) in [
        ("", true),
        (", \"global\"", true),
        (", \"instance\"", false),
    ] {
        let out = sim("cs", &model(ty), deck, &[]);
        let (a, b) = (op_value(&out, "V(a) ="), op_value(&out, "V(b) ="));
        assert_eq!(a == b, same, "type `{ty}`: {a} vs {b}");
    }
    let out = sim("cs-bad", &model(", \"each\""), deck, &[]);
    assert!(out.contains("E0398"), "{out}");
}

/// A draw with a seed variable outside any event body jumps at every step in a transient run:
/// warned at compile time (`W0403`), and the run cannot integrate it.
#[test]
fn a_per_evaluation_draw_is_warned_about() {
    let model = "module nz(out); inout out; electrical out;
  integer seed; real u;
  analog begin
    @(initial_step) seed = 11;
    u = $rdist_uniform(seed, 0, 1);
    V(out) <+ u;
  end
endmodule
";
    let out = sim(
        "nz",
        model,
        "* nz\nX1 out nz\nR1 out 0 1k\n.tran 0.1u 0.5u\n",
        &["--tran"],
    );
    assert!(out.contains("warning[W0403]"), "{out}");
}
