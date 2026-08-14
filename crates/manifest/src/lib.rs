mod config;
pub mod defaults;
mod model;
pub mod model_catalog;
pub mod paths;
pub mod plugin;
mod profile;
mod scope;

pub use config::{
    CompactionConfigPartial, EngineManifestConfig, FileUploadLimitsPartial,
    PermissionConfigPartial, ResolveError, SessionConfigPartial, ToolOutputLimitsPartial,
    WorkerManifestConfig, WorkerMetaConfig,
};
pub use model::{
    AuthRef, ModelCapability, ModelManifest, ReasoningControl, ReasoningEffort, SchemeKind,
};
pub use paths::user_profiles_path;
pub use profile::{
    ProfileDiscovery, ProfileError, ProfileManifestSnapshot, ProfileMetadata, ProfileRegistry,
    ProfileRegistryEntry, ProfileRegistrySource, ProfileResolveOptions, ProfileResolver,
    ProfileSelector, ProfileSource, ResolvedProfile, resolve_profile_artifact,
    resolve_profile_artifact_value,
};
pub use protocol::{Permission, ScopeRule};
pub use scope::{DelegationScope, Scope, ScopeError, SharedScope};

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

/// Declarative configuration for a Worker.
///
/// Parsed from a TOML manifest file. Describes the model, system prompt,
/// and directory scope (required). The Worker's working directory is **not**
/// part of the manifest — it is the process's `std::env::current_dir()`
/// at construction time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerManifest {
    pub worker: WorkerMeta,
    pub model: ModelManifest,
    pub engine: EngineManifest,
    /// Direct filesystem authority for this Worker's own tools.
    pub scope: ScopeConfig,
    /// Filesystem authority this Worker may pass to spawned children. Missing
    /// metadata/config defaults to no delegation authority.
    #[serde(default)]
    pub delegation_scope: ScopeConfig,
    /// Session/debug persistence settings. Defaults keep extra traces off.
    #[serde(default)]
    pub session: SessionConfig,
    /// Optional manifest-level tool permission policy. Absent means the
    /// permission layer is disabled and tool calls run as before.
    #[serde(default)]
    pub permissions: Option<ToolPermissionConfig>,
    /// Explicit built-in feature/tool-surface enablement. Omitted feature flags
    /// resolve disabled so Profile authors choose the exposed built-in surfaces.
    #[serde(default)]
    pub feature: FeatureConfig,
    /// Explicit plugin package enablement. Discovery remains read-only; only
    /// source-qualified entries listed here may resolve to active plugin metadata.
    #[serde(default)]
    pub plugins: plugin::PluginConfig,
    /// Explicit external Model Context Protocol provider configuration. This
    /// is config data only: declaring a server never starts a subprocess or
    /// grants OS sandboxing. Runtime MCP lifecycle/registration is a separate
    /// consumer boundary.
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
    /// Memory subsystem configuration. Presence of `[memory]` configures memory
    /// storage, extraction, consolidation, and resident injection, but memory
    /// tools are surfaced only when `[feature.memory].enabled = true`.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    /// First-class web tools configuration. Network access remains fail-closed
    /// under this config; WebSearch/WebFetch schemas are surfaced only when
    /// `[feature.web].enabled = true`.
    #[serde(default)]
    pub web: Option<WebConfig>,
    /// External Agent Skills (`SKILL.md`) candidate directories. Each entry
    /// is a path to a skills *root* (i.e. a
    /// directory whose children are individual `<name>/SKILL.md` skill
    /// bundles). Paths are resolved against the manifest's base
    /// directory like other path fields. Absent ⇒ no skills loaded;
    /// there is no implicit `$config_dir/skills/` or builtin probe.
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    /// Optional profile provenance for manifests produced by profile resolution.
    /// Stored only after profile resolution so Worker restore can prefer the
    /// validated snapshot over current profile files or one-file Manifest input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<profile::ProfileManifestSnapshot>,
}

/// Explicit built-in feature/tool-surface enablement. These flags are
/// profile/config data only: they do not carry runtime Worker names, sockets,
/// sessions, secrets, or resolved host state. Tool registration still applies
/// the normal scope, host-authority, backend, memory, and network checks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureConfig {
    #[serde(default)]
    pub task: FeatureFlagConfig,
    #[serde(default)]
    pub memory: MemoryFeatureConfig,
    #[serde(default)]
    pub web: FeatureFlagConfig,
    #[serde(default)]
    pub image: FeatureFlagConfig,
    #[serde(default)]
    pub sub_worker: FeatureFlagConfig,
    #[serde(default)]
    pub flow: FeatureFlagConfig,
    #[serde(default)]
    pub worker: WorkerFeatureConfig,
    #[serde(default)]
    pub objective: FeatureFlagConfig,
    #[serde(default)]
    pub manage_workdir: FeatureFlagConfig,
    #[serde(default)]
    pub ticket: TicketFeatureConfig,
    #[serde(default)]
    pub orchestration: FeatureFlagConfig,
    #[serde(default)]
    pub plugins: FeatureFlagConfig,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            task: FeatureFlagConfig::disabled(),
            memory: MemoryFeatureConfig::disabled(),
            web: FeatureFlagConfig::disabled(),
            image: FeatureFlagConfig::disabled(),
            sub_worker: FeatureFlagConfig::disabled(),
            flow: FeatureFlagConfig::disabled(),
            worker: WorkerFeatureConfig::disabled(),
            objective: FeatureFlagConfig::disabled(),
            manage_workdir: FeatureFlagConfig::disabled(),
            ticket: TicketFeatureConfig::default(),
            orchestration: FeatureFlagConfig::disabled(),
            plugins: FeatureFlagConfig::disabled(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl FeatureFlagConfig {
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }

    pub const fn enabled() -> Self {
        Self { enabled: true }
    }
}

impl Default for FeatureFlagConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerFeatureConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Exposes the generic WorkerSpawn tool. Stateful lifecycle services remain
    /// available to dependent semantic features when this is false.
    #[serde(default = "default_true")]
    pub direct_spawn: bool,
}

impl WorkerFeatureConfig {
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            direct_spawn: true,
        }
    }

    pub const fn enabled() -> Self {
        Self {
            enabled: true,
            direct_spawn: true,
        }
    }
}

impl Default for WorkerFeatureConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFeatureConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Exposes Memory staging queue tools in addition to normal Memory CRUD/query tools.
    #[serde(default)]
    pub staging: bool,
}

impl MemoryFeatureConfig {
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            staging: false,
        }
    }

    pub const fn enabled() -> Self {
        Self {
            enabled: true,
            staging: false,
        }
    }
}

impl Default for MemoryFeatureConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TicketFeatureConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub authoring: bool,
    #[serde(default)]
    pub thread: bool,
    #[serde(default)]
    pub intake: bool,
    #[serde(default)]
    pub workflow: bool,
}

/// External Agent Skills (`SKILL.md`) ingest configuration. Skills are
/// loaded *only* from the directories listed here — there is no
/// implicit `$config_dir/skills/` or builtin probe. Profile and Manifest
/// resolution may compose these entries before validation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Skills *roots*. Children of each root must be individual
    /// `<name>/SKILL.md` bundles; the directory itself is not a skill.
    /// Resolved against the manifest base directory before
    /// [`WorkerManifest`] is materialised.
    #[serde(default)]
    pub directories: Vec<PathBuf>,
}

/// Explicit Model Context Protocol configuration.
///
/// The manifest layer records local stdio MCP server declarations but never
/// starts them. Future lifecycle code must opt in to spawning and must keep MCP
/// process authority separate from Plugin permissions and `worker::feature` flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// Named local stdio servers. The list form keeps declarations explicit and
    /// lets validation reject duplicate names after profile/override merging.
    #[serde(default, rename = "stdio_server")]
    pub stdio_servers: Vec<McpStdioServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpStdioServerConfig {
    /// Stable profile-local name used by later lifecycle/tool-surface code.
    pub name: String,
    /// Executable path/name passed directly to process-spawn code in a later
    /// ticket. This is not a shell string and is not executed by config parsing.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional working-directory policy for the future subprocess. Omitted
    /// means no config-level cwd override. Relative `path` values are resolved
    /// against the manifest/profile layer before final validation.
    #[serde(default)]
    pub cwd: Option<McpStdioCwdPolicy>,
    /// Explicit environment policy. There is no implicit environment discovery;
    /// future spawn code should inherit only names listed here and set only
    /// entries declared here.
    #[serde(default)]
    pub env: McpEnvConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpStdioCwdPolicy {
    /// Leave cwd selection to the lifecycle caller.
    Inherit,
    /// Use this absolute (after path resolution) working directory.
    Path { path: PathBuf },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpEnvConfig {
    /// Host environment variable names to copy explicitly at spawn time.
    #[serde(default)]
    pub inherit: Vec<String>,
    /// Environment variables to set explicitly.
    #[serde(default)]
    pub set: BTreeMap<String, McpEnvValue>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpEnvValue {
    /// Literal value. Use only for non-secret values; Debug/diagnostics redact
    /// it defensively because env values often become credentials over time.
    Literal { value: String },
    /// Local secret-store id. The plaintext is resolved only by a future runtime
    /// consumer and is never loaded during manifest/profile parsing.
    #[serde(rename = "secret_ref")]
    SecretRef {
        #[serde(rename = "ref")]
        ref_: String,
    },
    /// Name of a host environment variable to read explicitly at spawn time.
    EnvRef { name: String },
}

impl fmt::Debug for McpEnvValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal { .. } => f
                .debug_struct("Literal")
                .field("value", &"[redacted]")
                .finish(),
            Self::SecretRef { ref_ } => f.debug_struct("SecretRef").field("ref_", ref_).finish(),
            Self::EnvRef { name } => f.debug_struct("EnvRef").field("name", name).finish(),
        }
    }
}

/// Configuration for WebSearch and WebFetch built-in tools.
///
/// Network tools are fail-closed: absent config or `enabled = false` disables
/// both tools. Per-tool `enabled = false` can disable a tool under an enabled
/// global section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebConfig {
    /// Global opt-in for web tools. Defaults to false when omitted.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Escape hatch for tests / trusted local deployments. Defaults to false.
    #[serde(default)]
    pub allow_private_addresses: Option<bool>,
    #[serde(default)]
    pub search: Option<WebSearchConfig>,
    #[serde(default)]
    pub fetch: Option<WebFetchConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    Brave,
}

/// WebSearch provider configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub provider: Option<WebSearchProvider>,
    /// Local secret-store id for the provider API key. Raw secrets do not
    /// belong in manifest files.
    #[serde(default)]
    pub api_key_secret: Option<String>,
    /// Request timeout in seconds. Tool implementation applies a safe default
    /// when this is omitted.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Optional provider endpoint override for tests/proxies. Defaults to the
    /// Brave web search endpoint for the Brave provider.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub search_lang: Option<String>,
    #[serde(default)]
    pub ui_lang: Option<String>,
    #[serde(default)]
    pub safesearch: Option<String>,
}

/// WebFetch HTTP client limits and policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebFetchConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub redirect_limit: Option<usize>,
    #[serde(default)]
    pub max_response_bytes: Option<usize>,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
    /// Per-fetch escape hatch; when absent falls back to `[web]`
    /// `allow_private_addresses`, then false.
    #[serde(default)]
    pub allow_private_addresses: Option<bool>,
}

/// Memory subsystem configuration. Presence in the manifest enables
/// memory; `workspace_root` pins the memory workspace explicitly. When it
/// is absent, memory resolution searches upward from the Worker's pwd for a
/// `.yoi/memory` marker rather than treating `.yoi` project records alone
/// as a memory root.
///
/// All fields are `Option`; defaults are applied at the consumer
/// (`.unwrap_or(defaults::...)`). This keeps cascade `merge` simple
/// (`upper.x.or(self.x)`) without a separate partial/resolved split.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Override for the memory workspace root. When `None`, consumers resolve
    /// the root from their default path and ancestor `.yoi/memory` markers.
    /// When set, must be an absolute path.
    #[serde(default)]
    pub workspace_root: Option<PathBuf>,
    /// Maximum number of records returned by `MemoryQuery` /
    /// `MemoryQuery` per call. `None` ⇒ tool default (20).
    #[serde(default)]
    pub query_result_limit: Option<usize>,
    /// Lines of context before and after each match in query excerpts.
    /// Ignored when the request omits `query`. `None` ⇒ tool default (3).
    #[serde(default)]
    pub query_excerpt_lines: Option<usize>,
    /// Whether the body of `memory/summary.md` is exposed in the resident
    /// system-prompt section. `None` ⇒ enabled.
    #[serde(default)]
    pub inject_summary: Option<bool>,
    /// Language used by memory extraction / consolidation sub_worker for durable
    /// memory text. Free-form so workspaces can use names like
    /// `English`, `Japanese`, or locale tags. `None` ⇒
    /// [`defaults::MEMORY_LANGUAGE`].
    #[serde(default)]
    pub language: Option<String>,
    /// Optional model for the extract worker. When `None`,
    /// the main engine model is cloned via `clone_boxed()`. Lightweight
    /// reasoning-capable models (Haiku / 4o-mini / Flash class) are
    /// recommended.
    #[serde(default)]
    pub extract_model: Option<ModelManifest>,
    /// Cumulative input-token threshold (since the last extract pointer)
    /// that triggers an extract run. `None` disables the extract trigger
    /// entirely; memory tools and resident injection still work, only
    /// the auto-extract trigger is dormant.
    #[serde(default)]
    pub extract_threshold: Option<u64>,
    /// Optional maximum extract-worker tool-loop depth. `None` leaves
    /// the worker unlimited; the default bounds runaway short-context
    /// loops. Falls through to
    /// [`defaults::MEMORY_EXTRACT_WORKER_MAX_TURNS`] when unset.
    #[serde(default)]
    pub extract_worker_max_turns: Option<u32>,
    /// Optional model for the consolidation worker. When
    /// `None`, the main engine model is cloned via `clone_boxed()`.
    /// Reasoning-class models are recommended.
    #[serde(default)]
    pub consolidation_model: Option<ModelManifest>,
    /// Consolidation trigger: file-count threshold of `_staging/`. The
    /// consolidation run fires when the staging directory has at least
    /// this many entries. Either threshold reaching its limit fires
    /// consolidation (logical OR). `None` for both thresholds ⇒
    /// consolidation disabled.
    #[serde(default)]
    pub consolidation_threshold_files: Option<usize>,
    /// Consolidation trigger: byte-size threshold across all `_staging/`
    /// entries. Either threshold reaching its limit fires consolidation.
    /// `None` for both thresholds ⇒ consolidation disabled.
    #[serde(default)]
    pub consolidation_threshold_bytes: Option<u64>,
}

/// Worker metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerMeta {
    pub name: String,
}

/// Worker-level configuration embedded in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineManifest {
    /// Exact catalog-root dotted Prompt name (for example `default` or
    /// `role.coder`).
    #[serde(default = "default_instruction")]
    pub instruction: String,
    /// Language policy used by the main worker for normal prose responses.
    /// Free-form so workspaces can use names like `English`, `Japanese`,
    /// locale tags, or a policy phrase. Unset manifests fall through to
    /// [`defaults::WORKER_LANGUAGE`].
    #[serde(default = "default_worker_language")]
    pub language: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<NonZeroU32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub stop_sequences: Vec<String>,
    #[serde(default)]
    pub reasoning: Option<ReasoningControl>,
    /// Byte-size caps applied to tool `content` before it reaches the
    /// conversation history. The section is optional in TOML — when
    /// omitted, `ToolOutputLimits::default()` (64 KiB default cap, no
    /// per-tool overrides) is applied so truncation is on by default.
    #[serde(default)]
    pub tool_output: ToolOutputLimits,
    /// Byte-size cap applied to submit-time FileRef uploads / attachments.
    /// For file refs this caps the file body; for normal directory refs this
    /// caps the rendered shallow listing body.
    /// This is intentionally separate from tool-output truncation because
    /// user-requested file attachments can usually tolerate a larger budget.
    #[serde(default)]
    pub file_upload: FileUploadLimits,
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

/// Byte-size cap for submit-time FileRef uploads / attachments.
///
/// This governs the `[File: <path>]` system-message attachment produced
/// when a user explicitly submits a `@<path>` file reference, and the
/// rendered body of a shallow `[Dir: <path>]` listing for a normal directory
/// reference. It does not affect tool result truncation; see
/// [`ToolOutputLimits`] for that path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileUploadLimits {
    /// Cap applied to each resolved FileRef file body or directory-listing body.
    #[serde(default = "default_file_upload_max_bytes")]
    pub max_bytes: usize,
}

fn default_tool_output_max_bytes() -> usize {
    defaults::TOOL_OUTPUT_MAX_BYTES
}

fn default_file_upload_max_bytes() -> usize {
    defaults::FILE_UPLOAD_MAX_BYTES
}

fn default_instruction() -> String {
    defaults::DEFAULT_INSTRUCTION.to_string()
}

fn default_worker_language() -> String {
    defaults::WORKER_LANGUAGE.to_string()
}

impl Default for ToolOutputLimits {
    fn default() -> Self {
        Self {
            default_max_bytes: default_tool_output_max_bytes(),
            per_tool: HashMap::new(),
        }
    }
}

impl Default for FileUploadLimits {
    fn default() -> Self {
        Self {
            max_bytes: default_file_upload_max_bytes(),
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
/// A Worker may only touch paths whose effective permission (computed from
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SessionConfig {
    /// Persist normalized provider stream events and lifecycle diagnostics to a
    /// `.trace.jsonl` sidecar next to the segment log. This is not guaranteed to
    /// be a byte-for-byte raw SSE capture. Intended for debugging stalls between
    /// stream requests; off by default because it can be verbose.
    #[serde(default)]
    pub record_event_trace: bool,
}

/// Manifest-level pattern-based tool permission policy.
///
/// Presence of `[permissions]` enables this layer. Rules are evaluated
/// in declaration order; if none match, [`Self::default_action`] is used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPermissionConfig {
    pub default_action: ToolPermissionAction,
    #[serde(default, rename = "rule")]
    pub rules: Vec<ToolPermissionRule>,
}

/// One `[[permissions.rule]]` entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPermissionRule {
    /// Tool registration name. Matching is case-insensitive at runtime so
    /// manifests may use either `Bash` or `bash`.
    pub tool: String,
    /// Glob-like pattern matched against the tool's permission target
    /// (for built-in tools, commonly `command`, `file_path`, or `pattern`).
    pub pattern: String,
    pub action: ToolPermissionAction,
}

/// Tool permission decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionAction {
    Allow,
    Deny,
    Ask,
}

/// Context compaction configuration.
///
/// Controls Prune (content removal from old tool results) and Compact
/// (full history summarisation). Omitting `[compaction]` disables both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Token budget at the history tail protected from pruning.
    #[serde(default = "default_prune_protected_tokens")]
    pub prune_protected_tokens: u64,

    /// Minimum estimated token savings to trigger a prune.
    #[serde(default = "default_prune_min_savings")]
    pub prune_min_savings: u64,

    /// Proactive (between-turns) compaction threshold.
    ///
    /// Checked by the Controller after each run. When current occupancy
    /// exceeds this value, compact runs before the next turn. `None`
    /// disables the between-turns check.
    #[serde(default, alias = "compact_threshold")]
    pub threshold: Option<u64>,

    /// Safety-net (between-requests) compaction threshold.
    ///
    /// Checked by `WorkerInterceptor::pre_llm_request` inside a turn. When
    /// current occupancy exceeds this value, the run yields so that the
    /// Controller can compact before the next LLM request. `None`
    /// disables the between-requests check.
    ///
    /// Expected relation: `threshold < request_threshold` (proactive triggers
    /// before safety net). A reversed configuration is accepted but logged as
    /// a warning.
    #[serde(default, alias = "compact_request_threshold")]
    pub request_threshold: Option<u64>,

    /// Token budget retained verbatim at the tail of the history after
    /// compaction. Measured against the occupancy estimate from
    /// `UsageRecord` history; turn boundaries are ignored.
    #[serde(default = "default_retained_tokens", alias = "compact_retained_tokens")]
    pub retained_tokens: u64,

    /// Target size for the deterministic overview/index fed to the compact
    /// worker. Overshooting this target is not an error.
    #[serde(default = "default_overview_target_tokens")]
    pub overview_target_tokens: u64,

    /// Warning threshold for deterministic overview/index size.
    #[serde(default = "default_overview_warning_tokens")]
    pub overview_warning_tokens: u64,

    /// Deadline threshold for deterministic overview/index generation.
    /// Oversized overviews fall back to a coarser deterministic index.
    #[serde(default = "default_overview_deadline_tokens")]
    pub overview_deadline_tokens: u64,

    /// Current prompt-occupancy cap for the compact worker's own LLM
    /// requests. Exceeding this aborts the compact run.
    #[serde(
        default = "default_worker_context_max_tokens",
        alias = "compact_worker_max_input_tokens"
    )]
    pub worker_context_max_tokens: u64,

    /// Remaining compact-worker context threshold that triggers a warning and
    /// an instruction to stop exploring and call `write_summary`.
    #[serde(default = "default_finish_warning_remaining_tokens")]
    pub finish_warning_remaining_tokens: u64,

    /// Context reserve preserved for final summary/tool closing turns.
    #[serde(default = "default_final_reserve_tokens")]
    pub final_reserve_tokens: u64,

    /// Optional maximum compact-worker tool-loop depth. `None` leaves the
    /// worker unlimited; the default bounds runaway short-context loops.
    #[serde(
        default = "default_worker_max_turns",
        alias = "compact_worker_max_turns"
    )]
    pub worker_max_turns: Option<u32>,

    /// Target size for the `write_summary` text. Used in prompt/nudge text.
    #[serde(default = "default_summary_target_tokens")]
    pub summary_target_tokens: u64,

    /// Hard validation cap for the final `write_summary` text.
    #[serde(default = "default_summary_max_tokens")]
    pub summary_max_tokens: u64,

    /// Aggregate token budget for auto-read file contents injected into
    /// the compacted session by the compact worker.
    #[serde(
        default = "default_auto_read_budget_tokens",
        alias = "compact_auto_read_budget"
    )]
    pub auto_read_budget_tokens: u64,

    /// Dry-run cap for the compacted session's initial request context.
    #[serde(default = "default_result_context_max_tokens")]
    pub result_context_max_tokens: u64,

    /// Optional model for the compactor (summary) LLM.
    /// If omitted, the main model is cloned via `clone_boxed()`.
    #[serde(default)]
    pub model: Option<ModelManifest>,
}

fn default_prune_protected_tokens() -> u64 {
    defaults::PRUNE_PROTECTED_TOKENS
}
fn default_prune_min_savings() -> u64 {
    defaults::PRUNE_MIN_SAVINGS
}
fn default_retained_tokens() -> u64 {
    defaults::COMPACT_RETAINED_TOKENS
}
fn default_overview_target_tokens() -> u64 {
    defaults::COMPACT_OVERVIEW_TARGET_TOKENS
}
fn default_overview_warning_tokens() -> u64 {
    defaults::COMPACT_OVERVIEW_WARNING_TOKENS
}
fn default_overview_deadline_tokens() -> u64 {
    defaults::COMPACT_OVERVIEW_DEADLINE_TOKENS
}
fn default_worker_context_max_tokens() -> u64 {
    defaults::COMPACT_WORKER_MAX_INPUT_TOKENS
}
fn default_finish_warning_remaining_tokens() -> u64 {
    defaults::COMPACT_FINISH_WARNING_REMAINING_TOKENS
}
fn default_final_reserve_tokens() -> u64 {
    defaults::COMPACT_FINAL_RESERVE_TOKENS
}
fn default_worker_max_turns() -> Option<u32> {
    defaults::COMPACT_WORKER_MAX_TURNS
}
fn default_summary_target_tokens() -> u64 {
    defaults::COMPACT_SUMMARY_TARGET_TOKENS
}
fn default_summary_max_tokens() -> u64 {
    defaults::COMPACT_SUMMARY_MAX_TOKENS
}
fn default_auto_read_budget_tokens() -> u64 {
    defaults::COMPACT_AUTO_READ_BUDGET
}
fn default_result_context_max_tokens() -> u64 {
    defaults::COMPACT_RESULT_CONTEXT_MAX_TOKENS
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            prune_protected_tokens: default_prune_protected_tokens(),
            prune_min_savings: default_prune_min_savings(),
            threshold: None,
            request_threshold: None,
            retained_tokens: default_retained_tokens(),
            overview_target_tokens: default_overview_target_tokens(),
            overview_warning_tokens: default_overview_warning_tokens(),
            overview_deadline_tokens: default_overview_deadline_tokens(),
            worker_context_max_tokens: default_worker_context_max_tokens(),
            finish_warning_remaining_tokens: default_finish_warning_remaining_tokens(),
            final_reserve_tokens: default_final_reserve_tokens(),
            worker_max_turns: default_worker_max_turns(),
            summary_target_tokens: default_summary_target_tokens(),
            summary_max_tokens: default_summary_max_tokens(),
            auto_read_budget_tokens: default_auto_read_budget_tokens(),
            result_context_max_tokens: default_result_context_max_tokens(),
            model: None,
        }
    }
}

impl WorkerManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        config::reject_removed_manifest_fields(s)?;
        let manifest: Self = toml::from_str(s)?;
        config::validate_mcp_config(&manifest.mcp)
            .map_err(|error| toml::de::Error::custom(error.to_string()))?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_REQUIRED: &str = r#"
[worker]
name = "test-agent"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert_eq!(manifest.worker.name, "test-agent");
        assert_eq!(manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(
            manifest.model.model_id.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert!(manifest.model.auth.is_none());
        assert_eq!(manifest.scope.allow.len(), 1);
        assert!(manifest.scope.deny.is_empty());
        assert!(manifest.delegation_scope.allow.is_empty());
        assert!(manifest.delegation_scope.deny.is_empty());
        assert_eq!(manifest.engine.instruction, defaults::DEFAULT_INSTRUCTION);
        assert!(manifest.engine.top_p.is_none());
        assert!(manifest.engine.top_k.is_none());
        assert!(manifest.engine.stop_sequences.is_empty());
        assert!(manifest.web.is_none());
    }

    #[test]
    fn deserialize_old_manifest_snapshot_defaults_to_no_delegation() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        let mut snapshot = serde_json::to_value(&manifest).unwrap();
        snapshot.as_object_mut().unwrap().remove("delegation_scope");
        let restored: WorkerManifest = serde_json::from_value(snapshot).unwrap();
        assert_eq!(restored.scope.allow.len(), 1);
        assert!(restored.delegation_scope.allow.is_empty());
        assert!(restored.delegation_scope.deny.is_empty());
    }

    #[test]
    fn parse_web_config() {
        let toml = format!(
            "{}\n[web]\nenabled = true\n\n[web.search]\nprovider = \"brave\"\napi_key_secret = \"web/brave/default\"\ntimeout_secs = 12\n\n[web.fetch]\ntimeout_secs = 7\nredirect_limit = 3\nmax_response_bytes = 12345\nmax_output_bytes = 2048\n",
            MINIMAL_REQUIRED
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let web = manifest.web.unwrap();
        assert_eq!(web.enabled, Some(true));
        let search = web.search.unwrap();
        assert_eq!(search.provider, Some(WebSearchProvider::Brave));
        assert_eq!(search.timeout_secs, Some(12));
        let fetch = web.fetch.unwrap();
        assert_eq!(fetch.timeout_secs, Some(7));
        assert_eq!(fetch.redirect_limit, Some(3));
        assert_eq!(fetch.max_response_bytes, Some(12345));
        assert_eq!(fetch.max_output_bytes, Some(2048));
    }

    #[test]
    fn parse_full_manifest() {
        let toml = r#"
[worker]
name = "code-reviewer"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"
auth = { kind = "api_key", file = "/abs/keys/anthropic" }

[engine]
instruction = "role.reviewer"
max_tokens = 4096
temperature = 0.3
top_p = 0.9
top_k = 40
stop_sequences = ["\n\n", "</stop>"]
reasoning = "medium"

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

[[delegation_scope.allow]]
target = "/abs/project/tasks"
permission = "write"

[[delegation_scope.deny]]
target = "/abs/project/tasks/private"
permission = "write"
"#;
        let manifest = WorkerManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.worker.name, "code-reviewer");
        let file = match manifest.model.auth.as_ref() {
            Some(AuthRef::ApiKey { file, .. }) => file.as_deref(),
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(file, Some(std::path::Path::new("/abs/keys/anthropic")));
        assert_eq!(manifest.engine.instruction, "role.reviewer");
        assert_eq!(manifest.engine.max_tokens, Some(4096));
        assert_eq!(manifest.engine.temperature, Some(0.3));
        assert_eq!(manifest.engine.top_p, Some(0.9));
        assert_eq!(manifest.engine.top_k, Some(40));
        assert_eq!(manifest.engine.stop_sequences, vec!["\n\n", "</stop>"]);
        assert_eq!(
            manifest.engine.reasoning,
            Some(ReasoningControl::Effort(ReasoningEffort::Medium))
        );
        let allow = &manifest.scope.allow;
        assert_eq!(allow.len(), 2);
        assert_eq!(allow[0].permission, Permission::Write);
        assert!(allow[0].recursive);
        assert_eq!(allow[1].permission, Permission::Read);
        assert!(!allow[1].recursive);
        assert_eq!(manifest.scope.deny.len(), 1);
        assert_eq!(manifest.scope.deny[0].permission, Permission::Write);
        assert_eq!(manifest.delegation_scope.allow.len(), 1);
        assert_eq!(
            manifest.delegation_scope.allow[0].permission,
            Permission::Write
        );
        assert_eq!(manifest.delegation_scope.deny.len(), 1);
    }

    #[test]
    fn reject_missing_scope() {
        let toml = r#"
[worker]
name = "missing-scope"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
"#;
        assert!(WorkerManifest::from_toml(toml).is_err());
    }

    #[test]
    fn parse_plugin_enablement_config() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [[plugins.enabled]]\n\
             id = \"project:example\"\n\
             version = \"0.1.0\"\n\
             digest = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n\
             surfaces = [\"hook\"]\n\n\
             [plugins.enabled.config]\n\
             greeting = \"hello\"\n"
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        assert_eq!(manifest.plugins.enabled.len(), 1);
        let enabled = &manifest.plugins.enabled[0];
        assert_eq!(enabled.id, "project:example");
        assert_eq!(
            enabled.version.as_ref().map(|version| version.0.as_str()),
            Some("0.1.0")
        );
        assert_eq!(enabled.surfaces, vec![plugin::PluginSurface::Hook]);
        assert_eq!(
            enabled
                .config
                .as_ref()
                .and_then(|value| value.get("greeting"))
                .and_then(|value| value.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn parse_max_turns() {
        let toml = MINIMAL_REQUIRED.replace("[engine]\n", "[engine]\nmax_turns = 50\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        assert_eq!(manifest.engine.max_turns.unwrap().get(), 50);
    }

    #[test]
    fn parse_reasoning_budget() {
        let toml = MINIMAL_REQUIRED.replace("[engine]\n", "[engine]\nreasoning = -1\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        assert_eq!(
            manifest.engine.reasoning,
            Some(ReasoningControl::BudgetTokens(-1))
        );
    }

    #[test]
    fn omitted_max_turns_is_none() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.engine.max_turns.is_none());
    }

    #[test]
    fn reject_max_turns_zero() {
        let toml = MINIMAL_REQUIRED.replace("[engine]\n", "[engine]\nmax_turns = 0\n");
        assert!(WorkerManifest::from_toml(&toml).is_err());
    }

    #[test]
    fn parse_compaction_config() {
        let toml = format!("{MINIMAL_REQUIRED}\n[compaction]\nthreshold = 80000\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.prune_protected_tokens, 8000);
        assert_eq!(c.prune_min_savings, 4096);
        assert_eq!(c.threshold, Some(80000));
        assert_eq!(c.request_threshold, None);
        assert_eq!(c.retained_tokens, 8000);
        assert_eq!(c.worker_max_turns, Some(20));
    }

    #[test]
    fn reject_removed_prune_protected_turns_field() {
        let toml = format!("{MINIMAL_REQUIRED}\n[compaction]\nprune_protected_turns = 3\n");
        let err = WorkerManifest::from_toml(&toml).unwrap_err();
        assert!(
            err.to_string().contains("compaction.prune_protected_turns"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_compaction_worker_max_turns() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             worker_max_turns = 7\n"
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.worker_max_turns, Some(7));
    }

    #[test]
    fn parse_compaction_both_thresholds() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             threshold = 80000\n\
             request_threshold = 90000\n"
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.threshold, Some(80000));
        assert_eq!(c.request_threshold, Some(90000));
    }

    #[test]
    fn parse_compaction_request_threshold_only() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             request_threshold = 90000\n"
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        assert_eq!(c.threshold, None);
        assert_eq!(c.request_threshold, Some(90000));
    }

    #[test]
    fn parse_compaction_with_model() {
        let toml = format!(
            "{MINIMAL_REQUIRED}\n\
             [compaction]\n\
             threshold = 80000\n\n\
             [compaction.model]\n\
             scheme = \"gemini\"\n\
             model_id = \"gemini-2.0-flash\"\n"
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let c = manifest.compaction.unwrap();
        let p = c.model.unwrap();
        assert_eq!(p.scheme, Some(SchemeKind::Gemini));
        assert_eq!(p.model_id.as_deref(), Some("gemini-2.0-flash"));
    }

    #[test]
    fn omitted_compaction_is_none() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.compaction.is_none());
    }

    #[test]
    fn omitted_memory_is_none() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert!(manifest.memory.is_none());
    }

    #[test]
    fn empty_memory_section_enables_with_default_root() {
        let toml = format!("{MINIMAL_REQUIRED}\n[memory]\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.expect("memory section parsed");
        assert!(mem.workspace_root.is_none());
        assert_eq!(mem.inject_summary, None);
    }

    #[test]
    fn memory_section_with_inject_summary_false() {
        let toml = format!("{MINIMAL_REQUIRED}\n[memory]\ninject_summary = false\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.unwrap();
        assert_eq!(mem.inject_summary, Some(false));
    }

    #[test]
    fn memory_section_with_explicit_root() {
        let toml = format!("{MINIMAL_REQUIRED}\n[memory]\nworkspace_root = \"/some/where\"\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.unwrap();
        assert_eq!(
            mem.workspace_root.unwrap(),
            std::path::PathBuf::from("/some/where")
        );
    }

    #[test]
    fn memory_section_with_language() {
        let toml = format!("{MINIMAL_REQUIRED}\n[memory]\nlanguage = \"Japanese\"\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let mem = manifest.memory.unwrap();
        assert_eq!(mem.language.as_deref(), Some("Japanese"));
    }

    #[test]
    fn reject_unknown_scheme() {
        let toml =
            MINIMAL_REQUIRED.replace("scheme = \"anthropic\"", "scheme = \"unknown_scheme\"");
        assert!(WorkerManifest::from_toml(&toml).is_err());
    }

    #[test]
    fn omitted_limits_fall_back_to_defaults() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        let limits = &manifest.engine.tool_output;
        assert_eq!(limits.default_max_bytes, defaults::TOOL_OUTPUT_MAX_BYTES);
        assert!(limits.per_tool.is_empty());
        assert_eq!(
            manifest.engine.file_upload.max_bytes,
            defaults::FILE_UPLOAD_MAX_BYTES
        );
    }

    #[test]
    fn worker_language_defaults_and_parses() {
        let manifest = WorkerManifest::from_toml(MINIMAL_REQUIRED).unwrap();
        assert_eq!(manifest.engine.language, defaults::WORKER_LANGUAGE);

        let toml = MINIMAL_REQUIRED.replace("[engine]\n", "[engine]\nlanguage = \"Japanese\"\n");
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        assert_eq!(manifest.engine.language, "Japanese");
    }

    #[test]
    fn parse_worker_output_limits() {
        let toml = MINIMAL_REQUIRED.replace(
            "[engine]\n",
            "[engine]\n\
             [engine.tool_output]\n\
             default_max_bytes = 8192\n\n\
             [engine.tool_output.per_tool]\n\
             Read = 32768\n\
             Grep = 4096\n\n\
             [engine.file_upload]\n\
             max_bytes = 12345\n",
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let limits = &manifest.engine.tool_output;
        assert_eq!(limits.default_max_bytes, 8192);
        assert_eq!(limits.limit_for("Read"), 32768);
        assert_eq!(limits.limit_for("Grep"), 4096);
        assert_eq!(limits.limit_for("Unknown"), 8192);
        assert_eq!(manifest.engine.file_upload.max_bytes, 12345);
    }

    #[test]
    fn empty_tool_output_section_uses_default_max_bytes() {
        let toml = MINIMAL_REQUIRED.replace(
            "[engine]\n",
            "[engine]\n\
             [engine.tool_output]\n",
        );
        let manifest = WorkerManifest::from_toml(&toml).unwrap();
        let limits = &manifest.engine.tool_output;
        assert_eq!(limits.default_max_bytes, defaults::TOOL_OUTPUT_MAX_BYTES);
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
