//! Recovery tests use the normal Worker execution and append-log commit gate.
use super::*;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::{ClientError, Request, ResponseStream};
use agen::tool::{Attachment, ImageAttachment};
use async_trait::async_trait;

#[derive(Clone, Default)]
struct RejectImages {
    requests: Arc<Mutex<Vec<Request>>>,
    cancel_on_retry: Arc<Mutex<Option<tokio::sync::mpsc::Sender<()>>>>,
}
#[async_trait]
impl LlmClient for RejectImages {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let has_image = request.items.iter().any(
            |item| matches!(item, Item::ToolResult { attachments, .. } if !attachments.is_empty()),
        );
        self.requests.lock().unwrap().push(request);
        if has_image {
            return Err(ClientError::Api {
                status: Some(400),
                code: Some("invalid_value".into()),
                message: "The image requires 52150 patches, exceeding the limit of 30000.".into(),
                retry_after: None,
            });
        }
        if let Some(cancel) = self.cancel_on_retry.lock().unwrap().take() {
            cancel.try_send(()).unwrap();
            return Ok(Box::pin(futures::stream::pending()));
        }
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::text_block_start(0)),
            Ok(LlmEvent::text_delta(0, "Resize the source image")),
            Ok(LlmEvent::text_block_stop(0, None)),
            Ok(LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            })),
        ])))
    }
}

async fn image_worker() -> (
    tempfile::TempDir,
    Worker<RejectImages, session_store::FsStore>,
    RejectImages,
) {
    let dir = tempfile::tempdir().unwrap();
    let store = session_store::FsStore::new(dir.path().join("session")).unwrap();
    let manifest = WorkerManifest::from_toml(
        r#"
[worker]
name = "image-recovery-test"
[model]
scheme = "anthropic"
model_id = "test"
[engine]
[scope]
allow = []
"#,
    )
    .unwrap();
    let client = RejectImages::default();
    let mut worker = Worker::new(
        manifest,
        Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone()),
        store,
        WorkerWorkspaceContext::local_filesystem(None),
        WorkerFilesystemAuthority::None,
        Scope::writable(dir.path()).unwrap(),
    )
    .await
    .unwrap();
    worker.disable_manifest_lifecycle_features();
    worker.set_history_for_test(vec![
        Item::user_message("inspect this image"),
        Item::tool_call("image-call", "ViewImage", "{}"),
        Item::tool_result_item_with_disposition_and_attachments(
            "image-call",
            "Attached image",
            None,
            agen::ToolResultDisposition::Success,
            vec![Attachment::Image(ImageAttachment::new(
                "image/png",
                vec![1_u8, 2, 3],
            ))],
        ),
    ]);
    worker.wire_history_persistence();
    (dir, worker, client)
}
fn rejected(item: &Item) -> bool {
    matches!(item, Item::ToolResult { call_id, is_error: true, attachments, .. } if call_id == "image-call" && attachments.is_empty())
}

#[tokio::test]
async fn image_correction_is_committed_before_retry_and_survives_restore_and_reseed() {
    let (_dir, mut worker, client) = image_worker().await;
    assert_eq!(
        worker.run_text("continue").await.unwrap(),
        WorkerRunResult::Finished
    );
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    let raw = worker
        .store
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    let correction = raw
        .iter()
        .find(|entry| matches!(entry, LogEntry::ToolResultCorrected { .. }))
        .unwrap()
        .clone();
    let restored = restore_history_entries(worker.session_id(), worker.segment_id(), &raw).unwrap();
    assert_eq!(
        restored
            .iter()
            .filter(|entry| rejected(&entry.item))
            .count(),
        1
    );
    assert_eq!(
        session_store::collect_state(&raw)
            .history
            .iter()
            .filter(|item| rejected(item))
            .count(),
        1
    );
    let LogEntry::ToolResultCorrected {
        entry: corrected, ..
    } = &correction
    else {
        unreachable!()
    };
    let original = match &raw[0] {
        LogEntry::AnnotatedSegmentStart { history, .. } => history
            .iter()
            .find(|entry| entry.metadata.entry_id == corrected.metadata.entry_id)
            .unwrap(),
        _ => panic!("missing seed"),
    };
    assert!(
        matches!(&original.item, session_store::LoggedItem::ToolResult { attachments, is_error: false, .. } if attachments.len() == 1)
    );
    assert_eq!(
        restored
            .iter()
            .find(|entry| rejected(&entry.item))
            .unwrap()
            .annotation,
        original.metadata
    );
    let snapshot = session_store::public_snapshot::project_current_session_snapshot(&raw);
    let projected = snapshot
        .entries
        .iter()
        .find(|entry| entry.entry_id == corrected.metadata.entry_id.0)
        .unwrap();
    assert!(
        matches!(&projected.data, protocol::SessionSnapshotEntryData::ToolResult { is_error: true, attachments, .. } if attachments.is_empty())
    );
    assert!(matches!(
        crate::ipc::protocol_session::live_log_entry_event(correction),
        Some(Event::SessionEntryCommitted { .. })
    ));

    // Fork/compact seed writers serialize the effective annotated history.
    let seed = LogEntry::AnnotatedSegmentStart {
        ts: 1,
        session_id: worker.session_id(),
        system_prompt: None,
        config: agen::llm_client::RequestConfig::default(),
        history: restored.iter().map(to_logged_history_entry).collect(),
        forked_from: None,
        compacted_from: None,
    };
    let restored_again =
        restore_history_entries(worker.session_id(), worker.segment_id(), &[seed]).unwrap();
    assert_eq!(
        restored_again
            .iter()
            .filter(|entry| rejected(&entry.item))
            .count(),
        1
    );
    let mut history = History::from_entries(restored_again);
    let out = Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone())
        .run_with_annotation(&mut history, "continue after restart", &mut |_| {
            Ok(SessionHistoryMetadata::legacy_unknown())
        })
        .await;
    assert!(matches!(out.result, EngineRunExit::Finished));
    assert_eq!(client.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn cancellation_after_image_correction_does_not_rollback_the_correction() {
    let (_dir, mut worker, client) = image_worker().await;
    *client.cancel_on_retry.lock().unwrap() = Some(worker.engine().cancel_sender());
    let result = worker.run_text("continue").await.unwrap();
    assert_eq!(result, WorkerRunResult::Cancelled);
    let raw = worker
        .store
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert!(
        raw.iter()
            .any(|entry| matches!(entry, LogEntry::ToolResultCorrected { .. }))
    );
    let restored = restore_history_entries(worker.session_id(), worker.segment_id(), &raw).unwrap();
    assert_eq!(
        restored
            .iter()
            .filter(|entry| rejected(&entry.item))
            .count(),
        1
    );
}
