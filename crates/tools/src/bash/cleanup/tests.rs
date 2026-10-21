//! Component tests enter through the production CommandGuard Drop, cleanup, page
//! validation and retained store. Only the Workdir provider is replaced; paused time
//! and a cancel barrier make faults/deadlines deterministic without OS timing.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use workdir::*;

const PRIVATE: &str = "SECRET command /private/host/path output password=poison";

#[derive(Clone, Copy, Debug)]
enum Fault {
    Cancel,
    CancelHang,
    Drain,
    TerminalTimeout,
    Sequence,
    Cursor,
    RetainedChunk,
    TerminalMetadata,
    CollectionIncomplete,
    Complete,
}

#[derive(Debug)]
struct Provider {
    local: LocalWorkdirSession,
    fault: Fault,
    cancelling: tokio::sync::Notify,
    allow_drain: AtomicBool,
}

#[async_trait]
impl WorkdirSession for Provider {
    fn workdir(&self) -> &Workdir {
        self.local.workdir()
    }
    fn capabilities(&self) -> WorkdirSessionCapabilities {
        WorkdirSessionCapabilities::ALL
    }
    async fn stat(&self, _: StatRequest) -> Result<StatResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn read(&self, _: ReadRequest) -> Result<ReadResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn write(&self, _: WriteRequest) -> Result<WriteResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn edit(&self, _: EditRequest) -> Result<EditResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn list(&self, _: ListRequest) -> Result<ListResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn glob(&self, _: GlobRequest) -> Result<GlobResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn grep(&self, _: GrepRequest) -> Result<GrepResult, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn start_command(&self, _: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }
    async fn command_status(&self, _: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        Err(WorkdirError::OperationFailed)
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        assert_eq!(request.cursor, 3);
        while !self.allow_drain.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        match self.fault {
            Fault::Drain => {
                return Err(WorkdirError::Transport(PRIVATE.into())
                    .with_failure_reason(WorkdirFailureReason::HttpTransfer));
            }
            Fault::TerminalTimeout => return std::future::pending().await,
            _ => {}
        }
        let mut chunk = CommandByteChunk {
            sequence: 3,
            offset: 3,
            stream: CommandStream::Stdout,
            bytes: b"tail".to_vec(),
        };
        if matches!(self.fault, Fault::Sequence) {
            chunk.sequence = 4;
        }
        if matches!(self.fault, Fault::RetainedChunk) {
            chunk.offset = 42;
        }
        let mut terminal = terminal();
        if matches!(self.fault, Fault::TerminalMetadata) {
            terminal.stdout_delivered_bytes = 6;
        }
        if matches!(self.fault, Fault::CollectionIncomplete) {
            terminal.output_collection = CommandOutputCollection::Incomplete {
                failure: CommandFailure {
                    stage: CommandFailureStage::StdoutDrain,
                    reason: CommandIoErrorKind::Other,
                    os_code: None,
                },
            };
        }
        Ok(CommandOutput {
            chunks: vec![chunk],
            next_cursor: if matches!(self.fault, Fault::Cursor) {
                8
            } else {
                7
            },
            terminal: Some(terminal),
        })
    }

    async fn cancel_command(&self, _: CommandHandle) -> Result<(), WorkdirError> {
        self.cancelling.notify_one();
        match self.fault {
            Fault::Cancel => Err(WorkdirError::Unavailable(PRIVATE.into())),
            Fault::CancelHang => std::future::pending().await,
            _ => Ok(()),
        }
    }
    async fn close(&self) -> Result<(), WorkdirError> {
        Ok(())
    }
}

fn terminal() -> CommandTerminal {
    CommandTerminal {
        status: CommandStatus::Completed,
        exit_code: Some(0),
        process_failure: None,
        output_collection: CommandOutputCollection::Complete,
        stdout_bytes: 7,
        stderr_bytes: 0,
        stdout_delivered_bytes: 7,
        stderr_delivered_bytes: 0,
        truncation_reason: None,
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    provider: Arc<Provider>,
    state: Arc<Mutex<BashExecutionState>>,
    view: BashOutputView,
    id: String,
}

impl Fixture {
    fn start(fault: Fault) -> (Self, CommandGuard) {
        let base =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/cleanup-fixtures");
        std::fs::create_dir_all(&base).unwrap();
        let root = tempfile::tempdir_in(base).unwrap();
        let provider = Arc::new(Provider {
            local: LocalWorkdirSession::new(
                manifest::Scope::writable(root.path()).unwrap(),
                root.path().into(),
            ),
            fault,
            cancelling: Default::default(),
            allow_drain: AtomicBool::new(false),
        });
        let view = BashOutputView::new();
        let mut retained = view.begin().unwrap();
        retained.append(CommandStream::Stdout, b"pre");
        let id = retained.id();
        let state = Arc::new(Mutex::new(BashExecutionState::default()));
        let handle = CommandHandle(PRIVATE.into());
        state.lock().unwrap().active.insert(
            PRIVATE.into(),
            ActiveCommand {
                call_id: PRIVATE.into(),
                execution_nonce: 1,
                session: provider.clone(),
                handle: handle.clone(),
            },
        );
        state
            .lock()
            .unwrap()
            .cancellation_requested
            .insert(PRIVATE.into());
        let guard = CommandGuard {
            session: provider.clone(),
            state: state.clone(),
            execution_id: PRIVATE.into(),
            execution_nonce: 1,
            handle: Some(handle),
            cursor: 3,
            retained: Some(retained),
        };
        (
            Self {
                _root: root,
                provider,
                state,
                view,
                id,
            },
            guard,
        )
    }

    async fn finish(&self) -> BashOutputMetadata {
        self.provider.allow_drain.store(true, Ordering::Release);
        // The provider's read retry backoff and cleanup deadlines are virtual time.
        for _ in 0..4 {
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(3)).await;
        }
        for _ in 0..1000 {
            if let Some(metadata) = self.view.metadata(&self.id) {
                return metadata;
            }
            tokio::task::yield_now().await;
        }
        panic!("finite cleanup did not publish partial evidence");
    }

    fn assert_evidence(&self, m: &BashOutputMetadata, terminal: bool, bytes: &[u8]) {
        assert_eq!(
            self.view
                .read(&self.id, CommandStream::Stdout, 0, 64)
                .unwrap(),
            bytes
        );
        assert_eq!(m.stdout_saved, bytes.len());
        assert_eq!(m.stdout_delivered, bytes.len() as u64);
        assert_eq!(m.stdout_seen, bytes.len() as u64);
        assert_eq!(
            m.terminal.status,
            terminal.then_some(CommandStatus::Completed)
        );
        assert_eq!(m.terminal.exit_code, terminal.then_some(0));
        if !terminal {
            assert!(m.terminal.output_collection.is_none());
            assert!(m.terminal.process_failure.is_none());
            assert!(!m.terminal.transport_complete);
        }
        let diagnostics = m.cleanup.as_ref().unwrap();
        assert_eq!(diagnostics.execution_nonce, 1);
        let json = serde_json::to_string(diagnostics).unwrap();
        let debug = format!("{diagnostics:?}");
        for forbidden in [
            PRIVATE, "SECRET", "/private", "password", "poison", "tail", "pre",
        ] {
            assert!(!json.contains(forbidden), "diagnostics leaked data: {json}");
            assert!(
                !debug.contains(forbidden),
                "tracing payload leaked data: {debug}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn cancel_failure_does_not_preclude_confirmed_terminal() {
    let (f, guard) = Fixture::start(Fault::Cancel);
    drop(guard);
    f.provider.cancelling.notified().await;
    assert!(f.state.lock().unwrap().active.contains_key(PRIVATE));
    let m = f.finish().await;
    f.assert_evidence(&m, true, b"pretail");
    assert!(m.terminal.transport_complete);
    assert!(matches!(
        m.cleanup.as_ref().unwrap().cancel,
        CleanupCancel::Failed { .. }
    ));
    assert!(matches!(
        m.cleanup.as_ref().unwrap().drain,
        CleanupDrain::TerminalConfirmed
    ));
    assert!(!m.cleanup.as_ref().unwrap().deadline_expired);
    assert!(f.state.lock().unwrap().active.is_empty());
    assert!(f.state.lock().unwrap().cancellation_requested.is_empty());
}

#[tokio::test(start_paused = true)]
async fn acknowledged_cancel_drain_faults_keep_distinct_reasons_and_partial_bytes() {
    for (fault, reason, bytes) in [
        (Fault::Drain, "provider", b"pre".as_slice()),
        (Fault::Sequence, "sequence_gap", b"pre"),
        (Fault::Cursor, "cursor_gap", b"pretail"),
        (Fault::RetainedChunk, "retained_chunk_invalid", b"pre"),
        (
            Fault::TerminalMetadata,
            "terminal_metadata_invalid",
            b"pretail",
        ),
    ] {
        let (f, guard) = Fixture::start(fault);
        drop(guard);
        f.provider.cancelling.notified().await;
        let m = f.finish().await;
        f.assert_evidence(&m, false, bytes);
        let diagnostic = serde_json::to_value(m.cleanup.as_ref().unwrap()).unwrap();
        assert_eq!(diagnostic["cancel"]["outcome"], "acknowledged");
        assert_eq!(diagnostic["drain"]["outcome"], "failed");
        assert_eq!(diagnostic["drain"]["reason"]["kind"], reason);
        assert_eq!(diagnostic["registration"], "released");
        assert_eq!(diagnostic["deadline_expired"], false);
        assert!(f.state.lock().unwrap().active.is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_deadline_never_claims_stop_or_leaves_active_collector() {
    let (f, guard) = Fixture::start(Fault::TerminalTimeout);
    drop(guard);
    f.provider.cancelling.notified().await;
    let m = f.finish().await;
    f.assert_evidence(&m, false, b"pre");
    assert!(matches!(
        m.cleanup.as_ref().unwrap().cancel,
        CleanupCancel::Acknowledged
    ));
    assert!(matches!(
        m.cleanup.as_ref().unwrap().drain,
        CleanupDrain::DeadlineExceeded
    ));
    assert!(m.cleanup.as_ref().unwrap().deadline_expired);
    assert!(f.state.lock().unwrap().active.is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancel_deadline_still_attempts_independent_terminal_observation() {
    let (f, guard) = Fixture::start(Fault::CancelHang);
    drop(guard);
    f.provider.cancelling.notified().await;
    let m = f.finish().await;
    f.assert_evidence(&m, true, b"pretail");
    assert!(matches!(
        m.cleanup.as_ref().unwrap().cancel,
        CleanupCancel::DeadlineExceeded
    ));
    assert!(matches!(
        m.cleanup.as_ref().unwrap().drain,
        CleanupDrain::TerminalConfirmed
    ));
    assert!(!m.cleanup.as_ref().unwrap().deadline_expired);
    assert!(f.state.lock().unwrap().active.is_empty());
}

#[tokio::test(start_paused = true)]
async fn old_cleanup_preserves_new_execution_and_cancellation_registration() {
    let (f, guard) = Fixture::start(Fault::Complete);
    drop(guard);
    f.provider.cancelling.notified().await;
    f.state
        .lock()
        .unwrap()
        .active
        .get_mut(PRIVATE)
        .unwrap()
        .execution_nonce = 2;
    let m = f.finish().await;
    f.assert_evidence(&m, true, b"pretail");
    assert!(matches!(
        m.cleanup.as_ref().unwrap().registration,
        CleanupRegistration::Superseded
    ));
    assert_eq!(f.state.lock().unwrap().active[PRIVATE].execution_nonce, 2);
    assert!(
        f.state
            .lock()
            .unwrap()
            .cancellation_requested
            .contains(PRIVATE)
    );
}

#[tokio::test(start_paused = true)]
async fn terminal_confirmation_is_independent_of_output_collection_completeness() {
    let (f, guard) = Fixture::start(Fault::CollectionIncomplete);
    drop(guard);
    f.provider.cancelling.notified().await;
    let m = f.finish().await;
    f.assert_evidence(&m, true, b"pretail");
    assert!(!m.terminal.transport_complete);
    assert!(matches!(
        m.terminal.output_collection,
        Some(CommandOutputCollection::Incomplete { .. })
    ));
    assert!(matches!(
        m.cleanup.as_ref().unwrap().drain,
        CleanupDrain::TerminalConfirmed
    ));
    assert!(f.state.lock().unwrap().active.is_empty());
}
