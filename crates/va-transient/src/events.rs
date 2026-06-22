//! Discrete events: timepoint breakpoints and threshold crossings during transient.

/// A scheduled transient event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Force a solve exactly at this time (e.g. a source breakpoint).
    Breakpoint(f64),
    /// A monitored crossing was detected at this time.
    Crossing(f64),
}

/// A time-ordered queue of breakpoints the integrator must land on.
#[derive(Clone, Debug, Default)]
pub struct EventQueue {
    breakpoints: Vec<f64>,
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
}
