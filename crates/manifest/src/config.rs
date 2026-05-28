//! Partial-form of [`crate::PodManifest`] used as cascade layers.
//!
//! `PodManifestConfig` mirrors `PodManifest` but every field is optional
//! so individual layers (builtin defaults, user manifest, project
//! manifest, programmatic overlay) can be partial. Layers are combined
//! via [`PodManifestConfig::merge`] and the final config is converted to
//! a validated [`PodManifest`] via `TryFrom`.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::model::{AuthRef, ModelManifest, ReasoningControl};
use crate::{
    CompactionConfig, FileUploadLimits, MemoryConfig, PodManifest, PodMeta, ScopeConfig,
    SessionConfig, SkillsConfig, ToolOutputLimits, ToolPermissionConfig, ToolPermissionRule,
    WorkerManifest,
};

/// Partial-form Pod manifest. Every field is optional; one or more
/// instances merge via [`PodManifestConfig::merge`] before being
/// converted to a validated [`PodManifest`] via `TryFrom`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodManifestConfig {
    #[serde(default)]
    pub pod: PodMetaConfig,
    /// `[model]` セクションは partial でも完成形でも同じ
    /// [`ModelManifest`] を使う。ref / inline の両形を受け入れるための
    /// 全 Optional 構造なので、カスケード層と最終マニフェストで型を
    /// 分ける必要がない。
    #[serde(default)]
    pub model: ModelManifest,
    #[serde(default)]
    pub worker: WorkerManifestConfig,
    #[serde(default)]
    pub scope: ScopeConfig,
    #[serde(default)]
    pub session: Option<SessionConfigPartial>,
    /// Optional `[permissions]` section. `None` means the permission layer
    /// is disabled; `Some` requires `default_action` during final resolve.
    #[serde(default)]
    pub permissions: Option<PermissionConfigPartial>,
    #[serde(default)]
    pub compaction: Option<CompactionConfigPartial>,
    /// Memory subsystem opt-in. See [`MemoryConfig`].
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    /// External Agent Skills directories. See [`crate::SkillsConfig`].
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodMetaConfig {
    #[serde(default)]
    pub name: Option<String>,
    /// Optional `PromptCatalog` manifest pack override. See
    /// [`crate::PodMeta::prompt_pack`] for semantics. Relative paths
    /// are resolved through [`PodManifestConfig::resolve_paths`].
    #[serde(default)]
    pub prompt_pack: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerManifestConfig {
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
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
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    pub reasoning: Option<ReasoningControl>,
    #[serde(default)]
    pub tool_output: ToolOutputLimitsPartial,
    #[serde(default)]
    pub file_upload: FileUploadLimitsPartial,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutputLimitsPartial {
    #[serde(default)]
    pub default_max_bytes: Option<usize>,
    #[serde(default)]
    pub per_tool: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileUploadLimitsPartial {
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfigPartial {
    #[serde(default)]
    pub record_event_trace: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionConfigPartial {
    #[serde(default)]
    pub default_action: Option<crate::ToolPermissionAction>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<ToolPermissionRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionConfigPartial {
    #[serde(default)]
    pub prune_protected_tokens: Option<u64>,
    #[serde(default)]
    pub prune_min_savings: Option<u64>,
    #[serde(default, alias = "compact_threshold")]
    pub threshold: Option<u64>,
    #[serde(default, alias = "compact_request_threshold")]
    pub request_threshold: Option<u64>,
    #[serde(default, alias = "compact_retained_tokens")]
    pub retained_tokens: Option<u64>,
    #[serde(default)]
    pub overview_target_tokens: Option<u64>,
    #[serde(default)]
    pub overview_warning_tokens: Option<u64>,
    #[serde(default)]
    pub overview_deadline_tokens: Option<u64>,
    #[serde(default, alias = "compact_worker_max_input_tokens")]
    pub worker_context_max_tokens: Option<u64>,
    #[serde(default)]
    pub finish_warning_remaining_tokens: Option<u64>,
    #[serde(default)]
    pub final_reserve_tokens: Option<u64>,
    #[serde(default, alias = "compact_worker_max_turns")]
    pub worker_max_turns: Option<u32>,
    #[serde(default)]
    pub summary_target_tokens: Option<u64>,
    #[serde(default)]
    pub summary_max_tokens: Option<u64>,
    #[serde(default, alias = "compact_auto_read_budget")]
    pub auto_read_budget_tokens: Option<u64>,
    #[serde(default)]
    pub result_context_max_tokens: Option<u64>,
    #[serde(default)]
    pub model: Option<ModelManifest>,
}

/// Errors raised when converting a [`PodManifestConfig`] to a validated
/// [`PodManifest`] via `TryFrom`.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("path must be absolute ({field}): {}", .path.display())]
    RelativePath { field: &'static str, path: PathBuf },
}

/// Reject manifest fields that were intentionally removed and must not be
/// silently swallowed by the general warn-and-ignore unknown-field policy.
pub(crate) fn reject_removed_manifest_fields(s: &str) -> Result<(), toml::de::Error> {
    let value: toml::Value = toml::from_str(s)?;
    if value
        .get("compaction")
        .and_then(toml::Value::as_table)
        .is_some_and(|table| table.contains_key("prune_protected_turns"))
    {
        return Err(toml::de::Error::custom(
            "unknown field in manifest: compaction.prune_protected_turns \
             (removed; use compaction.prune_protected_tokens)",
        ));
    }
    if value
        .get("memory")
        .and_then(toml::Value::as_table)
        .is_some_and(|table| table.contains_key("extract_worker_max_input_tokens"))
    {
        return Err(toml::de::Error::custom(
            "unknown field in manifest: memory.extract_worker_max_input_tokens (removed)",
        ));
    }
    Ok(())
}

impl PodManifestConfig {
    /// Parse a partial manifest from a TOML string. Unknown top-level or
    /// nested fields emit a `tracing::warn!` and are ignored; use
    /// `tracing_subscriber` with `WARN` enabled to surface them to the
    /// operator. Removed fields that must not be silently ignored (currently
    /// `compaction.prune_protected_turns`) are rejected before deserialization.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        reject_removed_manifest_fields(s)?;
        let de = toml::Deserializer::parse(s)?;
        serde_ignored::deserialize(de, |path| {
            tracing::warn!("unknown field in manifest: {}", path);
        })
    }

    /// Cascade layer populated with the in-code defaults listed in
    /// [`crate::defaults`]. Used by [`PodFactory::resolve`] as the
    /// bottom layer, so every per-field default lives at exactly one
    /// call site (the `defaults` module).
    ///
    /// `TryFrom<PodManifestConfig>` also reads the same constants as a
    /// belt-and-suspenders fallback, so a manually-constructed config
    /// that skips this layer still resolves to the same values.
    pub fn builtin_defaults() -> Self {
        Self {
            worker: WorkerManifestConfig {
                tool_output: ToolOutputLimitsPartial {
                    default_max_bytes: Some(defaults::TOOL_OUTPUT_MAX_BYTES),
                    per_tool: HashMap::new(),
                },
                file_upload: FileUploadLimitsPartial {
                    max_bytes: Some(defaults::FILE_UPLOAD_MAX_BYTES),
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Resolve every relative path inside this partial config against
    /// `base` (assumed absolute). Paths that are already absolute are
    /// left untouched. This is the only place per-layer path resolution
    /// happens — cascade merge runs against fully absolute paths so
    /// rules from different layers do not accidentally inherit another
    /// layer's base.
    ///
    /// Affected fields: `model.auth.file`,
    /// `scope.allow[].target`, `scope.deny[].target`,
    /// `compaction.model.auth.file`.
    pub fn resolve_paths(mut self, base: &Path) -> Self {
        debug_assert!(
            base.is_absolute(),
            "resolve_paths base must be absolute: {}",
            base.display()
        );
        resolve_auth_file(&mut self.model.auth, base);
        if let Some(ref mut pack) = self.pod.prompt_pack {
            *pack = join_if_relative(base, pack);
        }
        for rule in &mut self.scope.allow {
            rule.target = join_if_relative(base, &rule.target);
        }
        for rule in &mut self.scope.deny {
            rule.target = join_if_relative(base, &rule.target);
        }
        if let Some(ref mut memory) = self.memory
            && let Some(ref mut root) = memory.workspace_root
        {
            *root = join_if_relative(base, root);
        }
        if let Some(ref mut compaction) = self.compaction
            && let Some(ref mut cp) = compaction.model
        {
            resolve_auth_file(&mut cp.auth, base);
        }
        if let Some(ref mut skills) = self.skills {
            for dir in &mut skills.directories {
                *dir = join_if_relative(base, dir);
            }
        }
        self
    }

    /// Merge `upper` into `self`. Fields present in `upper` override
    /// fields from `self`. Map entries merge key-wise with `upper`
    /// winning on conflict. Scope rules from both layers accumulate
    /// (see [`ScopeConfig`] semantics).
    pub fn merge(self, upper: PodManifestConfig) -> Self {
        Self {
            pod: self.pod.merge(upper.pod),
            model: self.model.merge(upper.model),
            worker: self.worker.merge(upper.worker),
            scope: merge_scope(self.scope, upper.scope),
            session: merge_option(self.session, upper.session, SessionConfigPartial::merge),
            permissions: merge_option(
                self.permissions,
                upper.permissions,
                PermissionConfigPartial::merge,
            ),
            compaction: merge_option(
                self.compaction,
                upper.compaction,
                CompactionConfigPartial::merge,
            ),
            memory: merge_option(self.memory, upper.memory, MemoryConfig::merge),
            skills: merge_option(self.skills, upper.skills, SkillsConfig::merge),
        }
    }
}

impl SkillsConfig {
    fn merge(mut self, upper: Self) -> Self {
        self.directories.extend(upper.directories);
        self
    }
}

impl MemoryConfig {
    fn merge(self, upper: Self) -> Self {
        Self {
            workspace_root: upper.workspace_root.or(self.workspace_root),
            query_result_limit: upper.query_result_limit.or(self.query_result_limit),
            query_excerpt_lines: upper.query_excerpt_lines.or(self.query_excerpt_lines),
            inject_summary: upper.inject_summary.or(self.inject_summary),
            language: upper.language.or(self.language),
            extract_model: upper.extract_model.or(self.extract_model),
            extract_threshold: upper.extract_threshold.or(self.extract_threshold),
            extract_worker_max_turns: upper
                .extract_worker_max_turns
                .or(self.extract_worker_max_turns),
            consolidation_model: upper.consolidation_model.or(self.consolidation_model),
            consolidation_threshold_files: upper
                .consolidation_threshold_files
                .or(self.consolidation_threshold_files),
            consolidation_threshold_bytes: upper
                .consolidation_threshold_bytes
                .or(self.consolidation_threshold_bytes),
        }
    }
}

impl PodMetaConfig {
    fn merge(self, upper: Self) -> Self {
        Self {
            name: upper.name.or(self.name),
            prompt_pack: upper.prompt_pack.or(self.prompt_pack),
        }
    }
}

impl WorkerManifestConfig {
    fn merge(self, upper: Self) -> Self {
        Self {
            instruction: upper.instruction.or(self.instruction),
            language: upper.language.or(self.language),
            max_tokens: upper.max_tokens.or(self.max_tokens),
            max_turns: upper.max_turns.or(self.max_turns),
            temperature: upper.temperature.or(self.temperature),
            top_p: upper.top_p.or(self.top_p),
            top_k: upper.top_k.or(self.top_k),
            stop_sequences: upper.stop_sequences.or(self.stop_sequences),
            reasoning: upper.reasoning.or(self.reasoning),
            tool_output: self.tool_output.merge(upper.tool_output),
            file_upload: self.file_upload.merge(upper.file_upload),
        }
    }
}

impl ToolOutputLimitsPartial {
    fn merge(self, upper: Self) -> Self {
        let mut per_tool = self.per_tool;
        per_tool.extend(upper.per_tool);
        Self {
            default_max_bytes: upper.default_max_bytes.or(self.default_max_bytes),
            per_tool,
        }
    }
}

impl FileUploadLimitsPartial {
    fn merge(self, upper: Self) -> Self {
        Self {
            max_bytes: upper.max_bytes.or(self.max_bytes),
        }
    }
}

impl SessionConfigPartial {
    fn merge(self, upper: Self) -> Self {
        Self {
            record_event_trace: upper.record_event_trace.or(self.record_event_trace),
        }
    }
}

impl PermissionConfigPartial {
    fn merge(mut self, upper: Self) -> Self {
        self.rules.extend(upper.rules);
        Self {
            default_action: upper.default_action.or(self.default_action),
            rules: self.rules,
        }
    }
}

impl CompactionConfigPartial {
    fn merge(self, upper: Self) -> Self {
        Self {
            prune_protected_tokens: upper.prune_protected_tokens.or(self.prune_protected_tokens),
            prune_min_savings: upper.prune_min_savings.or(self.prune_min_savings),
            threshold: upper.threshold.or(self.threshold),
            request_threshold: upper.request_threshold.or(self.request_threshold),
            retained_tokens: upper.retained_tokens.or(self.retained_tokens),
            overview_target_tokens: upper.overview_target_tokens.or(self.overview_target_tokens),
            overview_warning_tokens: upper
                .overview_warning_tokens
                .or(self.overview_warning_tokens),
            overview_deadline_tokens: upper
                .overview_deadline_tokens
                .or(self.overview_deadline_tokens),
            worker_context_max_tokens: upper
                .worker_context_max_tokens
                .or(self.worker_context_max_tokens),
            finish_warning_remaining_tokens: upper
                .finish_warning_remaining_tokens
                .or(self.finish_warning_remaining_tokens),
            final_reserve_tokens: upper.final_reserve_tokens.or(self.final_reserve_tokens),
            worker_max_turns: upper.worker_max_turns.or(self.worker_max_turns),
            summary_target_tokens: upper.summary_target_tokens.or(self.summary_target_tokens),
            summary_max_tokens: upper.summary_max_tokens.or(self.summary_max_tokens),
            auto_read_budget_tokens: upper
                .auto_read_budget_tokens
                .or(self.auto_read_budget_tokens),
            result_context_max_tokens: upper
                .result_context_max_tokens
                .or(self.result_context_max_tokens),
            model: merge_option(self.model, upper.model, ModelManifest::merge),
        }
    }
}

fn merge_scope(mut lower: ScopeConfig, upper: ScopeConfig) -> ScopeConfig {
    lower.allow.extend(upper.allow);
    lower.deny.extend(upper.deny);
    lower
}

fn merge_option<T>(lower: Option<T>, upper: Option<T>, merge: fn(T, T) -> T) -> Option<T> {
    match (lower, upper) {
        (Some(l), Some(u)) => Some(merge(l, u)),
        (l, u) => u.or(l),
    }
}

fn join_if_relative(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Invariant check: every path in a fully-resolved [`PodManifestConfig`]
/// must be absolute. Relative paths are resolved per-layer via
/// [`PodManifestConfig::resolve_paths`]; if one reaches `TryFrom` it
/// indicates a caller skipped the per-layer resolve step.
fn ensure_absolute(field: &'static str, path: &Path) -> Result<(), ResolveError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ResolveError::RelativePath {
            field,
            path: path.to_path_buf(),
        })
    }
}

/// `AuthRef::ApiKey { file, .. }` が相対パスのとき `base` を前置する。
fn resolve_auth_file(auth: &mut Option<AuthRef>, base: &Path) {
    if let Some(AuthRef::ApiKey { file: Some(p), .. }) = auth.as_mut() {
        *p = join_if_relative(base, p);
    }
}

/// モデル宣言に含まれる `auth.file` が絶対パスであることを検証する。
/// ref / scheme / model_id 等の論理的な有効性（ref があるか、inline が
/// 揃っているか）の検証はカタログを知る `crates/provider` 側で行う。
fn validate_model_paths(model: &ModelManifest, field: &'static str) -> Result<(), ResolveError> {
    if let Some(AuthRef::ApiKey { file: Some(p), .. }) = &model.auth {
        ensure_absolute(field, p)?;
    }
    Ok(())
}

impl TryFrom<PodManifestConfig> for PodManifest {
    type Error = ResolveError;

    fn try_from(cfg: PodManifestConfig) -> Result<Self, Self::Error> {
        let name = cfg.pod.name.ok_or(ResolveError::MissingField("pod.name"))?;
        let prompt_pack = cfg.pod.prompt_pack;
        if let Some(ref p) = prompt_pack {
            ensure_absolute("pod.prompt_pack", p)?;
        }

        validate_model_paths(&cfg.model, "model.auth.file")?;

        let worker = WorkerManifest {
            instruction: cfg
                .worker
                .instruction
                .unwrap_or_else(|| defaults::DEFAULT_INSTRUCTION.to_string()),
            language: cfg
                .worker
                .language
                .unwrap_or_else(|| defaults::WORKER_LANGUAGE.to_string()),
            max_tokens: cfg.worker.max_tokens,
            max_turns: cfg.worker.max_turns,
            temperature: cfg.worker.temperature,
            top_p: cfg.worker.top_p,
            top_k: cfg.worker.top_k,
            stop_sequences: cfg.worker.stop_sequences.unwrap_or_default(),
            reasoning: cfg.worker.reasoning,
            tool_output: ToolOutputLimits {
                default_max_bytes: cfg
                    .worker
                    .tool_output
                    .default_max_bytes
                    .unwrap_or(defaults::TOOL_OUTPUT_MAX_BYTES),
                per_tool: cfg.worker.tool_output.per_tool,
            },
            file_upload: FileUploadLimits {
                max_bytes: cfg
                    .worker
                    .file_upload
                    .max_bytes
                    .unwrap_or(defaults::FILE_UPLOAD_MAX_BYTES),
            },
        };

        if cfg.scope.allow.is_empty() {
            return Err(ResolveError::MissingField("scope.allow"));
        }
        for rule in &cfg.scope.allow {
            ensure_absolute("scope.allow.target", &rule.target)?;
        }
        for rule in &cfg.scope.deny {
            ensure_absolute("scope.deny.target", &rule.target)?;
        }
        let session = SessionConfig {
            record_event_trace: cfg
                .session
                .and_then(|s| s.record_event_trace)
                .unwrap_or(false),
        };

        let permissions = cfg
            .permissions
            .map(|p| {
                Ok(ToolPermissionConfig {
                    default_action: p
                        .default_action
                        .ok_or(ResolveError::MissingField("permissions.default_action"))?,
                    rules: p.rules,
                })
            })
            .transpose()?;

        let compaction = cfg
            .compaction
            .map(|c| -> Result<CompactionConfig, ResolveError> {
                if let Some(ref cm) = c.model {
                    validate_model_paths(cm, "compaction.model.auth.file")?;
                }
                Ok(CompactionConfig {
                    prune_protected_tokens: c
                        .prune_protected_tokens
                        .unwrap_or(defaults::PRUNE_PROTECTED_TOKENS),
                    prune_min_savings: c.prune_min_savings.unwrap_or(defaults::PRUNE_MIN_SAVINGS),
                    threshold: c.threshold,
                    request_threshold: c.request_threshold,
                    retained_tokens: c
                        .retained_tokens
                        .unwrap_or(defaults::COMPACT_RETAINED_TOKENS),
                    overview_target_tokens: c
                        .overview_target_tokens
                        .unwrap_or(defaults::COMPACT_OVERVIEW_TARGET_TOKENS),
                    overview_warning_tokens: c
                        .overview_warning_tokens
                        .unwrap_or(defaults::COMPACT_OVERVIEW_WARNING_TOKENS),
                    overview_deadline_tokens: c
                        .overview_deadline_tokens
                        .unwrap_or(defaults::COMPACT_OVERVIEW_DEADLINE_TOKENS),
                    worker_context_max_tokens: c
                        .worker_context_max_tokens
                        .unwrap_or(defaults::COMPACT_WORKER_MAX_INPUT_TOKENS),
                    finish_warning_remaining_tokens: c
                        .finish_warning_remaining_tokens
                        .unwrap_or(defaults::COMPACT_FINISH_WARNING_REMAINING_TOKENS),
                    final_reserve_tokens: c
                        .final_reserve_tokens
                        .unwrap_or(defaults::COMPACT_FINAL_RESERVE_TOKENS),
                    worker_max_turns: c.worker_max_turns.or(defaults::COMPACT_WORKER_MAX_TURNS),
                    summary_target_tokens: c
                        .summary_target_tokens
                        .unwrap_or(defaults::COMPACT_SUMMARY_TARGET_TOKENS),
                    summary_max_tokens: c
                        .summary_max_tokens
                        .unwrap_or(defaults::COMPACT_SUMMARY_MAX_TOKENS),
                    auto_read_budget_tokens: c
                        .auto_read_budget_tokens
                        .unwrap_or(defaults::COMPACT_AUTO_READ_BUDGET),
                    result_context_max_tokens: c
                        .result_context_max_tokens
                        .unwrap_or(defaults::COMPACT_RESULT_CONTEXT_MAX_TOKENS),
                    model: c.model,
                })
            })
            .transpose()?;

        if let Some(ref skills) = cfg.skills {
            for dir in &skills.directories {
                ensure_absolute("skills.directories", dir)?;
            }
        }

        Ok(PodManifest {
            pod: PodMeta { name, prompt_pack },
            model: cfg.model,
            worker,
            scope: cfg.scope,
            session,
            permissions,
            compaction,
            memory: cfg.memory,
            skills: cfg.skills,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SchemeKind;
    use crate::{Permission, ReasoningEffort, ScopeRule};

    fn abs(path: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/insomnia-test{path}"))
    }

    fn api_key_file_auth(path: PathBuf) -> AuthRef {
        AuthRef::ApiKey {
            env: None,
            file: Some(path),
        }
    }

    fn minimal_valid() -> PodManifestConfig {
        PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("test".into()),
                prompt_pack: None,
            },
            model: ModelManifest {
                scheme: Some(SchemeKind::Anthropic),
                model_id: Some("claude-sonnet-4-20250514".into()),
                ..Default::default()
            },
            worker: WorkerManifestConfig::default(),
            scope: ScopeConfig {
                allow: vec![ScopeRule {
                    target: abs("/pod"),
                    permission: Permission::Write,
                    recursive: true,
                }],
                deny: Vec::new(),
            },
            permissions: None,
            session: None,
            compaction: None,
            memory: None,
            skills: None,
        }
    }

    #[test]
    fn resolve_minimal_succeeds() {
        let manifest: PodManifest = minimal_valid().try_into().unwrap();
        assert_eq!(manifest.pod.name, "test");
        assert_eq!(manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert!(manifest.permissions.is_none());
    }

    #[test]
    fn resolve_session_record_event_trace() {
        let mut cfg = minimal_valid();
        cfg.session = Some(SessionConfigPartial {
            record_event_trace: Some(true),
        });

        let manifest: PodManifest = cfg.try_into().unwrap();
        assert!(manifest.session.record_event_trace);
    }

    #[test]
    fn resolve_permissions_requires_default_action_when_present() {
        let mut cfg = minimal_valid();
        cfg.permissions = Some(PermissionConfigPartial {
            default_action: None,
            rules: Vec::new(),
        });

        let err = PodManifest::try_from(cfg).unwrap_err();

        assert!(matches!(
            err,
            ResolveError::MissingField("permissions.default_action")
        ));
    }

    #[test]
    fn resolve_permissions_preserves_actions_and_rule_order() {
        let mut cfg = minimal_valid();
        cfg.permissions = Some(PermissionConfigPartial {
            default_action: Some(crate::ToolPermissionAction::Ask),
            rules: vec![
                ToolPermissionRule {
                    tool: "Bash".into(),
                    pattern: "rm *".into(),
                    action: crate::ToolPermissionAction::Deny,
                },
                ToolPermissionRule {
                    tool: "Read".into(),
                    pattern: "*".into(),
                    action: crate::ToolPermissionAction::Allow,
                },
            ],
        });

        let manifest: PodManifest = cfg.try_into().unwrap();
        let permissions = manifest.permissions.unwrap();

        assert_eq!(permissions.default_action, crate::ToolPermissionAction::Ask);
        assert_eq!(permissions.rules.len(), 2);
        assert_eq!(permissions.rules[0].tool, "Bash");
        assert_eq!(permissions.rules[1].tool, "Read");
    }

    #[test]
    fn resolve_paths_joins_relative_auth_file() {
        let mut cfg = minimal_valid();
        cfg.model.auth = Some(api_key_file_auth(PathBuf::from("keys/anthropic")));
        let resolved = cfg.resolve_paths(Path::new("/home/user/.config/insomnia"));
        let file = match resolved.model.auth {
            Some(AuthRef::ApiKey { file, .. }) => file,
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(
            file.as_deref(),
            Some(Path::new("/home/user/.config/insomnia/keys/anthropic"))
        );
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_untouched() {
        let mut cfg = minimal_valid();
        cfg.model.auth = Some(api_key_file_auth(PathBuf::from("/etc/already/abs")));
        let resolved = cfg.resolve_paths(Path::new("/home/user"));
        let file = match resolved.model.auth {
            Some(AuthRef::ApiKey { file, .. }) => file,
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(file.as_deref(), Some(Path::new("/etc/already/abs")));
    }

    #[test]
    fn resolve_paths_joins_relative_scope_targets() {
        let mut cfg = minimal_valid();
        cfg.scope.allow[0].target = PathBuf::from(".");
        cfg.scope.deny.push(ScopeRule {
            target: PathBuf::from("secrets"),
            permission: Permission::Write,
            recursive: true,
        });
        let resolved = cfg.resolve_paths(Path::new("/workspace/proj"));
        assert_eq!(resolved.scope.allow[0].target, Path::new("/workspace/proj"));
        assert_eq!(
            resolved.scope.deny[0].target,
            Path::new("/workspace/proj/secrets")
        );
    }

    #[test]
    fn try_from_invariant_rejects_lingering_relative_auth_file() {
        let mut cfg = minimal_valid();
        cfg.model.auth = Some(api_key_file_auth(PathBuf::from("keys/relative")));
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::RelativePath {
                field: "model.auth.file",
                ..
            }
        ));
    }

    #[test]
    fn try_from_invariant_rejects_lingering_relative_scope_target() {
        let mut cfg = minimal_valid();
        cfg.scope.allow[0].target = PathBuf::from("docs");
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::RelativePath {
                field: "scope.allow.target",
                ..
            }
        ));
    }

    #[test]
    fn resolve_rejects_missing_pod_name() {
        let mut cfg = minimal_valid();
        cfg.pod.name = None;
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(err, ResolveError::MissingField("pod.name")));
    }

    #[test]
    fn resolve_rejects_empty_scope() {
        let mut cfg = minimal_valid();
        cfg.scope.allow.clear();
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(err, ResolveError::MissingField("scope.allow")));
    }

    #[test]
    fn merge_scalar_upper_wins() {
        let lower = PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("lower".into()),
                prompt_pack: None,
            },
            model: ModelManifest {
                model_id: Some("lower-model".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("upper".into()),
                prompt_pack: None,
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(merged.pod.name.as_deref(), Some("upper"));
        // model_id not present in upper — retain lower
        assert_eq!(merged.model.model_id.as_deref(), Some("lower-model"));
    }

    #[test]
    fn merge_worker_reasoning_upper_wins() {
        let lower = PodManifestConfig {
            worker: WorkerManifestConfig {
                reasoning: Some(ReasoningControl::Effort(ReasoningEffort::Low)),
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            worker: WorkerManifestConfig {
                reasoning: Some(ReasoningControl::BudgetTokens(4096)),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = lower.merge(upper);

        assert_eq!(
            merged.worker.reasoning,
            Some(ReasoningControl::BudgetTokens(4096))
        );
    }

    #[test]
    fn merge_worker_generation_settings_upper_wins() {
        let lower = PodManifestConfig {
            worker: WorkerManifestConfig {
                top_p: Some(0.8),
                top_k: Some(20),
                stop_sequences: Some(vec!["lower".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            worker: WorkerManifestConfig {
                top_p: Some(0.9),
                stop_sequences: Some(vec!["upper".into()]),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = lower.merge(upper);

        assert_eq!(merged.worker.top_p, Some(0.9));
        assert_eq!(merged.worker.top_k, Some(20));
        assert_eq!(merged.worker.stop_sequences, Some(vec!["upper".into()]));
    }

    #[test]
    fn merge_scope_accumulates_allow_and_deny() {
        let lower = PodManifestConfig {
            scope: ScopeConfig {
                allow: vec![ScopeRule {
                    target: abs("/a"),
                    permission: Permission::Read,
                    recursive: true,
                }],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            scope: ScopeConfig {
                allow: vec![ScopeRule {
                    target: abs("/b"),
                    permission: Permission::Write,
                    recursive: true,
                }],
                deny: vec![ScopeRule {
                    target: abs("/a/secret"),
                    permission: Permission::Read,
                    recursive: false,
                }],
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(merged.scope.allow.len(), 2);
        assert_eq!(merged.scope.deny.len(), 1);
    }

    #[test]
    fn merge_permissions_accumulates_rules_and_upper_default_wins() {
        let lower = PodManifestConfig {
            permissions: Some(PermissionConfigPartial {
                default_action: Some(crate::ToolPermissionAction::Allow),
                rules: vec![ToolPermissionRule {
                    tool: "Bash".into(),
                    pattern: "git *".into(),
                    action: crate::ToolPermissionAction::Allow,
                }],
            }),
            ..Default::default()
        };
        let upper = PodManifestConfig {
            permissions: Some(PermissionConfigPartial {
                default_action: Some(crate::ToolPermissionAction::Deny),
                rules: vec![ToolPermissionRule {
                    tool: "Bash".into(),
                    pattern: "rm *".into(),
                    action: crate::ToolPermissionAction::Deny,
                }],
            }),
            ..Default::default()
        };

        let merged = lower.merge(upper).permissions.unwrap();

        assert_eq!(
            merged.default_action,
            Some(crate::ToolPermissionAction::Deny)
        );
        assert_eq!(merged.rules.len(), 2);
        assert_eq!(merged.rules[0].pattern, "git *");
        assert_eq!(merged.rules[1].pattern, "rm *");
    }

    #[test]
    fn merge_tool_output_per_tool_keywise() {
        let lower = PodManifestConfig {
            worker: WorkerManifestConfig {
                tool_output: ToolOutputLimitsPartial {
                    default_max_bytes: Some(8192),
                    per_tool: [("Read".to_string(), 1024)].into_iter().collect(),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            worker: WorkerManifestConfig {
                tool_output: ToolOutputLimitsPartial {
                    default_max_bytes: None,
                    per_tool: [("Read".to_string(), 2048), ("Grep".to_string(), 512)]
                        .into_iter()
                        .collect(),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        let to = &merged.worker.tool_output;
        assert_eq!(to.default_max_bytes, Some(8192));
        assert_eq!(to.per_tool.get("Read"), Some(&2048));
        assert_eq!(to.per_tool.get("Grep"), Some(&512));
    }

    #[test]
    fn merge_file_upload_max_bytes_upper_wins() {
        let lower = PodManifestConfig {
            worker: WorkerManifestConfig {
                file_upload: FileUploadLimitsPartial {
                    max_bytes: Some(8192),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            worker: WorkerManifestConfig {
                file_upload: FileUploadLimitsPartial {
                    max_bytes: Some(54_321),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(merged.worker.file_upload.max_bytes, Some(54_321));
    }
    #[test]
    fn merge_option_struct_field_wise() {
        let lower = PodManifestConfig {
            compaction: Some(CompactionConfigPartial {
                threshold: Some(50_000),
                prune_protected_tokens: Some(5_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let upper = PodManifestConfig {
            compaction: Some(CompactionConfigPartial {
                threshold: Some(80_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = lower.merge(upper);
        let c = merged.compaction.unwrap();
        assert_eq!(c.threshold, Some(80_000));
        // field from lower retained when upper has None
        assert_eq!(c.prune_protected_tokens, Some(5_000));
    }

    #[test]
    fn from_toml_type_mismatch_is_hard_error() {
        let bad = r#"
[pod]
name = "x"

[worker]
max_tokens = "not-a-number"
"#;
        assert!(PodManifestConfig::from_toml(bad).is_err());
    }

    #[test]
    fn from_toml_accepts_unknown_field() {
        // Unknown keys are warn-and-ignored, not hard errors.
        // `pod.pwd` specifically is silently dropped after the
        // path-resolution ticket — keep it in the fixture to exercise
        // that code path.
        let ok = r#"
[pod]
name = "x"
pwd = "/obsolete"

[worker]
max_tokens = 1000
unknown_future_field = "tolerated"
"#;
        let cfg = PodManifestConfig::from_toml(ok).unwrap();
        assert_eq!(cfg.worker.max_tokens, Some(1000));
    }

    #[test]
    fn from_toml_rejects_removed_prune_protected_turns_field() {
        let bad = r#"
[compaction]
prune_protected_turns = 3
"#;
        let err = PodManifestConfig::from_toml(bad).unwrap_err();
        assert!(
            err.to_string().contains("compaction.prune_protected_turns"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_toml_rejects_removed_extract_worker_max_input_tokens_field() {
        let bad = r#"
[memory]
extract_worker_max_input_tokens = 30000
"#;
        let err = PodManifestConfig::from_toml(bad).unwrap_err();
        assert!(
            err.to_string()
                .contains("memory.extract_worker_max_input_tokens"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_toml_accepts_extract_worker_max_turns() {
        let cfg = PodManifestConfig::from_toml(
            r#"
[memory]
extract_worker_max_turns = 2
"#,
        )
        .unwrap();
        assert_eq!(cfg.memory.unwrap().extract_worker_max_turns, Some(2));
    }

    #[test]
    fn from_toml_accepts_worker_reasoning_string_or_integer() {
        let effort = PodManifestConfig::from_toml(
            r#"
[worker]
reasoning = "xhigh"
"#,
        )
        .unwrap();
        assert_eq!(
            effort.worker.reasoning,
            Some(ReasoningControl::Effort(ReasoningEffort::XHigh))
        );

        let budget = PodManifestConfig::from_toml(
            r#"
[worker]
reasoning = -1
"#,
        )
        .unwrap();
        assert_eq!(
            budget.worker.reasoning,
            Some(ReasoningControl::BudgetTokens(-1))
        );
    }

    #[test]
    fn from_toml_accepts_worker_generation_settings() {
        let cfg = PodManifestConfig::from_toml(
            r#"
[worker]
top_p = 0.9
top_k = 40
stop_sequences = ["\n\n", "</stop>"]
"#,
        )
        .unwrap();

        assert_eq!(cfg.worker.top_p, Some(0.9));
        assert_eq!(cfg.worker.top_k, Some(40));
        assert_eq!(
            cfg.worker.stop_sequences,
            Some(vec!["\n\n".into(), "</stop>".into()])
        );
    }

    #[test]
    fn from_toml_accepts_worker_max_turns() {
        let cfg = PodManifestConfig::from_toml(
            r#"
[compaction]
worker_max_turns = 7
"#,
        )
        .unwrap();

        assert_eq!(cfg.compaction.unwrap().worker_max_turns, Some(7));
    }

    #[test]
    fn try_from_compaction_defaults_worker_max_turns() {
        let mut cfg = minimal_valid();
        cfg.compaction = Some(CompactionConfigPartial::default());

        let manifest = PodManifest::try_from(cfg).unwrap();

        assert_eq!(
            manifest.compaction.unwrap().worker_max_turns,
            defaults::COMPACT_WORKER_MAX_TURNS
        );
    }

    #[test]
    fn from_toml_partial_layer_succeeds() {
        // A project-layer manifest with only scope set must parse fine.
        let toml = r#"
[[scope.allow]]
target = "/abs/project"
permission = "write"
"#;
        let cfg = PodManifestConfig::from_toml(toml).unwrap();
        assert!(cfg.pod.name.is_none());
        assert_eq!(cfg.scope.allow.len(), 1);
    }

    #[test]
    fn builtin_defaults_populates_worker_limit_defaults() {
        let cfg = PodManifestConfig::builtin_defaults();
        assert_eq!(
            cfg.worker.tool_output.default_max_bytes,
            Some(defaults::TOOL_OUTPUT_MAX_BYTES)
        );
        assert_eq!(
            cfg.worker.file_upload.max_bytes,
            Some(defaults::FILE_UPLOAD_MAX_BYTES)
        );
    }

    #[test]
    fn builtin_defaults_merged_into_minimal_resolves_with_defaults() {
        // Starting from builtin_defaults and overlaying only the
        // required fields must resolve to a PodManifest carrying the
        // centralised default values.
        let overlay = PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("x".into()),
                prompt_pack: None,
            },
            model: ModelManifest {
                scheme: Some(SchemeKind::Anthropic),
                model_id: Some("m".into()),
                ..Default::default()
            },
            scope: ScopeConfig {
                allow: vec![ScopeRule {
                    target: abs("/pod"),
                    permission: Permission::Write,
                    recursive: true,
                }],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let merged = PodManifestConfig::builtin_defaults().merge(overlay);
        let manifest: PodManifest = merged.try_into().unwrap();
        assert_eq!(
            manifest.worker.tool_output.default_max_bytes,
            defaults::TOOL_OUTPUT_MAX_BYTES
        );
        assert_eq!(
            manifest.worker.file_upload.max_bytes,
            defaults::FILE_UPLOAD_MAX_BYTES
        );
    }

    #[test]
    fn end_to_end_cascade() {
        let builtin = PodManifestConfig::default();
        let user = PodManifestConfig::from_toml(
            r#"
[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"
"#,
        )
        .unwrap();
        let project = PodManifestConfig::from_toml(
            r#"
[[scope.allow]]
target = "/abs/project"
permission = "write"
"#,
        )
        .unwrap();
        let overlay = PodManifestConfig::from_toml(
            r#"
[pod]
name = "dbg"
"#,
        )
        .unwrap();

        let merged = builtin.merge(user).merge(project).merge(overlay);
        let manifest: PodManifest = merged.try_into().unwrap();
        assert_eq!(manifest.pod.name, "dbg");
        assert_eq!(manifest.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(manifest.scope.allow.len(), 1);
    }

    #[test]
    fn skills_directories_resolved_against_base() {
        let mut cfg = minimal_valid();
        cfg.skills = Some(SkillsConfig {
            directories: vec![
                PathBuf::from(".claude/skills"),
                PathBuf::from("/abs/elsewhere"),
            ],
        });
        let resolved = cfg.resolve_paths(Path::new("/workspace/proj"));
        let dirs = resolved.skills.as_ref().unwrap().directories.clone();
        assert_eq!(dirs[0], PathBuf::from("/workspace/proj/.claude/skills"));
        assert_eq!(dirs[1], PathBuf::from("/abs/elsewhere"));
    }

    #[test]
    fn skills_relative_path_rejected_post_resolve() {
        let mut cfg = minimal_valid();
        cfg.skills = Some(SkillsConfig {
            directories: vec![PathBuf::from("relative/skills")],
        });
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::RelativePath {
                field: "skills.directories",
                ..
            }
        ));
    }

    #[test]
    fn skills_merge_extends_directories() {
        let lower = PodManifestConfig {
            skills: Some(SkillsConfig {
                directories: vec![PathBuf::from("/a")],
            }),
            ..Default::default()
        };
        let upper = PodManifestConfig {
            skills: Some(SkillsConfig {
                directories: vec![PathBuf::from("/b")],
            }),
            ..Default::default()
        };
        let merged = lower.merge(upper);
        let dirs = merged.skills.unwrap().directories;
        assert_eq!(dirs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn from_toml_parses_skills_section() {
        let toml = r#"
[pod]
name = "x"

[skills]
directories = [".claude/skills", ".cursor/skills"]
"#;
        let cfg = PodManifestConfig::from_toml(toml).unwrap();
        let dirs = cfg.skills.unwrap().directories;
        assert_eq!(
            dirs,
            vec![
                PathBuf::from(".claude/skills"),
                PathBuf::from(".cursor/skills"),
            ]
        );
    }

    #[test]
    fn merge_preserves_ref() {
        let lower = PodManifestConfig {
            model: ModelManifest {
                ref_: Some("anthropic/claude-sonnet-4-6".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            model: ModelManifest {
                // only override auth
                auth: Some(AuthRef::None),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(
            merged.model.ref_.as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert_eq!(merged.model.auth, Some(AuthRef::None));
    }
}
