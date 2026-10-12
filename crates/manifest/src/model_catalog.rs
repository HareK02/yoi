//! Provider / model catalogs.
//!
//! - builtin providers: `resources/providers/builtin.toml`
//! - builtin models:    `resources/models/builtin.toml`
//! - user override:     `<config_dir>/{providers,models}.toml`
//!
//! See [`crate::paths::config_dir`] for `<config_dir>` resolution. Each override
//! replaces its builtin catalog if present, otherwise the builtin is used; no
//! merging occurs. Provider and model catalogs load independently, so either
//! can be overridden alone.
//!
//! [`resolve_model_manifest`] is the single entry point for resolving either ref
//! or inline [`crate::ModelManifest`] declarations into a final [`ModelConfig`],
//! including validation before passing it to the wire layer.

use std::path::{Path, PathBuf};

use crate::{AuthRef, ModelManifest, SchemeKind};
use agen::llm_client::capability::ModelCapability;
use serde::{Deserialize, Serialize};

const BUILTIN_PROVIDERS: &str = include_str!("../../../resources/providers/builtin.toml");
const BUILTIN_MODELS: &str = include_str!("../../../resources/models/builtin.toml");

/// Conservative fallback used when neither the manifest nor catalogs specify
/// a model context window. Greeting still carries a concrete number, while
/// catalog / manifest metadata can override unknown or inline models.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

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

/// Manifest resolution error: `ModelManifest` cannot satisfy a catalog reference
/// or an inline declaration is missing required fields.
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
    #[error("model.ref omitted; manifest must specify scheme, model_id, and auth (missing: {0})")]
    InlineMissing(&'static str),
}

/// UI authentication hint.
///
/// Metadata describing what to display/request, separate from runtime [`AuthRef`].
/// Maps 1:1 to it via `auth_hint_to_ref` during resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthHint {
    /// No authentication required (e.g. local Ollama).
    None,
    /// API key file reference. Normal credential configuration should prefer
    /// [`AuthHint::SecretRef`] so plaintext credentials stay out of manifests.
    ApiKey,
    /// Local secret-store reference. The catalog/profile explicitly chooses the
    /// logical key id; the secret store itself has no provider semantics.
    #[serde(rename = "secret_ref")]
    SecretRef {
        #[serde(rename = "ref")]
        ref_: String,
    },
    /// ChatGPT OAuth (`~/.codex/auth.json`)
    #[serde(rename = "codex_oauth")]
    CodexOAuth,
}

/// One provider catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderEntry {
    pub id: String,
    pub display_name: String,
    pub scheme: SchemeKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth_hint: AuthHint,
    /// Fallback when this provider serves a model absent from the model catalog
    /// (e.g. a ref has a provider but no model entry). If omitted,
    /// `Scheme::default_capability()` is the final fallback.
    #[serde(default)]
    pub default_capability: Option<ModelCapability>,
    /// Default context window for models absent from the model catalog.
    /// Uses [`DEFAULT_CONTEXT_WINDOW`] if omitted.
    #[serde(default)]
    pub default_context_window: Option<u64>,
}

/// One model catalog entry.
///
/// `id` is **unique within a provider**. The same `gpt-5` may appear under different
/// providers without ambiguity, since refs always include `<provider>/<model_id>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    /// Per-model capability override, falling back to
    /// `ProviderEntry::default_capability` if omitted.
    #[serde(default)]
    pub capability: Option<ModelCapability>,
    /// Per-model context window. If omitted, falls back to the provider default,
    /// then the builtin fallback. The effective value is clamped by
    /// `max_context_window`.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Maximum context window actually accepted by the backend. UI and
    /// pre-request safety checks use the clamped effective value, not the desired one.
    #[serde(default)]
    pub max_context_window: Option<u64>,
}

/// Resolved model configuration, ready for `build_client`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub scheme: SchemeKind,
    pub base_url: Option<String>,
    pub model_id: String,
    pub auth: AuthRef,
    pub capability: Option<ModelCapability>,
    /// Effective context window after backend maximum clamping.
    pub context_window: u64,
    /// Backend maximum that constrained `context_window`, when known.
    pub max_context_window: Option<u64>,
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

/// Returns an [`AuthRef`] template corresponding to `auth_hint`.
fn auth_hint_to_ref(hint: &AuthHint) -> AuthRef {
    match hint {
        AuthHint::None => AuthRef::None,
        AuthHint::ApiKey => AuthRef::ApiKey { file: None },
        AuthHint::SecretRef { ref_ } => AuthRef::SecretRef { ref_: ref_.clone() },
        AuthHint::CodexOAuth => AuthRef::CodexOAuth,
    }
}

// --- providers ---------------------------------------------------------------

/// Returns the provider catalog after resolving builtin + user overrides.
///
/// A user override (`<config_dir>/providers.toml`) replaces the builtin if present;
/// otherwise only the builtin is used. An invalid override returns an error rather
/// than silently falling back, since ignoring user configuration is hard to notice.
pub fn load_providers() -> Result<Vec<ProviderEntry>, CatalogError> {
    if let Some(path) = crate::paths::user_catalog_override("providers.toml")
        && path.is_file()
    {
        return load_providers_from(&path);
    }
    load_builtin_providers()
}

/// Returns only the builtin provider catalog.
pub fn load_builtin_providers() -> Result<Vec<ProviderEntry>, CatalogError> {
    let parsed: ProviderCatalogFile =
        toml::from_str(BUILTIN_PROVIDERS).map_err(CatalogError::BuiltinParse)?;
    Ok(parsed.provider)
}

/// Reads a provider catalog from the given path.
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

/// Returns the model catalog after resolving builtin + user overrides.
pub fn load_models() -> Result<Vec<ModelEntry>, CatalogError> {
    if let Some(path) = crate::paths::user_catalog_override("models.toml")
        && path.is_file()
    {
        return load_models_from(&path);
    }
    load_builtin_models()
}

/// Returns only the builtin model catalog.
pub fn load_builtin_models() -> Result<Vec<ModelEntry>, CatalogError> {
    let parsed: ModelCatalogFile =
        toml::from_str(BUILTIN_MODELS).map_err(CatalogError::BuiltinParse)?;
    Ok(parsed.model)
}

/// Reads a model catalog from the given path.
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

// --- ref resolution / manifest → ModelConfig ---------------------------------

/// Splits `<provider_id>/<model_id>` once, at the first `/`.
/// Model IDs containing `/`, such as OpenRouter's
/// `openrouter/anthropic/claude-sonnet-4.6`, resolve as provider=`openrouter`,
/// model_id=`anthropic/claude-sonnet-4.6`.
fn split_ref(s: &str) -> Option<(&str, &str)> {
    let (provider, rest) = s.split_once('/')?;
    if provider.is_empty() || rest.is_empty() {
        return None;
    }
    Some((provider, rest))
}

/// Resolves `ModelManifest` against catalogs into a final [`ModelConfig`].
///
/// - **With `ref`**: looks up the provider; an unknown provider is a hard error.
///   A missing model entry only logs a warning and falls back to
///   `provider.default_capability` (typos are eventually caught by the API's
///   `model not found` error).
/// - **Without `ref`**: requires `scheme` / `model_id` / `auth` and builds
///   `ModelConfig` directly.
///
/// Field precedence:
/// scheme/base_url: explicit manifest > provider; model_id: explicit manifest > ref;
/// auth: explicit manifest > provider.auth_hint; capability:
/// explicit manifest > model catalog > provider.default_capability >
/// `Scheme::default_capability()` (in `build_client`).
/// context_window: explicit manifest > model catalog > provider default >
/// [`DEFAULT_CONTEXT_WINDOW`]. The effective context_window is clamped by the
/// manifest/model max_context_window.
pub fn resolve_model_manifest(manifest: &ModelManifest) -> Result<ModelConfig, ResolveError> {
    let providers = load_providers().map_err(ResolveError::LoadProviders)?;
    let models = load_models().map_err(ResolveError::LoadModels)?;
    resolve_with_catalogs(manifest, &providers, &models)
}

/// Resolution path accepting in-memory catalogs, e.g. for tests.
pub fn resolve_with_catalogs(
    manifest: &ModelManifest,
    providers: &[ProviderEntry],
    models: &[ModelEntry],
) -> Result<ModelConfig, ResolveError> {
    if let Some(ref_str) = &manifest.ref_ {
        let (provider_id, ref_model_id) =
            split_ref(ref_str).ok_or_else(|| ResolveError::MalformedRef(ref_str.clone()))?;
        let provider = providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| ResolveError::UnknownProvider(provider_id.to_string()))?;

        // Missing model entries may proceed with a warning + provider.default_capability.
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
        let desired_context_window = manifest
            .context_window
            .or_else(|| model_entry.and_then(|m| m.context_window))
            .or(provider.default_context_window)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let max_context_window = manifest
            .max_context_window
            .or_else(|| model_entry.and_then(|m| m.max_context_window));
        let context_window = clamp_context_window(desired_context_window, max_context_window);
        Ok(ModelConfig {
            scheme,
            base_url,
            model_id,
            auth,
            capability,
            context_window,
            max_context_window,
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
        let desired_context_window = manifest.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let max_context_window = manifest.max_context_window;
        Ok(ModelConfig {
            scheme,
            base_url: manifest.base_url.clone(),
            model_id,
            auth,
            capability: manifest.capability.clone(),
            context_window: clamp_context_window(desired_context_window, max_context_window),
            max_context_window,
        })
    }
}

fn clamp_context_window(desired: u64, max: Option<u64>) -> u64 {
    max.map(|limit| desired.min(limit)).unwrap_or(desired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn test_catalogs() -> (Vec<ProviderEntry>, Vec<ModelEntry>) {
        (
            vec![ProviderEntry {
                id: "test-provider".to_string(),
                display_name: "Test Provider".to_string(),
                scheme: SchemeKind::OpenaiChat,
                base_url: Some("https://example.invalid/v1".to_string()),
                auth_hint: AuthHint::None,
                default_capability: None,
                default_context_window: Some(100_000),
            }],
            vec![ModelEntry {
                id: "test-model".to_string(),
                provider: "test-provider".to_string(),
                context_window: Some(100_000),
                max_context_window: Some(80_000),
                capability: None,
            }],
        )
    }

    #[test]
    fn resolve_ref_merges_provider_and_model_catalog() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("test-provider/test-model".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::OpenaiChat);
        assert_eq!(cfg.model_id, "test-model");
        assert_eq!(cfg.base_url.as_deref(), Some("https://example.invalid/v1"));
        assert!(matches!(cfg.auth, AuthRef::None));
        assert_eq!(cfg.context_window, 80_000);
    }

    #[test]
    fn context_window_manifest_overrides_catalog() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("test-provider/test-model".into()),
            context_window: Some(70_000),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.context_window, 70_000);
    }

    #[test]
    fn inline_context_window_is_clamped_by_manifest_backend_max() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            model_id: Some("custom".into()),
            auth: Some(AuthRef::None),
            context_window: Some(1_000_000),
            max_context_window: Some(272_000),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.context_window, 272_000);
        assert_eq!(cfg.max_context_window, Some(272_000));
    }

    #[test]
    fn manifest_backend_max_clamps_ref_context_override() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("test-provider/test-model".into()),
            context_window: Some(100_000),
            max_context_window: Some(50_000),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.context_window, 50_000);
        assert_eq!(cfg.max_context_window, Some(50_000));
    }

    #[test]
    fn resolve_ref_with_inline_overrides() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("test-provider/test-model".into()),
            auth: Some(AuthRef::ApiKey {
                file: Some(PathBuf::from("/tmp/sk-ant")),
            }),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        match cfg.auth {
            AuthRef::ApiKey { file } => {
                assert_eq!(file.as_deref(), Some(Path::new("/tmp/sk-ant")));
            }
            _ => panic!("override auth should win"),
        }
    }

    #[test]
    fn resolve_ref_with_nested_model_id() {
        let (providers, mut models) = test_catalogs();
        models.push(ModelEntry {
            id: "namespace/model".to_string(),
            provider: "test-provider".to_string(),
            context_window: None,
            max_context_window: None,
            capability: None,
        });
        let manifest = ModelManifest {
            ref_: Some("test-provider/namespace/model".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::OpenaiChat);
        assert_eq!(cfg.model_id, "namespace/model");
    }

    #[test]
    fn resolve_ref_unknown_provider_is_hard_error() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("nope/some-model".into()),
            ..Default::default()
        };
        let err = resolve_with_catalogs(&manifest, &providers, &models).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownProvider(_)));
    }

    #[test]
    fn resolve_ref_unknown_model_is_warn_not_error() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("test-provider/unknown-model".into()),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.model_id, "unknown-model");
        assert!(cfg.capability.is_none(), "should use provider default");
    }

    #[test]
    fn resolve_inline_full_form() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            model_id: Some("claude-sonnet-4-6".into()),
            auth: Some(AuthRef::ApiKey {
                file: Some(PathBuf::from("/tmp/sk")),
            }),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.scheme, SchemeKind::Anthropic);
        assert_eq!(cfg.model_id, "claude-sonnet-4-6");
        assert!(cfg.capability.is_none(), "no catalog hit for inline-only");
        assert_eq!(cfg.context_window, DEFAULT_CONTEXT_WINDOW);
    }

    #[test]
    fn resolve_inline_context_window_override() {
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            model_id: Some("claude-sonnet-4-6".into()),
            auth: Some(AuthRef::ApiKey {
                file: Some(PathBuf::from("/tmp/sk")),
            }),
            context_window: Some(777_000),
            ..Default::default()
        };
        let cfg = resolve_with_catalogs(&manifest, &providers, &models).unwrap();
        assert_eq!(cfg.context_window, 777_000);
    }

    #[test]
    fn resolve_inline_missing_auth_errors() {
        let (providers, models) = test_catalogs();
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
        let (providers, models) = test_catalogs();
        let manifest = ModelManifest {
            ref_: Some("noslash".into()),
            ..Default::default()
        };
        let err = resolve_with_catalogs(&manifest, &providers, &models).unwrap_err();
        assert!(matches!(err, ResolveError::MalformedRef(_)));
    }

    #[test]
    fn builtin_codex_catalog_replaces_gpt_6_sol_with_6_1_and_keeps_astra_and_luna() {
        let models = load_builtin_models().unwrap();
        let providers = load_builtin_providers().unwrap();
        assert!(
            !models
                .iter()
                .any(|model| model.provider == "codex-oauth" && model.id == "gpt-6-sol")
        );

        for model_id in ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna"] {
            let model = models
                .iter()
                .find(|model| model.provider == "codex-oauth" && model.id == model_id)
                .unwrap_or_else(|| panic!("missing codex-oauth/{model_id}"));
            assert_eq!(model.context_window, Some(1_050_000));
            assert_eq!(model.max_context_window, Some(272_000));
            assert!(model.capability.is_some());

            let config = resolve_with_catalogs(
                &ModelManifest {
                    ref_: Some(format!("codex-oauth/{model_id}")),
                    context_window: Some(1_050_000),
                    ..Default::default()
                },
                &providers,
                &models,
            )
            .unwrap();
            assert_eq!(config.model_id, model_id);
            assert_eq!(config.scheme, SchemeKind::OpenaiResponses);
            assert_eq!(config.auth, AuthRef::CodexOAuth);
            assert_eq!(config.context_window, 272_000);
            assert_eq!(config.max_context_window, Some(272_000));
            assert_eq!(config.capability, model.capability);
        }
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

    /// Test guard pointing `YOI_CONFIG_DIR` to a temporary directory.
    /// `paths::config_dir` prioritizes it over YOI_HOME / XDG_CONFIG_HOME,
    /// isolating tests from the development machine's environment settings.
    struct ConfigDirGuard {
        prev: Option<String>,
    }

    impl ConfigDirGuard {
        fn new(path: &Path) -> Self {
            let prev = std::env::var("YOI_CONFIG_DIR").ok();
            // SAFETY: serial_test's `#[serial]` attribute serializes tests that
            // modify the environment.
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

    #[test]
    #[serial]
    fn load_prefers_user_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            r#"
[[provider]]
id = "only-one"
display_name = "Only"
scheme = "anthropic"
auth_hint = { kind = "none" }
"#,
        )
        .unwrap();

        let _g = ConfigDirGuard::new(dir.path());
        let entries = load_providers().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "only-one");
    }

    #[test]
    #[serial]
    fn load_falls_back_to_builtin_when_override_absent() {
        let dir = tempfile::tempdir().unwrap();
        // Do not create an override file.
        let _g = ConfigDirGuard::new(dir.path());
        let entries = load_providers().unwrap();
        assert!(!entries.is_empty());
    }
}
