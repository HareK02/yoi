//! Materialized source (with or without adoption) -> SQLite reopen -> current
//! catalog -> caller-neutral creation selection component coverage.
//! Real schema evaluation, resolver and storage are used;
//! identity creation and Runtime dispatch belong to the Server connected suites.

use super::*;
use crate::config_source::{MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider};
use crate::policy_config::PermissionConfigSchemaProvider;
use config_source::{ConfigTreeChange, VirtualPath};
use feature_permissions::*;
use server_api::subject_policy::{SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation};

const WORKSPACE: &str = "human-config-workspace";
const OWNER: &str = "human-config-owner";
const SELECTED: &str = r#"{ permissions = {
    sets = [{ id = "research"; sets = ["ticket"]; }];
    worker_default = { sets = ["research"]; };
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
            sets: ["ticket", "implementer"]
                .into_iter()
                .map(|set| SetContribution {
                    set: PermissionSetId::parse(set).unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse("ticket").unwrap(),
                        keys: vec![PermissionKey::parse("ticket.read").unwrap()],
                    })],
                })
                .collect(),
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

fn commit_unadopted_source(store: &SqliteWorkspaceStore, host: &PermissionHost, source: &str) {
    let candidate = store
        .evaluate_workspace_config_candidate_with_schema(
            WORKSPACE,
            &request(store, host, "unadopted", source).source,
            schema(),
        )
        .unwrap();
    store
        .commit_evaluated_workspace_config(WORKSPACE, &candidate)
        .unwrap();
}

fn assert_unadopted(store: &SqliteWorkspaceStore) {
    let counts: (i64, i64, i64) = store
        .with_conn(|connection| {
            connection
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM workspace_configuration_authorities WHERE workspace_id=?1),
                        (SELECT COUNT(*) FROM workspace_configuration_policies WHERE workspace_id=?1),
                        (SELECT COUNT(*) FROM workspace_configuration_receipts WHERE workspace_id=?1)",
                    [WORKSPACE],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(Error::from)
        })
        .unwrap();
    assert_eq!(counts, (0, 0, 0));
    assert_eq!(store.configuration_head(WORKSPACE, OWNER).unwrap(), None);
}

#[test]
fn permission_reads_reuse_native_authority_reservation_without_releasing_or_writing_it() {
    let (directory, store) = fixture();
    let host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    let before = store.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let managed =
        SqliteWorkspaceStore::connect_prepared_fixture(&directory.path().join("server.sqlite"))
            .unwrap();
    let reserved = managed.subject_authority_reader().unwrap();
    let configured = reserved
        .configured_permission_host(WORKSPACE, &host)
        .unwrap();
    let (selected_host, assignment) = reserved.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(keys(&configured, &assignment), vec!["ticket.read"]);
    assert_eq!(keys(&selected_host, &assignment), vec!["ticket.read"]);
    assert_eq!(
        reserved
            .adopted_subject_legacy_permissions(WORKSPACE, "unset", &host)
            .unwrap(),
        None
    );
    reserved
        .with_conn(|connection| {
            assert!(!connection.is_autocommit());
            assert!(
                connection
                    .execute(
                        "UPDATE workspaces SET display_name='forbidden' WHERE workspace_id=?1",
                        [WORKSPACE]
                    )
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    drop(reserved);
    assert_eq!(
        managed.load_workspace_config(WORKSPACE).unwrap().unwrap(),
        before
    );
    assert_eq!(
        keys(
            &managed
                .configured_permission_host(WORKSPACE, &host)
                .unwrap(),
            &assignment
        ),
        vec!["ticket.read"]
    );
}

#[test]
fn unadopted_missing_default_selects_implementer_but_explicit_empty_stays_empty() {
    let (_directory, store) = fixture();
    let host = host();
    let before = store.load_workspace_config(WORKSPACE).unwrap();
    assert_unadopted(&store);
    let (configured, selected) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(
        selected,
        policy_config::PermissionSource::default()
            .worker_default
            .assignment()
    );
    assert_eq!(keys(&configured, &selected), ["ticket.read"]);
    let empty = PermissionAssignment::default();
    assert_eq!(
        store
            .worker_assignment(WORKSPACE, &host, Some(&empty))
            .unwrap(),
        empty
    );
    assert!(matches!(
        store.adopted_subject_legacy_permissions(WORKSPACE, "subject-a", &host),
        Err(Error::InvalidInput(message)) if message == "Workspace policy transfer is not adopted"
    ));
    assert_eq!(store.load_workspace_config(WORKSPACE).unwrap(), before);
    assert_unadopted(&store);
}

#[test]
fn unadopted_default_reopens_and_explicit_inputs_replace_without_policy_transfer() {
    let (directory, store) = fixture();
    let host = host();
    commit_unadopted_source(&store, &host, SELECTED);
    assert_unadopted(&store);
    drop(store);
    let reopened =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    let before = reopened.load_workspace_config(WORKSPACE).unwrap();
    let (configured, saved) = reopened.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(
        saved.grants,
        vec![PermissionReference::Set(
            PermissionSetId::parse("research").unwrap()
        )]
    );
    assert_eq!(keys(&configured, &saved), ["ticket.read"]);
    let empty = PermissionAssignment::default();
    assert_eq!(
        reopened
            .worker_assignment(WORKSPACE, &host, Some(&empty))
            .unwrap(),
        empty
    );
    let explicit = PermissionAssignment {
        grants: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![
                PermissionKey::parse("ticket.read").unwrap(),
                PermissionKey::parse("ticket.write").unwrap(),
            ],
        })],
        exclusions: saved.grants.clone(),
        revoked: false,
    };
    for revoked in [false, true] {
        let explicit = PermissionAssignment {
            revoked,
            ..explicit.clone()
        };
        let (current, selected) = reopened
            .worker_permissions(WORKSPACE, &host, Some(&explicit))
            .unwrap();
        assert_eq!(selected, explicit);
        if revoked {
            assert!(keys(&current, &selected).is_empty());
        } else {
            assert_eq!(keys(&current, &selected), ["ticket.write"]);
        }
        let unknown = PermissionAssignment {
            grants: vec![PermissionReference::Set(
                PermissionSetId::parse("unknown").unwrap(),
            )],
            revoked,
            ..Default::default()
        };
        assert!(
            reopened
                .worker_assignment(WORKSPACE, &host, Some(&unknown))
                .is_err()
        );
    }
    assert_eq!(reopened.load_workspace_config(WORKSPACE).unwrap(), before);
    assert_unadopted(&reopened);

    commit_unadopted_source(
        &reopened,
        &host,
        r#"{ permissions = {
            sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.write"]; }]; }];
        }; } as WorkspaceConfigSchema"#,
    );
    let after = reopened.load_workspace_config(WORKSPACE).unwrap();
    let (current, selected) = reopened.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(
        selected,
        policy_config::PermissionSource::default()
            .worker_default
            .assignment()
    );
    assert_eq!(keys(&current, &saved), ["ticket.read", "ticket.write"]);
    assert_eq!(keys(&configured, &saved), ["ticket.read"]);
    assert_eq!(reopened.load_workspace_config(WORKSPACE).unwrap(), after);
    assert_unadopted(&reopened);
}

#[test]
fn adopted_default_reopens_as_references_and_explicit_inputs_replace_without_merging() {
    let (directory, store) = fixture();
    let host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    drop(store);
    let reopened =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    let (configured, selected) = reopened.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(
        selected.grants,
        vec![PermissionReference::Set(
            PermissionSetId::parse("research").unwrap()
        )]
    );
    assert_eq!(keys(&configured, &selected), ["ticket.read"]);
    let empty = PermissionAssignment::default();
    let (_, selected_empty) = reopened
        .worker_permissions(WORKSPACE, &host, Some(&empty))
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
        .worker_permissions(WORKSPACE, &host, Some(&explicit))
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
                .worker_permissions(WORKSPACE, &host, Some(&unknown))
                .is_err()
        );
    }
}

#[test]
fn subject_and_worker_assignments_resolve_candidate_sets_with_exclusions() {
    let (directory, store) = fixture();
    let host = host();
    let source = r#"{ permissions = {
        sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.write"]; }]; }];
        worker_default = { sets = ["research"]; exclude_namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; };
    }; } as WorkspaceConfigSchema"#;
    let candidate = request(&store, &host, "shared-sets", source);
    let preview = store
        .preview_configuration_adoption(WORKSPACE, OWNER, &candidate, schema(), &host, None, |_| {
            panic!("shared configuration must not resolve individual Subjects")
        })
        .unwrap();
    assert!(preview.changed_subjects.is_empty());
    let receipt = adopt(&store, &host, &candidate).unwrap();
    let (expected_change_id, previous) = store
        .subject_permission_policy_state(WORKSPACE, "subject-a")
        .unwrap();
    assert!(previous.is_none());
    let research = PermissionReference::Set(PermissionSetId::parse("research").unwrap());
    let execution = PermissionAssignment {
        grants: vec![research.clone()],
        exclusions: vec![PermissionReference::Set(
            PermissionSetId::parse("ticket").unwrap(),
        )],
        revoked: false,
    };
    let delegation = PermissionAssignment {
        grants: vec![research.clone()],
        exclusions: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![PermissionKey::parse("ticket.write").unwrap()],
        })],
        revoked: false,
    };
    let mutation = SubjectPermissionPolicyMutation {
        operation_id: "subject-policy".into(),
        expected_change_id,
        action: SubjectPermissionPolicyAction::Replace {
            execution: execution.clone(),
            delegation: delegation.clone(),
        },
    };
    let applied = store
        .mutate_subject_permission_policy(WORKSPACE, OWNER, "subject-a", &mutation, &host)
        .unwrap();
    let subject = applied.policy.clone().unwrap();
    assert_eq!(subject.execution, execution);
    assert_eq!(subject.delegation, delegation);
    assert_eq!(applied.change_id.as_ref(), Some(&subject.change_id));
    assert_eq!(
        store.load_workspace_config(WORKSPACE).unwrap(),
        Some(receipt.state.clone())
    );
    assert_eq!(
        store.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(receipt.history_head.clone())
    );
    drop(store);
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    let (configured, selected) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(selected.grants, vec![research]);
    assert_eq!(
        selected.exclusions,
        vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![PermissionKey::parse("ticket.read").unwrap()],
        })]
    );
    assert_eq!(keys(&configured, &selected), ["ticket.write"]);
    assert_eq!(
        store
            .current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap(),
        Some(subject.clone())
    );
    assert_eq!(keys(&configured, &subject.execution), ["ticket.write"]);
    assert_eq!(keys(&configured, &subject.delegation), ["ticket.read"]);
    assert_eq!(
        store
            .mutate_subject_permission_policy(WORKSPACE, OWNER, "subject-a", &mutation, &host)
            .unwrap(),
        applied
    );
    assert_eq!(adopt(&store, &host, &candidate).unwrap(), receipt);
    assert_eq!(
        store
            .current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap(),
        Some(subject)
    );
    assert!(
        store
            .adopted_subject_legacy_permissions(WORKSPACE, "subject-a", &host)
            .is_err(),
        "new keys must not become all-target legacy operations"
    );
}

#[test]
fn default_changes_only_new_selection_while_saved_worker_sets_remain_dynamic() {
    let (_directory, store) = fixture();
    let host = host();
    let initial = request(&store, &host, "initial", SELECTED);
    let first_receipt = adopt(&store, &host, &initial).unwrap();
    let (old_host, saved) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
    let expanded = r#"{ permissions = {
        sets = [{ id = "research"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.write"]; }]; }];
        worker_default = {};
    }; } as WorkspaceConfigSchema"#;
    adopt(
        &store,
        &old_host,
        &request(&store, &old_host, "expanded", expanded),
    )
    .unwrap();
    let (current_host, new_selection) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
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
        r#"{ permissions.worker_default = { sets = ["missing"]; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.worker_default = { sets = ["missing"]; revoked = true; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.worker_default = { revoked = "yes"; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.worker_default = { policy = "all"; }; } as WorkspaceConfigSchema"#,
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
    let (configured, selection) = reopened.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(keys(&configured, &selection), ["ticket.read"]);
    assert!(
        reopened
            .configuration_adoption_result(WORKSPACE, OWNER, "failed")
            .unwrap()
            .is_none()
    );
    adopt(&reopened, &host, &change).unwrap();
    let (_, selected) = reopened.worker_permissions(WORKSPACE, &host, None).unwrap();
    assert_eq!(
        selected,
        policy_config::PermissionSource::default()
            .worker_default
            .assignment()
    );
}

#[test]
fn unadopted_unavailable_source_and_cross_workspace_never_fallback() {
    let (_directory, store) = fixture();
    let host = host();
    let empty = PermissionAssignment::default();
    assert_eq!(
        store
            .worker_assignment(WORKSPACE, &host, Some(&empty))
            .unwrap(),
        empty
    );
    commit_unadopted_source(&store, &host, SELECTED);
    let foreign = PermissionHost::new(WorkspaceId::parse("other-workspace").unwrap());
    assert!(matches!(
        store.configured_permission_host(WORKSPACE, &foreign),
        Err(Error::WorkspaceIdMismatch)
    ));
    assert!(matches!(
        store.worker_permissions(WORKSPACE, &foreign, Some(&empty)),
        Err(Error::WorkspaceIdMismatch)
    ));
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
            .worker_permissions(WORKSPACE, &host, Some(&empty))
            .is_err()
    );
    assert_unadopted(&store);
}

#[test]
fn unadopted_missing_source_or_invalid_evaluation_identity_rejects_explicit_empty() {
    for corruption in [
        "DELETE FROM workspace_config_trees WHERE workspace_id=?1",
        "UPDATE workspace_config_trees SET toolchain_fingerprint='invalid' WHERE workspace_id=?1",
        "UPDATE workspace_config_trees SET projection_digest='invalid' WHERE workspace_id=?1",
        "UPDATE workspace_config_trees SET schema_bundle_json='{}' WHERE workspace_id=?1",
    ] {
        let (_directory, store) = fixture();
        let host = host();
        store
            .with_conn(|connection| {
                connection.execute(corruption, [WORKSPACE])?;
                Ok(())
            })
            .unwrap();
        assert!(
            store.configured_permission_host(WORKSPACE, &host).is_err(),
            "{corruption}"
        );
        assert!(
            store
                .worker_permissions(WORKSPACE, &host, Some(&PermissionAssignment::default()))
                .is_err(),
            "{corruption}"
        );
        assert_unadopted(&store);
    }
}

#[test]
fn unadopted_unresolved_defaults_reject_even_explicit_empty_without_adoption() {
    for source in [
        r#"{ permissions.worker_default = { sets = ["missing"]; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.worker_default = { exclude_sets = ["missing"]; revoked = true; }; } as WorkspaceConfigSchema"#,
        r#"{ permissions.sets = [{ id = "research"; sets = ["missing"]; }]; } as WorkspaceConfigSchema"#,
    ] {
        let (_directory, store) = fixture();
        let host = host();
        commit_unadopted_source(&store, &host, source);
        let before = store.load_workspace_config(WORKSPACE).unwrap();
        assert!(store.configured_permission_host(WORKSPACE, &host).is_err());
        assert!(
            store
                .worker_permissions(WORKSPACE, &host, Some(&PermissionAssignment::default()))
                .is_err()
        );
        assert_eq!(store.load_workspace_config(WORKSPACE).unwrap(), before);
        assert_unadopted(&store);
    }
}

#[test]
fn disabled_feature_withdraws_operations_without_losing_selected_reference() {
    let (_directory, store) = fixture();
    let mut host = host();
    adopt(&store, &host, &request(&store, &host, "initial", SELECTED)).unwrap();
    let selected = store.worker_assignment(WORKSPACE, &host, None).unwrap();
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

#[test]
fn missing_empty_excluded_and_revoked_config_defaults_survive_reopen_without_merging() {
    let implementer = policy_config::PermissionSource::default()
        .worker_default
        .assignment();
    let empty = PermissionAssignment::default();
    let excluded = PermissionAssignment {
        exclusions: implementer.grants.clone(),
        ..implementer.clone()
    };
    let revoked = PermissionAssignment {
        revoked: true,
        ..implementer.clone()
    };
    for adopted in [false, true] {
        for (source, expected, allowed) in [
            (
                "{} as WorkspaceConfigSchema",
                &implementer,
                vec!["ticket.read"],
            ),
            (
                "{ permissions = {}; } as WorkspaceConfigSchema",
                &implementer,
                vec!["ticket.read"],
            ),
            (
                "{ permissions.worker_default = {}; } as WorkspaceConfigSchema",
                &empty,
                vec![],
            ),
            (
                "{ permissions.worker_default = { sets = []; }; } as WorkspaceConfigSchema",
                &empty,
                vec![],
            ),
            (
                r#"{ permissions.worker_default = { sets = ["implementer"]; exclude_sets = ["implementer"]; }; } as WorkspaceConfigSchema"#,
                &excluded,
                vec![],
            ),
            (
                r#"{ permissions.worker_default = { sets = ["implementer"]; revoked = true; }; } as WorkspaceConfigSchema"#,
                &revoked,
                vec![],
            ),
        ] {
            let (directory, store) = fixture();
            let host = host();
            if adopted {
                adopt(
                    &store,
                    &host,
                    &request(&store, &host, "config-default", source),
                )
                .unwrap();
            } else {
                commit_unadopted_source(&store, &host, source);
                assert_unadopted(&store);
            }
            let before = store.load_workspace_config(WORKSPACE).unwrap();
            let (configured, selected) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
            assert_eq!(&selected, expected, "{source}");
            assert_eq!(keys(&configured, &selected), allowed, "{source}");
            drop(store);
            let store =
                SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite"))
                    .unwrap();
            let (configured, selected) = store.worker_permissions(WORKSPACE, &host, None).unwrap();
            assert_eq!(&selected, expected, "{source}");
            assert_eq!(keys(&configured, &selected), allowed, "{source}");
            assert_eq!(
                store
                    .worker_assignment(WORKSPACE, &host, Some(&empty))
                    .unwrap(),
                empty
            );
            assert_eq!(store.load_workspace_config(WORKSPACE).unwrap(), before);
            if !adopted {
                assert_unadopted(&store);
            }
        }
    }
}
