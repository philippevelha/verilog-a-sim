//! T6 — `va-cli` library: the pipeline wiring, exposed so `va-harness` can drive it.
//!
//! The binary (`main.rs`) is a thin argument-parsing wrapper over [`run_sim`]. Keeping the
//! pipeline in a library lets the validation harness call it directly instead of shelling
//! out to the executable.

#![forbid(unsafe_code)]

use anyhow::Result;

/// Which analysis to run for a `sim` invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Analysis {
    /// DC operating point / sweep (the default bring-up analysis).
    #[default]
    Dc,
    /// Transient analysis.
    Transient,
    /// AC small-signal analysis.
    Ac,
}

/// Run the full pipeline for `netlist` + Verilog-A `model` under `analysis`.
///
/// Wires `va-frontend` → `va-codegen` → `va-netlist` → `va-core` (and the transient / AC
/// crates as needed). Stubbed until the T6 integration milestone.
pub fn run_sim(netlist: &str, model: &str, analysis: Analysis) -> Result<()> {
    let _ = (netlist, model, analysis);
    todo!("T6: frontend → codegen → netlist → va-core, then print/emit results")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_default_is_dc() {
        assert_eq!(Analysis::default(), Analysis::Dc);
    }
}
