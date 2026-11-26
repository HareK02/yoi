//! Local, Host-bound self observation. Neither model input nor Workspace clients
//! choose the Store, Session or active Segment. Only adopted ancestry is read;
//! sibling segments and a parent's unadopted suffix are never exploration input.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::session_capture::{CaptureLimits, SessionCapture};
use session_store::{LogEntry, SegmentId, SessionId, Store};

#[derive(Clone)]
pub(crate) struct SelfSessionReader {
    read: Arc<dyn Fn() -> Result<SessionCapture, String> + Send + Sync>,
}

impl SelfSessionReader {
    pub(crate) fn new(
        read: impl Fn() -> Result<SessionCapture, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            read: Arc::new(read),
        }
    }

    pub(crate) fn capture(&self) -> Result<SessionCapture, String> {
        (self.read)()
    }
}

/// The caller holds the live append fence, including active-location selection.
/// Saved identity prefixes, not timestamps or a directory scan, select ancestry.
pub(crate) fn capture(
    store: &impl Store,
    session: SessionId,
    active: SegmentId,
) -> Result<SessionCapture, String> {
    let mut segment = active;
    let mut expected: Option<Vec<String>> = None;
    let mut visited = HashSet::new();
    let mut captures = Vec::new();
    let mut total = 0usize;
    loop {
        if visited.len() >= 256 || !visited.insert(segment) {
            return Err("Self Session lineage is cyclic or exceeds the read limit".into());
        }
        if store
            .read_entry_count(session, segment)
            .map_err(|e| e.to_string())?
            > CaptureLimits::default().max_entries
        {
            return Err("Self Session record limit exceeded".into());
        }
        let mut log = store
            .read_all(session, segment)
            .map_err(|e| e.to_string())?;
        if let Some(ids) = expected.take() {
            log = adopted_prefix(log, &ids)?;
        }
        let Some(LogEntry::AnnotatedSegmentStart {
            session_id,
            history: seed,
            forked_from,
            compacted_from,
            ..
        }) = log.first()
        else {
            return Err("Self Session segment header is missing".into());
        };
        if *session_id != session || (forked_from.is_some() && compacted_from.is_some()) {
            return Err("Self Session lineage does not match its bound Session".into());
        }
        let parent = forked_from.as_ref().or(compacted_from.as_ref());
        let next = parent.map(|origin| origin.segment_id);
        expected = if compacted_from.is_some() {
            let summaries: Vec<_> = seed
                .iter()
                .filter(|entry| {
                    entry.metadata.input_metadata.as_ref().is_some_and(|input| {
                        input.usage().compaction
                            == session_store::InputCompactionUse::OpeningSummary
                    })
                })
                .collect();
            if summaries.len() != 1 {
                return Err("Self Session compaction source is unavailable".into());
            }
            Some(
                summaries[0]
                    .metadata
                    .derivation
                    .as_ref()
                    .ok_or("Self Session compaction source identities are missing")?
                    .sources
                    .iter()
                    .map(|id| id.0.clone())
                    .collect(),
            )
        } else if forked_from.is_some() {
            Some(
                seed.iter()
                    .map(|entry| entry.metadata.entry_id.0.clone())
                    .collect(),
            )
        } else {
            None
        };
        let history = crate::session_history::restore_history_entries(session, segment, &log)?;
        total = total
            .checked_add(history.len())
            .ok_or("Self Session read limit exceeded")?;
        if total > CaptureLimits::default().max_entries {
            return Err("Self Session read limit exceeded".into());
        }
        captures.push(
            SessionCapture::from_complete_history(
                session.to_string(),
                segment.to_string(),
                history,
            )
            .to_dto(),
        );
        match next {
            Some(id) => segment = id,
            None => break,
        }
    }
    captures.reverse();
    let mut dto =
        SessionCapture::from_complete_history(session.to_string(), active.to_string(), Vec::new())
            .to_dto();
    let mut seen = HashMap::<String, usize>::new();
    for capture in captures {
        for mut entry in capture.entries {
            if let Some(&index) = seen.get(&entry.entry_id) {
                // A later adopted correction may replace the body, but not the
                // original source location or the stable entry identity.
                entry.order = dto.entries[index].order;
                entry.source = dto.entries[index].source.clone();
                dto.entries[index] = entry;
            } else {
                entry.order = dto.entries.len() as u64;
                seen.insert(entry.entry_id.clone(), dto.entries.len());
                dto.entries.push(entry);
            }
        }
    }
    dto.view_entry_count = dto.entries.len() as u64;
    SessionCapture::try_from_dto(dto, CaptureLimits::default()).map_err(|e| e.to_string())
}

fn adopted_prefix(log: Vec<LogEntry>, expected: &[String]) -> Result<Vec<LogEntry>, String> {
    let mut consumed = 0;
    let mut prefix = Vec::new();
    let mut seen = HashMap::<String, session_store::LoggedHistoryEntry>::new();
    for mut record in log {
        // Keep trailing corrections, but never cross the next history addition.
        if consumed == expected.len()
            && !prefix.is_empty()
            && matches!(
                &record,
                LogEntry::AnnotatedSegmentStart { .. }
                    | LogEntry::AnnotatedUserInput { .. }
                    | LogEntry::AnnotatedAssistantItem { .. }
                    | LogEntry::AnnotatedToolResult { .. }
                    | LogEntry::AnnotatedSystemItem { .. }
            )
        {
            break;
        }
        let additions = match &record {
            LogEntry::AnnotatedSegmentStart { history, .. }
            | LogEntry::AnnotatedUserInput { history, .. } => history.clone(),
            LogEntry::AnnotatedAssistantItem { entry, .. }
            | LogEntry::AnnotatedToolResult { entry, .. } => vec![entry.clone()],
            LogEntry::AnnotatedSystemItem { entry, .. } => vec![entry.to_history_entry()],
            LogEntry::ToolResultCorrected { entry, .. } => {
                if seen.contains_key(&entry.metadata.entry_id.0) {
                    seen.insert(entry.metadata.entry_id.0.clone(), entry.clone());
                }
                Vec::new()
            }
            _ => Vec::new(),
        };
        let mut truncate_at = None;
        for (index, entry) in additions.into_iter().enumerate() {
            let id = &entry.metadata.entry_id.0;
            // A precommitted assistant item may also be logged at turn flush.
            // Adoption names logical identities, not physical append count. Only
            // exact duplicates can be skipped; changed bodies need correction records.
            if let Some(previous) = seen.get(id) {
                if previous != &entry {
                    return Err(
                        "Self Session repeated history identity has conflicting content".into(),
                    );
                }
                continue;
            }
            if consumed == expected.len() {
                truncate_at = Some(index);
                break;
            }
            if id != &expected[consumed] {
                return Err("Self Session adopted history prefix does not match".into());
            }
            consumed += 1;
            seen.insert(id.clone(), entry);
        }
        if let Some(index) = truncate_at {
            match &mut record {
                LogEntry::AnnotatedSegmentStart { history, .. }
                | LogEntry::AnnotatedUserInput { history, .. } => history.truncate(index),
                _ => return Err("Self Session adopted history boundary is invalid".into()),
            }
        }
        prefix.push(record);
    }
    if consumed != expected.len() {
        return Err("Self Session adopted history is missing".into());
    }
    Ok(prefix)
}

#[cfg(test)]
#[path = "self_session_tests.rs"]
mod tests;
