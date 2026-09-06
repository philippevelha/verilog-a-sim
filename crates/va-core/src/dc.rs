//! DC analysis: operating point and parameter/source sweep.

use crate::newton::{self, NewtonConfig};
use crate::CoreError;
use va_abi::ModelInstance;

/// Result of a DC operating-point solve: the converged solution vector.
#[derive(Clone, Debug)]
pub struct OperatingPoint {
    /// Node voltages / branch currents at the operating point (global unknown order).
    pub x: Vec<f64>,
}

/// Compute the DC operating point of a circuit described by `instances`.
///
/// # Errors
///
/// Propagates [`CoreError`] from the underlying Newton solve.
/// Each instance's `above`-site values at one operating point, indexed `[instance][slot]`.
///
/// `None` marks a slot that is not an `above`: a `cross` never fires in a static solve and a
/// `timer` has no value there, so those slots take part in nothing. Carried between the points
/// of a DC sweep, where the LRM wants a crossing from below rather than a bare "is positive".
pub type AboveValues = Vec<Vec<Option<f64>>>;

/// Solve an operating point, firing any `above` event whose expression is already past its
/// threshold — Verilog-A's one analog event that triggers in a **static** solve (LRM §5.10.2).
///
/// Two phases, because the two facts depend on each other: which events fire is a property of
/// the solution, and the bodies they guard change the equations that produce it. So this solves
/// once with nothing fired, asks each instance what its sites read there, and re-solves with the
/// firings set. `previous` supplies the values from the preceding point of a DC *sweep*, where
/// the LRM asks for a crossing from below rather than a bare "is positive"; pass `None` for a
/// standalone operating point, which is the initialization case.
///
/// It iterates to a fixed point rather than re-solving once, because a body may push another
/// site past its own threshold. Bounded: an event can only ever turn on here (a fired set that
/// stopped growing is the fixed point), so the loop is at most one pass per site.
///
/// # Errors
///
/// As [`operating_point`].
pub fn operating_point_with_events(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    previous: Option<&AboveValues>,
) -> Result<(OperatingPoint, AboveValues), CoreError> {
    let mut fired = va_abi::FiredEvents::new(instances);
    let mut x = newton::solve_with_events(instances, dim, cfg, &fired)?;
    let mut values = poll_above(instances, &x);

    if !fired.is_empty() {
        for _ in 0..=fired.len() {
            let mut changed = false;
            for (i, slots) in values.iter().enumerate() {
                for (slot, v) in slots.iter().enumerate() {
                    let Some(now) = *v else { continue };
                    // Initialization fires on "already positive"; a sweep point fires only on a
                    // crossing from below, so a signal that stays positive fires once, at the
                    // point it arrived, and not at every point thereafter.
                    let trigger = match previous.and_then(|p| p.get(i)).and_then(|s| s.get(slot)) {
                        Some(Some(was)) => *was <= 0.0 && now > 0.0,
                        _ => now > 0.0,
                    };
                    if trigger && !fired.is_set(i, slot) {
                        fired.set(i, slot);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
            x = newton::solve_with_events(instances, dim, cfg, &fired)?;
            values = poll_above(instances, &x);
        }
    }

    Ok((OperatingPoint { x }, values))
}

/// Each instance's `above` sites' values at `x`, indexed `[instance][slot]`.
///
/// `None` for a slot that is not an `above` — a `cross` never fires in a static solve, and a
/// `timer` has no value here — so those slots take part in nothing.
fn poll_above(instances: &[&dyn ModelInstance], x: &[f64]) -> AboveValues {
    instances
        .iter()
        .map(|inst| {
            let n = inst.event_count();
            if n == 0 {
                return Vec::new();
            }
            let mut sink = va_abi::events::RecordingEventSink::new();
            inst.events(x, &va_abi::ANALYSIS_DC, &mut sink);
            let mut per_slot = vec![None; n];
            for (slot, value, spec) in sink.monitors {
                if slot < n && spec.at_initialization {
                    per_slot[slot] = Some(value);
                }
            }
            per_slot
        })
        .collect()
}

pub fn operating_point(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
) -> Result<OperatingPoint, CoreError> {
    let x = newton::solve(instances, dim, cfg)?;
    Ok(OperatingPoint { x })
}

/// Sweep an externally-controlled quantity, solving a DC operating point at each step.
///
/// `points` are the swept values (e.g. a source voltage or a parameter). For each, `rebuild`
/// produces the instance set for that value; the operating point is solved and collected.
/// This keeps `va-core` agnostic about *what* is being swept — the caller owns the device
/// construction and just hands back fresh instances.
///
/// # Errors
///
/// Propagates the first [`CoreError`] encountered; earlier results are discarded.
pub fn sweep<'a, F>(
    points: &[f64],
    dim: usize,
    cfg: NewtonConfig,
    mut rebuild: F,
) -> Result<Vec<OperatingPoint>, CoreError>
where
    F: FnMut(f64) -> Vec<Box<dyn ModelInstance + 'a>>,
{
    let mut out = Vec::with_capacity(points.len());
    for &value in points {
        let owned = rebuild(value);
        let refs: Vec<&dyn ModelInstance> = owned.iter().map(|b| b.as_ref()).collect();
        out.push(operating_point(&refs, dim, cfg)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::VSource;
    use va_abi::reference::{Resistor, GROUND};

    #[test]
    fn divider_operating_point() {
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let op = operating_point(&insts, 3, NewtonConfig::default()).expect("converges");
        assert!((op.x[1] - 1.0).abs() < 1e-9, "midpoint = {}", op.x[1]);
    }

    #[test]
    fn sweep_divider_input() {
        // Sweep Vin; the midpoint of an equal divider tracks Vin/2.
        let points = [0.0, 1.0, 2.0, 5.0];
        let results = sweep(&points, 3, NewtonConfig::default(), |vin| {
            vec![
                Box::new(VSource::new(0, GROUND, 2, vin)),
                Box::new(Resistor::new(0, 1, 1000.0)),
                Box::new(Resistor::new(1, GROUND, 1000.0)),
            ]
        })
        .expect("all points converge");

        for (vin, op) in points.iter().zip(&results) {
            assert!(
                (op.x[1] - vin / 2.0).abs() < 1e-9,
                "vin {vin}: mid {}",
                op.x[1]
            );
        }
    }
}
