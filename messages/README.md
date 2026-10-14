# Diagnostic message catalogue

Every error, warning, refusal and user-facing message of `va-cli` has a **code**, such as
`E0703`. This directory holds what the program says about each code, one JSON file per code
and per language:

```
messages/
  en/E0000.json   English (the reference; required for every code)
  en/E0701.json
  fr/E0701.json   a translation (optional; any code, any language)
```

Program code decides *when* a problem happens and supplies its details (a name, a value, a
location). This directory decides *how it is explained* and *what the user should do*. You can
improve the text, or add a language, without touching Rust.

## What a user sees

Every occurrence prints the one-line `message`, with its details filled in:

```
error[E0703]: invalid value `fast` for --solver; expected `auto`, `dense` or `sparse`
  = for what this means and what to do: va-cli --explain E0703
```

`va-cli --explain E0703` prints the full entry:

```
E0703 (error)
invalid value `{value}` for {option}; expected {expected}

What it means:
  A command-line option or an environment variable was given a value it does not accept. …

What to do:
  1. Use one of the accepted values listed in the message.
  2. …
```

## The file format

```json
{
  "code": "E0703",
  "rev": 1,
  "message": "invalid value `{value}` for {option}; expected {expected}",
  "description": "What the problem is, in a few sentences.",
  "actions": [
    "What to try first.",
    "What to try next."
  ]
}
```

| Field | Required | Meaning |
|-------|----------|---------|
| `code` | always | The code. Must equal the file name (`E0703.json` holds `"E0703"`). |
| `message` | always | The one-line text shown with every occurrence. `{name}` placeholders are filled in by the program. |
| `description` | always | What the problem means. Shown by `--explain`. Use `\n` for a line break. |
| `actions` | always | What to do, as a list, best first. At least one entry. Shown numbered by `--explain`. |
| `rev` | English only | The revision of the English text. **Raise it by one whenever you change what the English text says** (not for a typo), so translations can tell they are out of date. |
| `source_rev` | translations only | The English `rev` this translation was made from. |
| `retired` | optional | `true` when the problem can no longer happen. The code stays reserved forever. |

Any other field is rejected, so a misspelt field name is caught rather than ignored.

### Placeholders

`{value}`, `{option}`, `{name}` and so on stand for details the program supplies. The program
decides which placeholders a code has; the English entry shows them. Rules:

- Keep every placeholder of the English entry, spelled exactly the same, in every translation.
  You may move them anywhere in the sentence.
- Do not add placeholders the English entry does not have; they would show up literally.
- Placeholders usually appear only in `message`. A refusal (`R` code) also fills them in its
  `description` and `actions`, because its explanation names the construct the user wrote.
  Elsewhere, `--explain` shows `description` and `actions` without an occurrence to fill them
  from, so a placeholder there would show literally; keep those general.
- A placeholder name starts with a letter or `_`. So `{1}` or `{1, tau}`, a Verilog-A array
  literal quoted in a message, is plain text, not a placeholder.
- Some placeholders hold a few English words supplied by the program rather than a name or a
  number (`{via}` in `E0374`, `{construct}` in `E0375`, `{what}` and `{list}`). They are not
  translated yet; keep them, and phrase the sentence so they read as a quotation.
- Counts are not pluralised automatically, so phrase a sentence with a number so it reads
  correctly for any number ("declares 1 argument(s)", or "declares N, this call passes M").

## Codes

A code is a letter and four digits.

| Letter | Kind | Shown as |
|--------|------|----------|
| `E` | error: the input is wrong, or the run failed | `error[E…]` |
| `W` | warning: the run goes on, but something deserves a look | `warning[W…]` |
| `R` | refusal: valid Verilog-A that this simulator deliberately declines to run | `refused[R…]` |
| `C` | communication: information a user may act on | `note[C…]` |

The first two digits say which stage of the program raised it:

| Range | Stage |
|-------|-------|
| `00xx` | general. `E0000` is an error with no code yet |
| `01xx` | preprocessor (`` `include ``, `` `define ``, comments) |
| `02xx` | lexer and parser (syntax) |
| `03xx` | elaboration (names, declarations, types) |
| `04xx` | code generation |
| `05xx` | netlist |
| `06xx` | numerical: DC, transient, AC, noise |
| `07xx` | files and the command line |

**Codes are permanent.** A released code is never renumbered and never reused for another
problem, because users search for it and translations are keyed to it. New codes are assigned
by the developers who add the check (in `crates/va-diag/src/codes.rs`), together with the first
English entry.

`E0000` deserves a note. It is shown for any error that has not been given a code yet, with the
program's own wording. Each `E0000` a user meets is a gap to close, so if you see one in a report,
pass it on.

## Writing good entries

- **message:** one line, no full stop, starting lower case. Name what the user wrote, using the
  placeholders. It is read hundreds of times in a long log, so keep it short.
- **description:** say what went wrong and why it is a problem. If the program refused to guess
  at a meaning, say what the wrong guess would have been.
- **actions:** concrete steps, best first. Show the corrected source where you can
  (`` `real x;` ``). End with a fallback (where to report it) only when the first actions may not
  apply.
- Write for someone who knows circuits but has not read this program's source.

## Adding a language

1. Create `messages/<language>/`, using the two-letter code (`fr`, `de`, `it`, `es`, `zh`, …).
2. Copy the English files you are translating, translate `message`, `description` and
   `actions`, replace `"rev": N` with `"source_rev": N` (the same N), and keep `code` and every
   placeholder.
3. Untranslated codes fall back to English automatically, so you can translate a few at a time.

Users choose the language with `VA_LANG=fr`, or through their system locale (`LANG=fr_FR.UTF-8`).
When English changes later (its `rev` goes up), the translation is still shown, with a note that
it was made from an older English text, until `source_rev` is brought up to date.

The words around the entries ("error", "What it means", "What to do", the `--explain` hint) are
English in every language for now. Translating them comes with the first translation.

## Trying a change without rebuilding

The English catalogue is built into `va-cli`, so the program works even without this directory.
To see edited text without rebuilding, point `VA_MESSAGES_DIR` at a copy of this directory:

```
VA_MESSAGES_DIR=path/to/messages va-cli --explain E0703
VA_MESSAGES_DIR=path/to/messages VA_LANG=fr va-cli --explain E0703
```

A file that does not parse is reported as a warning, and the built-in text is used instead.

## Checking your changes

```
cargo test -p va-diag
```

This checks every file: that it parses with exactly the fields above, that its name is its code,
that every code the program defines has English text and every English file is a code the
program defines, and that every translation has a `source_rev` and uses exactly the English
entry's placeholders. Out-of-date translations are listed, not failed.
