use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::providers::anthropic::AnthropicClient;
use llm_worker::llm_client::providers::gemini::GeminiClient;
use llm_worker::llm_client::providers::ollama::OllamaClient;
use llm_worker::llm_client::providers::openai::OpenAIClient;

use crate::manifest::{ProviderConfig, ProviderKind};
use crate::pod::PodError;

/// Build an [`LlmClient`] from a [`ProviderConfig`].
///
/// Resolves the API key from the environment variable specified in the config.
pub fn build_client(config: &ProviderConfig) -> Result<Box<dyn LlmClient>, PodError> {
    let api_key = config
        .api_key_env
        .as_deref()
        .map(std::env::var)
        .transpose()
        .map_err(|e| PodError::ProviderConfig(format!("env var: {e}")))?;

    match config.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                PodError::ProviderConfig("anthropic requires api_key_env".into())
            })?;
            let mut client = AnthropicClient::new(key, &config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
        ProviderKind::Openai => {
            let key = api_key.ok_or_else(|| {
                PodError::ProviderConfig("openai requires api_key_env".into())
            })?;
            let mut client = OpenAIClient::new(key, &config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| {
                PodError::ProviderConfig("gemini requires api_key_env".into())
            })?;
            let mut client = GeminiClient::new(key, &config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
        ProviderKind::Ollama => {
            let mut client = OllamaClient::new(&config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
    }
}
