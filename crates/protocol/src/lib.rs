pub mod stream;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Method (Client → Pod via Unix Socket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    Run { input: String },
    Notify { source: String, message: String },
    Resume,
    Cancel,
    Shutdown,
    GetHistory,
}

// ---------------------------------------------------------------------------
// Event (Pod → Client via Unix Socket broadcast)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    TurnStart {
        turn: usize,
    },
    TurnEnd {
        turn: usize,
        result: TurnResult,
    },
    TextDelta {
        text: String,
    },
    TextDone {
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
        output: String,
        is_error: bool,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    RunEnd {
        result: RunResult,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    History {
        items: Vec<serde_json::Value>,
        greeting: Greeting,
    },
    Notification(Notification),
    Shutdown,
}

/// User-facing notification emitted from the Pod layer.
///
/// This is a separate channel from `tracing` (developer logs): entries
/// here are assembled explicitly by the Pod when a condition should be
/// surfaced to the person driving the client. Keep messages short and
/// human-readable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub level: NotificationLevel,
    pub source: NotificationSource,
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSource {
    Pod,
    Worker,
    Compactor,
    AgentsMd,
}

/// Pod self-description rendered by the TUI when a session starts empty.
///
/// Built once in the Pod controller from the resolved manifest and
/// transmitted alongside `Event::History` so clients don't need their
/// own view of the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Greeting {
    pub pod_name: String,
    pub cwd: String,
    pub provider: String,
    pub model: String,
    pub scope_summary: String,
    pub tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnResult {
    Finished,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Finished,
    Paused,
    LimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AlreadyRunning,
    NotRunning,
    NotPaused,
    ProviderError,
    ToolError,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_run_json_roundtrip() {
        let json = r#"{"method":"run","params":{"input":"Hello"}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Run { ref input } if input == "Hello"));

        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn method_without_params() {
        let json = r#"{"method":"resume"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Resume));
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
    fn method_notify_json_roundtrip() {
        let json = r#"{"method":"notify","params":{"source":"child-pod","message":"turn done"}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(
            method,
            Method::Notify { ref source, ref message }
            if source == "child-pod" && message == "turn done"
        ));
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn method_get_history() {
        let json = r#"{"method":"get_history"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::GetHistory));
    }

    #[test]
    fn event_history_format() {
        let event = Event::History {
            items: vec![serde_json::json!({"type": "message", "role": "user"})],
            greeting: Greeting {
                pod_name: "test".into(),
                cwd: "/tmp".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                scope_summary: "Writable:\n  - /tmp".into(),
                tools: vec!["Read".into()],
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "history");
        assert!(parsed["data"]["items"].is_array());
        assert_eq!(parsed["data"]["items"][0]["role"], "user");
        assert_eq!(parsed["data"]["greeting"]["pod_name"], "test");
        assert_eq!(parsed["data"]["greeting"]["tools"][0], "Read");
    }

    #[test]
    fn event_notification_format() {
        let event = Event::Notification(Notification {
            level: NotificationLevel::Warn,
            source: NotificationSource::Compactor,
            message: "compaction failed".into(),
            timestamp_ms: 1_700_000_000_000,
        });
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "notification");
        assert_eq!(parsed["data"]["level"], "warn");
        assert_eq!(parsed["data"]["source"], "compactor");
        assert_eq!(parsed["data"]["message"], "compaction failed");
        assert_eq!(parsed["data"]["timestamp_ms"], 1_700_000_000_000i64);
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
}
