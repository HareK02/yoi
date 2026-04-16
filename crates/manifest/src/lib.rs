mod config;
pub mod defaults;
mod scope;

pub use config::{
    CompactionConfigPartial, PodManifestConfig, PodMetaConfig, ProviderConfigPartial, ResolveError,
    ToolOutputLimitsPartial, WorkerManifestConfig,
};
pub use scope::{Scope, ScopeError};

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Declarative configuration for a Pod.
///
/// Parsed from a TOML manifest file. Describes the provider, model,
/// system prompt, and directory scope (required).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodManifest {
    pub pod: PodMeta,
    pub provider: ProviderConfig,
    pub worker: WorkerManifest,
    pub scope: ScopeConfig,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
}

/// Pod metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodMeta {
    pub name: String,
    /// Working directory for the Pod. Relative paths are resolved against
    /// the directory containing the manifest file.
    pub pwd: PathBuf,
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
    /// Reference to the instruction prompt asset used as the body of
    /// the worker's system prompt. Uses the `PromptLoader` prefix
    /// addressing scheme (`$insomnia/...`, `$user/...`,
    /// `$workspace/...`) and is always populated after resolution —
    /// unset manifests fall through to [`defaults::DEFAULT_INSTRUCTION`].
    #[serde(default = "default_instruction")]
    pub instruction: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<NonZeroU32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Byte-size caps applied to tool `content` before it reaches the
    /// conversation history. The section is optional in TOML — when
    /// omitted, `ToolOutputLimits::default()` (16KB default cap, no
    /// per-tool overrides) is applied so truncation is on by default.
    #[serde(default)]
    pub tool_output: ToolOutputLimits,
}

/// Byte-size caps applied to tool execution `content` before it enters
/// conversation history. Guards against a single oversized tool result
/// blowing past the provider's per-minute input-token rate limit.
///
/// Field names are deliberately phrased in bytes (not tokens) because
/// accurate pre-send token counting is not yet available; the caps can
/// be migrated to token units later without renaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputLimits {
    /// Cap applied to any tool not listed in `per_tool`.
    #[serde(default = "default_tool_output_max_bytes")]
    pub default_max_bytes: usize,
    /// Per-tool overrides, keyed by tool registration name (e.g. "Glob").
    #[serde(default)]
    pub per_tool: HashMap<String, usize>,
}

fn default_tool_output_max_bytes() -> usize {
    defaults::TOOL_OUTPUT_MAX_BYTES
}

fn default_instruction() -> String {
    defaults::DEFAULT_INSTRUCTION.to_string()
}

impl Default for ToolOutputLimits {
    fn default() -> Self {
        Self {
            default_max_bytes: default_tool_output_max_bytes(),
            per_tool: HashMap::new(),
        }
    }
}

impl ToolOutputLimits {
    /// Resolve the cap for a given tool name.
    pub fn limit_for(&self, tool_name: &str) -> usize {
        self.per_tool
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_max_bytes)
    }
}

/// Declarative scope configuration.
///
/// A Pod may only touch paths whose effective permission (computed from
/// allow/deny rules below) is at least `Read` / `Write`. See
/// [`Scope`] for the resolved runtime form.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConfig {
    /// Rules granting access. At least one entry is required for the
    /// scope to be meaningful; [`Scope::from_config`] enforces this.
    #[serde(default)]
    pub allow: Vec<ScopeRule>,
    /// Rules capping access below the stated permission level. Empty by
    /// default.
    #[serde(default)]
    pub deny: Vec<ScopeRule>,
}

/// A single allow or deny rule inside [`ScopeConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRule {
    /// Target path. Relative paths are resolved against the Pod's pwd
    /// when [`Scope::from_config`] runs.
    pub target: PathBuf,
    /// Permission level this rule grants (allow) or caps strictly below
    /// (deny).
    pub permission: Permission,
    /// When `false`, the rule only matches the target itself and its
    /// direct children. Defaults to `true`.
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_recursive() -> bool {
    true
}

/// Permission lattice used by [`ScopeRule`].
///
/// The derived `Ord` instance follows declaration order, so
/// `Read < Write`. Allow rules grant the stated level (and by extension
/// everything below); deny rules cap the effective level **strictly
/// below** the stated level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Write,
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
    pub prune_min_savings: u64,

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

fn default_prune_protected_turns() -> usize {
    defaults::PRUNE_PROTECTED_TURNS
}
fn default_prune_min_savings() -> u64 {
    defaults::PRUNE_MIN_SAVINGS
}
fn default_compact_retained_turns() -> usize {
    defaults::COMPACT_RETAINED_TURNS
}

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

    const MINIMAL_REQUIRED: &str = r#"
[pod]
name = "test-agent"
pwd = "./"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]

[[scope.allow]]
target = "./"
permission = "write"
"#;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert_eq!(manifest.pod.name, "test-agent");
        assert_eq!(manifest.pod.pwd, PathBuf::from("./"));
        assert_eq!(manifest.provider.kind, ProviderKind::Anthropic);
        assert_eq!(manifest.provider.model, "claude-sonnet-4-20250514");
        assert!(manifest.provider.api_key_file.is_none());
        assert_eq!(manifest.scope.allow.len(), 1);
        assert!(manifest.scope.deny.is_empty());
        assert_eq!(manifest.worker.instruction, defaults::DEFAULT_INSTRUCTION);
    }

    #[test]
    fn parse_full_manifest() {
        let toml = r#"
[pod]
name = "code-reviewer"
pwd = "./src"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_file = "~/.config/insomnia/keys/anthropic"

[worker]
instruction = "$user/reviewer"
max_tokens = 4096
temperature = 0.3

[[scope.allow]]
target = "./"
permission = "write"

[[scope.allow]]
target = "../docs"
permission = "read"
recursive = false

[[scope.deny]]
target = "./secrets.rs"
permission = "write"
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.pod.name, "code-reviewer");
        assert_eq!(manifest.pod.pwd, PathBuf::from("./src"));
        assert_eq!(
            manifest.provider.api_key_file.as_deref(),
            Some(std::path::Path::new("~/.config/insomnia/keys/anthropic"))
        );
        assert_eq!(manifest.worker.instruction, "$user/reviewer");
        assert_eq!(manifest.worker.max_tokens, Some(4096));
        assert_eq!(manifest.worker.temperature, Some(0.3));
        let allow = &manifest.scope.allow;
        assert_eq!(allow.len(), 2);
        assert_eq!(allow[0].permission, Permission::Write);
        assert!(allow[0].recursive);
        assert_eq!(allow[1].permission, Permission::Read);
        assert!(!allow[1].recursive);
        assert_eq!(manifest.scope.deny.len(), 1);
        assert_eq!(manifest.scope.deny[0].permission, Permission::Write);
    }

    #[test]
    fn reject_missing_scope() {
        let toml = r#"
[pod]
name = "missing-scope"
pwd = "./"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
"#;
        assert!(PodManifest::from_toml(toml).is_err());
    }

    #[test]
    fn reject_missing_pwd() {
        let toml = r#"
[pod]
name = "missing-pwd"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]

[[scope.allow]]
target = "./"
permission = "write"
"#;
        assert!(PodManifest::from_toml(toml).is_err());
    }

    #[test]
    fn parse_max_turns() {
        let toml = MINIMAL_REQUIRED.replace("[worker]\n", "[worker]\nmax_turns = 50\n");
        let manifest = PodManifest::from_toml(&toml).unwrap();
        assert_eq!(manifest.worker.max_turns.unwrap().get(), 50);
    }

    #[test]
    fn omitted_max_turns_is_none() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.worker.max_turns.is_none());
    }

    #[test]
    fn reject_max_turns_zero() {
        let toml = MINIMAL_REQUIRED.replace("[worker]\n", "[worker]\nmax_turns = 0\n");
        assert!(PodManifest::from_toml(&toml).is_err());
    }

    #[test]
    fn parse_compaction_config() {
        let toml = format!("{MINIMAL_REQUIRED}\n[compaction]\ncompact_threshold = 80000\n");
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.prune_protected_turns, 3);
        assert_eq!(c.prune_min_savings, 4096);
        assert_eq!(c.compact_threshold, Some(80000));
        assert_eq!(c.compact_retained_turns, 2);
    }

    #[test]
    fn parse_compaction_with_provider() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             compact_threshold = 80000\n\n\
             [compaction.provider]\n\
             kind = \"gemini\"\n\
             model = \"gemini-2.0-flash\"\n"
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        let p = c.provider.unwrap();
        assert_eq!(p.kind, ProviderKind::Gemini);
        assert_eq!(p.model, "gemini-2.0-flash");
    }

    #[test]
    fn omitted_compaction_is_none() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.compaction.is_none());
    }

    #[test]
    fn reject_unknown_provider() {
        let toml = MINIMAL_REQUIRED.replace("kind = \"anthropic\"", "kind = \"unknown_provider\"");
        assert!(PodManifest::from_toml(&toml).is_err());
    }

    #[test]
    fn omitted_tool_output_falls_back_to_default_16k() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        let limits = &manifest.worker.tool_output;
        assert_eq!(limits.default_max_bytes, 16 * 1024);
        assert!(limits.per_tool.is_empty());
    }

    #[test]
    fn parse_tool_output_limits() {
        let toml = MINIMAL_REQUIRED.replace(
            "[worker]\n",
            "[worker]\n\
             [worker.tool_output]\n\
             default_max_bytes = 8192\n\n\
             [worker.tool_output.per_tool]\n\
             Read = 32768\n\
             Grep = 4096\n",
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let limits = &manifest.worker.tool_output;
        assert_eq!(limits.default_max_bytes, 8192);
        assert_eq!(limits.limit_for("Read"), 32768);
        assert_eq!(limits.limit_for("Grep"), 4096);
        assert_eq!(limits.limit_for("Unknown"), 8192);
    }

    #[test]
    fn empty_tool_output_section_uses_default_max_bytes() {
        let toml = MINIMAL_REQUIRED.replace(
            "[worker]\n",
            "[worker]\n\
             [worker.tool_output]\n",
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let limits = &manifest.worker.tool_output;
        assert_eq!(limits.default_max_bytes, 16 * 1024);
        assert!(limits.per_tool.is_empty());
    }

    #[test]
    fn default_recursive_true() {
        let rule: ScopeRule = toml::from_str(
            r#"
target = "./"
permission = "read"
"#,
        )
        .unwrap();
        assert!(rule.recursive);
    }
}
