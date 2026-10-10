//! Reconnect projection derived only from the parent's published child stream.
//!
//! Records are prepared before the first child Run. Their initial projection and
//! receiver are captured at that boundary, not when forwarding eventually starts.
//! Later source reads may resolve one immutable committed entry by ID, but never
//! replace live state with the child's (possibly unpublished) current state.

use crate::internal_worker::InternalWorkerSessionHandle;
use protocol::{Event, InFlightBlock, InternalWorkerSnapshot, SessionSnapshotEntryData as Data};
use std::sync::Mutex;
use tokio::sync::broadcast;

pub(super) struct PublishedProtocol {
    pub snapshot: Mutex<InternalWorkerSnapshot>,
    pub events: Mutex<Option<broadcast::Receiver<Event>>>,
}

impl PublishedProtocol {
    /// The host registers a prepared Idle session before its first Run.
    pub fn new(source: &InternalWorkerSessionHandle, worker: protocol::InternalWorkerRef) -> Self {
        let events = source.subscribe_events();
        let initial = source.protocol_snapshot();
        Self {
            snapshot: Mutex::new(InternalWorkerSnapshot {
                worker,
                session: initial.session,
                greeting: initial.greeting,
                status: initial.status,
                error: initial.error,
                in_flight: initial.in_flight,
                internal_workers: initial.internal_workers,
            }),
            events: Mutex::new(Some(events)),
        }
    }
}

pub(super) fn apply(
    snapshot: &mut InternalWorkerSnapshot,
    event: &Event,
    source: &InternalWorkerSessionHandle,
) {
    match event {
        Event::Snapshot {
            session,
            greeting,
            state,
            in_flight,
            internal_workers,
        } => {
            snapshot.session = session.clone();
            snapshot.greeting = Some(greeting.clone());
            snapshot.status = state.catalog_status();
            snapshot.in_flight = in_flight.clone();
            snapshot.internal_workers = internal_workers.clone();
        }
        Event::WorkerState { snapshot: state } => {
            snapshot.status = state.catalog_status();
            if snapshot.status != protocol::WorkerStatus::Running {
                snapshot.in_flight = Default::default();
            } else {
                snapshot.error = None;
            }
        }
        Event::Shutdown => {
            snapshot.status = protocol::WorkerStatus::Stopped;
            snapshot.in_flight = Default::default();
        }
        Event::Error { message, .. } => snapshot.error = Some(message.clone()),
        Event::ContextUsage { usage } => {
            if let Some(greeting) = &mut snapshot.greeting {
                greeting.context_usage = *usage;
                greeting.context_tokens = usage.map_or(0, |usage| usage.tokens);
            }
        }
        Event::PendingSubmissionsChanged { pending } => {
            snapshot.session.pending_submissions = pending.clone()
        }
        Event::SegmentRotated { session } => {
            snapshot.session = session.clone();
            snapshot.in_flight = Default::default();
        }
        Event::SessionEntryCommitted { entry } => {
            match &entry.data {
                Data::ToolCall { call_id, .. } => snapshot.in_flight.blocks.retain(
                    |block| !matches!(block, InFlightBlock::ToolCall { id, .. } if id == call_id),
                ),
                Data::Message {
                    role: protocol::SessionMessageRole::Assistant,
                    content,
                } => {
                    let body = content
                        .iter()
                        .map(|part| match part {
                            protocol::SessionContentPart::Text { text } => text.as_str(),
                            protocol::SessionContentPart::Refusal { refusal } => refusal.as_str(),
                        })
                        .collect::<String>();
                    if let Some(index) = snapshot.in_flight.blocks.iter().position(
                        |block| matches!(block, InFlightBlock::Text { text, .. } if text == &body),
                    ) {
                        snapshot.in_flight.blocks.remove(index);
                    }
                }
                _ => {}
            }
            if let Some(existing) = snapshot
                .session
                .entries
                .iter_mut()
                .find(|old| old.entry_id == entry.entry_id)
            {
                *existing = entry.clone();
            } else {
                snapshot.session.entries.push(entry.clone());
            }
        }
        Event::UserMessage {
            entry_id: Some(id), ..
        }
        | Event::SystemItem {
            entry_id: Some(id), ..
        } => {
            // Resolve only this committed identity. Reading the entire current
            // source projection here would admit future/buffered history.
            if !snapshot
                .session
                .entries
                .iter()
                .any(|entry| &entry.entry_id == id)
            {
                if let Some(entry) = committed_entries(source, &snapshot.worker.session_id)
                    .into_iter()
                    .find(|entry| &entry.entry_id == id)
                {
                    snapshot.session.entries.push(entry);
                }
            }
        }
        Event::InvokeStart {
            timestamp_ms: Some(ts),
            ..
        }
        | Event::Usage {
            timestamp_ms: Some(ts),
            ..
        } => {
            if let Some(entry) = committed_entries(source, &snapshot.worker.session_id)
                .into_iter()
                .find(|entry| {
                    entry.timestamp == *ts
                        && !snapshot
                            .session
                            .entries
                            .iter()
                            .any(|old| old.entry_id == entry.entry_id)
                        && match (&entry.data, event) {
                            (Data::Invoke { trigger }, Event::InvokeStart { kind, .. }) => {
                                trigger == kind
                            }
                            (
                                Data::Usage {
                                    input_tokens,
                                    cache_read_input_tokens,
                                    output_tokens,
                                },
                                Event::Usage {
                                    input_tokens: input,
                                    cache_read_input_tokens: cache,
                                    output_tokens: output,
                                    ..
                                },
                            ) => {
                                *input_tokens == input.unwrap_or_default()
                                    && *cache_read_input_tokens == cache.unwrap_or_default()
                                    && *output_tokens == output.unwrap_or_default()
                            }
                            _ => false,
                        }
                })
            {
                snapshot.session.entries.push(entry);
            }
        }
        Event::TextDelta { text } => match snapshot.in_flight.blocks.last_mut() {
            Some(InFlightBlock::Text {
                text: body,
                finished: false,
            }) => body.push_str(text),
            _ => snapshot.in_flight.blocks.push(InFlightBlock::Text {
                text: text.clone(),
                finished: false,
            }),
        },
        Event::TextDone { text } => {
            if let Some(InFlightBlock::Text {
                text: body,
                finished,
            }) = snapshot.in_flight.blocks.last_mut()
            {
                *body = text.clone();
                *finished = true;
            } else {
                snapshot.in_flight.blocks.push(InFlightBlock::Text {
                    text: text.clone(),
                    finished: true,
                });
            }
        }
        Event::ThinkingStart => snapshot.in_flight.blocks.push(InFlightBlock::Thinking {
            text: String::new(),
            finished: false,
        }),
        Event::ThinkingDelta { text } => {
            if let Some(InFlightBlock::Thinking { text: body, .. }) =
                snapshot.in_flight.blocks.last_mut()
            {
                body.push_str(text);
            }
        }
        Event::ThinkingDone { text } => {
            if let Some(InFlightBlock::Thinking {
                text: body,
                finished,
            }) = snapshot.in_flight.blocks.last_mut()
            {
                *body = text.clone();
                *finished = true;
            }
        }
        Event::ToolCallStart { id, name } => {
            snapshot.in_flight.blocks.push(InFlightBlock::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args: String::new(),
                state: protocol::InFlightToolCallState::Pending,
            })
        }
        Event::ToolCallArgsDelta { id, json } => if let Some(InFlightBlock::ToolCall {
            args,
            state,
            ..
        }) = snapshot.in_flight.blocks.iter_mut().find(
            |block| matches!(block, InFlightBlock::ToolCall { id: current, .. } if current == id),
        ) {
            args.push_str(json);
            *state = protocol::InFlightToolCallState::StreamingArgs;
        },
        Event::ToolCallDone {
            id,
            name,
            arguments,
        } => if let Some(InFlightBlock::ToolCall {
            args,
            name: current_name,
            state,
            ..
        }) = snapshot.in_flight.blocks.iter_mut().find(
            |block| matches!(block, InFlightBlock::ToolCall { id: current, .. } if current == id),
        ) {
            *args = arguments.clone();
            *current_name = name.clone();
            *state = protocol::InFlightToolCallState::Done;
        },
        Event::RunEnd { .. } => snapshot.in_flight = Default::default(),
        Event::CompactionProgress { compaction } => {
            snapshot.in_flight.compaction = compaction.clone()
        }
        Event::Command { event } => apply_command(&mut snapshot.in_flight.commands, event),
        Event::InternalWorker { worker, event } => {
            if let Some(child) = snapshot
                .internal_workers
                .iter_mut()
                .find(|child| child.worker.session_id == worker.session_id)
            {
                apply(child, event, source);
            } else if let Event::Snapshot {
                session,
                greeting,
                state,
                in_flight,
                internal_workers,
            } = event.as_ref()
            {
                snapshot.internal_workers.push(InternalWorkerSnapshot {
                    worker: worker.clone(),
                    session: session.clone(),
                    greeting: Some(greeting.clone()),
                    status: state.catalog_status(),
                    error: None,
                    in_flight: in_flight.clone(),
                    internal_workers: internal_workers.clone(),
                });
            }
        }
        Event::InternalWorkerRemoved { worker } => snapshot
            .internal_workers
            .retain(|child| child.worker.session_id != worker.session_id),
        _ => {}
    }
}

fn committed_entries(
    source: &InternalWorkerSessionHandle,
    session_id: &str,
) -> Vec<protocol::SessionSnapshotEntry> {
    fn find(
        children: &[InternalWorkerSnapshot],
        id: &str,
    ) -> Option<Vec<protocol::SessionSnapshotEntry>> {
        for child in children {
            if child.worker.session_id == id {
                return Some(child.session.entries.clone());
            }
            if let Some(entries) = find(&child.internal_workers, id) {
                return Some(entries);
            }
        }
        None
    }
    let projection = source.protocol_snapshot();
    if source.session_id_string() == session_id {
        let mut entries = projection.session.entries;
        // A buffered pre-rotation identity may no longer be in the current
        // segment projection. Resolve it from the retained immutable log too.
        for entry in
            session_store::public_snapshot::project_current_session_snapshot(&source.entries())
                .entries
        {
            if !entries
                .iter()
                .any(|current| current.entry_id == entry.entry_id)
            {
                entries.push(entry);
            }
        }
        entries
    } else {
        find(&projection.internal_workers, session_id).unwrap_or_default()
    }
}

fn apply_command(commands: &mut Vec<protocol::CommandSnapshot>, event: &protocol::CommandEvent) {
    use protocol::{
        CommandEvent, CommandSnapshot, CommandStatus, CommandStream, CommandStreamSlice,
    };
    match event {
        CommandEvent::Started {
            command_id,
            tool_call_id,
            observed_at_ms,
        } => {
            commands.retain(|command| &command.command_id != command_id);
            commands.push(CommandSnapshot {
                command_id: command_id.clone(),
                tool_call_id: tool_call_id.clone(),
                status: CommandStatus::Running,
                started_at_ms: *observed_at_ms,
                observed_at_ms: *observed_at_ms,
                last_output_at_ms: None,
                stdout: CommandStreamSlice::default(),
                stderr: CommandStreamSlice::default(),
                exit_code: None,
            });
        }
        CommandEvent::Output {
            command_id,
            stream,
            start_offset,
            end_offset,
            content,
            observed_at_ms,
        } => {
            if let Some(command) = commands
                .iter_mut()
                .find(|command| &command.command_id == command_id)
            {
                command.observed_at_ms = *observed_at_ms;
                command.last_output_at_ms = Some(*observed_at_ms);
                let target = match stream {
                    CommandStream::Stdout => &mut command.stdout,
                    CommandStream::Stderr => &mut command.stderr,
                };
                if target.end_offset != *start_offset {
                    target.content.clear();
                    target.start_offset = *start_offset;
                    target.truncated = *start_offset > 0;
                }
                target.content.push_str(content);
                target.end_offset = *end_offset;
                if target.content.len() > 32 * 1024 {
                    let mut cut = target.content.len() - 32 * 1024;
                    while !target.content.is_char_boundary(cut) {
                        cut += 1;
                    }
                    target.content.drain(..cut);
                    target.start_offset = target
                        .end_offset
                        .saturating_sub(target.content.len() as u64);
                    target.truncated = true;
                }
            }
        }
        CommandEvent::Terminal { command_id, .. } => {
            commands.retain(|command| &command.command_id != command_id)
        }
    }
}
