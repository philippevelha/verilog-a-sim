//! `--tran-noise` through the binary (2.55.0): white noise sources active in a transient run,
//! reproducible by seed, a warning for the sources it cannot generate, flicker sources (2.57.0),
//! and compact-model decks that once could not start (2.57.0).

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

/// A flicker source (2.57.0) is generated, not left out: the node moves, W0602 is silent, the
/// band is announced; `--tran-noise-fmin` above the band turns it off and W0602 says so. A
/// `noise_table` source is still left out.
#[test]
fn a_flicker_source_is_generated_and_a_table_one_left_out() {
    let va = "`include \"disciplines.vams\"
module fl(p, n); inout p, n; electrical p, n;
              analog I(p, n) <+ V(p, n) / 1000.0 + flicker_noise(1e-14, 1.0, \"f\");
endmodule
";
    let tab = "`include \"disciplines.vams\"
module tb(p, n); inout p, n; electrical p, n;
               analog I(p, n) <+ V(p, n) / 1000.0 + noise_table({1.0, 1e-20, 1e9, 1e-20}, \"t\");
endmodule
";
    let disc = include_str!("../../../models/disciplines.vams");
    let deck = "* flicker
X1 out gnd fl
C1 out gnd 1e-12
E1 amp gnd out gnd 1
R2 amp gnd 1e9
                .tran 25e-12 1e-9
.end
";
    let files = [("fl.va", va), ("disciplines.vams", disc), ("d.net", deck)];
    let args = |extra: &'static [&'static str]| {
        let mut a = vec![
            "sim",
            "d.net",
            "--model",
            "fl.va",
            "--tran",
            "--report",
            "amp",
            "--tran-noise",
            "20e9",
        ];
        a.extend_from_slice(extra);
        a
    };
    let (out, err) = run("flicker", &files, &args(&[]));
    assert!(!err.contains("W0602"), "{err}");
    assert!(
        out.contains("flicker sources from 1.000e9 to 5.000e9 Hz"),
        "{out}"
    );
    // 1e-14 A^2/Hz * ln(5) through 1 kOhm: about 0.13 mV rms
    assert!(
        amp_values(&out).iter().skip(1).any(|v| v.abs() > 1e-6),
        "{out}"
    );

    let (out, err) = run("fmin", &files, &args(&["--tran-noise-fmin", "1e10"]));
    assert!(err.contains("warning[W0602]"), "{err}");
    assert!(out.contains("no flicker band"), "{out}");

    let tfiles = [
        ("fl.va", tab),
        ("disciplines.vams", disc),
        (
            "d.net",
            &deck.replace(
                " fl
", " tb
",
            ),
        ),
    ];
    let (_, err) = run("table", &tfiles, &args(&[]));
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

/// Runs `va-cli` on a fixture deck with `--tran-noise 1e11`, reporting node `node`, killing it
/// after `limit`: a regression here was a run that never finished, which must fail the test, not
/// hang it. Both pipes are drained on their own threads while the child runs (a run prints a
/// line per timepoint, far more than a pipe holds).
fn run_fixture(deck: &str, node: &str, limit: std::time::Duration) -> Result<String, String> {
    use std::io::Read;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let psp = root.join("../../models/sg13g2/psp103/psp103.va");
    let mut child = Command::new(env!("CARGO_BIN_EXE_va-cli"))
        .arg("sim")
        .arg(root.join("tests/fixtures").join(deck))
        .arg("--model")
        .arg(psp)
        .args(["--tran", "--tran-noise", "1e11", "--report", node])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("va-cli runs");
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            r.read_to_string(&mut text).ok();
            text
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("stdout")));
    let err = drain(Box::new(child.stderr.take().expect("stderr")));
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break Some(status);
        }
        if start.elapsed() > limit {
            child.kill().ok();
            child.wait().ok();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let text = out.join().unwrap_or_default() + &err.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => Ok(text),
        Some(_) => Err(text),
        None => Err(format!("{deck}: no result within {limit:?}")),
    }
}

/// Two SG13G2 HV PSP103 decks (the IHP open PDK's cards, inline) that transient noise could not
/// run before 2.57.0 (found 2026-11-17 on the codesign microring driver): a transistor on at
/// Vds = 0, and an inverter switching. The transient met every source as a step at its first
/// timepoint, which PSP103's charge rows turned into a step controller that never got past
/// `t = 0` (verified: with that step, neither deck gives a result within a minute). Since 2.57.0
/// every source is ramped in over the first sample. The inverter also
/// guards the densities' continuity through a switching edge, where they change by decades
/// (holding each constant between accepted timepoints made them jump there and ended a
/// longer run with E0605); that variant no longer exists in the code to be shown failing here.
#[test]
fn psp103_decks_run_under_transient_noise() {
    let limit = std::time::Duration::from_secs(180);
    for (deck, node, points_at_least) in [
        ("tran_noise_psp_hv_vds0.net", "d", 50),
        ("tran_noise_psp_hv_inverter.net", "o", 400),
    ] {
        let out = run_fixture(deck, node, limit).unwrap_or_else(|e| panic!("{e}"));
        let n: usize = out
            .split("Transient analysis (")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{deck}: no transient result\n{out}"));
        assert!(n >= points_at_least, "{deck}: {n} points\n{out}");
    }
}
