mod scope;

pub use scope::Scope;

use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Declarative configuration for a Pod.
///
/// Parsed from a TOML manifest file. Describes the provider, model,
/// system prompt, and optional directory scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodManifest {
    pub pod: PodMeta,
    pub provider: ProviderConfig,
    pub worker: WorkerManifest,
    #[serde(default)]
    pub scope: Option<ScopeConfig>,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
}

/// Pod metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodMeta {
    pub name: String,
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    /// Path to a file containing the API key (read and trimmed at startup).
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    /// Custom base URL for the provider API.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Gemini,
    Ollama,
}

impl ProviderKind {
    /// Conventional environment variable name for the API key.
    ///
    /// Returns `INSOMNIA_API_KEY_{KIND}` (e.g. `INSOMNIA_API_KEY_ANTHROPIC`).
    pub fn env_var_name(self) -> String {
        let kind = match self {
            Self::Anthropic => "ANTHROPIC",
            Self::Openai => "OPENAI",
            Self::Gemini => "GEMINI",
            Self::Ollama => "OLLAMA",
        };
        format!("INSOMNIA_API_KEY_{kind}")
    }
}

/// Worker-level configuration embedded in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerManifest {
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<NonZeroU32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// Directory scope configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeConfig {
    pub root: PathBuf,
}

/// Context compaction configuration.
///
/// Controls Prune (content removal from old tool results) and Compact
/// (full history summarisation). Omitting `[compaction]` disables both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Number of recent turns protected from pruning.
    #[serde(default = "default_prune_protected_turns")]
    pub prune_protected_turns: usize,

    /// Minimum estimated token savings to trigger a prune.
    #[serde(default = "default_prune_min_savings")]
    pub prune_min_savings: usize,

    /// When `input_tokens` exceeds this, run compact. `None` = compact disabled.
    #[serde(default)]
    pub compact_threshold: Option<u64>,

    /// Number of recent turns retained after compaction.
    #[serde(default = "default_compact_retained_turns")]
    pub compact_retained_turns: usize,

    /// Optional provider for the compactor (summary) LLM.
    /// If omitted, the main provider is cloned via `clone_boxed()`.
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
}

fn default_prune_protected_turns() -> usize { 3 }
fn default_prune_min_savings() -> usize { 4096 }
fn default_compact_retained_turns() -> usize { 2 }

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            prune_protected_turns: default_prune_protected_turns(),
            prune_min_savings: default_prune_min_savings(),
            compact_threshold: None,
            compact_retained_turns: default_compact_retained_turns(),
            provider: None,
        }
    }
}

impl PodManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_manifest() {
        let toml = r#"
[pod]
name = "test-agent"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.pod.name, "test-agent");
        assert_eq!(manifest.provider.kind, ProviderKind::Anthropic);
        assert_eq!(manifest.provider.model, "claude-sonnet-4-20250514");
        assert!(manifest.provider.api_key_file.is_none());
        assert!(manifest.scope.is_none());
        assert!(manifest.worker.system_prompt.is_none());
    }

    #[test]
    fn parse_full_manifest() {
        let toml = r#"
[pod]
name = "code-reviewer"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_file = "~/.config/insomnia/keys/anthropic"

[worker]
system_prompt = "You are a code reviewer."
max_tokens = 4096
temperature = 0.3

[scope]
root = "./src"
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.pod.name, "code-reviewer");
        assert_eq!(
            manifest.provider.api_key_file.as_deref(),
            Some(std::path::Path::new("~/.config/insomnia/keys/anthropic"))
        );
        assert_eq!(
            manifest.worker.system_prompt.as_deref(),
            Some("You are a code reviewer.")
        );
        assert_eq!(manifest.worker.max_tokens, Some(4096));
        assert_eq!(manifest.worker.temperature, Some(0.3));
        assert_eq!(
            manifest.scope.as_ref().unwrap().root,
            PathBuf::from("./src")
        );
    }

    #[test]
    fn parse_ollama_no_api_key() {
        let toml = r#"
[pod]
name = "local-agent"

[provider]
kind = "ollama"
model = "llama3"

[worker]
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.provider.kind, ProviderKind::Ollama);
        assert!(manifest.provider.api_key_file.is_none());
    }

    #[test]
    fn parse_max_turns() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
max_turns = 50
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.worker.max_turns.unwrap().get(), 50);
    }

    #[test]
    fn omitted_max_turns_is_none() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert!(manifest.worker.max_turns.is_none());
    }

    #[test]
    fn reject_max_turns_zero() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
max_turns = 0
"#;
        assert!(PodManifest::from_toml(toml).is_err());
    }

    #[test]
    fn parse_compaction_config() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]

[compaction]
compact_threshold = 80000
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.prune_protected_turns, 3);
        assert_eq!(c.prune_min_savings, 4096);
        assert_eq!(c.compact_threshold, Some(80000));
        assert_eq!(c.compact_retained_turns, 2);
    }

    #[test]
    fn parse_compaction_with_provider() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]

[compaction]
compact_threshold = 80000

[compaction.provider]
kind = "gemini"
model = "gemini-2.0-flash"
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        let c = manifest.compaction.unwrap();
        let p = c.provider.unwrap();
        assert_eq!(p.kind, ProviderKind::Gemini);
        assert_eq!(p.model, "gemini-2.0-flash");
    }

    #[test]
    fn omitted_compaction_is_none() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert!(manifest.compaction.is_none());
    }

    #[test]
    fn reject_unknown_provider() {
        let toml = r#"
[pod]
name = "test"

[provider]
kind = "unknown_provider"
model = "x"

[worker]
"#;
        assert!(PodManifest::from_toml(toml).is_err());
    }
}
