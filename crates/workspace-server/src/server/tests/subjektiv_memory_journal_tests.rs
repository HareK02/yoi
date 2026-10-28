//! Connected ordinary library tests: production router, signed Runtime authentication,
//! singleton/Job authority and Subjektiv FeatureStorage/SQLite. Extraction output and
//! Host admission/generation are fixture inputs; staging/adoption and canonical Session
//! serialization are real. No Runtime execution, model, external config or network is used.
use super::*;
use subjektiv::journal::memory_links::*;
use subjektiv::journal::transport::{JournalJobRequest, JournalTextRead, JournalTextTarget};
use subjektiv::journal::*;

const RUNTIME: &str = "runtime-memory-journal";
const JOURNAL_BODY: &str =
    "A was proposed; B was chosen after checking the constraint. Work remains.";

fn route_path() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/journal/memory")
}

async fn response(app: Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (status, bytes.to_vec())
}

struct Fixture {
    _dir: tempfile::TempDir,
    api: WorkspaceApi,
    identity: worker_runtime::auth::RuntimeIdentityMaterial,
    store: crate::subjektiv::SubjektivStore,
    host: JournalHostContext,
    job: JournalJobContext,
    worker: RuntimeWorkerRef,
    source_worker: RuntimeWorkerRef,
    source_root: PathBuf,
    selector: MemoryJournalSelector,
    journal_id: String,
    entry_id: String,
    ranges: Vec<JournalSourceRange>,
}

impl Fixture {
    fn grant_permissions(store: &crate::subjektiv::SubjektivStore, subject_id: &str) {
        store
            .set_subject_permissions(
                subject_id,
                &server_api::authorization::SubjectPermissionsUpdate {
                    operation_id: format!("memory-journal-fixture-grant-{subject_id}"),
                    expected_change_id: None,
                    execution: [
                        "journal.memory.links.reconcile",
                        "journal.memory.links.read",
                        "journal.memory.read",
                        "journal.memory.related.read",
                    ]
                    .into_iter()
                    .map(|operation| server_api::authorization::ScopedPermission {
                        operation: operation.into(),
                        resource: server_api::authorization::PermissionResource::Subject {
                            workspace_id: TEST_WORKSPACE_ID.into(),
                            subject_id: subject_id.into(),
                        },
                    })
                    .collect(),
                    delegation: vec![],
                },
            )
            .unwrap();
    }

    async fn new() -> Self {
        Self::with_permissions(true).await
    }

    async fn with_permissions(granted: bool) -> Self {
        use session_store::{LogEntry, Store as _, WorkerSessionStore};
        let dir = tempfile::tempdir().unwrap();
        let mut api = test_api(dir.path()).await;
        let identity = worker_runtime::auth::RuntimeIdentityMaterial::generate(RUNTIME).unwrap();
        configure_runtime_request_auth(&mut api, &identity, RUNTIME);
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("memory-journal").unwrap())
            .unwrap();
        if granted {
            Self::grant_permissions(&store, &subject.id);
        }
        let worker = Self::body_worker(&api, &subject.id);
        let source_worker = RuntimeWorkerRef::new(RUNTIME, Uuid::new_v4().to_string());
        seed_worker_source_member(&api, RUNTIME, &source_worker.worker_id);
        let session = session_store::new_session_id();
        let segment = Uuid::new_v4();
        let source_root = dir.path().join("old-worker/session");
        let history = [
            "Choose B after checking the constraint",
            "Correction: B still needs validation",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, text)| session_store::LoggedHistoryEntry {
            item: session_store::LoggedItem::Message {
                role: session_store::LoggedRole::User,
                content: vec![session_store::LoggedContentPart::Text { text: text.into() }],
            },
            metadata: session_store::LoggedSessionHistoryMetadata {
                input_metadata: None,
                entry_id: session_store::LoggedSessionHistoryEntryId(format!("original-{index}")),
                origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
            },
        })
        .collect();
        WorkerSessionStore::new(&source_root)
            .unwrap()
            .create_segment(
                session,
                segment,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id: session,
                    system_prompt: None,
                    config: agen::llm_client::RequestConfig::default(),
                    history,
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        let reader = session_store::public_material::SessionPublicMaterialReader::open_worker(
            &source_root,
            segment,
            Default::default(),
        )
        .unwrap();
        let ranges: Vec<_> = reader
            .descriptors()
            .map(|d| JournalSourceRange {
                source: JournalSource {
                    session_id: session.to_string(),
                    pointer: JournalSourcePointer {
                        segment_id: d.identity.segment_id,
                        entry_ref: d.identity.entry_ref.clone(),
                    },
                    order: d.canonical_order,
                    content_sha256: d.sha256.clone(),
                    total_bytes: d.total_bytes,
                },
                bytes: JournalByteRange {
                    start: 0,
                    end: d.total_bytes,
                },
            })
            .collect();
        assert_eq!(ranges.len(), 2);
        let attr = store
            .record_session_attribution(
                crate::subjektiv::SubjectSessionAttribution::new(
                    &subject.id,
                    RUNTIME,
                    &source_worker.worker_id,
                    session.to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        let writer = store
            .claim_journal_writer(&subject.id, RUNTIME, &source_worker.worker_id)
            .unwrap();
        let host = store.bind_journal_session(&writer, attr.clone()).unwrap();
        let journal_id = store
            .accept_journal_sources(&host, "original activity", &ranges)
            .unwrap();
        let binding = subjektiv::api::JobAttemptBinding {
            job_id: "fixture-journal-generation".into(),
            attempt_id: "fixture-attempt".into(),
        };
        let generation = store
            .prepare_journal_generation(&host, &journal_id, binding.clone(), ranges.clone())
            .unwrap();
        let job = JournalJobContext::validated(&store, &host, &generation.id, binding).unwrap();
        for (descriptor, range) in reader.descriptors().zip(&ranges) {
            subjektiv::journal::service::execute_bounded(
                &store,
                &job,
                JournalJobRequest::ReadText {
                    read: JournalTextRead {
                        target: JournalTextTarget::Source {
                            pointer: range.source.pointer.clone(),
                        },
                        offset: 0,
                        limit: MAX_JOURNAL_READ_BYTES,
                        content_token: None,
                    },
                },
                |source, start, limit| {
                    assert_eq!(source, &range.source);
                    let page = reader.read_content(descriptor, start, limit).unwrap();
                    assert_eq!(page.next_offset, range.bytes.end);
                    Ok(subjektiv::journal::recording::JournalMaterial {
                        range: range.clone(),
                        text: page.content,
                    })
                },
            )
            .unwrap();
        }
        let receipt = store
            .execute_journal(
                &job,
                JournalOperation {
                    request_id: "initial-open".into(),
                    expected_state: generation.initial_journal.state_token,
                    action: JournalAction::Create {
                        title: "Choosing the approach".into(),
                        body: JOURNAL_BODY.into(),
                        ongoing: "Validation pending".into(),
                        state: JournalEntryState::Open,
                        source_end: None,
                        correction_refs: vec![],
                    },
                    covered_ranges: vec![ranges[0].clone()],
                },
            )
            .unwrap();
        let entry_id = receipt.entry_id.unwrap();
        let kind = memory::extract::CandidateKind::Lesson;
        let evidence_id = ranges[0].source.pointer.entry_ref.clone();
        let candidate = memory::extract::StagingRecord::from_candidate(
            "extracted-candidate",
            "extraction-run",
            memory::schema::SourceRef {
                segment_id: segment.to_string(),
                range: [0, 0],
            },
            memory::extract::ExtractedCandidate {
                kind: kind.clone(),
                claim: "Check the constraint before choosing".into(),
                why_useful: "Avoid repeating the rejected approach".into(),
                staleness: None,
                evidence_ids: vec![evidence_id.clone()],
            },
            vec![memory::extract::StagingEvidence {
                id: evidence_id.clone(),
                kind: memory::schema::EvidenceKind::new("message"),
                entry_range: Some([0, 0]),
                origin: Some(serde_json::from_value(json!({"kind":"human_input"})).unwrap()),
                excerpt: Some("Choose B after checking the constraint".into()),
                summary: None,
            }],
            vec![memory::schema::SourceEvidenceRef {
                session_id: Some(attr.session_id),
                segment_id: Some(segment.to_string()),
                entry_range: Some([0, 0]),
                evidence_id: Some(evidence_id),
                origin: Some(serde_json::from_value(json!({"kind":"human_input"})).unwrap()),
                evidence_kind: Some(memory::schema::EvidenceKind::new("message")),
                ..Default::default()
            }],
        );
        let (staged, _) = store
            .stage_candidate_with_attribution(
                crate::subjektiv::SubjectStagingRecord::attach(&subject.id, candidate),
                store
                    .session_attribution(&session.to_string())
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let (memory, _) = store
            .apply_candidate(
                &subject.id,
                &staged.id,
                crate::subjektiv::MemoryChangeTarget::Create,
                crate::subjektiv::MemoryDraft::active(
                    kind,
                    &staged.claim,
                    "Check before choosing.",
                    &staged.why_useful,
                    "Adopt the extracted lesson",
                ),
                "Accepted after comparison",
            )
            .unwrap();
        let selector = MemoryJournalSelector {
            memory_id: memory.id,
            change_id: memory.change_id,
            candidate_id: staged.id,
        };
        Self {
            _dir: dir,
            api,
            identity,
            store,
            host,
            job,
            worker,
            source_worker,
            source_root,
            selector,
            journal_id,
            entry_id,
            ranges,
        }
    }

    fn body_worker(api: &WorkspaceApi, subject: &str) -> RuntimeWorkerRef {
        let memory = api
            .config_store
            .get_workspace_memory_settings(TEST_WORKSPACE_ID)
            .unwrap();
        let key = crate::subjektiv::subject_worker_singleton_key(subject).unwrap();
        let reservation = api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                RUNTIME,
                &format!("body-{subject}"),
                &format!("body-input-{subject}"),
                Some(&key),
                &memory,
            )
            .unwrap();
        let worker = RuntimeWorkerRef::new(RUNTIME, reservation.worker_id.to_string());
        seed_worker_source_member(api, RUNTIME, &worker.worker_id);
        api.config_store
            .complete_worker_create_reservation(TEST_WORKSPACE_ID, reservation.worker_id)
            .unwrap();
        worker
    }

    fn request(&self, worker: Option<&str>, request: &MemoryJournalRequest) -> Request<Body> {
        runtime_source_request(
            &self.identity,
            worker,
            "POST",
            &route_path(),
            serde_json::to_vec(request).unwrap(),
        )
    }

    async fn call_as(
        &self,
        worker: &RuntimeWorkerRef,
        request: MemoryJournalRequest,
    ) -> (StatusCode, Vec<u8>) {
        response(
            build_router(self.api.clone()),
            self.request(Some(&worker.worker_id), &request),
        )
        .await
    }

    async fn call(&self, request: MemoryJournalRequest) -> MemoryJournalResponse {
        let (status, bytes) = self.call_as(&self.worker, request).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap()
    }

    fn fix_initial(&self) {
        self.store
            .execute_journal(
                &self.job,
                JournalOperation {
                    request_id: "fix-initial".into(),
                    expected_state: self
                        .store
                        .read_journal(&self.host, &self.journal_id)
                        .unwrap()
                        .state_token,
                    action: JournalAction::Fix {
                        entry_id: self.entry_id.clone(),
                        source_end: self.ranges[0].source.pointer.clone(),
                    },
                    covered_ranges: vec![self.ranges[0].clone()],
                },
            )
            .unwrap();
    }

    fn add_correction(&self) -> String {
        self.store
            .execute_journal(
                &self.job,
                JournalOperation {
                    request_id: "later-correction".into(),
                    expected_state: self
                        .store
                        .read_journal(&self.host, &self.journal_id)
                        .unwrap()
                        .state_token,
                    action: JournalAction::Create {
                        title: "Correction".into(),
                        body: "The chosen approach still needs validation.".into(),
                        ongoing: String::new(),
                        state: JournalEntryState::Fixed,
                        source_end: Some(self.ranges[1].source.pointer.clone()),
                        correction_refs: vec![self.entry_id.clone()],
                    },
                    covered_ranges: vec![self.ranges[1].clone()],
                },
            )
            .unwrap()
            .entry_id
            .unwrap()
    }

    fn read_request(&self) -> MemoryJournalRequest {
        MemoryJournalRequest::Read {
            selector: self.selector.clone(),
            source_index: 0,
            journal_id: self.journal_id.clone(),
            entry_id: self.entry_id.clone(),
            expected_token: None,
            byte_offset: 0,
            max_bytes: 4096,
        }
    }
}

#[tokio::test]
async fn memory_journal_router_requires_signed_bound_worker_not_user_or_forged_identity() {
    let f = Fixture::new().await;
    let request = MemoryJournalRequest::Links {
        selector: f.selector.clone(),
    };
    let body = serde_json::to_vec(&request).unwrap();
    let app = build_router(f.api.clone());
    let anonymous = Request::builder()
        .method("POST")
        .uri(route_path())
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(
        response(app.clone(), anonymous).await.0,
        StatusCode::UNAUTHORIZED
    );
    let forged = Request::builder()
        .method("POST")
        .uri(route_path())
        .header(CONTENT_TYPE, "application/json")
        .header(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            "forged",
        )
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(
        response(app.clone(), forged).await.0,
        StatusCode::UNAUTHORIZED
    );
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "memory-journal-user").await;
    let user = Request::builder()
        .method("POST")
        .uri(route_path())
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    assert_eq!(response(app.clone(), user).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        response(app, f.request(None, &request)).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn memory_journal_router_default_worker_and_job_have_no_subject_capability() {
    let f = Fixture::new().await;
    let request = MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    };
    let ordinary = RuntimeWorkerRef::new(RUNTIME, Uuid::new_v4().to_string());
    seed_worker_source_member(&f.api, RUNTIME, &ordinary.worker_id);
    let (status, bytes) = f.call_as(&ordinary, request.clone()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    // Even a singleton owner cannot reuse its body authority through a restricted Job binding.
    let job = BackendJobRequest {
        job_id: "restricted-job".into(),
        purpose: "test".into(),
        input_ref: "fixture://job".into(),
        input: json!({}),
        instruction: "No Memory Journal grant".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    let now = Utc::now().to_rfc3339();
    let reserved = f
        .api
        .store
        .reserve_backend_job(TEST_WORKSPACE_ID, &job, &now)
        .unwrap();
    f.api
        .store
        .claim_backend_job_attempt_dispatch(
            TEST_WORKSPACE_ID,
            &job.job_id,
            &reserved.attempt.attempt_id,
            &now,
        )
        .unwrap();
    f.api
        .store
        .bind_backend_job_attempt_worker(
            TEST_WORKSPACE_ID,
            &job.job_id,
            &reserved.attempt.attempt_id,
            &f.worker,
            None,
            &now,
        )
        .unwrap();
    let (status, bytes) = f.call_as(&f.worker, request).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let reader = JournalHostContext::validated_reader(
        &f.store,
        &f.store
            .read_journal(&f.host, &f.journal_id)
            .unwrap()
            .subject_id,
    )
    .unwrap();
    let MemoryJournalResponse::Links(links) = f
        .store
        .execute_memory_journal(
            &reader,
            MemoryJournalRequest::Links {
                selector: f.selector.clone(),
            },
        )
        .unwrap()
    else {
        panic!("links required")
    };
    assert_eq!(
        links.sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::NotReconciled
        }
    );
}

#[tokio::test]
async fn memory_journal_router_reconciles_only_fixed_events_and_retries_without_duplication() {
    let f = Fixture::new().await;
    let reconcile = MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    };
    let MemoryJournalResponse::Links(open) = f.call(reconcile.clone()).await else {
        panic!("links required")
    };
    assert_eq!(
        open.sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::JournalNotFixed
        }
    );
    assert_eq!(
        f.store
            .list_journal_entries(&f.host, &f.journal_id, None, 32)
            .unwrap()
            .items[0]
            .state,
        JournalEntryState::Open
    );
    f.fix_initial();
    // Reading stored state alone must not silently trigger reconciliation.
    assert_eq!(
        f.call(MemoryJournalRequest::Links {
            selector: f.selector.clone()
        })
        .await,
        MemoryJournalResponse::Links(open)
    );
    let linked = f.call(reconcile.clone()).await;
    assert_eq!(f.call(reconcile).await, linked);
    assert_eq!(
        f.call(MemoryJournalRequest::Links {
            selector: f.selector.clone()
        })
        .await,
        linked
    );
    let MemoryJournalResponse::Links(links) = linked else {
        panic!("links required")
    };
    assert_eq!(links.sources.len(), 1);
    assert_eq!(
        links.sources[0].connection,
        SourceConnection::Linked {
            journal_id: f.journal_id.clone(),
            entry_id: f.entry_id.clone(),
            target: JournalTargetAvailability::Available,
            reconsideration: false,
        }
    );
    assert_eq!(
        f.store
            .list_journal_entries(&f.host, &f.journal_id, None, 32)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn memory_journal_router_reads_saved_text_after_original_session_and_source_worker_removal() {
    let f = Fixture::new().await;
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    assert!(f.source_root.exists());
    fs::remove_dir_all(&f.source_root).unwrap();
    f.api
        .store
        .delete_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap();
    assert!(!f.source_root.exists());
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
            .unwrap()
            .is_none()
    );
    // No Runtime adapter exists for RUNTIME, so a restore/history dependency would fail.
    let MemoryJournalResponse::Read(read) = f.call(f.read_request()).await else {
        panic!("read required")
    };
    assert_eq!(
        read.source.original_source,
        OriginalSourceAvailability::Missing
    );
    assert_eq!(read.page.unwrap().text, JOURNAL_BODY);
    let MemoryJournalResponse::Related(related) = f
        .call(MemoryJournalRequest::Related {
            journal_id: f.journal_id.clone(),
            entry_id: f.entry_id.clone(),
            after: None,
            limit: 32,
        })
        .await
    else {
        panic!("related required")
    };
    assert_eq!(related.items.len(), 1);
    assert_eq!(related.items[0].selector, f.selector);
    assert!(!related.items[0].reconsideration);
    assert!(related.next.is_none());
}

#[tokio::test]
async fn memory_journal_router_ids_do_not_grant_cross_subject_reads_or_reconciliation() {
    let f = Fixture::new().await;
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let other = f
        .store
        .create_subject(crate::subjektiv::SubjectRole::new("other").unwrap())
        .unwrap();
    let other_worker = Fixture::body_worker(&f.api, &other.id);
    for granted in [false, true] {
        if granted {
            Fixture::grant_permissions(&f.store, &other.id);
        }
        for request in [
            MemoryJournalRequest::Reconcile {
                selector: f.selector.clone(),
            },
            MemoryJournalRequest::Links {
                selector: f.selector.clone(),
            },
            f.read_request(),
            MemoryJournalRequest::Related {
                journal_id: f.journal_id.clone(),
                entry_id: f.entry_id.clone(),
                after: None,
                limit: 32,
            },
        ] {
            let (status, bytes) = f.call_as(&other_worker, request).await;
            assert_eq!(
                status,
                if granted {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::FORBIDDEN
                },
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            assert!(!String::from_utf8_lossy(&bytes).contains(JOURNAL_BODY));
        }
    }
    // A model cannot override the authenticated Subject with an additional JSON field.
    let mut body = serde_json::to_value(f.read_request()).unwrap();
    body["subject_id"] = json!(other.id);
    let request = runtime_source_request(
        &f.identity,
        Some(&f.worker.worker_id),
        "POST",
        &route_path(),
        serde_json::to_vec(&body).unwrap(),
    );
    assert_eq!(
        response(build_router(f.api.clone()), request).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn memory_journal_router_correction_reverse_lookup_preserves_memory_and_original_link() {
    let f = Fixture::new().await;
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let subject = f
        .store
        .read_journal(&f.host, &f.journal_id)
        .unwrap()
        .subject_id;
    let before =
        serde_json::to_value(f.store.memory(&subject, &f.selector.memory_id).unwrap()).unwrap();
    let correction = f.add_correction();
    let MemoryJournalResponse::Related(related) = f
        .call(MemoryJournalRequest::Related {
            journal_id: f.journal_id.clone(),
            entry_id: correction,
            after: None,
            limit: 32,
        })
        .await
    else {
        panic!("related required")
    };
    assert_eq!(related.items.len(), 1);
    assert_eq!(related.items[0].selector, f.selector);
    assert!(related.items[0].reconsideration);
    let MemoryJournalResponse::Read(read) = f.call(f.read_request()).await else {
        panic!("read required")
    };
    assert_eq!(read.page.unwrap().text, JOURNAL_BODY);
    assert!(matches!(read.source.connection, SourceConnection::Linked {
        entry_id, reconsideration:true, ..
    } if entry_id == f.entry_id));
    assert_eq!(
        serde_json::to_value(f.store.memory(&subject, &f.selector.memory_id).unwrap()).unwrap(),
        before
    );
}

#[path = "subjektiv_journal_browser_tests.rs"]
mod browser_tests;

#[tokio::test]
async fn memory_journal_unset_or_revoked_permissions_deny_every_route_without_link_writes() {
    for revoked in [false, true] {
        let f = Fixture::with_permissions(revoked).await;
        let lease = f
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.worker)
            .unwrap()
            .unwrap();
        let (subject, _) = subjektiv_scope_from_singleton_key(&lease.key).unwrap();
        if revoked {
            let policy = f.store.subject_permissions(&subject).unwrap().unwrap();
            f.store
                .set_subject_permissions(
                    &subject,
                    &server_api::authorization::SubjectPermissionsUpdate {
                        operation_id: "revoke-memory-journal".into(),
                        expected_change_id: Some(policy.change_id),
                        execution: vec![],
                        delegation: vec![],
                    },
                )
                .unwrap();
        }
        let count_links = || {
            f.api
                .feature_storage
                .open(&f.api.subjektiv_registration)
                .unwrap()
                .with_connection(|conn| {
                    Ok(
                        conn.query_row("SELECT COUNT(*) FROM memory_journal_links", [], |row| {
                            row.get::<_, i64>(0)
                        })?,
                    )
                })
                .unwrap()
        };
        let before = count_links();
        let requests = [
            MemoryJournalRequest::Reconcile {
                selector: f.selector.clone(),
            },
            MemoryJournalRequest::Links {
                selector: f.selector.clone(),
            },
            MemoryJournalRequest::ReconcilePage {
                selector: f.selector.clone(),
                source_offset: 0,
                limit: 1,
            },
            MemoryJournalRequest::LinksPage {
                selector: f.selector.clone(),
                source_offset: 0,
                limit: 1,
            },
            f.read_request(),
            MemoryJournalRequest::Related {
                journal_id: f.journal_id.clone(),
                entry_id: f.entry_id.clone(),
                after: None,
                limit: 1,
            },
        ];
        for request in requests {
            let (status, body) = f.call_as(&f.worker, request).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{}",
                String::from_utf8_lossy(&body)
            );
            assert!(!String::from_utf8_lossy(&body).contains(JOURNAL_BODY));
            assert_eq!(count_links(), before);
        }
    }
}
