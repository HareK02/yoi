use config_source::{
    ConfigTreeSnapshot, EvaluationResult, SnapshotEnvironment, ToolchainContract, VirtualPath,
};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn evaluate_snapshot(snapshot: JsValue, contract: JsValue) -> Result<JsValue, JsValue> {
    let snapshot: ConfigTreeSnapshot = decode(snapshot)?;
    let contract: ToolchainContract = decode(contract)?;
    encode(
        SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .map_err(|diagnostics| {
                to_value(&diagnostics).unwrap_or_else(|_| JsValue::from_str("evaluation failed"))
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
    to_value(&value).map_err(|error| JsValue::from_str(&error.to_string()))
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[allow(dead_code)]
fn _assert_serializable(_: EvaluationResult) {}
