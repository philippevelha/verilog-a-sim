//! The [`StampSink`] trait — how a model deposits its contributions into the system.
//!
//! Rows and columns are **global unknown indices** (the same space returned by
//! [`crate::ModelInstance::unknowns`]). The assembler that implements this trait maps those
//! indices into the MNA matrix/RHS, applying the ground/reference reduction. Models never
//! see the matrix directly — they only emit `(row, value)` and `(row, col, value)` triples.

/// A sink for the channels a model produces during `load`.
///
/// Implementors accumulate (sum) the values; a model may stamp the same `(row, col)` more
/// than once. The reference node (ground) is handled by the assembler, not the model.
///
/// The four matrix channels are required. The two below them — [`Self::excitation`] and
/// [`Self::bound_step`] — are **defaulted no-ops** (§6 change, 2026-08-06): they exist for
/// `ac_stim` and `bound_step`, which only a compiled Verilog-A model emits, and only one
/// analysis each consumes. Defaulting them keeps every existing assembler compiling untouched,
/// exactly as [`crate::ModelInstance::unknown_kind`] and [`crate::ModelInstance::noise`] did.
pub trait StampSink {
    /// Add `value` to the residual at global row `row` (current flowing **into** node `row`).
    fn residual(&mut self, row: usize, value: f64);

    /// Add `value` to the Jacobian entry `∂residual[row] / ∂x[col]`.
    fn jacobian(&mut self, row: usize, col: usize, value: f64);

    /// Add `value` to the charge `Q` at global row `row` (transient only).
    fn charge(&mut self, row: usize, value: f64);

    /// Add `value` to the charge Jacobian `∂Q[row] / ∂x[col]` (transient only).
    fn dcharge(&mut self, row: usize, col: usize, value: f64);

    /// Add the complex constant `re + j·im` to the **small-signal residual** at global row
    /// `row` — Verilog-A's `ac_stim` (LRM §4.5.2). AC analysis only; never called otherwise.
    ///
    /// The sign convention is [`Self::residual`]'s, deliberately: a model writing
    /// `I(p,n) <+ ac_stim(mag, phase)` emits `+A` at `p` and `−A` at `n`, the same way it would
    /// stamp any other flow contribution, and does not have to know which side of the equals
    /// sign its term will end up on. Since the small-signal system is `(G + jω·C)·X = B`, an
    /// assembler moves this to the right-hand side and therefore **negates it**: `B[row] −= A`.
    /// That is the same relationship a [`crate::reference::VSource`] already has between its
    /// `residual(b, vp − vn − value)` constraint row and the `+value` its AC excitation carries.
    ///
    /// A stimulus is a constant, not a function of `x`, so it has no Jacobian entry — which is
    /// precisely why it needs a channel of its own instead of riding on [`Self::residual`],
    /// whose real value would be folded into `G` and double-counted.
    ///
    /// Default: no excitation.
    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        let _ = (row, re, im);
    }

    /// Request that the **next** transient timestep be no longer than `dt` seconds —
    /// Verilog-A's `bound_step` (LRM §9.17.2). Transient only; never called otherwise.
    ///
    /// A *hint*, and only ever downward: an implementor takes the minimum of every request it
    /// receives (and of its own configured ceiling), so one model can tighten the step but none
    /// can loosen another's bound or force a step larger than the integrator already chose.
    /// A non-positive or non-finite `dt` is meaningless as a bound and is ignored rather than
    /// stalling the run — the LRM gives it no interpretation, and honoring it literally would
    /// wedge the timestep controller against its own floor. A positive but absurdly small `dt`
    /// has the same practical effect, so an integrator is expected to clamp the bound at its
    /// own configured minimum timestep (`va_transient::TranConfig::tstep_min`) rather than take
    /// 10¹² steps: a bound is a *request*, and one that cannot be met is met as closely as the
    /// configured floor allows.
    ///
    /// Because this is emitted from the analog block, it may sit inside an `if`: whether a bound
    /// applies at all can depend on the operating point, and a request made during one Newton
    /// iteration is not binding — an assembler should read the bound from the evaluation at the
    /// **accepted** point, not from a rejected candidate.
    ///
    /// Default: no bound.
    fn bound_step(&mut self, dt: f64) {
        let _ = dt;
    }
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
    /// Small-signal excitation as `(re, im)` per row, length `dim` — see
    /// [`StampSink::excitation`] for the sign convention (residual-side, not RHS-side).
    pub excitation: Vec<(f64, f64)>,
    /// The tightest [`StampSink::bound_step`] request received, or `None` if no model asked
    /// for one. Non-positive and non-finite requests are discarded on the way in.
    pub bound_step: Option<f64>,
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
            excitation: vec![(0.0, 0.0); dim],
            bound_step: None,
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

    fn excitation(&mut self, row: usize, re: f64, im: f64) {
        if row < self.dim {
            self.excitation[row].0 += re;
            self.excitation[row].1 += im;
        }
    }

    fn bound_step(&mut self, dt: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.bound_step = Some(self.bound_step.map_or(dt, |cur| cur.min(dt)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Excitation accumulates like every other channel: two models exciting the same row sum,
    /// and a row belonging to the reference node is dropped rather than panicking.
    #[test]
    fn excitation_accumulates_per_row() {
        let mut s = DenseStamp::new(2);
        s.excitation(0, 1.0, 0.0);
        s.excitation(0, 0.5, -2.0);
        s.excitation(1, -1.0, 0.0);
        s.excitation(99, 1e9, 1e9); // ground sentinel — ignored, not a panic
        assert_eq!(s.excitation[0], (1.5, -2.0));
        assert_eq!(s.excitation[1], (-1.0, 0.0));
    }

    /// A bound is a floor-seeking hint: the tightest request wins regardless of arrival order,
    /// and requests that cannot mean anything are discarded rather than wedging the controller
    /// at zero.
    #[test]
    fn bound_step_keeps_the_tightest_meaningful_request() {
        let mut s = DenseStamp::new(1);
        assert_eq!(s.bound_step, None);

        s.bound_step(1e-6);
        assert_eq!(s.bound_step, Some(1e-6));
        // A looser later request must not relax the bound.
        s.bound_step(1e-3);
        assert_eq!(s.bound_step, Some(1e-6));
        // A tighter one must take it.
        s.bound_step(1e-9);
        assert_eq!(s.bound_step, Some(1e-9));

        // Meaningless bounds are ignored — a zero bound would stall the run outright.
        let mut z = DenseStamp::new(1);
        z.bound_step(0.0);
        z.bound_step(-1.0);
        z.bound_step(f64::NAN);
        z.bound_step(f64::INFINITY);
        assert_eq!(z.bound_step, None);
    }
}
