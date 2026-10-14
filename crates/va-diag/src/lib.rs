//! Coded diagnostics: every error, warning, refusal and user-facing message carries a code
//! (`E0301`, `W0301`, `R0402`, `C0601`; see [`codes`]), and its text comes from a catalogue of
//! JSON files instead of from the Rust source.
//!
//! # Why a catalogue
//!
//! The code that detects a problem knows *what* happened and the specific names involved. What
//! the problem *means* and what the user should *do* are written better, and translated, by people
//! who do not edit Rust. So a [`Diagnostic`] carries only a [`Code`] and its arguments
//! (`name = "Rx"`, an `at` location), and the text lives in `messages/<lang>/<CODE>.json`:
//!
//! ```json
//! {
//!   "code": "E0703",
//!   "rev": 1,
//!   "message": "invalid value `{value}` for {option}; expected {expected}",
//!   "description": "What the problem is, in a few sentences.",
//!   "actions": ["What to try first.", "What to try next."]
//! }
//! ```
//!
//! `message` is the one-line form shown with every occurrence; `{name}` placeholders are filled
//! from the diagnostic's arguments. `description` and `actions` are shown by
//! `va-cli --explain <CODE>`. The format is documented for the people who write it in
//! `messages/README.md`.
//!
//! # Finding the text
//!
//! [`Catalogue::from_env`] starts from the English catalogue embedded at build time, so a binary
//! copied on its own still explains itself, then overlays `VA_MESSAGES_DIR` if it is set (to try
//! edited text without rebuilding). The language comes from `VA_LANG`, else the locale
//! (`LC_ALL`, `LC_MESSAGES`, `LANG`), else English. A code is looked up in that language, then in
//! English; with neither, the code and the raw detail are shown. Rendering never fails.
//!
//! # Limitations
//!
//! The words around the catalogue text (`error`, `warning`, "What it means", "What to do", the
//! `--explain` hint) are English for every language; translating them belongs with the first
//! translation. Plural forms are not modelled: a message with a count must be phrased so it reads
//! correctly for any number (`port(s)`, or "declares N, passes M").

#![forbid(unsafe_code)]

pub mod codes;

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// A diagnostic code: a kind letter and four digits, such as `E0301`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Code(&'static str);

impl Code {
    /// The code spelled `s`. Only [`codes`] creates these; `catalogue_is_consistent` checks that
    /// each one is well formed and has catalogue text.
    pub const fn new(s: &'static str) -> Code {
        Code(s)
    }

    /// The code as written, e.g. `"E0301"`.
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// What kind of diagnostic this code is, from its letter.
    pub fn kind(&self) -> Kind {
        Kind::of(self.0).unwrap_or(Kind::Error)
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// The kind of a diagnostic, given by its code's letter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// `E`: the input is wrong, or the run failed.
    Error,
    /// `W`: the run goes on, but something deserves a look.
    Warning,
    /// `R`: valid Verilog-A this simulator deliberately declines to run (CLAUDE.md §5).
    Refusal,
    /// `C`: information a user may act on.
    Communication,
}

impl Kind {
    /// The kind a code string's letter names, if it is one.
    pub fn of(code: &str) -> Option<Kind> {
        Some(match code.chars().next()? {
            'E' => Kind::Error,
            'W' => Kind::Warning,
            'R' => Kind::Refusal,
            'C' => Kind::Communication,
            _ => return None,
        })
    }

    /// The word that opens a rendered diagnostic of this kind.
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Error => "error",
            Kind::Warning => "warning",
            Kind::Refusal => "refused",
            Kind::Communication => "note",
        }
    }
}

/// Whether `s` is a well-formed code: one of `E`, `W`, `R`, `C`, then exactly four digits.
pub fn is_well_formed(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('E' | 'W' | 'R' | 'C'))
        && s.len() == 5
        && chars.all(|c| c.is_ascii_digit())
}

/// One occurrence of a coded problem: the code, the values its message mentions, and where.
///
/// It is an [`std::error::Error`], so it travels through `?` and `anyhow` like any other error,
/// and the program's top level finds it in the error chain to render it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    code: Code,
    args: Vec<(String, String)>,
    at: Option<String>,
}

impl Diagnostic {
    /// A diagnostic with code `code` and no arguments yet.
    pub fn new(code: Code) -> Diagnostic {
        Diagnostic {
            code,
            args: Vec::new(),
            at: None,
        }
    }

    /// An `E0000`: an error with no code yet, carrying its uncoded text as `detail`.
    pub fn unknown(detail: impl ToString) -> Diagnostic {
        Diagnostic::new(codes::E0000).arg("detail", detail)
    }

    /// This diagnostic, with `{name}` in the message filled by `value`.
    pub fn arg(mut self, name: &str, value: impl ToString) -> Diagnostic {
        self.args.push((name.to_string(), value.to_string()));
        self
    }

    /// This diagnostic, located at `at` (a `file:line:col`).
    pub fn at(mut self, at: impl Into<String>) -> Diagnostic {
        self.at = Some(at.into());
        self
    }

    /// The code.
    pub fn code(&self) -> Code {
        self.code
    }

    /// The arguments, in the order they were given.
    pub fn args(&self) -> &[(String, String)] {
        &self.args
    }

    /// Where it happened, if known.
    pub fn location(&self) -> Option<&str> {
        self.at.as_deref()
    }
}

impl std::fmt::Display for Diagnostic {
    /// The one-line form, from the process-wide catalogue ([`Catalogue::global`]).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Catalogue::global().headline(self))
    }
}

impl std::error::Error for Diagnostic {}

/// One catalogue file: the text for one code in one language.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The code; must equal the file's name.
    pub code: String,
    /// English only: the revision of this text, raised whenever its meaning changes, so a
    /// translation can say which one it was made from.
    #[serde(default)]
    pub rev: Option<u32>,
    /// Translations only: the English `rev` this translation was made from. Lower than the
    /// English `rev` means the translation is out of date.
    #[serde(default)]
    pub source_rev: Option<u32>,
    /// The one-line message shown with every occurrence, with `{name}` placeholders.
    pub message: String,
    /// What the problem means, shown by `--explain`.
    pub description: String,
    /// What to do about it, in the order to try them, shown by `--explain`.
    pub actions: Vec<String>,
    /// Set when the problem can no longer happen; the code stays reserved.
    #[serde(default)]
    pub retired: bool,
}

/// The message text for every code, in every language that has some.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    /// The preferred language.
    lang: String,
    /// language -> code -> entry.
    entries: HashMap<String, HashMap<String, Entry>>,
    /// Problems met while loading (a file that does not parse, a name that does not match its
    /// code). Loading never fails; the caller decides whether to show these.
    load_problems: Vec<String>,
}

impl Catalogue {
    /// The catalogue embedded at build time, in English.
    pub fn embedded() -> Catalogue {
        let mut c = Catalogue {
            lang: "en".to_string(),
            ..Catalogue::default()
        };
        for (lang, text) in EMBEDDED {
            c.add_text(lang, text, &format!("embedded {lang} catalogue"));
        }
        c
    }

    /// The embedded catalogue, overlaid with `VA_MESSAGES_DIR` and set to the language the
    /// environment asks for (see the crate documentation).
    pub fn from_env() -> Catalogue {
        let mut c = Catalogue::embedded();
        if let Some(dir) = std::env::var_os("VA_MESSAGES_DIR") {
            c.overlay_dir(Path::new(&dir));
        }
        c.lang = detect_language();
        c
    }

    /// The catalogue every [`Diagnostic`]'s `Display` uses: [`Catalogue::from_env`], read once.
    pub fn global() -> &'static Catalogue {
        static GLOBAL: std::sync::OnceLock<Catalogue> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(Catalogue::from_env)
    }

    /// This catalogue, preferring `lang`.
    pub fn with_language(mut self, lang: &str) -> Catalogue {
        self.lang = lang.to_string();
        self
    }

    /// The preferred language.
    pub fn language(&self) -> &str {
        &self.lang
    }

    /// Problems met while loading.
    pub fn load_problems(&self) -> &[String] {
        &self.load_problems
    }

    /// Add every `<lang>/<CODE>.json` under `dir`, replacing entries already present.
    pub fn overlay_dir(&mut self, dir: &Path) {
        let Ok(langs) = std::fs::read_dir(dir) else {
            self.load_problems.push(format!(
                "cannot read the message directory {}",
                dir.display()
            ));
            return;
        };
        for lang in langs.flatten() {
            if !lang.path().is_dir() {
                continue;
            }
            let lang_name = lang.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(lang.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                        let before = self.load_problems.len();
                        let added = self.add_text(&lang_name, &text, &path.display().to_string());
                        if let (Some(code), Some(stem)) = (added, stem) {
                            if code != stem && self.load_problems.len() == before {
                                self.load_problems.push(format!(
                                    "{}: the file is named `{stem}` but holds code `{code}`",
                                    path.display()
                                ));
                            }
                        }
                    }
                    Err(e) => self
                        .load_problems
                        .push(format!("{}: cannot read: {e}", path.display())),
                }
            }
        }
    }

    /// Parse one file's text and add it under `lang`; returns its code if it parsed.
    fn add_text(&mut self, lang: &str, text: &str, origin: &str) -> Option<String> {
        match serde_json::from_str::<Entry>(text) {
            Ok(entry) => {
                let code = entry.code.clone();
                self.entries
                    .entry(lang.to_string())
                    .or_default()
                    .insert(code.clone(), entry);
                Some(code)
            }
            Err(e) => {
                self.load_problems.push(format!("{origin}: {e}"));
                None
            }
        }
    }

    /// The text for `code`: in the preferred language, else in English. The flag is true when
    /// the text is a translation made from an older English revision than the current one.
    pub fn lookup(&self, code: &str) -> Option<(&Entry, bool)> {
        let english = self.entries.get("en").and_then(|m| m.get(code));
        let preferred = self.entries.get(&self.lang).and_then(|m| m.get(code));
        match (preferred, english) {
            (Some(t), Some(en)) if self.lang != "en" => {
                let stale = matches!((t.source_rev, en.rev), (Some(s), Some(r)) if s < r)
                    || t.source_rev.is_none();
                Some((t, stale))
            }
            (Some(t), _) => Some((t, false)),
            (None, Some(en)) => Some((en, false)),
            (None, None) => None,
        }
    }

    /// The one-line form: `error[E0703]: invalid value …`, or with a location
    /// `error[E0301] at f.va:7:31: …`.
    pub fn headline(&self, d: &Diagnostic) -> String {
        let code = d.code.as_str();
        let message = match self.lookup(code) {
            Some((entry, _)) => fill(&entry.message, &d.args),
            // No text anywhere: say what is known rather than nothing.
            None => match d.args.iter().find(|(k, _)| k == "detail") {
                Some((_, detail)) => detail.clone(),
                None => "(this code has no text in the message catalogue)".to_string(),
            },
        };
        let at =
            d.at.as_deref()
                .map_or_else(String::new, |a| format!(" at {a}"));
        format!("{}[{code}]{at}: {message}", d.code.kind().label())
    }

    /// The full rendering for one occurrence: the headline and a pointer to `--explain`.
    pub fn render(&self, d: &Diagnostic) -> String {
        format!(
            "{}\n  = for what this means and what to do: va-cli --explain {}",
            self.headline(d),
            d.code
        )
    }

    /// The `--explain` text for `code`, or `None` if no catalogue has it.
    pub fn explain(&self, code: &str) -> Option<String> {
        let (entry, stale) = self.lookup(code)?;
        let kind = Kind::of(code).map_or("diagnostic", |k| k.label());
        let mut out = format!("{code} ({kind})\n{}\n", entry.message);
        if entry.retired {
            out.push_str("\nThis code is retired: the problem it named can no longer happen.\n");
        }
        if stale {
            out.push_str(
                "\n(This translation was made from an older English text; \
                 `VA_LANG=en` shows the current one.)\n",
            );
        }
        out.push_str("\nWhat it means:\n");
        out.push_str(&indent(&entry.description));
        if !entry.actions.is_empty() {
            out.push_str("\nWhat to do:\n");
            for (i, action) in entry.actions.iter().enumerate() {
                out.push_str(&format!("  {}. {action}\n", i + 1));
            }
        }
        Some(out)
    }

    /// Every code with English text, sorted.
    pub fn english_codes(&self) -> BTreeSet<String> {
        self.entries
            .get("en")
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// `text` indented two spaces, ending in a newline.
fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

/// `template` with each `{name}` replaced by the matching argument. A placeholder with no
/// argument is left as written, so a missing argument shows instead of silently vanishing.
pub fn fill(template: &str, args: &[(String, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder_name(&after[..close]) => {
                let name = &after[..close];
                match args.iter().find(|(k, _)| k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(&rest[open..open + close + 2]),
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The placeholder names `template` uses.
pub fn placeholders(template: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder_name(&after[..close]) => {
                out.insert(after[..close].to_string());
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

fn is_placeholder_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The language the environment asks for: `VA_LANG`, else the locale's language
/// (`fr_FR.UTF-8` -> `fr`), else `en`.
pub fn detect_language() -> String {
    let from = |var: &str| {
        std::env::var(var)
            .ok()
            .map(|v| {
                v.split(['_', '.', '@'])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
            })
            .filter(|v| !v.is_empty() && v != "c" && v != "posix")
    };
    from("VA_LANG")
        .or_else(|| from("LC_ALL"))
        .or_else(|| from("LC_MESSAGES"))
        .or_else(|| from("LANG"))
        .unwrap_or_else(|| "en".to_string())
}

/// Check a catalogue directory against the codes the program defines. Returns one line per
/// problem; empty means consistent. This is what the `catalogue_is_consistent` test runs, and
/// what a team editing the text can run before sending a change.
///
/// Checked: every file parses with exactly the documented fields; its name is its code; the code
/// is well formed; message, description and at least one action are present; English carries a
/// `rev`; every code in [`codes::ALL`] has English text and every English file is such a code; a
/// translation carries a `source_rev`, names a code English has, and uses exactly English's
/// placeholders. An out-of-date translation is not a problem (it is shown with a note), so it is
/// reported by [`stale_translations`] instead.
pub fn validate(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let mut by_lang: BTreeMap<String, BTreeMap<String, Entry>> = BTreeMap::new();
    let Ok(langs) = std::fs::read_dir(root) else {
        return vec![format!("cannot read {}", root.display())];
    };
    for lang in langs.flatten() {
        if !lang.path().is_dir() {
            continue;
        }
        let lang_name = lang.file_name().to_string_lossy().into_owned();
        let Ok(files) = std::fs::read_dir(lang.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let shown = format!("{lang_name}/{}", file.file_name().to_string_lossy());
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    problems.push(format!("{shown}: cannot read: {e}"));
                    continue;
                }
            };
            let entry: Entry = match serde_json::from_str(&text) {
                Ok(e) => e,
                Err(e) => {
                    problems.push(format!("{shown}: {e}"));
                    continue;
                }
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if entry.code != stem {
                problems.push(format!("{shown}: holds code `{}`", entry.code));
            }
            if !is_well_formed(&entry.code) {
                problems.push(format!(
                    "{shown}: `{}` is not a code (a letter E, W, R or C, then four digits)",
                    entry.code
                ));
            }
            for (field, value) in [
                ("message", &entry.message),
                ("description", &entry.description),
            ] {
                if value.trim().is_empty() {
                    problems.push(format!("{shown}: `{field}` is empty"));
                }
            }
            if entry.actions.iter().all(|a| a.trim().is_empty()) {
                problems.push(format!("{shown}: `actions` needs at least one entry"));
            }
            by_lang
                .entry(lang_name.clone())
                .or_default()
                .insert(stem, entry);
        }
    }

    let english = by_lang.get("en").cloned().unwrap_or_default();
    let defined: BTreeSet<&str> = codes::ALL.iter().map(|c| c.as_str()).collect();
    for code in &defined {
        if !english.contains_key(*code) {
            problems.push(format!(
                "en/{code}.json is missing: `{code}` is defined in codes.rs"
            ));
        }
    }
    for (code, entry) in &english {
        if !defined.contains(code.as_str()) {
            problems.push(format!(
                "en/{code}.json: `{code}` is not defined in codes.rs"
            ));
        }
        if entry.rev.is_none() {
            problems.push(format!("en/{code}.json: English text needs a `rev`"));
        }
        if entry.source_rev.is_some() {
            problems.push(format!(
                "en/{code}.json: `source_rev` is for translations only"
            ));
        }
    }
    for (lang, entries) in &by_lang {
        if lang == "en" {
            continue;
        }
        for (code, entry) in entries {
            let Some(en) = english.get(code) else {
                problems.push(format!("{lang}/{code}.json: no English text for `{code}`"));
                continue;
            };
            if entry.source_rev.is_none() {
                problems.push(format!(
                    "{lang}/{code}.json: a translation needs `source_rev` (the English `rev` it \
                     was made from)"
                ));
            }
            let (want, got) = (placeholders(&en.message), placeholders(&entry.message));
            if want != got {
                problems.push(format!(
                    "{lang}/{code}.json: the message uses placeholders {got:?}, but English uses \
                     {want:?}"
                ));
            }
        }
    }
    problems
}

/// Translations made from an older English text than the current one, as `lang/CODE`.
pub fn stale_translations(root: &Path) -> Vec<String> {
    let mut cat = Catalogue::default();
    cat.overlay_dir(root);
    let mut out = Vec::new();
    for (lang, entries) in &cat.entries {
        if lang == "en" {
            continue;
        }
        for code in entries.keys() {
            if let Some((_, true)) = cat.clone().with_language(lang).lookup(code) {
                out.push(format!("{lang}/{code}"));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_messages() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../messages")
    }

    /// The repository's catalogue and `codes.rs` agree, and every file is well formed. This is
    /// the test a team editing the text runs (`cargo test -p va-diag`).
    #[test]
    fn catalogue_is_consistent() {
        let problems = validate(&repo_messages());
        assert!(
            problems.is_empty(),
            "catalogue problems:\n  {}",
            problems.join("\n  ")
        );
        let stale = stale_translations(&repo_messages());
        if !stale.is_empty() {
            println!(
                "translations older than their English text: {}",
                stale.join(", ")
            );
        }
    }

    /// The embedded catalogue holds every defined code, so a binary copied without the
    /// `messages/` directory still renders and explains every one.
    #[test]
    fn every_code_is_embedded() {
        let cat = Catalogue::embedded();
        assert!(cat.load_problems().is_empty(), "{:?}", cat.load_problems());
        for code in codes::ALL {
            assert!(
                cat.lookup(code.as_str()).is_some(),
                "{code} is not embedded"
            );
            assert!(
                cat.explain(code.as_str()).is_some(),
                "{code} does not explain"
            );
        }
    }

    #[test]
    fn codes_are_well_formed_and_unique() {
        let mut seen = BTreeSet::new();
        for code in codes::ALL {
            assert!(is_well_formed(code.as_str()), "{code}");
            assert!(seen.insert(code.as_str()), "{code} is listed twice");
        }
        assert!(is_well_formed("R0402") && is_well_formed("C0601"));
        for bad in ["E301", "E03010", "X0301", "e0301", "E03a1"] {
            assert!(!is_well_formed(bad), "{bad}");
        }
    }

    #[test]
    fn placeholders_are_filled_and_a_missing_one_stays_visible() {
        let args = vec![("name".to_string(), "Rx".to_string())];
        assert_eq!(fill("unknown `{name}`", &args), "unknown `Rx`");
        assert_eq!(fill("{name} and {other}", &args), "Rx and {other}");
        assert_eq!(
            fill("a {not a placeholder} {", &args),
            "a {not a placeholder} {"
        );
        assert_eq!(
            placeholders("`{value}` for {option}; {expected}"),
            ["expected", "option", "value"].map(String::from).into()
        );
    }

    #[test]
    fn a_seeded_code_renders_with_its_arguments() {
        let d = Diagnostic::new(codes::E0703)
            .arg("option", "--solver")
            .arg("value", "fast")
            .arg("expected", "`auto`, `dense` or `sparse`");
        let line = Catalogue::embedded().headline(&d);
        assert!(line.starts_with("error[E0703]: "), "{line}");
        assert!(
            line.contains("`fast`") && line.contains("--solver"),
            "{line}"
        );
        assert!(!line.contains('{'), "an argument was not filled: {line}");
    }

    /// A translation is used when present, English when not, and the bare detail when no
    /// language has the code. An out-of-date translation is marked.
    #[test]
    fn languages_fall_back_to_english_then_to_the_detail() {
        let dir = std::env::temp_dir().join(format!("va_diag_fallback_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("en")).unwrap();
        std::fs::create_dir_all(dir.join("fr")).unwrap();
        let en = |code: &str, rev: u32| {
            format!(
                r#"{{"code":"{code}","rev":{rev},"message":"bad `{{name}}`","description":"d","actions":["a"]}}"#
            )
        };
        std::fs::write(dir.join("en/E0701.json"), en("E0701", 2)).unwrap();
        std::fs::write(dir.join("en/E0702.json"), en("E0702", 1)).unwrap();
        std::fs::write(
            dir.join("fr/E0701.json"),
            r#"{"code":"E0701","source_rev":1,"message":"mauvais `{name}`","description":"d","actions":["a"]}"#,
        )
        .unwrap();
        let mut cat = Catalogue::default();
        cat.overlay_dir(&dir);
        assert!(cat.load_problems().is_empty(), "{:?}", cat.load_problems());
        let cat = cat.with_language("fr");

        let d = Diagnostic::new(codes::E0701).arg("name", "x");
        assert_eq!(cat.headline(&d), "error[E0701]: mauvais `x`");
        assert!(cat.explain("E0701").unwrap().contains("older English text"));
        let d = Diagnostic::new(codes::E0702).arg("name", "y");
        assert_eq!(
            cat.headline(&d),
            "error[E0702]: bad `y`",
            "falls back to English"
        );
        let d = Diagnostic::unknown("disk on fire");
        assert_eq!(
            cat.headline(&d),
            "error[E0000]: disk on fire",
            "no text anywhere"
        );
        assert_eq!(stale_translations(&dir), vec!["fr/E0701".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The checks a text-editing team relies on catch what they are meant to.
    #[test]
    fn validation_catches_broken_files() {
        let dir = std::env::temp_dir().join(format!("va_diag_validate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("en")).unwrap();
        std::fs::create_dir_all(dir.join("de")).unwrap();
        for code in codes::ALL {
            std::fs::write(
                dir.join(format!("en/{code}.json")),
                format!(
                    r#"{{"code":"{code}","rev":1,"message":"m {{value}}","description":"d","actions":["a"]}}"#
                ),
            )
            .unwrap();
        }
        assert!(validate(&dir).is_empty(), "{:?}", validate(&dir));
        std::fs::write(dir.join("en/E9999.json"), r#"{"code":"E9998"}"#).unwrap();
        std::fs::write(
            dir.join("de/E0703.json"),
            r#"{"code":"E0703","message":"m {wert}","description":"d","actions":["a"]}"#,
        )
        .unwrap();
        let problems = validate(&dir).join("\n");
        assert!(problems.contains("en/E9999.json"), "{problems}");
        assert!(problems.contains("needs `source_rev`"), "{problems}");
        assert!(problems.contains("placeholders"), "{problems}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
