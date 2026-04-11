use protocol::{Event, Method};

pub struct App {
    pub pod_name: String,
    pub connected: bool,
    pub messages: Vec<Message>,
    pub current_text: String,
    pub status_line: String,
    pub input: String,
    pub cursor: usize,
    pub scroll: u16,
    pub quit: bool,
}

pub struct Message {
    pub kind: MessageKind,
    pub content: String,
}

#[derive(Clone, Copy)]
pub enum MessageKind {
    User,
    Assistant,
    Tool,
    Error,
}

impl App {
    pub fn new(pod_name: String) -> Self {
        Self {
            pod_name,
            connected: false,
            messages: Vec::new(),
            current_text: String::new(),
            status_line: String::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            quit: false,
        }
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.messages.push(Message {
            kind: MessageKind::User,
            content: text.clone(),
        });
        self.input.clear();
        self.cursor = 0;
        self.scroll_to_bottom();
        Some(Method::Run { input: text })
    }

    pub fn handle_pod_event(&mut self, event: Event) {
        match event {
            Event::TurnStart { turn } => {
                self.status_line = format!("turn {turn}");
            }
            Event::TextDelta { text } => {
                self.current_text.push_str(&text);
            }
            Event::TextDone { .. } => {
                let text = std::mem::take(&mut self.current_text);
                if !text.is_empty() {
                    self.messages.push(Message {
                        kind: MessageKind::Assistant,
                        content: text,
                    });
                    self.scroll_to_bottom();
                }
            }
            Event::TurnEnd { turn, result } => {
                if !self.current_text.is_empty() {
                    let text = std::mem::take(&mut self.current_text);
                    self.messages.push(Message {
                        kind: MessageKind::Assistant,
                        content: text,
                    });
                }
                self.status_line = format!("turn {turn} {result:?}");
            }
            Event::ToolCallStart { name, .. } => {
                self.status_line = format!("tool: {name}");
                self.messages.push(Message {
                    kind: MessageKind::Tool,
                    content: format!("[tool] {name}"),
                });
                self.scroll_to_bottom();
            }
            Event::ToolCallDone {
                name, arguments, ..
            } => {
                self.messages.push(Message {
                    kind: MessageKind::Tool,
                    content: format!("[tool] {name} done ({} bytes)", arguments.len()),
                });
                self.scroll_to_bottom();
            }
            Event::ToolResult {
                output, is_error, ..
            } => {
                let prefix = if is_error { "[tool error]" } else { "[tool result]" };
                let display = if output.len() > 200 {
                    format!("{}...", &output[..200])
                } else {
                    output
                };
                self.messages.push(Message {
                    kind: MessageKind::Tool,
                    content: format!("{prefix} {display}"),
                });
                self.scroll_to_bottom();
            }
            Event::Usage {
                input_tokens,
                output_tokens,
            } => {
                let in_t = input_tokens.unwrap_or(0);
                let out_t = output_tokens.unwrap_or(0);
                self.status_line
                    .push_str(&format!(" | in:{in_t} out:{out_t}"));
            }
            Event::Error { code, message } => {
                self.messages.push(Message {
                    kind: MessageKind::Error,
                    content: format!("[{code:?}] {message}"),
                });
                self.scroll_to_bottom();
            }
            Event::RunEnd { result } => {
                self.status_line = format!("{result:?}");
            }
            Event::ToolCallArgsDelta { .. } => {}
            Event::History { items } => {
                self.restore_history(&items);
            }
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

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(3);
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(3);
    }

    /// Total visible lines (for rendering the in-progress text as part of output).
    pub fn display_lines(&self) -> Vec<(&MessageKind, &str)> {
        let mut lines: Vec<(&MessageKind, &str)> = self
            .messages
            .iter()
            .map(|m| (&m.kind, m.content.as_str()))
            .collect();
        if !self.current_text.is_empty() {
            lines.push((&MessageKind::Assistant, &self.current_text));
        }
        lines
    }

    fn restore_history(&mut self, items: &[serde_json::Value]) {
        self.messages.clear();
        for item in items {
            let item_type = item["type"].as_str().unwrap_or("");
            match item_type {
                "message" => {
                    let role = item["role"].as_str().unwrap_or("");
                    let kind = match role {
                        "user" => MessageKind::User,
                        "assistant" => MessageKind::Assistant,
                        _ => continue,
                    };
                    let text = item["content"]
                        .as_array()
                        .and_then(|parts| {
                            parts
                                .iter()
                                .filter_map(|p| p["text"].as_str())
                                .next()
                        })
                        .unwrap_or("");
                    if !text.is_empty() {
                        self.messages.push(Message {
                            kind,
                            content: text.to_owned(),
                        });
                    }
                }
                "tool_call" => {
                    let name = item["name"].as_str().unwrap_or("?");
                    self.messages.push(Message {
                        kind: MessageKind::Tool,
                        content: format!("[tool] {name}"),
                    });
                }
                "tool_result" => {
                    let output = item["output"].as_str().unwrap_or("");
                    let display = if output.len() > 200 {
                        format!("{}...", &output[..200])
                    } else {
                        output.to_owned()
                    };
                    self.messages.push(Message {
                        kind: MessageKind::Tool,
                        content: format!("[tool result] {display}"),
                    });
                }
                _ => {}
            }
        }
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&mut self) {
        // Will be clamped during rendering
        self.scroll = u16::MAX;
    }
}
