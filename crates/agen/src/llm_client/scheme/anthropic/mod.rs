//! Anthropic Messages API scheme.
//!
//! - Request JSON generation.
//! - SSE event parsing → Event conversion.

mod capability;
mod events;
mod request;
mod scheme_impl;

pub use scheme_impl::AnthropicState;

/// Anthropic scheme.
///
/// Handles request/response conversion for the Anthropic Messages API.
#[derive(Debug, Clone)]
pub struct AnthropicScheme {
    /// API version.
    pub api_version: String,
    /// Whether to enable fine-grained tool streaming.
    pub fine_grained_tool_streaming: bool,
}

impl Default for AnthropicScheme {
    fn default() -> Self {
        Self {
            api_version: "2023-06-01".to_string(),
            fine_grained_tool_streaming: true,
        }
    }
}

impl AnthropicScheme {
    /// Create a new scheme.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable fine-grained tool streaming.
    pub fn with_fine_grained_tool_streaming(mut self, enabled: bool) -> Self {
        self.fine_grained_tool_streaming = enabled;
        self
    }
}
