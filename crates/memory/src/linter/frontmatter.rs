//! YAML frontmatter parsing helpers shared by every kind.

use serde::de::DeserializeOwned;

use crate::error::LintError;

/// Strict YAML deserialization that maps serde errors into the linter's
/// `MissingField` / `InvalidField` / `MalformedFrontmatter` taxonomy
/// when possible.
pub fn deserialize_strict<F: DeserializeOwned>(yaml: &str) -> Result<F, LintError> {
    serde_yaml::from_str::<F>(yaml).map_err(map_serde_error)
}

fn map_serde_error(err: serde_yaml::Error) -> LintError {
    let msg = err.to_string();

    // `missing field \`X\`` is the exact pattern serde uses for missing
    // required fields. Hoist into the typed variant so the LLM sees a
    // crisp message it can act on.
    if let Some(field) = parse_missing_field(&msg) {
        return LintError::MissingField(field);
    }
    if let Some((field, message)) = parse_invalid_status(&msg) {
        if field == "status" {
            return LintError::InvalidStatus(message);
        }
        return LintError::InvalidField { field, message };
    }
    LintError::MalformedFrontmatter(msg)
}

fn parse_missing_field(msg: &str) -> Option<&'static str> {
    let needle = "missing field `";
    let start = msg.find(needle)? + needle.len();
    let end = msg[start..].find('`')? + start;
    let field_name = &msg[start..end];
    static FIELDS: &[&str] = &[
        "created_at",
        "updated_at",
        "sources",
        "status",
        "kind",
        "description",
        "model_invokation",
        "user_invocable",
        "last_sources",
        "auto_invoke",
        "requires",
    ];
    FIELDS.iter().copied().find(|n| *n == field_name)
}

fn parse_invalid_status(msg: &str) -> Option<(&'static str, String)> {
    // serde renders enum failures as: "unknown variant `Foo`, expected one of ..."
    // We can't reliably attribute it to a specific field from the message
    // alone, so we conservatively label it as `status` only when the
    // message mentions one of the DecisionStatus variants in the
    // expected set.
    if msg.contains("unknown variant") && msg.contains("`open`") {
        let needle = "unknown variant `";
        let start = msg.find(needle)? + needle.len();
        let end = msg[start..].find('`')? + start;
        let bad = msg[start..end].to_string();
        return Some(("status", bad));
    }
    None
}
