//! Cross-store component tests connect configuration migration and adoption to a
//! real guarded Subjektiv FeatureDatabase and the production canonical-source
//! bridge, which reads the real Server policy store on every call.
//! These ordinary crate tests use isolated temporary SQLite stores, not HTTP,
//! authenticated Worker execution, or a distributed transaction/crash guarantee.

use std::path::Path;
use std::sync::Arc;

use config_source::{
    ConfigContentType, ConfigTreeChange, VirtualPath, WorkspaceConfigSchemaBundle,
};
use feature_permissions::{
    FeatureIdentity, NamespaceDeclaration, NamespaceId, PermissionDefinition, PermissionHost,
    PermissionKey, WorkspaceId,
};
use feature_storage::{FeatureDatabase, FeatureStorage, ScopedFeatureStorage};
use rusqlite::Connection;
use server_api::authorization::{
    PermissionResource, ScopedPermission, SubjectPermissions, SubjectPermissionsUpdate,
    WorkerExecutionGrant,
};
use subjektiv::{SubjectRole, SubjektivError, SubjektivStore};

use crate::config_adoption::{
    CanonicalConfigurationSource, ConfigurationAdoptionRequest, ConfigurationAdoptionResult,
    ConfigurationMigrationBaseline, ConfigurationPolicyTransfer, catalog_digest,
    request_fingerprint,
};
use crate::config_source::{
    ConfigCommitRequest, MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider,
    WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::configuration_migration::{MigrationSeed, preview_migration};
use crate::policy_config::{AssignmentSource, MigrationDiagnostic, PermissionConfigSchemaProvider};
use crate::{Error, Result, SqliteWorkspaceStore};

const WORKSPACE: &str = "configuration-transfer-workspace";
const OWNER: &str = "configuration-transfer-owner";
const EXISTING_MAIN: &str = r#"{
    # Keep this owner's explanation, including the word permissions.
    prompts = { common = { language = import "./existing.dcdl"; }; };
} as WorkspaceConfigSchema
"#;
const EXISTING_DATA: &str = "\"KEEP LANGUAGE\"\n";
const OCCUPIED_MIGRATION: &str =
    "# An unrelated, unreferenced file must not be overwritten.\n{ keep = true; }\n";

fn path(value: &str) -> VirtualPath {
    VirtualPath::parse(value).unwrap()
}

fn schema() -> WorkspaceConfigSchemaBundle {
    WorkspaceConfigSchemaBundle::compose([
        PermissionConfigSchemaProvider.contribution().unwrap(),
        crate::prompt_settings::PromptConfigSchemaProvider
            .contribution()
            .unwrap(),
    ])
    .unwrap()
}

fn host() -> PermissionHost {
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
            permissions: ["file.read", "file.read.text"]
                .into_iter()
                .map(|key| PermissionDefinition {
                    key: PermissionKey::parse(key).unwrap(),
                    description: String::new(),
                    children: vec![],
                })
                .collect(),
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

struct FeatureFixture {
    store: SubjektivStore,
    database: FeatureDatabase,
    scope: ScopedFeatureStorage,
    manager: FeatureStorage,
}

impl FeatureFixture {
    fn open(root: &Path, expected: Option<&feature_storage::DatabaseIdentity>) -> Self {
        let manager = FeatureStorage::new(root);
        let scope = manager.workspace(WORKSPACE).unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        let database = match expected {
            Some(identity) => scope.open_existing(&registration, identity).unwrap(),
            None => scope
                .provision(
                    &registration,
                    "fixture-initial-provision",
                    "isolated component-test Host provisioning",
                )
                .unwrap(),
        };
        let store = SubjektivStore::from_database(database.clone()).unwrap();
        Self {
            store,
            database,
            scope,
            manager,
        }
    }

    fn close(self) {
        drop(self.store);
        drop(self.database);
        self.scope.shutdown().unwrap();
        self.manager.shutdown().unwrap();
    }
}

struct Subjects {
    unset: String,
    empty: String,
    limited: String,
    empty_source: SubjectPermissions,
    limited_source: SubjectPermissions,
    exact_legacy_update: SubjectPermissionsUpdate,
    stale_grant: WorkerExecutionGrant,
    live_grant: WorkerExecutionGrant,
}

fn grant(subject: &str, change: &str, worker: &str) -> WorkerExecutionGrant {
    WorkerExecutionGrant {
        subject_id: subject.into(),
        source_change_id: change.into(),
        runtime_id: "runtime".into(),
        worker_id: worker.into(),
        origin_runtime_id: "runtime".into(),
        origin_worker_id: "authenticated-root".into(),
        profile: "builtin:coder".into(),
        profile_digest: "resolved-profile".into(),
        permissions: vec![limited("file.read.text")],
        redelegation: vec![],
    }
}

fn replace(
    store: &SubjektivStore,
    subject: &str,
    id: &str,
    execution: Vec<ScopedPermission>,
    delegation: Vec<ScopedPermission>,
) -> (SubjectPermissionsUpdate, SubjectPermissions) {
    let update = SubjectPermissionsUpdate {
        operation_id: id.into(),
        expected_change_id: store
            .current_subject_policy(subject)
            .unwrap()
            .map(|source| source.change_id),
        execution,
        delegation,
    };
    let source = store.set_subject_permissions(subject, &update).unwrap();
    (update, source)
}

fn subjects(features: &FeatureFixture) -> Subjects {
    let create = || {
        features
            .store
            .create_subject(SubjectRole::new("component fixture").unwrap())
            .unwrap()
            .id
    };
    let unset = create();
    let empty = create();
    let subject = create();
    // Historical Subjects can have no source. Keep the production schema and
    // physical identity; seed only that historical row state through its guard.
    features
        .database
        .try_transaction::<_, SubjektivError>(|tx| {
            tx.execute(
                "DELETE FROM subject_permissions WHERE subject_id=?1",
                [&unset],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, empty_source) = replace(&features.store, &empty, "explicit-empty", vec![], vec![]);
    let (_, old) = replace(
        &features.store,
        &subject,
        "old-limited",
        vec![limited("file.read")],
        vec![limited("file.read.text")],
    );
    let stale_grant = grant(&subject, &old.change_id, "already-stale-child");
    features
        .store
        .bind_worker_execution_grant(&stale_grant)
        .unwrap();
    let (exact_legacy_update, limited_source) = replace(
        &features.store,
        &subject,
        "current-limited",
        vec![limited("file.read")],
        vec![limited("file.read.text")],
    );
    let live_grant = grant(&subject, &limited_source.change_id, "live-child");
    features
        .store
        .bind_worker_execution_grant(&live_grant)
        .unwrap();
    Subjects {
        unset,
        empty,
        limited: subject,
        empty_source,
        limited_source,
        exact_legacy_update,
        stale_grant,
        live_grant,
    }
}

fn server(root: &Path) -> SqliteWorkspaceStore {
    SqliteWorkspaceStore::open_for_test(root.join("server.sqlite")).unwrap()
}

fn fixture() -> (
    tempfile::TempDir,
    SqliteWorkspaceStore,
    FeatureFixture,
    Subjects,
) {
    let root = tempfile::tempdir().unwrap();
    let server = server(root.path());
    server.with_immediate_transaction(|tx| {
        tx.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES(?1,'user',?1,'Owner','1','1')", [OWNER])?;
        tx.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES(?1,?2,'Transfer','active','1','1')", [WORKSPACE, OWNER])?;
        Ok(())
    }).unwrap();
    let initial = server
        .ensure_workspace_config_materialized_with_schema(WORKSPACE, "now", schema())
        .unwrap();
    let main = path(MAIN_CONFIG_ENTRYPOINT);
    let candidate = server
        .evaluate_workspace_config_candidate_with_schema(
            WORKSPACE,
            &ConfigCommitRequest {
                base_digest: initial.snapshot.digest.clone(),
                changes: vec![
                    ConfigTreeChange::Update {
                        path: main.clone(),
                        expected_digest: initial
                            .snapshot
                            .get(&main)
                            .unwrap()
                            .content_digest
                            .clone(),
                        content: EXISTING_MAIN.into(),
                    },
                    ConfigTreeChange::Create {
                        path: path("existing.dcdl"),
                        content_type: ConfigContentType::Decodal,
                        content: EXISTING_DATA.into(),
                    },
                    ConfigTreeChange::Create {
                        path: path("permissions/migrated.dcdl"),
                        content_type: ConfigContentType::Decodal,
                        content: OCCUPIED_MIGRATION.into(),
                    },
                ],
                entrypoints: vec![main],
            },
            schema(),
        )
        .unwrap();
    server
        .commit_evaluated_workspace_config(WORKSPACE, &candidate)
        .unwrap();
    let features = FeatureFixture::open(&root.path().join("features"), None);
    let subjects = subjects(&features);
    (root, server, features, subjects)
}

fn migration(
    server: &SqliteWorkspaceStore,
    features: &FeatureFixture,
    host: &PermissionHost,
) -> MigrationSeed {
    preview_migration(server, &features.store, WORKSPACE, schema(), host).unwrap()
}

fn baseline(seed: &MigrationSeed) -> ConfigurationMigrationBaseline<'_> {
    ConfigurationMigrationBaseline {
        original: &seed.original,
        state: &seed.state,
        policies: &seed.policies,
        diagnostics: &seed.diagnostics,
        export_digest: &seed.snapshot.export_digest,
    }
}

fn migration_request(seed: &MigrationSeed, host: &PermissionHost) -> ConfigurationAdoptionRequest {
    ConfigurationAdoptionRequest {
        operation_id: "adopt-migration".into(),
        expected_head: None,
        expected_catalog_digest: catalog_digest(host).unwrap(),
        expected_legacy_export: Some(seed.snapshot.export_digest.clone()),
        source: ConfigCommitRequest {
            base_digest: seed.state.snapshot.digest.clone(),
            changes: vec![],
            entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
        },
    }
}

fn require_subject(store: &SubjektivStore, subject: &str) -> Result<()> {
    store
        .subject(subject)
        .map_err(|error| Error::Store(error.to_string()))?
        .ok_or_else(|| Error::SubjektivSubjectNotFound(subject.into()))?;
    Ok(())
}

fn adopt_migration(
    server: &SqliteWorkspaceStore,
    features: &FeatureFixture,
    host: &PermissionHost,
    seed: &MigrationSeed,
    request: &ConfigurationAdoptionRequest,
) -> Result<ConfigurationAdoptionResult> {
    features
        .store
        .with_permission_authority_guard(&seed.snapshot.identity, |authority| {
            Ok(server.adopt_configuration(
                WORKSPACE,
                OWNER,
                request,
                schema(),
                host,
                Some(&baseline(seed)),
                |subject| {
                    authority
                        .require_subject(subject)
                        .map_err(|error| Error::Store(error.to_string()))
                },
                |fingerprint| {
                    let receipt = authority
                        .seal_permission_sources(fingerprint, &seed.snapshot.export_digest)
                        .map_err(|error| Error::Store(error.to_string()))?;
                    assert_eq!(receipt.snapshot, seed.snapshot);
                    Ok(ConfigurationPolicyTransfer {
                        transfer_identity: receipt.transfer_id,
                        physical_identity: receipt.snapshot.identity.physical_id,
                        export_digest: receipt.snapshot.export_digest,
                        policies: seed.policies.clone(),
                    })
                },
            ))
        })
        .map_err(|error| Error::Store(error.to_string()))?
}

// Observe persisted records rather than calls. Sorting row renderings also covers
// composite-key/WITHOUT ROWID tables without relying on SQLite traversal order.
fn table_rows(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("SELECT * FROM {table}"))?;
    let columns = statement.column_count();
    let mut rows = statement
        .query_map([], |row| {
            let values = (0..columns)
                .map(|index| row.get::<_, rusqlite::types::Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(format!("{values:?}"))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.sort();
    Ok(rows)
}

fn server_records(server: &SqliteWorkspaceStore) -> Vec<(String, Vec<String>)> {
    server.with_conn(|conn| {
        let tables = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND (name LIKE 'workspace_config%' OR name LIKE 'content_%') ORDER BY name")?
            .query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        tables.into_iter().map(|table| Ok((table.clone(), table_rows(conn, &table)?))).collect()
    }).unwrap()
}

fn legacy_records(features: &FeatureFixture) -> Vec<Vec<String>> {
    features
        .database
        .try_with_connection::<_, SubjektivError>(|conn| {
            [
                "subject_permissions",
                "subject_permission_changes",
                "subject_permission_receipts",
                "worker_execution_grants",
            ]
            .into_iter()
            .map(|table| Ok(table_rows(conn, table)?))
            .collect()
        })
        .unwrap()
}

fn feature_schema(features: &FeatureFixture) -> Vec<String> {
    features
        .database
        .try_with_connection::<_, SubjektivError>(|conn| Ok(table_rows(conn, "sqlite_schema")?))
        .unwrap()
}

fn assert_existing_data_preserved(before: &WorkspaceConfigState, after: &WorkspaceConfigState) {
    let before_projection = evaluate_workspace_config_state(before, schema()).unwrap();
    let after_projection = evaluate_workspace_config_state(after, schema()).unwrap();
    for field in ["prompts"] {
        assert_eq!(
            after_projection.projections[0].data_json[field],
            before_projection.projections[0].data_json[field],
            "{field}"
        );
    }
    for file in ["existing.dcdl", "permissions/migrated.dcdl"] {
        assert_eq!(
            after.snapshot.get(&path(file)),
            before.snapshot.get(&path(file)),
            "{file}"
        );
    }
    let main = &after
        .snapshot
        .get(&path(MAIN_CONFIG_ENTRYPOINT))
        .unwrap()
        .content;
    assert!(main.contains("# Keep this owner's explanation, including the word permissions."));
    assert!(main.contains("permissions = import \"./permissions/migrated-1.dcdl\";"));
    assert!(
        after
            .snapshot
            .get(&path("permissions/migrated-1.dcdl"))
            .is_some()
    );
}

#[test]
fn migration_and_adoption_preview_preserve_existing_data_without_writing_either_store() {
    let (_root, server, features, subjects) = fixture();
    let host = host();
    let before = server.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let server_before = server_records(&server);
    let legacy_before = legacy_records(&features);
    let schema_before = feature_schema(&features);
    let seed = migration(&server, &features, &host);
    let request = migration_request(&seed, &host);
    let preview = server
        .preview_configuration_adoption(
            WORKSPACE,
            OWNER,
            &request,
            schema(),
            &host,
            Some(&baseline(&seed)),
            |subject| require_subject(&features.store, subject),
        )
        .unwrap();
    assert_eq!(preview.state, seed.state);
    assert_existing_data_preserved(&before, &preview.state);
    assert!(!seed.policies.contains_key(&subjects.unset));
    assert_eq!(
        seed.policies[&subjects.empty].change_id,
        subjects.empty_source.change_id
    );
    assert_eq!(
        seed.policies[&subjects.empty].source.execution,
        AssignmentSource::default()
    );
    assert_eq!(
        seed.policies[&subjects.empty].source.delegation,
        AssignmentSource::default()
    );
    assert_eq!(
        seed.policies[&subjects.limited].legacy_read(&host).unwrap(),
        subjects.limited_source
    );
    assert!(
        seed.diagnostics
            .iter()
            .all(|diagnostic| matches!(diagnostic, MigrationDiagnostic::DomainConstraint { .. }))
    );
    assert_eq!(seed.diagnostics.len(), 2);
    assert_eq!(
        server.load_workspace_config(WORKSPACE).unwrap(),
        Some(before)
    );
    assert_eq!(server.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert_eq!(
        server
            .configuration_adoption_result(WORKSPACE, OWNER, &request.operation_id)
            .unwrap(),
        None
    );
    assert_eq!(server_records(&server), server_before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(feature_schema(&features), schema_before);
    assert!(
        features
            .store
            .permission_transfer_receipt(&seed.snapshot.identity)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        features
            .store
            .subject_permissions(&subjects.limited)
            .unwrap(),
        Some(subjects.limited_source)
    );
}

#[test]
fn unsupported_legacy_operation_blocks_core_adoption_before_the_real_seal_boundary() {
    let (_root, server, features, subjects) = fixture();
    let host = host();
    let unsupported = limited("file.unknown");
    replace(
        &features.store,
        &subjects.limited,
        "unsupported-legacy-source",
        vec![unsupported.clone()],
        vec![limited("file.read.text")],
    );
    let before = server_records(&server);
    let legacy_before = legacy_records(&features);
    let schema_before = feature_schema(&features);
    let seed = migration(&server, &features, &host);
    assert!(
        seed.diagnostics
            .contains(&MigrationDiagnostic::ExplicitConfigurationRequired {
                subject_id: subjects.limited.clone(),
                permission: unsupported,
            })
    );
    let request = migration_request(&seed, &host);
    let error = adopt_migration(&server, &features, &host, &seed, &request).unwrap_err();
    assert!(
        matches!(error, Error::InvalidInput(ref message) if message.contains("require explicit configuration")),
        "{error}"
    );
    assert_eq!(server_records(&server), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(feature_schema(&features), schema_before);
    assert!(
        features
            .store
            .permission_transfer_receipt(&seed.snapshot.identity)
            .unwrap()
            .is_none()
    );
    // This is a pre-seal rejection, not a sealed-but-unadopted transfer.
    replace(
        &features.store,
        &subjects.limited,
        "writer-after-diagnostic",
        vec![],
        vec![],
    );
}

#[test]
fn server_failure_after_seal_rolls_back_and_reopen_resumes_only_the_exact_full_request() {
    let (root, db, features, subjects) = fixture();
    let host = host();
    let seed = migration(&db, &features, &host);
    let request = migration_request(&seed, &host);
    let before = server_records(&db);
    let legacy_before = legacy_records(&features);
    let identity = features.store.database_identity().unwrap();
    let physical_identity = features.database.identity().clone();
    let old_writer = SubjektivStore::from_database(features.database.clone()).unwrap();
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TEMP TRIGGER fail_adoption_receipt BEFORE INSERT ON workspace_configuration_receipts BEGIN SELECT RAISE(ABORT,'injected adoption receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    let error = adopt_migration(&db, &features, &host, &seed, &request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected adoption receipt failure"),
        "{error}"
    );
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(seed.original.clone())
    );
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert_eq!(
        db.configuration_adoption_result(WORKSPACE, OWNER, &request.operation_id)
            .unwrap(),
        None
    );
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(
        db.adopted_subject_policy(WORKSPACE, &subjects.limited)
            .is_err()
    );
    let fingerprint = request_fingerprint(WORKSPACE, OWNER, &request).unwrap();
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    assert_eq!(sealed.transfer_id, fingerprint);
    assert_eq!(sealed.snapshot, seed.snapshot);
    // This is the same physical gate used by legacy logical configuration writes.
    // A Server rollback must not reopen that writer or change its editing base.
    let legacy_candidate = db
        .evaluate_workspace_config_candidate_with_schema(
            WORKSPACE,
            &ConfigCommitRequest {
                base_digest: seed.original.snapshot.digest.clone(),
                changes: vec![ConfigTreeChange::Create {
                    path: path("late-legacy.txt"),
                    content_type: ConfigContentType::Text,
                    content: "must not be stored during pending cutover".into(),
                }],
                entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
            },
            schema(),
        )
        .unwrap();
    let legacy_write = features
        .store
        .with_permission_authority_guard(&identity, |authority| {
            authority.require_unsealed()?;
            Ok(db.commit_evaluated_workspace_config(WORKSPACE, &legacy_candidate))
        });
    assert!(matches!(
        legacy_write,
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert_eq!(server_records(&db), before);
    assert_eq!(
        features
            .store
            .seal_permission_sources(&identity, &fingerprint, &seed.snapshot.export_digest)
            .unwrap(),
        sealed
    );
    assert!(matches!(
        old_writer.subject_permissions(&subjects.limited),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        old_writer.bind_worker_execution_grant(&grant(
            &subjects.limited,
            &subjects.limited_source.change_id,
            "no-canonical-provider"
        )),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    for update in [
        subjects.exact_legacy_update.clone(),
        SubjectPermissionsUpdate {
            operation_id: "late-old-writer".into(),
            expected_change_id: Some(subjects.limited_source.change_id.clone()),
            execution: vec![],
            delegation: vec![],
        },
    ] {
        assert!(matches!(
            old_writer.set_subject_permissions(&subjects.limited, &update),
            Err(SubjektivError::PermissionAuthoritySealed)
        ));
        assert!(matches!(
            old_writer.set_managed_subject_permissions(&subjects.limited, &update),
            Err(SubjektivError::PermissionAuthoritySealed)
        ));
    }
    assert!(
        features
            .database
            .try_transaction::<_, SubjektivError>(|tx| {
                tx.execute(
                    "UPDATE subject_permissions SET change_id=change_id WHERE subject_id=?1",
                    [&subjects.limited],
                )?;
                Ok(())
            })
            .is_err()
    );
    drop(old_writer);
    drop(db);
    features.close();

    let db = server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&physical_identity));
    assert_eq!(features.store.database_identity().unwrap(), identity);
    let reopened_legacy_write =
        features
            .store
            .with_permission_authority_guard(&identity, |authority| {
                authority.require_unsealed()?;
                Ok(db.commit_evaluated_workspace_config(WORKSPACE, &legacy_candidate))
            });
    assert!(matches!(
        reopened_legacy_write,
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(
        features
            .store
            .seal_permission_sources(&identity, &fingerprint, &seed.snapshot.export_digest)
            .unwrap(),
        sealed
    );
    assert!(matches!(
        features.store.subject_permissions(&subjects.limited),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        features
            .store
            .set_subject_permissions(&subjects.limited, &subjects.exact_legacy_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    let resumed = migration(&db, &features, &host);
    assert_eq!(resumed.original, seed.original);
    assert_eq!(resumed.state, seed.state);
    assert_eq!(resumed.snapshot, seed.snapshot);
    assert_eq!(migration_request(&resumed, &host), request);
    let mut different_id = request.clone();
    different_id.operation_id = "not-the-pending-request".into();
    let error = adopt_migration(&db, &features, &host, &resumed, &different_id).unwrap_err();
    assert!(
        error.to_string().contains("permission transfer conflicts"),
        "{error}"
    );
    let mut different_content = request.clone();
    let existing = path("existing.dcdl");
    let entry = resumed.state.snapshot.get(&existing).unwrap();
    different_content
        .source
        .changes
        .push(ConfigTreeChange::Update {
            path: existing,
            expected_digest: entry.content_digest.clone(),
            content: EXISTING_DATA.replace("count = 7", "count = 8"),
        });
    let error = adopt_migration(&db, &features, &host, &resumed, &different_content).unwrap_err();
    assert!(
        error.to_string().contains("permission transfer conflicts"),
        "{error}"
    );
    let mut different_export = request.clone();
    different_export.expected_legacy_export = Some("another-export".into());
    assert!(adopt_migration(&db, &features, &host, &resumed, &different_export).is_err());
    assert!(matches!(
        features
            .store
            .seal_permission_sources(&identity, &fingerprint, "another-export"),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert_eq!(server_records(&db), before);
    let adopted = adopt_migration(&db, &features, &host, &resumed, &request).unwrap();
    assert_eq!(adopted.state, seed.state);
    assert_existing_data_preserved(&seed.original, &adopted.state);
    assert_eq!(
        db.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(adopted.history_head.clone())
    );
    assert_eq!(
        db.configuration_adoption_result(WORKSPACE, OWNER, &request.operation_id)
            .unwrap(),
        Some(adopted.clone())
    );
    assert_eq!(
        adopt_migration(&db, &features, &host, &resumed, &request).unwrap(),
        adopted
    );
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, &subjects.unset)
            .unwrap(),
        None
    );
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, &subjects.empty)
            .unwrap()
            .unwrap()
            .legacy_read(&host)
            .unwrap(),
        subjects.empty_source
    );
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, &subjects.limited)
            .unwrap()
            .unwrap()
            .legacy_read(&host)
            .unwrap(),
        subjects.limited_source
    );
    for grant in [&subjects.stale_grant, &subjects.live_grant] {
        assert_eq!(
            features
                .store
                .worker_execution_grant(&grant.runtime_id, &grant.worker_id)
                .unwrap(),
            Some(grant.clone())
        );
    }
    assert_eq!(legacy_records(&features), legacy_before);
}

#[test]
fn workspace_purge_removes_adopted_authority_without_reopening_sealed_legacy_writers() {
    use crate::workspace_deletion::WorkspaceDeletionStore;

    let (root, db, features, subjects) = fixture();
    let host = host();
    let seed = migration(&db, &features, &host);
    let request = migration_request(&seed, &host);
    adopt_migration(&db, &features, &host, &seed, &request).unwrap();
    let legacy_before = legacy_records(&features);
    let identity = features.store.database_identity().unwrap();
    let physical_identity = features.database.identity().clone();
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: db.clone(),
            host: self::host(),
        }));
    assert_eq!(
        canonical.subject_permissions(&subjects.limited).unwrap(),
        Some(subjects.limited_source.clone())
    );
    assert!(
        db.workspace_deletion_preflight("not-the-owner", WORKSPACE)
            .is_err()
    );
    db.with_conn(|conn| {
        conn.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('retained-other',?1,'Other','active','1','1')", [OWNER])?;
        Ok(())
    }).unwrap();
    let preflight = db.workspace_deletion_preflight(OWNER, WORKSPACE).unwrap();
    let operation = db
        .reserve_workspace_deletion(
            OWNER,
            WORKSPACE,
            &server_api::WorkspaceDeletionRequest {
                operation_id: "purge-adopted-authority".into(),
                expected_workspace_updated_at: preflight.expected_workspace_updated_at,
                confirmation: "Transfer".into(),
            },
        )
        .unwrap()
        .operation;
    db.finalize_workspace_deletion(&operation.operation_id)
        .unwrap();
    db.with_conn(|conn| {
        for table in [
            "workspace_configuration_authorities",
            "workspace_configuration_policies",
            "workspace_configuration_receipts",
        ] {
            let count: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE workspace_id=?1"),
                [WORKSPACE],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0, "purged authority must not survive in {table}");
        }
        Ok(())
    })
    .unwrap();
    assert!(canonical.subject_permissions(&subjects.limited).is_err());
    assert!(matches!(
        features
            .store
            .set_subject_permissions(&subjects.limited, &subjects.exact_legacy_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert!(matches!(
        features
            .store
            .with_permission_authority_guard(&identity, |authority| authority.require_unsealed()),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert_eq!(legacy_records(&features), legacy_before);
    drop(canonical);
    drop(db);
    features.close();

    let db = server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&physical_identity));
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed)
    );
    assert!(
        db.adopted_subject_policy(WORKSPACE, &subjects.limited)
            .is_err()
    );
    assert!(matches!(
        features
            .store
            .set_subject_permissions(&subjects.limited, &subjects.exact_legacy_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert!(matches!(
        features
            .store
            .with_permission_authority_guard(&identity, |authority| authority.require_unsealed()),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert_eq!(legacy_records(&features), legacy_before);
}

fn edit_request(
    server: &SqliteWorkspaceStore,
    host: &PermissionHost,
    id: &str,
    changes: Vec<ConfigTreeChange>,
) -> ConfigurationAdoptionRequest {
    let current = server.load_workspace_config(WORKSPACE).unwrap().unwrap();
    ConfigurationAdoptionRequest {
        operation_id: id.into(),
        expected_head: server.configuration_head(WORKSPACE, OWNER).unwrap(),
        expected_catalog_digest: catalog_digest(host).unwrap(),
        expected_legacy_export: None,
        source: ConfigCommitRequest {
            base_digest: current.snapshot.digest,
            changes,
            entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
        },
    }
}

fn adopt_edit(
    server: &SqliteWorkspaceStore,
    features: &FeatureFixture,
    host: &PermissionHost,
    request: &ConfigurationAdoptionRequest,
) -> ConfigurationAdoptionResult {
    server
        .adopt_configuration(
            WORKSPACE,
            OWNER,
            request,
            schema(),
            host,
            None,
            |subject| require_subject(&features.store, subject),
            |_| panic!("an ordinary edit must not prepare another legacy transfer"),
        )
        .unwrap()
}

#[test]
fn adopted_source_remains_editable_and_canonical_reads_cannot_widen_or_revive_old_grants() {
    let (root, server, features, subjects) = fixture();
    let host = host();
    let seed = migration(&server, &features, &host);
    let initial_request = migration_request(&seed, &host);
    let initial = adopt_migration(&server, &features, &host, &seed, &initial_request).unwrap();
    let legacy_before = legacy_records(&features);
    let identity = features.database.identity().clone();
    drop(server);
    features.close();
    let server = self::server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&identity));
    assert_eq!(
        server.load_workspace_config(WORKSPACE).unwrap(),
        Some(initial.state.clone())
    );
    assert_eq!(
        server
            .configuration_adoption_result(WORKSPACE, OWNER, &initial_request.operation_id)
            .unwrap(),
        Some(initial.clone())
    );
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(matches!(
        features.store.subject_permissions(&subjects.limited),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server.clone(),
            host: self::host(),
        }));
    assert_eq!(
        canonical.subject_permissions(&subjects.unset).unwrap(),
        None
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.empty).unwrap(),
        Some(subjects.empty_source.clone())
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.limited).unwrap(),
        Some(subjects.limited_source.clone())
    );
    let child = grant(
        &subjects.limited,
        &subjects.limited_source.change_id,
        "canonical-child",
    );
    canonical.bind_worker_execution_grant(&child).unwrap();
    let mut execution_only = grant(
        &subjects.limited,
        &subjects.limited_source.change_id,
        "execution-is-not-delegation",
    );
    execution_only.permissions = vec![limited("file.read")];
    assert!(matches!(
        canonical.bind_worker_execution_grant(&execution_only),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let mut wider = grant(
        &subjects.limited,
        &subjects.limited_source.change_id,
        "wider-child",
    );
    wider.permissions[0].resource = PermissionResource::Workdir {
        working_directory_id: "one-workdir".into(),
        path: ".".into(),
        recursive: true,
    };
    assert!(matches!(
        canonical.bind_worker_execution_grant(&wider),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        canonical.bind_worker_execution_grant(&grant(
            &subjects.limited,
            &subjects.stale_grant.source_change_id,
            "stale-source-child"
        )),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let mut wrong_workdir = child.clone();
    wrong_workdir.worker_id = "other-workdir-child".into();
    wrong_workdir.permissions[0].resource = PermissionResource::Workdir {
        working_directory_id: "another-workdir".into(),
        path: "only-this.txt".into(),
        recursive: false,
    };
    assert!(matches!(
        canonical.bind_worker_execution_grant(&wrong_workdir),
        Err(SubjektivError::AuthorizationDenied(_))
    ));

    let generated = path("permissions/migrated-1.dcdl");
    let state = server.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let entry = state.snapshot.get(&generated).unwrap();
    let revoked_source = format!(
        "{{ subjects = [{{ subject_id = {}; execution = {{ revoked = true; }}; delegation = {{ revoked = true; }}; }}]; }}\n",
        serde_json::to_string(&subjects.limited).unwrap(),
    );
    // A client-chosen operation ID equal to the old change identity must not
    // make its retained child grants current again.
    let revoke = edit_request(
        &server,
        &host,
        &subjects.limited_source.change_id,
        vec![ConfigTreeChange::Update {
            path: generated.clone(),
            expected_digest: entry.content_digest.clone(),
            content: revoked_source,
        }],
    );
    let revoked = adopt_edit(&server, &features, &host, &revoke);
    assert_ne!(revoked.history_head, initial.history_head);
    assert_existing_data_preserved(&seed.original, &revoked.state);
    let current = canonical
        .subject_permissions(&subjects.limited)
        .unwrap()
        .unwrap();
    assert_ne!(current.change_id, subjects.limited_source.change_id);
    assert_ne!(current.change_id, subjects.stale_grant.source_change_id);
    assert!(current.execution.is_empty());
    assert!(current.delegation.is_empty());
    let policy = server
        .adopted_subject_policy(WORKSPACE, &subjects.limited)
        .unwrap()
        .unwrap();
    assert_eq!(
        policy.legacy_constraints,
        Some(subjects.limited_source.clone())
    );
    assert!(policy.source.execution.revoked && policy.source.delegation.revoked);
    // Binding replays are history only; neither replay can restore current policy.
    canonical.bind_worker_execution_grant(&child).unwrap();
    canonical
        .bind_worker_execution_grant(&subjects.live_grant)
        .unwrap();
    assert!(matches!(
        canonical.bind_worker_execution_grant(&grant(
            &subjects.limited,
            &subjects.limited_source.change_id,
            "revoked-old-child"
        )),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        canonical.bind_worker_execution_grant(&grant(
            &subjects.limited,
            &current.change_id,
            "revoked-current-child"
        )),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(
        canonical
            .worker_execution_grant("runtime", "revoked-current-child")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        adopt_migration(&server, &features, &host, &seed, &initial_request).unwrap(),
        initial
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.limited).unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        server.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(revoked.history_head)
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.empty).unwrap(),
        Some(subjects.empty_source)
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.unset).unwrap(),
        None
    );
    assert_eq!(legacy_records(&features), legacy_before);
}

/// This registered component path uses real creation receipts -> virtual source
/// -> the production single adoption writer -> the canonical provider -> current
/// Feature resolution. It is not an HTTP/Runtime acceptance test.
fn enrollment_host() -> PermissionHost {
    use feature_permissions::{
        NamespacePermissions, PermissionReference, PermissionSetId, SetContribution,
    };
    let mut host = host();
    for (namespace, keys) in [
        ("chat", vec!["chat.send", "chat.read"]),
        ("ticket", vec!["ticket.assignment.read", "ticket.read"]),
    ] {
        let feature = host
            .register_feature(FeatureIdentity::parse(format!("fixture:{namespace}")).unwrap())
            .unwrap();
        let handle = host
            .bind_namespace(&feature, NamespaceId::parse(namespace).unwrap())
            .unwrap();
        let keys: Vec<_> = keys
            .into_iter()
            .map(|key| PermissionKey::parse(key).unwrap())
            .collect();
        host.declare(
            &handle,
            NamespaceDeclaration {
                permissions: keys
                    .iter()
                    .cloned()
                    .map(|key| PermissionDefinition {
                        key,
                        description: String::new(),
                        children: vec![],
                    })
                    .collect(),
                sets: vec![SetContribution {
                    set: PermissionSetId::parse(
                        server_api::authorization::SUBJECT_DEFAULT_PERMISSION_SET,
                    )
                    .unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse(namespace).unwrap(),
                        keys,
                    })],
                }],
            },
        )
        .unwrap();
    }
    host
}

fn enroll(
    server: &SqliteWorkspaceStore,
    features: &FeatureFixture,
    host: &PermissionHost,
    subject: &str,
) -> Result<ConfigurationAdoptionResult> {
    server
        .enroll_created_subject(WORKSPACE, OWNER, &features.store, subject, schema(), || {
            Ok(((), host.clone()))
        })
        .map(|result| result.expect("post-seal creation evidence"))
}

#[test]
fn postseal_creation_adopts_source_once_and_reopen_replays_never_restore_removed_policy() {
    let (root, server, features, _subjects) = fixture();
    let host = enrollment_host();
    let seed = migration(&server, &features, &host);
    let migration_request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &migration_request).unwrap();
    let created = features
        .store
        .create_subject_idempotent(
            SubjectRole::new("new").unwrap(),
            "behavior".into(),
            "new-subject",
        )
        .unwrap();
    assert!(
        server
            .adopted_subject_policy(WORKSPACE, &created.id)
            .unwrap()
            .is_none()
    );
    let before = server.configuration_current(WORKSPACE, OWNER).unwrap();
    assert!(
        server
            .enroll_created_subject(
                WORKSPACE,
                "not-owner",
                &features.store,
                &created.id,
                schema(),
                || Ok(((), host.clone()))
            )
            .is_err()
    );
    let adopted = enroll(&server, &features, &host, &created.id).unwrap();
    assert!(adopted.source_stored && adopted.policy_committed);
    let old_main = before
        .state
        .snapshot
        .get(&path(MAIN_CONFIG_ENTRYPOINT))
        .unwrap();
    let new_main = adopted
        .state
        .snapshot
        .get(&path(MAIN_CONFIG_ENTRYPOINT))
        .unwrap();
    assert_eq!(
        adopted
            .state
            .snapshot
            .get(&path("subject-enrollment-base.dcdl"))
            .unwrap()
            .content,
        old_main.content
    ); // comments/imports remain at the same virtual directory
    for (path, entry) in &before.state.snapshot.entries {
        if path.as_str() != MAIN_CONFIG_ENTRYPOINT {
            assert_eq!(adopted.state.snapshot.get(path), Some(entry));
        }
    }
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server.clone(),
            host: host.clone(),
        }));
    let policy = canonical
        .current_subject_policy(&created.id)
        .unwrap()
        .unwrap();
    assert!(policy.legacy_constraints.is_none());
    assert_eq!(
        policy.execution,
        server_api::authorization::subject_default_permission_assignment()
    );
    let resolved = host.resolve(Some(&policy.execution));
    for key in [
        "chat.send",
        "chat.read",
        "ticket.assignment.read",
        "ticket.read",
    ] {
        assert!(
            resolved
                .projection()
                .allowed
                .contains_key(&PermissionKey::parse(key).unwrap())
        );
    }
    assert!(
        host.resolve(Some(&policy.delegation))
            .projection()
            .allowed
            .is_empty()
    );
    assert_eq!(
        enroll(&server, &features, &host, &created.id).unwrap(),
        adopted
    );

    let empty_edit = edit_request(
        &server,
        &host,
        "empty-new",
        vec![ConfigTreeChange::Update {
            path: path(MAIN_CONFIG_ENTRYPOINT),
            expected_digest: new_main.content_digest.clone(),
            content: format!(
                "{{ permissions = {{ subjects = [{{ subject_id = {}; execution = {{}}; delegation = {{}}; }}]; }}; }} as WorkspaceConfigSchema",
                serde_json::to_string(&created.id).unwrap()
            ),
        }],
    );
    let empty_adoption = adopt_edit(&server, &features, &host, &empty_edit);
    let empty_policy = canonical
        .current_subject_policy(&created.id)
        .unwrap()
        .unwrap();
    assert!(empty_policy.execution.grants.is_empty());
    assert_eq!(
        enroll(&server, &features, &host, &created.id).unwrap(),
        adopted
    );
    assert_eq!(
        canonical.current_subject_policy(&created.id).unwrap(),
        Some(empty_policy)
    );

    // Remove explicitly using normal source editing/adoption, not an enrollment flag.
    let removed_main = format!(
        "{{ permissions = {{ remove_subjects = [{}]; }}; }} as WorkspaceConfigSchema",
        serde_json::to_string(&created.id).unwrap()
    );
    let remove = edit_request(
        &server,
        &host,
        "remove-new",
        vec![ConfigTreeChange::Update {
            path: path(MAIN_CONFIG_ENTRYPOINT),
            expected_digest: empty_adoption
                .state
                .snapshot
                .get(&path(MAIN_CONFIG_ENTRYPOINT))
                .unwrap()
                .content_digest
                .clone(),
            content: removed_main,
        }],
    );
    adopt_edit(&server, &features, &host, &remove);
    assert_eq!(canonical.current_subject_policy(&created.id).unwrap(), None);
    let removed_head = server.configuration_head(WORKSPACE, OWNER).unwrap();
    drop(canonical);
    let identity = features.database.identity().clone();
    features.close();
    drop(server);
    let server = self::server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&identity));
    let retried = features
        .store
        .create_subject_idempotent(
            SubjectRole::new("new").unwrap(),
            "behavior".into(),
            "new-subject",
        )
        .unwrap();
    assert_eq!(retried.id, created.id);
    assert_eq!(
        enroll(&server, &features, &host, &created.id).unwrap(),
        adopted
    ); // evidence only
    assert!(
        server
            .adopted_subject_policy(WORKSPACE, &created.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        server.configuration_head(WORKSPACE, OWNER).unwrap(),
        removed_head
    );
}

#[test]
fn enrollment_failure_is_retryable_and_explicit_empty_or_removal_before_enrollment_wins() {
    let (_root, server, features, _subjects) = fixture();
    let host = enrollment_host();
    let seed = migration(&server, &features, &host);
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    let created = features
        .store
        .create_subject(SubjectRole::new("new").unwrap())
        .unwrap();
    let before = server_records(&server);
    server.with_conn(|conn| { conn.execute_batch("CREATE TRIGGER fail_enrollment BEFORE INSERT ON workspace_configuration_receipts BEGIN SELECT RAISE(ABORT,'injected enrollment receipt failure'); END;")?; Ok(()) }).unwrap();
    assert!(enroll(&server, &features, &host, &created.id).is_err());
    assert_eq!(server_records(&server), before);
    assert!(
        features
            .store
            .subject_permission_enrollment(&created.id)
            .unwrap()
            .is_some()
    );
    server
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER fail_enrollment;")?;
            Ok(())
        })
        .unwrap();
    enroll(&server, &features, &host, &created.id).unwrap();

    for remove in [false, true] {
        let subject = features
            .store
            .create_subject(SubjectRole::new("explicit").unwrap())
            .unwrap();
        let current = server.configuration_current(WORKSPACE, OWNER).unwrap();
        let main = current
            .state
            .snapshot
            .get(&path(MAIN_CONFIG_ENTRYPOINT))
            .unwrap();
        let source = if remove {
            format!(
                "remove_subjects = [{}];",
                serde_json::to_string(&subject.id).unwrap()
            )
        } else {
            format!(
                "subjects = [{{ subject_id = {}; execution = {{}}; delegation = {{}}; }}];",
                serde_json::to_string(&subject.id).unwrap()
            )
        };
        let edit = edit_request(
            &server,
            &host,
            &format!("explicit-{remove}"),
            vec![ConfigTreeChange::Update {
                path: path(MAIN_CONFIG_ENTRYPOINT),
                expected_digest: main.content_digest.clone(),
                content: format!("{{ permissions = {{ {source} }}; }} as WorkspaceConfigSchema"),
            }],
        );
        adopt_edit(&server, &features, &host, &edit);
        let explicit = server
            .adopted_subject_policy(WORKSPACE, &subject.id)
            .unwrap();
        let result = enroll(&server, &features, &host, &subject.id).unwrap();
        assert_eq!(
            server
                .adopted_subject_policy(WORKSPACE, &subject.id)
                .unwrap(),
            explicit
        );
        assert_eq!(
            result.state,
            server.load_workspace_config(WORKSPACE).unwrap().unwrap()
        );
        if let Some(policy) = explicit {
            assert!(
                host.resolve(Some(&policy.current().execution))
                    .projection()
                    .allowed
                    .is_empty()
            );
        }
    }
}

#[test]
fn preseal_feature_defaults_transfer_as_live_sets_not_legacy_copies() {
    let (_root, server, features, _subjects) = fixture();
    let host = enrollment_host();
    let subject = features
        .store
        .create_subject(SubjectRole::new("preseal").unwrap())
        .unwrap();
    let original = features
        .store
        .current_subject_policy(&subject.id)
        .unwrap()
        .unwrap();
    assert!(
        server
            .enroll_created_subject(
                WORKSPACE,
                OWNER,
                &features.store,
                &subject.id,
                schema(),
                || Ok(((), host.clone()))
            )
            .unwrap()
            .is_none()
    );
    let seed = migration(&server, &features, &host);
    assert_eq!(seed.policies[&subject.id].current(), original);
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server,
            host,
        }));
    assert_eq!(
        canonical.current_subject_policy(&subject.id).unwrap(),
        Some(original)
    );
    assert!(canonical.subject_permissions(&subject.id).is_err()); // no all-target legacy projection
}

#[test]
fn canonical_feature_grants_bind_without_legacy_projection_and_keep_current_scope_after_reopen() {
    let (root, server, features, subjects) = fixture();
    let host = enrollment_host();
    let seed = migration(&server, &features, &host);
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    let created = features
        .store
        .create_subject(SubjectRole::new("Feature-grant fixture").unwrap())
        .unwrap();
    enroll(&server, &features, &host, &created.id).unwrap();
    let state = server.configuration_current(WORKSPACE, OWNER).unwrap();
    let main = state
        .state
        .snapshot
        .get(&path(MAIN_CONFIG_ENTRYPOINT))
        .unwrap();
    let source = format!(
        "{{ permissions = {{ subjects = [{{ subject_id = {}; execution = {{ namespaces = [{{ namespace = \"file\"; keys = [\"file.read.text\"]; }}]; }}; delegation = {{ namespaces = [{{ namespace = \"file\"; keys = [\"file.read.text\"]; }}]; }}; }}]; }}; }} as WorkspaceConfigSchema",
        serde_json::to_string(&created.id).unwrap()
    );
    let edit = edit_request(
        &server,
        &host,
        "grant-feature-delegation",
        vec![ConfigTreeChange::Update {
            path: path(MAIN_CONFIG_ENTRYPOINT),
            expected_digest: main.content_digest.clone(),
            content: source,
        }],
    );
    adopt_edit(&server, &features, &host, &edit);
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server.clone(),
            host: host.clone(),
        }));
    let current = canonical
        .current_subject_policy(&created.id)
        .unwrap()
        .unwrap();
    assert!(current.legacy_constraints.is_none());
    assert!(canonical.subject_permissions(&created.id).is_err()); // never synthesize legacy all-target vectors
    let bound = grant(&created.id, &current.change_id, "canonical-feature-child");
    canonical
        .bind_worker_execution_grant_with_host_lease(&bound, || Ok(((), host.clone())))
        .unwrap();
    assert_eq!(
        canonical
            .worker_execution_grant("runtime", "canonical-feature-child")
            .unwrap(),
        Some(bound.clone())
    );
    // Migrated policy keeps its finite domain scope as a separate conjunct too.
    let bounded = grant(
        &subjects.limited,
        &subjects.limited_source.change_id,
        "canonical-limited-child-v2",
    );
    canonical
        .bind_worker_execution_grant_with_host_lease(&bounded, || Ok(((), host.clone())))
        .unwrap();
    let mut wider = bounded.clone();
    wider.worker_id = "wider-canonical-limits".into();
    wider.permissions[0].resource = PermissionResource::Workdir {
        working_directory_id: "another-workdir".into(),
        path: "only-this.txt".into(),
        recursive: false,
    };
    assert!(
        canonical
            .bind_worker_execution_grant_with_host_lease(&wider, || Ok(((), host.clone())))
            .is_err()
    );
    let current = server.configuration_current(WORKSPACE, OWNER).unwrap();
    let main = current
        .state
        .snapshot
        .get(&path(MAIN_CONFIG_ENTRYPOINT))
        .unwrap();
    let revoke = edit_request(
        &server,
        &host,
        "revoke-feature-delegation",
        vec![ConfigTreeChange::Update {
            path: path(MAIN_CONFIG_ENTRYPOINT),
            expected_digest: main.content_digest.clone(),
            content: format!(
                "{{ permissions = {{ remove_subjects = [{}]; }}; }} as WorkspaceConfigSchema",
                serde_json::to_string(&created.id).unwrap()
            ),
        }],
    );
    adopt_edit(&server, &features, &host, &revoke);
    let mut late = bound.clone();
    late.worker_id = "removed-source-child".into();
    assert!(
        canonical
            .bind_worker_execution_grant_with_host_lease(&late, || Ok(((), host.clone())))
            .is_err()
    );
    canonical
        .bind_worker_execution_grant_with_host_lease(&bound, || Ok(((), host.clone())))
        .unwrap(); // retained receipt only
    assert!(
        canonical
            .current_subject_policy(&created.id)
            .unwrap()
            .is_none()
    );
    drop(canonical);
    let identity = features.database.identity().clone();
    features.close();
    drop(server);
    let server = self::server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&identity));
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server,
            host: host.clone(),
        }));
    canonical
        .bind_worker_execution_grant_with_host_lease(&bound, || Ok(((), host.clone())))
        .unwrap();
    assert!(
        canonical
            .bind_worker_execution_grant_with_host_lease(&late, || Ok(((), host.clone())))
            .is_err()
    );
    assert!(
        canonical
            .current_subject_policy(&created.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_unicode_subject_identity_migrates_through_virtual_source_and_reopen_without_corruption() {
    let (root, server, features, _subjects) = fixture();
    let host = enrollment_host();
    let subject = "legacy-日本語😀";
    // Historical data fixture: a real persisted Subject predating creation
    // enrollment, rather than changing today's generated Subject ID contract.
    let record = serde_json::json!({ "schema_version": 1, "id": subject, "role": "Unicode fixture", "behavior_md": "", "state": "active", "created_at": "1", "updated_at": "1" });
    features.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute("INSERT INTO subjects(subject_id,role,behavior_md,state,record_json,created_at,updated_at) VALUES(?1,'Unicode fixture','','active',?2,'1','1')", rusqlite::params![subject, record.to_string()])?;
        Ok(())
    }).unwrap();
    let (_, original) = replace(
        &features.store,
        subject,
        "unicode-limited-source",
        vec![limited("file.read")],
        vec![limited("file.read.text")],
    );
    let seed = migration(&server, &features, &host);
    assert_eq!(seed.policies[subject].source.subject_id, subject);
    assert!(
        seed.state
            .snapshot
            .entries
            .values()
            .any(|entry| entry.content_type == ConfigContentType::Text && entry.content == subject)
    );
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    assert_eq!(
        server
            .adopted_subject_policy(WORKSPACE, subject)
            .unwrap()
            .unwrap()
            .legacy_read(&host)
            .unwrap(),
        original
    );
    let identity = features.database.identity().clone();
    features.close();
    drop(server);
    let server = self::server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&identity));
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server.clone(),
            host: host.clone(),
        }));
    let current = canonical.current_subject_policy(subject).unwrap().unwrap();
    assert_eq!(current.change_id, original.change_id);
    assert_eq!(current.legacy_constraints, Some(original.clone()));
    let child = grant(subject, &current.change_id, "unicode-canonical-child");
    canonical
        .bind_worker_execution_grant_with_host_lease(&child, || Ok(((), host.clone())))
        .unwrap();
    assert_eq!(
        canonical.subject_permissions(subject).unwrap(),
        Some(original)
    );
}

// These leases observe committed records from a separate connection, not the
// writer's uncommitted view. Catalog release before COMMIT must fail the check.
struct AfterCommit<F: FnMut()>(F);
impl<F: FnMut()> Drop for AfterCommit<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[test]
fn deferred_binding_catalog_lease_outlives_native_commit() {
    let (root, server, features, subjects) = fixture();
    let host = host();
    let seed = migration(&server, &features, &host);
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    let canonical = features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(CanonicalConfigurationSource {
            store: server,
            host: host.clone(),
        }));
    let observer = FeatureFixture::open(
        &root.path().join("features"),
        Some(features.database.identity()),
    );
    let grant = grant(
        &subjects.limited,
        &subjects.limited_source.change_id,
        "leased-binding",
    );
    let catalog = std::sync::RwLock::new(host);
    let released = std::cell::Cell::new(false);
    canonical
        .bind_worker_execution_grant_with_host_lease(&grant, || {
            let registry = catalog.read().unwrap();
            let host = registry.clone();
            let committed = AfterCommit(|| {
                assert!(
                    catalog.try_write().is_err(),
                    "catalog still fenced at commit observation"
                );
                assert_eq!(
                    observer
                        .store
                        .worker_execution_grant(&grant.runtime_id, &grant.worker_id)
                        .unwrap(),
                    Some(grant.clone())
                );
                released.set(true);
            });
            // Tuple field order keeps the registry held during the commit observation.
            Ok(((committed, registry), host))
        })
        .unwrap();
    assert!(released.get(), "owner released admission after COMMIT");
    assert!(catalog.try_write().is_ok());
}

#[test]
fn deferred_enrollment_catalog_lease_outlives_adoption_commit() {
    let (_root, server, features, _subjects) = fixture();
    let host = enrollment_host();
    let seed = migration(&server, &features, &host);
    let request = migration_request(&seed, &host);
    adopt_migration(&server, &features, &host, &seed, &request).unwrap();
    let subject = features
        .store
        .create_subject_idempotent(
            SubjectRole::new("leased-enrollment").unwrap(),
            "behavior".into(),
            "leased-enrollment",
        )
        .unwrap();
    let catalog = std::sync::RwLock::new(host);
    let released = std::cell::Cell::new(false);
    let result = server
        .enroll_created_subject(
            WORKSPACE,
            OWNER,
            &features.store,
            &subject.id,
            schema(),
            || {
                let registry = catalog.read().unwrap();
                let host = registry.clone();
                let committed = AfterCommit(|| {
                    assert!(
                        catalog.try_write().is_err(),
                        "catalog still fenced at commit observation"
                    );
                    let receipt = server
                        .configuration_adoption_result(
                            WORKSPACE,
                            OWNER,
                            &format!("subject-enrollment:{}", subject.id),
                        )
                        .unwrap()
                        .expect("adoption receipt committed before catalog release");
                    assert!(receipt.source_stored && receipt.policy_committed);
                    assert!(
                        server
                            .adopted_subject_policy(WORKSPACE, &subject.id)
                            .unwrap()
                            .is_some()
                    );
                    released.set(true);
                });
                Ok(((committed, registry), host))
            },
        )
        .unwrap()
        .unwrap();
    assert!(result.policy_committed);
    assert!(released.get(), "owner released catalog after Server COMMIT");
    assert!(catalog.try_write().is_ok());
}
