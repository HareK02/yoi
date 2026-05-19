//! Free functions for segment persistence operations.
//!
//! These functions record and restore segment state without owning a Worker.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.

use crate::SegmentId;
use crate::logged_item::{LoggedItem, to_logged};
use crate::segment_log::{self, LogEntry, PodScopeSnapshot, SegmentOrigin};
use crate::store::{Store, StoreError};
use crate::system_item::SystemItem;
use llm_worker::WorkerResult;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::types::Item;
use protocol::Segment;

/// State snapshot for creating a SegmentStart entry.
pub struct SegmentStartState<'a> {
    pub system_prompt: Option<&'a str>,
    pub config: &'a RequestConfig,
    pub history: &'a [Item],
}

/// Create a new segment, writing the initial `SegmentStart` entry.
pub fn create_segment(
    store: &impl Store,
    state: SegmentStartState<'_>,
) -> Result<SegmentId, StoreError> {
    let segment_id = crate::new_segment_id();
    create_segment_with_id(store, segment_id, state)?;
    Ok(segment_id)
}

/// Write a fresh `SegmentStart` entry using a pre-generated segment ID.
///
/// Used by callers that need to reserve a segment ID synchronously but
/// defer the initial log append (e.g. Pod, which resolves a templated
/// system prompt only at first turn).
pub fn create_segment_with_id(
    store: &impl Store,
    segment_id: SegmentId,
    state: SegmentStartState<'_>,
) -> Result<(), StoreError> {
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.append(segment_id, &entry)
}

/// Create a compacted segment from an existing one.
///
/// Records `compacted_from` provenance linking back to the source segment
/// at the turn boundary captured by `source_turn_count` (the most recent
/// completed turn in the source).
pub fn create_compacted_segment(
    store: &impl Store,
    state: SegmentStartState<'_>,
    source_segment_id: SegmentId,
    source_turn_count: usize,
) -> Result<SegmentId, StoreError> {
    let segment_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: Some(SegmentOrigin {
            segment_id: source_segment_id,
            at_turn_index: source_turn_count,
        }),
    };
    store.append(segment_id, &entry)?;
    Ok(segment_id)
}

/// Restore segment state from a stored log.
///
/// Returns the reconstructed state. The caller is responsible for
/// applying it to a Worker.
pub fn restore(
    store: &impl Store,
    segment_id: SegmentId,
) -> Result<crate::segment_log::RestoredState, StoreError> {
    let entries = store.read_all(segment_id)?;
    Ok(segment_log::collect_state(&entries))
}

/// Check if the store's entry count still matches the writer's tally.
/// If not, auto-fork into a new segment.
///
/// Updates `segment_id` and `entries_written` in place when a fork occurs.
pub fn ensure_head_or_fork(
    store: &impl Store,
    segment_id: &mut SegmentId,
    entries_written: &mut usize,
    state: SegmentStartState<'_>,
) -> Result<(), StoreError> {
    let store_count = store.read_entry_count(*segment_id)?;
    if store_count == *entries_written {
        return Ok(());
    }
    let fork_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.create_segment(fork_id, &[entry])?;
    *segment_id = fork_id;
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
    segment_id: SegmentId,
    segments: Vec<Segment>,
) -> Result<(), StoreError> {
    append_entry(
        store,
        segment_id,
        LogEntry::UserInput {
            ts: segment_log::now_millis(),
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
        append_entry(store, segment_id, entry)?;
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
    segment_id: SegmentId,
    item: SystemItem,
) -> Result<(), StoreError> {
    append_entry(
        store,
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
    segment_id: SegmentId,
    turn_count: usize,
) -> Result<(), StoreError> {
    append_entry(
        store,
        segment_id,
        LogEntry::TurnEnd {
            ts: segment_log::now_millis(),
            turn_count,
        },
    )
}

/// Log a `RunCompleted` entry — `run()` / `resume()` returned `Ok(WorkerResult)`.
pub fn save_run_completed(
    store: &impl Store,
    segment_id: SegmentId,
    result: WorkerResult,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
        segment_id,
        LogEntry::RunCompleted {
            ts: segment_log::now_millis(),
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
    segment_id: SegmentId,
    message: String,
    interrupted: bool,
) -> Result<(), StoreError> {
    append_entry(
        store,
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
    segment_id: SegmentId,
    history_len: usize,
    input_total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
) -> Result<(), StoreError> {
    append_entry(
        store,
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
    segment_id: SegmentId,
    domain: impl Into<String>,
    payload: serde_json::Value,
) -> Result<(), StoreError> {
    append_entry(
        store,
        segment_id,
        LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: domain.into(),
            payload,
        },
    )
}

/// Log the Pod's latest runtime scope snapshot.
pub fn save_pod_scope(
    store: &impl Store,
    segment_id: SegmentId,
    snapshot: &PodScopeSnapshot,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(snapshot)?;
    save_extension(
        store,
        segment_id,
        segment_log::POD_SCOPE_EXTENSION_DOMAIN,
        payload,
    )
}

/// Log a `ConfigChanged` entry.
pub fn save_config_changed(
    store: &impl Store,
    segment_id: SegmentId,
    config: &RequestConfig,
) -> Result<(), StoreError> {
    append_entry(
        store,
        segment_id,
        LogEntry::ConfigChanged {
            ts: segment_log::now_millis(),
            config: config.clone(),
        },
    )
}

/// Fork the current state into a new segment.
pub fn fork(store: &impl Store, state: SegmentStartState<'_>) -> Result<SegmentId, StoreError> {
    let fork_id = crate::new_segment_id();
    let entry = LogEntry::SegmentStart {
        ts: segment_log::now_millis(),
        system_prompt: state.system_prompt.map(String::from),
        config: state.config.clone(),
        history: to_logged(state.history),
        forked_from: None,
        compacted_from: None,
    };
    store.create_segment(fork_id, &[entry])?;
    Ok(fork_id)
}

/// Fork from a turn boundary in a stored segment log.
///
/// `at_turn_index` is the `turn_count` of the most recent completed
/// `TurnEnd` in the source segment that the fork should branch from.
/// Replay collects state up to and including that `TurnEnd`; entries
/// after it are not carried into the new segment.
pub fn fork_at(
    store: &impl Store,
    source_id: SegmentId,
    at_turn_index: usize,
) -> Result<SegmentId, StoreError> {
    let entries = store.read_all(source_id)?;
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
        system_prompt: state.system_prompt,
        config: state.config,
        history: to_logged(&state.history),
        forked_from: Some(SegmentOrigin {
            segment_id: source_id,
            at_turn_index,
        }),
        compacted_from: None,
    };
    store.create_segment(fork_id, &[entry])?;
    Ok(fork_id)
}

/// Append a single `LogEntry`.
///
/// Lower-level dual of the `save_*` convenience wrappers in this module.
/// Use when the caller already builds the typed entry itself (e.g. when
/// it needs the same value for an in-memory mirror + broadcast).
pub fn append_entry(
    store: &impl Store,
    segment_id: SegmentId,
    entry: LogEntry,
) -> Result<(), StoreError> {
    store.append(segment_id, &entry)
}
