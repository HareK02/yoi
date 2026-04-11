pub mod stream;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Method (Client → Pod via Unix Socket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    Run { input: String },
    Resume,
    Cancel,
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
    },
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
    fn method_get_history() {
        let json = r#"{"method":"get_history"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::GetHistory));
    }

    #[test]
    fn event_history_format() {
        let event = Event::History {
            items: vec![serde_json::json!({"type": "message", "role": "user"})],
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "history");
        assert!(parsed["data"]["items"].is_array());
        assert_eq!(parsed["data"]["items"][0]["role"], "user");
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
