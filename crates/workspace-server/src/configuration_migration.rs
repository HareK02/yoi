//! Read-only preparation of a Workspace permission migration draft.
//!
//! Native exports supply Subject policies; configuration supplies only shared
//! sets and the human Worker default. Preview neither saves the draft nor seals
//! the legacy writer. Adoption must recheck source, head and export identity.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{
    ConfigContentType, ConfigTreeChange, EvaluationResult, SnapshotEnvironment, ToolchainContract,
    VirtualPath, WorkspaceConfigSchemaBundle,
};
use feature_permissions::{NamespacePermissions, PermissionHost};
use subjektiv::{PermissionTransferSnapshot, SubjektivStore};

use crate::config_source::{
    EvaluatedConfigCandidate, MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigState,
    validate_workspace_config_candidate_projections,
};
use crate::policy_config::{
    self, AdoptedSubjectPolicy, AssignmentSource, MigrationDiagnostic, PermissionSource,
};
use crate::{Error, Result, SqliteWorkspaceStore};

pub(crate) struct MigrationSeed {
    pub original: WorkspaceConfigState,
    pub state: WorkspaceConfigState,
    pub snapshot: PermissionTransferSnapshot,
    pub policies: BTreeMap<String, AdoptedSubjectPolicy>,
    pub diagnostics: Vec<MigrationDiagnostic>,
}

pub(crate) fn preview_migration(
    store: &SqliteWorkspaceStore,
    legacy: &SubjektivStore,
    workspace: &str,
    schema: WorkspaceConfigSchemaBundle,
    host: &PermissionHost,
) -> Result<MigrationSeed> {
    if legacy.scope_id() != workspace || host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    let identity = legacy.database_identity().map_err(legacy_error)?;
    if identity.scope_id != workspace || identity.feature_id != subjektiv::SUBJEKTIV_FEATURE_ID {
        return Err(Error::WorkspaceIdMismatch);
    }
    let original = store.with_conn(|conn| {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND state='active')",
            [workspace],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::WorkspaceIdMismatch);
        }
        crate::config_source::load_state(conn, workspace)?.ok_or_else(|| {
            Error::RegistryInconsistency("workspace config tree is not materialized".into())
        })
    })?;
    let snapshot = legacy
        .preview_permission_transfer(&identity)
        .map_err(legacy_error)?;
    if snapshot.identity != identity {
        return Err(Error::WorkspaceConfigConflict(
            "legacy export identity changed".into(),
        ));
    }
    let before = SnapshotEnvironment::new(original.snapshot.clone())
        .evaluate_contract(&original.contract)
        .map_err(|error| {
            Error::InvalidInput(format!(
                "cannot inspect current migration source: {error:?}"
            ))
        })?;
    // Explicit old individual source is never an instruction to grant or revoke.
    // Until source-range removal is available, block instead of publishing a copy
    // of it in a new active wrapper. Schema-injected empty defaults are not source.
    let mut source = shared_source_for_migration(&original, &before)?;
    let references_changed = policy_config::migrate_source_references(&mut source);
    let changes = if references_changed {
        reference_migration_changes(&original, &source)?
    } else {
        vec![]
    };
    let candidate = store.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        evaluate_migration_candidate(&tx, workspace, &original, &changes, schema)
    })?;
    crate::config_adoption::semantic_references::prove_reference_only(
        &before,
        &candidate.evaluation,
    )?;
    validate_workspace_config_candidate_projections(workspace, &candidate)?;
    let evaluated_source = policy_config::source(&candidate.evaluation)?;
    if evaluated_source != source {
        return Err(Error::InvalidInput(
            "migration changed shared permission configuration beyond the exact mapping".into(),
        ));
    }
    let configured = policy_config::configured_host(host, &evaluated_source)?;
    let (policies, diagnostics) = migrate_policies(&configured, &snapshot)?;
    let state = WorkspaceConfigState {
        snapshot: candidate.snapshot,
        contract: candidate.contract,
        projection_digest: candidate.evaluation.projection_digest,
    };
    Ok(MigrationSeed {
        original,
        state,
        snapshot,
        policies,
        diagnostics,
    })
}

fn legacy_error(error: subjektiv::SubjektivError) -> Error {
    Error::Store(format!("legacy permission migration preview: {error}"))
}

fn path(value: &str) -> Result<VirtualPath> {
    VirtualPath::parse(value).map_err(|error| Error::InvalidInput(error.to_string()))
}

/// The stored toolchain, never a failed current decode, selects legacy decoding.
pub(crate) fn shared_source_for_migration(
    current: &WorkspaceConfigState,
    before: &EvaluationResult,
) -> Result<PermissionSource> {
    let old_schema = current
        .contract
        .schema_bundle
        .contributions
        .iter()
        .any(|provider| {
            provider.provider_id == "builtin:permissions"
                && provider
                    .version
                    .parse::<u32>()
                    .is_ok_and(|version| version <= 3)
        });
    let source = if old_schema {
        let old = policy_config::legacy_source(before)?;
        if !old.remove_subjects.is_empty() {
            return Err(Error::WorkspaceConfigConflict(
                "legacy remove_subjects requires a durable Subject-domain removal fence before source migration; no policy was recreated".into(),
            ));
        }
        if !old.subjects.is_empty() {
            return Err(individual_source_conflict());
        }
        ensure_no_individual_source(current)?;
        old.shared_source()
    } else {
        policy_config::source(before)?
    };
    Ok(source)
}

fn individual_source_conflict() -> Error {
    Error::WorkspaceConfigConflict(
        "legacy individual Subject source requires typed source-range removal; configuration bytes and current domain policies were not changed".into(),
    )
}

fn ensure_no_individual_source(current: &WorkspaceConfigState) -> Result<()> {
    let open = ToolchainContract::with_schema_bundle(
        current.contract.schema_version,
        current.contract.entrypoints.clone(),
        current.contract.import_policy_version,
        WorkspaceConfigSchemaBundle::empty(),
    );
    let raw = SnapshotEnvironment::new(current.snapshot.clone())
        .evaluate_contract(&open)
        .map_err(|error| Error::WorkspaceConfigConflict(format!(
            "cannot prove absence of individual Subject source without schema defaults: {error:?}; source was not changed"
        )))?;
    if raw.projections.iter().any(|projection| {
        projection
            .data_json
            .get("permissions")
            .is_some_and(|permissions| {
                permissions.get("subjects").is_some()
                    || permissions.get("remove_subjects").is_some()
            })
    }) {
        return Err(individual_source_conflict());
    }
    Ok(())
}

/// Reevaluate unchanged authored expressions without passing through the owner
/// writer's object normalizer. Changed wrappers use normal candidate validation.
pub(crate) fn evaluate_migration_candidate(
    tx: &rusqlite::Transaction<'_>,
    workspace: &str,
    original: &WorkspaceConfigState,
    changes: &[ConfigTreeChange],
    schema: WorkspaceConfigSchemaBundle,
) -> Result<EvaluatedConfigCandidate> {
    let permissions = schema
        .contributions
        .into_iter()
        .find(|provider| provider.provider_id == "builtin:permissions" && provider.version == "4")
        .ok_or_else(|| Error::WorkspaceConfigConflict(
            "permission migration requires the current shared-only schema; legacy frozen targets require explicit recovery".into(),
        ))?;
    // A permission representation change must not adopt unrelated live defaults
    // (for example a rebuilt bundled prompt) into the saved configuration.
    let providers = original
        .contract
        .schema_bundle
        .contributions
        .iter()
        .filter(|provider| provider.provider_id != "builtin:permissions")
        .cloned()
        .chain(std::iter::once(permissions));
    let schema = WorkspaceConfigSchemaBundle::compose(providers)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    let base_identity = crate::config_source::load_evaluation_identity(tx, workspace)?;
    if !changes.is_empty() {
        let staged = WorkspaceConfigState {
            snapshot: original
                .snapshot
                .apply(changes)
                .map_err(|error| Error::InvalidInput(error.to_string()))?,
            ..original.clone()
        };
        return crate::config_source::evaluate_candidate_in_transaction(
            tx,
            workspace,
            staged,
            base_identity,
            &[],
            schema,
        );
    }
    let (contract, evaluation) = if schema
        .contributions
        .iter()
        .any(|provider| provider.provider_id == "builtin:plugins")
    {
        let (contract, evaluation, _) = crate::plugin_settings::evaluate_catalog_config(
            tx,
            workspace,
            &original.snapshot,
            schema,
        )?;
        (contract, evaluation)
    } else {
        let contract = ToolchainContract::with_schema_bundle(
            original.contract.schema_version,
            original.contract.entrypoints.clone(),
            original.contract.import_policy_version,
            schema,
        );
        let evaluation = SnapshotEnvironment::new(original.snapshot.clone())
            .evaluate_contract(&contract)
            .map_err(|error| {
                Error::InvalidInput(format!("cannot reevaluate migration source: {error:?}"))
            })?;
        (contract, evaluation)
    };
    Ok(EvaluatedConfigCandidate {
        base_digest: original.snapshot.digest.clone(),
        base_toolchain_fingerprint: base_identity.0,
        base_projection_digest: base_identity.1,
        snapshot: original.snapshot.clone(),
        contract,
        evaluation,
    })
}

fn migrate_policies(
    host: &PermissionHost,
    snapshot: &PermissionTransferSnapshot,
) -> Result<(
    BTreeMap<String, AdoptedSubjectPolicy>,
    Vec<MigrationDiagnostic>,
)> {
    let mut policies = BTreeMap::new();
    let mut diagnostics = vec![];
    let mut seen = BTreeSet::new();
    for item in &snapshot.sources {
        if !seen.insert(&item.subject_id) {
            return Err(Error::RegistryInconsistency(
                "duplicate Subject in legacy export".into(),
            ));
        }
        if let Some(current) = &item.feature_policy {
            if item.permissions.is_some() {
                return Err(Error::RegistryInconsistency(
                    "mixed policy representations".into(),
                ));
            }
            let policy = AdoptedSubjectPolicy::from_current(&item.subject_id, current);
            policy_config::validate_assignment(host, &policy.source.execution)?;
            policy_config::validate_assignment(host, &policy.source.delegation)?;
            policies.insert(item.subject_id.clone(), policy);
            continue;
        }
        // Unset stays unset, including on retries. Creation defaults are not migration.
        let Some(legacy) = &item.permissions else {
            continue;
        };
        let (policy, mut converted) =
            policy_config::migrate_subject(host, &item.subject_id, legacy)?;
        policies.insert(item.subject_id.clone(), policy);
        diagnostics.append(&mut converted);
    }
    Ok((policies, diagnostics))
}

/// Only shared references may be rendered. Original individual records must stay
/// in content history, not in any newly published active source file.
pub(crate) fn reference_migration_changes(
    current: &WorkspaceConfigState,
    source: &PermissionSource,
) -> Result<Vec<ConfigTreeChange>> {
    ensure_no_individual_source(current)?;
    // Evaluation alone cannot prove that a dead branch, imported array or unused
    // binding contains no individual records. Without an AST removal API, a
    // conservative textual guard may reject harmless comments, but must never
    // copy potentially individual source into a newly active wrapper.
    if current.snapshot.entries.values().any(|entry| {
        ["subject_id", "subjects", "remove_subjects"]
            .iter()
            .any(|field| entry.content.contains(field))
    }) {
        return Err(individual_source_conflict());
    }
    let main_path = path(MAIN_CONFIG_ENTRYPOINT)?;
    let main = current.snapshot.get(&main_path).ok_or_else(|| {
        Error::RegistryInconsistency("workspace config tree has no main.dcdl".into())
    })?;
    if main.content_type != ConfigContentType::Decodal {
        return Err(Error::InvalidInput(
            "main.dcdl is not Decodal source".into(),
        ));
    }
    let before = SnapshotEnvironment::new(current.snapshot.clone())
        .evaluate_contract(&current.contract)
        .map_err(|error| {
            Error::InvalidInput(format!(
                "cannot inspect reference migration source: {error:?}"
            ))
        })?;
    let root = before
        .projections
        .first()
        .and_then(|projection| projection.data_json.as_object())
        .ok_or_else(|| Error::InvalidInput("Workspace projection is not an object".into()))?;
    let mut base = None;
    for suffix in 0..=current.snapshot.entries.len() {
        let candidate = path(&format!("permission-references-base-{suffix}.dcdl"))?;
        if current.snapshot.get(&candidate).is_none() {
            base = Some(candidate);
            break;
        }
    }
    let base = base.expect("N entries cannot occupy N+1 distinct paths");
    let inherited = format!("(import {})", quote(&format!("./{}", base.as_str())));
    let mut wrapper = String::from("{\n");
    for key in root.keys() {
        if key != "permissions" {
            wrapper.push_str(&format!("    {key} = {inherited}.{key};\n"));
        }
    }
    wrapper.push_str(&format!(
        "    permissions = {};\n}} as WorkspaceConfigSchema\n",
        render_permission_source(source),
    ));
    Ok(vec![
        ConfigTreeChange::Create {
            path: base,
            content_type: ConfigContentType::Decodal,
            content: main.content.clone(),
        },
        ConfigTreeChange::Update {
            path: main_path,
            expected_digest: main.content_digest.clone(),
            content: wrapper,
        },
    ])
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("a string serializes")
}

fn render_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    format!(
        "[{}]",
        values.into_iter().map(quote).collect::<Vec<_>>().join(", ")
    )
}

fn render_namespaces(values: &[NamespacePermissions]) -> String {
    let items = values
        .iter()
        .map(|value| {
            format!(
                "{{ namespace = {}; keys = {}; }}",
                quote(value.namespace.as_str()),
                render_strings(value.keys.iter().map(|key| key.as_str())),
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

fn render_assignment(source: &AssignmentSource) -> String {
    format!(
        "{{ sets = {}; namespaces = {}; exclude_sets = {}; exclude_namespaces = {}; revoked = {}; }}",
        render_strings(source.sets.iter().map(|set| set.as_str())),
        render_namespaces(&source.namespaces),
        render_strings(source.exclude_sets.iter().map(|set| set.as_str())),
        render_namespaces(&source.exclude_namespaces),
        source.revoked,
    )
}

fn render_permission_source(source: &PermissionSource) -> String {
    let sets = source
        .sets
        .iter()
        .map(|set| {
            format!(
                "{{ id = {}; sets = {}; namespaces = {}; }}",
                quote(set.id.as_str()),
                render_strings(set.sets.iter().map(|set| set.as_str())),
                render_namespaces(&set.namespaces),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{ sets = [{sets}]; human_worker_default = {}; }}",
        render_assignment(&source.human_worker_default)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_source::WorkspaceConfigSchemaProvider;
    use config_source::{ConfigEntry, ConfigTreeSnapshot};
    use feature_permissions::{
        FeatureIdentity, NamespaceDeclaration, NamespaceId, PermissionDefinition, PermissionKey,
        PermissionSetId, WorkspaceId,
    };
    use server_api::authorization::{PermissionResource, ScopedPermission, SubjectPermissions};
    use subjektiv::{DatabaseIdentity, PermissionSourceExport};

    const WORKSPACE: &str = "migration-workspace";

    fn host(children: bool) -> PermissionHost {
        let mut host = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
        let feature = host
            .register_feature(FeatureIdentity::parse("builtin:file").unwrap())
            .unwrap();
        let namespace = host
            .bind_namespace(&feature, NamespaceId::parse("file").unwrap())
            .unwrap();
        host.declare(
            &namespace,
            NamespaceDeclaration {
                permissions: vec![
                    PermissionDefinition {
                        key: PermissionKey::parse("file.read").unwrap(),
                        description: String::new(),
                        children: if children {
                            vec![PermissionKey::parse("file.read.text").unwrap()]
                        } else {
                            vec![]
                        },
                    },
                    PermissionDefinition {
                        key: PermissionKey::parse("file.read.text").unwrap(),
                        description: String::new(),
                        children: vec![],
                    },
                ],
                sets: vec![],
            },
        )
        .unwrap();
        host
    }

    fn limited(operation: &str) -> ScopedPermission {
        ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workdir {
                working_directory_id: "one-workdir".into(),
                path: "only-this.txt".into(),
                recursive: false,
            },
        }
    }

    fn export(sources: Vec<PermissionSourceExport>) -> PermissionTransferSnapshot {
        PermissionTransferSnapshot {
            identity: DatabaseIdentity {
                scope_id: WORKSPACE.into(),
                feature_id: subjektiv::SUBJEKTIV_FEATURE_ID.into(),
                physical_id: "typed-test-identity".into(),
            },
            sources,
            export_digest: "not-used-by-pure-conversion-tests".into(),
        }
    }

    fn current(main: &str) -> WorkspaceConfigState {
        let mut current = crate::config_source::initial_state().unwrap();
        current.snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            path(MAIN_CONFIG_ENTRYPOINT).unwrap(),
            ConfigContentType::Decodal,
            main,
        )
        .unwrap()])
        .unwrap();
        current
    }

    #[test]
    fn native_unset_empty_and_limited_policies_keep_identity_and_constraints_without_config_ids() {
        let empty = SubjectPermissions {
            change_id: "explicit-empty".into(),
            execution: vec![],
            delegation: vec![],
        };
        let limited = SubjectPermissions {
            change_id: "keep-limited-change".into(),
            execution: vec![limited("file.read")],
            delegation: vec![limited("file.read.text")],
        };
        let snapshot = export(vec![
            PermissionSourceExport {
                feature_policy: None,
                subject_id: "unset".into(),
                permissions: None,
            },
            PermissionSourceExport {
                feature_policy: None,
                subject_id: "empty".into(),
                permissions: Some(empty.clone()),
            },
            PermissionSourceExport {
                feature_policy: None,
                subject_id: "limited".into(),
                permissions: Some(limited.clone()),
            },
        ]);
        let host = host(false);
        let original = current("{ } as WorkspaceConfigSchema");
        let before = original.clone();
        let (policies, diagnostics) = migrate_policies(&host, &snapshot).unwrap();
        assert!(!policies.contains_key("unset"));
        assert_eq!(policies["empty"].change_id, empty.change_id);
        assert_eq!(
            policies["empty"].source.execution,
            AssignmentSource::default()
        );
        assert_eq!(
            policies["empty"].source.delegation,
            AssignmentSource::default()
        );
        assert_eq!(policies["limited"].legacy_read(&host).unwrap(), limited);
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|d| matches!(d, MigrationDiagnostic::DomainConstraint { .. }))
        );
        let evaluation = SnapshotEnvironment::new(original.snapshot.clone())
            .evaluate_contract(&original.contract)
            .unwrap();
        assert_eq!(
            shared_source_for_migration(&original, &evaluation).unwrap(),
            PermissionSource::default()
        );
        assert_eq!(original, before);
        let rendered = render_permission_source(&PermissionSource::default());
        assert!(!rendered.contains("subject"));
        for subject in ["unset", "empty", "limited"] {
            assert!(!rendered.contains(subject));
        }
    }

    #[test]
    fn native_feature_export_maps_old_references_without_mutating_the_snapshot() {
        use feature_permissions::{PermissionAssignment, PermissionReference, SetContribution};
        use server_api::authorization::{CurrentSubjectPolicy, PermissionReferenceEncoding};

        let mut host = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
        let feature = host
            .register_feature(FeatureIdentity::parse("fixture:current").unwrap())
            .unwrap();
        let namespace = host
            .bind_namespace(&feature, NamespaceId::parse("file").unwrap())
            .unwrap();
        host.declare(
            &namespace,
            NamespaceDeclaration {
                permissions: vec![PermissionDefinition {
                    key: PermissionKey::parse("file.read").unwrap(),
                    description: String::new(),
                    children: vec![],
                }],
                sets: ["conversation", "work-coordination", "work-execution"]
                    .into_iter()
                    .map(|set| SetContribution {
                        set: PermissionSetId::parse(set).unwrap(),
                        children: vec![PermissionReference::Namespace(NamespacePermissions {
                            namespace: NamespaceId::parse("file").unwrap(),
                            keys: vec![PermissionKey::parse("file.read").unwrap()],
                        })],
                    })
                    .collect(),
            },
        )
        .unwrap();
        let constraints = SubjectPermissions {
            change_id: "keep-feature-change".into(),
            execution: vec![limited("file.read")],
            delegation: vec![],
        };
        let original = CurrentSubjectPolicy {
            change_id: constraints.change_id.clone(),
            execution: PermissionAssignment {
                grants: vec![PermissionReference::Set(
                    PermissionSetId::parse("subject-default").unwrap(),
                )],
                exclusions: vec![],
                revoked: false,
            },
            delegation: PermissionAssignment {
                grants: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: NamespaceId::parse("subject").unwrap(),
                    keys: vec![PermissionKey::parse("subject.delegate").unwrap()],
                })],
                exclusions: vec![],
                revoked: true,
            },
            legacy_constraints: Some(constraints.clone()),
            reference_encoding: Default::default(),
        };
        let snapshot = export(vec![PermissionSourceExport {
            subject_id: "feature-private-id".into(),
            permissions: None,
            feature_policy: Some(original.clone()),
        }]);
        let frozen = serde_json::to_value(&snapshot).unwrap();
        let (policies, diagnostics) = migrate_policies(&host, &snapshot).unwrap();
        assert!(diagnostics.is_empty());
        let policy = &policies["feature-private-id"];
        assert_eq!(policy.change_id, "keep-feature-change");
        assert_eq!(policy.legacy_constraints, Some(constraints));
        assert_eq!(
            policy.reference_encoding,
            PermissionReferenceEncoding::FeatureSetsV2
        );
        assert_eq!(
            policy
                .source
                .execution
                .sets
                .iter()
                .map(|set| set.as_str())
                .collect::<Vec<_>>(),
            vec!["conversation", "work-coordination"]
        );
        assert_eq!(
            policy
                .source
                .delegation
                .sets
                .iter()
                .map(|set| set.as_str())
                .collect::<Vec<_>>(),
            vec!["work-execution"]
        );
        assert!(policy.source.delegation.revoked);
        assert_eq!(policy.current(), original.into_current_reference_encoding());
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), frozen);
    }

    #[test]
    fn unsupported_or_child_expanding_operations_return_full_blocking_diagnostics() {
        for (host, permission) in [
            (host(false), limited("file.unknown")),
            (host(true), limited("file.read")),
        ] {
            let legacy = SubjectPermissions {
                change_id: "keep-change".into(),
                execution: vec![permission.clone()],
                delegation: vec![],
            };
            let snapshot = export(vec![PermissionSourceExport {
                feature_policy: None,
                subject_id: "blocked".into(),
                permissions: Some(legacy.clone()),
            }]);
            let (policies, diagnostics) = migrate_policies(&host, &snapshot).unwrap();
            assert_eq!(
                diagnostics,
                vec![MigrationDiagnostic::ExplicitConfigurationRequired {
                    subject_id: "blocked".into(),
                    permission
                }]
            );
            assert_eq!(policies["blocked"].legacy_constraints, Some(legacy));
            assert!(policies["blocked"].source.execution.namespaces.is_empty());
        }
    }

    #[test]
    fn shared_renderer_round_trips_exclusions_and_revocation_without_individual_fields() {
        let source = PermissionSource {
            sets: vec![policy_config::WorkspacePermissionSetSource {
                id: PermissionSetId::parse("local").unwrap(),
                sets: vec![],
                namespaces: vec![],
            }],
            human_worker_default: AssignmentSource {
                sets: vec![PermissionSetId::parse("local").unwrap()],
                exclude_sets: vec![PermissionSetId::parse("excluded").unwrap()],
                revoked: true,
                ..Default::default()
            },
        };
        let rendered = render_permission_source(&source);
        assert!(!rendered.contains("subjects"));
        assert!(!rendered.contains("subject_id"));
        let state = current(&format!(
            "{{ permissions = {rendered}; }} as WorkspaceConfigSchema"
        ));
        let evaluation = SnapshotEnvironment::new(state.snapshot)
            .evaluate_contract(&state.contract)
            .unwrap();
        assert_eq!(policy_config::source(&evaluation).unwrap(), source);
    }

    #[test]
    fn computed_individual_source_blocks_without_copying_or_modifying_bytes() {
        let state = current(
            "let old = [{ subject_id = \"private-id\"; }]; in { permissions = { subjects = old; }; } as WorkspaceConfigSchema",
        );
        let before = state.clone();
        let error = reference_migration_changes(&state, &PermissionSource::default()).unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
        assert_eq!(state, before);
    }

    #[test]
    fn current_shared_validation_does_not_look_up_subjects() {
        policy_config::validate_source(&host(false), &PermissionSource::default(), |_| {
            panic!("shared configuration must not consult Subject existence")
        })
        .unwrap();
    }

    #[test]
    fn provider_schema_and_strict_decoder_exclude_even_empty_individual_fields() {
        let provider = policy_config::PermissionConfigSchemaProvider
            .contribution()
            .unwrap();
        assert_eq!(provider.version, "4");
        assert!(!provider.source.contains("subjects"));
        assert!(!provider.source.contains("subject_id"));
        for value in [
            serde_json::json!({"subjects":[]}),
            serde_json::json!({"remove_subjects":[]}),
        ] {
            assert!(serde_json::from_value::<PermissionSource>(value.clone()).is_err());
            assert_eq!(
                serde_json::from_value::<policy_config::LegacyPermissionSource>(value)
                    .unwrap()
                    .shared_source(),
                PermissionSource::default()
            );
        }
    }
}
