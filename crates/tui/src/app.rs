use std::time::Instant;

use protocol::{
    AlertLevel, AlertSource, CompletionEntry, CompletionKind, Event, Method, RunResult, Segment,
};

use crate::block::{
    Block, CompactEvent, ThinkingBlock, ThinkingState, ToolCallBlock, ToolCallState,
};
use crate::cache::FileCache;
use crate::input::InputBuffer;
use crate::scroll::Scroll;
use crate::ui::Mode;

/// In-flight completion popup state. Lives on `App` while the user is
/// typing inside a `@` / `#` / `/` token. Cleared whenever the trigger
/// is invalidated (cursor moved out, whitespace landed inside the
/// token, the sigil was deleted, or the candidate was confirmed).
pub struct CompletionState {
    pub kind: CompletionKind,
    /// Atom index of the leading sigil (`@` / `#` / `/`).
    pub prefix_start: usize,
    /// Text typed after the sigil (sigil itself excluded).
    pub prefix: String,
    /// Latest candidate set returned by the Pod for `(kind, prefix)`.
    /// Initially empty until `Event::Completions` lands.
    pub entries: Vec<CompletionEntry>,
    pub selected: usize,
}

impl CompletionState {
    pub fn is_active(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Maximum rows the popup ever renders. Caller can clip to fewer
    /// rows if vertical space is tight.
    pub const MAX_VISIBLE: usize = 6;
}

pub struct App {
    pub pod_name: String,
    pub connected: bool,
    pub running: bool,
    /// True while the Pod is in `PodStatus::Paused`. Set on
    /// `RunEnd::Paused` and cleared when a new turn starts (either via
    /// `Resume` or a fresh `Run`).
    pub paused: bool,
    pub run_requests: usize,
    /// Sum of `input_tokens - cache_read_input_tokens` across the
    /// current turn's LLM requests — i.e. the net tokens this turn
    /// actually had to upload at full price (cache writes included,
    /// cache reads excluded). Reset on `RunEnd`.
    pub run_upload_tokens: u64,
    pub run_output_tokens: u64,
    pub turn_index: usize,
    pub current_tool: Option<String>,
    pub input: InputBuffer,
    pub quit: bool,
    /// 2-tap guard for `Ctrl-C` when the Pod is not running. First press
    /// records the instant; a second press within the timeout exits the
    /// TUI (the Pod itself stays alive).
    pub quit_confirm: Option<std::time::Instant>,
    /// Full display history in render order.
    pub blocks: Vec<Block>,
    pub scroll: Scroll,
    pub mode: Mode,
    pub cache: FileCache,
    /// True when the latest AssistantText block is still being streamed
    /// and future text deltas should append to it instead of starting a
    /// fresh block.
    assistant_streaming: bool,
    /// Completion popup state, when an `@` / `#` / `/` token is in
    /// flight. `None` whenever the trigger conditions don't hold.
    pub completion: Option<CompletionState>,
}

impl App {
    pub fn new(pod_name: String) -> Self {
        Self {
            pod_name,
            connected: false,
            running: false,
            paused: false,
            run_requests: 0,
            run_upload_tokens: 0,
            run_output_tokens: 0,
            turn_index: 0,
            current_tool: None,
            input: InputBuffer::new(),
            quit: false,
            quit_confirm: None,
            blocks: Vec::new(),
            scroll: Scroll::default(),
            mode: Mode::Normal,
            cache: FileCache::new(),
            assistant_streaming: false,
            completion: None,
        }
    }

    /// Re-evaluate the completion popup against the current input.
    /// Returns a `Method::ListCompletions` to send when the
    /// `(kind, prefix_start, prefix)` triple changed; otherwise `None`.
    /// Callers should invoke this after every input mutation that could
    /// move the cursor or change atoms.
    pub fn refresh_completion(&mut self) -> Option<Method> {
        match self.input.pending_completion_prefix() {
            Some((kind, start, prefix)) => {
                let need_query = match &self.completion {
                    Some(c) => c.kind != kind || c.prefix_start != start || c.prefix != prefix,
                    None => true,
                };
                let entries = match self.completion.take() {
                    Some(c) if c.kind == kind && c.prefix_start == start => c.entries,
                    _ => Vec::new(),
                };
                self.completion = Some(CompletionState {
                    kind,
                    prefix_start: start,
                    prefix: prefix.clone(),
                    entries,
                    selected: 0,
                });
                if need_query {
                    Some(Method::ListCompletions { kind, prefix })
                } else {
                    None
                }
            }
            None => {
                self.completion = None;
                None
            }
        }
    }

    pub fn move_completion_up(&mut self) {
        if let Some(c) = self.completion.as_mut()
            && !c.entries.is_empty()
        {
            c.selected = if c.selected == 0 {
                c.entries.len() - 1
            } else {
                c.selected - 1
            };
        }
    }

    pub fn move_completion_down(&mut self) {
        if let Some(c) = self.completion.as_mut()
            && !c.entries.is_empty()
        {
            c.selected = (c.selected + 1) % c.entries.len();
        }
    }

    pub fn cancel_completion(&mut self) {
        self.completion = None;
    }

    /// Tab path: insert the popup-selected entry's value (with a
    /// trailing `/` when it's a directory) as raw text replacing the
    /// in-flight `@<typed>` portion. The popup state is preserved so
    /// the re-evaluated trigger can fetch fresh candidates for the new
    /// prefix (drill-in for directories, narrow-to-one for files).
    /// Returns the follow-up `Method::ListCompletions` to send when
    /// the new prefix differs from the old one.
    pub fn apply_completion_text(&mut self) -> Option<Method> {
        let state = self.completion.as_ref()?;
        if state.entries.is_empty() {
            return None;
        }
        let entry = &state.entries[state.selected];
        let text = if entry.is_dir {
            format!("{}/", entry.value)
        } else {
            entry.value.clone()
        };
        // `prefix_start` indexes the sigil atom; the text we want to
        // replace lives just after it (sigil itself stays).
        let typed_start = state.prefix_start + 1;
        self.input.replace_with_text_at(typed_start, &text);
        self.refresh_completion()
    }

    /// Space path: replace the `@<typed>` range with a chip atom and
    /// clear the popup if `prefix` (= the text the user has typed
    /// after the sigil) resolves to a confirmable target. Three
    /// matching modes:
    ///
    /// 1. **Direct value match**: some entry's `value` equals `prefix`
    ///    (covers files and slash-less directory form).
    /// 2. **Slashed directory match**: some directory entry's
    ///    `value + "/"` equals `prefix` (the form Tab inserts).
    /// 3. **Drilled-into-directory match**: `prefix` ends with `/`
    ///    and at least one entry lives under it.
    ///
    /// Directory chips always carry a trailing `/` so the rendered
    /// label reads `@crates/`.
    ///
    /// `selected` is intentionally ignored — terminating with a
    /// space is a typed-based "I'm done with this token" signal,
    /// so a race-y top entry shouldn't block confirmation when the
    /// typed text matches another entry.
    pub fn chipify_completion_if_exact_match(&mut self) -> bool {
        let Some(state) = self.completion.as_ref() else {
            return false;
        };
        let direct = state
            .entries
            .iter()
            .find(|e| {
                state.prefix == e.value || (e.is_dir && state.prefix == format!("{}/", e.value))
            })
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.value)
                } else {
                    e.value.clone()
                }
            });
        let drilled = (direct.is_none() && state.prefix.ends_with('/'))
            .then(|| {
                state
                    .entries
                    .iter()
                    .any(|e| e.value.starts_with(&state.prefix))
                    .then(|| state.prefix.clone())
            })
            .flatten();
        let Some(value) = direct.or(drilled) else {
            return false;
        };
        let kind = state.kind;
        let start = state.prefix_start;
        match kind {
            CompletionKind::File => self.input.replace_with_file_ref(start, value),
            CompletionKind::Knowledge => self.input.replace_with_knowledge_ref(start, value),
            CompletionKind::Workflow => self.input.replace_with_workflow_invoke(start, value),
        }
        self.completion = None;
        true
    }

    /// Enter path: commit the currently *selected* popup entry,
    /// regardless of how much of its value the user has typed. This
    /// is the popup-UI sense of "Enter accepts the highlighted
    /// suggestion" — partial typing like `@README.` followed by
    /// Enter should chip when the popup is on `README.md`.
    ///
    /// Files (and Knowledge / Workflow entries, which have no dir
    /// concept) chipify here. Directory file entries return `false`
    /// so the caller can fall through to `apply_completion_text`
    /// for drill-in — chip-ifying a directory on Enter would strand
    /// the user with no way to inspect children.
    pub fn chipify_selected_completion_if_committable(&mut self) -> bool {
        let Some(state) = self.completion.as_ref() else {
            return false;
        };
        if state.entries.is_empty() {
            return false;
        }
        let entry = &state.entries[state.selected];
        if state.kind == CompletionKind::File && entry.is_dir {
            return false;
        }
        let kind = state.kind;
        let start = state.prefix_start;
        let value = entry.value.clone();
        match kind {
            CompletionKind::File => self.input.replace_with_file_ref(start, value),
            CompletionKind::Knowledge => self.input.replace_with_knowledge_ref(start, value),
            CompletionKind::Workflow => self.input.replace_with_workflow_invoke(start, value),
        }
        self.completion = None;
        true
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            // Empty Enter only does something meaningful when the Pod
            // is paused: resume the interrupted turn. Otherwise no-op.
            if self.paused {
                self.input.clear();
                return Some(Method::Resume);
            }
            return None;
        }
        // TurnHeader / UserMessage blocks are pushed in response to
        // `Event::UserMessage` (single source of truth, shared by every
        // client subscribed to the Pod). Locally we only clear the
        // input buffer and forward the method.
        self.input.clear();
        Some(Method::Run { input: segments })
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Pod,
            message: message.into(),
        });
    }

    pub fn handle_pod_event(&mut self, event: Event) {
        match event {
            Event::UserMessage { segments } => {
                self.turn_index += 1;
                self.blocks.push(Block::TurnHeader {
                    turn: self.turn_index,
                });
                self.blocks.push(Block::UserMessage { segments });
                self.assistant_streaming = false;
            }
            Event::Notify { message } => {
                self.blocks.push(Block::Notify { message });
                self.assistant_streaming = false;
            }
            Event::PodEvent(event) => {
                self.blocks.push(Block::PodEvent { event });
                self.assistant_streaming = false;
            }
            Event::TurnStart { .. } => {
                self.running = true;
                self.paused = false;
                self.run_requests += 1;
                self.current_tool = None;
                self.assistant_streaming = false;
            }
            Event::TextDelta { text } => {
                self.append_assistant_text(&text);
            }
            Event::TextDone { .. } => {
                self.assistant_streaming = false;
            }
            Event::ThinkingStart => {
                self.assistant_streaming = false;
                self.blocks.push(Block::Thinking(ThinkingBlock {
                    text: String::new(),
                    state: ThinkingState::Streaming {
                        started_at: Instant::now(),
                    },
                }));
            }
            Event::ThinkingDelta { text } => {
                if let Some(b) = self.last_streaming_thinking_mut() {
                    b.text.push_str(&text);
                }
            }
            Event::ThinkingDone { text } => {
                if let Some(b) = self.last_streaming_thinking_mut() {
                    // Delta-accumulated text wins. `text` here is the
                    // Done payload (full body), used only as a fallback
                    // for providers that don't stream deltas.
                    if b.text.is_empty() {
                        b.text = text;
                    }
                    let elapsed = match &b.state {
                        ThinkingState::Streaming { started_at } => {
                            Some(started_at.elapsed().as_secs())
                        }
                        _ => None,
                    };
                    b.state = ThinkingState::Finished {
                        elapsed_secs: elapsed,
                    };
                }
            }
            Event::TurnEnd { .. } => {
                self.assistant_streaming = false;
                self.mark_orphan_tool_calls_incomplete();
                self.mark_orphan_thinking_incomplete();
                self.current_tool = None;
            }
            Event::ToolCallStart { id, name } => {
                self.current_tool = Some(name.clone());
                self.assistant_streaming = false;
                self.blocks.push(Block::ToolCall(ToolCallBlock {
                    id,
                    name,
                    args_stream: String::new(),
                    arguments: None,
                    state: ToolCallState::Pending,
                    edit_snapshot: None,
                }));
            }
            Event::ToolCallArgsDelta { id, json } => {
                if let Some(b) = self.find_tool_call_mut(&id) {
                    b.args_stream.push_str(&json);
                    if matches!(b.state, ToolCallState::Pending) {
                        b.state = ToolCallState::Streaming;
                    }
                }
            }
            Event::ToolCallDone { id, arguments, .. } => {
                self.current_tool = None;
                if let Some(b) = self.find_tool_call_mut(&id) {
                    b.arguments = Some(arguments);
                    // Only advance the state when it's still in-flight.
                    // If a ToolResult arrived out of order and already
                    // transitioned us to Done/Error, keep that.
                    if matches!(b.state, ToolCallState::Pending | ToolCallState::Streaming) {
                        b.state = ToolCallState::Executing;
                    }
                }
            }
            Event::ToolResult {
                id,
                summary,
                output,
                is_error,
            } => {
                // Pull the name / args out first so we can look at the
                // (immutable) cache before taking the mutable block
                // borrow below.
                let (name, args) = self
                    .find_tool_call_mut(&id)
                    .map(|b| (b.name.clone(), b.arguments.clone()))
                    .unwrap_or_default();
                let edit_snapshot = if !is_error && name == "Edit" {
                    args.as_deref()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
                        .and_then(|path| self.cache.get(&path).map(|s| s.to_owned()))
                } else {
                    None
                };

                if let Some(b) = self.find_tool_call_mut(&id) {
                    if edit_snapshot.is_some() {
                        b.edit_snapshot = edit_snapshot;
                    }
                    b.state = if is_error {
                        ToolCallState::Error {
                            summary,
                            output: output.clone(),
                        }
                    } else {
                        ToolCallState::Done {
                            summary,
                            output: output.clone(),
                        }
                    };
                    if !is_error {
                        apply_cache_update(
                            &mut self.cache,
                            &name,
                            args.as_deref(),
                            output.as_deref(),
                        );
                    }
                } else {
                    // Result for an unknown tool call. Surface it as an
                    // alert so it isn't silently dropped.
                    let level = if is_error {
                        AlertLevel::Error
                    } else {
                        AlertLevel::Warn
                    };
                    self.blocks.push(Block::Alert {
                        level,
                        source: AlertSource::Pod,
                        message: format!("orphan tool result ({id}): {summary}"),
                    });
                }
            }
            Event::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
            } => {
                // Subtract the cache-hit portion so a tool loop that
                // re-sends the same prefix on every request doesn't
                // re-count it. cache_creation stays in (it is full
                // price on this request).
                let net_input = input_tokens
                    .unwrap_or(0)
                    .saturating_sub(cache_read_input_tokens.unwrap_or(0));
                self.run_upload_tokens += net_input;
                self.run_output_tokens += output_tokens.unwrap_or(0);
            }
            Event::Error { code, message } => {
                self.push_error(format!("[{code:?}] {message}"));
            }
            Event::RunEnd { result } => {
                self.blocks.push(Block::TurnStats {
                    requests: self.run_requests,
                    upload_tokens: self.run_upload_tokens,
                    output_tokens: self.run_output_tokens,
                });
                self.running = false;
                self.paused = matches!(result, RunResult::Paused);
                self.run_requests = 0;
                self.run_upload_tokens = 0;
                self.run_output_tokens = 0;
                self.current_tool = None;
                self.assistant_streaming = false;
            }
            Event::CompactStart => {
                self.blocks.push(Block::Compact(CompactEvent::Start));
            }
            Event::CompactDone { new_session_id } => {
                self.blocks
                    .push(Block::Compact(CompactEvent::Done { new_session_id }));
            }
            Event::CompactFailed { error } => {
                self.blocks
                    .push(Block::Compact(CompactEvent::Failed { error }));
            }
            Event::Alert(alert) => {
                self.blocks.push(Block::Alert {
                    level: alert.level,
                    source: alert.source,
                    message: alert.message,
                });
            }
            Event::History { items, greeting } => {
                self.restore_history(&items, greeting);
            }
            Event::Completions { kind, entries } => {
                // Apply only if the popup is still on the same
                // (kind, prefix) the request was issued for; an
                // out-of-date reply (the user typed past it) is dropped.
                if let Some(state) = self.completion.as_mut()
                    && state.kind == kind
                {
                    state.entries = entries;
                    state.selected = 0;
                }
            }
            Event::Shutdown => {
                self.quit = true;
            }
        }
    }

    fn append_assistant_text(&mut self, text: &str) {
        if self.assistant_streaming {
            if let Some(Block::AssistantText { text: existing }) = self.blocks.last_mut() {
                existing.push_str(text);
                return;
            }
        }
        self.blocks.push(Block::AssistantText {
            text: text.to_owned(),
        });
        self.assistant_streaming = true;
    }

    /// Walk the most recently pushed blocks looking for a thinking
    /// block that's still in `Streaming`. Stops at the current
    /// `TurnHeader` to avoid latching onto a thinking block from a
    /// previous turn after it was somehow left dangling.
    fn last_streaming_thinking_mut(&mut self) -> Option<&mut ThinkingBlock> {
        for b in self.blocks.iter_mut().rev() {
            match b {
                Block::Thinking(t) if matches!(t.state, ThinkingState::Streaming { .. }) => {
                    return Some(t);
                }
                Block::TurnHeader { .. } => return None,
                _ => continue,
            }
        }
        None
    }

    fn mark_orphan_thinking_incomplete(&mut self) {
        // A turn can carry several thinking blocks; we walk all the way
        // to `TurnHeader` and convert every still-Streaming one rather
        // than breaking on the first Finished hit (which is what the
        // tool-call equivalent does, since tool calls finalize in
        // submission order).
        for b in self.blocks.iter_mut().rev() {
            match b {
                Block::Thinking(t) => {
                    if let ThinkingState::Streaming { started_at } = t.state {
                        t.state = ThinkingState::Incomplete {
                            elapsed_secs: Some(started_at.elapsed().as_secs()),
                        };
                    }
                }
                Block::TurnHeader { .. } => break,
                _ => {}
            }
        }
    }

    fn find_tool_call_mut(&mut self, id: &str) -> Option<&mut ToolCallBlock> {
        for b in self.blocks.iter_mut().rev() {
            if let Block::ToolCall(tc) = b
                && tc.id == id
            {
                return Some(tc);
            }
        }
        None
    }

    /// Called on `TurnEnd`: mark any tool call still in an in-progress
    /// state as `Incomplete` so the user sees something was left hanging
    /// instead of a silently-truncated block.
    fn mark_orphan_tool_calls_incomplete(&mut self) {
        for b in self.blocks.iter_mut().rev() {
            if let Block::ToolCall(tc) = b {
                if matches!(
                    tc.state,
                    ToolCallState::Pending | ToolCallState::Streaming | ToolCallState::Executing
                ) {
                    tc.state = ToolCallState::Incomplete;
                } else {
                    // Earlier tool calls in the same list are already
                    // finalized; stop walking.
                    break;
                }
            } else if matches!(b, Block::TurnHeader { .. }) {
                break;
            }
        }
    }

    // Input manipulation — thin forwarders so call sites in main.rs
    // stay readable.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert_char(c);
    }
    pub fn insert_newline(&mut self) {
        self.input.insert_newline();
    }
    pub fn insert_paste(&mut self, content: String) {
        self.input.insert_paste(content);
    }
    pub fn delete_char_before(&mut self) {
        self.input.delete_before();
    }
    pub fn delete_char_after(&mut self) {
        self.input.delete_after();
    }
    pub fn move_cursor_left(&mut self) {
        self.input.move_left();
    }
    pub fn move_cursor_right(&mut self) {
        self.input.move_right();
    }
    pub fn move_cursor_start(&mut self) {
        self.input.move_start();
    }
    pub fn move_cursor_home(&mut self) {
        self.input.move_home();
    }
    pub fn move_cursor_end(&mut self) {
        self.input.move_end();
    }
    pub fn move_cursor_up(&mut self) {
        self.input.move_up();
    }
    pub fn move_cursor_down(&mut self) {
        self.input.move_down();
    }

    fn restore_history(&mut self, items: &[serde_json::Value], greeting: protocol::Greeting) {
        // Fresh session: greeting + any replayed items. Append-only — we
        // don't try to merge with already-displayed live events because
        // `History` only fires on an empty live state.
        self.turn_index = 0;
        self.blocks.clear();
        self.cache = FileCache::new();
        self.blocks.push(Block::Greeting(greeting));
        self.assistant_streaming = false;

        for item in items {
            let item_type = item["type"].as_str().unwrap_or("");
            match item_type {
                "message" => {
                    let role = item["role"].as_str().unwrap_or("");
                    let text = item["content"]
                        .as_array()
                        .and_then(|parts| parts.iter().filter_map(|p| p["text"].as_str()).next())
                        .unwrap_or("")
                        .to_owned();
                    match role {
                        "user" => {
                            self.turn_index += 1;
                            self.blocks.push(Block::TurnHeader {
                                turn: self.turn_index,
                            });
                            // Pod attaches the original `Vec<Segment>` to
                            // user messages from live submissions, so we
                            // can rebuild typed atoms (paste chips, refs)
                            // here. Seed history loaded post-compaction
                            // has no `segments` field — fall back to a
                            // single text segment.
                            let segments = item
                                .get("segments")
                                .and_then(|v| {
                                    serde_json::from_value::<Vec<Segment>>(v.clone()).ok()
                                })
                                .unwrap_or_else(|| {
                                    if text.is_empty() {
                                        Vec::new()
                                    } else {
                                        vec![Segment::text(text.clone())]
                                    }
                                });
                            if !segments.is_empty() {
                                self.blocks.push(Block::UserMessage { segments });
                            }
                        }
                        "assistant" if !text.is_empty() => {
                            self.blocks.push(Block::AssistantText { text });
                        }
                        _ => {}
                    }
                }
                "tool_call" => {
                    // `Item::ToolCall` serializes the linking key as
                    // `call_id`; `id` is a separate optional item-level
                    // identifier. Use `call_id` so this matches how
                    // Event::ToolCallStart populates the block.
                    let id = item["call_id"].as_str().unwrap_or("").to_owned();
                    let name = item["name"].as_str().unwrap_or("?").to_owned();
                    let arguments = item["arguments"].as_str().map(|s| s.to_owned());
                    self.blocks.push(Block::ToolCall(ToolCallBlock {
                        id,
                        name,
                        args_stream: arguments.clone().unwrap_or_default(),
                        arguments,
                        state: ToolCallState::Executing,
                        edit_snapshot: None,
                    }));
                }
                "reasoning" => {
                    let text = item["text"].as_str().unwrap_or("").to_owned();
                    let body = if text.is_empty() {
                        item["summary"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default()
                    } else {
                        text
                    };
                    self.blocks.push(Block::Thinking(ThinkingBlock {
                        text: body,
                        state: ThinkingState::Finished { elapsed_secs: None },
                    }));
                }
                "tool_result" => {
                    let id = item["call_id"].as_str().unwrap_or("").to_owned();
                    let summary = item["summary"].as_str().unwrap_or("").to_owned();
                    let output = item["content"].as_str().map(|s| s.to_owned());
                    let is_error = item["is_error"].as_bool().unwrap_or(false);
                    let (name, args) = self
                        .find_tool_call_mut(&id)
                        .map(|b| (b.name.clone(), b.arguments.clone()))
                        .unwrap_or_default();
                    let edit_snapshot = if !is_error && name == "Edit" {
                        args.as_deref()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                            .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
                            .and_then(|path| self.cache.get(&path).map(|s| s.to_owned()))
                    } else {
                        None
                    };
                    if let Some(tc) = self.find_tool_call_mut(&id) {
                        if edit_snapshot.is_some() {
                            tc.edit_snapshot = edit_snapshot;
                        }
                        tc.state = if is_error {
                            ToolCallState::Error {
                                summary,
                                output: output.clone(),
                            }
                        } else {
                            ToolCallState::Done {
                                summary,
                                output: output.clone(),
                            }
                        };
                        if !is_error {
                            apply_cache_update(
                                &mut self.cache,
                                &name,
                                args.as_deref(),
                                output.as_deref(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        // Any tool_call entries that never got paired with a
        // tool_result (truncated or racing mid-turn on the server side)
        // stay as Executing up to this point. Surface them as
        // Incomplete so the replay matches live semantics.
        for b in self.blocks.iter_mut() {
            if let Block::ToolCall(tc) = b
                && matches!(
                    tc.state,
                    ToolCallState::Executing | ToolCallState::Pending | ToolCallState::Streaming
                )
            {
                tc.state = ToolCallState::Incomplete;
            }
        }
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Strip the `cat -n` line-number gutter that the Read tool prepends to
/// its output (one `"{n:>6}\t{content}"` per line) and return the raw
/// file body. Lines that don't match the pattern are kept verbatim, so
/// unrelated payloads pass through unharmed.
fn strip_cat_n_prefix(formatted: &str) -> String {
    let mut out = String::with_capacity(formatted.len());
    let mut first = true;
    for line in formatted.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        match line.split_once('\t') {
            Some((prefix, rest)) if prefix.trim().chars().all(|c| c.is_ascii_digit()) => {
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// True if the submitted segment list carries no user-visible content
/// (only whitespace / newlines, no paste, no typed atoms). Used to
/// decide whether an empty Enter should be a no-op or trigger a
/// `Resume` when the Pod is paused.
fn segments_are_blank(segments: &[Segment]) -> bool {
    segments.iter().all(|s| match s {
        Segment::Text { content } => content.trim().is_empty(),
        _ => false,
    })
}

pub fn alert_source_label(source: AlertSource) -> &'static str {
    match source {
        AlertSource::Pod => "pod",
        AlertSource::Worker => "worker",
        AlertSource::Compactor => "compactor",
        AlertSource::AgentsMd => "AGENTS.md",
    }
}

#[cfg(test)]
mod completion_flow_tests {
    use super::*;

    #[test]
    fn typing_at_creates_completion_state_and_emits_query() {
        let mut app = App::new("test".into());
        app.insert_char('@');
        let method = app.refresh_completion();
        match method {
            Some(Method::ListCompletions { kind, prefix }) => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
        assert!(app.completion.is_some());
    }

    #[test]
    fn appending_to_token_emits_updated_query() {
        let mut app = App::new("test".into());
        app.insert_char('@');
        let _ = app.refresh_completion();
        app.insert_char('s');
        let method = app.refresh_completion();
        match method {
            Some(Method::ListCompletions { kind, prefix }) => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "s");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
    }

    #[test]
    fn space_after_token_clears_completion_state() {
        let mut app = App::new("test".into());
        for c in "@x".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        assert!(app.completion.is_some());
        app.insert_char(' ');
        let method = app.refresh_completion();
        assert!(method.is_none());
        assert!(app.completion.is_none());
    }

    #[test]
    fn tab_inserts_entry_value_as_text_for_file() {
        let mut app = App::new("test".into());
        for c in "@s".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
        }];
        // Tab path: text inserted, popup re-triggered with new prefix
        // (still File kind since the typed range stays after `@`).
        let _ = app.apply_completion_text();
        // The input now reads `@src/main.rs` as plain Char atoms; no
        // chip yet.
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@src/main.rs"));
        assert!(app.completion.is_some());
    }

    #[test]
    fn tab_appends_trailing_slash_for_directory() {
        let mut app = App::new("test".into());
        for c in "@cr".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
        }];
        let _ = app.apply_completion_text();
        // Typed prefix advances to `crates/` so the next query can
        // descend into the directory.
        assert_eq!(app.completion.as_ref().unwrap().prefix, "crates/");
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@crates/"));
    }

    #[test]
    fn space_chipifies_on_exact_match() {
        let mut app = App::new("test".into());
        for c in "@src/main.rs".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
        }];
        assert!(app.chipify_completion_if_exact_match());
        assert!(app.completion.is_none());
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "src/main.rs"));
    }

    #[test]
    fn space_does_not_chipify_on_partial_match() {
        let mut app = App::new("test".into());
        for c in "@s".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
        }];
        // typed = "s", expected = "src/main.rs" → no match, no chip.
        assert!(!app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@s"));
    }

    #[test]
    fn space_chipifies_directory_with_or_without_trailing_slash() {
        // Slash-less typed form chipifies the directory; the chip's
        // path keeps a trailing slash so the rendered label is `@crates/`.
        let mut app = App::new("test".into());
        for c in "@crates".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
        }];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));

        // Slashed typed form (the shape Tab inserts) — same chip.
        let mut app = App::new("test".into());
        for c in "@crates/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
        }];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));
    }

    #[test]
    fn space_chipifies_directory_when_popup_shows_its_children() {
        // `@crates/` is the form Tab leaves you in after picking a
        // directory; the popup is showing the children of `crates/`.
        // Hitting space at this point should chipify `crates`, not
        // require the user to back up and remove the trailing slash.
        let mut app = App::new("test".into());
        for c in "@crates/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![
            CompletionEntry {
                value: "crates/daemon".into(),
                is_dir: true,
            },
            CompletionEntry {
                value: "crates/llm-worker".into(),
                is_dir: true,
            },
        ];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));
    }

    #[test]
    fn enter_does_not_chipify_directory_so_drill_in_works() {
        // Enter on a selected directory entry must NOT chipify —
        // otherwise the user can never drill into the dir to see
        // its children.
        let mut app = App::new("test".into());
        for c in "@crates".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
        }];
        assert!(!app.chipify_selected_completion_if_committable());
        // Popup is still active so the caller can fall through to
        // apply_completion_text.
        assert!(app.completion.is_some());
    }

    #[test]
    fn enter_path_appends_trailing_space_after_file_chip() {
        // Mirrors the main.rs Enter handler sequence: chipify the
        // selected entry, then insert a space so the cursor is ready
        // for the next token without a manual separator.
        let mut app = App::new("test".into());
        for c in "@README.".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "README.md".into(),
            is_dir: false,
        }];
        assert!(app.chipify_selected_completion_if_committable());
        app.insert_char(' ');
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "README.md"));
        assert!(matches!(&segs[1], Segment::Text { content } if content == " "));
    }

    #[test]
    fn enter_chipifies_selected_file_even_when_typed_is_partial() {
        // Enter respects the selected entry: typed text may be a
        // prefix of the entry's value, but the popup-highlighted
        // file should still chipify on Enter.
        let mut app = App::new("test".into());
        for c in "@README.".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "README.md".into(),
            is_dir: false,
        }];
        assert!(app.chipify_selected_completion_if_committable());
        assert!(app.completion.is_none());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "README.md"));
    }

    #[test]
    fn space_does_not_chipify_drilled_state_with_unrelated_entries() {
        // Stale entries that don't live under the typed prefix should
        // not satisfy the drilled-into-directory rule.
        let mut app = App::new("test".into());
        for c in "@xyz/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates/daemon".into(),
            is_dir: true,
        }];
        assert!(!app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@xyz/"));
    }

    #[test]
    fn chipify_finds_match_outside_selected_index() {
        // Regression guard for the race where a stale reply leaves a
        // non-matching entry at index 0 but an entry deeper in the
        // list does match the current typed text.
        let mut app = App::new("test".into());
        for c in "@src/main.rs".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![
            CompletionEntry {
                value: "src/main.rs.bak".into(),
                is_dir: false,
            },
            CompletionEntry {
                value: "src/main.rs".into(),
                is_dir: false,
            },
        ];
        // selected stays at 0 (the non-matching one) but find() should
        // still locate the match.
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "src/main.rs"));
    }

    #[test]
    fn apply_completion_text_with_no_entries_is_a_noop() {
        let mut app = App::new("test".into());
        for c in "@x".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        // No `Event::Completions` arrived yet — entries is still empty.
        assert!(app.apply_completion_text().is_none());
        assert!(app.completion.is_some());
    }

    #[test]
    fn outdated_completions_event_is_dropped() {
        let mut app = App::new("test".into());
        for c in "@x".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        // Reply for a different kind shouldn't overwrite state.
        app.handle_pod_event(Event::Completions {
            kind: CompletionKind::Workflow,
            entries: vec![CompletionEntry {
                value: "stale".into(),
                is_dir: false,
            }],
        });
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
    }
}

/// Seed / mutate the file-content cache based on a completed tool call.
///
/// Each built-in file tool has its own rule: Read copies the result body
/// into the cache, Write replaces it with `args.content`, Edit applies
/// the `old_string → new_string` swap in-place.
fn apply_cache_update(
    cache: &mut FileCache,
    name: &str,
    arguments: Option<&str>,
    output: Option<&str>,
) {
    let args = arguments.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    match name {
        "Read" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            if let Some(content) = output {
                // The Read tool emits a `cat -n` style display: each
                // line is "{lineno:>6}\tcontent". Strip that framing
                // so the cache mirrors the real file body and the
                // Edit diff renderer has a faithful "before" view.
                cache.put(path, strip_cat_n_prefix(content));
            }
        }
        "Write" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            let Some(content) = args["content"].as_str() else {
                return;
            };
            cache.put(path, content.to_owned());
        }
        "Edit" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            let Some(old) = args["old_string"].as_str() else {
                return;
            };
            let Some(new) = args["new_string"].as_str() else {
                return;
            };
            cache.apply_edit(path, old, new);
        }
        _ => {}
    }
}
