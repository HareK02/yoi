//! Process/pipe integration through the production local provider. Temporary roots
//! and provider tasks are RAII-owned; byte pulls and bounded timeouts synchronize tests.

use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn missing_terminal_is_diagnosed_without_changing_error_classification() {
    let (sender, receiver) = mpsc::channel(1);
    let (_status, terminal_status) = watch::channel(None);
    drop(sender);
    let mut transport = Transport {
        receiver,
        pending: None,
        cursor: 0,
        replay: None,
        terminal: None,
        terminal_status,
    };
    let error = transport
        .read(&CommandOutputRequest {
            handle: CommandHandle("diagnostic-test".into()),
            cursor: 0,
            limit: 16,
            wait: true,
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.failure_reason(),
        Some(crate::WorkdirFailureReason::CommandTerminalMissing)
    );
    assert!(matches!(
        error.classification_source(),
        WorkdirError::Unavailable(_)
    ));
}

struct Harness {
    session: LocalWorkdirSession,
    _root: TempDir,
}

impl Harness {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let session = LocalWorkdirSession::new(
            Scope::writable(root.path()).unwrap(),
            root.path().to_path_buf(),
        );
        Self {
            session,
            _root: root,
        }
    }

    async fn start(&self, command: &str, timeout_secs: u64) -> CommandHandle {
        self.start_budget(command, timeout_secs, crate::MAX_COMMAND_TRANSPORT_BYTES)
            .await
    }

    fn request(command: &str, timeout_secs: u64, budget: u64) -> CommandRequest {
        CommandRequest {
            transport_byte_budget: budget,
            command: command.into(),
            timeout_secs,
            cwd: WorkdirPath::root(),
            tool_call_id: None,
        }
    }

    async fn start_budget(&self, command: &str, timeout_secs: u64, budget: u64) -> CommandHandle {
        self.session
            .start_command(Self::request(command, timeout_secs, budget))
            .await
            .unwrap()
    }

    async fn page(&self, handle: &CommandHandle, cursor: u64, limit: usize) -> CommandOutput {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.session.command_output(CommandOutputRequest {
                handle: handle.clone(),
                cursor,
                limit,
                wait: true,
            }),
        )
        .await
        .expect("bounded command pull")
        .unwrap()
    }

    async fn collect(
        &self,
        handle: &CommandHandle,
        mut cursor: u64,
    ) -> (Vec<crate::CommandByteChunk>, crate::CommandTerminal) {
        let mut chunks = Vec::new();
        loop {
            let page = self
                .page(handle, cursor, crate::MAX_COMMAND_READ_BYTES)
                .await;
            assert!(
                page.chunks
                    .iter()
                    .map(|chunk| chunk.bytes.len())
                    .sum::<usize>()
                    <= crate::MAX_COMMAND_READ_BYTES
            );
            cursor = page.next_cursor;
            chunks.extend(page.chunks);
            assert!(chunks.iter().map(|chunk| chunk.bytes.len()).sum::<usize>() <= 8 * 1024 * 1024);
            if let Some(terminal) = page.terminal {
                return (chunks, terminal);
            }
        }
    }
}

fn stream_bytes(chunks: &[crate::CommandByteChunk], stream: CommandStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in chunks.iter().filter(|chunk| chunk.stream == stream) {
        assert_eq!(chunk.offset, bytes.len() as u64);
        bytes.extend_from_slice(&chunk.bytes);
    }
    bytes
}

#[tokio::test]
async fn high_single_line_output_survives_absent_and_lagged_console_subscribers() {
    for lagged in [false, true] {
        let harness = Harness::new();
        let _lagged_receiver = lagged.then(|| harness.session.subscribe_command_events().unwrap());
        let handle = harness.start("head -c 3145728 /dev/zero | tr '\\0' x; head -c 65536 /dev/zero | tr '\\0' e >&2; exit 7", 20).await;
        let (chunks, terminal) = harness.collect(&handle, 0).await;
        assert_eq!(terminal.status, CommandStatus::Failed);
        assert_eq!(terminal.exit_code, Some(7));
        assert_eq!(
            stream_bytes(&chunks, CommandStream::Stdout),
            vec![b'x'; 3145728]
        );
        assert_eq!(
            stream_bytes(&chunks, CommandStream::Stderr),
            vec![b'e'; 65536]
        );
        let mut cursor = 0;
        for chunk in chunks {
            assert_eq!(chunk.sequence, cursor);
            cursor += chunk.bytes.len() as u64;
        }
        harness.session.close().await.unwrap();
    }
}

#[tokio::test]
async fn one_byte_pages_preserve_raw_utf8_invalid_bytes_and_replay_until_acknowledged() {
    let harness = Harness::new();
    let handle = harness
        .start(
            "printf '\\342\\202\\254\\377\\000'; printf '\\346\\227\\245' >&2",
            5,
        )
        .await;
    let mut chunks = Vec::new();
    let mut cursor = 0;
    loop {
        let page = harness.page(&handle, cursor, 1).await;
        assert_eq!(harness.page(&handle, cursor, 1).await, page);
        cursor = page.next_cursor;
        chunks.extend(page.chunks);
        if let Some(terminal) = page.terminal {
            assert_eq!(terminal.status, CommandStatus::Completed);
            break;
        }
    }
    assert_eq!(
        stream_bytes(&chunks, CommandStream::Stdout),
        b"\xe2\x82\xac\xff\0"
    );
    assert_eq!(
        stream_bytes(&chunks, CommandStream::Stderr),
        b"\xe6\x97\xa5"
    );
    assert!(
        harness
            .session
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1,
                wait: false
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancellation_while_backpressured_drains_bytes_before_terminal_fence() {
    let harness = Harness::new();
    let handle = harness.start("printf ready; exec yes x", 30).await;
    let first = harness.page(&handle, 0, 1).await;
    harness
        .session
        .cancel_command(handle.clone())
        .await
        .unwrap();
    let (mut chunks, terminal) = harness.collect(&handle, first.next_cursor).await;
    let mut all = first.chunks;
    all.append(&mut chunks);
    assert!(stream_bytes(&all, CommandStream::Stdout).starts_with(b"ready"));
    assert_eq!(terminal.status, CommandStatus::Cancelled);
    let final_page = harness
        .page(
            &handle,
            all.iter().map(|chunk| chunk.bytes.len() as u64).sum(),
            1,
        )
        .await;
    assert!(final_page.chunks.is_empty());
    assert_eq!(final_page.terminal, Some(terminal));
}

#[tokio::test]
async fn timeout_and_session_close_finalize_without_an_output_subscriber() {
    let harness = Harness::new();
    let handle = harness
        .start("printf ready; exec tail -f /dev/null", 1)
        .await;
    let first = harness.page(&handle, 0, 5).await;
    assert_eq!(stream_bytes(&first.chunks, CommandStream::Stdout), b"ready");
    // The process has started; expire its deadline without delaying the test.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::resume();
    let (chunks, terminal) = harness.collect(&handle, 0).await;
    assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"ready");
    assert_eq!(terminal.status, CommandStatus::TimedOut);
    let handle = harness.start("exec yes x", 30).await;
    // Receiving one byte establishes actual process output, without a timing sleep.
    harness.page(&handle, 0, 1).await;
    tokio::time::timeout(Duration::from_secs(5), harness.session.close())
        .await
        .unwrap()
        .unwrap();
    assert!(harness.session.command_status(handle).await.is_err());
}

#[tokio::test]
async fn interleaved_streams_keep_independent_offsets_and_bounded_read_validation() {
    let harness = Harness::new();
    let handle = harness
        .start(
            "for ((i=0;i<100;i++)); do printf 'out%s;' \"$i\"; printf 'err%s;' \"$i\" >&2; done",
            5,
        )
        .await;
    for limit in [0, crate::MAX_COMMAND_READ_BYTES + 1] {
        assert!(
            harness
                .session
                .command_output(CommandOutputRequest {
                    handle: handle.clone(),
                    cursor: 0,
                    limit,
                    wait: false
                })
                .await
                .is_err()
        );
    }
    let (chunks, terminal) = harness.collect(&handle, 0).await;
    let expected = |prefix| {
        (0..100)
            .map(|i| format!("{prefix}{i};"))
            .collect::<String>()
            .into_bytes()
    };
    assert_eq!(
        stream_bytes(&chunks, CommandStream::Stdout),
        expected("out")
    );
    assert_eq!(
        stream_bytes(&chunks, CommandStream::Stderr),
        expected("err")
    );
    assert_eq!(terminal.status, CommandStatus::Completed);
}

#[tokio::test]
async fn byte_budget_zero_exact_partial_and_shared_keep_seen_and_delivered_distinct() {
    let harness = Harness::new();
    for (command, budget, stdout_seen, stderr_seen, exit_code) in [
        ("printf abc; printf defg >&2", 0, 3, 4, 0),
        ("printf abc", 3, 3, 0, 0),
        ("printf abcdefg", 3, 7, 0, 0),
        ("printf abcdefg; exit 7", 3, 7, 0, 7),
        (
            "head -c 32768 /dev/zero; head -c 32768 /dev/zero >&2",
            12345,
            32768,
            32768,
            0,
        ),
    ] {
        let handle = harness.start_budget(command, 5, budget).await;
        let (chunks, terminal) = harness.collect(&handle, 0).await;
        let stdout = stream_bytes(&chunks, CommandStream::Stdout);
        let stderr = stream_bytes(&chunks, CommandStream::Stderr);
        let mut cursor = 0;
        for chunk in &chunks {
            assert_eq!(chunk.sequence, cursor);
            cursor += chunk.bytes.len() as u64;
        }
        assert_eq!(cursor, budget.min(stdout_seen + stderr_seen));
        assert_eq!(terminal.stdout_bytes, stdout_seen);
        assert_eq!(terminal.stderr_bytes, stderr_seen);
        assert_eq!(terminal.stdout_delivered_bytes, stdout.len() as u64);
        assert_eq!(terminal.stderr_delivered_bytes, stderr.len() as u64);
        assert_eq!(terminal.exit_code, Some(exit_code));
        assert_eq!(
            terminal.status,
            if exit_code == 0 {
                CommandStatus::Completed
            } else {
                CommandStatus::Failed
            }
        );
        assert_eq!(
            terminal.truncation_reason,
            (cursor < stdout_seen + stderr_seen)
                .then_some(crate::CommandOutputTruncation::ByteBudget)
        );
    }
}

#[tokio::test]
async fn exhausted_budget_finishes_without_receiver_and_keeps_timeout_cancel_distinct() {
    let harness = Harness::new();
    for budget in [0, 1, (QUEUE_CHUNKS * COMMAND_EVENT_CHUNK_BYTES) as u64] {
        let handle = harness
            .start_budget("head -c 1048576 /dev/zero; printf error >&2", 5, budget)
            .await;
        let mut status = harness.session.inner.commands.lock().await[&handle.0]
            .status
            .clone();
        let terminal = tokio::time::timeout(
            Duration::from_secs(5),
            status.wait_for(|value| value.is_some()),
        )
        .await
        .unwrap()
        .unwrap()
        .clone()
        .unwrap();
        assert_eq!(terminal.status, CommandStatus::Completed);
        assert_eq!(terminal.stdout_bytes, 1048576);
        assert_eq!(terminal.stderr_bytes, 5);
        assert_eq!(
            terminal.stdout_delivered_bytes + terminal.stderr_delivered_bytes,
            budget
        );
        // Full capture queue cannot retain the process task or its resources.
        {
            let mut commands = harness.session.inner.commands.lock().await;
            tokio::time::timeout(
                Duration::from_secs(5),
                &mut commands.get_mut(&handle.0).unwrap().task,
            )
            .await
            .unwrap()
            .unwrap();
        }
        // Only now acknowledge the captured queue and terminal fence.
        let (_, delivered_terminal) = harness.collect(&handle, 0).await;
        assert_eq!(delivered_terminal, terminal);
    }
    for (cancel, expected) in [
        (false, CommandStatus::TimedOut),
        (true, CommandStatus::Cancelled),
    ] {
        let handle = harness
            .start_budget(
                "printf ready-extra; head -c 32768 /dev/zero; touch produced; exec tail -f /dev/null",
                1,
                5,
            )
            .await;
        let first = harness.page(&handle, 0, 5).await;
        assert_eq!(first.next_cursor, 5);
        if cancel {
            harness
                .session
                .cancel_command(handle.clone())
                .await
                .unwrap();
        } else {
            // The byte budget hides later output. Observe the process's explicit
            // completion marker before expiring it, so all expected bytes exist.
            tokio::time::timeout(Duration::from_secs(5), async {
                while !harness._root.path().join("produced").try_exists().unwrap() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::time::resume();
        }
        let (rest, terminal) = harness.collect(&handle, first.next_cursor).await;
        assert!(rest.is_empty());
        assert_eq!(terminal.status, expected);
        assert_eq!(terminal.stdout_delivered_bytes, 5);
        assert!(terminal.stdout_bytes >= 11);
        assert_eq!(
            terminal.truncation_reason,
            Some(crate::CommandOutputTruncation::ByteBudget)
        );
        if !cancel {
            assert_eq!(terminal.stdout_bytes, 32779);
            assert_eq!(
                terminal.truncation_reason,
                Some(crate::CommandOutputTruncation::ByteBudget)
            );
        }
    }
}

#[tokio::test]
async fn budget_replay_preserves_unique_bytes_and_rejects_future_and_old_cursors() {
    let harness = Harness::new();
    let handle = harness.start_budget("printf abcdefghij", 5, 7).await;
    let first = harness.page(&handle, 0, 2).await;
    for _ in 0..3 {
        assert_eq!(harness.page(&handle, 0, 2).await, first);
    }
    assert!(
        harness
            .session
            .command_output(CommandOutputRequest {
                handle: handle.clone(),
                cursor: 8,
                limit: 2,
                wait: false,
            })
            .await
            .is_err()
    );
    let (rest, terminal) = harness.collect(&handle, first.next_cursor).await;
    let mut all = first.chunks;
    all.extend(rest);
    assert_eq!(stream_bytes(&all, CommandStream::Stdout), b"abcdefg");
    assert_eq!(terminal.stdout_bytes, 10);
    assert_eq!(terminal.stdout_delivered_bytes, 7);
    assert!(
        harness
            .session
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 2,
                wait: false,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_budget_is_rejected_before_process_start() {
    let harness = Harness::new();
    assert!(matches!(
        harness
            .session
            .start_command(Harness::request(
                "touch must-not-exist",
                5,
                crate::MAX_COMMAND_TRANSPORT_BYTES + 1,
            ))
            .await,
        Err(WorkdirError::InvalidArgument(_))
    ));
    assert!(!harness._root.path().join("must-not-exist").exists());
    assert!(harness.session.inner.commands.lock().await.is_empty());
}

#[tokio::test]
async fn slow_receiver_backpressures_before_quota_but_full_queue_does_not_block_discard() {
    use std::future::Future;
    use std::task::Poll;
    let telemetry = CommandTelemetry::new();
    let input = vec![b'x'; 1024 * 1024];
    for budget in [
        crate::MAX_COMMAND_TRANSPORT_BYTES,
        (QUEUE_CHUNKS * COMMAND_EVENT_CHUNK_BYTES) as u64,
    ] {
        let (sender, mut receiver) = mpsc::channel(QUEUE_CHUNKS);
        let mut accounting = ByteAccounting::default();
        {
            let output = pump(
                input.as_slice(),
                &b""[..],
                &sender,
                &telemetry,
                "test",
                budget,
                &mut accounting,
            );
            tokio::pin!(output);
            let polled = std::future::poll_fn(|cx| Poll::Ready(output.as_mut().poll(cx))).await;
            if budget == crate::MAX_COMMAND_TRANSPORT_BYTES {
                assert!(polled.is_pending(), "a slow receiver must stop pipe reads");
            } else {
                assert!(
                    matches!(polled, Poll::Ready(Ok(()))),
                    "discard must not reserve queue capacity"
                );
            }
        }
        assert_eq!(receiver.len(), QUEUE_CHUNKS);
        assert_eq!(
            accounting.stdout_delivered,
            (QUEUE_CHUNKS * COMMAND_EVENT_CHUNK_BYTES) as u64
        );
        assert_eq!(
            accounting.stdout,
            if budget == crate::MAX_COMMAND_TRANSPORT_BYTES {
                accounting.stdout_delivered
            } else {
                input.len() as u64
            }
        );
        let mut queued_bytes = 0;
        while let Ok(Message::Bytes(chunk)) = receiver.try_recv() {
            queued_bytes += chunk.bytes.len();
        }
        assert_eq!(queued_bytes, QUEUE_CHUNKS * COMMAND_EVENT_CHUNK_BYTES);
    }
}

#[derive(Clone, Default)]
struct Gate(Arc<std::sync::Mutex<(bool, Option<std::task::Waker>)>>);

impl Gate {
    fn open(&self) {
        let mut state = self.0.lock().unwrap();
        state.0 = true;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    }

    fn poll(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let mut state = self.0.lock().unwrap();
        if state.0 {
            std::task::Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

struct FaultProcess {
    ready: Gate,
    exited: Gate,
    wait_error: Option<std::io::ErrorKind>,
}

impl CommandProcess for FaultProcess {
    fn id(&self) -> Option<u32> {
        None
    }
    fn start_kill(&mut self) {
        self.ready.open();
    }
    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        std::future::poll_fn(|cx| self.ready.poll(cx)).await;
        self.exited.open();
        match self.wait_error {
            Some(kind) => Err(std::io::Error::new(
                kind,
                "/private/path TOKEN=secret command-body",
            )),
            None => Ok(std::process::ExitStatus::default()),
        }
    }
}

struct FaultReader {
    prefix: &'static [u8],
    prefix_read: Gate,
    fail_ready: Gate,
}

impl AsyncRead for FaultReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.prefix.is_empty() {
            let count = self.prefix.len().min(buffer.remaining());
            buffer.put_slice(&self.prefix[..count]);
            self.prefix = &self.prefix[count..];
            self.prefix_read.open();
            return std::task::Poll::Ready(Ok(()));
        }
        std::task::ready!(self.fail_ready.poll(cx));
        std::task::Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "/private/path TOKEN=secret command-body",
        )))
    }
}

// Faults replace only OS wait/read boundaries, not collection or finalization.
#[tokio::test]
async fn read_and_post_exit_drain_faults_keep_prefix_exit_and_safe_original_reason() {
    for stream in [CommandStream::Stdout, CommandStream::Stderr] {
        for after_exit in [false, true] {
            let ready = Gate::default();
            let exited = Gate::default();
            let fail_ready = if after_exit {
                exited.clone()
            } else {
                let gate = Gate::default();
                gate.open();
                gate
            };
            let reader = FaultReader {
                prefix: b"partial",
                prefix_read: if after_exit {
                    ready.clone()
                } else {
                    Gate::default()
                },
                fail_ready,
            };
            let process = FaultProcess {
                ready,
                exited,
                wait_error: None,
            };
            let (sender, mut receiver) = mpsc::channel(QUEUE_CHUNKS);
            let (_cancel, cancel_rx) = watch::channel(false);
            let telemetry = CommandTelemetry::new();
            let terminal = tokio::time::timeout(Duration::from_secs(5), async {
                match stream {
                    CommandStream::Stdout => {
                        supervise(
                            process,
                            reader,
                            &b""[..],
                            10,
                            100,
                            "fault",
                            &telemetry,
                            cancel_rx,
                            &sender,
                        )
                        .await
                    }
                    CommandStream::Stderr => {
                        supervise(
                            process,
                            &b""[..],
                            reader,
                            10,
                            100,
                            "fault",
                            &telemetry,
                            cancel_rx,
                            &sender,
                        )
                        .await
                    }
                }
            })
            .await
            .unwrap();
            let Message::Bytes(chunk) = receiver.recv().await.unwrap();
            assert_eq!(chunk.bytes, b"partial");
            assert_eq!(chunk.stream, stream);
            assert_eq!(chunk.sequence, 0);
            assert_eq!(chunk.offset, 0);
            assert!(receiver.try_recv().is_err());
            assert_eq!(terminal.status, CommandStatus::Completed);
            assert_eq!(terminal.exit_code, Some(0));
            assert_eq!(terminal.process_failure, None);
            assert_eq!(terminal.stdout_bytes + terminal.stderr_bytes, 7);
            assert_eq!(
                terminal.stdout_delivered_bytes + terminal.stderr_delivered_bytes,
                7
            );
            assert_eq!(terminal.truncation_reason, None);
            assert_eq!(
                terminal.output_collection,
                CommandOutputCollection::Incomplete {
                    failure: CommandFailure {
                        stage: match (stream, after_exit) {
                            (CommandStream::Stdout, false) => CommandFailureStage::StdoutRead,
                            (CommandStream::Stdout, true) => CommandFailureStage::StdoutDrain,
                            (CommandStream::Stderr, false) => CommandFailureStage::StderrRead,
                            (CommandStream::Stderr, true) => CommandFailureStage::StderrDrain,
                        },
                        reason: CommandIoErrorKind::PermissionDenied,
                        os_code: None
                    },
                }
            );
            let wire = serde_json::to_string(&terminal).unwrap();
            for secret in ["/private", "TOKEN", "secret", "command-body"] {
                assert!(!wire.contains(secret));
            }
        }
    }
}

#[tokio::test]
async fn wait_fault_keeps_collected_bytes_and_does_not_invent_exit_information() {
    let ready = Gate::default();
    ready.open();
    let process = FaultProcess {
        ready,
        exited: Gate::default(),
        wait_error: Some(std::io::ErrorKind::InvalidData),
    };
    let (sender, mut receiver) = mpsc::channel(QUEUE_CHUNKS);
    let (_cancel, cancel_rx) = watch::channel(false);
    let terminal = supervise(
        process,
        &b"out"[..],
        &b"err"[..],
        10,
        100,
        "wait-fault",
        &CommandTelemetry::new(),
        cancel_rx,
        &sender,
    )
    .await;
    assert_eq!(terminal.status, CommandStatus::Failed);
    assert_eq!(terminal.exit_code, None);
    assert_eq!(
        terminal.process_failure,
        Some(CommandFailure {
            stage: CommandFailureStage::Wait,
            reason: CommandIoErrorKind::InvalidData,
            os_code: None
        })
    );
    assert_eq!(
        terminal.output_collection,
        CommandOutputCollection::Complete
    );
    assert_eq!(terminal.stdout_bytes, 3);
    assert_eq!(terminal.stderr_bytes, 3);
    let mut chunks = Vec::new();
    while let Ok(Message::Bytes(chunk)) = receiver.try_recv() {
        chunks.push(chunk);
    }
    assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"out");
    assert_eq!(stream_bytes(&chunks, CommandStream::Stderr), b"err");
    assert!(!serde_json::to_string(&terminal).unwrap().contains("secret"));
}

#[tokio::test]
async fn spawn_fault_retains_safe_reason_without_claiming_output_collection() {
    let root = TempDir::new().unwrap();
    let (sender, _receiver) = mpsc::channel(QUEUE_CHUNKS);
    let (_cancel, cancel_rx) = watch::channel(false);
    let terminal = execute(
        root.path().join("missing-secret-path"),
        Harness::request("secret-command", 10, 100),
        "spawn-fault",
        &CommandTelemetry::new(),
        BTreeMap::new(),
        Arc::new(crate::CommandHost::default()),
        cancel_rx,
        &sender,
    )
    .await;
    assert_eq!(
        terminal.process_failure,
        Some(CommandFailure {
            stage: CommandFailureStage::Spawn,
            reason: CommandIoErrorKind::NotFound,
            os_code: Some(libc::ENOENT)
        })
    );
    assert_eq!(
        terminal.output_collection,
        CommandOutputCollection::NotStarted
    );
    assert_eq!(terminal.exit_code, None);
    assert_eq!(terminal.status, CommandStatus::Failed);
    assert_eq!(terminal.stdout_bytes + terminal.stderr_bytes, 0);
    assert!(!serde_json::to_string(&terminal).unwrap().contains("secret"));
}

#[cfg(feature = "http-client")]
mod http;

#[tokio::test]
async fn drain_failure_and_byte_budget_keep_independent_loss_diagnostics() {
    for stream in [CommandStream::Stdout, CommandStream::Stderr] {
        let ready = Gate::default();
        let exited = Gate::default();
        let process = FaultProcess {
            ready: ready.clone(),
            exited: exited.clone(),
            wait_error: None,
        };
        let reader = FaultReader {
            prefix: b"partial",
            prefix_read: ready,
            fail_ready: exited,
        };
        let (sender, mut receiver) = mpsc::channel(QUEUE_CHUNKS);
        let (_cancel, cancel_rx) = watch::channel(false);
        let telemetry = CommandTelemetry::new();
        let terminal = tokio::time::timeout(Duration::from_secs(5), async {
            match stream {
                CommandStream::Stdout => {
                    supervise(
                        process,
                        reader,
                        &b""[..],
                        10,
                        3,
                        "quota-fault",
                        &telemetry,
                        cancel_rx,
                        &sender,
                    )
                    .await
                }
                CommandStream::Stderr => {
                    supervise(
                        process,
                        &b""[..],
                        reader,
                        10,
                        3,
                        "quota-fault",
                        &telemetry,
                        cancel_rx,
                        &sender,
                    )
                    .await
                }
            }
        })
        .await
        .unwrap();
        let Message::Bytes(chunk) = receiver.recv().await.unwrap();
        assert_eq!(chunk.stream, stream);
        assert_eq!(chunk.bytes, b"par");
        assert!(receiver.try_recv().is_err());
        assert_eq!(terminal.exit_code, Some(0));
        assert_eq!(terminal.stdout_bytes + terminal.stderr_bytes, 7);
        assert_eq!(
            terminal.stdout_delivered_bytes + terminal.stderr_delivered_bytes,
            3
        );
        assert_eq!(
            terminal.truncation_reason,
            Some(crate::CommandOutputTruncation::ByteBudget)
        );
        assert_eq!(
            terminal.output_collection,
            CommandOutputCollection::Incomplete {
                failure: CommandFailure {
                    stage: match stream {
                        CommandStream::Stdout => CommandFailureStage::StdoutDrain,
                        CommandStream::Stderr => CommandFailureStage::StderrDrain,
                    },
                    reason: CommandIoErrorKind::PermissionDenied,
                    os_code: None,
                },
            }
        );
        assert!(terminal.metadata_valid());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires a user systemd manager, cgroup v2 memory controller and user@.service OOMPolicy=continue"]
async fn systemd_scope_oom_is_attributed_without_stopping_sibling_or_provider() {
    let root = TempDir::new().unwrap();
    let host = crate::CommandHost::systemd(32 * 1024 * 1024, 2).unwrap();
    let session = LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    )
    .with_command_host(host)
    .unwrap();
    let harness = Harness {
        session,
        _root: root,
    };
    let sibling = harness
        .start("printf sibling-ready; exec sleep 100", 30)
        .await;
    let page = harness.page(&sibling, 0, 13).await;
    assert_eq!(
        stream_bytes(&page.chunks, CommandStream::Stdout),
        b"sibling-ready"
    );
    let oom = harness.start(
        "printf before-oom; x=$(head -c 67108864 /dev/zero | tr '\\0' x); printf unexpected-success", 15,
    ).await;
    let (chunks, terminal) = harness.collect(&oom, 0).await;
    assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"before-oom");
    assert_eq!(terminal.status, CommandStatus::Failed);
    assert_eq!(
        terminal.resource_outcome,
        Some(crate::CommandResourceOutcome::Systemd {
            oom: crate::CommandOomObservation::SystemdOomKill,
            cleanup_confirmed: true,
        })
    );
    assert!(terminal.metadata_valid(), "{terminal:?}");
    assert_eq!(
        harness
            .session
            .command_status(sibling.clone())
            .await
            .unwrap(),
        CommandStatus::Running
    );
    harness
        .session
        .cancel_command(sibling.clone())
        .await
        .unwrap();
    let (_, cancelled) = harness.collect(&sibling, 0).await;
    assert_eq!(cancelled.status, CommandStatus::Cancelled);
    let next = harness.start("printf after-oom", 10).await;
    let (chunks, terminal) = harness.collect(&next, 0).await;
    assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"after-oom");
    assert_eq!(terminal.status, CommandStatus::Completed);
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires a user systemd manager, cgroup v2 memory controller and user@.service OOMPolicy=continue"]
async fn systemd_scope_collects_detached_grandchildren_and_distinguishes_kill_from_oom() {
    let root = TempDir::new().unwrap();
    let session = LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    )
    .with_command_host(crate::CommandHost::systemd(32 * 1024 * 1024, 1).unwrap())
    .unwrap();
    let harness = Harness {
        session,
        _root: root,
    };
    for (script, expected) in [
        (
            "setsid bash -c 'trap \"\" TERM; printf grandchild; exec sleep 100' & wait",
            CommandStatus::TimedOut,
        ),
        (
            "setsid bash -c 'printf grandchild; exec sleep 100' &",
            CommandStatus::Completed,
        ),
        ("kill -KILL $$", CommandStatus::Failed),
        ("exit 137", CommandStatus::Failed),
    ] {
        let command = format!("cat /proc/self/cgroup; {script}");
        let handle = harness.start(&command, 1).await;
        let (chunks, terminal) = harness.collect(&handle, 0).await;
        let stdout = stream_bytes(&chunks, CommandStream::Stdout);
        let text = std::str::from_utf8(&stdout).unwrap();
        let cgroup = text.lines().next().unwrap().strip_prefix("0::/").unwrap();
        assert!(
            !Path::new("/sys/fs/cgroup").join(cgroup).exists(),
            "scope cgroup survived cleanup"
        );
        assert_eq!(terminal.status, expected, "{terminal:?}");
        assert!(
            matches!(
                terminal.resource_outcome,
                Some(crate::CommandResourceOutcome::Systemd {
                    oom: crate::CommandOomObservation::NotObserved
                        | crate::CommandOomObservation::Unknown,
                    cleanup_confirmed: true,
                })
            ),
            "{terminal:?}"
        );
        assert!(terminal.metadata_valid(), "{terminal:?}");
        if script.starts_with("kill") {
            // systemd-run may report its child's signal as exit 137. Only the
            // launcher's actual wait signal is retained, never reconstructed.
            assert!(terminal.signal == Some(libc::SIGKILL) || terminal.exit_code == Some(137));
        }
        if script.starts_with("exit") {
            assert_eq!(terminal.exit_code, Some(137));
            assert_eq!(terminal.signal, None);
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_host_budget_refuses_commands_before_launch_across_workdirs() {
    let host = crate::CommandHost::systemd(32 * 1024 * 1024, 1).unwrap();
    let reservation = host.admit().unwrap();
    for _ in 0..2 {
        let root = TempDir::new().unwrap();
        let session =
            LocalWorkdirSession::new(Scope::writable(root.path()).unwrap(), root.path().into())
                .with_command_host(host.clone())
                .unwrap();
        let error = session
            .start_command(Harness::request("touch must-not-start", 1, 10))
            .await
            .unwrap_err();
        assert!(matches!(error, WorkdirError::Unavailable(_)));
        assert!(!root.path().join("must-not-start").exists());
    }
    drop(reservation);
    assert!(host.admit().is_ok());
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires a user systemd manager, cgroup v2 memory controller and user@.service OOMPolicy=continue"]
async fn systemd_scope_missing_launcher_never_falls_back_to_unconfined_execution() {
    let root = TempDir::new().unwrap();
    let session = LocalWorkdirSession::materialized_bound_with_environment(
        Workdir::new("missing-launcher"),
        root.path().into(),
        root.path().into(),
        SharedScope::new(Scope::writable(root.path()).unwrap()),
        WorkdirSessionCapabilities::ALL,
        BTreeMap::from([("PATH".into(), "/nonexistent-yoi-command-path".into())]),
        vec![],
    )
    .with_command_host(crate::CommandHost::systemd(32 * 1024 * 1024, 1).unwrap())
    .unwrap();
    let harness = Harness {
        session,
        _root: root,
    };
    let handle = harness.start("touch must-not-start", 5).await;
    let (_, terminal) = harness.collect(&handle, 0).await;
    assert_eq!(terminal.status, CommandStatus::Failed);
    assert_eq!(
        terminal.output_collection,
        CommandOutputCollection::NotStarted
    );
    assert_eq!(
        terminal.resource_outcome,
        Some(crate::CommandResourceOutcome::SetupUnconfirmed {
            oom: crate::CommandOomObservation::Unknown,
            cleanup_confirmed: true
        })
    );
    assert!(terminal.metadata_valid());
    assert!(!harness._root.path().join("must-not-start").exists());
}

// Re-exec only the exact current test: PATH injection is confined to that child,
// including systemd::control(), which intentionally uses the process environment.
// No application boundary, wrapper verification, or outcome classifier is mocked.
#[cfg(target_os = "linux")]
mod systemd_r2 {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const MEMORY: u64 = 32 * 1024 * 1024;

    fn executable(name: &str) -> PathBuf {
        std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .map(|path| path.join(name))
            .find(|path| {
                path.is_absolute()
                    && std::fs::metadata(path)
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
            // Preserve argv[0] applet selection (Nix sleep may link to coreutils).
            .unwrap_or_else(|| panic!("real {name} required"))
    }

    fn quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    // The parent owns the unit ledger and cleanup even when the child panics or
    // its overall deadline expires. Never run a wildcard stop/reset operation.
    struct Fixture {
        root: TempDir,
        systemctl: PathBuf,
    }

    impl Fixture {
        fn unit(&self) -> Option<String> {
            let unit = std::fs::read_to_string(self.root.path().join("unit")).ok()?;
            let id = unit.strip_prefix("yoi-command-")?.strip_suffix(".scope")?;
            (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(unit)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let Some(unit) = self.unit() else { return };
            for args in [
                vec!["kill", "--signal=KILL", "--kill-whom=all", unit.as_str()],
                vec!["stop", unit.as_str()],
                vec!["reset-failed", unit.as_str()],
            ] {
                let Ok(mut child) = std::process::Command::new(&self.systemctl)
                    .args(["--user", "--no-pager", "--no-ask-password"])
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    continue;
                };
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn isolated(test: &str, mode: &str) -> bool {
        if std::env::var("YOI_R2_CHILD").as_deref() == Ok(mode) {
            return false;
        }
        // module_path! includes the crate name; libtest's names do not.
        let test = test.split_once("::").expect("qualified test name").1;
        // Resolve every executable used by the fixture before changing child PATH.
        let bash = executable("bash");
        let run = executable("systemd-run");
        let systemctl = executable("systemctl");
        let setsid = executable("setsid");
        let sleep = executable("sleep");
        let fixture = Fixture {
            root: TempDir::new().unwrap(),
            systemctl,
        };
        let bin = fixture.root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let launcher = format!(
            "#!{}\n{}",
            bash.display(),
            r#"set -eu
args=("$@")
for i in "${!args[@]}"; do
    case "${args[$i]}" in
        --unit=yoi-command-*.scope) printf '%s' "${args[$i]#--unit=}" >"$YOI_R2_DIR/unit" ;;
    esac
    if [ "$YOI_R2_CHILD" = reject ] && [ "${args[$i]}" = yoi-command ]; then
        # Change only the expected memory value passed to the REAL wrapper.
        # The manager still creates the REAL scope with MemoryMax=32MiB/swap=0.
        args[$((i+2))]=33554433
    fi
done
exec "$YOI_R2_REAL_RUN" "${args[@]}"
"#
        );
        let controller = format!(
            "#!{}\n{}",
            bash.display(),
            r#"set -eu
if [ "$YOI_R2_CHILD" = uncertain ] && [ -f "$YOI_R2_DIR/unit" ]; then
    unit=$(<"$YOI_R2_DIR/unit")
    owned=false
    action=
    for arg in "$@"; do
        [ "$arg" != "$unit" ] || owned=true
        case "$arg" in show|kill|stop) action=$arg ;; esac
    done
    if $owned && [ -n "$action" ]; then
        printf '%s\n' "$action" >>"$YOI_R2_DIR/rejected-controls"
        exit 1
    fi
fi
# In particular, user@UID.service preflight is never replaced or rejected.
exec "$YOI_R2_REAL_CTL" "$@"
"#
        );
        for (name, body) in [("systemd-run", launcher), ("systemctl", controller)] {
            let path = bin.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                test,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PATH", path)
            .env("YOI_R2_CHILD", mode)
            .env("YOI_R2_DIR", fixture.root.path())
            .env("YOI_R2_REAL_RUN", run)
            .env("YOI_R2_REAL_CTL", &fixture.systemctl)
            .env("YOI_R2_REAL_BASH", bash)
            .env("YOI_R2_REAL_SETSID", setsid)
            .env("YOI_R2_REAL_SLEEP", sleep)
            .kill_on_drop(true);
        let result = tokio::time::timeout(Duration::from_secs(45), child.output()).await;
        // Cleanup bypasses shims, after (not before) the child's uncertainty and
        // permit assertions. It also runs if any assertion in the child failed.
        let cleanup_unit = fixture.unit();
        let real_systemctl = fixture.systemctl.clone();
        drop(fixture);
        if let Some(unit) = cleanup_unit {
            let output = tokio::time::timeout(
                Duration::from_secs(3),
                Command::new(real_systemctl)
                    .args([
                        "--user",
                        "show",
                        &unit,
                        "-p",
                        "LoadState",
                        "-p",
                        "ActiveState",
                    ])
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            let state = String::from_utf8(output.stdout).unwrap();
            eprintln!("R2 real cleanup {unit}: {state}");
            assert!(
                state.lines().any(|line| line == "LoadState=not-found"),
                "{state}"
            );
        }
        let output = result.expect("R2 subprocess deadline").unwrap();
        eprintln!(
            "R2 {test}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "{test}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // An incorrectly filtered subprocess must not silently pass with 0 tests.
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        true
    }

    fn unit() -> String {
        std::fs::read_to_string(Path::new(&std::env::var_os("YOI_R2_DIR").unwrap()).join("unit"))
            .expect("launcher captured the exact unit")
    }

    async fn properties(unit: &str) -> String {
        let mut command = Command::new(std::env::var_os("YOI_R2_REAL_CTL").unwrap());
        command
            .args([
                "--user",
                "--no-pager",
                "--no-ask-password",
                "show",
                unit,
                "-p",
                "LoadState",
                "-p",
                "ActiveState",
                "-p",
                "ControlGroup",
                "-p",
                "MemoryMax",
                "-p",
                "MemorySwapMax",
            ])
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(3), command.output())
            .await
            .expect("bounded real manager observation")
            .unwrap();
        // systemctl may return nonzero with LoadState=not-found after collection.
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.lines().any(|line| line.starts_with("LoadState=")),
            "{text}"
        );
        text
    }

    async fn active_scope() -> PathBuf {
        let unit = unit();
        let text = properties(&unit).await;
        for property in [
            "LoadState=loaded",
            "ActiveState=active",
            "MemoryMax=33554432",
            "MemorySwapMax=0",
        ] {
            assert!(text.lines().any(|line| line == property), "{text}");
        }
        let group = text
            .lines()
            .find_map(|line| line.strip_prefix("ControlGroup="))
            .unwrap();
        assert!(group.ends_with(&format!("/app.slice/{unit}")), "{group}");
        let path = Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
        assert!(path.exists(), "real scope cgroup must exist");
        path
    }

    async fn gone(unit: &str, cgroup: Option<&Path>) {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let text = properties(unit).await;
                let inactive = text.lines().any(|line| {
                    matches!(
                        line,
                        "LoadState=not-found" | "ActiveState=inactive" | "ActiveState=failed"
                    )
                });
                let empty = cgroup.is_none_or(|path| {
                    !path.exists()
                        || std::fs::read_to_string(path.join("cgroup.events"))
                            .is_ok_and(|text| text.lines().any(|line| line == "populated 0"))
                });
                if inactive && empty {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("exact unit and detached processes must be gone");
    }

    fn harness(host: Arc<crate::CommandHost>) -> Harness {
        let root = TempDir::new().unwrap();
        let session =
            LocalWorkdirSession::new(Scope::writable(root.path()).unwrap(), root.path().into())
                .with_command_host(host)
                .unwrap();
        Harness {
            session,
            _root: root,
        }
    }

    fn detached_script() -> String {
        let real = |name| quote(&std::env::var(name).unwrap());
        let inner = format!("printf ready; exec {} 15", real("YOI_R2_REAL_SLEEP"));
        format!(
            "{} {} -c {} & wait",
            real("YOI_R2_REAL_SETSID"),
            real("YOI_R2_REAL_BASH"),
            quote(&inner)
        )
    }

    async fn retained_output(
        transport: &Arc<Mutex<Transport>>,
        handle: &CommandHandle,
    ) -> (Vec<crate::CommandByteChunk>, crate::CommandTerminal) {
        tokio::time::timeout(Duration::from_secs(12), async {
            let mut chunks = Vec::new();
            let mut cursor = 0;
            loop {
                let page = transport
                    .lock()
                    .await
                    .read(&CommandOutputRequest {
                        handle: handle.clone(),
                        cursor,
                        limit: 4096,
                        wait: true,
                    })
                    .await
                    .unwrap();
                cursor = page.next_cursor;
                chunks.extend(page.chunks);
                if let Some(terminal) = page.terminal {
                    return (chunks, terminal);
                }
            }
        })
        .await
        .expect("bounded retained output")
    }

    #[tokio::test]
    #[ignore = "real user manager/cgroup v2; reexecs exact test with child-only PATH shims"]
    async fn rejected_preflight_after_real_scope_creation_has_no_script_side_effect() {
        if isolated(
            concat!(
                module_path!(),
                "::rejected_preflight_after_real_scope_creation_has_no_script_side_effect"
            ),
            "reject",
        )
        .await
        {
            return;
        }
        let host = crate::CommandHost::systemd(MEMORY, 1).unwrap();
        let harness = harness(host.clone());
        let handle = harness.start(": > must-not-start", 15).await;
        let (chunks, terminal) = harness.collect(&handle, 0).await;
        assert_eq!(terminal.status, CommandStatus::Failed);
        assert_eq!(terminal.exit_code, Some(125));
        assert_eq!(terminal.stdout_bytes, 0);
        assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"");
        let stderr = stream_bytes(&chunks, CommandStream::Stderr);
        // This diagnostic follows real membership and controller-file reads in
        // WRAPPER; a missing launcher or a pre-creation refusal cannot satisfy it.
        assert!(
            String::from_utf8_lossy(&stderr)
                .contains("Workdir isolation: memory limits or OOM group policy do not match")
        );
        assert_eq!(terminal.stderr_bytes, stderr.len() as u64);
        assert_eq!(terminal.stderr_delivered_bytes, stderr.len() as u64);
        assert_eq!(
            terminal.output_collection,
            CommandOutputCollection::Complete
        );
        assert!(
            matches!(
                terminal.resource_outcome,
                Some(crate::CommandResourceOutcome::SetupUnconfirmed {
                    cleanup_confirmed: true,
                    ..
                })
            ),
            "{terminal:?}"
        );
        assert!(terminal.metadata_valid());
        assert!(!harness._root.path().join("must-not-start").exists());
        gone(&unit(), None).await;
        assert!(
            host.admit().is_ok(),
            "confirmed cleanup releases the permit"
        );
    }

    #[tokio::test]
    #[ignore = "real user manager/cgroup v2; reexecs exact test with child-only PATH shims"]
    async fn manager_failures_retain_cleanup_uncertainty_and_host_reservation() {
        if isolated(
            concat!(
                module_path!(),
                "::manager_failures_retain_cleanup_uncertainty_and_host_reservation"
            ),
            "uncertain",
        )
        .await
        {
            return;
        }
        let host = crate::CommandHost::systemd(MEMORY, 1).unwrap();
        let harness = harness(host.clone());
        let handle = harness.start(&detached_script(), 15).await;
        let first = harness.page(&handle, 0, 5).await;
        assert_eq!(stream_bytes(&first.chunks, CommandStream::Stdout), b"ready");
        let cgroup = active_scope().await;
        harness
            .session
            .cancel_command(handle.clone())
            .await
            .unwrap();
        let (chunks, terminal) = harness.collect(&handle, 0).await;
        assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"ready");
        assert_eq!(terminal.status, CommandStatus::Cancelled);
        assert_eq!(
            terminal.resource_outcome,
            Some(crate::CommandResourceOutcome::Systemd {
                oom: crate::CommandOomObservation::Unknown,
                cleanup_confirmed: false,
            })
        );
        assert!(
            matches!(
                terminal.output_collection,
                CommandOutputCollection::Incomplete {
                    failure: CommandFailure {
                        stage: CommandFailureStage::OutputDrain,
                        reason: CommandIoErrorKind::TimedOut,
                        ..
                    }
                }
            ),
            "{terminal:?}"
        );
        assert_eq!(terminal.stdout_bytes, 5);
        assert_eq!(terminal.stdout_delivered_bytes, 5);
        assert!(terminal.metadata_valid());
        let log = std::fs::read_to_string(
            Path::new(&std::env::var_os("YOI_R2_DIR").unwrap()).join("rejected-controls"),
        )
        .unwrap();
        for action in ["show", "kill", "stop"] {
            assert!(log.lines().any(|line| line == action), "{log}");
        }
        // Bypass the shim to prove this is a live unit, not just fake metadata.
        assert_eq!(active_scope().await, cgroup);
        assert!(
            host.admit().is_err(),
            "uncertain cleanup must retain the host permit"
        );
        let error = harness
            .session
            .start_command(Harness::request(": > refused", 15, 100))
            .await
            .unwrap_err();
        assert!(matches!(error, WorkdirError::Unavailable(_)));
        assert!(!harness._root.path().join("refused").exists());
        harness.session.close().await.unwrap();
        assert!(
            host.admit().is_err(),
            "closing a session cannot forgive uncertainty"
        );
        // Only now bypass the failures for fixture cleanup. Parent RAII remains
        // the panic/deadline fallback, so a failed assertion cannot leak the unit.
        let unit = unit();
        for args in [
            vec!["kill", "--signal=KILL", "--kill-whom=all", unit.as_str()],
            vec!["stop", unit.as_str()],
        ] {
            let mut command = Command::new(std::env::var_os("YOI_R2_REAL_CTL").unwrap());
            command
                .args(["--user", "--no-pager", "--no-ask-password"])
                .args(args)
                .kill_on_drop(true);
            // Successful kill may unload the scope before stop reaches the
            // manager. The real postcondition below, not both exit codes,
            // establishes cleanup of the exact observed live cgroup.
            let status = tokio::time::timeout(Duration::from_secs(3), command.status())
                .await
                .expect("bounded real fixture cleanup")
                .unwrap();
            eprintln!("R2 real cleanup request: {status}");
        }
        gone(&unit, Some(&cgroup)).await;
        assert!(
            host.admit().is_err(),
            "external cleanup is not provider confirmation"
        );
    }

    #[tokio::test]
    #[ignore = "real user manager/cgroup v2; reexecs exact test with child-only PATH shims"]
    async fn scoped_sender_receiver_and_session_loss_kill_unit_without_inventing_terminal() {
        if isolated(
            concat!(
                module_path!(),
                "::scoped_sender_receiver_and_session_loss_kill_unit_without_inventing_terminal"
            ),
            "loss",
        )
        .await
        {
            return;
        }
        for loss in ["sender", "receiver", "session"] {
            let host = crate::CommandHost::systemd(MEMORY, 1).unwrap();
            let harness = harness(host.clone());
            let handle = harness.start(&detached_script(), 15).await;
            let first = harness.page(&handle, 0, 5).await;
            assert_eq!(stream_bytes(&first.chunks, CommandStream::Stdout), b"ready");
            let cgroup = active_scope().await;
            let unit = unit();
            let (transport, mut status) = {
                let commands = harness.session.inner.commands.lock().await;
                let command = &commands[&handle.0];
                (command.transport.clone(), command.status.clone())
            };
            if loss == "session" {
                // Inner Drop aborts (it does not gracefully cancel) its tasks.
                // Retain existing bytes, but do NOT invent a terminal fence.
                drop(harness);
                tokio::time::timeout(Duration::from_secs(5), status.changed())
                    .await
                    .unwrap()
                    .unwrap_err();
                assert!(status.borrow().is_none());
                assert!(
                    host.admit().is_err(),
                    "aborted cleanup has no confirmation to release its permit"
                );
                let mut transport = transport.lock().await;
                let replay = transport
                    .read(&CommandOutputRequest {
                        handle: handle.clone(),
                        cursor: 0,
                        limit: 5,
                        wait: true,
                    })
                    .await
                    .unwrap();
                assert_eq!(replay, first);
                let error = tokio::time::timeout(
                    Duration::from_secs(5),
                    transport.read(&CommandOutputRequest {
                        handle: handle.clone(),
                        cursor: 5,
                        limit: 4096,
                        wait: true,
                    }),
                )
                .await
                .unwrap()
                .unwrap_err();
                assert_eq!(
                    error.failure_reason(),
                    Some(crate::WorkdirFailureReason::CommandTerminalMissing)
                );
            } else {
                let task = if loss == "sender" {
                    // Dropping LocalCommand alone does not abort its JoinHandle.
                    // Remove it from the session so the last cancel sender drops.
                    let command = harness
                        .session
                        .inner
                        .commands
                        .lock()
                        .await
                        .remove(&handle.0)
                        .unwrap();
                    let LocalCommand { task, cancel, .. } = command;
                    drop(cancel);
                    Some(task)
                } else {
                    transport.lock().await.close();
                    None
                };
                let (chunks, terminal) = retained_output(&transport, &handle).await;
                assert_eq!(stream_bytes(&chunks, CommandStream::Stdout), b"ready");
                assert!(
                    matches!(
                        terminal.status,
                        CommandStatus::Cancelled | CommandStatus::Failed
                    ),
                    "{terminal:?}"
                );
                if loss == "sender" {
                    assert_eq!(terminal.status, CommandStatus::Cancelled);
                    assert_eq!(
                        terminal.output_collection,
                        CommandOutputCollection::Complete
                    );
                } else {
                    assert!(
                        matches!(
                            terminal.output_collection,
                            CommandOutputCollection::Complete
                                | CommandOutputCollection::Incomplete {
                                    failure: CommandFailure {
                                        stage: CommandFailureStage::OutputDelivery,
                                        reason: CommandIoErrorKind::Unknown,
                                        os_code: None,
                                    },
                                }
                        ),
                        "{terminal:?}"
                    );
                    if terminal.status == CommandStatus::Failed {
                        assert!(matches!(
                            terminal.output_collection,
                            CommandOutputCollection::Incomplete { .. }
                        ));
                    }
                }
                assert!(
                    matches!(
                        terminal.resource_outcome,
                        Some(crate::CommandResourceOutcome::Systemd {
                            oom: crate::CommandOomObservation::NotObserved
                                | crate::CommandOomObservation::Unknown,
                            cleanup_confirmed: true,
                        })
                    ),
                    "{terminal:?}"
                );
                assert_eq!(terminal.stdout_bytes, 5);
                assert_eq!(terminal.stdout_delivered_bytes, 5);
                assert_eq!(terminal.truncation_reason, None);
                assert!(terminal.metadata_valid());
                if let Some(task) = task {
                    task.await.unwrap();
                }
                harness.session.close().await.unwrap();
                assert!(host.admit().is_ok());
            }
            gone(&unit, Some(&cgroup)).await;
        }
    }
}
