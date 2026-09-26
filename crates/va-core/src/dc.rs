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
/// **Three tiers, cheapest in practice first** (since 1.23.0): the ladder with each Newton
/// step's change to a node capped ([`RESCUE_NODE_STEP`]); if that fails, the plain ladder; if
/// that fails, the ladder with Newton's residual line search ([`RESCUE_DAMPING_HALVINGS`]). Each
/// tier is reached only when the ones before it have failed, and a tier whose aid the caller
/// already asked for is skipped.
///
/// **Why the capped ladder first** (`docs/proposals/dc-rescue.md`): the plain ladder used to run
/// first, so that what it solved kept its path bit for bit — but on every circuit measured that
/// needs the rescue the capped ladder is never more expensive, and where the plain ladder fails
/// it fails slowly. On c432 it spent 265 Newton iterations failing (a 2-cycle of runaway steps)
/// before the capped ladder converged in 290; capped first, the solve takes 293 instead of 558.
/// The plain ladder stays as the next tier, so a circuit the cap cannot solve and the ladder can
/// is still solved. The cost: a circuit the plain ladder used to rescue now takes the capped
/// path, and its answer moves in the last digits (c17's starting point; the PSP103 chains of 40
/// and 80 stages, by 1.6e-16 V).
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
        Err(first) if cfg.gmin_steps == 0 && worth_a_gmin_retry(&first) => {
            let laddered = NewtonConfig {
                gmin_steps: GMIN_RESCUE_STEPS,
                max_iters: cfg.max_iters.max(GMIN_RESCUE_ITERS),
                ..cfg
            };
            // The tiers, cheapest in practice first; later ones are only reached when earlier
            // ones fail (§ above, `RESCUE_NODE_STEP`, `RESCUE_DAMPING_HALVINGS`).
            let tiers = [
                (cfg.max_node_step > RESCUE_NODE_STEP).then_some(NewtonConfig {
                    max_node_step: RESCUE_NODE_STEP,
                    ..laddered
                }),
                Some(laddered),
                (cfg.max_damping_halvings < RESCUE_DAMPING_HALVINGS).then_some(NewtonConfig {
                    max_damping_halvings: RESCUE_DAMPING_HALVINGS,
                    ..laddered
                }),
            ];
            for tier in tiers.into_iter().flatten() {
                match attempt(tier) {
                    Ok(ok) => return Ok(ok),
                    Err(e) if worth_a_gmin_retry(&e) => {}
                    Err(_) => break,
                }
            }
            Err(first)
        }
        Err(e) => Err(e),
    }
}

/// How many stages the `gmin` rescue ramps over. Thirty is enough for the floating-node cases
/// this exists for and cheap enough to be worth trying before giving up, since it is only ever
/// reached on a solve that has already failed.
const GMIN_RESCUE_STEPS: usize = 30;

/// The node-step cap in the rescue's first tier (`NewtonConfig::max_node_step`), in the node's
/// own units: 0.5 V on an electrical node.
///
/// **Why it exists.** ISCAS'85 c432 at transistor level (910 PSP103 devices, 15 416 unknowns) is
/// not solved by the ladder or the damped ladder. Traced, both stuck on the internal nodes of
/// 4-high NMOS stacks (the `and4` cell inside a composed 4-input NAND), which are set only by
/// leakage while a lower device is off: each linearized step divides by a near-zero conductance
/// and proposes 11–23 V on a 1.8 V circuit. Undamped the ladder cycles between two such points
/// for its 150 iterations; damped, the whole step shrinks until nothing moves (the residual sat
/// at 1.01e-4). Capping each node's step instead lets every other row take its full step, and
/// the ladder converges: c432 solves in 3.8 min, all seven outputs matching the logic.
///
/// **Why before damping.** It costs nothing per iteration, where the line search costs a trial
/// evaluation per halving: the PSP103 inverter chains the damped tier was added for (1.10.1)
/// solve with the cap too, faster — 95 stages in 14.0 s against 23.2 s damped, 160 in 27.4 s
/// against 40.8 s. **Why before the plain ladder** (1.23.0): see [`with_gmin_rescue`].
///
/// **Not in units other than volts:** on a thermal or optical node 0.5 is 0.5 of that node's
/// unit, which may be slow. Since 1.23.0 this tier is the first a failed solve reaches, so that
/// limitation now applies to every rescue on such a circuit: a solve the cap slows but does not
/// stop is paid for; one it stops falls through to the plain ladder.
const RESCUE_NODE_STEP: f64 = 0.5;

/// Step halvings allowed in the rescue's last tier, which reruns the `gmin` ladder with
/// Newton's residual line search (`NewtonConfig::max_damping_halvings`) switched on.
///
/// **Why it exists.** On a chain of PSP103 CMOS inverters the ladder alone fails from ~95
/// stages (~3 150 unknowns), on the dense and the sparse path alike: part-way down the ladder a
/// single undamped Newton step proposes a change of ~5.7e4 V at a net near the end of the chain,
/// the residual jumps from 1.5e-4 to 54, and the step back lands on a Jacobian that is
/// numerically singular. The step is not limited by anything else: junction limiting covers only
/// the unknowns a model marks as junctions. With the line search the step that makes the residual
/// 3.6e5 times worse is halved until it improves it instead, and the chain solves (measured at
/// 95, 100 and 160 stages; `docs/validation.md`). **Limit:** at 320 stages (10 566 unknowns) it
/// still fails, differently — the Jacobian goes singular right after a 0.5 V step, not a runaway
/// one, which no line search can help. Not diagnosed.
///
/// Since 1.12.0 the node-step tier comes first and solves those chains itself; this tier remains
/// as the last resort.
///
/// **Why a tier rather than part of the ladder.** Damping costs at least one extra
/// assembly per iteration. Switched on for the whole rescue, the 80-stage chain — which the
/// ladder alone already solves — went from 6.8 s to 35 s. As its own tier it costs nothing to
/// anything the first tier solves. Twenty halvings reduce the 5.7e4 V step to ~0.05 V.
const RESCUE_DAMPING_HALVINGS: usize = 20;

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
    /// nodes one at a time with no other conductance path to hold them, and does not arrive
    /// within the default iteration budget. That test pins that `newton::solve` still fails;
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
        // floating point, not of this code: at the default budget of 100 it can run out of
        // iterations (`NoConvergence`), overflow the factorization (`Singular`) or report a
        // non-finite row on the way. (Given 2000 iterations it now arrives — see `newton.rs`'s
        // sibling for why that was never a property to rely on.) This test pinned `Singular`
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

    /// The rescue's tiers, driven by a scripted attempt so the control flow is checked on its
    /// own: which configs are tried, in which order, and which error surfaces. The physical cases
    /// the tiers exist for (a PSP103 inverter chain, ISCAS'85 c432) need a model that is not in
    /// the repository, so they are measured and recorded in `docs/validation.md` and
    /// `docs/proposals/dc-rescue.md` rather than run here.
    #[test]
    fn the_rescue_tries_the_capped_ladder_then_the_ladder_then_the_damped_ladder() {
        let inf = f64::INFINITY;
        let tried = std::cell::RefCell::new(Vec::new());
        let record = |c: &NewtonConfig| {
            tried
                .borrow_mut()
                .push((c.gmin_steps, c.max_node_step, c.max_damping_halvings));
        };
        // Fails unless the ladder *and* the line search are on: every tier is tried, in order.
        let needs_damping = |c: NewtonConfig| {
            record(&c);
            if c.gmin_steps > 0 && c.max_damping_halvings > 0 {
                Ok(c.max_damping_halvings)
            } else {
                Err(CoreError::Singular)
            }
        };
        let got = with_gmin_rescue(NewtonConfig::default(), needs_damping);
        assert_eq!(
            got.expect("the damped ladder solves it"),
            RESCUE_DAMPING_HALVINGS
        );
        assert_eq!(
            *tried.borrow(),
            vec![
                (0, inf, 0),
                (GMIN_RESCUE_STEPS, RESCUE_NODE_STEP, 0),
                (GMIN_RESCUE_STEPS, inf, 0),
                (GMIN_RESCUE_STEPS, inf, RESCUE_DAMPING_HALVINGS)
            ]
        );

        // What the capped ladder solves is the first rescue tried, and reaches no other.
        tried.borrow_mut().clear();
        let needs_cap = |c: NewtonConfig| {
            record(&c);
            if c.max_node_step.is_finite() {
                Ok(())
            } else {
                Err(CoreError::Singular)
            }
        };
        with_gmin_rescue(NewtonConfig::default(), needs_cap).expect("the capped ladder solves it");
        assert_eq!(
            *tried.borrow(),
            vec![(0, inf, 0), (GMIN_RESCUE_STEPS, RESCUE_NODE_STEP, 0)]
        );

        // A circuit the cap cannot solve but the plain ladder can is still solved: the plain
        // ladder is the next tier, and the rescue stops there.
        tried.borrow_mut().clear();
        let needs_uncapped_ladder = |c: NewtonConfig| {
            record(&c);
            if c.gmin_steps > 0 && !c.max_node_step.is_finite() {
                Ok(())
            } else {
                Err(CoreError::Singular)
            }
        };
        with_gmin_rescue(NewtonConfig::default(), needs_uncapped_ladder)
            .expect("the plain ladder solves it");
        assert_eq!(
            *tried.borrow(),
            vec![
                (0, inf, 0),
                (GMIN_RESCUE_STEPS, RESCUE_NODE_STEP, 0),
                (GMIN_RESCUE_STEPS, inf, 0)
            ]
        );

        // If every tier fails, the error is the first one: the real circuit's, not a variant's.
        let always_fails = |c: NewtonConfig| -> Result<(), CoreError> {
            if c.gmin_steps == 0 {
                Err(CoreError::NoConvergence {
                    iters: 7,
                    residual: 1.0,
                })
            } else {
                Err(CoreError::Singular)
            }
        };
        let err = with_gmin_rescue(NewtonConfig::default(), always_fails).unwrap_err();
        assert!(
            matches!(err, CoreError::NoConvergence { iters: 7, .. }),
            "{err}"
        );
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
