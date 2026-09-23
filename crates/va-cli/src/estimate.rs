//! Pre-flight sizing: what a `sim` run is about to solve, and roughly what that costs.
//!
//! Printed before the solve starts, because the two things a user cannot discover afterwards
//! are whether the run is the size they thought it was and whether it is worth waiting for.
//! The size half is exact — device count, unknown count, and the point count the deck's own
//! card implies. The cost half is a **rough bracket**, and says so: it is scaled from
//! `cargo run --release -p xtask -- bench-scale` on one machine (§ [`CALIBRATION_MACHINE`]),
//! and this simulator's cost per point also depends on how expensive the models are to
//! evaluate, which no table of matrix sizes can know.
//!
//! **Why a bracket and not a number.** Inside the measured range the bracket is the two
//! neighbouring measured rows, which is a statement of fact: the cost at 300 unknowns lies
//! between the cost measured at 202 and the cost measured at 402. Beyond the last measured
//! row it is the two exponents dense LU is bounded by in this size range — the factorization
//! is O(dim³) and assembly O(dim²). Measured, the transient column grows 4.29x from 202 to 402
//! unknowns and 3.67x from 402 to 802 (exponents 2.10 and 1.88), so exponent 2 is already a
//! slightly pessimistic low end at these sizes and exponent 3 a genuine upper bound as the
//! factorization takes over. A single extrapolated number would be more precise than the data.
//!
//! **On the sparse path** (from `SPARSE_THRESHOLD` unknowns, or `--solver sparse`) the cost
//! depends on the circuit's connectivity, not only on its size, so the bracket's two ends come
//! from two circuits measured at every size: an RC ladder (no fill-in, sparse LU's best case) for
//! the low end and a square RC mesh (a 2-D grid, which fills in) for the high end
//! ([`SparseCalibration`]). Beyond the last measured row (6 402 unknowns) the exponents are 1 and
//! 2; the measured growth per doubling at the top of the two tables is between exponent 0.9 and
//! 1.6. A circuit denser than a 2-D grid can exceed the high end.

use crate::Analysis;
use va_core::sparse::{Solver, SPARSE_THRESHOLD};

/// The pre-flight line naming the linear solver a run uses, why it was chosen, and what it
/// covers.
///
/// Since Step 4 of `docs/proposals/sparse-solve.md` every analysis runs wholly on the path
/// chosen here, and since Step 5 the cost estimate above this line is calibrated on the same
/// path ([`Sizing::lines_with`]).
#[must_use]
pub fn solver_line(solver: Solver, unknowns: usize) -> String {
    let sparse = solver.uses_sparse(unknowns);
    let why = match solver {
        Solver::Auto if sparse => format!("{unknowns} >= {SPARSE_THRESHOLD} unknowns"),
        Solver::Auto => format!("{unknowns} < {SPARSE_THRESHOLD} unknowns"),
        Solver::Dense => "--solver dense".to_string(),
        Solver::Sparse => "--solver sparse".to_string(),
    };
    if !sparse {
        return format!("[va-cli] linear solve: dense LU ({why})");
    }
    format!("[va-cli] linear solve: sparse LU ({why}), for the whole run")
}

/// The machine every figure in [`TRAN_MS_PER_POINT`] and its siblings was measured on. Named
/// in the output because the absolute numbers belong to it; `cargo run --release -p xtask --
/// bench-scale` re-measures them on the machine at hand.
pub const CALIBRATION_MACHINE: &str = "i7-1185G7";

/// Transient cost per accepted timepoint, in milliseconds, against matrix dimension.
///
/// Measured 2026-09-17 with `bench-scale` (release profile, single-threaded) on an RC ladder
/// built from `va-abi`'s reference primitives, so the compiler is not in the timing. Two runs
/// were taken and each entry is the **slower** of the two: run-to-run spread is ~30% at the
/// middle sizes, and an estimate that overruns slightly is worth more than one that undershoots.
/// Ascending in `dim`, which [`bracket`] relies on.
///
/// The jump between 22 and 52 unknowns is far larger than any factorization cost can explain
/// and reproduces across runs. The accepted-point count is identical at every size, so it is
/// extra work *per* point — rejected steps or Newton iterations on a ladder that has become
/// stiffer — not the solve. It is left in the table rather than smoothed away: an estimate
/// that interpolates between honest rows is worth more than one fitted to a curve the data
/// does not follow.
const TRAN_MS_PER_POINT: &[(usize, f64)] = &[
    (12, 0.01),
    (22, 0.017),
    (52, 0.466),
    (102, 1.39),
    (202, 3.319),
    (402, 14.242),
    (802, 52.335),
];

/// Cost of one DC operating-point solve (the whole Newton loop), in milliseconds. A `.dc`
/// sweep pays this per swept value; a bare `.op` pays it once.
const DC_MS_PER_SOLVE: &[(usize, f64)] = &[
    (12, 0.02),
    (22, 0.03),
    (52, 0.38),
    (102, 0.85),
    (202, 2.24),
    (402, 8.08),
    (802, 63.46),
];

/// AC cost per frequency point: one complex factorization, no Newton loop.
const AC_MS_PER_POINT: &[(usize, f64)] = &[
    (12, 0.006),
    (22, 0.408),
    (52, 0.544),
    (102, 1.463),
    (202, 4.735),
    (402, 21.494),
    (802, 78.766),
];

/// Noise cost per frequency point: the AC solve plus the adjoint solve behind the
/// input-referred and per-device spectra.
const NOISE_MS_PER_POINT: &[(usize, f64)] = &[
    (12, 0.006),
    (22, 0.361),
    (52, 0.474),
    (102, 1.464),
    (202, 6.093),
    (402, 22.297),
    (802, 120.293),
];

/// A sparse-path cost table: the same analysis measured on two circuits at every size, because
/// sparse LU's cost depends on connectivity as well as `dim`. The ladder (tridiagonal, no
/// fill-in) gives the bracket's low end and the mesh (a square 2-D grid, which fills in) its
/// high end.
///
/// Measured 2026-09-23 with `bench-scale --solver sparse --topology ladder|mesh` (release
/// profile, single-threaded, on [`CALIBRATION_MACHINE`]), two runs each, the **slower** of the
/// two kept, as for the dense tables. Run-to-run spread was up to 1.9x at the smallest sizes and
/// under 1.4x from 400 unknowns. The mesh's `dim` is `k² + 2`, hence its uneven sizes.
/// Ascending in `dim`, which [`bracket`] relies on.
pub struct SparseCalibration {
    /// The low-end circuit: an RC ladder.
    pub ladder: &'static [(usize, f64)],
    /// The high-end circuit: a square RC mesh.
    pub mesh: &'static [(usize, f64)],
}

/// Sparse transient cost per accepted timepoint, in milliseconds (§ [`SparseCalibration`]).
const SPARSE_TRAN_MS_PER_POINT: SparseCalibration = SparseCalibration {
    ladder: &[
        (12, 0.019),
        (22, 0.023),
        (52, 0.048),
        (102, 0.068),
        (202, 0.136),
        (402, 0.234),
        (802, 0.602),
        (1602, 0.928),
        (3202, 2.138),
        (6402, 4.173),
    ],
    mesh: &[
        (11, 0.018),
        (18, 0.033),
        (51, 0.072),
        (102, 0.131),
        (198, 0.358),
        (402, 1.326),
        (786, 3.221),
        (1602, 6.361),
        (3251, 12.613),
        (6402, 36.798),
    ],
};

/// Sparse cost of one DC operating-point solve, in milliseconds (§ [`SparseCalibration`]).
const SPARSE_DC_MS_PER_SOLVE: SparseCalibration = SparseCalibration {
    ladder: &[
        (12, 0.11),
        (22, 0.11),
        (52, 0.20),
        (102, 0.27),
        (202, 0.84),
        (402, 0.95),
        (802, 1.77),
        (1602, 3.47),
        (3202, 8.98),
        (6402, 16.17),
    ],
    mesh: &[
        (11, 0.07),
        (18, 0.19),
        (51, 0.42),
        (102, 0.60),
        (198, 1.65),
        (402, 2.88),
        (786, 7.45),
        (1602, 21.16),
        (3251, 38.46),
        (6402, 92.61),
    ],
};

/// Sparse AC cost per frequency point, in milliseconds (§ [`SparseCalibration`]).
const SPARSE_AC_MS_PER_POINT: SparseCalibration = SparseCalibration {
    ladder: &[
        (12, 0.019),
        (22, 0.022),
        (52, 0.034),
        (102, 0.056),
        (202, 0.131),
        (402, 0.305),
        (802, 0.851),
        (1602, 1.391),
        (3202, 2.328),
        (6402, 4.137),
    ],
    mesh: &[
        (11, 0.024),
        (18, 0.042),
        (51, 0.114),
        (102, 0.369),
        (198, 1.413),
        (402, 3.220),
        (786, 9.462),
        (1602, 32.598),
        (3251, 49.874),
        (6402, 142.561),
    ],
};

/// Sparse noise cost per frequency point, in milliseconds (§ [`SparseCalibration`]).
const SPARSE_NOISE_MS_PER_POINT: SparseCalibration = SparseCalibration {
    ladder: &[
        (12, 0.018),
        (22, 0.021),
        (52, 0.035),
        (102, 0.061),
        (202, 0.104),
        (402, 0.233),
        (802, 0.539),
        (1602, 1.100),
        (3202, 2.431),
        (6402, 5.144),
    ],
    mesh: &[
        (11, 0.029),
        (18, 0.042),
        (51, 0.105),
        (102, 0.369),
        (198, 1.283),
        (402, 2.939),
        (786, 9.389),
        (1602, 24.645),
        (3251, 57.380),
        (6402, 142.783),
    ],
};

/// Scaling exponents for extrapolating past a dense table: assembly O(dim²) and factorization
/// O(dim³) (module docs).
const DENSE_EXPONENTS: (f64, f64) = (2.0, 3.0);

/// Scaling exponents for extrapolating past a sparse table: linear (no fill-in, the ladder) to
/// quadratic. The measured growth at the top of the tables is exponent 0.9–1.6.
const SPARSE_EXPONENTS: (f64, f64) = (1.0, 2.0);

/// The sparse bracket at `dim`: the ladder's low end and the mesh's high end.
fn sparse_bracket(dim: usize, cal: &SparseCalibration) -> (f64, f64) {
    let lo = bracket_with(dim, cal.ladder, SPARSE_EXPONENTS).0;
    let hi = bracket_with(dim, cal.mesh, SPARSE_EXPONENTS).1;
    (lo.min(hi), hi.max(lo))
}

/// Extra cost per *evaluated* point, in milliseconds, for one compiled Verilog-A instance —
/// the term [`TRAN_MS_PER_POINT`]'s ladder cannot see, because that ladder is built from
/// `va-abi` reference primitives whose evaluation the table already contains.
///
/// A compiled instance is evaluated once per Newton iteration, through automatically
/// differentiated code whose cost is the model's own expression size; neither the iteration
/// count nor the expression size is knowable from a matrix dimension, which is why this is a
/// range and not a number. Calibrated against this repository's decks (wall time minus the
/// ~77 ms of process start-up and model compilation, divided by realised points): a rectifier's
/// one compiled diode 0.162 ms/point, two actuator instances 0.059 each, four microring-example
/// instances 0.028 each, eight traffic sections 0.037 each.
const COMPILED_EVAL_MS: (f64, f64) = (0.006, 0.2);

/// The same term for a *non-linear reference primitive* (`diode`, `bjt`): cheaper than a
/// compiled model — the code is hand-written, not generated — but still several Newton
/// iterations per point, unlike the linear `resistor`/`capacitor` the calibration ladder is
/// made of. Calibrated on `circuits/ring_osc.net`: three BJTs, 0.086 ms/point for the whole
/// circuit.
const NONLINEAR_PRIMITIVE_EVAL_MS: (f64, f64) = (0.002, 0.04);

/// How many timepoints a transient run really takes, as a multiple of the `tstop/tstep` the
/// deck asks for. The step controller adds points where the solution moves fast, so the card's
/// own ratio is a floor; measured across this repository's validated decks (rectifier 1.44,
/// ring oscillator 1.12, microring 1.01, motorway 1.00, actuator 1.02, laplace step 1.08) the
/// realised count runs between 1.0x and 1.5x nominal.
const ADAPTIVE_SPREAD: (f64, f64) = (1.0, 1.5);

/// How many points an analysis will evaluate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Points {
    /// The count is known before the solve: one operating point, a `.dc` sweep's values, or a
    /// frequency grid.
    Exact(usize),
    /// A transient run, whose real count the step controller decides. Carries the deck's own
    /// `tstop/tstep` nominal, which is a floor (§ [`ADAPTIVE_SPREAD`]).
    Adaptive(usize),
}

/// What a run is about to solve. Built by [`crate::sizing`] once every instance has claimed
/// its unknowns, which is the first moment `unknowns` is knowable.
#[derive(Clone, Debug)]
pub struct Sizing {
    /// Which analysis the invocation selected.
    pub analysis: Analysis,
    /// Devices instantiated from the deck (every `R`/`C`/`V`/… line and every Verilog-A
    /// instance).
    pub devices: usize,
    /// Of those, how many are instances of a compiled Verilog-A module — the expensive ones to
    /// evaluate (§ [`COMPILED_EVAL_MS`]).
    pub compiled: usize,
    /// Of those, how many are non-linear reference primitives (`diode`, `bjt`).
    pub nonlinear_primitives: usize,
    /// Non-ground nets in the deck.
    pub nodes: usize,
    /// Rows in the matrix: the nets above plus every auxiliary row — a branch current per
    /// voltage source and inductor, a compiled model's internal nodes, an `idt` accumulator, a
    /// `laplace_*` filter's state per denominator degree.
    pub unknowns: usize,
    /// Points the analysis will evaluate.
    pub points: Points,
}

impl Sizing {
    /// Auxiliary rows: everything in the matrix that is not one of the deck's own nets.
    #[must_use]
    pub fn auxiliary(&self) -> usize {
        self.unknowns.saturating_sub(self.nodes)
    }

    /// Peak dense-matrix memory, in bytes: the assembled Jacobian, the copy `faer` factorizes,
    /// and the factors themselves — three `dim x dim` matrices, doubled for the complex matrix
    /// an AC or noise sweep solves. Excludes everything that does not scale with `dim²`.
    #[must_use]
    pub fn matrix_bytes(&self) -> u64 {
        let width = match self.analysis {
            Analysis::Ac | Analysis::Noise => 16u64, // Complex<f64>
            Analysis::Dc | Analysis::Transient => 8u64,
        };
        let dim = self.unknowns as u64;
        3 * width * dim * dim
    }

    /// Rough wall-clock bracket for the solve under [`Solver::Auto`], in seconds. See
    /// [`Sizing::seconds_with`].
    #[must_use]
    pub fn seconds(&self) -> (f64, f64) {
        self.seconds_with(Solver::Auto)
    }

    /// Rough wall-clock bracket for the solve on the path `solver` picks for this size, in
    /// seconds. See the module docs for what the two ends mean; the figure covers the analysis
    /// itself, not process start-up or model compilation (tens of milliseconds, and independent
    /// of circuit size).
    #[must_use]
    pub fn seconds_with(&self, solver: Solver) -> (f64, f64) {
        let sparse = solver.uses_sparse(self.unknowns);
        let per_point = |dense: &[(usize, f64)], sparse_cal: &SparseCalibration| {
            if sparse {
                sparse_bracket(self.unknowns, sparse_cal)
            } else {
                bracket(self.unknowns, dense)
            }
        };
        let (solve_lo, solve_hi) = match self.analysis {
            Analysis::Transient => per_point(TRAN_MS_PER_POINT, &SPARSE_TRAN_MS_PER_POINT),
            Analysis::Ac => per_point(AC_MS_PER_POINT, &SPARSE_AC_MS_PER_POINT),
            Analysis::Noise => per_point(NOISE_MS_PER_POINT, &SPARSE_NOISE_MS_PER_POINT),
            Analysis::Dc => per_point(DC_MS_PER_SOLVE, &SPARSE_DC_MS_PER_SOLVE),
        };
        // Model evaluation, which the calibration circuits cannot speak for: they are built from
        // linear reference primitives, so a deck's compiled instances and non-linear primitives
        // add a cost per point on top of the table's own. Below ~100 unknowns this term is the
        // whole estimate; on the dense path above ~400 the factorization swamps it.
        let eval = |per: (f64, f64), n: usize| (per.0 * n as f64, per.1 * n as f64);
        let (c_lo, c_hi) = eval(COMPILED_EVAL_MS, self.compiled);
        let (p_lo, p_hi) = eval(NONLINEAR_PRIMITIVE_EVAL_MS, self.nonlinear_primitives);
        let (lo_ms, hi_ms) = (solve_lo + c_lo + p_lo, solve_hi + c_hi + p_hi);
        let (lo_pts, hi_pts) = match self.points {
            Points::Exact(n) => (n as f64, n as f64),
            Points::Adaptive(n) => (n as f64 * ADAPTIVE_SPREAD.0, n as f64 * ADAPTIVE_SPREAD.1),
        };
        // An AC or noise sweep solves one operating point first; at large `dim` that single
        // Newton loop is not negligible next to a short sweep.
        let op = match self.analysis {
            Analysis::Ac | Analysis::Noise => per_point(DC_MS_PER_SOLVE, &SPARSE_DC_MS_PER_SOLVE),
            Analysis::Dc | Analysis::Transient => (0.0, 0.0),
        };
        ((lo_ms * lo_pts + op.0) / 1e3, (hi_ms * hi_pts + op.1) / 1e3)
    }

    /// The pre-flight lines under [`Solver::Auto`]. See [`Sizing::lines_with`].
    #[must_use]
    pub fn lines(&self) -> [String; 2] {
        self.lines_with(Solver::Auto)
    }

    /// The pre-flight lines, ready to print, for a run on the path `solver` picks. Two lines:
    /// what is being solved, and what it is expected to cost.
    ///
    /// On the sparse path no matrix memory is quoted: it follows the nonzeros and their fill-in,
    /// which the unknown count does not determine. Measured on a PSP103 inverter chain, the whole
    /// process peaked at 30 MB at 2 646 unknowns (247 MB dense), so it is not the constraint.
    #[must_use]
    pub fn lines_with(&self, solver: Solver) -> [String; 2] {
        let aux = self.auxiliary();
        let points = match self.points {
            Points::Exact(1) => "1 point".to_string(),
            Points::Exact(n) => format!("{n} points"),
            Points::Adaptive(n) => format!("~{n} points (adaptive, {n} is the card's floor)"),
        };
        let compiled = if self.compiled > 0 {
            format!(" ({} compiled)", self.compiled)
        } else {
            String::new()
        };
        let rows = format!("{} net(s) + {aux} auxiliary row(s)", self.nodes);
        let what = format!(
            "[va-cli] circuit: {} device(s){compiled}, {} unknown(s) ({rows}), {points}",
            self.devices, self.unknowns,
        );
        let (lo, hi) = self.seconds_with(solver);
        let cost = if solver.uses_sparse(self.unknowns) {
            format!(
                "[va-cli] estimate: {} of solve, sparse matrix — rough, sparse LU scaled from \
                 bench-scale (RC ladder to RC mesh) on an {CALIBRATION_MACHINE}",
                time_range(lo, hi),
            )
        } else {
            format!(
                "[va-cli] estimate: {} of solve, {} of matrix — rough, dense LU scaled from \
                 bench-scale on an {CALIBRATION_MACHINE}",
                time_range(lo, hi),
                bytes(self.matrix_bytes()),
            )
        };
        [what, cost]
    }
}

/// The dense cost-per-point bracket at `dim`, from a measured table (ascending in `dim`).
///
/// Between two measured rows the bracket is those two rows: the table is monotone in `dim`, so
/// the truth lies between its neighbours and no curve fit is needed. Outside the table it is
/// the exponent-2 and exponent-3 scalings of the nearest row — below the first row those
/// shrink the number (so exponent 3 is the low end), above the last row they grow it (so
/// exponent 3 is the high end).
#[must_use]
pub fn bracket(dim: usize, table: &[(usize, f64)]) -> (f64, f64) {
    bracket_with(dim, table, DENSE_EXPONENTS)
}

/// [`bracket`] with the extrapolation exponents `(low, high)` given, `low <= high`.
#[must_use]
pub fn bracket_with(dim: usize, table: &[(usize, f64)], exponents: (f64, f64)) -> (f64, f64) {
    debug_assert!(!table.is_empty());
    let (e_lo, e_hi) = exponents;
    let (first_n, first_t) = table[0];
    let (last_n, last_t) = table[table.len() - 1];
    if dim <= first_n {
        let r = dim as f64 / first_n as f64;
        return (first_t * r.powf(e_hi), first_t * r.powf(e_lo));
    }
    if dim >= last_n {
        let r = dim as f64 / last_n as f64;
        return (last_t * r.powf(e_lo), last_t * r.powf(e_hi));
    }
    let hi = table
        .iter()
        .position(|&(n, _)| n >= dim)
        .expect("dim is below the last row, so some row is >= it");
    (table[hi - 1].1, table[hi].1)
}

/// A seconds range as text, in whichever unit keeps the upper end legible.
///
/// The unit comes from the upper end so both numbers share one, and the decimal count from the
/// *lower* end so a wide bracket does not print its floor as `0.0` — an estimate whose low end
/// reads as zero says nothing.
fn time_range(lo: f64, hi: f64) -> String {
    let (scale, unit) = if hi < 1e-3 {
        (1e-6, "us")
    } else if hi < 1.0 {
        (1e-3, "ms")
    } else if hi < 120.0 {
        (1.0, "s")
    } else if hi < 7200.0 {
        (60.0, "min")
    } else {
        (3600.0, "h")
    };
    let (l, h) = (lo / scale, hi / scale);
    let digits = if l < 0.1 {
        2
    } else if l < 10.0 {
        1
    } else {
        0
    };
    format!("{l:.digits$}-{h:.digits$} {unit}")
}

/// `n` bytes in whichever unit keeps the number under four digits.
fn bytes(n: u64) -> String {
    let n = n as f64;
    if n < 1e3 {
        format!("{n:.0} B")
    } else if n < 1e6 {
        format!("{:.1} kB", n / 1e3)
    } else if n < 1e9 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.2} GB", n / 1e9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizing(analysis: Analysis, unknowns: usize, points: Points) -> Sizing {
        Sizing {
            analysis,
            devices: 3,
            compiled: 0,
            nonlinear_primitives: 0,
            nodes: unknowns.saturating_sub(1),
            unknowns,
            points,
        }
    }

    /// Inside the measured range the bracket is the neighbouring measured rows themselves —
    /// the claim is "between what was measured at 202 and what was measured at 402", which
    /// needs no exponent at all.
    #[test]
    fn bracket_between_measured_rows_is_those_rows() {
        let (lo, hi) = bracket(300, TRAN_MS_PER_POINT);
        assert_eq!((lo, hi), (3.319, 14.242));
    }

    /// An exactly-measured size still brackets: 202 is the upper neighbour of the 102 row.
    #[test]
    fn bracket_at_a_measured_size_uses_that_row_as_the_upper_end() {
        let (lo, hi) = bracket(202, TRAN_MS_PER_POINT);
        assert_eq!((lo, hi), (1.39, 3.319));
    }

    /// Beyond the table the exponents take over: doubling `dim` multiplies the low end by 4
    /// and the high end by 8. The measured 3.67x per doubling at the top of the table is just
    /// *below* the low end, which is the honest state of the data — at 800 unknowns the
    /// factorization has not yet made the cost cubic — so extrapolation errs on the slow side.
    #[test]
    fn bracket_beyond_the_table_is_the_exponent_two_and_three_scalings() {
        let (lo, hi) = bracket(1604, TRAN_MS_PER_POINT);
        assert!((lo - 52.335 * 4.0).abs() < 1e-6, "exponent 2 at 2x: {lo}");
        assert!((hi - 52.335 * 8.0).abs() < 1e-6, "exponent 3 at 2x: {hi}");
        let measured_ratio = 52.335 / 14.242;
        assert!(
            measured_ratio < 4.0,
            "the measured 400->800 ratio {measured_ratio} is below quadratic growth; if this              ever exceeds 4 the extrapolation stopped being conservative"
        );
    }

    /// Below the smallest measured circuit the scalings shrink rather than grow, so the low
    /// end is the cubic one and the whole bracket stays under the first measured row.
    #[test]
    fn bracket_below_the_table_shrinks_and_stays_ordered() {
        let (lo, hi) = bracket(6, TRAN_MS_PER_POINT);
        assert!(lo < hi, "{lo} < {hi}");
        assert!(hi <= 0.01, "below the first measured row: {hi}");
    }

    /// The transient bracket multiplies by points, and an adaptive run widens it by the
    /// measured 1.0x–1.5x overshoot of the card's nominal count.
    #[test]
    fn a_transient_estimate_scales_with_points_and_widens_for_adaptive_stepping() {
        let s = sizing(Analysis::Transient, 300, Points::Adaptive(1000));
        let (lo, hi) = s.seconds();
        assert!((lo - 3.319 * 1000.0 / 1e3).abs() < 1e-9, "{lo}");
        assert!((hi - 14.242 * 1500.0 / 1e3).abs() < 1e-9, "{hi}");
    }

    /// An AC sweep pays one operating-point solve before the grid; a transient does not.
    #[test]
    fn an_ac_estimate_includes_the_operating_point_solve_a_transient_does_not() {
        let ac = sizing(Analysis::Ac, 300, Points::Exact(10)).seconds();
        let grid = (
            AC_MS_PER_POINT[4].1 * 10.0 / 1e3,
            AC_MS_PER_POINT[5].1 * 10.0 / 1e3,
        );
        assert!(ac.0 > grid.0, "AC low end {} > grid alone {}", ac.0, grid.0);
        assert!(
            ac.1 > grid.1,
            "AC high end {} > grid alone {}",
            ac.1,
            grid.1
        );
    }

    /// Memory is three dense matrices, and an AC run's are complex — twice the width.
    #[test]
    fn matrix_memory_is_three_dense_copies_and_doubles_for_a_complex_sweep() {
        let dc = sizing(Analysis::Dc, 100, Points::Exact(1)).matrix_bytes();
        let ac = sizing(Analysis::Ac, 100, Points::Exact(1)).matrix_bytes();
        assert_eq!(dc, 3 * 8 * 100 * 100);
        assert_eq!(ac, 2 * dc);
    }

    /// The reported unknown split adds up, and the lines name both halves.
    #[test]
    fn the_preflight_lines_state_the_size_and_say_the_cost_is_rough() {
        let s = Sizing {
            analysis: Analysis::Transient,
            devices: 4,
            compiled: 1,
            nonlinear_primitives: 0,
            nodes: 3,
            unknowns: 5,
            points: Points::Adaptive(500),
        };
        assert_eq!(s.auxiliary(), 2);
        let [what, cost] = s.lines();
        assert!(what.contains("4 device(s)"), "{what}");
        assert!(what.contains("5 unknown(s)"), "{what}");
        assert!(what.contains("3 net(s) + 2 auxiliary row(s)"), "{what}");
        assert!(what.contains("~500 points"), "{what}");
        assert!(cost.contains("rough"), "{cost}");
        assert!(cost.contains(CALIBRATION_MACHINE), "{cost}");
    }

    /// A one-point `.op` is quoted as one solve, not as a sweep.
    #[test]
    fn an_operating_point_is_one_solve() {
        let s = sizing(Analysis::Dc, 300, Points::Exact(1));
        let (lo, hi) = s.seconds();
        assert!((lo - DC_MS_PER_SOLVE[4].1 / 1e3).abs() < 1e-9, "{lo}");
        assert!((hi - DC_MS_PER_SOLVE[5].1 / 1e3).abs() < 1e-9, "{hi}");
        assert!(s.lines()[0].contains("1 point"), "{}", s.lines()[0]);
    }

    /// A wide bracket must not print its low end as `0.0`: the decimals come from the low end
    /// even though the unit comes from the high one.
    #[test]
    fn a_wide_range_keeps_significant_digits_at_its_low_end() {
        let text = time_range(0.003, 0.15);
        assert!(text.starts_with("3.0-"), "{text}");
        // And a floor three orders below its ceiling still shows two digits rather than none.
        let tiny = time_range(0.00004, 0.15);
        assert!(tiny.starts_with("0.04-"), "{tiny}");
        assert!(text.ends_with("ms"), "{text}");
        assert!(
            !text.starts_with("0.0"),
            "a floor printed as zero says nothing: {text}"
        );
    }

    /// Compiled instances, not matrix size, are what a small circuit's cost is made of: the
    /// calibration ladder is linear primitives, so its table cannot see them.
    #[test]
    fn model_evaluation_dominates_a_small_circuit_and_scales_with_compiled_instances() {
        let bare = sizing(Analysis::Transient, 5, Points::Adaptive(500));
        let with_models = Sizing {
            compiled: 4,
            ..bare.clone()
        };
        let (solve_only_lo, solve_only_hi) = bare.seconds();
        let (lo, hi) = with_models.seconds();
        assert!(lo > solve_only_lo * 10.0, "{lo} vs {solve_only_lo}");
        assert!(hi > solve_only_hi * 10.0, "{hi} vs {solve_only_hi}");
        let one = Sizing {
            compiled: 1,
            ..bare.clone()
        };
        let (one_lo, _) = one.seconds();
        let ratio = (lo - solve_only_lo) / (one_lo - solve_only_lo);
        assert!(
            (ratio - 4.0).abs() < 1e-6,
            "four instances cost 4x one: {ratio}"
        );
    }

    /// The bracket has to contain what the real decks actually took, or it is decoration.
    /// Measured 2026-09-17 (release binary, wall time minus the ~77 ms of start-up and model
    /// compilation, which the estimate deliberately excludes).
    #[test]
    fn the_bracket_contains_every_measured_deck() {
        // (name, unknowns, compiled, non-linear primitives, nominal points, measured seconds)
        let decks = [
            ("rectifier", 3usize, 1usize, 0usize, 502usize, 0.116),
            ("ring_osc", 8, 0, 3, 2001, 0.192),
            ("microring_thermal", 18, 4, 0, 2001, 0.446),
            ("motorway_ramp", 28, 8, 0, 12601, 5.196),
            ("actuator_plant", 5, 2, 0, 1001, 0.120),
        ];
        for (name, unknowns, compiled, nonlinear_primitives, points, measured) in decks {
            let s = Sizing {
                analysis: Analysis::Transient,
                devices: compiled + nonlinear_primitives + 1,
                compiled,
                nonlinear_primitives,
                nodes: unknowns - 1,
                unknowns,
                points: Points::Adaptive(points),
            };
            let (lo, hi) = s.seconds();
            assert!(
                lo <= measured && measured <= hi,
                "{name}: measured {measured}s outside the estimated {lo}..{hi}s"
            );
        }
    }

    /// Every table the estimate reads must be ascending in `dim` and in cost — `bracket`
    /// returns a neighbour pair as an ordered range on that assumption.
    #[test]
    fn every_calibration_table_is_monotone() {
        for (name, table) in [
            ("tran", TRAN_MS_PER_POINT),
            ("dc", DC_MS_PER_SOLVE),
            ("ac", AC_MS_PER_POINT),
            ("noise", NOISE_MS_PER_POINT),
            ("sparse tran ladder", SPARSE_TRAN_MS_PER_POINT.ladder),
            ("sparse tran mesh", SPARSE_TRAN_MS_PER_POINT.mesh),
            ("sparse dc ladder", SPARSE_DC_MS_PER_SOLVE.ladder),
            ("sparse dc mesh", SPARSE_DC_MS_PER_SOLVE.mesh),
            ("sparse ac ladder", SPARSE_AC_MS_PER_POINT.ladder),
            ("sparse ac mesh", SPARSE_AC_MS_PER_POINT.mesh),
            ("sparse noise ladder", SPARSE_NOISE_MS_PER_POINT.ladder),
            ("sparse noise mesh", SPARSE_NOISE_MS_PER_POINT.mesh),
        ] {
            for pair in table.windows(2) {
                assert!(pair[0].0 < pair[1].0, "{name}: dim must ascend");
                assert!(pair[0].1 <= pair[1].1, "{name}: cost must not decrease");
            }
        }
    }

    /// The sparse bracket is ordered at every size, inside and beyond the measured range, and
    /// is the ladder's low end and the mesh's high end where both are measured.
    #[test]
    fn the_sparse_bracket_is_ordered_and_spans_ladder_to_mesh() {
        for cal in [
            &SPARSE_TRAN_MS_PER_POINT,
            &SPARSE_DC_MS_PER_SOLVE,
            &SPARSE_AC_MS_PER_POINT,
            &SPARSE_NOISE_MS_PER_POINT,
        ] {
            for dim in [
                1, 5, 11, 12, 30, 300, 500, 1000, 5000, 6402, 20_000, 100_000,
            ] {
                let (lo, hi) = sparse_bracket(dim, cal);
                assert!(lo > 0.0 && lo <= hi, "dim {dim}: {lo}..{hi}");
            }
        }
        // 1000 unknowns: ladder rows 802/1602, mesh rows 786/1602.
        let (lo, hi) = sparse_bracket(1000, &SPARSE_TRAN_MS_PER_POINT);
        assert_eq!((lo, hi), (0.602, 6.361));
        // Beyond the tables: exponent 1 on the ladder's last row, exponent 2 on the mesh's.
        let (lo, hi) = sparse_bracket(12_804, &SPARSE_TRAN_MS_PER_POINT);
        assert!((lo - 4.173 * 2.0).abs() < 1e-9, "{lo}");
        assert!((hi - 36.798 * 4.0).abs() < 1e-9, "{hi}");
    }

    /// The estimate follows the solver: the same 300-unknown transient is quoted from the dense
    /// table under `Auto` and from the sparse one when sparse is forced, and above the threshold
    /// `Auto` is the sparse quote. The sparse line quotes no dense matrix memory.
    #[test]
    fn the_estimate_is_calibrated_on_the_path_the_run_takes() {
        let s = sizing(Analysis::Transient, 300, Points::Exact(1000));
        assert_eq!(s.seconds(), s.seconds_with(Solver::Dense));
        let (dense_lo, _) = s.seconds_with(Solver::Dense);
        let (_, sparse_hi) = s.seconds_with(Solver::Sparse);
        assert!(
            sparse_hi < dense_lo,
            "sparse {sparse_hi} vs dense {dense_lo}"
        );
        assert!((dense_lo - 3.319).abs() < 1e-9, "{dense_lo}");
        assert!((sparse_hi - 1.326).abs() < 1e-9, "{sparse_hi}");
        let big = sizing(Analysis::Transient, SPARSE_THRESHOLD, Points::Exact(1));
        assert_eq!(big.seconds(), big.seconds_with(Solver::Sparse));
        let [_, dense_cost] = s.lines_with(Solver::Dense);
        assert!(dense_cost.contains("dense LU"), "{dense_cost}");
        assert!(dense_cost.contains("of matrix"), "{dense_cost}");
        let [_, sparse_cost] = s.lines_with(Solver::Sparse);
        assert!(sparse_cost.contains("sparse LU"), "{sparse_cost}");
        assert!(!sparse_cost.contains("of matrix"), "{sparse_cost}");
    }

    /// The sparse bracket has to contain what the sparse runs of `bench-scale` measured on both
    /// calibration circuits, the faster run included: the ladder at the low end, the mesh at the
    /// high end. Values from the faster of the two 2026-09-23 runs (the tables keep the slower).
    #[test]
    fn the_sparse_bracket_contains_both_calibration_circuits() {
        // (dim, faster ladder tran ms/pt, faster mesh tran ms/pt), at sizes both measured.
        for (dim, ladder, mesh) in [
            (102, 0.061, 0.114),
            (402, 0.217, 0.769),
            (1602, 0.893, 5.339),
        ] {
            let (lo, hi) = sparse_bracket(dim, &SPARSE_TRAN_MS_PER_POINT);
            assert!(
                lo <= ladder && mesh <= hi,
                "dim {dim}: {lo}..{hi} vs {ladder}, {mesh}"
            );
        }
    }

    #[test]
    fn solver_line_names_the_solver_the_reason_and_the_scope() {
        let dense = solver_line(Solver::Auto, 499);
        assert!(dense.contains("dense LU (499 < 500 unknowns)"), "{dense}");
        let auto = solver_line(Solver::Auto, 500);
        assert!(auto.contains("sparse LU (500 >= 500 unknowns)"), "{auto}");
        assert!(auto.contains("whole run"), "{auto}");
        let forced = solver_line(Solver::Sparse, 12);
        assert!(forced.contains("--solver sparse"), "{forced}");
        assert!(forced.contains("whole run"), "{forced}");
        let ac = solver_line(Solver::Sparse, 12);
        assert!(ac.contains("whole run"), "{ac}");
        let kept = solver_line(Solver::Dense, 10_000);
        assert!(kept.contains("dense LU (--solver dense)"), "{kept}");
    }
}
