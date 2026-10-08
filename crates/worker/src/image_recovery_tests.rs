//! Recovery tests use the normal Worker execution and append-log commit gate.
use super::*;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::scheme::{Scheme, openai_responses::OpenAIResponsesScheme};
use agen::llm_client::{ClientError, Request, ResponseStream};
use agen::tool::{Attachment, ImageAttachment};
use async_trait::async_trait;

#[derive(Clone, Copy, Debug, Default)]
enum RejectionRoute {
    #[default]
    Http,
    SseError,
    ResponseFailed,
}

const REJECTION_ROUTES: [RejectionRoute; 3] = [
    RejectionRoute::Http,
    RejectionRoute::SseError,
    RejectionRoute::ResponseFailed,
];

struct RetryObservation {
    store: session_store::FsStore,
    session_id: SessionId,
    segment_id: SegmentId,
    committed: Option<Vec<LogEntry>>,
}

#[derive(Clone, Default)]
struct RejectImages {
    route: RejectionRoute,
    requests: Arc<Mutex<Vec<Request>>>,
    retry_observation: Arc<Mutex<Option<RetryObservation>>>,
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
            if matches!(self.route, RejectionRoute::Http) {
                return Err(ClientError::Api {
                    status: Some(400),
                    code: Some("invalid_value".into()),
                    message: "The image requires 52150 patches, exceeding the limit of 30000."
                        .into(),
                    retry_after: None,
                });
            }
            // Exercise the public Responses parser, not a hand-built ErrorEvent.
            // Started is metadata; no generated block precedes this rejection.
            let scheme = OpenAIResponsesScheme::new();
            let mut state = Default::default();
            let mut events = scheme.parse_sse(
                "response.created",
                r#"{"response":{"id":"image-response"}}"#,
                &mut state,
            )?;
            let (event_type, data) = match self.route {
                RejectionRoute::Http => unreachable!(),
                RejectionRoute::SseError => (
                    "error",
                    r#"{"type":"error","code":"invalid_value","message":"The image requires 52150 patches, exceeding the limit of 30000."}"#,
                ),
                RejectionRoute::ResponseFailed => (
                    "response.failed",
                    r#"{"type":"response.failed","response":{"id":"image-response","status":"failed","error":{"code":"invalid_value","message":"The image requires 52150 patches, exceeding the limit of 30000."}}}"#,
                ),
            };
            events.extend(scheme.parse_sse(event_type, data, &mut state)?);
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        // Capture committed filesystem state at the retry boundary, before any
        // successful output or cancellation can cause a later append.
        if let Some(observation) = self.retry_observation.lock().unwrap().as_mut()
            && observation.committed.is_none()
        {
            observation.committed = Some(
                observation
                    .store
                    .read_all(observation.session_id, observation.segment_id)
                    .unwrap(),
            );
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

async fn image_worker(
    route: RejectionRoute,
) -> (
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
    let client = RejectImages {
        route,
        ..RejectImages::default()
    };
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
    *client.retry_observation.lock().unwrap() = Some(RetryObservation {
        // Reopen the store so this observes filesystem records, not engine state.
        store: session_store::FsStore::new(worker.store.root_dir()).unwrap(),
        session_id: worker.session_id(),
        segment_id: worker.segment_id(),
        committed: None,
    });
    (dir, worker, client)
}
fn rejected(item: &Item) -> bool {
    matches!(item, Item::ToolResult { call_id, is_error: true, attachments, .. } if call_id == "image-call" && attachments.is_empty())
}

fn assert_correction_committed_before_retry(client: &RejectImages) {
    let observation = client.retry_observation.lock().unwrap();
    let observation = observation.as_ref().unwrap();
    let raw = observation
        .committed
        .as_ref()
        .expect("the retry must observe the committed correction");
    assert_eq!(
        raw.iter()
            .filter(|entry| matches!(entry, LogEntry::ToolResultCorrected { .. }))
            .count(),
        1,
        "{:?}: correction must already be appended at retry",
        client.route
    );
    let restored =
        restore_history_entries(observation.session_id, observation.segment_id, raw).unwrap();
    let corrected = restored
        .iter()
        .find(|entry| rejected(&entry.item))
        .expect("committed retry history must contain the attachment-free error");
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "{:?}", client.route);
    assert!(requests[0].items.iter().any(
        |item| matches!(item, Item::ToolResult { attachments, .. } if !attachments.is_empty())
    ));
    assert_eq!(
        requests[1].items.iter().find(|item| rejected(item)),
        Some(&corrected.item),
        "{:?}: retry must use the durable corrected result",
        client.route
    );
    assert!(requests[1].items.iter().all(
        |item| !matches!(item, Item::ToolResult { attachments, .. } if !attachments.is_empty())
    ));
}

#[tokio::test]
async fn image_correction_is_committed_before_retry_and_survives_restore_and_reseed() {
    for route in REJECTION_ROUTES {
        assert_image_correction_survives_restore_and_reseed(route).await;
    }
}

async fn assert_image_correction_survives_restore_and_reseed(route: RejectionRoute) {
    let (_dir, mut worker, client) = image_worker(route).await;
    assert_eq!(
        worker.run_text("continue").await.unwrap(),
        WorkerRunResult::Finished
    );
    assert_correction_committed_before_retry(&client);
    let raw = worker
        .store
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert_eq!(
        raw.iter()
            .filter(|entry| matches!(entry, LogEntry::ToolResultCorrected { .. }))
            .count(),
        1,
        "{route:?}: recovery must append exactly one correction"
    );
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
    let reseeded_segment_id = session_store::new_segment_id();
    worker
        .store
        .create_segment(worker.session_id(), reseeded_segment_id, &[seed])
        .unwrap();
    let reopened_store = session_store::FsStore::new(worker.store.root_dir()).unwrap();
    let reseeded_raw = reopened_store
        .read_all(worker.session_id(), reseeded_segment_id)
        .unwrap();
    let restored_again =
        restore_history_entries(worker.session_id(), reseeded_segment_id, &reseeded_raw).unwrap();
    assert_eq!(
        restored_again
            .iter()
            .filter(|entry| rejected(&entry.item))
            .count(),
        1
    );
    let effective_correction = restored_again
        .iter()
        .find(|entry| rejected(&entry.item))
        .unwrap();
    assert_eq!(effective_correction.annotation, original.metadata);
    let expected_result = effective_correction.item.clone();
    let mut history = History::from_entries(restored_again);
    let out = Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone())
        .run_with_annotation(&mut history, "continue after restart", &mut |_| {
            Ok(SessionHistoryMetadata::legacy_unknown())
        })
        .await;
    assert!(matches!(out.result, EngineRunExit::Finished));
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "{route:?}");
    assert_eq!(
        requests[2].items.iter().find(|item| rejected(item)),
        Some(&expected_result),
        "{route:?}: next request must retain the restored correction"
    );
    assert!(requests[2].items.iter().all(
        |item| !matches!(item, Item::ToolResult { attachments, .. } if !attachments.is_empty())
    ));
}

#[tokio::test]
async fn cancellation_after_image_correction_does_not_rollback_the_correction() {
    for route in REJECTION_ROUTES {
        assert_cancellation_preserves_image_correction(route).await;
    }
}

async fn assert_cancellation_preserves_image_correction(route: RejectionRoute) {
    let (_dir, mut worker, client) = image_worker(route).await;
    *client.cancel_on_retry.lock().unwrap() = Some(worker.engine().cancel_sender());
    let result = worker.run_text("continue").await.unwrap();
    assert_eq!(result, WorkerRunResult::Cancelled, "{route:?}");
    assert_correction_committed_before_retry(&client);
    let raw = worker
        .store
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert_eq!(
        raw.iter()
            .filter(|entry| matches!(entry, LogEntry::ToolResultCorrected { .. }))
            .count(),
        1,
        "{route:?}: cancellation must preserve exactly one correction"
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
