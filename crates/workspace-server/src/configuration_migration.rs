//! Read-only preparation of an editable Workspace permission migration draft.
//!
//! The authenticated owner boundary lives in Server/core. This helper checks the
//! guarded stores and exact Workspace attribution, but neither saves the draft nor
//! seals the legacy writer. Adoption must check `original`, the actual editing
//! head and export identity/digest again, and reject blocking diagnostics before
//! sealing. A successful preview alone is not permission authority.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{
    ConfigContentType, ConfigTreeChange, ConfigTreeSnapshot, SnapshotEnvironment,
    ToolchainContract, VirtualPath, WorkspaceConfigSchemaBundle,
};
use feature_permissions::{NamespacePermissions, PermissionHost};
use subjektiv::{PermissionTransferSnapshot, SubjektivStore};

use crate::config_source::{
    ConfigCommitRequest, MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigState,
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
    // This API validates native physical identity under the FeatureDatabase guard.
    // Do not replace it with path inspection or SQL against the legacy database.
    let identity = legacy.database_identity().map_err(legacy_error)?;
    if identity.scope_id != workspace || identity.feature_id != subjektiv::SUBJEKTIV_FEATURE_ID {
        return Err(Error::WorkspaceIdMismatch);
    }
    let original = store.with_conn(|conn| {
        // with_conn also owns the managed Server catalog identity/retention guard.
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
    let (policies, diagnostics) = migrate_policies(host, &snapshot)?;
    let source = PermissionSource {
        subjects: policies
            .values()
            .map(|policy| policy.source.clone())
            .collect(),
        remove_subjects: vec![],
        ..Default::default()
    };
    let changes = draft_changes(&original, &source)?;
    // Caller's real application provider bundle (including PromptSchema) is
    // required: a migration-specific substitute cannot validate other settings.
    let candidate = store.evaluate_workspace_config_candidate_with_schema(
        workspace,
        &ConfigCommitRequest {
            base_digest: original.snapshot.digest.clone(),
            changes,
            entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)?],
        },
        schema,
    )?;
    validate_workspace_config_candidate_projections(workspace, &candidate)?;
    let evaluated_source = policy_config::source(&candidate.evaluation)?;
    if evaluated_source != source {
        return Err(Error::InvalidInput(
            "migration import did not produce the exact generated Subject assignments".into(),
        ));
    }
    let subjects: BTreeSet<_> = snapshot
        .sources
        .iter()
        .map(|item| item.subject_id.as_str())
        .collect();
    policy_config::validate_source(host, &evaluated_source, |subject| {
        if subjects.contains(subject) {
            Ok(())
        } else {
            Err(Error::SubjektivSubjectNotFound(subject.into()))
        }
    })?;
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
            policy_config::validate_source(
                host,
                &PermissionSource {
                    subjects: vec![policy.source.clone()],
                    remove_subjects: vec![],
                    ..Default::default()
                },
                |_| Ok(()),
            )?;
            policies.insert(item.subject_id.clone(), policy);
            continue;
        }
        // None stays unset. In particular, never enroll it in a default Set.
        let Some(legacy) = &item.permissions else {
            continue;
        };
        let (policy, mut converted) =
            policy_config::migrate_subject(host, &item.subject_id, legacy)?;
        policies.insert(item.subject_id.clone(), policy);
        // ExplicitConfigurationRequired must survive preview, even though the
        // unsupported operation is absent from the editable assignment. Core must
        // block adoption on it; this is not a silently successful conversion.
        diagnostics.append(&mut converted);
    }
    Ok((policies, diagnostics))
}

/// Creation-only additive virtual-source edit. Keep the original main text at
/// the same virtual directory (so relative imports retain meaning), and leave all
/// other existing entries unchanged. Decodal performs patch/array composition;
/// ordinary adoption applies its existing formatter and whole-source validation.
/// No evaluated configuration values are rendered back over user-authored source.
pub(crate) fn enrollment_changes(
    current: &WorkspaceConfigState,
    subject: &policy_config::SubjectAssignmentSource,
) -> Result<Vec<ConfigTreeChange>> {
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
            Error::InvalidInput(format!("cannot inspect enrollment source: {error:?}"))
        })?;
    let addition = format!(
        "{{ subject_id = {}; execution = {}; delegation = {}; }}",
        quote(&subject.subject_id),
        render_assignment(&subject.execution),
        render_assignment(&subject.delegation)
    );
    // Source-only locator, never authority. It names an ordinary editable virtual
    // array source imported by main.dcdl. The whole-source additive proof below
    // prevents stale/unreferenced/owner-modified markers from changing policy.
    const MARKER: &str = "# Subject creation additions source: ";
    let locator = main
        .content
        .lines()
        .map(str::trim)
        .nth(1)
        .and_then(|line| line.strip_prefix(MARKER))
        .map(|raw| {
            serde_json::from_str::<String>(raw)
                .map_err(|error| Error::InvalidInput(error.to_string()))
        })
        .transpose()?;
    let changes = if let Some(locator) = locator {
        let additions_path = path(&locator)?;
        let additions = current.snapshot.get(&additions_path).ok_or_else(|| {
            Error::InvalidInput(
                "enrollment source locator no longer exists; reconcile source explicitly".into(),
            )
        })?;
        if additions.content_type != ConfigContentType::Decodal {
            return Err(Error::InvalidInput(
                "enrollment source is not Decodal".into(),
            ));
        }
        // Concatenate in the same source, without nested imports/let environments
        // per creation. Existing text/comments are not parsed or rewritten here.
        vec![ConfigTreeChange::Update {
            path: additions_path,
            expected_digest: additions.content_digest.clone(),
            content: format!("{}\n++ [{addition}]\n", additions.content),
        }]
    } else {
        let unused = |stem: &str| -> Result<VirtualPath> {
            for suffix in 0..=current.snapshot.entries.len() {
                let name = if suffix == 0 {
                    format!("{stem}.dcdl")
                } else {
                    format!("{stem}-{suffix}.dcdl")
                };
                let candidate = path(&name)?;
                if current.snapshot.get(&candidate).is_none() {
                    return Ok(candidate);
                }
            }
            unreachable!("N entries cannot occupy N+1 distinct paths")
        };
        // Keep the old entrypoint text in the SAME virtual directory so all its
        // relative imports retain meaning. No host paths, values-only JSON copy,
        // alternate policy storage or per-Subject import chain is introduced.
        let base_path = unused("subject-enrollment-base")?;
        let additions_path = unused("permissions/subject-enrollment")?;
        let base_import = format!("(import {})", quote(&format!("./{}", base_path.as_str())));
        let additions_import = format!(
            "(import {})",
            quote(&format!("./{}", additions_path.as_str()))
        );
        let root = before
            .projections
            .first()
            .and_then(|p| p.data_json.as_object())
            .ok_or_else(|| Error::InvalidInput("Workspace projection is not an object".into()))?;
        let empty = root
            .get("permissions")
            .and_then(|p| p.get("subjects"))
            .and_then(|value| value.as_array())
            .is_some_and(Vec::is_empty);
        let previous = if empty {
            "[]".to_owned()
        } else {
            format!("{base_import}.permissions.subjects")
        };
        let mut content = format!("{{\n    {MARKER}{}\n", quote(additions_path.as_str()));
        for key in root.keys() {
            // Registered application schema supplies root field names. Existing
            // Decodal evaluation validates these references, not a Feature table.
            if key == "permissions" {
                content.push_str(&format!("    permissions = ({base_import}.permissions // {{ subjects = {previous} ++ {additions_import}; }});\n"));
            } else {
                content.push_str(&format!("    {key} = {base_import}.{key};\n"));
            }
        }
        content.push_str("} as WorkspaceConfigSchema\n");
        vec![
            ConfigTreeChange::Create {
                path: base_path,
                content_type: ConfigContentType::Decodal,
                content: main.content.clone(),
            },
            ConfigTreeChange::Create {
                path: additions_path,
                content_type: ConfigContentType::Decodal,
                content: format!("[{addition}]\n"),
            },
            ConfigTreeChange::Update {
                path: main_path,
                expected_digest: main.content_digest.clone(),
                content,
            },
        ]
    };
    let candidate = current
        .snapshot
        .apply(&changes)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    let mut evaluated = SnapshotEnvironment::new(candidate)
        .evaluate_contract(&current.contract)
        .map_err(|error| {
            Error::InvalidInput(format!("cannot compose enrollment source: {error:?}"))
        })?;
    for projection in &mut evaluated.projections {
        let appended = projection
            .data_json
            .get_mut("permissions")
            .and_then(|p| p.get_mut("subjects"))
            .and_then(|value| value.as_array_mut())
            .and_then(Vec::pop);
        if appended
            .as_ref()
            .and_then(|p| p.get("subject_id"))
            .and_then(|value| value.as_str())
            != Some(subject.subject_id.as_str())
        {
            return Err(Error::InvalidInput(
                "enrollment did not append the exact new Subject; source was not changed".into(),
            ));
        }
    }
    if evaluated
        .projections
        .iter()
        .map(|p| (&p.entrypoint, &p.data_json))
        .collect::<Vec<_>>()
        != before
            .projections
            .iter()
            .map(|p| (&p.entrypoint, &p.data_json))
            .collect::<Vec<_>>()
    {
        return Err(Error::InvalidInput(
            "enrollment changed existing configuration; source was not changed".into(),
        ));
    }
    Ok(changes)
}

fn draft_changes(
    original: &WorkspaceConfigState,
    source: &PermissionSource,
) -> Result<Vec<ConfigTreeChange>> {
    // Evaluate with the existing open schema, not application defaults. This
    // distinguishes an explicit `permissions = {}` (including imports/dotted
    // fields) from an omitted field without introducing another Decodal parser.
    let open_contract = ToolchainContract::with_schema_bundle(
        original.contract.schema_version,
        original.contract.entrypoints.clone(),
        original.contract.import_policy_version,
        WorkspaceConfigSchemaBundle::empty(),
    );
    let raw = SnapshotEnvironment::new(original.snapshot.clone())
        .evaluate_contract(&open_contract)
        .map_err(|diagnostics| {
            Error::InvalidInput(format!(
                "cannot inspect current configuration before permission migration: {diagnostics:?}"
            ))
        })?;
    if raw
        .projections
        .iter()
        .any(|projection| projection.data_json.get("permissions").is_some())
    {
        return Err(Error::InvalidInput(
            "current source explicitly defines permissions; automatic migration cannot merge or replace it; preserve and reconcile that source explicitly".into(),
        ));
    }
    let generated_path = unused_source_path(&original.snapshot)?;
    let main_path = path(MAIN_CONFIG_ENTRYPOINT)?;
    let main = original.snapshot.get(&main_path).ok_or_else(|| {
        Error::RegistryInconsistency("workspace config tree has no main.dcdl".into())
    })?;
    if main.content_type != ConfigContentType::Decodal {
        return Err(Error::InvalidInput(
            "main.dcdl is not Decodal source".into(),
        ));
    }
    // Use the existing syntax formatter to supply a final field separator when
    // needed. It preserves comments; no local lexer or alternate language rules.
    let formatted_main = SnapshotEnvironment::new(original.snapshot.clone())
        .format(&main.content)
        .map_err(|error| {
            Error::InvalidInput(format!("cannot format migration entrypoint: {error}"))
        })?;
    let (rendered, mut changes) =
        render_permission_source(source, &original.snapshot, &generated_path)?;
    changes.extend([
        ConfigTreeChange::Create {
            path: generated_path.clone(),
            content_type: ConfigContentType::Decodal,
            content: rendered,
        },
        ConfigTreeChange::Update {
            path: main_path,
            expected_digest: main.content_digest.clone(),
            content: append_import(&formatted_main, &generated_path)?,
        },
    ]);
    Ok(changes)
}

fn unused_source_path(snapshot: &ConfigTreeSnapshot) -> Result<VirtualPath> {
    // With N entries, one of these N+1 names is unused. Never overwrite even an
    // unreferenced preexisting migration file.
    for suffix in 0..=snapshot.entries.len() {
        let name = if suffix == 0 {
            "permissions/migrated.dcdl".to_owned()
        } else {
            format!("permissions/migrated-{suffix}.dcdl")
        };
        let candidate = path(&name)?;
        if snapshot.get(&candidate).is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("N entries cannot occupy N+1 distinct migration paths")
}

fn append_import(source: &str, imported: &VirtualPath) -> Result<String> {
    // Materialized main.dcdl is a literal outer object with a trailing schema
    // assertion. Limit this edit to that existing representation: no rewrites of
    // arbitrary expressions, no JSON round-trip, no replacement of other fields.
    let trimmed = source.trim_end();
    let object = trimmed
        .strip_suffix("as WorkspaceConfigSchema")
        .map(str::trim_end)
        .unwrap_or(trimmed);
    if !object.starts_with('{') || !object.ends_with('}') {
        return Err(Error::InvalidInput(
            "permission migration requires main.dcdl's outer object before its WorkspaceConfigSchema assertion; existing source was not changed".into(),
        ));
    }
    let close = object.len() - 1;
    let import = quote(&format!("./{}", imported.as_str()));
    Ok(format!(
        "{}\n    permissions = import {import};\n{}",
        &source[..close],
        &source[close..]
    ))
}

// A deliberately specific, payload-free renderer. JSON quoting is used only for
// string literals, never as a renderer for the configuration/assignment objects.
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

/// Render identifiers without the Decodal 0.4 string lexer's non-ASCII byte
/// corruption. The existing virtual Markdown value-import route supplies exact
/// UTF-8 content; it is ordinary editable configuration source, not a policy DB
/// or a host-file lookup. Never use unsupported JSON Unicode escapes (which that
/// lexer accepts but changes into literal `uXXXX`). ASCII catalog keys stay typed
/// string literals. All generated files participate in the same adoption request.
fn render_permission_source(
    source: &PermissionSource,
    snapshot: &ConfigTreeSnapshot,
    containing: &VirtualPath,
) -> Result<(String, Vec<ConfigTreeChange>)> {
    let mut changes = Vec::new();
    let mut strings = BTreeMap::<String, VirtualPath>::new();
    let mut literal = |value: &str| -> Result<String> {
        server_api::authorization::validate_authorization_identifier(value)
            .map_err(Error::InvalidInput)?;
        if value.is_ascii() {
            return Ok(quote(value));
        }
        let imported = if let Some(path) = strings.get(value) {
            path.clone()
        } else {
            let mut free = None;
            for suffix in 0..=snapshot.entries.len() + strings.len() {
                let candidate = path(&format!("{}.string-{suffix}.md", containing.as_str()))?;
                if snapshot.get(&candidate).is_none()
                    && !strings.values().any(|path| *path == candidate)
                {
                    free = Some(candidate);
                    break;
                }
            }
            let imported = free.expect("N entries cannot occupy N+1 distinct literal paths");
            changes.push(ConfigTreeChange::Create {
                path: imported.clone(),
                content_type: ConfigContentType::Text,
                content: value.to_owned(),
            });
            strings.insert(value.to_owned(), imported.clone());
            imported
        };
        // This is a sibling of the containing virtual Decodal source. Input IDs
        // are content only, never path components or import authority.
        let sibling = imported
            .as_str()
            .rsplit('/')
            .next()
            .expect("validated virtual path");
        Ok(format!(
            "(import {}).content",
            quote(&format!("./{sibling}"))
        ))
    };
    let subjects = source
        .subjects
        .iter()
        .map(|item| {
            Ok(format!(
                "        {{ subject_id = {}; execution = {}; delegation = {}; }}",
                literal(&item.subject_id)?,
                render_assignment(&item.execution),
                render_assignment(&item.delegation)
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",\n");
    let removals = source
        .remove_subjects
        .iter()
        .map(|id| literal(id))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let rendered = format!(
        "{{\n    subjects = [\n{subjects}\n    ];\n    remove_subjects = [{removals}];\n}}\n"
    );
    Ok((rendered, changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_source::WorkspaceConfigSchemaProvider;
    use config_source::{ConfigEntry, EvaluationResult};
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

    fn current(main: &str, extra: Vec<ConfigEntry>) -> WorkspaceConfigState {
        let mut current = crate::config_source::initial_state().unwrap();
        let mut entries = vec![
            ConfigEntry::new(
                path(MAIN_CONFIG_ENTRYPOINT).unwrap(),
                ConfigContentType::Decodal,
                main,
            )
            .unwrap(),
        ];
        entries.extend(extra);
        current.snapshot = ConfigTreeSnapshot::from_entries(entries).unwrap();
        current
    }

    fn evaluate(snapshot: ConfigTreeSnapshot) -> EvaluationResult {
        let contract = crate::config_source::initial_state().unwrap().contract;
        SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .unwrap()
    }

    #[test]
    fn unset_empty_and_limited_policies_keep_change_identity_and_constraints() {
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
                    permission,
                }]
            );
            assert_eq!(policies["blocked"].legacy_constraints, Some(legacy));
            assert!(policies["blocked"].source.execution.namespaces.is_empty());
            assert!(policies["blocked"].source.execution.sets.is_empty());
        }
    }

    #[test]
    fn renderer_round_trips_sets_namespaces_exclusions_revocation_and_quoted_strings() {
        let assignment = AssignmentSource {
            sets: vec![PermissionSetId::parse("shared").unwrap()],
            namespaces: vec![NamespacePermissions {
                namespace: NamespaceId::parse("file").unwrap(),
                keys: vec![PermissionKey::parse("file.read.text").unwrap()],
            }],
            exclude_sets: vec![PermissionSetId::parse("excluded").unwrap()],
            exclude_namespaces: vec![NamespacePermissions {
                namespace: NamespaceId::parse("file").unwrap(),
                keys: vec![PermissionKey::parse("file.read").unwrap()],
            }],
            revoked: true,
        };
        let source = PermissionSource {
            subjects: vec![policy_config::SubjectAssignmentSource {
                subject_id: "subject-\"quoted\\日本語".into(),
                execution: assignment,
                delegation: AssignmentSource::default(),
            }],
            remove_subjects: vec!["removed".into()],
            sets: vec![],
            human_worker_default: AssignmentSource::default(),
        };
        let main_path = path(MAIN_CONFIG_ENTRYPOINT).unwrap();
        let (rendered, mut changes) =
            render_permission_source(&source, &ConfigTreeSnapshot::empty(), &main_path).unwrap();
        assert!(rendered.contains("sets = [\"shared\"]"));
        assert!(rendered.contains("keys = [\"file.read.text\"]"));
        assert!(!rendered.contains("legacy_constraints"));
        assert!(rendered.contains(".content"));
        assert!(
            matches!(&changes[0], ConfigTreeChange::Create { content, content_type: ConfigContentType::Text, .. } if content == &source.subjects[0].subject_id)
        );
        changes.push(ConfigTreeChange::Create {
            path: main_path,
            content_type: ConfigContentType::Decodal,
            content: format!("{{ permissions = {rendered}; }} as WorkspaceConfigSchema"),
        });
        let snapshot = ConfigTreeSnapshot::empty().apply(&changes).unwrap();
        assert_eq!(policy_config::source(&evaluate(snapshot)).unwrap(), source);
    }

    #[test]
    fn draft_preserves_main_comments_other_settings_and_existing_migration_files() {
        let main = "{\n    # permissions in a comment are not a field\n    other = { text = \"brace } and permissions\"; };\n} as WorkspaceConfigSchema\n";
        let occupied = ConfigEntry::new(
            path("permissions/migrated.dcdl").unwrap(),
            ConfigContentType::Decodal,
            "{ keep = true; }\n",
        )
        .unwrap();
        let original = current(main, vec![occupied.clone()]);
        let source = PermissionSource::default();
        let changes = draft_changes(&original, &source).unwrap();
        let draft = original.snapshot.apply(&changes).unwrap();
        assert_eq!(draft.get(&occupied.path), Some(&occupied));
        assert!(
            draft
                .get(&path("permissions/migrated-1.dcdl").unwrap())
                .is_some()
        );
        let updated_main = &draft
            .get(&path(MAIN_CONFIG_ENTRYPOINT).unwrap())
            .unwrap()
            .content;
        assert!(updated_main.contains("# permissions in a comment are not a field"));
        assert!(updated_main.contains("brace } and permissions"));
        assert!(updated_main.contains("permissions = import \"./permissions/migrated-1.dcdl\";"));
        assert!(updated_main.ends_with("} as WorkspaceConfigSchema\n"));
        let before = evaluate(original.snapshot.clone());
        let after = evaluate(draft);
        assert_eq!(
            after.projections[0].data_json["other"],
            before.projections[0].data_json["other"]
        );
        assert_eq!(policy_config::source(&after).unwrap(), source);
        assert_eq!(
            original
                .snapshot
                .get(&path(MAIN_CONFIG_ENTRYPOINT).unwrap())
                .unwrap()
                .content,
            main
        );
    }

    #[test]
    fn existing_explicit_permissions_are_not_merged_or_discarded_even_when_empty() {
        for main in [
            "{ permissions = {}; } as WorkspaceConfigSchema",
            "{ permissions.subjects = []; } as WorkspaceConfigSchema",
            "{ permissions = import \"./permissions/existing.dcdl\"; } as WorkspaceConfigSchema",
        ] {
            let existing = ConfigEntry::new(
                path("permissions/existing.dcdl").unwrap(),
                ConfigContentType::Decodal,
                "{ subjects = []; }",
            )
            .unwrap();
            let original = current(main, vec![existing]);
            let before = original.clone();
            let error = draft_changes(&original, &PermissionSource::default()).unwrap_err();
            assert!(
                error.to_string().contains("explicitly defines permissions"),
                "{error}"
            );
            assert_eq!(original, before);
        }
    }

    #[test]
    fn enrollment_composes_expression_and_field_sources_without_changing_existing_text() {
        let bundle = WorkspaceConfigSchemaBundle::compose([
            policy_config::PermissionConfigSchemaProvider
                .contribution()
                .unwrap(),
            config_source::ConfigSchemaContribution::new(
                "fixture:other",
                "other",
                "1",
                "{ other = { text = String; }; }",
            )
            .unwrap(),
        ])
        .unwrap();
        for main in [
            "# before\n{ other = { text = \"brace }\"; }; permissions = {}; } as WorkspaceConfigSchema\n# after",
            "# before\nother.text = \"brace }\"; permissions = {}; # after",
            "let prior = { other.text = \"brace }\"; permissions = {}; }; in prior",
        ] {
            let mut original = current(main, vec![]);
            original.contract = ToolchainContract::with_schema_bundle(
                original.contract.schema_version,
                original.contract.entrypoints.clone(),
                original.contract.import_policy_version,
                bundle.clone(),
            );
            let subject = policy_config::SubjectAssignmentSource {
                subject_id: "fresh".into(),
                execution: AssignmentSource::default(),
                delegation: AssignmentSource::default(),
            };
            let changes = enrollment_changes(&original, &subject).unwrap();
            let edited = original.snapshot.apply(&changes).unwrap();
            assert!(
                edited
                    .get(&path("subject-enrollment-base.dcdl").unwrap())
                    .unwrap()
                    .content
                    == main
            );
            let before = SnapshotEnvironment::new(original.snapshot.clone())
                .evaluate_contract(&original.contract)
                .unwrap();
            let after = SnapshotEnvironment::new(edited.clone())
                .evaluate_contract(&original.contract)
                .unwrap();
            assert_eq!(
                before.projections[0].data_json["other"],
                after.projections[0].data_json["other"]
            );
            assert_eq!(
                policy_config::source(&after).unwrap().subjects,
                vec![subject]
            );
            original.snapshot = edited;
            for index in 1..35 {
                let subject = policy_config::SubjectAssignmentSource {
                    subject_id: format!("fresh-{index}"),
                    execution: AssignmentSource::default(),
                    delegation: AssignmentSource::default(),
                };
                original.snapshot = original
                    .snapshot
                    .apply(&enrollment_changes(&original, &subject).unwrap())
                    .unwrap();
                let environment = SnapshotEnvironment::new(original.snapshot.clone());
                original.snapshot = ConfigTreeSnapshot::from_entries(
                    original.snapshot.entries.values().map(|entry| {
                        ConfigEntry::new(
                            entry.path.clone(),
                            entry.content_type,
                            environment.format(&entry.content).unwrap(),
                        )
                        .unwrap()
                    }),
                )
                .unwrap();
                let evaluated = SnapshotEnvironment::new(original.snapshot.clone())
                    .evaluate_contract(&original.contract)
                    .unwrap();
                assert_eq!(
                    policy_config::source(&evaluated).unwrap().subjects.len(),
                    index + 1
                );
                assert_eq!(original.snapshot.entries.len(), 3); // no per-Subject import chain
            }
        }
    }
}
