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
                "printf ready-extra; head -c 32768 /dev/zero; exec tail -f /dev/null",
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
