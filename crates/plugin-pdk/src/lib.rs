#![doc = include_str!("../README.md")]

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use wit_bindgen;

pub type Result<T> = std::result::Result<T, ToolError>;

pub const TOOL_WORLD: &str = "yoi:plugin/tool-plugin@2.0.0";
pub const HTTPS_TOOL_WORLD: &str = "yoi:plugin/https-tool-plugin@2.0.0";
pub const TOOL_WIT: &str = include_str!("../../../resources/plugin/wit/yoi-plugin-v2.wit");
pub const HOST_WIT: &str =
    include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit");
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_TOOL_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_ERROR_MESSAGE_BYTES: usize = 4096;

/// Non-secret call snapshot. Invocation identity tracks effects; it grants no authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContext {
    invocation_id: String,
    tool_name: String,
    config_json: String,
    remaining_ms: u32,
}

impl ToolContext {
    pub fn new(
        invocation_id: String,
        tool_name: String,
        config_json: String,
        remaining_ms: u32,
    ) -> Self {
        Self {
            invocation_id,
            tool_name,
            config_json,
            remaining_ms,
        }
    }

    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
    pub fn remaining_ms(&self) -> u32 {
        self.remaining_ms
    }

    /// Decode the plan's non-secret config. Host validation remains authoritative.
    pub fn config<T: DeserializeOwned>(&self) -> Result<T> {
        parse_bounded_json(&self.config_json, MAX_CONFIG_BYTES)
    }
}

/// Output crosses WIT as a record, never as a success envelope hiding an error.
/// Oversized output is rejected, not silently truncated into invalid JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolOutput {
    summary: String,
    content: Option<String>,
    structured_json: Option<String>,
}

impl ToolOutput {
    pub fn new(summary: impl Into<String>, content: Option<String>) -> Result<Self> {
        Self::checked(summary.into(), content, None)
    }

    pub fn summary(summary: impl Into<String>) -> Result<Self> {
        Self::new(summary, None)
    }

    /// Structured JSON is separate from presentation text and needs a manifest output schema.
    pub fn json(summary: impl Into<String>, value: impl Serialize) -> Result<Self> {
        let json = serde_json::to_string(&value).map_err(ToolError::serialization)?;
        Self::checked(summary.into(), None, Some(json))
    }

    fn checked(
        summary: String,
        content: Option<String>,
        structured_json: Option<String>,
    ) -> Result<Self> {
        let output = Self {
            summary,
            content,
            structured_json,
        };
        let bytes = serde_json::to_vec(&output).map_err(ToolError::serialization)?;
        if bytes.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err(ToolError::invalid_output(
                "serialized output exceeds 64 KiB",
            ));
        }
        Ok(output)
    }

    pub fn summary_text(&self) -> &str {
        &self.summary
    }
    pub fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }
    pub fn structured_json(&self) -> Option<&str> {
        self.structured_json.as_deref()
    }

    #[doc(hidden)]
    pub fn into_parts(self) -> (String, Option<String>, Option<String>) {
        (self.summary, self.content, self.structured_json)
    }
}

/// Guest business errors are not Host authorization decisions or effect receipts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorCode {
    InvalidInput,
    InvalidOutput,
    Serialization,
    UnsupportedTool,
    Failed,
}

impl ToolErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidOutput => "invalid_output",
            Self::Serialization => "serialization",
            Self::UnsupportedTool => "unsupported_tool",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    code: ToolErrorCode,
    message: String,
}

impl ToolError {
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_MESSAGE_BYTES {
            let mut end = MAX_ERROR_MESSAGE_BYTES;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self { code, message }
    }
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidInput, message)
    }
    pub fn invalid_output(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidOutput, message)
    }
    /// Serialization diagnostics must not include the original payload.
    pub fn serialization(_error: impl std::fmt::Display) -> Self {
        Self::new(ToolErrorCode::Serialization, "output serialization failed")
    }
    pub fn unsupported_tool(_name: impl AsRef<str>) -> Self {
        Self::new(ToolErrorCode::UnsupportedTool, "unsupported Tool name")
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Failed, message)
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
}

fn parse_bounded_json<T: DeserializeOwned>(json: &str, limit: usize) -> Result<T> {
    if json.len() > limit {
        return Err(ToolError::invalid_input("JSON exceeds 64 KiB"));
    }
    serde_json::from_str(json)
        .map_err(|_| ToolError::invalid_input("JSON does not match the expected schema"))
}

pub fn parse_json_input<T: DeserializeOwned>(json: &str) -> Result<T> {
    parse_bounded_json(json, MAX_TOOL_INPUT_BYTES)
}

/// Host performs schema and admission checks; this boundary only decodes guest data.
/// Failures remain the WIT result's error arm. No retry or success stub is introduced.
pub fn run_json_tool<I: DeserializeOwned>(
    context: ToolContext,
    input_json: &str,
    handler: impl FnOnce(ToolContext, I) -> Result<ToolOutput>,
) -> Result<ToolOutput> {
    let _: serde_json::Value = context.config()?;
    let input = parse_json_input(input_json)?;
    handler(context, input)
}

/// Adapt bindings generated from any supported v2 Tool world. The generated glue
/// owns the canonical ABI; callers never implement pointer/length exports.
#[macro_export]
macro_rules! export_component_tool {
    ($adapter:ident, $handler:path) => {
        struct $adapter;
        impl exports::yoi::plugin::tool::Guest for $adapter {
            fn invoke(
                input: exports::yoi::plugin::tool::Invocation,
            ) -> ::std::result::Result<
                exports::yoi::plugin::tool::Output,
                exports::yoi::plugin::tool::ToolError,
            > {
                let context = $crate::ToolContext::new(
                    input.invocation_id,
                    input.tool_name,
                    input.config_json,
                    input.remaining_ms,
                );
                $crate::run_json_tool(context, &input.input_json, $handler)
                    .map(|output| {
                        let (summary, content, structured_json) = output.into_parts();
                        exports::yoi::plugin::tool::Output {
                            summary,
                            content,
                            structured_json,
                        }
                    })
                    .map_err(|error| exports::yoi::plugin::tool::ToolError {
                        code: error.code_str().to_owned(),
                        message: error.message().to_owned(),
                    })
            }
        }
        export!($adapter);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        text: String,
    }

    fn context(config: &str) -> ToolContext {
        ToolContext::new("call-1".into(), "echo".into(), config.into(), 500)
    }

    #[test]
    fn typed_handler_keeps_structured_output_separate_from_text() {
        let output = run_json_tool(context("{}"), r#"{"text":"hello"}"#, |ctx, input: Input| {
            assert_eq!(ctx.invocation_id(), "call-1");
            assert_eq!(ctx.remaining_ms(), 500);
            ToolOutput::json(
                "echoed",
                json!({"text": input.text, "tool": ctx.tool_name()}),
            )
        })
        .unwrap();
        assert_eq!(output.content(), None);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(output.structured_json().unwrap()).unwrap(),
            json!({"text":"hello", "tool":"echo"})
        );
    }

    #[test]
    fn invalid_input_or_config_never_runs_handler_or_returns_success() {
        for (config, input) in [
            ("{}", r#"{"text":1}"#),
            ("broken", r#"{"text":"ok"}"#),
            ("{}", r#"{"text":"ok","extra":true}"#),
        ] {
            let error = run_json_tool::<Input>(context(config), input, |_, _| {
                panic!("handler must not run")
            })
            .unwrap_err();
            assert_eq!(error.code(), ToolErrorCode::InvalidInput);
        }
    }

    #[test]
    fn payload_limits_reject_overflow_including_json_escaping() {
        for text in [
            "x".repeat(MAX_TOOL_OUTPUT_BYTES),
            "\"".repeat(MAX_TOOL_OUTPUT_BYTES / 2),
            "語".repeat(MAX_TOOL_OUTPUT_BYTES / 3),
        ] {
            assert_eq!(
                ToolOutput::json("large", &text).unwrap_err().code(),
                ToolErrorCode::InvalidOutput
            );
        }
        assert!(
            parse_json_input::<serde_json::Value>(&format!(
                "\"{}\"",
                "x".repeat(MAX_TOOL_INPUT_BYTES)
            ))
            .is_err()
        );
        let large = format!("\"{}\"", "x".repeat(MAX_CONFIG_BYTES));
        assert!(context(&large).config::<serde_json::Value>().is_err());
    }

    #[test]
    fn business_failure_is_preserved_and_message_is_utf8_bounded() {
        let error = run_json_tool(context("{}"), "null", |_, _: ()| {
            Err(ToolError::failed("語".repeat(MAX_ERROR_MESSAGE_BYTES)))
        })
        .unwrap_err();
        assert_eq!(error.code_str(), "failed");
        assert!(error.message().len() <= MAX_ERROR_MESSAGE_BYTES);
        assert!(error.message().ends_with('語'));
    }
}
