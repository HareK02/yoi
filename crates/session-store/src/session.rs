//! Free functions for session persistence operations.
//!
//! These functions record and restore session state without owning a Worker.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.

use crate::SessionId;
use crate::logged_item::{LoggedItem, to_logged};
use crate::session_log::{self, EntryHash, HashedEntry, LogEntry, PodScopeSnapshot, SessionOrigin};
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
///
/// Returns the new session ID and head hash.
pub fn create_session(
    store: &impl Store,
    state: SessionStartState<'_>,
) -> Result<(SessionId, EntryHash), StoreError> {
    let session_id = crate::new_session_id();
    let hash = create_session_with_id(store, session_id, state)?;
    Ok((session_id, hash))
}

/// Write a fresh `SessionStart` entry using a pre-generated session ID.
///
/// Used by callers that need to reserve a session ID synchronously but
/// defer the initial log append (e.g. Pod, which resolves a templated
/// system prompt only at first turn). Returns the resulting head hash.
pub fn create_session_with_id(
    store: &impl Store,
    session_id: SessionId,
    state: SessionStartState<'_>,
) -> Result<EntryHash, StoreError> {
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: None,
        entry,
    };
    store.append(session_id, &hashed_entry)?;
    Ok(hash)
}

/// Create a compacted session from an existing one.
///
/// Records `compacted_from` provenance linking back to the source session.
/// Returns the new session ID and head hash.
pub fn create_compacted_session(
    store: &impl Store,
    state: SessionStartState<'_>,
    source_session_id: SessionId,
    source_head_hash: EntryHash,
) -> Result<(SessionId, EntryHash), StoreError> {
    let session_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: Some(SessionOrigin {
            session_id: source_session_id,
            at_hash: source_head_hash,
        }),
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: None,
        entry,
    };
    store.append(session_id, &hashed_entry)?;
    Ok((session_id, hash))
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

/// Check if the store's head still matches the expected head hash.
/// If not, auto-fork into a new session.
///
/// Updates `session_id` and `head_hash` in place when a fork occurs.
pub fn ensure_head_or_fork(
    store: &impl Store,
    session_id: &mut SessionId,
    head_hash: &mut Option<EntryHash>,
    state: SessionStartState<'_>,
) -> Result<(), StoreError> {
    let store_head = store.read_head_hash(*session_id)?;
    if store_head == *head_hash {
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
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: None,
        entry,
    };
    store.create_session(fork_id, &[hashed_entry])?;
    *session_id = fork_id;
    *head_hash = Some(hash);
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
    head_hash: &mut Option<EntryHash>,
    segments: Vec<Segment>,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
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
        append_entry(store, session_id, head_hash, entry)?;
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
    head_hash: &mut Option<EntryHash>,
    item: SystemItem,
) -> Result<EntryHash, StoreError> {
    append_entry_with_hash(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    turn_count: usize,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    result: WorkerResult,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    message: String,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    history_len: usize,
    input_total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    domain: impl Into<String>,
    payload: serde_json::Value,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    head_hash: &mut Option<EntryHash>,
    snapshot: &PodScopeSnapshot,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(snapshot)?;
    save_extension(
        store,
        session_id,
        head_hash,
        session_log::POD_SCOPE_EXTENSION_DOMAIN,
        payload,
    )
}

/// Log a `ConfigChanged` entry.
pub fn save_config_changed(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    config: &RequestConfig,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        head_hash,
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
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash,
        prev_hash: None,
        entry,
    };
    store.create_session(fork_id, &[hashed_entry])?;
    Ok(fork_id)
}

/// Fork from an arbitrary point in a stored session's log.
pub fn fork_at(
    store: &impl Store,
    source_id: SessionId,
    at_hash: &EntryHash,
) -> Result<SessionId, StoreError> {
    let entries = store.read_all(source_id)?;
    let cut = entries
        .iter()
        .position(|e| &e.hash == at_hash)
        .map(|i| i + 1)
        .unwrap_or(entries.len());
    let state = session_log::collect_state(&entries[..cut]);

    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt,
        config: state.config,
        history: to_logged(&state.history),
        forked_from: Some(session_log::SessionOrigin {
            session_id: source_id,
            at_hash: at_hash.clone(),
        }),
        compacted_from: None,
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash,
        prev_hash: None,
        entry,
    };
    store.create_session(fork_id, &[hashed_entry])?;
    Ok(fork_id)
}

/// Append a single `LogEntry`, chaining the hash and updating `head_hash`.
///
/// Lower-level dual of the `save_*` convenience wrappers in this module.
/// Use when the caller already builds the typed entry itself (e.g. when
/// it needs the same value for an in-memory mirror + broadcast).
pub fn append_entry(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    entry: LogEntry,
) -> Result<(), StoreError> {
    append_entry_with_hash(store, session_id, head_hash, entry)?;
    Ok(())
}

/// Same as [`append_entry`] but returns the freshly computed entry hash.
///
/// Used by paths that need the hash for downstream broadcast or mirror
/// updates (e.g. the Pod's `SessionLogSink`).
pub fn append_entry_with_hash(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    entry: LogEntry,
) -> Result<EntryHash, StoreError> {
    let hash = session_log::compute_hash(head_hash.as_ref(), &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: head_hash.clone(),
        entry,
    };
    store.append(session_id, &hashed_entry)?;
    *head_hash = Some(hash.clone());
    Ok(hash)
}
