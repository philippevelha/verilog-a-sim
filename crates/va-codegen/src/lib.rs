//! T2 — code generation: lower a [`va_ir::Module`] into a [`va_abi::ModelInstance`].
//!
//! This is the highest-risk crate: it differentiates the IR (forward-mode AD over the
//! expression arena) to produce exact Jacobians. Per §5 every differentiated operator is
//! checked against a central finite difference — a wrong Jacobian silently kills Newton.

#![forbid(unsafe_code)]

pub mod ad;
pub mod lower;

use thiserror::Error;
use va_abi::ModelInstance;
use va_ir::Module;

/// Errors raised while lowering/differentiating the IR.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// The IR used a construct this codegen subset does not yet support.
    #[error("unsupported construct: {0}")]
    Unsupported(String),
}

/// Compile an elaborated IR module into a loadable model instance bound to `terminals`
/// (the global unknown indices the instance's ports map onto).
///
/// Stubbed until T2 lands. The returned instance must satisfy the AD-vs-FD contract (§5).
pub fn build_instance(
    _module: &Module,
    _terminals: &[usize],
) -> Result<Box<dyn ModelInstance>, CodegenError> {
    todo!("T2: lower IR + AD into a ModelInstance")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "T2: AD Jacobian must match central finite differences (§5)"]
    fn ad_matches_finite_difference() {
        // Milestone: differentiate the diode IR and assert the analytic Jacobian agrees
        // with a central difference to < 1e-5 relative error.
    }
}
