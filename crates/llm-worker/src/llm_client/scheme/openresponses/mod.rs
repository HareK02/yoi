//! Open Responses Scheme
//!
//! Handles request/response conversion for the Open Responses API.
//! Since our internal types are already Open Responses native, this scheme
//! primarily passes through data with minimal transformation.

mod events;
mod request;

use crate::llm_client::{ClientError, Request};

pub use events::*;
pub use request::*;

/// Open Responses Scheme
///
/// Handles conversion between internal types and the Open Responses wire format.
#[derive(Debug, Clone, Default)]
pub struct OpenResponsesScheme {
    /// Optional model override
    pub model: Option<String>,
}

impl OpenResponsesScheme {
    /// Create a new OpenResponsesScheme
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Build Open Responses request from internal Request
    pub fn build_request(&self, model: &str, request: &Request) -> OpenResponsesRequest {
        build_request(model, request)
    }

    /// Parse SSE event data into internal Event(s)
    pub fn parse_event(
        &self,
        event_type: &str,
        data: &str,
    ) -> Result<Option<Vec<crate::llm_client::Event>>, ClientError> {
        parse_event(event_type, data)
    }
}
