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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
/// - `UserInput` / `AssistantItems` / `ToolResults` / `HookInjectedItems` — history appends
/// - `TurnEnd` — turn boundary marker
/// - `Locked` / `CacheUnlocked` — KV cache state transitions
/// - `RunOutcome` — marks end of a `run()` or `resume()` call
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
        history: Vec<Item>,
        /// Origin: forked from another session at a specific entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forked_from: Option<SessionOrigin>,
        /// Origin: compacted from another session at a specific entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacted_from: Option<SessionOrigin>,
    },

    /// User input pushed to history (worker.rs:229).
    UserInput { ts: u64, item: Item },

    /// Assistant response items added to history (worker.rs:1040-1041).
    AssistantItems { ts: u64, items: Vec<Item> },

    /// Tool execution results added to history (worker.rs:897-900, 1072-1076).
    ToolResults { ts: u64, items: Vec<Item> },

    /// Items injected by `on_turn_end` hook via `ContinueWithMessages` (worker.rs:1055).
    HookInjectedItems { ts: u64, items: Vec<Item> },

    /// Turn boundary. Records the turn count after increment.
    TurnEnd { ts: u64, turn_count: usize },

    /// KV cache locked. Records the history prefix length that is now immutable.
    #[serde(alias = "cache_locked")]
    Locked { ts: u64, locked_prefix_len: usize },

    /// KV cache unlocked.
    CacheUnlocked { ts: u64 },

    /// Outcome of a `run()` or `resume()` call.
    /// This is metadata for auditing; state collection does not branch on the outcome.
    RunOutcome {
        ts: u64,
        outcome: Outcome,
        interrupted: bool,
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
}

/// Provenance reference to a parent session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionOrigin {
    /// Session ID of the source session.
    pub session_id: crate::SessionId,
    /// Hash of the entry in the source session at the point of fork/compact.
    pub at_hash: EntryHash,
}

/// Outcome of a run/resume call. Metadata for auditing only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Finished,
    Paused,
    LimitReached,
    /// Worker yielded control to the caller for external processing.
    /// Distinct from `Paused`: caller handles internally and resumes.
    Yielded,
    Error { message: String },
}

/// State collected from log entries.
#[derive(Debug, Clone)]
pub struct RestoredState {
    pub system_prompt: Option<String>,
    pub config: RequestConfig,
    pub history: Vec<Item>,
    pub turn_count: usize,
    pub locked_prefix_len: usize,
    pub last_run_interrupted: bool,
    /// Hash of the last entry in the chain (None if empty).
    pub head_hash: Option<EntryHash>,
    /// LLM リクエストごとの Usage スナップショット時系列。
    /// `LogEntry::LlmUsage` を replay して時系列順に積まれる。
    /// 任意位置のトークン数推定に使う。
    pub usage_history: Vec<UsageRecord>,
}

/// LLM リクエスト送信時点での占有量スナップショット。
///
/// `LogEntry::LlmUsage` の replay 時に `RestoredState.usage_history` に積まれる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// 送信時の history.len()
    pub history_len: usize,
    /// history[..history_len] の占有量（プロンプト全長、実測）
    pub input_total_tokens: u64,
    /// 上記のうちキャッシュから読み出された分
    pub cache_read_tokens: u64,
    /// 上記のうちこのリクエストでキャッシュに書かれた分
    pub cache_write_tokens: u64,
    /// このリクエストで生成された出力トークン数
    pub output_tokens: u64,
}

/// Replay a sequence of hashed entries to reconstruct worker state.
pub fn collect_state(entries: &[HashedEntry]) -> RestoredState {
    let mut state = RestoredState {
        system_prompt: None,
        config: RequestConfig::default(),
        history: Vec::new(),
        turn_count: 0,
        locked_prefix_len: 0,
        last_run_interrupted: false,
        head_hash: None,
        usage_history: Vec::new(),
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
                state.history = history.clone();
            }
            LogEntry::UserInput { item, .. } => {
                state.history.push(item.clone());
            }
            LogEntry::AssistantItems { items, .. } => {
                state.history.extend(items.iter().cloned());
            }
            LogEntry::ToolResults { items, .. } => {
                state.history.extend(items.iter().cloned());
            }
            LogEntry::HookInjectedItems { items, .. } => {
                state.history.extend(items.iter().cloned());
            }
            LogEntry::TurnEnd { turn_count, .. } => {
                state.turn_count = *turn_count;
            }
            LogEntry::Locked {
                locked_prefix_len, ..
            } => {
                state.locked_prefix_len = *locked_prefix_len;
            }
            LogEntry::CacheUnlocked { .. } => {
                state.locked_prefix_len = 0;
            }
            LogEntry::RunOutcome { interrupted, .. } => {
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
        assert_eq!(state.locked_prefix_len, 0);
        assert!(state.head_hash.is_none());
    }

    #[test]
    fn replay_session_start_sets_initial_state() {
        let entries = build_chain(&[LogEntry::SessionStart {
            ts: 1000,
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![Item::user_message("seed")],
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
                item: Item::user_message("Hello"),
            },
            LogEntry::AssistantItems {
                ts: 3000,
                items: vec![Item::assistant_message("Hi!")],
            },
            LogEntry::TurnEnd {
                ts: 3100,
                turn_count: 1,
            },
            LogEntry::RunOutcome {
                ts: 3200,
                outcome: Outcome::Finished,
                interrupted: false,
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
                item: Item::user_message("Check weather"),
            },
            LogEntry::AssistantItems {
                ts: 3000,
                items: vec![Item::tool_call("call_1", "get_weather", r#"{"city":"Tokyo"}"#)],
            },
            LogEntry::ToolResults {
                ts: 3500,
                items: vec![Item::tool_result("call_1", "Sunny, 25C")],
            },
            LogEntry::AssistantItems {
                ts: 4000,
                items: vec![Item::assistant_message("It's sunny in Tokyo!")],
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
    fn replay_cache_lock_unlock() {
        let entries = build_chain(&[
            LogEntry::SessionStart {
                ts: 1000,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![Item::user_message("a"), Item::assistant_message("b")],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::Locked {
                ts: 2000,
                locked_prefix_len: 2,
            },
            LogEntry::CacheUnlocked { ts: 3000 },
        ]);
        let state = collect_state(&entries);
        assert_eq!(state.locked_prefix_len, 0);

        // Check locked state before unlock
        let state_locked = collect_state(&entries[..2]);
        assert_eq!(state_locked.locked_prefix_len, 2);
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
                item: Item::user_message("Hello"),
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
            item: Item::user_message("Hello"),
        };
        let entry_b = LogEntry::UserInput {
            ts: 1000,
            item: Item::user_message("World"),
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
                item: Item::user_message("hi"),
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
                items: vec![Item::assistant_message("yo")],
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
                item: Item::user_message("hi"),
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
}
