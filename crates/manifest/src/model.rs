//! LLM model declaration types.
//!
//! The Worker manifest's `[model]` section uses one [`ModelManifest`] with all
//! fields optional to support both `ref` (a provider/model shorthand) and inline
//! `scheme` / `model_id` declarations. [`crate::model_catalog`] resolves refs via
//! provider/model catalogs and fills in `scheme`, `model_id`, and other fields;
//! this module provides only the data representation.
//!
//! Sharing the type between partial cascade layers and final manifests avoids
//! duplicate merging and final conversion logic.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// `ModelCapability` is a runtime type defined in `agen`; re-export only the type
// to allow optional manifest overrides.
pub use agen::llm_client::capability::{ModelCapability, ReasoningControl, ReasoningEffort};

/// The Worker manifest's `[model]` section.
///
/// - Ref only: `[model] ref = "anthropic/claude-sonnet-4-6"`
/// - Ref with overrides: use ref as a base and override fields such as `auth`.
/// - Fully inline: omit `ref` and specify `scheme` / `model_id` / `auth` directly.
///
/// [`crate::model_catalog::resolve_model_manifest`] determines which forms are valid.
/// This crate only represents configuration sources; it does not raise hard errors
/// merely because fields are unset.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelManifest {
    /// Catalog reference in `<provider_id>/<model_id_in_ref>` form. Splits at
    /// the first `/` to look up the provider catalog. For model IDs containing
    /// `/`, such as OpenRouter's `anthropic/claude-sonnet-4`, use
    /// `openrouter/anthropic/claude-sonnet-4`; only the first `/` is split.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    /// Explicit wire format. Required without ref; an override with ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<SchemeKind>,
    /// API base URL, overriding the scheme default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Provider-accepted model ID. Required without ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Authentication method. Required without ref; an override with ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthRef>,
    /// Explicit model capabilities. If omitted, resolves in order: model catalog,
    /// provider `default_capability`, then scheme default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<ModelCapability>,
    /// Desired model context window in tokens. Can explicitly override uncataloged
    /// or inline models. The effective value is clamped by `max_context_window`
    /// or the catalog's backend maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Maximum context window in tokens actually accepted by the backend.
    /// The effective window used for display and safety checks is the smaller
    /// of `context_window` and this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
}

impl ModelManifest {
    /// Merges explicit `upper` values over `self`, retaining lower values when omitted.
    pub fn merge(self, upper: Self) -> Self {
        Self {
            ref_: upper.ref_.or(self.ref_),
            scheme: upper.scheme.or(self.scheme),
            base_url: upper.base_url.or(self.base_url),
            model_id: upper.model_id.or(self.model_id),
            auth: upper.auth.or(self.auth),
            capability: upper.capability.or(self.capability),
            context_window: upper.context_window.or(self.context_window),
            max_context_window: upper.max_context_window.or(self.max_context_window),
        }
    }
}

/// Supported wire scheme kinds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemeKind {
    /// Anthropic Messages API (`/v1/messages`), also used for Ollama `/v1/messages`.
    Anthropic,
    /// OpenAI Chat Completions (`/v1/chat/completions`), shared by compatible routers.
    OpenaiChat,
    /// OpenAI Responses API (`/v1/responses`).
    OpenaiResponses,
    /// Google Gemini (`/v1beta/models/...:streamGenerateContent`)
    Gemini,
}

/// Authentication reference.
///
/// A worker-side factory resolves actual tokens (local secret store / file reads,
/// OAuth refresh, etc.). This is only a declaration of where to obtain them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthRef {
    /// No authentication required (e.g. local Ollama).
    #[default]
    None,
    /// API key file reference. Prefer [`AuthRef::SecretRef`] for normal
    /// provider credentials; this remains an explicit file source for low-level
    /// manifests and tests.
    ApiKey {
        /// File containing the key (absolute path).
        #[serde(default)]
        file: Option<PathBuf>,
    },
    /// ChatGPT OAuth (`~/.codex/auth.json`).
    #[serde(rename = "codex_oauth")]
    CodexOAuth,
    /// Typed local secret-store reference. The profile resolver preserves this
    /// reference verbatim; secret-store lookup/deobfuscation is intentionally a
    /// later consumer-boundary concern.
    #[serde(rename = "secret_ref")]
    SecretRef {
        #[serde(rename = "ref")]
        ref_: String,
    },
}
