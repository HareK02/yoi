//! Segment log types for append-only JSONL persistence.
//!
//! Each [`LogEntry`] represents a single state transition within one
//! segment, serialized as one line in a `.jsonl` file. Reading all
//! entries and collecting them via [`collect_state`] reconstructs the
//! full [`Engine`] state at that segment.
//!
//! The on-disk format is one `LogEntry` per line — entries are positionally
//! ordered. Fork lineage references between segments use turn-number indices
//! (`SegmentOrigin.at_turn_index`) rather than per-entry hashes.

use agen::llm_client::types::{Item, RequestConfig};
use agen::{EngineResult, UsageRecord};
use protocol::{InvokeKind, Segment};
use serde::{Deserialize, Serialize};

use crate::history::{LoggedHistoryEntry, LoggedSystemHistoryEntry};
use crate::logged_item::LoggedItem;

/// A single segment log entry, serialized as one JSONL line.
///
/// Variants correspond to specific mutation points in `Engine`:
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionExtension {
    pub domain: String,
    pub payload: serde_json::Value,
}

impl SessionExtension {
    pub fn new(domain: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            domain: domain.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEntry {
    /// Canonical segment seed. Retained entries keep their stable logical
    /// identity and origin across fork/compaction/restore.
    AnnotatedSegmentStart {
        ts: u64,
        session_id: crate::SessionId,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<LoggedHistoryEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forked_from: Option<SegmentOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacted_from: Option<SegmentOrigin>,
    },

    /// IDLE → active marker. Records the start of a new self-driving
    /// cycle (Invoke range). The range extends implicitly until the
    /// next `Invoke` entry; this entry carries the trigger only — the
    /// actual payload (user text / notify message / worker event body) is
    /// in the immediately following Turn entry (`UserInput` / `SystemItem`).
    ///
    /// Used by `worker-session-fork` style operations: the fork-point seq
    /// (`at_turn_index` in persistence-semantics) points at one of these
    /// `Invoke` entries so "back to N-th send" maps cleanly to the
    /// IDLE-break boundary the user sees.
    ///
    /// Field name is `trigger` (not `kind`) because the LogEntry
    /// serde tag already occupies `"kind"`.
    ///
    /// Replay marks the run interrupted until a terminal `RunCompleted`,
    /// `RunErrored`, or `PausedTurnAbandoned` entry proves how it ended. This
    /// makes a process/disk failure between Invoke and its terminal record
    /// restore conservatively instead of re-running a dangling tool call.
    Invoke { ts: u64, trigger: InvokeKind },

    /// Canonical user submission with its exact model-visible entries. Typed
    /// Flow instructions and caller-attributed input remain separate entries.
    AnnotatedUserInput {
        ts: u64,
        segments: Vec<Segment>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<SessionExtension>,
        history: Vec<LoggedHistoryEntry>,
    },

    /// Canonical model output and metadata committed as one journal record.
    AnnotatedAssistantItem { ts: u64, entry: LoggedHistoryEntry },

    /// Canonical tool output and metadata committed as one journal record.
    AnnotatedToolResult { ts: u64, entry: LoggedHistoryEntry },

    /// Canonical typed system event and model-visible metadata committed
    /// together.
    AnnotatedSystemItem {
        ts: u64,
        entry: LoggedSystemHistoryEntry,
    },

    /// Turn boundary. Records the turn count after increment.
    TurnEnd { ts: u64, turn_count: usize },

    /// `run()` / `resume()` が `EngineResult` で正常終了した。
    /// Replay restores both interruption state and any resumable logical-run
    /// turn budget.
    RunCompleted {
        ts: u64,
        interrupted: bool,
        result: EngineResult,
        /// AgentTurns consumed by a paused/yielded logical run. Terminal
        /// outcomes persist `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_run_turn_count: Option<usize>,
    },

    /// `run()` / `resume()` が `EngineError` で終了した。
    /// `EngineError` は `Serialize` 不可なので `message` のみ lossy 保持する。
    /// Audit-only metadata: replay は `interrupted` のみ反映する。
    RunErrored {
        ts: u64,
        interrupted: bool,
        message: String,
    },

    /// Restores an active logical-run budget at a segment boundary, notably
    /// after compaction replaced the segment that held the original Invoke and
    /// RunCompleted entries.
    ActiveRunCheckpoint {
        ts: u64,
        active_turn_count: usize,
        total_turn_count: usize,
    },

    /// A paused interrupted turn was explicitly abandoned without calling
    /// `run()` or `resume()` again. Replay clears the interrupted marker so
    /// the restored Worker is idle and future user input starts a normal new turn.
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
    /// Canonical persisted history with stable identity and provenance. This is
    /// the authority for rewrites, forks, and annotated restore; `history` is
    /// retained as the model-facing item projection.
    pub annotated_history: Vec<LoggedHistoryEntry>,
    pub turn_count: usize,
    /// AgentTurns consumed by the active paused/yielded logical run.
    pub active_run_turn_count: Option<usize>,
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
    /// One entry per `LogEntry::AnnotatedUserInput`; the K-th entry corresponds to
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
        annotated_history: Vec::new(),
        turn_count: 0,
        active_run_turn_count: None,
        last_run_interrupted: false,
        entries_count: 0,
        usage_history: Vec::new(),
        extensions: Vec::new(),
        user_segments: Vec::new(),
    };

    for entry in entries {
        state.entries_count += 1;

        match entry {
            LogEntry::AnnotatedSegmentStart {
                session_id,
                system_prompt,
                config,
                history,
                ..
            } => {
                state.session_id = Some(*session_id);
                state.system_prompt = system_prompt.clone();
                state.config = config.clone();
                state.annotated_history = history.clone();
                state.history = history
                    .iter()
                    .cloned()
                    .map(|entry| Item::from(entry.item))
                    .collect();
            }
            LogEntry::Invoke { .. } => {
                // A terminal run record below clears or refines this. If the
                // log ends first, restore must treat the turn as interrupted.
                state.last_run_interrupted = true;
                state.active_run_turn_count = Some(0);
            }
            LogEntry::AnnotatedUserInput {
                segments,
                extensions,
                history,
                ..
            } => {
                state.annotated_history.extend(history.iter().cloned());
                state
                    .history
                    .extend(history.iter().cloned().map(|entry| Item::from(entry.item)));
                state.user_segments.push(segments.clone());
                state.extensions.extend(
                    extensions
                        .iter()
                        .map(|extension| (extension.domain.clone(), extension.payload.clone())),
                );
            }
            LogEntry::AnnotatedAssistantItem { entry, .. }
            | LogEntry::AnnotatedToolResult { entry, .. } => {
                state.annotated_history.push(entry.clone());
                state.history.push(Item::from(entry.item.clone()));
            }
            LogEntry::AnnotatedSystemItem { entry, .. } => {
                state.annotated_history.push(LoggedHistoryEntry {
                    item: LoggedItem::from(entry.item.to_history_item()),
                    metadata: entry.metadata.clone(),
                });
                state.history.push(entry.item.to_history_item());
            }
            LogEntry::TurnEnd { turn_count, .. } => {
                if let Some(active_turn_count) = &mut state.active_run_turn_count {
                    *active_turn_count += turn_count.saturating_sub(state.turn_count);
                }
                state.turn_count = *turn_count;
            }
            LogEntry::RunCompleted {
                interrupted,
                result,
                active_run_turn_count,
                ..
            } => {
                state.last_run_interrupted = *interrupted;
                if *interrupted && matches!(result, EngineResult::Paused | EngineResult::Yielded) {
                    // Legacy entries omit the explicit field; retain the
                    // Invoke/TurnEnd-derived count in that case.
                    if let Some(turn_count) = active_run_turn_count {
                        state.active_run_turn_count = Some(*turn_count);
                    }
                } else {
                    state.active_run_turn_count = None;
                }
            }
            LogEntry::RunErrored { interrupted, .. } => {
                state.last_run_interrupted = *interrupted;
                state.active_run_turn_count = None;
            }
            LogEntry::ActiveRunCheckpoint {
                active_turn_count,
                total_turn_count,
                ..
            } => {
                state.active_run_turn_count = Some(*active_turn_count);
                state.turn_count = *total_turn_count;
                state.last_run_interrupted = true;
            }
            LogEntry::PausedTurnAbandoned { .. } => {
                state.last_run_interrupted = false;
                state.active_run_turn_count = None;
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
    use crate::{
        LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
    };

    fn annotated(item: Item) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item: LoggedItem::from(item),
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId::new(),
                origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        }
    }

    #[test]
    fn replay_empty() {
        let state = collect_state(&[]);
        assert!(state.history.is_empty());
        assert_eq!(state.turn_count, 0);
        assert_eq!(state.entries_count, 0);
    }

    #[test]
    fn replay_segment_start_sets_initial_state() {
        let state = collect_state(&[LogEntry::AnnotatedSegmentStart {
            ts: 1000,
            session_id: uuid::Uuid::nil(),
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![annotated(Item::user_message("seed"))],
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
            LogEntry::AnnotatedSegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::AnnotatedUserInput {
                ts: 2000,
                extensions: vec![],
                segments: vec![Segment::text("Hello")],
                history: vec![annotated(Item::user_message("Hello"))],
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 3000,
                entry: annotated(Item::assistant_message("Hi!")),
            },
            LogEntry::TurnEnd {
                ts: 3100,
                turn_count: 1,
            },
            LogEntry::RunCompleted {
                ts: 3200,
                interrupted: false,
                result: EngineResult::Finished,
                active_run_turn_count: None,
            },
        ]);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.turn_count, 1);
        assert!(!state.last_run_interrupted);
    }

    #[test]
    fn replay_incomplete_invoke_is_interrupted() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::Invoke {
                ts: 2000,
                trigger: InvokeKind::UserSend,
            },
            LogEntry::AnnotatedUserInput {
                ts: 2001,
                extensions: vec![],
                segments: vec![Segment::text("run a tool")],
                history: vec![annotated(Item::user_message("run a tool"))],
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 3000,
                entry: annotated(Item::tool_call("call_1", "side_effect", "{}")),
            },
        ]);

        assert!(state.last_run_interrupted);
    }

    #[test]
    fn replay_with_tool_calls() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::AnnotatedUserInput {
                ts: 2000,
                extensions: vec![],
                segments: vec![Segment::text("Check weather")],
                history: vec![annotated(Item::user_message("Check weather"))],
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 3000,
                entry: annotated(Item::tool_call(
                    "call_1",
                    "get_weather",
                    r#"{"city":"Tokyo"}"#,
                )),
            },
            LogEntry::AnnotatedToolResult {
                ts: 3500,
                entry: annotated(Item::tool_result("call_1", "Sunny, 25C")),
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 4000,
                entry: annotated(Item::assistant_message("It's sunny in Tokyo!")),
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
    fn replay_restores_durable_tool_image_detail() {
        let entry = LogEntry::AnnotatedToolResult {
            ts: 3500,
            entry: annotated(Item::tool_result_item_with_attachments(
                "call_image",
                "attached",
                None,
                false,
                vec![agen::tool::Attachment::Image(
                    agen::tool::ImageAttachment::new("image/png", b"durable-image".to_vec()),
                )],
            )),
        };
        let persisted = serde_json::to_string(&entry).unwrap();
        let restored_entry: LogEntry = serde_json::from_str(&persisted).unwrap();
        let state = collect_state(&[restored_entry]);

        assert!(matches!(
            &state.history[0],
            Item::ToolResult { attachments, .. }
                if matches!(
                    attachments.as_slice(),
                    [agen::tool::Attachment::Image(image)]
                        if image.data() == b"durable-image"
                )
        ));
    }

    #[test]
    fn replay_config_changed() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
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
            LogEntry::AnnotatedSegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::AnnotatedUserInput {
                ts: 2000,
                extensions: vec![],
                segments: vec![Segment::text("hi")],
                history: vec![annotated(Item::user_message("hi"))],
            },
            LogEntry::LlmUsage {
                ts: 2100,
                history_len: 1,
                input_total_tokens: 50,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 10,
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 2200,
                entry: annotated(Item::assistant_message("yo")),
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
            LogEntry::AnnotatedSegmentStart {
                ts: 1000,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::AnnotatedUserInput {
                ts: 2000,
                extensions: vec![],
                segments: vec![Segment::text("hi")],
                history: vec![annotated(Item::user_message("hi"))],
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
    fn replay_invoke_marker_only_mutates_interrupted_state() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
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
            LogEntry::AnnotatedUserInput {
                ts: 101,
                extensions: vec![],
                segments: vec![Segment::text("hi")],
                history: vec![annotated(Item::user_message("hi"))],
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
        assert!(state.last_run_interrupted);
    }

    #[test]
    fn replay_paused_turn_abandoned_clears_interrupted_marker() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
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
                result: EngineResult::Paused,
                active_run_turn_count: Some(1),
            },
            LogEntry::PausedTurnAbandoned { ts: 200 },
        ]);
        assert!(!state.last_run_interrupted);
        assert_eq!(state.active_run_turn_count, None);
    }

    #[test]
    fn replay_restores_active_run_budget_across_compaction_checkpoint() {
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
                ts: 0,
                session_id: uuid::Uuid::nil(),
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::ActiveRunCheckpoint {
                ts: 100,
                active_turn_count: 3,
                total_turn_count: 9,
            },
        ]);

        assert_eq!(state.turn_count, 9);
        assert_eq!(state.active_run_turn_count, Some(3));
        assert!(state.last_run_interrupted);
    }

    #[test]
    fn legacy_interrupted_run_derives_budget_from_invoke_and_turn_end() {
        let entry: LogEntry = serde_json::from_value(serde_json::json!({
            "kind": "run_completed",
            "ts": 300,
            "interrupted": true,
            "result": "paused"
        }))
        .expect("legacy run-completed entry");
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
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
            LogEntry::TurnEnd {
                ts: 200,
                turn_count: 2,
            },
            entry,
        ]);

        assert_eq!(state.active_run_turn_count, Some(2));
        assert!(state.last_run_interrupted);
    }

    #[test]
    fn non_resumable_interruption_clears_the_active_run_budget() {
        let state = collect_state(&[
            LogEntry::Invoke {
                ts: 100,
                trigger: InvokeKind::UserSend,
            },
            LogEntry::TurnEnd {
                ts: 200,
                turn_count: 2,
            },
            LogEntry::RunCompleted {
                ts: 300,
                interrupted: true,
                result: EngineResult::LimitReached,
                active_run_turn_count: None,
            },
        ]);

        assert!(state.last_run_interrupted);
        assert_eq!(state.active_run_turn_count, None);
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
            LogEntry::AnnotatedSegmentStart {
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

    #[test]
    fn user_input_extensions_restore_with_the_same_committed_input() {
        let segments = vec![Segment::text("Flow instructions"), Segment::text("Ticket")];
        let entry = LogEntry::AnnotatedUserInput {
            ts: 9999,
            segments: segments.clone(),
            history: vec![annotated(Item::user_message(Segment::flatten_to_text(
                &segments,
            )))],
            extensions: vec![SessionExtension::new(
                "flow.runtime.v1",
                serde_json::json!({ "state": "implement", "revision": 0 }),
            )],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: LogEntry = serde_json::from_str(&json).unwrap();
        let state = collect_state(&[decoded]);
        assert_eq!(state.user_segments, vec![segments]);
        assert_eq!(state.extensions.len(), 1);
        assert_eq!(state.extensions[0].0, "flow.runtime.v1");
        assert_eq!(state.extensions[0].1["state"], "implement");
    }

    /// Mixed segments survive a JSON round-trip through `LogEntry::AnnotatedUserInput`,
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
        let entry = LogEntry::AnnotatedUserInput {
            ts: 4242,
            extensions: vec![],
            segments: segments.clone(),
            history: vec![annotated(Item::user_message(Segment::flatten_to_text(
                &segments,
            )))],
        };
        // JSON round-trip preserves the variant byte-for-byte.
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        let state = collect_state(&[
            LogEntry::AnnotatedSegmentStart {
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
        // Engine history gets a flattened user_message item.
        assert_eq!(state.history.len(), 1);
        match &state.history[0] {
            Item::Message { role, content, .. } => {
                assert!(matches!(role, agen::Role::User));
                assert_eq!(content.len(), 1);
                match &content[0] {
                    agen::ContentPart::Text { text } => {
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
