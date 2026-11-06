//! Shared real Session log/reopen fixture for model-facing Tool results.
use agen::{Item, tool::ToolResult};

pub(crate) fn reopen_result(result: ToolResult) -> Item {
    use session_store::{
        LogEntry, LoggedHistoryEntry, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
        LoggedWorkerSubject, WorkerSessionStore,
    };
    let root = tempfile::tempdir().unwrap();
    let store = WorkerSessionStore::new(root.path()).unwrap();
    let config = agen::llm_client::RequestConfig::default();
    let (session, segment) = session_store::segment::create_segment(
        &store,
        session_store::segment::SegmentStartState {
            system_prompt: None,
            config: &config,
            history: Vec::new(),
            user_segments: Vec::new(),
        },
    )
    .unwrap();
    let item = Item::tool_result_item_with_disposition_and_attachments(
        result.tool_use_id,
        result.summary,
        result.content,
        result.disposition,
        result.attachments,
    );
    session_store::segment::append_entry(
        &store,
        session,
        segment,
        LogEntry::AnnotatedToolResult {
            ts: 1,
            entry: LoggedHistoryEntry {
                item: item.into(),
                metadata: LoggedSessionHistoryMetadata {
                    entry_id: Default::default(),
                    origin: LoggedSessionHistoryOrigin::ToolOutput {
                        worker: LoggedWorkerSubject {
                            workspace_id: None,
                            runtime_id: None,
                            worker_id: "tool-result-test".into(),
                        },
                    },
                    derivation: None,
                    input_metadata: None,
                },
            },
        },
    )
    .unwrap();
    drop(store);
    let reopened = WorkerSessionStore::open_read_only(root.path()).unwrap();
    let mut history = session_store::segment::restore(&reopened, session, segment)
        .unwrap()
        .history;
    assert_eq!(history.len(), 1);
    history.remove(0)
}
