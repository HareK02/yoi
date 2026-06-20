//! Guest-side helpers for Yoi Component Model Tool plugins.
//!
//! This crate is intentionally small and guest-only: it depends on JSON/WIT
//! binding support, but not on Yoi host/runtime crates. It grants no authority;
//! package manifests and host-side Plugin grants decide whether a Tool or host
//! API can run.
//!
//! Component authors still generate the WIT bindings in their guest crate, then
//! delegate the exported `call` function to [`run_json_tool`] or to the
//! [`export_component_tool!`] macro:
//!
//! ```ignore
//! use yoi_plugin_pdk::wit_bindgen;
//!
//! wit_bindgen::generate!({
//!     world: "tool",
//!     path: "../../../../resources/plugin/wit",
//!     generate_all,
//!     runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
//! });
//!
//! fn echo(
//!     ctx: yoi_plugin_pdk::ToolContext,
//!     input: EchoInput,
//! ) -> Result<yoi_plugin_pdk::ToolOutput, yoi_plugin_pdk::ToolError> {
//!     yoi_plugin_pdk::ToolOutput::json(
//!         format!("{} ok", ctx.tool_name()),
//!         EchoOutput { text: input.text },
//!     )
//! }
//!
//! yoi_plugin_pdk::export_component_tool!(Plugin, echo);
//! ```

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use wit_bindgen;

pub type Result<T> = std::result::Result<T, ToolError>;

/// Current Yoi Component Model Tool world targeted by this PDK.
pub const TOOL_WORLD: &str = "yoi:plugin/tool@1.0.0";

/// Repository WIT for the current Tool world, exposed for authoring tools and
/// tests. Runtime components should still generate bindings at compile time.
pub const TOOL_WIT: &str = include_str!("../../../resources/plugin/wit/yoi-plugin-tool-v1.wit");

/// Repository WIT for the grant-bound host APIs importable by Tool components.
pub const HOST_WIT: &str =
    include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit");

/// Maximum serialized ToolOutput JSON accepted by Yoi's current Plugin runtime.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
/// Maximum summary bytes accepted by Yoi's current Plugin runtime.
pub const MAX_SUMMARY_BYTES: usize = 1024;
/// Conservative content cap that leaves room for JSON framing and escaping.
pub const MAX_CONTENT_BYTES: usize = MAX_TOOL_OUTPUT_BYTES - MAX_SUMMARY_BYTES - 4096;
/// Maximum structured error message bytes retained by the PDK.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 4096;

/// Per-call context passed to a typed JSON handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContext {
    tool_name: String,
}

impl ToolContext {
    /// Create context for the manifest-declared Tool name selected by the host.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: bounded_text(tool_name.into(), MAX_SUMMARY_BYTES),
        }
    }

    /// Manifest-declared Tool name supplied by the host runtime.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// ToolOutput JSON shape accepted by the current Yoi Plugin runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolOutput {
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

impl ToolOutput {
    /// Create an ordinary Tool output.
    pub fn new(summary: impl Into<String>, content: Option<String>) -> Self {
        let mut output = Self {
            summary: normalize_summary(summary.into()),
            content: content.map(|value| bounded_text(value, MAX_CONTENT_BYTES)),
        };
        output.bound_serialized_json();
        output
    }

    /// Create a Tool output whose content is typed JSON.
    pub fn json(
        summary: impl Into<String>,
        value: impl Serialize,
    ) -> std::result::Result<Self, ToolError> {
        let content = serde_json::to_string(&value).map_err(ToolError::serialization)?;
        let output = Self {
            summary: normalize_summary(summary.into()),
            content: Some(content),
        };
        let serialized = serde_json::to_string(&output).map_err(ToolError::serialization)?;
        if serialized.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err(ToolError::invalid_output(format!(
                "serialized ToolOutput JSON exceeds {MAX_TOOL_OUTPUT_BYTES} bytes"
            )));
        }
        Ok(output)
    }

    /// Create a summary-only Tool output.
    pub fn summary(summary: impl Into<String>) -> Self {
        Self::new(summary, None)
    }

    /// Return the bounded summary.
    pub fn summary_text(&self) -> &str {
        &self.summary
    }

    /// Return optional detailed content.
    pub fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }

    /// Serialize to the ToolOutput JSON string returned by the WIT `call` export.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"summary":"tool error: serialization","content":"{\"error\":{\"code\":\"serialization\",\"message\":\"failed to serialize ToolOutput\"}}"}"#.to_string()
        })
    }

    fn bound_serialized_json(&mut self) {
        loop {
            let Ok(serialized) = serde_json::to_string(self) else {
                return;
            };
            if serialized.len() <= MAX_TOOL_OUTPUT_BYTES {
                return;
            }
            let Some(content) = self.content.take() else {
                return;
            };
            if content.is_empty() {
                return;
            }

            let overflow = serialized.len() - MAX_TOOL_OUTPUT_BYTES;
            let target = content.len().saturating_sub(overflow + "…".len());
            self.content = Some(bounded_text(content, target));
        }
    }
}

/// Stable, low-cardinality error codes for PDK-produced Tool errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    InvalidInput,
    InvalidOutput,
    Serialization,
    Denied,
    UnsupportedTool,
    Failed,
}

impl ToolErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidOutput => "invalid_output",
            Self::Serialization => "serialization",
            Self::Denied => "denied",
            Self::UnsupportedTool => "unsupported_tool",
            Self::Failed => "failed",
        }
    }
}

/// Structured, bounded error that is rendered as ordinary ToolOutput JSON.
#[derive(Clone, Debug, PartialEq, Serialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    code: ToolErrorCode,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ToolError {
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: bounded_text(message.into(), MAX_ERROR_MESSAGE_BYTES),
            details: None,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidInput, message)
    }

    pub fn invalid_output(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidOutput, message)
    }

    pub fn serialization(error: impl std::fmt::Display) -> Self {
        Self::new(ToolErrorCode::Serialization, error.to_string())
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Denied, message)
    }

    pub fn unsupported_tool(tool_name: impl Into<String>) -> Self {
        Self::new(
            ToolErrorCode::UnsupportedTool,
            format!("unsupported tool `{}`", tool_name.into()),
        )
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Failed, message)
    }

    pub fn with_details(mut self, details: impl Serialize) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub fn code(&self) -> ToolErrorCode {
        self.code
    }

    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Render this error as ordinary ToolOutput JSON content.
    pub fn into_tool_output(self) -> ToolOutput {
        #[derive(Serialize)]
        struct ErrorBody<'a> {
            error: ErrorPayload<'a>,
        }

        #[derive(Serialize)]
        struct ErrorPayload<'a> {
            code: &'static str,
            message: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            details: Option<&'a Value>,
        }

        let summary = format!("tool error: {}", self.code.as_str());
        let payload = ErrorBody {
            error: ErrorPayload {
                code: self.code.as_str(),
                message: &self.message,
                details: self.details.as_ref(),
            },
        };
        let mut content = serde_json::to_string(&payload).unwrap_or_else(|_| {
            format!(
                r#"{{"error":{{"code":"{}","message":"failed to serialize error details"}}}}"#,
                ToolErrorCode::Serialization.as_str()
            )
        });
        if content.len() > MAX_CONTENT_BYTES {
            let payload = ErrorBody {
                error: ErrorPayload {
                    code: self.code.as_str(),
                    message: &self.message,
                    details: None,
                },
            };
            content = serde_json::to_string(&payload).unwrap_or_else(|_| {
                r#"{"error":{"code":"serialization","message":"failed to serialize error"}}"#
                    .to_string()
            });
        }
        ToolOutput::new(summary, Some(content))
    }
}

/// Parse the WIT `input-json` string into a typed input value.
pub fn parse_json_input<T>(input_json: &str) -> std::result::Result<T, ToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(input_json).map_err(|error| {
        ToolError::invalid_input(format!(
            "tool input is not valid JSON for this schema: {error}"
        ))
    })
}

/// Execute a typed JSON Tool handler and return ToolOutput JSON for the runtime.
///
/// Handler errors and parse/serialization failures are not panics or host-side
/// authority decisions. They are rendered into ordinary ToolOutput JSON so the
/// runtime can route them through the normal Tool result path.
pub fn run_json_tool<I, F>(tool_name: &str, input_json: &str, handler: F) -> String
where
    I: DeserializeOwned,
    F: FnOnce(ToolContext, I) -> std::result::Result<ToolOutput, ToolError>,
{
    let result = parse_json_input::<I>(input_json).and_then(|input| {
        let context = ToolContext::new(tool_name);
        handler(context, input)
    });
    match result {
        Ok(output) => output.to_json_string(),
        Err(error) => error.into_tool_output().to_json_string(),
    }
}

/// Implement the generated Component Model `Guest` trait for a typed JSON
/// handler and export it with the `wit-bindgen` generated `export!` macro.
///
/// The caller must import the PDK's `wit_bindgen` re-export and invoke
/// `wit_bindgen::generate!` for the `tool` world first, with
/// `runtime_path: "yoi_plugin_pdk::wit_bindgen::rt"`. That defines the
/// `Guest` trait and `export!` macro in the current module. The generated
/// component still imports only WIT-declared
/// host APIs; this macro does not grant filesystem, network, or environment
/// authority.
#[macro_export]
macro_rules! export_component_tool {
    ($adapter:ident, $handler:path) => {
        struct $adapter;

        impl Guest for $adapter {
            fn call(
                tool_name: ::std::string::String,
                input_json: ::std::string::String,
            ) -> ::std::string::String {
                $crate::run_json_tool(&tool_name, &input_json, $handler)
            }
        }

        export!($adapter);
    };
}

fn normalize_summary(summary: String) -> String {
    let summary = bounded_text(summary, MAX_SUMMARY_BYTES);
    if summary.trim().is_empty() {
        "tool completed".to_string()
    } else {
        summary
    }
}

fn bounded_text(mut value: String, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }

    value = value
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    if value.len() <= max_bytes {
        return value;
    }

    let suffix = "…";
    let budget = max_bytes.saturating_sub(suffix.len());
    let mut cut = budget.min(value.len());
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    value.truncate(cut);
    value.push_str(suffix);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct EchoInput {
        text: String,
    }

    #[derive(Debug, Serialize)]
    struct EchoOutput<'a> {
        tool: &'a str,
        text: String,
    }

    #[test]
    fn run_json_tool_parses_input_and_serializes_output() {
        let output = run_json_tool(
            "example_echo",
            r#"{"text":"hello"}"#,
            |ctx, input: EchoInput| {
                ToolOutput::json(
                    format!("{} ok", ctx.tool_name()),
                    EchoOutput {
                        tool: ctx.tool_name(),
                        text: input.text,
                    },
                )
            },
        );

        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["summary"], "example_echo ok");
        let content: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
        assert_eq!(content, json!({"tool":"example_echo","text":"hello"}));
        assert!(output.len() <= MAX_TOOL_OUTPUT_BYTES);
    }

    #[test]
    fn run_json_tool_renders_error_output() {
        let output =
            run_json_tool::<EchoInput, _>("example_echo", r#"{"text":1}"#, |_ctx, _input| {
                Ok(ToolOutput::summary("unreachable"))
            });

        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["summary"], "tool error: invalid_input");
        let content: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["error"]["code"], "invalid_input");
        assert!(content["error"]["message"].as_str().unwrap().len() <= MAX_ERROR_MESSAGE_BYTES);
        assert!(output.len() <= MAX_TOOL_OUTPUT_BYTES);
    }

    #[test]
    fn json_output_rejects_oversized_serialized_tool_output() {
        let too_large = vec!["quoted \" text"; MAX_TOOL_OUTPUT_BYTES / 4];
        let error = ToolOutput::json("too large", too_large).unwrap_err();

        assert_eq!(error.code(), ToolErrorCode::InvalidOutput);
    }

    #[test]
    fn tool_error_bounds_message_and_control_characters() {
        let message = format!("bad\u{0007}{}", "x".repeat(MAX_ERROR_MESSAGE_BYTES * 2));
        let error = ToolError::failed(message);

        assert_eq!(error.code_str(), "failed");
        assert!(!error.message().contains('\u{0007}'));
        assert!(error.message().len() <= MAX_ERROR_MESSAGE_BYTES + "…".len());
    }

    #[test]
    fn wit_constants_match_current_world() {
        assert!(TOOL_WIT.contains("package yoi:plugin@1.0.0"));
        assert!(TOOL_WIT.contains("world tool"));
        assert!(TOOL_WIT.contains("export call"));
        assert_eq!(TOOL_WORLD, "yoi:plugin/tool@1.0.0");
        assert!(HOST_WIT.contains("interface https"));
        assert!(HOST_WIT.contains("interface fs"));
        assert!(HOST_WIT.contains("%list: func"));
    }
}

/// Versioned Component Model instance world handled by the host-managed
/// PluginInstanceRegistry.
pub const PLUGIN_INSTANCE_WORLD: &str = "yoi:plugin/instance@1.0.0";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PluginIngressEvent {
    pub kind: String,
    pub source: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PluginStatus {
    pub state: String,
    #[serde(default)]
    pub data: Value,
}

impl PluginStatus {
    pub fn ready(data: Value) -> Self {
        Self {
            state: "ready".to_string(),
            data,
        }
    }

    pub fn stopped() -> Self {
        Self {
            state: "stopped".to_string(),
            data: Value::Null,
        }
    }
}

/// Rust-facing instance Plugin contract. Hosts call `start` once, then route
/// Tool/Ingress surfaces through the same mutable instance.
pub trait Plugin: Sized + 'static {
    fn start(config: Value) -> Result<Self>;
    fn handle_tool(&mut self, name: &str, input: Value) -> Result<ToolOutput>;
    fn handle_ingress(&mut self, name: &str, event: PluginIngressEvent) -> Result<Value>;
    fn status(&self) -> Result<PluginStatus> {
        Ok(PluginStatus::ready(Value::Null))
    }
    fn stop(&mut self) -> Result<PluginStatus> {
        Ok(PluginStatus::stopped())
    }
}

#[doc(hidden)]
pub fn plugin_instance_error(message: impl Into<String>) -> String {
    serde_json::json!({ "error": message.into() }).to_string()
}

/// Export the simple string-json instance ABI used by
/// `yoi:plugin/instance@1.0.0`.
#[macro_export]
macro_rules! export_plugin_instance {
    ($plugin:ty) => {
        mod __yoi_plugin_instance_export {
            use super::*;
            use std::cell::RefCell;

            thread_local! {
                static INSTANCE: RefCell<Option<$plugin>> = const { RefCell::new(None) };
            }

            #[unsafe(export_name = "start")]
            pub extern "C" fn __yoi_start(
                _config_json_ptr: *const u8,
                _config_json_len: usize,
            ) -> usize {
                // This low-level symbol is a placeholder for non-component raw builds.
                // Component builds should bind this macro through generated WIT glue.
                0
            }

            pub struct InstanceGuest;

            impl InstanceGuest {
                pub fn start(config_json: String) -> String {
                    let config =
                        serde_json::from_str(&config_json).unwrap_or(serde_json::Value::Null);
                    match <$plugin as $crate::Plugin>::start(config) {
                        Ok(plugin) => {
                            INSTANCE.with(|slot| *slot.borrow_mut() = Some(plugin));
                            serde_json::to_string(&$crate::PluginStatus::ready(
                                serde_json::Value::Null,
                            ))
                            .unwrap()
                        }
                        Err(error) => $crate::plugin_instance_error(error.to_string()),
                    }
                }

                pub fn handle_tool(name: String, input_json: String) -> String {
                    let input =
                        serde_json::from_str(&input_json).unwrap_or(serde_json::Value::Null);
                    INSTANCE.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let Some(plugin) = slot.as_mut() else {
                            return $crate::plugin_instance_error(
                                "plugin instance has not been started",
                            );
                        };
                        match plugin.handle_tool(&name, input) {
                            Ok(output) => serde_json::to_string(&output).unwrap(),
                            Err(error) => $crate::plugin_instance_error(error.to_string()),
                        }
                    })
                }

                pub fn handle_ingress(name: String, event_json: String) -> String {
                    let event =
                        match serde_json::from_str::<$crate::PluginIngressEvent>(&event_json) {
                            Ok(event) => event,
                            Err(error) => return $crate::plugin_instance_error(error.to_string()),
                        };
                    INSTANCE.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let Some(plugin) = slot.as_mut() else {
                            return $crate::plugin_instance_error(
                                "plugin instance has not been started",
                            );
                        };
                        match plugin.handle_ingress(&name, event) {
                            Ok(output) => serde_json::to_string(&output).unwrap(),
                            Err(error) => $crate::plugin_instance_error(error.to_string()),
                        }
                    })
                }

                pub fn status() -> String {
                    INSTANCE.with(|slot| {
                        let slot = slot.borrow();
                        let Some(plugin) = slot.as_ref() else {
                            return $crate::plugin_instance_error(
                                "plugin instance has not been started",
                            );
                        };
                        match plugin.status() {
                            Ok(status) => serde_json::to_string(&status).unwrap(),
                            Err(error) => $crate::plugin_instance_error(error.to_string()),
                        }
                    })
                }

                pub fn stop() -> String {
                    INSTANCE.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let Some(plugin) = slot.as_mut() else {
                            return $crate::plugin_instance_error(
                                "plugin instance has not been started",
                            );
                        };
                        match plugin.stop() {
                            Ok(status) => serde_json::to_string(&status).unwrap(),
                            Err(error) => $crate::plugin_instance_error(error.to_string()),
                        }
                    })
                }
            }
        }
    };
}
