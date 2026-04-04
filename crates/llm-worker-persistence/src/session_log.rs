//! Session log types for append-only JSONL persistence.
//!
//! Each [`LogEntry`] represents a single state transition in a session,
//! serialized as one line in a `.jsonl` file. Reading all entries and
//! collecting them via [`collect_state`] reconstructs the full [`Worker`] state.

use llm_worker::llm_client::types::{Item, RequestConfig};
use serde::{Deserialize, Serialize};

/// A single session log entry, serialized as one JSONL line.
///
/// Variants correspond to specific mutation points in `Worker`:
/// - `SessionStart` — always the first entry; captures initial state
/// - `UserInput` / `AssistantItems` / `ToolResults` / `HookInjectedItems` — history appends
/// - `TurnEnd` — turn boundary marker
/// - `CacheLocked` / `CacheUnlocked` — KV cache state transitions
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
    CacheLocked { ts: u64, locked_prefix_len: usize },

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
}

/// Outcome of a run/resume call. Metadata for auditing only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Finished,
    Paused,
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
}

/// Replay a sequence of log entries to reconstruct worker state.
pub fn collect_state(entries: &[LogEntry]) -> RestoredState {
    let mut state = RestoredState {
        system_prompt: None,
        config: RequestConfig::default(),
        history: Vec::new(),
        turn_count: 0,
        locked_prefix_len: 0,
        last_run_interrupted: false,
    };

    for entry in entries {
        match entry {
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
            LogEntry::CacheLocked {
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
        assert_eq!(state.locked_prefix_len, 0);
    }

    #[test]
    fn replay_session_start_sets_initial_state() {
        let entries = vec![LogEntry::SessionStart {
            ts: 1000,
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![Item::user_message("seed")],
        }];
        let state = collect_state(&entries);
        assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
        assert_eq!(state.config.max_tokens, Some(1024));
        assert_eq!(state.history.len(), 1);
    }

    #[test]
    fn replay_full_turn() {
        let entries = vec![
            LogEntry::SessionStart {
                ts: 1000,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
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
        ];
        let state = collect_state(&entries);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.turn_count, 1);
        assert!(!state.last_run_interrupted);
    }

    #[test]
    fn replay_with_tool_calls() {
        let entries = vec![
            LogEntry::SessionStart {
                ts: 1000,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
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
        ];
        let state = collect_state(&entries);
        assert_eq!(state.history.len(), 4);
        assert!(state.history[1].is_tool_call());
        assert!(state.history[2].is_tool_result());
    }

    #[test]
    fn replay_cache_lock_unlock() {
        let entries = vec![
            LogEntry::SessionStart {
                ts: 1000,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![Item::user_message("a"), Item::assistant_message("b")],
            },
            LogEntry::CacheLocked {
                ts: 2000,
                locked_prefix_len: 2,
            },
            LogEntry::CacheUnlocked { ts: 3000 },
        ];
        let state = collect_state(&entries);
        assert_eq!(state.locked_prefix_len, 0);

        // Check locked state before unlock
        let state_locked = collect_state(&entries[..2]);
        assert_eq!(state_locked.locked_prefix_len, 2);
    }

    #[test]
    fn replay_config_changed() {
        let entries = vec![
            LogEntry::SessionStart {
                ts: 1000,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![],
            },
            LogEntry::ConfigChanged {
                ts: 2000,
                config: RequestConfig::default().with_temperature(0.5),
            },
        ];
        let state = collect_state(&entries);
        assert_eq!(state.config.temperature, Some(0.5));
    }
}
