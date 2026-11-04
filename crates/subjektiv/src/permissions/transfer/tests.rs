//! Real FeatureDatabase tests. No Server adoption, Host authentication or cross-DB atomicity claim.
use super::*;
use crate::tests::{open_store, role};
use server_api::authorization::{
    PermissionResource, ScopedPermission, SubjectPermissionsUpdate, WorkerExecutionGrant,
};
use std::collections::BTreeMap;
use std::sync::{Barrier, Mutex};

fn read_permission() -> ScopedPermission {
    ScopedPermission {
        operation: "file.read".into(),
        resource: PermissionResource::Workdir {
            working_directory_id: "wd".into(),
            path: ".".into(),
            recursive: true,
        },
    }
}

fn replace(
    store: &SubjektivStore,
    subject: &str,
    operation: &str,
    delegation: Vec<ScopedPermission>,
) -> (SubjectPermissionsUpdate, SubjectPermissions) {
    let update = SubjectPermissionsUpdate {
        operation_id: operation.into(),
        expected_change_id: store
            .subject_permissions(subject)
            .unwrap()
            .map(|source| source.change_id),
        execution: vec![],
        delegation,
    };
    let source = store.set_subject_permissions(subject, &update).unwrap();
    (update, source)
}

fn child(subject: &str, source: &str, worker: &str) -> WorkerExecutionGrant {
    WorkerExecutionGrant {
        subject_id: subject.into(),
        source_change_id: source.into(),
        runtime_id: "runtime".into(),
        worker_id: worker.into(),
        origin_runtime_id: "runtime".into(),
        origin_worker_id: "host-authenticated-root".into(),
        profile: "builtin:coder".into(),
        profile_digest: "resolved-profile".into(),
        permissions: vec![read_permission()],
        redelegation: vec![],
    }
}

fn counts(store: &SubjektivStore) -> Vec<i64> {
    store
        .database
        .try_with_connection::<_, SubjektivError>(|connection| {
            [
                "subject_permissions",
                "subject_permission_changes",
                "subject_permission_receipts",
                "worker_execution_grants",
            ]
            .iter()
            .map(|table| {
                Ok(
                    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .collect()
        })
        .unwrap()
}

fn seal(store: &SubjektivStore) -> PermissionTransferReceipt {
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    store
        .seal_permission_sources(
            &identity,
            "server-request-fingerprint",
            &preview.export_digest,
        )
        .unwrap()
}

#[test]
fn identity_dto_round_trips_the_native_contract_and_checks_every_expected_component() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let identity = store.database_identity().unwrap();
    let native = store.database.identity();
    assert_eq!(identity.scope_id, native.scope);
    assert_eq!(identity.feature_id, native.feature);
    assert_eq!(identity.physical_id, native.physical);
    let encoded = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "scope_id": native.scope,
            "feature_id": native.feature,
            "physical_id": native.physical,
        })
    );
    let decoded: DatabaseIdentity = serde_json::from_value(encoded).unwrap();
    let restored = decoded.storage_identity();
    assert_eq!(restored.scope, native.scope);
    assert_eq!(restored.feature, native.feature);
    assert_eq!(restored.physical, native.physical);
    drop(store.database.guard(&restored).unwrap());
    let preview = store.preview_permission_transfer(&decoded).unwrap();
    for component in ["scope", "feature", "physical"] {
        let mut wrong = decoded.clone();
        match component {
            "scope" => wrong.scope_id.push_str("-different"),
            "feature" => wrong.feature_id.push_str("-different"),
            "physical" => wrong.physical_id.push_str("-different"),
            _ => unreachable!(),
        }
        assert!(matches!(
            store.preview_permission_transfer(&wrong),
            Err(SubjektivError::PermissionTransferConflict)
        ));
        assert!(matches!(
            store.seal_permission_sources(&wrong, "transfer", &preview.export_digest),
            Err(SubjektivError::PermissionTransferConflict)
        ));
        assert!(matches!(
            store.with_permission_authority_guard::<()>(&wrong, |_| {
                panic!("an identity mismatch must not enter the guarded action")
            }),
            Err(SubjektivError::PermissionTransferConflict)
        ));
        assert!(matches!(
            store.permission_transfer_transaction::<()>(&wrong, |_| {
                panic!("an identity mismatch must not enter the SQL transaction body")
            }),
            Err(SubjektivError::PermissionTransferConflict)
        ));
    }
    assert!(
        !store
            .database
            .try_with_connection::<_, SubjektivError>(|c| has_table(
                c,
                "permission_source_transfer_receipt"
            ))
            .unwrap()
    );
}

#[test]
fn complete_export_preserves_unset_empty_limited_grants_and_history_and_replays_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, scope, store) = open_store(temp.path(), "workspace-a");
    let unset = store.create_subject(role()).unwrap().id;
    let empty = store.create_subject(role()).unwrap().id;
    let limited = store.create_subject(role()).unwrap().id;
    store
        .database
        .try_transaction::<_, SubjektivError>(|tx| {
            tx.execute(
                "DELETE FROM subject_permissions WHERE subject_id=?1",
                [&unset],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, empty_source) = replace(&store, &empty, "empty", vec![]);
    let (old_update, limited_source) =
        replace(&store, &limited, "limited", vec![read_permission()]);
    let grant = child(&limited, &limited_source.change_id, "historical-child");
    store.bind_worker_execution_grant(&grant).unwrap();
    let before = counts(&store);
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    assert_eq!(preview.sources.len(), 3);
    assert!(
        preview
            .sources
            .windows(2)
            .all(|pair| pair[0].subject_id < pair[1].subject_id)
    );
    assert_eq!(
        preview
            .sources
            .iter()
            .find(|row| row.subject_id == unset)
            .unwrap()
            .permissions,
        None
    );
    assert_eq!(
        preview
            .sources
            .iter()
            .find(|row| row.subject_id == empty)
            .unwrap()
            .permissions,
        Some(empty_source)
    );
    assert_eq!(
        preview
            .sources
            .iter()
            .find(|row| row.subject_id == limited)
            .unwrap()
            .permissions,
        Some(limited_source.clone())
    );
    assert_eq!(
        preview,
        store.preview_permission_transfer(&identity).unwrap()
    );
    // Preview is physically read-only, including the domain schema.
    assert!(
        !store
            .database
            .try_with_connection::<_, SubjektivError>(|c| has_table(
                c,
                "permission_source_transfer_receipt"
            ))
            .unwrap()
    );
    let receipt = store
        .seal_permission_sources(&identity, "transfer", &preview.export_digest)
        .unwrap();
    assert_eq!(receipt.snapshot, preview);
    assert_eq!(counts(&store), before);
    assert_eq!(
        store
            .worker_execution_grant("runtime", "historical-child")
            .unwrap(),
        Some(grant.clone())
    );
    store.bind_worker_execution_grant(&grant).unwrap(); // Historical replay, not source authority.
    assert!(matches!(
        store.set_subject_permissions(&limited, &old_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert!(matches!(
        store.set_managed_subject_permissions(&limited, &old_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert!(matches!(
        store.seal_permission_sources(&identity, "different", &preview.export_digest),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(matches!(
        store.seal_permission_sources(&identity, "transfer", "different-export"),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    let mut wrong_identity = identity.clone();
    wrong_identity.scope_id = "workspace-b".into();
    assert!(matches!(
        store.seal_permission_sources(&wrong_identity, "transfer", &preview.export_digest),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(matches!(
        store.preview_permission_transfer(&wrong_identity),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    drop(store);
    scope.shutdown().unwrap();
    drop(scope);
    drop(manager);
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert_eq!(identity, store.database_identity().unwrap());
    assert_eq!(
        receipt,
        store
            .seal_permission_sources(&identity, "transfer", &preview.export_digest)
            .unwrap()
    );
    assert_eq!(
        preview,
        store.preview_permission_transfer(&identity).unwrap()
    );
    assert_eq!(counts(&store), before);
    assert!(matches!(
        store.subject_permissions(&limited),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
}

#[test]
fn preview_cas_rejects_old_writer_change_without_sealing_or_installing_schema() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    replace(&store, &subject, "old-writer-after-preview", vec![]);
    assert!(matches!(
        store.seal_permission_sources(&identity, "transfer", &preview.export_digest),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(
        !store
            .database
            .try_with_connection::<_, SubjektivError>(|c| is_sealed(c))
            .unwrap()
    );
    assert!(
        !store
            .database
            .try_with_connection::<_, SubjektivError>(|c| has_table(
                c,
                "permission_source_transfer_receipt"
            ))
            .unwrap()
    );
    replace(&store, &subject, "still-writable", vec![]);
    let fresh = store.preview_permission_transfer(&identity).unwrap();
    assert_ne!(fresh.export_digest, preview.export_digest);
    store
        .seal_permission_sources(&identity, "new-request", &fresh.export_digest)
        .unwrap();
}

#[test]
fn receipt_failure_rolls_back_seal_and_reopens_with_old_writer_intact() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    store
        .database
        .transaction(crate::storage_formats::install_permission_transfer)
        .unwrap();
    store.database.try_with_connection::<_, SubjektivError>(|connection| {
        connection.execute_batch("CREATE TEMP TRIGGER fail_permission_transfer BEFORE INSERT ON main.permission_source_transfer_receipt BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .seal_permission_sources(&identity, "transfer", &preview.export_digest)
            .is_err()
    );
    assert!(
        !store
            .database
            .try_with_connection::<_, SubjektivError>(|c| is_sealed(c))
            .unwrap()
    );
    assert!(
        store
            .database
            .try_with_connection::<_, SubjektivError>(|c| retained_receipt(c))
            .unwrap()
            .is_none()
    );
    replace(&store, &subject, "writer-after-rollback", vec![]);
    drop(store);
    scope.shutdown().unwrap();
    drop(scope);
    drop(manager);
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert!(store.subject_permissions(&subject).unwrap().is_some());
    seal(&store);
}

struct Canonical {
    sources: BTreeMap<String, Option<SubjectPermissions>>,
    calls: Mutex<Vec<(String, String)>>,
}
impl CanonicalPermissionSource for Canonical {
    fn subject_permissions(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<SubjectPermissions>> {
        self.calls
            .lock()
            .unwrap()
            .push((workspace.into(), subject.into()));
        if workspace != "workspace-a" {
            return Err(denied("foreign Workspace"));
        }
        self.sources
            .get(subject)
            .cloned()
            .ok_or_else(|| denied("canonical source unavailable"))
    }
}

#[test]
fn sealed_reads_and_new_grants_use_only_exact_injected_source_and_creation_does_not_write_old_policy()
 {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let (_, old) = replace(&store, &subject, "old", vec![read_permission()]);
    let receipt = seal(&store);
    assert!(matches!(
        store.subject_permissions(&subject),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        store.bind_worker_execution_grant(&child(&subject, &old.change_id, "no-provider")),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let before = counts(&store);
    let created = store
        .create_subject_idempotent(role(), "new behavior".into(), "create-after-seal")
        .unwrap();
    assert_eq!(
        created,
        store
            .create_subject_idempotent(role(), "new behavior".into(), "create-after-seal")
            .unwrap()
    );
    let non_idempotent = store.create_subject(role()).unwrap();
    assert!(
        store
            .subject_permission_enrollment(&created.id)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .subject_permission_enrollment(&non_idempotent.id)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .subject_permission_enrollment(&subject)
            .unwrap()
            .is_none()
    );
    assert_eq!(counts(&store), before);
    store.database.try_with_connection::<_, SubjektivError>(|connection| {
        for id in [&created.id, &non_idempotent.id] {
            let raw: String = connection.query_row("SELECT permission_enrollment_json FROM subject_creation_receipts WHERE subject_id=?1", [id], |row| row.get(0))?;
            let enrollment: SubjectPermissions = serde_json::from_str(&raw)?;
            assert!(!enrollment.execution.is_empty());
            assert!(source_in_connection(connection, id)?.is_none());
        }
        Ok(())
    }).unwrap();
    let canonical = SubjectPermissions {
        change_id: "canonical-change".into(),
        execution: vec![],
        delegation: vec![read_permission()],
    };
    let provider = Arc::new(Canonical {
        sources: BTreeMap::from([
            (subject.clone(), Some(canonical.clone())),
            (created.id.clone(), None),
        ]),
        calls: Mutex::new(vec![]),
    });
    let store = store.with_canonical_permission_source(provider.clone());
    assert_eq!(
        store.subject_permissions(&subject).unwrap(),
        Some(canonical.clone())
    );
    assert_eq!(store.subject_permissions(&created.id).unwrap(), None); // Never resurrect a default.
    assert!(store.subject_permissions(&non_idempotent.id).is_err()); // Provider failure propagates.
    assert!(
        store
            .bind_worker_execution_grant(&child(&subject, &old.change_id, "stale-source"))
            .is_err()
    );
    store
        .bind_worker_execution_grant(&child(&subject, &canonical.change_id, "canonical-child"))
        .unwrap();
    assert_eq!(
        store
            .list_subject_worker_execution_grants(&subject)
            .unwrap(),
        vec![child(&subject, &canonical.change_id, "canonical-child")]
    );
    assert_eq!(
        store
            .worker_execution_grant("runtime", "canonical-child")
            .unwrap(),
        Some(child(&subject, &canonical.change_id, "canonical-child"))
    );
    for sql in [
        "INSERT OR REPLACE INTO canonical_worker_execution_grants SELECT * FROM canonical_worker_execution_grants",
        "UPDATE canonical_worker_execution_grants SET source_change_id='forged'",
        "DELETE FROM canonical_worker_execution_grants",
    ] {
        assert!(
            store
                .database
                .try_with_connection::<_, SubjektivError>(|c| {
                    c.execute_batch(sql)?;
                    Ok(())
                })
                .is_err()
        );
    }
    assert!(
        provider
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(workspace, id)| workspace == "workspace-a"
                && [
                    subject.as_str(),
                    created.id.as_str(),
                    non_idempotent.id.as_str()
                ]
                .contains(&id.as_str()))
    );
    assert_eq!(
        store
            .preview_permission_transfer(&receipt.snapshot.identity)
            .unwrap(),
        receipt.snapshot
    );
    store
        .begin_subject_deletion(&subject, "delete-canonical-subject")
        .unwrap();
    let workers = store.deletion_subject_workers(&subject).unwrap();
    assert!(workers.contains(&("runtime".into(), "canonical-child".into())));
    assert!(workers.contains(&("runtime".into(), "host-authenticated-root".into())));
    assert!(
        store
            .finish_subject_deletion(&subject, "delete-canonical-subject")
            .unwrap()
            .completed
    );
    assert!(
        store
            .worker_execution_grant("runtime", "canonical-child")
            .is_err()
    );
    assert_eq!(
        store
            .preview_permission_transfer(&receipt.snapshot.identity)
            .unwrap(),
        receipt.snapshot
    );
}

#[test]
fn sql_source_writers_and_receipt_replacement_stay_fenced_but_explicit_erasure_remains_possible() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    replace(&store, &subject, "old-receipt", vec![]);
    let receipt = seal(&store);
    // Reacquire a managed store handle; tests must not infer/open physical storage paths.
    let old_writer = SubjektivStore::from_database(store.database.clone()).unwrap();
    assert!(
        old_writer
            .database
            .try_transaction::<_, SubjektivError>(|tx| {
                tx.execute(
                    "UPDATE subject_permissions SET change_id=change_id WHERE subject_id=?1",
                    [&subject],
                )?;
                Ok(())
            })
            .is_err()
    );
    drop(old_writer);
    for sql in [
        "INSERT OR REPLACE INTO subject_permissions SELECT * FROM subject_permissions",
        "UPDATE subject_permissions SET change_id=change_id",
        "DELETE FROM subject_permissions",
        "INSERT OR REPLACE INTO subject_permission_changes SELECT * FROM subject_permission_changes",
        "UPDATE subject_permission_changes SET record_json=record_json",
        "DELETE FROM subject_permission_changes",
        "INSERT OR REPLACE INTO subject_permission_receipts SELECT * FROM subject_permission_receipts",
        "UPDATE subject_permission_receipts SET request_fingerprint=request_fingerprint",
        "DELETE FROM subject_permission_receipts",
        "INSERT INTO worker_execution_grants SELECT 'runtime','late-writer',subject_id,change_id,'{}' FROM subject_permissions LIMIT 1",
        "INSERT OR REPLACE INTO permission_authority_seal SELECT * FROM permission_authority_seal",
        "UPDATE permission_authority_seal SET transfer_id='changed'",
        "DELETE FROM permission_authority_seal",
        "INSERT OR REPLACE INTO permission_source_transfer_receipt SELECT * FROM permission_source_transfer_receipt",
        "UPDATE permission_source_transfer_receipt SET receipt_json='{}'",
        "DELETE FROM permission_source_transfer_receipt",
    ] {
        assert!(
            store
                .database
                .try_with_connection::<_, SubjektivError>(|c| {
                    c.execute_batch(sql)?;
                    Ok(())
                })
                .is_err(),
            "{sql}"
        );
    }
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute("INSERT INTO subject_deletions(subject_id,operation_id,completed,started_at) VALUES(?1,'explicit-delete',0,'now')", [&subject])?;
        tx.execute("DELETE FROM subject_permissions WHERE subject_id=?1", [&subject])?;
        tx.execute("DELETE FROM subject_permission_receipts WHERE subject_id=?1", [&subject])?;
        tx.execute("DELETE FROM subject_permission_changes WHERE subject_id=?1", [&subject])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        store
            .preview_permission_transfer(&receipt.snapshot.identity)
            .unwrap(),
        receipt.snapshot
    );
    assert_eq!(
        store
            .seal_permission_sources(
                &receipt.snapshot.identity,
                &receipt.transfer_id,
                &receipt.snapshot.export_digest
            )
            .unwrap(),
        receipt
    );
}

#[test]
fn concurrent_old_writer_and_seal_serialize_instead_of_accepting_a_changed_export() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let initial = store.subject_permissions(&subject).unwrap().unwrap();
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let writer = store.clone();
    let start = barrier.clone();
    let thread = std::thread::spawn(move || {
        start.wait();
        writer.set_subject_permissions(
            &subject,
            &SubjectPermissionsUpdate {
                operation_id: "racing-old-writer".into(),
                expected_change_id: Some(initial.change_id),
                execution: vec![],
                delegation: vec![],
            },
        )
    });
    barrier.wait();
    let sealed = store.seal_permission_sources(&identity, "transfer", &preview.export_digest);
    let written = thread.join().unwrap();
    match (sealed, written) {
        (Ok(receipt), Err(SubjektivError::PermissionAuthoritySealed)) => {
            assert_eq!(receipt.snapshot, preview)
        }
        (Err(SubjektivError::PermissionTransferConflict), Ok(_)) => assert!(
            !store
                .database
                .try_with_connection::<_, SubjektivError>(is_sealed)
                .unwrap()
        ),
        other => panic!("unexpected serialized outcomes: {other:?}"),
    }
}

#[test]
fn provider_source_outside_exact_workspace_is_rejected_without_legacy_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    seal(&store);
    let provider = Arc::new(Canonical {
        sources: BTreeMap::from([(
            subject.clone(),
            Some(SubjectPermissions {
                change_id: "foreign-source".into(),
                execution: vec![ScopedPermission {
                    operation: "file.read".into(),
                    resource: PermissionResource::Workspace {
                        workspace_id: "workspace-b".into(),
                    },
                }],
                delegation: vec![read_permission()],
            }),
        )]),
        calls: Mutex::new(vec![]),
    });
    let store = store.with_canonical_permission_source(provider);
    assert!(store.subject_permissions(&subject).is_err());
    assert!(
        store
            .bind_worker_execution_grant(&child(&subject, "foreign-source", "foreign-child"))
            .is_err()
    );
    assert!(
        store
            .worker_execution_grant("runtime", "foreign-child")
            .unwrap()
            .is_none()
    );
}

#[test]
fn authority_guard_gates_legacy_callback_and_releases_an_unsealed_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let identity = store.database_identity().unwrap();
    let calls = std::cell::Cell::new(0);
    let legacy_write = || {
        store.with_permission_authority_guard(&identity, |guard| {
            assert!(
                !guard.transaction.borrow().as_ref().unwrap().is_autocommit(),
                "the Feature transaction must already be active on callback entry"
            );
            guard.require_unsealed()?;
            guard.require_subject(&subject)?;
            // Stand-in for a logical Server write/commit. Never reenter Feature storage here.
            calls.set(calls.get() + 1);
            Ok(())
        })
    };
    legacy_write().unwrap();
    assert_eq!(calls.get(), 1);
    store
        .database
        .try_with_connection::<_, SubjektivError>(|connection| {
            assert!(connection.is_autocommit());
            assert!(!has_table(
                connection,
                "permission_source_transfer_receipt"
            )?);
            assert!(!is_sealed(connection)?);
            Ok(())
        })
        .unwrap();
    // A logical write does not mutate Feature, and an early callback failure does not seal.
    let failed: Result<()> = store.with_permission_authority_guard(&identity, |guard| {
        guard.require_unsealed()?;
        Err(SubjektivError::InvalidRecord("callback before seal".into()))
    });
    assert!(matches!(failed, Err(SubjektivError::InvalidRecord(_))));
    replace(&store, &subject, "writer-after-guard-return", vec![]);
    seal(&store);
    assert!(matches!(
        legacy_write(),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    assert_eq!(calls.get(), 1, "a sealed legacy callback must not execute");
}

#[test]
fn authority_guard_metadata_checks_do_not_call_canonical_source_and_honor_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let deleting = store.create_subject(role()).unwrap().id;
    seal(&store);
    store
        .begin_subject_deletion(&deleting, "deleting-before-guard")
        .unwrap();
    let provider = Arc::new(Canonical {
        sources: BTreeMap::new(), // Any provider call would fail, and could reenter Server.
        calls: Mutex::new(vec![]),
    });
    let store = store.with_canonical_permission_source(provider.clone());
    let identity = store.database_identity().unwrap();
    store
        .with_permission_authority_guard(&identity, |guard| {
            guard.require_subject(&subject)?; // Metadata still exists even when authority is sealed.
            assert!(matches!(
                guard.require_subject("missing-subject"),
                Err(SubjektivError::SubjectNotFound(_))
            ));
            assert!(matches!(
                guard.require_subject(&deleting),
                Err(SubjektivError::SubjectNotFound(_))
            ));
            assert!(matches!(
                guard.require_unsealed(),
                Err(SubjektivError::PermissionAuthoritySealed)
            ));
            Ok(())
        })
        .unwrap();
    assert!(provider.calls.lock().unwrap().is_empty());
}

#[test]
fn authority_guard_seal_survives_callback_failure_and_exact_retry_preserves_server_error() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    let before = counts(&store);
    let failed: Result<()> = store.with_permission_authority_guard(&identity, |guard| {
        guard.require_subject(&subject)?;
        let receipt =
            guard.seal_permission_sources("server-fingerprint", &preview.export_digest)?;
        assert_eq!(receipt.snapshot, preview);
        assert!(
            guard.transaction.borrow().is_none(),
            "seal must end the Feature transaction before adoption"
        );
        assert!(matches!(
            guard.require_subject(&subject),
            Err(SubjektivError::PermissionAuthoritySealed)
        ));
        assert!(matches!(
            guard.require_unsealed(),
            Err(SubjektivError::PermissionAuthoritySealed)
        ));
        Err(SubjektivError::InvalidRecord(
            "callback after committed seal".into(),
        ))
    });
    assert!(matches!(failed, Err(SubjektivError::InvalidRecord(_))));
    let receipt = store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.transfer_id, "server-fingerprint");
    assert_eq!(receipt.snapshot, preview);
    assert_eq!(counts(&store), before);
    let nested: Result<std::result::Result<(), u16>> =
        store.with_permission_authority_guard(&identity, |guard| {
            guard.require_subject(&subject)?;
            assert_eq!(
                guard.seal_permission_sources("server-fingerprint", &preview.export_digest)?,
                receipt
            );
            assert!(
                guard.transaction.borrow().is_none(),
                "receipt retry must also end its transaction"
            );
            // Preserve the Server error/status independently of the Feature result.
            Ok(Err(409))
        });
    assert!(matches!(nested, Ok(Err(409))));
    assert_eq!(
        store.permission_transfer_receipt(&identity).unwrap(),
        Some(receipt)
    );
    for (transfer, digest) in [
        ("another-request", preview.export_digest.as_str()),
        ("server-fingerprint", "changed-export"),
    ] {
        assert!(matches!(
            store.with_permission_authority_guard(&identity, |guard| {
                guard.seal_permission_sources(transfer, digest)
            }),
            Err(SubjektivError::PermissionTransferConflict)
        ));
    }
    assert_eq!(counts(&store), before);
}

#[test]
fn authority_guard_seal_sql_failure_rolls_back_before_callback_returns() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    store.create_subject(role()).unwrap();
    let identity = store.database_identity().unwrap();
    let preview = store.preview_permission_transfer(&identity).unwrap();
    store
        .database
        .transaction(crate::storage_formats::install_permission_transfer)
        .unwrap();
    store.database.try_with_connection::<_, SubjektivError>(|connection| {
        connection.execute_batch("CREATE TEMP TRIGGER fail_guard_transfer BEFORE INSERT ON main.permission_source_transfer_receipt BEGIN SELECT RAISE(ABORT,'injected guarded receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    store
        .with_permission_authority_guard(&identity, |guard| {
            assert!(
                guard
                    .seal_permission_sources("transfer", &preview.export_digest)
                    .is_err()
            );
            assert!(guard.transaction.borrow().is_none());
            // Failure consumes the transaction; a caught error must not enable an unfenced writer.
            assert!(guard.require_unsealed().is_err());
            assert!(
                guard
                    .seal_permission_sources("transfer", &preview.export_digest)
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    store
        .database
        .try_with_connection::<_, SubjektivError>(|connection| {
            assert!(connection.is_autocommit());
            assert!(!is_sealed(connection)?);
            assert!(retained_receipt(connection)?.is_none());
            connection.execute_batch("DROP TRIGGER fail_guard_transfer")?;
            Ok(())
        })
        .unwrap();
    store
        .with_permission_authority_guard(&identity, |guard| {
            guard.require_unsealed()?;
            guard.seal_permission_sources("transfer", &preview.export_digest)
        })
        .unwrap();
}

#[test]
fn distinct_physical_database_identity_cannot_preview_or_seal_even_in_same_workspace() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let (_manager_a, _scope_a, store_a) = open_store(first.path(), "workspace-a");
    let (_manager_b, _scope_b, store_b) = open_store(second.path(), "workspace-a");
    let a = store_a.database_identity().unwrap();
    let b = store_b.database_identity().unwrap();
    assert_ne!(a, b);
    let preview = store_a.preview_permission_transfer(&a).unwrap();
    assert!(matches!(
        store_b.preview_permission_transfer(&a),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(matches!(
        store_b.seal_permission_sources(&a, "transfer", &preview.export_digest),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(matches!(
        store_b.with_permission_authority_guard::<()>(&a, |_| {
            panic!("a different physical database must not enter the guarded action")
        }),
        Err(SubjektivError::PermissionTransferConflict)
    ));
    assert!(
        !store_b
            .database
            .try_with_connection::<_, SubjektivError>(is_sealed)
            .unwrap()
    );
}
