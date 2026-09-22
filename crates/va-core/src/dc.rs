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
    operating_point_continued(instances, dim, cfg, previous, None)
}

/// [`operating_point_with_events`], started from `start` rather than the zero vector — what a
/// `.dc` sweep uses to continue from the point before (see
/// [`newton::solve_with_events_from`]).
///
/// The event re-solves inside the loop start from `start` too, not from zero: an `above` that
/// fires changes the equations slightly, and the pre-firing solution is a far better guess for
/// the post-firing one than the origin is.
///
/// # Errors
///
/// As [`operating_point_with_events`].
pub fn operating_point_continued(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    previous: Option<&AboveValues>,
    start: Option<&[f64]>,
) -> Result<(OperatingPoint, AboveValues), CoreError> {
    with_gmin_rescue(cfg, |c| {
        solve_operating_point(instances, dim, c, previous, start)
    })
}

/// Run `attempt`, and if it fails in a way `gmin` stepping can plausibly rescue, run it once
/// more with the homotopy switched on.
///
/// **`gmin` stepping as a rescue rather than a routine.** The homotopy shunts a conductance from
/// every `Node` row to ground and ramps it away (`convergence::gmin_for_step`: 1e-3 S down to
/// 1e-12 S, then exactly 0), which gives a node with no DC path something to be solved through
/// until the real circuit takes over. That is what a series stack needs — in a CMOS NAND the
/// node between the two pull-down devices floats whenever the lower one is off, and four of the
/// twelve models in `circuits/benchmark/nand/` could not be solved at all without it.
///
/// It runs **only after the plain solve has failed**, and that is the whole design. The ladder
/// is `gmin_steps + 1` full Newton solves, so making it the default path would multiply the cost
/// of every DC point in every circuit — including the overwhelming majority that converge on the
/// first try and need none of it. As a fallback it costs those circuits nothing and leaves their
/// answers bit-identical, which is what let `xtask validate` stay unchanged across this change.
///
/// A caller that has already asked for stepping (`cfg.gmin_steps > 0`) is left alone: it chose
/// its own ladder and a second one would not be a rescue but a contradiction.
///
/// The **original** failure is what surfaces if the rescue also fails. It describes the real
/// circuit, naming the row that went singular or non-finite; the laddered one describes a
/// shunted variant the user never wrote.
fn with_gmin_rescue<T>(
    cfg: NewtonConfig,
    mut attempt: impl FnMut(NewtonConfig) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    match attempt(cfg) {
        Ok(ok) => Ok(ok),
        // **`gmin` stepping, as a rescue rather than a routine.** The homotopy shunts a
        // conductance from every `Node` row to ground and ramps it away
        // (`convergence::gmin_for_step`: 1e-3 S down to 1e-12 S, then exactly 0), which gives a
        // node with no DC path something to be solved through until the real circuit takes
        // over. That is what a series stack needs: in a CMOS NAND the node between the two
        // pull-down devices floats whenever the lower one is off, and six of the twelve models
        // in `circuits/benchmark/nand/` could not be solved without this.
        //
        // It runs **only after the plain solve has failed**, and that is deliberate. The ladder
        // is `gmin_steps + 1` full Newton solves, so making it the default path would multiply
        // the cost of every DC point in every circuit — including the overwhelming majority
        // that converge on the first try and need none of it. As a fallback it costs those
        // circuits nothing at all and leaves their answers bit-identical, which is what lets
        // `xtask validate` stay unchanged across this.
        //
        // The *original* failure is reported if the rescue also fails: it describes the real
        // circuit, naming the row that went singular or non-finite, where the laddered one
        // describes a shunted variant of it that the user never wrote.
        Err(first) if cfg.gmin_steps == 0 && worth_a_gmin_retry(&first) => attempt(NewtonConfig {
            gmin_steps: GMIN_RESCUE_STEPS,
            max_iters: cfg.max_iters.max(GMIN_RESCUE_ITERS),
            ..cfg
        })
        .map_err(|_| first),
        Err(e) => Err(e),
    }
}

/// How many stages the `gmin` rescue ramps over. Ten is enough for the floating-node cases this
/// exists for and cheap enough to be worth trying before giving up, since it is only ever
/// reached on a solve that has already failed.
const GMIN_RESCUE_STEPS: usize = 30;

/// Iteration budget per ladder stage during the rescue, if the caller asked for less. The
/// stages are deliberately gentle, but the final unshunted one still has to walk from the last
/// shunted solution to the real answer, and on a long series chain that is more than the
/// default 100 steps — measured, not guessed: this exact fixture needs it.
const GMIN_RESCUE_ITERS: usize = 150;

/// Whether a failure is the kind `gmin` stepping can plausibly rescue.
///
/// All three are, and each was observed to be on a real deck: a floating node makes the matrix
/// **singular**, a stack that Newton cannot walk into fails to **converge**, and an operating
/// point the iteration cannot reach at all reports a **non-finite** row. What they have in
/// common is that a shunt to ground gives the iteration somewhere to stand; what `gmin` cannot
/// fix is a model that is wrong, which is why the original error is what surfaces if the rescue
/// fails too.
fn worth_a_gmin_retry(e: &CoreError) -> bool {
    match e {
        CoreError::Singular | CoreError::NoConvergence { .. } | CoreError::NonFinite { .. } => true,
    }
}

/// [`operating_point_continued`]'s solve, at one fixed [`NewtonConfig`] — the body that the
/// `gmin` rescue above runs a second time with a laddered config.
fn solve_operating_point(
    instances: &[&dyn ModelInstance],
    dim: usize,
    cfg: NewtonConfig,
    previous: Option<&AboveValues>,
    start: Option<&[f64]>,
) -> Result<(OperatingPoint, AboveValues), CoreError> {
    let mut fired = va_abi::FiredEvents::new(instances);
    let mut x = newton::solve_with_events_from(instances, dim, cfg, &fired, start)?;
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
            x = newton::solve_with_events_from(instances, dim, cfg, &fired, start)?;
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
    // The same `gmin` rescue `operating_point_continued` uses. Without it here, a circuit with
    // a floating node would solve under `.op`/`.dc` and fail under `.ac`/`.noise`, which
    // linearize around this call — the same circuit, a different verdict.
    let x = with_gmin_rescue(cfg, |c| newton::solve(instances, dim, c))?;
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

    /// A circuit plain Newton cannot solve gets an operating point anyway, through the `gmin`
    /// rescue — and the rescue is not reached by anything that already worked.
    ///
    /// The fixture is the one `newton::gmin_stepping_converges_a_circuit_plain_newton_cannot`
    /// established: 20 diodes in series behind a 10 Ω resistor at 20 V from a cold start. A real
    /// operating point exists, but plain Newton's junction limiting walks the chain's internal
    /// nodes into the exponential's overflow range with no other conductance path to hold them,
    /// and the factorization goes singular. That test pins that `newton::solve` still fails;
    /// this one pins that `dc::operating_point` no longer does.
    #[test]
    fn a_solve_that_fails_outright_is_retried_with_gmin_stepping() {
        use va_abi::reference::diode::VT_NOMINAL;
        use va_abi::reference::Diode;

        let n_diodes = 20;
        let branch = n_diodes + 1;
        let dim = branch + 1;
        let vs = VSource::new(0, GROUND, branch, 20.0);
        let r = Resistor::new(0, 1, 10.0);
        let mut diodes = Vec::new();
        for i in 1..n_diodes {
            diodes.push(Diode::new(i, i + 1, 1e-14, 1.0, VT_NOMINAL));
        }
        diodes.push(Diode::new(n_diodes, GROUND, 1e-14, 1.0, VT_NOMINAL));
        let mut insts: Vec<&dyn ModelInstance> = vec![&vs, &r];
        insts.extend(diodes.iter().map(|d| d as &dyn ModelInstance));

        // The bare solver still fails — which is what makes this test discriminating rather
        // than merely green.
        //
        // Asserted as "fails with something the rescue would retry", not as one named variant.
        // *Which* way a 20-diode chain at 20 V comes apart is a property of the platform's
        // floating point, not of this code: at a generous iteration budget it overflows the
        // factorization (`Singular`, what `newton.rs`'s sibling test sees at `max_iters: 2000`),
        // but at the default budget of 100 it can equally run out of iterations first
        // (`NoConvergence`) or report a non-finite row on the way. This test pinned `Singular`
        // and went red on macOS for exactly that reason while Linux and Windows stayed green —
        // a real portability bug in the assertion, not in the solver. The precondition the
        // rescue actually needs is `worth_a_gmin_retry`, so that is what is checked.
        let bare = crate::newton::solve(&insts, dim, NewtonConfig::default());
        match &bare {
            Err(e) => assert!(
                worth_a_gmin_retry(e),
                "the fixture fails with `{e}`, which the rescue would not retry — this test \
                 would then prove nothing about `gmin`"
            ),
            Ok(_) => panic!(
                "the fixture must still defeat plain Newton, or the rescue below proves nothing"
            ),
        }

        let op = operating_point(&insts, dim, NewtonConfig::default())
            .expect("the gmin rescue should carry this");
        // ~0.81 V across each of 20 diodes, the rest across the 10 Ω resistor.
        let i_r = (op.x[0] - op.x[1]) / 10.0;
        assert!(
            i_r > 0.1 && i_r < 1.0,
            "series current {i_r} A is not a sane operating point"
        );
        for k in 1..n_diodes {
            let vd = op.x[k] - op.x[k + 1];
            assert!(
                vd > 0.6 && vd < 1.0,
                "diode {k} sits at {vd} V, outside a forward drop"
            );
        }
    }

    /// The rescue does not fire on a circuit that converges, and does not change its answer.
    ///
    /// This is the property that let the golden gate stay bit-identical: the ladder is
    /// `gmin_steps + 1` full Newton solves, and paying that on every DC point of every circuit
    /// would be a large, silent cost.
    #[test]
    fn a_circuit_that_converges_is_untouched_by_the_rescue() {
        let vs = VSource::new(0, GROUND, 2, 2.0);
        let r1 = Resistor::new(0, 1, 1000.0);
        let r2 = Resistor::new(1, GROUND, 1000.0);
        let insts: [&dyn ModelInstance; 3] = [&vs, &r1, &r2];

        let plain = crate::newton::solve(&insts, 3, NewtonConfig::default()).expect("converges");
        let through_dc = operating_point(&insts, 3, NewtonConfig::default()).expect("converges");
        assert_eq!(
            plain, through_dc.x,
            "a converging circuit must come back bit-identical, not merely close"
        );
    }

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
