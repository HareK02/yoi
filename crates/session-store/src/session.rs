//! Free functions for session persistence operations.
//!
//! These functions record and restore session state without owning a Worker.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.

use crate::SessionId;
use crate::session_log::{self, EntryHash, HashedEntry, LogEntry, SessionOrigin};
use crate::store::{Store, StoreError};
use llm_worker::WorkerResult;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::types::Item;

/// State snapshot for creating a SessionStart entry.
pub struct SessionStartState<'a> {
    pub system_prompt: Option<&'a str>,
    pub config: &'a RequestConfig,
    pub history: &'a [Item],
}

/// Create a new session, writing the initial `SessionStart` entry.
///
/// Returns the new session ID and head hash.
pub async fn create_session(
    store: &impl Store,
    state: SessionStartState<'_>,
) -> Result<(SessionId, EntryHash), StoreError> {
    let session_id = crate::new_session_id();
    let hash = create_session_with_id(store, session_id, state).await?;
    Ok((session_id, hash))
}

/// Write a fresh `SessionStart` entry using a pre-generated session ID.
///
/// Used by callers that need to reserve a session ID synchronously but
/// defer the initial log append (e.g. Pod, which resolves a templated
/// system prompt only at first turn). Returns the resulting head hash.
pub async fn create_session_with_id(
    store: &impl Store,
    session_id: SessionId,
    state: SessionStartState<'_>,
) -> Result<EntryHash, StoreError> {
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: state.history.to_vec(),
        forked_from: None,
        compacted_from: None,
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: None,
        entry,
    };
    store.append(session_id, &hashed_entry).await?;
    Ok(hash)
}

/// Create a compacted session from an existing one.
///
/// Records `compacted_from` provenance linking back to the source session.
/// Returns the new session ID and head hash.
pub async fn create_compacted_session(
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
        history: state.history.to_vec(),
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
    store.append(session_id, &hashed_entry).await?;
    Ok((session_id, hash))
}

/// Restore session state from a stored log.
///
/// Returns the reconstructed state. The caller is responsible for
/// applying it to a Worker.
pub async fn restore(
    store: &impl Store,
    session_id: SessionId,
) -> Result<crate::session_log::RestoredState, StoreError> {
    let entries = store.read_all(session_id).await?;
    Ok(session_log::collect_state(&entries))
}

/// Check if the store's head still matches the expected head hash.
/// If not, auto-fork into a new session.
///
/// Updates `session_id` and `head_hash` in place when a fork occurs.
pub async fn ensure_head_or_fork(
    store: &impl Store,
    session_id: &mut SessionId,
    head_hash: &mut Option<EntryHash>,
    state: SessionStartState<'_>,
) -> Result<(), StoreError> {
    let store_head = store.read_head_hash(*session_id).await?;
    if store_head == *head_hash {
        return Ok(());
    }
    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: state.history.to_vec(),
        forked_from: None,
        compacted_from: None,
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: None,
        entry,
    };
    store.create_session(fork_id, &[hashed_entry]).await?;
    *session_id = fork_id;
    *head_hash = Some(hash);
    Ok(())
}

/// Log the history delta — new items added since the previous snapshot.
///
/// Classifies items into UserInput, AssistantItems, ToolResults, and
/// HookInjectedItems entries automatically.
pub async fn save_delta(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    new_items: &[Item],
) -> Result<(), StoreError> {
    if new_items.is_empty() {
        return Ok(());
    }

    let ts = session_log::now_millis();
    let mut i = 0;

    while i < new_items.len() {
        let item = &new_items[i];
        if item.is_user_message() {
            append_entry(
                store,
                session_id,
                head_hash,
                LogEntry::UserInput {
                    ts,
                    item: new_items[i].clone(),
                },
            )
            .await?;
            i += 1;
        } else if item.is_tool_result() {
            let start = i;
            while i < new_items.len() && new_items[i].is_tool_result() {
                i += 1;
            }
            append_entry(
                store,
                session_id,
                head_hash,
                LogEntry::ToolResults {
                    ts,
                    items: new_items[start..i].to_vec(),
                },
            )
            .await?;
        } else if item.is_assistant_message() || item.is_tool_call() || item.is_reasoning() {
            let start = i;
            while i < new_items.len()
                && (new_items[i].is_assistant_message()
                    || new_items[i].is_tool_call()
                    || new_items[i].is_reasoning())
            {
                i += 1;
            }
            append_entry(
                store,
                session_id,
                head_hash,
                LogEntry::AssistantItems {
                    ts,
                    items: new_items[start..i].to_vec(),
                },
            )
            .await?;
        } else {
            append_entry(
                store,
                session_id,
                head_hash,
                LogEntry::HookInjectedItems {
                    ts,
                    items: vec![new_items[i].clone()],
                },
            )
            .await?;
            i += 1;
        }
    }
    Ok(())
}

/// Log a TurnEnd entry.
pub async fn save_turn_end(
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
    .await
}

/// Log a `RunCompleted` entry — `run()` / `resume()` returned `Ok(WorkerResult)`.
pub async fn save_run_completed(
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
    .await
}

/// Log a `RunErrored` entry — `run()` / `resume()` returned `Err(WorkerError)`.
///
/// `WorkerError` is not `Serialize`, so the caller passes a lossy
/// `to_string()` rendering as `message`.
pub async fn save_run_errored(
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
    .await
}

/// Log an `LlmUsage` entry — 1 LLM リクエスト分の Usage スナップショット。
///
/// `history_len` は送信時の `history.len()`。`input_total_tokens` は
/// その prefix をプロバイダが実測した占有量（プロンプト全長）で、
/// プロバイダ別の正規化（Anthropic では `input + cache_read + cache_creation`）を
/// 済ませた値を渡す。
pub async fn save_usage(
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
    .await
}

/// Log an `Extension` entry — domain-tagged opaque payload.
///
/// session-store treats `payload` as an unstructured `serde_json::Value`.
/// Each domain is responsible for serializing into and folding out of it.
/// Use `RestoredState.extensions` to read entries back at restore time.
pub async fn save_extension(
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
    .await
}

/// Log a `ConfigChanged` entry.
pub async fn save_config_changed(
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
    .await
}

/// Fork the current state into a new session.
pub async fn fork(
    store: &impl Store,
    state: SessionStartState<'_>,
) -> Result<SessionId, StoreError> {
    let fork_id = crate::new_session_id();
    let entry = LogEntry::SessionStart {
        ts: session_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: state.history.to_vec(),
        forked_from: None,
        compacted_from: None,
    };
    let hash = session_log::compute_hash(None, &entry);
    let hashed_entry = HashedEntry {
        hash,
        prev_hash: None,
        entry,
    };
    store.create_session(fork_id, &[hashed_entry]).await?;
    Ok(fork_id)
}

/// Fork from an arbitrary point in a stored session's log.
pub async fn fork_at(
    store: &impl Store,
    source_id: SessionId,
    at_hash: &EntryHash,
) -> Result<SessionId, StoreError> {
    let entries = store.read_all(source_id).await?;
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
        history: state.history,
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
    store.create_session(fork_id, &[hashed_entry]).await?;
    Ok(fork_id)
}

// ── Private helper ──────────────────────────────────────────────────────

async fn append_entry(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    entry: LogEntry,
) -> Result<(), StoreError> {
    let hash = session_log::compute_hash(head_hash.as_ref(), &entry);
    let hashed_entry = HashedEntry {
        hash: hash.clone(),
        prev_hash: head_hash.clone(),
        entry,
    };
    store.append(session_id, &hashed_entry).await?;
    *head_hash = Some(hash);
    Ok(())
}
