//! Free functions for session persistence operations.
//!
//! These functions record and restore session state without owning a Worker.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.

use crate::SessionId;
use crate::logged_item::{LoggedItem, to_logged};
use crate::session_log::{self, LogEntry, PodScopeSnapshot, SessionOrigin};
use crate::store::{Store, StoreError};
use crate::system_item::SystemItem;
use llm_worker::WorkerResult;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::types::Item;
use protocol::Segment;

/// State snapshot for creating a SessionStart entry.
pub struct SessionStartState<'a> {
    pub system_prompt: Option<&'a str>,
    pub config: &'a RequestConfig,
    pub history: &'a [Item],
}

/// Create a new session, writing the initial `SessionStart` entry.
pub fn create_session(
    store: &impl Store,
    state: SessionStartState<'_>,
) -> Result<SessionId, StoreError> {
    let session_id = crate::new_session_id();
    create_session_with_id(store, session_id, state)?;
    Ok(session_id)
}

/// Write a fresh `SessionStart` entry using a pre-generated session ID.
///
/// Used by callers that need to reserve a session ID synchronously but
/// defer the initial log append (e.g. Pod, which resolves a templated
/// system prompt only at first turn).
pub fn create_session_with_id(
    store: &impl Store,
    session_id: SessionId,
    state: SessionStartState<'_>,
) -> Result<(), StoreError> {
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.append(session_id, &entry)
}

/// Create a compacted session from an existing one.
///
/// Records `compacted_from` provenance linking back to the source session
/// at the turn boundary captured by `source_turn_count` (the most recent
/// completed turn in the source).
pub fn create_compacted_session(
    store: &impl Store,
    state: SessionStartState<'_>,
    source_session_id: SessionId,
    source_turn_count: usize,
) -> Result<SessionId, StoreError> {
    let session_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: Some(SessionOrigin {
            session_id: source_session_id,
            at_turn_index: source_turn_count,
        }),
    };
    store.append(session_id, &entry)?;
    Ok(session_id)
}

/// Restore session state from a stored log.
///
/// Returns the reconstructed state. The caller is responsible for
/// applying it to a Worker.
pub fn restore(
    store: &impl Store,
    session_id: SessionId,
) -> Result<crate::session_log::RestoredState, StoreError> {
    let entries = store.read_all(session_id)?;
    Ok(session_log::collect_state(&entries))
}

/// Check if the store's entry count still matches the writer's tally.
/// If not, auto-fork into a new session.
///
/// Updates `session_id` and `entries_written` in place when a fork occurs.
pub fn ensure_head_or_fork(
    store: &impl Store,
    session_id: &mut SessionId,
    entries_written: &mut usize,
    state: SessionStartState<'_>,
) -> Result<(), StoreError> {
    let store_count = store.read_entry_count(*session_id)?;
    if store_count == *entries_written {
        return Ok(());
    }
    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.create_session(fork_id, &[entry])?;
    *session_id = fork_id;
    *entries_written = 1;
    Ok(())
}

/// Log a `UserInput` entry from the original typed `Vec<Segment>`.
///
/// Submit-time entry. Pod calls this at the head of a `Run` turn before
/// the worker pushes its flattened user message into history; replay
/// derives the worker `Item::user_message` from these segments via
/// [`Segment::flatten_to_text`].
pub fn save_user_input(
    store: &impl Store,
    session_id: SessionId,
    segments: Vec<Segment>,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::UserInput {
            ts: session_log::now_millis(),
            segments,
        },
    )
}

/// Log the history delta — new items added since the previous snapshot.
///
/// Classifies items into AssistantItem / ToolResult / HookInjectedItems
/// entries automatically (one entry per item). User messages are skipped
/// because they are persisted upfront via [`save_user_input`] at submit
/// time; the worker pushes a flattened copy into its history that
/// arrives here in `new_items` and would otherwise produce a duplicate
/// `UserInput` entry.
pub fn save_delta(
    store: &impl Store,
    session_id: SessionId,
    new_items: &[Item],
) -> Result<(), StoreError> {
    if new_items.is_empty() {
        return Ok(());
    }

    let ts = session_log::now_millis();
    for item in new_items {
        if item.is_user_message() {
            // Already persisted by save_user_input at submit time.
            continue;
        }
        let entry = classify_history_item(item, ts);
        append_entry(store, session_id, entry)?;
    }
    Ok(())
}

/// Map one history item to its singular `LogEntry` form. Used by the
/// fallback `save_delta` path and the controller's worker-callback
/// classifier so write classification lives in one place.
pub fn classify_history_item(item: &Item, ts: u64) -> LogEntry {
    if item.is_tool_result() {
        LogEntry::ToolResult {
            ts,
            item: LoggedItem::from(item),
        }
    } else if item.is_assistant_message() || item.is_tool_call() || item.is_reasoning() {
        LogEntry::AssistantItem {
            ts,
            item: LoggedItem::from(item),
        }
    } else {
        // Defensive: anything else (future Item kinds) routes through
        // AssistantItem rather than getting silently dropped.
        LogEntry::AssistantItem {
            ts,
            item: LoggedItem::from(item),
        }
    }
}

/// Append a single typed system item as `LogEntry::SystemItem`. Helper
/// for the Pod-side interceptor commit path; mirrors the per-item
/// commit shape used for assistant / tool result entries.
pub fn append_system_item(
    store: &impl Store,
    session_id: SessionId,
    item: SystemItem,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::SystemItem {
            ts: session_log::now_millis(),
            item,
        },
    )
}

/// Log a TurnEnd entry.
pub fn save_turn_end(
    store: &impl Store,
    session_id: SessionId,
    turn_count: usize,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::TurnEnd {
            ts: session_log::now_millis(),
            turn_count,
        },
    )
}

/// Log a `RunCompleted` entry — `run()` / `resume()` returned `Ok(WorkerResult)`.
pub fn save_run_completed(
    store: &impl Store,
    session_id: SessionId,
    result: WorkerResult,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::RunCompleted {
            ts: session_log::now_millis(),
            interrupted,
            result,
        },
    )
}

/// Log a `RunErrored` entry — `run()` / `resume()` returned `Err(WorkerError)`.
///
/// `WorkerError` is not `Serialize`, so the caller passes a lossy
/// `to_string()` rendering as `message`.
pub fn save_run_errored(
    store: &impl Store,
    session_id: SessionId,
    message: String,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::RunErrored {
            ts: session_log::now_millis(),
            interrupted,
            message,
        },
    )
}

/// Log an `LlmUsage` entry — 1 LLM リクエスト分の Usage スナップショット。
///
/// `history_len` は送信時の `history.len()`。`input_total_tokens` は
/// その prefix をプロバイダが実測した占有量（プロンプト全長）で、
/// プロバイダ別の正規化（Anthropic では `input + cache_read + cache_creation`）を
/// 済ませた値を渡す。
pub fn save_usage(
    store: &impl Store,
    session_id: SessionId,
    history_len: usize,
    input_total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::LlmUsage {
            ts: session_log::now_millis(),
            history_len,
            input_total_tokens,
            cache_read_tokens,
            cache_write_tokens,
            output_tokens,
        },
    )
}

/// Log an `Extension` entry — domain-tagged opaque payload.
///
/// session-store treats `payload` as an unstructured `serde_json::Value`.
/// Each domain is responsible for serializing into and folding out of it.
/// Use `RestoredState.extensions` to read entries back at restore time.
pub fn save_extension(
    store: &impl Store,
    session_id: SessionId,
    domain: impl Into<String>,
    payload: serde_json::Value,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::Extension {
            ts: session_log::now_millis(),
            domain: domain.into(),
            payload,
        },
    )
}

/// Log the Pod's latest runtime scope snapshot.
pub fn save_pod_scope(
    store: &impl Store,
    session_id: SessionId,
    snapshot: &PodScopeSnapshot,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(snapshot)?;
    save_extension(
        store,
        session_id,
        session_log::POD_SCOPE_EXTENSION_DOMAIN,
        payload,
    )
}

/// Log a `ConfigChanged` entry.
pub fn save_config_changed(
    store: &impl Store,
    session_id: SessionId,
    config: &RequestConfig,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        LogEntry::ConfigChanged {
            ts: session_log::now_millis(),
            config: config.clone(),
        },
    )
}

/// Fork the current state into a new session.
pub fn fork(store: &impl Store, state: SessionStartState<'_>) -> Result<SessionId, StoreError> {
    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.create_session(fork_id, &[entry])?;
    Ok(fork_id)
}

/// Fork from a turn boundary in a stored session's log.
///
/// `at_turn_index` is the `turn_count` of the most recent completed
/// `TurnEnd` in the source segment that the fork should branch from.
/// Replay collects state up to and including that `TurnEnd`; entries
/// after it are not carried into the new segment.
pub fn fork_at(
    store: &impl Store,
    source_id: SessionId,
    at_turn_index: usize,
) -> Result<SessionId, StoreError> {
    let entries = store.read_all(source_id)?;
    let cut = if at_turn_index == 0 {
        // Branch directly after the SessionStart (or whatever opens the
        // segment), before any turn completes.
        entries
            .iter()
            .position(|e| !matches!(e, LogEntry::SessionStart { .. }))
            .unwrap_or(entries.len())
    } else {
        entries
            .iter()
            .position(|e| matches!(e, LogEntry::TurnEnd { turn_count, .. } if *turn_count == at_turn_index))
            .map(|i| i + 1)
            .unwrap_or(entries.len())
    };
    let state = session_log::collect_state(&entries[..cut]);

    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt,
        config: state.config,
        history: to_logged(&state.history),
        forked_from: Some(SessionOrigin {
            session_id: source_id,
            at_turn_index,
        }),
        compacted_from: None,
    };
    store.create_session(fork_id, &[entry])?;
    Ok(fork_id)
}

/// Append a single `LogEntry`.
///
/// Lower-level dual of the `save_*` convenience wrappers in this module.
/// Use when the caller already builds the typed entry itself (e.g. when
/// it needs the same value for an in-memory mirror + broadcast).
pub fn append_entry(
    store: &impl Store,
    session_id: SessionId,
    entry: LogEntry,
) -> Result<(), StoreError> {
    store.append(session_id, &entry)
}
