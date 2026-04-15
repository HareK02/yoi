use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::providers::anthropic::AnthropicClient;
use llm_worker::llm_client::providers::gemini::GeminiClient;
use llm_worker::llm_client::providers::ollama::OllamaClient;
use llm_worker::llm_client::providers::openai::OpenAIClient;

use manifest::{ProviderConfig, ProviderKind};

/// Errors from provider client construction.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider configuration error: {0}")]
    Config(String),

    #[error("API key not provided for {provider}")]
    ApiKeyMissing { provider: String },
}

/// Resolve the API key for the given provider configuration.
///
/// Resolution order:
/// 1. Environment variable `INSOMNIA_API_KEY_{KIND}`
/// 2. File specified by `api_key_file` (must be an absolute path; the
///    cascade layer is responsible for normalisation)
/// 3. `None`
fn resolve_api_key(config: &ProviderConfig) -> Result<Option<String>, ProviderError> {
    let env_name = config.kind.env_var_name();
    if let Ok(val) = std::env::var(&env_name) {
        return Ok(Some(val));
    }

    if let Some(ref path) = config.api_key_file {
        if !path.is_absolute() {
            return Err(ProviderError::Config(format!(
                "api_key_file must be absolute: {}",
                path.display()
            )));
        }
        let contents = std::fs::read_to_string(path).map_err(|e| {
            ProviderError::Config(format!(
                "failed to read api_key_file {}: {e}",
                path.display()
            ))
        })?;
        return Ok(Some(contents.trim().to_owned()));
    }

    Ok(None)
}

/// Build an [`LlmClient`] from a [`ProviderConfig`].
///
/// `api_key_file` (if set) must already be an absolute path — relative
/// paths are rejected because cascade resolution is the sole source of
/// path normalisation.
pub fn build_client(config: &ProviderConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    let api_key = resolve_api_key(config)?;

    match config.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| ProviderError::ApiKeyMissing {
                provider: "anthropic".into(),
            })?;
            let mut client = AnthropicClient::new(key, &config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
        ProviderKind::Openai => {
            let key = api_key.ok_or_else(|| ProviderError::ApiKeyMissing {
                provider: "openai".into(),
            })?;
            let mut client = OpenAIClient::new(key, &config.model);
            if let Some(ref url) = config.base_url {
                client = client.with_base_url(url);
            }
            Ok(Box::new(client))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| ProviderError::ApiKeyMissing {
                provider: "gemini".into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use std::path::PathBuf;

    fn anthropic_config() -> ProviderConfig {
        ProviderConfig {
            kind: ProviderKind::Anthropic,
            model: "test-model".into(),
            api_key_file: None,
            base_url: None,
        }
    }

    #[test]
    #[serial]
    fn resolve_from_env() {
        let env_name = ProviderKind::Anthropic.env_var_name();
        unsafe { std::env::set_var(&env_name, "sk-from-env") };
        let key = resolve_api_key(&anthropic_config()).unwrap();
        unsafe { std::env::remove_var(&env_name) };
        assert_eq!(key.as_deref(), Some("sk-from-env"));
    }

    #[test]
    fn resolve_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.txt");
        {
            let mut f = std::fs::File::create(&key_path).unwrap();
            write!(f, "  sk-from-file\n").unwrap();
        }
        let config = ProviderConfig {
            api_key_file: Some(key_path),
            ..anthropic_config()
        };
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key.as_deref(), Some("sk-from-file"));
    }

    #[test]
    #[serial]
    fn env_takes_precedence_over_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.txt");
        std::fs::write(&key_path, "sk-from-file").unwrap();

        let env_name = ProviderKind::Anthropic.env_var_name();
        unsafe { std::env::set_var(&env_name, "sk-from-env") };

        let config = ProviderConfig {
            api_key_file: Some(key_path),
            ..anthropic_config()
        };
        let key = resolve_api_key(&config).unwrap();
        unsafe { std::env::remove_var(&env_name) };
        assert_eq!(key.as_deref(), Some("sk-from-env"));
    }

    #[test]
    fn relative_api_key_file_is_rejected() {
        let config = ProviderConfig {
            api_key_file: Some(PathBuf::from("keys/anthropic")),
            ..anthropic_config()
        };
        let err = resolve_api_key(&config).unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)));
    }

    #[test]
    fn missing_key_returns_api_key_missing() {
        let config = anthropic_config();
        let result = build_client(&config);
        assert!(matches!(result, Err(ProviderError::ApiKeyMissing { .. })));
    }

    #[test]
    fn ollama_succeeds_without_key() {
        let config = ProviderConfig {
            kind: ProviderKind::Ollama,
            model: "llama3".into(),
            api_key_file: None,
            base_url: None,
        };
        assert!(build_client(&config).is_ok());
    }
}
