//! `impl Scheme for OpenAIScheme`

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    capability::ModelCapability,
    event::Event,
    auth::AuthRequirement,
    scheme::Scheme,
    types::Request,
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
        // `data: [DONE]` は終端マーカー
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        Ok(self.parse_event(data)?.unwrap_or_default())
    }

    fn capability_for(&self, model_id: &str) -> Option<ModelCapability> {
        super::capability::lookup(model_id)
    }
}
