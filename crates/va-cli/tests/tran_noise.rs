//! `--tran-noise` through the binary (2.55.0): white noise sources active in a transient run,
//! reproducible by seed, and a warning for the sources it cannot generate.

use std::process::Command;

fn run(tag: &str, files: &[(&str, &str)], args: &[&str]) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("va-cli-tnoise-{}-{tag}", std::process::id()));
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
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A 1 kOhm resistor on 1 pF, read through a noiseless x10^4 source (the node itself is
/// microvolts, below what a printed voltage resolves).
const DECK: &str = "* thermal noise\nR1 out gnd 1000\nC1 out gnd 1e-12\nE1 amp gnd out gnd 10000\n\
                    R2 amp gnd 1e9\n.tran 25e-12 20e-9\n.end\n";

fn amp_values(stdout: &str) -> Vec<f64> {
    stdout
        .lines()
        .filter_map(|l| l.split("V(amp)=").nth(1))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}

#[test]
fn a_noisy_transient_moves_and_is_reproducible_by_its_seed() {
    let go = |tag: &str, extra: &[&str]| {
        let mut args = vec![
            "sim", "d.net", "--tran", "--report", "amp", "--abstol", "1e-9",
        ];
        args.extend_from_slice(extra);
        run(tag, &[("d.net", DECK)], &args).0
    };
    let quiet = go("quiet", &[]);
    assert!(amp_values(&quiet).iter().all(|&v| v == 0.0), "{quiet}");

    let a = go("a", &["--tran-noise", "20e9"]);
    assert!(a.contains("transient noise: white sources"), "{a}");
    let va = amp_values(&a);
    // kT/C is 64 uV rms at the node, 0.64 V after the x10^4 source.
    let rms = (va.iter().map(|v| v * v).sum::<f64>() / va.len() as f64).sqrt();
    assert!(
        rms > 0.1 && rms < 3.0,
        "rms {rms} V over {} points",
        va.len()
    );

    assert_eq!(
        a,
        go("b", &["--tran-noise", "20e9", "--tran-noise-seed", "1"])
    );
    assert_ne!(
        a,
        go("c", &["--tran-noise", "20e9", "--tran-noise-seed", "2"])
    );
}

#[test]
fn a_flicker_source_is_reported_as_left_out() {
    let va = "`include \"disciplines.vams\"\nmodule fl(p, n); inout p, n; electrical p, n;\n\
              analog I(p, n) <+ V(p, n) / 1000.0 + flicker_noise(1e-20, 1.0, \"f\");\nendmodule\n";
    let disc = include_str!("../../../models/disciplines.vams");
    let deck = "* flicker\nX1 out gnd fl\nC1 out gnd 1e-12\n.tran 25e-12 1e-9\n.end\n";
    let files = [("fl.va", va), ("disciplines.vams", disc), ("d.net", deck)];
    let (_, err) = run(
        "flicker",
        &files,
        &[
            "sim",
            "d.net",
            "--model",
            "fl.va",
            "--tran",
            "--tran-noise",
            "20e9",
        ],
    );
    assert!(err.contains("warning[W0602]"), "{err}");
    let (_, err) = run(
        "noflag",
        &files,
        &["sim", "d.net", "--model", "fl.va", "--tran"],
    );
    assert!(!err.contains("W0602"), "{err}");
}

#[test]
fn a_bad_bandwidth_is_an_error() {
    let (out, err) = run(
        "bad",
        &[("d.net", DECK)],
        &["sim", "d.net", "--tran", "--tran-noise", "fast"],
    );
    assert!((out + &err).contains("error[E0703]"));
}
