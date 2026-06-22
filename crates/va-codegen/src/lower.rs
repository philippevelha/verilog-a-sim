//! Lowering: walk a [`va_ir::Module`]'s analog block and turn `<+` contributions into the
//! residual/charge stamps a generated [`va_abi::ModelInstance`] emits.

use crate::CodegenError;

/// A lowered, evaluable representation of a module's analog block, ready to be wrapped in a
/// [`va_abi::ModelInstance`]. Concrete shape is defined during T2.
#[derive(Debug, Default)]
pub struct Lowered {
    /// Number of unknowns (terminals + internal nodes) the lowered model touches.
    pub n_unknowns: usize,
}

/// Lower a module's analog block into a [`Lowered`] plan.
///
/// # Errors
///
/// Returns [`CodegenError::Unsupported`] on IR constructs outside the codegen subset.
pub fn lower(_module: &va_ir::Module) -> Result<Lowered, CodegenError> {
    todo!("T2: lower the analog block into evaluable contributions")
}
