//! The [`StampSink`] trait — how a model deposits its contributions into the system.
//!
//! Rows and columns are **global unknown indices** (the same space returned by
//! [`crate::ModelInstance::unknowns`]). The assembler that implements this trait maps those
//! indices into the MNA matrix/RHS, applying the ground/reference reduction. Models never
//! see the matrix directly — they only emit `(row, value)` and `(row, col, value)` triples.

/// A sink for the four contribution channels a model produces during `load`.
///
/// Implementors accumulate (sum) the values; a model may stamp the same `(row, col)` more
/// than once. The reference node (ground) is handled by the assembler, not the model.
pub trait StampSink {
    /// Add `value` to the residual at global row `row` (current flowing **into** node `row`).
    fn residual(&mut self, row: usize, value: f64);

    /// Add `value` to the Jacobian entry `∂residual[row] / ∂x[col]`.
    fn jacobian(&mut self, row: usize, col: usize, value: f64);

    /// Add `value` to the charge `Q` at global row `row` (transient only).
    fn charge(&mut self, row: usize, value: f64);

    /// Add `value` to the charge Jacobian `∂Q[row] / ∂x[col]` (transient only).
    fn dcharge(&mut self, row: usize, col: usize, value: f64);
}

/// A minimal in-memory [`StampSink`] backed by dense vectors. Intended for tests and for
/// `va-abi`'s own reference-model checks; production assembly lives in `va-core`.
///
/// `dim` is the number of global unknowns. Out-of-range indices are ignored, which lets a
/// caller model the reference node as a sentinel index `>= dim` (e.g. ground).
#[derive(Clone, Debug)]
pub struct DenseStamp {
    dim: usize,
    /// Residual vector, length `dim`.
    pub residual: Vec<f64>,
    /// Dense Jacobian, row-major `dim * dim`.
    pub jacobian: Vec<f64>,
    /// Charge vector, length `dim`.
    pub charge: Vec<f64>,
    /// Dense charge Jacobian, row-major `dim * dim`.
    pub dcharge: Vec<f64>,
}

impl DenseStamp {
    /// Allocate a zeroed sink for a system of `dim` global unknowns.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            residual: vec![0.0; dim],
            jacobian: vec![0.0; dim * dim],
            charge: vec![0.0; dim],
            dcharge: vec![0.0; dim * dim],
        }
    }

    /// Number of global unknowns this sink covers.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Read a Jacobian entry, or `0.0` if either index is the reference node.
    pub fn jac(&self, row: usize, col: usize) -> f64 {
        if row < self.dim && col < self.dim {
            self.jacobian[row * self.dim + col]
        } else {
            0.0
        }
    }
}

impl StampSink for DenseStamp {
    fn residual(&mut self, row: usize, value: f64) {
        if row < self.dim {
            self.residual[row] += value;
        }
    }

    fn jacobian(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            self.jacobian[row * self.dim + col] += value;
        }
    }

    fn charge(&mut self, row: usize, value: f64) {
        if row < self.dim {
            self.charge[row] += value;
        }
    }

    fn dcharge(&mut self, row: usize, col: usize, value: f64) {
        if row < self.dim && col < self.dim {
            self.dcharge[row * self.dim + col] += value;
        }
    }
}
