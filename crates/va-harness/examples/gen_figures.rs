//! Regenerates the committed sim-vs-golden overlay figure(s) embedded in `docs/tutorials/`.
//!
//! This is the reproduction command `docs/tutorials/t6-integration/03-validation.qmd` names
//! next to the figure it embeds — `CLAUDE.md`'s tutorial-skeleton note that "a tutorial that
//! cannot be re-run to reproduce its figures has rotted" applies to a picture exactly as much as
//! to a code snippet, so the figure is not hand-drawn or committed from a one-off script outside
//! the crate: it comes from the same [`va_harness::plot::overlay_tran`] every future regen would
//! call.
//!
//! # Run it
//!
//! ```text
//! cargo run -p va-harness --example gen_figures
//! ```
//!
//! (from anywhere — paths are resolved from `CARGO_MANIFEST_DIR`, not the caller's `cwd`, the
//! same robustness trick the crate's own `#[cfg(test)]` modules already use.)

use va_harness::golden::GoldenTran;
use va_harness::plot::overlay_tran;
use va_harness::tran::run_tran;

/// Absolute path to a workspace file, robust to `cargo run`'s working directory.
fn workspace_path(rel: &str) -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rung 4: half-wave rectifier, the tightest of the six ladder-rung margins
    // (`t6-integration/03-validation.qmd`'s own `error=6.766e-4` against a `1e-3` tolerance) —
    // the figure that most needs a picture, since the margin is the thinnest.
    let circuit = workspace_path("circuits/rectifier.net");
    let model = workspace_path("models/diode.va");
    let golden_path = workspace_path("golden/rectifier.golden");
    let out = workspace_path("docs/tutorials/t6-integration/figures/rectifier-overlay.svg");

    let got = run_tran(&circuit, Some(&model))?;
    let golden = GoldenTran::read(std::path::Path::new(&golden_path))?;
    overlay_tran(&out, &got, &golden)?;
    println!("[gen_figures] wrote {out}");

    Ok(())
}
