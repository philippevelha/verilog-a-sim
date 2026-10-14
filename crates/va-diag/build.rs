//! Embed the message catalogue (`messages/<lang>/<CODE>.json` at the repository root) into the
//! crate, so a `va-cli` binary copied on its own still explains its codes. `VA_MESSAGES_DIR`
//! overrides it at run time (see `Catalogue::from_env`).

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../messages");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut entries = Vec::new();
    if let Ok(langs) = std::fs::read_dir(&root) {
        for lang in langs.flatten() {
            let lang_path = lang.path();
            if !lang_path.is_dir() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", lang_path.display());
            let lang_name = lang.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(&lang_path) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "json") {
                    println!("cargo:rerun-if-changed={}", path.display());
                    let abs = path.canonicalize().expect("catalogue file path");
                    entries.push((lang_name.clone(), abs));
                }
            }
        }
    }
    entries.sort();
    let mut out = String::from(
        "/// Every catalogue file found at build time: `(language, file contents)`.\n",
    );
    out.push_str("pub(crate) static EMBEDDED: &[(&str, &str)] = &[\n");
    for (lang, path) in &entries {
        writeln!(
            out,
            "    ({lang:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("write to String");
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("embedded.rs");
    std::fs::write(dest, out).expect("write embedded catalogue");
}
