//! Display-only accounting; never inserted into model history or written back
//! to the journal. Both snapshot and bounded history paging use this projection.
use std::collections::HashMap;

use protocol::{SessionEntryProvenance, SessionSnapshotEntry, SessionSnapshotEntryData};

use crate::LogEntry;

#[derive(Default)]
struct Activity {
    started_at: u64,
    requests: u64,
    upload: u64,
    output: u64,
    anchor: Option<String>,
}

pub(super) fn project<'a>(
    records: impl IntoIterator<Item = &'a LogEntry>,
) -> HashMap<String, SessionSnapshotEntry> {
    let mut active: Option<Activity> = None;
    let mut stats = HashMap::new();
    for record in records {
        match record {
            LogEntry::Invoke { ts, .. } => {
                active = Some(Activity {
                    started_at: *ts,
                    ..Activity::default()
                })
            }
            LogEntry::LlmUsage {
                input_total_tokens,
                cache_read_tokens,
                output_tokens,
                ..
            } => {
                if let Some(run) = &mut active {
                    run.requests += 1;
                    run.upload = run
                        .upload
                        .saturating_add(input_total_tokens.saturating_sub(*cache_read_tokens));
                    run.output = run.output.saturating_add(*output_tokens);
                }
            }
            LogEntry::AnnotatedUserInput { history, .. } => {
                if let Some(run) = &mut active {
                    for entry in history {
                        if super::project_item(&entry.metadata.entry_id.0, &entry.item).is_some() {
                            run.anchor = Some(entry.metadata.entry_id.0.clone());
                        }
                    }
                }
            }
            LogEntry::AnnotatedAssistantItem { entry, .. }
            | LogEntry::AnnotatedToolResult { entry, .. } => {
                if let Some(run) = &mut active
                    && super::project_item(&entry.metadata.entry_id.0, &entry.item).is_some()
                {
                    run.anchor = Some(entry.metadata.entry_id.0.clone());
                }
            }
            LogEntry::AnnotatedSystemItem { entry, .. } => {
                if let Some(run) = &mut active {
                    run.anchor = Some(entry.metadata.entry_id.0.clone());
                }
            }
            LogEntry::RunCompleted { ts, result, .. }
                if !matches!(result, agen::EngineResult::Yielded) =>
            {
                finish(&active, *ts, &mut stats);
                // Pause/resume retains the Invoke's cumulative accounting, just
                // like the live TUI. Terminal outcomes cannot leak traffic into
                // a later resumed segment whose Invoke is unavailable.
                if !matches!(result, agen::EngineResult::Paused) {
                    active = None;
                }
            }
            LogEntry::RunCancelled { ts, .. } | LogEntry::RunErrored { ts, .. } => {
                finish(&active, *ts, &mut stats);
                active = None;
            }
            // Compaction seeds repeat retained entries, not fresh traffic.
            // A page traverses adopted lineage in journal order and retains the
            // open Invoke across these checkpoints. A standalone new segment
            // without that Invoke cannot claim complete accounting.
            _ => {}
        }
    }
    stats
}

fn finish(
    active: &Option<Activity>,
    ended_at: u64,
    stats: &mut HashMap<String, SessionSnapshotEntry>,
) {
    let Some(run) = active else { return };
    let Some(anchor) = &run.anchor else { return };
    // No usage records means unavailable measurement, not known zero traffic.
    if run.requests == 0 {
        return;
    }
    stats.insert(
        anchor.clone(),
        SessionSnapshotEntry {
            entry_id: format!("run-stats:{anchor}"),
            timestamp: ended_at,
            provenance: SessionEntryProvenance::LegacyUnknown,
            derived_from: vec![anchor.clone()],
            data: SessionSnapshotEntryData::RunStats {
                elapsed_ms: ended_at.saturating_sub(run.started_at),
                requests: run.requests,
                upload_tokens: run.upload,
                output_tokens: run.output,
            },
        },
    );
}

pub(super) fn append_to(
    entries: &mut Vec<SessionSnapshotEntry>,
    stats: &HashMap<String, SessionSnapshotEntry>,
) {
    let mut with_stats = Vec::with_capacity(entries.len());
    for entry in std::mem::take(entries) {
        let summary = stats.get(&entry.entry_id);
        with_stats.push(entry);
        if let Some(summary) = summary {
            with_stats.push(summary.clone());
        }
    }
    *entries = with_stats;
}
