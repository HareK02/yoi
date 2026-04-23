//! プロバイダ/モデルカタログ。
//!
//! builtin (`assets/providers.toml`) と user override
//! (`$XDG_CONFIG_HOME/insomnia/providers.toml`) を読み、
//! `Vec<ProviderEntry>` を返す。user override がある場合は builtin を
//! 置き換える（マージしない）。
//!
//! `ProviderEntry` から [`ModelConfig`] への変換は
//! [`ProviderEntry::to_model_config`] で行う。`auth_hint` はここでは
//! UI 表示用のヒントで、実際の認証解決は従来通り [`crate::build_client`]
//! が `AuthRef` から行う。

use std::path::{Path, PathBuf};

use manifest::{AuthRef, ModelConfig, SchemeKind};
use serde::{Deserialize, Serialize};

const BUILTIN_CATALOG: &str = include_str!("../assets/providers.toml");

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("failed to read catalog at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse catalog at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to parse builtin catalog: {0}")]
    BuiltinParse(#[source] toml::de::Error),
}

/// UI 向けの認証ヒント。
///
/// 「何を表示・要求するか」のメタ情報で、ランタイムの [`AuthRef`]
/// とは責務が別。1:1 の対応関係にあり、
/// [`ProviderEntry::to_model_config`] で相互変換される。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthHint {
    /// 認証不要（ローカル Ollama 等）
    None,
    /// API key。`env` が指定されていれば UI はその env 名を提示する
    ApiKey {
        #[serde(default)]
        env: Option<String>,
    },
    /// ChatGPT OAuth（`~/.codex/auth.json`）
    #[serde(rename = "codex_oauth")]
    CodexOAuth,
}

/// カタログ 1 エントリ。
///
/// 将来 `discover: Option<DiscoverMode>` を任意で追加予定（Ollama
/// `/api/tags` 等の動的モデル列挙）。別チケットで実装する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderEntry {
    pub id: String,
    pub display_name: String,
    pub scheme: SchemeKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth_hint: AuthHint,
    #[serde(default)]
    pub default_models: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    provider: Vec<ProviderEntry>,
}

impl ProviderEntry {
    /// 選ばれた `model_id` と組み合わせて [`ModelConfig`] を構築する。
    pub fn to_model_config(&self, model_id: impl Into<String>) -> ModelConfig {
        let auth = match &self.auth_hint {
            AuthHint::None => AuthRef::None,
            AuthHint::ApiKey { env } => AuthRef::ApiKey {
                env: env.clone(),
                file: None,
            },
            AuthHint::CodexOAuth => AuthRef::CodexOAuth,
        };
        ModelConfig {
            scheme: self.scheme,
            base_url: self.base_url.clone(),
            model_id: model_id.into(),
            auth,
            capability: None,
        }
    }
}

/// builtin + user override を解決してカタログを返す。
///
/// user override (`$XDG_CONFIG_HOME/insomnia/providers.toml`) が
/// 存在すれば builtin を置き換える。存在しなければ builtin のみ。
/// user override が存在するが壊れている場合はエラーを返す（silent
/// fallback はしない — ユーザーが書いた設定が silent に無視されて
/// builtin に戻る挙動は気付きにくいため）。
pub fn load() -> Result<Vec<ProviderEntry>, CatalogError> {
    if let Some(path) = user_override_path()
        && path.is_file()
    {
        return load_from_path(&path);
    }
    load_builtin()
}

/// builtin カタログ (`assets/providers.toml`) のみを返す。
pub fn load_builtin() -> Result<Vec<ProviderEntry>, CatalogError> {
    let parsed: CatalogFile =
        toml::from_str(BUILTIN_CATALOG).map_err(CatalogError::BuiltinParse)?;
    Ok(parsed.provider)
}

/// 指定パスから読む（テスト・明示指定用）。
pub fn load_from_path(path: &Path) -> Result<Vec<ProviderEntry>, CatalogError> {
    let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: CatalogFile = toml::from_str(&text).map_err(|source| CatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(parsed.provider)
}

fn user_override_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("insomnia").join("providers.toml"));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("insomnia")
                .join("providers.toml"),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_client;
    use serial_test::serial;

    #[test]
    fn builtin_has_four_entries() {
        let entries = load_builtin().unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["anthropic", "ollama-local", "codex-oauth", "openrouter"]
        );
    }

    #[test]
    fn builtin_ollama_shape() {
        let entries = load_builtin().unwrap();
        let ollama = entries.iter().find(|e| e.id == "ollama-local").unwrap();
        assert_eq!(ollama.scheme, SchemeKind::Anthropic);
        assert_eq!(
            ollama.base_url.as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(ollama.auth_hint, AuthHint::None);
        assert!(!ollama.default_models.is_empty());
    }

    #[test]
    fn builtin_codex_oauth_shape() {
        let entries = load_builtin().unwrap();
        let codex = entries.iter().find(|e| e.id == "codex-oauth").unwrap();
        assert_eq!(codex.scheme, SchemeKind::OpenaiResponses);
        assert_eq!(codex.auth_hint, AuthHint::CodexOAuth);
        // base_url 未指定 → Codex OAuth のデフォルト backend に解決される
        assert!(codex.base_url.is_none());
    }

    #[test]
    fn builtin_openrouter_uses_explicit_env() {
        let entries = load_builtin().unwrap();
        let router = entries.iter().find(|e| e.id == "openrouter").unwrap();
        match &router.auth_hint {
            AuthHint::ApiKey { env } => {
                assert_eq!(env.as_deref(), Some("INSOMNIA_API_KEY_OPENROUTER"));
            }
            _ => panic!("openrouter should use ApiKey hint"),
        }
    }

    #[test]
    fn to_model_config_maps_auth_hint() {
        let entries = load_builtin().unwrap();

        let ollama = entries.iter().find(|e| e.id == "ollama-local").unwrap();
        let cfg = ollama.to_model_config("llama3");
        assert_eq!(cfg.auth, AuthRef::None);
        assert_eq!(cfg.model_id, "llama3");

        let router = entries.iter().find(|e| e.id == "openrouter").unwrap();
        let cfg = router.to_model_config("openai/gpt-5");
        match cfg.auth {
            AuthRef::ApiKey { env, file } => {
                assert_eq!(env.as_deref(), Some("INSOMNIA_API_KEY_OPENROUTER"));
                assert!(file.is_none());
            }
            _ => panic!("expected ApiKey"),
        }

        let codex = entries.iter().find(|e| e.id == "codex-oauth").unwrap();
        let cfg = codex.to_model_config("gpt-5");
        assert_eq!(cfg.auth, AuthRef::CodexOAuth);
    }

    #[test]
    fn ollama_entry_builds_client() {
        // カタログ読取 → ProviderEntry 選択 → ModelConfig 生成 →
        // build_client が成功する end-to-end path。
        let entries = load_builtin().unwrap();
        let ollama = entries.iter().find(|e| e.id == "ollama-local").unwrap();
        let cfg = ollama.to_model_config("llama3");
        assert!(build_client(&cfg).is_ok());
    }

    #[test]
    fn load_from_path_reads_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(
            &path,
            r#"
[[provider]]
id = "custom"
display_name = "Custom"
scheme = "anthropic"
base_url = "http://example.com"
auth_hint = { kind = "none" }
default_models = ["model-x"]
"#,
        )
        .unwrap();
        let entries = load_from_path(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "custom");
    }

    #[test]
    fn malformed_override_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, "this is not valid ][ toml").unwrap();
        let err = load_from_path(&path).unwrap_err();
        assert!(matches!(err, CatalogError::Parse { .. }));
    }

    #[test]
    #[serial]
    fn load_prefers_override_over_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let insomnia_dir = dir.path().join("insomnia");
        std::fs::create_dir_all(&insomnia_dir).unwrap();
        std::fs::write(
            insomnia_dir.join("providers.toml"),
            r#"
[[provider]]
id = "only-one"
display_name = "Only"
scheme = "anthropic"
auth_hint = { kind = "none" }
"#,
        )
        .unwrap();

        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let entries = load().unwrap();
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "only-one");
    }

    #[test]
    #[serial]
    fn load_falls_back_to_builtin_when_override_absent() {
        let dir = tempfile::tempdir().unwrap();
        // override ファイルは作らない
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let entries = load().unwrap();
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        assert_eq!(entries.len(), 4);
    }
}
