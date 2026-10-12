//! Google Gemini API scheme.
//!
//! - Request JSON generation.
//! - SSE event parsing → Event conversion.

mod capability;
mod events;
mod request;
mod scheme_impl;

/// Gemini scheme.
///
/// Handles request/response conversion for the Google Gemini API.
#[derive(Debug, Clone, Default)]
pub struct GeminiScheme {
    /// Whether to enable streaming function call arguments.
    pub stream_function_call_arguments: bool,
}

impl GeminiScheme {
    /// Create a new scheme.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable streaming function call arguments.
    pub fn with_stream_function_call_arguments(mut self, enabled: bool) -> Self {
        self.stream_function_call_arguments = enabled;
        self
    }
}
