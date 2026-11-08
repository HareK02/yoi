//! Config adoption -> SQLite reopen -> current catalog -> human creation selection
//! component coverage. Real schema evaluation, resolver and storage are used;
//! identity creation and Runtime dispatch belong to the Server connected suites.

use super::*;
use crate::config_source::{MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider};
use crate::policy_config::PermissionConfigSchemaProvider;
use config_source::{ConfigTreeChange, VirtualPath};
use feature_permissions::*;

const WORKSPACE: &str = "human-config-workspace";
const OWNER: &str = "human-config-owner";
const SELECTED: &str = r#"{ permissions = {
    sets = [{ id = "research"; sets = ["ticket"]; }];
    human_worker_default = { sets = ["research"]; };
}; } as WorkspaceConfigSchema"#;

fn host() -> PermissionHost {
    let mut host = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
    let feature = host
        .register_feature(FeatureIdentity::parse("builtin:ticket").unwrap())
        .unwrap();
    let namespace = host
        .bind_namespace(&feature, NamespaceId::parse("ticket").unwrap())
        .unwrap();
    host.declare(
        &namespace,
        NamespaceDeclaration {
            permissions: ["ticket.read", "ticket.write"]
                .into_iter()
                .map(|key| PermissionDefinition {
                    key: PermissionKey::parse(key).unwrap(),
                    description: String::new(),
                    children: vec![],
                })
                .collect(),
            sets: vec![SetContribution {
                set: PermissionSetId::parse("ticket").unwrap(),
                children: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: NamespaceId::parse("ticket").unwrap(),
                    keys: vec![PermissionKey::parse("ticket.read").unwrap()],
                })],
            }],
        },
    )
    .unwrap();
    host
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
fn fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    store.with_immediate_transaction(|transaction| {
        transaction.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('human-config-owner','user','human-config-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('human-config-workspace','human-config-owner','Config','active','1','1');")?;
        Ok(())
    }).unwrap();
    store
        .ensure_workspace_config_materialized_with_schema(WORKSPACE, "now", schema())
        .unwrap();
    (directory, store)
}
fn request(
    store: &SqliteWorkspaceStore,
    host: &PermissionHost,
    id: &str,
    source: &str,
) -> ConfigurationAdoptionRequest {
    let current = store.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let path = VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap();
    let expected_head = store.configuration_head(WORKSPACE, OWNER).unwrap();
    ConfigurationAdoptionRequest {
        operation_id: id.into(),
        expected_legacy_export: expected_head.is_none().then(|| "empty-export".into()),
        expected_head,
        expected_catalog_digest: catalog_digest(host).unwrap(),
        source: ConfigCommitRequest {
            base_digest: current.snapshot.digest.clone(),
            changes: vec![ConfigTreeChange::Update {
                expected_digest: current.snapshot.get(&path).unwrap().content_digest.clone(),
                path: path.clone(),
                content: source.into(),
            }],
            entrypoints: vec![path],
        },
    }
}
fn adopt(
    store: &SqliteWorkspaceStore,
    host: &PermissionHost,
    request: &ConfigurationAdoptionRequest,
) -> Result<ConfigurationAdoptionResult> {
    store.adopt_configuration(
        WORKSPACE,
        OWNER,
        request,
        schema(),
        host,
        None,
        |_| Ok(()),
        |fingerprint| {
            Ok(ConfigurationPolicyTransfer {
                transfer_identity: fingerprint.into(),
                physical_identity: "empty-physical".into(),
                export_digest: "empty-export".into(),
                policies: BTreeMap::new(),
            })
        },
    )
}
fn keys(host: &PermissionHost, assignment: &PermissionAssignment) -> Vec<String> {
    host.resolve(Some(assignment))
        .projection()
        .allowed
        .keys()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn adopted_default_reopens_as_references_and_explicit_inputs_replace_without_merging() {
    let (directory, store) = fixture();
    let host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    drop(store);
    let reopened =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    let (configured, selected) = reopened
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    assert_eq!(
        selected.grants,
        vec![PermissionReference::Set(
            PermissionSetId::parse("research").unwrap()
        )]
    );
    assert_eq!(keys(&configured, &selected), ["ticket.read"]);
    let empty = PermissionAssignment::default();
    let (_, selected_empty) = reopened
        .human_worker_permissions(WORKSPACE, &host, Some(&empty))
        .unwrap();
    assert_eq!(selected_empty, empty);
    let explicit = PermissionAssignment {
        grants: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![PermissionKey::parse("ticket.write").unwrap()],
        })],
        ..Default::default()
    };
    let (_, selected_explicit) = reopened
        .human_worker_permissions(WORKSPACE, &host, Some(&explicit))
        .unwrap();
    assert_eq!(selected_explicit, explicit);
    assert_eq!(keys(&configured, &selected_explicit), ["ticket.write"]);
    for revoked in [false, true] {
        let unknown = PermissionAssignment {
            grants: vec![PermissionReference::Set(
                PermissionSetId::parse("unknown").unwrap(),
            )],
            revoked,
            ..Default::default()
        };
        assert!(
            reopened
                .human_worker_permissions(WORKSPACE, &host, Some(&unknown))
                .is_err()
        );
    }
}

#[test]
fn subject_and_human_assignments_resolve_candidate_sets_with_exclusions() {
    let (_directory, store) = fixture();
    let host = host();
    let source = r#"{ permissions = {
        sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.write"]; }]; }];
        subjects = [{ subject_id = "subject-a"; execution = { sets = ["research"]; exclude_sets = ["ticket"]; }; }];
        human_worker_default = { sets = ["research"]; exclude_namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; };
    }; } as WorkspaceConfigSchema"#;
    let candidate = request(&store, &host, "with-subject", source);
    store
        .preview_configuration_adoption(WORKSPACE, OWNER, &candidate, schema(), &host, None, |_| {
            Ok(())
        })
        .unwrap();
    adopt(&store, &host, &candidate).unwrap();
    let (configured, selected) = store
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    assert_eq!(keys(&configured, &selected), ["ticket.write"]);
    let subject = store
        .adopted_subject_policy(WORKSPACE, "subject-a")
        .unwrap()
        .unwrap();
    assert_eq!(
        keys(&configured, &subject.current().execution),
        ["ticket.write"]
    );
    assert!(
        store
            .adopted_subject_legacy_permissions(WORKSPACE, "subject-a", &host)
            .is_err(),
        "new keys must not become all-target legacy operations"
    );
}

#[test]
fn default_changes_only_new_selection_while_saved_human_sets_remain_dynamic() {
    let (_directory, store) = fixture();
    let host = host();
    let initial = request(&store, &host, "initial", SELECTED);
    let first_receipt = adopt(&store, &host, &initial).unwrap();
    let (old_host, saved) = store
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    let expanded = r#"{ permissions = {
        sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.write"]; }]; }];
        human_worker_default = {};
    }; } as WorkspaceConfigSchema"#;
    adopt(
        &store,
        &old_host,
        &request(&store, &old_host, "expanded", expanded),
    )
    .unwrap();
    let (current_host, new_selection) = store
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    assert_eq!(new_selection, PermissionAssignment::default());
    assert_eq!(keys(&current_host, &saved), ["ticket.read", "ticket.write"]);
    assert_eq!(adopt(&store, &host, &initial).unwrap(), first_receipt);
    let after_replay = store.configured_permission_host(WORKSPACE, &host).unwrap();
    assert_eq!(keys(&after_replay, &saved), ["ticket.read", "ticket.write"]);
    let shrunk = r#"{ permissions = {
        sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.write"]; }]; }];
    }; } as WorkspaceConfigSchema"#;
    adopt(
        &store,
        &current_host,
        &request(&store, &current_host, "shrunk", shrunk),
    )
    .unwrap();
    let shrunk_host = store
        .configured_permission_host(WORKSPACE, &current_host)
        .unwrap();
    assert_eq!(keys(&shrunk_host, &saved), ["ticket.write"]);
    adopt(
        &store,
        &shrunk_host,
        &request(
            &store,
            &shrunk_host,
            "removed",
            "{} as WorkspaceConfigSchema",
        ),
    )
    .unwrap();
    let removed_host = store
        .configured_permission_host(WORKSPACE, &shrunk_host)
        .unwrap();
    let resolution = removed_host.resolve(Some(&saved));
    assert!(resolution.projection().allowed.is_empty());
    assert!(
        resolution
            .projection()
            .diagnostics
            .contains(&PermissionDiagnostic::UnknownSet(
                PermissionSetId::parse("research").unwrap()
            ))
    );
}

#[test]
fn invalid_sets_defaults_and_schema_do_not_change_adopted_source_or_receipts() {
    let (_directory, store) = fixture();
    let host = host();
    let initial = adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    for (index, source) in [
        r#"{ permissions.sets = [{ id = "a"; sets = ["missing"]; }]; } as WorkspaceConfigSchema"#,
        r#"{ permissions.sets = [{ id = "a"; sets = ["b"]; }, { id = "b"; sets = ["a"]; }]; } as WorkspaceConfigSchema"#,
        r#"{ permissions.sets = [{ id = "a"; }, { id = "a"; }]; } as WorkspaceConfigSchema"#,
        r#"{ permissions.sets = [{ id = "ticket"; }]; } as WorkspaceConfigSchema"#,
        r#"{ permissions.sets = [{ id = "a"; namespaces = [{ namespace = "ticket"; keys = ["ticket.unknown"]; }]; }]; } as WorkspaceConfigSchema"#,
        r#"{ permissions.human_worker_default = { sets = ["missing"]; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.human_worker_default = { sets = ["missing"]; revoked = true; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.human_worker_default = { revoked = "yes"; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.human_worker_default = { policy = "all"; }; } as WorkspaceConfigSchema"#,
    ].into_iter().enumerate() {
        let id = format!("invalid-{index}");
        let candidate = request(&store, &host, &id, source);
        assert!(adopt(&store, &host, &candidate).is_err(), "{source}");
        assert_eq!(store.load_workspace_config(WORKSPACE).unwrap(), Some(initial.state.clone()));
        assert_eq!(store.configuration_head(WORKSPACE, OWNER).unwrap(), Some(initial.history_head.clone()));
        assert!(store.configuration_adoption_result(WORKSPACE, OWNER, &id).unwrap().is_none());
    }
}

#[test]
fn source_write_failure_keeps_old_sets_and_default_after_reopen() {
    let (directory, store) = fixture();
    let host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    let change = request(&store, &host, "failed", "{} as WorkspaceConfigSchema");
    store.with_conn(|connection| {
        connection.execute_batch("CREATE TEMP TRIGGER injected_failure BEFORE INSERT ON workspace_configuration_receipts BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        Ok(())
    }).unwrap();
    assert!(adopt(&store, &host, &change).is_err());
    drop(store);
    let reopened =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    let (configured, selection) = reopened
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    assert_eq!(keys(&configured, &selection), ["ticket.read"]);
    assert!(
        reopened
            .configuration_adoption_result(WORKSPACE, OWNER, "failed")
            .unwrap()
            .is_none()
    );
    adopt(&reopened, &host, &change).unwrap();
    let (_, empty) = reopened
        .human_worker_permissions(WORKSPACE, &host, None)
        .unwrap();
    assert_eq!(empty, PermissionAssignment::default());
}

#[test]
fn unavailable_source_unadopted_workspace_and_cross_workspace_never_fallback() {
    let (_directory, store) = fixture();
    let host = host();
    let empty = PermissionAssignment::default();
    assert!(
        store
            .human_worker_permissions(WORKSPACE, &host, Some(&empty))
            .is_err()
    );
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    let foreign = PermissionHost::new(WorkspaceId::parse("other-workspace").unwrap());
    assert!(
        store
            .configured_permission_host(WORKSPACE, &foreign)
            .is_err()
    );
    store
        .with_conn(|connection| {
            connection.execute(
                "UPDATE workspace_config_entries SET content='{}' WHERE workspace_id=?1",
                [WORKSPACE],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(store.configured_permission_host(WORKSPACE, &host).is_err());
    assert!(
        store
            .human_worker_permissions(WORKSPACE, &host, Some(&empty))
            .is_err()
    );
}

#[test]
fn disabled_feature_withdraws_operations_without_losing_selected_reference() {
    let (_directory, store) = fixture();
    let mut host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    let selected = store
        .human_worker_assignment(WORKSPACE, &host, None)
        .unwrap();
    // Reconstruct a Feature lifecycle change in the trusted loader, retaining the
    // Workspace source. Reinstallation here intentionally gets a new registration.
    let mut disabled = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
    let feature = disabled
        .register_feature(FeatureIdentity::parse("builtin:ticket").unwrap())
        .unwrap();
    let namespace = disabled
        .bind_namespace(&feature, NamespaceId::parse("ticket").unwrap())
        .unwrap();
    disabled
        .declare(&namespace, host.catalog().namespaces.remove(0).declaration)
        .unwrap();
    disabled.set_enabled(&feature, false).unwrap();
    host = store
        .configured_permission_host(WORKSPACE, &disabled)
        .unwrap();
    let resolution = host.resolve(Some(&selected));
    assert!(resolution.projection().allowed.is_empty());
    assert!(
        resolution
            .projection()
            .diagnostics
            .contains(&PermissionDiagnostic::DisabledFeature(
                FeatureIdentity::parse("builtin:ticket").unwrap()
            ))
    );
}
