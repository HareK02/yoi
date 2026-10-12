//! OpenAI Chat Completions API scheme.
//!
//! - Request JSON generation.
//! - SSE event parsing → Event conversion.

pub(crate) mod capability;
mod events;
mod request;
mod scheme_impl;

/// OpenAI scheme.
///
/// Handles request/response conversion for the OpenAI Chat Completions API
/// (and compatible APIs).
#[derive(Debug, Clone, Default)]
pub struct OpenAIScheme {
    /// Model name (supplied per request, but may also be stored as a default).
    pub model: Option<String>,
    /// Whether to use legacy max_tokens (for Ollama compatibility).
    pub use_legacy_max_tokens: bool,
}

impl OpenAIScheme {
    /// Create a new scheme.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether to use legacy max_tokens.
    pub fn with_legacy_max_tokens(mut self, use_legacy: bool) -> Self {
        self.use_legacy_max_tokens = use_legacy;
        self
    }
}
