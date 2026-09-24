//! Event counters for assembly, read by `va-cli sim --logfull`.
//!
//! [`stamp_lookups`] counts the hash lookups the sparse system does to find where a Jacobian or
//! `dcharge` stamp goes (`SparseSystem`'s `(row, col) → slot` map): one per stamp. The dense
//! path indexes directly and does none.
//!
//! **Off by default, and nearly free while off**: each stamp checks one relaxed atomic flag
//! before counting. On, a count is a relaxed atomic add. Process-wide totals.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STAMP_LOOKUPS: AtomicU64 = AtomicU64::new(0);

/// Switch counting on or off. Counts already taken are kept.
pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Zero the counters.
pub fn reset() {
    STAMP_LOOKUPS.store(0, Ordering::Relaxed);
}

/// Hash lookups done by sparse stamping since the process started (or since [`reset`]).
pub fn stamp_lookups() -> u64 {
    STAMP_LOOKUPS.load(Ordering::Relaxed)
}

#[inline]
pub(crate) fn stamp_lookup() {
    if ENABLED.load(Ordering::Relaxed) {
        STAMP_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    }
}
