//! プロバイダ / モデルカタログ。
//!
//! - builtin プロバイダ: `resources/providers/builtin.toml`
//! - builtin モデル:    `resources/models/builtin.toml`
//! - user override:     `$XDG_CONFIG_HOME/insomnia/{providers,models}.toml`
//!
//! どちらの override も「あれば builtin を置換、無ければ builtin」と
//! いう一方向の差し替え（マージしない）。providers / models は独立に
//! 読み、片方だけ user override も可。
//!
//! [`resolve_model_manifest`] が `manifest::ModelManifest`（ref / inline
//! 両形）を最終的な [`ModelConfig`] に解決する単一の入口で、wire 層
//! に渡す前のバリデーションもここで行う。

use std::path::{Path, PathBuf};

use llm_worker::llm_client::capability::ModelCapability;
use manifest::{AuthRef, ModelManifest, SchemeKind};
use serde::{Deserialize, Serialize};

const BUILTIN_PROVIDERS: &str = include_str!("../../../resources/providers/builtin.toml");
const BUILTIN_MODELS: &str = include_str!("../../../resources/models/builtin.toml");

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

/// マニフェスト解決時のエラー。`ModelManifest` がカタログ参照を満たせ
/// ない、あるいは inline フォームでも必須フィールドが揃わない場合に
/// 返す。
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("failed to load provider catalog: {0}")]
    LoadProviders(#[source] CatalogError),
    #[error("failed to load model catalog: {0}")]
    LoadModels(#[source] CatalogError),
    #[error("invalid model.ref `{0}`: must be `<provider>/<model_id>`")]
    MalformedRef(String),
    #[error("model.ref points to unknown provider `{0}`")]
    UnknownProvider(String),
    #[error(
        "model.ref omitted; manifest must specify scheme, model_id, and auth (missing: {0})"
    )]
    InlineMissing(&'static str),
}

/// UI 向けの認証ヒント。
///
/// 「何を表示・要求するか」のメタ情報で、ランタイムの [`AuthRef`]
/// とは責務が別。1:1 の対応関係にあり、[`auth_hint_to_ref`] / [`AuthHint`]
/// 解決経路で相互変換される。
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

/// プロバイダカタログの 1 エントリ。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderEntry {
    pub id: String,
    pub display_name: String,
    pub scheme: SchemeKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth_hint: AuthHint,
    /// モデルカタログ未登録モデルでこの provider が使われたとき
    /// （ref で provider はあるが model 行は無い等）のフォールバック。
    /// 省略時は `Scheme::default_capability()` を最終フォールバックに
    /// 使う。
    #[serde(default)]
    pub default_capability: Option<ModelCapability>,
}

/// モデルカタログの 1 エントリ。
///
/// `id` は **provider 内ユニーク**。同じ `gpt-5` が異なる provider に
/// 存在するのは OK で、ref が必ず `<provider>/<model_id>` を含むため
/// 曖昧性が出ない。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    /// モデル単位の capability override。省略時は
    /// `ProviderEntry::default_capability` にフォールバックする。
    #[serde(default)]
    pub capability: Option<ModelCapability>,
}

/// 解決済みモデル設定。`build_client` が消費する完成形。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub scheme: SchemeKind,
    pub base_url: Option<String>,
    pub model_id: String,
    pub auth: AuthRef,
    pub capability: Option<ModelCapability>,
}

#[derive(Debug, Deserialize)]
struct ProviderCatalogFile {
    #[serde(default)]
    provider: Vec<ProviderEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelCatalogFile {
    #[serde(default)]
    model: Vec<ModelEntry>,
}

/// `auth_hint` に対応する [`AuthRef`] のひな型を返す。env / file は
/// マニフェスト側で override 可能なので、ここでは hint そのままを
/// 反映した最小形だけを返す（`AuthRef::ApiKey { env: hint_env, file: None }`）。
fn auth_hint_to_ref(hint: &AuthHint) -> AuthRef {
    match hint {
        AuthHint::None => AuthRef::None,
        AuthHint::ApiKey { env } => AuthRef::ApiKey {
            env: env.clone(),
            file: None,
        },
        AuthHint::CodexOAuth => AuthRef::CodexOAuth,
    }
}

// --- providers ---------------------------------------------------------------

/// builtin + user override を解決して provider カタログを返す。
///
/// user override (`$XDG_CONFIG_HOME/insomnia/providers.toml`) が
/// 存在すれば builtin を置き換える。存在しなければ builtin のみ。
/// user override が存在するが壊れている場合はエラーを返す（silent
/// fallback はしない — ユーザーが書いた設定が silent に無視されて
/// builtin に戻る挙動は気付きにくいため）。
pub fn load_providers() -> Result<Vec<ProviderEntry>, CatalogError> {
    if let Some(path) = user_override_path("providers.toml")
        && path.is_file()
    {
        return load_providers_from(&path);
    }
    load_builtin_providers()
}

/// builtin provider カタログのみを返す。
pub fn load_builtin_providers() -> Result<Vec<ProviderEntry>, CatalogError> {
    let parsed: ProviderCatalogFile =
        toml::from_str(BUILTIN_PROVIDERS).map_err(CatalogError::BuiltinParse)?;
    Ok(parsed.provider)
}

/// 指定パスから provider カタログを読む。
pub fn load_providers_from(path: &Path) -> Result<Vec<ProviderEntry>, CatalogError> {
    let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: ProviderCatalogFile =
        toml::from_str(&text).map_err(|source| CatalogError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(parsed.provider)
}

// --- models ------------------------------------------------------------------

/// builtin + user override を解決してモデルカタログを返す。
pub fn load_models() -> Result<Vec<ModelEntry>, CatalogError> {
    if let Some(path) = user_override_path("models.toml")
        && path.is_file()
    {
        return load_models_from(&path);
    }
    load_builtin_models()
}

/// builtin model カタログのみを返す。
pub fn load_builtin_models() -> Result<Vec<ModelEntry>, CatalogError> {
    let parsed: ModelCatalogFile =
        toml::from_str(BUILTIN_MODELS).map_err(CatalogError::BuiltinParse)?;
    Ok(parsed.model)
}

/// 指定パスからモデルカタログを読む。
pub fn load_models_from(path: &Path) -> Result<Vec<ModelEntry>, CatalogError> {
    let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: ModelCatalogFile = toml::from_str(&text).map_err(|source| CatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(parsed.model)
}

// --- ref 解決 / マニフェスト → ModelConfig ---------------------------------

/// `<provider_id>/<model_id>` の最初の `/` で 1 回だけ split する。
/// OpenRouter の `openrouter/anthropic/claude-sonnet-4` のように
/// model_id に `/` を含むケースは、provider=`openrouter`、
/// model_id=`anthropic/claude-sonnet-4` として通る。
fn split_ref(s: &str) -> Option<(&str, &str)> {
    let (provider, rest) = s.split_once('/')?;
    if provider.is_empty() || rest.is_empty() {
        return None;
    }
    Some((provider, rest))
}

/// `ModelManifest` をカタログ込みで解決し、最終 [`ModelConfig`] を返す。
///
/// - **`ref` あり** → provider カタログを引き、未登録なら hard error。
///   model カタログは未登録でも warn ログだけに留め、`provider.default_capability`
///   にフォールバック（タイポで動く可能性は API 側 `model not found` で
///   結果的に検出されるため）。
/// - **`ref` なし** → `scheme` / `model_id` / `auth` の 3 つが揃って
///   いることを検証し、そのまま `ModelConfig` を組む。
///
/// 各フィールドの解決順は ticket の表に準拠:
/// scheme/base_url は manifest 明示 > provider、model_id は manifest 明示 > ref、
/// auth は manifest 明示 > provider.auth_hint 由来、capability は
/// manifest 明示 > model catalog > provider.default_capability >
/// （`build_client` 側で）`Scheme::default_capability()`。
pub fn resolve_model_manifest(manifest: &ModelManifest) -> Result<ModelConfig, ResolveError> {
    let providers = load_providers().map_err(ResolveError::LoadProviders)?;
    let models = load_models().map_err(ResolveError::LoadModels)?;
    resolve_with_catalogs(manifest, &providers, &models)
}

/// テスト等で in-memory カタログを差し込む解決経路。
pub fn resolve_with_catalogs(
    manifest: &ModelManifest,
    providers: &[ProviderEntry],
    models: &[ModelEntry],
) -> Result<ModelConfig, ResolveError> {
    if let Some(ref_str) = &manifest.ref_ {
        let (provider_id, ref_model_id) = split_ref(ref_str)
            .ok_or_else(|| ResolveError::MalformedRef(ref_str.clone()))?;
        let provider = providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| ResolveError::UnknownProvider(provider_id.to_string()))?;

        // model 行は無くても続行可（warn ログ + provider.default_capability）。
        let model_entry = models
            .iter()
            .find(|m| m.provider == provider_id && m.id == ref_model_id);
        if model_entry.is_none() {
            tracing::warn!(
                provider = provider_id,
                model = ref_model_id,
                "model.ref not found in model catalog; falling back to provider.default_capability"
            );
        }

        let scheme = manifest.scheme.unwrap_or(provider.scheme);
        let base_url = manifest
            .base_url
            .clone()
            .or_else(|| provider.base_url.clone());
        let model_id = manifest
            .model_id
            .clone()
            .unwrap_or_else(|| ref_model_id.to_string());
        let auth = manifest
            .auth
            .clone()
            .unwrap_or_else(|| auth_hint_to_ref(&provider.auth_hint));
        let capability = manifest.capability.clone().or_else(|| {
            model_entry
                .and_then(|m| m.capability.clone())
                .or_else(|| provider.default_capability.clone())
        });
        Ok(ModelConfig {
            scheme,
            base_url,
            model_id,
            auth,
            capability,
        })
    } else {
        let scheme = manifest
            .scheme
            .ok_or(ResolveError::InlineMissing("scheme"))?;
        let model_id = manifest
            .model_id
            .clone()
            .ok_or(ResolveError::InlineMissing("model_id"))?;
        let auth = manifest
            .auth
            .clone()
            .ok_or(ResolveError::InlineMissing("auth"))?;
        Ok(ModelConfig {
            scheme,
            base_url: manifest.base_url.clone(),
            model_id,
            auth,
            capability: manifest.capability.clone(),
        })
    }
}

fn user_override_path(file_name: &str) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("insomnia").join(file_name));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("insomnia")
                .join(file_name),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn builtin_has_four_providers() {
        let entries = load_builtin_providers().unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["anthropic", "ollama-local", "codex-oauth", "openrouter"]
        );
    }

    #[test]
    fn builtin_provider_default_capability_present() {
        let entries = load_builtin_providers().unwrap();
        let anthropic = entries.iter().find(|e| e.id == "anthropic").unwrap();
        assert!(anthropic.default_capability.is_some());
    }

    #[test]
    fn builtin_models_cover_each_provider() {
        let entries = load_builtin_models().unwrap();
        let providers: std::collections::BTreeSet<&str> =
            entries.iter().map(|m| m.provider.as_str()).collect();
        for p in ["anthropic", "ollama-local", "codex-oauth", "openrouter"] {
            assert!(
                providers.contains(p),
                "model catalog should cover provider `{p}`"
            );
        }
    }

    #[test]
    fn resolve_ref_pulls_provider_defaults() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("anthropic/claude-sonnet-4-6".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::Anthropic);
        assert_eq!(cfg.model_id, "claude-sonnet-4-6");
        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        match cfg.auth {
            AuthRef::ApiKey { env, file } => {
                assert_eq!(env.as_deref(), Some("INSOMNIA_API_KEY_ANTHROPIC"));
                assert!(file.is_none());
            }
            _ => panic!("expected ApiKey auth from provider hint"),
        }
        assert!(cfg.capability.is_some(), "should fall back to provider.default_capability");
    }

    #[test]
    fn resolve_ref_with_inline_overrides() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("anthropic/claude-sonnet-4-6".into()),
            auth: Some(AuthRef::ApiKey {
                env: None,
                file: Some(PathBuf::from("/tmp/sk-ant")),
            }),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        match cfg.auth {
            AuthRef::ApiKey { env, file } => {
                assert!(env.is_none());
                assert_eq!(file.as_deref(), Some(Path::new("/tmp/sk-ant")));
            }
            _ => panic!("override auth should win"),
        }
    }

    #[test]
    fn resolve_ref_with_nested_model_id() {
        // OpenRouter: `<router>/<provider>/<model>` 形式の model_id を持つ
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("openrouter/anthropic/claude-sonnet-4".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::OpenaiChat);
        assert_eq!(cfg.model_id, "anthropic/claude-sonnet-4");
    }

    #[test]
    fn resolve_ref_unknown_provider_is_hard_error() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("nope/some-model".into()),
            ..Default::default()
        };
        let err = resolve_with_catalogs(&manifest, &providers, &models).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownProvider(_)));
    }

    #[test]
    fn resolve_ref_unknown_model_is_warn_not_error() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("anthropic/some-future-claude".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.model_id, "some-future-claude");
        assert!(cfg.capability.is_some(), "should use provider default");
    }

    #[test]
    fn resolve_inline_full_form() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            model_id: Some("claude-sonnet-4-6".into()),
            auth: Some(AuthRef::ApiKey {
                env: None,
                file: Some(PathBuf::from("/tmp/sk")),
            }),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::Anthropic);
        assert_eq!(cfg.model_id, "claude-sonnet-4-6");
        assert!(cfg.capability.is_none(), "no catalog hit for inline-only");
    }

    #[test]
    fn resolve_inline_missing_auth_errors() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            model_id: Some("claude".into()),
            ..Default::default()
        };
        let err = resolve_with_catalogs(&manifest, &providers, &models).unwrap_err();
        assert!(matches!(err, ResolveError::InlineMissing("auth")));
    }

    #[test]
    fn malformed_ref_errors() {
        let providers = load_builtin_providers().unwrap();
        let models = load_builtin_models().unwrap();
        let manifest = ModelManifest {
            ref_: Some("noslash".into()),
            ..Default::default()
        };
        let err = resolve_with_catalogs(&manifest, &providers, &models).unwrap_err();
        assert!(matches!(err, ResolveError::MalformedRef(_)));
    }

    #[test]
    fn load_providers_from_path() {
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
"#,
        )
        .unwrap();
        let entries = load_providers_from(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "custom");
    }

    #[test]
    fn malformed_provider_override_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, "this is not valid ][ toml").unwrap();
        let err = load_providers_from(&path).unwrap_err();
        assert!(matches!(err, CatalogError::Parse { .. }));
    }

    #[test]
    #[serial]
    fn load_prefers_user_override() {
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
        let entries = load_providers().unwrap();
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
        let entries = load_providers().unwrap();
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        assert_eq!(entries.len(), 4);
    }
}
