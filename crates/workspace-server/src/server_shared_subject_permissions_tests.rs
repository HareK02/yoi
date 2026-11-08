// Production Subject creation/connection, Ticket assignment persistence, notification
// delivery authorization, Runtime admission and subsequent reads. Only the Runtime
// Controller/model is substituted by the recording backend; acceptance is not work.
use super::*;

#[tokio::test]
async fn shared_subject_set_accepts_new_ticket_assignment_without_id_enrollment() {
    use server_api::authorization::{
        OperationAuthorizationRequest, PermissionResource, SubjectPolicySource,
    };
    use session_store::WorkerMetadataStore;
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (mut api, execution) = assignment_test_api_with_recording_backend(dir.path()).await;
    configure_runtime_request_auth(
        &mut api,
        &EMBEDDED_RUNTIME_REQUEST_IDENTITY,
        EMBEDDED_WORKER_RUNTIME_ID,
    );
    let subjects = open_subjektiv_store(&api).unwrap();
    let (_, Json(created)) = scoped_create_subjektiv_subject(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        Extension(test_owner_actor()),
        Json(server_api::SubjektivSubjectCreateRequest {
            operation_id: "create-shared-set-recipient".into(),
            role: "shared-set-recipient".into(),
            behavior_md: "Handle the assigned Ticket; admission is not completion.".into(),
        }),
    )
    .await
    .unwrap();
    let subject = subjects.subject(&created.id).unwrap().unwrap();
    let policy = match subjects
        .subject_policy_source(&subject.id)
        .unwrap()
        .unwrap()
    {
        SubjectPolicySource::Feature(policy) => policy,
        _ => panic!("new Subjects must save the shared assignment, not a fixed scoped copy"),
    };
    assert_eq!(
        policy.execution,
        server_api::authorization::subject_default_permission_assignment()
    );
    assert_eq!(policy.delegation, server_api::authorization::subject_default_delegation_assignment());
    assert!(policy.legacy_constraints.is_none());
    let body = create_test_assignment_body(&api, EMBEDDED_WORKER_RUNTIME_ID, &subject.id).await;

    // The recording Runtime substitutes Controller startup; attach real canonical
    // pending Session metadata and writer identity without altering policy.
    let name = format!("worker-runtime-{}", body.worker_id);
    let aggregate = session_store::worker_metadata::WorkerAggregateStore::new(
        api.config
            .embedded_runtime_store_root
            .join("workers")
            .join(&body.worker_id),
        &name,
    )
    .unwrap();
    let mut metadata = aggregate
        .read_read_only()
        .unwrap()
        .unwrap_or_else(|| session_store::WorkerMetadata::new(&name, None));
    let session = metadata
        .active
        .as_ref()
        .map(|active| active.session_id)
        .unwrap_or_else(session_store::new_session_id);
    metadata.active = Some(session_store::WorkerActiveSegmentRef::pending_segment(
        session,
    ));
    aggregate
        .write(&metadata.with_workspace_id(api.workspace_id()))
        .unwrap();
    subjektiv_journal::bind_host_session(
        &api,
        crate::subjektiv::SubjectSessionAttribution::new(
            &subject.id,
            &body.runtime_id,
            &body.worker_id,
            session.to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    for operation in ["chat.read", "chat.send"] {
        subject_authorization::authorize(
            &api,
            &body,
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: PermissionResource::Subject {
                    workspace_id: api.workspace_id().into(),
                    subject_id: subject.id.clone(),
                },
            },
        )
        .unwrap();
    }
    let ticket = browser_ticket_backend(&api)
        .unwrap()
        .create(ticket::NewTicket::new(
            "Created after Subject permission enrollment",
        ))
        .unwrap();
    api.store
        .set_current_ticket_role_assignment(
            &TicketRoleAssignmentRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                ticket_id: ticket.id.clone(),
                assignment_id: "shared-set-assignment".into(),
                role: TicketAssignmentRole::Orchestrator,
                principal: TicketAssignmentPrincipal::Subject {
                    subject_id: subject.id.clone(),
                },
                assigned_by: "fixture-owner".into(),
                assigned_at: TEST_CREATED_AT.into(),
            },
            None,
            "shared-set-event",
            "shared-set-assign",
            false,
        )
        .unwrap();
    let saved = api
        .store
        .list_pending_ticket_assignment_deliveries(TEST_WORKSPACE_ID, 10)
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].ticket_id, ticket.id);
    authorize_assignment_delivery(&api, &body, &ticket.id).unwrap();
    notification_delivery::recover_ticket_notifications(&api);
    let inputs = execution.take_inputs();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0.worker_id.to_string(), body.worker_id);
    assert!(inputs[0].1.contains("shared-set-assignment"));
    assert!(
        api.store
            .list_pending_ticket_assignment_deliveries(TEST_WORKSPACE_ID, 10)
            .unwrap()
            .is_empty()
    );
    for operation in [
        "ticket.read",
        "ticket.assignment.query",
        "ticket.assignment.history",
    ] {
        subject_authorization::authorize(
            &api,
            &body,
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: PermissionResource::Ticket {
                    workspace_id: api.workspace_id().into(),
                    ticket_id: ticket.id.clone(),
                },
            },
        )
        .unwrap();
    }
    for (method, path, input) in [
        (
            "POST",
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/show", ticket.id),
            b"{}".to_vec(),
        ),
        (
            "GET",
            format!(
                "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/assignment-tracking",
                ticket.id
            ),
            vec![],
        ),
    ] {
        let request = runtime_source_request(
            &EMBEDDED_RUNTIME_REQUEST_IDENTITY,
            Some(&body.worker_id),
            method,
            &path,
            input,
        );
        let response = build_router(api.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(status, StatusCode::OK, "{method} {path}: {text}");
        assert!(text.contains("shared-set-assignment"), "{text}");
    }
    assert_eq!(
        subjects.subject_policy_source(&subject.id).unwrap(),
        Some(SubjectPolicySource::Feature(policy))
    );
    assert_eq!(
        browser_ticket_backend(&api)
            .unwrap()
            .show(TicketIdOrSlug::Id(ticket.id))
            .unwrap()
            .meta
            .workflow_state,
        TicketState::Open
    );
}
