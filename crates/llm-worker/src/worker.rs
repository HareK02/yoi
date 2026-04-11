use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::{
    Item,
    hook::{
        Hook, HookError, HookRegistry, OnAbort, OnPromptSubmit, OnPromptSubmitResult,
        OnStreamChunk, OnStreamComplete, OnTextDelta, OnToolCallDelta, OnTurnEnd, OnTurnEndResult,
        PostToolCall, PostToolCallContext, PostToolCallResult, PreLlmRequest, PreLlmRequestResult,
        PreToolCall, PreToolCallResult, StreamChunkContext, StreamCompleteContext,
        StreamHookResult, TextDeltaContext, ToolCall, ToolCallContext, ToolCallDeltaContext,
        ToolResult,
    },
    llm_client::{ClientError, ConfigWarning, LlmClient, Request, RequestConfig, ToolDefinition},
    state::{CacheLocked, Mutable, WorkerState},
    callback::{
        ClosureMetaHandler, ClosureTextBlockHandler, ClosureToolUseBlockHandler, TextBlockScope,
        ToolUseBlockScope,
    },
    handler::{ErrorKind, StatusKind, ToolUseBlockStart, UsageKind},
    timeline::{TextBlockCollector, Timeline, ToolCallCollector},
    timeline::event::{ErrorEvent, StatusEvent, UsageEvent},
    tool::{ToolDefinition as WorkerToolDefinition, ToolError, ToolOutputProcessor},
    tool_server::{ToolServer, ToolServerError, ToolServerHandle},
};

// =============================================================================
// Worker Error
// =============================================================================

/// Worker errors
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// Client error
    #[error("Client error: {0}")]
    Client(#[from] ClientError),
    /// Tool error
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    /// Hook error
    #[error("Hook error: {0}")]
    Hook(#[from] HookError),
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

// =============================================================================
// Worker Config
// =============================================================================

/// Worker configuration
#[derive(Debug, Clone, Default)]
pub struct WorkerConfig {
    // Reserved for future extensions (currently empty)
    _private: (),
}

// =============================================================================
// Worker Result Types
// =============================================================================

/// Worker execution result (status)
#[derive(Debug)]
pub enum WorkerResult {
    /// Completed (waiting for user input)
    Finished,
    /// Paused (can be resumed)
    Paused,
    /// Turn limit reached (max_turns exceeded)
    LimitReached,
}

/// Internal: tool execution result
enum ToolExecutionResult {
    Completed(Vec<ToolResult>),
    Paused,
}

// =============================================================================
// Worker
// =============================================================================

/// Central component for managing LLM interactions
///
/// Receives input from the user, sends requests to the LLM, and
/// automatically executes tool calls if any, advancing the turn.
///
/// # State Transitions (Type-state)
///
/// - [`Mutable`]: Initial state. System prompt and history can be freely edited.
/// - [`CacheLocked`]: Cache-protected state. Transition via `lock()`. Prefix context is immutable.
///
/// # Examples
///
/// ```ignore
/// use llm_worker::{Worker, Item};
///
/// // Create a Worker and register tools
/// let mut worker = Worker::new(client)
///     .system_prompt("You are a helpful assistant.");
/// worker.register_tool(my_tool);
///
/// // Run the interaction
/// let history = worker.run("Hello!").await?;
/// ```
///
/// # When Cache Protection is Needed
///
/// ```ignore
/// let mut worker = Worker::new(client)
///     .system_prompt("...");
///
/// // After setting history, lock to protect cache
/// let mut locked = worker.lock();
/// locked.run("user input").await?;
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
    /// Tool server handle
    tool_server: ToolServerHandle,
    /// Hook registry
    hooks: HookRegistry,
    /// System prompt
    system_prompt: Option<String>,
    /// Item history (owned by Worker)
    history: Vec<Item>,
    /// History length at lock time (only meaningful in CacheLocked state)
    locked_prefix_len: usize,
    /// Turn count
    turn_count: usize,
    /// Maximum number of turns (None = unlimited)
    max_turns: Option<u32>,
    /// Turn-start callbacks
    turn_start_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// Turn-end callbacks
    turn_end_cbs: Vec<Box<dyn Fn(usize) + Send + Sync>>,
    /// Request configuration (max_tokens, temperature, etc.)
    request_config: RequestConfig,
    /// Whether the previous run was interrupted
    last_run_interrupted: bool,
    /// Optional processor for large tool outputs (stores externally, returns summary)
    output_processor: Option<Arc<dyn ToolOutputProcessor>>,
    /// Cancel notification channel (for interrupting execution)
    cancel_tx: mpsc::Sender<()>,
    cancel_rx: mpsc::Receiver<()>,
    /// State marker
    _state: PhantomData<S>,
}

// =============================================================================
// Common Implementation (available in all states)
// =============================================================================

impl<C: LlmClient, S: WorkerState> Worker<C, S> {
    fn reset_interruption_state(&mut self) {
        self.last_run_interrupted = false;
    }

    /// Execute a turn
    ///
    /// Adds a new user message to history and sends a request to the LLM.
    /// Automatically loops if there are tool calls.
    pub async fn run(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<WorkerResult, WorkerError> {
        self.reset_interruption_state();
        // Hook: on_prompt_submit
        let mut user_item = Item::user_message(user_input);
        let result = self.run_on_prompt_submit_hooks(&mut user_item).await;
        let result = match result {
            Ok(value) => value,
            Err(err) => return self.finalize_interruption(Err(err)).await,
        };
        match result {
            OnPromptSubmitResult::Cancel(reason) => {
                self.last_run_interrupted = true;
                return self
                    .finalize_interruption(Err(WorkerError::Aborted(reason)))
                    .await;
            }
            OnPromptSubmitResult::Continue => {}
        }
        self.history.push(user_item);
        let result = self.run_turn_loop().await;
        self.finalize_interruption(result).await
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
        self.timeline
            .on_text_block(ClosureTextBlockHandler {
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
        self.timeline
            .on_tool_use_block(ClosureToolUseBlockHandler {
                setup: Box::new(setup),
            });
    }

    /// Register a usage event callback.
    pub fn on_usage(
        &mut self,
        callback: impl FnMut(&UsageEvent) + Send + Sync + 'static,
    ) {
        self.timeline.on_usage(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<UsageKind>,
        });
    }

    /// Register a status event callback.
    pub fn on_status(
        &mut self,
        callback: impl FnMut(&StatusEvent) + Send + Sync + 'static,
    ) {
        self.timeline.on_status(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<StatusKind>,
        });
    }

    /// Register an error event callback.
    pub fn on_error(
        &mut self,
        callback: impl FnMut(&ErrorEvent) + Send + Sync + 'static,
    ) {
        self.timeline.on_error(ClosureMetaHandler {
            callback,
            _kind: PhantomData::<ErrorKind>,
        });
    }

    /// Register a turn-start callback (receives 0-based turn number).
    pub fn on_turn_start(
        &mut self,
        callback: impl Fn(usize) + Send + Sync + 'static,
    ) {
        self.turn_start_cbs.push(Box::new(callback));
    }

    /// Register a turn-end callback (receives 0-based turn number).
    pub fn on_turn_end(
        &mut self,
        callback: impl Fn(usize) + Send + Sync + 'static,
    ) {
        self.turn_end_cbs.push(Box::new(callback));
    }

    /// Get a shared tool server handle.
    pub fn tool_server_handle(&self) -> ToolServerHandle {
        self.tool_server.clone()
    }

    /// Add an on_prompt_submit Hook
    ///
    /// Called immediately after receiving a user message in `run()`.
    pub fn add_on_prompt_submit_hook(&mut self, hook: impl Hook<OnPromptSubmit> + 'static) {
        self.hooks.on_prompt_submit.push(Box::new(hook));
    }

    /// Add a pre_llm_request Hook
    ///
    /// Called before sending an LLM request for each turn.
    pub fn add_pre_llm_request_hook(&mut self, hook: impl Hook<PreLlmRequest> + 'static) {
        self.hooks.pre_llm_request.push(Box::new(hook));
    }

    /// Add a pre_tool_call Hook
    pub fn add_pre_tool_call_hook(&mut self, hook: impl Hook<PreToolCall> + 'static) {
        self.hooks.pre_tool_call.push(Box::new(hook));
    }

    /// Add a post_tool_call Hook
    pub fn add_post_tool_call_hook(&mut self, hook: impl Hook<PostToolCall> + 'static) {
        self.hooks.post_tool_call.push(Box::new(hook));
    }

    /// Add an on_turn_end Hook
    pub fn add_on_turn_end_hook(&mut self, hook: impl Hook<OnTurnEnd> + 'static) {
        self.hooks.on_turn_end.push(Box::new(hook));
    }

    /// Add an on_abort Hook
    pub fn add_on_abort_hook(&mut self, hook: impl Hook<OnAbort> + 'static) {
        self.hooks.on_abort.push(Box::new(hook));
    }

    /// Add an on_text_delta Hook
    pub fn add_on_text_delta_hook(&mut self, hook: impl Hook<OnTextDelta> + 'static) {
        self.hooks.on_text_delta.push(Box::new(hook));
    }

    /// Add an on_tool_call_delta Hook
    pub fn add_on_tool_call_delta_hook(&mut self, hook: impl Hook<OnToolCallDelta> + 'static) {
        self.hooks.on_tool_call_delta.push(Box::new(hook));
    }

    /// Add an on_stream_chunk Hook
    pub fn add_on_stream_chunk_hook(&mut self, hook: impl Hook<OnStreamChunk> + 'static) {
        self.hooks.on_stream_chunk.push(Box::new(hook));
    }

    /// Add an on_stream_complete Hook
    pub fn add_on_stream_complete_hook(&mut self, hook: impl Hook<OnStreamComplete> + 'static) {
        self.hooks.on_stream_complete.push(Box::new(hook));
    }

    /// Get a mutable reference to the timeline (for additional handler registration)
    pub fn timeline_mut(&mut self) -> &mut Timeline {
        &mut self.timeline
    }

    /// Get a reference to the history
    pub fn history(&self) -> &[Item] {
        &self.history
    }

    /// Get a reference to the system prompt
    pub fn get_system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Get the current turn count
    pub fn turn_count(&self) -> usize {
        self.turn_count
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

    /// Build assistant response items from text blocks and tool calls
    fn build_assistant_items(&self, text_blocks: &[String], tool_calls: &[ToolCall]) -> Vec<Item> {
        let mut items = Vec::new();

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

        request
    }

    /// Hooks: on_prompt_submit
    ///
    /// Called immediately after receiving a user message in `run()` (first time only).
    async fn run_on_prompt_submit_hooks(
        &self,
        item: &mut Item,
    ) -> Result<OnPromptSubmitResult, WorkerError> {
        for hook in &self.hooks.on_prompt_submit {
            let result = hook.call(item).await?;
            match result {
                OnPromptSubmitResult::Continue => continue,
                OnPromptSubmitResult::Cancel(reason) => {
                    return Ok(OnPromptSubmitResult::Cancel(reason));
                }
            }
        }
        Ok(OnPromptSubmitResult::Continue)
    }

    /// Hooks: pre_llm_request
    ///
    /// Called before sending an LLM request for each turn.
    async fn run_pre_llm_request_hooks(
        &self,
    ) -> Result<(PreLlmRequestResult, Vec<Item>), WorkerError> {
        let mut temp_context = self.history.clone();
        for hook in &self.hooks.pre_llm_request {
            let result = hook.call(&mut temp_context).await?;
            match result {
                PreLlmRequestResult::Continue => continue,
                PreLlmRequestResult::Cancel(reason) => {
                    return Ok((PreLlmRequestResult::Cancel(reason), temp_context));
                }
            }
        }
        Ok((PreLlmRequestResult::Continue, temp_context))
    }

    /// Hooks: on_turn_end
    async fn run_on_turn_end_hooks(&self) -> Result<OnTurnEndResult, WorkerError> {
        let mut temp_items = self.history.clone();
        for hook in &self.hooks.on_turn_end {
            let result = hook.call(&mut temp_items).await?;
            match result {
                OnTurnEndResult::Finish => continue,
                OnTurnEndResult::ContinueWithMessages(items) => {
                    return Ok(OnTurnEndResult::ContinueWithMessages(items));
                }
                OnTurnEndResult::Paused => return Ok(OnTurnEndResult::Paused),
            }
        }
        Ok(OnTurnEndResult::Finish)
    }

    /// Hooks: on_abort
    async fn run_on_abort_hooks(&self, reason: &str) -> Result<(), WorkerError> {
        let mut reason = reason.to_string();
        for hook in &self.hooks.on_abort {
            hook.call(&mut reason).await?;
        }
        Ok(())
    }

    fn apply_stream_hook_result(result: StreamHookResult) -> Result<(), WorkerError> {
        match result {
            StreamHookResult::Continue => Ok(()),
            StreamHookResult::Abort(reason) => Err(WorkerError::Aborted(reason)),
            StreamHookResult::Pause => {
                Err(WorkerError::Aborted("Paused by stream hook".to_string()))
            }
        }
    }

    async fn run_on_stream_chunk_hooks(
        &self,
        event: crate::event::Event,
    ) -> Result<(), WorkerError> {
        let mut context = StreamChunkContext { event };
        for hook in &self.hooks.on_stream_chunk {
            let result = hook.call(&mut context).await?;
            Self::apply_stream_hook_result(result)?;
        }
        Ok(())
    }

    async fn run_on_text_delta_hooks(
        &self,
        index: usize,
        delta: String,
    ) -> Result<(), WorkerError> {
        let mut context = TextDeltaContext { index, delta };
        for hook in &self.hooks.on_text_delta {
            let result = hook.call(&mut context).await?;
            Self::apply_stream_hook_result(result)?;
        }
        Ok(())
    }

    async fn run_on_tool_call_delta_hooks(
        &self,
        index: usize,
        delta_json_fragment: String,
    ) -> Result<(), WorkerError> {
        let mut context = ToolCallDeltaContext {
            index,
            delta_json_fragment,
        };
        for hook in &self.hooks.on_tool_call_delta {
            let result = hook.call(&mut context).await?;
            Self::apply_stream_hook_result(result)?;
        }
        Ok(())
    }

    async fn run_on_stream_complete_hooks(
        &self,
        turn: usize,
        event_count: usize,
    ) -> Result<(), WorkerError> {
        let mut context = StreamCompleteContext { turn, event_count };
        for hook in &self.hooks.on_stream_complete {
            let result = hook.call(&mut context).await?;
            Self::apply_stream_hook_result(result)?;
        }
        Ok(())
    }

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
                if let Err(hook_err) = self.run_on_abort_hooks(&reason).await {
                    self.last_run_interrupted = true;
                    return Err(hook_err);
                }
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
                    let input = serde_json::from_str(arguments)
                        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
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

        // Phase 1: Apply pre_tool_call hooks (determine skip/abort)
        let mut approved_calls = Vec::new();
        for mut tool_call in tool_calls {
            // Get tool definition
            if let Some((meta, tool)) = self.tool_server.get_tool(&tool_call.name) {
                // Create context
                let mut context = ToolCallContext {
                    call: tool_call.clone(),
                    meta,
                    tool,
                };

                let mut skip = false;
                for hook in &self.hooks.pre_tool_call {
                    let result = hook
                        .call(&mut context)
                        .await
                        .inspect_err(|_| self.last_run_interrupted = true)?;
                    match result {
                        PreToolCallResult::Continue => {}
                        PreToolCallResult::Skip => {
                            skip = true;
                            break;
                        }
                        PreToolCallResult::Abort(reason) => {
                            self.last_run_interrupted = true;
                            return Err(WorkerError::Aborted(reason));
                        }
                        PreToolCallResult::Pause => {
                            self.last_run_interrupted = true;
                            return Ok(ToolExecutionResult::Paused);
                        }
                    }
                }

                // Reflect changes made by hooks
                tool_call = context.call;

                // Save to map (only if executing)
                if !skip {
                    call_info_map.insert(
                        tool_call.id.clone(),
                        (
                            tool_call.clone(),
                            context.meta.clone(),
                            context.tool.clone(),
                        ),
                    );
                    approved_calls.push(tool_call);
                }
            } else {
                // Unknown tools go into approved list as-is (will error at execution)
                // Hooks are not applied (no Meta available)
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
                        Ok(content) => ToolResult::success(&tool_call.id, content),
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

        // Phase 2.5: Apply output processor (store large results externally)
        if let Some(ref processor) = self.output_processor {
            for tool_result in &mut results {
                if !tool_result.is_error {
                    match processor.process(tool_result.content.clone()).await {
                        Ok(processed) => tool_result.content = processed,
                        Err(e) => {
                            warn!(error = %e, "Output processor failed, keeping original content");
                        }
                    }
                }
            }
        }

        // Phase 3: Apply post_tool_call hooks
        for tool_result in &mut results {
            // Get saved information
            if let Some((tool_call, meta, tool)) = call_info_map.get(&tool_result.tool_use_id) {
                let mut context = PostToolCallContext {
                    call: tool_call.clone(),
                    result: tool_result.clone(),
                    meta: meta.clone(),
                    tool: tool.clone(),
                };

                for hook in &self.hooks.post_tool_call {
                    let result = hook
                        .call(&mut context)
                        .await
                        .inspect_err(|_| self.last_run_interrupted = true)?;
                    match result {
                        PostToolCallResult::Continue => {}
                        PostToolCallResult::Abort(reason) => {
                            self.last_run_interrupted = true;
                            return Err(WorkerError::Aborted(reason));
                        }
                    }
                }
                // Reflect hook-modified results
                *tool_result = context.result;
            }
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

        // Resume check: Pending tool calls
        if let Some(tool_calls) = self.get_pending_tool_calls() {
            info!("Resuming pending tool calls");
            match self.execute_tools(tool_calls).await {
                Ok(ToolExecutionResult::Paused) => {
                    self.last_run_interrupted = true;
                    return Ok(WorkerResult::Paused);
                }
                Ok(ToolExecutionResult::Completed(results)) => {
                    for result in results {
                        self.history.push(Item::tool_result(
                            &result.tool_use_id,
                            &result.content,
                        ));
                    }
                    // Continue to loop
                }
                Err(err) => {
                    self.last_run_interrupted = true;
                    return Err(err);
                }
            }
        }

        loop {
            // Check for cancellation
            if self.try_cancelled() {
                info!("Execution cancelled");
                self.timeline.abort_current_block();
                self.last_run_interrupted = true;
                return Err(WorkerError::Cancelled);
            }

            // Notify turn start
            let current_turn = self.turn_count;
            debug!(turn = current_turn, "Turn start");
            for cb in &self.turn_start_cbs {
                cb(current_turn);
            }

            // Hook: pre_llm_request
            let (control, request_context) = self
                .run_pre_llm_request_hooks()
                .await
                .inspect_err(|_| self.last_run_interrupted = true)?;
            match control {
                PreLlmRequestResult::Cancel(reason) => {
                    info!(reason = %reason, "Aborted by hook");
                    for cb in &self.turn_end_cbs {
                        cb(current_turn);
                    }
                    self.last_run_interrupted = true;
                    return Err(WorkerError::Aborted(reason));
                }
                PreLlmRequestResult::Continue => {}
            }

            // Build request
            let request = self.build_request(&tool_definitions, &request_context);
            debug!(
                item_count = request.items.len(),
                tool_count = request.tools.len(),
                has_system = request.system_prompt.is_some(),
                "Sending request to LLM"
            );

            // Stream processing
            debug!("Starting stream...");
            let mut event_count = 0;

            // Get stream (cancellable)
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

            loop {
                tokio::select! {
                    // Receive event from stream
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
                                    .inspect_err(|_| self.last_run_interrupted = true)?;
                                self.timeline.dispatch(&event);

                                self.run_on_stream_chunk_hooks(event.clone())
                                    .await
                                    .inspect_err(|_| self.last_run_interrupted = true)?;

                                if let crate::llm_client::event::Event::BlockDelta(delta) = &event {
                                    match &delta.delta {
                                        crate::llm_client::event::DeltaContent::Text(text) => {
                                            self.run_on_text_delta_hooks(delta.index, text.clone())
                                                .await
                                                .inspect_err(|_| self.last_run_interrupted = true)?;
                                        }
                                        crate::llm_client::event::DeltaContent::InputJson(json_fragment) => {
                                            self.run_on_tool_call_delta_hooks(delta.index, json_fragment.clone())
                                                .await
                                                .inspect_err(|_| self.last_run_interrupted = true)?;
                                        }
                                        crate::llm_client::event::DeltaContent::Thinking(_) => {}
                                    }
                                }
                            }
                            None => break, // Stream ended
                        }
                    }
                    // Wait for cancellation
                    cancel = self.cancel_rx.recv() => {
                        if cancel.is_some() {
                            info!("Stream cancelled");
                        }
                        self.timeline.abort_current_block();
                        self.last_run_interrupted = true;
                        return Err(WorkerError::Cancelled);
                    }
                }
            }
            self.run_on_stream_complete_hooks(current_turn, event_count)
                .await
                .inspect_err(|_| self.last_run_interrupted = true)?;
            debug!(event_count = event_count, "Stream completed");

            // Notify turn end
            for cb in &self.turn_end_cbs {
                cb(current_turn);
            }
            self.turn_count += 1;

            // Get collected results
            let text_blocks = self.text_block_collector.take_collected();
            let tool_calls = self.tool_call_collector.take_collected();

            // Add assistant items to history
            let assistant_items = self.build_assistant_items(&text_blocks, &tool_calls);
            self.history.extend(assistant_items);

            if tool_calls.is_empty() {
                // No tool calls → determine turn end
                let turn_result = self
                    .run_on_turn_end_hooks()
                    .await
                    .inspect_err(|_| self.last_run_interrupted = true)?;
                match turn_result {
                    OnTurnEndResult::Finish => {
                        self.last_run_interrupted = false;
                        return Ok(WorkerResult::Finished);
                    }
                    OnTurnEndResult::ContinueWithMessages(additional) => {
                        self.history.extend(additional);
                        continue;
                    }
                    OnTurnEndResult::Paused => {
                        self.last_run_interrupted = true;
                        return Ok(WorkerResult::Paused);
                    }
                }
            }

            // Execute tools
            match self.execute_tools(tool_calls).await {
                Ok(ToolExecutionResult::Paused) => {
                    self.last_run_interrupted = true;
                    return Ok(WorkerResult::Paused);
                }
                Ok(ToolExecutionResult::Completed(results)) => {
                    for result in results {
                        self.history.push(Item::tool_result(
                            &result.tool_use_id,
                            &result.content,
                        ));
                    }
                }
                Err(err) => {
                    self.last_run_interrupted = true;
                    return Err(err);
                }
            }

            // Check turn limit (after assistant items and tool results are in history)
            if let Some(max) = self.max_turns {
                if self.turn_count >= max as usize {
                    info!(turn_count = self.turn_count, max_turns = max, "Turn limit reached");
                    self.last_run_interrupted = false;
                    return Ok(WorkerResult::LimitReached);
                }
            }
        }
    }

    /// Resume execution (from Paused state)
    ///
    /// Resumes turn processing from current state without adding a new user message to history.
    pub async fn resume(&mut self) -> Result<WorkerResult, WorkerError> {
        self.reset_interruption_state();
        let result = self.run_turn_loop().await;
        self.finalize_interruption(result).await
    }
}

// =============================================================================
// Mutable State-Specific Implementation
// =============================================================================

impl<C: LlmClient> Worker<C, Mutable> {
    /// Create a new Worker (in Mutable state)
    pub fn new(client: C) -> Self {
        let text_block_collector = TextBlockCollector::new();
        let tool_call_collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        let (cancel_tx, cancel_rx) = mpsc::channel(1);

        // Register collectors with Timeline
        timeline.on_text_block(text_block_collector.clone());
        timeline.on_tool_use_block(tool_call_collector.clone());

        Self {
            client,
            timeline,
            text_block_collector,
            tool_call_collector,
            tool_server: ToolServer::new().handle(),
            hooks: HookRegistry::new(),
            system_prompt: None,
            history: Vec::new(),
            locked_prefix_len: 0,
            turn_count: 0,
            max_turns: None,
            turn_start_cbs: Vec::new(),
            turn_end_cbs: Vec::new(),
            request_config: RequestConfig::default(),
            last_run_interrupted: false,
            output_processor: None,
            cancel_tx,
            cancel_rx,
            _state: PhantomData,
        }
    }

    /// Register a tool
    ///
    /// Registered tools are automatically executed when called by the LLM.
    /// Registering a tool with the same name will result in an error.
    ///
    /// Available only in Mutable state.
    pub fn register_tool(
        &mut self,
        factory: WorkerToolDefinition,
    ) -> Result<(), ToolRegistryError> {
        match self.tool_server.register_tool(factory) {
            Ok(()) => Ok(()),
            Err(ToolServerError::DuplicateName(name)) => {
                Err(ToolRegistryError::DuplicateName(name))
            }
            Err(ToolServerError::ToolNotFound(_) | ToolServerError::ToolExecution(_)) => {
                unreachable!("register_tool should only fail with DuplicateName")
            }
        }
    }

    /// Register multiple tools
    ///
    /// Available only in Mutable state.
    pub fn register_tools(
        &mut self,
        factories: impl IntoIterator<Item = WorkerToolDefinition>,
    ) -> Result<(), ToolRegistryError> {
        match self.tool_server.register_tools(factories) {
            Ok(()) => Ok(()),
            Err(ToolServerError::DuplicateName(name)) => {
                Err(ToolRegistryError::DuplicateName(name))
            }
            Err(ToolServerError::ToolNotFound(_) | ToolServerError::ToolExecution(_)) => {
                unreachable!("register_tools should only fail with DuplicateName")
            }
        }
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
    /// Available only in Mutable state. History can be freely edited.
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

    /// Set a tool output processor for handling large tool results.
    ///
    /// When set, tool execution results are passed through this processor
    /// before being placed into conversation history.
    pub fn set_output_processor(&mut self, processor: Arc<dyn ToolOutputProcessor>) {
        self.output_processor = Some(processor);
    }

    /// Apply configuration (reserved for future extensions)
    #[allow(dead_code)]
    pub fn config(self, _config: WorkerConfig) -> Self {
        self
    }

    /// Lock and transition to CacheLocked state
    ///
    /// This operation fixes the current system prompt and history as a "committed prefix".
    /// After this, only appending to history is allowed, ensuring cache hits.
    pub fn lock(self) -> Worker<C, CacheLocked> {
        let locked_prefix_len = self.history.len();
        Worker {
            client: self.client,
            timeline: self.timeline,
            text_block_collector: self.text_block_collector,
            tool_call_collector: self.tool_call_collector,
            tool_server: self.tool_server,
            hooks: self.hooks,
            system_prompt: self.system_prompt,
            history: self.history,
            locked_prefix_len,
            turn_count: self.turn_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            request_config: self.request_config,
            last_run_interrupted: self.last_run_interrupted,
            output_processor: self.output_processor,
            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            _state: PhantomData,
        }
    }
}

// =============================================================================
// CacheLocked State-Specific Implementation
// =============================================================================

impl<C: LlmClient> Worker<C, CacheLocked> {
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
            tool_server: self.tool_server,
            hooks: self.hooks,
            system_prompt: self.system_prompt,
            history: self.history,
            locked_prefix_len: 0,
            turn_count: self.turn_count,
            max_turns: self.max_turns,
            turn_start_cbs: self.turn_start_cbs,
            turn_end_cbs: self.turn_end_cbs,
            request_config: self.request_config,
            last_run_interrupted: self.last_run_interrupted,
            output_processor: self.output_processor,
            cancel_tx: self.cancel_tx,
            cancel_rx: self.cancel_rx,
            _state: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    // Basic tests only. Tests using LlmClient are done in integration tests.
}
