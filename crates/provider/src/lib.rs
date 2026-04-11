use std::path::{Path, PathBuf};

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
/// 2. File specified by `api_key_file` (trimmed)
/// 3. `None`
fn resolve_api_key(
    config: &ProviderConfig,
    manifest_dir: Option<&Path>,
) -> Result<Option<String>, ProviderError> {
    // 1. Convention-based environment variable
    let env_name = config.kind.env_var_name();
    if let Ok(val) = std::env::var(&env_name) {
        return Ok(Some(val));
    }

    // 2. File
    if let Some(ref raw_path) = config.api_key_file {
        let path = expand_key_path(raw_path, manifest_dir)?;
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            ProviderError::Config(format!("failed to read api_key_file {}: {e}", path.display()))
        })?;
        return Ok(Some(contents.trim().to_owned()));
    }

    Ok(None)
}

/// Expand `~` and resolve relative paths against `manifest_dir`.
fn expand_key_path(
    raw: &Path,
    manifest_dir: Option<&Path>,
) -> Result<PathBuf, ProviderError> {
    let path = if raw.starts_with("~") {
        let home = std::env::var("HOME")
            .map_err(|_| ProviderError::Config("HOME is not set for ~ expansion".into()))?;
        PathBuf::from(home).join(raw.strip_prefix("~").unwrap())
    } else {
        raw.to_path_buf()
    };

    if path.is_relative() {
        match manifest_dir {
            Some(dir) => Ok(dir.join(&path)),
            None => Err(ProviderError::Config(format!(
                "relative api_key_file '{}' requires a manifest directory",
                path.display()
            ))),
        }
    } else {
        Ok(path)
    }
}

/// Build an [`LlmClient`] from a [`ProviderConfig`].
///
/// Resolves the API key from `INSOMNIA_API_KEY_{KIND}` env var or `api_key_file`.
/// `manifest_dir` is used to resolve relative `api_key_file` paths.
pub fn build_client(
    config: &ProviderConfig,
    manifest_dir: Option<&Path>,
) -> Result<Box<dyn LlmClient>, ProviderError> {
    let api_key = resolve_api_key(config, manifest_dir)?;

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
        let key = resolve_api_key(&anthropic_config(), None).unwrap();
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
        let key = resolve_api_key(&config, None).unwrap();
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
        let key = resolve_api_key(&config, None).unwrap();
        unsafe { std::env::remove_var(&env_name) };
        assert_eq!(key.as_deref(), Some("sk-from-env"));
    }

    #[test]
    fn relative_path_resolved_against_manifest_dir() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("keys").join("anthropic");
        std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        std::fs::write(&key_path, "sk-relative").unwrap();

        let config = ProviderConfig {
            api_key_file: Some(PathBuf::from("keys/anthropic")),
            ..anthropic_config()
        };
        let key = resolve_api_key(&config, Some(dir.path())).unwrap();
        assert_eq!(key.as_deref(), Some("sk-relative"));
    }

    #[test]
    fn relative_path_without_manifest_dir_errors() {
        let config = ProviderConfig {
            api_key_file: Some(PathBuf::from("keys/anthropic")),
            ..anthropic_config()
        };
        let err = resolve_api_key(&config, None).unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)));
    }

    #[test]
    fn missing_key_returns_api_key_missing() {
        let config = anthropic_config();
        let result = build_client(&config, None);
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
        assert!(build_client(&config, None).is_ok());
    }
}
