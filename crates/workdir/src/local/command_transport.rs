//! Captured process bytes use a bounded queue, never the lossy Console channel.
//! Pipe backpressure bounds unread process output without temporary-file growth.
//! A replay page survives a lost HTTP response until its next cursor is sent.

use super::*;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

const QUEUE_CHUNKS: usize = 8;
pub(super) const MAX_COMMANDS: usize = 64;

#[derive(Debug)]
enum Message {
    Bytes(crate::CommandByteChunk),
}

#[derive(Debug)]
pub(super) struct LocalCommand {
    pub task: JoinHandle<()>,
    pub cancel: watch::Sender<bool>,
    pub status: watch::Receiver<Option<crate::CommandTerminal>>,
    pub transport: Arc<Mutex<Transport>>,
}

#[derive(Debug)]
pub(super) struct Transport {
    receiver: mpsc::Receiver<Message>,
    pending: Option<crate::CommandByteChunk>,
    cursor: u64,
    replay: Option<(u64, CommandOutput)>,
    terminal: Option<crate::CommandTerminal>,
    terminal_status: watch::Receiver<Option<crate::CommandTerminal>>,
}

impl Transport {
    pub async fn read(
        &mut self,
        request: &CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        if !(1..=crate::MAX_COMMAND_READ_BYTES).contains(&request.limit) {
            return Err(WorkdirError::Conflict(
                "command byte limit is outside transport bounds".into(),
            ));
        }
        if let Some((cursor, page)) = &self.replay {
            if request.cursor == *cursor {
                if page
                    .chunks
                    .iter()
                    .map(|chunk| chunk.bytes.len())
                    .sum::<usize>()
                    > request.limit
                {
                    return Err(WorkdirError::Conflict(
                        "retry byte limit is smaller than retained page".into(),
                    ));
                }
                return Ok(page.clone());
            }
            if request.cursor != page.next_cursor {
                return Err(WorkdirError::Conflict(
                    "command cursor is not the current page acknowledgement".into(),
                ));
            }
            self.cursor = page.next_cursor;
            self.replay = None;
        }
        if request.cursor != self.cursor {
            return Err(WorkdirError::Conflict(
                "command cursor was acknowledged or is ahead of output".into(),
            ));
        }
        let mut page = CommandOutput {
            chunks: Vec::new(),
            next_cursor: self.cursor,
            terminal: self.terminal.clone(),
        };
        let mut remaining = request.limit;
        loop {
            let message = if let Some(chunk) = self.pending.take() {
                Some(Message::Bytes(chunk))
            } else if page.chunks.is_empty() && page.terminal.is_none() && request.wait {
                self.receiver.recv().await
            } else {
                match self.receiver.try_recv() {
                    Ok(message) => Some(message),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => None,
                }
            };
            match message {
                Some(Message::Bytes(mut chunk)) => {
                    let count = remaining.min(chunk.bytes.len());
                    if count < chunk.bytes.len() {
                        let tail = chunk.bytes.split_off(count);
                        self.pending = Some(crate::CommandByteChunk {
                            sequence: chunk.sequence + count as u64,
                            stream: chunk.stream,
                            offset: chunk.offset + count as u64,
                            bytes: tail,
                        });
                    }
                    page.next_cursor += count as u64;
                    remaining -= count;
                    page.chunks.push(chunk);
                    if remaining == 0 {
                        break;
                    }
                }
                None if page.terminal.is_some() => break,
                None => {
                    let terminal = self.terminal_status.borrow().clone().ok_or_else(|| {
                        WorkdirError::Unavailable(
                            "command transport ended before terminal fence".into(),
                        )
                        .with_failure_reason(crate::WorkdirFailureReason::CommandTerminalMissing)
                    })?;
                    self.terminal = Some(terminal.clone());
                    page.terminal = Some(terminal);
                    break;
                }
            }
        }
        // An empty poll must not become a permanently replayed empty page.
        if !page.chunks.is_empty() || page.terminal.is_some() {
            self.replay = Some((self.cursor, page.clone()));
        }
        Ok(page)
    }

    pub fn terminal_delivered(&self) -> bool {
        self.replay
            .as_ref()
            .is_some_and(|(_, page)| page.terminal.is_some())
    }

    pub fn close(&mut self) {
        self.receiver.close();
    }
}

pub(super) fn start(
    cwd: PathBuf,
    request: CommandRequest,
    command_id: String,
    telemetry: CommandTelemetry,
    environment: BTreeMap<String, String>,
    resources: Vec<Arc<dyn WorkdirSessionResource>>,
) -> LocalCommand {
    let (sender, receiver) = mpsc::channel(QUEUE_CHUNKS);
    let (cancel, cancel_rx) = watch::channel(false);
    let (status_tx, status) = watch::channel(None);
    let transport = Arc::new(Mutex::new(Transport {
        receiver,
        pending: None,
        cursor: 0,
        replay: None,
        terminal: None,
        terminal_status: status.clone(),
    }));
    let task = tokio::spawn(async move {
        let _resources = resources;
        let terminal = execute(
            cwd,
            request,
            &command_id,
            &telemetry,
            environment,
            cancel_rx,
            &sender,
        )
        .await;
        telemetry.terminal(&command_id, terminal.status, terminal.exit_code);
        // Closing the byte queue fences this already-published terminal, without
        // requiring spare queue capacity or a receiver to release task resources.
        let _ = status_tx.send(Some(terminal));
    });
    LocalCommand {
        task,
        cancel,
        status,
        transport,
    }
}

#[derive(Default)]
struct ByteAccounting {
    stdout: u64,
    stderr: u64,
    stdout_delivered: u64,
    stderr_delivered: u64,
}

impl ByteAccounting {
    fn terminal(&self, status: CommandStatus, exit_code: Option<i32>) -> crate::CommandTerminal {
        crate::CommandTerminal {
            status,
            exit_code,
            stdout_bytes: self.stdout,
            stderr_bytes: self.stderr,
            stdout_delivered_bytes: self.stdout_delivered,
            stderr_delivered_bytes: self.stderr_delivered,
            truncation_reason: (self.stdout > self.stdout_delivered
                || self.stderr > self.stderr_delivered)
                .then_some(crate::CommandOutputTruncation::ByteBudget),
        }
    }
}

async fn pump<R: AsyncRead + Unpin>(
    mut stdout: R,
    mut stderr: impl AsyncRead + Unpin,
    sender: &mpsc::Sender<Message>,
    telemetry: &CommandTelemetry,
    command_id: &str,
    mut remaining: u64,
    accounting: &mut ByteAccounting,
) -> std::io::Result<()> {
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_decoder = CommandOutputDecoder::default();
    let mut stderr_decoder = CommandOutputDecoder::default();
    let mut sequence = 0;
    let mut stdout_buffer = [0; COMMAND_EVENT_CHUNK_BYTES];
    let mut stderr_buffer = [0; COMMAND_EVENT_CHUNK_BYTES];
    while !stdout_done || !stderr_done {
        // After quota, drain locally even if nobody acknowledges the queue.
        // Before quota, reserve first to retain bounded pipe backpressure.
        let permit = if remaining > 0 {
            let Ok(permit) = sender.reserve().await else {
                return Ok(());
            };
            Some(permit)
        } else {
            None
        };
        let (stream, result) = tokio::select! {
            result = stdout.read(&mut stdout_buffer), if !stdout_done => (CommandStream::Stdout, result),
            result = stderr.read(&mut stderr_buffer), if !stderr_done => (CommandStream::Stderr, result),
        };
        let buffer = match stream {
            CommandStream::Stdout => &stdout_buffer,
            CommandStream::Stderr => &stderr_buffer,
        };
        let count = result?;
        let decoder = match stream {
            CommandStream::Stdout => &mut stdout_decoder,
            CommandStream::Stderr => &mut stderr_decoder,
        };
        if count == 0 {
            match stream {
                CommandStream::Stdout => stdout_done = true,
                CommandStream::Stderr => stderr_done = true,
            }
            publish_decoded_output(decoder, telemetry, command_id, stream, true);
            continue;
        }
        let (seen, delivered) = match stream {
            CommandStream::Stdout => (&mut accounting.stdout, &mut accounting.stdout_delivered),
            CommandStream::Stderr => (&mut accounting.stderr, &mut accounting.stderr_delivered),
        };
        *seen = seen.saturating_add(count as u64);
        let captured = remaining.min(count as u64) as usize;
        if captured > 0 {
            let offset = *delivered;
            decoder.read_offset += captured as u64;
            decoder.pending.extend_from_slice(&buffer[..captured]);
            publish_decoded_output(decoder, telemetry, command_id, stream, false);
            permit
                .expect("reserved while budget remains")
                .send(Message::Bytes(crate::CommandByteChunk {
                    sequence,
                    stream,
                    offset,
                    bytes: buffer[..captured].to_vec(),
                }));
            *delivered += captured as u64;
            sequence += captured as u64;
            remaining -= captured as u64;
        }
    }
    Ok(())
}

async fn execute(
    cwd: PathBuf,
    request: CommandRequest,
    command_id: &str,
    telemetry: &CommandTelemetry,
    environment: BTreeMap<String, String>,
    mut cancel: watch::Receiver<bool>,
    sender: &mpsc::Sender<Message>,
) -> crate::CommandTerminal {
    telemetry.started(command_id, request.tool_call_id);
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(request.command)
        .current_dir(cwd)
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    let mut accounting = ByteAccounting::default();
    let Ok(mut child) = command.spawn() else {
        return accounting.terminal(CommandStatus::Failed, None);
    };
    let mut process_group = CommandProcessGroupGuard::new(child.id());
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (status, exit_code) = {
        let output = pump(
            stdout,
            stderr,
            sender,
            telemetry,
            command_id,
            request.transport_byte_budget,
            &mut accounting,
        );
        tokio::pin!(output);
        let mut drained = false;
        let timeout = tokio::time::sleep(Duration::from_secs(request.timeout_secs.max(1)));
        tokio::pin!(timeout);
        let terminal = loop {
            tokio::select! {
                result = &mut output, if !drained => {
                    drained = true;
                    if result.is_err() || sender.is_closed() {
                        process_group.terminate();
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        break (CommandStatus::Failed, None);
                    }
                }
                exit = child.wait() => {
                    process_group.terminate();
                    break match exit {
                        Ok(exit) => (if exit.success() { CommandStatus::Completed } else { CommandStatus::Failed }, exit.code()),
                        Err(_) => (CommandStatus::Failed, None),
                    };
                }
                _ = &mut timeout => {
                    process_group.terminate();
                    let _ = child.start_kill();
                    let code = child.wait().await.ok().and_then(|exit| exit.code());
                    break (CommandStatus::TimedOut, code);
                }
                _ = sender.closed() => {
                    process_group.terminate();
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    break (CommandStatus::Cancelled, None);
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        process_group.terminate();
                        let _ = child.start_kill();
                        let code = child.wait().await.ok().and_then(|exit| exit.code());
                        break (CommandStatus::Cancelled, code);
                    }
                }
            }
        };
        // Even after kill, preserve the bounded pipe tail before publishing terminal.
        if !drained {
            let _ = output.await;
        }
        terminal
    };
    accounting.terminal(status, exit_code)
}

#[cfg(test)]
mod tests;
