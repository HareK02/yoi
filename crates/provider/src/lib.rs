//! Pod マニフェストの [`ModelManifest`] を [`Box<dyn LlmClient>`]
//! に落とすファクトリ。
//!
//! 段階:
//! 1. `ModelManifest` を [`catalog::resolve_model_manifest`] で
//!    カタログ込み [`ModelConfig`] に解決（ref → 展開 / inline → 検証）
//! 2. `AuthRef` を local secret store / ファイルから解決して [`ResolvedAuth`] に
//! 3. `scheme.required_auth()` と解決値を照合（非対応組合せは構築エラー）
//! 4. `ModelCapability` は manifest 明示 > model catalog > provider
//!    default_capability > scheme 既定 の順でフォールバック（上位 3 段は
//!    `catalog::resolve_model_manifest` が [`ModelConfig`] に詰め込む）
//!
//! llm-worker は低レベル基盤に留める方針なので、高レベル側で必要に
//! なる認証ストア解決（Codex OAuth の `~/.codex/auth.json` 読取等）は
//! このクレートに追加する。

pub mod catalog;
pub mod codex_oauth;

use std::sync::Arc;

use llm_worker::llm_client::{
    LlmClient,
    capability::ModelCapability,
    scheme::{
        Scheme, anthropic::AnthropicScheme, gemini::GeminiScheme, openai_chat::OpenAIScheme,
        openai_responses::OpenAIResponsesScheme,
    },
    transport::{HttpTransport, ResolvedAuth},
};

use manifest::{AuthRef, ModelManifest, SchemeKind};
use secrets::{SecretStore, SecretValue};

pub use catalog::{ModelConfig, ResolveError as CatalogResolveError};

/// プロバイダ構築時のエラー。
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

/// `AuthRef` をランタイムで使える [`ResolvedAuth`] に解決する。
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
        let data_dir = manifest::paths::data_dir().ok_or_else(|| secrets::Error::Read {
            path: std::path::PathBuf::from("<data_dir>"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not determine insomnia data directory",
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
            let provider = codex_oauth::CodexAuthProvider::from_default_home()
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

/// `AuthRef::CodexOAuth` 指定時、`base_url` 未指定なら ChatGPT backend を既定とする。
/// Codex CLI が使う `/backend-api/codex` を base に取り、scheme 側の `/responses`
/// path と結合して `https://chatgpt.com/backend-api/codex/responses` になる。
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
) -> Result<Box<dyn LlmClient>, ProviderError> {
    if !resolved.matches(scheme.required_auth()) {
        return Err(ProviderError::AuthMismatch {
            scheme: config.scheme,
        });
    }
    // capability の優先順位 (上位 3 段は `ModelConfig` に既に反映済み):
    //   1. manifest 明示
    //   2. model catalog
    //   3. provider.default_capability
    //   4. `Scheme::default_capability()`（scheme ごとの wire-level 安全側）
    let capability: ModelCapability = config
        .capability
        .clone()
        .unwrap_or_else(|| scheme.default_capability());
    let base_url = effective_base_url(&scheme, config);
    Ok(Box::new(HttpTransport::new(
        scheme,
        config.model_id.clone(),
        base_url,
        resolved,
        capability,
    )))
}

fn build_from_config(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    let resolved = resolve_auth(config.scheme, &config.auth)?;
    match config.scheme {
        SchemeKind::Anthropic => build_transport(AnthropicScheme::new(), config, resolved),
        SchemeKind::OpenaiChat => build_transport(OpenAIScheme::new(), config, resolved),
        SchemeKind::Gemini => build_transport(GeminiScheme::new(), config, resolved),
        SchemeKind::OpenaiResponses => {
            // ChatGPT backend (codex-oauth) は `max_output_tokens` /
            // `temperature` / `top_p` を 400 で弾くため、その経路では
            // 送出を止める。
            let send_to_official = !matches!(config.auth, AuthRef::CodexOAuth);
            let scheme = OpenAIResponsesScheme::new()
                .with_send_max_output_tokens(send_to_official)
                .with_send_sampling_params(send_to_official);
            build_transport(scheme, config, resolved)
        }
    }
}

/// [`ModelManifest`] から [`LlmClient`] を構築する。ref / inline の
/// いずれも受け取り、カタログ解決は内部で行う。
pub fn build_client(manifest: &ModelManifest) -> Result<Box<dyn LlmClient>, ProviderError> {
    let config = catalog::resolve_model_manifest(manifest)?;
    build_from_config(&config)
}

/// 既に解決済みの [`ModelConfig`] から [`LlmClient`] を構築する。
/// `ModelManifest` から既に `catalog::resolve_model_manifest` を通した
/// ケース（factory / spawn 経路でカタログ引きを 1 回だけにしたい等）で
/// 使う。
pub fn build_client_from_config(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    build_from_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use std::path::PathBuf;

    struct TestSecrets(std::collections::BTreeMap<String, String>);

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
    fn ref_manifest_builds_client() {
        // Ollama は AuthRef::None で構築できる end-to-end path。
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
    fn inline_manifest_builds_client() {
        // Form C: 完全直書き。Ollama 相当を AuthRef::None で構築。
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
        // Ollama = Anthropic scheme + base_url 差し替え + AuthRef::None
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
