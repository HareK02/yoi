//! Runtime/Session-store/retention and signed HTTP/generated-client integration.
//! Production catalog, filesystem stores, canonical reader, router and signature
//! verifier are connected. Only controller execution is replaced by AcceptingBackend;
//! no model, external server, user credentials or running Runtime is used.
use super::*;
use crate::retention::{DiagnosticsDisposition, SessionDisposition, runtime_session_retention};
use runtime_api::{
    SessionMaterialCatalogAvailability as Catalog, SessionMaterialReadAvailability as Read,
    SessionPublicSource as Source, SessionPublicUnavailableReason as Reason,
};
use session_store::retention::{SessionDeletionIntent, SessionRetentionPin};
use session_store::{
    LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
    LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, Store,
    WorkerAggregateStore, WorkerMetadataStore, WorkerSessionStore,
};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Duration;

struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    scope: RuntimeWorkspaceScope,
    worker: WorkerRef,
    session: session_store::SessionId,
    segment: session_store::SegmentId,
    store: WorkerSessionStore,
}

fn message(id: &str, role: LoggedRole, text: &str) -> LoggedHistoryEntry {
    LoggedHistoryEntry {
        item: LoggedItem::Message {
            role,
            content: vec![LoggedContentPart::Text { text: text.into() }],
        },
        metadata: LoggedSessionHistoryMetadata {
            system_input: None,
            entry_id: LoggedSessionHistoryEntryId(id.into()),
            origin: LoggedSessionHistoryOrigin::LocalHumanInput,
            derivation: None,
        },
    }
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test"),
            Arc::new(AcceptingBackend),
        )
        .unwrap();
        runtime.bind_runtime_identity("runtime-test").unwrap();
        let scope = RuntimeWorkspaceScope::new("workspace-a", "https://backend.test");
        let mut request = task_request("canonical Session fixture");
        request.config_bundle = None;
        request.memory_settings.as_mut().unwrap().workspace_id = scope.workspace_id.clone();
        let worker = runtime
            .create_worker_scoped(&scope, request)
            .unwrap()
            .worker_ref;
        let aggregate = temp
            .path()
            .join("workers")
            .join(worker.worker_id.to_string());
        let session = session_store::new_session_id();
        let segment = session_store::new_segment_id();
        let store = WorkerSessionStore::new_with_retention(
            aggregate.join("session"),
            runtime_session_retention(temp.path()),
        )
        .unwrap();
        store
            .create_segment(
                session,
                segment,
                &[
                    LogEntry::AnnotatedSegmentStart {
                        ts: 1,
                        session_id: session,
                        system_prompt: Some("private-system-prompt".into()),
                        config: agen::llm_client::RequestConfig::default(),
                        history: vec![],
                        forked_from: None,
                        compacted_from: None,
                    },
                    LogEntry::Invoke {
                        ts: 2,
                        trigger: protocol::InvokeKind::UserSend,
                    },
                    LogEntry::AnnotatedUserInput {
                        ts: 3,
                        segments: vec![],
                        extensions: vec![],
                        history: vec![message("user-1", LoggedRole::User, "公開の依頼")],
                    },
                    LogEntry::AnnotatedAssistantItem {
                        ts: 4,
                        entry: message("answer-1", LoggedRole::Assistant, "公開の回答🍎"),
                    },
                    LogEntry::RunCancelled {
                        ts: 5,
                        entry_id: None,
                    },
                ],
            )
            .unwrap();
        WorkerAggregateStore::new(&aggregate, format!("worker-runtime-{}", worker.worker_id))
            .unwrap()
            .write(
                &session_store::WorkerMetadata::new(
                    format!("worker-runtime-{}", worker.worker_id),
                    Some(session_store::WorkerActiveSegmentRef::active_segment(
                        session, segment,
                    )),
                )
                .with_workspace_id(&scope.workspace_id),
            )
            .unwrap();
        Self {
            temp,
            runtime,
            scope,
            worker,
            session,
            segment,
            store,
        }
    }
    fn root(&self) -> PathBuf {
        self.temp
            .path()
            .join("workers")
            .join(self.worker.worker_id.to_string())
            .join("session")
    }
    fn source(&self) -> Source {
        Source::Retained {
            worker_id: self.worker.worker_id,
        }
    }
    fn request(&self) -> runtime_api::SessionMaterialCatalogRequest {
        runtime_api::SessionMaterialCatalogRequest {
            workspace_id: self.scope.workspace_id.clone(),
            source: self.source(),
            expected_session_id: self.session.to_string(),
            expected_catalog_sha256: None,
            offset: 0,
            limit: 1,
        }
    }
    fn catalog(&self) -> runtime_api::SessionMaterialCatalogPage {
        match self
            .runtime
            .session_material_catalog_scoped(&self.scope, &self.request())
        {
            Catalog::Page { page } => page,
            other => panic!("{other:?}"),
        }
    }
    fn stop(&self) {
        self.runtime
            .stop_worker_scoped(&self.scope, &self.worker, None)
            .unwrap();
    }
    fn retention_request(
        &self,
        disposition: SessionDisposition,
    ) -> WorkerRetentionExecutionRequest {
        WorkerRetentionExecutionRequest {
            operation_id: "remove-source".into(),
            input_fingerprint: "source-and-archive".into(),
            archive_id: (disposition == SessionDisposition::Archive)
                .then(|| "canonical-archive".into()),
            workspace_id: self.scope.workspace_id.clone(),
            source_runtime_id: "runtime-test".into(),
            worker_id: self.worker.worker_id,
            source_created_at: "test".into(),
            removed_at: "test".into(),
            effective_profile: None,
            retention_class: None,
            policy_id: "test-policy".into(),
            session_disposition: disposition,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
        }
    }
    fn reader(
        &self,
        source: &Source,
    ) -> session_store::public_material::SessionPublicMaterialReader {
        self.runtime
            .session_public_material_reader_scoped(&self.scope, source, &self.session.to_string())
            .unwrap()
    }
}

#[test]
fn pending_session_proof_is_not_missing_material_and_pin_release_allows_empty_archive() {
    let f = Fixture::new();
    std::fs::remove_dir_all(f.root()).unwrap();
    let aggregate = f.root().parent().unwrap().to_path_buf();
    let metadata =
        WorkerAggregateStore::new(&aggregate, format!("worker-runtime-{}", f.worker.worker_id))
            .unwrap();
    metadata
        .write(
            &session_store::WorkerMetadata::new(
                format!("worker-runtime-{}", f.worker.worker_id),
                Some(session_store::WorkerActiveSegmentRef::pending_segment(
                    f.session,
                )),
            )
            .with_workspace_id(&f.scope.workspace_id),
        )
        .unwrap();
    let pin = SessionRetentionPin(uuid::Uuid::now_v7());
    let retention = runtime_session_retention(f.temp.path());
    retention.retain_before_publication(f.session, pin).unwrap();
    assert!(
        matches!(f.runtime.session_material_catalog_scoped(&f.scope, &f.request()), Catalog::NotMaterialized { session_id } if session_id == f.session.to_string())
    );
    assert!(
        f.runtime
            .pin_session_public_source_scoped(&f.scope, &f.source(), &f.session.to_string(), pin)
            .is_err(),
        "retention intent is not source availability"
    );
    // A missing committed source is never the same fact as a pending pointer.
    metadata
        .write(
            &session_store::WorkerMetadata::new(
                format!("worker-runtime-{}", f.worker.worker_id),
                Some(session_store::WorkerActiveSegmentRef::active_segment(
                    f.session, f.segment,
                )),
            )
            .with_workspace_id(&f.scope.workspace_id),
        )
        .unwrap();
    assert!(matches!(
        f.runtime
            .session_material_catalog_scoped(&f.scope, &f.request()),
        Catalog::Unavailable { .. }
    ));
    metadata
        .write(
            &session_store::WorkerMetadata::new(
                format!("worker-runtime-{}", f.worker.worker_id),
                Some(session_store::WorkerActiveSegmentRef::pending_segment(
                    f.session,
                )),
            )
            .with_workspace_id(&f.scope.workspace_id),
        )
        .unwrap();
    // Any unexpected source file invalidates the empty proof, too.
    std::fs::create_dir_all(f.root()).unwrap();
    std::fs::write(f.root().join("unattested-original"), "do not discard").unwrap();
    assert!(matches!(
        f.runtime
            .session_material_catalog_scoped(&f.scope, &f.request()),
        Catalog::Unavailable { .. }
    ));
    std::fs::remove_file(f.root().join("unattested-original")).unwrap();
    f.runtime
        .release_session_public_source_pin_scoped(
            &f.scope,
            &f.source(),
            &f.session.to_string(),
            pin,
        )
        .unwrap();
    assert!(retention.pins(f.session).unwrap().is_empty());
    f.stop();
    let inventory = f
        .runtime
        .worker_retention_inventory(&f.scope.workspace_id, &f.worker)
        .unwrap();
    assert_eq!(inventory.session_id, None);
    assert_eq!(inventory.session_bytes, 0);
    let removed = f
        .runtime
        .execute_worker_retention(&f.retention_request(SessionDisposition::Archive))
        .unwrap();
    let archive = removed.archive.unwrap();
    assert!(removed.source_removed);
    assert_eq!(archive.source_session_id, f.session.to_string());
    assert_eq!(archive.content_file_count, 0);
    assert!(archive.source_active_segment_id.is_none());
}

#[test]
fn canonical_pages_read_frozen_originals_after_tail_append_and_reject_forged_descriptors() {
    let f = Fixture::new();
    let first = f.catalog();
    assert_eq!(first.descriptors.len(), 1);
    assert!(first.next_offset.is_some());
    let metadata_json = serde_json::to_string(&first).unwrap();
    assert!(!metadata_json.contains("公開の依頼"));
    assert!(!metadata_json.contains("private-system-prompt"));
    let mut continuation = f.request();
    continuation.offset = first.next_offset.unwrap();
    continuation.expected_catalog_sha256 = Some(first.catalog_sha256.clone());
    let second = match f
        .runtime
        .session_material_catalog_scoped(&f.scope, &continuation)
    {
        Catalog::Page { page } => page,
        other => panic!("{other:?}"),
    };
    assert_eq!(second.descriptors[0].canonical_order, 1);
    continuation.offset = second
        .next_offset
        .expect("saved cancellation boundary is catalogued");
    let boundary = match f
        .runtime
        .session_material_catalog_scoped(&f.scope, &continuation)
    {
        Catalog::Page { page } => page,
        other => panic!("{other:?}"),
    };
    assert!(boundary.next_offset.is_none());
    let boundary_origin: serde_json::Value =
        serde_json::from_str(&boundary.descriptors[0].origin_json).unwrap();
    assert_eq!(boundary_origin["kind"], "host_run_boundary");
    let boundary_read = f.runtime.session_material_read_scoped(
        &f.scope,
        &runtime_api::SessionMaterialReadRequest {
            workspace_id: f.scope.workspace_id.clone(),
            source: f.source(),
            expected_session_id: f.session.to_string(),
            expected_descriptor: boundary.descriptors[0].clone(),
            byte_offset: 0,
            max_content_bytes: 16384,
        },
    );
    let Read::Page {
        page: boundary_text,
    } = boundary_read
    else {
        panic!("{boundary_read:?}")
    };
    assert!(boundary_text.content.contains("cancelled"));
    let expected = second.descriptors[0].clone();
    f.store
        .append(
            f.session,
            f.segment,
            &LogEntry::AnnotatedAssistantItem {
                ts: 6,
                entry: message("tail", LoggedRole::Assistant, "new tail, not Job input"),
            },
        )
        .unwrap();
    assert!(matches!(
        f.runtime
            .session_material_catalog_scoped(&f.scope, &continuation),
        Catalog::Unavailable {
            reason: Reason::InvalidCursor,
            ..
        }
    ));
    let mut request = runtime_api::SessionMaterialReadRequest {
        workspace_id: f.scope.workspace_id.clone(),
        source: f.source(),
        expected_session_id: f.session.to_string(),
        expected_descriptor: expected.clone(),
        byte_offset: 0,
        max_content_bytes: 5,
    };
    let mut body = String::new();
    while request.byte_offset < expected.total_bytes {
        let page = match f.runtime.session_material_read_scoped(&f.scope, &request) {
            Read::Page { page } => page,
            other => panic!("{other:?}"),
        };
        assert_eq!(page.descriptor, expected);
        assert!(page.content.len() <= 5);
        assert!(page.next_offset > request.byte_offset);
        body.push_str(&page.content);
        request.byte_offset = page.next_offset;
    }
    assert_eq!(body.len() as u64, expected.total_bytes);
    assert_eq!(
        crate::workspace_issuer::hex_lower(&sha2::Sha256::digest(body.as_bytes())),
        expected.sha256
    );
    assert!(body.contains("公開の回答🍎"));
    assert!(!body.contains("new tail"));
    request.byte_offset = body.find('公').unwrap() as u64 + 1;
    assert!(matches!(
        f.runtime.session_material_read_scoped(&f.scope, &request),
        Read::Unavailable {
            reason: Reason::InvalidCursor,
            ..
        }
    ));
    request.byte_offset = 0;
    for forged in 0..5 {
        request.expected_descriptor = expected.clone();
        match forged {
            0 => request.expected_descriptor.sha256 = "forged".into(),
            1 => request.expected_descriptor.canonical_order += 1,
            2 => request.expected_descriptor.total_bytes += 1,
            3 => request.expected_descriptor.origin_json = "{}".into(),
            _ => {
                request.expected_descriptor.segment_id = session_store::new_segment_id().to_string()
            }
        }
        assert!(matches!(
            f.runtime.session_material_read_scoped(&f.scope, &request),
            Read::Unavailable { .. }
        ));
    }
}

#[test]
fn source_scope_session_and_active_pointer_mismatches_never_return_material() {
    let f = Fixture::new();
    assert!(
        f.runtime
            .session_public_material_reader_scoped(
                &RuntimeWorkspaceScope::new("other", "server"),
                &f.source(),
                &f.session.to_string()
            )
            .is_err()
    );
    assert!(
        f.runtime
            .session_public_material_reader_scoped(
                &f.scope,
                &f.source(),
                &session_store::new_session_id().to_string()
            )
            .is_err()
    );
    let mut request = f.request();
    request.workspace_id = "other".into();
    assert!(matches!(
        f.runtime
            .session_material_catalog_scoped(&f.scope, &request),
        Catalog::Unavailable { .. }
    ));
    let metadata = WorkerAggregateStore::new(
        f.root().parent().unwrap(),
        format!("worker-runtime-{}", f.worker.worker_id),
    )
    .unwrap();
    metadata
        .set_active(
            &format!("worker-runtime-{}", f.worker.worker_id),
            Some(session_store::WorkerActiveSegmentRef::active_segment(
                session_store::new_session_id(),
                f.segment,
            )),
            None,
        )
        .unwrap();
    assert!(
        f.runtime
            .session_public_material_reader_scoped(&f.scope, &f.source(), &f.session.to_string())
            .is_err()
    );
    assert!(
        f.runtime
            .session_public_material_reader_scoped(
                &f.scope,
                &Source::Archived {
                    archive_id: "../escape".into()
                },
                &f.session.to_string()
            )
            .is_err()
    );
}

#[test]
fn pinned_stopped_worker_archives_before_removal_and_job_reads_archived_source() {
    let f = Fixture::new();
    let original: Vec<_> = f.reader(&f.source()).descriptors().cloned().collect();
    let pin = SessionRetentionPin(uuid::Uuid::now_v7());
    f.runtime
        .pin_session_public_source_scoped(&f.scope, &f.source(), &f.session.to_string(), pin)
        .unwrap();
    f.stop();
    assert!(f.runtime.delete_worker_scoped(&f.scope, &f.worker).is_err());
    assert!(f.root().is_dir());
    assert!(
        f.runtime
            .execute_worker_retention(&f.retention_request(SessionDisposition::Purge))
            .is_err()
    );
    assert!(f.root().is_dir());
    // Use another operation identity after the deliberately different Purge request.
    let mut request = f.retention_request(SessionDisposition::Archive);
    request.operation_id = "archive-source".into();
    let result = f.runtime.execute_worker_retention(&request).unwrap();
    assert!(result.source_removed);
    assert_eq!(
        result.archive.as_ref().unwrap().source_active_segment_id,
        Some(f.segment)
    );
    assert_eq!(
        f.runtime.execute_worker_retention(&request).unwrap(),
        result
    );
    assert!(!f.root().exists());
    assert!(f.runtime.worker_detail(&f.worker).is_err());
    assert!(
        f.runtime
            .session_public_material_reader_scoped(&f.scope, &f.source(), &f.session.to_string())
            .is_err()
    );
    let archived = Source::Archived {
        archive_id: "canonical-archive".into(),
    };
    let reader = f.reader(&archived);
    assert_eq!(reader.descriptors().cloned().collect::<Vec<_>>(), original);
    let body = reader.read_content(&original[0], 0, 16384).unwrap();
    assert!(body.content.contains("公開の依頼"));
    assert!(
        f.runtime
            .delete_session_public_source_scoped(
                &f.scope,
                &archived,
                &f.session.to_string(),
                SessionDeletionIntent::Ordinary
            )
            .is_err()
    );
    f.runtime
        .release_session_public_source_pin_scoped(&f.scope, &archived, &f.session.to_string(), pin)
        .unwrap();
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &archived,
            &f.session.to_string(),
            SessionDeletionIntent::Ordinary,
        )
        .unwrap();
    assert!(
        f.runtime
            .pin_session_public_source_scoped(&f.scope, &archived, &f.session.to_string(), pin)
            .is_err()
    );
    assert!(f.temp.path().join("retention/session-sources").is_dir());
}

#[test]
fn explicit_erase_invalidates_pins_and_fences_old_readers_publishers_and_retry() {
    let f = Fixture::new();
    let pin = SessionRetentionPin(uuid::Uuid::now_v7());
    f.runtime
        .pin_session_public_source_scoped(&f.scope, &f.source(), &f.session.to_string(), pin)
        .unwrap();
    let reader = f.reader(&f.source());
    let descriptor = reader.descriptors().next().unwrap().clone();
    f.stop();
    let unrelated = session_store::new_session_id();
    assert!(
        f.runtime
            .delete_session_public_source_scoped(
                &f.scope,
                &f.source(),
                &unrelated.to_string(),
                SessionDeletionIntent::ErasePermanently
            )
            .is_err()
    );
    runtime_session_retention(f.temp.path())
        .ensure_live(unrelated)
        .unwrap();
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &f.source(),
            &f.session.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    assert!(!f.root().exists());
    assert!(matches!(
        reader.read_content(&descriptor, 0, 64),
        Err(session_store::public_material::PublicMaterialError::Erased)
    ));
    assert!(matches!(
        f.store.create_segment(f.session, f.segment, &[]),
        Err(session_store::StoreError::SessionErased(_))
    ));
    assert!(matches!(
        f.store.append(
            f.session,
            f.segment,
            &LogEntry::TurnEnd {
                ts: 10,
                turn_count: 1
            }
        ),
        Err(session_store::StoreError::SessionErased(_))
    ));
    assert!(
        runtime_session_retention(f.temp.path())
            .pins(f.session)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.runtime
            .pin_session_public_source_scoped(&f.scope, &f.source(), &f.session.to_string(), pin)
            .is_err()
    );
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &f.source(),
            &f.session.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    // Ordinary registration removal can resume a pre-existing permanent erase
    // decision. It cannot establish one or recreate original Session material.
    f.runtime
        .delete_worker_with_intent_scoped(&f.scope, &f.worker, SessionDeletionIntent::Ordinary)
        .unwrap();
    assert!(!f.root().parent().unwrap().exists());
    assert!(matches!(
        reader.read_content(&descriptor, 0, 64),
        Err(session_store::public_material::PublicMaterialError::Erased)
    ));
}

#[test]
fn catalog_rejects_oversize_record_with_explicit_resource_limit() {
    let f = Fixture::new();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(
            f.root()
                .join("segments")
                .join(format!("{}.jsonl", f.segment)),
        )
        .unwrap();
    let start = file.seek(SeekFrom::End(0)).unwrap();
    file.set_len(start + 64 * 1024 * 1024 + 1).unwrap();
    file.write_all(b"\n").unwrap();
    assert!(matches!(
        f.runtime
            .session_material_catalog_scoped(&f.scope, &f.request()),
        Catalog::Unavailable {
            reason: Reason::ResourceLimit,
            ..
        }
    ));
}

#[test]
fn archive_scope_and_legacy_active_pointer_are_checked_and_erase_retry_never_restores_originals() {
    let f = Fixture::new();
    f.stop();
    let mut request = f.retention_request(SessionDisposition::Archive);
    request.diagnostics_disposition = DiagnosticsDisposition::Retain;
    let log = f
        .root()
        .parent()
        .unwrap()
        .join("runs/run-original/worker.out.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, "archived diagnostic original").unwrap();
    let original_receipt = f.runtime.execute_worker_retention(&request).unwrap();
    assert!(original_receipt.diagnostics_retained);
    let diagnostics = f
        .temp
        .path()
        .join("archives/diagnostics")
        .join(&request.operation_id);
    assert!(
        diagnostics
            .join("runs/run-original/worker.out.log")
            .is_file()
    );
    let staged_diagnostics =
        diagnostics.with_file_name(format!(".{}.tmp-42-1", request.operation_id));
    std::fs::create_dir_all(&staged_diagnostics).unwrap();
    std::fs::write(
        staged_diagnostics.join("partial-original.log"),
        "incomplete copy",
    )
    .unwrap();
    let unrelated = diagnostics.with_file_name("another-operation");
    std::fs::create_dir_all(&unrelated).unwrap();
    std::fs::write(unrelated.join("untouched.log"), "unrelated diagnostic").unwrap();
    let source = Source::Archived {
        archive_id: "canonical-archive".into(),
    };
    let reader = f.reader(&source);
    let descriptor = reader.descriptors().next().unwrap().clone();
    let manifest_path = f
        .temp
        .path()
        .join("archives/workers/canonical-archive/manifest.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    for field in [
        "workspace_id",
        "source_runtime_id",
        "source_active_segment_id",
    ] {
        let mut changed = original.clone();
        if field == "source_active_segment_id" {
            changed.as_object_mut().unwrap().remove(field);
        } else {
            changed[field] = serde_json::json!("other");
        }
        std::fs::write(&manifest_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            f.runtime
                .session_public_material_reader_scoped(&f.scope, &source, &f.session.to_string())
                .is_err()
        );
    }
    std::fs::write(&manifest_path, serde_json::to_vec(&original).unwrap()).unwrap();
    let pin = SessionRetentionPin(uuid::Uuid::now_v7());
    f.runtime
        .pin_session_public_source_scoped(&f.scope, &source, &f.session.to_string(), pin)
        .unwrap();
    let root = f
        .temp
        .path()
        .join("archives/workers/canonical-archive/session");
    let staged = f
        .temp
        .path()
        .join("archives/workers/.interrupted-transfer/session");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::copy(
        &manifest_path,
        staged.parent().unwrap().join("manifest.json"),
    )
    .unwrap();
    std::fs::write(
        staged.join("partial-original.jsonl"),
        "partially copied original",
    )
    .unwrap();
    assert!(!staged.join("session.json").exists());
    // Simulate the documented fence-before-physical-removal failure boundary.
    let retention = runtime_session_retention(f.temp.path());
    assert!(
        retention
            .delete(
                f.session,
                SessionDeletionIntent::ErasePermanently,
                || Err::<(), _>(session_store::StoreError::Io(std::io::Error::other(
                    "injected erase failure"
                )))
            )
            .is_err()
    );
    assert!(root.exists());
    assert!(matches!(
        reader.read_content(&descriptor, 0, 64),
        Err(session_store::public_material::PublicMaterialError::Erased)
    ));
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &source,
            &f.session.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &source,
            &f.session.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    assert!(!root.exists());
    assert!(!staged.exists());
    assert!(
        f.runtime
            .pin_session_public_source_scoped(&f.scope, &source, &f.session.to_string(), pin)
            .is_err()
    );
    // An old receipt is historical evidence, not permission to republish erased bytes.
    assert_eq!(
        f.runtime.execute_worker_retention(&request).unwrap(),
        original_receipt
    );
    assert!(!diagnostics.exists());
    assert!(!staged_diagnostics.exists());
    assert!(unrelated.join("untouched.log").is_file());
    assert!(!root.exists());
    assert!(retention.pins(f.session).unwrap().is_empty());
}

#[test]
fn erasure_rejects_mismatched_archived_diagnostics_then_resumes_under_the_same_fence() {
    let f = Fixture::new();
    f.stop();
    let mut request = f.retention_request(SessionDisposition::Archive);
    request.diagnostics_disposition = DiagnosticsDisposition::Retain;
    let receipt = f.runtime.execute_worker_retention(&request).unwrap();
    let source = Source::Archived {
        archive_id: request.archive_id.clone().unwrap(),
    };
    let manifest = f
        .temp
        .path()
        .join("archives/diagnostics")
        .join(&request.operation_id)
        .join("manifest.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    for field in ["workspace_id", "input_fingerprint", "source_worker_id"] {
        let mut changed = original.clone();
        changed[field] = serde_json::json!(uuid::Uuid::now_v7().to_string());
        std::fs::write(&manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            f.runtime
                .delete_session_public_source_scoped(
                    &f.scope,
                    &source,
                    &f.session.to_string(),
                    SessionDeletionIntent::ErasePermanently
                )
                .is_err()
        );
        assert!(manifest.exists());
        assert!(matches!(
            runtime_session_retention(f.temp.path()).ensure_live(f.session),
            Err(session_store::StoreError::SessionErased(_))
        ));
    }
    std::fs::write(&manifest, serde_json::to_vec(&original).unwrap()).unwrap();
    f.runtime
        .delete_session_public_source_scoped(
            &f.scope,
            &source,
            &f.session.to_string(),
            SessionDeletionIntent::ErasePermanently,
        )
        .unwrap();
    assert!(!manifest.parent().unwrap().exists());
    assert_eq!(
        f.runtime.execute_worker_retention(&request).unwrap(),
        receipt
    );
    assert!(
        receipt.diagnostics_retained,
        "historical receipt must not be rewritten after erasure"
    );
}

#[test]
fn material_request_bounds_return_explicit_unavailable_without_previews() {
    let f = Fixture::new();
    for limit in [0, 101] {
        let mut request = f.request();
        request.limit = limit;
        assert!(matches!(
            f.runtime
                .session_material_catalog_scoped(&f.scope, &request),
            Catalog::Unavailable {
                reason: Reason::ResourceLimit,
                ..
            }
        ));
    }
    let descriptor = f.catalog().descriptors[0].clone();
    for max_content_bytes in [0, 3, 16385] {
        let request = runtime_api::SessionMaterialReadRequest {
            workspace_id: f.scope.workspace_id.clone(),
            source: f.source(),
            expected_session_id: f.session.to_string(),
            expected_descriptor: descriptor.clone(),
            byte_offset: 0,
            max_content_bytes,
        };
        assert!(matches!(
            f.runtime.session_material_read_scoped(&f.scope, &request),
            Read::Unavailable {
                reason: Reason::ResourceLimit,
                ..
            }
        ));
    }
}

#[derive(Clone)]
struct SignedAuthorizer {
    identity: RuntimeIdentityMaterial,
    trust: WorkspaceIssuerTrustRecord,
}
impl runtime_api::client_support::RequestAuthorizer for SignedAuthorizer {
    fn authorize(
        &self,
        request: runtime_api::client_support::AuthorizerRequest<'_>,
    ) -> Result<
        runtime_api::client_support::framework::header::HeaderMap,
        runtime_api::client_support::AuthorizationError,
    > {
        let claims = signed_claims(
            &self.trust,
            request.method.as_str(),
            request.path_and_query,
            request.body,
        );
        let token =
            issue_workspace_capability_token(&self.identity.signing_key().unwrap(), &claims)
                .unwrap();
        let mut headers = runtime_api::client_support::framework::header::HeaderMap::new();
        headers.insert("x-yoi-workspace-id", "workspace-a".parse().unwrap());
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        Ok(headers)
    }
}
struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn signed_generated_client_reads_canonical_pages_and_pins_with_real_router() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let f = Fixture::new();
        let identity = RuntimeIdentityMaterial::generate("canonical-client-key").unwrap();
        let trust = workspace_trust(&identity);
        let app = runtime_http_router_with_workspace_auth(
            f.runtime.clone(),
            None,
            workspace_test_auth(vec![trust.clone()]),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = ServerTask(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        let client = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .authorizer(SignedAuthorizer { identity, trust })
            .build()
            .unwrap();
        let page = match client.session_material_catalog(f.request()).await.unwrap() {
            Catalog::Page { page } => page,
            other => panic!("{other:?}"),
        };
        let descriptor = page.descriptors[0].clone();
        let read = client
            .session_material_read(runtime_api::SessionMaterialReadRequest {
                workspace_id: "workspace-a".into(),
                source: f.source(),
                expected_session_id: f.session.to_string(),
                expected_descriptor: descriptor.clone(),
                byte_offset: 0,
                max_content_bytes: 16384,
            })
            .await
            .unwrap();
        let Read::Page { page } = read else {
            panic!("{read:?}")
        };
        assert_eq!(page.descriptor, descriptor);
        assert_eq!(page.next_offset, descriptor.total_bytes);
        assert!(page.content.contains("公開の依頼"));
        let pin = runtime_api::SessionMaterialPinRequest {
            workspace_id: "workspace-a".into(),
            source: f.source(),
            expected_session_id: f.session.to_string(),
            pin_id: uuid::Uuid::now_v7().to_string(),
        };
        assert_eq!(
            client
                .session_material_pin(pin.clone())
                .await
                .unwrap()
                .pin_id,
            pin.pin_id
        );
        assert_eq!(
            runtime_session_retention(f.temp.path())
                .pins(f.session)
                .unwrap()
                .len(),
            1
        );
        client.session_material_release(pin.clone()).await.unwrap();
        assert!(
            runtime_session_retention(f.temp.path())
                .pins(f.session)
                .unwrap()
                .is_empty()
        );
        client.session_material_pin(pin).await.unwrap();
        f.stop();
        let erase = runtime_api::SessionMaterialEraseRequest {
            workspace_id: f.scope.workspace_id.clone(),
            source: f.source(),
            expected_session_id: f.session.to_string(),
        };
        for _ in 0..2 {
            assert_eq!(
                client
                    .session_material_erase(erase.clone())
                    .await
                    .unwrap()
                    .session_id,
                f.session.to_string()
            );
        }
        assert!(!f.root().exists());
        assert!(matches!(
            client.session_material_catalog(f.request()).await.unwrap(),
            Catalog::Unavailable { .. }
        ));
        assert!(matches!(
            runtime_session_retention(f.temp.path()).ensure_live(f.session),
            Err(session_store::StoreError::SessionErased(_))
        ));
        // Register/lifecycle cleanup after erasure must not need a fresh archive or Job.
        let result = f
            .runtime
            .execute_worker_retention(&f.retention_request(SessionDisposition::Purge))
            .unwrap();
        assert!(result.source_removed);
        assert!(result.archive.is_none());
        assert!(!f.root().parent().unwrap().exists());
    })
    .await
    .expect("bounded signed client integration");
}

#[tokio::test]
async fn canonical_http_rejects_replay_tampered_body_and_workspace_mismatch() {
    let f = Fixture::new();
    let identity = RuntimeIdentityMaterial::generate("canonical-key").unwrap();
    let trust = workspace_trust(&identity);
    let app = runtime_http_router_with_workspace_auth(
        f.runtime.clone(),
        None,
        workspace_test_auth(vec![trust.clone()]),
    );
    let path = "/v1/session-public/material/catalog";
    let body = serde_json::to_vec(&f.request()).unwrap();
    let claims = signed_claims(&trust, "POST", path, &body);
    let first = signed_request(app.clone(), &identity, &claims, body.clone(), "workspace-a").await;
    assert_eq!(first.status(), StatusCode::OK);
    let replay = signed_request(app.clone(), &identity, &claims, body.clone(), "workspace-a").await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    let claims = signed_claims(&trust, "POST", path, &body);
    let changed = signed_request(
        app.clone(),
        &identity,
        &claims,
        b"{}".to_vec(),
        "workspace-a",
    )
    .await;
    assert_eq!(changed.status(), StatusCode::UNAUTHORIZED);
    let mut wrong_scope = f.request();
    wrong_scope.workspace_id = "other-workspace".into();
    let wrong_body = serde_json::to_vec(&wrong_scope).unwrap();
    let claims = signed_claims(&trust, "POST", path, &wrong_body);
    let response = signed_request(app.clone(), &identity, &claims, wrong_body, "workspace-a").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let unsigned = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn subject_create_recovery_signed_http_requires_delete_and_matching_workspace() {
    let f = Fixture::new();
    let identity = RuntimeIdentityMaterial::generate("recovery-key").unwrap();
    let trust = workspace_trust(&identity);
    let app = runtime_http_router_with_workspace_auth(
        f.runtime.clone(),
        None,
        workspace_test_auth(vec![trust.clone()]),
    );
    let path = "/v1/subject-create-recovery";
    let mut request = runtime_api::SubjectCreateRecoveryRequest {
        workspace_id: f.scope.workspace_id.clone(),
        worker_id: WorkerId::now_v7(),
        create_fingerprint: "unpublished-create".into(),
        operation_id: "recovery-1".into(),
    };
    for operation in ["workers:read", "workers:stop", "workers:create"] {
        let body = serde_json::to_vec(&request).unwrap();
        let mut claims = signed_claims(&trust, "POST", path, &body);
        claims.operation = operation.into();
        let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    request.workspace_id = "other-workspace".into();
    let body = serde_json::to_vec(&request).unwrap();
    let claims = signed_claims(&trust, "POST", path, &body);
    let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    request.workspace_id = f.scope.workspace_id.clone();
    for _ in 0..2 {
        let body = serde_json::to_vec(&request).unwrap();
        let claims = signed_claims(&trust, "POST", path, &body);
        assert_eq!(claims.operation, "workers:delete");
        let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let receipt: runtime_api::SubjectCreateRecoveryResponse =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(receipt.worker_id, request.worker_id);
        assert_eq!(receipt.workspace_id, request.workspace_id);
        assert_eq!(receipt.create_fingerprint, request.create_fingerprint);
        assert_eq!(receipt.operation_id, request.operation_id);
    }
}

#[tokio::test]
async fn explicit_material_erasure_requires_delete_capability_and_matching_scope() {
    let f = Fixture::new();
    f.stop();
    let identity = RuntimeIdentityMaterial::generate("erase-key").unwrap();
    let trust = workspace_trust(&identity);
    let app = runtime_http_router_with_workspace_auth(
        f.runtime.clone(),
        None,
        workspace_test_auth(vec![trust.clone()]),
    );
    let path = "/v1/session-public/material/retention/erase";
    let mut request = runtime_api::SessionMaterialEraseRequest {
        workspace_id: f.scope.workspace_id.clone(),
        source: f.source(),
        expected_session_id: f.session.to_string(),
    };
    let body = serde_json::to_vec(&request).unwrap();
    let mut claims = signed_claims(&trust, "POST", path, &body);
    claims.operation = "workers:read".into();
    let response = signed_request(app.clone(), &identity, &claims, body, "workspace-a").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(f.root().exists());
    request.workspace_id = "other-workspace".into();
    let body = serde_json::to_vec(&request).unwrap();
    let claims = signed_claims(&trust, "POST", path, &body);
    let response = signed_request(app, &identity, &claims, body, "workspace-a").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        runtime_session_retention(f.temp.path())
            .ensure_live(f.session)
            .is_ok()
    );
    assert!(f.root().exists());
}
