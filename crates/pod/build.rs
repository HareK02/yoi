//! Emits `$OUT_DIR/internal_keys.rs` containing the sorted list of keys
//! present in `resources/prompts/internal.toml`. The generated slice is
//! included into `src/prompts.rs` where a `const _` assertion compares
//! it bidirectionally against the `PodPrompt` enum's own key list, so
//! that a mismatch fails the build (see ticket: pod-prompt-catalog).
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let toml_path = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("resources")
        .join("prompts")
        .join("internal.toml");

    println!("cargo:rerun-if-changed={}", toml_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let toml_str = fs::read_to_string(&toml_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", toml_path.display()));

    let parsed: toml::Value = toml::from_str(&toml_str)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", toml_path.display()));

    let prompt_section = parsed
        .get("prompt")
        .and_then(|v| v.as_table())
        .unwrap_or_else(|| panic!("{} must contain a `[prompt]` table", toml_path.display()));

    let mut keys: Vec<String> = prompt_section.keys().cloned().collect();
    keys.sort();

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let out_path = PathBuf::from(out_dir).join("internal_keys.rs");

    let mut code = String::from("pub(crate) const INTERNAL_KEYS: &[&str] = &[\n");
    for k in &keys {
        code.push_str("    ");
        code.push_str(&format!("{k:?}"));
        code.push_str(",\n");
    }
    code.push_str("];\n");

    fs::write(&out_path, code)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
}
