//! Registered SQLite tests of the Feature-aware persistence boundary. The injected
//! canonical source is typed, mutable test authority; production Server-store
//! coverage lives in workspace-server::configuration_transfer_tests. A retained
//! root is Host-attested here, not authentication by absence of a parent row.
use super::*;
use crate::tests::{open_store, role};
use feature_permissions::{
    FeatureHandle, FeatureIdentity, NamespaceDeclaration, NamespaceHandle, NamespaceId,
    NamespacePermissions, PermissionAssignment, PermissionDefinition, PermissionHost,
    PermissionKey, PermissionReference, WorkspaceId,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn host(workspace: &str) -> (PermissionHost, FeatureHandle, NamespaceHandle) {
    let mut host = PermissionHost::new(WorkspaceId::parse(workspace).unwrap());
    let feature = host
        .register_feature(FeatureIdentity::parse("fixture:file").unwrap())
        .unwrap();
    let namespace = host
        .bind_namespace(&feature, NamespaceId::parse("file").unwrap())
        .unwrap();
    host.declare(&namespace, declaration(true)).unwrap();
    (host, feature, namespace)
}

fn declaration(children: bool) -> NamespaceDeclaration {
    NamespaceDeclaration {
        permissions: ["file.read", "file.read.text"]
            .into_iter()
            .map(|key| PermissionDefinition {
                key: PermissionKey::parse(key).unwrap(),
                description: String::new(),
                children: if children && key == "file.read" {
                    vec![PermissionKey::parse("file.read.text").unwrap()]
                } else {
                    vec![]
                },
            })
            .collect(),
        sets: vec![],
    }
}

fn reference(namespace: &str, key: &str) -> PermissionReference {
    PermissionReference::Namespace(NamespacePermissions {
        namespace: NamespaceId::parse(namespace).unwrap(),
        keys: vec![PermissionKey::parse(key).unwrap()],
    })
}

fn policy(change: &str) -> CurrentSubjectPolicy {
    let lane = PermissionAssignment {
        grants: vec![reference("file", "file.read")],
        ..Default::default()
    };
    CurrentSubjectPolicy {
        change_id: change.into(),
        execution: lane.clone(),
        delegation: lane,
        legacy_constraints: None,
        reference_encoding: server_api::authorization::PermissionReferenceEncoding::FeatureSetsV2,
    }
}

fn permission(operation: &str, path: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workdir {
            working_directory_id: "wd".into(),
            path: path.into(),
            recursive: false,
        },
    }
}

fn grant(subject: &str, change: &str, worker: &str) -> WorkerExecutionGrant {
    WorkerExecutionGrant {
        subject_id: subject.into(),
        source_change_id: change.into(),
        runtime_id: "runtime".into(),
        worker_id: worker.into(),
        origin_runtime_id: "runtime".into(),
        origin_worker_id: "body".into(),
        profile: "builtin:coder".into(),
        profile_digest: "fixture-resolved-profile".into(),
        permissions: vec![permission("file.read", "src/one.txt")],
        redelegation: vec![],
    }
}

fn install_feature_fixture(store: &SubjektivStore, subject: &str, policy: &CurrentSubjectPolicy) {
    // Test-only administration of the existing source/history tables, not a new
    // production writer or a legacy projection of the Feature assignment.
    store.database.try_transaction::<_, SubjektivError>(|tx| {
        tx.execute("INSERT INTO subject_permission_changes(subject_id,change_id,record_json) VALUES(?1,?2,?3)", params![subject, policy.change_id, serde_json::to_string(policy)?])?;
        tx.execute("UPDATE subject_permissions SET change_id=?2 WHERE subject_id=?1", params![subject, policy.change_id])?;
        Ok(())
    }).unwrap();
}

struct Canonical {
    subject: String,
    source: Mutex<Option<SubjectPolicySource>>,
    legacy_calls: AtomicUsize,
}
impl CanonicalPermissionSource for Canonical {
    fn subject_policy_source(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<SubjectPolicySource>> {
        if workspace != "workspace-a" || subject != self.subject {
            return Err(denied("foreign canonical query"));
        }
        Ok(self.source.lock().unwrap().clone())
    }
    fn subject_permissions(&self, _: &str, _: &str) -> Result<Option<SubjectPermissions>> {
        self.legacy_calls.fetch_add(1, Ordering::SeqCst);
        Err(denied("Feature source has no legacy all-target projection"))
    }
}

#[test]
fn effective_native_reads_normalize_without_changing_raw_transfer_or_history() {
    use server_api::authorization::PermissionReferenceEncoding;

    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let mut raw = policy("stored-v1");
    raw.reference_encoding = PermissionReferenceEncoding::LegacySubjectV1;
    raw.execution = PermissionAssignment {
        grants: vec![PermissionReference::Set(
            feature_permissions::PermissionSetId::parse("subject-default").unwrap(),
        )],
        exclusions: vec![reference("subject", "subject.orchestrate")],
        revoked: true,
    };
    raw.delegation = PermissionAssignment::default();
    install_feature_fixture(&store, &subject, &raw);
    let stored_bytes = serde_json::to_string(&raw).unwrap();
    assert!(!stored_bytes.contains("reference_encoding"));
    let identity = store.database_identity().unwrap();
    let before = store.preview_permission_transfer(&identity).unwrap();
    assert_eq!(before.sources[0].feature_policy, Some(raw.clone()));

    let expected = raw.clone().into_current_reference_encoding();
    assert_eq!(
        store.current_subject_policy(&subject).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        store.subject_policy_source(&subject).unwrap(),
        Some(SubjectPolicySource::Feature(expected))
    );
    let persisted: String = store
        .database
        .try_with_connection::<_, SubjektivError>(|connection| {
            Ok(connection.query_row(
                "SELECT record_json FROM subject_permission_changes WHERE subject_id=?1 AND change_id=?2",
                params![subject, raw.change_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(persisted, stored_bytes);
    assert_eq!(
        store.preview_permission_transfer(&identity).unwrap(),
        before
    );
    let receipt = store
        .seal_permission_sources(&identity, "raw-v1-seal", &before.export_digest)
        .unwrap();
    assert_eq!(receipt.snapshot, before);
    assert_eq!(
        store.permission_transfer_receipt(&identity).unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        store
            .seal_permission_sources(&identity, "raw-v1-seal", &before.export_digest)
            .unwrap(),
        receipt
    );
}

#[test]
fn canonical_effective_reads_normalize_without_replacing_provider_evidence() {
    use server_api::authorization::PermissionReferenceEncoding;

    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let identity = store.database_identity().unwrap();
    let snapshot = store.preview_permission_transfer(&identity).unwrap();
    store
        .seal_permission_sources(&identity, "v1-provider-seal", &snapshot.export_digest)
        .unwrap();
    let mut raw = policy("canonical-v1");
    raw.reference_encoding = PermissionReferenceEncoding::LegacySubjectV1;
    raw.delegation.grants = vec![reference("subject", "subject.delegate")];
    let provider = Arc::new(Canonical {
        subject: subject.clone(),
        source: Mutex::new(Some(SubjectPolicySource::Feature(raw.clone()))),
        legacy_calls: AtomicUsize::new(0),
    });
    let store = store.with_canonical_permission_source(provider.clone());
    let expected = raw.clone().into_current_reference_encoding();
    assert_eq!(
        store.current_subject_policy(&subject).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        store.subject_policy_source(&subject).unwrap(),
        Some(SubjectPolicySource::Feature(expected))
    );
    assert_eq!(
        *provider.source.lock().unwrap(),
        Some(SubjectPolicySource::Feature(raw))
    );
    assert_eq!(provider.legacy_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn pending_creation_enrollment_readers_retain_raw_reference_evidence() {
    use server_api::authorization::PermissionReferenceEncoding;

    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let identity = store.database_identity().unwrap();
    // Pre-seal non-idempotent creation leaves no receipt. Seed the historical
    // enrollment in a fresh row instead of rewriting immutable creation evidence.
    let subject = store.create_subject(role()).unwrap().id;
    let snapshot = store.preview_permission_transfer(&identity).unwrap();
    store
        .seal_permission_sources(&identity, "enrollment-seal", &snapshot.export_digest)
        .unwrap();
    let mut raw = policy("pending-v1");
    raw.reference_encoding = PermissionReferenceEncoding::LegacySubjectV1;
    raw.delegation.grants = vec![reference("subject", "subject.delegate")];
    let stored_bytes = serde_json::to_string(&raw).unwrap();
    store
        .database
        .try_transaction::<_, SubjektivError>(|tx| {
            tx.execute(
                "INSERT INTO subject_creation_receipts(operation_id,subject_id,request_fingerprint,permission_enrollment_json) VALUES(?1,?2,?3,?4)",
                params![
                    "pending-v1-enrollment",
                    subject,
                    "fixture-fingerprint",
                    stored_bytes
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let expected = Some(SubjectPolicySource::Feature(raw));
    assert_eq!(
        store.subject_permission_enrollment(&subject).unwrap(),
        expected
    );
    store
        .with_permission_authority_guard(&identity, |authority| {
            assert_eq!(authority.subject_permission_enrollment(&subject)?, expected);
            Ok(())
        })
        .unwrap();
    let persisted: String = store
        .database
        .try_with_connection::<_, SubjektivError>(|connection| {
            Ok(connection.query_row(
                "SELECT permission_enrollment_json FROM subject_creation_receipts WHERE subject_id=?1",
                [&subject],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(persisted, stored_bytes);
}

#[test]
fn feature_binding_uses_exact_frozen_keys_and_retained_parent_scope_without_expansion() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let source = policy("feature-change");
    install_feature_fixture(&store, &subject, &source);
    let (mut host, _feature, namespace) = host("workspace-a");
    let mut root = grant(&subject, &source.change_id, "first-child");
    root.redelegation = root.permissions.clone();
    assert!(store.bind_worker_execution_grant(&root).is_err()); // old API stays legacy-only
    store
        .bind_worker_execution_grant_with_host(&root, &host)
        .unwrap();
    assert_eq!(
        store
            .worker_execution_grant("runtime", "first-child")
            .unwrap(),
        Some(root.clone())
    );
    let mut child = grant(&subject, &source.change_id, "grandchild");
    child.origin_worker_id = root.worker_id.clone();
    store
        .bind_worker_execution_grant_with_host(&child, &host)
        .unwrap();
    let mut wider = child.clone();
    wider.worker_id = "wider".into();
    wider.permissions[0].resource = PermissionResource::Workdir {
        working_directory_id: "wd".into(),
        path: "src".into(),
        recursive: true,
    };
    assert!(
        store
            .bind_worker_execution_grant_with_host(&wider, &host)
            .is_err()
    );
    let mut expanded = child.clone();
    expanded.worker_id = "expanded".into();
    expanded.permissions[0].operation = "file.read.text".into();
    assert!(
        store
            .bind_worker_execution_grant_with_host(&expanded, &host)
            .is_err()
    ); // parent passed file.read only
    let mut direct_child_key = expanded.clone();
    direct_child_key.worker_id = "direct-key".into();
    direct_child_key.origin_worker_id = "body".into();
    store
        .bind_worker_execution_grant_with_host(&direct_child_key, &host)
        .unwrap(); // declared child in current Subject ceiling
    host.declare(&namespace, declaration(false)).unwrap();
    let mut after_removal = direct_child_key.clone();
    after_removal.worker_id = "after-child-removal".into();
    assert!(
        store
            .bind_worker_execution_grant_with_host(&after_removal, &host)
            .is_err()
    );
    assert_eq!(
        store
            .worker_execution_grant("runtime", "first-child")
            .unwrap()
            .unwrap()
            .permissions,
        root.permissions
    );
    let mut conflict = root;
    conflict.permissions.clear();
    assert!(matches!(
        store.bind_worker_execution_grant_with_host(&conflict, &host),
        Err(SubjektivError::WorkerExecutionGrantConflict(_))
    ));
}

#[test]
fn canonical_binding_honors_current_delegation_constraints_scope_replay_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (mut host, feature, _) = host("workspace-a");
    let source = policy("canonical-feature-change");
    let saved;
    let provider;
    {
        let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
        let subject = store.create_subject(role()).unwrap().id;
        let identity = store.database_identity().unwrap();
        let snapshot = store.preview_permission_transfer(&identity).unwrap();
        store
            .seal_permission_sources(&identity, "canonical-fixture-seal", &snapshot.export_digest)
            .unwrap();
        provider = Arc::new(Canonical {
            subject: subject.clone(),
            source: Mutex::new(Some(SubjectPolicySource::Feature(source.clone()))),
            legacy_calls: AtomicUsize::new(0),
        });
        let store = store.with_canonical_permission_source(provider.clone());
        saved = grant(&subject, &source.change_id, "saved");
        store
            .bind_worker_execution_grant_with_host(&saved, &host)
            .unwrap();
        let old = store
            .database
            .try_with_connection::<_, SubjektivError>(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*) FROM subject_permission_changes WHERE change_id=?1",
                    [&source.change_id],
                    |row| row.get::<_, usize>(0),
                )?)
            })
            .unwrap();
        assert_eq!(old, 0); // canonical IDs were not mirrored into Feature source/history
        let (wrong_host, _, _) = self::host("workspace-b");
        assert!(
            store
                .bind_worker_execution_grant_with_host(&saved, &wrong_host)
                .is_err()
        );
        for (index, denied_source) in [
            CurrentSubjectPolicy {
                delegation: PermissionAssignment::default(),
                ..source.clone()
            },
            CurrentSubjectPolicy {
                delegation: PermissionAssignment {
                    revoked: true,
                    ..source.delegation.clone()
                },
                ..source.clone()
            },
            CurrentSubjectPolicy {
                delegation: PermissionAssignment {
                    exclusions: vec![reference("file", "file.read")],
                    ..source.delegation.clone()
                },
                ..source.clone()
            },
            CurrentSubjectPolicy {
                delegation: PermissionAssignment {
                    grants: vec![reference("wrong", "file.read")],
                    ..Default::default()
                },
                ..source.clone()
            },
            CurrentSubjectPolicy {
                delegation: PermissionAssignment {
                    grants: vec![reference("file", "file.unknown")],
                    ..Default::default()
                },
                ..source.clone()
            },
            CurrentSubjectPolicy {
                change_id: "replaced".into(),
                ..source.clone()
            },
        ]
        .into_iter()
        .enumerate()
        {
            *provider.source.lock().unwrap() =
                Some(SubjectPolicySource::Feature(denied_source.clone()));
            let mut requested = saved.clone();
            requested.worker_id = format!("denied-{index}");
            assert!(
                store
                    .bind_worker_execution_grant_with_host(&requested, &host)
                    .is_err()
            );
            store
                .bind_worker_execution_grant_with_host(&saved, &host)
                .unwrap(); // historical, no policy restoration
            assert_eq!(
                *provider.source.lock().unwrap(),
                Some(SubjectPolicySource::Feature(denied_source.clone()))
            );
        }
        *provider.source.lock().unwrap() = Some(SubjectPolicySource::Feature(source.clone()));
        host.set_enabled(&feature, false).unwrap();
        let mut fresh = saved.clone();
        fresh.worker_id = "disabled".into();
        assert!(
            store
                .bind_worker_execution_grant_with_host(&fresh, &host)
                .is_err()
        );
        store
            .bind_worker_execution_grant_with_host(&saved, &host)
            .unwrap();
        host.set_enabled(&feature, true).unwrap();
        let mut limited = source.clone();
        limited.legacy_constraints = Some(SubjectPermissions {
            change_id: "old-limited-change".into(),
            execution: vec![],
            delegation: vec![permission("file.read", "only.txt")],
        });
        *provider.source.lock().unwrap() = Some(SubjectPolicySource::Feature(limited));
        fresh.worker_id = "domain-too-wide".into();
        assert!(
            store
                .bind_worker_execution_grant_with_host(&fresh, &host)
                .is_err()
        );
        fresh.permissions[0] = permission("file.read", "only.txt");
        fresh.worker_id = "domain-exact".into();
        store
            .bind_worker_execution_grant_with_host(&fresh, &host)
            .unwrap();
        fresh.permissions[0].resource = PermissionResource::Subject {
            workspace_id: "workspace-b".into(),
            subject_id: subject.clone(),
        };
        fresh.worker_id = "foreign-resource".into();
        assert!(
            store
                .bind_worker_execution_grant_with_host(&fresh, &host)
                .is_err()
        );
        *provider.source.lock().unwrap() = None;
        fresh.worker_id = "removed".into();
        fresh.permissions = saved.permissions.clone();
        assert!(
            store
                .bind_worker_execution_grant_with_host(&fresh, &host)
                .is_err()
        );
        assert_eq!(provider.legacy_calls.load(Ordering::SeqCst), 0);
    }
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let store = store.with_canonical_permission_source(provider.clone());
    assert_eq!(
        store.worker_execution_grant("runtime", "saved").unwrap(),
        Some(saved.clone())
    );
    store
        .bind_worker_execution_grant_with_host(&saved, &host)
        .unwrap();
    let mut fresh = saved.clone();
    fresh.worker_id = "reopen-unset".into();
    assert!(
        store
            .bind_worker_execution_grant_with_host(&fresh, &host)
            .is_err()
    );
    assert_eq!(*provider.source.lock().unwrap(), None);
    store
        .begin_subject_deletion(&saved.subject_id, "delete")
        .unwrap();
    assert!(
        store
            .bind_worker_execution_grant_with_host(&saved, &host)
            .is_err()
    );
    store
        .finish_subject_deletion(&saved.subject_id, "delete")
        .unwrap();
    assert!(
        store
            .bind_worker_execution_grant_with_host(&saved, &host)
            .is_err()
    );
}
