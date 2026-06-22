//! Workspace-level integration tests.
//!
//! These exercise the crates as a user would compose them. The end-to-end netlist→solve test
//! is `#[ignore]` until the T3/T6 milestones wire the pipeline; the smoke test below runs
//! today against the `va-abi` reference models to keep the harness honest from day one.

use va_abi::reference::{Capacitor, Diode, Resistor, GROUND, VT_300K};
use va_abi::stamps::DenseStamp;
use va_abi::ModelInstance;

/// A two-resistor divider stamped by hand: with the mid node free and the top node pinned to
/// 1 V (modeled as ground-referenced sources), the assembled conductances are symmetric.
#[test]
fn reference_models_stamp_together() {
    // One free unknown: the divider mid-node (index 0). Top tied high, bottom to ground via
    // the sentinel. This only checks that stamps from multiple models accumulate coherently.
    let r_top = Resistor::new(GROUND, 0, 1000.0); // from a (pinned) supply rail into mid
    let r_bot = Resistor::new(0, GROUND, 1000.0); // from mid to ground
    let cap = Capacitor::new(0, GROUND, 1e-9);

    let mut sink = DenseStamp::new(1);
    let x = [0.5];
    r_top.load(&x, &mut sink);
    r_bot.load(&x, &mut sink);
    cap.load(&x, &mut sink);

    // Two 1 kΩ resistors on the diagonal → 2 mS total self-conductance at the mid node.
    assert!((sink.jac(0, 0) - 2e-3).abs() < 1e-12);
    // The capacitor only touches the charge channel in DC assembly.
    assert!((sink.dcharge[0] - 1e-9).abs() < 1e-18);
}

/// The diode reference model's analytic conductance agrees with a central difference — the
/// §5 AD-vs-FD discipline, exercised at the workspace level too.
#[test]
fn diode_conductance_is_consistent() {
    let d = Diode::new(0, GROUND, 1e-14, 1.0, VT_300K);
    let vd = 0.7;
    let h = 1e-6;
    let fd = (d.current(vd + h) - d.current(vd - h)) / (2.0 * h);
    let rel = (fd - d.conductance(vd)).abs() / d.conductance(vd).abs();
    assert!(rel < 1e-5);
}

#[test]
#[ignore = "T3/T6: solve circuits/divider.net end-to-end to V(mid)=0.5 within DC tolerance"]
fn divider_end_to_end() {}
