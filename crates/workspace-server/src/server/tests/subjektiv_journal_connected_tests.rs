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

#[derive(Clone, Default)]
struct ConnectedJournalModel {
    dialogs: Arc<Mutex<Vec<Request>>>,
}

impl ConnectedJournalModel {
    fn journal_response(request: &Request) -> Vec<Event> {
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
                    "covered_ranges":state["value"]["selected_ranges"],
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
            // Deliver the actual frozen Runtime source through authenticated Job tools, rather
            // than injecting a fixture Journal or claiming unseen source was processed.
            for (index, range) in state["selected_ranges"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let mut offset = range["bytes"]["start"].as_u64().unwrap();
                let mut token = Value::Null;
                loop {
                    let id = format!("source-{index}-{offset}");
                    if let Some(page) = result(&id) {
                        let page = &page["value"];
                        if let Some(next) = page["next_byte"].as_u64() {
                            assert!(next > offset);
                            offset = next;
                            token = page["content_token"].clone();
                        } else {
                            break;
                        }
                    } else {
                        assert!(
                            request
                                .tools
                                .iter()
                                .any(|tool| tool.name == "ReadJournalText")
                        );
                        return vec![Event::tool_use_start(0, &id, "ReadJournalText"), Event::tool_input_delta(0, json!({
                            "target":{"target":"source","pointer":range["source"]["pointer"]},
                            "offset":offset,"limit":MAX_JOURNAL_READ_BYTES,"content_token":token,
                        }).to_string()), Event::tool_use_stop(0)];
                    }
                }
            }
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
                    "action":action,"covered_ranges":state["selected_ranges"],
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
            }, control_operation_id: None,
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
    let dir = tempfile::tempdir().unwrap();
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
    let startup: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, log)| match log {
            session_store::LogEntry::AnnotatedSystemItem { entry, .. } => match &entry.item {
                session_store::SystemItem::JournalStartup {
                    snapshot,
                    snapshot_sha256,
                    body,
                    prompt_provenance,
                } => Some((index, snapshot, snapshot_sha256, body, prompt_provenance)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(startup.len(), 1, "startup must be durably committed once");
    let (startup_index, snapshot, digest, body, provenance) = startup[0];
    let first_input = entries
        .iter()
        .position(|entry| matches!(entry, session_store::LogEntry::AnnotatedUserInput { .. }))
        .expect("new body persisted its initial human input");
    assert!(
        startup_index < first_input,
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
    server.abort();
}
