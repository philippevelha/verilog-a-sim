//! Event counters for the model-evaluation hot path, read by `va-cli sim --logfull`.
//!
//! Three things a compiled instance's `load` does that cost time without computing physics,
//! counted so their price can be put next to the time they take:
//!
//! - [`Counters::ctx_maps_built`] — lookup maps built from the module's lowering at the start of
//!   every evaluation (`GeneratedModel::ctx`: branch-current slots, `idt` slots, stateful-call
//!   slots — three per call). They are the same on every call of a given instance; counted here
//!   because each one is a hash-map construction on the hot path.
//! - [`Counters::probe_allocs`] — gradient vectors allocated by a potential or flow read
//!   (`V(...)`, `I(...)`, an `idt` value), one per read, each sized by the instance's own
//!   unknowns (never the circuit's).
//! - [`Counters::ctx_map_lookups`] — hash lookups into those maps and into the per-call
//!   bookkeeping sets (`mixed_branch_potential_used`, `flow_current_totals`).
//! - [`Counters::grad_allocs`] — dense gradient vectors the dual-number arithmetic allocates:
//!   every operation on a value that depends on an unknown builds a new one (`Grad::map`,
//!   `zip_with`, `Dual::variable`). Probe reads are not in it; they are
//!   [`Counters::probe_allocs`]. A value depending on no unknown carries a `Grad::Zero` and
//!   allocates nothing, so is never counted.
//! - [`Counters::grad_clones`] — copies of a dense gradient (reading a local variable copies its
//!   value). A copy shares the buffer, so it is **not** an allocation and not in `grad_allocs`;
//!   until 1.14.0+1 it was, and was ~47% of them.
//! - [`Counters::exprs_visited`], [`Counters::exprs_hoistable`] — expression nodes the tree
//!   walk visits, and how many of them sit inside an invariant subtree
//!   (`lower::Invariance::hoistable`): Stage 1 of `docs/proposals/evaluator-tree-walk.md`.
//!   **Only with `--features walk-stats`**; zero otherwise, because counting puts a check on
//!   every node of every evaluation.
//! - [`Counters::grad_in_place`] — gradient results written into an operand's own buffer
//!   because the operation owned it and nothing else shared it (1.14.0+2): each one an
//!   allocation `grad_allocs` no longer shows.
//!
//! **Off by default, and nearly free while off**: each site checks one relaxed atomic flag
//! before counting. On, each count is a relaxed atomic add — a few nanoseconds against
//! allocations and hash lookups that cost more — so a run with counting on is slightly slower
//! than one without, and its timings should be read with that in mind.
//!
//! Process-wide totals. Counts from evaluations on several threads would add up correctly, but
//! nothing in the simulator evaluates in parallel today.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static CTX_MAPS_BUILT: AtomicU64 = AtomicU64::new(0);
static PROBE_ALLOCS: AtomicU64 = AtomicU64::new(0);
static CTX_MAP_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static GRAD_ALLOCS: AtomicU64 = AtomicU64::new(0);
static GRAD_CLONES: AtomicU64 = AtomicU64::new(0);
static GRAD_IN_PLACE: AtomicU64 = AtomicU64::new(0);
static EXPRS_VISITED: AtomicU64 = AtomicU64::new(0);
static EXPRS_HOISTABLE: AtomicU64 = AtomicU64::new(0);

/// A snapshot of the counters since the process started (or since [`reset`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Lookup maps built by `GeneratedModel::ctx`, three per evaluation.
    pub ctx_maps_built: u64,
    /// Gradient vectors allocated by potential/flow/`idt` reads.
    pub probe_allocs: u64,
    /// Hash lookups into the evaluation context's maps and sets.
    pub ctx_map_lookups: u64,
    /// Dense gradient vectors allocated by dual-number arithmetic, probe reads excluded.
    pub grad_allocs: u64,
    /// Copies of a dense gradient: shared buffers, not allocations.
    pub grad_clones: u64,
    /// Gradient results written into an operand's own buffer instead of a new one (an
    /// operation that owned an unshared operand); each is an allocation avoided.
    pub grad_in_place: u64,
    /// Expression nodes `ad::eval` visited — the tree walk's work.
    pub exprs_visited: u64,
    /// The part of [`Self::exprs_visited`] that `lower::Invariance::hoistable` marks: nodes
    /// strictly inside a subtree whose value is the same on every evaluation, which a hoisting
    /// stage could stop visiting (`docs/proposals/evaluator-tree-walk.md`, Stage 1).
    pub exprs_hoistable: u64,
}

/// Switch counting on or off. Counts already taken are kept.
pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Zero every counter.
pub fn reset() {
    CTX_MAPS_BUILT.store(0, Ordering::Relaxed);
    PROBE_ALLOCS.store(0, Ordering::Relaxed);
    CTX_MAP_LOOKUPS.store(0, Ordering::Relaxed);
    GRAD_ALLOCS.store(0, Ordering::Relaxed);
    GRAD_CLONES.store(0, Ordering::Relaxed);
    GRAD_IN_PLACE.store(0, Ordering::Relaxed);
    EXPRS_VISITED.store(0, Ordering::Relaxed);
    EXPRS_HOISTABLE.store(0, Ordering::Relaxed);
}

/// The current totals.
pub fn snapshot() -> Counters {
    Counters {
        ctx_maps_built: CTX_MAPS_BUILT.load(Ordering::Relaxed),
        probe_allocs: PROBE_ALLOCS.load(Ordering::Relaxed),
        ctx_map_lookups: CTX_MAP_LOOKUPS.load(Ordering::Relaxed),
        grad_allocs: GRAD_ALLOCS.load(Ordering::Relaxed),
        grad_clones: GRAD_CLONES.load(Ordering::Relaxed),
        grad_in_place: GRAD_IN_PLACE.load(Ordering::Relaxed),
        exprs_visited: EXPRS_VISITED.load(Ordering::Relaxed),
        exprs_hoistable: EXPRS_HOISTABLE.load(Ordering::Relaxed),
    }
}

#[inline]
fn bump(counter: &AtomicU64, n: u64) {
    if ENABLED.load(Ordering::Relaxed) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
}

#[inline]
pub(crate) fn ctx_maps_built(n: u64) {
    bump(&CTX_MAPS_BUILT, n);
}

#[inline]
pub(crate) fn probe_alloc() {
    bump(&PROBE_ALLOCS, 1);
}

#[inline]
pub(crate) fn ctx_map_lookup() {
    bump(&CTX_MAP_LOOKUPS, 1);
}

#[inline]
pub(crate) fn grad_alloc() {
    bump(&GRAD_ALLOCS, 1);
}

/// One `ad::eval` visit to node `expr`, and whether `hoistable` marks it. Called only when the
/// crate is built with `--features walk-stats`.
#[cfg(feature = "walk-stats")]
#[inline]
pub(crate) fn expr_visit(hoistable: &[bool], expr: usize) {
    if ENABLED.load(Ordering::Relaxed) {
        EXPRS_VISITED.fetch_add(1, Ordering::Relaxed);
        if hoistable.get(expr).copied().unwrap_or(false) {
            EXPRS_HOISTABLE.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[inline]
pub(crate) fn grad_in_place() {
    bump(&GRAD_IN_PLACE, 1);
}

/// A copy shares its buffer, so it is counted as a copy, not an allocation.
#[inline]
pub(crate) fn grad_clone() {
    bump(&GRAD_CLONES, 1);
}
