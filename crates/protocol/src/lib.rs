#[cfg(feature = "stream")]
pub mod stream;
#[cfg(feature = "typescript")]
pub mod typescript;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

// ---------------------------------------------------------------------------
// Method (Client → Pod via Unix Socket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    Run {
        input: Vec<Segment>,
    },
    /// Human-readable text injected into the target Pod's LLM context
    /// as a non-blocking system message. `auto_run` controls whether an
    /// idle target is kicked into `RunForNotification`; weak notifications
    /// (`auto_run: false`) are only queued for the next turn/resume/run.
    /// No side effects beyond LLM context; use `PodEvent` for typed
    /// lifecycle reports.
    Notify {
        message: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        auto_run: bool,
    },
    /// Typed lifecycle report from a child Pod to its direct parent.
    PodEvent(PodEvent),
    Resume,
    Cancel,
    /// Stop the in-flight turn and transition to `Paused`.
    ///
    /// Unlike `Cancel` (which discards and returns to `Idle`), a paused
    /// Pod can resume the interrupted work via `Resume`, or start a
    /// fresh turn via `Run` (orphan `tool_use` items are closed with a
    /// synthetic tool result before the new user message is appended).
    Pause,
    /// Request an explicit compaction while the Pod is otherwise idle.
    ///
    /// This is a typed control method: clients must not send `compact` as a
    /// `Method::Run` user message.
    Compact,
    /// Ask the Pod to list valid rewind targets from its authoritative session log.
    ListRewindTargets,
    /// Truncate the current session back to the selected rewind target and
    /// return the selected user input to the client composer.
    RewindTo {
        target: RewindTargetId,
        expected_head_entries: usize,
    },
    Shutdown,
    /// Request a list of completion candidates from the Pod.
    ///
    /// Reply is sent on the same socket as `Event::Completions` (not
    /// broadcast). The IPC server handles this directly and writes
    /// the response straight back to the requesting socket. Empty
    /// results for resolvers that are not yet wired up
    /// (Knowledge / Workflow).
    ListCompletions {
        kind: CompletionKind,
        prefix: String,
    },
    /// List Pods visible to this Pod from durable Pod state and the spawned-child
    /// registry. This is not a host-wide Pod universe query.
    ListPods,
    /// Restore a visible stopped/restorable Pod, or report that it is already
    /// live. Missing state and not-visible state are distinct errors.
    RestorePod {
        name: String,
    },
    /// Register another existing Pod as a reciprocal peer of this Pod.
    ///
    /// This is metadata/control state only: it does not ask the target's live
    /// controller for consent, and it must not grant delegated scope,
    /// spawned-child ownership, output cursors, or child lifecycle authority.
    RegisterPeer {
        name: String,
    },
}

/// Typed lifecycle events sent from a child Pod to its parent.
///
/// Delivered as `Method::PodEvent` over the parent's Unix socket. The
/// parent Controller always applies variant-specific side effects
/// (registry / pod-registry updates). Agent-visible variants are also
/// queued into the notification buffer; control-plane-only variants are
/// not injected into the parent's LLM context.
///
/// Transport is fire-and-forget; receivers must tolerate out-of-order
/// delivery (e.g. `TurnEnded` arriving after `ShutDown` for the same
/// child Pod).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PodEvent {
    /// Child finished one turn and is back to IDLE.
    TurnEnded { pod_name: String },

    /// Engine execution error occurred inside the child's turn.
    ///
    /// Limited to worker runtime failures (provider / tool errors) —
    /// does not include transient method-rejection responses such as
    /// `AlreadyRunning`.
    Errored { pod_name: String, message: String },

    /// Child has stopped (controller loop is exiting).
    ShutDown { pod_name: String },

    /// Child sub-delegated scope to a grandchild Pod via `SpawnPod`.
    ///
    /// Control-plane only: receivers apply registry side effects and
    /// propagate upward, but do not expose this as an agent notification.
    ///
    /// The parent uses this to add the grandchild to its own
    /// `spawned_pods.json` so it can manage the grandchild directly
    /// even if the intermediate child dies. The parent then re-fires
    /// this event upward (if it has a parent of its own) to maintain
    /// the chain to root.
    ScopeSubDelegated {
        /// Sub-delegating Pod (= the sender itself).
        parent_pod: String,
        /// Name of the grandchild Pod.
        sub_pod: String,
        /// Unix-socket path where the grandchild is reachable.
        sub_socket: PathBuf,
        /// Scope delegated to the grandchild.
        scope: Vec<ScopeRule>,
    },
}

impl PodEvent {
    /// Whether this event should become an agent-visible notification/history item.
    ///
    /// Control-plane-only events still travel over the same wire enum and still
    /// run receiver side effects, but they must not wake the parent LLM or enter
    /// the notification buffer.
    pub fn should_notify_agent(&self) -> bool {
        match self {
            PodEvent::TurnEnded { .. } | PodEvent::Errored { .. } | PodEvent::ShutDown { .. } => {
                true
            }
            PodEvent::ScopeSubDelegated { .. } => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Segment — typed pieces of a user submission
// ---------------------------------------------------------------------------

/// One typed piece of a user submission.
///
/// `Method::Run` and `Event::UserMessage` carry `Vec<Segment>`. Dumb
/// clients (CLI piping, scripts) only need to produce a single
/// `Segment::Text`; richer clients (TUI / GUI) construct typed atoms
/// (paste chips, file refs, knowledge refs, workflow invocations) and
/// send them through directly so the Pod side never has to re-parse a
/// flattened string.
///
/// Forward compat: payloads with unknown `kind` deserialize to
/// `Segment::Unknown`. Pod treats this the same as known-but-unresolved
/// variants — emits an alert and inserts a `[unknown input segment]`
/// placeholder into the LLM context so neither user nor LLM is blind to
/// the dropped intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Segment {
    /// Free-form text. The fallback every client can produce.
    Text { content: String },
    /// Bracketed-paste capture from a TUI-style client. `id`, `chars`
    /// and `lines` carry the metadata needed to re-render a
    /// `[Clipboard #N | X chars, Y lines]` chip in `Event::UserMessage`
    /// re-broadcast.
    Paste {
        id: u32,
        chars: u32,
        lines: u32,
        content: String,
    },
    /// `@<path>` file-system reference. Pod resolves readable files to
    /// `[File: <path>]` attachments and readable normal directories to shallow
    /// `[Dir: <path>]` listings; the flattened user text keeps the literal
    /// `@<path>` placeholder either way.
    FileRef { path: String },
    /// `#<slug>` Knowledge reference (see `docs/plan/memory.md`).
    KnowledgeRef { slug: String },
    /// `/<slug>` Workflow invocation (see `docs/plan/workflow.md`).
    WorkflowInvoke { slug: String },
    /// Unknown variant from a newer client. Pod treats this as an
    /// unresolved input — surfaces an alert and inserts a placeholder.
    /// Round-trip is lossy: re-serializing yields `{"kind":"unknown"}`.
    #[serde(other)]
    Unknown,
}

impl Segment {
    /// Convenience constructor for the most common case.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { content: s.into() }
    }

    /// Flatten a segment slice into the single string the LLM receives
    /// as a user message. Pure — no I/O, no alerts. Callers that need
    /// to surface user-visible alerts for unresolved refs should do so
    /// alongside this call (Pod does so at submit time).
    ///
    /// Sigil-prefixed variants (`FileRef` / `KnowledgeRef` / `WorkflowInvoke`)
    /// flatten back to their literal sigil form (`@<path>`, `#<slug>`,
    /// `/<slug>`) — matching what the user originally typed. Resolved
    /// content (e.g. file body or shallow directory listing for `FileRef`) is
    /// delivered as separate `Item::system_message`s adjacent to the user
    /// message; the resolution itself is the caller's job. `Unknown` falls back to
    /// a bracketed placeholder since there is no sigil to render.
    pub fn flatten_to_text(segments: &[Segment]) -> String {
        let mut out = String::new();
        for seg in segments {
            match seg {
                Segment::Text { content } => out.push_str(content),
                Segment::Paste { content, .. } => out.push_str(content),
                Segment::FileRef { path } => {
                    out.push('@');
                    out.push_str(path);
                }
                Segment::KnowledgeRef { slug } => {
                    out.push('#');
                    out.push_str(slug);
                }
                Segment::WorkflowInvoke { slug } => {
                    out.push('/');
                    out.push_str(slug);
                }
                Segment::Unknown => {
                    out.push_str("[unknown input segment]");
                }
            }
        }
        out
    }
}

impl Method {
    /// Convenience: a `Run` carrying a single `Segment::Text`.
    /// Used by dumb clients, inter-Pod tools, and tests that only have
    /// a string to forward.
    pub fn run_text(s: impl Into<String>) -> Self {
        Self::Run {
            input: vec![Segment::text(s)],
        }
    }
}

// ---------------------------------------------------------------------------
// Event (Pod → Client via Unix Socket broadcast)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// A user input message was accepted, persisted as
    /// `LogEntry::UserInput`, and is about to start a new turn.
    /// Broadcast to every subscribed client so TUI / GUI instances show
    /// the same user line that reconnect snapshots would replay from
    /// history; clients must not synthesize a separate pending/fake
    /// message for accepted runs.
    ///
    /// Fires exactly once per committed user input, after
    /// `InvokeStart { kind: UserSend }` and before the first
    /// `TurnStart`. Rejected runs (e.g. `AlreadyRunning`) do not emit.
    UserMessage {
        segments: Vec<Segment>,
    },
    /// One agent-injected system item committed to history.
    ///
    /// Carries the JSON form of `session_store::SystemItem`. Covers
    /// `Method::Notify` echoes, child-Pod lifecycle events from
    /// `Method::PodEvent`, `@<path>` / `#<slug>` / `/<slug>`
    /// resolution payloads, and any future agent-side injection kind.
    /// Clients dispatch on the `kind` tag for typed rendering instead
    /// of parsing free-text prefixes like `[Notification] …` or
    /// `[File: …]`.
    ///
    /// One event per `LogEntry::SystemItem` commit. Disk-side and
    /// wire-side are 1:1.
    SystemItem {
        #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
        item: serde_json::Value,
    },
    /// A new self-driving cycle has begun (IDLE → active transition).
    ///
    /// Marker event for the start of an Invoke range; the range extends
    /// implicitly until the next `InvokeStart`. Fires for every accepted
    /// `Method::Run` (kind=`UserSend`), `Method::Notify` (kind=`Notify`),
    /// `Method::PodEvent` re-injection (kind=`PodEvent`), and any other
    /// IDLE-breaking trigger. Mid-run interrupts (e.g. hook output,
    /// `<system-reminder>` injection that doesn't break IDLE) do not
    /// emit `InvokeStart` — they appear as `SystemItem` only.
    ///
    /// Carries `kind` only; the payload (user text / notify message /
    /// pod event body) is delivered separately via the immediately
    /// following `UserMessage` / `SystemItem` event.
    InvokeStart {
        kind: InvokeKind,
    },
    /// One AgentTurn boundary opened. An AgentTurn is a maximal run of
    /// LLM generation calls whose input messages are identical (i.e.
    /// retries from network errors / 5xx / stream disconnects collapse
    /// into the same AgentTurn). When the input changes (a new tool
    /// result lands, a user interrupts, etc.), the next LLM call belongs
    /// to a new AgentTurn.
    ///
    /// `turn` is the AgentTurn index from `Engine::turn_count`.
    ///
    /// Currently retry is not yet implemented (`llm-engine-stream-continuation`)
    /// so AgentTurn and `LlmCall` fire 1:1, but the contract here is
    /// the AgentTurn boundary; consumers that want per-LLM-call signals
    /// must subscribe to `LlmCallStart` / `LlmCallEnd` instead.
    TurnStart {
        turn: usize,
    },
    /// AgentTurn closed.
    TurnEnd {
        turn: usize,
        result: TurnResult,
    },
    /// One LLM generation call started (1 request → 1 generation, retry
    /// included). Multiple `LlmCall*` pairs may fire inside a single
    /// `TurnStart` / `TurnEnd` pair when a request is retried; today
    /// they fire 1:1 because retry is not implemented.
    ///
    /// `llm_call` is the worker-wide running counter from
    /// `Engine::llm_call_count`.
    LlmCallStart {
        llm_call: usize,
    },
    /// LLM generation call ended.
    LlmCallEnd {
        llm_call: usize,
    },
    /// A transport-level LLM request retry has been scheduled.
    ///
    /// This is operational state for clients to render while the worker is
    /// waiting in backoff. It is not part of conversation history.
    LlmRetry {
        llm_call: usize,
        /// The attempt that just failed. 1 origin.
        failed_attempt: u32,
        max_attempts: u32,
        wait_ms: u64,
        elapsed_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        error: String,
    },
    /// Stream generation was interrupted after events had begun and the worker
    /// is continuing with a follow-up LLM request.
    LlmContinuation {
        llm_call: usize,
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
    TextDelta {
        text: String,
    },
    TextDone {
        text: String,
    },
    /// A reasoning / thinking block has started.
    ///
    /// Always paired with a `ThinkingDone`. `ThinkingDelta` is optional:
    /// some providers (or some configurations) emit thinking metadata
    /// without plaintext, in which case Start → Done arrive with no
    /// deltas in between. Multiple thinking blocks per turn are allowed.
    ThinkingStart,
    ThinkingDelta {
        text: String,
    },
    /// Thinking block completed. `text` is the full accumulated body
    /// (empty string when the provider didn't emit plaintext).
    ThinkingDone {
        text: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        id: String,
        json: String,
    },
    ToolCallDone {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        id: String,
        /// Short human-readable summary. Always present; used by clients
        /// that only want a 1-line rendering (e.g. collapsed views).
        summary: String,
        /// Full tool output. Absent when the tool chose to return
        /// summary-only, or when the result was pruned.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default)]
        is_error: bool,
    },
    /// Token accounting for one LLM request.
    ///
    /// `input_tokens` is the prompt prefix occupancy (cache reads /
    /// cache writes included), as normalised by the worker layer.
    /// `cache_read_input_tokens` is the cache-hit subset of that
    /// occupancy; subtracting it yields the "net upload" the client
    /// actually paid full price to send on this request, which is what
    /// the TUI status line accumulates per turn.
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_input_tokens: Option<u64>,
    },
    RunEnd {
        result: RunResult,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// Sent exactly once at the start of every client connection.
    ///
    /// `entries` is the session-log mirror at subscribe time, serialised
    /// as the JSON form of `session_store::LogEntry`. This is the
    /// bulk-reconstruction lane: clients walk the entries to seed their
    /// derived view.
    ///
    /// `greeting` and `status` accompany the snapshot so clients render
    /// pod identity and current controller state without an extra round
    /// trip.
    ///
    /// Live updates after the snapshot arrive through the streaming
    /// events (`TextDelta` / `ToolCall*` / `ToolResult` / etc.) plus
    /// role-specific entry events (`SegmentRotated` / `SystemItem`) —
    /// there is no generic "every committed entry" broadcast.
    Snapshot {
        #[cfg_attr(feature = "typescript", ts(type = "Array<unknown>"))]
        entries: Vec<serde_json::Value>,
        greeting: Greeting,
        #[serde(default)]
        status: PodStatus,
        /// Unfinished model output that has already streamed in the current
        /// run but is not yet represented by committed snapshot entries.
        #[serde(default, skip_serializing_if = "InFlightSnapshot::is_empty")]
        in_flight: InFlightSnapshot,
    },
    /// Server-side segment log rotated to a fresh `SegmentStart`.
    ///
    /// Fires on compaction and on auto-fork when the store head drifts
    /// from the live writer's cached head. Clients drop their derived
    /// view and reseed from `entry.history` exactly the way they would
    /// from a connect-time `Snapshot`.
    ///
    /// Payload is the JSON form of `session_store::LogEntry::SegmentStart`.
    SegmentRotated {
        #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
        entry: serde_json::Value,
    },
    /// Current Pod controller status. Broadcast on every controller-level
    /// transition and included in `History` snapshots for late attach.
    Status {
        status: PodStatus,
    },
    /// Reply to `Method::ListCompletions`. Delivered only to the
    /// requesting socket (not broadcast). `entries` is empty when no
    /// candidates match or when the requested kind has no resolver
    /// wired up.
    Completions {
        kind: CompletionKind,
        entries: Vec<CompletionEntry>,
    },
    /// Reply to `Method::ListRewindTargets`. Clients should only open a picker
    /// in response to their own pending request; the event may be broadcast.
    RewindTargets {
        head_entries: usize,
        targets: Vec<RewindTarget>,
    },
    /// A rewind has truncated the authoritative session. `entries` is the
    /// retained session-log prefix clients should use to reseed display state.
    RewindApplied {
        #[cfg_attr(feature = "typescript", ts(type = "Array<unknown>"))]
        entries: Vec<serde_json::Value>,
        input: Vec<Segment>,
        summary: RewindSummary,
    },
    /// Reply to `Method::ListPods`. Payload is a stable JSON value so the Pod
    /// crate can evolve discovery fields without introducing a protocol
    /// dependency on session-store.
    PodsListed {
        #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
        pods: serde_json::Value,
    },
    /// Reply to `Method::RestorePod`.
    PodRestored {
        #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
        result: serde_json::Value,
    },
    /// Reply to `Method::RegisterPeer`.
    PeerRegistered {
        #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
        result: serde_json::Value,
    },
    Alert(Alert),
    /// Latest memory extract/consolidation lifecycle event for UI observability.
    ///
    /// This is not part of LLM history or prompt context; clients may display it
    /// briefly as operational status.
    MemoryWorker(MemoryWorkerEvent),
    /// Pod has started compacting the current session.
    ///
    /// Fired immediately before a compaction run. Success is signalled by
    /// `CompactDone` (with the new `SegmentId`); failure by `CompactFailed`.
    /// Broadcast to all clients; not replayed to late subscribers.
    CompactStart,
    /// Compaction completed and the session was rotated.
    ///
    /// `new_segment_id` is the UUID of the freshly created session that
    /// replaced the old history.
    CompactDone {
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        new_segment_id: uuid::Uuid,
    },
    /// Compaction failed. The session is unchanged.
    CompactFailed {
        error: String,
    },
    Shutdown,
}

/// User-facing alert emitted from the Pod layer.
///
/// This is a separate channel from `tracing` (developer logs): entries
/// here are assembled explicitly by the Pod when a condition should be
/// surfaced to the person driving the client. Keep messages short and
/// human-readable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Alert {
    pub level: AlertLevel,
    pub source: AlertSource,
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct MemoryWorkerEvent {
    pub worker: String,
    pub status: String,
    pub run_id: String,
    pub trigger: String,
    pub reason: String,
    /// Human-readable compact form for actionbar rendering.
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AlertSource {
    Pod,
    Engine,
    Compactor,
    AgentsMd,
}

/// Kind of completion requested by `Method::ListCompletions`.
///
/// Mirrors the TUI prefix sigils: `@` → `File`, `#` → `Knowledge`,
/// `/` → `Workflow`. Knowledge and Workflow resolvers are currently
/// stubs (always reply with empty `entries`); the wire shape is
/// nailed down here so the TUI side can ship without waiting for
/// the memory / workflow tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    File,
    Knowledge,
    Workflow,
}

/// One candidate returned in `Event::Completions::entries`.
///
/// `value` is a path (file kind) or a slug (knowledge / workflow).
/// `is_dir` is meaningful only for the file kind — it lets the TUI
/// keep a trailing `/` after a directory selection so the user can
/// drill in without re-typing the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct CompletionEntry {
    pub value: String,
    #[serde(default)]
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RewindTargetId {
    #[cfg_attr(feature = "typescript", ts(type = "string"))]
    pub segment_id: uuid::Uuid,
    pub user_input_entry_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RewindTarget {
    pub id: RewindTargetId,
    pub expected_head_entries: usize,
    pub truncate_entries: usize,
    pub turn_index: usize,
    pub timestamp_ms: Option<u64>,
    pub preview: String,
    pub eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RewindSummary {
    pub truncated_to_entries: usize,
    pub discarded_entries: usize,
    pub tool_side_effect_warning: bool,
}

/// Unfinished model output included in `Event::Snapshot` for clients that
/// attach while an LLM response is still streaming.
///
/// These blocks are presentation state only: they are reconstructed from the
/// active Pod controller and must not be treated as committed assistant
/// history. Finalized assistant items continue to come from ordinary snapshot
/// entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct InFlightSnapshot {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<InFlightBlock>,
}

impl InFlightSnapshot {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InFlightBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        finished: bool,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        finished: bool,
    },
    ToolCall {
        id: String,
        name: String,
        args: String,
        #[serde(default, skip_serializing_if = "InFlightToolCallState::is_pending")]
        state: InFlightToolCallState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum InFlightToolCallState {
    #[default]
    Pending,
    StreamingArgs,
    Done,
}

impl InFlightToolCallState {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// Pod self-description rendered by the TUI when a session starts empty.
///
/// Built once in the Pod controller from the resolved manifest and
/// transmitted alongside `Event::Snapshot` so clients don't need
/// their own view of the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Greeting {
    pub pod_name: String,
    pub cwd: String,
    pub provider: String,
    pub model: String,
    pub scope_summary: String,
    pub tools: Vec<String>,
    /// Model context window in tokens. Always filled by the Pod greeting.
    #[serde(default)]
    pub context_window: u64,
    /// Estimated current session context tokens at connect time.
    #[serde(default)]
    pub context_tokens: u64,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PodStatus {
    #[default]
    Idle,
    Running,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TurnResult {
    Finished,
    Paused,
}

/// Kind of trigger that opened a new Invoke (IDLE → active) range.
///
/// One Invoke groups all entries from this trigger up to the next
/// `Invoke` marker. The kind is the only payload — content (user text,
/// notify message, pod event body) is delivered by the immediately
/// following Turn entry, not by the marker itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum InvokeKind {
    /// `Method::Run` — a user submission.
    UserSend,
    /// `Method::Notify` — free-text notification injected into history.
    Notify,
    /// `Method::PodEvent` — typed lifecycle report from a child Pod.
    PodEvent,
    /// `<system-reminder>` etc. that crosses an IDLE boundary (mid-run
    /// reminders that don't break IDLE are SystemItem-only and do not
    /// open a new Invoke).
    SystemReminder,
    /// Cron / RemoteTrigger style scheduled wake-up. Reserved; no
    /// producer is wired today but the variant is part of the initial
    /// set so future schedulers don't have to amend the wire enum.
    Wakeup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Finished,
    Paused,
    LimitReached,
    /// The accepted Method::Run produced no assistant/tool output before
    /// user interruption, so the Pod rolled the submit-time turn state back
    /// to its pre-submit snapshot. Clients should treat the Pod as Idle and
    /// restore the just-submitted input into the editable composer if desired.
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AlreadyRunning,
    NotRunning,
    NotPaused,
    ProviderError,
    ToolError,
    InvalidRequest,
    Internal,
}

// ---------------------------------------------------------------------------
// Scope rule / permission (wire type)
//
// Defined here so that both `manifest` (config parsing) and `protocol`
// itself (inter-pod messaging such as `PodEvent::ScopeSubDelegated`) can
// reference the same type without introducing a reverse dependency.
// ---------------------------------------------------------------------------

/// A single allow or deny rule inside a scope configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ScopeRule {
    /// Target path. Must be absolute by the time a `Scope` is built from
    /// this rule — relative paths are resolved per-layer against the
    /// manifest file's directory (cwd for overlay layers) before cascade
    /// merge.
    pub target: PathBuf,
    /// Permission level this rule grants (allow) or caps strictly below
    /// (deny).
    pub permission: Permission,
    /// When `false`, the rule only matches the target itself and its
    /// direct children. Defaults to `true`.
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_recursive() -> bool {
    true
}

/// Permission lattice used by [`ScopeRule`].
///
/// The derived `Ord` instance follows declaration order, so
/// `Read < Write`. Allow rules grant the stated level (and by extension
/// everything below); deny rules cap the effective level **strictly
/// below** the stated level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Write,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_run_json_roundtrip() {
        let json = r#"{"method":"run","params":{"input":[{"kind":"text","content":"Hello"}]}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        match &method {
            Method::Run { input } => {
                assert_eq!(input.len(), 1);
                match &input[0] {
                    Segment::Text { content } => assert_eq!(content, "Hello"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected Run, got {other:?}"),
        }
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn method_run_paste_segment_roundtrip() {
        let method = Method::Run {
            input: vec![
                Segment::text("see "),
                Segment::Paste {
                    id: 7,
                    chars: 12,
                    lines: 2,
                    content: "line1\nline2".into(),
                },
            ],
        };
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::Run { input } => {
                assert_eq!(input.len(), 2);
                match &input[1] {
                    Segment::Paste {
                        id,
                        chars,
                        lines,
                        content,
                    } => {
                        assert_eq!(*id, 7);
                        assert_eq!(*chars, 12);
                        assert_eq!(*lines, 2);
                        assert_eq!(content, "line1\nline2");
                    }
                    other => panic!("expected Paste, got {other:?}"),
                }
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn segment_unknown_variant_decodes_as_unknown() {
        // A future client sends a segment kind this Pod has never heard of.
        // Forward compat requirement: deserialization must succeed and the
        // unknown payload must surface as `Segment::Unknown` so the Pod
        // fallback path (placeholder + alert) can fire.
        let json = r#"{"kind":"image_ref","url":"https://example.com/x.png"}"#;
        let seg: Segment = serde_json::from_str(json).unwrap();
        assert!(matches!(seg, Segment::Unknown));
    }

    #[test]
    fn method_run_with_unknown_segment_decodes() {
        let json = r#"{"method":"run","params":{"input":[{"kind":"text","content":"hi"},{"kind":"future_thing","x":1}]}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        match method {
            Method::Run { input } => {
                assert_eq!(input.len(), 2);
                assert!(matches!(input[0], Segment::Text { .. }));
                assert!(matches!(input[1], Segment::Unknown));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn method_without_params() {
        let json = r#"{"method":"resume"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Resume));
    }

    #[test]
    fn method_pause_roundtrip() {
        let json = r#"{"method":"pause"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Pause));
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn method_compact_roundtrip() {
        let json = r#"{"method":"compact"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Compact));
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn event_text_delta_format() {
        let event = Event::TextDelta {
            text: "Hello".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "text_delta");
        assert_eq!(parsed["data"]["text"], "Hello");
    }

    #[test]
    fn event_thinking_roundtrip() {
        for event in [
            Event::ThinkingStart,
            Event::ThinkingDelta {
                text: "step 1".into(),
            },
            Event::ThinkingDone {
                text: "step 1\nstep 2".into(),
            },
        ] {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: Event = serde_json::from_str(&json).unwrap();
            match (&event, &decoded) {
                (Event::ThinkingStart, Event::ThinkingStart) => {}
                (Event::ThinkingDelta { text: a }, Event::ThinkingDelta { text: b })
                | (Event::ThinkingDone { text: a }, Event::ThinkingDone { text: b }) => {
                    assert_eq!(a, b);
                }
                _ => panic!("variant mismatch: {event:?} vs {decoded:?}"),
            }
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Event::ThinkingStart).unwrap()).unwrap();
        assert_eq!(parsed["event"], "thinking_start");
    }

    #[test]
    fn event_run_end_format() {
        let event = Event::RunEnd {
            result: RunResult::LimitReached,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "run_end");
        assert_eq!(parsed["data"]["result"], "limit_reached");
    }

    #[test]
    fn event_invoke_start_roundtrip() {
        for kind in [
            InvokeKind::UserSend,
            InvokeKind::Notify,
            InvokeKind::PodEvent,
            InvokeKind::SystemReminder,
            InvokeKind::Wakeup,
        ] {
            let event = Event::InvokeStart { kind };
            let json = serde_json::to_string(&event).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed["event"], "invoke_start");
            let decoded: Event = serde_json::from_str(&json).unwrap();
            match decoded {
                Event::InvokeStart { kind: k } => assert_eq!(k, kind),
                other => panic!("expected InvokeStart, got {other:?}"),
            }
        }
        let parsed: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&Event::InvokeStart {
                kind: InvokeKind::UserSend,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(parsed["data"]["kind"], "user_send");
    }

    #[test]
    fn event_llm_call_start_end_roundtrip() {
        for event in [
            Event::LlmCallStart { llm_call: 0 },
            Event::LlmCallEnd { llm_call: 7 },
        ] {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: Event = serde_json::from_str(&json).unwrap();
            match (&event, &decoded) {
                (Event::LlmCallStart { llm_call: a }, Event::LlmCallStart { llm_call: b })
                | (Event::LlmCallEnd { llm_call: a }, Event::LlmCallEnd { llm_call: b }) => {
                    assert_eq!(a, b);
                }
                _ => panic!("variant mismatch: {event:?} vs {decoded:?}"),
            }
        }
        let parsed: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&Event::LlmCallStart { llm_call: 3 }).unwrap(),
        )
        .unwrap();
        assert_eq!(parsed["event"], "llm_call_start");
        assert_eq!(parsed["data"]["llm_call"], 3);
    }

    #[test]
    fn event_llm_retry_roundtrip() {
        let event = Event::LlmRetry {
            llm_call: 3,
            failed_attempt: 1,
            max_attempts: 4,
            wait_ms: 800,
            elapsed_ms: 120,
            status: Some(504),
            error: "API error (status: 504): gateway timeout".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "llm_retry");
        assert_eq!(parsed["data"]["status"], 504);
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::LlmRetry {
                llm_call,
                failed_attempt,
                max_attempts,
                wait_ms,
                status,
                ..
            } => {
                assert_eq!(llm_call, 3);
                assert_eq!(failed_attempt, 1);
                assert_eq!(max_attempts, 4);
                assert_eq!(wait_ms, 800);
                assert_eq!(status, Some(504));
            }
            other => panic!("expected LlmRetry, got {other:?}"),
        }
    }

    #[test]
    fn event_llm_continuation_roundtrip() {
        let event = Event::LlmContinuation {
            llm_call: 4,
            attempt: 1,
            max_attempts: 3,
            reason: "SSE parse error: closed".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "llm_continuation");
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::LlmContinuation {
                llm_call,
                attempt,
                max_attempts,
                reason,
            } => {
                assert_eq!(llm_call, 4);
                assert_eq!(attempt, 1);
                assert_eq!(max_attempts, 3);
                assert_eq!(reason, "SSE parse error: closed");
            }
            other => panic!("expected LlmContinuation, got {other:?}"),
        }
    }

    #[test]
    fn method_notify_json_roundtrip_defaults_to_auto_run() {
        let json = r#"{"method":"notify","params":{"message":"turn done"}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(
            method,
            Method::Notify { ref message, auto_run: true } if message == "turn done"
        ));
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn method_notify_weak_json_roundtrip_serializes_auto_run_false() {
        let json = r#"{"method":"notify","params":{"message":"progress","auto_run":false}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(
            method,
            Method::Notify { ref message, auto_run: false } if message == "progress"
        ));
        assert_eq!(serde_json::to_string(&method).unwrap(), json);
    }

    #[test]
    fn method_list_completions_roundtrip() {
        let method = Method::ListCompletions {
            kind: CompletionKind::File,
            prefix: "src/".into(),
        };
        let json = serde_json::to_string(&method).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["method"], "list_completions");
        assert_eq!(parsed["params"]["kind"], "file");
        assert_eq!(parsed["params"]["prefix"], "src/");

        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::ListCompletions { kind, prefix } => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "src/");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
    }

    #[test]
    fn event_completions_format_and_default_is_dir() {
        let event = Event::Completions {
            kind: CompletionKind::Workflow,
            entries: vec![CompletionEntry {
                value: "clear".into(),
                is_dir: false,
            }],
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "completions");
        assert_eq!(parsed["data"]["kind"], "workflow");
        assert_eq!(parsed["data"]["entries"][0]["value"], "clear");

        // is_dir defaults to false on inbound payloads that omit it.
        let inbound =
            r#"{"event":"completions","data":{"kind":"file","entries":[{"value":"main.rs"}]}}"#;
        let decoded: Event = serde_json::from_str(inbound).unwrap();
        match decoded {
            Event::Completions { kind, entries } => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].value, "main.rs");
                assert!(!entries[0].is_dir);
            }
            other => panic!("expected Completions, got {other:?}"),
        }
    }

    #[test]
    fn event_snapshot_format() {
        let event = Event::Snapshot {
            entries: vec![serde_json::json!({"kind": "user_input", "ts": 1, "segments": []})],
            greeting: Greeting {
                pod_name: "test".into(),
                cwd: "/tmp".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                scope_summary: "Writable:\n  - /tmp".into(),
                tools: vec!["Read".into()],
                context_window: 200_000,
                context_tokens: 42_000,
            },
            status: PodStatus::Paused,
            in_flight: InFlightSnapshot::default(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "snapshot");
        assert!(parsed["data"]["entries"].is_array());
        assert_eq!(parsed["data"]["entries"][0]["kind"], "user_input");
        assert_eq!(parsed["data"]["greeting"]["pod_name"], "test");
        assert_eq!(parsed["data"]["greeting"]["tools"][0], "Read");
        assert_eq!(parsed["data"]["greeting"]["context_window"], 200_000);
        assert_eq!(parsed["data"]["greeting"]["context_tokens"], 42_000);
        assert_eq!(parsed["data"]["status"], "paused");
    }

    #[test]
    fn event_snapshot_in_flight_roundtrip_and_default() {
        let inbound = r#"{"event":"snapshot","data":{"entries":[],"greeting":{"pod_name":"test","cwd":"/tmp","provider":"p","model":"m","scope_summary":"s","tools":[]},"status":"running"}}"#;
        let decoded: Event = serde_json::from_str(inbound).unwrap();
        match decoded {
            Event::Snapshot { in_flight, .. } => assert!(in_flight.is_empty()),
            other => panic!("expected Snapshot, got {other:?}"),
        }

        let event = Event::Snapshot {
            entries: Vec::new(),
            greeting: Greeting {
                pod_name: "test".into(),
                cwd: "/tmp".into(),
                provider: "p".into(),
                model: "m".into(),
                scope_summary: "s".into(),
                tools: Vec::new(),
                context_window: 0,
                context_tokens: 0,
            },
            status: PodStatus::Running,
            in_flight: InFlightSnapshot {
                blocks: vec![
                    InFlightBlock::Text {
                        text: "hel".into(),
                        finished: false,
                    },
                    InFlightBlock::Thinking {
                        text: "why".into(),
                        finished: true,
                    },
                    InFlightBlock::ToolCall {
                        id: "call_1".into(),
                        name: "Read".into(),
                        args: r#"{"file"#.into(),
                        state: InFlightToolCallState::StreamingArgs,
                    },
                ],
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["data"]["in_flight"]["blocks"][0]["text"], "hel");
        assert_eq!(parsed["data"]["in_flight"]["blocks"][1]["finished"], true);
        assert_eq!(
            parsed["data"]["in_flight"]["blocks"][2]["state"],
            "streaming_args"
        );

        match serde_json::from_str::<Event>(&json).unwrap() {
            Event::Snapshot { in_flight, .. } => assert_eq!(in_flight.blocks.len(), 3),
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    #[test]
    fn event_segment_rotated_roundtrip() {
        let event = Event::SegmentRotated {
            entry: serde_json::json!({"kind": "segment_start", "ts": 1, "history": []}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "segment_rotated");
        assert_eq!(parsed["data"]["entry"]["kind"], "segment_start");
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::SegmentRotated { entry } => assert_eq!(entry["kind"], "segment_start"),
            other => panic!("expected SegmentRotated, got {other:?}"),
        }
    }

    #[test]
    fn event_system_item_roundtrip() {
        let event = Event::SystemItem {
            item: serde_json::json!({"kind": "notification", "message": "hello"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "system_item");
        assert_eq!(parsed["data"]["item"]["kind"], "notification");
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::SystemItem { item } => assert_eq!(item["kind"], "notification"),
            other => panic!("expected SystemItem, got {other:?}"),
        }
    }

    #[test]
    fn event_status_format() {
        let event = Event::Status {
            status: PodStatus::Running,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "status");
        assert_eq!(parsed["data"]["status"], "running");

        let decoded: Event = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Event::Status {
                status: PodStatus::Running
            }
        ));
    }

    #[test]
    fn event_snapshot_legacy_without_status_defaults_to_idle() {
        let json = r#"{"event":"snapshot","data":{"entries":[],"greeting":{"pod_name":"test","cwd":"/tmp","provider":"anthropic","model":"claude","scope_summary":"","tools":[]}}}"#;
        let decoded: Event = serde_json::from_str(json).unwrap();
        match decoded {
            Event::Snapshot {
                status, greeting, ..
            } => {
                assert_eq!(status, PodStatus::Idle);
                assert_eq!(greeting.context_window, 0);
                assert_eq!(greeting.context_tokens, 0);
            }
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    #[test]
    fn method_pod_event_turn_ended_roundtrip() {
        let method = Method::PodEvent(PodEvent::TurnEnded {
            pod_name: "child".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["method"], "pod_event");
        assert_eq!(parsed["params"]["kind"], "turn_ended");
        assert_eq!(parsed["params"]["pod_name"], "child");

        let decoded: Method = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Method::PodEvent(PodEvent::TurnEnded { ref pod_name }) if pod_name == "child"
        ));
    }

    #[test]
    fn method_pod_event_errored_roundtrip() {
        let method = Method::PodEvent(PodEvent::Errored {
            pod_name: "child".into(),
            message: "provider 429".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::PodEvent(PodEvent::Errored { pod_name, message }) => {
                assert_eq!(pod_name, "child");
                assert_eq!(message, "provider 429");
            }
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[test]
    fn method_pod_event_shutdown_roundtrip() {
        let method = Method::PodEvent(PodEvent::ShutDown {
            pod_name: "child".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Method::PodEvent(PodEvent::ShutDown { ref pod_name }) if pod_name == "child"
        ));
    }

    #[test]
    fn pod_event_agent_notification_classification() {
        assert!(
            PodEvent::TurnEnded {
                pod_name: "child".into()
            }
            .should_notify_agent()
        );
        assert!(
            PodEvent::Errored {
                pod_name: "child".into(),
                message: "boom".into()
            }
            .should_notify_agent()
        );
        assert!(
            PodEvent::ShutDown {
                pod_name: "child".into()
            }
            .should_notify_agent()
        );
        assert!(
            !PodEvent::ScopeSubDelegated {
                parent_pod: "child".into(),
                sub_pod: "grandchild".into(),
                sub_socket: "/tmp/grandchild.sock".into(),
                scope: vec![],
            }
            .should_notify_agent()
        );
    }

    #[test]
    fn method_pod_event_scope_sub_delegated_roundtrip() {
        let method = Method::PodEvent(PodEvent::ScopeSubDelegated {
            parent_pod: "child".into(),
            sub_pod: "grandchild".into(),
            sub_socket: "/run/yoi/grandchild/sock".into(),
            scope: vec![ScopeRule {
                target: "/tmp/work".into(),
                permission: Permission::Write,
                recursive: true,
            }],
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::PodEvent(PodEvent::ScopeSubDelegated {
                parent_pod,
                sub_pod,
                sub_socket,
                scope,
            }) => {
                assert_eq!(parent_pod, "child");
                assert_eq!(sub_pod, "grandchild");
                assert_eq!(sub_socket, PathBuf::from("/run/yoi/grandchild/sock"));
                assert_eq!(scope.len(), 1);
                assert_eq!(scope[0].target, PathBuf::from("/tmp/work"));
                assert_eq!(scope[0].permission, Permission::Write);
                assert!(scope[0].recursive);
            }
            other => panic!("expected ScopeSubDelegated, got {other:?}"),
        }
    }

    #[test]
    fn event_alert_format() {
        let event = Event::Alert(Alert {
            level: AlertLevel::Warn,
            source: AlertSource::Compactor,
            message: "compaction failed".into(),
            timestamp_ms: 1_700_000_000_000,
        });
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "alert");
        assert_eq!(parsed["data"]["level"], "warn");
        assert_eq!(parsed["data"]["source"], "compactor");
        assert_eq!(parsed["data"]["message"], "compaction failed");
        assert_eq!(parsed["data"]["timestamp_ms"], 1_700_000_000_000i64);
    }

    #[test]
    fn event_compact_start_roundtrip() {
        let event = Event::CompactStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"event":"compact_start"}"#);
        let decoded: Event = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, Event::CompactStart));
    }

    #[test]
    fn event_compact_done_roundtrip() {
        let id = uuid::Uuid::parse_str("0192f0e8-4d84-7d6e-a000-000000000001").unwrap();
        let event = Event::CompactDone { new_segment_id: id };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "compact_done");
        assert_eq!(
            parsed["data"]["new_segment_id"],
            "0192f0e8-4d84-7d6e-a000-000000000001"
        );
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::CompactDone { new_segment_id } => assert_eq!(new_segment_id, id),
            other => panic!("expected CompactDone, got {other:?}"),
        }
    }

    #[test]
    fn event_compact_failed_roundtrip() {
        let event = Event::CompactFailed {
            error: "provider 429".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "compact_failed");
        assert_eq!(parsed["data"]["error"], "provider 429");
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::CompactFailed { error } => assert_eq!(error, "provider 429"),
            other => panic!("expected CompactFailed, got {other:?}"),
        }
    }

    #[test]
    fn event_tool_result_roundtrip() {
        let event = Event::ToolResult {
            id: "call_1".into(),
            summary: "Read 128 bytes".into(),
            output: Some("hello world".into()),
            is_error: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "tool_result");
        assert_eq!(parsed["data"]["id"], "call_1");
        assert_eq!(parsed["data"]["summary"], "Read 128 bytes");
        assert_eq!(parsed["data"]["output"], "hello world");
        assert_eq!(parsed["data"]["is_error"], false);

        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::ToolResult {
                id,
                summary,
                output,
                is_error,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(summary, "Read 128 bytes");
                assert_eq!(output.as_deref(), Some("hello world"));
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn event_tool_result_omits_absent_output() {
        let event = Event::ToolResult {
            id: "call_2".into(),
            summary: "ok".into(),
            output: None,
            is_error: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("\"output\""),
            "absent output must not be serialized: {json}"
        );
    }

    #[test]
    fn event_tool_result_error_roundtrip() {
        let event = Event::ToolResult {
            id: "call_3".into(),
            summary: "invalid argument".into(),
            output: None,
            is_error: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["data"]["is_error"], true);

        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::ToolResult {
                summary,
                output,
                is_error,
                ..
            } => {
                assert_eq!(summary, "invalid argument");
                assert!(output.is_none());
                assert!(is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn event_error_format() {
        let event = Event::Error {
            code: ErrorCode::AlreadyRunning,
            message: "Pod is already executing a turn".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "error");
        assert_eq!(parsed["data"]["code"], "already_running");
    }

    #[test]
    fn event_user_message_roundtrip() {
        let event = Event::UserMessage {
            segments: vec![Segment::text("hello 世界")],
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "user_message");
        assert_eq!(parsed["data"]["segments"][0]["kind"], "text");
        assert_eq!(parsed["data"]["segments"][0]["content"], "hello 世界");

        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::UserMessage { segments } => {
                assert_eq!(segments.len(), 1);
                match &segments[0] {
                    Segment::Text { content } => assert_eq!(content, "hello 世界"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn pod_discovery_methods_roundtrip() {
        let methods = [
            Method::ListPods,
            Method::RestorePod {
                name: "child".into(),
            },
            Method::RegisterPeer {
                name: "peer".into(),
            },
        ];
        for method in methods {
            let json = serde_json::to_string(&method).unwrap();
            let decoded: Method = serde_json::from_str(&json).unwrap();
            match (decoded, method) {
                (Method::ListPods, Method::ListPods)
                | (Method::RestorePod { .. }, Method::RestorePod { .. })
                | (Method::RegisterPeer { .. }, Method::RegisterPeer { .. }) => {}
                (decoded, expected) => panic!("decoded {decoded:?}, expected {expected:?}"),
            }
        }
    }

    #[test]
    fn pod_discovery_events_roundtrip() {
        let events = [
            Event::PodsListed {
                pods: serde_json::json!([{ "pod_name": "child" }]),
            },
            Event::PodRestored {
                result: serde_json::json!({ "action": "already_live" }),
            },
            Event::PeerRegistered {
                result: serde_json::json!({ "source": "self", "peer": "other" }),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: Event = serde_json::from_str(&json).unwrap();
            match (decoded, event) {
                (Event::PodsListed { pods }, Event::PodsListed { pods: expected }) => {
                    assert_eq!(pods, expected)
                }
                (Event::PodRestored { result }, Event::PodRestored { result: expected }) => {
                    assert_eq!(result, expected)
                }
                (Event::PeerRegistered { result }, Event::PeerRegistered { result: expected }) => {
                    assert_eq!(result, expected)
                }
                (decoded, expected) => panic!("decoded {decoded:?}, expected {expected:?}"),
            }
        }
    }
}
