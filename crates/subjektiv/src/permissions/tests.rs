//! Real SQLite component tests through store APIs; Host authentication, canonical singleton
//! ownership and live lineage checks are outside this suite. Root origins are Host-attested inputs;
//! Journal fixtures exercise defensive attribution only, never execution authority.
use super::*;
use crate::tests::{open_store, role};
use feature_storage::FeatureStorage;
use std::path::Path;

fn permission(operation: &str, path: &str, recursive: bool) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workdir {
            working_directory_id: "wd".into(),
            path: path.into(),
            recursive,
        },
    }
}

fn update(
    operation: &str,
    expected: Option<&str>,
    delegation: Vec<ScopedPermission>,
) -> SubjectPermissionsUpdate {
    SubjectPermissionsUpdate {
        operation_id: operation.into(),
        expected_change_id: expected.map(Into::into),
        execution: Vec::new(),
        delegation,
    }
}

fn grant(subject: &str, source: &str, worker: &str) -> WorkerExecutionGrant {
    WorkerExecutionGrant {
        subject_id: subject.into(),
        origin_runtime_id: "runtime".into(),
        origin_worker_id: "body".into(),
        source_change_id: source.into(),
        runtime_id: "runtime".into(),
        worker_id: worker.into(),
        profile: "builtin:coder".into(),
        profile_digest: "resolved-profile-sha256".into(),
        permissions: vec![permission("file.read", "src", false)],
        redelegation: Vec::new(),
    }
}

fn schema(store: &SubjektivStore) -> Vec<(String, String)> {
    store.database.try_with_connection::<_, SubjektivError>(|connection| {
        let mut statement = connection.prepare("SELECT name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY name")?;
        Ok(statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<std::result::Result<_, _>>()?)
    }).unwrap()
}

fn open_version(
    root: &Path,
    version: usize,
) -> (FeatureStorage, ScopedFeatureStorage, SubjektivStore) {
    let manager = FeatureStorage::new(root);
    let scope = manager.scope("workspace-a").unwrap();
    let registration = scope
        .register(FeatureRegistration::new(
            SUBJEKTIV_FEATURE_ID,
            &MIGRATIONS[..version],
        ))
        .unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    (manager, scope, store)
}

#[test]
fn replacements_are_cas_fenced_replayable_and_never_restore_old_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let subject;
    let first;
    let first_update = update("allow", None, vec![permission("file.read", ".", true)]);
    {
        let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
        subject = store.create_subject(role()).unwrap().id;
        assert!(store.subject_permissions(&subject).unwrap().is_none());
        first = store
            .set_subject_permissions(&subject, &first_update)
            .unwrap();
        assert_eq!(
            store
                .set_subject_permissions(&subject, &first_update)
                .unwrap(),
            first
        );
        assert!(matches!(
            store.set_subject_permissions(&subject, &update("stale", None, Vec::new())),
            Err(SubjektivError::SubjectPermissionsConflict(_))
        ));
        assert!(matches!(
            store.set_subject_permissions(&subject, &update("allow", None, Vec::new())),
            Err(SubjektivError::PermissionOperationConflict(_))
        ));
        let other = store.create_subject(role()).unwrap();
        assert!(matches!(
            store.set_subject_permissions(&other.id, &first_update),
            Err(SubjektivError::PermissionOperationConflict(_))
        ));
        let same = store
            .set_subject_permissions(
                &subject,
                &update("same", Some(&first.change_id), first.delegation.clone()),
            )
            .unwrap();
        assert_ne!(same.change_id, first.change_id);
        let revoked = store
            .set_subject_permissions(
                &subject,
                &update("revoke", Some(&same.change_id), Vec::new()),
            )
            .unwrap();
        assert!(revoked.delegation.is_empty());
        assert_eq!(
            store
                .set_subject_permissions(&subject, &first_update)
                .unwrap(),
            first
        );
        assert_eq!(store.subject_permissions(&subject).unwrap(), Some(revoked));
    }
    let (_reopened_manager, _reopened_scope, reopened) = open_store(temp.path(), "workspace-a");
    assert!(
        reopened
            .subject_permissions(&subject)
            .unwrap()
            .unwrap()
            .delegation
            .is_empty()
    );
    assert_eq!(
        reopened
            .set_subject_permissions(&subject, &first_update)
            .unwrap(),
        first
    );
}

#[test]
fn failed_receipt_insert_rolls_back_source_history_and_projection() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let first = store
        .set_subject_permissions(&subject, &update("first", None, Vec::new()))
        .unwrap();
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_permission_receipt BEFORE INSERT ON subject_permission_receipts BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    let replacement = update(
        "second",
        Some(&first.change_id),
        vec![permission("file.read", ".", true)],
    );
    assert!(
        store
            .set_subject_permissions(&subject, &replacement)
            .is_err()
    );
    assert_eq!(store.subject_permissions(&subject).unwrap(), Some(first));
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        let counts: (u32, u32) = tx.query_row("SELECT (SELECT count(*) FROM subject_permission_changes),(SELECT count(*) FROM subject_permission_receipts)", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!(counts, (1, 1));
        tx.execute_batch("DROP TRIGGER fail_permission_receipt")?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .set_subject_permissions(&subject, &replacement)
            .is_ok()
    );
}

#[test]
fn scope_and_validation_rejections_do_not_issue_permission_sources() {
    let temp = tempfile::tempdir().unwrap();
    let (_a_manager, _a_scope, a) = open_store(&temp.path().join("a"), "workspace-a");
    let (_b_manager, _b_scope, b) = open_store(&temp.path().join("b"), "workspace-b");
    let subject = a.create_subject(role()).unwrap().id;
    assert!(
        b.set_subject_permissions(&subject, &update("cross-subject", None, Vec::new()))
            .is_err()
    );
    let cross_workspace = ScopedPermission {
        operation: "ticket.create".into(),
        resource: PermissionResource::Workspace {
            workspace_id: "workspace-b".into(),
        },
    };
    assert!(
        a.set_subject_permissions(
            &subject,
            &update("cross-workspace", None, vec![cross_workspace])
        )
        .is_err()
    );
    for (operation, path) in [
        ("file.*", "src"),
        ("file..read", "src"),
        ("file.read", "../src"),
        ("file.read", "src/../escape"),
    ] {
        assert!(
            a.set_subject_permissions(
                &subject,
                &update("invalid", None, vec![permission(operation, path, true)])
            )
            .is_err()
        );
    }
    assert!(a.subject_permissions(&subject).unwrap().is_none());
}

#[test]
fn child_grants_are_immutable_bounded_and_do_not_create_subject_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", "src", true)]),
        )
        .unwrap();
    let child = grant(&subject, &source.change_id, "child");
    store.bind_worker_execution_grant(&child).unwrap();
    store.bind_worker_execution_grant(&child).unwrap();
    assert_eq!(
        store.worker_execution_grant("runtime", "child").unwrap(),
        Some(child.clone())
    );
    assert!(
        store
            .journal_body_is_unpublished(&subject, "runtime", "child")
            .unwrap()
    );
    let mut changed = child.clone();
    changed.profile_digest = "changed-profile".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&changed),
        Err(SubjektivError::WorkerExecutionGrantConflict(_))
    ));
    for (worker, requested) in [
        ("write", permission("file.write", "src", false)),
        ("escape", permission("file.read", ".", true)),
        ("prefix", permission("file.read", "src-other", false)),
    ] {
        let mut candidate = grant(&subject, &source.change_id, worker);
        candidate.permissions = vec![requested];
        assert!(store.bind_worker_execution_grant(&candidate).is_err());
        assert!(
            store
                .worker_execution_grant("runtime", worker)
                .unwrap()
                .is_none()
        );
    }
    let mut self_origin = child.clone();
    self_origin.worker_id = "body".into();
    assert!(store.bind_worker_execution_grant(&self_origin).is_err());
    let other = store.create_subject(role()).unwrap();
    store
        .claim_journal_writer(&other.id, "runtime", "other-body")
        .unwrap();
    let mut invalid = grant(&subject, &source.change_id, "other-body");
    assert!(store.bind_worker_execution_grant(&invalid).is_err());
    invalid.worker_id = "forged-child".into();
    invalid.origin_worker_id = "other-body".into();
    assert!(store.bind_worker_execution_grant(&invalid).is_err());
}

#[test]
fn redelegation_requires_current_source_and_explicit_retained_parent_ceiling() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", ".", true)]),
        )
        .unwrap();
    let mut child = grant(&subject, &source.change_id, "child");
    child.redelegation = vec![permission("file.read", "src", true)];
    store.bind_worker_execution_grant(&child).unwrap();
    let mut descendant = grant(&subject, &source.change_id, "grandchild");
    descendant.origin_worker_id = "child".into();
    store.bind_worker_execution_grant(&descendant).unwrap();
    let mut excessive = descendant.clone();
    excessive.worker_id = "excessive".into();
    excessive.redelegation = vec![permission("file.read", ".", true)];
    assert!(store.bind_worker_execution_grant(&excessive).is_err());
    let mut nondelegator = descendant.clone();
    nondelegator.origin_worker_id = "grandchild".into();
    nondelegator.worker_id = "great-grandchild".into();
    assert!(store.bind_worker_execution_grant(&nondelegator).is_err());
    let mut stale = descendant.clone();
    stale.worker_id = "after-source-replacement".into();
    let revoked = store
        .set_subject_permissions(
            &subject,
            &update("revoke", Some(&source.change_id), Vec::new()),
        )
        .unwrap();
    assert_ne!(revoked.change_id, source.change_id);
    stale.origin_worker_id = "new-body".into();
    assert!(store.bind_worker_execution_grant(&stale).is_err());
    // Exact retries/readback recover evidence but cannot revive the source or origin.
    store.bind_worker_execution_grant(&child).unwrap();
    assert_eq!(
        store.worker_execution_grant("runtime", "child").unwrap(),
        Some(child)
    );
    store.retire_subject(&subject).unwrap();
    stale.source_change_id = revoked.change_id;
    assert!(store.bind_worker_execution_grant(&stale).is_err());
}

#[test]
fn journal_state_neither_authenticates_roots_nor_bypasses_retained_child_ceilings() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", ".", true)]),
        )
        .unwrap();
    // The Host has authenticated root ownership outside this store. No Journal record is
    // required; changing Journal writer state cannot supply or invalidate that authentication.
    store
        .bind_worker_execution_grant(&grant(&subject, &source.change_id, "without-journal"))
        .unwrap();
    let journal = store
        .claim_journal_writer(&subject, "runtime", "body")
        .unwrap();
    store
        .bind_worker_execution_grant(&grant(&subject, &source.change_id, "with-active-journal"))
        .unwrap();
    store.release_journal_writer(&journal).unwrap();
    store
        .bind_worker_execution_grant(&grant(&subject, &source.change_id, "with-released-journal"))
        .unwrap();
    // An active Journal binding must never upgrade a child's empty redelegation ceiling.
    store
        .claim_journal_writer(&subject, "runtime", "without-journal")
        .unwrap();
    let mut descendant = grant(&subject, &source.change_id, "descendant");
    descendant.origin_worker_id = "without-journal".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&descendant),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(
        store
            .worker_execution_grant("runtime", "descendant")
            .unwrap()
            .is_none()
    );
}

#[test]
fn retained_origins_reject_cycles_foreign_subjects_and_stale_parent_sources() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", ".", true)]),
        )
        .unwrap();
    let mut child = grant(&subject, &source.change_id, "child");
    child.redelegation = source.delegation.clone();
    store.bind_worker_execution_grant(&child).unwrap();
    // Closing a retained chain back through its previously unbound root must not create a cycle.
    let mut cycle = grant(&subject, &source.change_id, "body");
    cycle.origin_worker_id = "child".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&cycle),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let other = store.create_subject(role()).unwrap().id;
    let other_source = store
        .set_subject_permissions(&other, &update("other", None, source.delegation.clone()))
        .unwrap();
    let mut foreign = grant(&other, &other_source.change_id, "foreign-child");
    foreign.origin_worker_id = "child".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&foreign),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let current = store
        .set_subject_permissions(
            &subject,
            &update("replace", Some(&source.change_id), source.delegation),
        )
        .unwrap();
    let mut stale_parent = grant(&subject, &current.change_id, "stale-parent-child");
    stale_parent.origin_worker_id = "child".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&stale_parent),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
}

#[test]
fn failed_grant_insert_leaves_no_binding_and_can_retry_same_grant() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", ".", true)]),
        )
        .unwrap();
    let child = grant(&subject, &source.change_id, "child");
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_grant BEFORE INSERT ON worker_execution_grants BEGIN SELECT RAISE(ABORT,'injected grant failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(store.bind_worker_execution_grant(&child).is_err());
    assert!(
        store
            .worker_execution_grant("runtime", "child")
            .unwrap()
            .is_none()
    );
    store
        .database
        .try_transaction::<_, SubjektivError>(|tx| {
            tx.execute_batch("DROP TRIGGER fail_grant")?;
            Ok(())
        })
        .unwrap();
    store.bind_worker_execution_grant(&child).unwrap();
    for sql in [
        "UPDATE worker_execution_grants SET source_change_id='forged'",
        "DELETE FROM worker_execution_grants",
        "UPDATE subject_permission_changes SET record_json='{}'",
        "DELETE FROM subject_permission_receipts",
    ] {
        assert!(
            store
                .database
                .try_transaction::<_, SubjektivError>(|tx| {
                    tx.execute_batch(sql)?;
                    Ok(())
                })
                .is_err(),
            "{sql}"
        );
    }
    assert_eq!(
        store.worker_execution_grant("runtime", "child").unwrap(),
        Some(child)
    );
}

#[test]
fn current_schema_migration_preserves_subject_and_matches_new_database() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("migrated");
    let original;
    {
        let (_old_manager, _old_scope, old) = open_version(&root, 11);
        original = old
            .create_subject_with_behavior(role(), "Keep behavior separate".into())
            .unwrap();
    }
    let (_migrated_manager, _migrated_scope, migrated) = open_store(&root, "workspace-a");
    assert_eq!(
        migrated.subject(&original.id).unwrap(),
        Some(original.clone())
    );
    assert!(
        migrated
            .subject_permissions(&original.id)
            .unwrap()
            .is_none()
    );
    let (_fresh_manager, _fresh_scope, fresh) =
        open_store(&temp.path().join("fresh"), "workspace-a");
    assert_eq!(schema(&migrated), schema(&fresh));
    let source = migrated
        .set_subject_permissions(&original.id, &update("new-source", None, Vec::new()))
        .unwrap();
    assert!(!source.change_id.is_empty());
    assert_eq!(migrated.subject(&original.id).unwrap(), Some(original));
}

#[test]
fn unset_or_execution_only_source_cannot_issue_child_grants_and_saved_grants_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let saved;
    {
        let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
        let subject = store.create_subject(role()).unwrap().id;
        assert!(
            store
                .bind_worker_execution_grant(&grant(&subject, "unset", "child"))
                .is_err()
        );
        let mut execution_only = update("execution", None, Vec::new());
        execution_only.execution = vec![permission("file.read", ".", true)];
        let source = store
            .set_subject_permissions(&subject, &execution_only)
            .unwrap();
        assert!(
            store
                .bind_worker_execution_grant(&grant(&subject, &source.change_id, "child"))
                .is_err()
        );
        let source = store
            .set_subject_permissions(
                &subject,
                &update(
                    "delegation",
                    Some(&source.change_id),
                    execution_only.execution,
                ),
            )
            .unwrap();
        saved = grant(&subject, &source.change_id, "child");
        store.bind_worker_execution_grant(&saved).unwrap();
    }
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert_eq!(
        store.worker_execution_grant("runtime", "child").unwrap(),
        Some(saved.clone())
    );
    store.bind_worker_execution_grant(&saved).unwrap();
    let mut conflicting = saved;
    conflicting.profile = "builtin:default".into();
    assert!(matches!(
        store.bind_worker_execution_grant(&conflicting),
        Err(SubjektivError::WorkerExecutionGrantConflict(_))
    ));
}

#[test]
fn subject_grant_listing_is_scoped_retained_ordered_and_fails_on_overflow() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let other = store.create_subject(role()).unwrap().id;
    assert!(
        store
            .list_subject_worker_execution_grants(&subject)
            .unwrap()
            .is_empty()
    );
    let source = store
        .set_subject_permissions(
            &subject,
            &update("allow", None, vec![permission("file.read", ".", true)]),
        )
        .unwrap();
    let other_source = store
        .set_subject_permissions(&other, &update("other", None, source.delegation.clone()))
        .unwrap();
    let other_grant = grant(&other, &other_source.change_id, "other-child");
    store.bind_worker_execution_grant(&other_grant).unwrap();
    for index in (0..MAX_SUBJECT_WORKER_EXECUTION_GRANTS).rev() {
        store
            .bind_worker_execution_grant(&grant(
                &subject,
                &source.change_id,
                &format!("child-{index:04}"),
            ))
            .unwrap();
    }
    store
        .set_subject_permissions(
            &subject,
            &update("revoke", Some(&source.change_id), Vec::new()),
        )
        .unwrap();
    store.retire_subject(&subject).unwrap();
    let listed = store
        .list_subject_worker_execution_grants(&subject)
        .unwrap();
    assert_eq!(listed.len(), MAX_SUBJECT_WORKER_EXECUTION_GRANTS);
    assert!(
        listed
            .iter()
            .all(|grant| grant.subject_id == subject && grant.source_change_id == source.change_id)
    );
    assert!(
        listed
            .windows(2)
            .all(|pair| (&pair[0].runtime_id, &pair[0].worker_id)
                < (&pair[1].runtime_id, &pair[1].worker_id))
    );
    assert_eq!(
        store.list_subject_worker_execution_grants(&other).unwrap(),
        vec![other_grant.clone()]
    );
    // Seed one more retained row directly: the retired Subject cannot issue new grants.
    // Listing must report overflow rather than returning a seemingly complete cancellation set.
    let overflow = grant(&subject, &source.change_id, "overflow");
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute("INSERT INTO worker_execution_grants(runtime_id,worker_id,subject_id,source_change_id,record_json) VALUES(?1,?2,?3,?4,?5)",
            params![overflow.runtime_id, overflow.worker_id, overflow.subject_id, overflow.source_change_id, serde_json::to_string(&overflow)?])?;
        Ok(())
    }).unwrap();
    assert!(
        matches!(store.list_subject_worker_execution_grants(&subject), Err(SubjektivError::WorkerExecutionGrantListOverflow { subject_id, limit: MAX_SUBJECT_WORKER_EXECUTION_GRANTS }) if subject_id == subject)
    );
    assert_eq!(
        store.list_subject_worker_execution_grants(&other).unwrap(),
        vec![other_grant]
    );
    let (_foreign_manager, _foreign_scope, foreign) =
        open_store(&temp.path().join("foreign"), "workspace-b");
    assert!(matches!(
        foreign.list_subject_worker_execution_grants(&subject),
        Err(SubjektivError::SubjectNotFound(_))
    ));
}

#[test]
fn failed_schema_migration_rolls_back_new_tables_and_preserves_old_data() {
    let temp = tempfile::tempdir().unwrap();
    let original;
    {
        let (_old_manager, _old_scope, old) = open_version(temp.path(), 11);
        original = old.create_subject(role()).unwrap();
        old.database
            .try_transaction::<_, SubjektivError>(|tx| {
                tx.execute_batch("CREATE TABLE subject_permissions (collision TEXT)")?;
                Ok(())
            })
            .unwrap();
    }
    {
        let manager = FeatureStorage::new(temp.path());
        let scope = manager.scope("workspace-a").unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        assert!(SubjektivStore::open(&scope, &registration).is_err());
    }
    let (_old_manager, _old_scope, old) = open_version(temp.path(), 11);
    assert_eq!(old.database.schema_version().unwrap(), 11);
    assert_eq!(old.subject(&original.id).unwrap(), Some(original));
    old.database.try_with_connection::<_, SubjektivError>(|connection| {
        let created: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='subject_permission_changes')", [], |row| row.get(0))?;
        assert!(!created);
        Ok(())
    }).unwrap();
}
