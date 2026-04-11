use protocol::{Event, Method};
use tui_scrollview::ScrollViewState;

pub struct App {
    pub pod_name: String,
    pub connected: bool,
    pub messages: Vec<Message>,
    pub current_text: String,
    pub running: bool,
    pub run_requests: usize,
    pub run_input_tokens: u64,
    pub run_output_tokens: u64,
    pub turn_index: usize,
    pub current_tool: Option<String>,
    pub input: String,
    pub cursor: usize,
    pub scroll_state: ScrollViewState,
    pub quit: bool,
}

pub struct Message {
    pub kind: MessageKind,
    pub content: String,
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
            messages: Vec::new(),
            current_text: String::new(),
            running: false,
            run_requests: 0,
            run_input_tokens: 0,
            run_output_tokens: 0,
            turn_index: 0,
            current_tool: None,
            input: String::new(),
            cursor: 0,
            scroll_state: ScrollViewState::new(),
            quit: false,
        }
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.turn_index += 1;
        self.messages.push(Message {
            kind: MessageKind::TurnHeader,
            content: format!("#{}", self.turn_index),
        });
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
            Event::TurnStart { .. } => {
                self.running = true;
                self.run_requests += 1;
                self.current_tool = None;
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
            Event::TurnEnd { .. } => {
                if !self.current_text.is_empty() {
                    let text = std::mem::take(&mut self.current_text);
                    self.messages.push(Message {
                        kind: MessageKind::Assistant,
                        content: text,
                    });
                }
                self.current_tool = None;
            }
            Event::ToolCallStart { name, .. } => {
                self.current_tool = Some(name.clone());
                self.messages.push(Message {
                    kind: MessageKind::Tool,
                    content: format!("[tool] {name}"),
                });
                self.scroll_to_bottom();
            }
            Event::ToolCallDone {
                name, arguments, ..
            } => {
                self.current_tool = None;
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
                self.run_input_tokens += input_tokens.unwrap_or(0);
                self.run_output_tokens += output_tokens.unwrap_or(0);
            }
            Event::Error { code, message } => {
                self.messages.push(Message {
                    kind: MessageKind::Error,
                    content: format!("[{code:?}] {message}"),
                });
                self.scroll_to_bottom();
            }
            Event::RunEnd { .. } => {
                self.messages.push(Message {
                    kind: MessageKind::TurnStats,
                    content: format!(
                        "{} reqs ↑{}/↓{}",
                        self.run_requests,
                        fmt_tokens(self.run_input_tokens),
                        fmt_tokens(self.run_output_tokens),
                    ),
                });
                self.running = false;
                self.run_requests = 0;
                self.run_input_tokens = 0;
                self.run_output_tokens = 0;
                self.current_tool = None;
                self.scroll_to_bottom();
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
        self.scroll_state.scroll_up();
        self.scroll_state.scroll_up();
        self.scroll_state.scroll_up();
    }

    pub fn scroll_down(&mut self) {
        self.scroll_state.scroll_down();
        self.scroll_state.scroll_down();
        self.scroll_state.scroll_down();
    }

    fn restore_history(&mut self, items: &[serde_json::Value]) {
        self.messages.clear();
        self.turn_index = 0;
        for item in items {
            let item_type = item["type"].as_str().unwrap_or("");
            match item_type {
                "message" => {
                    let role = item["role"].as_str().unwrap_or("");
                    let kind = match role {
                        "user" => {
                            self.turn_index += 1;
                            self.messages.push(Message {
                                kind: MessageKind::TurnHeader,
                                content: format!("#{}", self.turn_index),
                            });
                            MessageKind::User
                        }
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
        self.scroll_state.scroll_to_bottom();
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
