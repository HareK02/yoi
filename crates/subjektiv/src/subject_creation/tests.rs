//! SQLite component coverage: normal store creation, persisted source/CAS and exact retries.
//! Authentication/Runtime connections are covered by the Backend connected chat suite.
use super::*;
use crate::tests::{open_store, role};
use server_api::authorization::{SubjectPermissionsUpdate, validate_managed_subject_permissions};

#[test]
fn creation_persists_self_scoped_execution_and_no_delegation() {
    let dir = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(dir.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap();
    let source = store.subject_permissions(&subject.id).unwrap().unwrap();
    assert_eq!(
        source.execution,
        subject_conversation_permissions("workspace-a", &subject.id)
    );
    assert!(source.delegation.is_empty());
    validate_managed_subject_permissions(
        None,
        &SubjectPermissionsUpdate {
            operation_id: "validate-initial".into(),
            expected_change_id: None,
            execution: source.execution.clone(),
            delegation: vec![],
        },
    )
    .unwrap();
    for permission in source.execution {
        let mut foreign = permission.clone();
        foreign.resource = server_api::authorization::PermissionResource::Subject {
            workspace_id: "workspace-a".into(),
            subject_id: "other".into(),
        };
        assert!(!permission.covers(&foreign));
    }
}

#[test]
fn creation_replay_after_reopen_never_duplicates_or_restores_revoked_source() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    let revoked;
    {
        let (_manager, _scope, store) = open_store(dir.path(), "workspace-a");
        let first = store
            .create_subject_idempotent(role(), "original".into(), "create")
            .unwrap();
        assert_eq!(
            store
                .create_subject_idempotent(role(), "original".into(), "create")
                .unwrap(),
            first
        );
        assert!(matches!(
            store.create_subject_idempotent(role(), "changed".into(), "create"),
            Err(SubjektivError::SubjectCreationConflict(_))
        ));
        id = first.id;
        let source = store.subject_permissions(&id).unwrap().unwrap();
        revoked = store
            .set_subject_permissions(
                &id,
                &SubjectPermissionsUpdate {
                    operation_id: "revoke".into(),
                    expected_change_id: Some(source.change_id),
                    execution: vec![],
                    delegation: vec![],
                },
            )
            .unwrap();
    }
    let (_manager, _scope, store) = open_store(dir.path(), "workspace-a");
    assert_eq!(
        store
            .create_subject_idempotent(role(), "original".into(), "create")
            .unwrap()
            .id,
        id
    );
    assert_eq!(store.subject_permissions(&id).unwrap(), Some(revoked));
    assert_eq!(store.list_subjects_after(100, None).unwrap().items.len(), 1);
}

#[test]
fn permission_write_failure_rolls_back_subject_and_creation_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(dir.path(), "workspace-a");
    store.database.with_connection(|c| {
        Ok(c.execute_batch("CREATE TRIGGER fail_initial_policy BEFORE INSERT ON subject_permissions BEGIN SELECT RAISE(ABORT,'injected policy failure'); END;")?)
    }).unwrap();
    assert!(
        store
            .create_subject_idempotent(role(), String::new(), "atomic")
            .is_err()
    );
    assert!(
        store
            .list_subjects_after(100, None)
            .unwrap()
            .items
            .is_empty()
    );
    store
        .database
        .with_connection(|c| Ok(c.execute_batch("DROP TRIGGER fail_initial_policy;")?))
        .unwrap();
    let subject = store
        .create_subject_idempotent(role(), String::new(), "atomic")
        .unwrap();
    assert!(store.subject_permissions(&subject.id).unwrap().is_some());
}

#[test]
fn creation_replay_after_erasure_does_not_recreate_subject_or_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(dir.path(), "workspace-a");
    let created = store
        .create_subject_idempotent(role(), "private".into(), "create-erased")
        .unwrap();
    store.begin_subject_deletion(&created.id, "erase").unwrap();
    assert!(
        store
            .create_subject_idempotent(role(), "private".into(), "create-erased")
            .is_err()
    );
    store.finish_subject_deletion(&created.id, "erase").unwrap();
    assert!(
        store
            .create_subject_idempotent(role(), "private".into(), "create-erased")
            .is_err()
    );
    assert!(
        store
            .list_subjects_after(100, None)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn receipt_migration_preserves_existing_unset_and_revoked_policies_and_rolls_back_failure() {
    let dir = tempfile::tempdir().unwrap();
    let manager = feature_storage::FeatureStorage::new(dir.path());
    let scope = manager.scope("workspace-a").unwrap();
    let old_registration = scope
        .register(feature_storage::FeatureRegistration::new(
            SUBJEKTIV_FEATURE_ID,
            &MIGRATIONS[..15],
        ))
        .unwrap();
    let old = SubjektivStore::open(&scope, &old_registration).unwrap();
    let unset = old.create_subject(role()).unwrap();
    let revoked = old.create_subject(role()).unwrap();
    let source = old.subject_permissions(&revoked.id).unwrap().unwrap();
    let empty = old
        .set_subject_permissions(
            &revoked.id,
            &SubjectPermissionsUpdate {
                operation_id: "revoke-before-upgrade".into(),
                expected_change_id: Some(source.change_id),
                execution: vec![],
                delegation: vec![],
            },
        )
        .unwrap();
    old.database
        .with_connection(|c| {
            c.execute(
                "DELETE FROM subject_permissions WHERE subject_id=?1",
                [&unset.id],
            )?;
            c.execute_batch("CREATE TABLE subject_creation_receipts(conflicting_column TEXT);")?;
            Ok(())
        })
        .unwrap();
    drop(old);
    drop(scope);
    drop(manager);
    {
        let manager = feature_storage::FeatureStorage::new(dir.path());
        let scope = manager.scope("workspace-a").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        assert!(SubjektivStore::open(&scope, &registration).is_err());
    }
    let manager = feature_storage::FeatureStorage::new(dir.path());
    let scope = manager.scope("workspace-a").unwrap();
    let registration = scope
        .register(feature_storage::FeatureRegistration::new(
            SUBJEKTIV_FEATURE_ID,
            &MIGRATIONS[..15],
        ))
        .unwrap();
    let old = SubjektivStore::open(&scope, &registration).unwrap();
    assert_eq!(old.database.schema_version().unwrap(), 15);
    assert_eq!(old.subject(&unset.id).unwrap(), Some(unset.clone()));
    assert_eq!(
        old.subject_permissions(&revoked.id).unwrap(),
        Some(empty.clone())
    );
    old.database
        .with_connection(|c| {
            c.execute_batch("DROP TABLE subject_creation_receipts;")?;
            Ok(())
        })
        .unwrap();
    drop(old);
    drop(scope);
    drop(manager);
    let manager = feature_storage::FeatureStorage::new(dir.path());
    let scope = manager.scope("workspace-a").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let upgraded = SubjektivStore::open(&scope, &registration).unwrap();
    assert_eq!(upgraded.subject_permissions(&unset.id).unwrap(), None);
    assert_eq!(
        upgraded.subject_permissions(&revoked.id).unwrap(),
        Some(empty)
    );
    let fresh = upgraded.create_subject(role()).unwrap();
    assert!(upgraded.subject_permissions(&fresh.id).unwrap().is_some());
}
