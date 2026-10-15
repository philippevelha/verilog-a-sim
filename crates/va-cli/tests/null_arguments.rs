//! Omitted ("null") arguments, LRM A.6.4/A.6.5: a filter whose coefficient or root list is left
//! out must behave exactly as the same filter with the list written as unity. Checked at the
//! level that matters, the model instance's stamps, not the IR, because the root-list forms
//! reach codegen with an empty list rather than an explicit `{1}`.

use va_abi::stamps::DenseStamp;
use va_abi::{AnalysisCtx, ModelState};

/// Every stamp `src`'s model writes at the bias point `x`, with its unknown count.
fn stamps(src: &str, x_of: impl Fn(usize) -> f64) -> (usize, DenseStamp) {
    let design = va_frontend::compile(src).unwrap_or_else(|e| panic!("compiles: {e}\n{src}"));
    let m = &design.modules[0];
    let terminals: Vec<usize> = (0..m.nodes.len()).collect();
    let mut next = terminals.len();
    let inst = va_codegen::build_instance(m, &terminals, &mut next)
        .unwrap_or_else(|e| panic!("builds: {e}\n{src}"));
    let x: Vec<f64> = (0..next).map(x_of).collect();
    let committed = vec![0.0; inst.state_len()];
    let mut scratch = committed.clone();
    let mut sink = DenseStamp::new(next);
    inst.load(
        &x,
        &AnalysisCtx::dc(),
        &mut ModelState::new(&committed, &mut scratch),
        &mut sink,
    );
    (next, sink)
}

fn module(contribution: &str) -> String {
    format!(
        "module m(inp, out); inout inp, out; electrical inp, out;\n\
         analog V(out) <+ {contribution};\nendmodule\n"
    )
}

/// Each pair: an omitted list, and the same filter with that list written as unity.
#[test]
fn an_omitted_filter_list_loads_exactly_like_unity() {
    let pairs = [
        // A coefficient list left out is the polynomial 1.
        (
            "laplace_nd(V(inp), , {1, 1e-6})",
            "laplace_nd(V(inp), {1}, {1, 1e-6})",
        ),
        // A root list left out is no roots: the empty product, 1.
        (
            "laplace_zp(V(inp), , {-1e6, 0})",
            "laplace_np(V(inp), {1}, {-1e6, 0})",
        ),
        (
            "laplace_zd(V(inp), , {1, 1e-6})",
            "laplace_nd(V(inp), {1}, {1, 1e-6})",
        ),
        ("laplace_np(V(inp), {1}, )", "laplace_nd(V(inp), {1}, {1})"),
        (
            "zi_zd(V(inp), , {1, -0.5}, 1e-6)",
            "zi_nd(V(inp), {1}, {1, -0.5}, 1e-6)",
        ),
        (
            "zi_zp(V(inp), , {0.5, 0}, 1e-6)",
            "zi_np(V(inp), {1}, {0.5, 0}, 1e-6)",
        ),
        // An omitted transition time is the default, as if it were not written.
        (
            "zi_nd(V(inp), {0.5}, {1, -0.5}, 1e-6, )",
            "zi_nd(V(inp), {0.5}, {1, -0.5}, 1e-6)",
        ),
    ];
    let x_of = |i: usize| 0.3 + 0.17 * i as f64;
    for (omitted, written) in pairs {
        let (n_a, a) = stamps(&module(omitted), x_of);
        let (n_b, b) = stamps(&module(written), x_of);
        assert_eq!(n_a, n_b, "unknown count: `{omitted}` vs `{written}`");
        for (what, va, vb) in [
            ("residual", &a.residual, &b.residual),
            ("jacobian", &a.jacobian, &b.jacobian),
            ("charge", &a.charge, &b.charge),
            ("dcharge", &a.dcharge, &b.dcharge),
        ] {
            for (k, (p, q)) in va.iter().zip(vb).enumerate() {
                assert!(
                    (p - q).abs() <= 1e-12 * (1.0 + q.abs()),
                    "{what}[{k}]: `{omitted}` gives {p}, `{written}` gives {q}"
                );
            }
        }
    }
}
