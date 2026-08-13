use config_source::{
    ConfigTreeChange, ConfigTreeSnapshot, EvaluationResult, SnapshotEnvironment, ToolchainContract,
    VirtualPath,
};
use serde_wasm_bindgen::{Serializer, from_value};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

thread_local! {
    static SESSION: RefCell<Option<ConfigTreeSnapshot>> = const { RefCell::new(None) };
}

#[wasm_bindgen]
pub fn set_snapshot(snapshot: JsValue) -> Result<(), JsValue> {
    let snapshot: ConfigTreeSnapshot = decode(snapshot)?;
    SESSION.with(|session| session.replace(Some(snapshot)));
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
    utf8_byte_offset: usize,
    explicit: bool,
) -> Result<JsValue, JsValue> {
    let entrypoint = VirtualPath::parse(entrypoint).map_err(js_error)?;
    SESSION.with(|session| {
        let session = session.borrow();
        let snapshot = session
            .as_ref()
            .ok_or_else(|| JsValue::from_str("config source snapshot is not initialized"))?;
        let result = SnapshotEnvironment::new(snapshot.clone())
            .complete(&entrypoint, &source, utf8_byte_offset, explicit)
            .map_err(|error| JsValue::from_str(&format!("{error:?}")))?
            .map(|result| WasmCompletionResult {
                from: result.from,
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
            });
        encode(result)
    })
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
    encode(SnapshotEnvironment::new(snapshot).analyze(&entrypoint, source_override.as_deref()))
}

#[wasm_bindgen]
pub fn format_source(source: String) -> Result<String, JsValue> {
    SnapshotEnvironment::new(ConfigTreeSnapshot::empty())
        .format(&source)
        .map_err(|error| JsValue::from_str(&error))
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
