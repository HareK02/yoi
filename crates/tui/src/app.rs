use protocol::{Event, Greeting, Method};

pub struct App {
    pub pod_name: String,
    pub connected: bool,
    pub running: bool,
    pub run_requests: usize,
    pub run_input_tokens: u64,
    pub run_output_tokens: u64,
    pub turn_index: usize,
    pub current_tool: Option<String>,
    pub input: String,
    pub cursor: usize,
    pub quit: bool,
    /// Lines waiting to be flushed to terminal via insert_before.
    pub output_queue: Vec<OutputItem>,
    /// Partial streaming text not yet terminated by newline.
    pending_text: String,
}

/// A unit of output to push above the inline viewport.
pub enum OutputItem {
    TurnHeader(String),
    Padded(MessageKind, String),
    PaddedRight(MessageKind, String),
    GreetingCard(Greeting),
    Blank,
}

#[derive(Clone, Copy)]
pub enum MessageKind {
    TurnHeader,
    User,
    Assistant,
    Tool,
    Error,
    TurnStats,
}

impl App {
    pub fn new(pod_name: String) -> Self {
        Self {
            pod_name,
            connected: false,
            running: false,
            run_requests: 0,
            run_input_tokens: 0,
            run_output_tokens: 0,
            turn_index: 0,
            current_tool: None,
            input: String::new(),
            cursor: 0,
            quit: false,
            output_queue: Vec::new(),
            pending_text: String::new(),
        }
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.turn_index += 1;
        self.output_queue.push(OutputItem::Blank);
        self.output_queue
            .push(OutputItem::TurnHeader(format!("#{}", self.turn_index)));
        self.output_queue
            .push(OutputItem::Padded(MessageKind::User, text.clone()));
        self.output_queue.push(OutputItem::Blank);
        self.input.clear();
        self.cursor = 0;
        Some(Method::Run { input: text })
    }

    pub fn handle_pod_event(&mut self, event: Event) {
        match event {
            Event::TurnStart { .. } => {
                self.running = true;
                self.run_requests += 1;
                self.current_tool = None;
            }
            Event::TextDelta { text } => {
                self.pending_text.push_str(&text);
                self.flush_pending_lines();
            }
            Event::TextDone { .. } => {
                // Flush any remaining partial line
                if !self.pending_text.is_empty() {
                    let text = std::mem::take(&mut self.pending_text);
                    self.output_queue
                        .push(OutputItem::Padded(MessageKind::Assistant, text));
                }
            }
            Event::TurnEnd { .. } => {
                // Flush streaming text if TextDone wasn't received
                if !self.pending_text.is_empty() {
                    let text = std::mem::take(&mut self.pending_text);
                    self.output_queue
                        .push(OutputItem::Padded(MessageKind::Assistant, text));
                }
                self.current_tool = None;
            }
            Event::ToolCallStart { name, .. } => {
                self.current_tool = Some(name.clone());
                self.output_queue.push(OutputItem::Padded(
                    MessageKind::Tool,
                    format!("[tool] {name}"),
                ));
            }
            Event::ToolCallDone {
                name, arguments, ..
            } => {
                self.current_tool = None;
                self.output_queue.push(OutputItem::Padded(
                    MessageKind::Tool,
                    format!("[tool] {name} done ({} bytes)", arguments.len()),
                ));
            }
            Event::ToolResult {
                output, is_error, ..
            } => {
                let prefix = if is_error {
                    "[tool error]"
                } else {
                    "[tool result]"
                };
                let display = if output.len() > 200 {
                    format!("{}...", &output[..200])
                } else {
                    output
                };
                self.output_queue.push(OutputItem::Padded(
                    MessageKind::Tool,
                    format!("{prefix} {display}"),
                ));
            }
            Event::Usage {
                input_tokens,
                output_tokens,
            } => {
                self.run_input_tokens += input_tokens.unwrap_or(0);
                self.run_output_tokens += output_tokens.unwrap_or(0);
            }
            Event::Error { code, message } => {
                self.output_queue.push(OutputItem::Padded(
                    MessageKind::Error,
                    format!("[{code:?}] {message}"),
                ));
            }
            Event::RunEnd { .. } => {
                self.output_queue.push(OutputItem::PaddedRight(
                    MessageKind::TurnStats,
                    format!(
                        "{} reqs ↑{}/↓{}",
                        self.run_requests,
                        fmt_tokens(self.run_input_tokens),
                        fmt_tokens(self.run_output_tokens),
                    ),
                ));
                self.output_queue.push(OutputItem::Blank);
                self.running = false;
                self.run_requests = 0;
                self.run_input_tokens = 0;
                self.run_output_tokens = 0;
                self.current_tool = None;
            }
            Event::ToolCallArgsDelta { .. } => {}
            Event::History { items, greeting } => {
                self.restore_history(&items);
                if self.turn_index == 0 {
                    self.output_queue
                        .insert(0, OutputItem::GreetingCard(greeting));
                    self.output_queue.insert(1, OutputItem::Blank);
                }
            }
        }
    }

    /// Extract complete lines (ending with \n) from pending_text and queue them.
    fn flush_pending_lines(&mut self) {
        while let Some(pos) = self.pending_text.find('\n') {
            let line = self.pending_text[..pos].to_owned();
            self.pending_text = self.pending_text[pos + 1..].to_owned();
            self.output_queue
                .push(OutputItem::Padded(MessageKind::Assistant, line));
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn delete_char_before(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.drain(prev..self.cursor);
            self.cursor = prev;
        }
    }

    pub fn delete_char_after(&mut self) {
        if self.cursor < self.input.len() {
            let next = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
            self.input.drain(self.cursor..next);
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    fn restore_history(&mut self, items: &[serde_json::Value]) {
        self.turn_index = 0;
        for item in items {
            let item_type = item["type"].as_str().unwrap_or("");
            match item_type {
                "message" => {
                    let role = item["role"].as_str().unwrap_or("");
                    let kind = match role {
                        "user" => {
                            self.turn_index += 1;
                            self.output_queue.push(OutputItem::Blank);
                            self.output_queue
                                .push(OutputItem::TurnHeader(format!("#{}", self.turn_index)));
                            MessageKind::User
                        }
                        "assistant" => MessageKind::Assistant,
                        _ => continue,
                    };
                    let text = item["content"]
                        .as_array()
                        .and_then(|parts| parts.iter().filter_map(|p| p["text"].as_str()).next())
                        .unwrap_or("");
                    if !text.is_empty() {
                        self.output_queue
                            .push(OutputItem::Padded(kind, text.to_owned()));
                        if matches!(kind, MessageKind::User) {
                            self.output_queue.push(OutputItem::Blank);
                        }
                    }
                }
                "tool_call" => {
                    let name = item["name"].as_str().unwrap_or("?");
                    self.output_queue.push(OutputItem::Padded(
                        MessageKind::Tool,
                        format!("[tool] {name}"),
                    ));
                }
                "tool_result" => {
                    let output = item["output"].as_str().unwrap_or("");
                    let display = if output.len() > 200 {
                        format!("{}...", &output[..200])
                    } else {
                        output.to_owned()
                    };
                    self.output_queue.push(OutputItem::Padded(
                        MessageKind::Tool,
                        format!("[tool result] {display}"),
                    ));
                }
                _ => {}
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
