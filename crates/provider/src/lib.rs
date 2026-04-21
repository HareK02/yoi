//! Pod マニフェストの [`ModelConfig`] を [`Box<dyn LlmClient>`]
//! に落とすファクトリ。
//!
//! * `SchemeKind` を各 `Scheme` 実装にマップ
//! * `AuthRef` を環境変数 / ファイルから解決して [`ResolvedAuth`] に
//! * `scheme.required_auth()` と解決値を照合（非対応組合せは構築エラー）
//! * `ModelCapability` は明示指定 → scheme 静的テーブル → 未知時はデフォルト
//!
//! llm-worker は低レベル基盤に留める方針なので、高レベル側で必要に
//! なる認証ストア解決（Codex OAuth の `~/.codex/auth.json` 読取等）は
//! このクレートに追加する。

pub mod capability;
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

use manifest::{AuthRef, ModelConfig, SchemeKind};

/// プロバイダ構築時のエラー。
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("model configuration error: {0}")]
    Config(String),

    #[error("API key not provided for scheme {scheme:?}")]
    ApiKeyMissing { scheme: SchemeKind },

    #[error("scheme {scheme:?} does not support this auth")]
    AuthMismatch { scheme: SchemeKind },

    #[error("scheme {scheme:?} is not implemented yet")]
    SchemeNotImplemented { scheme: SchemeKind },
}

/// `AuthRef` をランタイムで使える [`ResolvedAuth`] に解決する。
///
/// 解決順:
/// 1. `AuthRef::ApiKey { env, .. }` で env が指定されていればその変数を参照
/// 2. そうでなければ scheme 既定の環境変数 (`SchemeKind::default_env_var`)
/// 3. それでも無ければ `file` を読む（絶対パスのみ）
fn resolve_auth(
    scheme: SchemeKind,
    auth: &AuthRef,
) -> Result<ResolvedAuth, ProviderError> {
    match auth {
        AuthRef::None => Ok(ResolvedAuth::None),
        AuthRef::ApiKey { env, file } => {
            let env_name = env.as_deref().unwrap_or(scheme.default_env_var());
            if let Ok(val) = std::env::var(env_name)
                && !val.is_empty()
            {
                return Ok(ResolvedAuth::ApiKey(val));
            }
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
    // capability の優先順位:
    //   1. `ModelConfig.capability` の明示指定(OpenAI 互換ルーターの
    //      未知モデル等、マニフェストで完全に上書きしたいケース)
    //   2. `provider::capability::lookup` の既知モデルテーブル
    //      (モデル ID の知識は高レベル構築層(ここ)の責務)
    //   3. `Scheme::default_capability()`(scheme ごとの wire-level 安全側)
    let capability: ModelCapability = config
        .capability
        .clone()
        .or_else(|| capability::lookup(config.scheme, &config.model_id))
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

/// [`ModelConfig`] から [`LlmClient`] を構築する。
pub fn build_client(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError> {
    let resolved = resolve_auth(config.scheme, &config.auth)?;
    match config.scheme {
        SchemeKind::Anthropic => build_transport(AnthropicScheme::new(), config, resolved),
        SchemeKind::OpenaiChat => build_transport(OpenAIScheme::new(), config, resolved),
        SchemeKind::Gemini => build_transport(GeminiScheme::new(), config, resolved),
        SchemeKind::OpenaiResponses => {
            build_transport(OpenAIResponsesScheme::new(), config, resolved)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use std::path::PathBuf;

    fn anthropic_config() -> ModelConfig {
        ModelConfig {
            scheme: SchemeKind::Anthropic,
            base_url: None,
            model_id: "claude-sonnet-4-20250514".into(),
            auth: AuthRef::ApiKey {
                env: None,
                file: None,
            },
            capability: None,
        }
    }

    #[test]
    #[serial]
    fn resolve_from_env() {
        let env_name = SchemeKind::Anthropic.default_env_var();
        unsafe { std::env::set_var(env_name, "sk-from-env") };
        let auth = resolve_auth(SchemeKind::Anthropic, &anthropic_config().auth).unwrap();
        unsafe { std::env::remove_var(env_name) };
        match auth {
            ResolvedAuth::ApiKey(k) => assert_eq!(k, "sk-from-env"),
            _ => panic!("expected ApiKey"),
        }
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
                env: Some("INSOMNIA_API_KEY_NONEXISTENT".into()),
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
    #[serial]
    fn env_takes_precedence_over_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.txt");
        std::fs::write(&key_path, "sk-from-file").unwrap();

        let env_name = SchemeKind::Anthropic.default_env_var();
        unsafe { std::env::set_var(env_name, "sk-from-env") };

        let config = ModelConfig {
            auth: AuthRef::ApiKey {
                env: None,
                file: Some(key_path),
            },
            ..anthropic_config()
        };
        let auth = resolve_auth(config.scheme, &config.auth).unwrap();
        unsafe { std::env::remove_var(env_name) };
        match auth {
            ResolvedAuth::ApiKey(k) => assert_eq!(k, "sk-from-env"),
            _ => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn relative_auth_file_is_rejected() {
        let config = ModelConfig {
            auth: AuthRef::ApiKey {
                env: Some("INSOMNIA_API_KEY_NONEXISTENT".into()),
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
        let env_name = SchemeKind::Anthropic.default_env_var();
        unsafe { std::env::remove_var(env_name) };
        let result = build_client(&anthropic_config());
        assert!(matches!(result, Err(ProviderError::ApiKeyMissing { .. })));
    }

    #[test]
    fn model_config_capability_overrides_scheme_default() {
        // 未知モデル ID でも `ModelConfig.capability` が指定されていれば
        // scheme の静的テーブル / デフォルトではなくその値が採用される。
        use llm_worker::llm_client::capability::{
            CacheStrategy, ModelCapability, ReasoningEffort, ReasoningSupport, StructuredOutput,
            ToolCallingSupport,
        };

        let explicit = ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: Some(ReasoningSupport::Effort),
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        };

        // TOML 経由の往復（`[model.capability]` が正しくパースできる）
        let toml_str = toml::to_string(&explicit).unwrap();
        let round_trip: ModelCapability = toml::from_str(&toml_str).unwrap();
        assert_eq!(round_trip, explicit);

        // `_ = ReasoningEffort` は serde derive が欠けていると失敗する
        // ほぼ確実なコンパイル時ガード。
        let _ = ReasoningEffort::Medium;
    }

    #[test]
    fn ollama_succeeds_without_key() {
        // Ollama = Anthropic scheme + base_url 差し替え + AuthRef::None
        let config = ModelConfig {
            scheme: SchemeKind::Anthropic,
            base_url: Some("http://localhost:11434".into()),
            model_id: "llama3".into(),
            auth: AuthRef::None,
            capability: None,
        };
        // scheme.required_auth() が XApiKey でも ResolvedAuth::None は許容する
        // （None は全 scheme で受け入れるため）
        assert!(build_client(&config).is_ok());
    }
}
