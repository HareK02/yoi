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

use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::{
    CompactionConfig, PodManifest, PodMeta, ProviderConfig, ProviderKind, ScopeConfig,
    ToolOutputLimits, WorkerManifest,
};

/// Partial-form Pod manifest. Every field is optional; one or more
/// instances merge via [`PodManifestConfig::merge`] before being
/// converted to a validated [`PodManifest`] via `TryFrom`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodManifestConfig {
    #[serde(default)]
    pub pod: PodMetaConfig,
    #[serde(default)]
    pub provider: ProviderConfigPartial,
    #[serde(default)]
    pub worker: WorkerManifestConfig,
    #[serde(default)]
    pub scope: ScopeConfig,
    #[serde(default)]
    pub compaction: Option<CompactionConfigPartial>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodMetaConfig {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfigPartial {
    #[serde(default)]
    pub kind: Option<ProviderKind>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerManifestConfig {
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<NonZeroU32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub tool_output: ToolOutputLimitsPartial,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutputLimitsPartial {
    #[serde(default)]
    pub default_max_bytes: Option<usize>,
    #[serde(default)]
    pub per_tool: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionConfigPartial {
    #[serde(default)]
    pub prune_protected_turns: Option<usize>,
    #[serde(default)]
    pub prune_min_savings: Option<u64>,
    #[serde(default)]
    pub compact_threshold: Option<u64>,
    #[serde(default)]
    pub compact_retained_turns: Option<usize>,
    #[serde(default)]
    pub provider: Option<ProviderConfigPartial>,
}

/// Errors raised when converting a [`PodManifestConfig`] to a validated
/// [`PodManifest`] via `TryFrom`.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("path must be absolute ({field}): {}", .path.display())]
    RelativePath {
        field: &'static str,
        path: PathBuf,
    },
}

impl PodManifestConfig {
    /// Parse a partial manifest from a TOML string. Unknown top-level or
    /// nested fields emit a `tracing::warn!` and are ignored; use
    /// `tracing_subscriber` with `WARN` enabled to surface them to the
    /// operator.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
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
    /// Affected fields: `provider.api_key_file`,
    /// `scope.allow[].target`, `scope.deny[].target`,
    /// `compaction.provider.api_key_file`.
    pub fn resolve_paths(mut self, base: &Path) -> Self {
        debug_assert!(
            base.is_absolute(),
            "resolve_paths base must be absolute: {}",
            base.display()
        );
        if let Some(ref mut p) = self.provider.api_key_file {
            *p = join_if_relative(base, p);
        }
        for rule in &mut self.scope.allow {
            rule.target = join_if_relative(base, &rule.target);
        }
        for rule in &mut self.scope.deny {
            rule.target = join_if_relative(base, &rule.target);
        }
        if let Some(ref mut compaction) = self.compaction
            && let Some(ref mut cp) = compaction.provider
            && let Some(ref mut p) = cp.api_key_file
        {
            *p = join_if_relative(base, p);
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
            provider: self.provider.merge(upper.provider),
            worker: self.worker.merge(upper.worker),
            scope: merge_scope(self.scope, upper.scope),
            compaction: merge_option(
                self.compaction,
                upper.compaction,
                CompactionConfigPartial::merge,
            ),
        }
    }
}

impl PodMetaConfig {
    fn merge(self, upper: Self) -> Self {
        Self {
            name: upper.name.or(self.name),
        }
    }
}

impl ProviderConfigPartial {
    fn merge(self, upper: Self) -> Self {
        Self {
            kind: upper.kind.or(self.kind),
            model: upper.model.or(self.model),
            api_key_file: upper.api_key_file.or(self.api_key_file),
            base_url: upper.base_url.or(self.base_url),
        }
    }
}

impl WorkerManifestConfig {
    fn merge(self, upper: Self) -> Self {
        Self {
            instruction: upper.instruction.or(self.instruction),
            max_tokens: upper.max_tokens.or(self.max_tokens),
            max_turns: upper.max_turns.or(self.max_turns),
            temperature: upper.temperature.or(self.temperature),
            tool_output: self.tool_output.merge(upper.tool_output),
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

impl CompactionConfigPartial {
    fn merge(self, upper: Self) -> Self {
        Self {
            prune_protected_turns: upper.prune_protected_turns.or(self.prune_protected_turns),
            prune_min_savings: upper.prune_min_savings.or(self.prune_min_savings),
            compact_threshold: upper.compact_threshold.or(self.compact_threshold),
            compact_retained_turns: upper
                .compact_retained_turns
                .or(self.compact_retained_turns),
            provider: merge_option(self.provider, upper.provider, ProviderConfigPartial::merge),
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

fn resolve_provider(
    cfg: ProviderConfigPartial,
    kind_field: &'static str,
    model_field: &'static str,
    api_key_field: &'static str,
) -> Result<ProviderConfig, ResolveError> {
    let kind = cfg.kind.ok_or(ResolveError::MissingField(kind_field))?;
    let model = cfg.model.ok_or(ResolveError::MissingField(model_field))?;
    if let Some(ref p) = cfg.api_key_file {
        ensure_absolute(api_key_field, p)?;
    }
    Ok(ProviderConfig {
        kind,
        model,
        api_key_file: cfg.api_key_file,
        base_url: cfg.base_url,
    })
}

impl TryFrom<PodManifestConfig> for PodManifest {
    type Error = ResolveError;

    fn try_from(cfg: PodManifestConfig) -> Result<Self, Self::Error> {
        let name = cfg
            .pod
            .name
            .ok_or(ResolveError::MissingField("pod.name"))?;

        let provider = resolve_provider(
            cfg.provider,
            "provider.kind",
            "provider.model",
            "provider.api_key_file",
        )?;

        let worker = WorkerManifest {
            instruction: cfg
                .worker
                .instruction
                .unwrap_or_else(|| defaults::DEFAULT_INSTRUCTION.to_string()),
            max_tokens: cfg.worker.max_tokens,
            max_turns: cfg.worker.max_turns,
            temperature: cfg.worker.temperature,
            tool_output: ToolOutputLimits {
                default_max_bytes: cfg
                    .worker
                    .tool_output
                    .default_max_bytes
                    .unwrap_or(defaults::TOOL_OUTPUT_MAX_BYTES),
                per_tool: cfg.worker.tool_output.per_tool,
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

        let compaction = cfg
            .compaction
            .map(|c| -> Result<CompactionConfig, ResolveError> {
                let comp_provider = c
                    .provider
                    .map(|p| {
                        resolve_provider(
                            p,
                            "compaction.provider.kind",
                            "compaction.provider.model",
                            "compaction.provider.api_key_file",
                        )
                    })
                    .transpose()?;
                Ok(CompactionConfig {
                    prune_protected_turns: c
                        .prune_protected_turns
                        .unwrap_or(defaults::PRUNE_PROTECTED_TURNS),
                    prune_min_savings: c
                        .prune_min_savings
                        .unwrap_or(defaults::PRUNE_MIN_SAVINGS),
                    compact_threshold: c.compact_threshold,
                    compact_retained_turns: c
                        .compact_retained_turns
                        .unwrap_or(defaults::COMPACT_RETAINED_TURNS),
                    provider: comp_provider,
                })
            })
            .transpose()?;

        Ok(PodManifest {
            pod: PodMeta { name },
            provider,
            worker,
            scope: cfg.scope,
            compaction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Permission, ScopeRule};

    fn abs(path: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/insomnia-test{path}"))
    }

    fn minimal_valid() -> PodManifestConfig {
        PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("test".into()),
            },
            provider: ProviderConfigPartial {
                kind: Some(ProviderKind::Anthropic),
                model: Some("claude-sonnet-4-20250514".into()),
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
            compaction: None,
        }
    }

    #[test]
    fn resolve_minimal_succeeds() {
        let manifest: PodManifest = minimal_valid().try_into().unwrap();
        assert_eq!(manifest.pod.name, "test");
        assert_eq!(manifest.provider.kind, ProviderKind::Anthropic);
    }

    #[test]
    fn resolve_paths_joins_relative_api_key_file() {
        let mut cfg = minimal_valid();
        cfg.provider.api_key_file = Some(PathBuf::from("keys/anthropic"));
        let resolved = cfg.resolve_paths(Path::new("/home/user/.config/insomnia"));
        assert_eq!(
            resolved.provider.api_key_file.as_deref(),
            Some(Path::new("/home/user/.config/insomnia/keys/anthropic"))
        );
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_untouched() {
        let mut cfg = minimal_valid();
        cfg.provider.api_key_file = Some(PathBuf::from("/etc/already/abs"));
        let resolved = cfg.resolve_paths(Path::new("/home/user"));
        assert_eq!(
            resolved.provider.api_key_file.as_deref(),
            Some(Path::new("/etc/already/abs"))
        );
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
    fn try_from_invariant_rejects_lingering_relative_api_key_file() {
        let mut cfg = minimal_valid();
        cfg.provider.api_key_file = Some(PathBuf::from("keys/relative"));
        // Skipping resolve_paths on purpose: TryFrom must catch the
        // invariant violation.
        let err = PodManifest::try_from(cfg).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::RelativePath {
                field: "provider.api_key_file",
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
            },
            provider: ProviderConfigPartial {
                model: Some("lower-model".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let upper = PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("upper".into()),
            },
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(merged.pod.name.as_deref(), Some("upper"));
        // model not present in upper — retain lower
        assert_eq!(merged.provider.model.as_deref(), Some("lower-model"));
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
                    per_tool: [
                        ("Read".to_string(), 2048),
                        ("Grep".to_string(), 512),
                    ]
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
    fn merge_option_struct_field_wise() {
        let lower = PodManifestConfig {
            compaction: Some(CompactionConfigPartial {
                compact_threshold: Some(50_000),
                prune_protected_turns: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let upper = PodManifestConfig {
            compaction: Some(CompactionConfigPartial {
                compact_threshold: Some(80_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = lower.merge(upper);
        let c = merged.compaction.unwrap();
        assert_eq!(c.compact_threshold, Some(80_000));
        // field from lower retained when upper has None
        assert_eq!(c.prune_protected_turns, Some(5));
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
    fn builtin_defaults_populates_tool_output_max_bytes() {
        let cfg = PodManifestConfig::builtin_defaults();
        assert_eq!(
            cfg.worker.tool_output.default_max_bytes,
            Some(defaults::TOOL_OUTPUT_MAX_BYTES)
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
            },
            provider: ProviderConfigPartial {
                kind: Some(ProviderKind::Anthropic),
                model: Some("m".into()),
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
    }

    #[test]
    fn end_to_end_cascade() {
        let builtin = PodManifestConfig::default();
        let user = PodManifestConfig::from_toml(
            r#"
[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"
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
        assert_eq!(manifest.provider.kind, ProviderKind::Anthropic);
        assert_eq!(manifest.scope.allow.len(), 1);
    }
}
