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

// Deliberately freeze review dates: freshness must follow the exact item
// revision and source/result snapshot, not a comparison of wall-clock dates.
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
        input.workflow_state = Some(TicketWorkflowState::InProgress);
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

    fn revision(&self) -> String {
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
                ticket_content_digest: self.revision(),
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

    fn completion(&self, _approval: &ReviewEvent) -> ticket::TicketCompletion {
        ticket::TicketCompletion {
            operation_key: "evidence-ticket-completion".into(),
            expected_content_digest: self.revision(),
            expected_state: TicketWorkflowState::InProgress,
            reason: "Implementation judged complete independently of MR attestation".into(),
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
                        "Revised requirements require a fresh exact-revision approval.",
                    )),
                    ..Default::default()
                },
            )
            .unwrap();
    }
}

#[tokio::test]
async fn done_merged_remote_ticket_uses_immutable_result_in_show_query_list_and_paged_detail() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("integration");
    let merge = fixture.integrate(&approval);
    let revision = fixture.revision();
    let completion = fixture
        .backend
        .complete(&fixture.ticket_id, fixture.completion(&approval))
        .unwrap();
    assert_eq!(ticket::ticket_content_digest(&completion), revision);
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
    assert_eq!(shown.state, "done");
    assert_eq!(shown.content_digest, revision);
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
async fn latest_postmerge_approval_requires_exact_revision_and_stored_source_result_snapshot() {
    let fixture = EvidenceApiFixture::new().await;
    let integration = fixture.approve("integration");
    let merge = fixture.integrate(&integration);
    fixture.rescope();
    let revision = fixture.revision();
    assert_ne!(integration.ticket_content_digest, revision);

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
    let mut outdated_completion = fixture.completion(&integration);
    outdated_completion.expected_content_digest = integration.ticket_content_digest.clone();
    assert!(
        fixture
            .backend
            .complete(&fixture.ticket_id, outdated_completion)
            .is_err()
    );
    assert_eq!(fixture.show().await.state, "inprogress");

    let latest = fixture.approve("latest-revision");
    assert_eq!(latest.created_at, integration.created_at);
    assert_eq!(latest.ticket_content_digest, revision);
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
    // A current revision alone is insufficient: simulate a stored attestation
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
    assert_eq!(fixture.show().await.state, "inprogress");

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
        .complete(&fixture.ticket_id, fixture.completion(&repaired))
        .unwrap();
    assert_eq!(ticket::ticket_content_digest(&completion), revision);
    assert_eq!(fixture.show().await.state, "done");
    fixture.assert_filters(true, false, false).await;
}

#[tokio::test]
async fn open_source_without_runtime_is_typed_unavailable_not_stale_after_rescope() {
    let fixture = EvidenceApiFixture::new().await;
    let approval = fixture.approve("open-review");
    fixture.rescope();
    // End this assignment identity's work without erasing its responsibility.
    // A retained Coder must not silently supply a Runtime for an open source.
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
            code: "source_ref_runtime_unavailable".into(),
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
    assert_eq!(
        listed.items[0].ref_diagnostics[0].code,
        "source_ref_runtime_unavailable"
    );
    let detail = fixture.detail(approval.sequence, 1).await;
    assert!(detail.merge_request.thread.is_empty());
    assert_eq!(detail.source.status, "unknown");
    assert_eq!(detail.source.resolved_ref, None);
    assert_eq!(
        detail.source.diagnostic.unwrap().code,
        "source_ref_runtime_unavailable"
    );
    assert_eq!(detail.target.status, "unknown");
    assert_eq!(
        detail.target.diagnostic.unwrap().code,
        "target_ref_runtime_unavailable"
    );
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
        .update_state(
            &fixture.ticket_id,
            ticket::TicketStateUpdate {
                operation_key: "close-with-open-mr".into(),
                expected_content_digest: fixture.revision(),
                expected_state: TicketWorkflowState::InProgress,
                state: TicketWorkflowState::Closed,
                reason: "The request was withdrawn; no integration or approval is asserted".into(),
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
    assert_eq!(shown.state, "closed");
    assert!(!shown.evidence.approved_current_subject);
    assert_eq!(shown.merge_requests[0].state, "open");
}

#[tokio::test]
async fn mrless_completion_is_a_ticket_judgment_not_approved_or_missing_merge_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Investigation with no repository"))
        .unwrap();
    let ticket = backend.show(reference.id.clone().into()).unwrap();
    backend
        .complete(
            &reference.id,
            ticket::TicketCompletion {
                operation_key: "research-result".into(),
                expected_content_digest: ticket::ticket_content_digest(&ticket),
                expected_state: TicketWorkflowState::Planning,
                reason: "Answer recorded in the Ticket thread".into(),
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
    assert_eq!(shown.state, "done");
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

#[tokio::test]
async fn all_public_state_and_close_routes_require_explicit_cas_and_operation_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api_with_remote_repository(temp.path()).await;
    let backend = browser_ticket_backend(&api).unwrap();
    let reference = backend
        .create(ticket::NewTicket::new("Transport decisions"))
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let app = build_inner_router(api);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", reference.id);
    for suffix in ["workflow-state", "state-changes", "state-fields/state"] {
        request_json(
            app.clone(),
            "POST",
            &format!("{path}/{suffix}"),
            Some(json!({
                "from":"planning","to":"done","reason":"old unguarded request","body":""
            })),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    request_json(
        app.clone(),
        "POST",
        &format!("{path}/workflow/close"),
        Some(json!("unguarded close")),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    let close = json!({
        "operation_key":"guarded-close","expected_content_digest":ticket::ticket_content_digest(&before),
        "expected_state":"planning","reason":"No further work required","author":"spoofed-client-author"
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{path}/workflow/close"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let closed = backend.show(reference.id.clone().into()).unwrap();
    assert_eq!(closed.meta.workflow_state, TicketWorkflowState::Closed);
    let receipt = closed.events.last().unwrap();
    assert_eq!(receipt.author.as_deref(), Some("workspace-user"));
    assert_eq!(receipt.from.as_deref(), Some("planning"));
    assert_eq!(
        receipt.attributes.get("operation_key"),
        Some(&"guarded-close".to_string())
    );
    let reopen = json!({
        "operation_key":"reopen","expected_content_digest":ticket::ticket_content_digest(&closed),
        "expected_state":"closed","state":"planning","reason":"A new question arrived"
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{path}/workflow-state"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(reopen.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    // Exact replay of the old close is a receipt lookup, not another close.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{path}/workflow/close"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let reopened = backend.show(reference.id.into()).unwrap();
    assert_eq!(reopened.meta.workflow_state, TicketWorkflowState::Planning);
    assert_eq!(reopened.events.len(), closed.events.len() + 1);
}
