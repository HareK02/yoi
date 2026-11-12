// Real Controller/Stop and producer-owned saved history; only the model stream
// is substituted. These tests do not manufacture termination or attribution rows.
use super::*;
use crate::retention::{
    DiagnosticsDisposition, SessionDisposition, WorkerRetentionExecutionRequest,
};
use crate::{FsRuntimeStoreOptions, RuntimeWorkspaceScope};
use session_store::worker_sources::{WorkerSessionOwner, WorkerSessionSources};
use session_store::{LogEntry, Store};

pub(super) struct RemovalFactory {
    pub(super) root: PathBuf,
    pub(super) client: MockClient,
}

#[async_trait]
impl RuntimeWorkerFactory for RemovalFactory {
    async fn prepare_restore(
        &self,
        _request: WorkerExecutionRestoreRequest,
    ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        Err("fixture must not restore a frozen source".into())
    }
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let manifest = WorkerManifest::from_toml(&format!(
            r#"
[worker]
name = "{name}"
pwd = "./"
[model]
scheme = "anthropic"
model_id = "fixture"
auth = {{ kind = "none" }}
[engine]
max_tokens = 100
[[scope.allow]]
target = "./"
permission = "write"
"#
        ))
        .map_err(|error| error.to_string())?;
        let workspace = request
            .request
            .workspace_api
            .as_ref()
            .unwrap()
            .workspace_id
            .clone();
        let sources = WorkerSessionSources::new(
            &self.root,
            WorkerSessionOwner {
                workspace_id: workspace.clone(),
                worker_id: request.worker_ref.worker_id.to_string(),
            },
        )
        .map_err(|error| error.to_string())?;
        let sessions = WorkerSessionStore::new_with_retention(
            sources.worker_root().join("session"),
            crate::retention::runtime_session_retention(&self.root),
        )
        .map_err(|error| error.to_string())?
        .with_runtime_owner(sources.clone())
        .map_err(|error| error.to_string())?;
        let metadata = WorkerAggregateStore::new(sources.worker_root(), &name)
            .map_err(|error| error.to_string())?;
        let context = WorkerWorkspaceContext::with_client(
            Some(WorkspaceId::new(workspace.clone()).unwrap()),
            Arc::new(LegacyNonSubjectAuthorizationClient(
                WorkerWorkspaceContext::local_filesystem(Some(
                    WorkspaceId::new(workspace).unwrap(),
                ))
                .client_handle(),
            )),
        );
        let workspace_client = context.client_handle();
        let mut worker = Worker::new(
            manifest,
            Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                self.client.clone(),
            ),
            CombinedStore::new(sessions, metadata),
            context,
            WorkerFilesystemAuthority::None,
            Scope::writable(&self.root).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        worker.engine_mut().set_system_prompt("fixture source");
        worker
            .enable_worker_metadata_write_through()
            .map_err(|error| error.to_string())?;
        let started = worker::bootstrap::start_worker_controller(
            worker,
            worker::bootstrap::WorkerBootstrapLayout::Direct {
                runtime_base: self.root.join("ipc"),
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(RuntimeWorkerController {
            handle: started.handle,
            shutdown: Arc::new(tokio::sync::Mutex::new(Some(started.shutdown))),
            controller_task: started.controller_task,
            workspace_client,
            restore_streams: None,
        })
    }
}

fn seed(session: session_store::SessionId, text: &str) -> LogEntry {
    LogEntry::AnnotatedSegmentStart {
        ts: 1,
        session_id: session,
        system_prompt: None,
        config: Default::default(),
        history: vec![session_store::LoggedHistoryEntry {
            item: session_store::LoggedItem::Message {
                role: session_store::LoggedRole::User,
                content: vec![session_store::LoggedContentPart::Text { text: text.into() }],
            },
            metadata: session_store::LoggedSessionHistoryMetadata {
                entry_id: session_store::LoggedSessionHistoryEntryId(
                    uuid::Uuid::now_v7().to_string(),
                ),
                origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
                input_metadata: None,
            },
        }],
        forked_from: None,
        compacted_from: None,
    }
}

fn removal_request(worker: WorkerId) -> WorkerRetentionExecutionRequest {
    WorkerRetentionExecutionRequest {
        operation_id: "remove-original".into(),
        input_fingerprint: "fixed-removal-input".into(),
        archive_id: None,
        workspace_id: "workspace-test".into(),
        source_runtime_id: "runtime-test".into(),
        worker_id: worker,
        source_created_at: "2026-01-01T00:00:00Z".into(),
        removed_at: "2026-01-01T01:00:00Z".into(),
        effective_profile: None,
        retention_class: None,
        policy_id: "fixture".into(),
        session_disposition: SessionDisposition::Purge,
        diagnostics_disposition: DiagnosticsDisposition::Purge,
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn removal_freezes_real_stopped_owner_captures_all_sessions_and_survives_reopen() {
    for disposition in [SessionDisposition::Purge, SessionDisposition::Archive] {
        let root = tempfile::tempdir().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(RemovalFactory {
                root: root.path().into(),
                client: MockClient::sequential(vec![MockResponse::Gated {
                    started: started.clone(),
                    release: release.clone(),
                    events: simple_text_events(),
                }]),
            })
            .unwrap(),
        );
        let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test");
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        runtime.bind_runtime_identity("runtime-test").unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scope = RuntimeWorkspaceScope::new("workspace-test", "server-test");
        let mut request = create_request("canonical removal");
        request.workspace_api = Some(crate::catalog::WorkspaceApiRef {
            workspace_id: scope.workspace_id.clone(),
            base_url: "https://backend.test".into(),
        });
        request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: scope.workspace_id.clone(),
            language: "English".into(),
        });
        let worker = runtime
            .create_worker_scoped(&scope, request.clone())
            .unwrap();
        runtime
            .send_input(&worker.worker_ref, WorkerInput::user("original input"))
            .unwrap();
        backend
            .run_on_adapter_runtime(async move {
                tokio::time::timeout(Duration::from_secs(5), started.notified())
                    .await
                    .map_err(|_| "source did not start".to_string())
            })
            .unwrap();
        let mut removal = removal_request(worker.worker_id);
        removal.session_disposition = disposition;
        removal.archive_id =
            (disposition == SessionDisposition::Archive).then(|| "all-sessions-archive".into());
        assert!(
            runtime
                .freeze_worker_removal_scoped(&scope, &removal)
                .is_err(),
            "running source must not freeze"
        );
        release.notify_one();
        wait_for_adapter_state(&backend, &worker.worker_ref, WorkerStatus::Idle);
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        assert!(
            runtime
                .execution_termination(&worker.worker_ref)
                .unwrap()
                .is_some(),
            "real Stop owner must prove shutdown"
        );
        let sources = WorkerSessionSources::new(
            root.path(),
            WorkerSessionOwner {
                workspace_id: scope.workspace_id.clone(),
                worker_id: worker.worker_id.to_string(),
            },
        )
        .unwrap();
        let producer = session_store::FsStore::new_runtime_owned(
            sources.clone(),
            crate::retention::runtime_session_retention(root.path()),
        )
        .unwrap();
        let retained_session = session_store::new_session_id();
        let retained_segment = session_store::new_segment_id();
        let text = "retained UTF-8 🦀\\\"\n".repeat(6000);
        producer
            .create_segment(
                retained_session,
                retained_segment,
                &[seed(retained_session, &text)],
            )
            .unwrap();
        let source = runtime_api::SessionPublicSource::Retained {
            worker_id: worker.worker_id,
        };
        let pin = session_store::retention::SessionRetentionPin(uuid::Uuid::now_v7());
        runtime
            .pin_session_public_source_scoped(
                &scope,
                &source,
                &retained_session.to_string(),
                pin.clone(),
            )
            .unwrap();

        assert!(
            runtime
                .publish_worker_removal_archive_scoped(&scope, &removal)
                .is_err(),
            "publication requires a persisted freeze and an archive disposition"
        );
        let frozen = runtime
            .freeze_worker_removal_scoped(&scope, &removal)
            .unwrap();
        let published = if disposition == SessionDisposition::Archive {
            let manifest = runtime
                .publish_worker_removal_archive_scoped(&scope, &removal)
                .unwrap();
            assert_eq!(
                runtime
                    .publish_worker_removal_archive_scoped(&scope, &removal)
                    .unwrap(),
                manifest
            );
            assert!(
                sources.worker_root().exists(),
                "publication must not delete the producer"
            );
            assert_eq!(
                crate::retention::runtime_session_retention(root.path())
                    .pins(retained_session)
                    .unwrap(),
                vec![pin.clone()]
            );
            let archive_root = root
                .path()
                .join("archives/workers")
                .join(&manifest.archive_id);
            let manifest_path = archive_root.join("manifest.json");
            let bytes = std::fs::read(&manifest_path).unwrap();
            std::fs::write(&manifest_path, b"invalid archive manifest").unwrap();
            assert!(
                runtime
                    .publish_worker_removal_archive_scoped(&scope, &removal)
                    .is_err()
            );
            assert!(
                sources.worker_root().exists(),
                "a failed replay must preserve the original"
            );
            std::fs::write(manifest_path, bytes).unwrap();
            Some(manifest)
        } else {
            assert!(
                runtime
                    .publish_worker_removal_archive_scoped(&scope, &removal)
                    .is_err()
            );
            None
        };
        assert_eq!(frozen.members.len(), 2);
        assert_eq!(
            runtime
                .freeze_worker_removal_scoped(&scope, &removal)
                .unwrap(),
            frozen
        );
        let member = frozen
            .members
            .iter()
            .find(|member| member.session_id == retained_session)
            .unwrap();
        assert_eq!(member.descriptors.len(), 1);
        assert!(
            producer
                .append(
                    retained_session,
                    retained_segment,
                    &LogEntry::TurnEnd {
                        ts: 2,
                        turn_count: 1
                    }
                )
                .is_err(),
            "persisted fence must reject source append"
        );
        assert!(
            runtime
                .send_input(&worker.worker_ref, WorkerInput::user("late input"))
                .is_err()
        );
        assert!(runtime.create_worker_scoped(&scope, request).is_err());
        let mut conflicting = removal.clone();
        conflicting.operation_id = "replacement".into();
        assert!(
            runtime
                .freeze_worker_removal_scoped(&scope, &conflicting)
                .is_err()
        );
        assert!(runtime.execute_worker_retention(&conflicting).is_err());
        assert!(
            runtime
                .publish_worker_removal_archive_scoped(&scope, &conflicting)
                .is_err()
        );
        let captures = frozen
            .members
            .iter()
            .filter(|member| member.materialized)
            .map(|member| {
                let capture = runtime
                    .capture_worker_removal_session_scoped(
                        &scope,
                        &removal.operation_id,
                        worker.worker_id,
                        member.session_id,
                        member.active_segment_id,
                        Default::default(),
                        4096,
                        4 * 1024 * 1024,
                    )
                    .unwrap();
                (member.session_id, member.active_segment_id, capture)
            })
            .collect::<Vec<_>>();
        let mut position = Default::default();
        let mut body = String::new();
        loop {
            let slice = runtime
                .capture_worker_removal_session_scoped(
                    &scope,
                    &removal.operation_id,
                    worker.worker_id,
                    retained_session,
                    Some(retained_segment),
                    position,
                    2,
                    80 * 1024,
                )
                .unwrap();
            assert!(serde_json::to_vec(&slice).unwrap().len() <= 80 * 1024);
            for entry in slice.capture.entries {
                if let session_store::capture::CaptureBody::CanonicalPublic { text, .. } =
                    entry.body
                {
                    body.push_str(&text);
                }
            }
            match slice.next {
                Some(next) => {
                    assert_ne!(next, position);
                    position = next;
                }
                None => break,
            }
        }
        assert_eq!(
            body.len() as u64,
            member.descriptors[0].total_bytes,
            "all canonical bytes must survive bounded transport slicing"
        );
        use sha2::Digest;
        assert_eq!(
            sha2::Sha256::digest(body.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            member.descriptors[0].sha256
        );
        drop(runtime);
        let reopened =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options, backend).unwrap();
        reopened.bind_runtime_identity("runtime-test").unwrap();
        assert!(
            reopened
                .send_input(&worker.worker_ref, WorkerInput::user("post-restart input"))
                .is_err()
        );
        assert_eq!(
            reopened
                .freeze_worker_removal_scoped(&scope, &removal)
                .unwrap(),
            frozen
        );
        if removal.session_disposition == SessionDisposition::Purge {
            assert!(reopened.execute_worker_retention(&removal).is_err());
            assert!(sources.worker_root().exists());
            reopened
                .release_session_public_source_pin_scoped(
                    &scope,
                    &source,
                    &retained_session.to_string(),
                    pin.clone(),
                )
                .unwrap();
        }
        let done = reopened.execute_worker_retention(&removal).unwrap();
        if let Some(manifest) = &done.archive {
            assert_eq!(
                crate::retention::runtime_session_retention(root.path())
                    .pins(retained_session)
                    .unwrap(),
                vec![pin.clone()]
            );
            assert!(
                reopened
                    .delete_session_public_source_scoped(
                        &scope,
                        &runtime_api::SessionPublicSource::Archived {
                            archive_id: manifest.archive_id.clone()
                        },
                        &retained_session.to_string(),
                        session_store::retention::SessionDeletionIntent::Ordinary,
                    )
                    .is_err(),
                "ordinary GC must still respect the transferred recording pin"
            );
            assert_eq!(published.as_ref(), Some(manifest));
            assert!(!sources.worker_root().exists());
            assert_eq!(
                reopened
                    .publish_worker_removal_archive_scoped(&scope, &removal)
                    .unwrap(),
                *manifest
            );
            for (session, segment, capture) in &captures {
                assert_eq!(
                    reopened
                        .capture_worker_removal_session_scoped(
                            &scope,
                            &removal.operation_id,
                            worker.worker_id,
                            *session,
                            *segment,
                            Default::default(),
                            4096,
                            4 * 1024 * 1024,
                        )
                        .unwrap(),
                    *capture,
                    "all frozen originals remain readable without the original Worker"
                );
            }
            assert_eq!(manifest.retained_sessions.len(), 1);
            let saved = root
                .path()
                .join("archives/workers")
                .join(&manifest.archive_id)
                .join("session/retained-sessions")
                .join(retained_session.to_string())
                .join(format!("{retained_segment}.jsonl"));
            assert!(
                saved.exists(),
                "archive must preserve every retained Session original"
            );
            let archived = runtime_api::SessionPublicSource::Archived {
                archive_id: manifest.archive_id.clone(),
            };
            reopened
                .delete_session_public_source_scoped(
                    &scope,
                    &archived,
                    &manifest.source_session_id,
                    session_store::retention::SessionDeletionIntent::ErasePermanently,
                )
                .unwrap();
            assert!(
                saved.exists(),
                "erasing the primary Session must preserve unrelated retained Sessions"
            );
            let retained_capture = captures
                .iter()
                .find(|(session, _, _)| *session == retained_session)
                .unwrap();
            assert_eq!(
                reopened
                    .capture_worker_removal_session_scoped(
                        &scope,
                        &removal.operation_id,
                        worker.worker_id,
                        retained_session,
                        Some(retained_segment),
                        Default::default(),
                        4096,
                        4 * 1024 * 1024,
                    )
                    .unwrap(),
                retained_capture.2,
                "erasing another Session must not block this recording"
            );
            // At the provider boundary, explicit whole-Subject erasure must
            // fence every archived Session, even after primary-only erasure.
            // The canonical removal/Stop receipt above is real and unchanged.
            let archived_pin = session_store::retention::SessionRetentionPin(uuid::Uuid::now_v7());
            reopened
                .pin_session_public_source_scoped(
                    &scope,
                    &archived,
                    &retained_session.to_string(),
                    archived_pin,
                )
                .unwrap();
            crate::retention::FsWorkerRetentionProvider::new(root.path())
                .purge_subject_job_removal(&removal)
                .unwrap();
            assert!(!saved.exists());
            let retention = crate::retention::runtime_session_retention(root.path());
            assert!(matches!(
                retention.ensure_live(retained_session),
                Err(session_store::StoreError::SessionErased(id)) if id == retained_session
            ));
            assert!(retention.pins(retained_session).unwrap().is_empty());
            assert!(
                reopened
                    .publish_worker_removal_archive_scoped(&scope, &removal)
                    .is_err()
            );
            assert!(
                reopened
                    .capture_worker_removal_session_scoped(
                        &scope,
                        &removal.operation_id,
                        worker.worker_id,
                        retained_session,
                        Some(retained_segment),
                        Default::default(),
                        4096,
                        4 * 1024 * 1024,
                    )
                    .is_err(),
                "explicit erasure must fence saved archive capture"
            );
            // Explicit per-Session erasure remains idempotent after whole-owner
            // cleanup; ownership and the historical receipt remain resolvable.
            reopened
                .delete_session_public_source_scoped(
                    &scope,
                    &archived,
                    &retained_session.to_string(),
                    session_store::retention::SessionDeletionIntent::ErasePermanently,
                )
                .unwrap();
            assert!(!saved.exists());
        }
        assert!(done.source_removed);
        assert_eq!(reopened.execute_worker_retention(&removal).unwrap(), done);
    }
}

fn stopped_source_with_queue(
    queued: bool,
) -> (
    tempfile::TempDir,
    EmbeddedRuntime,
    RuntimeWorkspaceScope,
    crate::catalog::WorkerDetail,
) {
    let root = tempfile::tempdir().unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(RemovalFactory {
            root: root.path().into(),
            client: MockClient::sequential(vec![MockResponse::Gated {
                started: started.clone(),
                release: Arc::new(tokio::sync::Notify::new()),
                events: simple_text_events(),
            }]),
        })
        .unwrap(),
    );
    let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
        FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test"),
        backend.clone(),
    )
    .unwrap();
    runtime.bind_runtime_identity("runtime-test").unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = RuntimeWorkspaceScope::new("workspace-test", "server-test");
    let mut request = create_request("queue reconciliation");
    request.workspace_api = Some(crate::catalog::WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://backend.test".into(),
    });
    request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    let worker = runtime.create_worker_scoped(&scope, request).unwrap();
    runtime
        .send_input(&worker.worker_ref, WorkerInput::user("first source input"))
        .unwrap();
    backend
        .run_on_adapter_runtime(async move {
            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .map_err(|_| "source did not start".to_string())
        })
        .unwrap();
    if queued {
        runtime
            .send_input(
                &worker.worker_ref,
                WorkerInput::user("accepted waiting input"),
            )
            .unwrap();
    }
    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    (root, runtime, scope, worker)
}

#[test]
#[serial_test::serial(worker_allocation)]
fn planned_retention_reopens_after_history_erasure_with_saved_stop() {
    let (root, runtime, scope, worker) = stopped_source_with_queue(false);
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    let worker_root = root
        .path()
        .join("workers")
        .join(worker.worker_id.to_string());
    let primary = crate::retention::aggregate_session_id(&worker_root)
        .unwrap()
        .unwrap();
    let request = removal_request(worker.worker_id);
    let mut archive = request.clone();
    archive.operation_id = "archive-before-erasure".into();
    archive.input_fingerprint = "archive-before-erasure-input".into();
    archive.session_disposition = SessionDisposition::Archive;
    archive.archive_id = Some("archive-before-erasure".into());
    runtime
        .freeze_worker_removal_scoped(&scope, &archive)
        .unwrap();
    assert!(
        runtime.execute_worker_retention(&request).is_err(),
        "ordinary policy changes cannot replace a frozen operation"
    );
    runtime
        .delete_session_public_source_scoped(
            &scope,
            &runtime_api::SessionPublicSource::Retained {
                worker_id: worker.worker_id,
            },
            &primary.to_string(),
            session_store::retention::SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    assert!(!worker_root.join("session").exists());
    assert!(worker_root.join("worker.json").exists());
    assert!(
        crate::retention::FsWorkerRetentionProvider::new(root.path())
            .completed_for(&request)
            .unwrap()
            .is_none()
    );
    drop(runtime);

    let no_launch = root.path().join("must-not-launch");
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(ProfileRuntimeWorkerFactory::new(&no_launch)).unwrap(),
    );
    let reopened = EmbeddedRuntime::with_fs_store_and_execution_backend(
        FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test"),
        backend.clone(),
    )
    .unwrap();
    reopened.bind_runtime_identity("runtime-test").unwrap();
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    let completed = reopened.execute_worker_retention(&request).unwrap();
    assert!(completed.source_removed);
    assert_eq!(completed.operation_id, request.operation_id);
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            root.path()
                .join("retention/operations")
                .join(format!("{}.json", request.operation_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["request"], serde_json::to_value(&request).unwrap());
    assert_eq!(
        saved["shutdown"]["termination"],
        serde_json::to_value(&original).unwrap()
    );
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(!worker_root.exists());
    assert!(!no_launch.exists());
}

#[test]
#[serial_test::serial(worker_allocation)]
fn pending_retention_reopens_after_history_erasure_with_original_stop_and_operation() {
    check_pending_retention_erasure_recovery(true);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn pending_retention_live_stopped_catalog_consumes_original_receipt_after_source_removal() {
    check_pending_retention_erasure_recovery(false);
}

fn check_pending_retention_erasure_recovery(restart: bool) {
    use session_store::retention::{SessionDeletionIntent, SessionRetentionPin};

    let (root, runtime, scope, worker) = stopped_source_with_queue(false);
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    let owner = WorkerSessionSources::new(
        root.path(),
        WorkerSessionOwner {
            workspace_id: scope.workspace_id.clone(),
            worker_id: worker.worker_id.to_string(),
        },
    )
    .unwrap();
    let primary = crate::retention::aggregate_session_id(&owner.worker_root())
        .unwrap()
        .unwrap();
    let retention = crate::retention::runtime_session_retention(root.path());
    let producer =
        session_store::FsStore::new_runtime_owned(owner.clone(), retention.clone()).unwrap();
    let sibling = session_store::new_session_id();
    producer
        .create_segment(
            sibling,
            session_store::new_segment_id(),
            &[seed(sibling, "retain until the consumer releases its pin")],
        )
        .unwrap();
    let source = runtime_api::SessionPublicSource::Retained {
        worker_id: worker.worker_id,
    };
    let pin = SessionRetentionPin(uuid::Uuid::now_v7());
    runtime
        .pin_session_public_source_scoped(&scope, &source, &sibling.to_string(), pin)
        .unwrap();
    let request = removal_request(worker.worker_id);
    runtime
        .freeze_worker_removal_scoped(&scope, &request)
        .unwrap();
    // The real provider saves its pending receipt before ordinary deletion
    // encounters the consumer pin. No receipt or completion proof is fabricated.
    let error = runtime.execute_worker_retention(&request).unwrap_err();
    assert!(error.to_string().contains("durable consumer"), "{error}");
    let provider = crate::retention::FsWorkerRetentionProvider::new(root.path());
    assert!(provider.completed_for(&request).unwrap().is_none());
    let receipt_path = root
        .path()
        .join("retention/operations")
        .join(format!("{}.json", request.operation_id));
    let pending_bytes = std::fs::read(&receipt_path).unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&pending_bytes).unwrap();
    assert_eq!(pending["result"]["source_removed"], false);
    assert_eq!(
        pending["shutdown"]["termination"],
        serde_json::to_value(&original).unwrap()
    );
    runtime
        .release_session_public_source_pin_scoped(&scope, &source, &sibling.to_string(), pin)
        .unwrap();
    runtime
        .delete_session_public_source_scoped(
            &scope,
            &source,
            &primary.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    runtime
        .delete_session_public_source_scoped(
            &scope,
            &source,
            &sibling.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    assert!(!owner.worker_root().join("session").exists());
    assert!(owner.worker_root().join("worker.json").exists());
    if !restart {
        // Fault boundary: deletion of the already-erased aggregate completed,
        // but the provider's real pending receipt and live stopped catalog remain.
        std::fs::remove_dir_all(owner.worker_root()).unwrap();
        let completed = runtime.execute_worker_retention(&request).unwrap();
        assert!(completed.source_removed);
        assert_eq!(completed.operation_id, request.operation_id);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(saved["request"], pending["request"]);
        assert_eq!(saved["shutdown"], pending["shutdown"]);
        assert_eq!(saved["result"]["source_removed"], true);
        assert!(runtime.worker_detail(&worker.worker_ref).is_err());
        return;
    }
    drop(runtime);

    let no_launch = root.path().join("must-not-launch");
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(ProfileRuntimeWorkerFactory::new(&no_launch)).unwrap(),
    );
    let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test");
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    reopened.bind_runtime_identity("runtime-test").unwrap();
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    let mut changed = request.clone();
    changed.input_fingerprint = "different-removal-input".into();
    assert!(reopened.execute_worker_retention(&changed).is_err());
    changed.operation_id = "replacement-must-not-reparent".into();
    assert!(
        reopened.execute_worker_retention(&changed).is_err(),
        "even full explicit erasure must preserve an issued original receipt"
    );
    assert_eq!(
        std::fs::read(&receipt_path).unwrap(),
        pending_bytes,
        "pending receipt must remain unchanged"
    );
    assert!(owner.worker_root().exists());
    let completed = reopened.execute_worker_retention(&request).unwrap();
    assert!(completed.source_removed);
    assert_eq!(completed.operation_id, request.operation_id);
    assert_eq!(completed.input_fingerprint, request.input_fingerprint);
    assert!(!owner.worker_root().exists());
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(saved["request"], pending["request"]);
    assert_eq!(saved["shutdown"], pending["shutdown"]);
    assert_eq!(saved["result"]["source_removed"], true);
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(!no_launch.exists());
    assert!(matches!(
        retention.ensure_live(primary),
        Err(session_store::StoreError::SessionErased(id)) if id == primary
    ));
    drop(reopened);
    let reopened = EmbeddedRuntime::with_fs_store_and_execution_backend(options, backend).unwrap();
    reopened.bind_runtime_identity("runtime-test").unwrap();
    assert_eq!(
        reopened.execute_worker_retention(&request).unwrap(),
        completed
    );
}

#[test]
#[serial_test::serial(worker_allocation)]
fn aggregate_cleanup_does_not_spread_a_session_tombstone_to_pinned_siblings() {
    use session_store::retention::{SessionDeletionIntent, SessionRetentionPin};
    for intent in [
        SessionDeletionIntent::Ordinary,
        SessionDeletionIntent::ErasePermanently,
    ] {
        let (root, runtime, scope, worker) = stopped_source_with_queue(false);
        let sources = WorkerSessionSources::new(
            root.path(),
            WorkerSessionOwner {
                workspace_id: scope.workspace_id.clone(),
                worker_id: worker.worker_id.to_string(),
            },
        )
        .unwrap();
        let retention = crate::retention::runtime_session_retention(root.path());
        let producer =
            session_store::FsStore::new_runtime_owned(sources.clone(), retention.clone()).unwrap();
        let sibling = session_store::new_session_id();
        producer
            .create_segment(
                sibling,
                session_store::new_segment_id(),
                &[seed(
                    sibling,
                    "retained sibling must not inherit erase authority",
                )],
            )
            .unwrap();
        let source = runtime_api::SessionPublicSource::Retained {
            worker_id: worker.worker_id,
        };
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        runtime
            .pin_session_public_source_scoped(&scope, &source, &sibling.to_string(), pin)
            .unwrap();
        let primary = crate::retention::aggregate_session_id(&sources.worker_root())
            .unwrap()
            .unwrap();
        // The supported erase API persists its fence before physical cleanup.
        // Leave cleanup pending, as after an interruption, without forging rows.
        retention
            .delete(primary, SessionDeletionIntent::ErasePermanently, || Ok(()))
            .unwrap();
        let result =
            crate::retention::delete_worker_aggregate(root.path(), &sources.worker_root(), intent);
        if intent == SessionDeletionIntent::Ordinary {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("durable consumer"), "{error}");
            assert!(sources.worker_root().exists());
            retention.ensure_live(sibling).unwrap();
            assert_eq!(retention.pins(sibling).unwrap(), [pin]);
            retention.release(sibling, pin).unwrap();
            crate::retention::delete_worker_aggregate(root.path(), &sources.worker_root(), intent)
                .unwrap();
            retention.ensure_live(sibling).unwrap();
        } else {
            result.unwrap();
            assert!(matches!(
                retention.ensure_live(sibling),
                Err(session_store::StoreError::SessionErased(id)) if id == sibling
            ));
            assert!(retention.pins(sibling).unwrap().is_empty());
        }
        assert!(!sources.worker_root().exists());
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn removal_holds_real_accepted_queue_without_discarding_or_freezing_its_recovery() {
    let (root, runtime, scope, worker) = stopped_source_with_queue(true);
    let source = root
        .path()
        .join("workers")
        .join(worker.worker_id.to_string());
    let error = runtime
        .freeze_worker_removal_scoped(&scope, &removal_request(worker.worker_id))
        .unwrap_err();
    assert!(
        matches!(
            &error,
            crate::error::RuntimeError::RecordingExecutionUnconfirmed { .. }
        ),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("accepted pending input"),
        "{error}"
    );
    assert!(source.join("session/session.json").exists());
    assert!(
        !root
            .path()
            .join("retention/removal-freezes")
            .join(format!("{}.json", worker.worker_id))
            .exists(),
        "accepted inputs need explicit recovery before fixed removal ends exist"
    );
}

#[test]
#[serial_test::serial(worker_allocation)]
fn removal_holds_unmapped_producer_material_instead_of_returning_empty_members() {
    let (root, runtime, scope, worker) = stopped_source_with_queue(false);
    let owner = WorkerSessionSources::new(
        root.path(),
        WorkerSessionOwner {
            workspace_id: scope.workspace_id.clone(),
            worker_id: worker.worker_id.to_string(),
        },
    )
    .unwrap();
    let producer = session_store::FsStore::new_runtime_owned(
        owner.clone(),
        crate::retention::runtime_session_retention(root.path()),
    )
    .unwrap();
    let session = session_store::new_session_id();
    let segment = session_store::new_segment_id();
    producer
        .create_segment(session, segment, &[seed(session, "owned saved history")])
        .unwrap();
    // Fault injection removes a real producer's binding, never invents ownership.
    std::fs::remove_file(
        owner
            .worker_root()
            .join("retained-session-bindings")
            .join(format!("{session}.json")),
    )
    .unwrap();
    assert!(
        runtime
            .freeze_worker_removal_scoped(&scope, &removal_request(worker.worker_id))
            .is_err()
    );
    assert!(
        owner
            .retained_root()
            .join(session.to_string())
            .join(format!("{segment}.jsonl"))
            .exists()
    );
}

struct ReleaseFixture {
    root: tempfile::TempDir,
    runtime: EmbeddedRuntime,
    backend: Arc<WorkerRuntimeExecutionBackend<RemovalFactory>>,
    scope: RuntimeWorkspaceScope,
    worker: crate::catalog::WorkerDetail,
}

impl ReleaseFixture {
    fn new(memory: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(RemovalFactory {
                root: root.path().into(),
                client: MockClient::sequential(vec![MockResponse::Gated {
                    started: started.clone(),
                    release: release.clone(),
                    events: simple_text_events(),
                }]),
            })
            .unwrap(),
        );
        let runtime = if memory {
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap()
        } else {
            EmbeddedRuntime::with_fs_store_and_execution_backend(
                FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test"),
                backend.clone(),
            )
            .unwrap()
        };
        runtime.bind_runtime_identity("runtime-test").unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scope = RuntimeWorkspaceScope::new("workspace-test", "server-test");
        let mut create = create_request("release full retained scope");
        create.workspace_api = Some(crate::catalog::WorkspaceApiRef {
            workspace_id: scope.workspace_id.clone(),
            base_url: "https://backend.test".into(),
        });
        create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: scope.workspace_id.clone(),
            language: "English".into(),
        });
        let worker = runtime.create_worker_scoped(&scope, create).unwrap();
        runtime
            .send_input(
                &worker.worker_ref,
                WorkerInput::user("active source material"),
            )
            .unwrap();
        backend
            .run_on_adapter_runtime(async move {
                tokio::time::timeout(Duration::from_secs(5), started.notified())
                    .await
                    .map_err(|_| "release fixture source did not start".to_string())
            })
            .unwrap();
        wait_for_adapter_state(&backend, &worker.worker_ref, WorkerStatus::Running);
        release.notify_one();
        wait_for_adapter_state(&backend, &worker.worker_ref, WorkerStatus::Idle);
        Self {
            root,
            runtime,
            backend,
            scope,
            worker,
        }
    }

    fn request(&self) -> runtime_api::SubjectReleaseRequest {
        runtime_api::SubjectReleaseRequest {
            workspace_id: self.scope.workspace_id.clone(),
            worker_id: self.worker.worker_id,
            binding_id: "binding-1".into(),
            operation_id: "release-1".into(),
            action: runtime_api::SubjectReleaseAction::Reserve,
        }
    }

    fn owner(&self) -> WorkerSessionSources {
        WorkerSessionSources::new(
            self.root.path(),
            WorkerSessionOwner {
                workspace_id: self.scope.workspace_id.clone(),
                worker_id: self.worker.worker_id.to_string(),
            },
        )
        .unwrap()
    }
}

fn release_capture_request(
    release: &runtime_api::SubjectReleaseRequest,
    member: &runtime_api::WorkerRemovalSessionMember,
) -> runtime_api::SubjectReleaseSessionCaptureRequest {
    runtime_api::SubjectReleaseSessionCaptureRequest {
        workspace_id: release.workspace_id.clone(),
        operation_id: release.operation_id.clone(),
        binding_id: release.binding_id.clone(),
        session_id: member.session_id,
        active_segment_id: member.active_segment_id,
        start: Default::default(),
        max_entries: 4096,
        max_serialized_bytes: 4 * 1024 * 1024,
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn subject_release_scope_fixes_multiple_sessions_and_detached_branches_until_exact_cancel() {
    use runtime_api::SubjectReleaseAction as Action;
    let fixture = ReleaseFixture::new(false);
    let owner = fixture.owner();
    let retention = crate::retention::runtime_session_retention(fixture.root.path());
    let producer =
        session_store::FsStore::new_runtime_owned(owner.clone(), retention.clone()).unwrap();
    let primary = session_store::WorkerAggregateStore::open_read_only(
        owner.worker_root(),
        ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&fixture.worker.worker_ref),
    )
    .unwrap()
    .read_read_only()
    .unwrap()
    .unwrap()
    .active
    .unwrap();
    let primary_producer = WorkerSessionStore::open_existing_with_retention(
        owner.worker_root().join("session"),
        retention,
    )
    .unwrap()
    .with_runtime_owner(owner.clone())
    .unwrap();
    let detached = session_store::new_segment_id();
    primary_producer
        .create_segment(
            primary.session_id,
            detached,
            &[seed(primary.session_id, "primary detached original")],
        )
        .unwrap();
    let session = session_store::new_session_id();
    let active = session_store::new_segment_id();
    let branch = session_store::new_segment_id();
    producer
        .create_segment(
            session,
            active,
            &[seed(session, "retained active original")],
        )
        .unwrap();
    producer
        .create_segment(
            session,
            branch,
            &[seed(
                session,
                &"retained detached original UTF-8 🦀\\\"\n".repeat(6000),
            )],
        )
        .unwrap();
    let mut request = fixture.request();
    let held = fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    let scope = held
        .recording_scope
        .as_ref()
        .expect("Reserve must return a complete scope");
    assert_eq!(scope.workspace_id, fixture.scope.workspace_id);
    assert_eq!(scope.source_runtime_id, "runtime-test");
    assert_eq!(scope.worker_id, fixture.worker.worker_id);
    assert_eq!(scope.members.len(), 2);
    let retained = scope
        .members
        .iter()
        .find(|member| member.session_id == session)
        .unwrap();
    assert_eq!(retained.active_segment_id, Some(active));
    assert_eq!(
        retained.descriptors.len(),
        2,
        "detached originals must not be omitted"
    );
    assert_eq!(retained.end.canonical_order, 2);
    let main = scope
        .members
        .iter()
        .find(|member| member.session_id == primary.session_id)
        .unwrap();
    assert!(
        main.descriptors.len() >= 3,
        "active input/result and detached original must be present"
    );
    // Reserved scopes support validation reads, not recording dispatch. Rebuild
    // every canonical original through bounded pages, including detached branches.
    for member in &scope.members {
        let mut capture = release_capture_request(&request, member);
        capture.max_entries = 1;
        capture.max_serialized_bytes = 16 * 1024;
        let mut bodies = vec![String::new(); member.descriptors.len()];
        let mut pages = 0;
        loop {
            let slice = fixture
                .runtime
                .capture_subject_release_session_scoped(
                    &fixture.scope,
                    fixture.worker.worker_id,
                    &capture,
                )
                .unwrap();
            assert!(serde_json::to_vec(&slice).unwrap().len() <= capture.max_serialized_bytes);
            assert!(slice.capture.entries.len() <= capture.max_entries);
            assert_eq!(slice.capture.view_entry_count, member.end.canonical_order);
            for entry in slice.capture.entries {
                let session_store::capture::CaptureBody::CanonicalPublic {
                    descriptor,
                    byte_offset,
                    text,
                    ..
                } = entry.body
                else {
                    panic!("release capture must preserve canonical originals");
                };
                let order = entry.order as usize;
                assert_eq!(descriptor, member.descriptors[order]);
                assert_eq!(byte_offset, bodies[order].len() as u64);
                bodies[order].push_str(&text);
            }
            pages += 1;
            assert!(pages < 1000, "bounded capture must make progress");
            match slice.next {
                Some(next) => {
                    assert_ne!(next, capture.start);
                    capture.start = next;
                }
                None => break,
            }
        }
        use sha2::Digest;
        for (body, descriptor) in bodies.iter().zip(&member.descriptors) {
            assert_eq!(body.len() as u64, descriptor.total_bytes);
            assert_eq!(
                sha2::Sha256::digest(body.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                descriptor.sha256
            );
        }
        if member.session_id == session {
            assert!(
                pages > member.descriptors.len(),
                "large originals must be byte-sliced"
            );
        }
    }
    let capture_request = release_capture_request(&request, retained);
    let capture = |request: &runtime_api::SubjectReleaseSessionCaptureRequest| {
        fixture.runtime.capture_subject_release_session_scoped(
            &fixture.scope,
            fixture.worker.worker_id,
            request,
        )
    };
    for changed in 0..10 {
        let mut invalid = capture_request.clone();
        match changed {
            0 => invalid.workspace_id = "another-workspace".into(),
            1 => invalid.operation_id = "another-operation".into(),
            2 => invalid.binding_id = "another-binding".into(),
            3 => invalid.session_id = session_store::new_session_id(),
            4 => invalid.active_segment_id = Some(branch),
            5 => invalid.active_segment_id = None,
            6 => invalid.max_entries = 0,
            7 => invalid.max_entries = 4097,
            8 => invalid.max_serialized_bytes = 1023,
            _ => invalid.max_serialized_bytes = 4 * 1024 * 1024 + 1,
        }
        assert!(
            capture(&invalid).is_err(),
            "changed capture field {changed}"
        );
    }
    assert!(
        fixture
            .runtime
            .capture_subject_release_session_scoped(
                &fixture.scope,
                WorkerId::now_v7(),
                &capture_request,
            )
            .is_err()
    );
    // Tamper a real saved original outside the first requested page. The full
    // union must be verified even when only one entry is requested.
    for path in [
        owner
            .worker_root()
            .join("session/segments")
            .join(format!("{detached}.jsonl")),
        owner
            .retained_root()
            .join(session.to_string())
            .join(format!("{branch}.jsonl")),
    ] {
        let original = std::fs::read(&path).unwrap();
        let changed = String::from_utf8(original.clone())
            .unwrap()
            .replace("detached original", "modified original");
        assert_ne!(changed.as_bytes(), original.as_slice());
        std::fs::write(&path, changed).unwrap();
        let member = if path.starts_with(owner.retained_root()) {
            retained
        } else {
            main
        };
        let mut invalid = release_capture_request(&request, member);
        invalid.max_entries = 1;
        assert!(matches!(
            capture(&invalid),
            Err(crate::error::RuntimeError::RecordingSourceUnavailable { .. })
        ));
        std::fs::write(path, original).unwrap();
        capture(&invalid).unwrap();
    }
    assert_eq!(
        fixture
            .runtime
            .subject_release_scoped(&fixture.scope, request.clone())
            .unwrap(),
        held
    );
    let fence_bytes = std::fs::read(owner.release_fence_path()).unwrap();
    std::fs::remove_file(owner.release_fence_path()).unwrap();
    assert!(
        capture(&capture_request).is_err(),
        "no ordinary active capture fallback"
    );
    std::fs::write(owner.release_fence_path(), &fence_bytes).unwrap();
    for (section, field) in [
        ("request", "workspace_id"),
        ("request", "worker_id"),
        ("request", "binding_id"),
        ("request", "operation_id"),
        ("recording_scope", "workspace_id"),
        ("recording_scope", "worker_id"),
        ("recording_scope", "source_runtime_id"),
    ] {
        let mut saved: serde_json::Value = serde_json::from_slice(&fence_bytes).unwrap();
        saved[section][field] = if field == "worker_id" {
            serde_json::to_value(WorkerId::now_v7()).unwrap()
        } else {
            serde_json::json!("different-identity")
        };
        std::fs::write(
            owner.release_fence_path(),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        assert!(
            capture(&capture_request).is_err(),
            "saved {section}.{field} must match"
        );
    }
    std::fs::write(owner.release_fence_path(), &fence_bytes).unwrap();
    capture(&capture_request).unwrap();
    let append = || {
        producer.append(
            session,
            active,
            &LogEntry::TurnEnd {
                ts: 3,
                turn_count: 1,
            },
        )
    };
    assert!(append().is_err());
    assert!(
        producer
            .create_segment(
                session,
                session_store::new_segment_id(),
                &[seed(session, "late import")]
            )
            .is_err()
    );
    assert!(
        producer
            .select_recording_active_segment(session, branch)
            .is_err()
    );
    assert!(
        primary_producer
            .append(
                primary.session_id,
                primary.segment_id.unwrap(),
                &LogEntry::TurnEnd {
                    ts: 3,
                    turn_count: 1
                }
            )
            .is_err()
    );
    assert!(owner.release_fence_path().exists());
    assert!(
        !owner.freeze_path().exists(),
        "live release must not install a permanent removal freeze"
    );
    request.action = Action::Cancel;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    assert!(!owner.release_fence_path().exists());
    assert!(
        capture(&capture_request).is_err(),
        "cancelled scope cannot be captured"
    );
    // Even a stale fence left by interrupted cancellation is not read authority.
    std::fs::write(owner.release_fence_path(), &fence_bytes).unwrap();
    assert!(capture(&capture_request).is_err());
    std::fs::remove_file(owner.release_fence_path()).unwrap();
    append().unwrap();
    let old_cancel = request.clone();
    request.operation_id = "release-2".into();
    request.action = Action::Reserve;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, old_cancel)
        .unwrap();
    assert!(
        append().is_err(),
        "stale cancellation must not clear the newer owner's fence"
    );
    request.action = Action::Disconnect;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    assert!(append().is_err(), "disconnected body remains NoPublication");
    assert!(
        capture(&capture_request).is_err(),
        "new release cannot revive old capture identity"
    );
    let mut disconnected_capture = capture_request.clone();
    disconnected_capture.operation_id = request.operation_id.clone();
    capture(&disconnected_capture).unwrap();
    request.action = Action::Cancel;
    assert!(
        fixture
            .runtime
            .subject_release_scoped(&fixture.scope, request)
            .is_err()
    );
    assert!(append().is_err());
    fixture
        .runtime
        .stop_worker(&fixture.worker.worker_ref, None)
        .unwrap();
    let retention = crate::retention::runtime_session_retention(fixture.root.path());
    let guard = retention.lock_live(primary.session_id).unwrap();
    let after_stop = session_store::capture::recording_union_catalog_worker_guarded(
        &owner.worker_root().join("session"),
        primary.session_id,
        primary.segment_id.unwrap(),
        Default::default(),
        &guard,
    )
    .unwrap();
    assert_eq!(
        after_stop, main.descriptors,
        "Stop housekeeping must preserve every fixed recording descriptor"
    );
    drop(guard);
    let before_remove = capture(&disconnected_capture).unwrap();
    let mut removal = removal_request(fixture.worker.worker_id);
    removal.session_disposition = SessionDisposition::Archive;
    removal.archive_id = Some("released-body-archive".into());
    fixture
        .runtime
        .freeze_worker_removal_scoped(&fixture.scope, &removal)
        .unwrap();
    fixture.runtime.execute_worker_retention(&removal).unwrap();
    assert!(
        capture(&disconnected_capture).is_err(),
        "release capture is live-source only, even when a removal archive exists"
    );
    assert_eq!(
        fixture
            .runtime
            .capture_worker_removal_session_scoped(
                &fixture.scope,
                &removal.operation_id,
                fixture.worker.worker_id,
                disconnected_capture.session_id,
                disconnected_capture.active_segment_id,
                disconnected_capture.start,
                disconnected_capture.max_entries,
                disconnected_capture.max_serialized_bytes,
            )
            .unwrap(),
        before_remove,
        "Backend can explicitly switch to the later Remove capture",
    );
}

#[test]
#[serial_test::serial(worker_allocation)]
fn subject_release_restart_requires_actual_controller_guard_before_returning_saved_scope() {
    use runtime_api::SubjectReleaseAction as Action;
    let fixture = ReleaseFixture::new(false);
    let owner = fixture.owner();
    let mut request = fixture.request();
    let held = fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    assert!(held.recording_scope.is_some());
    let capture_request =
        release_capture_request(&request, &held.recording_scope.as_ref().unwrap().members[0]);
    let captured = fixture
        .runtime
        .capture_subject_release_session_scoped(
            &fixture.scope,
            fixture.worker.worker_id,
            &capture_request,
        )
        .unwrap();
    let ReleaseFixture {
        root,
        runtime,
        backend,
        scope,
        worker,
    } = fixture;
    drop(runtime);
    drop(backend); // actual Controller and its in-memory guard are gone
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(RemovalFactory {
            root: root.path().into(),
            client: MockClient::new(simple_text_events()),
        })
        .unwrap(),
    );
    let reopened = EmbeddedRuntime::with_fs_store_and_execution_backend(
        FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-test"),
        backend.clone(),
    )
    .unwrap();
    reopened.bind_runtime_identity("runtime-test").unwrap();
    assert_eq!(
        reopened
            .capture_subject_release_session_scoped(&scope, worker.worker_id, &capture_request)
            .unwrap(),
        captured,
        "saved union validation requires no restored Controller",
    );
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(matches!(
        reopened.subject_release_scoped(&scope, request.clone()),
        Err(crate::error::RuntimeError::RecordingExecutionUnconfirmed { .. })
    ));
    assert!(owner.release_fence_path().exists());
    assert!(
        owner.lock_publication().is_err(),
        "restart must retain publication exclusion"
    );
    assert!(
        reopened
            .send_input(&worker.worker_ref, WorkerInput::user("must remain held"))
            .is_err()
    );
    request.action = Action::Cancel;
    reopened
        .subject_release_scoped(&scope, request.clone())
        .unwrap();
    assert!(!owner.release_fence_path().exists());
    assert!(
        reopened
            .capture_subject_release_session_scoped(&scope, worker.worker_id, &capture_request)
            .is_err()
    );
    assert!(owner.lock_publication().is_ok());
    assert!(
        reopened
            .subject_release_scoped(
                &scope,
                runtime_api::SubjectReleaseRequest {
                    action: Action::Reserve,
                    ..request
                }
            )
            .is_err(),
        "cancelled operation cannot regain authority after restart"
    );
}

#[test]
#[serial_test::serial(worker_allocation)]
fn subject_release_memory_source_unavailable_keeps_actual_reservation_until_cancel() {
    let fixture = ReleaseFixture::new(true);
    let mut request = fixture.request();
    let result = fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone());
    assert!(
        matches!(
            result,
            Err(crate::error::RuntimeError::RecordingSourceUnavailable { .. })
        ),
        "{result:?}"
    );
    assert!(
        fixture
            .runtime
            .send_input(
                &fixture.worker.worker_ref,
                WorkerInput::user("not admitted")
            )
            .is_err()
    );
    let reservations = fixture.backend.subject_reservations.lock().unwrap();
    let saved = reservations
        .get(&(
            fixture.worker.worker_ref.clone(),
            request.operation_id.clone(),
        ))
        .unwrap();
    assert!(
        saved.guard.is_some(),
        "failed scope response must not cancel actual HeldReservation"
    );
    drop(reservations);
    request.action = runtime_api::SubjectReleaseAction::Cancel;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request)
        .unwrap();
    fixture
        .runtime
        .stop_worker(&fixture.worker.worker_ref, None)
        .unwrap();
}

#[test]
#[serial_test::serial(worker_allocation)]
fn subject_release_missing_retained_source_holds_typed_cause_and_fence_until_exact_cancel() {
    let fixture = ReleaseFixture::new(false);
    let owner = fixture.owner();
    let producer = session_store::FsStore::new_runtime_owned(
        owner.clone(),
        crate::retention::runtime_session_retention(fixture.root.path()),
    )
    .unwrap();
    let session = session_store::new_session_id();
    let segment = session_store::new_segment_id();
    producer
        .create_segment(
            session,
            segment,
            &[seed(session, "producer-owned retained material")],
        )
        .unwrap();
    // Source-unavailable fault injection keeps the canonical producer binding.
    std::fs::remove_file(
        owner
            .retained_root()
            .join(session.to_string())
            .join(format!("{segment}.jsonl")),
    )
    .unwrap();
    let mut request = fixture.request();
    let result = fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone());
    assert!(
        matches!(
            result,
            Err(crate::error::RuntimeError::RecordingSourceUnavailable { .. })
        ),
        "{result:?}"
    );
    assert!(owner.release_fence_path().exists());
    assert!(
        producer
            .create_segment(session, segment, &[seed(session, "repair")])
            .is_err(),
        "scope failure must retain actual reservation and publication exclusion"
    );
    request.action = runtime_api::SubjectReleaseAction::Cancel;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request.clone())
        .unwrap();
    assert!(!owner.release_fence_path().exists());
    producer
        .create_segment(
            session,
            segment,
            &[seed(session, "explicit producer repair after cancellation")],
        )
        .unwrap();
    request.action = runtime_api::SubjectReleaseAction::Reserve;
    assert!(
        fixture
            .runtime
            .subject_release_scoped(&fixture.scope, request.clone())
            .is_err()
    );
    request.operation_id = "release-after-repair".into();
    assert_eq!(
        fixture
            .runtime
            .subject_release_scoped(&fixture.scope, request.clone())
            .unwrap()
            .recording_scope
            .unwrap()
            .members
            .len(),
        2
    );
    request.action = runtime_api::SubjectReleaseAction::Cancel;
    fixture
        .runtime
        .subject_release_scoped(&fixture.scope, request)
        .unwrap();
    fixture
        .runtime
        .stop_worker(&fixture.worker.worker_ref, None)
        .unwrap();
}
