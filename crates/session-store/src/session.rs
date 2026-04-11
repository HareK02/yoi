//! Free functions for session persistence operations.
//!
//! These functions record and restore session state without owning a Worker.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.

use crate::session_log::{self, EntryHash, HashedEntry, LogEntry, Outcome, SessionOrigin};
use crate::store::{Store, StoreError};
use crate::SessionId;
use llm_worker::llm_client::types::Item;
use llm_worker::llm_client::RequestConfig;

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
    Ok((session_id, hash))
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
            append_entry(store, session_id, head_hash, LogEntry::UserInput {
                ts,
                item: new_items[i].clone(),
            })
            .await?;
            i += 1;
        } else if item.is_tool_result() {
            let start = i;
            while i < new_items.len() && new_items[i].is_tool_result() {
                i += 1;
            }
            append_entry(store, session_id, head_hash, LogEntry::ToolResults {
                ts,
                items: new_items[start..i].to_vec(),
            })
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
            append_entry(store, session_id, head_hash, LogEntry::AssistantItems {
                ts,
                items: new_items[start..i].to_vec(),
            })
            .await?;
        } else {
            append_entry(store, session_id, head_hash, LogEntry::HookInjectedItems {
                ts,
                items: vec![new_items[i].clone()],
            })
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
    append_entry(store, session_id, head_hash, LogEntry::TurnEnd {
        ts: session_log::now_millis(),
        turn_count,
    })
    .await
}

/// Log a RunOutcome entry.
pub async fn save_outcome(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    outcome: Outcome,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(store, session_id, head_hash, LogEntry::RunOutcome {
        ts: session_log::now_millis(),
        outcome,
        interrupted,
    })
    .await
}

/// Log a `Locked` entry (KV cache locked).
pub async fn save_cache_locked(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    locked_prefix_len: usize,
) -> Result<(), StoreError> {
    append_entry(store, session_id, head_hash, LogEntry::Locked {
        ts: session_log::now_millis(),
        locked_prefix_len,
    })
    .await
}

/// Log a `CacheUnlocked` entry.
pub async fn save_cache_unlocked(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
) -> Result<(), StoreError> {
    append_entry(store, session_id, head_hash, LogEntry::CacheUnlocked {
        ts: session_log::now_millis(),
    })
    .await
}

/// Log a `ConfigChanged` entry.
pub async fn save_config_changed(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    config: &RequestConfig,
) -> Result<(), StoreError> {
    append_entry(store, session_id, head_hash, LogEntry::ConfigChanged {
        ts: session_log::now_millis(),
        config: config.clone(),
    })
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
