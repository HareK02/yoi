use std::collections::HashMap;
use std::marker::PhantomData;

use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::{
    Item,
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
        ClientError, ConfigWarning, LlmClient, Request, RequestConfig, ToolDefinition,
        types::parse_tool_arguments,
    },
    state::{Locked, Mutable, WorkerState},
    timeline::event::{ErrorEvent, StatusEvent, UsageEvent},
    timeline::{ReasoningItemCollector, TextBlockCollector, Timeline, ToolCallCollector},
    tool::{
        ToolCall, ToolDefinition as WorkerToolDefinition, ToolError, ToolOutputLimits, ToolResult,
        truncate_content,
    },
    tool_server::{ToolServer, ToolServerHandle},
};

/// Worker errors
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
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
}

/// Tool registration error
#[derive(Debug, thiserror::Error)]
pub enum ToolRegistryError {
    /// A tool with the same name is already registered
    #[error("Tool with name '{0}' already registered")]
    DuplicateName(String),
}

/// Worker configuration
#[derive(Debug, Clone, Default)]
pub struct WorkerConfig {
    // Reserved for future extensions (currently empty)
    _private: (),
}

/// Worker execution result (status)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerResult {
    /// Completed (waiting for user input)
    Finished,
    /// Paused (can be resumed)
    Paused,
    /// Turn limit reached (max_turns exceeded)
    LimitReached,
    /// Yielded to caller for external processing (e.g. context compaction).
    ///
    /// Distinct from `Paused`: internal machinery, not user-facing. The
    /// caller is expected to perform some side work and then call `resume()`
    /// to continue the turn loop.
    Yielded,
}

/// Result of [`Worker<C, Mutable>::run()`] / [`Worker<C, Mutable>::resume()`].
///
/// Contains the `Locked` Worker (ready for subsequent runs) and the outcome.
pub struct RunOutput<C: LlmClient> {
    /// The Worker, now in Locked state.
    pub worker: Worker<C, Locked>,
    /// Outcome of the turn.
    pub result: WorkerResult,
}

/// Internal: tool execution result
enum ToolExecutionResult {
    Completed(Vec<ToolResult>),
    Paused,
}

/// Central component for managing LLM interactions
///
/// Receives input from the user, sends requests to the LLM, and
/// automatically executes tool calls if any, advancing the turn.
///
/// # State Transitions (Type-state)
///
/// - [`Mutable`]: Initial state. System prompt, history, and tools can be freely edited.
/// - [`Locked`]: Cache-protected state. Prefix context is immutable; only `run()` / `resume()` are available.
///
/// Calling `run()` on a `Mutable` Worker consumes it and returns a
/// `Locked` Worker together with the result. This ensures the
/// cache prefix is fixed for optimal KV cache hit rate.
///
/// ```ignore
/// let mut worker = Worker::new(client)
///     .system_prompt("You are a helpful assistant.");
/// worker.register_tool(my_tool);
///
/// // Mutable::run() consumes self → RunOutput { worker: Locked, result }
/// let out = worker.run("Hello").await?;
/// let mut worker = out.worker;
///
/// // Locked::run() borrows &mut self
/// worker.run("Follow-up").await?;
///
/// // To edit between turns, unlock back to Mutable
/// let mut worker = worker.unlock();
/// worker.history_mut().truncate(5);
/// let out = worker.run("Continue").await?;
/// let mut worker = out.worker;
/// ```
pub struct Worker<C: LlmClient, S: WorkerState = Mutable> {
    /// LLM client
    client: C,
    /// Event timeline
    timeline: Timeline,
    /// Text block collector (Timeline handler)
    text_block_collector: TextBlockCollector,
    /// Tool call collector (Timeline handler)
    tool_call_collector: ToolCallCollector,
    /// Reasoning item collector (Timeline handler)。完成済み reasoning
    /// item を 1 ターン分バッファし、history に append する。
    reasoning_item_collector: ReasoningItemCollector,
    /// Tool server handle
    tool_server: ToolServerHandle,
    /// Interceptor for control-flow decisions
    interceptor: Box<dyn Interceptor>,
    /// System prompt
    system_prompt: Option<String>,
    /// Item history (owned by Worker)
    history: Vec<Item>,
    /// History length at lock time (only meaningful in Locked state)
    locked_prefix_len: usize,
    /// AgentTurn count.
    ///
    /// Once retry (`llm-worker-stream-continuation`) is implemented, an
    /// AgentTurn collapses N retried `LlmCall`s with identical input;
    /// today retry is not implemented so AgentTurn and LlmCall fire 1:1
    /// and the increment site (the LLM-call loop) is shared.
    /// `max_turns` is interpreted as a per-`run()` AgentTurn cap.
    turn_count: usize,
    /// LlmCall count (per-Worker running counter, monotonic). Unlike
    /// `turn_count` this never collapses retries.
    llm_call_count: usize,
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
    /// Non-fatal warning callbacks. Invoked when the Worker wants to
    /// surface an advisory message to the upper layer (e.g. Pod) so it
    /// can be forwarded to the user — distinct from `tracing::warn!`,
    /// which is for developer-facing logs.
    warning_cbs: Vec<Box<dyn Fn(&str) + Send + Sync>>,
    /// Tool-result callbacks. Invoked once per completed tool call
    /// after post-execution interceptors and the output byte-cap
    /// truncation have been applied — i.e. on the same data that
    /// enters history.
    tool_result_cbs: Vec<Box<dyn Fn(&ToolResult) + Send + Sync>>,
    /// History-append callbacks. Invoked for non-streamed items when they
    /// are appended to persistent worker history, so upper layers can
    /// broadcast those items using history itself as the source of truth.
    history_append_cbs: Vec<Box<dyn Fn(&Item) + Send + Sync>>,
    /// Request configuration (max_tokens, temperature, etc.)
    request_config: RequestConfig,
    /// Whether the previous run was interrupted
    last_run_interrupted: bool,
    /// Cancel notification channel (for interrupting execution)
    cancel_tx: mpsc::Sender<()>,
    cancel_rx: mpsc::Receiver<()>,
    /// Byte-size caps applied to tool `content` before it reaches history.
    /// `None` disables truncation (tests and minimal setups).
    tool_output_limits: Option<ToolOutputLimits>,
    /// Prune configuration. `None` disables the prune projection.
    prune_config: Option<crate::prune::PruneConfig>,
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
    /// [`Request::cache_key`] at request build time. Pod 側では
    /// `SessionId` を渡す。
    cache_key: Option<String>,
    /// State marker
    _state: PhantomData<S>,
}

impl<C: LlmClient, S: WorkerState> Worker<C, S> {
    fn reset_interruption_state(&mut self) {
        self.last_run_interrupted = false;
    }

    fn drain_cancel_queue(&mut self) {
        use tokio::sync::mpsc::error::TryRecvError;
        loop {
            match self.cancel_rx.try_recv() {
                Ok(()) => continue,
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
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
    /// worker.on_text_block(|block| {
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
    /// worker.on_tool_use_block(|start, block| {
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

    /// Register a non-fatal warning callback.
    ///
    /// The callback is invoked with a short human-readable message
    /// whenever the Worker encounters a condition that should be
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
    /// exactly what is persisted to history. Intended for upper layers
    /// (e.g. Pod) to forward tool results to clients.
    pub fn on_tool_result(&mut self, callback: impl Fn(&ToolResult) + Send + Sync + 'static) {
        self.tool_result_cbs.push(Box::new(callback));
    }

    fn emit_tool_result(&self, result: &ToolResult) {
        for cb in &self.tool_result_cbs {
            cb(result);
        }
    }

    /// Register a callback invoked for items appended directly to worker
    /// history outside streaming timeline callbacks.
    pub fn on_history_append(&mut self, callback: impl Fn(&Item) + Send + Sync + 'static) {
        self.history_append_cbs.push(Box::new(callback));
    }

    fn emit_history_append(&self, item: &Item) {
        for cb in &self.history_append_cbs {
            cb(item);
        }
    }

    fn extend_history_with_callbacks(&mut self, items: impl IntoIterator<Item = Item>) {
        for item in items {
            self.emit_history_append(&item);
            self.history.push(item);
        }
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

    /// Get a reference to the history
    pub fn history(&self) -> &[Item] {
        &self.history
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

    /// Get the current LlmCall count (per-Worker running counter, never
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
    /// worker.set_max_tokens(4096);
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
    /// worker.set_temperature(0.7);
    /// ```
    pub fn set_temperature(&mut self, temperature: f32) {
        self.request_config.temperature = Some(temperature);
    }

    /// Set top_p (nucleus sampling)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// worker.set_top_p(0.9);
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
    /// worker.set_top_k(40);
    /// ```
    pub fn set_top_k(&mut self, top_k: u32) {
        self.request_config.top_k = Some(top_k);
    }

    /// Add a stop sequence
    ///
    /// # Examples
    ///
    /// ```ignore
    /// worker.add_stop_sequence("\n\n");
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
    /// WorkerError::Cancelled is returned at the next event loop checkpoint.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// let worker = Arc::new(Mutex::new(Worker::new(client)));
    ///
    /// // Run in another thread
    /// let worker_clone = worker.clone();
    /// tokio::spawn(async move {
    ///     let mut w = worker_clone.lock().unwrap();
    ///     w.run("Long task...").await
    /// });
    ///
    /// // Cancel
    /// worker.lock().unwrap().cancel();
    /// ```
    pub fn cancel(&self) {
        let _ = self.cancel_tx.try_send(());
    }

    /// Check if cancelled
    pub fn is_cancelled(&mut self) -> bool {
        self.try_cancelled()
    }

    /// Whether the previous run was interrupted
    pub fn last_run_interrupted(&self) -> bool {
        self.last_run_interrupted
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
        reasoning_items: &[crate::llm_client::event::ReasoningItemEvent],
        text_blocks: &[String],
        tool_calls: &[ToolCall],
    ) -> Vec<Item> {
        let mut items = Vec::new();

        for r in reasoning_items {
            let mut item = Item::reasoning(r.text.clone());
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

        // Add items directly (Request now uses Items natively)
        request = request.items(context.iter().cloned());

        // Add tool definitions
        for tool_def in tool_definitions {
            request = request.tool(tool_def.clone());
        }

        // Apply request configuration
        request = request.config(self.request_config.clone());

        // Attach the cache prefix anchor (may be narrower than `context`
        // if the prune projection trimmed items from the head — keep it
        // in range).
        request.cache_anchor = self.cache_anchor.filter(|&anchor| anchor < context.len());
        request.cache_key = self.cache_key.clone();

        request
    }

    /// Hooks: on_prompt_submit
    ///
    async fn finalize_interruption<T>(
        &mut self,
        result: Result<T, WorkerError>,
    ) -> Result<T, WorkerError> {
        match result {
            Ok(value) => Ok(value),
            Err(err) => {
                self.last_run_interrupted = true;
                let reason = match &err {
                    WorkerError::Aborted(reason) => reason.clone(),
                    WorkerError::Cancelled => "Cancelled".to_string(),
                    _ => err.to_string(),
                };
                self.interceptor.on_abort(&reason).await;
                Err(err)
            }
        }
    }

    /// Check for pending tool calls (for resuming from Pause)
    fn get_pending_tool_calls(&self) -> Option<Vec<ToolCall>> {
        // Find the last ToolCall items that don't have corresponding ToolResult
        let mut pending_calls = Vec::new();
        let mut answered_call_ids = std::collections::HashSet::new();

        // First pass: collect all answered call IDs
        for item in &self.history {
            if let Item::ToolResult { call_id, .. } = item {
                answered_call_ids.insert(call_id.clone());
            }
        }

        // Second pass: find unanswered tool calls
        for item in &self.history {
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
        tool_calls: Vec<ToolCall>,
    ) -> Result<ToolExecutionResult, WorkerError> {
        use futures::future::join_all;

        // Map from tool call ID to (ToolCall, Meta, Tool)
        // Retained because it's needed for PostToolCall hooks
        let mut call_info_map = HashMap::new();
        let mut synthetic_results = Vec::new();

        // Phase 1: Apply pre_tool_call interceptor (determine skip/abort/synthetic result)
        let mut approved_calls = Vec::new();
        for mut tool_call in tool_calls {
            if let Some((meta, tool)) = self.tool_server.get_tool(&tool_call.name) {
                let mut info = ToolCallInfo {
                    call: tool_call.clone(),
                    meta,
                    tool,
                };

                match self.interceptor.pre_tool_call(&mut info).await {
                    PreToolAction::Continue => {}
                    PreToolAction::Skip => {
                        continue;
                    }
                    PreToolAction::SyntheticResult(result) => {
                        let tool_call = info.call;
                        call_info_map.insert(
                            tool_call.id.clone(),
                            (tool_call, info.meta.clone(), info.tool.clone()),
                        );
                        synthetic_results.push(result);
                        continue;
                    }
                    PreToolAction::Abort(reason) => {
                        self.last_run_interrupted = true;
                        return Err(WorkerError::Aborted(reason));
                    }
                    PreToolAction::Pause => {
                        self.last_run_interrupted = true;
                        return Ok(ToolExecutionResult::Paused);
                    }
                }

                // Reflect changes made by interceptor
                tool_call = info.call;

                call_info_map.insert(
                    tool_call.id.clone(),
                    (tool_call.clone(), info.meta.clone(), info.tool.clone()),
                );
                approved_calls.push(tool_call);
            } else {
                // Unknown tools go into approved list as-is (will error at execution)
                approved_calls.push(tool_call);
            }
        }

        // Phase 2: Execute approved tools in parallel (cancellable)
        let futures: Vec<_> = approved_calls
            .into_iter()
            .map(|tool_call| {
                let tool_server = self.tool_server.clone();
                async move {
                    let input_json = serde_json::to_string(&tool_call.input).unwrap_or_default();
                    match tool_server.call_tool(&tool_call.name, &input_json).await {
                        Ok(output) => ToolResult::from_output(&tool_call.id, output),
                        Err(e) => ToolResult::error(&tool_call.id, e.to_string()),
                    }
                }
            })
            .collect();

        // Make tool execution cancellable
        let mut results = tokio::select! {
            results = join_all(futures) => results,
            cancel = self.cancel_rx.recv() => {
                if cancel.is_some() {
                    info!("Tool execution cancelled");
                }
                self.timeline.abort_current_block();
                self.last_run_interrupted = true;
                return Err(WorkerError::Cancelled);
            }
        };
        results.extend(synthetic_results);

        // Phase 3: Apply post_tool_call interceptor
        for tool_result in &mut results {
            if let Some((tool_call, meta, tool)) = call_info_map.get(&tool_result.tool_use_id) {
                let mut info = ToolResultInfo {
                    call: tool_call.clone(),
                    result: tool_result.clone(),
                    meta: meta.clone(),
                    tool: tool.clone(),
                };

                match self.interceptor.post_tool_call(&mut info).await {
                    PostToolAction::Continue => {}
                    PostToolAction::Abort(reason) => {
                        self.last_run_interrupted = true;
                        return Err(WorkerError::Aborted(reason));
                    }
                }
                // Reflect interceptor-modified results
                *tool_result = info.result;
            }
        }

        // Phase 4: Cap `content` byte-size before it enters history.
        // Runs *after* post_tool_call so interceptors (audit, logging,
        // classification) still observe the full content, and any
        // content they inject is also truncated — closing the last gap
        // before the data reaches the next LLM request.
        if let Some(limits) = self.tool_output_limits.as_ref() {
            for tool_result in &mut results {
                let Some(content) = tool_result.content.as_mut() else {
                    continue;
                };
                let Some((tool_call, _, _)) = call_info_map.get(&tool_result.tool_use_id) else {
                    continue;
                };
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
        }

        // Emit per-result callbacks on the post-truncation payload.
        for tool_result in &results {
            self.emit_tool_result(tool_result);
        }

        Ok(ToolExecutionResult::Completed(results))
    }

    /// Internal turn execution logic
    async fn run_turn_loop(&mut self) -> Result<WorkerResult, WorkerError> {
        self.reset_interruption_state();
        self.drain_cancel_queue();
        let tool_definitions = self.build_tool_definitions();

        info!(
            item_count = self.history.len(),
            tool_count = tool_definitions.len(),
            "Starting worker run"
        );

        // Resume pending tool calls from a previous Pause
        if let Some(tool_calls) = self.get_pending_tool_calls() {
            info!("Resuming pending tool calls");
            if let Some(result) = self.execute_and_commit_tools(tool_calls).await? {
                return Ok(result);
            }
        }

        loop {
            if self.try_cancelled() {
                info!("Execution cancelled");
                self.timeline.abort_current_block();
                self.last_run_interrupted = true;
                return Err(WorkerError::Cancelled);
            }

            let current_turn = self.turn_count;
            debug!(turn = current_turn, "Turn start");
            for cb in &self.turn_start_cbs {
                cb(current_turn);
            }

            // Drain interceptor-side inputs that are meant to land in
            // history (notifications, cross-Pod events, system
            // reminders). These are committed *before* the per-request
            // clone so they participate in the LLM request below and
            // get persisted by the upper layer that owns history.json.
            let pending = self.interceptor.pending_history_appends().await;
            if !pending.is_empty() {
                self.extend_history_with_callbacks(pending);
            }

            // Clone the history into a per-request context. Everything
            // below (prune projection, interceptor hooks) mutates only
            // this clone, so the persistent `self.history` stays intact.
            let mut request_context = self.history.clone();

            // Prune projection: if both the config and the savings
            // estimator are configured, drop ToolResult.content from
            // prunable candidates whose estimated savings meet the
            // threshold. Worker does not own usage history itself; the
            // estimator is injected by the layer that does.
            if let (Some(config), Some(estimator)) = (&self.prune_config, &self.savings_estimator) {
                let (candidates, border_turn) =
                    crate::prune::evaluate_candidates(&request_context, config.protected_turns);
                let evaluation = if candidates.is_empty() {
                    crate::prune::PruneEvaluation {
                        candidate_count: 0,
                        estimated_savings: 0,
                        border_turn,
                        decision: crate::prune::PruneDecision::SkippedNoCandidates,
                    }
                } else {
                    let savings = estimator(&request_context, &candidates);
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
                            border_turn,
                            decision: crate::prune::PruneDecision::Fired {
                                pruned_count: pruned,
                            },
                        }
                    } else {
                        crate::prune::PruneEvaluation {
                            candidate_count: candidates.len(),
                            estimated_savings: savings,
                            border_turn,
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
                    self.last_run_interrupted = true;
                    return Err(WorkerError::Aborted(reason));
                }
                PreRequestAction::Yield => {
                    info!("Yielded by interceptor");
                    for cb in &self.turn_end_cbs {
                        cb(current_turn);
                    }
                    self.last_run_interrupted = true;
                    return Ok(WorkerResult::Yielded);
                }
                PreRequestAction::Continue => {}
            }

            // LlmCall boundary fires per LLM generation request — today
            // 1:1 with AgentTurn, but retry (`llm-worker-stream-continuation`)
            // will multiply this within a single AgentTurn.
            let current_llm_call = self.llm_call_count;
            for cb in &self.llm_call_start_cbs {
                cb(current_llm_call);
            }

            // Stream LLM response
            let request = self.build_request(&tool_definitions, &request_context);
            self.stream_response(request).await?;

            for cb in &self.llm_call_end_cbs {
                cb(current_llm_call);
            }
            self.llm_call_count += 1;

            for cb in &self.turn_end_cbs {
                cb(current_turn);
            }
            self.turn_count += 1;

            // Collect and commit assistant items. Routed through
            // `extend_history_with_callbacks` so observers (e.g. the
            // Pod-side per-item session-log committer) see each item
            // as it lands.
            let reasoning_items = self.reasoning_item_collector.take_collected();
            let text_blocks = self.text_block_collector.take_collected();
            let tool_calls = self.tool_call_collector.take_collected();
            let assistant_items =
                self.build_assistant_items(&reasoning_items, &text_blocks, &tool_calls);
            self.extend_history_with_callbacks(assistant_items);

            if tool_calls.is_empty() {
                match self.interceptor.on_turn_end(&self.history).await {
                    TurnEndAction::Finish => {
                        self.last_run_interrupted = false;
                        return Ok(WorkerResult::Finished);
                    }
                    TurnEndAction::ContinueWithMessages(additional) => {
                        self.extend_history_with_callbacks(additional);
                        continue;
                    }
                    TurnEndAction::Pause => {
                        self.last_run_interrupted = true;
                        return Ok(WorkerResult::Paused);
                    }
                }
            }

            if let Some(result) = self.execute_and_commit_tools(tool_calls).await? {
                return Ok(result);
            }

            if let Some(max) = self.max_turns {
                if self.turn_count >= max as usize {
                    info!(
                        turn_count = self.turn_count,
                        max_turns = max,
                        "Turn limit reached"
                    );
                    self.last_run_interrupted = false;
                    return Ok(WorkerResult::LimitReached);
                }
            }
        }
    }

    /// Open a stream, dispatch all events to the timeline, handle cancellation.
    async fn stream_response(&mut self, request: Request) -> Result<(), WorkerError> {
        debug!(
            item_count = request.items.len(),
            tool_count = request.tools.len(),
            has_system = request.system_prompt.is_some(),
            "Sending request to LLM"
        );

        let mut stream = tokio::select! {
            stream_result = self.client.stream(request) => stream_result
                .inspect_err(|_| self.last_run_interrupted = true)?,
            cancel = self.cancel_rx.recv() => {
                if cancel.is_some() {
                    info!("Cancelled before stream started");
                }
                self.timeline.abort_current_block();
                self.last_run_interrupted = true;
                return Err(WorkerError::Cancelled);
            }
        };

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
                            let event = result
                                .inspect_err(|_| {
                                    self.last_run_interrupted = true;
                                    // 部分情報でも発火しておく（料金会計用）
                                    self.timeline.flush_usage();
                                })?;
                            self.timeline.dispatch(&event);
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
                    self.last_run_interrupted = true;
                    return Err(WorkerError::Cancelled);
                }
            }
        }
        // ストリーム完了時に集約済み Usage を 1 度だけ発火
        self.timeline.flush_usage();
        debug!(event_count = event_count, "Stream completed");
        Ok(())
    }

    /// Execute tools and push results to history.
    /// Returns `Some(result)` if execution should stop (Paused),
    /// `None` if the turn loop should continue.
    async fn execute_and_commit_tools(
        &mut self,
        tool_calls: Vec<ToolCall>,
    ) -> Result<Option<WorkerResult>, WorkerError> {
        match self.execute_tools(tool_calls).await {
            Ok(ToolExecutionResult::Paused) => {
                self.last_run_interrupted = true;
                Ok(Some(WorkerResult::Paused))
            }
            Ok(ToolExecutionResult::Completed(results)) => {
                // Route per-result pushes through the callback path so
                // observers (e.g. the Pod-side per-item session-log
                // committer) see each tool result as it lands.
                let items = results.into_iter().map(|result| {
                    Item::tool_result_item(
                        &result.tool_use_id,
                        &result.summary,
                        result.content,
                        result.is_error,
                    )
                });
                self.extend_history_with_callbacks(items);
                Ok(None)
            }
            Err(err) => {
                self.last_run_interrupted = true;
                Err(err)
            }
        }
    }
}

impl<C: LlmClient> Worker<C, Mutable> {
    /// Create a new Worker (in Mutable state)
    pub fn new(client: C) -> Self {
        let text_block_collector = TextBlockCollector::new();
        let tool_call_collector = ToolCallCollector::new();
        let reasoning_item_collector = ReasoningItemCollector::new();
        let mut timeline = Timeline::new();
        let (cancel_tx, cancel_rx) = mpsc::channel(1);

        // Register collectors with Timeline
        timeline.on_text_block(text_block_collector.clone());
        timeline.on_tool_use_block(tool_call_collector.clone());
        timeline.on_reasoning_item(reasoning_item_collector.clone());

        Self {
            client,
            timeline,
            text_block_collector,
            tool_call_collector,
            reasoning_item_collector,
            tool_server: ToolServer::new().handle(),
            interceptor: Box::new(DefaultInterceptor),
            system_prompt: None,
            history: Vec::new(),
            locked_prefix_len: 0,
            turn_count: 0,
            llm_call_count: 0,
            max_turns: None,
            turn_start_cbs: Vec::new(),
            turn_end_cbs: Vec::new(),
            llm_call_start_cbs: Vec::new(),
            llm_call_end_cbs: Vec::new(),
            warning_cbs: Vec::new(),
            tool_result_cbs: Vec::new(),
            history_append_cbs: Vec::new(),
            request_config: RequestConfig::default(),
            last_run_interrupted: false,
            cancel_tx,
            cancel_rx,
            tool_output_limits: None,
            prune_config: None,
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
    /// Duplicate name detection occurs at that point and surfaces as
    /// [`WorkerError::ToolRegistry`].
    pub fn register_tool(&mut self, factory: WorkerToolDefinition) {
        self.tool_server.register_tool(factory);
    }

    /// Register multiple tool factories for deferred initialization.
    pub fn register_tools(&mut self, factories: impl IntoIterator<Item = WorkerToolDefinition>) {
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
    /// Passing `None` (the default) disables truncation. Higher layers
    /// (e.g. Pod) translate manifest configuration into a concrete
    /// [`ToolOutputLimits`] and install it here.
    pub fn set_tool_output_limits(&mut self, limits: Option<ToolOutputLimits>) {
        self.tool_output_limits = limits;
    }

    /// Set maximum tokens (builder pattern)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let worker = Worker::new(client)
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
    /// let worker = Worker::new(client)
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
    /// let worker = Worker::new(client)
    ///     .system_prompt("...")
    ///     .with_config(config);
    /// ```
    pub fn with_config(mut self, config: RequestConfig) -> Self {
        self.request_config = config;
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
    /// let worker = Worker::new(client)
    ///     .temperature(0.7)
    ///     .top_k(40)
    ///     .validate()?;  // Error if using OpenAI since top_k is not supported
    /// ```
    ///
    /// # Returns
    /// * `Ok(Self)` - Validation successful
    /// * `Err(WorkerError::ConfigWarnings)` - Has unsupported settings
    pub fn validate(self) -> Result<Self, WorkerError> {
        let warnings = self.client.validate_config(&self.request_config);
        if warnings.is_empty() {
            Ok(self)
        } else {
            Err(WorkerError::ConfigWarnings(warnings))
        }
    }

    /// Get a mutable reference to history
    ///
    /// Available only in Mutable state.
    pub fn history_mut(&mut self) -> &mut Vec<Item> {
        &mut self.history
    }

    /// Set history
    pub fn set_history(&mut self, items: Vec<Item>) {
        self.history = items;
    }

    /// Add an item to history (builder pattern)
    pub fn with_item(mut self, item: Item) -> Self {
        self.history.push(item);
        self
    }

    /// Add an item to history
    pub fn push_item(&mut self, item: Item) {
        self.history.push(item);
    }

    /// Add multiple items to history (builder pattern)
    pub fn with_items(mut self, items: impl IntoIterator<Item = Item>) -> Self {
        self.history.extend(items);
        self
    }

    /// Add multiple items to history
    pub fn extend_history(&mut self, items: impl IntoIterator<Item = Item>) {
        self.history.extend(items);
    }

    /// Clear history
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Set the turn count (for session restoration)
    pub fn set_turn_count(&mut self, count: usize) {
        self.turn_count = count;
    }

    /// Set the maximum number of turns. None means unlimited.
    pub fn set_max_turns(&mut self, max_turns: Option<u32>) {
        self.max_turns = max_turns;
    }

    /// Set the last_run_interrupted flag (for session restoration)
    pub fn set_last_run_interrupted(&mut self, interrupted: bool) {
        self.last_run_interrupted = interrupted;
    }

    /// Apply configuration (reserved for future extensions)
    #[allow(dead_code)]
    pub fn config(self, _config: WorkerConfig) -> Self {
        self
    }

    /// Execute a turn, consuming self and transitioning to Locked.
    ///
    /// This is the primary entry point for first use. Equivalent to
    /// `self.lock()` followed by `locked.run(user_input)`.
    ///
    /// Subsequent runs can use [`Worker<C, Locked>::run()`] directly.
    /// To edit state between turns, call [`unlock()`](Worker::unlock) first.
    pub async fn run(self, user_input: impl Into<String>) -> Result<RunOutput<C>, WorkerError> {
        let mut locked = self.lock();
        let result = locked.run(user_input).await?;
        Ok(RunOutput {
            worker: locked,
            result,
        })
    }

    /// Resume from Paused, consuming self and transitioning to Locked.
    ///
    /// Used after `unlock()` → edit → resume.
    pub async fn resume(self) -> Result<RunOutput<C>, WorkerError> {
        let mut locked = self.lock();
        let result = locked.resume().await?;
        Ok(RunOutput {
            worker: locked,
            result,
        })
    }

    /// Lock and transition to Locked state
    ///
    /// Flushes pending tool factories, then fixes the current system prompt
    /// and history as a "committed prefix". After this, only `run()` / `resume()`
    /// may append to history, ensuring cache hits.
    ///
    /// Most callers should use [`run()`](Self::run) instead, which calls
    /// this internally. Use `lock()` directly only when you need the
    /// `Locked` worker back on error (e.g. in a persistence layer).
    ///
    /// # Panics
    ///
    /// Panics if a pending tool factory produces a duplicate name.
    pub fn lock(self) -> Worker<C, Locked> {
        self.tool_server.flush_pending();
        let locked_prefix_len = self.history.len();
        Worker {
            client: self.client,
            timeline: self.timeline,
            text_block_collector: self.text_block_collector,
            tool_call_collector: self.tool_call_collector,
            reasoning_item_collector: self.reasoning_item_collector,
            tool_server: self.tool_server,
            interceptor: self.interceptor,
            system_prompt: self.system_prompt,
            history: self.history,
            locked_prefix_len,
            turn_count: self.turn_count,
            llm_call_count: self.llm_call_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            llm_call_start_cbs: self.llm_call_start_cbs,
            llm_call_end_cbs: self.llm_call_end_cbs,
            warning_cbs: self.warning_cbs,
            tool_result_cbs: self.tool_result_cbs,
            history_append_cbs: self.history_append_cbs,
            request_config: self.request_config,
            last_run_interrupted: self.last_run_interrupted,

            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            tool_output_limits: self.tool_output_limits,
            prune_config: self.prune_config,
            savings_estimator: self.savings_estimator,
            prune_observer: self.prune_observer,
            cache_anchor: self.cache_anchor,
            cache_key: self.cache_key,
            _state: PhantomData,
        }
    }
}

impl<C: LlmClient> Worker<C, Locked> {
    /// Execute a turn
    ///
    /// Adds a new user message to history and sends a request to the LLM.
    /// Automatically loops if there are tool calls.
    pub async fn run(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<WorkerResult, WorkerError> {
        self.reset_interruption_state();
        // Interceptor: on_prompt_submit
        let mut user_item = Item::user_message(user_input);
        let extras = match self.interceptor.on_prompt_submit(&mut user_item).await {
            PromptAction::Cancel(reason) => {
                self.last_run_interrupted = true;
                return self
                    .finalize_interruption(Err(WorkerError::Aborted(reason)))
                    .await;
            }
            PromptAction::Continue => Vec::new(),
            PromptAction::ContinueWith(items) => items,
        };
        self.history.push(user_item);
        if !extras.is_empty() {
            self.extend_history_with_callbacks(extras);
        }
        let result = self.run_turn_loop().await;
        self.finalize_interruption(result).await
    }

    /// Resume execution (from Paused state)
    ///
    /// Resumes turn processing from current state without adding a new user message.
    pub async fn resume(&mut self) -> Result<WorkerResult, WorkerError> {
        self.reset_interruption_state();
        let result = self.run_turn_loop().await;
        self.finalize_interruption(result).await
    }

    /// Get the prefix length at lock time
    pub fn locked_prefix_len(&self) -> usize {
        self.locked_prefix_len
    }

    /// Unlock and return to Mutable state
    ///
    /// Note: After this operation, subsequent requests may not hit the cache.
    /// Use only when you need to edit history.
    pub fn unlock(self) -> Worker<C, Mutable> {
        Worker {
            client: self.client,
            timeline: self.timeline,
            text_block_collector: self.text_block_collector,
            tool_call_collector: self.tool_call_collector,
            reasoning_item_collector: self.reasoning_item_collector,
            tool_server: self.tool_server,
            interceptor: self.interceptor,
            system_prompt: self.system_prompt,
            history: self.history,
            locked_prefix_len: 0,
            turn_count: self.turn_count,
            llm_call_count: self.llm_call_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            llm_call_start_cbs: self.llm_call_start_cbs,
            llm_call_end_cbs: self.llm_call_end_cbs,
            warning_cbs: self.warning_cbs,
            tool_result_cbs: self.tool_result_cbs,
            history_append_cbs: self.history_append_cbs,
            request_config: self.request_config,
            last_run_interrupted: self.last_run_interrupted,

            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            tool_output_limits: self.tool_output_limits,
            prune_config: self.prune_config,
            savings_estimator: self.savings_estimator,
            prune_observer: self.prune_observer,
            cache_anchor: self.cache_anchor,
            cache_key: self.cache_key,
            _state: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    // Basic tests only. Tests using LlmClient are done in integration tests.
}
