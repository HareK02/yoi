//! Local Host -> real Subject Memory store checks. The fixture replaces only
//! generic Job attempt admission; the writer lifetime, operation/context checks
//! and Memory service are production code. No model or live store is used.
use super::*;
use std::sync::mpsc;
use std::time::Duration;

use server_api::authorization::{CurrentSubjectPolicy, SubjectPermissions};

struct MutablePolicy {
    workspace: String,
    subject: String,
    current: Mutex<Option<CurrentSubjectPolicy>>,
}
impl subjektiv::CanonicalPermissionSource for MutablePolicy {
    fn subject_permissions(
        &self,
        _: &str,
        _: &str,
    ) -> subjektiv::Result<Option<SubjectPermissions>> {
        panic!("the local consumer must not use the legacy policy projection")
    }
    fn current_subject_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<CurrentSubjectPolicy>> {
        assert_eq!(workspace, self.workspace);
        assert_eq!(subject, self.subject);
        Ok(self.current.lock().unwrap().clone())
    }
}

fn canonical_fixture() -> (ConsolidationRecoveryFixture, Arc<MutablePolicy>) {
    let mut provider = None;
    let fixture = ConsolidationRecoveryFixture::with_policy(&["candidate"], |catalog, subject| {
        let source = Arc::new(MutablePolicy {
            workspace: catalog.scope_id.clone(),
            subject: subject.id.clone(),
            current: Mutex::new(catalog.store.current_subject_policy(&subject.id).unwrap()),
        });
        let identity = catalog.store.database_identity().unwrap();
        let preview = catalog
            .store
            .preview_permission_transfer(&identity)
            .unwrap();
        catalog
            .store
            .seal_permission_sources(&identity, "local-test-transfer", &preview.export_digest)
            .unwrap();
        catalog.store = catalog
            .store
            .clone()
            .with_canonical_permission_source(source.clone());
        provider = Some(source);
    });
    (fixture, provider.unwrap())
}

struct OpenFence;
impl crate::jobs::JobAttemptFence for OpenFence {
    fn enter(&self) -> Result<Box<dyn crate::jobs::JobEffectGuard + '_>, String> {
        Ok(Box::new(()))
    }
    fn ensure_live(&self) -> Result<(), String> {
        Ok(())
    }
}

fn job_host(
    fixture: &ConsolidationRecoveryFixture,
    fence: Arc<dyn crate::jobs::JobAttemptFence>,
) -> LocalConsolidationHost {
    LocalConsolidationHost {
        host: fixture.body.host.clone(),
        grant: LocalConsolidationGrant {
            binding: fixture.body.binding.clone(),
            candidate_ids: vec!["candidate".into()],
            operations: None,
        },
        job_id: fixture.saved.request.job_id.clone(),
        attempt_id: fixture.saved.attempt.attempt_id.clone(),
        fence,
    }
}

fn context(job: &LocalConsolidationHost) -> SubjektivHostContext {
    let attempt = format!("job:{}", job.attempt_id);
    SubjektivHostContext {
        worker_id: attempt.clone(),
        session_id: attempt,
    }
}

fn read_candidate() -> SubjektivMemoryBackendOperation {
    SubjektivMemoryBackendOperation::ReadCandidate(SubjektivMemoryCandidateReadRequest {
        candidate_id: "candidate".into(),
    })
}

#[test]
fn consolidation_input_supply_rechecks_policy_after_grant_preparation() {
    use worker::job::JobInputAuthority;
    let (fixture, source) = canonical_fixture();
    let job = job_host(&fixture, Arc::new(OpenFence));
    assert!(futures::executor::block_on(job.authorize_supply()).is_ok());
    source
        .current
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .execution
        .revoked = true;
    assert!(futures::executor::block_on(job.authorize_supply()).is_err());
}

#[test]
fn consolidation_service_reads_bound_candidate_but_rejects_foreign_attempt_context() {
    let fixture = ConsolidationRecoveryFixture::new();
    let job = job_host(&fixture, Arc::new(OpenFence));
    let bound = context(&job);
    assert!(matches!(
        job.memory(&bound, read_candidate()).unwrap(),
        SubjektivMemoryBackendResponse::Candidate(_)
    ));
    for foreign in [
        SubjektivHostContext {
            worker_id: "job:foreign".into(),
            ..bound.clone()
        },
        SubjektivHostContext {
            session_id: "job:foreign".into(),
            ..bound.clone()
        },
        SubjektivHostContext {
            worker_id: fixture.body.host.worker_key.clone(),
            session_id: session_store::SessionId::now_v7().to_string(),
        },
    ] {
        assert!(matches!(
            job.memory(&foreign, read_candidate()),
            Err(SubjektivHostError::PermissionDenied(_))
        ));
    }
    assert!(matches!(
        job.memory(
            &bound,
            SubjektivMemoryBackendOperation::ResidentContext(
                memory::backend::MemoryResidentSummaryOperation {}
            )
        ),
        Err(SubjektivHostError::PermissionDenied(_))
    ));
}

struct WaitingFence {
    entered: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl crate::jobs::JobAttemptFence for WaitingFence {
    fn enter(&self) -> Result<Box<dyn crate::jobs::JobEffectGuard + '_>, String> {
        self.entered.send(()).map_err(|e| e.to_string())?;
        self.resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        Ok(Box::new(()))
    }
    fn ensure_live(&self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn canonical_revocation_during_attempt_wait_prevents_candidate_delivery() {
    // Only the canonical policy owner is substituted. Transfer sealing, exact
    // scope/Subject lookup and the operation consumer are real implementations.
    let (fixture, source) = canonical_fixture();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let job = job_host(
        &fixture,
        Arc::new(WaitingFence {
            entered: entered_tx,
            resume: Mutex::new(resume_rx),
        }),
    );
    let bound = context(&job);
    assert!(
        job.authorize_operation(&bound, "memory.staging.read")
            .is_ok()
    );
    std::thread::scope(|scope| {
        let call = scope.spawn(|| job.memory(&bound, read_candidate()));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        source
            .current
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .execution
            .revoked = true;
        resume_tx.send(()).unwrap();
        assert!(matches!(
            call.join().unwrap(),
            Err(SubjektivHostError::PermissionDenied(_))
        ));
    });
}

#[test]
fn canonical_empty_unset_and_foreign_target_never_restore_default_authority() {
    use server_api::authorization::{PermissionResource, ScopedPermission};
    let (fixture, source) = canonical_fixture();
    let job = job_host(&fixture, Arc::new(OpenFence));
    let bound = context(&job);
    assert!(job.memory(&bound, read_candidate()).is_ok());
    let original = source.current.lock().unwrap().clone().unwrap();
    source.current.lock().unwrap().as_mut().unwrap().execution = Default::default();
    assert!(matches!(
        job.memory(&bound, read_candidate()),
        Err(SubjektivHostError::PermissionDenied(_))
    ));
    *source.current.lock().unwrap() = None;
    assert!(matches!(
        job.memory(&bound, read_candidate()),
        Err(SubjektivHostError::PermissionDenied(_))
    ));
    let legacy = SubjectPermissions {
        change_id: "foreign-limited-policy".into(),
        execution: vec![ScopedPermission {
            operation: "memory.staging.read".into(),
            resource: PermissionResource::Subject {
                workspace_id: source.workspace.clone(),
                subject_id: "foreign-subject".into(),
            },
        }],
        delegation: vec![],
    };
    *source.current.lock().unwrap() = Some(CurrentSubjectPolicy::from_legacy(legacy).unwrap());
    assert!(matches!(
        job.memory(&bound, read_candidate()),
        Err(SubjektivHostError::PermissionDenied(_))
    ));
    *source.current.lock().unwrap() = Some(original);
    assert!(job.memory(&bound, read_candidate()).is_ok());
}

#[test]
fn current_children_and_feature_disable_apply_to_existing_local_connection() {
    use ::feature_permissions::{
        NamespaceId, NamespacePermissions, PermissionAssignment, PermissionKey, PermissionReference,
    };
    use worker::feature::FeatureId;
    let (fixture, source) = canonical_fixture();
    let host = &fixture.body.host;
    let context = SubjektivHostContext {
        worker_id: host.worker_key.clone(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let query = PermissionKey::parse("memory.query").unwrap();
    source.current.lock().unwrap().as_mut().unwrap().execution = PermissionAssignment {
        grants: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("memory").unwrap(),
            keys: vec![query.clone()],
        })],
        ..Default::default()
    };
    let deep = PermissionKey::parse("memory.query.extra.deep").unwrap();
    {
        let mut registry = host.catalog.permissions.lock().unwrap();
        let handle = registry
            .namespace_handle(
                &FeatureId::builtin("memory"),
                &NamespaceId::parse("memory").unwrap(),
            )
            .unwrap()
            .clone();
        let mut declaration = registry
            .host()
            .catalog()
            .namespaces
            .into_iter()
            .find(|ns| ns.namespace.as_str() == "memory")
            .unwrap()
            .declaration;
        for key in [
            deep.clone(),
            PermissionKey::parse("memory.query.unrelated").unwrap(),
        ] {
            declaration
                .permissions
                .push(::feature_permissions::PermissionDefinition {
                    key,
                    description: "explicit local test Feature operation".into(),
                    children: vec![],
                });
        }
        registry.update(&handle, declaration).unwrap();
    }
    assert!(host.authorize_operation(&context, deep.as_str()).is_err());
    assert!(
        host.authorize_operation(&context, "memory.query.unrelated")
            .is_err()
    );
    assert!(host.authorize_operation(&context, "memory.query").is_ok());
    assert!(host.authorize_operation(&context, "memory.read").is_err());
    let read = PermissionKey::parse("memory.read").unwrap();
    let declaration = {
        let mut registry = host.catalog.permissions.lock().unwrap();
        let handle = registry
            .namespace_handle(
                &FeatureId::builtin("memory"),
                &NamespaceId::parse("memory").unwrap(),
            )
            .unwrap()
            .clone();
        let mut declaration = registry
            .host()
            .catalog()
            .namespaces
            .into_iter()
            .find(|ns| ns.namespace.as_str() == "memory")
            .unwrap()
            .declaration;
        let original = declaration.clone();
        declaration
            .permissions
            .iter_mut()
            .find(|entry| entry.key == query)
            .unwrap()
            .children
            .extend([read, deep.clone()]);
        registry.update(&handle, declaration).unwrap();
        original
    };
    assert!(host.authorize_operation(&context, "memory.read").is_ok());
    assert!(host.authorize_operation(&context, deep.as_str()).is_ok());
    assert!(
        host.authorize_operation(&context, "memory.query.unrelated")
            .is_err()
    );
    {
        let mut registry = host.catalog.permissions.lock().unwrap();
        registry
            .set_enabled(&FeatureId::builtin("memory"), false)
            .unwrap();
    }
    assert!(host.authorize_operation(&context, "memory.read").is_err());
    {
        let mut registry = host.catalog.permissions.lock().unwrap();
        registry
            .set_enabled(&FeatureId::builtin("memory"), true)
            .unwrap();
        let handle = registry
            .namespace_handle(
                &FeatureId::builtin("memory"),
                &NamespaceId::parse("memory").unwrap(),
            )
            .unwrap()
            .clone();
        registry.update(&handle, declaration).unwrap();
    }
    assert!(host.authorize_operation(&context, "memory.query").is_ok());
    assert!(host.authorize_operation(&context, "memory.read").is_err());
    assert!(host.authorize_operation(&context, deep.as_str()).is_err());
}

#[test]
fn newly_frozen_job_grant_does_not_grow_when_parent_set_expands() {
    use ::feature_permissions::{
        NamespaceId, NamespacePermissions, PermissionAssignment, PermissionKey, PermissionReference,
    };
    let (fixture, source) = canonical_fixture();
    let original = source.current.lock().unwrap().clone().unwrap();
    source.current.lock().unwrap().as_mut().unwrap().execution = PermissionAssignment {
        grants: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("memory").unwrap(),
            keys: vec![PermissionKey::parse("memory.query").unwrap()],
        })],
        ..Default::default()
    };
    let mut job = job_host(&fixture, Arc::new(OpenFence));
    job.grant.operations = Some(
        fixture
            .body
            .host
            .freeze_operations(CONSOLIDATION_OPERATIONS)
            .unwrap(),
    );
    job.grant = serde_json::from_slice(&serde_json::to_vec(&job.grant).unwrap()).unwrap();
    assert_eq!(job.grant.operations.as_deref().unwrap(), &["memory.query"]);
    let bound = context(&job);
    *source.current.lock().unwrap() = Some(original);
    assert!(job.authorize_operation(&bound, "memory.query").is_ok());
    assert!(matches!(
        job.memory(&bound, read_candidate()),
        Err(SubjektivHostError::PermissionDenied(_))
    ));
    job.grant.operations = Some(vec![]);
    assert!(job.authorize_operation(&bound, "memory.query").is_err());
    let mut old = serde_json::to_value(&job.grant).unwrap();
    old.as_object_mut().unwrap().remove("operations");
    job.grant = serde_json::from_value(old).unwrap();
    assert!(job.memory(&bound, read_candidate()).is_ok());
    assert!(
        job.authorize_operation(&bound, "memory.resident.read")
            .is_err()
    );
}

#[test]
fn session_registration_revocation_after_preflight_does_not_create_attribution() {
    let (fixture, source) = canonical_fixture();
    let host = &fixture.body.host;
    let context = SubjektivHostContext {
        worker_id: host.worker_key.clone(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let revoke = source.clone();
    *host.registration_before_attribution.lock().unwrap() = Some(Box::new(move || {
        revoke
            .current
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .execution
            .revoked = true;
    }));
    assert_eq!(
        host.record_session(&context, true)
            .unwrap_err()
            .session_registration_cause(),
        SessionRegistrationCause::denied(SessionPrincipal::Subject)
    );
    assert!(
        host.catalog
            .store
            .session_attribution(&context.session_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn new_subject_shared_assignment_and_normal_read_survive_reopen_and_same_body_restore() {
    use ::feature_permissions::PermissionReference;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&root).unwrap();
    let subject = catalog.create("restore-owner", None).unwrap();
    let policy = catalog
        .store
        .current_subject_policy(&subject.id)
        .unwrap()
        .unwrap();
    assert!(
        matches!(policy.execution.grants.as_slice(), [PermissionReference::Set(set)] if set.as_str() == "subject-default")
    );
    let id = protocol::WorkerId::now_v7();
    let mut first = catalog.connect(&subject.id, id).unwrap();
    let context = SubjektivHostContext {
        worker_id: first.host.worker_key.clone(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let resident = || {
        SubjektivMemoryBackendOperation::ResidentContext(
            memory::backend::MemoryResidentSummaryOperation {},
        )
    };
    assert!(first.host.memory(&context, resident()).is_ok());
    first.close().unwrap();
    let restored = StandaloneSubjects::open_existing(&root).unwrap();
    assert_eq!(
        restored.store.current_subject_policy(&subject.id).unwrap(),
        Some(policy)
    );
    let mut second = restored.connect(&subject.id, id).unwrap();
    assert!(second.host.memory(&context, resident()).is_ok());
    second.close().unwrap();
}

#[test]
fn consolidation_service_rechecks_writer_after_attempt_admission_wait() {
    let fixture = ConsolidationRecoveryFixture::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let job = job_host(
        &fixture,
        Arc::new(WaitingFence {
            entered: entered_tx,
            resume: Mutex::new(resume_rx),
        }),
    );
    let bound = context(&job);
    assert!(
        job.authorize_operation(&bound, "memory.staging.read")
            .is_ok()
    );
    std::thread::scope(|scope| {
        let call = scope.spawn(|| job.memory(&bound, read_candidate()));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        fixture
            .body
            .host
            .catalog
            .store
            .release_journal_writer(&fixture.body.host.writer)
            .unwrap();
        resume_tx.send(()).unwrap();
        assert!(matches!(
            call.join().unwrap(),
            Err(SubjektivHostError::Unavailable(reason))
                if reason == "Subject writer binding was released"
        ));
    });
    assert!(
        fixture
            .body
            .host
            .catalog
            .store
            .staging_candidate(&fixture.body.binding.subject_id, "candidate")
            .unwrap()
            .is_some()
    );
}
