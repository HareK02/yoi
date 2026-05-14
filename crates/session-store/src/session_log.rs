//! Session log types for append-only JSONL persistence.
//!
//! Each [`LogEntry`] represents a single state transition in a session,
//! serialized as one line in a `.jsonl` file. Reading all entries and
//! collecting them via [`collect_state`] reconstructs the full [`Worker`] state.
//!
//! Entries are chained via [`EntryHash`]: each [`HashedEntry`] records the hash
//! of the previous entry, forming a tamper-evident append-only chain. This
//! enables safe fork detection when multiple writers share a session.

use llm_worker::llm_client::types::{Item, RequestConfig};
use llm_worker::{UsageRecord, WorkerResult};
use protocol::{InvokeKind, ScopeRule, Segment};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::logged_item::LoggedItem;
use crate::system_item::SystemItem;

/// SHA-256 hash identifying a specific log entry in the chain.
///
/// Computed as `sha256(prev_hash_bytes || canonical_json(entry))`.
/// Displayed and serialized as a lowercase hex string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryHash([u8; 32]);

impl EntryHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let mut buf = [0u8; 32];
        hex::decode_to_slice(s, &mut buf)?;
        Ok(Self(buf))
    }
}

impl std::fmt::Display for EntryHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for EntryHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EntryHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Compute the hash for a log entry given its predecessor's hash.
pub fn compute_hash(prev: Option<&EntryHash>, entry: &LogEntry) -> EntryHash {
    let mut hasher = Sha256::new();

    // Feed prev_hash bytes (32 zero bytes if None).
    match prev {
        Some(h) => hasher.update(h.as_bytes()),
        None => hasher.update([0u8; 32]),
    }

    // Canonical JSON of the entry.
    let json = serde_json::to_string(entry).expect("LogEntry serialization cannot fail");
    hasher.update(json.as_bytes());

    EntryHash(hasher.finalize().into())
}

/// A [`LogEntry`] with hash-chain metadata.
///
/// This is the unit persisted to JSONL — one line per `HashedEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashedEntry {
    pub hash: EntryHash,
    pub prev_hash: Option<EntryHash>,
    #[serde(flatten)]
    pub entry: LogEntry,
}

/// A single session log entry, serialized as one JSONL line.
///
/// Variants correspond to specific mutation points in `Worker`:
/// - `SessionStart` — always the first entry; captures initial state
/// - `Invoke` — IDLE → active marker (start of a new self-driving cycle)
/// - `UserInput` / `AssistantItems` / `ToolResults` / `HookInjectedItems` — history appends
/// - `TurnEnd` — AgentTurn boundary marker; carries the post-increment
///   `turn_count`. With retry unimplemented today this fires once per
///   `run()`/`resume()` (current callers persist a single TurnEnd at
///   run completion); the fork-point seq for `at_turn_index` is the
///   preceding `Invoke` entry, not the TurnEnd.
/// - `RunCompleted` / `RunErrored` — marks end of a `run()` or `resume()` call
/// - `ConfigChanged` — `RequestConfig` mutation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEntry {
    /// Session start. Always the first entry in a log.
    /// For forked sessions, `history` contains the seed state from the parent.
    SessionStart {
        ts: u64,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<LoggedItem>,
        /// Origin: forked from another session at a specific entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forked_from: Option<SessionOrigin>,
        /// Origin: compacted from another session at a specific entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacted_from: Option<SessionOrigin>,
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
    /// file/knowledge refs, workflow invocations) on session restore.
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

    /// Legacy plural form: kept **read-only** so old session logs still
    /// open. New writes always use the singular `AssistantItem`. Items
    /// are flattened on replay.
    AssistantItems { ts: u64, items: Vec<LoggedItem> },

    /// Legacy plural form: kept **read-only**. New writes use the
    /// singular `ToolResult`.
    ToolResults { ts: u64, items: Vec<LoggedItem> },

    /// Legacy plural form: kept **read-only**. New writes use the
    /// singular `SystemItem`.
    SystemItems { ts: u64, items: Vec<SystemItem> },

    /// Legacy pre-`SystemItem*` form. Deserialize-only. Items are
    /// flattened to `Item::system_message` on replay.
    HookInjectedItems { ts: u64, items: Vec<LoggedItem> },

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

/// Provenance reference to a parent session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionOrigin {
    /// Session ID of the source session.
    pub session_id: crate::SessionId,
    /// Hash of the entry in the source session at the point of fork/compact.
    pub at_hash: EntryHash,
}

/// Domain used by Pod to persist its latest effective runtime scope.
pub const POD_SCOPE_EXTENSION_DOMAIN: &str = "pod.scope";

/// Payload stored in `LogEntry::Extension { domain: "pod.scope", .. }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodScopeSnapshot {
    pub allow: Vec<ScopeRule>,
    pub deny: Vec<ScopeRule>,
}

/// State collected from log entries.
#[derive(Debug, Clone)]
pub struct RestoredState {
    pub system_prompt: Option<String>,
    pub config: RequestConfig,
    pub history: Vec<Item>,
    pub turn_count: usize,
    pub last_run_interrupted: bool,
    /// Hash of the last entry in the chain (None if empty).
    pub head_hash: Option<EntryHash>,
    /// LLM リクエストごとの Usage スナップショット時系列。
    /// `LogEntry::LlmUsage` を replay して時系列順に積まれる。
    /// 任意位置のトークン数推定に使う。
    pub usage_history: Vec<UsageRecord>,
    /// `LogEntry::Extension` を replay 順に積んだもの。`(domain, payload)`。
    /// session-store は domain を不透明扱いし、各ドメインが自前で fold する。
    pub extensions: Vec<(String, serde_json::Value)>,
    /// Latest runtime scope snapshot persisted by the Pod. `None` means
    /// the session predates scope persistence or the payload was corrupt.
    pub pod_scope: Option<PodScopeSnapshot>,
    /// User submissions in original typed form, in submit order.
    /// One entry per `LogEntry::UserInput`; the K-th entry corresponds to
    /// the K-th `Item::user_message` derived during replay (modulo
    /// pre-compaction history seeded via `SessionStart.history`, whose
    /// original segments are not preserved). Used by clients to re-render
    /// typed atoms (paste chips, refs) on session restore.
    pub user_segments: Vec<Vec<Segment>>,
}

/// Replay a sequence of hashed entries to reconstruct worker state.
pub fn collect_state(entries: &[HashedEntry]) -> RestoredState {
    let mut state = RestoredState {
        system_prompt: None,
        config: RequestConfig::default(),
        history: Vec::new(),
        turn_count: 0,
        last_run_interrupted: false,
        head_hash: None,
        usage_history: Vec::new(),
        extensions: Vec::new(),
        pod_scope: None,
        user_segments: Vec::new(),
    };

    for hashed in entries {
        state.head_hash = Some(hashed.hash.clone());

        match &hashed.entry {
            LogEntry::SessionStart {
                system_prompt,
                config,
                history,
                ..
            } => {
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
            LogEntry::AssistantItems { items, .. } => {
                state.history.extend(items.iter().cloned().map(Item::from));
            }
            LogEntry::ToolResults { items, .. } => {
                state.history.extend(items.iter().cloned().map(Item::from));
            }
            LogEntry::SystemItems { items, .. } => {
                state
                    .history
                    .extend(items.iter().map(|si| si.to_history_item()));
            }
            LogEntry::HookInjectedItems { items, .. } => {
                state.history.extend(items.iter().cloned().map(Item::from));
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
                if domain == POD_SCOPE_EXTENSION_DOMAIN {
                    match serde_json::from_value::<PodScopeSnapshot>(payload.clone()) {
                        Ok(snapshot) => state.pod_scope = Some(snapshot),
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "discarding malformed pod.scope snapshot from session log"
                            );
                        }
                    }
                }
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

/// Build a hash chain from plain `LogEntry` values.
///
/// Useful for tests and for seeding new sessions from a list of entries.
pub fn build_chain(entries: &[LogEntry]) -> Vec<HashedEntry> {
    let mut chain = Vec::with_capacity(entries.len());
    let mut prev: Option<EntryHash> = None;

    for entry in entries {
        let hash = compute_hash(prev.as_ref(), entry);
        chain.push(HashedEntry {
            hash: hash.clone(),
            prev_hash: prev,
            entry: entry.clone(),
        });
        prev = Some(hash);
    }

    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_empty() {
        let state = collect_state(&[]);
        assert!(state.history.is_empty());
        assert_eq!(state.turn_count, 0);
        assert!(state.head_hash.is_none());
    }

    #[test]
    fn replay_session_start_sets_initial_state() {
        let entries = build_chain(&[LogEntry::SessionStart {
            ts: 1000,
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![Item::user_message("seed").into()],
            forked_from: None,
            compacted_from: None,
        }]);
        let state = collect_state(&entries);
        assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
        assert_eq!(state.config.max_tokens, Some(1024));
        assert_eq!(state.history.len(), 1);
        assert!(state.head_hash.is_some());
    }

    #[test]
    fn replay_full_turn() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
            LogEntry::AssistantItems {
                ts: 3000,
                items: vec![Item::assistant_message("Hi!").into()],
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
        let state = collect_state(&entries);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.turn_count, 1);
        assert!(!state.last_run_interrupted);
    }

    #[test]
    fn replay_with_tool_calls() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
            LogEntry::AssistantItems {
                ts: 3000,
                items: vec![Item::tool_call("call_1", "get_weather", r#"{"city":"Tokyo"}"#).into()],
            },
            LogEntry::ToolResults {
                ts: 3500,
                items: vec![Item::tool_result("call_1", "Sunny, 25C").into()],
            },
            LogEntry::AssistantItems {
                ts: 4000,
                items: vec![Item::assistant_message("It's sunny in Tokyo!").into()],
            },
            LogEntry::TurnEnd {
                ts: 4100,
                turn_count: 1,
            },
        ]);
        let state = collect_state(&entries);
        assert_eq!(state.history.len(), 4);
        assert!(state.history[1].is_tool_call());
        assert!(state.history[2].is_tool_result());
    }

    #[test]
    fn replay_config_changed() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
        let state = collect_state(&entries);
        assert_eq!(state.config.temperature, Some(0.5));
    }

    #[test]
    fn hash_chain_is_deterministic() {
        let raw = vec![
            LogEntry::SessionStart {
                ts: 1000,
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
        ];
        let chain_a = build_chain(&raw);
        let chain_b = build_chain(&raw);
        assert_eq!(chain_a[0].hash, chain_b[0].hash);
        assert_eq!(chain_a[1].hash, chain_b[1].hash);
    }

    #[test]
    fn different_content_produces_different_hash() {
        let entry_a = LogEntry::UserInput {
            ts: 1000,
            segments: vec![Segment::text("Hello")],
        };
        let entry_b = LogEntry::UserInput {
            ts: 1000,
            segments: vec![Segment::text("World")],
        };
        let hash_a = compute_hash(None, &entry_a);
        let hash_b = compute_hash(None, &entry_b);
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn replay_llm_usage_appends_to_usage_history() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
            LogEntry::AssistantItems {
                ts: 2200,
                items: vec![Item::assistant_message("yo").into()],
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
        let state = collect_state(&entries);
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
        // 既存ログ互換: LlmUsage entry が無くても collect_state は壊れない
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
        let state = collect_state(&entries);
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
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 0,
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
        let state = collect_state(&entries);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.turn_count, 1);
    }

    #[test]
    fn replay_extension_collects_domain_payload_pairs() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
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
        let state = collect_state(&entries);
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
    fn hash_hex_round_trip() {
        let entry = LogEntry::SessionStart {
            ts: 1000,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        };
        let hash = compute_hash(None, &entry);
        let hex = hash.to_hex();
        let parsed = EntryHash::from_hex(&hex).unwrap();
        assert_eq!(hash, parsed);
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
        // Hash + JSON round-trip preserves the variant byte-for-byte.
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            parsed,
        ]);
        let state = collect_state(&entries);
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
