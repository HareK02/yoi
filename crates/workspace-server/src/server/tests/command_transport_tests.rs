//! Provider-bridge lifecycle tests. The production bridge runs against a controlled
//! provider boundary; the websocket/CLI-pump test in the parent module covers real byte transport.
use super::*;
use workdir::WorkdirSession;

fn command_fixture() -> (
    Arc<ExternalProviderWorkdirSession>,
    tokio::sync::mpsc::Receiver<ExternalProviderCommand>,
) {
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    let session = Arc::new(ExternalProviderWorkdirSession::new(
        Arc::new(ExternalProviderConnection {
            grant_id: "command-grant".into(),
            workdir_id: "command-workdir".into(),
            provider_instance_id: "command-provider".into(),
            generation: 1,
            expires_at: None,
            capabilities: workdir::WorkdirSessionCapabilities::COMMAND_ONLY,
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
        }),
        None,
    ));
    (session, receiver)
}

fn page(cursor: u64, bytes: &[u8], terminal: bool) -> workdir::CommandOutput {
    workdir::CommandOutput {
        chunks: if bytes.is_empty() {
            Vec::new()
        } else {
            vec![workdir::CommandByteChunk {
                sequence: cursor,
                stream: workdir::CommandStream::Stdout,
                offset: cursor,
                bytes: bytes.to_vec(),
            }]
        },
        next_cursor: cursor + bytes.len() as u64,
        terminal: terminal.then_some(workdir::CommandTerminal {
            process_failure: None,
            output_collection: workdir::CommandOutputCollection::Complete,
            status: workdir::CommandStatus::Cancelled,
            exit_code: None,
            stdout_bytes: cursor + bytes.len() as u64,
            stderr_bytes: 0,
            stdout_delivered_bytes: cursor + bytes.len() as u64,
            stderr_delivered_bytes: 0,
            truncation_reason: None,
        }),
    }
}

async fn receive(
    receiver: &mut tokio::sync::mpsc::Receiver<ExternalProviderCommand>,
) -> (
    WorkdirSessionOperation,
    tokio::sync::oneshot::Sender<
        std::result::Result<WorkdirSessionOperationResult, WorkdirTransportError>,
    >,
) {
    let command = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let ExternalProviderCommand::Operation {
        operation,
        response,
        ..
    } = command
    else {
        panic!("expected provider operation")
    };
    (operation, response)
}

#[tokio::test]
async fn close_cancels_waiting_reader_then_drains_from_latest_cursor() {
    let (session, mut receiver) = command_fixture();
    session.active_commands.lock().await.insert(
        "external".into(),
        Arc::new(ExternalProviderCommandState::new(CommandHandle(
            "provider".into(),
        ))),
    );
    let reader = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .command_output(workdir::CommandOutputRequest {
                    handle: CommandHandle("external".into()),
                    cursor: 0,
                    limit: 3,
                    wait: true,
                })
                .await
        })
    };
    let (_, response) = receive(&mut receiver).await;
    response
        .send(Ok(WorkdirSessionOperationResult::CommandOutput(page(
            0, b"one", false,
        ))))
        .unwrap();
    assert_eq!(reader.await.unwrap().unwrap().next_cursor, 3);

    let reader = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .command_output(workdir::CommandOutputRequest {
                    handle: CommandHandle("external".into()),
                    cursor: 3,
                    limit: 3,
                    wait: true,
                })
                .await
        })
    };
    let (_, pending_read) = receive(&mut receiver).await;
    let closer = {
        let session = session.clone();
        tokio::spawn(async move { session.close().await })
    };
    let (operation, response) = receive(&mut receiver).await;
    assert!(
        matches!(operation, WorkdirSessionOperation::CommandCancel(ref handle) if handle.0 == "provider")
    );
    response
        .send(Ok(WorkdirSessionOperationResult::CommandCancel))
        .unwrap();
    pending_read
        .send(Ok(WorkdirSessionOperationResult::CommandOutput(page(
            3, b"two", false,
        ))))
        .unwrap();
    assert_eq!(reader.await.unwrap().unwrap().next_cursor, 6);

    for (cursor, bytes, terminal) in [(6, &b"end"[..], false), (9, &b""[..], true)] {
        let (operation, response) = receive(&mut receiver).await;
        assert!(
            matches!(operation, WorkdirSessionOperation::CommandOutput(ref request) if request.handle.0 == "provider" && request.cursor == cursor && request.wait)
        );
        response
            .send(Ok(WorkdirSessionOperationResult::CommandOutput(page(
                cursor, bytes, terminal,
            ))))
            .unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), closer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn terminal_output_retains_same_binding_for_current_page_replay() {
    let (session, mut receiver) = command_fixture();
    session.active_commands.lock().await.insert(
        "external".into(),
        Arc::new(ExternalProviderCommandState::new(CommandHandle(
            "provider".into(),
        ))),
    );
    for _ in 0..2 {
        let reader = {
            let session = session.clone();
            tokio::spawn(async move {
                session
                    .command_output(workdir::CommandOutputRequest {
                        handle: CommandHandle("external".into()),
                        cursor: 0,
                        limit: 3,
                        wait: true,
                    })
                    .await
            })
        };
        let (operation, response) = receive(&mut receiver).await;
        assert!(
            matches!(operation, WorkdirSessionOperation::CommandOutput(ref request) if request.handle.0 == "provider" && request.cursor == 0)
        );
        response
            .send(Ok(WorkdirSessionOperationResult::CommandOutput(page(
                0, b"end", true,
            ))))
            .unwrap();
        assert_eq!(reader.await.unwrap().unwrap(), page(0, b"end", true));
    }
    assert_eq!(session.active_commands.lock().await.len(), 1);
}

#[tokio::test]
async fn terminal_registry_retention_is_bounded_and_excludes_active_commands() {
    let (source, _) = command_fixture();
    let worker = RuntimeWorkerRef::new("runtime", "worker");
    let mut registry = WorkdirSessionRegistry::default();
    let active = registry.register_command(
        worker.clone(),
        "checkout".into(),
        source.clone(),
        CommandHandle("active".into()),
    );
    let mut last = None;
    for index in 0..MAX_RETAINED_TERMINAL_COMMANDS + 4 {
        let handle = registry.register_command(
            worker.clone(),
            "checkout".into(),
            source.clone(),
            CommandHandle(format!("terminal-{index}")),
        );
        registry.mark_terminal(&worker, &handle);
        last = Some(handle);
    }
    assert_eq!(registry.active_command_count(&worker), 1);
    assert_eq!(registry.commands.len(), MAX_RETAINED_TERMINAL_COMMANDS + 1);
    assert!(registry.command(&worker, "checkout", &active).is_some());
    assert!(
        registry
            .command(&worker, "checkout", &last.unwrap())
            .is_some()
    );
}
