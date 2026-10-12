//! `impl Scheme for OpenAIScheme`

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    auth::AuthRequirement,
    capability::ModelCapability,
    client::ConfigWarning,
    event::Event,
    scheme::Scheme,
    types::{Request, RequestConfig},
};

use super::OpenAIScheme;

impl Scheme for OpenAIScheme {
    type State = ();

    fn default_base_url(&self) -> &'static str {
        "https://api.openai.com"
    }

    fn path(&self, _model_id: &str) -> String {
        "/v1/chat/completions".to_string()
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
        let req = self.build_request(model_id, request, capability);
        serde_json::to_value(&req).expect("OpenAIRequest is always serialisable")
    }

    fn parse_sse(
        &self,
        _event_type: &str,
        data: &str,
        _state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError> {
        // `data: [DONE]` is a terminal marker.
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        Ok(self.parse_event(data)?.unwrap_or_default())
    }

    fn default_capability(&self) -> ModelCapability {
        super::capability::default_capability()
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();
        // The OpenAI Chat Completions API does not accept top_k.
        if config.top_k.is_some() {
            warnings.push(ConfigWarning::unsupported("top_k", "OpenAI Chat"));
        }
        warnings
    }
}
