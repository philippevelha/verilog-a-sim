# Golden reference outputs

This directory holds **committed** ngspice reference outputs, one per zoo circuit. `va-harness`
compares the simulator's results against these to the tolerances in `docs/validation.md`.

ngspice is used as an oracle only (§7) — we are not building on it.

## Regenerating

```bash
cargo xtask gen-golden    # shells out to ngspice (if installed) and writes outputs here
```

Commit the regenerated files alongside the change that motivated them, and note in the PR
why the golden data moved. An empty `golden/` means no rung has been captured yet; this file
is the placeholder until the first reference output lands.
