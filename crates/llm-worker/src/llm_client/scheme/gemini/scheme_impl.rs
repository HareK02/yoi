//! `impl Scheme for GeminiScheme`

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    capability::ModelCapability,
    event::Event,
    auth::AuthRequirement,
    scheme::Scheme,
    types::Request,
};

use super::GeminiScheme;

impl Scheme for GeminiScheme {
    type State = ();

    fn default_base_url(&self) -> &'static str {
        "https://generativelanguage.googleapis.com"
    }

    fn path(&self, model_id: &str) -> String {
        format!("/v1beta/models/{model_id}:streamGenerateContent?alt=sse")
    }

    fn required_auth(&self) -> AuthRequirement {
        AuthRequirement::QueryParam { name: "key" }
    }

    fn build_request_body(
        &self,
        _model_id: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> Value {
        let req = self.build_request(request, capability);
        serde_json::to_value(&req).expect("GeminiRequest is always serialisable")
    }

    fn parse_sse(
        &self,
        _event_type: &str,
        data: &str,
        _state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError> {
        Ok(self.parse_event(data)?.unwrap_or_default())
    }

    fn capability_for(&self, model_id: &str) -> Option<ModelCapability> {
        super::capability::lookup(model_id)
    }
}
