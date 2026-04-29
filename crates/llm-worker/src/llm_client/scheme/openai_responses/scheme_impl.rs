//! `impl Scheme for OpenAIResponsesScheme`

use serde_json::Value;

use crate::llm_client::{
    ClientError, auth::AuthRequirement, capability::ModelCapability,
    client::ConfigWarning, event::Event, scheme::Scheme,
    types::{Request, RequestConfig},
};

use super::OpenAIResponsesScheme;

pub use super::events::OpenAIResponsesState;

impl Scheme for OpenAIResponsesScheme {
    type State = OpenAIResponsesState;

    fn default_base_url(&self) -> &'static str {
        // `/v1` は base_url 側に寄せる。ChatGPT OAuth 経由のときは
        // `https://chatgpt.com/backend-api/codex` を base にすれば同じ
        // `/responses` path で両系統を吸収できる（Codex CLI 準拠）。
        "https://api.openai.com/v1"
    }

    fn path(&self, _model_id: &str) -> String {
        "/responses".to_string()
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

    fn default_capability(&self) -> ModelCapability {
        super::capability::default_capability()
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();
        // ChatGPT backend (codex-oauth) は `max_output_tokens` を 400 で弾く。
        // scheme 構築時に `send_max_output_tokens=false` で組まれていれば
        // body 投影は止まっているので、ユーザの意図が落ちることだけを通知する。
        if !self.send_max_output_tokens && config.max_tokens.is_some() {
            warnings.push(ConfigWarning::unsupported(
                "max_tokens",
                "OpenAI Responses (ChatGPT backend)",
            ));
        }
        // 同上、`temperature` / `top_p` も ChatGPT backend では 400 で弾かれる。
        if !self.send_sampling_params {
            if config.temperature.is_some() {
                warnings.push(ConfigWarning::unsupported(
                    "temperature",
                    "OpenAI Responses (ChatGPT backend)",
                ));
            }
            if config.top_p.is_some() {
                warnings.push(ConfigWarning::unsupported(
                    "top_p",
                    "OpenAI Responses (ChatGPT backend)",
                ));
            }
        }
        warnings
    }
}
