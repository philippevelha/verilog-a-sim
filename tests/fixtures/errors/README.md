# Error corpus

Each `.va` file here is a Verilog-A model with **one** deliberate mistake. Its header says what
`va-cli check --codegen` must report for it. `crates/va-cli/tests/error_corpus.rs` runs the real
binary on every file, so the test checks what a user actually reads: the stage that rejected the
file, the words of the message, the location, and the exit code (1 for a rejected file, 0 for an
accepted one).

The corpus covers the other half of `docs/token-reference.md`. That document tracks which
constructs are *accepted*. This corpus tracks whether invalid input is *rejected*, and whether
the message helps the user fix it.

## Header

The header is the file's leading `// key: value` lines:

| Key        | Meaning |
|------------|---------|
| `expect`   | Required. The verdict: `pp`, `lex`, `parse`, `elab`, `cgen` (the stage label `check` prints), `refused` (a deliberate refusal, see CLAUDE.md §5), or `ok` (accepted). |
| `message`  | Text the output must contain. May repeat; every one must match. |
| `at`       | `line:col` the output must name, matched as `:line:col`. Header lines count. |
| `fixed`    | The version that started rejecting this file, and what happened before. Not asserted; it's there for the reader. |
| `poor`     | A known defect in an otherwise correct diagnostic. Not asserted (that would lock the defect in), but counted and printed by the test. |
| `gap`      | With `expect: ok`, required unless `warning` is given. Says why the file is invalid even though it's still accepted. |
| `warning`  | With `expect: ok`: text a `[warn ]` line must contain. The file is accepted, with that warning. |

## Known gaps and poor messages

`expect: ok` + `gap:` records invalid Verilog-A that is still accepted. The test requires the
file to *stay* accepted. When a change starts rejecting it, the test fails and points here: set
`expect` to the new verdict, add `message`/`fixed`, and rename the file from `gap_*` to the
stage prefix. A gap can't close, or reopen, without the corpus noticing.

When you improve a `poor:` message, delete the note and tighten the `message:`/`at:` lines so
the improvement is locked in.

`cargo test -p va-cli --test error_corpus -- --nocapture` prints the current list of both.

## Warnings

A file named `warn_*` is accepted, with a warning. Use this for source whose validity is
unsettled, or which a simulator should run but flag. The `warning:` line is asserted like a
`message:`, and `at:` checks the warning's location. Currently one file:
`warn_port_without_direction.va` (see `release.txt` 2.18.5 for why it is not an error).

## Adding a case

Name the file `<stage>_<what is wrong>.va` and keep it minimal: one mistake, in an otherwise
valid model. That way the expected verdict is unambiguous. Assert the part of the message that
names what the user wrote (the identifier, the function, the counts), not the boilerplate around
it. Give every case an `at:` when the diagnostic is located; since 2.18.3 every stage's is,
except `cgen`, whose input (the IR) carries no source positions.
