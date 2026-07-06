//! Discrete events: timepoint breakpoints and threshold crossings during transient.

/// A scheduled transient event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Force a solve exactly at this time (e.g. a source breakpoint).
    Breakpoint(f64),
    /// A monitored crossing was detected at this time.
    Crossing(f64),
}

/// A watched threshold crossing: `crate::integrator::run_with_events` checks whether
/// `x[unknown] − threshold` changes sign between two consecutive *accepted* timepoints, and if
/// so linearly interpolates the crossing time between them.
///
/// This is deliberately not a genuine re-solve at the interpolated time — an honest
/// simplification, not the full LRM `cross()` semantics — but is accurate enough that two
/// accepted points straddling a crossing are already close together (the same LTE control
/// that bounds the state's error between them bounds the interpolation error too).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossingWatch {
    /// Global unknown index to watch.
    pub unknown: usize,
    /// The threshold value that triggers a crossing.
    pub threshold: f64,
}

/// A time-ordered queue of breakpoints the integrator must land on exactly, plus the threshold
/// crossings it should watch for while doing so.
#[derive(Clone, Debug, Default)]
pub struct EventQueue {
    breakpoints: Vec<f64>,
    watches: Vec<CrossingWatch>,
}

impl EventQueue {
    /// Create an empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a breakpoint time (kept sorted, de-duplicated by the integrator).
    pub fn push_breakpoint(&mut self, t: f64) {
        self.breakpoints.push(t);
        self.breakpoints.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }

    /// The next breakpoint strictly after `t`, if any.
    pub fn next_after(&self, t: f64) -> Option<f64> {
        self.breakpoints.iter().copied().find(|&bp| bp > t)
    }

    /// Watch global unknown `unknown` for a crossing of `threshold`.
    pub fn push_watch(&mut self, unknown: usize, threshold: f64) {
        self.watches.push(CrossingWatch { unknown, threshold });
    }

    /// The registered crossing watches, in the order they were pushed (an index into this
    /// slice identifies which watch fired in `crate::integrator::Waveform::crossings`).
    pub fn watches(&self) -> &[CrossingWatch] {
        &self.watches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_breakpoint_after() {
        let mut q = EventQueue::new();
        q.push_breakpoint(2.0);
        q.push_breakpoint(1.0);
        assert_eq!(q.next_after(0.5), Some(1.0));
        assert_eq!(q.next_after(1.0), Some(2.0));
        assert_eq!(q.next_after(2.0), None);
    }

    #[test]
    fn watches_are_recorded_in_push_order() {
        let mut q = EventQueue::new();
        q.push_watch(1, 2.5);
        q.push_watch(0, 0.0);
        assert_eq!(
            q.watches(),
            &[
                CrossingWatch {
                    unknown: 1,
                    threshold: 2.5
                },
                CrossingWatch {
                    unknown: 0,
                    threshold: 0.0
                },
            ]
        );
    }
}
