//! `deck-run` and `deck-compare`: the two halves of `deck-diff`, split so the halves can run on
//! different machines (Road to 2.0 item 3 — reproducibility across Linux, macOS and Windows).
//!
//! `deck-diff` needs both binaries on one machine. To compare *platforms*, each CI runner runs
//! `deck-run` with its own `va-cli`, writing one file per deck; the files travel as build
//! artifacts, and `deck-compare` diffs two such trees. A deck's file is exactly what
//! `deck-diff` compares — exit code, stdout, stderr without the timing `estimate:` lines — so a
//! tree compared with itself across two binaries on one machine gives `deck-diff`'s verdict.
//!
//! Every result goes to disk as it is produced: c432's transient prints ~100 MB, and holding
//! two of them in memory is what got a local `deck-diff --all` killed (2026-09-27).
//!
//! Limitation: a deck that **fails on both sides** is reported as such, not as a match — the
//! lesson of 1.26.1, where four never-run decks compared "identical" all day. Their stderr (an
//! OS error message, which differs by platform) is not compared.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::{collect_decks, deck_analysis_flag, deck_model, repo_root, DECK_DIFF_SLOW};

/// The marker lines of a result file. Chosen to be impossible as `va-cli` output lines.
const EXIT: &str = "=== exit: ";
const STDOUT: &str = "=== stdout";
const STDERR: &str = "=== stderr";

/// `cargo xtask deck-run <va-cli> <out-dir> [--all] [--include <deck>]... [--skip <deck>]...` —
/// run every deck under `circuits/` through one `va-cli` and write `<out-dir>/<deck>.txt` per
/// deck (the deck's repository path, `/`-separated, plus `.txt`). The deck set and each deck's
/// command line are `deck-diff`'s: the slow ISCAS'85 decks are left out unless `--all`, or
/// named with `--include`.
///
/// # Errors
///
/// If the binary is missing, a deck cannot be read, or a result cannot be written. A deck that
/// *fails* is not an error here — its exit code is recorded, which is the point.
pub fn deck_run(args: &[String]) -> Result<()> {
    let mut positional = Vec::new();
    let (mut all, mut include, mut skip) = (false, Vec::<String>::new(), Vec::<String>::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--all" => all = true,
            "--include" | "--skip" => {
                let deck = it
                    .next()
                    .with_context(|| format!("{a} needs a deck path"))?
                    .replace('\\', "/");
                if a == "--include" {
                    include.push(deck);
                } else {
                    skip.push(deck);
                }
            }
            _ => positional.push(PathBuf::from(a)),
        }
    }
    let [exe, out_dir] = positional.as_slice() else {
        bail!("usage: cargo xtask deck-run <va-cli> <out-dir> [--all] [--include <deck>]... [--skip <deck>]...");
    };
    if !exe.is_file() {
        bail!("{} is not a file", exe.display());
    }
    let exe = std::fs::canonicalize(exe).with_context(|| format!("resolving {}", exe.display()))?;
    if !all {
        skip.extend(
            DECK_DIFF_SLOW
                .iter()
                .filter(|s| !include.iter().any(|i| i == *s))
                .map(|s| (*s).to_string()),
        );
    }

    let root = repo_root();
    let mut decks = Vec::new();
    collect_decks(&root.join("circuits"), &mut decks)?;
    decks.sort();

    let (mut ran, mut failed, mut skipped) = (0, 0, 0);
    for deck in &decks {
        let rel = rel_path(deck, &root);
        if skip.contains(&rel) {
            skipped += 1;
            continue;
        }
        let text = std::fs::read_to_string(deck).with_context(|| format!("reading {rel}"))?;
        let dest = out_dir.join(format!("{rel}.txt"));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let stdout_path = dest.with_extension("stdout.tmp");
        let stderr_path = dest.with_extension("stderr.tmp");
        let mut cmd = Command::new(&exe);
        cmd.current_dir(&root).args(["sim", rel.as_str()]);
        if let Some(m) = deck_model(&text, &rel, &root) {
            cmd.args(["--model", m.as_str()]);
        }
        if let Some(f) = deck_analysis_flag(&text) {
            cmd.arg(f);
        }
        let started = std::time::Instant::now();
        let status = cmd
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout_path)?)
            .stderr(std::fs::File::create(&stderr_path)?)
            .status()
            .with_context(|| format!("running {} on {rel}", exe.display()))?;
        let code = status.code();
        write_result(&dest, code, &stdout_path, &stderr_path)?;
        ran += 1;
        if code != Some(0) {
            failed += 1;
        }
        eprintln!(
            "[xtask] {rel}: exit {code:?} ({:.1} s)",
            started.elapsed().as_secs_f64()
        );
    }
    eprintln!("[xtask] deck-run: {ran} run ({failed} nonzero exit), {skipped} skipped");
    Ok(())
}

/// Assemble one result file from the two streamed temporaries, then remove them. Stdout is
/// copied, not loaded; stderr is small and filtered line by line.
fn write_result(dest: &Path, code: Option<i32>, stdout: &Path, stderr: &Path) -> Result<()> {
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(dest).with_context(|| format!("writing {}", dest.display()))?,
    );
    writeln!(out, "{EXIT}{code:?}")?;
    writeln!(out, "{STDOUT}")?;
    std::io::copy(&mut std::fs::File::open(stdout)?, &mut out)?;
    writeln!(out, "{STDERR}")?;
    for line in std::fs::read_to_string(stderr)?.lines() {
        if !line.contains("estimate:") {
            writeln!(out, "{line}")?;
        }
    }
    out.flush()?;
    drop(out);
    std::fs::remove_file(stdout)?;
    std::fs::remove_file(stderr)?;
    Ok(())
}

/// `cargo xtask deck-compare <dir-a> <dir-b>` — compare two `deck-run` trees deck by deck.
/// Reports each deck that differs (first differing line), each deck present on one side only,
/// and each deck that failed on both sides (not counted as a match).
///
/// # Errors
///
/// If a tree cannot be read, or any deck differs or is missing from one side.
pub fn deck_compare(args: &[String]) -> Result<()> {
    let [a, b] = args else {
        bail!("usage: cargo xtask deck-compare <dir-a> <dir-b>");
    };
    let (a, b) = (PathBuf::from(a), PathBuf::from(b));
    let (mut left, mut right) = (Vec::new(), Vec::new());
    collect_results(&a, &a, &mut left)?;
    collect_results(&b, &b, &mut right)?;
    left.sort();
    right.sort();

    let (mut same, mut differ, mut both_failed, mut one_sided) = (0, 0, 0, 0);
    for rel in left.iter().filter(|r| !right.contains(r)) {
        one_sided += 1;
        eprintln!("[xtask] ONLY IN {}: {rel}", a.display());
    }
    for rel in right.iter().filter(|r| !left.contains(r)) {
        one_sided += 1;
        eprintln!("[xtask] ONLY IN {}: {rel}", b.display());
    }
    for rel in left.iter().filter(|r| right.contains(r)) {
        let x = std::fs::read_to_string(a.join(rel)).with_context(|| format!("reading {rel}"))?;
        let y = std::fs::read_to_string(b.join(rel)).with_context(|| format!("reading {rel}"))?;
        let (ex, ey) = (exit_line(&x), exit_line(&y));
        if ex != "Some(0)" && ey != "Some(0)" {
            both_failed += 1;
            eprintln!("[xtask] FAILED ON BOTH {rel} (exit {ex} vs {ey})");
            continue;
        }
        if x == y {
            same += 1;
            continue;
        }
        differ += 1;
        eprintln!("[xtask] DIFF {rel} (exit {ex} vs {ey})");
        if let Some((i, (l, r))) = x
            .lines()
            .zip(y.lines())
            .enumerate()
            .find(|(_, (l, r))| l != r)
        {
            let (col, a, b) = around_first_difference(l, r);
            let differing = x.lines().zip(y.lines()).filter(|(l, r)| l != r).count();
            eprintln!(
                "[xtask]   line {}, column {col} ({differing} line(s) differ):\n\
                 [xtask]     a: {a}\n[xtask]     b: {b}",
                i + 1
            );
            eprintln!("[xtask]   {}", NumericBound::of(&x, &y));
        } else {
            eprintln!(
                "[xtask]   {} lines vs {}",
                x.lines().count(),
                y.lines().count()
            );
        }
    }
    eprintln!(
        "[xtask] deck-compare: {same} identical, {differ} different, {both_failed} failed on \
         both sides, {one_sided} on one side only"
    );
    if differ > 0 || one_sided > 0 {
        bail!("{differ} deck(s) differ, {one_sided} present on one side only");
    }
    Ok(())
}

/// Values at or below this magnitude on both sides are round-off around zero (a ground-tied
/// node's `-7.8e-32 V` against `3.2e-32 V`): their relative difference is meaningless, so the
/// relative bound skips them. Their absolute difference is still reported.
const ROUNDOFF_FLOOR: f64 = 1e-15;

/// How far apart two same-shaped outputs are, as numbers: over every whitespace token pair that
/// differs, `name=value` compared by value. What turns "these decks differ" into a bound that
/// can be stated in writing (Road to 2.0 item 3: "every difference explained and bounded").
#[derive(Debug, Default, PartialEq)]
struct NumericBound {
    /// Largest `|a - b|`, and the token (`name=`) it was found at.
    max_abs: (f64, String),
    /// Largest `|a - b| / max(|a|, |b|)` among pairs above [`ROUNDOFF_FLOOR`], and where.
    max_rel: (f64, String),
    /// Differing token pairs that are not both numbers — a structural difference, not a
    /// numerical one (a line count, a point count in a header).
    non_numeric: usize,
}

impl NumericBound {
    fn of(x: &str, y: &str) -> Self {
        let mut bound = Self::default();
        for (l, r) in x.lines().zip(y.lines()).filter(|(l, r)| l != r) {
            for (a, b) in l.split_whitespace().zip(r.split_whitespace()) {
                if a == b {
                    continue;
                }
                let (name, va) = a.rsplit_once('=').unwrap_or(("", a));
                let vb = b.rsplit_once('=').map_or(b, |(_, v)| v);
                let (Ok(va), Ok(vb)) = (
                    va.trim_end_matches(['s', ',']).parse::<f64>(),
                    vb.trim_end_matches(['s', ',']).parse::<f64>(),
                ) else {
                    bound.non_numeric += 1;
                    continue;
                };
                let abs = (va - vb).abs();
                if abs > bound.max_abs.0 {
                    bound.max_abs = (abs, name.to_string());
                }
                let scale = va.abs().max(vb.abs());
                if scale > ROUNDOFF_FLOOR && abs / scale > bound.max_rel.0 {
                    bound.max_rel = (abs / scale, name.to_string());
                }
            }
        }
        bound
    }
}

impl std::fmt::Display for NumericBound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "numeric: max |a-b| {:.3e} ({}); max relative {:.3e} ({}) among values above {:e}; \
             {} non-numeric token(s) differ",
            self.max_abs.0,
            self.max_abs.1,
            self.max_rel.0,
            self.max_rel.1,
            ROUNDOFF_FLOOR,
            self.non_numeric
        )
    }
}

/// Where two lines first differ (a 1-based character column) and a window of each around it: a
/// transient's output line holds every reported quantity and runs to thousands of characters,
/// so printing it whole hides the one number that moved.
fn around_first_difference<'a>(a: &'a str, b: &'a str) -> (usize, &'a str, &'a str) {
    let at = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i);
    let window = |s: &'a str| -> &'a str {
        let mut from = at.saturating_sub(60).min(s.len());
        while !s.is_char_boundary(from) {
            from -= 1;
        }
        let mut to = (at + 60).min(s.len());
        while !s.is_char_boundary(to) {
            to += 1;
        }
        &s[from..to]
    };
    (a[..at].chars().count() + 1, window(a), window(b))
}

/// The recorded exit code of a result file, as written (`Some(0)`, `Some(1)`, `None`).
fn exit_line(text: &str) -> &str {
    text.lines()
        .next()
        .and_then(|l| l.strip_prefix(EXIT))
        .unwrap_or("?")
}

/// Every `.txt` result under `dir`, as a `/`-separated path relative to `base`.
fn collect_results(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_results(base, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "txt") {
            out.push(rel_path(&path, base));
        }
    }
    Ok(())
}

/// `path` relative to `base`, `/`-separated on every platform.
fn rel_path(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree compared with itself is identical; a changed line, a deck failing on both sides
    /// and a deck missing from one side are each reported as what they are.
    /// Round-off around zero moves the absolute bound but not the relative one; a real change
    /// moves the relative one; a point count is non-numeric noise of its own kind.
    #[test]
    fn the_numeric_bound_separates_roundoff_from_a_real_change() {
        let a = "t=1.0e-9s  V(vss)=-7.780757e-32 V  V(out)=1.200000 V\n";
        let b = "t=1.0e-9s  V(vss)=3.204747e-32 V  V(out)=1.200000 V\n";
        let n = NumericBound::of(a, b);
        assert_eq!(n.max_rel.0, 0.0, "{n}");
        assert!(n.max_abs.0 > 1e-31 && n.max_abs.1 == "V(vss)", "{n}");
        let c = b.replace("1.200000", "1.200012");
        let m = NumericBound::of(a, &c);
        assert!(
            (m.max_rel.0 - 1e-5).abs() < 1e-9 && m.max_rel.1 == "V(out)",
            "{m}"
        );
        let h = NumericBound::of(
            "Transient analysis (1778 points",
            "Transient analysis (1682 points",
        );
        assert_eq!(h.non_numeric, 1, "{h}");
    }

    /// The window is centred on the first differing character, so a long line's one changed
    /// number is what gets printed.
    #[test]
    fn the_first_difference_is_located_and_windowed() {
        let a = format!("{}V(out)=1.000000 V{}", "x".repeat(200), "y".repeat(200));
        let b = a.replace("1.000000", "1.000001");
        let (col, wa, wb) = around_first_difference(&a, &b);
        assert_eq!(col, 200 + "V(out)=1.00000".len() + 1);
        assert!(
            wa.contains("1.000000") && wb.contains("1.000001"),
            "{wa} / {wb}"
        );
        assert!(wa.len() <= 120);
    }

    #[test]
    fn deck_compare_tells_differences_failures_and_gaps_apart() {
        let base = std::env::temp_dir().join(format!("va-deck-compare-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        let put = |dir: &Path, rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(p, text).expect("write");
        };
        let ok = format!("{EXIT}Some(0)\n{STDOUT}\nV(out) = 1.0 V\n{STDERR}\n");
        for d in [&a, &b] {
            put(d, "circuits/x.net.txt", &ok);
        }
        deck_compare(&[a.display().to_string(), b.display().to_string()])
            .expect("identical trees compare equal");

        put(
            &b,
            "circuits/x.net.txt",
            &ok.replace("1.0 V", "1.0000001 V"),
        );
        assert!(deck_compare(&[a.display().to_string(), b.display().to_string()]).is_err());

        put(&b, "circuits/x.net.txt", &ok);
        let fail_a = format!("{EXIT}Some(1)\n{STDOUT}\n{STDERR}\nNo such file (os error 2)\n");
        let fail_b = fail_a.replace("No such file", "The system cannot find the file");
        put(&a, "circuits/y.net.txt", &fail_a);
        put(&b, "circuits/y.net.txt", &fail_b);
        deck_compare(&[a.display().to_string(), b.display().to_string()])
            .expect("a deck failing on both sides is reported, not a difference");

        put(&a, "circuits/z.net.txt", &ok);
        assert!(deck_compare(&[a.display().to_string(), b.display().to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
