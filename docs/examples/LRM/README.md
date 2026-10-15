# LRM examples

Short Verilog-A examples taken from the language reference manual (LRM), each a construct the
standard allows. They check that this simulator accepts what the LRM says is valid, and rejects
what it says is not. Run one with:

```
cargo run -p va-cli -- check docs/examples/LRM/<file>.va --codegen
```

Each file's header names its LRM section and where the example came from.

## From OpenVAF-Reloaded PR #35

[OpenVAF-Reloaded PR #35](https://github.com/OpenVAF/OpenVAF-Reloaded/pull/35) closed parser
gaps in OpenVAF for the analog event and filter syntax of LRM A.6.4 and A.6.5. The same examples
were run through this simulator on 2026-10-14 (2.28.0):

| File | LRM | Construct | Valid? | 2.28.0 | 2.29.0 |
|------|-----|-----------|--------|--------|--------|
| `cross_null_tolerances.va` | 5.10.3.1, A.6.5 | `cross(expr, dir, , , en)`: tolerances omitted | yes | rejected, `E0207` | accepted |
| `timer_null_tolerance.va` | 5.10.3.3, A.6.5 | `timer(start, period, , en)`: tolerance omitted | yes | rejected, `E0207` | accepted |
| `laplace_null_zeros.va` | 4.5.11, A.6.4 | `laplace_nd(x, , den)`: numerator omitted, meaning unity | yes | rejected, `E0207` | accepted, numerator 1 |
| `event_or.va` | 5.10.1 | `@(initial_step or cross(...))` | yes | accepted | accepted |
| `event_comma.va` | 5.10.1 | `@(initial_step, cross(...))`: a comma ORs events like `or` | yes | refused, `R0201` | accepted, same as `or` |
| `event_or_analysis_list.va` | 5.10.1, 5.10.2 | `@(initial_step("tran") or final_step("tran"))` | yes | refused, `R0202` | refused, `R0202` (deliberate, see below) |
| `timer_null_period_err.va` | 5.10.3.3, A.6.5 | `timer(0, 1e-6, 1e-9, )`: the enable argument omitted, where it may not be | **no** | rejected, generic `E0207` | rejected, `E0238` "argument 4 of `timer` may not be omitted" |

2.29.0 closed the parser gaps:

1. **Omitted (null) arguments** are accepted where LRM A.6.4/A.6.5 allows them: `cross`'s
   direction and tolerances, `above`'s tolerances, `timer`'s period and tolerance, a Laplace
   filter's numerator, denominator and tolerance, a Z-domain filter's numerator, denominator and
   transition time. An omitted argument means "not given", the default. An omitted filter list
   means unity (no roots, or the polynomial 1): `crates/va-cli/tests/null_arguments.rs` checks
   that each such filter loads exactly like the one with `{1}` written.
2. **A comma between events** means `or`, and compiles to the same thing.
3. **An argument omitted where it is required** is `E0238`, naming the argument.

One refusal stays, on purpose: a step event with an analysis list, `initial_step("tran")`,
inside an `or` list (`R0202`). That is not a parser gap: it parses, and the question is what it
should mean. Its body must run only at the first step of the named analysis; this engine honours
the bare `initial_step`, and running the body at every analysis's first step instead would run
it exactly where the model excluded it. It is tracked in `docs/roadmap.md`, "Analog events".

