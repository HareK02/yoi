use std::sync::{Arc, Mutex, MutexGuard};

use protocol::{Event, InFlightBlock, InFlightSnapshot, InFlightToolCallState};
use session_store::{LoggedContentPart, LoggedItem};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlightBlockId(u64);

#[derive(Debug, Clone)]
pub struct InFlightEvents {
    inner: Arc<Mutex<InFlightInner>>,
    event_tx: broadcast::Sender<Event>,
}

#[derive(Debug)]
pub(crate) struct InFlightInner {
    next_block_id: u64,
    blocks: Vec<TrackedBlock>,
}

#[derive(Debug, Clone)]
enum TrackedBlock {
    Text {
        block_id: InFlightBlockId,
        text: String,
        finished: bool,
    },
    Thinking {
        block_id: InFlightBlockId,
        text: String,
        finished: bool,
    },
    ToolCall {
        block_id: InFlightBlockId,
        id: String,
        name: String,
        args: String,
        state: InFlightToolCallState,
    },
}

impl InFlightEvents {
    pub(crate) fn new(event_tx: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InFlightInner {
                next_block_id: 1,
                blocks: Vec::new(),
            })),
            event_tx,
        }
    }

    pub(crate) fn snapshot_guard(&self) -> MutexGuard<'_, InFlightInner> {
        self.inner.lock().expect("in-flight event mutex poisoned")
    }

    pub(crate) fn start_text_block(&self) -> InFlightBlockId {
        let mut inner = self.lock();
        let block_id = inner.next_id();
        inner.blocks.push(TrackedBlock::Text {
            block_id,
            text: String::new(),
            finished: false,
        });
        block_id
    }

    pub(crate) fn text_delta(&self, block_id: InFlightBlockId, text: String) {
        let mut inner = self.lock();
        if let Some(TrackedBlock::Text {
            text: current,
            finished,
            ..
        }) = inner.find_block_mut(block_id)
        {
            current.push_str(&text);
            *finished = false;
        }
        let _ = self.event_tx.send(Event::TextDelta { text });
    }

    pub(crate) fn text_done(&self, block_id: InFlightBlockId, text: String) {
        let mut inner = self.lock();
        if let Some(TrackedBlock::Text {
            text: current,
            finished,
            ..
        }) = inner.find_block_mut(block_id)
        {
            if current.is_empty() {
                *current = text.clone();
            }
            *finished = true;
        }
        let _ = self.event_tx.send(Event::TextDone { text });
    }

    pub(crate) fn thinking_start(&self) -> InFlightBlockId {
        let mut inner = self.lock();
        let block_id = inner.next_id();
        inner.blocks.push(TrackedBlock::Thinking {
            block_id,
            text: String::new(),
            finished: false,
        });
        let _ = self.event_tx.send(Event::ThinkingStart);
        block_id
    }

    pub(crate) fn thinking_delta(&self, block_id: InFlightBlockId, text: String) {
        let mut inner = self.lock();
        if let Some(TrackedBlock::Thinking {
            text: current,
            finished,
            ..
        }) = inner.find_block_mut(block_id)
        {
            current.push_str(&text);
            *finished = false;
        }
        let _ = self.event_tx.send(Event::ThinkingDelta { text });
    }

    pub(crate) fn thinking_done(&self, block_id: InFlightBlockId, text: String) {
        let mut inner = self.lock();
        if let Some(TrackedBlock::Thinking {
            text: current,
            finished,
            ..
        }) = inner.find_block_mut(block_id)
        {
            if current.is_empty() {
                *current = text.clone();
            }
            *finished = true;
        }
        let _ = self.event_tx.send(Event::ThinkingDone { text });
    }

    pub(crate) fn tool_call_start(&self, id: String, name: String) -> InFlightBlockId {
        let mut inner = self.lock();
        let block_id = inner.next_id();
        inner.blocks.push(TrackedBlock::ToolCall {
            block_id,
            id: id.clone(),
            name: name.clone(),
            args: String::new(),
            state: InFlightToolCallState::Pending,
        });
        let _ = self.event_tx.send(Event::ToolCallStart { id, name });
        block_id
    }

    pub(crate) fn tool_call_args_delta(
        &self,
        block_id: InFlightBlockId,
        id: String,
        delta: String,
    ) {
        let mut inner = self.lock();
        if let Some(TrackedBlock::ToolCall { args, state, .. }) = inner.find_block_mut(block_id) {
            args.push_str(&delta);
            *state = InFlightToolCallState::StreamingArgs;
        }
        let _ = self
            .event_tx
            .send(Event::ToolCallArgsDelta { id, json: delta });
    }

    pub(crate) fn tool_call_done(&self, block_id: InFlightBlockId, id: String, args: String) {
        let mut inner = self.lock();
        let mut name = String::new();
        if let Some(TrackedBlock::ToolCall {
            name: current_name,
            args: current,
            state,
            ..
        }) = inner.find_block_mut(block_id)
        {
            name = current_name.clone();
            if current.is_empty() {
                *current = args.clone();
            }
            *state = InFlightToolCallState::Done;
        }
        let _ = self.event_tx.send(Event::ToolCallDone {
            id,
            name,
            arguments: args,
        });
    }

    pub(crate) fn clear_for_committed_item_then<R>(
        &self,
        item: &LoggedItem,
        f: impl FnOnce() -> R,
    ) -> R {
        let mut inner = self.lock();
        inner.clear_for_committed_item(item);
        f()
    }

    fn lock(&self) -> MutexGuard<'_, InFlightInner> {
        self.inner.lock().expect("in-flight event mutex poisoned")
    }
}

impl InFlightInner {
    fn next_id(&mut self) -> InFlightBlockId {
        let id = InFlightBlockId(self.next_block_id);
        self.next_block_id = self.next_block_id.saturating_add(1);
        id
    }

    fn find_block_mut(&mut self, block_id: InFlightBlockId) -> Option<&mut TrackedBlock> {
        self.blocks
            .iter_mut()
            .find(|block| block.block_id() == block_id)
    }

    fn clear_for_committed_item(&mut self, item: &LoggedItem) {
        match item {
            LoggedItem::Message { role, content }
                if matches!(role, session_store::LoggedRole::Assistant) =>
            {
                let text = content
                    .iter()
                    .filter_map(|part| match part {
                        LoggedContentPart::Text { text } => Some(text.as_str()),
                        LoggedContentPart::Refusal { refusal } => Some(refusal.as_str()),
                    })
                    .collect::<String>();
                if !text.is_empty() {
                    self.remove_first_text_matching(&text);
                }
            }
            LoggedItem::Reasoning {
                text,
                summary,
                encrypted_content,
                ..
            } => {
                let mut removed = false;
                if !text.is_empty() {
                    removed |= self.remove_first_thinking_matching(text);
                }
                for summary_text in summary {
                    if !summary_text.is_empty() {
                        removed |= self.remove_first_thinking_matching(summary_text);
                    }
                }
                if !removed && encrypted_content.is_some() {
                    self.remove_first_empty_finished_thinking();
                }
            }
            LoggedItem::ToolCall { call_id, .. } => {
                self.remove_tool_call(call_id);
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> InFlightSnapshot {
        InFlightSnapshot {
            blocks: self
                .blocks
                .iter()
                .filter_map(TrackedBlock::to_snapshot_block)
                .collect(),
        }
    }

    fn remove_first_text_matching(&mut self, committed: &str) -> bool {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::Text { text, .. } => text == committed,
            _ => false,
        }) {
            self.blocks.remove(index);
            true
        } else {
            false
        }
    }

    fn remove_first_thinking_matching(&mut self, committed: &str) -> bool {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::Thinking { text, .. } => text == committed,
            _ => false,
        }) {
            self.blocks.remove(index);
            true
        } else {
            false
        }
    }

    fn remove_first_empty_finished_thinking(&mut self) -> bool {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::Thinking { text, finished, .. } => text.is_empty() && *finished,
            _ => false,
        }) {
            self.blocks.remove(index);
            true
        } else {
            false
        }
    }

    fn remove_tool_call(&mut self, call_id: &str) {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::ToolCall { id, .. } => id == call_id,
            _ => false,
        }) {
            self.blocks.remove(index);
        }
    }
}

impl TrackedBlock {
    fn block_id(&self) -> InFlightBlockId {
        match self {
            TrackedBlock::Text { block_id, .. }
            | TrackedBlock::Thinking { block_id, .. }
            | TrackedBlock::ToolCall { block_id, .. } => *block_id,
        }
    }

    fn to_snapshot_block(&self) -> Option<InFlightBlock> {
        match self {
            TrackedBlock::Text { text, finished, .. } => {
                if text.is_empty() {
                    None
                } else {
                    Some(InFlightBlock::Text {
                        text: text.clone(),
                        finished: *finished,
                    })
                }
            }
            TrackedBlock::Thinking { text, finished, .. } => {
                if text.is_empty() && *finished {
                    None
                } else {
                    Some(InFlightBlock::Thinking {
                        text: text.clone(),
                        finished: *finished,
                    })
                }
            }
            TrackedBlock::ToolCall {
                id,
                name,
                args,
                state,
                ..
            } => Some(InFlightBlock::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
                state: *state,
            }),
        }
    }
}

pub(crate) fn snapshot_from_guard(guard: &MutexGuard<'_, InFlightInner>) -> InFlightSnapshot {
    guard.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_boundary_does_not_duplicate_or_gap_delta_sent_after_subscribe() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx.clone());
        let block_id = in_flight.start_text_block();
        in_flight.text_delta(block_id, "hel".into());

        let guard = in_flight.snapshot_guard();
        let mut rx = event_tx.subscribe();
        let snapshot = snapshot_from_guard(&guard);
        drop(guard);

        in_flight.text_delta(block_id, "lo".into());

        assert_eq!(
            snapshot.blocks,
            vec![InFlightBlock::Text {
                text: "hel".into(),
                finished: false,
            }]
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            Event::TextDelta { text } if text == "lo"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn session_log_and_in_flight_snapshot_prevents_mirror_only_assistant_gap() {
        use std::sync::mpsc;
        use std::thread;

        use crate::segment_log_sink::SegmentLogSink;
        use session_store::{LogEntry, LoggedRole};

        let (event_tx, _) = broadcast::channel(16);
        let sink = SegmentLogSink::new();
        let in_flight = InFlightEvents::new(event_tx);
        let block_id = in_flight.start_text_block();
        in_flight.text_delta(block_id, "done".into());
        in_flight.text_done(block_id, "done".into());

        let assistant_item = LoggedItem::Message {
            role: LoggedRole::Assistant,
            content: vec![LoggedContentPart::Text {
                text: "done".into(),
            }],
        };
        let assistant_entry = LogEntry::AssistantItem {
            ts: 1,
            item: assistant_item.clone(),
        };

        let in_flight_guard = in_flight.snapshot_guard();
        let in_flight_for_commit = in_flight.clone();
        let sink_for_commit = sink.clone();
        let (committed_tx, committed_rx) = mpsc::channel();
        let commit_thread = thread::spawn(move || {
            // This mirrors Pod::append_entry ordering: clear in-flight first,
            // then publish the finalized AssistantItem. AssistantItem entries
            // are mirror-only and are not delivered as live entry events.
            in_flight_for_commit.clear_for_committed_item_then(&assistant_item, || {
                sink_for_commit.publish(assistant_entry);
            });
            committed_tx.send(()).unwrap();
        });

        let (entries_snapshot, mut entry_rx) = sink.subscribe_with_snapshot();
        let in_flight_snapshot = snapshot_from_guard(&in_flight_guard);
        drop(in_flight_guard);

        committed_rx.recv().unwrap();
        commit_thread.join().unwrap();

        assert!(entries_snapshot.is_empty());
        assert!(matches!(
            in_flight_snapshot.blocks.as_slice(),
            [InFlightBlock::Text { text, finished: true }] if text == "done"
        ));
        assert!(entry_rx.try_recv().is_err());
        let post_commit_guard = in_flight.snapshot_guard();
        assert!(snapshot_from_guard(&post_commit_guard).is_empty());
    }

    #[test]
    fn committed_assistant_snapshot_does_not_duplicate_in_flight_block() {
        use crate::segment_log_sink::SegmentLogSink;
        use session_store::{LogEntry, LoggedRole};

        let (event_tx, _) = broadcast::channel(16);
        let sink = SegmentLogSink::new();
        let in_flight = InFlightEvents::new(event_tx);
        let block_id = in_flight.start_text_block();
        in_flight.text_delta(block_id, "done".into());
        in_flight.text_done(block_id, "done".into());

        let assistant_item = LoggedItem::Message {
            role: LoggedRole::Assistant,
            content: vec![LoggedContentPart::Text {
                text: "done".into(),
            }],
        };
        let assistant_entry = LogEntry::AssistantItem {
            ts: 1,
            item: assistant_item.clone(),
        };

        in_flight.clear_for_committed_item_then(&assistant_item, || {
            sink.publish(assistant_entry);
        });

        let in_flight_guard = in_flight.snapshot_guard();
        let (entries_snapshot, _entry_rx) = sink.subscribe_with_snapshot();
        let in_flight_snapshot = snapshot_from_guard(&in_flight_guard);

        assert!(matches!(
            entries_snapshot.as_slice(),
            [LogEntry::AssistantItem { item, .. }] if item == &assistant_item
        ));
        assert!(in_flight_snapshot.is_empty());
    }

    #[test]
    fn committed_item_clears_matching_in_flight_block() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx);
        let block_id = in_flight.start_text_block();
        in_flight.text_delta(block_id, "done".into());
        in_flight.clear_for_committed_item_then(
            &LoggedItem::Message {
                role: session_store::LoggedRole::Assistant,
                content: vec![LoggedContentPart::Text {
                    text: "done".into(),
                }],
            },
            || (),
        );

        let guard = in_flight.snapshot_guard();
        assert!(snapshot_from_guard(&guard).is_empty());
    }

    #[test]
    fn committed_reasoning_summary_clears_matching_in_flight_thinking_blocks() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx);
        let first = in_flight.thinking_start();
        in_flight.thinking_delta(first, "summary A".into());
        in_flight.thinking_done(first, "".into());
        let second = in_flight.thinking_start();
        in_flight.thinking_delta(second, "summary B".into());
        in_flight.thinking_done(second, "".into());

        in_flight.clear_for_committed_item_then(
            &LoggedItem::Reasoning {
                text: String::new(),
                summary: vec!["summary A".into(), "summary B".into()],
                encrypted_content: Some("opaque".into()),
                signature: None,
            },
            || (),
        );

        let guard = in_flight.snapshot_guard();
        assert!(snapshot_from_guard(&guard).is_empty());
    }

    #[test]
    fn committed_encrypted_only_reasoning_clears_empty_finished_thinking_block() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx);
        let first = in_flight.thinking_start();
        in_flight.thinking_done(first, "".into());
        let second = in_flight.thinking_start();
        in_flight.thinking_delta(second, "still running".into());

        in_flight.clear_for_committed_item_then(
            &LoggedItem::Reasoning {
                text: String::new(),
                summary: Vec::new(),
                encrypted_content: Some("opaque".into()),
                signature: None,
            },
            || (),
        );

        let guard = in_flight.snapshot_guard();
        assert_eq!(
            snapshot_from_guard(&guard).blocks,
            vec![InFlightBlock::Thinking {
                text: "still running".into(),
                finished: false,
            }]
        );
    }

    #[test]
    fn snapshot_omits_empty_finished_thinking_blocks() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx);
        let empty_finished = in_flight.thinking_start();
        in_flight.thinking_done(empty_finished, "".into());
        let empty_running = in_flight.thinking_start();
        let visible_finished = in_flight.thinking_start();
        in_flight.thinking_delta(visible_finished, "visible".into());
        in_flight.thinking_done(visible_finished, "".into());

        let guard = in_flight.snapshot_guard();
        assert_eq!(
            snapshot_from_guard(&guard).blocks,
            vec![
                InFlightBlock::Thinking {
                    text: String::new(),
                    finished: false,
                },
                InFlightBlock::Thinking {
                    text: "visible".into(),
                    finished: true,
                }
            ]
        );
        assert_ne!(empty_running, empty_finished);
    }
}
