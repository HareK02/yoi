//! Completion admission component: real owner policy adoption, singleton/assignment
//! stores and MR transaction callback. No MR is created: the positive control
//! reaches domain NotFound, while authority changes must refuse before domain load.
//! Full signed HTTP/approved integration is owned by the connected MR scenario.
use super::*;

#[tokio::test]
async fn completion_admission_rechecks_canonical_policy_assignment_body_and_signed_binding() {
    for change in ["unchanged", "policy", "assignment", "body", "binding"] {
        let f = Fixture::new().await;
        f.adopt_policy(r#"{ namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#, "{}").await;
        let ticket = browser_ticket_backend(&f.api)
            .unwrap()
            .create(NewTicket::new("MR completion admission"))
            .unwrap()
            .id;
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator");
        let (status, assigned) = response(&f.api, human_request(&f.owner, "PUT", &path,
            Some(json!({"operation_id":"completion-admission-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":f.subject}})))).await;
        assert_eq!(status, StatusCode::OK, "{assigned}");
        let admission =
            subject_authorization::merge_completion_subject(&f.api, &f.source, &ticket).unwrap();
        let binding = f.api.config_store.with_conn(|connection| {
            Ok(connection.query_row("SELECT binding_id FROM workspace_runtime_bindings WHERE workspace_id=?1 AND runtime_id=?2 AND revoked_at IS NULL", rusqlite::params![TEST_WORKSPACE_ID, f.source.runtime_id], |row| row.get::<_, String>(0))?)
        }).unwrap();
        let source = server_api::ServerRuntimeSource {
            runtime_id: f.source.runtime_id.clone(),
            worker_id: Some(f.source.worker_id.clone()),
            authentication_binding_id: binding,
        };
        match change {
            "policy" => f.adopt_policy("{}", "{}").await,
            "assignment" => {
                let next = open_subjektiv_store(&f.api)
                    .unwrap()
                    .create_subject(crate::subjektiv::SubjectRole::new("Next assignee").unwrap())
                    .unwrap();
                let (status, body) = response(&f.api, human_request(&f.owner, "PUT", &path,
                    Some(json!({"operation_id":"completion-admission-reassign","expected_assignment_id":assigned["assignment"]["assignment_id"],"principal":{"kind":"subject","subject_id":next.id}})))).await;
                assert_eq!(status, StatusCode::OK, "{body}");
            }
            "body" => f.release_source_lease(),
            "binding" => {
                assert!(
                    f.api
                        .config_store
                        .revoke_workspace_runtime_binding(
                            TEST_WORKSPACE_ID,
                            &f.source.runtime_id,
                            &Utc::now().to_rfc3339()
                        )
                        .unwrap()
                );
            }
            _ => {}
        }
        let store = merge_request_store(&f.api, TEST_WORKSPACE_ID).unwrap();
        let completion = merge_request::CompleteMergeRequest {
            merge_request_id: "not-created".into(),
            ticket_id: ticket.clone(),
            operation_id: "admission".into(),
            approval_event_id: "not-created".into(),
            current_subject_ref: "source".into(),
            target_ref_before: "before".into(),
            target_ref_after: "after".into(),
            strategy: merge_request::MergeStrategy::FastForward,
            resolution: merge_request::ConflictResolution::None,
            auth: merge_request::MergeRequestAuth {
                workspace_id: TEST_WORKSPACE_ID.into(),
                repository_id: "test-repository".into(),
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone(),
                assignment_id: "not-created".into(),
            },
            now: Utc::now(),
        };
        let error = admission
            .complete(&f.api, &source, &store, completion)
            .unwrap_err();
        let status = error.into_response().status();
        assert_eq!(
            status,
            if change == "unchanged" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            },
            "{change}"
        );
        assert!(
            store
                .list_for_ticket(TEST_WORKSPACE_ID, &ticket)
                .unwrap()
                .is_empty()
        );
    }
}
