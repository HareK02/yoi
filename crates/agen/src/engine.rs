use std::collections::HashMap;
use std::{marker::PhantomData, sync::Arc, time::Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::{
    History, HistoryEntry, Item,
    callback::{
        ClosureMetaHandler, ClosureTextBlockHandler, ClosureThinkingBlockHandler,
        ClosureToolUseBlockHandler, TextBlockScope, ThinkingBlockScope, ToolUseBlockScope,
    },
    handler::{ErrorKind, StatusKind, ToolUseBlockStart, UsageKind},
    interceptor::{
        DefaultInterceptor, Interceptor, PostToolAction, PreRequestAction, PreToolAction,
        PromptAction, ToolCallInfo, ToolResultInfo, TurnEndAction,
    },
    llm_client::{
        ClientError, ConfigWarning, LlmClient, Request, RequestConfig, ResponseStream,
        ToolDefinition, error::is_retryable, event::Event, retry::RetryPolicy,
        transport::DEFAULT_FIRST_STREAM_EVENT_TIMEOUT, types::parse_tool_arguments,
    },
    state::{EngineState, Locked, Mutable},
    timeline::event::{ErrorEvent, StatusEvent, UsageEvent},
    timeline::{TextBlockCollector, ThinkingBlockCollector, Timeline, ToolCallCollector},
    tool::{
        ToolCall, ToolDefinition as EngineToolDefinition, ToolError, ToolExecutionContext,
        ToolOutputLimits, ToolResult, truncate_content,
    },
    tool_server::{ToolServer, ToolServerHandle},
};

/// Engine errors
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Client error
    #[error("Client error: {0}")]
    Client(#[from] ClientError),
    /// Tool error
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    /// Execution was aborted
    #[error("Aborted: {0}")]
    Aborted(String),
    /// Cancelled by CancellationToken
    #[error("Cancelled")]
    Cancelled,
    /// Config warnings (unsupported options)
    #[error("Config warnings: {}", .0.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(", "))]
    ConfigWarnings(Vec<ConfigWarning>),
    /// A durable-history observer rejected an item before it entered history.
    #[error("History append failed: {0}")]
    HistoryAppend(String),
}

/// Tool registration error
#[derive(Debug, thiserror::Error)]
pub enum ToolRegistryError {
    /// A tool with the same name is already registered
    #[error("Tool with name '{0}' already registered")]
    DuplicateName(String),
}

/// Engine configuration
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    // Reserved for future extensions (currently empty)
    _private: (),
}

/// Project terminal tool outputs into the assistant's original ToolCall order.
///
/// Runtime history intentionally retains completion order so every result can
/// be committed without waiting for slower siblings. The provider projection
/// is deterministic within each contiguous result batch and does not rewrite
/// the committed transcript.
struct ProviderHistoryProjection {
    items: Vec<Item>,
    original_to_projected_index: Vec<usize>,
}

fn materialize_provider_history(items: &[Item]) -> ProviderHistoryProjection {
    let mut materialized: Vec<_> = items.iter().cloned().enumerate().collect();
    let mut call_order = HashMap::<String, usize>::new();
    let mut next_call_order = 0usize;
    let mut index = 0usize;

    while index < materialized.len() {
        match &materialized[index].1 {
            Item::ToolCall { call_id, .. } => {
                call_order.insert(call_id.clone(), next_call_order);
                next_call_order += 1;
                index += 1;
            }
            Item::ToolResult { .. } => {
                let start = index;
                while index < materialized.len()
                    && matches!(materialized[index].1, Item::ToolResult { .. })
                {
                    index += 1;
                }
                materialized[start..index].sort_by_key(|(_, item)| match item {
                    Item::ToolResult { call_id, .. } => {
                        call_order.get(call_id).copied().unwrap_or(usize::MAX)
                    }
                    _ => unreachable!("tool-result run contains only ToolResult items"),
                });
            }
            _ => index += 1,
        }
    }

    let mut original_to_projected_index = vec![0; materialized.len()];
    for (projected_index, (original_index, _)) in materialized.iter().enumerate() {
        original_to_projected_index[*original_index] = projected_index;
    }

    ProviderHistoryProjection {
        items: materialized.into_iter().map(|(_, item)| item).collect(),
        original_to_projected_index,
    }
}

/// Legacy serializable outcome used by the Worker session-log compatibility boundary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EngineResult {
    Finished,
    Paused,
    LimitReached,
    Yielded,
}

/// The public termination boundary for one logical engine run.
#[derive(Debug)]
pub enum EngineRunExit {
    Finished,
    Paused,
    Yielded,
    Interrupted(StopReason),
}

/// A typed reason why an engine run could not finish normally.
#[derive(Debug)]
pub enum StopReason {
    LimitReached,
    ContextWindowExceeded,
    Cancelled,
    Unexpected(EngineError),
}

impl From<Result<EngineResult, EngineError>> for EngineRunExit {
    fn from(result: Result<EngineResult, EngineError>) -> Self {
        match result {
            Ok(EngineResult::Finished) => Self::Finished,
            Ok(EngineResult::Paused) => Self::Paused,
            Ok(EngineResult::Yielded) => Self::Yielded,
            Ok(EngineResult::LimitReached) => Self::Interrupted(StopReason::LimitReached),
            Err(EngineError::Client(ClientError::ContextWindowExceeded)) => {
                Self::Interrupted(StopReason::ContextWindowExceeded)
            }
            Err(EngineError::Cancelled) => Self::Interrupted(StopReason::Cancelled),
            Err(error) => Self::Interrupted(StopReason::Unexpected(error)),
        }
    }
}

/// Result of [`Engine::run`] or [`Engine::resume`].
///
/// Contains the `Locked` Engine (ready for subsequent runs) and the outcome.
pub struct EngineRunOutput<C: LlmClient, A = ()> {
    /// The Engine, now in Locked state.
    pub engine: Engine<C, Locked, A>,
    /// Outcome of the turn.
    pub result: EngineRunExit,
}

/// Internal: tool execution result
enum ToolExecutionResult {
    Completed,
    Paused,
}

const MAX_STREAM_CONTINUATIONS: u32 = 3;

/// Central component for managing LLM interactions
///
/// Receives input from the user, sends requests to the LLM, and
/// automatically executes tool calls if any, advancing the turn.
///
/// # State Transitions (Type-state)
///
/// - [`Mutable`]: Initial state. System prompt and tools can be edited; history is caller-owned.
/// - [`Locked`]: Cache-protected state. Prefix context is immutable; only `run()` / `resume()` are available.
///
/// Calling `run()` on a `Mutable` Engine consumes it and returns a
/// `Locked` Engine together with the result. The engine borrows the caller's
/// [`History`](crate::History) only while running, so host annotations stay with
/// the host-owned history and are never projected to providers.
///
/// ```ignore
/// let mut history = History::new();
/// let mut engine = Engine::new(client)
///     .system_prompt("You are a helpful assistant.");
/// engine.register_tool(my_tool);
///
/// // Mutable::run() consumes self → EngineRunOutput { engine: Locked, result }
/// let out = engine.run(&mut history, "Hello").await?;
/// let mut engine = out.engine;
///
/// // Locked::run() borrows &mut self
/// engine.run(&mut history, "Follow-up").await?;
///
/// // To edit between turns, unlock back to Mutable
/// let mut engine = engine.unlock();
/// history.truncate(5);
/// let out = engine.run(&mut history, "Continue").await?;
/// let mut engine = out.engine;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRetryNotice {
    /// 直近で失敗した attempt 番号。1 origin。
    pub failed_attempt: u32,
    pub max_attempts: u32,
    pub wait: std::time::Duration,
    pub elapsed: std::time::Duration,
    pub status: Option<u16>,
    pub error: String,
}

#[derive(Debug)]
enum StreamCompletion {
    Complete,
    Interrupted { reason: String },
}

pub struct Engine<C: LlmClient, S: EngineState = Mutable, A = ()> {
    /// LLM client
    client: C,
    /// Retry policy for opening an LLM response stream.
    retry_policy: RetryPolicy,
    /// Event timeline
    timeline: Timeline,
    /// Text block collector (Timeline handler)
    text_block_collector: TextBlockCollector,
    /// Tool call collector (Timeline handler)
    tool_call_collector: ToolCallCollector,
    /// Thinking block collector (Timeline handler)。metadata 付きで完了した
    /// Thinking block を 1 ターン分バッファし、history に append する。
    thinking_block_collector: ThinkingBlockCollector,
    /// Tool server handle
    tool_server: ToolServerHandle,
    /// Interceptor for control-flow decisions
    interceptor: Box<dyn Interceptor>,
    /// System prompt
    system_prompt: Option<String>,
    /// History length at lock time (only meaningful in Locked state)
    locked_prefix_len: usize,
    /// AgentTurn count across the lifetime of this Engine.
    ///
    /// Once retry (`agen-stream-continuation`) is implemented, an
    /// AgentTurn collapses N retried `LlmCall`s with identical input;
    /// today retry is not implemented so AgentTurn and LlmCall fire 1:1
    /// and the increment site (the LLM-call loop) is shared.
    turn_count: usize,
    /// AgentTurns consumed by the currently active logical run.
    ///
    /// A fresh [`run`](Self::run) starts at zero. Pause and Yield retain the
    /// count for [`resume`](Self::resume), while terminal outcomes clear it.
    /// `max_turns` is enforced against this run-scoped count rather than the
    /// cumulative `turn_count` above.
    active_run_turn_count: Option<usize>,
    /// LlmCall count (per-Engine running counter, monotonic). Unlike
    /// `turn_count` this never collapses retries.
    llm_call_count: usize,
    /// Tool execution batch count (per-Engine running counter, monotonic).
    /// Each batch corresponds to one collected assistant tool-call set or one
    /// resumed pending tool-call set.
    tool_execution_batch_count: usize,
    /// Maximum number of AgentTurns (None = unlimited)
    max_turns: Option<u32>,
    /// AgentTurn-start callbacks (1:1 with LlmCall today)
    turn_start_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// AgentTurn-end callbacks (1:1 with LlmCall today)
    turn_end_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// LlmCall-start callbacks (per individual LLM generation request,
    /// retries included once retry lands)
    llm_call_start_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// LlmCall-end callbacks
    llm_call_end_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// Transport-level retry callbacks for a specific LlmCall.
    llm_retry_cbs: Vec<Box<dyn Fn(usize, &LlmRetryNotice) + Send + Sync>>,
    /// Stream continuation callbacks for a specific LlmCall.
    llm_continuation_cbs: Vec<Box<dyn Fn(usize, u32, u32, &str) + Send + Sync>>,
    /// Stream event callbacks. Fired for every normalized provider stream
    /// event before it enters the Timeline.
    stream_event_cbs: Vec<Box<dyn Fn(usize, usize, &Event) + Send + Sync>>,
    /// Pre-stream lifecycle callbacks for debugging stalls before provider
    /// stream events become visible.
    lifecycle_trace_cbs: Vec<Arc<dyn Fn(usize, usize, &str, &Value) + Send + Sync>>,
    /// Non-fatal warning callbacks. Invoked when the Engine wants to
    /// surface an advisory message to the caller so it can be forwarded
    /// to the user — distinct from `tracing::warn!`, which is for
    /// developer-facing logs.
    warning_cbs: Vec<Box<dyn Fn(&str) + Send + Sync>>,
    /// Tool-result callbacks. Invoked once per completed tool call
    /// after post-execution interceptors and the output byte-cap
    /// truncation have been applied — i.e. on the same data that
    /// enters history.
    tool_result_cbs: Vec<Box<dyn Fn(&ToolResult) + Send + Sync>>,
    /// History-append callbacks. Invoked before non-streamed items enter
    /// engine history. An error rejects the item and aborts the turn, allowing
    /// upper layers to make durable storage the commit gate.
    history_append_cbs: Vec<Box<dyn Fn(&Item) -> Result<(), String> + Send + Sync>>,
    /// Request configuration (max_tokens, temperature, etc.)
    request_config: RequestConfig,
    /// Cancel notification channel (for interrupting execution)
    cancel_tx: mpsc::Sender<()>,
    cancel_rx: mpsc::Receiver<()>,
    /// Byte-size caps applied to tool `content` before it reaches history.
    /// `None` disables truncation (tests and minimal setups).
    tool_output_limits: Option<ToolOutputLimits>,
    /// Prune configuration. `None` disables the prune projection.
    prune_config: Option<crate::prune::PruneConfig>,
    /// Callback that estimates prefix token counts, injected by higher
    /// layers that own usage measurements. `None` disables the prune
    /// projection.
    token_estimator: Option<crate::prune::TokenEstimator>,
    /// Callback that estimates token savings for a drop range, injected
    /// by higher layers that own usage measurements. `None` disables
    /// the prune projection.
    savings_estimator: Option<crate::prune::SavingsEstimator>,
    /// Optional observer fired once per prune evaluation (regardless of
    /// whether projection actually fired). `None` disables instrumentation.
    prune_observer: Option<crate::prune::PruneObserver>,
    /// Index of the last stable cache prefix item, set by higher layers.
    /// Plumbed into [`Request::cache_anchor`] at request build time.
    cache_anchor: Option<usize>,
    /// Conversation-scoped cache key, set by higher layers. Plumbed into
    /// [`Request::cache_key`] at request build time. Callers should pass a
    /// stable conversation identifier when the backend benefits from one.
    cache_key: Option<String>,
    /// State marker
    _state: PhantomData<(S, A)>,
}

impl<C: LlmClient, S: EngineState, A> Engine<C, S, A> {
    fn start_logical_run(&mut self) {
        self.active_run_turn_count = Some(0);
    }

    fn ensure_logical_run(&mut self) {
        self.active_run_turn_count.get_or_insert(0);
    }

    fn finish_logical_run(&mut self, result: &Result<EngineResult, EngineError>) {
        if !matches!(result, Ok(EngineResult::Paused) | Ok(EngineResult::Yielded)) {
            self.active_run_turn_count = None;
        }
    }

    fn drain_cancel_queue(&mut self) {
        while self.cancel_rx.try_recv().is_ok() {}
    }

    /// Discard pending cancellation notifications while the engine is idle.
    ///
    /// Cancellation is a running-turn control signal. Callers that own a higher
    /// level run state can use this before starting a new turn so an old idle
    /// signal does not poison the next request, while cancellation queued after
    /// the run has been accepted remains observable by the turn loop.
    pub fn clear_pending_cancel(&mut self) {
        self.drain_cancel_queue();
    }

    fn try_cancelled(&mut self) -> bool {
        use tokio::sync::mpsc::error::TryRecvError;
        match self.cancel_rx.try_recv() {
            Ok(()) => true,
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => true,
        }
    }

    /// Register a text block observer with scoped callbacks.
    ///
    /// The setup closure is called once per text block. Inside it, register
    /// `on_delta` and/or `on_stop` callbacks on the provided scope.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.on_text_block(|block| {
    ///     block.on_delta(|text| print!("{}", text));
    ///     block.on_stop(|full_text| println!("\n--- {} chars ---", full_text.len()));
    /// });
    /// ```
    pub fn on_text_block(
        &mut self,
        setup: impl FnMut(&mut TextBlockScope) + Send + Sync + 'static,
    ) {
        self.timeline.on_text_block(ClosureTextBlockHandler {
            setup: Box::new(setup),
        });
    }

    /// Register a thinking block observer with scoped callbacks.
    ///
    /// Mirrors `on_text_block`. Some providers don't expose plaintext
    /// reasoning content; in that case the block fires Start and Stop
    /// with no Delta in between, and `on_stop` receives an empty string.
    pub fn on_thinking_block(
        &mut self,
        setup: impl FnMut(&mut ThinkingBlockScope) + Send + Sync + 'static,
    ) {
        self.timeline
            .on_thinking_block(ClosureThinkingBlockHandler {
                setup: Box::new(setup),
            });
    }

    /// Register a tool use block observer with scoped callbacks.
    ///
    /// The setup closure receives `&ToolUseBlockStart` (containing `id` and `name`)
    /// and a scope for registering `on_delta` and `on_stop` callbacks.
    ///
    /// `on_stop` receives a fully assembled `&ToolCall` with parsed JSON input.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.on_tool_use_block(|start, block| {
    ///     println!("Tool: {} ({})", start.name, start.id);
    ///     block.on_delta(|json| { /* streaming JSON fragment */ });
    ///     block.on_stop(|call| println!("Done: {}", call.name));
    /// });
    /// ```
    pub fn on_tool_use_block(
        &mut self,
        setup: impl FnMut(&ToolUseBlockStart, &mut ToolUseBlockScope) + Send + Sync + 'static,
    ) {
        self.timeline.on_tool_use_block(ClosureToolUseBlockHandler {
            setup: Box::new(setup),
        });
    }

    /// Register a usage event callback.
    pub fn on_usage(&mut self, callback: impl FnMut(&UsageEvent) + Send + Sync + 'static) {
        self.timeline.on_usage(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<UsageKind>,
        });
    }

    /// Register a status event callback.
    pub fn on_status(&mut self, callback: impl FnMut(&StatusEvent) + Send + Sync + 'static) {
        self.timeline.on_status(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<StatusKind>,
        });
    }

    /// Register an error event callback.
    pub fn on_error(&mut self, callback: impl FnMut(&ErrorEvent) + Send + Sync + 'static) {
        self.timeline.on_error(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<ErrorKind>,
        });
    }

    /// Register an AgentTurn-start callback (receives the AgentTurn
    /// index from `turn_count`).
    ///
    /// Today fires 1:1 with the per-LLM-call boundary because retry is
    /// not yet implemented. Once retry lands, this will fire only once
    /// per AgentTurn (= retried LlmCall group with identical input).
    pub fn on_turn_start(&mut self, callback: impl Fn(usize) + Send + Sync + 'static) {
        self.turn_start_cbs.push(Box::new(callback));
    }

    /// Register an LlmCall-start callback (receives the LlmCall index
    /// from `llm_call_count`). Fires once per LLM generation request,
    /// retries included.
    pub fn on_llm_call_start(&mut self, callback: impl Fn(usize) + Send + Sync + 'static) {
        self.llm_call_start_cbs.push(Box::new(callback));
    }

    /// Register an LlmCall-end callback.
    pub fn on_llm_call_end(&mut self, callback: impl Fn(usize) + Send + Sync + 'static) {
        self.llm_call_end_cbs.push(Box::new(callback));
    }

    /// Register a transport-level retry callback.
    pub fn on_llm_retry(
        &mut self,
        callback: impl Fn(usize, &LlmRetryNotice) + Send + Sync + 'static,
    ) {
        self.llm_retry_cbs.push(Box::new(callback));
    }

    /// Register a stream continuation callback.
    pub fn on_llm_continuation(
        &mut self,
        callback: impl Fn(usize, u32, u32, &str) + Send + Sync + 'static,
    ) {
        self.llm_continuation_cbs.push(Box::new(callback));
    }

    fn emit_llm_continuation(
        &self,
        llm_call: usize,
        attempt: u32,
        max_attempts: u32,
        reason: &str,
    ) {
        for cb in &self.llm_continuation_cbs {
            cb(llm_call, attempt, max_attempts, reason);
        }
    }

    /// Register a raw normalized stream event callback.
    pub fn on_stream_event(
        &mut self,
        callback: impl Fn(usize, usize, &Event) + Send + Sync + 'static,
    ) {
        self.stream_event_cbs.push(Box::new(callback));
    }

    fn emit_stream_event(&self, turn: usize, llm_call: usize, event: &Event) {
        for cb in &self.stream_event_cbs {
            cb(turn, llm_call, event);
        }
    }

    /// Register a pre-stream lifecycle trace callback.
    pub fn on_lifecycle_trace(
        &mut self,
        callback: impl Fn(usize, usize, &str, &Value) + Send + Sync + 'static,
    ) {
        self.lifecycle_trace_cbs.push(Arc::new(callback));
    }

    fn emit_lifecycle_trace(&self, turn: usize, llm_call: usize, label: &str, data: Value) {
        for cb in &self.lifecycle_trace_cbs {
            cb(turn, llm_call, label, &data);
        }
    }

    fn attach_transport_trace(&self, request: Request, turn: usize, llm_call: usize) -> Request {
        if self.lifecycle_trace_cbs.is_empty() {
            return request;
        }

        let callbacks = self.lifecycle_trace_cbs.clone();
        request.transport_trace(move |label, data| {
            for cb in &callbacks {
                cb(turn, llm_call, label, &data);
            }
        })
    }

    /// Register a non-fatal warning callback.
    ///
    /// The callback is invoked with a short human-readable message
    /// whenever the Engine encounters a condition that should be
    /// surfaced to a human (e.g. tool output byte-cap truncation).
    /// This channel is separate from `tracing::warn!`, which remains
    /// in place for developer logs.
    pub fn on_warning(&mut self, callback: impl Fn(&str) + Send + Sync + 'static) {
        self.warning_cbs.push(Box::new(callback));
    }

    fn emit_warning(&self, message: &str) {
        for cb in &self.warning_cbs {
            cb(message);
        }
    }

    /// Register a callback invoked once per completed tool execution.
    ///
    /// Fired after `post_tool_call` interceptors and any `content`
    /// truncation from `tool_output_limits`, so the callback observes
    /// exactly what is persisted to history. Intended for callers that need
    /// to forward tool results to clients.
    pub fn on_tool_result(&mut self, callback: impl Fn(&ToolResult) + Send + Sync + 'static) {
        self.tool_result_cbs.push(Box::new(callback));
    }

    fn emit_tool_result(&self, result: &ToolResult) {
        for cb in &self.tool_result_cbs {
            cb(result);
        }
    }

    /// Register a fallible callback invoked before an item enters engine
    /// history. Returning an error rejects that item and aborts the turn.
    pub fn on_history_append(
        &mut self,
        callback: impl Fn(&Item) -> Result<(), String> + Send + Sync + 'static,
    ) {
        self.history_append_cbs.push(Box::new(callback));
    }

    fn emit_history_append(&self, item: &Item) -> Result<(), EngineError> {
        for cb in &self.history_append_cbs {
            cb(item).map_err(EngineError::HistoryAppend)?;
        }
        Ok(())
    }

    fn append_history_items(
        &mut self,
        history: &mut History<A>,
        items: impl IntoIterator<Item = Item>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<(), EngineError> {
        for item in items {
            self.emit_history_append(&item)?;
            history
                .append_with(item, annotate)
                .map_err(EngineError::HistoryAppend)?;
        }
        Ok(())
    }

    fn request_trace_payload(&self, request: &Request) -> Value {
        items_trace_payload(
            &request.items,
            request.tools.len(),
            request.cache_anchor,
            request.cache_key.is_some(),
        )
    }

    /// Register an AgentTurn-end callback. See [`on_turn_start`](Self::on_turn_start)
    /// for the 1:1-vs-N relation with `LlmCall*`.
    pub fn on_turn_end(&mut self, callback: impl Fn(usize) + Send + Sync + 'static) {
        self.turn_end_cbs.push(Box::new(callback));
    }

    /// Get a shared tool server handle.
    pub fn tool_server_handle(&self) -> ToolServerHandle {
        self.tool_server.clone()
    }

    /// Set the interceptor for control-flow decisions.
    ///
    /// The interceptor governs approval, skip, pause, and abort decisions
    /// at key points in the execution loop. If not set, the default
    /// interceptor is used (all Continue / Finish).
    pub fn set_interceptor(&mut self, interceptor: impl Interceptor + 'static) {
        self.interceptor = Box::new(interceptor);
    }

    /// Configure the prune projection applied to each outgoing request
    /// context.
    ///
    /// Both this and [`set_savings_estimator`](Self::set_savings_estimator)
    /// must be set for the projection to fire; missing either one is a
    /// no-op. See the crate-level [`prune`](crate::prune) docs for the
    /// semantics.
    pub fn set_prune_config(&mut self, config: Option<crate::prune::PruneConfig>) {
        self.prune_config = config;
    }

    /// Inject the callback used to estimate prefix token counts for prune's
    /// protected-token boundary.
    ///
    /// The callback is invoked with the *request context* (a clone of
    /// history). It must be pure/idempotent since it may be called once per
    /// LLM request. Returning `NoData` estimates makes prune skip as if no
    /// candidates existed.
    pub fn set_token_estimator(&mut self, estimator: Option<crate::prune::TokenEstimator>) {
        self.token_estimator = estimator;
    }

    /// Inject the callback used to estimate token savings for a prune
    /// candidate range.
    ///
    /// The callback is invoked with the *request context* (a clone of
    /// history) and the candidate index range. It must be pure/idempotent
    /// since it may be called once per LLM request. Return `0` to signal
    /// "no data" or "refuse to prune".
    pub fn set_savings_estimator(&mut self, estimator: Option<crate::prune::SavingsEstimator>) {
        self.savings_estimator = estimator;
    }

    /// Install an observer notified after each prune evaluation pass.
    ///
    /// Fires once per outgoing LLM request (the same point as the
    /// `prune_config` / `savings_estimator` pair), regardless of whether
    /// projection actually applied. Intended for upper layers that want
    /// to instrument fire/skip rates without owning the prune logic.
    pub fn set_prune_observer(&mut self, observer: Option<crate::prune::PruneObserver>) {
        self.prune_observer = observer;
    }

    /// Mark an index into the current history as a stable, cacheable
    /// prefix boundary. The value is included in each outgoing
    /// [`Request`] via [`Request::cache_anchor`] — caching-aware
    /// providers (Anthropic) place a long-lived breakpoint there.
    ///
    /// Pass `None` to clear. Typically set by layers that compact the
    /// conversation: after a compaction rebuilds history starting with a
    /// summary item, the anchor is `Some(0)`.
    pub fn set_cache_anchor(&mut self, anchor: Option<usize>) {
        self.cache_anchor = anchor;
    }

    /// Set the conversation-scoped cache key. Plumbed into each outgoing
    /// [`Request`] via [`Request::cache_key`] — caching-aware providers
    /// that scope cache by an explicit key (OpenAI Responses) read it as
    /// `prompt_cache_key`. Pass `None` to clear.
    pub fn set_cache_key(&mut self, key: Option<String>) {
        self.cache_key = key;
    }

    /// Get a mutable reference to the timeline (for additional handler registration)
    pub fn timeline_mut(&mut self) -> &mut Timeline {
        &mut self.timeline
    }

    /// Get a reference to the LLM client.
    pub fn client(&self) -> &C {
        &self.client
    }

    /// Borrow caller-owned annotated history entries.
    pub fn history<'h>(&self, history: &'h History<A>) -> &'h [HistoryEntry<A>] {
        history.entries()
    }

    /// Get a reference to the system prompt
    pub fn get_system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Get the current AgentTurn count.
    ///
    /// AgentTurn is a maximal run of LLM generation calls with identical
    /// input; today retry is unimplemented so this is also the LLM call
    /// count. Use [`llm_call_count`](Self::llm_call_count) when the
    /// caller specifically needs the per-LLM-call number.
    pub fn turn_count(&self) -> usize {
        self.turn_count
    }

    /// Get the AgentTurns consumed by an interrupted logical run.
    ///
    /// `Some` is retained only while Pause or Yield permits a later
    /// [`resume`](Self::resume). Terminal outcomes return this to `None`.
    pub fn active_run_turn_count(&self) -> Option<usize> {
        self.active_run_turn_count
    }

    /// Restore the persisted turn budget of an interrupted logical run.
    ///
    /// Session owners restore this together with the cumulative turn count and
    /// history. `None` means there is no resumable logical run and the next
    /// [`resume`](Self::resume) starts a fresh budget.
    pub fn set_active_run_turn_count(&mut self, turn_count: Option<usize>) {
        self.active_run_turn_count = turn_count;
    }

    /// Get the current LlmCall count (per-Engine running counter, never
    /// collapsed by retry).
    pub fn llm_call_count(&self) -> usize {
        self.llm_call_count
    }

    /// Get a reference to the current request configuration
    pub fn request_config(&self) -> &RequestConfig {
        &self.request_config
    }

    /// Set maximum tokens
    ///
    /// This setting is independent of cache lock and applies to each request.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.set_max_tokens(4096);
    /// ```
    pub fn set_max_tokens(&mut self, max_tokens: u32) {
        self.request_config.max_tokens = Some(max_tokens);
    }

    /// Set temperature
    ///
    /// Set in the range of 0.0 to 1.0 (or 2.0).
    /// Lower values produce more deterministic output, higher values produce more diverse output.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.set_temperature(0.7);
    /// ```
    pub fn set_temperature(&mut self, temperature: f32) {
        self.request_config.temperature = Some(temperature);
    }

    /// Set top_p (nucleus sampling)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.set_top_p(0.9);
    /// ```
    pub fn set_top_p(&mut self, top_p: f32) {
        self.request_config.top_p = Some(top_p);
    }

    /// Set top_k
    ///
    /// Specifies the top k tokens to consider when selecting tokens.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.set_top_k(40);
    /// ```
    pub fn set_top_k(&mut self, top_k: u32) {
        self.request_config.top_k = Some(top_k);
    }

    /// Add a stop sequence
    ///
    /// # Examples
    ///
    /// ```ignore
    /// engine.add_stop_sequence("\n\n");
    /// ```
    pub fn add_stop_sequence(&mut self, sequence: impl Into<String>) {
        self.request_config.stop_sequences.push(sequence.into());
    }

    /// Clear stop sequences
    pub fn clear_stop_sequences(&mut self) {
        self.request_config.stop_sequences.clear();
    }

    /// Get the cancel notification sender
    pub fn cancel_sender(&self) -> mpsc::Sender<()> {
        self.cancel_tx.clone()
    }

    /// Set request configuration at once
    pub fn set_request_config(&mut self, config: RequestConfig) {
        self.request_config = config;
    }

    /// Cancel execution
    ///
    /// Interrupts currently running streaming or tool execution.
    /// EngineError::Cancelled is returned at the next event loop checkpoint.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// let engine = Arc::new(Mutex::new(Engine::new(client)));
    ///
    /// // Run in another thread
    /// let worker_clone = engine.clone();
    /// tokio::spawn(async move {
    ///     let mut w = worker_clone.lock().unwrap();
    ///     w.run("Long task...").await
    /// });
    ///
    /// // Cancel
    /// engine.lock().unwrap().cancel();
    /// ```
    pub fn cancel(&self) {
        let _ = self.cancel_tx.try_send(());
    }

    /// Check if cancelled
    pub fn is_cancelled(&mut self) -> bool {
        self.try_cancelled()
    }

    /// Generate list of ToolDefinitions for LLM from registered tools
    fn build_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_server.tool_definitions_sorted()
    }

    /// Build assistant response items from reasoning items, text blocks, and tool calls.
    ///
    /// Reasoning items come first (Anthropic / OpenAI Responses 双方ともに
    /// アシスタント応答内で reasoning は先頭に並ぶ仕様)。これは Anthropic
    /// が新世代モデルで thinking ブロックを assistant メッセージの先頭に
    /// 置くことを要求するためでもある。
    fn build_assistant_items(
        &self,
        reasoning_items: &[crate::llm_client::event::ReasoningBlockData],
        text_blocks: &[String],
        tool_calls: &[ToolCall],
    ) -> Vec<Item> {
        let mut items = Vec::new();

        for r in reasoning_items {
            let mut item = Item::reasoning(r.text.clone().unwrap_or_default());
            if let Some(id) = &r.id {
                item = item.with_id(id);
            }
            if !r.summary.is_empty() {
                item = item.with_reasoning_summary(r.summary.clone());
            }
            if let Some(enc) = &r.encrypted_content {
                item = item.with_encrypted_content(enc);
            }
            if let Some(sig) = &r.signature {
                item = item.with_signature(sig);
            }
            items.push(item);
        }

        // Add text as assistant message if present
        let text = text_blocks.join("");
        if !text.is_empty() {
            items.push(Item::assistant_message(text));
        }

        // Add tool calls as ToolCall items
        for call in tool_calls {
            items.push(Item::tool_call_json(
                &call.id,
                &call.name,
                call.input.clone(),
            ));
        }

        items
    }

    /// Build a request
    fn build_request(&self, tool_definitions: &[ToolDefinition], context: &[Item]) -> Request {
        let mut request = Request::new();

        // Set system prompt
        if let Some(ref system) = self.system_prompt {
            request = request.system(system);
        }

        // History keeps terminal tool outputs in completion order so each
        // result can be committed immediately. Providers, however, expect a
        // deterministic projection matching the assistant's ToolCall order.
        let projection = materialize_provider_history(context);
        let projected_cache_anchor = self
            .cache_anchor
            .and_then(|anchor| projection.original_to_projected_index.get(anchor).copied());
        request = request.items(projection.items);

        // Add tool definitions
        for tool_def in tool_definitions {
            request = request.tool(tool_def.clone());
        }

        // Apply request configuration
        request = request.config(self.request_config.clone());

        // Attach the cache prefix anchor (may be narrower than `context`
        // if the prune projection trimmed items from the head — keep it
        // in range).
        request.cache_anchor = projected_cache_anchor;
        request.cache_key = self.cache_key.clone();

        request
    }

    /// Hooks: on_prompt_submit
    ///
    async fn finalize_interruption<T>(
        &mut self,
        result: Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        match result {
            Ok(value) => Ok(value),
            Err(err) => {
                let reason = match &err {
                    EngineError::Aborted(reason) => reason.clone(),
                    EngineError::Cancelled => "Cancelled".to_string(),
                    _ => err.to_string(),
                };
                self.interceptor.on_abort(&reason).await;
                Err(err)
            }
        }
    }

    /// Check for pending tool calls (for resuming from Pause)
    fn get_pending_tool_calls(&self, history: &History<A>) -> Option<Vec<ToolCall>> {
        // Find the last ToolCall items that don't have corresponding ToolResult
        let mut pending_calls = Vec::new();
        let mut answered_call_ids = std::collections::HashSet::new();

        // First pass: collect all answered call IDs
        for item in history.items() {
            if let Item::ToolResult { call_id, .. } = item {
                answered_call_ids.insert(call_id.clone());
            }
        }

        // Second pass: find unanswered tool calls
        for item in history.items() {
            if let Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } = item
            {
                if !answered_call_ids.contains(call_id) {
                    let input = parse_tool_arguments(arguments);
                    pending_calls.push(ToolCall {
                        id: call_id.clone(),
                        name: name.clone(),
                        input,
                    });
                }
            }
        }

        if pending_calls.is_empty() {
            None
        } else {
            Some(pending_calls)
        }
    }

    /// Execute tools in parallel
    ///
    /// After running pre_tool_call hooks on all tools,
    /// executes approved tools in parallel and applies post_tool_call hooks to results.
    async fn execute_tools(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<ToolExecutionResult, EngineError> {
        use futures::stream::{FuturesUnordered, StreamExt};

        // Map from tool call ID to (ToolCall, Meta, Tool, Context)
        // Retained because it's needed for PostToolCall hooks
        let mut call_info_map = HashMap::new();
        let mut synthetic_results = Vec::new();
        let batch_id = format!("tool-batch-{}", self.tool_execution_batch_count);
        self.tool_execution_batch_count += 1;

        // Phase 1: Apply pre_tool_call interceptor (determine skip/abort/synthetic result)
        let mut approved_calls = Vec::new();
        for (call_index, mut tool_call) in tool_calls.into_iter().enumerate() {
            let context = ToolExecutionContext::new(&tool_call.id, &batch_id, call_index);
            if let Some((meta, tool)) = self.tool_server.get_tool(&tool_call.name) {
                let mut info = ToolCallInfo {
                    call: tool_call.clone(),
                    meta,
                    tool,
                    context,
                };

                match self.interceptor.pre_tool_call(&mut info).await {
                    PreToolAction::Continue => {}
                    PreToolAction::Skip => {
                        continue;
                    }
                    PreToolAction::SyntheticResult(result) => {
                        let tool_call = info.call;
                        let mut context = info.context;
                        context.call_id = tool_call.id.clone();
                        call_info_map.insert(
                            tool_call.id.clone(),
                            (tool_call, info.meta.clone(), info.tool.clone(), context),
                        );
                        synthetic_results.push(result);
                        continue;
                    }
                    PreToolAction::Abort(reason) => {
                        return Err(EngineError::Aborted(reason));
                    }
                    PreToolAction::Pause => {
                        return Ok(ToolExecutionResult::Paused);
                    }
                }

                // Reflect changes made by interceptor
                tool_call = info.call;
                let mut context = info.context;
                context.call_id = tool_call.id.clone();

                call_info_map.insert(
                    tool_call.id.clone(),
                    (
                        tool_call.clone(),
                        info.meta.clone(),
                        info.tool.clone(),
                        context.clone(),
                    ),
                );
                approved_calls.push((tool_call, context));
            } else {
                // Unknown tools go into approved list as-is (will error at execution)
                let context = ToolExecutionContext::new(&tool_call.id, &batch_id, call_index);
                approved_calls.push((tool_call, context));
            }
        }

        // Phase 2: Execute approved tools in parallel. FuturesUnordered yields
        // each terminal result as soon as that call completes instead of
        // holding fast siblings behind the slowest call in the batch.
        let futures: FuturesUnordered<_> = approved_calls
            .into_iter()
            .map(|(tool_call, context)| {
                let tool_server = self.tool_server.clone();
                async move {
                    let input_json = serde_json::to_string(&tool_call.input).unwrap_or_default();
                    match tool_server
                        .call_tool(&tool_call.name, &input_json, context)
                        .await
                    {
                        Ok(output) => ToolResult::from_output(&tool_call.id, output),
                        Err(e) => ToolResult::error(&tool_call.id, e.to_string()),
                    }
                }
            })
            .collect();

        // Synthetic results are already terminal and need no execution wait.
        // Commit them before polling ordinary calls so they obey the same
        // commit-before-publish boundary.
        for result in synthetic_results {
            self.finalize_and_commit_tool_result(history, annotate, result, &call_info_map)
                .await?;
        }

        let mut futures = futures;
        while !futures.is_empty() {
            tokio::select! {
                // If cancellation and a completed result are both ready, drain
                // the completed result first. This preserves every terminal
                // output observed before the cancellation boundary.
                biased;
                result = futures.next() => {
                    let result = result.expect("non-empty FuturesUnordered returns a result");
                    self.finalize_and_commit_tool_result(
                        history,
                        annotate,
                        result,
                        &call_info_map,
                    ).await?;
                }
                cancel = self.cancel_rx.recv() => {
                    if cancel.is_some() {
                        info!("Tool execution cancelled");
                    }
                    self.timeline.abort_current_block();
                    return Err(EngineError::Cancelled);
                }
            }
        }

        Ok(ToolExecutionResult::Completed)
    }

    /// Apply post-execution policy, bound the model-visible payload, durably
    /// append one terminal ToolResult, and only then publish it to observers.
    async fn finalize_and_commit_tool_result(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
        mut tool_result: ToolResult,
        call_info_map: &HashMap<
            String,
            (
                ToolCall,
                crate::tool::ToolMeta,
                Arc<dyn crate::tool::Tool>,
                ToolExecutionContext,
            ),
        >,
    ) -> Result<(), EngineError> {
        let call_info = call_info_map.get(&tool_result.tool_use_id);
        if let Some((tool_call, meta, tool, context)) = call_info {
            let mut info = ToolResultInfo {
                call: tool_call.clone(),
                result: tool_result,
                meta: meta.clone(),
                tool: tool.clone(),
                context: context.clone(),
            };

            match self.interceptor.post_tool_call(&mut info).await {
                PostToolAction::Continue => {}
                PostToolAction::Abort(reason) => {
                    return Err(EngineError::Aborted(reason));
                }
            }
            tool_result = info.result;
        }

        // Cap content only after post_tool_call so interceptors still observe
        // the full payload and any content they inject is bounded too.
        if let (Some(limits), Some((tool_call, _, _, _)), Some(content)) = (
            self.tool_output_limits.as_ref(),
            call_info,
            tool_result.content.as_mut(),
        ) {
            let limit = limits.limit_for(&tool_call.name);
            let before = content.len();
            truncate_content(content, limit);
            if content.len() != before {
                warn!(
                    tool = %tool_call.name,
                    before_bytes = before,
                    after_bytes = content.len(),
                    limit_bytes = limit,
                    "Tool output exceeded byte limit and was truncated"
                );
                self.emit_warning(&format!(
                    "tool `{}` output truncated from {} to {} bytes (limit {})",
                    tool_call.name,
                    before,
                    content.len(),
                    limit
                ));
            }
        }

        let item = Item::tool_result_item_with_attachments(
            &tool_result.tool_use_id,
            &tool_result.summary,
            tool_result.content.clone(),
            tool_result.is_error,
            tool_result.attachments.clone(),
        );
        self.append_history_items(history, std::iter::once(item), annotate)?;
        self.emit_tool_result(&tool_result);
        Ok(())
    }

    /// Internal turn execution logic
    async fn run_turn_loop(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<EngineResult, EngineError> {
        let tool_definitions = self.build_tool_definitions();

        info!(
            item_count = history.len(),
            tool_count = tool_definitions.len(),
            "Starting engine run"
        );

        // Resume pending tool calls from a previous Pause
        if let Some(tool_calls) = self.get_pending_tool_calls(history) {
            info!("Resuming pending tool calls");
            if let Some(result) = self
                .execute_and_commit_tools(history, annotate, tool_calls)
                .await?
            {
                return Ok(result);
            }
        }

        let mut stream_continuations: u32 = 0;
        let mut continuing_stream = false;
        loop {
            if self.try_cancelled() {
                info!("Execution cancelled");
                self.timeline.abort_current_block();
                return Err(EngineError::Cancelled);
            }

            if let Some(max) = self.max_turns
                && self.active_run_turn_count.unwrap_or(0) >= max as usize
            {
                info!(
                    active_run_turn_count = self.active_run_turn_count.unwrap_or(0),
                    total_turn_count = self.turn_count,
                    max_turns = max,
                    "Logical run turn limit reached"
                );
                return Ok(EngineResult::LimitReached);
            }

            let current_turn = self.turn_count;
            if !continuing_stream {
                debug!(turn = current_turn, "Turn start");
                for cb in &self.turn_start_cbs {
                    cb(current_turn);
                }
            }

            // Drain interceptor-side inputs that are meant to land in
            // history (notifications, external events, system reminders).
            // These are committed *before* the per-request clone so they
            // participate in the LLM request below and get persisted by
            // the caller that owns durable history.
            let pending = self
                .interceptor
                .pending_history_appends()
                .await
                .map_err(EngineError::HistoryAppend)?;
            if !pending.is_empty() {
                self.append_history_items(history, pending, annotate)?;
            }

            // Clone the history into a per-request context. Everything
            // below (prune projection, interceptor hooks) mutates only
            // this clone, so the caller-owned `history` stays intact.
            let mut request_context = history.items_cloned();

            // Prune projection: if both the config and the savings
            // estimator are configured, drop ToolResult.content from
            // prunable candidates whose estimated savings meet the
            // threshold. Engine does not own usage history itself; the
            // estimator is injected by the layer that does.
            if let (Some(config), Some(token_estimator), Some(savings_estimator)) = (
                &self.prune_config,
                &self.token_estimator,
                &self.savings_estimator,
            ) {
                let token_estimates = token_estimator(&request_context);
                let (candidates, protected_start_index) = crate::prune::evaluate_candidates(
                    &request_context,
                    config.protected_tokens,
                    &token_estimates,
                );
                let evaluation = if candidates.is_empty() {
                    crate::prune::PruneEvaluation {
                        candidate_count: 0,
                        estimated_savings: 0,
                        protected_start_index,
                        decision: crate::prune::PruneDecision::SkippedNoCandidates,
                    }
                } else {
                    let savings = savings_estimator(&request_context, &candidates);
                    if savings >= config.min_savings {
                        let pruned = crate::prune::project(&mut request_context, &candidates);
                        if pruned > 0 {
                            debug!(
                                pruned,
                                estimated_savings_tokens = savings,
                                "Projected old tool-result content out of request context"
                            );
                        }
                        crate::prune::PruneEvaluation {
                            candidate_count: candidates.len(),
                            estimated_savings: savings,
                            protected_start_index,
                            decision: crate::prune::PruneDecision::Fired {
                                pruned_count: pruned,
                            },
                        }
                    } else {
                        crate::prune::PruneEvaluation {
                            candidate_count: candidates.len(),
                            estimated_savings: savings,
                            protected_start_index,
                            decision: crate::prune::PruneDecision::SkippedBelowMinSavings,
                        }
                    }
                };
                if let Some(observer) = &self.prune_observer {
                    observer(&evaluation);
                }
            }

            // Interceptor: pre_llm_request
            match self.interceptor.pre_llm_request(&mut request_context).await {
                PreRequestAction::Cancel(reason) => {
                    info!(reason = %reason, "Aborted by interceptor");
                    for cb in &self.turn_end_cbs {
                        cb(current_turn);
                    }
                    return Err(EngineError::Aborted(reason));
                }
                PreRequestAction::YieldWith(items) => {
                    self.append_history_items(history, items.clone(), annotate)?;
                    request_context.extend(items);
                    info!("Yielded by interceptor after pre-request history append");
                    for cb in &self.turn_end_cbs {
                        cb(current_turn);
                    }
                    return Ok(EngineResult::Yielded);
                }
                PreRequestAction::Yield => {
                    info!("Yielded by interceptor");
                    for cb in &self.turn_end_cbs {
                        cb(current_turn);
                    }
                    return Ok(EngineResult::Yielded);
                }
                PreRequestAction::ContinueWith(items) => {
                    self.append_history_items(history, items.clone(), annotate)?;
                    request_context.extend(items);
                }
                PreRequestAction::Continue => {}
            }

            // LlmCall boundary fires per LLM generation request — today
            // 1:1 with AgentTurn, but retry (`agen-stream-continuation`)
            // will multiply this within a single AgentTurn.
            let current_llm_call = self.llm_call_count;
            for cb in &self.llm_call_start_cbs {
                cb(current_llm_call);
            }

            // Stream LLM response
            self.emit_lifecycle_trace(
                current_turn,
                current_llm_call,
                "build_request_start",
                items_trace_payload(&request_context, tool_definitions.len(), None, false),
            );
            let request = self.build_request(&tool_definitions, &request_context);
            self.emit_lifecycle_trace(
                current_turn,
                current_llm_call,
                "build_request_done",
                self.request_trace_payload(&request),
            );
            let request = self.attach_transport_trace(request, current_turn, current_llm_call);
            let stream_outcome = self
                .stream_response(request, current_turn, current_llm_call)
                .await?;

            for cb in &self.llm_call_end_cbs {
                cb(current_llm_call);
            }
            self.llm_call_count += 1;

            if let StreamCompletion::Interrupted { reason } = stream_outcome {
                stream_continuations += 1;
                if stream_continuations > MAX_STREAM_CONTINUATIONS {
                    return Err(EngineError::Client(ClientError::Api {
                        status: None,
                        code: None,
                        message: format!("LLM stream interrupted too many times: {reason}"),
                        retry_after: None,
                    }));
                }

                self.timeline.abort_current_block();
                self.timeline.flush_usage();
                let reasoning_items = self.thinking_block_collector.take_collected();
                let text_blocks = self.text_block_collector.take_collected();
                // Do not recover tool calls from an interrupted stream. A completed
                // tool_use is executable only when the provider finishes the stream.
                let _dropped_tool_calls = self.tool_call_collector.take_collected();
                let assistant_items =
                    self.build_assistant_items(&reasoning_items, &text_blocks, &[]);
                if !assistant_items.is_empty() {
                    self.append_history_items(history, assistant_items, annotate)?;
                }
                self.emit_llm_continuation(
                    current_llm_call,
                    stream_continuations,
                    MAX_STREAM_CONTINUATIONS,
                    &reason,
                );
                continuing_stream = true;
                continue;
            }

            stream_continuations = 0;
            continuing_stream = false;

            for cb in &self.turn_end_cbs {
                cb(current_turn);
            }
            self.turn_count += 1;
            *self.active_run_turn_count.get_or_insert(0) += 1;

            // Collect and commit assistant items. Routed through
            // `append_history_items` so observers see each item as it lands.
            let reasoning_items = self.thinking_block_collector.take_collected();
            let text_blocks = self.text_block_collector.take_collected();
            let tool_calls = self.tool_call_collector.take_collected();
            let assistant_items =
                self.build_assistant_items(&reasoning_items, &text_blocks, &tool_calls);
            self.append_history_items(history, assistant_items, annotate)?;

            if tool_calls.is_empty() {
                let turn_end_context = history.items_cloned();
                match self.interceptor.on_turn_end(&turn_end_context).await {
                    TurnEndAction::Finish => {
                        return Ok(EngineResult::Finished);
                    }
                    TurnEndAction::ContinueWithMessages(additional) => {
                        self.append_history_items(history, additional, annotate)?;
                        continue;
                    }
                    TurnEndAction::Pause => {
                        return Ok(EngineResult::Paused);
                    }
                }
            }

            if let Some(result) = self
                .execute_and_commit_tools(history, annotate, tool_calls)
                .await?
            {
                return Ok(result);
            }
        }
    }

    async fn open_stream_with_retry(
        &mut self,
        request: Request,
        turn: usize,
        llm_call: usize,
    ) -> Result<ResponseStream, EngineError> {
        let policy = self.retry_policy.clone();
        let started = Instant::now();
        let mut failed_attempt: u32 = 0;

        loop {
            let attempt = failed_attempt + 1;
            self.emit_lifecycle_trace(
                turn,
                llm_call,
                "stream_open_start",
                json!({
                    "attempt": attempt,
                    "request": self.request_trace_payload(&request),
                }),
            );
            let stream_started = Instant::now();
            let stream_result = tokio::select! {
                stream_result = self.client.stream(request.clone()) => stream_result,
                cancel = self.cancel_rx.recv() => {
                    if cancel.is_some() {
                        info!("Cancelled before stream started");
                    }
                    self.emit_lifecycle_trace(
                        turn,
                        llm_call,
                        "stream_open_cancelled",
                        json!({
                            "attempt": attempt,
                            "elapsed_ms": stream_started.elapsed().as_millis() as u64,
                        }),
                    );
                    self.timeline.abort_current_block();
                    return Err(EngineError::Cancelled);
                }
            };

            let err = match stream_result {
                Ok(stream) => {
                    self.emit_lifecycle_trace(
                        turn,
                        llm_call,
                        "stream_open_success",
                        json!({
                            "attempt": attempt,
                            "elapsed_ms": stream_started.elapsed().as_millis() as u64,
                        }),
                    );
                    let first_event_result = tokio::select! {
                        first_event = wait_for_first_stream_event(stream, DEFAULT_FIRST_STREAM_EVENT_TIMEOUT) => first_event,
                        cancel = self.cancel_rx.recv() => {
                            if cancel.is_some() {
                                info!("Cancelled before first stream event");
                            }
                            self.emit_lifecycle_trace(
                                turn,
                                llm_call,
                                "stream_first_event_cancelled",
                                json!({
                                    "attempt": attempt,
                                    "elapsed_ms": stream_started.elapsed().as_millis() as u64,
                                }),
                            );
                            self.timeline.abort_current_block();
                            return Err(EngineError::Cancelled);
                        }
                    };
                    match first_event_result {
                        Ok(FirstStreamEvent::Ready(stream)) => return Ok(stream),
                        Ok(FirstStreamEvent::Empty(stream)) => return Ok(stream),
                        Err(err) => {
                            self.emit_lifecycle_trace(
                                turn,
                                llm_call,
                                "stream_first_event_error",
                                json!({
                                    "attempt": attempt,
                                    "elapsed_ms": stream_started.elapsed().as_millis() as u64,
                                    "retryable": is_retryable(&err),
                                    "error": err.to_string(),
                                }),
                            );
                            err
                        }
                    }
                }
                Err(err) => {
                    self.emit_lifecycle_trace(
                        turn,
                        llm_call,
                        "stream_open_error",
                        json!({
                            "attempt": attempt,
                            "elapsed_ms": stream_started.elapsed().as_millis() as u64,
                            "retryable": is_retryable(&err),
                            "status": err.status(),
                            "error": err.to_string(),
                        }),
                    );
                    err
                }
            };

            let next_failed_attempt = failed_attempt + 1;
            if next_failed_attempt >= policy.max_attempts || !is_retryable(&err) {
                return Err(EngineError::Client(err));
            }

            let wait = err
                .retry_after()
                .unwrap_or_else(|| policy.backoff(failed_attempt));
            let elapsed = started.elapsed();
            if elapsed + wait > policy.total_timeout {
                return Err(EngineError::Client(err));
            }

            warn!(
                error = %err,
                failed_attempt = next_failed_attempt,
                wait_ms = wait.as_millis() as u64,
                "transient LLM request error, retrying"
            );
            let notice = LlmRetryNotice {
                failed_attempt: next_failed_attempt,
                max_attempts: policy.max_attempts,
                wait,
                elapsed,
                status: err.status(),
                error: err.to_string(),
            };
            for cb in &self.llm_retry_cbs {
                cb(llm_call, &notice);
            }

            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                cancel = self.cancel_rx.recv() => {
                    if cancel.is_some() {
                        info!("Cancelled during LLM retry backoff");
                    }
                    self.timeline.abort_current_block();
                    return Err(EngineError::Cancelled);
                }
            }

            failed_attempt = next_failed_attempt;
        }
    }

    /// Open a stream, dispatch all events to the timeline, handle cancellation.
    async fn stream_response(
        &mut self,
        request: Request,
        turn: usize,
        llm_call: usize,
    ) -> Result<StreamCompletion, EngineError> {
        debug!(
            item_count = request.items.len(),
            tool_count = request.tools.len(),
            has_system = request.system_prompt.is_some(),
            "Sending request to LLM"
        );

        let mut stream = self.open_stream_with_retry(request, turn, llm_call).await?;

        let mut event_count: usize = 0;
        loop {
            tokio::select! {
                event_result = stream.next() => {
                    match event_result {
                        Some(result) => {
                            match &result {
                                Ok(event) => {
                                    trace!(event = ?event, "Received event");
                                    event_count += 1;
                                }
                                Err(e) => {
                                    warn!(error = %e, "Stream error");
                                }
                            }
                            let event = match result {
                                Ok(event) => event,
                                Err(err) => {
                                    // 部分情報でも発火しておく（料金会計用）
                                    self.timeline.flush_usage();
                                    return Ok(StreamCompletion::Interrupted {
                                        reason: err.to_string(),
                                    });
                                }
                            };
                            if event_count == 1 {
                                self.emit_lifecycle_trace(
                                    turn,
                                    llm_call,
                                    "stream_first_event",
                                    json!({}),
                                );
                            }
                            self.emit_stream_event(turn, llm_call, &event);
                            self.timeline.dispatch(&event);
                            if let Event::Error(err) = &event {
                                self.timeline.abort_current_block();
                                self.timeline.flush_usage();
                                return Err(EngineError::Client(ClientError::Api {
                                    status: None,
                                    code: err.code.clone(),
                                    message: err.message.clone(),
                                    retry_after: None,
                                }));
                            }
                        }
                        None => break,
                    }
                }
                cancel = self.cancel_rx.recv() => {
                    if cancel.is_some() {
                        info!("Stream cancelled");
                    }
                    self.timeline.abort_current_block();
                    self.timeline.flush_usage();
                    return Err(EngineError::Cancelled);
                }
            }
        }
        // ストリーム完了時に集約済み Usage を 1 度だけ発火
        self.timeline.flush_usage();
        debug!(event_count = event_count, "Stream completed");
        Ok(StreamCompletion::Complete)
    }

    /// Execute tools and push results to history.
    /// Returns `Some(result)` if execution should stop (Paused),
    /// `None` if the turn loop should continue.
    async fn execute_and_commit_tools(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<Option<EngineResult>, EngineError> {
        match self.execute_tools(history, annotate, tool_calls).await {
            Ok(ToolExecutionResult::Paused) => Ok(Some(EngineResult::Paused)),
            Ok(ToolExecutionResult::Completed) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

impl<C: LlmClient, A> Engine<C, Mutable, A> {
    /// Create a new annotated Engine (in Mutable state).
    pub fn new_annotated(client: C) -> Self {
        let text_block_collector = TextBlockCollector::new();
        let tool_call_collector = ToolCallCollector::new();
        let thinking_block_collector = ThinkingBlockCollector::new();
        let mut timeline = Timeline::new();
        let (cancel_tx, cancel_rx) = mpsc::channel(1);

        // Register collectors with Timeline
        timeline.on_text_block(text_block_collector.clone());
        timeline.on_tool_use_block(tool_call_collector.clone());
        timeline.on_thinking_block(thinking_block_collector.clone());

        Self {
            client,
            retry_policy: RetryPolicy::default(),
            timeline,
            text_block_collector,
            tool_call_collector,
            thinking_block_collector,
            tool_server: ToolServer::new().handle(),
            interceptor: Box::new(DefaultInterceptor),
            system_prompt: None,
            locked_prefix_len: 0,
            turn_count: 0,
            active_run_turn_count: None,
            llm_call_count: 0,
            tool_execution_batch_count: 0,
            max_turns: None,
            turn_start_cbs: Vec::new(),
            turn_end_cbs: Vec::new(),
            llm_call_start_cbs: Vec::new(),
            llm_call_end_cbs: Vec::new(),
            llm_retry_cbs: Vec::new(),
            llm_continuation_cbs: Vec::new(),
            stream_event_cbs: Vec::new(),
            lifecycle_trace_cbs: Vec::new(),
            warning_cbs: Vec::new(),
            tool_result_cbs: Vec::new(),
            history_append_cbs: Vec::new(),
            request_config: RequestConfig::default(),
            cancel_tx,
            cancel_rx,
            tool_output_limits: None,
            prune_config: None,
            token_estimator: None,
            savings_estimator: None,
            prune_observer: None,
            cache_anchor: None,
            cache_key: None,
            _state: PhantomData,
        }
    }

    /// Register a tool factory for deferred initialization.
    ///
    /// The factory is queued and executed at the next `run()` or `resume()` call.
    /// Duplicate name detection occurs when pending tools are flushed before that call.
    pub fn register_tool(&mut self, factory: EngineToolDefinition) {
        self.tool_server.register_tool(factory);
    }

    /// Register multiple tool factories for deferred initialization.
    pub fn register_tools(&mut self, factories: impl IntoIterator<Item = EngineToolDefinition>) {
        self.tool_server.register_tools(factories);
    }

    /// Set system prompt (builder pattern)
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set system prompt (mutable reference version)
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    /// Install byte-size caps for tool execution `content`.
    ///
    /// Passing `None` (the default) disables truncation. Callers translate
    /// their own configuration into a concrete [`ToolOutputLimits`] and
    /// install it here.
    pub fn set_tool_output_limits(&mut self, limits: Option<ToolOutputLimits>) {
        self.tool_output_limits = limits;
    }

    /// Set maximum tokens (builder pattern)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let engine = Engine::new(client)
    ///     .system_prompt("You are a helpful assistant.")
    ///     .max_tokens(4096);
    /// ```
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.request_config.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature (builder pattern)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let engine = Engine::new(client)
    ///     .temperature(0.7);
    /// ```
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.request_config.temperature = Some(temperature);
        self
    }

    /// Set top_p (builder pattern)
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.request_config.top_p = Some(top_p);
        self
    }

    /// Set top_k (builder pattern)
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.request_config.top_k = Some(top_k);
        self
    }

    /// Add stop sequence (builder pattern)
    pub fn stop_sequence(mut self, sequence: impl Into<String>) -> Self {
        self.request_config.stop_sequences.push(sequence.into());
        self
    }

    /// Set request configuration at once (builder pattern)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let config = RequestConfig::new()
    ///     .with_max_tokens(4096)
    ///     .with_temperature(0.7);
    ///
    /// let engine = Engine::new(client)
    ///     .system_prompt("...")
    ///     .with_config(config);
    /// ```
    pub fn with_config(mut self, config: RequestConfig) -> Self {
        self.request_config = config;
        self
    }

    /// Set the retry policy used when opening an LLM response stream.
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Validate current configuration against the provider
    ///
    /// Returns an error if there are unsupported settings.
    /// Call at the end of the chain to detect configuration issues early.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let engine = Engine::new(client)
    ///     .temperature(0.7)
    ///     .top_k(40)
    ///     .validate()?;  // Error if using OpenAI since top_k is not supported
    /// ```
    ///
    /// # Returns
    /// * `Ok(Self)` - Validation successful
    /// * `Err(EngineError::ConfigWarnings)` - Has unsupported settings
    pub fn validate(self) -> Result<Self, EngineError> {
        let warnings = self.client.validate_config(&self.request_config);
        if warnings.is_empty() {
            Ok(self)
        } else {
            Err(EngineError::ConfigWarnings(warnings))
        }
    }

    /// Replace caller-owned history during restore/rebuild without emitting append callbacks.
    ///
    /// This is not a history-growth API. Live append paths must use
    /// [`append_history_with`](Self::append_history_with) so observers and the
    /// trusted annotation callback see every inserted item.
    pub fn replace_history_entries(
        &mut self,
        history: &mut History<A>,
        entries: Vec<HistoryEntry<A>>,
    ) -> Vec<HistoryEntry<A>> {
        history.replace_entries(entries)
    }

    /// Append items to caller-owned history after every observer and the trusted
    /// annotation callback accepts the item.
    pub fn append_history_with(
        &mut self,
        history: &mut History<A>,
        items: impl IntoIterator<Item = Item>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<(), EngineError> {
        self.append_history_items(history, items, annotate)
    }

    /// Truncate caller-owned history without emitting append callbacks.
    pub fn truncate_history(&mut self, history: &mut History<A>, len: usize) {
        history.truncate(len);
    }

    /// Clear caller-owned history.
    pub fn clear_history(&mut self, history: &mut History<A>) {
        history.clear();
    }

    /// Set the turn count (for session restoration)
    pub fn set_turn_count(&mut self, count: usize) {
        self.turn_count = count;
    }

    /// Set the maximum number of turns. None means unlimited.
    pub fn set_max_turns(&mut self, max_turns: Option<u32>) {
        self.max_turns = max_turns;
    }

    /// Apply configuration (reserved for future extensions)
    #[allow(dead_code)]
    pub fn config(self, _config: EngineConfig) -> Self {
        self
    }

    /// Run the engine with one user input, appending to caller-owned history.
    ///
    /// The trusted `annotate` callback is invoked after append observers and before
    /// each new item becomes live in `history`. Providers, token counters, pruners,
    /// and interceptors receive only the `Item` projection.
    pub async fn run_with_annotation(
        self,
        history: &mut History<A>,
        user_input: impl Into<String>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> EngineRunOutput<C, A> {
        let mut locked = self.lock(history);
        let result = locked
            .run_with_annotation(history, user_input, annotate)
            .await;
        EngineRunOutput {
            engine: locked,
            result,
        }
    }

    /// Resume from Paused, consuming self and transitioning to Locked.
    ///
    /// Used after `unlock()` → edit → resume.
    pub async fn resume_with_annotation(
        self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> EngineRunOutput<C, A> {
        let mut locked = self.lock(history);
        let result = locked.resume_with_annotation(history, annotate).await;
        EngineRunOutput {
            engine: locked,
            result,
        }
    }

    /// Lock and transition to Locked state
    ///
    /// Flushes pending tool factories, then fixes the current system prompt
    /// and history as a "committed prefix". After this, only `run()` / `resume()`
    /// may append to history, ensuring cache hits.
    ///
    /// Most callers should use [`run()`](Self::run) instead, which calls
    /// this internally. Use `lock()` directly only when you need the
    /// `Locked` engine back on error (e.g. in a persistence layer).
    ///
    /// # Panics
    ///
    /// Panics if a pending tool factory produces a duplicate name.
    pub fn lock(self, history: &History<A>) -> Engine<C, Locked, A> {
        self.tool_server.flush_pending();
        let locked_prefix_len = history.len();
        Engine {
            client: self.client,
            retry_policy: self.retry_policy,
            timeline: self.timeline,
            text_block_collector: self.text_block_collector,
            tool_call_collector: self.tool_call_collector,
            thinking_block_collector: self.thinking_block_collector,
            tool_server: self.tool_server,
            interceptor: self.interceptor,
            system_prompt: self.system_prompt,
            locked_prefix_len,
            turn_count: self.turn_count,
            active_run_turn_count: self.active_run_turn_count,
            llm_call_count: self.llm_call_count,
            tool_execution_batch_count: self.tool_execution_batch_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            llm_call_start_cbs: self.llm_call_start_cbs,
            llm_call_end_cbs: self.llm_call_end_cbs,
            llm_retry_cbs: self.llm_retry_cbs,
            llm_continuation_cbs: self.llm_continuation_cbs,
            stream_event_cbs: self.stream_event_cbs,
            lifecycle_trace_cbs: self.lifecycle_trace_cbs,
            warning_cbs: self.warning_cbs,
            tool_result_cbs: self.tool_result_cbs,
            history_append_cbs: self.history_append_cbs,
            request_config: self.request_config,

            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            tool_output_limits: self.tool_output_limits,
            prune_config: self.prune_config,
            token_estimator: self.token_estimator,
            savings_estimator: self.savings_estimator,
            prune_observer: self.prune_observer,
            cache_anchor: self.cache_anchor,
            cache_key: self.cache_key,
            _state: PhantomData,
        }
    }
}

fn unit_history_annotation(_: &Item) -> Result<(), String> {
    Ok(())
}

impl<C: LlmClient> Engine<C, Mutable, ()> {
    /// Create a new Engine (in Mutable state) using unit history annotations.
    pub fn new(client: C) -> Self {
        Self::new_annotated(client)
    }

    /// Append unit-annotated items to caller-owned history.
    pub fn append_history(
        &mut self,
        history: &mut History<()>,
        items: impl IntoIterator<Item = Item>,
    ) -> Result<(), EngineError> {
        let mut annotate = unit_history_annotation;
        self.append_history_items(history, items, &mut annotate)
    }

    /// Replace unit-annotated history from plain items.
    pub fn set_history(&mut self, history: &mut History<()>, items: Vec<Item>) {
        history.replace_items(items);
    }

    /// Run using unit annotations.
    pub async fn run(
        self,
        history: &mut History<()>,
        user_input: impl Into<String>,
    ) -> EngineRunOutput<C> {
        let mut annotate = unit_history_annotation;
        self.run_with_annotation(history, user_input, &mut annotate)
            .await
    }

    /// Resume using unit annotations.
    pub async fn resume(self, history: &mut History<()>) -> EngineRunOutput<C> {
        let mut annotate = unit_history_annotation;
        self.resume_with_annotation(history, &mut annotate).await
    }
}

impl<C: LlmClient, A> Engine<C, Locked, A> {
    /// Execute a turn
    ///
    /// Adds a new user message to history and sends a request to the LLM.
    /// Automatically loops if there are tool calls.
    pub async fn run_with_annotation(
        &mut self,
        history: &mut History<A>,
        user_input: impl Into<String>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> EngineRunExit {
        self.run_result_with_annotation(history, user_input.into(), annotate)
            .await
            .into()
    }

    async fn run_result_with_annotation(
        &mut self,
        history: &mut History<A>,
        user_input: String,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<EngineResult, EngineError> {
        // Supplying new user input abandons any paused/yielded logical run.
        self.active_run_turn_count = None;
        let mut user_item = Item::user_message(user_input);
        let extras = match self.interceptor.on_prompt_submit(&mut user_item).await {
            PromptAction::Cancel(reason) => {
                return self
                    .finalize_interruption(Err(EngineError::Aborted(reason)))
                    .await;
            }
            PromptAction::Continue => Vec::new(),
            PromptAction::ContinueWith(items) => items,
        };
        self.append_history_items(history, std::iter::once(user_item), annotate)?;
        if !extras.is_empty() {
            self.append_history_items(history, extras, annotate)?;
        }
        self.start_logical_run();
        let result = self.run_turn_loop(history, annotate).await;
        let result = self.finalize_interruption(result).await;
        self.finish_logical_run(&result);
        result
    }

    /// Resume execution (from Paused state).
    pub async fn resume_with_annotation(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> EngineRunExit {
        self.resume_result_with_annotation(history, annotate)
            .await
            .into()
    }

    async fn resume_result_with_annotation(
        &mut self,
        history: &mut History<A>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<EngineResult, EngineError> {
        self.ensure_logical_run();
        let result = self.run_turn_loop(history, annotate).await;
        let result = self.finalize_interruption(result).await;
        self.finish_logical_run(&result);
        result
    }

    /// Get the prefix length at lock time
    pub fn locked_prefix_len(&self) -> usize {
        self.locked_prefix_len
    }

    /// Unlock and return to Mutable state
    ///
    /// Note: After this operation, subsequent requests may not hit the cache.
    /// Use only when you need to edit history.
    pub fn unlock(self) -> Engine<C, Mutable, A> {
        Engine {
            client: self.client,
            retry_policy: self.retry_policy,
            timeline: self.timeline,
            text_block_collector: self.text_block_collector,
            tool_call_collector: self.tool_call_collector,
            thinking_block_collector: self.thinking_block_collector,
            tool_server: self.tool_server,
            interceptor: self.interceptor,
            system_prompt: self.system_prompt,
            locked_prefix_len: 0,
            turn_count: self.turn_count,
            active_run_turn_count: self.active_run_turn_count,
            llm_call_count: self.llm_call_count,
            tool_execution_batch_count: self.tool_execution_batch_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            llm_call_start_cbs: self.llm_call_start_cbs,
            llm_call_end_cbs: self.llm_call_end_cbs,
            llm_retry_cbs: self.llm_retry_cbs,
            llm_continuation_cbs: self.llm_continuation_cbs,
            stream_event_cbs: self.stream_event_cbs,
            lifecycle_trace_cbs: self.lifecycle_trace_cbs,
            warning_cbs: self.warning_cbs,
            tool_result_cbs: self.tool_result_cbs,
            history_append_cbs: self.history_append_cbs,
            request_config: self.request_config,

            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            tool_output_limits: self.tool_output_limits,
            prune_config: self.prune_config,
            token_estimator: self.token_estimator,
            savings_estimator: self.savings_estimator,
            prune_observer: self.prune_observer,
            cache_anchor: self.cache_anchor,
            cache_key: self.cache_key,
            _state: PhantomData,
        }
    }
}

impl<C: LlmClient> Engine<C, Locked, ()> {
    /// Run another turn using unit annotations.
    pub async fn run(
        &mut self,
        history: &mut History<()>,
        user_input: impl Into<String>,
    ) -> EngineRunExit {
        let mut annotate = unit_history_annotation;
        self.run_with_annotation(history, user_input, &mut annotate)
            .await
    }

    /// Resume using unit annotations.
    pub async fn resume(&mut self, history: &mut History<()>) -> EngineRunExit {
        let mut annotate = unit_history_annotation;
        self.resume_with_annotation(history, &mut annotate).await
    }
}

enum FirstStreamEvent {
    Ready(ResponseStream),
    Empty(ResponseStream),
}

async fn wait_for_first_stream_event(
    mut stream: ResponseStream,
    timeout: std::time::Duration,
) -> Result<FirstStreamEvent, ClientError> {
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(first)) => {
            let first = first?;
            let stream = futures::stream::once(async move { Ok(first) }).chain(stream);
            Ok(FirstStreamEvent::Ready(Box::pin(stream)))
        }
        Ok(None) => Ok(FirstStreamEvent::Empty(stream)),
        Err(_) => Err(ClientError::Timeout {
            phase: "stream_first_event",
            timeout,
        }),
    }
}

fn items_trace_payload(
    items: &[Item],
    tools_len: usize,
    cache_anchor: Option<usize>,
    cache_key_present: bool,
) -> Value {
    let last = items.last();
    let last_tool_result = match last {
        Some(Item::ToolResult {
            call_id,
            summary,
            content,
            is_error,
            ..
        }) => {
            let tool_name = items.iter().rev().find_map(|item| match item {
                Item::ToolCall {
                    call_id: candidate,
                    name,
                    ..
                } if candidate == call_id => Some(name.as_str()),
                _ => None,
            });
            Some(json!({
                "call_id": call_id,
                "tool_name": tool_name,
                "summary": summary,
                "summary_bytes": summary.len(),
                "content_bytes": content.as_ref().map(|s| s.len()).unwrap_or(0),
                "is_error": is_error,
            }))
        }
        _ => None,
    };

    let mut reasoning_items = 0usize;
    let mut reasoning_encrypted_content_count = 0usize;
    let mut reasoning_encrypted_content_bytes = 0usize;
    for item in items {
        if let Item::Reasoning {
            encrypted_content, ..
        } = item
        {
            reasoning_items += 1;
            if let Some(encrypted) = encrypted_content {
                reasoning_encrypted_content_count += 1;
                reasoning_encrypted_content_bytes += encrypted.len();
            }
        }
    }

    json!({
        "items_len": items.len(),
        "items_json_bytes": serde_json::to_vec(items).map(|bytes| bytes.len()).ok(),
        "tools_len": tools_len,
        "cache_anchor": cache_anchor,
        "cache_key_present": cache_key_present,
        "reasoning_items": reasoning_items,
        "reasoning_encrypted_content_count": reasoning_encrypted_content_count,
        "reasoning_encrypted_content_bytes": reasoning_encrypted_content_bytes,
        "last_item_kind": last.map(item_kind),
        "last_item_json_bytes": last.and_then(|item| serde_json::to_vec(item).ok().map(|bytes| bytes.len())),
        "last_tool_result": last_tool_result,
    })
}

fn item_kind(item: &Item) -> &'static str {
    match item {
        Item::Message { .. } => "message",
        Item::ToolCall { .. } => "tool_call",
        Item::ToolResult { .. } => "tool_result",
        Item::Reasoning { .. } => "reasoning",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Attachment, ImageAttachment};
    use std::time::Duration;

    #[test]
    fn provider_projection_reorders_results_and_remaps_cache_anchor() {
        let items = vec![
            Item::tool_call_json("call_slow", "slow", serde_json::json!({})),
            Item::tool_call_json("call_fast", "fast", serde_json::json!({})),
            Item::tool_result_item("call_fast", "fast result", None, false),
            Item::tool_result_item("call_slow", "slow result", None, false),
        ];

        let projection = materialize_provider_history(&items);
        let result_order: Vec<_> = projection
            .items
            .iter()
            .filter_map(|item| match item {
                Item::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_order, ["call_slow", "call_fast"]);
        assert_eq!(projection.original_to_projected_index, [0, 1, 3, 2]);
    }

    #[test]
    fn tool_attachment_round_trips_through_durable_history_json() {
        let body: Arc<[u8]> = Arc::from(&b"image-body"[..]);
        let items = vec![Item::tool_result_item_with_attachments(
            "call_image",
            "attached",
            None,
            false,
            vec![Attachment::Image(ImageAttachment::new(
                "image/png",
                body.clone(),
            ))],
        )];

        let persisted = serde_json::to_string(&items).unwrap();
        assert!(persisted.contains("aW1hZ2UtYm9keQ=="));
        let restored: Vec<Item> = serde_json::from_str(&persisted).unwrap();
        assert_eq!(restored, items);
        assert!(matches!(
            &restored[0],
            Item::ToolResult { attachments, .. }
                if matches!(
                    attachments.as_slice(),
                    [Attachment::Image(image)]
                        if image.mime_type() == "image/png" && image.data() == body.as_ref()
                )
        ));
    }

    #[tokio::test]
    async fn first_stream_event_timeout_returns_retryable_timeout() {
        let stream: ResponseStream = Box::pin(futures::stream::pending());
        let err = match wait_for_first_stream_event(stream, Duration::from_millis(5)).await {
            Ok(_) => panic!("expected first event timeout"),
            Err(err) => err,
        };

        assert!(is_retryable(&err));
        assert!(matches!(
            err,
            ClientError::Timeout {
                phase: "stream_first_event",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn first_stream_event_is_replayed_after_probe() {
        let first = Event::Status(crate::llm_client::event::StatusEvent {
            status: crate::llm_client::event::ResponseStatus::Started,
        });
        let stream: ResponseStream = Box::pin(futures::stream::once({
            let first = first.clone();
            async move { Ok(first) }
        }));

        let FirstStreamEvent::Ready(mut stream) =
            wait_for_first_stream_event(stream, Duration::from_secs(1))
                .await
                .unwrap()
        else {
            panic!("expected first event to be buffered");
        };

        let replayed = stream.next().await.unwrap().unwrap();
        assert_eq!(replayed, first);
    }
}
