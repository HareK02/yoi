//! Free functions for segment persistence operations.
//!
//! These functions record and restore segment state without owning a Engine.
//! The caller (typically Worker) holds the Engine directly and calls these
//! functions after state-mutating operations.

use crate::logged_item::{LoggedItem, to_logged};
use crate::segment_log::{self, LogEntry, SegmentOrigin};
use crate::store::{Store, StoreError};
use crate::system_item::SystemItem;
use crate::{SegmentId, SessionId};
use agen::EngineResult;
use agen::llm_client::RequestConfig;
use agen::llm_client::types::Item;
use protocol::Segment;

/// State snapshot for creating a SegmentStart entry.
pub struct SegmentStartState<'a> {
    pub system_prompt: Option<&'a str>,
    pub config: &'a RequestConfig,
    pub history: &'a [Item],
}

/// Create a new session + initial segment, writing the initial
/// `SegmentStart` entry. Returns the freshly minted `(session_id, segment_id)`.
pub fn create_segment(
    store: &impl Store,
    state: SegmentStartState<'_>,
) -> Result<(SessionId, SegmentId), StoreError> {
    let session_id = crate::new_session_id();
    let segment_id = crate::new_segment_id();
    create_segment_with_ids(store, session_id, segment_id, state)?;
    Ok((session_id, segment_id))
}

/// Write a fresh `SegmentStart` entry using pre-generated IDs.
///
/// Used by callers that need to reserve `(session_id, segment_id)`
/// synchronously but defer the initial log append (e.g. Worker, which
/// resolves a templated system prompt only at first turn).
pub fn create_segment_with_ids(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    state: SegmentStartState<'_>,
) -> Result<(), StoreError> {
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        session_id,
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.append(session_id, segment_id, &entry)
}

/// Create a compacted segment from an existing one. Inherits the source's
/// `session_id` so the compacted lineage stays within the same Session.
///
/// Records `compacted_from` provenance linking back to the source segment
/// at the turn boundary captured by `source_turn_count` (the most recent
/// completed turn in the source).
pub fn create_compacted_segment(
    store: &impl Store,
    state: SegmentStartState<'_>,
    source_session_id: SessionId,
    source_segment_id: SegmentId,
    source_turn_count: usize,
) -> Result<SegmentId, StoreError> {
    let segment_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        session_id: source_session_id,
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: Some(SegmentOrigin {
            segment_id: source_segment_id,
            at_turn_index: source_turn_count,
        }),
    };
    store.append(source_session_id, segment_id, &entry)?;
    Ok(segment_id)
}

/// Restore segment state from a stored log.
///
/// Returns the reconstructed state. The caller is responsible for
/// applying it to a Engine.
pub fn restore(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
) -> Result<crate::segment_log::RestoredState, StoreError> {
    let entries = store.read_all(session_id, segment_id)?;
    Ok(segment_log::collect_state(&entries))
}

/// Restore segment state when only the segment ID is known. Uses
/// [`Store::lookup_session_of`] to resolve the parent Session.
///
/// Shim for legacy entry points (`worker-cli --session <UUID>` etc.) that
/// receive a Segment ID without a Session ID.
pub fn restore_by_segment(
    store: &impl Store,
    segment_id: SegmentId,
) -> Result<crate::segment_log::RestoredState, StoreError> {
    let session_id = store
        .lookup_session_of(segment_id)?
        .ok_or(StoreError::NotFound(segment_id))?;
    restore(store, session_id, segment_id)
}

/// Live auto-fork on concurrent-writer detection.
///
/// Checks whether the store's on-disk entry count still matches the
/// writer's own append tally. If they match, the writer still owns the
/// segment tail and nothing happens. If the store has grown behind the
/// writer's back, another process appended to the same segment — so we
/// branch into a fresh segment within the same Session.
///
/// # Marker form
///
/// Detection is by **tail entry-count comparison**, not by writing a
/// terminal marker into the source segment. The source segment is left
/// completely immutable — identical to the past-fork ([`fork_at`])
/// invariant. The fork relationship is instead recorded forward on the
/// *new* segment's `SegmentStart.forked_from`, so the lineage is still
/// reconstructable from the logs alone (read each segment's
/// `SegmentStart`; follow `forked_from` / `compacted_from` backward).
/// Listing a parent's children is a cheap `list_segments(session_id)`
/// scan filtered on `forked_from.segment_id`.
///
/// `at_turn_index` is the writer's current `turn_count`: the fork seeds
/// the new segment with the writer's in-memory history (which reflects
/// state up to that turn), so that is the divergence point relative to
/// the now-diverged source segment.
///
/// Updates `segment_id` and `entries_written` in place when a fork occurs.
pub fn ensure_head_or_fork(
    store: &impl Store,
    session_id: SessionId,
    segment_id: &mut SegmentId,
    entries_written: &mut usize,
    at_turn_index: usize,
    state: SegmentStartState<'_>,
) -> Result<(), StoreError> {
    let store_count = store.read_entry_count(session_id, *segment_id)?;
    if store_count == *entries_written {
        return Ok(());
    }
    let source_segment_id = *segment_id;
    let fork_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        session_id,
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: Some(SegmentOrigin {
            segment_id: source_segment_id,
            at_turn_index,
        }),
        compacted_from: None,
    };
    store.create_segment(session_id, fork_id, &[entry])?;
    *segment_id = fork_id;
    *entries_written = 1;
    Ok(())
}

/// Log a `UserInput` entry from the original typed `Vec<Segment>`.
///
/// Submit-time entry. Worker calls this at the head of a `Run` turn before
/// the worker pushes its flattened user message into history; replay
/// derives the worker `Item::user_message` from these segments via
/// [`Segment::flatten_to_text`].
pub fn save_user_input(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    segments: Vec<Segment>,
) -> Result<(), StoreError> {
    save_user_input_with_extensions(store, session_id, segment_id, segments, Vec::new())
}

/// Atomically persist one typed user submission and Runtime-owned session
/// extensions in the same log record.
pub fn save_user_input_with_extensions(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    segments: Vec<Segment>,
    extensions: Vec<segment_log::SessionExtension>,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::UserInput {
            ts: segment_log::now_millis(),
            segments,
            extensions,
        },
    )
}

/// Log the history delta — new items added since the previous snapshot.
///
/// Classifies items into AssistantItem / ToolResult entries automatically
/// (one entry per item). User messages are skipped
/// because they are persisted upfront via [`save_user_input`] at submit
/// time; the worker pushes a flattened copy into its history that
/// arrives here in `new_items` and would otherwise produce a duplicate
/// `UserInput` entry.
pub fn save_delta(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    new_items: &[Item],
) -> Result<(), StoreError> {
    if new_items.is_empty() {
        return Ok(());
    }

    let ts = segment_log::now_millis();
    for item in new_items {
        if item.is_user_message() {
            // Already persisted by save_user_input at submit time.
            continue;
        }
        let entry = classify_history_item(item, ts);
        append_entry(store, session_id, segment_id, entry)?;
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
/// for the Worker-side interceptor commit path; mirrors the per-item
/// commit shape used for assistant / tool result entries.
pub fn append_system_item(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    item: SystemItem,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::SystemItem {
            ts: segment_log::now_millis(),
            item,
        },
    )
}

/// Log a TurnEnd entry.
pub fn save_turn_end(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    turn_count: usize,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::TurnEnd {
            ts: segment_log::now_millis(),
            turn_count,
        },
    )
}

/// Log a `RunCompleted` entry — `run()` / `resume()` returned `Ok(EngineResult)`.
pub fn save_run_completed(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    result: EngineResult,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::RunCompleted {
            ts: segment_log::now_millis(),
            interrupted,
            result,
        },
    )
}

/// Log a `RunErrored` entry — `run()` / `resume()` returned `Err(EngineError)`.
///
/// `EngineError` is not `Serialize`, so the caller passes a lossy
/// `to_string()` rendering as `message`.
pub fn save_run_errored(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    message: String,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::RunErrored {
            ts: segment_log::now_millis(),
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
    segment_id: SegmentId,
    history_len: usize,
    input_total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::LlmUsage {
            ts: segment_log::now_millis(),
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
    segment_id: SegmentId,
    domain: impl Into<String>,
    payload: serde_json::Value,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: domain.into(),
            payload,
        },
    )
}

/// Log a `ConfigChanged` entry.
pub fn save_config_changed(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    config: &RequestConfig,
) -> Result<(), StoreError> {
    append_entry(
        store,
        session_id,
        segment_id,
        LogEntry::ConfigChanged {
            ts: segment_log::now_millis(),
            config: config.clone(),
        },
    )
}

/// Fork the current state into a brand-new Session (no parent lineage).
///
/// Use this for "start a fresh conversation from this state" — the
/// returned segment does not share `session_id` with any prior segment.
/// In-Session forks (live auto-fork / past-turn fork) go through
/// [`fork_at`] or [`ensure_head_or_fork`] instead.
pub fn fork(
    store: &impl Store,
    state: SegmentStartState<'_>,
) -> Result<(SessionId, SegmentId), StoreError> {
    let session_id = crate::new_session_id();
    let fork_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        session_id,
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.create_segment(session_id, fork_id, &[entry])?;
    Ok((session_id, fork_id))
}

/// Fork from a turn boundary in a stored segment log, keeping the new
/// segment in the same Session as `source_id`.
///
/// `at_turn_index` is the `turn_count` of the most recent completed
/// `TurnEnd` in the source segment that the fork should branch from.
/// Replay collects state up to and including that `TurnEnd`; entries
/// after it are not carried into the new segment.
///
/// # Invariant: the source segment is never mutated
///
/// Past-fork only reads the source and seeds a brand-new segment. It
/// writes no marker back into the source — exactly like live auto-fork
/// ([`ensure_head_or_fork`]). This keeps nested past-forks simple: a
/// fork of a fork just reads its own source and branches again, with no
/// marker-position bookkeeping to reconcile across the chain.
pub fn fork_at(
    store: &impl Store,
    source_session_id: SessionId,
    source_id: SegmentId,
    at_turn_index: usize,
) -> Result<SegmentId, StoreError> {
    let entries = store.read_all(source_session_id, source_id)?;
    let cut = if at_turn_index == 0 {
        // Branch directly after the SegmentStart (or whatever opens the
        // segment), before any turn completes.
        entries
            .iter()
            .position(|e| !matches!(e, LogEntry::SegmentStart { .. }))
            .unwrap_or(entries.len())
    } else {
        entries
            .iter()
            .position(|e| matches!(e, LogEntry::TurnEnd { turn_count, .. } if *turn_count == at_turn_index))
            .map(|i| i + 1)
            .unwrap_or(entries.len())
    };
    let state = segment_log::collect_state(&entries[..cut]);

    let fork_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        session_id: source_session_id,
        system_prompt: state.system_prompt,
        config: state.config,
        history: to_logged(&state.history),
        forked_from: Some(SegmentOrigin {
            segment_id: source_id,
            at_turn_index,
        }),
        compacted_from: None,
    };
    store.create_segment(source_session_id, fork_id, &[entry])?;
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
    segment_id: SegmentId,
    entry: LogEntry,
) -> Result<(), StoreError> {
    store.append(session_id, segment_id, &entry)
}
