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

| File | LRM | Construct | Valid? | This simulator |
|------|-----|-----------|--------|----------------|
| `cross_null_tolerances.va` | 5.10.3.1, A.6.5 | `cross(expr, dir, , , en)`: tolerances omitted | yes | **rejected**: `E0207` "expected an expression, found `,`" |
| `timer_null_tolerance.va` | 5.10.3.3, A.6.5 | `timer(start, period, , en)`: tolerance omitted | yes | **rejected**: `E0207` |
| `laplace_null_zeros.va` | 4.5.11, A.6.4 | `laplace_nd(x, , den)`: numerator omitted, meaning unity | yes | **rejected**: `E0207` |
| `event_or.va` | 5.10.1 | `@(initial_step or cross(...))` | yes | accepted |
| `event_comma.va` | 5.10.1 | `@(initial_step, cross(...))`: a comma ORs events like `or` | yes | **refused**: `R0201`, which says only an `or` list is honoured |
| `event_or_analysis_list.va` | 5.10.1, 5.10.2 | `@(initial_step("tran") or final_step("tran"))` | yes | **refused**: `R0202` (an analysis-filtered step event inside an `or` list) |
| `timer_null_period_err.va` | 5.10.3.3, A.6.5 | `timer(0, 1e-6, 1e-9, )`: the enable argument omitted where it may not be | **no** | rejected, but by the generic `E0207`; OpenVAF says "argument 4 of 'timer' may not be omitted" |

So the simulator has the same gaps OpenVAF had before PR #35:

1. **Omitted (null) arguments** in event functions and in Laplace/Z filters are a syntax error
   here. The LRM allows them in the tolerance positions of `cross`, `above`, `timer` and
   `absdelta`, and in a filter's coefficient vectors, where an omitted numerator means unity
   (H(s) = 1/D(s)), not zero.
2. **A comma between events** is refused, although it means the same as `or`.
3. **Step events with analysis lists in an `or` list** are refused. That one is a deliberate
   refusal (see `release.txt` and `docs/roadmap.md`, "Analog events"), not a parse gap.
4. A null argument where the LRM forbids one is rejected, but the message does not say which
   argument may not be omitted.
