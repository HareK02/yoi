//! OpenAI Responses API scheme (`/v1/responses`).
//!
//! An item-based wire format distinct from Chat Completions. Reasoning and
//! function_call items are first-class, and SSE events use the `response.*`
//! namespace.
//!
//! - Request JSON generation: `request`.
//! - SSE event parsing → [`Event`](crate::llm_client::event::Event) conversion: `events`.

mod capability;
mod events;
mod request;
mod scheme_impl;

pub use scheme_impl::OpenAIResponsesState;

/// OpenAI Responses scheme.
///
/// `store` / `include_encrypted_content` / `send_max_output_tokens` /
/// `send_sampling_params` are scheme-level wire settings. Defaults target the
/// official OpenAI Responses API (stateless + ZDR, with `max_output_tokens` /
/// `temperature` / `top_p` allowed). For compatible backends accepting only a
/// subset of parameters, the client construction layer overrides
/// `send_max_output_tokens=false` / `send_sampling_params=false`.
/// These do not belong in `ModelCapability`: they are wire policy, not model capabilities.
#[derive(Debug, Clone)]
pub struct OpenAIResponsesScheme {
    /// Whether to store responses on the server. `false` for ZDR/stateless operation.
    pub store: bool,
    /// Whether to add `include: ["reasoning.encrypted_content"]`.
    /// Required when using reasoning with `store=false`.
    pub include_encrypted_content: bool,
    /// Whether to include `max_output_tokens` in the body. The official OpenAI
    /// Responses API accepts it, but some compatible backends return 400 with
    /// `Unsupported parameter`; set this to `false` for those routes.
    pub send_max_output_tokens: bool,
    /// Whether to include `temperature` / `top_p` in the body. The official OpenAI
    /// Responses API accepts them, but some compatible backends return 400 with
    /// `Unsupported parameter`; set this to `false` for those routes.
    pub send_sampling_params: bool,
}

impl Default for OpenAIResponsesScheme {
    fn default() -> Self {
        Self {
            store: false,
            include_encrypted_content: true,
            send_max_output_tokens: true,
            send_sampling_params: true,
        }
    }
}

impl OpenAIResponsesScheme {
    /// Default settings (`store=false`, `include=["reasoning.encrypted_content"]`,
    /// `send_max_output_tokens=true`, `send_sampling_params=true`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override `store`.
    pub fn with_store(mut self, store: bool) -> Self {
        self.store = store;
        self
    }

    /// Override whether to include `include: ["reasoning.encrypted_content"]`.
    pub fn with_include_encrypted_content(mut self, include: bool) -> Self {
        self.include_encrypted_content = include;
        self
    }

    /// Override whether to include `max_output_tokens` in the body.
    pub fn with_send_max_output_tokens(mut self, send: bool) -> Self {
        self.send_max_output_tokens = send;
        self
    }

    /// Override whether to include `temperature` / `top_p` in the body.
    pub fn with_send_sampling_params(mut self, send: bool) -> Self {
        self.send_sampling_params = send;
        self
    }
}
