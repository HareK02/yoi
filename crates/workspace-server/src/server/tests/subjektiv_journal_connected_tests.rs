//! Connected Backend → production Runtime factory → real Controller → Journal → signed
//! Host startup reader → persisted first input. Only the external model stream is scripted.
//! Browser authentication is outside this regression: creation enters the real scoped handler.
use super::*;
use agen::llm_client::{
    LlmClient, Request,
    error::ClientError,
    event::{Event, StopReason},
    types::Item,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

use super::super::restored_parent_control_tests::connected_restore_tempdir;

#[derive(Clone, Default)]
struct ConnectedJournalModel {
    dialogs: Arc<Mutex<Vec<Request>>>,
    journal_inputs: Arc<Mutex<Vec<String>>>,
}

impl ConnectedJournalModel {
    fn journal_response(request: &Request) -> Vec<Event> {
        // Consume the actual Controller-rendered input, not a fixture generation or
        // a Source tool response. Decode also checks complete bodies/ranges/digests.
        let envelope = request
            .items
            .iter()
            .filter_map(Item::as_text)
            .find(|text| text.lines().any(|line| line.starts_with("input_json: ")))
            .expect("rendered immutable Backend Job envelope");
        let input_json = envelope
            .lines()
            .find_map(|line| line.strip_prefix("input_json: "))
            .unwrap();
        let input_value: Value = serde_json::from_str(input_json).unwrap();
        let frozen = subjektiv::journal::recording::JournalInput::decode(&input_value).unwrap();
        assert_eq!(
            serde_json::to_string(&input_value).unwrap(),
            input_json,
            "render preserves the exact persisted JSON bytes"
        );
        assert!(frozen.capture.entries.iter().all(|entry| matches!(
            &entry.body, session_store::capture::CaptureBody::CanonicalPublic { text, .. } if !text.is_empty())));
        let ranges = serde_json::to_value(&frozen.selected_ranges).unwrap();
        // Runtime Controllers deliberately retain alias-routed core schemas for their
        // lifetime even with zero attachments. Verify invocation authority, not schema
        // absence (standalone Internal Jobs use a different composition).
        for (id, tool, input) in [
            (
                "denied-bash",
                "Bash",
                json!({"command":"printf JOURNAL_MUST_NOT_EXECUTE","cwd":"."}),
            ),
            (
                "denied-read",
                "Read",
                json!({"file_path":"journal-must-not-be-readable"}),
            ),
        ] {
            if let Some(error) = request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id, is_error, ..
                } if call_id == id => Some(*is_error),
                _ => None,
            }) {
                assert!(
                    error,
                    "Journal Job unexpectedly gained {tool} Workdir authority"
                );
            } else {
                return vec![
                    Event::tool_use_start(0, id, tool),
                    Event::tool_input_delta(0, input.to_string()),
                    Event::tool_use_stop(0),
                ];
            }
        }
        let result = |id: &str| -> Option<Value> {
            request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    content,
                    is_error,
                    summary,
                    ..
                } if call_id == id => {
                    assert!(!is_error, "{id}: {summary}; {content:?}");
                    Some(
                        content
                            .as_deref()
                            .map(|text| serde_json::from_str(text).unwrap())
                            .unwrap_or(Value::Null),
                    )
                }
                _ => None,
            })
        };
        if result("submit").is_some() {
            return vec![
                Event::text_block_start(0),
                Event::text_delta(0, "saved"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ];
        }
        let (tool, id, input) = if result("checkpoint").is_some() {
            ("SubmitBackendJobResult", "submit", json!({"result":{}}))
        } else if let Some(position) = result("position") {
            let state = result("state").unwrap();
            (
                "CheckpointJournal",
                "checkpoint",
                json!({
                    "request_id":format!("checkpoint:{}",state["value"]["generation_id"].as_str().unwrap()),
                    "expected_state":position["value"]["state_token"],
                    "action":{"action":"checkpoint","record_operation_ids":[format!("record:{}",state["value"]["generation_id"].as_str().unwrap())]},
                    "covered_ranges":ranges,
                }),
            )
        } else if let Some(record) = result("record") {
            let state = result("state").unwrap();
            (
                "UpdateJournalPosition",
                "position",
                json!({
                    "request_id":format!("position:{}",state["value"]["generation_id"].as_str().unwrap()),
                    "expected_state":record["value"]["state_token"],
                    "action":{"action":"update_position","current_position":"CONNECTED_HANDOFF: partially implemented; outcome unconfirmed"},
                    "covered_ranges":[],
                }),
            )
        } else if let Some(state) = result("state") {
            let state = &state["value"];
            assert_eq!(
                state["selected_ranges"], ranges,
                "mutation coverage must come from the rendered frozen input"
            );
            assert_eq!(
                state["initial_journal"]["state_token"],
                input_value["initial_state_token"]
            );
            let (tool, action) = if state["current_open"].is_null() {
                (
                    "CreateJournalEntry",
                    json!({"action":"create","title":"Connected handoff","body":"CONNECTED_HANDOFF: proposed A, user chose B; only partial work performed.","ongoing":"Remaining work; do not claim success","state":"open","source_end":null,"correction_refs":[]}),
                )
            } else {
                (
                    "RewriteJournalEntry",
                    json!({"action":"rewrite","entry_id":state["current_open"]["id"],"title":"Connected handoff","body":"CONNECTED_HANDOFF: B selected; partial work remains.","ongoing":"Remaining work; result unknown"}),
                )
            };
            (
                tool,
                "record",
                json!({
                    "request_id":format!("record:{}",state["generation_id"].as_str().unwrap()),
                    "expected_state":state["current_journal"]["state_token"],
                    "action":action,"covered_ranges":ranges,
                }),
            )
        } else {
            ("ReadJournalState", "state", json!({}))
        };
        assert!(
            request
                .tools
                .iter()
                .any(|definition| definition.name == tool),
            "missing {tool}"
        );
        for forbidden in ["MemoryApplyCandidate", "SubjektivSessionRead"] {
            assert!(
                !request
                    .tools
                    .iter()
                    .any(|definition| definition.name == forbidden),
                "Journal exposed forbidden {forbidden}: {:?}",
                request
                    .tools
                    .iter()
                    .map(|tool| &tool.name)
                    .collect::<Vec<_>>()
            );
        }
        vec![
            Event::tool_use_start(0, id, tool),
            Event::tool_input_delta(0, input.to_string()),
            Event::tool_use_stop(0),
        ]
    }
}

#[async_trait::async_trait]
impl LlmClient for ConnectedJournalModel {
    async fn stream(
        &self,
        request: Request,
    ) -> std::result::Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = std::result::Result<Event, ClientError>> + Send>,
        >,
        ClientError,
    > {
        let events = if request
            .tools
            .iter()
            .any(|tool| tool.name == "ReadJournalState")
        {
            let envelope = request
                .items
                .iter()
                .filter_map(Item::as_text)
                .find(|text| text.lines().any(|line| line.starts_with("input_json: ")))
                .expect("actual Journal input in model render")
                .to_owned();
            let mut inputs = self.journal_inputs.lock().unwrap();
            if !inputs.contains(&envelope) {
                inputs.push(envelope);
            }
            drop(inputs);
            Self::journal_response(&request)
        } else if request.tools.iter().any(|tool| {
            matches!(
                tool.name.as_str(),
                "SubmitBackendJobResult" | "SubmitMemorySurface"
            )
        }) {
            // The real companion also requests an empty Memory consolidation Job. Keep that
            // lifecycle real without counting its model requests as main-body dialog requests.
            if request.items.iter().any(|item| matches!(item, Item::ToolResult { call_id, is_error, .. } if call_id == "empty-memory" && !is_error)) {
                vec![Event::text_block_start(0), Event::text_delta(0, "empty Memory acknowledged"), Event::text_block_stop(0, Some(StopReason::EndTurn))]
            } else {
                let (tool, input) = if request.tools.iter().any(|tool| tool.name == "SubmitMemorySurface") {
                    ("SubmitMemorySurface", json!({"points":[]}))
                } else {
                    let input: Value = request.items.iter().filter_map(Item::as_text).find_map(|text| text.lines().find_map(|line| line.strip_prefix("input_json: "))).map(|input| serde_json::from_str(input).unwrap()).expect("Backend Memory Job envelope");
                    assert_eq!(input["candidate_ids"], json!([]));
                    ("SubmitBackendJobResult", json!({"result":{"subject_id":input["subject_id"],"candidate_ids":[]}}))
                };
                vec![Event::tool_use_start(0, "empty-memory", tool), Event::tool_input_delta(0, input.to_string()), Event::tool_use_stop(0)]
            }
        } else {
            self.dialogs.lock().unwrap().push(request);
            vec![
                Event::text_block_start(0),
                Event::text_delta(
                    0,
                    "Partial B implementation; remaining work is not confirmed.",
                ),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

async fn connected_create(api: &WorkspaceApi, subject: &str, name: &str) -> RuntimeWorkerRef {
    let Json(created) = scoped_create_workspace_worker(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath { workspace_id: TEST_WORKSPACE_ID.into() }),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(), display_name: name.into(),
            singleton_key: None, profile: Some("builtin:companion".into()), ticket_assignment: None,
            initial_submit: vec![protocol::Segment::Text { content: "User chose B rather than A. Continue the partial work without assuming success.".into() }],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest { subject_id: subject.into() }),
            }, control_operation_id: None, execution_grant: None,
        }),
    ).await.unwrap();
    RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
}

async fn connected_wait_for_recording(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) {
    let host = JournalHostContext::validated_session(store, attr.clone()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(25));
        loop {
            tick.tick().await;
            let jobs = api
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &attr.session_id, None, 100)
                .unwrap();
            let generation_closed =
                store
                    .journal_for_session(&host)
                    .unwrap()
                    .is_some_and(|journal| {
                        store
                            .active_journal_generation(&host, &journal)
                            .unwrap()
                            .is_none()
                    });
            if !jobs.is_empty()
                && generation_closed
                && jobs
                    .iter()
                    .all(|job| job.state == BackendJobState::Completed)
                && !store.journal_retention_required(&host).unwrap()
                && api
                    .runtime
                    .worker(&RuntimeWorkerRef::new(&attr.runtime_id, &attr.worker_id))
                    .unwrap()
                    .worker_state
                    .is_some_and(|state| state.catalog_status() == protocol::WorkerStatus::Idle)
            {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "real Controller recording did not complete: {:?}",
            api.store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &attr.session_id, None, 100)
                .unwrap()
        )
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn backend_real_runtime_journal_release_and_next_body_persisted_startup() {
    let dir = connected_restore_tempdir();
    init_clean_git_workspace(dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = test_server_config(dir.path());
    config.backend_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
    let control = Arc::new(test_control_store(&config));
    seed_test_registered_workspace(control.as_ref(), &config)
        .await
        .unwrap();
    let broker = BackendResourceBroker::default();
    let dispatcher = Arc::new(
        crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
            config.clone(),
            control.clone(),
        ),
    );
    let model = ConnectedJournalModel::default();
    let model_factory = model.clone();
    let audience = embedded_runtime_request_audience(&config).unwrap();
    let execution = worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(
        worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
            config.workspace_execution_root.clone(),
        )
        .with_embedded_worker_mutation_dispatcher(EMBEDDED_WORKER_RUNTIME_ID, dispatcher.clone())
        .with_workspace_request_client(
            worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                TEST_WORKSPACE_ID,
                audience.clone(),
                EMBEDDED_WORKER_RUNTIME_ID,
            )
            .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience),
        )
        .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
        .with_resource_client(Arc::new(broker.clone()))
        .with_spawn_model_client_factory(move || Box::new(model_factory.clone())),
    )
    .unwrap();
    let api = WorkspaceApi::new_with_execution_backend_and_broker(
        config,
        control,
        Arc::new(execution),
        broker,
        HostExecutors::default(),
        Some(dispatcher),
    )
    .await
    .unwrap();
    seed_test_delivery_binding(&api);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let app = build_router(api.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            async move {
                let path = request.uri().path().to_string();
                let response = next.run(request).await;
                observed.lock().unwrap().push((path, response.status()));
                response
            }
        },
    ));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("connected-handoff").unwrap())
        .unwrap();
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "connected-journal-owner").await;
    // This fixture exercises Journal continuity, not unset-policy refusal. Assign
    // the companion's exact activation keys through authenticated administration;
    // no filesystem, shell or delegation permission is granted to its body.
    let update = server_api::authorization::SubjectPermissionsUpdate {
        operation_id: "connected-journal-activation".into(),
        expected_change_id: None,
        execution: [
            "task",
            "subjektiv",
            "web",
            "image",
            "sub_worker",
            "worker",
            "workspace_worker_discovery",
            "objective",
            "manage_workdir",
            "workdir_catalog",
            "drive",
            "ticket",
        ]
        .into_iter()
        .map(|name| server_api::authorization::ScopedPermission {
            operation: name.into(),
            resource: server_api::authorization::PermissionResource::Feature,
        })
        .collect(),
        delegation: Vec::new(),
    };
    let response = build_router(api.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions",
                    subject.id
                ))
                .header("authorization", format!("Bearer {owner}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&update).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let old = connected_create(&api, &subject.id, "connected-old-body").await;
    let old_attr = store
        .subject_session_attributions(&subject.id)
        .unwrap()
        .into_iter()
        .find(|attr| attr.worker_id == old.worker_id)
        .unwrap();
    connected_wait_for_recording(&api, &store, &old_attr).await;
    let old_host = JournalHostContext::validated_session(&store, old_attr.clone()).unwrap();
    let old_binding = JournalWriterBinding {
        subject_id: subject.id.clone(),
        runtime_id: old.runtime_id.clone(),
        worker_id: old.worker_id.clone(),
        binding_id: old_host.writer_binding_id().unwrap().into(),
    };
    // Inject a durable result-acceptance gap after real Journal completion. This must
    // reject before writer revocation and confirm cancellation of the actual Controller
    // guard; merely returning Err would strand the body and its execution-admission fence.
    let recorded = api
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &old_attr.session_id, None, 100)
        .unwrap();
    // Compare actual render with the durable identity, even if completion already
    // reclaimed the input body. Reconstituting only for comparison is test-local.
    for saved in &recorded {
        let inputs = model.journal_inputs.lock().unwrap();
        let prefix = format!("job_id: {}", saved.request.job_id);
        let rendered = inputs
            .iter()
            .find(|text| text.lines().any(|line| line == prefix))
            .expect("every saved Journal Job reached the real Controller");
        let actual = rendered
            .lines()
            .find_map(|line| line.strip_prefix("input_json: "))
            .unwrap();
        let mut original = saved.request.clone();
        original.input = serde_json::from_str(actual).unwrap();
        assert_eq!(original.input_digest().unwrap(), saved.input_digest);
        let attempt = crate::backend_job::attempt_id(&saved.request.job_id, saved.current_attempt);
        assert_eq!(*rendered, original.worker_input(&attempt).unwrap());
        let input = subjektiv::journal::recording::JournalInput::decode(&original.input).unwrap();
        assert_eq!(input.capture.session_id, old_attr.session_id);
        assert!(actual.contains("User chose B rather than A"));
        assert!(actual.contains("Partial B implementation"));
    }
    let uncertain_job = &recorded[0].request.job_id;
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let saved_result: (String, String) = conn.query_row(
        "SELECT result_json,result_digest FROM backend_jobs WHERE workspace_id=?1 AND job_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, uncertain_job], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    conn.execute(
        "UPDATE backend_jobs SET state='unknown',result_json=NULL,result_digest=NULL WHERE workspace_id=?1 AND job_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, uncertain_job],
    )
    .unwrap();
    let incomplete = json!({
        "worker":old,"binding_id":old_binding.binding_id,"operation_id":"connected-incomplete-release",
    });
    let error = subjektiv_journal::release_subject(
        &api,
        serde_json::from_value(incomplete.clone()).unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Unknown"), "{error}");
    assert!(store.journal_writer_active(&old_binding).unwrap());
    assert!(
        !api.config_store
            .subject_release_pending(TEST_WORKSPACE_ID, &old)
            .unwrap()
    );
    let cancelled: bool = conn.query_row("SELECT cancelled FROM subject_worker_releases WHERE workspace_id=?1 AND operation_id='connected-incomplete-release'", [TEST_WORKSPACE_ID], |r| r.get(0)).unwrap();
    assert!(
        cancelled,
        "checked pre-revoke refusal needs a terminal cancellation receipt"
    );
    assert!(
        subjektiv_journal::release_subject(&api, serde_json::from_value(incomplete).unwrap())
            .is_err(),
        "cancelled release identity must not rearm"
    );
    conn.execute(
        "UPDATE backend_jobs SET state='completed',result_json=?3,result_digest=?4 WHERE workspace_id=?1 AND job_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, uncertain_job, saved_result.0, saved_result.1],
    )
    .unwrap();
    // The new release below must reacquire the genuine Controller guard. It cannot
    // succeed if the failed pre-revoke operation left the body frozen.
    let release = subjektiv_journal::release_subject(&api, serde_json::from_value(json!({
        "worker":old,"binding_id":old_binding.binding_id,"operation_id":"connected-normal-release",
    })).unwrap()).unwrap();
    assert_eq!(
        release.state,
        runtime_api::SubjectReleaseState::Disconnected
    );
    assert!(!store.journal_writer_active(&old_binding).unwrap());
    let key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
    assert!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .is_none()
    );
    let stopped = api
        .runtime
        .stop_worker(&old, WorkerLifecycleRequest::default())
        .unwrap();
    assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    let removed = WorkerRemovalService::new(&api)
        .execute_target_removal(
            api.runtime.as_ref(),
            &old,
            "connected regression removes prior body",
            None,
        )
        .await
        .unwrap();
    assert_eq!(removed.status, StatusCode::OK.as_u16(), "{}", removed.body);
    assert!(api.runtime.worker(&old).is_err());
    let original_root = api
        .config
        .embedded_runtime_store_root
        .join("workers")
        .join(&old.worker_id)
        .join("session");
    assert!(
        !original_root.exists(),
        "prior Worker original Session must actually be removed before startup"
    );
    // Purge retained original archive through Runtime, not fixture filesystem deletion.
    // Journal text is independent and remains readable for new-body startup.
    if let Some(archive) = api
        .config_store
        .worker_tombstone(TEST_WORKSPACE_ID, &old)
        .unwrap()
        .and_then(|t| t.archive_id)
    {
        api.runtime
            .session_material_erase(
                &old.runtime_id,
                runtime_api::SessionMaterialEraseRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    source: runtime_api::SessionPublicSource::Archived {
                        archive_id: archive.clone(),
                    },
                    expected_session_id: old_attr.session_id.clone(),
                },
            )
            .unwrap();
        assert!(
            !api.config
                .embedded_runtime_store_root
                .join("archives/workers")
                .join(archive)
                .join("session")
                .exists()
        );
    }
    let next = connected_create(&api, &subject.id, "connected-next-body").await;
    assert_ne!(next, old);
    let next_attr = store
        .subject_session_attributions(&subject.id)
        .unwrap()
        .into_iter()
        .find(|attr| attr.worker_id == next.worker_id)
        .unwrap();
    assert_ne!(next_attr.session_id, old_attr.session_id);
    connected_wait_for_recording(&api, &store, &next_attr).await;

    // Read actual Session files produced by the real Controller; no fixture Session append,
    // startup-item injection, fake disconnect or store-only simulation is used in this test.
    use session_store::Store as _;
    let session = session_store::WorkerSessionStore::open_read_only(
        api.config
            .embedded_runtime_store_root
            .join("workers")
            .join(&next.worker_id)
            .join("session"),
    )
    .unwrap();
    let session_id = next_attr.session_id.parse().unwrap();
    let segments = session.list_segments(session_id).unwrap();
    assert_eq!(segments.len(), 1);
    let entries = session.read_all_read_only(session_id, segments[0]).unwrap();
    #[derive(serde::Deserialize)]
    struct SavedJournal {
        snapshot: serde_json::Value,
        snapshot_sha256: String,
        prompt_provenance: session_store::PromptRenderProvenance,
    }
    let startup: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, log)| {
            let session_store::LogEntry::AnnotatedSystemItem { entry, .. } = log else {
                return None;
            };
            let payload = entry
                .metadata
                .input_metadata
                .as_ref()?
                .feature_payload()?
                .decode::<SavedJournal>("journal:startup", 1)
                .ok()?;
            Some((index, payload, entry.item.history_text()))
        })
        .collect();
    assert_eq!(startup.len(), 1, "startup must be durably committed once");
    let (startup_index, payload, body) = &startup[0];
    let snapshot = &payload.snapshot;
    let digest = &payload.snapshot_sha256;
    let provenance = &payload.prompt_provenance;
    let first_input = entries
        .iter()
        .position(|entry| matches!(entry, session_store::LogEntry::AnnotatedUserInput { .. }))
        .expect("new body persisted its initial human input");
    assert!(
        *startup_index < first_input,
        "startup reference must be persisted before the first human input/model request"
    );
    assert_eq!(snapshot["subject_id"], subject.id);
    assert_eq!(
        snapshot["journals"][0]["source_session_id"], old_attr.session_id,
        "Host-selected writer predecessor must lead the snapshot"
    );
    assert!(body.contains("CONNECTED_HANDOFF"));
    assert_eq!(
        provenance.logical_name,
        "internal.subjektiv_journal_startup"
    );
    assert_eq!(
        *digest,
        Sha256::digest(serde_json::to_vec(snapshot).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let dialogs = model.dialogs.lock().unwrap();
    assert!(dialogs.len() >= 2);
    assert!(
        dialogs[1]
            .items
            .iter()
            .filter_map(Item::as_text)
            .any(|text| text.contains(body)),
        "first new-body model request must contain the same persisted startup body"
    );
    drop(dialogs);
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(
                |(path, status)| path.ends_with("/subjektiv/journal/startup")
                    && *status == StatusCode::OK
            )
            .count(),
        2,
        "both bodies must use the authenticated HTTP Host startup reader"
    );
    assert!(
        requests.iter().any(
            |(path, status)| path.ends_with("/subjektiv/sessions") && *status == StatusCode::OK
        )
    );
    drop(requests);
    let connection = connected_host_reader(&api, &next, &next_attr.session_id);
    let before = store
        .journal_resume_state(
            &JournalHostContext::validated_session(&store, next_attr.clone()).unwrap(),
            &store
                .journal_for_session(
                    &JournalHostContext::validated_session(&store, next_attr.clone()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    let (startup, compacted) = tokio::task::spawn_blocking(move || {
        (
            connection.journal_startup_context().unwrap(),
            connection.journal_compaction_context().unwrap(),
        )
    })
    .await
    .unwrap();
    assert_eq!(startup.journals[0].source_session_id, old_attr.session_id);
    assert_eq!(
        compacted.journals[0].source_session_id,
        next_attr.session_id
    );
    assert_eq!(compacted.journals[0].state_token, before.0.state_token);
    assert_eq!(compacted.journals[0].process_order, before.0.process_order);
    assert_eq!(compacted.journals[0].process_byte, before.0.process_byte);
    assert_eq!(
        store
            .journal_resume_state(
                &JournalHostContext::validated_session(&store, next_attr.clone()).unwrap(),
                &before.0.id
            )
            .unwrap(),
        before
    );
    for (worker, session) in [
        (&old, old_attr.session_id.as_str()),
        (&next, old_attr.session_id.as_str()),
        (&next, "unattributed-session"),
    ] {
        let connection = connected_host_reader(&api, worker, session);
        assert!(
            tokio::task::spawn_blocking(move || connection.journal_compaction_context())
                .await
                .unwrap()
                .is_err()
        );
    }
    server.abort();
}

#[tokio::test]
async fn signed_compaction_read_does_not_contact_runtime_or_wait_for_active_generation_and_rejects_jobs()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let identity = worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("snapshot").unwrap())
        .unwrap();
    let settings = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reserve = |key: String, operation: &str| {
        api.config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                "runtime-test",
                operation,
                &"f".repeat(64),
                Some(&key),
                &settings,
            )
            .unwrap()
            .worker_id
            .to_string()
    };
    let body = reserve(
        crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap(),
        "body",
    );
    let job = reserve(
        format!("{SUBJEKTIV_CONSOLIDATION_SINGLETON_PREFIX}{}", subject.id),
        "job",
    );
    let attr = store
        .record_session_attribution(
            crate::subjektiv::SubjectSessionAttribution::new(
                &subject.id,
                "runtime-test",
                &body,
                "committed-session",
            )
            .unwrap(),
        )
        .unwrap();
    let host = bind_fixture_session(&store, attr);
    let range = JournalSourceRange {
        source: JournalSource {
            session_id: "committed-session".into(),
            pointer: JournalSourcePointer {
                segment_id: uuid::Uuid::now_v7(),
                entry_ref: "E0".into(),
            },
            order: 0,
            content_sha256: "f".repeat(64),
            total_bytes: 10,
        },
        bytes: JournalByteRange { start: 0, end: 10 },
    };
    let id = store
        .accept_journal_sources(&host, "pending", &[range.clone()])
        .unwrap();
    let generation = store
        .prepare_journal_generation(
            &host,
            &id,
            subjektiv::api::JobAttemptBinding {
                job_id: "active-job".into(),
                attempt_id: "attempt".into(),
            },
            vec![range],
        )
        .unwrap();
    let expected = store.journal_compaction_snapshot(&host).unwrap();
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/journal/compaction");
    let app = build_router(api.clone());
    for (worker, session, status) in [
        (&body, "committed-session", StatusCode::OK),
        (&job, "committed-session", StatusCode::FORBIDDEN),
        (&body, "foreign-session", StatusCode::FORBIDDEN),
    ] {
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            app.clone().oneshot(runtime_source_request(
                &identity,
                Some(worker),
                "POST",
                &path,
                serde_json::to_vec(&JournalCompactionRequest {
                    session_id: session.into(),
                })
                .unwrap(),
            )),
        )
        .await
        .expect("snapshot must not wait for an unavailable Runtime or active generation")
        .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            let snapshot: subjektiv::journal::startup::JournalStartupSnapshot =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(snapshot, expected);
        }
    }
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(&path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&JournalCompactionRequest {
                        session_id: "committed-session".into(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(store.journal_compaction_snapshot(&host).unwrap(), expected);
    assert_eq!(
        store.active_journal_generation(&host, &id).unwrap(),
        Some(generation)
    );
}

fn connected_host_reader(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    session: &str,
) -> worker::subjektiv::SubjektivHostConnection {
    let audience = embedded_runtime_request_audience(&api.config).unwrap();
    let client = worker_runtime::worker_source::RuntimeOwnedWorkspaceClient::new(
        TEST_WORKSPACE_ID,
        api.config.backend_base_url.as_ref().unwrap(),
        &worker.runtime_id,
        &worker.worker_id,
    )
    .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience);
    let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
    config.profile.enabled = true;
    config
        .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: TEST_WORKSPACE_ID.into(),
            language: "English".into(),
        })
        .unwrap();
    worker::subjektiv::SubjektivHostConnection::new(
        worker::subjektiv::BackendSubjektivHost::from_resolved_config(&config, Arc::new(client))
            .unwrap()
            .unwrap(),
        worker::subjektiv::SubjektivHostContext {
            worker_id: worker.worker_id.clone(),
            session_id: session.into(),
        },
    )
    .unwrap()
}

// This wrapper changes timing only. Production owns reconstruction, recovery,
// activation and cleanup; the real SQLite trust binding supplies the denial.
#[derive(Default)]
struct RestoreAuthProbe {
    prepared: tokio::sync::Notify,
    release: tokio::sync::Notify,
    prepares: std::sync::atomic::AtomicUsize,
    activations: std::sync::atomic::AtomicUsize,
    publications: std::sync::atomic::AtomicUsize,
    handles: Mutex<Vec<worker::WorkerHandle>>,
    controller_tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    operations: Mutex<Vec<Value>>,
    repairs: Mutex<Vec<bool>>,
    repair_operations: Mutex<Vec<Value>>,
    gate_repair: std::sync::atomic::AtomicBool,
    repairing: tokio::sync::Notify,
    release_repair: tokio::sync::Notify,
}

struct RestoreAuthFactory {
    inner: worker_runtime::worker_backend::ProfileRuntimeWorkerFactory,
    probe: Arc<RestoreAuthProbe>,
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeWorkerFactory for RestoreAuthFactory {
    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> std::result::Result<(), String> {
        self.inner.observe_workspace_prompt_projection(projection)
    }

    async fn fetch_workspace_config(
        &self,
        request: worker_runtime::execution::WorkspaceConfigFetchRequest,
    ) -> std::result::Result<worker_runtime::execution::WorkspaceConfigFetchResult, String> {
        self.inner.fetch_workspace_config(request).await
    }

    fn retained_session_snapshot(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<
        session_store::RetainedSessionSnapshot,
        session_store::RetainedSnapshotReadError,
    > {
        self.inner.retained_session_snapshot(worker_ref)
    }

    fn retained_session_history_page(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> std::result::Result<protocol::SessionHistoryPage, session_store::RetainedHistoryReadError>
    {
        self.inner
            .retained_session_history_page(worker_ref, cursor, limit)
    }

    fn retained_session_attachment(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        session_id: &str,
        attachment_id: &str,
    ) -> std::result::Result<
        session_store::RetainedSessionAttachment,
        session_store::RetainedAttachmentReadError,
    > {
        self.inner
            .retained_session_attachment(worker_ref, session_id, attachment_id)
    }

    async fn spawn_controller(
        &self,
        mut request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        if request.request.backend_job.is_some() {
            return self.inner.spawn_controller(request).await;
        }
        // Save a no-auth model too, so a restore regression cannot reach developer
        // credentials. Only the external model stream is replaced, not Host I/O.
        let archive = worker_runtime::profile_archive::ProfileSourceArchive::build(
            worker_runtime::profile_archive::ProfileSourceArchiveInput {
                id: "profile-source-archive:restore-host-auth".into(),
                entrypoints: BTreeMap::from([("builtin:companion".into(), "profiles/body.dcdl".into())]),
                imports: BTreeMap::new(),
                sources: BTreeMap::from([("profiles/body.dcdl".into(), r#"{
                    slug = "body"; description = "Connected restore Host authority"; scope = "workspace_read";
                    model = { scheme = "anthropic"; model_id = "test-model"; auth = { kind = "none"; }; };
                    engine = { max_tokens = 100; };
                    feature = { subjektiv = { enabled = true; extraction = { enabled = false; }; }; };
                }"#.into())]),
            },
        ).map_err(|error| error.to_string())?;
        request.request.profile_source =
            worker_runtime::catalog::ProfileSourceArchiveSource::Embedded { archive };
        let controller = self.inner.spawn_controller(request).await?;
        self.probe
            .controller_tasks
            .lock()
            .unwrap()
            .push(controller.controller_task.abort_handle());
        self.probe
            .handles
            .lock()
            .unwrap()
            .push(controller.handle.clone());
        Ok(controller)
    }

    async fn preflight_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), String> {
        self.inner.preflight_restore(request).await
    }

    async fn recover_restore_source(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<bool, worker_runtime::worker_backend::RuntimeRestoreFailure> {
        self.probe
            .repair_operations
            .lock()
            .unwrap()
            .push(serde_json::to_value(request.operation_id).unwrap());
        if self
            .probe
            .gate_repair
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.probe.repairing.notify_one();
            self.probe.release_repair.notified().await;
        }
        let repaired = self.inner.recover_restore_source(request).await?;
        self.probe.repairs.lock().unwrap().push(repaired);
        Ok(repaired)
    }

    async fn prepare_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<
        Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let operation = serde_json::to_value(request.operation_id).unwrap();
        let inner = self.inner.prepare_restore(request).await?;
        self.probe.operations.lock().unwrap().push(operation);
        if self
            .probe
            .prepares
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            self.probe.prepared.notify_one();
            self.probe.release.notified().await;
        }
        Ok(Box::new(RestoreAuthCandidate {
            inner,
            probe: self.probe.clone(),
        }))
    }

    async fn cleanup_failed_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.cleanup_failed_restore(request).await
    }

    async fn reconcile_stopped_worker(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.reconcile_stopped_worker(worker_ref).await
    }

    fn activate_restored_controller(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        workspace_id: Option<&str>,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner
            .activate_restored_controller(worker_ref, workspace_id, handle)
    }

    fn deactivate_stopped_controller(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner.deactivate_stopped_controller(worker_ref, handle)
    }
}

struct RestoreAuthCandidate {
    inner: Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
    probe: Arc<RestoreAuthProbe>,
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeRestoreCandidate for RestoreAuthCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        self.inner.initial_state()
    }

    fn has_durable_effects(&self) -> bool {
        self.inner.has_durable_effects()
    }

    fn can_retry_publication(&self) -> bool {
        self.inner.can_retry_publication()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<
        worker_runtime::worker_backend::RuntimeWorkerController,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        self.probe
            .activations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let controller = self.inner.activate().await?;
        self.probe
            .publications
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.probe
            .controller_tasks
            .lock()
            .unwrap()
            .push(controller.controller_task.abort_handle());
        self.probe
            .handles
            .lock()
            .unwrap()
            .push(controller.handle.clone());
        Ok(controller)
    }

    async fn discard(
        &mut self,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.discard().await
    }
}

#[derive(Clone, Default)]
struct RestoreAuthModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    jobs: ConnectedJournalModel,
}

#[async_trait::async_trait]
impl LlmClient for RestoreAuthModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, ClientError> {
        use futures::StreamExt;
        if request.tools.iter().any(|tool| {
            matches!(
                tool.name.as_str(),
                "ReadJournalState" | "SubmitBackendJobResult" | "SubmitMemorySurface"
            )
        }) {
            return self.jobs.stream(request).await;
        }
        // Measure the saved body, not independent Journal jobs admitted by its
        // seed turn before Stop; those have a different execution owner.
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::pin(
            futures::stream::iter([
                Ok(Event::text_block_start(0)),
                Ok(Event::text_delta(
                    0,
                    "fixture turn held for durable queue setup",
                )),
            ])
            .chain(futures::stream::pending()),
        ))
    }
}

struct ConnectedRestoreServer {
    server: tokio::task::JoinHandle<()>,
    probe: Arc<RestoreAuthProbe>,
}

impl Drop for ConnectedRestoreServer {
    fn drop(&mut self) {
        // Unblock a prepared candidate even if an assertion unwinds at the gate.
        self.probe.release.notify_one();
        self.probe.release_repair.notify_one();
        self.server.abort();
        for task in self.probe.controller_tasks.lock().unwrap().iter() {
            task.abort();
        }
    }
}

async fn restore_auth_event(
    events: &mut tokio::sync::broadcast::Receiver<protocol::Event>,
    predicate: impl Fn(&protocol::Event) -> bool,
) -> protocol::Event {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("real Controller event stream");
            assert!(
                !matches!(
                    event,
                    protocol::Event::Error { .. }
                        | protocol::Event::SubmissionRejected { .. }
                        | protocol::Event::NotificationRejected { .. }
                ),
                "{event:?}"
            );
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("real Controller acknowledgement timed out")
}

// Only initial creation is substituted for the pending-head case: it writes a
// genuine first-Session Worker, but publishes no Controller/head. Every restore,
// repair, Host authorization, activation and Stop delegates to production.
struct PendingHeadSeedBackend {
    inner: Arc<worker_runtime::worker_backend::WorkerRuntimeExecutionBackend<RestoreAuthFactory>>,
    seed: std::sync::atomic::AtomicBool,
    root: std::path::PathBuf,
    model: RestoreAuthModel,
}

impl worker_runtime::execution::WorkerExecutionBackend for PendingHeadSeedBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }
    fn spawn_worker(
        &self,
        request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
        if !self.seed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return self.inner.spawn_worker(request);
        }
        let root = self
            .root
            .join("workers")
            .join(request.worker_ref.worker_id.to_string());
        let name = format!("worker-runtime-{}", request.worker_ref.worker_id);
        let model = self.model.clone();
        let settings = request.request.memory_settings.clone().unwrap();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    use session_store::WorkerMetadataStore as _;
                    let mut manifest = manifest::WorkerManifest::from_toml(&format!(
                        r#"
[worker]
name = "{name}"
pwd = "./"
[scope]
allow = []
[model]
scheme = "anthropic"
model_id = "test-model"
auth = {{ kind = "none" }}
[engine]
max_tokens = 100
[feature.subjektiv.profile]
enabled = true
[feature.subjektiv.profile.extraction]
enabled = false
"#
                    ))
                    .unwrap();
                    manifest
                        .feature
                        .subjektiv
                        .bind_workspace_settings(settings)
                        .unwrap();
                    let sessions =
                        session_store::WorkerSessionStore::new(root.join("session")).unwrap();
                    let metadata = session_store::WorkerAggregateStore::new(&root, &name).unwrap();
                    let engine = agen::Engine::<
                        _,
                        agen::state::Mutable,
                        worker::SessionHistoryMetadata,
                    >::new_annotated(model);
                    let mut head = worker::Worker::new(
                        manifest.clone(),
                        engine,
                        session_store::CombinedStore::new(sessions.clone(), metadata.clone()),
                        worker::WorkerWorkspaceContext::unavailable(
                            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                            "initial creation interrupted before first head",
                        ),
                        worker::WorkerFilesystemAuthority::None,
                        manifest::Scope::empty(),
                    )
                    .await
                    .unwrap();
                    head.enable_worker_metadata_write_through().unwrap();
                    let mut saved = metadata.read_by_name(&name).unwrap().unwrap();
                    assert!(saved.active.as_ref().unwrap().segment_id.is_none());
                    saved.resolved_manifest_snapshot = Some(
                        manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap(),
                    );
                    metadata.write(&saved).unwrap();
                    drop(head);
                });
        })
        .join()
        .unwrap();
        worker_runtime::execution::WorkerExecutionSpawnResult::Connected {
            worker_state: protocol::WorkerStateSnapshot::initial(),
            workdir_attachments: Vec::new(),
        }
    }
    fn fetch_workspace_config(
        &self,
        request: worker_runtime::execution::WorkspaceConfigFetchRequest,
    ) -> std::result::Result<worker_runtime::execution::WorkspaceConfigFetchResult, String> {
        self.inner.fetch_workspace_config(request)
    }
    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> std::result::Result<(), String> {
        self.inner.observe_workspace_prompt_projection(projection)
    }
    fn preflight_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), worker_runtime::execution::WorkerExecutionResult> {
        self.inner.preflight_restore(request)
    }
    fn completed_restore_failure(
        &self,
        id: worker_runtime::execution::WorkerLifecycleOperationId,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> Option<worker_runtime::execution::WorkerExecutionSpawnResult> {
        self.inner.completed_restore_failure(id, worker)
    }
    fn restore_worker(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
        self.inner.restore_worker(request)
    }
    fn restore_recovery_disposition(
        &self,
        id: worker_runtime::execution::WorkerLifecycleOperationId,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> Option<worker_runtime::execution::RestoreRecoveryDisposition> {
        self.inner.restore_recovery_disposition(id, worker)
    }
    // This caller-replay fixture gates repair/prepare to inspect the durable
    // lost-response state before its next explicit call. Runtime continuation
    // itself is covered with the unwrapped backend in the Runtime suite.
    fn supports_restore_continuation(&self) -> bool {
        false
    }
    fn reclaim_uncommitted_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
        self.inner.reclaim_uncommitted_restore(request)
    }
    fn discard_uncommitted_restore(
        &self,
        id: worker_runtime::execution::WorkerLifecycleOperationId,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
        self.inner.discard_uncommitted_restore(id, worker)
    }
    fn reconcile_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
        self.inner.reconcile_restore(request)
    }
    fn activate_restored_worker(
        &self,
        id: worker_runtime::execution::WorkerLifecycleOperationId,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.activate_restored_worker(id, worker)
    }
    fn dispatch_input(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        input: worker_runtime::interaction::WorkerInput,
    ) -> worker_runtime::execution::WorkerExecutionResult {
        self.inner.dispatch_input(worker, input)
    }
    fn dispatch_method(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        method: protocol::Method,
    ) -> worker_runtime::execution::WorkerExecutionResult {
        self.inner.dispatch_method(worker, method)
    }
    fn stop_worker_operation(
        &self,
        request: worker_runtime::execution::WorkerExecutionStopRequest,
    ) -> worker_runtime::execution::WorkerExecutionResult {
        self.inner.stop_worker_operation(request)
    }
    fn worker_snapshot(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> Option<protocol::Event> {
        self.inner.worker_snapshot(worker)
    }
}

// One real connected seed and stopped queue setup shared by the publication
// denial and explicit-repair regressions. Only timing and response delivery are
// intercepted: every callback still passes the production signer/router/store.
struct ConnectedRestoreFixture {
    api: WorkspaceApi,
    execution:
        Arc<worker_runtime::worker_backend::WorkerRuntimeExecutionBackend<RestoreAuthFactory>>,
    probe: Arc<RestoreAuthProbe>,
    model: RestoreAuthModel,
    worker: RuntimeWorkerRef,
    subject_id: String,
    requests: Arc<Mutex<Vec<(String, StatusCode)>>>,
    callbacks: Arc<Mutex<Vec<Value>>>,
    lose_response: Arc<std::sync::atomic::AtomicBool>,
    _server: ConnectedRestoreServer,
    _dir: tempfile::TempDir,
}

async fn connected_restore_fixture() -> ConnectedRestoreFixture {
    connected_restore_fixture_with_pending_head(false).await
}

async fn connected_restore_fixture_with_pending_head(
    pending_head: bool,
) -> ConnectedRestoreFixture {
    use std::sync::atomic::Ordering::SeqCst;

    // Guard the resolved global paths before factory/config/allocation access.
    // No Git or Workdir attachment is needed for this connected regression.
    let dir = connected_restore_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = test_server_config(dir.path());
    config.repositories.clear();
    config.backend_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
    let control = Arc::new(test_control_store(&config));
    seed_test_registered_workspace(control.as_ref(), &config)
        .await
        .unwrap();
    let broker = BackendResourceBroker::default();
    let dispatcher = Arc::new(
        crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
            config.clone(),
            control.clone(),
        ),
    );
    let model = RestoreAuthModel::default();
    let model_factory = model.clone();
    let audience = embedded_runtime_request_audience(&config).unwrap();
    let probe = Arc::new(RestoreAuthProbe::default());
    let factory = worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
        config.workspace_execution_root.clone(),
    )
    .with_embedded_worker_mutation_dispatcher(EMBEDDED_WORKER_RUNTIME_ID, dispatcher.clone())
    .with_workspace_request_client(
        worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
            TEST_WORKSPACE_ID,
            audience.clone(),
            EMBEDDED_WORKER_RUNTIME_ID,
        )
        .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience),
    )
    .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
    .with_resource_client(Arc::new(broker.clone()))
    .with_spawn_model_client_factory(move || Box::new(model_factory.clone()));
    let execution = Arc::new(
        worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(RestoreAuthFactory {
            inner: factory,
            probe: probe.clone(),
        })
        .unwrap(),
    );
    let seeded_execution = Arc::new(PendingHeadSeedBackend {
        inner: execution.clone(),
        seed: std::sync::atomic::AtomicBool::new(pending_head),
        root: config.embedded_runtime_store_root.clone(),
        model: model.clone(),
    });
    let api = WorkspaceApi::new_with_execution_backend_and_broker(
        config,
        control,
        seeded_execution,
        broker,
        HostExecutors::default(),
        Some(dispatcher),
    )
    .await
    .unwrap();
    seed_test_delivery_binding(&api);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let callbacks = Arc::new(Mutex::new(Vec::new()));
    let recorded = callbacks.clone();
    let lose_response = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lose_once = lose_response.clone();
    let app = build_router(api.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            let recorded = recorded.clone();
            let lose_once = lose_once.clone();
            async move {
                let path = request.uri().path().to_owned();
                let is_session = path.ends_with("/subjektiv/sessions");
                let request = if is_session {
                    let (parts, body) = request.into_parts();
                    let bytes = to_bytes(body, usize::MAX).await.unwrap();
                    recorded
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(&bytes).unwrap());
                    axum::http::Request::from_parts(parts, Body::from(bytes))
                } else {
                    request
                };
                let response = next.run(request).await;
                observed.lock().unwrap().push((path, response.status()));
                if is_session
                    && response.status() == StatusCode::OK
                    && lose_once.swap(false, SeqCst)
                {
                    // The real handler has already persisted its immutable SQLite
                    // attribution. Lose only its acknowledgement, never fake a Host.
                    axum::http::Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(Body::from("fixture lost committed Host response"))
                        .unwrap()
                } else {
                    response
                }
            }
        },
    ));
    let server = ConnectedRestoreServer {
        server: tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
        probe: probe.clone(),
    };
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("restore-host-auth").unwrap())
        .unwrap();
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "restore-host-auth-owner").await;
    let update = server_api::authorization::SubjectPermissionsUpdate {
        operation_id: "restore-host-auth-permissions".into(),
        expected_change_id: None,
        execution: [
            "task",
            "subjektiv",
            "web",
            "image",
            "sub_worker",
            "worker",
            "workspace_worker_discovery",
            "objective",
            "manage_workdir",
            "workdir_catalog",
            "drive",
            "ticket",
        ]
        .into_iter()
        .map(|name| server_api::authorization::ScopedPermission {
            operation: name.into(),
            resource: server_api::authorization::PermissionResource::Feature,
        })
        .collect(),
        delegation: vec![],
    };
    let response = build_router(api.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions",
                    subject.id
                ))
                .header("authorization", format!("Bearer {owner}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&update).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let Json(created) = scoped_create_workspace_worker(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "restore-auth-body".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.id.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    if pending_head {
        assert_eq!(
            api.runtime
                .stop_worker(&worker, WorkerLifecycleRequest::default())
                .unwrap()
                .state,
            InternalWorkerOperationState::Accepted
        );
    } else {
        let handle = probe.handles.lock().unwrap()[0].clone();
        let mut events = handle.subscribe();
        handle
            .send(protocol::Method::submit_text(
                "setup-running",
                "hold for queue setup",
            ))
            .await
            .unwrap();
        restore_auth_event(&mut events, |event| {
            matches!(event, protocol::Event::TextDelta { .. })
        })
        .await;
        let command = protocol::WorkerCommandEnvelope::new(
            handle.shared_state.snapshot().last_command_id + 1,
        );
        handle
            .send(protocol::Method::Pause { command })
            .await
            .unwrap();
        restore_auth_event(&mut events, |event| {
            matches!(event, protocol::Event::WorkerState { snapshot }
        if snapshot.catalog_status() == protocol::WorkerStatus::Paused)
        })
        .await;
        handle
            .send(protocol::Method::submit_text(
                "saved-submit",
                "must remain queued",
            ))
            .await
            .unwrap();
        restore_auth_event(&mut events, |event| {
            matches!(event, protocol::Event::SubmissionAccepted {
        submission_request_id, disposition: protocol::SubmissionDisposition::Queued, ..
    } if submission_request_id == "saved-submit")
        })
        .await;
        handle
            .send(protocol::Method::Notify {
                notification_request_id: "saved-notice".into(),
                message: "must remain queued too".into(),
            })
            .await
            .unwrap();
        restore_auth_event(&mut events, |event| {
            matches!(event, protocol::Event::NotificationAccepted {
        notification_request_id
    } if notification_request_id == "saved-notice")
        })
        .await;
        let stopped = api
            .runtime
            .stop_worker(&worker, WorkerLifecycleRequest::default())
            .unwrap();
        assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
        assert!(handle.protocol_is_closed());
        // Pause/Shutdown can admit real Journal work for the seed turn. Finish that
        // owned work before measuring the restore window; it is not restore I/O.
        let attr = store
            .subject_session_attributions(&subject.id)
            .unwrap()
            .into_iter()
            .find(|attr| attr.worker_id == worker.worker_id)
            .unwrap();
        let host = JournalHostContext::validated_session(&store, attr.clone()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(25));
            loop {
                ticks.tick().await;
                let jobs = api
                    .store
                    .list_backend_journal_jobs(TEST_WORKSPACE_ID, &attr.session_id, None, 100)
                    .unwrap();
                let closed = store
                    .journal_for_session(&host)
                    .unwrap()
                    .is_none_or(|journal| {
                        store
                            .active_journal_generation(&host, &journal)
                            .unwrap()
                            .is_none()
                    });
                if closed
                    && !store.journal_retention_required(&host).unwrap()
                    && jobs
                        .iter()
                        .all(|job| job.state == BackendJobState::Completed)
                {
                    break;
                }
            }
        })
        .await
        .expect("seed Journal work must settle before measuring restore");
    }

    ConnectedRestoreFixture {
        api,
        execution,
        probe,
        model,
        worker,
        subject_id: subject.id,
        requests,
        callbacks,
        lose_response,
        _server: server,
        _dir: dir,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn restore_after_real_prepare_rejects_revoked_host_trust_without_publication_or_queue_consumption()
 {
    use session_store::WorkerMetadataStore as _;
    use std::sync::atomic::Ordering::SeqCst;
    use worker_runtime::execution::WorkerExecutionBackend as _;

    let fixture = connected_restore_fixture().await;
    let api = &fixture.api;
    let execution = &fixture.execution;
    let probe = &fixture.probe;
    let model = &fixture.model;
    let worker = fixture.worker.clone();
    let requests = &fixture.requests;
    let store = open_subjektiv_store(api).unwrap();
    let subject_id = &fixture.subject_id;
    let aggregate = api
        .config
        .embedded_runtime_store_root
        .join("workers")
        .join(&worker.worker_id);
    let name = format!("worker-runtime-{}", worker.worker_id);
    let metadata = session_store::WorkerAggregateStore::open_read_only(&aggregate, &name).unwrap();
    let saved = metadata.read_by_name(&name).unwrap().unwrap();
    assert!(matches!(
        saved.subjektiv_session_attribution,
        Some(session_store::SubjektivSessionAttributionState::Confirmed { .. })
    ));
    let active = saved.active.as_ref().unwrap();
    let session =
        session_store::WorkerSessionStore::open_read_only(aggregate.join("session")).unwrap();
    let entries = || {
        session
            .read_all_read_only(active.session_id, active.segment_id.unwrap())
            .unwrap()
    };
    let before = serde_json::to_value(entries()).unwrap();
    let checkpoint = entries()
        .into_iter()
        .rev()
        .find_map(|entry| match entry {
            session_store::LogEntry::Extension {
                domain, payload, ..
            } if domain == "worker.pending_activations.v1" => Some(payload),
            _ => None,
        })
        .expect("real Controller persisted pending activation checkpoint");
    assert_eq!(
        checkpoint["pending"][0]["submission_request_id"],
        "saved-submit"
    );
    assert_eq!(
        checkpoint["pending_notifications"][0]["notification_request_id"],
        "saved-notice"
    );
    let attributions = store.subject_session_attributions(subject_id).unwrap();
    let key = crate::subjektiv::subject_worker_singleton_key(subject_id).unwrap();
    let singleton = api
        .store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
        .unwrap();
    let model_before = model.calls.load(SeqCst);
    let request = runtime_api::WorkerRestoreRequest {
        expected_observation_token: api
            .runtime
            .worker(&worker)
            .unwrap()
            .restore_observation_token
            .unwrap(),
        request_id: "restore-revoked-after-real-prepare".into(),
        preparation: Some(runtime_api::WorkerRestorePreparation::default()),
    };
    assert!(
        requests.lock().unwrap().iter().any(
            |(path, status)| path.ends_with("/subjektiv/sessions") && *status == StatusCode::OK
        ),
        "the same production signer must be accepted before trust revocation"
    );
    requests.lock().unwrap().clear();
    fixture.callbacks.lock().unwrap().clear();
    let runtime = api.runtime.clone();
    let target = worker.clone();
    let intent = request.clone();
    let restoring = tokio::task::spawn_blocking(move || runtime.restore_worker(&target, intent));
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        probe.prepared.notified(),
    )
    .await
    .expect("real prepare must complete before revocation");
    assert_eq!(probe.activations.load(SeqCst), 0);
    assert_eq!(model.calls.load(SeqCst), model_before);
    assert_eq!(
        serde_json::to_value(entries()).unwrap(),
        before,
        "preparation must not consume saved input or append history"
    );
    assert_eq!(metadata.read_by_name(&name).unwrap().unwrap(), saved);
    {
        let callbacks = fixture.callbacks.lock().unwrap();
        assert_eq!(
            callbacks.len(),
            1,
            "prepare verifies current Host authority"
        );
        assert_eq!(callbacks[0]["create_if_missing"], false);
        assert_eq!(callbacks[0]["session_id"], active.session_id.to_string());
    }
    assert_eq!(probe.publications.load(SeqCst), 0);
    assert_eq!(
        store.subject_session_attributions(subject_id).unwrap(),
        attributions
    );
    let prepared_record: Value =
        serde_json::from_slice(&std::fs::read(aggregate.join("worker.json")).unwrap()).unwrap();
    let operation = prepared_record["restore_guard"]["owners"]["restore-revoked-after-real-prepare"]["operation_id"].clone();
    assert!(
        !operation.is_null(),
        "admission must persist the operation owner before prepare"
    );
    assert!(
        api.config_store
            .revoke_workspace_runtime_binding(
                TEST_WORKSPACE_ID,
                EMBEDDED_WORKER_RUNTIME_ID,
                &now_registry_timestamp(),
            )
            .unwrap()
    );
    probe.release.notify_one();
    let first = tokio::time::timeout(std::time::Duration::from_secs(20), restoring)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{first:?}"
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, status)| path.ends_with("/subjektiv/sessions")
                && *status == StatusCode::UNAUTHORIZED),
        "actual signed Host publication must see revoked persisted trust"
    );
    assert_eq!(probe.prepares.load(SeqCst), 1);
    assert_eq!(probe.activations.load(SeqCst), 1);
    let runtime = api.runtime.clone();
    let target = worker.clone();
    let replay = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::task::spawn_blocking(move || runtime.restore_worker(&target, request)),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        replay.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{replay:?}"
    );
    // Reconciliation may reconstruct after confirmed cleanup, but must never
    // invent a new operation or repair an already confirmed attribution.
    assert!(
        probe
            .operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| *id == operation)
    );
    assert!(
        probe
            .repairs
            .lock()
            .unwrap()
            .iter()
            .all(|repaired| !repaired)
    );
    let worker_ref = worker_runtime::identity::WorkerRef::try_from(&worker).unwrap();
    assert!(
        execution.worker_snapshot(&worker_ref).is_none(),
        "no live Controller may be published"
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|(path, _)| !path.ends_with("/authorization/check")),
        "restoring saved Features must not re-admit them"
    );
    assert_eq!(probe.publications.load(SeqCst), 0);
    assert_eq!(
        probe.handles.lock().unwrap().len(),
        1,
        "only stopped seed Controller existed"
    );
    assert_eq!(model.calls.load(SeqCst), model_before);
    assert_eq!(
        serde_json::to_value(entries()).unwrap(),
        before,
        "Host denial must not consume either saved queue or execute Tools"
    );
    assert_eq!(
        metadata
            .read_by_name(&name)
            .unwrap()
            .unwrap()
            .subjektiv_session_attribution,
        saved.subjektiv_session_attribution
    );
    assert_eq!(
        store.subject_session_attributions(subject_id).unwrap(),
        attributions
    );
    assert_eq!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap(),
        singleton
    );
    // Activation may already reconcile metadata/pins/allocation. Denial is not a
    // fictitious rollback/Stopped: the same operation must retain its owner.
    let record: Value =
        serde_json::from_slice(&std::fs::read(aggregate.join("worker.json")).unwrap()).unwrap();
    assert_eq!(
        record["restore_guard"]["active_request_id"],
        "restore-revoked-after-real-prepare"
    );
    let owner = &record["restore_guard"]["owners"]["restore-revoked-after-real-prepare"];
    assert_eq!(
        owner["operation_id"], operation,
        "the durable operation owner must survive denial and replay"
    );
    assert_eq!(
        record["execution_state"]["state"],
        "reconciliation_required"
    );
    assert_eq!(
        record["execution_state"]["execution"]["operation"],
        "restore"
    );
    assert_eq!(
        record["execution_state"]["execution"]["intent"]["operation_id"],
        operation
    );
    assert!(
        owner["receipt"].is_null(),
        "no saved Accepted receipt may escape on replay"
    );
    // Only this fixture's retained operation is cleaned up, after its durable
    // evidence has been checked. Cleanup is not the restore outcome above.
    let cleanup = execution.stop_worker(&worker_ref);
    assert!(
        cleanup.is_accepted(),
        "fixture restore cleanup: {cleanup:?}"
    );
    shutdown_workspace_feature_storage(&api).await.unwrap();
}

#[derive(Clone, Copy)]
enum ConnectedRepair {
    IncompleteAttribution,
    PendingHead,
    RevokedAttribution,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_repair_existing_attribution_replays_lost_real_host_response_once() {
    connected_explicit_repair(ConnectedRepair::IncompleteAttribution).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_repair_pending_head_creates_one_session_after_lost_real_host_response() {
    connected_explicit_repair(ConnectedRepair::PendingHead).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_repair_replay_rejects_revoked_real_host_trust_without_false_accepted() {
    connected_explicit_repair(ConnectedRepair::RevokedAttribution).await;
}

async fn connected_explicit_repair(scenario: ConnectedRepair) {
    use session_store::{Store as _, WorkerMetadataStore as _};
    use std::sync::atomic::Ordering::SeqCst;
    use worker_runtime::execution::WorkerExecutionBackend as _;

    let pending = matches!(scenario, ConnectedRepair::PendingHead);
    let fixture = connected_restore_fixture_with_pending_head(pending).await;
    let api = &fixture.api;
    let probe = &fixture.probe;
    let worker = &fixture.worker;
    let revoked = matches!(scenario, ConnectedRepair::RevokedAttribution);
    let aggregate = api
        .config
        .embedded_runtime_store_root
        .join("workers")
        .join(&worker.worker_id);
    let name = format!("worker-runtime-{}", worker.worker_id);
    let metadata = session_store::WorkerAggregateStore::new(&aggregate, &name).unwrap();
    let mut saved = metadata.read_by_name(&name).unwrap().unwrap();
    let original_active = saved.active.clone().unwrap();
    // The genuine pending fixture has no Session manifest yet. Open its empty
    // store before measuring repair; after publication always reread authority
    // rather than the original Store's cached empty lineage.
    let pending_store =
        pending.then(|| session_store::WorkerSessionStore::new(aggregate.join("session")).unwrap());
    let sessions =
        || match session_store::WorkerSessionStore::open_read_only(aggregate.join("session")) {
            Ok(store) => store,
            Err(session_store::StoreError::Io(error))
                if pending && error.kind() == std::io::ErrorKind::NotFound =>
            {
                pending_store.as_ref().unwrap().clone()
            }
            Err(error) => panic!("read repaired Session: {error}"),
        };
    let original_entries = match original_active.segment_id {
        Some(segment_id) => serde_json::to_value(
            sessions()
                .read_all_read_only(original_active.session_id, segment_id)
                .unwrap(),
        )
        .unwrap(),
        None => {
            assert!(pending);
            assert!(
                sessions()
                    .list_segments(original_active.session_id)
                    .unwrap()
                    .is_empty()
            );
            serde_json::json!([])
        }
    };
    let store = open_subjektiv_store(api).unwrap();
    let initial_attributions = store
        .subject_session_attributions(&fixture.subject_id)
        .unwrap();
    let singleton_key =
        crate::subjektiv::subject_worker_singleton_key(&fixture.subject_id).unwrap();
    let singleton = api
        .store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &singleton_key)
        .unwrap();

    let session_id = saved.active.as_ref().unwrap().session_id;
    assert_eq!(
        initial_attributions
            .iter()
            .filter(|attr| attr.session_id == session_id.to_string())
            .count(),
        usize::from(!pending),
        "existing attribution repair is not first creation; pending-head repair is"
    );
    // Deliberate initial interruption fixture, before admitting any restore.
    // Subsequent transitions, including OutcomeUnknown, are production writes.
    saved.subjektiv_session_attribution = Some(
        session_store::SubjektivSessionAttributionState::OutcomeUnknown {
            session_id,
            diagnostic: "fixture interrupted Session attribution acknowledgement".into(),
        },
    );
    metadata.write(&saved).unwrap();
    let model_before = fixture.model.calls.load(SeqCst);
    fixture.requests.lock().unwrap().clear();
    fixture.callbacks.lock().unwrap().clear();
    fixture.lose_response.store(true, SeqCst);
    let request = runtime_api::WorkerRestoreRequest {
        expected_observation_token: api
            .runtime
            .worker(worker)
            .unwrap()
            .restore_observation_token
            .unwrap(),
        request_id: "connected-explicit-repair".into(),
        preparation: Some(runtime_api::WorkerRestorePreparation::default()),
    };
    let runtime = api.runtime.clone();
    let target = worker.clone();
    let intent = request.clone();
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::task::spawn_blocking(move || runtime.restore_worker(&target, intent)),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{first:?}"
    );
    assert!(
        !fixture.lose_response.load(SeqCst),
        "lose acknowledgement only after real Host OK"
    );
    assert_eq!(
        probe.prepares.load(SeqCst),
        0,
        "repair precedes candidate construction"
    );
    assert_eq!(probe.activations.load(SeqCst), 0);
    assert_eq!(probe.publications.load(SeqCst), 0);
    assert_eq!(fixture.model.calls.load(SeqCst), model_before);
    let failed = metadata.read_by_name(&name).unwrap().unwrap();
    assert_eq!(
        failed.active, saved.active,
        "response loss cannot publish a first head"
    );
    assert!(matches!(failed.subjektiv_session_attribution,
        Some(session_store::SubjektivSessionAttributionState::OutcomeUnknown { session_id: id, .. }) if id == session_id));
    let committed_attributions = store
        .subject_session_attributions(&fixture.subject_id)
        .unwrap();
    assert_eq!(
        committed_attributions.len(),
        initial_attributions.len() + usize::from(pending)
    );
    assert_eq!(
        committed_attributions
            .iter()
            .filter(|attr| attr.session_id == session_id.to_string()
                && attr.worker_id == worker.worker_id)
            .count(),
        1,
        "actual SQLite attribution persisted before response loss"
    );
    if !pending {
        assert_eq!(
            committed_attributions, initial_attributions,
            "reconcile the existing immutable row"
        );
    }
    assert_eq!(fixture.callbacks.lock().unwrap().len(), 1);
    {
        let callbacks = fixture.callbacks.lock().unwrap();
        assert_eq!(callbacks[0]["session_id"], session_id.to_string());
        assert_eq!(callbacks[0]["create_if_missing"], true);
    }
    let record = || -> Value {
        serde_json::from_slice(&std::fs::read(aggregate.join("worker.json")).unwrap()).unwrap()
    };
    let first_record = record();
    let operation =
        first_record["restore_guard"]["owners"]["connected-explicit-repair"]["operation_id"]
            .clone();
    assert!(!operation.is_null());
    assert!(
        first_record["restore_guard"]["owners"]["connected-explicit-repair"]["receipt"].is_null()
    );
    assert_eq!(
        first_record["restore_guard"]["active_request_id"],
        "connected-explicit-repair"
    );
    assert_eq!(
        first_record["execution_state"]["state"],
        "reconciliation_required"
    );
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(
                |(path, status)| path.ends_with("/subjektiv/sessions") && *status == StatusCode::OK
            )
    );
    assert!(
        !fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path.ends_with("/subjektiv/journal/startup")),
        "no startup before attribution acknowledgement"
    );
    if pending {
        assert!(sessions().list_segments(session_id).unwrap().is_empty());
    }

    // Re-enter the same public Runtime intent and durable operation. The repair
    // gate changes timing only; trust revocation is the real persisted binding.
    if revoked {
        probe.gate_repair.store(true, SeqCst);
    }
    let mut repaired_seed = None;
    let runtime = api.runtime.clone();
    let target = worker.clone();
    let intent = request.clone();
    let retry = tokio::task::spawn_blocking(move || runtime.restore_worker(&target, intent));
    if revoked {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            probe.repairing.notified(),
        )
        .await
        .unwrap();
        assert!(
            api.config_store
                .revoke_workspace_runtime_binding(
                    TEST_WORKSPACE_ID,
                    EMBEDDED_WORKER_RUNTIME_ID,
                    &now_registry_timestamp(),
                )
                .unwrap()
        );
        probe.release_repair.notify_one();
    } else {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            probe.prepared.notified(),
        )
        .await
        .unwrap();
        assert_eq!(probe.activations.load(SeqCst), 0);
        assert_eq!(fixture.model.calls.load(SeqCst), model_before);
        assert_eq!(*probe.repairs.lock().unwrap(), vec![true]);
        let repaired = metadata.read_by_name(&name).unwrap().unwrap();
        assert_eq!(repaired.active.as_ref().unwrap().session_id, session_id);
        assert!(matches!(repaired.subjektiv_session_attribution,
            Some(session_store::SubjektivSessionAttributionState::Confirmed { session_id: id, ref subject_id })
            if id == session_id && subject_id == &fixture.subject_id));
        assert_eq!(
            sessions().list_segments(session_id).unwrap().len(),
            1,
            "exactly one durable seed"
        );
        let segment_id = repaired.active.unwrap().segment_id.unwrap();
        let entries = sessions()
            .read_all_read_only(session_id, segment_id)
            .unwrap();
        let seeds = entries
            .iter()
            .filter(|entry| matches!(entry, session_store::LogEntry::AnnotatedSegmentStart { .. }))
            .collect::<Vec<_>>();
        assert_eq!(seeds.len(), 1);
        repaired_seed = Some((segment_id, serde_json::to_value(seeds[0]).unwrap()));
        if !pending {
            assert_eq!(
                serde_json::to_value(entries).unwrap(),
                original_entries,
                "attribution-only repair and normal prepare cannot reseed or consume saved input"
            );
        }
        assert_eq!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(path, _)| path.ends_with("/subjektiv/journal/startup"))
                .count(),
            usize::from(pending)
        );
        probe.release.notify_one();
    }
    let replay = tokio::time::timeout(std::time::Duration::from_secs(20), retry)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if revoked {
        assert_eq!(
            replay.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "{replay:?}"
        );
        assert!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|(path, status)| path.ends_with("/subjektiv/sessions")
                    && *status == StatusCode::UNAUTHORIZED)
        );
        assert_eq!(probe.prepares.load(SeqCst), 0);
        assert_eq!(probe.activations.load(SeqCst), 0);
        assert_eq!(probe.publications.load(SeqCst), 0);
        assert_eq!(fixture.model.calls.load(SeqCst), model_before);
        assert!(
            matches!(metadata.read_by_name(&name).unwrap().unwrap().subjektiv_session_attribution,
            Some(session_store::SubjektivSessionAttributionState::Failed { session_id: id, .. }) if id == session_id)
        );
        let denied = record();
        assert_eq!(
            denied["restore_guard"]["active_request_id"],
            "connected-explicit-repair"
        );
        assert_eq!(
            denied["restore_guard"]["owners"]["connected-explicit-repair"]["operation_id"],
            operation
        );
        assert!(
            denied["restore_guard"]["owners"]["connected-explicit-repair"]["receipt"].is_null()
        );
        assert_eq!(
            denied["execution_state"]["state"],
            "reconciliation_required"
        );
        assert_eq!(
            serde_json::to_value(
                sessions()
                    .read_all_read_only(
                        original_active.session_id,
                        original_active.segment_id.unwrap(),
                    )
                    .unwrap()
            )
            .unwrap(),
            original_entries
        );
    } else {
        assert_eq!(
            replay.state,
            server_api::WorkerRestoreState::Accepted,
            "{replay:?}"
        );
        let callbacks_after = fixture.callbacks.lock().unwrap().len();
        let repairs_after = probe.repair_operations.lock().unwrap().len();
        let runtime = api.runtime.clone();
        let target = worker.clone();
        let accepted = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::task::spawn_blocking(move || runtime.restore_worker(&target, request)),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            accepted.state,
            server_api::WorkerRestoreState::Accepted,
            "{accepted:?}"
        );
        assert_eq!(probe.prepares.load(SeqCst), 1);
        assert_eq!(probe.publications.load(SeqCst), 1);
        assert_eq!(probe.repair_operations.lock().unwrap().len(), repairs_after);
        assert_eq!(
            fixture.callbacks.lock().unwrap().len(),
            callbacks_after,
            "receipt replay performs no Host callback"
        );
        assert_eq!(sessions().list_segments(session_id).unwrap().len(), 1);
        let repaired = metadata.read_by_name(&name).unwrap().unwrap();
        let active = repaired.active.unwrap();
        let (segment_id, seed) = repaired_seed.unwrap();
        assert_eq!(active.session_id, session_id);
        assert_eq!(active.segment_id, Some(segment_id));
        let entries = sessions()
            .read_all_read_only(session_id, segment_id)
            .unwrap();
        let seeds = entries
            .iter()
            .filter(|entry| matches!(entry, session_store::LogEntry::AnnotatedSegmentStart { .. }))
            .collect::<Vec<_>>();
        assert_eq!(seeds.len(), 1);
        assert_eq!(
            serde_json::to_value(seeds[0]).unwrap(),
            seed,
            "activation and receipt replay preserve the exact repaired seed"
        );
    }
    if pending {
        assert_eq!(
            fixture.model.calls.load(SeqCst),
            model_before,
            "pending first-head repair has no body input to execute"
        );
        assert_eq!(
            session_id, original_active.session_id,
            "first head belongs to the originally pending Session"
        );
        assert!(original_active.segment_id.is_none());
    }
    assert_eq!(
        store
            .subject_session_attributions(&fixture.subject_id)
            .unwrap(),
        committed_attributions
    );
    assert_eq!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &singleton_key)
            .unwrap(),
        singleton
    );
    assert!(
        probe
            .repair_operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| *id == operation)
    );
    assert!(
        probe
            .operations
            .lock()
            .unwrap()
            .iter()
            .all(|id| *id == operation)
    );
    assert_eq!(
        probe.repair_operations.lock().unwrap().len(),
        2,
        "one failed repair and one same-operation replay"
    );
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(path, _)| !path.ends_with("/authorization/check")),
        "saved Features must not be re-admitted"
    );
    {
        let callbacks = fixture.callbacks.lock().unwrap();
        assert!(
            callbacks
                .iter()
                .all(|callback| callback["session_id"] == session_id.to_string())
        );
        assert_eq!(
            callbacks
                .iter()
                .filter(|callback| callback["create_if_missing"] == true)
                .count(),
            2,
            "repair retries the Session-keyed receipt; publication only verifies it"
        );
    }
    let worker_ref = worker_runtime::identity::WorkerRef::try_from(worker).unwrap();
    if revoked {
        assert!(fixture.execution.worker_snapshot(&worker_ref).is_none());
        assert!(
            fixture.execution.stop_worker(&worker_ref).is_accepted(),
            "fixture-owned repair resource cleanup"
        );
    } else {
        assert!(fixture.execution.worker_snapshot(&worker_ref).is_some());
        // Teardown is outside the measured restore window: Stop may admit an
        // independent Journal job, not a second execution of the restored body.
        assert_eq!(
            api.runtime
                .stop_worker(worker, WorkerLifecycleRequest::default())
                .unwrap()
                .state,
            InternalWorkerOperationState::Accepted
        );
    }
    shutdown_workspace_feature_storage(api).await.unwrap();
}
