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

    pub(crate) fn clear_for_committed_item(&self, item: &LoggedItem) {
        let mut inner = self.lock();
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
                    inner.remove_first_text_matching(&text);
                }
            }
            LoggedItem::Reasoning { text, .. } => {
                inner.remove_first_thinking_matching(text);
            }
            LoggedItem::ToolCall { call_id, .. } => {
                inner.remove_tool_call(call_id);
            }
            _ => {}
        }
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

    fn snapshot(&self) -> InFlightSnapshot {
        InFlightSnapshot {
            blocks: self
                .blocks
                .iter()
                .filter_map(TrackedBlock::to_snapshot_block)
                .collect(),
        }
    }

    fn remove_first_text_matching(&mut self, committed: &str) {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::Text { text, .. } => text == committed,
            _ => false,
        }) {
            self.blocks.remove(index);
        }
    }

    fn remove_first_thinking_matching(&mut self, committed: &str) {
        if let Some(index) = self.blocks.iter().position(|block| match block {
            TrackedBlock::Thinking { text, .. } => text == committed,
            _ => false,
        }) {
            self.blocks.remove(index);
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
            TrackedBlock::Thinking { text, finished, .. } => Some(InFlightBlock::Thinking {
                text: text.clone(),
                finished: *finished,
            }),
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
    fn committed_item_clears_matching_in_flight_block() {
        let (event_tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(event_tx);
        let block_id = in_flight.start_text_block();
        in_flight.text_delta(block_id, "done".into());
        in_flight.clear_for_committed_item(&LoggedItem::Message {
            role: session_store::LoggedRole::Assistant,
            content: vec![LoggedContentPart::Text {
                text: "done".into(),
            }],
        });

        let guard = in_flight.snapshot_guard();
        assert!(snapshot_from_guard(&guard).is_empty());
    }
}
