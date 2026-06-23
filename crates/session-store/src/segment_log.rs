//! Segment log types for append-only JSONL persistence.
//!
//! Each [`LogEntry`] represents a single state transition within one
//! segment, serialized as one line in a `.jsonl` file. Reading all
//! entries and collecting them via [`collect_state`] reconstructs the
//! full [`Worker`] state at that segment.
//!
//! The on-disk format is one `LogEntry` per line — entries are positionally
//! ordered. Fork lineage references between segments use turn-number indices
//! (`SegmentOrigin.at_turn_index`) rather than per-entry hashes.

use llm_worker::llm_client::types::{Item, RequestConfig};
use llm_worker::{UsageRecord, WorkerResult};
use protocol::{InvokeKind, Segment};
use serde::{Deserialize, Serialize};

use crate::logged_item::LoggedItem;
use crate::system_item::SystemItem;

/// A single segment log entry, serialized as one JSONL line.
///
/// Variants correspond to specific mutation points in `Worker`:
/// - `SegmentStart` — always the first entry; captures initial state
/// - `Invoke` — IDLE → active marker (start of a new self-driving cycle)
/// - `UserInput` / `AssistantItem` / `ToolResult` / `SystemItem` — history appends
/// - `TurnEnd` — AgentTurn boundary marker; carries the post-increment
///   `turn_count`. With retry unimplemented today this fires once per
///   `run()`/`resume()` (current callers persist a single TurnEnd at
///   run completion); the fork-point seq for `at_turn_index` is the
///   preceding `Invoke` entry, not the TurnEnd.
/// - `RunCompleted` / `RunErrored` — marks end of a `run()` or `resume()` call
/// - `PausedTurnAbandoned` — explicit abandon/cancel of a paused interrupted turn
/// - `ConfigChanged` — `RequestConfig` mutation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEntry {
    /// Segment start. Always the first entry in a segment log.
    /// For forked segments, `history` contains the seed state from the parent.
    SegmentStart {
        ts: u64,
        /// Session this segment belongs to. Compaction / fork inherits
        /// the source segment's session_id; only fresh "new conversation"
        /// segments mint a new session_id.
        session_id: crate::SessionId,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<LoggedItem>,
        /// Origin: forked from a sibling segment at a specific turn boundary.
        /// The referenced segment is guaranteed to share `session_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forked_from: Option<SegmentOrigin>,
        /// Origin: compacted from a sibling segment at a specific turn boundary.
        /// The referenced segment is guaranteed to share `session_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacted_from: Option<SegmentOrigin>,
    },

    /// IDLE → active marker. Records the start of a new self-driving
    /// cycle (Invoke range). The range extends implicitly until the
    /// next `Invoke` entry; this entry carries the trigger only — the
    /// actual payload (user text / notify message / pod event body) is
    /// in the immediately following Turn entry (`UserInput` / `SystemItem`).
    ///
    /// Used by `pod-session-fork` style operations: the fork-point seq
    /// (`at_turn_index` in persistence-semantics) points at one of these
    /// `Invoke` entries so "back to N-th send" maps cleanly to the
    /// IDLE-break boundary the user sees.
    ///
    /// Field name is `trigger` (not `kind`) because the LogEntry
    /// serde tag already occupies `"kind"`.
    ///
    /// Marker only — replay does not mutate `RestoredState`.
    Invoke { ts: u64, trigger: InvokeKind },

    /// User input accepted at submit time. Carries the original typed
    /// `Vec<Segment>` so clients can re-render typed atoms (paste chips,
    /// file/knowledge refs, workflow invocations) on segment restore.
    /// Replay flattens these into a `Item::user_message` for the worker
    /// history; the worker layer never sees segments directly.
    UserInput { ts: u64, segments: Vec<Segment> },

    /// One assistant-side item appended to history — assistant message,
    /// reasoning, or tool call. Singular: one entry per history item so
    /// the wire-side `Event::*` lane and on-disk LogEntry stay 1:1.
    AssistantItem { ts: u64, item: LoggedItem },

    /// One tool-execution result appended to history.
    ToolResult { ts: u64, item: LoggedItem },

    /// One typed agent-injected system item: notification, child-Pod
    /// lifecycle event, `@<path>` / `#<slug>` / `/<slug>` resolution
    /// payload. Each `SystemItem` carries kind metadata that the LLM
    /// itself never sees (the LLM gets `Item::system_message` with the
    /// item's denormalised `body`), but live clients and replay paths
    /// dispatch on `kind` for typed rendering.
    SystemItem { ts: u64, item: SystemItem },

    /// Turn boundary. Records the turn count after increment.
    TurnEnd { ts: u64, turn_count: usize },

    /// `run()` / `resume()` が `WorkerResult` で正常終了した。
    /// Audit-only metadata: replay は `interrupted` のみ反映する。
    RunCompleted {
        ts: u64,
        interrupted: bool,
        result: WorkerResult,
    },

    /// `run()` / `resume()` が `WorkerError` で終了した。
    /// `WorkerError` は `Serialize` 不可なので `message` のみ lossy 保持する。
    /// Audit-only metadata: replay は `interrupted` のみ反映する。
    RunErrored {
        ts: u64,
        interrupted: bool,
        message: String,
    },

    /// A paused interrupted turn was explicitly abandoned without calling
    /// `run()` or `resume()` again. Replay clears the interrupted marker so
    /// the restored Pod is idle and future user input starts a normal new turn.
    PausedTurnAbandoned { ts: u64 },

    /// `RequestConfig` changed.
    ConfigChanged { ts: u64, config: RequestConfig },

    /// LLM リクエスト 1 件分の Usage スナップショット。
    ///
    /// `history_len` は送信時の `history.len()`。`input_total_tokens` は
    /// その prefix をプロバイダが実測した占有量（プロンプト全長）。
    /// このリクエスト 1 件で新しく追加された分ではない。
    ///
    /// プロバイダ別の正規化（呼び出し側で行う想定）:
    ///   - Anthropic: `input_tokens + cache_read + cache_creation`
    ///   - OpenAI:    `prompt_tokens`
    ///   - Gemini:    `promptTokenCount`
    ///   - Ollama:    `prompt_eval_count`
    ///
    /// `cache_read_tokens` / `cache_write_tokens` は上記の内訳で、料金会計用。
    LlmUsage {
        ts: u64,
        history_len: usize,
        input_total_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        output_tokens: u64,
    },

    /// 汎用拡張点。ドメイン名で名前空間を切って任意 JSON を載せる。
    /// session-store は payload を不透明扱いし、replay 時は
    /// `RestoredState.extensions` に `(domain, payload)` を順に積むだけ。
    /// 各ドメイン側が自前で fold して最新値を取り出す前提。
    ///
    /// 想定用途: memory subsystem の extract 処理境界 pointer 等、
    /// 「session 寿命に縛りたいが session-store の型を汚したくない」
    /// メタデータ。
    Extension {
        ts: u64,
        domain: String,
        payload: serde_json::Value,
    },
}

/// Provenance reference to a parent segment.
///
/// `at_turn_index` is the `turn_count` value of the most recent
/// `TurnEnd` entry preceding the split point in the source segment.
/// A value of `0` means the split happened before any turn completed
/// (e.g. immediately after `SegmentStart`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentOrigin {
    pub segment_id: crate::SegmentId,
    pub at_turn_index: usize,
}

/// State collected from log entries.
#[derive(Debug, Clone)]
pub struct RestoredState {
    /// Session the replayed segment belongs to. Sourced from the
    /// `SegmentStart` entry; `None` only if the log was empty (in which
    /// case `entries_count == 0`).
    pub session_id: Option<crate::SessionId>,
    pub system_prompt: Option<String>,
    pub config: RequestConfig,
    pub history: Vec<Item>,
    pub turn_count: usize,
    pub last_run_interrupted: bool,
    /// Number of entries replayed. `0` means the segment log was empty.
    /// Writers track their own append count via the same counter so
    /// `ensure_head_or_fork` can compare it with the on-disk count.
    pub entries_count: usize,
    /// LLM リクエストごとの Usage スナップショット時系列。
    /// `LogEntry::LlmUsage` を replay して時系列順に積まれる。
    /// 任意位置のトークン数推定に使う。
    pub usage_history: Vec<UsageRecord>,
    /// `LogEntry::Extension` を replay 順に積んだもの。`(domain, payload)`。
    /// session-store は domain を不透明扱いし、各ドメインが自前で fold する。
    pub extensions: Vec<(String, serde_json::Value)>,
    /// User submissions in original typed form, in submit order.
    /// One entry per `LogEntry::UserInput`; the K-th entry corresponds to
    /// the K-th `Item::user_message` derived during replay (modulo
    /// pre-compaction history seeded via `SegmentStart.history`, whose
    /// original segments are not preserved). Used by clients to re-render
    /// typed atoms (paste chips, refs) on segment restore.
    pub user_segments: Vec<Vec<Segment>>,
}

/// Replay a sequence of log entries to reconstruct worker state.
pub fn collect_state(entries: &[LogEntry]) -> RestoredState {
    let mut state = RestoredState {
        session_id: None,
        system_prompt: None,
        config: RequestConfig::default(),
        history: Vec::new(),
        turn_count: 0,
        last_run_interrupted: false,
        entries_count: 0,
        usage_history: Vec::new(),
        extensions: Vec::new(),
        user_segments: Vec::new(),
    };

    for entry in entries {
        state.entries_count += 1;

        match entry {
            LogEntry::SegmentStart {
                session_id,
                system_prompt,
                config,
                history,
                ..
            } => {
                state.session_id = Some(*session_id);
                state.system_prompt = system_prompt.clone();
                state.config = config.clone();
                state.history = history.iter().cloned().map(Item::from).collect();
            }
            LogEntry::Invoke { .. } => {
                // Marker only; no state mutation. The trailing
                // UserInput / SystemItem / TurnEnd entries carry all
                // replay-relevant data.
            }
            LogEntry::UserInput { segments, .. } => {
                let text = Segment::flatten_to_text(segments);
                state.history.push(Item::user_message(text));
                state.user_segments.push(segments.clone());
            }
            LogEntry::AssistantItem { item, .. } => {
                state.history.push(Item::from(item.clone()));
            }
            LogEntry::ToolResult { item, .. } => {
                state.history.push(Item::from(item.clone()));
            }
            LogEntry::SystemItem { item, .. } => {
                state.history.push(item.to_history_item());
            }
            LogEntry::TurnEnd { turn_count, .. } => {
                state.turn_count = *turn_count;
            }
            LogEntry::RunCompleted { interrupted, .. } => {
                state.last_run_interrupted = *interrupted;
            }
            LogEntry::RunErrored { interrupted, .. } => {
                state.last_run_interrupted = *interrupted;
            }
            LogEntry::PausedTurnAbandoned { .. } => {
                state.last_run_interrupted = false;
            }
            LogEntry::ConfigChanged { config, .. } => {
                state.config = config.clone();
            }
            LogEntry::LlmUsage {
                history_len,
                input_total_tokens,
                cache_read_tokens,
                cache_write_tokens,
                output_tokens,
                ..
            } => {
                state.usage_history.push(UsageRecord {
                    history_len: *history_len,
                    input_total_tokens: *input_total_tokens,
                    cache_read_tokens: *cache_read_tokens,
                    cache_write_tokens: *cache_write_tokens,
                    output_tokens: *output_tokens,
                });
            }
            LogEntry::Extension {
                domain, payload, ..
            } => {
                state.extensions.push((domain.clone(), payload.clone()));
            }
        }
    }

    state
}

/// Get the current timestamp in milliseconds since Unix epoch.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_empty() {
        let state = collect_state(&[]);
        assert!(state.history.is_empty());
        assert_eq!(state.turn_count, 0);
        assert_eq!(state.entries_count, 0);
    }

    #[test]
    fn replay_segment_start_sets_initial_state() {
        let state = collect_state(&[LogEntry::SegmentStart {
            ts: 1000,
            session_id: uuid::Uuid::nil(),
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![Item::user_message("seed").into()],
            forked_from: None,
            compacted_from: None,
        }]);
        assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
        assert_eq!(state.config.max_tokens, Some(1024));
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.entries_count, 1);
    }

    #[test]
    fn replay_full_turn() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::UserInput {
                ts: 2000,
                segments: vec![Segment::text("Hello")],
            },
            LogEntry::AssistantItem {
                ts: 3000,
                item: Item::assistant_message("Hi!").into(),
            },
            LogEntry::TurnEnd {
                ts: 3100,
                turn_count: 1,
            },
            LogEntry::RunCompleted {
                ts: 3200,
                interrupted: false,
                result: WorkerResult::Finished,
            },
        ]);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.turn_count, 1);
        assert!(!state.last_run_interrupted);
    }

    #[test]
    fn replay_with_tool_calls() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::UserInput {
                ts: 2000,
                segments: vec![Segment::text("Check weather")],
            },
            LogEntry::AssistantItem {
                ts: 3000,
                item: Item::tool_call("call_1", "get_weather", r#"{"city":"Tokyo"}"#).into(),
            },
            LogEntry::ToolResult {
                ts: 3500,
                item: Item::tool_result("call_1", "Sunny, 25C").into(),
            },
            LogEntry::AssistantItem {
                ts: 4000,
                item: Item::assistant_message("It's sunny in Tokyo!").into(),
            },
            LogEntry::TurnEnd {
                ts: 4100,
                turn_count: 1,
            },
        ]);
        assert_eq!(state.history.len(), 4);
        assert!(state.history[1].is_tool_call());
        assert!(state.history[2].is_tool_result());
    }

    #[test]
    fn replay_config_changed() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::ConfigChanged {
                ts: 2000,
                config: RequestConfig::default().with_temperature(0.5),
            },
        ]);
        assert_eq!(state.config.temperature, Some(0.5));
    }

    #[test]
    fn replay_llm_usage_appends_to_usage_history() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::UserInput {
                ts: 2000,
                segments: vec![Segment::text("hi")],
            },
            LogEntry::LlmUsage {
                ts: 2100,
                history_len: 1,
                input_total_tokens: 50,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 10,
            },
            LogEntry::AssistantItem {
                ts: 2200,
                item: Item::assistant_message("yo").into(),
            },
            LogEntry::LlmUsage {
                ts: 3100,
                history_len: 2,
                input_total_tokens: 65,
                cache_read_tokens: 50,
                cache_write_tokens: 0,
                output_tokens: 5,
            },
        ]);
        // history は LlmUsage で変化しない
        assert_eq!(state.history.len(), 2);
        // usage_history は時系列順
        assert_eq!(state.usage_history.len(), 2);
        assert_eq!(state.usage_history[0].history_len, 1);
        assert_eq!(state.usage_history[0].input_total_tokens, 50);
        assert_eq!(state.usage_history[1].history_len, 2);
        assert_eq!(state.usage_history[1].cache_read_tokens, 50);
    }

    #[test]
    fn replay_without_llm_usage_keeps_usage_history_empty() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::UserInput {
                ts: 2000,
                segments: vec![Segment::text("hi")],
            },
        ]);
        assert!(state.usage_history.is_empty());
    }

    #[test]
    fn llm_usage_entry_round_trip_via_json() {
        let entry = LogEntry::LlmUsage {
            ts: 12345,
            history_len: 7,
            input_total_tokens: 1000,
            cache_read_tokens: 800,
            cache_write_tokens: 100,
            output_tokens: 42,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        match parsed {
            LogEntry::LlmUsage {
                ts,
                history_len,
                input_total_tokens,
                cache_read_tokens,
                cache_write_tokens,
                output_tokens,
            } => {
                assert_eq!(ts, 12345);
                assert_eq!(history_len, 7);
                assert_eq!(input_total_tokens, 1000);
                assert_eq!(cache_read_tokens, 800);
                assert_eq!(cache_write_tokens, 100);
                assert_eq!(output_tokens, 42);
            }
            other => panic!("expected LlmUsage, got {:?}", other),
        }
    }

    #[test]
    fn invoke_entry_round_trip_via_json() {
        let entry = LogEntry::Invoke {
            ts: 12345,
            trigger: InvokeKind::UserSend,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["kind"], "invoke");
        assert_eq!(parsed["trigger"], "user_send");
        let decoded: LogEntry = serde_json::from_str(&json).unwrap();
        match decoded {
            LogEntry::Invoke { ts, trigger } => {
                assert_eq!(ts, 12345);
                assert_eq!(trigger, InvokeKind::UserSend);
            }
            other => panic!("expected Invoke, got {other:?}"),
        }
    }

    #[test]
    fn replay_invoke_marker_does_not_mutate_state() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 0,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::Invoke {
                ts: 100,
                trigger: InvokeKind::UserSend,
            },
            LogEntry::UserInput {
                ts: 101,
                segments: vec![Segment::text("hi")],
            },
            LogEntry::TurnEnd {
                ts: 200,
                turn_count: 1,
            },
            LogEntry::Invoke {
                ts: 300,
                trigger: InvokeKind::Notify,
            },
        ]);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.turn_count, 1);
    }

    #[test]
    fn replay_paused_turn_abandoned_clears_interrupted_marker() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 0,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::RunCompleted {
                ts: 100,
                interrupted: true,
                result: WorkerResult::Paused,
            },
            LogEntry::PausedTurnAbandoned { ts: 200 },
        ]);
        assert!(!state.last_run_interrupted);
    }

    #[test]
    fn paused_turn_abandoned_entry_round_trip_via_json() {
        let entry = LogEntry::PausedTurnAbandoned { ts: 12345 };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["kind"], "paused_turn_abandoned");
        let decoded: LogEntry = serde_json::from_str(&json).unwrap();
        match decoded {
            LogEntry::PausedTurnAbandoned { ts } => assert_eq!(ts, 12345),
            other => panic!("expected PausedTurnAbandoned, got {other:?}"),
        }
    }

    #[test]
    fn replay_extension_collects_domain_payload_pairs() {
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::Extension {
                ts: 2000,
                domain: "memory.extract".to_string(),
                payload: serde_json::json!({ "processed_through_entry": 7 }),
            },
            LogEntry::Extension {
                ts: 3000,
                domain: "memory.extract".to_string(),
                payload: serde_json::json!({ "processed_through_entry": 12 }),
            },
            LogEntry::Extension {
                ts: 4000,
                domain: "other.domain".to_string(),
                payload: serde_json::json!({ "x": 1 }),
            },
        ]);
        // 順序保持で全件積まれる。fold は呼び出し側の責務。
        assert_eq!(state.extensions.len(), 3);
        assert_eq!(state.extensions[0].0, "memory.extract");
        assert_eq!(state.extensions[1].1["processed_through_entry"], 12);
        assert_eq!(state.extensions[2].0, "other.domain");
    }

    #[test]
    fn extension_entry_round_trip_via_json() {
        let entry = LogEntry::Extension {
            ts: 9999,
            domain: "memory.extract".to_string(),
            payload: serde_json::json!({ "a": 1, "b": "two" }),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        match parsed {
            LogEntry::Extension {
                ts,
                domain,
                payload,
            } => {
                assert_eq!(ts, 9999);
                assert_eq!(domain, "memory.extract");
                assert_eq!(payload["a"], 1);
                assert_eq!(payload["b"], "two");
            }
            other => panic!("expected Extension, got {:?}", other),
        }
    }

    /// Mixed segments survive a JSON round-trip through `LogEntry::UserInput`,
    /// and `collect_state` derives `Item::user_message` from the flattened
    /// text while preserving the original segments separately. This covers
    /// the segments → flatten → Item replay path from the ticket.
    #[test]
    fn replay_user_input_segments_round_trip() {
        let segments = vec![
            Segment::Text {
                content: "see ".into(),
            },
            Segment::Paste {
                id: 1,
                chars: 12,
                lines: 2,
                content: "line1\nline2".into(),
            },
            Segment::FileRef {
                path: "src/main.rs".into(),
            },
        ];
        let entry = LogEntry::UserInput {
            ts: 4242,
            segments: segments.clone(),
        };
        // JSON round-trip preserves the variant byte-for-byte.
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        let state = collect_state(&[
            LogEntry::SegmentStart {
                ts: 1,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            parsed,
        ]);
        // Worker history gets a flattened user_message item.
        assert_eq!(state.history.len(), 1);
        match &state.history[0] {
            Item::Message { role, content, .. } => {
                assert!(matches!(role, llm_worker::Role::User));
                assert_eq!(content.len(), 1);
                match &content[0] {
                    llm_worker::ContentPart::Text { text } => {
                        assert_eq!(text, "see line1\nline2@src/main.rs");
                    }
                    other => panic!("unexpected content: {other:?}"),
                }
            }
            other => panic!("unexpected variant: {other:?}"),
        }
        // Segments survive verbatim for client-side restore.
        assert_eq!(state.user_segments.len(), 1);
        assert_eq!(state.user_segments[0].len(), 3);
        match &state.user_segments[0][1] {
            Segment::Paste {
                id,
                chars,
                lines,
                content,
            } => {
                assert_eq!(*id, 1);
                assert_eq!(*chars, 12);
                assert_eq!(*lines, 2);
                assert_eq!(content, "line1\nline2");
            }
            other => panic!("expected Paste, got {other:?}"),
        }
    }
}
