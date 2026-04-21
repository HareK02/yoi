pub mod stream;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Method (Client → Pod via Unix Socket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    Run { input: String },
    /// Human-readable text injected into the target Pod's LLM context
    /// as a non-blocking system message. No side effects beyond LLM
    /// context; use `PodEvent` for typed lifecycle reports.
    Notify { message: String },
    /// Typed lifecycle report from a child Pod to its direct parent.
    PodEvent(PodEvent),
    Resume,
    Cancel,
    /// Stop the in-flight turn and transition to `Paused`.
    ///
    /// Unlike `Cancel` (which discards and returns to `Idle`), a paused
    /// Pod can resume the interrupted work via `Resume`, or start a
    /// fresh turn via `Run` (orphan `tool_use` items are closed with a
    /// synthetic tool result before the new user message is appended).
    Pause,
    Shutdown,
    GetHistory,
}

/// Typed lifecycle events sent from a child Pod to its parent.
///
/// Delivered as `Method::PodEvent` over the parent's Unix socket. The
/// parent Controller applies variant-specific side effects (registry /
/// scope-lock updates) and renders a human-readable string that is
/// injected into the parent's LLM context via the notification buffer.
///
/// Transport is fire-and-forget; receivers must tolerate out-of-order
/// delivery (e.g. `TurnEnded` arriving after `ShutDown` for the same
/// child Pod).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PodEvent {
    /// Child finished one turn and is back to IDLE.
    TurnEnded { pod_name: String },

    /// Worker execution error occurred inside the child's turn.
    ///
    /// Limited to worker runtime failures (provider / tool errors) —
    /// does not include transient method-rejection responses such as
    /// `AlreadyRunning`.
    Errored { pod_name: String, message: String },

    /// Child has stopped (controller loop is exiting).
    ShutDown { pod_name: String },

    /// Child sub-delegated scope to a grandchild Pod via `SpawnPod`.
    ///
    /// The parent uses this to add the grandchild to its own
    /// `spawned_pods.json` so it can manage the grandchild directly
    /// even if the intermediate child dies. The parent then re-fires
    /// this event upward (if it has a parent of its own) to maintain
    /// the chain to root.
    ScopeSubDelegated {
        /// Sub-delegating Pod (= the sender itself).
        parent_pod: String,
        /// Name of the grandchild Pod.
        sub_pod: String,
        /// Unix-socket path where the grandchild is reachable.
        sub_socket: PathBuf,
        /// Scope delegated to the grandchild.
        scope: Vec<ScopeRule>,
    },
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
    /// Pod has started compacting the current session.
    ///
    /// Fired immediately before a compaction run. Success is signalled by
    /// `CompactDone` (with the new `SessionId`); failure by `CompactFailed`.
    /// Broadcast to all clients; not replayed to late subscribers.
    CompactStart,
    /// Compaction completed and the session was rotated.
    ///
    /// `new_session_id` is the UUID of the freshly created session that
    /// replaced the old history.
    CompactDone {
        new_session_id: uuid::Uuid,
    },
    /// Compaction failed. The session is unchanged.
    CompactFailed {
        error: String,
    },
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

// ---------------------------------------------------------------------------
// Scope rule / permission (wire type)
//
// Defined here so that both `manifest` (config parsing) and `protocol`
// itself (inter-pod messaging such as `PodEvent::ScopeSubDelegated`) can
// reference the same type without introducing a reverse dependency.
// ---------------------------------------------------------------------------

/// A single allow or deny rule inside a scope configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRule {
    /// Target path. Must be absolute by the time a `Scope` is built from
    /// this rule — relative paths are resolved per-layer against the
    /// manifest file's directory (cwd for overlay layers) before cascade
    /// merge.
    pub target: PathBuf,
    /// Permission level this rule grants (allow) or caps strictly below
    /// (deny).
    pub permission: Permission,
    /// When `false`, the rule only matches the target itself and its
    /// direct children. Defaults to `true`.
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_recursive() -> bool {
    true
}

/// Permission lattice used by [`ScopeRule`].
///
/// The derived `Ord` instance follows declaration order, so
/// `Read < Write`. Allow rules grant the stated level (and by extension
/// everything below); deny rules cap the effective level **strictly
/// below** the stated level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Write,
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
    fn method_pause_roundtrip() {
        let json = r#"{"method":"pause"}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(method, Method::Pause));
        let serialized = serde_json::to_string(&method).unwrap();
        assert_eq!(serialized, json);
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
        let json = r#"{"method":"notify","params":{"message":"turn done"}}"#;
        let method: Method = serde_json::from_str(json).unwrap();
        assert!(matches!(
            method,
            Method::Notify { ref message } if message == "turn done"
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
    fn method_pod_event_turn_ended_roundtrip() {
        let method = Method::PodEvent(PodEvent::TurnEnded {
            pod_name: "child".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["method"], "pod_event");
        assert_eq!(parsed["params"]["kind"], "turn_ended");
        assert_eq!(parsed["params"]["pod_name"], "child");

        let decoded: Method = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Method::PodEvent(PodEvent::TurnEnded { ref pod_name }) if pod_name == "child"
        ));
    }

    #[test]
    fn method_pod_event_errored_roundtrip() {
        let method = Method::PodEvent(PodEvent::Errored {
            pod_name: "child".into(),
            message: "provider 429".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::PodEvent(PodEvent::Errored { pod_name, message }) => {
                assert_eq!(pod_name, "child");
                assert_eq!(message, "provider 429");
            }
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[test]
    fn method_pod_event_shutdown_roundtrip() {
        let method = Method::PodEvent(PodEvent::ShutDown {
            pod_name: "child".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Method::PodEvent(PodEvent::ShutDown { ref pod_name }) if pod_name == "child"
        ));
    }

    #[test]
    fn method_pod_event_scope_sub_delegated_roundtrip() {
        let method = Method::PodEvent(PodEvent::ScopeSubDelegated {
            parent_pod: "child".into(),
            sub_pod: "grandchild".into(),
            sub_socket: "/run/insomnia/grandchild/sock".into(),
            scope: vec![ScopeRule {
                target: "/tmp/work".into(),
                permission: Permission::Write,
                recursive: true,
            }],
        });
        let json = serde_json::to_string(&method).unwrap();
        let decoded: Method = serde_json::from_str(&json).unwrap();
        match decoded {
            Method::PodEvent(PodEvent::ScopeSubDelegated {
                parent_pod,
                sub_pod,
                sub_socket,
                scope,
            }) => {
                assert_eq!(parent_pod, "child");
                assert_eq!(sub_pod, "grandchild");
                assert_eq!(sub_socket, PathBuf::from("/run/insomnia/grandchild/sock"));
                assert_eq!(scope.len(), 1);
                assert_eq!(scope[0].target, PathBuf::from("/tmp/work"));
                assert_eq!(scope[0].permission, Permission::Write);
                assert!(scope[0].recursive);
            }
            other => panic!("expected ScopeSubDelegated, got {other:?}"),
        }
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
    fn event_compact_start_roundtrip() {
        let event = Event::CompactStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"event":"compact_start"}"#);
        let decoded: Event = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, Event::CompactStart));
    }

    #[test]
    fn event_compact_done_roundtrip() {
        let id = uuid::Uuid::parse_str("0192f0e8-4d84-7d6e-a000-000000000001").unwrap();
        let event = Event::CompactDone { new_session_id: id };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "compact_done");
        assert_eq!(
            parsed["data"]["new_session_id"],
            "0192f0e8-4d84-7d6e-a000-000000000001"
        );
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::CompactDone { new_session_id } => assert_eq!(new_session_id, id),
            other => panic!("expected CompactDone, got {other:?}"),
        }
    }

    #[test]
    fn event_compact_failed_roundtrip() {
        let event = Event::CompactFailed {
            error: "provider 429".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "compact_failed");
        assert_eq!(parsed["data"]["error"], "provider 429");
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::CompactFailed { error } => assert_eq!(error, "provider 429"),
            other => panic!("expected CompactFailed, got {other:?}"),
        }
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
