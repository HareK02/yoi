//! `impl Scheme for OpenAIResponsesScheme`

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    auth::AuthRequirement,
    capability::ModelCapability,
    event::Event,
    scheme::Scheme,
    types::Request,
};

use super::OpenAIResponsesScheme;

pub use super::events::OpenAIResponsesState;

impl Scheme for OpenAIResponsesScheme {
    type State = OpenAIResponsesState;

    fn default_base_url(&self) -> &'static str {
        "https://api.openai.com"
    }

    fn path(&self, _model_id: &str) -> String {
        "/v1/responses".to_string()
    }

    fn required_auth(&self) -> AuthRequirement {
        AuthRequirement::Bearer
    }

    fn build_request_body(
        &self,
        model_id: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> Value {
        let body = self.build_request(model_id, request, capability);
        serde_json::to_value(&body).expect("ResponsesRequest is always serialisable")
    }

    fn parse_sse(
        &self,
        event_type: &str,
        data: &str,
        state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError> {
        super::events::parse_sse(event_type, data, state)
    }

    fn capability_for(&self, model_id: &str) -> Option<ModelCapability> {
        super::capability::lookup(model_id)
    }

    fn default_capability(&self) -> ModelCapability {
        super::capability::default_capability()
    }
}
