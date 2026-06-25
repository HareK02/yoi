//! History blocks: the unit of the TUI's stored display model.
//!
//! The TUI holds a flat `Vec<Block>` and re-renders it every frame.
//! Streaming events mutate the most recent matching block instead of
//! queuing a new line, so one logical thing (a tool call, an assistant
//! reply) stays visually coherent as updates arrive.

#![allow(dead_code)] // Phase 5 will consume `output` in detail mode.

use std::time::Instant;

use protocol::{AlertLevel, AlertSource, Greeting, Segment, WorkerEvent};

pub enum Block {
    Greeting(Greeting),
    TurnHeader {
        turn: usize,
    },
    UserMessage {
        segments: Vec<Segment>,
    },
    /// Persisted `role:system` history item rendered as an ordinary log
    /// element. File refs, auto-read snippets, workflow bodies, and future
    /// system-message injections all share this lane.
    SystemMessage {
        text: String,
    },
    /// Echo of `Method::Notify` received by this Worker, surfaced as a log
    /// element so subscribers see the external input that drove any
    /// following auto-kicked turn.
    Notify {
        message: String,
    },
    /// Echo of `Method::WorkerEvent` received by this Worker. Same role as
    /// `Notify` — an input log element, not a turn-control signal.
    WorkerEvent {
        event: WorkerEvent,
    },
    AssistantText {
        text: String,
    },
    Thinking(ThinkingBlock),
    ToolCall(ToolCallBlock),
    Alert {
        level: AlertLevel,
        source: AlertSource,
        message: String,
    },
    Compact(CompactEvent),
    TurnStats {
        requests: usize,
        /// Net tokens uploaded across the turn's LLM requests
        /// (cache reads excluded; cache writes included). Same value
        /// the status line accumulates while the turn is in flight.
        upload_tokens: u64,
        output_tokens: u64,
    },
}

pub struct ThinkingBlock {
    /// Accumulated reasoning body. Empty for providers that emit only
    /// metadata (no plaintext deltas).
    pub text: String,
    pub state: ThinkingState,
}

pub enum ThinkingState {
    /// Live block: actively streaming. `started_at` powers the
    /// `Thinking... (Xs)` live timer. History-restored blocks never
    /// enter this state — they materialise as `Finished { elapsed_secs:
    /// None }` since the original duration is not persisted.
    Streaming { started_at: Instant },
    /// Block ended cleanly with `ThinkingDone`.
    Finished { elapsed_secs: Option<u64> },
    /// `TurnEnd` arrived before `ThinkingDone`. Elapsed time is frozen
    /// at the last observed instant.
    Incomplete { elapsed_secs: Option<u64> },
}

pub enum CompactEvent {
    /// Live block: compaction worker is running. `started_at` powers the
    /// `Compacting... (Xs)` live timer.
    Streaming { started_at: Instant },
    /// Compaction ended cleanly with `CompactDone`.
    Done {
        new_segment_id: uuid::Uuid,
        elapsed_secs: Option<u64>,
    },
    /// Compaction ended with `CompactFailed`.
    Failed {
        error: String,
        elapsed_secs: Option<u64>,
    },
    /// The TUI stopped observing events before a terminal compact event.
    Incomplete { elapsed_secs: Option<u64> },
}

pub struct ToolCallBlock {
    pub id: String,
    pub name: String,
    /// Raw JSON fragments accumulated from `ToolCallArgsDelta`.
    pub args_stream: String,
    /// Final arguments text once `ToolCallDone` lands.
    pub arguments: Option<String>,
    pub state: ToolCallState,
    /// For Edit tool calls: snapshot of the file content *before* the
    /// edit was applied to the cache. Captured at result time so the
    /// diff renderer can reproduce the old-content context even after
    /// subsequent mutations have rolled the cache forward.
    pub edit_snapshot: Option<String>,
}

pub enum ToolCallState {
    /// `ToolCallStart` received, nothing else yet.
    Pending,
    /// At least one `ToolCallArgsDelta` has arrived.
    Streaming,
    /// `ToolCallDone` received, waiting on the tool result.
    Executing,
    /// `ToolResult { is_error: false, .. }` received.
    Done {
        summary: String,
        output: Option<String>,
    },
    /// `ToolResult { is_error: true, .. }` received.
    Error {
        summary: String,
        output: Option<String>,
    },
    /// Turn ended before a matching `ToolResult` arrived.
    Incomplete,
}
