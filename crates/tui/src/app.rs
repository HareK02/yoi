use std::time::Instant;

use protocol::{AlertLevel, AlertSource, Event, Method, RunResult, Segment};

use crate::block::{
    Block, CompactEvent, ThinkingBlock, ThinkingState, ToolCallBlock, ToolCallState,
};
use crate::cache::FileCache;
use crate::input::InputBuffer;
use crate::scroll::Scroll;
use crate::ui::Mode;

pub struct App {
    pub pod_name: String,
    pub connected: bool,
    pub running: bool,
    /// True while the Pod is in `PodStatus::Paused`. Set on
    /// `RunEnd::Paused` and cleared when a new turn starts (either via
    /// `Resume` or a fresh `Run`).
    pub paused: bool,
    pub run_requests: usize,
    pub run_input_tokens: u64,
    pub run_output_tokens: u64,
    pub turn_index: usize,
    pub current_tool: Option<String>,
    pub input: InputBuffer,
    pub quit: bool,
    pub shutdown_confirm: Option<std::time::Instant>,
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
}

impl App {
    pub fn new(pod_name: String) -> Self {
        Self {
            pod_name,
            connected: false,
            running: false,
            paused: false,
            run_requests: 0,
            run_input_tokens: 0,
            run_output_tokens: 0,
            turn_index: 0,
            current_tool: None,
            input: InputBuffer::new(),
            quit: false,
            shutdown_confirm: None,
            quit_confirm: None,
            blocks: Vec::new(),
            scroll: Scroll::default(),
            mode: Mode::Normal,
            cache: FileCache::new(),
            assistant_streaming: false,
        }
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
            } => {
                self.run_input_tokens += input_tokens.unwrap_or(0);
                self.run_output_tokens += output_tokens.unwrap_or(0);
            }
            Event::Error { code, message } => {
                self.push_error(format!("[{code:?}] {message}"));
            }
            Event::RunEnd { result } => {
                self.blocks.push(Block::TurnStats {
                    requests: self.run_requests,
                    input_tokens: self.run_input_tokens,
                    output_tokens: self.run_output_tokens,
                });
                self.running = false;
                self.paused = matches!(result, RunResult::Paused);
                self.run_requests = 0;
                self.run_input_tokens = 0;
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
    pub fn delete_word_before(&mut self) {
        self.input.delete_word_before();
    }
    pub fn move_cursor_left(&mut self) {
        self.input.move_left();
    }
    pub fn move_cursor_right(&mut self) {
        self.input.move_right();
    }
    pub fn move_cursor_word_left(&mut self) {
        self.input.move_word_left();
    }
    pub fn move_cursor_word_right(&mut self) {
        self.input.move_word_right();
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
