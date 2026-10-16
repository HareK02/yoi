//! Shared sequential Job service. Backend and standalone authenticate and check the live attempt
//! before each call; the store additionally fences closed generations inside mutation transactions.
//! Job completion attests a saved processing checkpoint, not model prose or fixed event state.

use super::transport::*;
use super::*;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

fn metadata(j: &Journal) -> JournalMetadata {
    JournalMetadata {
        id: j.id.clone(),
        subject_id: j.subject_id.clone(),
        source_session_id: j.source_session_id.clone(),
        title: j.title.clone(),
        current_position_bytes: j.current_position.len(),
        process_order: j.process_order,
        process_byte: j.process_byte,
        state_token: j.state_token.clone(),
        created_at: j.created_at.clone(),
        updated_at: j.updated_at.clone(),
    }
}

fn text_page(
    text: &str,
    read: &JournalTextRead,
    condition: &[u8],
    maximum_end: usize,
) -> JournalResult<JournalTextPage> {
    if !(1..=MAX_JOURNAL_READ_BYTES).contains(&read.limit)
        || read.offset > maximum_end
        || !text.is_char_boundary(read.offset)
    {
        return Err(JournalError::InvalidInput(
            "invalid Journal UTF-8 read range".into(),
        ));
    }
    let token: String = Sha256::digest(condition)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if read
        .content_token
        .as_deref()
        .is_some_and(|expected| expected != token)
        || (read.offset > 0 && read.content_token.is_none())
    {
        return Err(JournalError::Conflict(
            "Journal text changed or continuation condition missing".into(),
        ));
    }
    let mut end = read.offset.saturating_add(read.limit).min(maximum_end);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == read.offset && end < maximum_end {
        return Err(JournalError::InvalidInput(
            "read limit cannot fit one UTF-8 scalar".into(),
        ));
    }
    Ok(JournalTextPage {
        text: text[read.offset..end].into(),
        next_byte: (end < maximum_end).then_some(end),
        content_token: token,
    })
}

/// Adapter for Host captures bounded by their storage contract. Production source reads should
/// use [`execute_bounded`] to avoid allocating an entire source for one small read.
pub fn execute(
    store: &SubjektivStore,
    context: &JournalJobContext,
    request: JournalJobRequest,
    mut read_source: impl FnMut(&JournalSource) -> JournalResult<String>,
) -> JournalResult<JournalJobResponse> {
    execute_bounded(store, context, request, |source, start, limit| {
        let text = read_source(source)?;
        super::recording::verify_canonical_text(source, &text)?;
        let start = usize::try_from(start)
            .map_err(|_| JournalError::InvalidInput("source offset overflow".into()))?;
        if !text.is_char_boundary(start) {
            return Err(JournalError::InvalidInput(
                "source offset is not UTF-8 aligned".into(),
            ));
        }
        let mut end = start.saturating_add(limit).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(recording::JournalMaterial {
            range: JournalSourceRange {
                source: source.clone(),
                bytes: JournalByteRange {
                    start: start as u64,
                    end: end as u64,
                },
            },
            text: text[start..end].into(),
        })
    })
}

/// Reads are delivered through this service before mutation authority is enabled. The durable
/// delivery ledger stores byte ranges only: Jobs may retain immutable descriptors without
/// accumulating a permanent second copy of the original Session in their request ledgers.
/// The Host verifies original provenance/digest/length and returns a bounded UTF-8 fragment.
pub fn execute_bounded(
    store: &SubjektivStore,
    context: &JournalJobContext,
    request: JournalJobRequest,
    mut read_source: impl FnMut(&JournalSource, u64, usize) -> JournalResult<recording::JournalMaterial>,
) -> JournalResult<JournalJobResponse> {
    let generation = store
        .database
        .try_with_connection(|tx| super::store::job(store, tx, context, true))?;
    let mut delivered = None;
    let response = match request {
        JournalJobRequest::Execute { operation } => {
            require_delivery(store, context, &generation.selected_ranges)?;
            let receipt = if matches!(operation.action, JournalAction::Checkpoint { .. }) {
                crate::api::checkpoint_journal(store, context, operation)?
            } else {
                crate::api::execute_journal(store, context, operation)?
            };
            JournalJobResponse::Saved(receipt)
        }
        JournalJobRequest::ReadState => {
            let state = store.database.try_with_connection(|tx| {
                let generation = super::store::job(store, tx, context, true)?;
                let current =
                    super::store::journal(tx, &context.host, &generation.journal_id, true)?;
                let current_open = super::store::open(tx, &generation.journal_id)?;
                Ok::<_, JournalError>(JournalJobState {
                    generation_id: generation.id,
                    initial_journal: metadata(&generation.initial_journal),
                    initial_open: generation
                        .initial_open
                        .map(|e| super::store::summary(tx, e))
                        .transpose()?,
                    current_journal: metadata(&current),
                    current_open: current_open
                        .map(|e| super::store::summary(tx, e))
                        .transpose()?,
                    previous_fixed_end: generation.previous_fixed_end,
                    selected_ranges: generation.selected_ranges,
                })
            })?;
            JournalJobResponse::State(state)
        }
        JournalJobRequest::QueryOperation { request_id } => {
            let status = match store.read_journal_operation_status(
                &context.host,
                &generation.journal_id,
                &request_id,
            ) {
                Ok(status) => Some(status),
                Err(JournalError::NotFound(_)) => None,
                Err(error) => return Err(error),
            };
            JournalJobResponse::Operation(status)
        }
        JournalJobRequest::ReadOperationCoverage {
            request_id,
            offset,
            limit,
        } => JournalJobResponse::Coverage(store.read_journal_operation_coverage(
            &context.host,
            &generation.journal_id,
            &request_id,
            offset,
            limit,
        )?),
        JournalJobRequest::ListOperations { after, limit } => {
            let receipts = store.list_journal_operations(
                &context.host,
                &generation.journal_id,
                after.as_deref(),
                limit,
            )?;
            let items = receipts
                .items
                .iter()
                .map(|r| {
                    store.read_journal_operation_status(
                        &context.host,
                        &generation.journal_id,
                        &r.request_id,
                    )
                })
                .collect::<JournalResult<Vec<_>>>()?;
            JournalJobResponse::Operations(JournalPage {
                items,
                next: receipts.next,
            })
        }
        JournalJobRequest::ReadText { read } => {
            let text = match &read.target {
                JournalTextTarget::Source { pointer } => {
                    let range = generation
                        .selected_ranges
                        .iter()
                        .find(|r| {
                            &r.source.pointer == pointer
                                && r.bytes.start <= read.offset as u64
                                && read.offset as u64 <= r.bytes.end
                        })
                        .ok_or_else(|| {
                            JournalError::PermissionDenied(
                                "source bytes are outside this generation".into(),
                            )
                        })?;
                    if !(1..=MAX_JOURNAL_READ_BYTES).contains(&read.limit) {
                        return Err(JournalError::InvalidInput(
                            "invalid source read limit".into(),
                        ));
                    }
                    let condition =
                        serde_json::to_vec(&(&generation.id, &range.source, range.bytes))?;
                    let token: String = Sha256::digest(&condition)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    if read
                        .content_token
                        .as_deref()
                        .is_some_and(|expected| expected != token)
                        || (read.offset as u64 != range.bytes.start && read.content_token.is_none())
                    {
                        return Err(JournalError::Conflict(
                            "source continuation condition missing or changed".into(),
                        ));
                    }
                    let mut material =
                        read_source(&range.source, read.offset as u64, read.limit.max(4))?;
                    if material.range.source != range.source
                        || material.range.bytes.start != read.offset as u64
                        || material.range.bytes.end
                            != (read.offset as u64).saturating_add(material.text.len() as u64)
                        || material.range.bytes.end > range.source.total_bytes
                        || material.text.len() > read.limit.max(4)
                    {
                        return Err(JournalError::Conflict(
                            "source fragment identity or byte range changed".into(),
                        ));
                    }
                    let mut length = material
                        .text
                        .len()
                        .min(read.limit)
                        .min((range.bytes.end - read.offset as u64) as usize);
                    while !material.text.is_char_boundary(length) {
                        length -= 1;
                    }
                    if length == 0 && (read.offset as u64) < range.bytes.end {
                        return Err(JournalError::InvalidInput(
                            "read limit cannot fit a source UTF-8 scalar".into(),
                        ));
                    }
                    material.text.truncate(length);
                    let end = read.offset + length;
                    if length > 0 {
                        delivered = Some((
                            range.source.order,
                            JournalByteRange {
                                start: read.offset as u64,
                                end: end as u64,
                            },
                        ));
                    }
                    JournalTextPage {
                        text: material.text,
                        next_byte: ((end as u64) < range.bytes.end).then_some(end),
                        content_token: token,
                    }
                }
                target => {
                    let (journal, open) = match target {
                        JournalTextTarget::Position { snapshot }
                        | JournalTextTarget::OpenBody { snapshot }
                        | JournalTextTarget::OpenOngoing { snapshot } => match snapshot {
                            JournalSnapshot::Initial => (
                                generation.initial_journal.clone(),
                                generation.initial_open.clone(),
                            ),
                            JournalSnapshot::Current => {
                                store.journal_resume_state(&context.host, &generation.journal_id)?
                            }
                        },
                        JournalTextTarget::Source { .. } => unreachable!(),
                    };
                    let text = match target {
                        JournalTextTarget::Position { .. } => journal.current_position.clone(),
                        JournalTextTarget::OpenBody { .. } => open
                            .as_ref()
                            .ok_or_else(|| JournalError::NotFound("no open entry".into()))?
                            .body
                            .clone(),
                        JournalTextTarget::OpenOngoing { .. } => open
                            .as_ref()
                            .ok_or_else(|| JournalError::NotFound("no open entry".into()))?
                            .ongoing
                            .clone(),
                        JournalTextTarget::Source { .. } => unreachable!(),
                    };
                    let condition = serde_json::to_vec(&(
                        &generation.id,
                        &read.target,
                        &journal.state_token,
                        &text,
                    ))?;
                    text_page(&text, &read, &condition, text.len())?
                }
            };
            JournalJobResponse::Text(text)
        }
    };
    if serde_json::to_vec(&response)?.len() > MAX_JOURNAL_RESPONSE_BYTES {
        return Err(JournalError::InvalidInput(
            "Journal response exceeds bounded tool budget".into(),
        ));
    }
    if let Some((order, range)) = delivered {
        store.database.try_transaction(|tx| {
            // A response that cannot fit the tool budget or loses its live generation must
            // not attest bytes. Lost responses can safely reread the identical source page.
            super::store::job(store, tx, context, true)?;
            let saved: Option<String> = tx.query_row(
                "SELECT ranges_json FROM journal_generation_deliveries WHERE generation_id=?1 AND source_order=?2",
                params![context.generation_id, order], |row| row.get(0),
            ).optional()?;
            let mut ranges: Vec<JournalByteRange> = saved.map(|raw| serde_json::from_str(&raw)).transpose()?.unwrap_or_default();
            ranges.push(range);
            tx.execute(
                "INSERT INTO journal_generation_deliveries(generation_id,source_order,ranges_json) VALUES(?1,?2,?3) ON CONFLICT(generation_id,source_order) DO UPDATE SET ranges_json=excluded.ranges_json",
                params![context.generation_id, order, serde_json::to_string(&super::store::merge(ranges)?)?],
            )?;
            Ok::<_, JournalError>(())
        })?;
    }
    Ok(response)
}

fn require_delivery(
    store: &SubjektivStore,
    context: &JournalJobContext,
    selected: &[JournalSourceRange],
) -> JournalResult<()> {
    store.database.try_with_connection(|tx| {
        super::store::job(store, tx, context, true)?;
        for range in selected {
            let saved: Option<String> = tx.query_row(
                "SELECT ranges_json FROM journal_generation_deliveries WHERE generation_id=?1 AND source_order=?2",
                params![context.generation_id, range.source.order], |row| row.get(0),
            ).optional()?;
            let delivered: Vec<JournalByteRange> = saved.map(|raw| serde_json::from_str(&raw)).transpose()?.unwrap_or_default();
            if !delivered.iter().any(|bytes| bytes.start <= range.bytes.start && range.bytes.end <= bytes.end) {
                return Err(JournalError::Conflict(
                    "read every selected source byte with ReadJournalText before recording or checkpointing".into(),
                ));
            }
        }
        Ok(())
    })
}

/// Atomically saves completion and fences new operations. Exact replay returns the same saved
/// completion even after later generations advance the Journal. Cancelled incomplete generations
/// cannot manufacture completion, and neither fixing an entry nor updating position can substitute
/// for the receipt-backed processing cursor.
pub fn finish(
    store: &SubjektivStore,
    context: &JournalJobContext,
) -> JournalResult<JournalJobCompletion> {
    store.database.try_transaction(|tx| {
        let generation = super::store::job(store, tx, context, false)?;
        let saved: Option<String> = tx
            .query_row(
                "SELECT completion_json FROM journal_generations WHERE id=?1",
                [&generation.id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if let Some(saved) = saved {
            return Ok(serde_json::from_str(&saved)?);
        }
        super::store::job(store, tx, context, true)?;
        super::binding::require_saved_position(tx, &generation.id)?;
        let last = generation
            .selected_ranges
            .last()
            .ok_or_else(|| JournalError::InvalidInput("empty generation selection".into()))?;
        let end = if last.bytes.end == last.source.total_bytes {
            (
                last.source
                    .order
                    .checked_add(1)
                    .ok_or_else(|| JournalError::InvalidInput("source order exhausted".into()))?,
                0,
            )
        } else {
            (last.source.order, last.bytes.end)
        };
        let journal = super::store::journal(tx, &context.host, &generation.journal_id, true)?;
        if (journal.process_order, journal.process_byte) != end {
            return Err(JournalError::Conflict(
                "generation has unreflected selected bytes; save a receipt-backed checkpoint first"
                    .into(),
            ));
        }
        let completion = JournalJobCompletion {
            generation_id: generation.id.clone(),
            state_token: journal.state_token,
            process_order: journal.process_order,
            process_byte: journal.process_byte,
        };
        tx.execute(
            "UPDATE journal_generations SET active=0,completion_json=?2 WHERE id=?1",
            params![generation.id, serde_json::to_string(&completion)?],
        )?;
        Ok(completion)
    })
}
