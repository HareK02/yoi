mod cascade;
mod config;
pub mod defaults;
mod model;
pub mod paths;
mod scope;

pub use cascade::{LayerLoadError, find_project_manifest_from, load_layer};
pub use paths::user_manifest_path;
pub use config::{
    CompactionConfigPartial, PodManifestConfig, PodMetaConfig, ResolveError,
    ToolOutputLimitsPartial, WorkerManifestConfig,
};
pub use model::{AuthRef, ModelCapability, ModelManifest, SchemeKind};
pub use protocol::{Permission, ScopeRule};
pub use scope::{Scope, ScopeError};

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Declarative configuration for a Pod.
///
/// Parsed from a TOML manifest file. Describes the model, system prompt,
/// and directory scope (required). The Pod's working directory is **not**
/// part of the manifest — it is the process's `std::env::current_dir()`
/// at construction time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodManifest {
    pub pod: PodMeta,
    pub model: ModelManifest,
    pub worker: WorkerManifest,
    pub scope: ScopeConfig,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
    /// Memory subsystem opt-in. Presence of `[memory]` in TOML enables
    /// the memory tools (MemoryRead / MemoryWrite / MemoryEdit) and
    /// causes Pod to deny generic write access to `<workspace>/memory/`
    /// and `<workspace>/knowledge/`. Absent ⇒ legacy behaviour, no
    /// memory tools registered.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
}

/// Memory subsystem configuration. Presence in the manifest enables
/// memory; the workspace root defaults to the Pod's pwd unless an
/// explicit override is given.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Override for the workspace root. When `None`, the Pod's pwd
    /// (resolved at construction time) is used. When set, must be an
    /// absolute path.
    #[serde(default)]
    pub workspace_root: Option<PathBuf>,
    /// Maximum number of hits returned by `MemorySearch` /
    /// `KnowledgeSearch`. `None` ⇒ tool default (20).
    #[serde(default)]
    pub search_hit_limit: Option<usize>,
    /// Lines of context before and after each match in search excerpts.
    /// `None` ⇒ tool default (3).
    #[serde(default)]
    pub search_excerpt_lines: Option<usize>,
}

/// Pod metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodMeta {
    pub name: String,
    /// Optional path to a TOML override file read as the top layer of
    /// `pod::PromptCatalog`. Subject to the same relative-path
    /// resolution as other manifest paths (joined against the
    /// manifest's base directory). `None` leaves the 4th overlay layer
    /// empty; auto-discovered user and workspace packs still apply.
    ///
    /// Note: unlike `worker.instruction`, this is a plain filesystem
    /// path — not a `$prefix/` prompt reference. Pack files carry
    /// structured TOML data, while `worker.instruction` points at a
    /// minijinja `.md` template; the two use different addressing
    /// conventions on purpose.
    #[serde(default)]
    pub prompt_pack: Option<PathBuf>,
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

    /// Proactive (between-turns) compaction threshold.
    ///
    /// Checked by the Controller after each run. When current occupancy
    /// exceeds this value, compact runs before the next turn. `None`
    /// disables the between-turns check.
    #[serde(default)]
    pub compact_threshold: Option<u64>,

    /// Safety-net (between-requests) compaction threshold.
    ///
    /// Checked by `PodInterceptor::pre_llm_request` inside a turn. When
    /// current occupancy exceeds this value, the run yields so that the
    /// Controller can compact before the next LLM request. `None`
    /// disables the between-requests check.
    ///
    /// Expected relation: `compact_threshold < compact_request_threshold`
    /// (proactive triggers before safety net). A reversed configuration
    /// is accepted but logged as a warning.
    #[serde(default)]
    pub compact_request_threshold: Option<u64>,

    /// Token budget retained verbatim at the tail of the history after
    /// compaction. Measured against the occupancy estimate from
    /// `UsageRecord` history; turn boundaries are ignored.
    #[serde(default = "default_compact_retained_tokens")]
    pub compact_retained_tokens: u64,

    /// Aggregate token budget for auto-read file contents injected into
    /// the compacted session by the compact worker.
    #[serde(default = "default_compact_auto_read_budget")]
    pub compact_auto_read_budget: u64,

    /// Cumulative input-token cap for the compact worker's own LLM
    /// calls. Exceeding this aborts the compact run.
    #[serde(default = "default_compact_worker_max_input_tokens")]
    pub compact_worker_max_input_tokens: u64,

    /// Optional model for the compactor (summary) LLM.
    /// If omitted, the main model is cloned via `clone_boxed()`.
    #[serde(default)]
    pub model: Option<ModelManifest>,
}

fn default_prune_protected_turns() -> usize {
    defaults::PRUNE_PROTECTED_TURNS
}
fn default_prune_min_savings() -> u64 {
    defaults::PRUNE_MIN_SAVINGS
}
fn default_compact_retained_tokens() -> u64 {
    defaults::COMPACT_RETAINED_TOKENS
}
fn default_compact_auto_read_budget() -> u64 {
    defaults::COMPACT_AUTO_READ_BUDGET
}
fn default_compact_worker_max_input_tokens() -> u64 {
    defaults::COMPACT_WORKER_MAX_INPUT_TOKENS
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            prune_protected_turns: default_prune_protected_turns(),
            prune_min_savings: default_prune_min_savings(),
            compact_threshold: None,
            compact_request_threshold: None,
            compact_retained_tokens: default_compact_retained_tokens(),
            compact_auto_read_budget: default_compact_auto_read_budget(),
            compact_worker_max_input_tokens: default_compact_worker_max_input_tokens(),
            model: None,
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

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[worker]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert_eq!(manifest.pod.name, "test-agent");
        assert_eq!(manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(
            manifest.model.model_id.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert!(manifest.model.auth.is_none());
        assert_eq!(manifest.scope.allow.len(), 1);
        assert!(manifest.scope.deny.is_empty());
        assert_eq!(manifest.worker.instruction, defaults::DEFAULT_INSTRUCTION);
    }

    #[test]
    fn parse_full_manifest() {
        let toml = r#"
[pod]
name = "code-reviewer"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"
auth = { kind = "api_key", file = "/abs/keys/anthropic" }

[worker]
instruction = "$user/reviewer"
max_tokens = 4096
temperature = 0.3

[[scope.allow]]
target = "/abs/project"
permission = "write"

[[scope.allow]]
target = "/abs/docs"
permission = "read"
recursive = false

[[scope.deny]]
target = "/abs/project/secrets.rs"
permission = "write"
"#;
        let manifest = PodManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.pod.name, "code-reviewer");
        let file = match manifest.model.auth.as_ref() {
            Some(AuthRef::ApiKey { file, .. }) => file.as_deref(),
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(file, Some(std::path::Path::new("/abs/keys/anthropic")));
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

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[worker]
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
        assert_eq!(c.compact_request_threshold, None);
        assert_eq!(c.compact_retained_tokens, 8000);
    }

    #[test]
    fn parse_compaction_both_thresholds() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             compact_threshold = 80000\n\
             compact_request_threshold = 90000\n"
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.compact_threshold, Some(80000));
        assert_eq!(c.compact_request_threshold, Some(90000));
    }

    #[test]
    fn parse_compaction_request_threshold_only() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             compact_request_threshold = 90000\n"
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.compact_threshold, None);
        assert_eq!(c.compact_request_threshold, Some(90000));
    }

    #[test]
    fn parse_compaction_with_model() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             compact_threshold = 80000\n\n\
             [compaction.model]\n\
             scheme = \"gemini\"\n\
             model_id = \"gemini-2.0-flash\"\n"
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        let p = c.model.unwrap();
        assert_eq!(p.scheme, Some(SchemeKind::Gemini));
        assert_eq!(p.model_id.as_deref(), Some("gemini-2.0-flash"));
    }

    #[test]
    fn omitted_compaction_is_none() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.compaction.is_none());
    }

    #[test]
    fn omitted_memory_is_none() {
        let manifest = PodManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.memory.is_none());
    }

    #[test]
    fn empty_memory_section_enables_with_default_root() {
        let toml = format!("{MINIMAL_REQUIRED}\n[memory]\n");
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.expect("memory section parsed");
        assert!(mem.workspace_root.is_none());
    }

    #[test]
    fn memory_section_with_explicit_root() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n[memory]\nworkspace_root = \"/some/where\"\n"
        );
        let manifest = PodManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.unwrap();
        assert_eq!(
            mem.workspace_root.unwrap(),
            std::path::PathBuf::from("/some/where")
        );
    }

    #[test]
    fn reject_unknown_scheme() {
        let toml =
            MINIMAL_REQUIRED.replace("scheme = \"anthropic\"", "scheme = \"unknown_scheme\"");
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
