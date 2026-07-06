//! T6 — `va-cli`: the binary front door that wires the whole pipeline together.
//!
//! Usage (the bring-up target):
//!
//! ```text
//! va-cli sim circuits/divider.net --model models/resistor.va
//! ```
//!
//! This binary only parses arguments and dispatches to [`va_cli::run_sim`]; the pipeline
//! itself lives in the library so `va-harness` can drive it directly.

use anyhow::{bail, Context, Result};
use va_cli::{check_models, run_sim, Analysis};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sim") => cmd_sim(&args[1..]),
        Some("check") => cmd_check(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            bail!("unknown subcommand `{other}`");
        }
    }
}

/// Print the CLI usage banner.
fn print_usage() {
    eprintln!(
        "va-cli — verilog-a-sim front door\n\n\
         USAGE:\n    \
         va-cli sim <netlist.net> [--model <model.va>] [--ac|--tran] [--plot <out.svg>]\n    \
         va-cli check <model.va|dir> [more…]   Run the frontend over models, report gaps\n\n\
         FLAGS:\n    \
         -h, --help    Print this help\n    \
         --plot <out.svg>   Write an SVG plot of the transient waveform (--tran only)"
    );
}

/// The `check` subcommand: run the frontend over models/directories and report what fails.
fn cmd_check(args: &[String]) -> Result<()> {
    if args.is_empty() {
        bail!("expected at least one model file or directory");
    }
    check_models(args)
}

/// The `sim` subcommand: run a netlist through the pipeline.
fn cmd_sim(args: &[String]) -> Result<()> {
    let netlist = args.first().context("expected a netlist path")?;
    // `--model` is optional: built-in primitives (R/C/D/V) are satisfied by the reference
    // models, so a Verilog-A model is only needed for custom devices.
    let model = parse_flag(args, "--model");
    let plot = parse_flag(args, "--plot");
    let analysis = if args.iter().any(|a| a == "--tran") {
        Analysis::Transient
    } else if args.iter().any(|a| a == "--ac") {
        Analysis::Ac
    } else {
        Analysis::Dc
    };

    eprintln!(
        "[va-cli] sim netlist={netlist} model={} analysis={analysis:?}",
        model.as_deref().unwrap_or("<none>")
    );
    run_sim(netlist, model.as_deref(), analysis, plot.as_deref())
}

/// Pull the value following `flag` out of `args`.
fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}
