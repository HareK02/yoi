use std::path::PathBuf;

use serde::Deserialize;

/// Declarative configuration for a Pod.
///
/// Parsed from a TOML manifest file. Describes the provider, model,
/// system prompt, and optional directory scope.
#[derive(Debug, Clone, Deserialize)]
pub struct PodManifest {
    pub pod: PodMeta,
    pub provider: ProviderConfig,
    pub worker: WorkerManifest,
    #[serde(default)]
    pub scope: Option<ScopeConfig>,
}

/// Pod metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct PodMeta {
    pub name: String,
}

/// LLM provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    /// Environment variable name holding the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Custom base URL for the provider API.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Gemini,
    Ollama,
}

/// Worker-level configuration embedded in the manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerManifest {
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// Directory scope configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ScopeConfig {
    pub root: PathBuf,
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
        assert!(manifest.provider.api_key_env.is_none());
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
api_key_env = "ANTHROPIC_API_KEY"

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
            manifest.provider.api_key_env.as_deref(),
            Some("ANTHROPIC_API_KEY")
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
        assert!(manifest.provider.api_key_env.is_none());
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
