//! Component tests use real isolated Server SQLite, config evaluation and the
//! permission resolver. Native creation evidence is supplied at the enrollment
//! transaction boundary; API authentication and native lifecycle lock ordering
//! belong to the Server connected suites, not this storage fixture.

use super::*;
use crate::config_source::WorkspaceConfigSchemaProvider;
use config_source::WorkspaceConfigSchemaBundle;
use feature_permissions::*;

const WORKSPACE: &str = "subject-policy-workspace";
const OWNER: &str = "subject-policy-owner";
const SUBJECT: &str = "subject-a";

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
            sets: [
                "ticket",
                "conversation",
                "work-coordination",
                "work-execution",
            ]
            .into_iter()
            .map(|name| SetContribution {
                set: PermissionSetId::parse(name).unwrap(),
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

fn fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let (directory, store) = unadopted_fixture();
    store.with_immediate_transaction(|transaction| {
        transaction.execute_batch(
            "INSERT INTO workspace_configuration_authorities(workspace_id,transfer_identity,legacy_physical_identity,legacy_export_digest) VALUES('subject-policy-workspace','fixture-transfer','fixture-physical','fixture-export');",
        )?;
        Ok(())
    }).unwrap();
    (directory, store)
}

fn unadopted_fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    store.with_immediate_transaction(|transaction| {
        transaction.execute_batch(
            "INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('subject-policy-owner','user','subject-policy-owner','Owner','1','1'),('other-owner','user','other-owner','Other','1','1');
             INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('subject-policy-workspace','subject-policy-owner','Subject policies','active','1','1');",
        )?;
        Ok(())
    }).unwrap();
    let schema = WorkspaceConfigSchemaBundle::compose([
        policy_config::PermissionConfigSchemaProvider
            .contribution()
            .unwrap(),
        crate::prompt_settings::PromptConfigSchemaProvider
            .contribution()
            .unwrap(),
    ])
    .unwrap();
    store
        .ensure_workspace_config_materialized_with_schema(WORKSPACE, "now", schema)
        .unwrap();
    (directory, store)
}

fn assignment(set: &str) -> PermissionAssignment {
    PermissionAssignment {
        grants: vec![PermissionReference::Set(
            PermissionSetId::parse(set).unwrap(),
        )],
        ..Default::default()
    }
}

fn replacement(
    operation: &str,
    expected: Option<&str>,
    execution: PermissionAssignment,
) -> SubjectPermissionPolicyMutation {
    SubjectPermissionPolicyMutation {
        operation_id: operation.into(),
        expected_change_id: expected.map(str::to_owned),
        action: SubjectPermissionPolicyAction::Replace {
            execution,
            delegation: PermissionAssignment::default(),
        },
    }
}

fn mutate(
    store: &SqliteWorkspaceStore,
    request: &SubjectPermissionPolicyMutation,
) -> Result<SubjectPermissionPolicyMutationResponse> {
    store.mutate_subject_permission_policy(WORKSPACE, OWNER, SUBJECT, request, &host())
}

fn remove(
    store: &SqliteWorkspaceStore,
    operation: &str,
    expected: Option<&str>,
) -> Result<SubjectPermissionPolicyMutationResponse> {
    mutate(
        store,
        &SubjectPermissionPolicyMutation {
            operation_id: operation.into(),
            expected_change_id: expected.map(str::to_owned),
            action: SubjectPermissionPolicyAction::Remove,
        },
    )
}

fn source() -> SubjectPolicySource {
    SubjectPolicySource::Feature(CurrentSubjectPolicy {
        change_id: "native-creation-change".into(),
        execution: assignment("ticket"),
        delegation: PermissionAssignment::default(),
        legacy_constraints: None,
        reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
    })
}

fn consume(
    store: &SqliteWorkspaceStore,
    enrollment: &SubjectPolicySource,
) -> Result<Option<CurrentSubjectPolicy>> {
    store.with_immediate_transaction(|transaction| {
        require_owner(transaction, WORKSPACE, OWNER)?;
        require_canonical(transaction, WORKSPACE)?;
        enroll(transaction, WORKSPACE, SUBJECT, enrollment, &host())
    })
}

fn count(store: &SqliteWorkspaceStore, table: &str) -> i64 {
    store
        .with_conn(|connection| {
            Ok(
                connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap()
}

fn config_identity(store: &SqliteWorkspaceStore) -> (String, Option<String>, i64) {
    (
        json(&store.load_workspace_config(WORKSPACE).unwrap()).unwrap(),
        store.configuration_head(WORKSPACE, OWNER).unwrap(),
        count(store, "workspace_configuration_receipts"),
    )
}

#[test]
fn owner_and_active_workspace_are_required_even_for_replay() {
    let (_directory, store) = fixture();
    let request = replacement("owner-operation", None, assignment("ticket"));
    assert!(matches!(
        store.mutate_subject_permission_policy(
            WORKSPACE,
            "other-owner",
            SUBJECT,
            &request,
            &host()
        ),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert_eq!(count(&store, "subject_permission_operations"), 0);
    let applied = mutate(&store, &request).unwrap().policy;
    store
        .with_immediate_transaction(|transaction| {
            transaction.execute(
                "UPDATE workspaces SET owner_account_id='other-owner' WHERE workspace_id=?1",
                [WORKSPACE],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        mutate(&store, &request),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert!(matches!(
        store.mutate_subject_permission_policy(
            WORKSPACE,
            "other-owner",
            SUBJECT,
            &request,
            &host()
        ),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    store
        .with_immediate_transaction(|transaction| {
            transaction.execute(
                "UPDATE workspaces SET owner_account_id=?2,state='deleted' WHERE workspace_id=?1",
                params![WORKSPACE, OWNER],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        mutate(&store, &request),
        Err(Error::WorkspacePermissionDenied(_))
    ));
    assert_eq!(count(&store, "subject_permission_operations"), 1);
    assert_eq!(
        store
            .current_subject_permission_policy(WORKSPACE, SUBJECT)
            .unwrap(),
        applied
    );
}

#[test]
fn policy_cas_rejects_stale_and_aba_edits_and_replay_never_reapplies() {
    let (directory, store) = fixture();
    let first_request = replacement("first", None, assignment("ticket"));
    let first = mutate(&store, &first_request).unwrap().policy.unwrap();
    let stale = replacement("stale", Some(&first.change_id), assignment("ticket"));
    let second = mutate(
        &store,
        &replacement(
            "empty",
            Some(&first.change_id),
            PermissionAssignment::default(),
        ),
    )
    .unwrap()
    .policy
    .unwrap();
    let third = mutate(
        &store,
        &replacement("back", Some(&second.change_id), assignment("ticket")),
    )
    .unwrap()
    .policy
    .unwrap();
    assert_eq!(first.execution, third.execution);
    assert_ne!(first.change_id, third.change_id);
    assert!(matches!(
        mutate(&store, &stale),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    drop(store);
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    assert_eq!(mutate(&store, &first_request).unwrap().policy, Some(first));
    assert_eq!(
        store
            .current_subject_permission_policy(WORKSPACE, SUBJECT)
            .unwrap(),
        Some(third)
    );
    assert_eq!(count(&store, "subject_permission_operations"), 3);
}

#[test]
fn removed_state_blocks_absent_aba_and_recreation_requires_its_change_id() {
    let (directory, store) = fixture();
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (None, None)
    );
    let stale_absent = replacement("stale-absent", None, assignment("ticket"));
    let first = mutate(
        &store,
        &replacement("first-state", None, assignment("ticket")),
    )
    .unwrap();
    assert_eq!(
        first.change_id.as_ref(),
        first.policy.as_ref().map(|policy| &policy.change_id)
    );
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (first.change_id.clone(), first.policy.clone())
    );
    let removed = remove(&store, "remove-state", first.change_id.as_deref()).unwrap();
    assert!(removed.policy.is_none());
    assert!(removed.change_id.is_some());
    assert_ne!(removed.change_id, first.change_id);
    drop(store);
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (removed.change_id.clone(), None)
    );
    assert_eq!(
        store
            .subject_permission_policy_change_id(WORKSPACE, SUBJECT)
            .unwrap(),
        removed.change_id
    );
    assert!(matches!(
        mutate(&store, &stale_absent),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    assert!(matches!(
        remove(&store, "stale-absent-remove", None),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    let recreated = mutate(
        &store,
        &replacement(
            "recreate",
            removed.change_id.as_deref(),
            assignment("ticket"),
        ),
    )
    .unwrap();
    assert_ne!(recreated.change_id, removed.change_id);
    assert_eq!(
        recreated.change_id.as_ref(),
        recreated.policy.as_ref().map(|policy| &policy.change_id)
    );
    assert_eq!(count(&store, "subject_permission_tombstones"), 0);
    let replay = remove(&store, "remove-state", first.change_id.as_deref()).unwrap();
    assert_eq!(replay, removed);
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (recreated.change_id.clone(), recreated.policy.clone())
    );
    let removed_again = remove(&store, "remove-again", recreated.change_id.as_deref()).unwrap();
    assert_ne!(removed_again.change_id, removed.change_id);
    let removed_twice = remove(&store, "remove-twice", removed_again.change_id.as_deref()).unwrap();
    assert_ne!(removed_twice.change_id, removed_again.change_id);
    assert_eq!(count(&store, "subject_permission_tombstones"), 1);
    assert_eq!(
        remove(&store, "remove-state", first.change_id.as_deref()).unwrap(),
        removed
    );
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (removed_twice.change_id.clone(), None)
    );
    assert!(matches!(
        mutate(
            &store,
            &replacement(
                "stale-removal",
                removed.change_id.as_deref(),
                assignment("ticket")
            ),
        ),
        Err(Error::WorkspaceConfigConflict(_))
    ));
}

#[test]
fn operation_id_binds_subject_and_full_request() {
    let (_directory, store) = fixture();
    let original = replacement("same-id", None, assignment("ticket"));
    mutate(&store, &original).unwrap();
    let mut changed = original.clone();
    changed.expected_change_id = Some("different-observation".into());
    assert!(matches!(
        mutate(&store, &changed),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    changed = original.clone();
    changed.action = SubjectPermissionPolicyAction::Remove;
    assert!(matches!(
        mutate(&store, &changed),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    assert!(matches!(
        store.mutate_subject_permission_policy(WORKSPACE, OWNER, "subject-b", &original, &host()),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    let request_json: String = store.with_conn(|connection| {
        Ok(connection.query_row("SELECT request_json FROM subject_permission_operations WHERE workspace_id=?1 AND operation_id='same-id'", [WORKSPACE], |row| row.get(0))?)
    }).unwrap();
    assert_eq!(request_json, json(&original).unwrap());
    assert_eq!(count(&store, "subject_permission_operations"), 1);
}

#[test]
fn revoked_empty_and_removed_policies_are_distinct_and_constraints_survive_replace() {
    let (_directory, store) = fixture();
    let mut legacy = match source() {
        SubjectPolicySource::Feature(policy) => policy,
        SubjectPolicySource::Legacy(_) => unreachable!(),
    };
    legacy.legacy_constraints = Some(server_api::authorization::SubjectPermissions {
        change_id: "old-limits".into(),
        execution: vec![],
        delegation: vec![],
    });
    store
        .with_immediate_transaction(|transaction| save(transaction, WORKSPACE, SUBJECT, &legacy))
        .unwrap();
    let empty = mutate(
        &store,
        &replacement(
            "explicit-empty",
            Some(&legacy.change_id),
            PermissionAssignment::default(),
        ),
    )
    .unwrap()
    .policy
    .unwrap();
    assert!(empty.execution.grants.is_empty());
    assert!(!empty.execution.revoked);
    assert_eq!(empty.legacy_constraints, legacy.legacy_constraints);
    let mut revoked = assignment("ticket");
    revoked.revoked = true;
    let revoked = mutate(
        &store,
        &replacement("revoke", Some(&empty.change_id), revoked),
    )
    .unwrap()
    .policy
    .unwrap();
    assert!(revoked.execution.revoked);
    assert_eq!(revoked.legacy_constraints, legacy.legacy_constraints);
    assert!(
        remove(&store, "remove", Some(&revoked.change_id))
            .unwrap()
            .policy
            .is_none()
    );
    assert!(
        store
            .current_subject_permission_policy(WORKSPACE, SUBJECT)
            .unwrap()
            .is_none()
    );
    assert!(consume(&store, &source()).unwrap().is_none());
}

#[test]
fn invalid_grants_and_exclusions_are_rejected_even_when_revoked() {
    let (_directory, store) = fixture();
    for (index, name) in ["missing", "subject-default"].into_iter().enumerate() {
        for excluded in [false, true] {
            let reference = PermissionReference::Set(PermissionSetId::parse(name).unwrap());
            let assignment = PermissionAssignment {
                grants: if excluded {
                    vec![]
                } else {
                    vec![reference.clone()]
                },
                exclusions: if excluded { vec![reference] } else { vec![] },
                revoked: true,
            };
            assert!(matches!(
                mutate(
                    &store,
                    &replacement(&format!("invalid-{index}-{excluded}"), None, assignment)
                ),
                Err(Error::InvalidInput(_))
            ));
        }
    }
    assert_eq!(count(&store, "workspace_configuration_policies"), 0);
    assert_eq!(count(&store, "subject_permission_operations"), 0);
}

#[test]
fn enrollment_consumption_is_durable_and_remove_never_revives_creation_policy() {
    let (directory, store) = fixture();
    let enrollment = source();
    let policy = consume(&store, &enrollment).unwrap().unwrap();
    assert_eq!(policy.change_id, "native-creation-change");
    assert_eq!(consume(&store, &enrollment).unwrap(), Some(policy.clone()));
    remove(&store, "remove-enrolled", Some(&policy.change_id)).unwrap();
    drop(store);
    let store =
        SqliteWorkspaceStore::open_for_test(&directory.path().join("server.sqlite")).unwrap();
    assert!(consume(&store, &enrollment).unwrap().is_none());
    assert_eq!(count(&store, "subject_permission_enrollments"), 1);
    assert_eq!(count(&store, "workspace_configuration_policies"), 0);
    let mut changed = policy;
    changed.execution.revoked = true;
    assert!(matches!(
        consume(&store, &SubjectPolicySource::Feature(changed)),
        Err(Error::WorkspaceConfigConflict(_))
    ));
}

#[test]
fn explicit_remove_before_enrollment_and_old_adoption_receipt_both_consume_without_issuance() {
    for old_receipt in [false, true] {
        let (_directory, store) = fixture();
        if old_receipt {
            store.with_immediate_transaction(|transaction| {
                transaction.execute(
                    "INSERT INTO workspace_configuration_receipts(workspace_id,operation_id,request_fingerprint,result_json) VALUES(?1,?2,'old-fingerprint','{}')",
                    params![WORKSPACE, format!("subject-enrollment:{SUBJECT}")],
                )?;
                Ok(())
            }).unwrap();
        } else {
            remove(&store, "remove-before-enrollment", None).unwrap();
        }
        assert!(consume(&store, &source()).unwrap().is_none());
        assert!(consume(&store, &source()).unwrap().is_none());
        assert_eq!(count(&store, "subject_permission_enrollments"), 1);
        assert_eq!(count(&store, "workspace_configuration_policies"), 0);
    }
}

#[test]
fn existing_policy_wins_enrollment_without_regenerating_a_change_identity() {
    let (_directory, store) = fixture();
    let previous = mutate(
        &store,
        &replacement("owner-empty", None, PermissionAssignment::default()),
    )
    .unwrap()
    .policy
    .unwrap();
    assert_eq!(consume(&store, &source()).unwrap(), Some(previous.clone()));
    assert_eq!(count(&store, "subject_permission_enrollments"), 1);
    remove(&store, "remove-owner-empty", Some(&previous.change_id)).unwrap();
    assert!(consume(&store, &source()).unwrap().is_none());
}

#[test]
fn old_creation_reference_encoding_is_converted_only_for_current_policy() {
    let (_directory, store) = fixture();
    let mut old = match source() {
        SubjectPolicySource::Feature(policy) => policy,
        SubjectPolicySource::Legacy(_) => unreachable!(),
    };
    old.reference_encoding = PermissionReferenceEncoding::LegacySubjectV1;
    old.execution = assignment("subject-default");
    old.execution.exclusions = assignment("subject-default").grants;
    let source = SubjectPolicySource::Feature(old);
    let raw = json(&source).unwrap();
    let enrolled = consume(&store, &source).unwrap().unwrap();
    assert_eq!(enrolled.change_id, "native-creation-change");
    assert_eq!(
        enrolled.reference_encoding,
        PermissionReferenceEncoding::FeatureSetsV2
    );
    assert_eq!(
        enrolled.execution.grants,
        vec![
            PermissionReference::Set(PermissionSetId::parse("conversation").unwrap()),
            PermissionReference::Set(PermissionSetId::parse("work-coordination").unwrap()),
        ]
    );
    assert_eq!(enrolled.execution.exclusions, enrolled.execution.grants);
    assert!(enrolled.delegation.grants.is_empty());
    let saved: String = store.with_conn(|connection| {
        Ok(connection.query_row("SELECT source_json FROM subject_permission_enrollments WHERE workspace_id=?1 AND subject_id=?2", params![WORKSPACE, SUBJECT], |row| row.get(0))?)
    }).unwrap();
    assert_eq!(saved, raw);
    assert_eq!(consume(&store, &source).unwrap(), Some(enrolled));
}

#[test]
fn mutations_enrollment_and_replays_never_edit_configuration_or_history() {
    let (_directory, store) = fixture();
    let before = config_identity(&store);
    let enrolled = consume(&store, &source()).unwrap().unwrap();
    let request = replacement(
        "no-config-edit",
        Some(&enrolled.change_id),
        assignment("ticket"),
    );
    let replaced = mutate(&store, &request).unwrap().policy.unwrap();
    remove(&store, "no-config-remove", Some(&replaced.change_id)).unwrap();
    assert_eq!(mutate(&store, &request).unwrap().policy, Some(replaced));
    assert!(consume(&store, &source()).unwrap().is_none());
    assert_eq!(config_identity(&store), before);
}

#[test]
fn receipt_insertion_failure_rolls_back_current_policy_and_retry_is_safe() {
    for enrollment in [false, true] {
        let (_directory, store) = fixture();
        let table = if enrollment {
            "subject_permission_enrollments"
        } else {
            "subject_permission_operations"
        };
        store.with_conn(|connection| {
            connection.execute_batch(&format!(
                "CREATE TRIGGER reject_receipt BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;"
            ))?;
            Ok(())
        }).unwrap();
        let request = replacement("atomic", None, assignment("ticket"));
        if enrollment {
            assert!(consume(&store, &source()).is_err());
        } else {
            assert!(mutate(&store, &request).is_err());
        }
        assert_eq!(count(&store, "workspace_configuration_policies"), 0);
        assert_eq!(count(&store, table), 0);
        store
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER reject_receipt")?;
                Ok(())
            })
            .unwrap();
        if enrollment {
            assert!(consume(&store, &source()).unwrap().is_some());
        } else {
            assert!(mutate(&store, &request).unwrap().policy.is_some());
        }
        assert_eq!(count(&store, table), 1);
    }
}

#[test]
fn unadopted_subject_writes_fail_closed_without_blocking_ordinary_human_selection() {
    let (_directory, store) = unadopted_fixture();
    assert!(matches!(
        mutate(
            &store,
            &replacement("not-adopted", None, assignment("ticket"))
        ),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    assert!(matches!(
        consume(&store, &source()),
        Err(Error::WorkspaceConfigConflict(_))
    ));
    assert_eq!(
        store
            .worker_assignment(WORKSPACE, &host(), Some(&assignment("ticket")))
            .unwrap(),
        assignment("ticket")
    );
    assert_eq!(count(&store, "subject_permission_operations"), 0);
    assert_eq!(count(&store, "subject_permission_enrollments"), 0);
}

#[test]
fn enrollment_rejects_unknown_saved_references_and_legacy_scalar_defaults() {
    let (_directory, store) = fixture();
    let invalid = SubjectPolicySource::Feature(CurrentSubjectPolicy {
        execution: assignment("missing-native-set"),
        ..match source() {
            SubjectPolicySource::Feature(policy) => policy,
            SubjectPolicySource::Legacy(_) => unreachable!(),
        }
    });
    assert!(matches!(
        consume(&store, &invalid),
        Err(Error::InvalidInput(_))
    ));
    let scalar = SubjectPolicySource::Legacy(server_api::authorization::SubjectPermissions {
        change_id: "scalar-creation".into(),
        execution: vec![],
        delegation: vec![],
    });
    assert!(matches!(
        consume(&store, &scalar),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(count(&store, "subject_permission_enrollments"), 0);
    assert_eq!(count(&store, "workspace_configuration_policies"), 0);
    assert!(consume(&store, &source()).unwrap().is_some());
}

#[test]
fn legacy_removals_preserve_current_and_existing_tombstone_and_fence_enrollment() {
    let (_directory, store) = fixture();
    let active = mutate(&store, &replacement("active", None, assignment("ticket"))).unwrap();
    let removed = store
        .mutate_subject_permission_policy(
            WORKSPACE,
            OWNER,
            "subject-b",
            &SubjectPermissionPolicyMutation {
                operation_id: "remove-b".into(),
                expected_change_id: None,
                action: SubjectPermissionPolicyAction::Remove,
            },
            &host(),
        )
        .unwrap();
    let before_config = config_identity(&store);
    let subjects = [SUBJECT, "subject-b", "subject-c", "subject-c"].map(str::to_owned);
    store
        .with_immediate_transaction(|transaction| {
            preserve_legacy_removals(transaction, WORKSPACE, &subjects)
        })
        .unwrap();
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, SUBJECT)
            .unwrap(),
        (active.change_id, active.policy)
    );
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, "subject-b")
            .unwrap(),
        (removed.change_id.clone(), None)
    );
    let legacy_removed = store
        .subject_permission_policy_state(WORKSPACE, "subject-c")
        .unwrap();
    assert!(legacy_removed.0.is_some());
    assert!(legacy_removed.1.is_none());
    assert_eq!(count(&store, "subject_permission_tombstones"), 2);
    assert_eq!(count(&store, "subject_permission_operations"), 2);
    store
        .with_immediate_transaction(|transaction| {
            preserve_legacy_removals(transaction, WORKSPACE, &subjects)?;
            for subject in ["subject-b", "subject-c"] {
                assert!(enroll(transaction, WORKSPACE, subject, &source(), &host())?.is_none());
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, "subject-c")
            .unwrap(),
        legacy_removed
    );
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, "subject-b")
            .unwrap(),
        (removed.change_id, None)
    );
    assert_eq!(count(&store, "subject_permission_enrollments"), 2);
    assert_eq!(count(&store, "workspace_configuration_policies"), 1);
    assert_eq!(config_identity(&store), before_config);
}

#[test]
fn legacy_removal_preservation_rolls_back_with_the_migration_transaction() {
    let (_directory, store) = fixture();
    assert!(
        store
            .with_immediate_transaction(|transaction| {
                preserve_legacy_removals(transaction, WORKSPACE, &["subject-c".into()])?;
                Err::<(), _>(Error::Store("injected migration failure".into()))
            })
            .is_err()
    );
    assert_eq!(count(&store, "subject_permission_tombstones"), 0);
    assert_eq!(
        store
            .subject_permission_policy_state(WORKSPACE, "subject-c")
            .unwrap(),
        (None, None)
    );
}

#[test]
fn receipt_failure_restores_tombstone_on_replace_and_active_policy_on_remove() {
    for initially_removed in [false, true] {
        let (_directory, store) = fixture();
        let initial = if initially_removed {
            remove(&store, "initial-removal", None).unwrap()
        } else {
            mutate(
                &store,
                &replacement("initial-active", None, assignment("ticket")),
            )
            .unwrap()
        };
        let before = (initial.change_id.clone(), initial.policy.clone());
        store
            .with_conn(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER reject_mutation_receipt BEFORE INSERT ON subject_permission_operations BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let request = if initially_removed {
            replacement(
                "transition",
                initial.change_id.as_deref(),
                assignment("ticket"),
            )
        } else {
            SubjectPermissionPolicyMutation {
                operation_id: "transition".into(),
                expected_change_id: initial.change_id.clone(),
                action: SubjectPermissionPolicyAction::Remove,
            }
        };
        assert!(mutate(&store, &request).is_err());
        assert_eq!(
            store
                .subject_permission_policy_state(WORKSPACE, SUBJECT)
                .unwrap(),
            before
        );
        assert_eq!(count(&store, "subject_permission_operations"), 1);
        assert_eq!(
            count(&store, "subject_permission_tombstones"),
            if initially_removed { 1 } else { 0 }
        );
        store
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER reject_mutation_receipt")?;
                Ok(())
            })
            .unwrap();
        let applied = mutate(&store, &request).unwrap();
        assert_ne!(applied.change_id, initial.change_id);
        assert_eq!(
            store
                .subject_permission_policy_state(WORKSPACE, SUBJECT)
                .unwrap(),
            (applied.change_id, applied.policy)
        );
    }
}

#[test]
fn subject_receipts_are_workspace_scoped_and_immutable() {
    let (_directory, store) = fixture();
    mutate(&store, &replacement("retained", None, assignment("ticket"))).unwrap();
    consume(&store, &source()).unwrap();
    for table in [
        "subject_permission_operations",
        "subject_permission_enrollments",
    ] {
        store
            .with_conn(|connection| {
                assert!(
                    connection
                        .execute(&format!("UPDATE {table} SET workspace_id=workspace_id"), [])
                        .is_err()
                );
                assert!(
                    connection
                        .execute(&format!("DELETE FROM {table}"), [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
    store.with_conn(|connection| {
        assert!(connection.execute("INSERT INTO subject_permission_operations(workspace_id,operation_id,subject_id,account_id,request_json,result_json) VALUES('absent-workspace','op','subject','account','{}','{}')", []).is_err());
        assert!(connection.execute("INSERT INTO subject_permission_enrollments(workspace_id,subject_id,source_json) VALUES('absent-workspace','subject','{}')", []).is_err());
        assert!(connection.execute("INSERT INTO subject_permission_tombstones(workspace_id,subject_id,change_id) VALUES('absent-workspace','subject','removed-change')", []).is_err());
        Ok(())
    }).unwrap();
}
