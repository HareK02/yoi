//! Shared LLM client trait definitions.

use std::pin::Pin;

use crate::llm_client::{ClientError, Request, RequestConfig, event::Event};
use async_trait::async_trait;
use futures::Stream;

/// Configuration warning.
///
/// Returned when using a configuration option unsupported by the provider.
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    /// Configuration option name.
    pub option_name: &'static str,
    /// Warning message.
    pub message: String,
}

impl ConfigWarning {
    /// Create a new warning.
    pub fn unsupported(option_name: &'static str, provider_name: &str) -> Self {
        Self {
            option_name,
            message: format!(
                "'{}' is not supported by {} and will be ignored",
                option_name, provider_name
            ),
        }
    }
}

impl std::fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.option_name, self.message)
    }
}

pub type ResponseStream = Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>;

/// Whether normalized `BlockStop(ToolUse)` events prove one call is complete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolCallCompletionSupport {
    /// Individual tool-call completion events are provider-authoritative.
    PerBlock,
    /// Tool calls are complete only once the response stream itself completes.
    #[default]
    ResponseComplete,
}

/// LLM client trait.
///
/// Each provider implements this trait to expose a unified interface.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a streaming request and return an Event stream.
    ///
    /// # Arguments
    /// * `request` - Request information.
    ///
    /// # Returns
    /// * `Ok(Stream)` - Event stream.
    /// * `Err(ClientError)` - Error.
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError>;

    /// Clone this client into a new `Box<dyn LlmClient>`.
    ///
    /// Used when a second client instance is needed (e.g. for context
    /// compaction) without access to the original construction parameters.
    fn clone_boxed(&self) -> Box<dyn LlmClient>;

    /// Describe the strongest safe tool-call completion boundary.
    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::ResponseComplete
    }

    /// Validate configuration and return warnings for unsupported options.
    ///
    /// # Arguments
    /// * `config` - Configuration to validate.
    ///
    /// # Returns
    /// List of warnings for unsupported options.
    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        // Default implementation: all options are supported.
        let _ = config;
        Vec::new()
    }
}

impl Clone for Box<dyn LlmClient> {
    fn clone(&self) -> Self {
        self.clone_boxed()
    }
}

/// `LlmClient` implementation for `Box<dyn LlmClient>`.
///
/// Allows clients using dynamic dispatch to be used with `Engine`.
#[async_trait]
impl LlmClient for Box<dyn LlmClient> {
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        (**self).stream(request).await
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        (**self).clone_boxed()
    }

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        (**self).tool_call_completion_support()
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        (**self).validate_config(config)
    }
}
