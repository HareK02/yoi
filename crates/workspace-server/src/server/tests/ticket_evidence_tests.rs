// Included as a child of server::tests so the existing router/fixture helpers
// remain private. These are API regressions, not repository/provider tests:
// every authority record lives in a temporary database, with no Git checkout.
use super::*;

use merge_request::{
    CompleteMergeRequest, ConflictResolution, MergeRequestAuth, MergeRequestReviewSubject,
    MergeRequestStore, MergeStrategy, OpenMergeRequest, RegisterReviewerChildSession,
    RequestMergeRequestReview, ReviewDecision, ReviewEvent, SubmitMergeRequestReview,
};
use rusqlite::{Connection, params};
use server_api::{
    MergeRequestDetailResponse, MergeRequestListResponse, TicketDetail, TicketQueryResponse,
    TicketSourceRefObservation,
};

const EVIDENCE_MR: &str = "ticket-evidence-mr";
const EVIDENCE_SOURCE: &str = "immutable-approved-source";
const EVIDENCE_TARGET: &str = "immutable-target-result";

#[tokio::test]
async fn integration_preserves_legacy_empty_post_ref_and_exposes_new_ref_with_server_schema() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("integration-post-ref");
    let connection = Connection::open(&fixture.api.config.database_path).unwrap();
    let legacy_index: i64 = connection.query_row(
        "SELECT COALESCE(MAX(event_index),-1)+1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",
        params![TEST_WORKSPACE_ID, fixture.ticket_id], |row| row.get(0),
    ).unwrap();
    connection.execute(
        "INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body)
         VALUES(?1,?2,?3,'','comment','Legacy integration')",
        params![TEST_WORKSPACE_ID, fixture.ticket_id, legacy_index],
    ).unwrap();

    let merged = fixture.integrate(&approval);
    assert_eq!(fixture.integrate(&approval), merged);
    let detail = fixture
        .backend
        .show(fixture.ticket_id.clone().into())
        .unwrap();
    let legacy = detail
        .events
        .iter()
        .find(|event| event.body.as_str() == "Legacy integration")
        .unwrap();
    assert_eq!(legacy.event_ref, "");
    let integrations = detail
        .events
        .iter()
        .filter(|event| {
            event.attributes.get("merge_request_id").map(String::as_str) == Some(EVIDENCE_MR)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        integrations.len(),
        1,
        "completion replay must not duplicate the Ticket post"
    );
    assert_eq!(
        integrations[0].event_ref,
        format!("{}:{}", fixture.ticket_id, legacy_index + 1)
    );
    assert_eq!(
        integrations[0].attributes["operation_id"],
        "evidence-integration"
    );
    let shown = fixture.show().await;
    assert_eq!(shown.merge_requests[0].state, "merged");
}

#[tokio::test]
async fn ticket_browse_does_not_observe_sources_but_authoritative_query_and_show_still_do() {
    struct CountingObserver(Arc<std::sync::atomic::AtomicUsize>);
    impl crate::authority::TicketSourceCommitResolver for CountingObserver {
        fn resolve_source_commit(
            &self,
            _ticket: &str,
            _repository: &str,
            _selector: &str,
        ) -> std::result::Result<String, MergeRequestRefDiagnostic> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(EVIDENCE_SOURCE.into())
        }
    }
    let mut fixture = EvidenceApiFixture::new().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    fixture.api.authority = fixture
        .api
        .authority
        .clone()
        .with_source_commit_resolver(Arc::new(CountingObserver(calls.clone())));
    let response = request_json(
        build_inner_router(fixture.api.clone()),
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/browse"),
        Some(json!({"limit":30})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["items"][0]["id"], fixture.ticket_id);
    assert!(response["items"][0].get("evidence").is_none());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    fixture
        .api
        .authority
        .query_tickets(Default::default())
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    fixture.api.authority.ticket(&fixture.ticket_id).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// Deliberately freeze review dates: freshness must follow the exact item
// content digest and source/result snapshot, not a comparison of wall-clock dates.
fn evidence_time() -> chrono::DateTime<Utc> {
    "2000-01-01T00:00:00Z".parse().unwrap()
}

struct EvidenceApiFixture {
    _temp: tempfile::TempDir,
    api: WorkspaceApi,
    app: Router,
    backend: SqliteTicketBackend,
    store: MergeRequestStore,
    ticket_id: String,
    auth: MergeRequestAuth,
}

impl EvidenceApiFixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let api = test_api_with_remote_repository(temp.path()).await;
        assert!(
            api.config
                .repositories
                .iter()
                .all(|repository| repository.path.is_none())
        );
        assert!(!temp.path().join(".git").exists());
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("T716 remote immutable evidence");
        input.targets = vec![ticket::TicketTarget {
            repository_key: "test-repository".into(),
            ref_selector: Some("develop".into()),
            access: TicketTargetAccess::ReadWrite,
        }];
        let ticket_id = backend.create(input).unwrap().id;
        // Registry/assignment authority only; there is no live Worker or Runtime
        // capable of supplying an observation for this fixture.
        let worker = RuntimeWorkerRef::new("evidence-offline-runtime", "evidence-coder");
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: worker.clone(),
                display_name: "Offline evidence Coder".into(),
                profile: Some("builtin:coder".into()),
                retention_state: "normal".into(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: evidence_time().to_rfc3339(),
                updated_at: evidence_time().to_rfc3339(),
            })
            .unwrap();
        let assignment_id = "evidence-assignment".to_string();
        api.store
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    ticket_id: ticket_id.clone(),
                    assignment_id: assignment_id.clone(),
                    worker: worker.clone(),
                    assigned_by: "fixture".into(),
                    assigned_at: evidence_time().to_rfc3339(),
                },
                None,
                "evidence-assigned",
                "evidence-assignment-operation",
                false,
            )
            .unwrap();
        let auth = MergeRequestAuth {
            workspace_id: TEST_WORKSPACE_ID.into(),
            repository_id: test_repository_id(&api),
            runtime_id: worker.runtime_id,
            worker_id: worker.worker_id,
            assignment_id,
        };
        let store = merge_request_store(&api, TEST_WORKSPACE_ID).unwrap();
        store
            .open_merge_request(OpenMergeRequest {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: ticket_id.clone(),
                repository_id: auth.repository_id.clone(),
                selector_from: "work/t716".into(),
                selector_to: "develop".into(),
                summary: String::new(),
                auth: auth.clone(),
                now: evidence_time(),
            })
            .unwrap();
        let app = build_inner_router(api.clone());
        Self {
            _temp: temp,
            api,
            app,
            backend,
            store,
            ticket_id,
            auth,
        }
    }

    fn content_digest(&self) -> String {
        ticket_item_checker::content_digest(
            &self.backend.show(self.ticket_id.clone().into()).unwrap(),
        )
    }

    fn subjects(&self) -> Vec<MergeRequestReviewSubject> {
        vec![MergeRequestReviewSubject {
            merge_request_id: EVIDENCE_MR.into(),
            subject_ref: EVIDENCE_SOURCE.into(),
        }]
    }

    fn approve(&self, token: &str) -> ReviewEvent {
        let child_session_id = format!("evidence-reviewer-{token}");
        self.store
            .register_reviewer_child_session(RegisterReviewerChildSession {
                workspace_id: TEST_WORKSPACE_ID.into(),
                parent_runtime_id: self.auth.runtime_id.clone(),
                parent_worker_id: self.auth.worker_id.clone(),
                child_session_id: child_session_id.clone(),
                reviewer_profile: "builtin:reviewer".into(),
                now: evidence_time(),
            })
            .unwrap();
        self.store
            .request_review(RequestMergeRequestReview {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: self.ticket_id.clone(),
                ticket_content_digest: self.content_digest(),
                ticket_merge_request_subjects: self.subjects(),
                subject_ref: EVIDENCE_SOURCE.into(),
                child_session_id,
                capability_token: token.into(),
                auth: self.auth.clone(),
                now: evidence_time(),
            })
            .unwrap();
        self.store
            .submit_review(SubmitMergeRequestReview {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: self.ticket_id.clone(),
                current_subject_ref: EVIDENCE_SOURCE.into(),
                capability_token: token.into(),
                decision: ReviewDecision::Approve,
                body: format!("Requirement approval {token}"),
                findings: vec![],
                now: evidence_time(),
            })
            .unwrap()
    }

    fn integrate(&self, approval: &ReviewEvent) -> merge_request::MergeEvent {
        self.store
            .complete(CompleteMergeRequest {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: self.ticket_id.clone(),
                operation_id: "evidence-integration".into(),
                approval_event_id: approval.event_id.clone(),
                current_subject_ref: EVIDENCE_SOURCE.into(),
                target_ref_before: "target-before".into(),
                target_ref_after: EVIDENCE_TARGET.into(),
                strategy: MergeStrategy::FastForward,
                resolution: ConflictResolution::None,
                auth: self.auth.clone(),
                now: evidence_time(),
            })
            .unwrap()
    }

    fn closure(&self) -> ticket::TicketCaseUpdate {
        ticket::TicketCaseUpdate {
            operation_key: "evidence-ticket-closure".into(),
            expected_content_digest: self.content_digest(),
            expected_state: ticket::TicketState::Open,
            expected_closure_operation_key: None,
            action: ticket::TicketCaseAction::Close {
                reason: ticket::TicketClosureReason::Completed,
            },
            explanation: "Implementation judged complete independently of MR attestation".into(),
            references: Vec::new(),
            author: Some("fixture".into()),
        }
    }

    async fn show(&self) -> TicketDetail {
        serde_json::from_value(
            get_json(
                self.app.clone(),
                &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", self.ticket_id),
            )
            .await,
        )
        .unwrap()
    }

    async fn query(&self, filter: Value) -> TicketQueryResponse {
        serde_json::from_value(
            request_json(
                self.app.clone(),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query"),
                Some(filter),
                StatusCode::OK,
            )
            .await,
        )
        .unwrap()
    }

    async fn assert_filters(&self, approved: bool, missing: bool, stale: bool) {
        for (filter, matches) in [
            (json!({"evidence": ["approved_review"]}), approved),
            (json!({"attention": ["missing_evidence"]}), missing),
            (json!({"attention": ["stale_after_rescope"]}), stale),
        ] {
            let result = self.query(filter.clone()).await;
            let ids = result
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            let expected = if matches {
                vec![self.ticket_id.as_str()]
            } else {
                vec![]
            };
            assert_eq!(ids, expected, "{filter}");
            if matches {
                let shown = self.show().await;
                assert_eq!(result.items[0].content_digest, shown.content_digest);
                assert_eq!(result.items[0].evidence, shown.evidence);
                assert_eq!(result.items[0].merge_requests, shown.merge_requests);
            }
        }
    }

    async fn list(&self) -> MergeRequestListResponse {
        serde_json::from_value(
            get_json(
                self.app.clone(),
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/merge-requests?ticket_ref={}",
                    self.ticket_id,
                ),
            )
            .await,
        )
        .unwrap()
    }

    async fn detail(&self, after: u64, limit: usize) -> MergeRequestDetailResponse {
        serde_json::from_value(
            get_json(
                self.app.clone(),
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/merge-requests/{EVIDENCE_MR}?after={after}&limit={limit}",
                ),
            )
            .await,
        )
        .unwrap()
    }

    fn rescope(&self) {
        self.backend
            .edit_item(
                self.ticket_id.clone().into(),
                TicketItemEdit {
                    body: Some(MarkdownText::new(
                        "Revised requirements require a fresh exact-content approval.",
                    )),
                    ..Default::default()
                },
            )
            .unwrap();
    }
}

#[tokio::test]
async fn closed_merged_remote_ticket_uses_immutable_result_in_show_query_list_and_paged_detail() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("integration");
    let merge = fixture.integrate(&approval);
    let content_digest = fixture.content_digest();
    let completion = fixture
        .backend
        .update_case(&fixture.ticket_id, fixture.closure())
        .unwrap();
    assert_eq!(ticket::ticket_content_digest(&completion), content_digest);
    assert!(
        fixture
            .api
            .store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_some()
    );

    let shown = fixture.show().await;
    assert_eq!(shown.state.as_str(), "closed");
    assert_eq!(shown.content_digest, content_digest);
    assert!(
        shown.evidence.complete_for_integration,
        "{:?}",
        shown.evidence.missing
    );
    assert!(shown.evidence.review_after_rescope);
    assert!(shown.evidence.missing.is_empty());
    assert_eq!(shown.merge_requests.len(), 1);
    let summary = &shown.merge_requests[0];
    assert_eq!(
        summary.current_subject_ref.as_deref(),
        Some(EVIDENCE_SOURCE)
    );
    assert_eq!(summary.review_subject_ref.as_deref(), Some(EVIDENCE_SOURCE));
    assert_eq!(summary.review_status, "approved");
    assert_eq!(
        summary.source_ref_observation,
        TicketSourceRefObservation::NotRequired {}
    );
    assert_eq!(summary.integration_evidence_error, None);
    fixture.assert_filters(true, false, false).await;

    let listed = fixture.list().await;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(&listed.items[0].summary, summary);
    assert!(listed.items[0].ref_diagnostics.is_empty());
    assert_eq!(listed.items[0].thread_event_count, merge.sequence as usize);
    // Neither page includes the MergeEvent. Ref evidence must be computed from
    // the full stored thread before applying response-thread pagination.
    let first = fixture.detail(0, 1).await;
    assert_eq!(first.merge_request.thread.len(), 1);
    assert!(matches!(
        first.merge_request.thread[0],
        server_api::MergeRequestThreadEvent::ReviewRequested(_)
    ));
    let empty = fixture.detail(merge.sequence, 1).await;
    assert!(empty.merge_request.thread.is_empty());
    for detail in [first, empty] {
        assert_eq!(detail.source.status, "known");
        assert_eq!(detail.source.resolved_ref.as_deref(), Some(EVIDENCE_SOURCE));
        assert_eq!(detail.target.status, "known");
        assert_eq!(detail.target.resolved_ref.as_deref(), Some(EVIDENCE_TARGET));
        assert_eq!(detail.source.observed_at, merge.created_at.to_rfc3339());
        assert_eq!(detail.source.diagnostic, None);
        assert_eq!(detail.target.diagnostic, None);
    }
}

#[tokio::test]
async fn latest_postmerge_approval_requires_exact_content_and_stored_source_result_snapshot() {
    let fixture = EvidenceApiFixture::new().await;
    let integration = fixture.approve("integration");
    let merge = fixture.integrate(&integration);
    fixture.rescope();
    let content_digest = fixture.content_digest();
    assert_ne!(integration.ticket_content_digest, content_digest);

    let stale = fixture.show().await;
    assert!(stale.evidence.approved_current_subject); // integration proof survives
    assert!(!stale.evidence.review_after_rescope);
    assert!(!stale.evidence.complete_for_integration);
    assert!(
        stale
            .evidence
            .missing
            .contains(&"review_after_rescope".into())
    );
    fixture.assert_filters(false, true, true).await;
    let mut outdated_closure = fixture.closure();
    outdated_closure.expected_content_digest = integration.ticket_content_digest.clone();
    assert!(
        fixture
            .backend
            .update_case(&fixture.ticket_id, outdated_closure)
            .is_err()
    );
    assert_eq!(fixture.show().await.state.as_str(), "open");

    let latest = fixture.approve("latest-content");
    assert_eq!(latest.created_at, integration.created_at);
    assert_eq!(latest.ticket_content_digest, content_digest);
    assert_eq!(latest.ticket_merge_request_subjects, fixture.subjects());
    let fresh = fixture.show().await;
    assert!(
        fresh.evidence.complete_for_integration,
        "{:?}",
        fresh.evidence.missing
    );
    assert_eq!(
        fresh.merge_requests[0].review_excerpt.as_deref(),
        Some("Requirement approval integration")
    );
    fixture.assert_filters(true, false, false).await;
    // A current Ticket content digest alone is insufficient: simulate a stored attestation
    // against a different result set using typed serialized payloads in this
    // temporary fixture database, never a live Workspace database.
    let mut wrong_snapshot = latest.clone();
    wrong_snapshot
        .ticket_merge_request_subjects
        .push(MergeRequestReviewSubject {
            merge_request_id: "other-result".into(),
            subject_ref: "other-approved-source".into(),
        });
    let mut requested = fixture
        .store
        .get_by_id(TEST_WORKSPACE_ID, EVIDENCE_MR)
        .unwrap()
        .thread
        .into_iter()
        .find_map(|event| match event {
            merge_request::MergeRequestThreadEvent::ReviewRequested(event)
                if event.event_id == latest.request_event_id =>
            {
                Some(event)
            }
            _ => None,
        })
        .unwrap();
    requested.ticket_merge_request_subjects = wrong_snapshot.ticket_merge_request_subjects.clone();
    let connection = Connection::open(&fixture.api.config.database_path).unwrap();
    for (event_id, payload) in [
        (
            &requested.event_id,
            serde_json::to_string(&requested).unwrap(),
        ),
        (
            &wrong_snapshot.event_id,
            serde_json::to_string(&wrong_snapshot).unwrap(),
        ),
    ] {
        assert_eq!(connection.execute(
            "UPDATE merge_request_thread_events SET payload_json=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND event_id=?3",
            params![TEST_WORKSPACE_ID, EVIDENCE_MR, event_id, payload],
        ).unwrap(), 1);
    }
    drop(connection);
    let wrong = fixture.show().await;
    assert!(wrong.evidence.approved_current_subject);
    assert!(!wrong.evidence.complete_for_integration);
    fixture.assert_filters(false, true, true).await;
    assert_eq!(fixture.show().await.state.as_str(), "open");

    let repaired = fixture.approve("exact-snapshot");
    fixture.assert_filters(true, false, false).await;
    // The newest requirement approval must not replace the historical approval
    // and source which actually authorized integration.
    let stored = fixture
        .store
        .get_by_id(TEST_WORKSPACE_ID, EVIDENCE_MR)
        .unwrap();
    assert_eq!(
        stored.integration_approval().unwrap().event_id,
        integration.event_id
    );
    assert_eq!(stored.merged_result().unwrap(), &merge);
    let detail = fixture.detail(repaired.sequence - 1, 1).await;
    assert_eq!(detail.merge_request.thread.len(), 1);
    assert_eq!(detail.source.resolved_ref.as_deref(), Some(EVIDENCE_SOURCE));
    assert_eq!(detail.target.resolved_ref.as_deref(), Some(EVIDENCE_TARGET));
    let completion = fixture
        .backend
        .update_case(&fixture.ticket_id, fixture.closure())
        .unwrap();
    assert_eq!(ticket::ticket_content_digest(&completion), content_digest);
    assert_eq!(fixture.show().await.state.as_str(), "closed");
    fixture.assert_filters(true, false, false).await;
}

#[tokio::test]
async fn released_assignment_still_observes_provider_and_reports_real_unavailability() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("open-review");
    fixture.rescope();
    // Work release must not short-circuit MR provider observation. This fixture
    // has no live Runtime, so retain the same real provider failure after release.
    let baseline = fixture.detail(approval.sequence, 1).await;
    let source_code = baseline.source.diagnostic.unwrap().code;
    let target_code = baseline.target.diagnostic.unwrap().code;
    assert_ne!(source_code, "source_ref_runtime_unavailable");
    assert_ne!(target_code, "target_ref_runtime_unavailable");
    let connection = Connection::open(&fixture.api.config.database_path).unwrap();
    assert_eq!(connection.execute(
        "INSERT INTO ticket_assignment_work_releases(workspace_id,assignment_id,released_at) VALUES(?1,?2,?3)",
        params![TEST_WORKSPACE_ID, fixture.auth.assignment_id, evidence_time().to_rfc3339()],
    ).unwrap(), 1);
    drop(connection);
    assert!(
        fixture
            .api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .api
            .store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_none()
    );

    let shown = fixture.show().await;
    let summary = &shown.merge_requests[0];
    assert_eq!(summary.state, "open");
    assert_eq!(summary.current_subject_ref, None);
    assert_eq!(
        summary.source_ref_observation,
        TicketSourceRefObservation::Unavailable {
            code: source_code.clone(),
        }
    );
    assert_eq!(shown.evidence.review_status.as_deref(), Some("unknown"));
    assert!(!shown.evidence.complete_for_integration);
    assert!(
        shown
            .evidence
            .missing
            .contains(&"source_ref_unavailable".into())
    );
    assert!(
        !shown
            .evidence
            .missing
            .contains(&"review_after_rescope".into())
    );
    fixture.assert_filters(false, true, false).await;
    let listed = fixture.list().await;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(&listed.items[0].summary, summary);
    assert_eq!(listed.items[0].ref_diagnostics[0].code, source_code);
    let detail = fixture.detail(approval.sequence, 1).await;
    assert!(detail.merge_request.thread.is_empty());
    assert_eq!(detail.source.status, "unknown");
    assert_eq!(detail.source.resolved_ref, None);
    assert_eq!(detail.source.diagnostic.unwrap().code, source_code);
    assert_eq!(detail.target.status, "unknown");
    assert_eq!(detail.target.diagnostic.unwrap().code, target_code);
}

#[tokio::test]
async fn evidence_query_continues_bounded_scans_past_nonmatching_tickets() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("integration");
    fixture.integrate(&approval);
    for title in [
        "A nonmatching one",
        "B nonmatching two",
        "C nonmatching three",
    ] {
        fixture
            .backend
            .create(ticket::NewTicket::new(title))
            .unwrap();
    }
    for filter in [
        json!({"evidence": ["approved_review"]}),
        json!({"review_status": "approved"}),
    ] {
        let mut request = filter.clone();
        request["sort"] = json!("title");
        request["limit"] = json!(1);
        let first = fixture.query(request.clone()).await;
        assert!(first.items.is_empty());
        assert!(first.page.has_more);
        assert!(first.page.source_truncated);
        request["cursor"] = json!(first.page.next_cursor.unwrap());
        let second = fixture.query(request).await;
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].id, fixture.ticket_id);
        assert!(second.items[0].evidence.review_after_rescope);
        assert!(!second.page.has_more);
    }
}

#[test]
fn source_observation_diagnostics_preserve_provider_failure_codes() {
    for (provider_code, expected) in [
        ("repository_ref_not_found", "source_ref_not_found"),
        (
            "repository_ref_provider_timeout",
            "source_ref_provider_timeout",
        ),
        (
            "repository_ref_provider_unavailable",
            "source_ref_provider_unavailable",
        ),
        (
            "repository_access_credential_expired",
            "source_ref_credential_expired",
        ),
    ] {
        let error = Error::RuntimeOperationFailed {
            runtime_id: "fixture-runtime".into(),
            code: provider_code.into(),
            message: "Provider observation failed".into(),
        };
        assert_eq!(
            merge_ref_diagnostic(remap_source_ref_error(error.into())).code,
            expected
        );
    }
}

#[tokio::test]
async fn closing_ticket_with_open_unreviewed_mr_does_not_change_its_proof_or_state() {
    let fixture = EvidenceApiFixture::new().await;
    let before = fixture
        .store
        .get_by_id(TEST_WORKSPACE_ID, EVIDENCE_MR)
        .unwrap();
    fixture
        .backend
        .update_case(
            &fixture.ticket_id,
            ticket::TicketCaseUpdate {
                operation_key: "close-with-open-mr".into(),
                expected_content_digest: fixture.content_digest(),
                expected_state: ticket::TicketState::Open,
                expected_closure_operation_key: None,
                action: ticket::TicketCaseAction::Close {
                    reason: ticket::TicketClosureReason::NotPlanned,
                },
                explanation: "The request was withdrawn; no integration or approval is asserted"
                    .into(),
                references: Vec::new(),
                author: Some("workspace-user".into()),
            },
        )
        .unwrap();
    let after = fixture
        .store
        .get_by_id(TEST_WORKSPACE_ID, EVIDENCE_MR)
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    let shown = fixture.show().await;
    assert_eq!(shown.state.as_str(), "closed");
    assert!(!shown.evidence.approved_current_subject);
    assert_eq!(shown.merge_requests[0].state, "open");
}

#[tokio::test]
async fn mrless_completed_closure_is_a_ticket_judgment_not_approved_or_missing_merge_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Investigation with no repository"))
        .unwrap();
    let ticket = backend.show(reference.id.clone().into()).unwrap();
    backend
        .update_case(
            &reference.id,
            ticket::TicketCaseUpdate {
                operation_key: "research-result".into(),
                expected_content_digest: ticket::ticket_content_digest(&ticket),
                expected_state: ticket::TicketState::Open,
                expected_closure_operation_key: None,
                action: ticket::TicketCaseAction::Close {
                    reason: ticket::TicketClosureReason::Completed,
                },
                explanation: "Answer recorded in the Ticket thread".into(),
                references: Vec::new(),
                author: Some("workspace-user".into()),
            },
        )
        .unwrap();
    let app = build_inner_router(api);
    let shown: TicketDetail = serde_json::from_value(
        get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", reference.id),
        )
        .await,
    )
    .unwrap();
    assert_eq!(shown.state.as_str(), "closed");
    assert!(!shown.evidence.has_merge_request);
    assert!(!shown.evidence.approved_current_subject);
    assert!(!shown.evidence.complete_for_integration);
    assert!(shown.evidence.missing.is_empty());
    assert!(shown.evidence.review_status.is_none());
    for filter in [
        json!({"attention":["missing_evidence"]}),
        json!({"evidence":["approved_review"]}),
    ] {
        let query: TicketQueryResponse = serde_json::from_value(
            request_json(
                app.clone(),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query"),
                Some(filter),
                StatusCode::OK,
            )
            .await,
        )
        .unwrap();
        assert!(query.items.is_empty());
    }
}

// Exercise the generated handler's JSON/CAS boundary, then reread the real
// Ticket authority. Supply the actor normally established by middleware; signed
// case-route tests cover real authentication. No live Runtime/provider runs.
fn case_actor_router(api: WorkspaceApi) -> Router {
    build_inner_router(api).layer(Extension(test_browser_request_actor()))
}

fn case_request(before: &ticket::Ticket, operation_key: &str, action: Value) -> Value {
    json!({
        "operation_key": operation_key,
        "expected_content_digest": ticket::ticket_content_digest(before),
        "expected_state": before.meta.workflow_state,
        "expected_closure_operation_key": before.closure_operation_key,
        "action": action,
        "explanation": "Explicit user case judgment",
        "references": [{"kind": "ticket", "target": "T-42"}],
    })
}

#[tokio::test]
async fn case_route_rejects_missing_cas_client_author_and_removed_workflow_routes() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Transport decisions"))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let app = case_actor_router(api);
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case",
        reference.id
    );
    let request = case_request(
        &before,
        "close",
        json!({"action": "close", "reason": "completed"}),
    );
    for field in [
        "operation_key",
        "expected_content_digest",
        "expected_state",
        "action",
        "explanation",
    ] {
        let mut missing = request.clone();
        missing.as_object_mut().unwrap().remove(field);
        request_json(
            app.clone(),
            "POST",
            &path,
            Some(missing),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    for (field, value) in [
        ("author", json!("spoofed-client-author")),
        ("expected_state", json!("done")),
        ("action", json!({"action": "close", "reason": "invented"})),
    ] {
        let mut invalid = request.clone();
        invalid[field] = value;
        request_json(
            app.clone(),
            "POST",
            &path,
            Some(invalid),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let legacy = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", reference.id);
    for suffix in [
        "workflow-state",
        "state-changes",
        "state-fields/state",
        "workflow/close",
        "complete",
    ] {
        // A surviving GET catch-all can produce 405 rather than 404 for an
        // absent POST route. Neither status authorizes a legacy mutation.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{legacy}/{suffix}"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "legacy POST {suffix}: {}",
            response.status()
        );
    }
    request_json(
        app,
        "POST",
        &format!(
            "/workspaces/019d0000-0000-7000-8000-0000000000bb/tickets/{}/case",
            reference.id
        ),
        Some(request),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), before);
}

#[tokio::test]
async fn case_route_preserves_corrected_history_replay_and_reopen_aba_guard_without_mr() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new(
            "Case judgments without repository work",
        ))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let digest = ticket::ticket_content_digest(&before);
    let app = case_actor_router(api);
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case",
        reference.id
    );
    let detail_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", reference.id);
    let close = case_request(
        &before,
        "guarded-close",
        json!({"action": "close", "reason": "completed"}),
    );
    let response = request_json(
        app.clone(),
        "POST",
        &path,
        Some(close.clone()),
        StatusCode::OK,
    )
    .await;
    let summary: server_api::TicketSummary = serde_json::from_value(response).unwrap();
    assert_eq!(summary.0.workflow_state, ticket::TicketState::Closed);
    let closed = backend.show(reference.id.clone().into()).unwrap();
    let actor = test_browser_request_actor();
    let expected_author = format!(
        "workspace-user:{}",
        serde_json::to_string(&(&actor.account_id, &actor.user_id)).unwrap()
    );
    assert_eq!(
        closed.current_closure.as_ref().unwrap().author,
        expected_author
    );
    let event = closed.events.last().unwrap();
    assert_eq!(event.attributes["source_account_id"], actor.account_id);
    assert_eq!(event.attributes["source_user_id"], actor.user_id);
    assert_eq!(
        closed.current_closure.as_ref().unwrap().reason,
        ticket::TicketClosureReason::Completed
    );
    assert_eq!(closed.case_history.len(), 1);
    assert_eq!(
        closed.case_history[0].references,
        closed.current_closure.as_ref().unwrap().references
    );

    let correction = case_request(
        &closed,
        "correct-closure",
        json!({"action": "correct_closure", "reason": {"other": "Answer superseded by user decision"}}),
    );
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(correction.clone()),
        StatusCode::OK,
    )
    .await;
    let corrected = backend.show(reference.id.clone().into()).unwrap();
    assert_eq!(corrected.case_history.len(), 2);
    assert_eq!(corrected.case_history[0], closed.case_history[0]);
    assert_eq!(
        corrected.current_closure.as_ref().unwrap().reason,
        ticket::TicketClosureReason::Other("Answer superseded by user decision".into())
    );
    assert_eq!(
        corrected.closure_operation_key.as_deref(),
        Some("correct-closure")
    );
    let detail: TicketDetail =
        serde_json::from_value(get_json(app.clone(), &detail_path).await).unwrap();
    assert_eq!(detail.current_closure, corrected.current_closure);
    assert_eq!(detail.closure_history, corrected.case_history);
    assert_eq!(
        detail.closure_operation_key,
        corrected.closure_operation_key
    );
    assert!(detail.legacy_handoff.is_none());
    assert!(detail.merge_requests.is_empty());

    // Same state and content are insufficient after correcting a closure.
    let stale_correction = case_request(
        &closed,
        "stale-correction",
        json!({"action": "correct_closure", "reason": "duplicate"}),
    );
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(stale_correction),
        StatusCode::CONFLICT,
    )
    .await;
    let reopen = case_request(&corrected, "reopen", json!({"action": "reopen"}));
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(reopen.clone()),
        StatusCode::OK,
    )
    .await;
    let reopened = backend.show(reference.id.clone().into()).unwrap();
    assert_eq!(reopened.meta.workflow_state, ticket::TicketState::Open);
    assert!(reopened.current_closure.is_none());
    assert_eq!(
        reopened.closure_operation_key,
        corrected.closure_operation_key
    );
    assert_eq!(reopened.case_history.len(), 3);
    assert_eq!(ticket::ticket_content_digest(&reopened), digest);

    // Exact replay returns current authority, not the old closed response.
    for replay in [close.clone(), correction, reopen] {
        let result = request_json(app.clone(), "POST", &path, Some(replay), StatusCode::OK).await;
        let summary: server_api::TicketSummary = serde_json::from_value(result).unwrap();
        assert_eq!(summary.0.workflow_state, ticket::TicketState::Open);
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), reopened);
    }
    let mut changed_replay = close.clone();
    changed_replay["explanation"] = json!("Different judgment under the same operation key");
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(changed_replay),
        StatusCode::CONFLICT,
    )
    .await;
    let mut aba_close = close;
    aba_close["operation_key"] = json!("stale-open-close");
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(aba_close),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(backend.show(reference.id.clone().into()).unwrap(), reopened);
    let second_close = case_request(
        &reopened,
        "second-close",
        json!({"action": "close", "reason": "not_planned"}),
    );
    request_json(
        app.clone(),
        "POST",
        &path,
        Some(second_close),
        StatusCode::OK,
    )
    .await;
    let final_ticket = backend.show(reference.id.into()).unwrap();
    assert_eq!(
        final_ticket.meta.workflow_state,
        ticket::TicketState::Closed
    );
    assert_eq!(final_ticket.case_history.len(), 4);
    assert_eq!(final_ticket.case_history[..3], reopened.case_history);
    assert_eq!(
        final_ticket.closure_operation_key.as_deref(),
        Some("second-close")
    );
    assert_eq!(ticket::ticket_content_digest(&final_ticket), digest);
}

#[tokio::test]
async fn case_route_rejects_stale_content_state_and_invalid_actions_without_audit_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("CAS decision"))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let app = case_actor_router(api);
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case",
        reference.id
    );
    let close = case_request(
        &before,
        "close",
        json!({"action": "close", "reason": "unknown"}),
    );
    for (field, value) in [
        ("expected_content_digest", json!("stale-content")),
        ("expected_state", json!("closed")),
        ("action", json!({"action": "reopen"})),
        (
            "action",
            json!({"action": "correct_closure", "reason": "duplicate"}),
        ),
    ] {
        let mut invalid = close.clone();
        invalid[field] = value;
        request_json(
            app.clone(),
            "POST",
            &path,
            Some(invalid),
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), before);
    }
    request_json(app.clone(), "POST", &path, Some(close), StatusCode::OK).await;
    let closed = backend.show(reference.id.clone().into()).unwrap();
    let duplicate_close = case_request(
        &closed,
        "duplicate-close",
        json!({"action": "close", "reason": "completed"}),
    );
    request_json(
        app,
        "POST",
        &path,
        Some(duplicate_close),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(backend.show(reference.id.into()).unwrap(), closed);
}

// Production case transition, assignment adapter, review store and completion
// transaction. Provider refs are supplied at the store boundary in this fixture.
#[tokio::test]
async fn closed_and_reopened_tickets_allow_orchestrator_review_and_integration_without_reassignment()
 {
    for reopen in [false, true] {
        let mut fixture = EvidenceApiFixture::new().await;
        fixture
            .backend
            .update_case(&fixture.ticket_id, fixture.closure())
            .unwrap();
        if reopen {
            fixture
                .backend
                .update_case(
                    &fixture.ticket_id,
                    ticket::TicketCaseUpdate {
                        operation_key: "reopen-for-test".into(),
                        expected_content_digest: fixture.content_digest(),
                        expected_state: ticket::TicketState::Closed,
                        expected_closure_operation_key: Some("evidence-ticket-closure".into()),
                        action: ticket::TicketCaseAction::Reopen,
                        explanation: "Continue the case without replacing implementation context"
                            .into(),
                        references: vec![],
                        author: Some("fixture".into()),
                    },
                )
                .unwrap();
        }
        assert!(
            fixture
                .api
                .store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .is_none()
        );
        // The review owner is not the implementing Coder. Its own attested child
        // still owns the verdict; changing the owner never grants self-review.
        fixture
            .store
            .register_reviewer_child_session(RegisterReviewerChildSession {
                workspace_id: TEST_WORKSPACE_ID.into(),
                parent_runtime_id: fixture.auth.runtime_id.clone(),
                parent_worker_id: fixture.auth.worker_id.clone(),
                child_session_id: "coder-owned-child".into(),
                reviewer_profile: "builtin:reviewer".into(),
                now: evidence_time(),
            })
            .unwrap();
        fixture.auth.runtime_id = "orchestrator-runtime".into();
        fixture.auth.worker_id = "orchestrator".into();
        let foreign_child = fixture.store.request_review(RequestMergeRequestReview {
            merge_request_id: EVIDENCE_MR.into(),
            ticket_id: fixture.ticket_id.clone(),
            ticket_content_digest: fixture.content_digest(),
            ticket_merge_request_subjects: fixture.subjects(),
            subject_ref: EVIDENCE_SOURCE.into(),
            child_session_id: "coder-owned-child".into(),
            capability_token: "foreign-child-token".into(),
            auth: fixture.auth.clone(),
            now: evidence_time(),
        });
        assert!(matches!(
            foreign_child,
            Err(merge_request::MergeRequestError::Unauthorized(_))
        ));
        let old = fixture.approve("old-review");
        let revoked = fixture
            .store
            .revoke_review(merge_request::RevokeMergeRequestReview {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: fixture.ticket_id.clone(),
                review_event_id: old.event_id,
                reason: "Request a fresh independent review".into(),
                auth: fixture.auth.clone(),
                now: evidence_time(),
            })
            .unwrap();
        assert_eq!(revoked.revoked_by.worker_id, "orchestrator");
        let approval = fixture.approve("fresh-review");
        let ready = fixture
            .store
            .readiness(merge_request::ReadinessCheck {
                merge_request_id: EVIDENCE_MR.into(),
                ticket_id: fixture.ticket_id.clone(),
                current_subject_ref: Some(EVIDENCE_SOURCE.into()),
                auth: fixture.auth.clone(),
            })
            .unwrap();
        assert!(ready.ready, "{:?}", ready.blockers);
        let integrated = fixture.integrate(&approval);
        assert_eq!(integrated.merged_by.worker_id, "orchestrator");
        assert_eq!(integrated.approval_event_id, approval.event_id);
        let shown = fixture.show().await;
        assert_eq!(shown.state.as_str(), if reopen { "open" } else { "closed" });
        assert_eq!(shown.merge_requests[0].state, "merged");
        assert!(
            fixture
                .api
                .store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture
                .api
                .store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .unwrap()
                .assignment_id,
            fixture.auth.assignment_id
        );
    }
}
