//! Worker-side factory converting [`ModelManifest`] into [`Box<dyn LlmClient>`].
//!
//! Steps:
//! 1. Resolve `ModelManifest` through [`catalog::resolve_model_manifest`] into catalog-aware
//!    [`ModelConfig`] (expand refs; validate inline configuration).
//! 2. Resolve `AuthRef` from the local secret store / files into [`ResolvedAuth`].
//! 3. Check the resolved value against `scheme.required_auth()`; unsupported pairs fail construction.
//! 4. `ModelCapability` falls back in order: explicit manifest > model catalog > provider
//!    default_capability > scheme default. `catalog::resolve_model_manifest` puts the first three
//!    levels into [`ModelConfig`].
//!
//! agen remains low-level infrastructure; worker handles the higher-level auth-store and secret-store
//! resolution.

use std::sync::Arc;

use agen::llm_client::{
    LlmClient,
    capability::ModelCapability,
    scheme::{
        Scheme, anthropic::AnthropicScheme, gemini::GeminiScheme, openai_chat::OpenAIScheme,
        openai_responses::OpenAIResponsesScheme,
    },
    transport::{HttpTransport, ResolvedAuth, TransportPolicy},
};
use agen::providers::codex::CodexAuthProvider;

use manifest::{AuthRef, ModelManifest, SchemeKind, model_catalog as catalog};
use secrets::{SecretStore, SecretValue};

pub use manifest::model_catalog::{ModelConfig, ResolveError as CatalogResolveError};

/// Errors during provider construction.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("model configuration error: {0}")]
    Config(String),

    #[error("API key not provided for scheme {scheme:?}")]
    ApiKeyMissing { scheme: SchemeKind },

    #[error("failed to resolve secret `{id}`: {source}")]
    SecretStore {
        id: String,
        #[source]
        source: secrets::Error,
    },

    #[error("scheme {scheme:?} does not support this auth")]
    AuthMismatch { scheme: SchemeKind },

    #[error("scheme {scheme:?} is not implemented yet")]
    SchemeNotImplemented { scheme: SchemeKind },

    #[error("failed to resolve model manifest: {0}")]
    ManifestResolve(#[from] catalog::ResolveError),
}

/// Resolve `AuthRef` into runtime-usable [`ResolvedAuth`].
fn resolve_auth(scheme: SchemeKind, auth: &AuthRef) -> Result<ResolvedAuth, ProviderError> {
    let resolver = DefaultSecretResolver;
    resolve_auth_with_resolver(scheme, auth, &resolver)
}

trait SecretResolver {
    fn get_secret(&self, id: &str) -> Result<SecretValue, secrets::Error>;
}

struct DefaultSecretResolver;

impl SecretResolver for DefaultSecretResolver {
    fn get_secret(&self, id: &str) -> Result<SecretValue, secrets::Error> {
        let data_dir = manifest::paths::secret_data_dir().ok_or_else(|| secrets::Error::Read {
            path: std::path::PathBuf::from("<secret_data_dir>"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not determine yoi secret data directory",
            ),
        })?;
        SecretStore::new(data_dir).get(id)
    }
}

fn resolve_auth_with_resolver(
    scheme: SchemeKind,
    auth: &AuthRef,
    resolver: &dyn SecretResolver,
) -> Result<ResolvedAuth, ProviderError> {
    match auth {
        AuthRef::None => Ok(ResolvedAuth::None),
        AuthRef::ApiKey { file } => {
            if let Some(path) = file {
                if !path.is_absolute() {
                    return Err(ProviderError::Config(format!(
                        "auth.file must be absolute: {}",
                        path.display()
                    )));
                }
                let contents = std::fs::read_to_string(path).map_err(|e| {
                    ProviderError::Config(format!(
                        "failed to read auth.file {}: {e}",
                        path.display()
                    ))
                })?;
                return Ok(ResolvedAuth::ApiKey(contents.trim().to_owned()));
            }
            Err(ProviderError::ApiKeyMissing { scheme })
        }
        AuthRef::CodexOAuth => {
            let provider = CodexAuthProvider::from_default_home()
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(ResolvedAuth::Custom(Arc::new(provider)))
        }
        AuthRef::SecretRef { ref_ } => {
            let value = resolver
                .get_secret(ref_)
                .map_err(|source| ProviderError::SecretStore {
                    id: ref_.clone(),
                    source,
                })?;
            Ok(ResolvedAuth::ApiKey(value.into_string()))
        }
    }
}

/// `AuthRef::CodexOAuth` defaults to the ChatGPT backend when `base_url` is unset.
/// The Codex CLI base `/backend-api/codex` combines with the scheme's `/responses` path
/// to form `https://chatgpt.com/backend-api/codex/responses`.
fn effective_base_url<S: Scheme>(scheme: &S, config: &ModelConfig) -> String {
    if let Some(b) = &config.base_url {
        return b.clone();
    }
    if matches!(config.auth, AuthRef::CodexOAuth) {
        return "https://chatgpt.com/backend-api/codex".to_string();
    }
    scheme.default_base_url().to_string()
}

fn build_transport<S: Scheme>(
    scheme: S,
    config: &ModelConfig,
    resolved: ResolvedAuth,
    policy: TransportPolicy,
) -> Result<Box<dyn LlmClient>, ProviderError> {
    if !resolved.matches(scheme.required_auth()) {
        return Err(ProviderError::AuthMismatch {
            scheme: config.scheme,
        });
    }
    // Capability precedence (the first three levels are already reflected in `ModelConfig`):
    //   1. explicit manifest
    //   2. model catalog
    //   3. provider.default_capability
    //   4. `Scheme::default_capability()` (scheme-specific conservative wire-level default)
    let capability: ModelCapability = config
        .capability
        .clone()
        .unwrap_or_else(|| scheme.default_capability());
    let base_url = effective_base_url(&scheme, config);
    Ok(Box::new(
        HttpTransport::new(
            scheme,
            config.model_id.clone(),
            base_url,
            resolved,
            capability,
        )
        .with_transport_policy(policy),
    ))
}

fn build_from_config(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    let resolved = resolve_auth(config.scheme, &config.auth)?;
    match config.scheme {
        SchemeKind::Anthropic => build_transport(
            AnthropicScheme::new(),
            config,
            resolved,
            TransportPolicy::standard(),
        ),
        SchemeKind::OpenaiChat => build_transport(
            OpenAIScheme::new(),
            config,
            resolved,
            TransportPolicy::standard(),
        ),
        SchemeKind::Gemini => build_transport(
            GeminiScheme::new(),
            config,
            resolved,
            TransportPolicy::standard(),
        ),
        SchemeKind::OpenaiResponses => {
            // The ChatGPT backend (codex-oauth) rejects `max_output_tokens`,
            // `temperature`, and `top_p` with HTTP 400; omit them on that path.
            let send_to_official = !matches!(config.auth, AuthRef::CodexOAuth);
            let scheme = OpenAIResponsesScheme::new()
                .with_send_max_output_tokens(send_to_official)
                .with_send_sampling_params(send_to_official);
            let policy = if matches!(config.auth, AuthRef::CodexOAuth) {
                TransportPolicy::openai_compatible_zstd()
            } else {
                TransportPolicy::standard()
            };
            build_transport(scheme, config, resolved, policy)
        }
    }
}

/// Build an [`LlmClient`] from a [`ModelManifest`], accepting refs or inline configuration
/// and resolving the catalog internally.
pub fn build_client(manifest: &ModelManifest) -> Result<Box<dyn LlmClient>, ProviderError> {
    let config = catalog::resolve_model_manifest(manifest)?;
    build_from_config(&config)
}

/// Build an [`LlmClient`] from an already resolved [`ModelConfig`].
/// Use when `ModelManifest` has already passed through `catalog::resolve_model_manifest`,
/// for example to resolve the catalog only once in factory / spawn paths.
pub fn build_client_from_config(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    build_from_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    struct TestSecrets(std::collections::BTreeMap<String, String>);

    struct ConfigDirGuard {
        prev: Option<String>,
    }

    impl ConfigDirGuard {
        fn new(path: &Path) -> Self {
            let prev = std::env::var("YOI_CONFIG_DIR").ok();
            // SAFETY: tests using this guard are marked `#[serial]`.
            unsafe { std::env::set_var("YOI_CONFIG_DIR", path) };
            Self { prev }
        }
    }

    impl Drop for ConfigDirGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("YOI_CONFIG_DIR", v),
                    None => std::env::remove_var("YOI_CONFIG_DIR"),
                }
            }
        }
    }

    impl SecretResolver for TestSecrets {
        fn get_secret(&self, id: &str) -> Result<SecretValue, secrets::Error> {
            self.0
                .get(id)
                .cloned()
                .map(SecretValue::new)
                .ok_or_else(|| secrets::Error::NotFound { id: id.to_string() })
        }
    }

    fn anthropic_config() -> ModelConfig {
        ModelConfig {
            scheme: SchemeKind::Anthropic,
            base_url: None,
            model_id: "claude-sonnet-4-20250514".into(),
            auth: AuthRef::ApiKey { file: None },
            capability: None,
            context_window: 200_000,
            max_context_window: None,
        }
    }

    #[test]
    fn resolve_from_secret_ref() {
        let resolver = TestSecrets(std::collections::BTreeMap::from([(
            "providers/anthropic/default".to_string(),
            "sk-from-store".to_string(),
        )]));
        let auth = resolve_auth_with_resolver(
            SchemeKind::Anthropic,
            &AuthRef::SecretRef {
                ref_: "providers/anthropic/default".into(),
            },
            &resolver,
        )
        .unwrap();
        match auth {
            ResolvedAuth::ApiKey(k) => assert_eq!(k, "sk-from-store"),
            _ => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn missing_secret_names_only_id() {
        let resolver = TestSecrets(Default::default());
        let err = resolve_auth_with_resolver(
            SchemeKind::Anthropic,
            &AuthRef::SecretRef {
                ref_: "providers/anthropic/missing".into(),
            },
            &resolver,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("providers/anthropic/missing"));
        assert!(!message.contains("sk-"));
    }

    #[test]
    fn resolve_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.txt");
        {
            let mut f = std::fs::File::create(&key_path).unwrap();
            writeln!(f, "  sk-from-file").unwrap();
        }
        let config = ModelConfig {
            auth: AuthRef::ApiKey {
                file: Some(key_path),
            },
            ..anthropic_config()
        };
        let auth = resolve_auth(config.scheme, &config.auth).unwrap();
        match auth {
            ResolvedAuth::ApiKey(k) => assert_eq!(k, "sk-from-file"),
            _ => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn relative_auth_file_is_rejected() {
        let config = ModelConfig {
            auth: AuthRef::ApiKey {
                file: Some(PathBuf::from("keys/anthropic")),
            },
            ..anthropic_config()
        };
        let err = resolve_auth(config.scheme, &config.auth).unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)));
    }

    #[test]
    #[serial]
    fn missing_key_returns_api_key_missing() {
        let result = build_client_from_config(&anthropic_config());
        assert!(matches!(result, Err(ProviderError::ApiKeyMissing { .. })));
    }

    #[test]
    #[serial]
    fn ref_manifest_builds_client() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::new(dir.path());

        // End-to-end path building Ollama with AuthRef::None.
        let manifest = ModelManifest {
            ref_: Some("ollama-local/llama3.1".into()),
            ..Default::default()
        };
        let client = build_client(&manifest);
        assert!(
            client.is_ok(),
            "ollama ref should build without credentials: {:?}",
            client.err()
        );
    }

    #[test]
    #[serial]
    fn inline_manifest_builds_client() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ConfigDirGuard::new(dir.path());

        // Form C: fully inline. Build an Ollama equivalent with AuthRef::None.
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            base_url: Some("http://localhost:11434".into()),
            model_id: Some("llama3".into()),
            auth: Some(AuthRef::None),
            ..Default::default()
        };
        let client = build_client(&manifest);
        assert!(
            client.is_ok(),
            "inline ollama config should build: {:?}",
            client.err()
        );
    }

    #[test]
    fn ollama_config_succeeds_without_key() {
        // Ollama = Anthropic scheme + overridden base_url + AuthRef::None
        let config = ModelConfig {
            scheme: SchemeKind::Anthropic,
            base_url: Some("http://localhost:11434".into()),
            model_id: "llama3".into(),
            auth: AuthRef::None,
            capability: None,
            context_window: 200_000,
            max_context_window: None,
        };
        assert!(build_client_from_config(&config).is_ok());
    }
}
