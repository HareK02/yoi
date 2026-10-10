use config_source::{
    ConfigSchemaContribution, ConfigTreeChange, ConfigTreeSnapshot, EvaluationResult,
    SnapshotEnvironment, ToolchainContract, VirtualPath, WorkspaceConfigSchemaBundle,
};
use serde_wasm_bindgen::{Serializer, from_value};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

thread_local! {
    static SESSION: RefCell<Option<ConfigTreeSnapshot>> = const { RefCell::new(None) };
    static SCHEMA_BUNDLE: RefCell<Option<WorkspaceConfigSchemaBundle>> = const { RefCell::new(None) };
}

#[wasm_bindgen]
pub fn compose_schema_bundle(contributions: JsValue) -> Result<JsValue, JsValue> {
    let contributions: Vec<ConfigSchemaContribution> = decode(contributions)?;
    encode(WorkspaceConfigSchemaBundle::compose(contributions).map_err(js_error)?)
}

#[wasm_bindgen]
pub fn set_snapshot(snapshot: JsValue) -> Result<(), JsValue> {
    let snapshot: ConfigTreeSnapshot = decode(snapshot)?;
    SESSION.with(|session| session.replace(Some(snapshot)));
    Ok(())
}

#[wasm_bindgen]
pub fn set_schema_bundle(schema_bundle: JsValue) -> Result<(), JsValue> {
    let schema_bundle: WorkspaceConfigSchemaBundle = decode(schema_bundle)?;
    SCHEMA_BUNDLE.with(|session| session.replace(Some(schema_bundle)));
    Ok(())
}

#[wasm_bindgen]
pub fn apply_changes(changes: JsValue) -> Result<JsValue, JsValue> {
    let changes: Vec<ConfigTreeChange> = decode(changes)?;
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        let snapshot = session
            .as_ref()
            .ok_or_else(|| JsValue::from_str("config source snapshot is not initialized"))?
            .apply(&changes)
            .map_err(js_error)?;
        *session = Some(snapshot.clone());
        encode(snapshot)
    })
}

#[wasm_bindgen]
pub fn changes_between(base: JsValue, candidate: JsValue) -> Result<JsValue, JsValue> {
    let base: ConfigTreeSnapshot = decode(base)?;
    let candidate: ConfigTreeSnapshot = decode(candidate)?;
    encode(base.changes_to(&candidate))
}

#[wasm_bindgen]
pub fn evaluate_current(contract: JsValue) -> Result<JsValue, JsValue> {
    let contract: ToolchainContract = decode(contract)?;
    SESSION.with(|session| {
        let session = session.borrow();
        let snapshot = session
            .as_ref()
            .ok_or_else(|| JsValue::from_str("config source snapshot is not initialized"))?;
        encode(
            SnapshotEnvironment::new(snapshot.clone())
                .evaluate_contract(&contract)
                .map_err(|diagnostics| {
                    encode(&diagnostics).unwrap_or_else(|_| JsValue::from_str("evaluation failed"))
                })?,
        )
    })
}

#[derive(serde::Serialize)]
struct WasmCompletionResult {
    from: usize,
    items: Vec<WasmCompletionItem>,
}

#[derive(serde::Serialize)]
struct WasmCompletionItem {
    label: String,
    kind: String,
    detail: Option<String>,
    priority: i32,
}

#[wasm_bindgen]
pub fn complete_current(
    entrypoint: String,
    source: String,
    utf16_offset: usize,
    explicit: bool,
) -> Result<JsValue, JsValue> {
    let entrypoint = VirtualPath::parse(entrypoint).map_err(js_error)?;
    SESSION.with(|session| {
        let session = session.borrow();
        let snapshot = session
            .as_ref()
            .ok_or_else(|| JsValue::from_str("config source snapshot is not initialized"))?;
        encode(complete_snapshot(
            snapshot,
            &entrypoint,
            &source,
            utf16_offset,
            explicit,
        )?)
    })
}

fn complete_snapshot(
    snapshot: &ConfigTreeSnapshot,
    entrypoint: &VirtualPath,
    source: &str,
    utf16_offset: usize,
    explicit: bool,
) -> Result<Option<WasmCompletionResult>, JsValue> {
    let utf8_byte_offset = utf16_to_utf8_offset(source, utf16_offset)?;
    let result = session_environment(snapshot.clone())
        .complete_config(entrypoint, source, utf8_byte_offset, explicit)
        .map_err(|error| JsValue::from_str(&format!("{error:?}")))?;
    let result = result
        .map(|result| {
            Ok::<WasmCompletionResult, JsValue>(WasmCompletionResult {
                from: utf8_to_utf16_offset(source, result.from)?,
                items: result
                    .items
                    .into_iter()
                    .map(|item| WasmCompletionItem {
                        label: item.label,
                        kind: format!("{:?}", item.kind).to_lowercase(),
                        detail: item.detail,
                        priority: item.priority,
                    })
                    .collect(),
            })
        })
        .transpose()?;
    Ok(result)
}

#[wasm_bindgen]
pub fn evaluate_snapshot(snapshot: JsValue, contract: JsValue) -> Result<JsValue, JsValue> {
    let snapshot: ConfigTreeSnapshot = decode(snapshot)?;
    let contract: ToolchainContract = decode(contract)?;
    encode(
        SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .map_err(|diagnostics| {
                encode(&diagnostics).unwrap_or_else(|_| JsValue::from_str("evaluation failed"))
            })?,
    )
}

#[wasm_bindgen]
pub fn analyze_snapshot(
    snapshot: JsValue,
    entrypoint: String,
    source_override: Option<String>,
) -> Result<JsValue, JsValue> {
    let snapshot: ConfigTreeSnapshot = decode(snapshot)?;
    let entrypoint = VirtualPath::parse(entrypoint).map_err(js_error)?;
    encode(session_environment(snapshot).analyze(&entrypoint, source_override.as_deref()))
}

fn session_environment(snapshot: ConfigTreeSnapshot) -> SnapshotEnvironment {
    let schema_bundle = SCHEMA_BUNDLE.with(|schema_bundle| schema_bundle.borrow().clone());
    let mut environment = SnapshotEnvironment::new(snapshot);
    if let Some(schema_bundle) = schema_bundle {
        environment = environment.with_schema_bundle(schema_bundle);
    }
    environment
}

#[wasm_bindgen]
pub fn format_source(source: String) -> Result<String, JsValue> {
    SnapshotEnvironment::new(ConfigTreeSnapshot::empty())
        .format(&source)
        .map_err(|error| JsValue::from_str(&error))
}

fn utf16_to_utf8_offset(source: &str, utf16_offset: usize) -> Result<usize, JsValue> {
    let mut units = 0usize;
    for (byte_offset, character) in source.char_indices() {
        if units == utf16_offset {
            return Ok(byte_offset);
        }
        units += character.len_utf16();
        if units > utf16_offset {
            return Err(JsValue::from_str("UTF-16 offset splits a surrogate pair"));
        }
    }
    if units == utf16_offset {
        Ok(source.len())
    } else {
        Err(JsValue::from_str("UTF-16 offset is outside the source"))
    }
}

fn utf8_to_utf16_offset(source: &str, utf8_offset: usize) -> Result<usize, JsValue> {
    if utf8_offset > source.len() {
        return Err(JsValue::from_str("UTF-8 offset is outside the source"));
    }
    if !source.is_char_boundary(utf8_offset) {
        return Err(JsValue::from_str("UTF-8 offset splits a character"));
    }
    Ok(source[..utf8_offset].encode_utf16().count())
}

fn decode<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    from_value(value).map_err(|error| JsValue::from_str(&error.to_string()))
}

fn encode<T: serde::Serialize>(value: T) -> Result<JsValue, JsValue> {
    value
        .serialize(&Serializer::json_compatible())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[allow(dead_code)]
fn _assert_serializable(_: EvaluationResult) {}

#[cfg(test)]
mod tests {
    use super::*;
    use config_source::{ConfigContentType, ConfigEntry};

    fn snapshot() -> ConfigTreeSnapshot {
        ConfigTreeSnapshot::from_entries([
            ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                "{}",
            )
            .unwrap(),
            // A same-suffix workspace file must not be an unknown builtin fallback.
            ConfigEntry::new(
                VirtualPath::parse("profiles/missing.dcdl").unwrap(),
                ConfigContentType::Decodal,
                "{}",
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn wasm_analysis_uses_applied_tree_content_identity() {
        let base = snapshot();
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let change = ConfigTreeChange::Update {
            path: path.clone(),
            expected_digest: base.entries[&path].content_digest.clone(),
            content: "{ broken = ; }".into(),
        };
        let changed = base.apply(&[change]).unwrap();
        let diagnostics = session_environment(changed.clone()).analyze(&path, None);
        assert!(!diagnostics.is_empty());
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.tree_digest == changed.digest)
        );
        assert_ne!(changed.digest, base.digest);
        let restored = changed.apply(&changed.changes_to(&base)).unwrap();
        assert_eq!(restored.digest, base.digest);
        assert!(
            session_environment(restored)
                .analyze(&path, None)
                .is_empty()
        );
    }

    #[test]
    fn wasm_completion_boundary_preserves_read_only_builtin_detail() {
        let snapshot = snapshot();
        let source = "let 名 = 1; import \"$builtin/profiles/comp\"";
        let cursor = source.encode_utf16().count() - 1;
        let result = complete_snapshot(
            &snapshot,
            &VirtualPath::parse("main.dcdl").unwrap(),
            source,
            cursor,
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.from, "let 名 = 1; import \"".encode_utf16().count());
        let item = result
            .items
            .iter()
            .find(|item| item.label == "$builtin/profiles/companion.dcdl")
            .unwrap();
        assert_eq!(item.kind, "file");
        assert_eq!(
            item.detail.as_deref(),
            Some("read-only builtin Decodal source")
        );
        assert_eq!(snapshot.entries.len(), 2);
        assert!(
            snapshot
                .entries
                .keys()
                .all(|path| !path.as_str().starts_with("$builtin/"))
        );
    }

    #[test]
    fn wasm_analysis_boundary_reports_unknown_builtin_without_workspace_fallback() {
        let snapshot = snapshot();
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let diagnostics = session_environment(snapshot.clone())
            .analyze(&path, Some("import \"$builtin/profiles/missing.dcdl\""));
        assert!(!diagnostics.is_empty());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .message
                    .contains("unknown or non-public read-only builtin source")
            })
            .unwrap();
        assert_eq!(diagnostic.path, path);
        assert_eq!(diagnostic.tree_digest, snapshot.digest);
        assert!(
            diagnostic
                .message
                .contains("$builtin/profiles/missing.dcdl")
        );
        assert!(
            session_environment(snapshot)
                .analyze(
                    &VirtualPath::parse("main.dcdl").unwrap(),
                    Some("import \"./profiles/missing.dcdl\"")
                )
                .is_empty()
        );
    }
}
