//! Additive, immutable provenance-to-context links, not additional evidence for a Memory claim.
//!
//! Reconciliation reads only saved candidate anchors and the Host-attested Journal source ledger.
//! It never fetches history, starts Workers, changes extraction/consolidation, fixes an open event,
//! or mutates Memory. A stable evidence reference identifies a public entry, not its legacy numeric
//! display range. Missing stable identity stays unconnected; numeric ranges are not guessed to be
//! canonical order. Saved edges remain usable without the original runtime and cannot be retargeted.
//! Journal content is never copied here. Explicit erasure retains edges and exposes the tombstone.

use super::{
    JournalError, JournalHostContext, JournalResult, JournalSourcePointer, JournalTextField,
    JournalTextPage, MAX_JOURNAL_PAGE, MAX_JOURNAL_RESPONSE_BYTES,
};
use crate::{MAX_STAGING_ANCHORS, SubjectStagingRecord, SubjektivStore};
use memory::schema::SourceEvidenceRef;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

pub(crate) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("schema.sql"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJournalSelector {
    pub memory_id: String,
    pub change_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryJournalRequest {
    Reconcile {
        selector: MemoryJournalSelector,
    },
    /// Reads saved link state only; does not implicitly retry reconciliation.
    Links {
        selector: MemoryJournalSelector,
    },
    /// Continues reconciliation over immutable source indices, at most MAX_STAGING_ANCHORS per call.
    ReconcilePage {
        selector: MemoryJournalSelector,
        source_offset: usize,
        limit: usize,
    },
    /// Reads another saved source page without reconciliation.
    LinksPage {
        selector: MemoryJournalSelector,
        source_offset: usize,
        limit: usize,
    },
    Read {
        selector: MemoryJournalSelector,
        source_index: usize,
        journal_id: String,
        entry_id: String,
        expected_token: Option<String>,
        byte_offset: usize,
        max_bytes: usize,
    },
    Related {
        journal_id: String,
        entry_id: String,
        after: Option<RelatedMemoryCursor>,
        limit: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MemoryJournalResponse {
    Links(MemoryJournalLinks),
    Read(MemoryJournalRead),
    Related(RelatedMemories),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginalSourceAvailability {
    /// No saved Host attestation. None of these values imply a live history fetch.
    Unknown,
    Missing,
    Available,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalTargetAvailability {
    Available,
    Unavailable,
    Erased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnconnectedReason {
    NotReconciled,
    InsufficientSource,
    SourceNotAdmitted,
    AmbiguousSource,
    JournalNotFixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceConnection {
    Unconnected {
        reason: UnconnectedReason,
    },
    Linked {
        journal_id: String,
        entry_id: String,
        target: JournalTargetAvailability,
        /// A saved correction invites reconsideration, never an automatic Memory edit.
        reconsideration: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJournalSourceLink {
    pub source_index: usize,
    pub original_source: OriginalSourceAvailability,
    pub connection: SourceConnection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJournalLinks {
    pub selector: MemoryJournalSelector,
    pub sources: Vec<MemoryJournalSourceLink>,
    pub total_sources: usize,
    pub next_source_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJournalRead {
    pub source: MemoryJournalSourceLink,
    /// Absent for an unavailable or explicitly erased target; never falls back to another entry.
    pub page: Option<JournalTextPage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedMemoryCursor {
    pub memory_id: String,
    pub change_id: String,
    pub candidate_id: String,
    pub source_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedMemory {
    pub selector: MemoryJournalSelector,
    pub source_index: usize,
    pub reconsideration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedMemories {
    pub items: Vec<RelatedMemory>,
    pub next: Option<RelatedMemoryCursor>,
}

fn invalid(message: &str) -> JournalError {
    JournalError::InvalidInput(message.into())
}

fn identifier(value: &str) -> JournalResult<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(invalid("invalid Memory/Journal selector identifier"));
    }
    Ok(())
}

fn selector_valid(selector: &MemoryJournalSelector) -> JournalResult<()> {
    identifier(&selector.memory_id)?;
    identifier(&selector.change_id)?;
    identifier(&selector.candidate_id)
}

fn candidate(
    store: &SubjektivStore,
    tx: &Connection,
    host: &JournalHostContext,
    selector: &MemoryJournalSelector,
) -> JournalResult<SubjectStagingRecord> {
    super::store::check_host(store, tx, host, false)?;
    selector_valid(selector)?;
    // The immutable change-candidate edge, not the current Memory projection or a supplied
    // candidate ID alone, authorizes this provenance selector.
    let raw: Option<String> = tx
        .query_row(
            "SELECT s.record_json FROM staging_records s JOIN memory_change_candidates c
            ON c.subject_id=s.subject_id AND c.candidate_id=s.candidate_id
         WHERE c.subject_id=?1 AND c.memory_id=?2 AND c.change_id=?3 AND c.candidate_id=?4",
            params![
                host.subject_id,
                selector.memory_id,
                selector.change_id,
                selector.candidate_id
            ],
            |r| r.get(0),
        )
        .optional()?;
    let record: SubjectStagingRecord = serde_json::from_str(
        &raw.ok_or_else(|| JournalError::NotFound("Memory change candidate membership".into()))?,
    )?;
    Ok(record)
}

#[derive(Debug)]
struct SavedLink {
    reason: String,
    journal_id: Option<String>,
    entry_id: Option<String>,
    session_id: Option<String>,
    order: Option<u64>,
}

fn saved(
    tx: &Connection,
    host: &JournalHostContext,
    candidate_id: &str,
    index: usize,
) -> JournalResult<Option<SavedLink>> {
    Ok(tx.query_row(
        "SELECT reason,journal_id,entry_id,source_session_id,source_order FROM memory_journal_links
         WHERE subject_id=?1 AND candidate_id=?2 AND source_index=?3",
        params![host.subject_id, candidate_id, index],
        |r| Ok(SavedLink { reason:r.get(0)?,journal_id:r.get(1)?,entry_id:r.get(2)?,session_id:r.get(3)?,order:r.get(4)? }),
    ).optional()?)
}

fn original_availability(
    tx: &Connection,
    journal: &str,
    order: u64,
) -> JournalResult<OriginalSourceAvailability> {
    let unavailable: Option<String> = tx
        .query_row(
            "SELECT unavailable_json FROM journal_sources WHERE journal_id=?1 AND source_order=?2",
            params![journal, order],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match unavailable {
        None => OriginalSourceAvailability::Unknown,
        Some(raw) => {
            let ranges: Vec<super::JournalByteRange> = serde_json::from_str(&raw)?;
            if ranges.is_empty() {
                OriginalSourceAvailability::Available
            } else {
                OriginalSourceAvailability::Missing
            }
        }
    })
}

fn project(
    tx: &Connection,
    host: &JournalHostContext,
    candidate_id: &str,
    index: usize,
) -> JournalResult<MemoryJournalSourceLink> {
    let Some(link) = saved(tx, host, candidate_id, index)? else {
        return Ok(MemoryJournalSourceLink {
            source_index: index,
            original_source: OriginalSourceAvailability::Unknown,
            connection: SourceConnection::Unconnected {
                reason: UnconnectedReason::NotReconciled,
            },
        });
    };
    if link.reason != "linked" {
        let reason = match link.reason.as_str() {
            "insufficient_source" => UnconnectedReason::InsufficientSource,
            "source_not_admitted" => UnconnectedReason::SourceNotAdmitted,
            "ambiguous_source" => UnconnectedReason::AmbiguousSource,
            "journal_not_fixed" => UnconnectedReason::JournalNotFixed,
            _ => return Err(invalid("invalid saved connection reason")),
        };
        return Ok(MemoryJournalSourceLink {
            source_index: index,
            original_source: OriginalSourceAvailability::Unknown,
            connection: SourceConnection::Unconnected { reason },
        });
    }
    let journal_id = link
        .journal_id
        .ok_or_else(|| invalid("saved link lacks Journal"))?;
    let entry_id = link
        .entry_id
        .ok_or_else(|| invalid("saved link lacks entry"))?;
    let session = link
        .session_id
        .ok_or_else(|| invalid("saved link lacks Session"))?;
    let order = link
        .order
        .ok_or_else(|| invalid("saved link lacks source order"))?;
    let erased: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM journal_erased_sessions WHERE subject_id=?1 AND session_id=?2)",
        params![host.subject_id, session], |r| r.get(0),
    )?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM journal_entries e JOIN journals j ON j.id=e.journal_id
         WHERE e.subject_id=?1 AND e.journal_id=?2 AND e.id=?3 AND j.source_session_id=?4)",
        params![host.subject_id, journal_id, entry_id, session],
        |r| r.get(0),
    )?;
    let reconsideration = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM journal_corrections WHERE subject_id=?1 AND target_id=?2)",
        params![host.subject_id, entry_id],
        |r| r.get(0),
    )?;
    Ok(MemoryJournalSourceLink {
        source_index: index,
        original_source: original_availability(tx, &journal_id, order)?,
        connection: SourceConnection::Linked {
            journal_id,
            entry_id,
            target: if erased {
                JournalTargetAvailability::Erased
            } else if exists {
                JournalTargetAvailability::Available
            } else {
                JournalTargetAvailability::Unavailable
            },
            reconsideration,
        },
    })
}

struct Resolved {
    journal: String,
    entry: String,
    session: String,
    order: u64,
}

fn resolve(
    tx: &Connection,
    host: &JournalHostContext,
    source: &SourceEvidenceRef,
) -> JournalResult<Result<Resolved, &'static str>> {
    let Some(evidence) = source.evidence_id.as_deref() else {
        return Ok(Err("insufficient_source"));
    };
    if !evidence.starts_with('E')
        || evidence.len() < 2
        || evidence.len() > 129
        || !evidence[1..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || (source.session_id.is_none() && source.segment_id.is_none())
    {
        return Ok(Err("insufficient_source"));
    }
    // Segment identity may be omitted only when an immutable Session attribution disambiguates it.
    // A candidate's primary legacy source range is not substituted for an individual anchor.
    let segment = match source.segment_id.as_deref() {
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(Err("insufficient_source")),
        },
        None => None,
    };
    let mut stmt = tx.prepare(
        "SELECT j.id,j.source_session_id,s.source_order,s.pointer_json FROM journal_sources s
         JOIN journals j ON j.id=s.journal_id
         JOIN subject_session_attributions a ON a.subject_id=j.subject_id AND a.session_id=j.source_session_id
         WHERE j.subject_id=?1 AND s.entry_ref=?2
            AND (?3 IS NULL OR j.source_session_id=?3)
            AND (?4 IS NULL OR json_extract(s.pointer_json,'$.segment_id')=?4)
         LIMIT 2",
    )?;
    let found = stmt
        .query_map(
            params![
                host.subject_id,
                evidence,
                source.session_id,
                segment.map(|s| s.to_string())
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if found.is_empty() {
        return Ok(Err("source_not_admitted"));
    }
    if found.len() != 1 {
        return Ok(Err("ambiguous_source"));
    }
    let (journal, session, order, pointer) = &found[0];
    let pointer: JournalSourcePointer = serde_json::from_str(pointer)?;
    if pointer.entry_ref != evidence || segment.is_some_and(|s| s != pointer.segment_id) {
        return Ok(Err("ambiguous_source"));
    }
    // A fixed endpoint covers (previous_fixed_end, source_end]. The Session start is the first
    // interval's lower boundary. Processing checkpoints and open entries cannot supply an end.
    let mut stmt = tx.prepare(
        "SELECT e.id FROM journal_entries e WHERE e.subject_id=?1 AND e.journal_id=?2
            AND e.state='fixed' AND e.source_order>=?3
            AND NOT EXISTS(SELECT 1 FROM journal_entries p WHERE p.journal_id=e.journal_id
                AND p.position<e.position AND p.state='fixed' AND p.source_order>=?3)
         ORDER BY e.position LIMIT 2",
    )?;
    let entries = stmt
        .query_map(params![host.subject_id, journal, order], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    match entries.as_slice() {
        [entry] => Ok(Ok(Resolved {
            journal: journal.clone(),
            entry: entry.clone(),
            session: session.clone(),
            order: *order,
        })),
        [] => Ok(Err("journal_not_fixed")),
        _ => Ok(Err("ambiguous_source")),
    }
}

fn reconcile(
    tx: &Connection,
    host: &JournalHostContext,
    record: &SubjectStagingRecord,
    indices: std::ops::Range<usize>,
) -> JournalResult<()> {
    for index in indices {
        let source = &record.source_refs[index];
        if saved(tx, host, &record.id, index)?.is_some_and(|s| s.reason == "linked") {
            continue;
        }
        let (reason, resolved) = match resolve(tx, host, source)? {
            Ok(link) => ("linked", Some(link)),
            Err(reason) => (reason, None),
        };
        tx.execute(
            "INSERT INTO memory_journal_links(subject_id,candidate_id,source_index,reason,
                journal_id,entry_id,source_session_id,source_order) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(subject_id,candidate_id,source_index) DO UPDATE SET
                reason=excluded.reason,journal_id=excluded.journal_id,entry_id=excluded.entry_id,
                source_session_id=excluded.source_session_id,source_order=excluded.source_order",
            params![
                host.subject_id,
                record.id,
                index,
                reason,
                resolved.as_ref().map(|r| r.journal.as_str()),
                resolved.as_ref().map(|r| r.entry.as_str()),
                resolved.as_ref().map(|r| r.session.as_str()),
                resolved.as_ref().map(|r| r.order)
            ],
        )?;
    }
    Ok(())
}

impl SubjektivStore {
    /// Scoped Memory provenance bridge. Reconcile is additive and idempotent, so a lost response
    /// needs no operation ledger: retry the same selector and reread saved edges. Reads never reconcile.
    pub fn execute_memory_journal(
        &self,
        host: &JournalHostContext,
        request: MemoryJournalRequest,
    ) -> JournalResult<MemoryJournalResponse> {
        let response = match request {
            MemoryJournalRequest::Reconcile { selector } => {
                self.memory_journal_links(host, selector, true, 0, MAX_STAGING_ANCHORS)?
            }
            MemoryJournalRequest::Links { selector } => {
                self.memory_journal_links(host, selector, false, 0, MAX_STAGING_ANCHORS)?
            }
            MemoryJournalRequest::ReconcilePage {
                selector,
                source_offset,
                limit,
            } => self.memory_journal_links(host, selector, true, source_offset, limit)?,
            MemoryJournalRequest::LinksPage {
                selector,
                source_offset,
                limit,
            } => self.memory_journal_links(host, selector, false, source_offset, limit)?,
            MemoryJournalRequest::Read {
                selector,
                source_index,
                journal_id,
                entry_id,
                expected_token,
                byte_offset,
                max_bytes,
            } => {
                identifier(&journal_id)?;
                identifier(&entry_id)?;
                if !(4..=super::MAX_JOURNAL_READ_BYTES).contains(&max_bytes) {
                    return Err(invalid("text page must be 4..16384 bytes"));
                }
                if let Some(token) = expected_token.as_deref() {
                    identifier(token)?;
                }
                let mut source = self.database.try_with_connection(|tx| {
                    let record = candidate(self, tx, host, &selector)?;
                    if source_index >= record.source_refs.len() {
                        return Err(invalid("source index outside candidate anchors"));
                    }
                    let link = project(tx, host, &selector.candidate_id, source_index)?;
                    match &link.connection {
                        SourceConnection::Linked {
                            journal_id: saved_journal,
                            entry_id: saved_entry,
                            ..
                        } if saved_journal == &journal_id && saved_entry == &entry_id => Ok(link),
                        _ => Err(JournalError::PermissionDenied(
                            "read requires the exact saved Journal link".into(),
                        )),
                    }
                })?;
                let page = match &source.connection {
                    SourceConnection::Linked {
                        target: JournalTargetAvailability::Available,
                        ..
                    } => {
                        match self.read_journal_text(
                            host,
                            &journal_id,
                            Some(&entry_id),
                            JournalTextField::Body,
                            expected_token.as_deref(),
                            byte_offset,
                            max_bytes,
                        ) {
                            Ok(page) => Some(page),
                            // Erasure between the saved-edge check and shared text read must not
                            // fall back, expose cached text, or pretend the link never existed.
                            Err(JournalError::NotFound(_)) => {
                                source = self.database.try_with_connection(|tx| {
                                    project(tx, host, &selector.candidate_id, source_index)
                                })?;
                                None
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    _ => None,
                };
                MemoryJournalResponse::Read(MemoryJournalRead { source, page })
            }
            MemoryJournalRequest::Related {
                journal_id,
                entry_id,
                after,
                limit,
            } => self.related_memories(host, &journal_id, &entry_id, after.as_ref(), limit)?,
        };
        if serde_json::to_string_pretty(&response)?.len() > MAX_JOURNAL_RESPONSE_BYTES {
            return Err(invalid("Memory Journal response exceeds byte budget"));
        }
        Ok(response)
    }

    fn memory_journal_links(
        &self,
        host: &JournalHostContext,
        selector: MemoryJournalSelector,
        do_reconcile: bool,
        source_offset: usize,
        limit: usize,
    ) -> JournalResult<MemoryJournalResponse> {
        if !(1..=MAX_STAGING_ANCHORS).contains(&limit) {
            return Err(invalid("source page exceeds staging anchor bound"));
        }
        self.database.try_transaction(|tx| {
            let record = candidate(self, tx, host, &selector)?;
            let total_sources = record.source_refs.len();
            if source_offset > total_sources {
                return Err(invalid("source offset outside immutable candidate anchors"));
            }
            let end = source_offset.saturating_add(limit).min(total_sources);
            if do_reconcile {
                reconcile(tx, host, &record, source_offset..end)?;
            }
            let sources = (source_offset..end)
                .map(|index| project(tx, host, &selector.candidate_id, index))
                .collect::<JournalResult<Vec<_>>>()?;
            Ok(MemoryJournalResponse::Links(MemoryJournalLinks {
                selector,
                sources,
                total_sources,
                next_source_index: (end < total_sources).then_some(end),
            }))
        })
    }

    fn related_memories(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        entry_id: &str,
        after: Option<&RelatedMemoryCursor>,
        limit: usize,
    ) -> JournalResult<MemoryJournalResponse> {
        identifier(journal_id)?;
        identifier(entry_id)?;
        if !(1..=MAX_JOURNAL_PAGE).contains(&limit) {
            return Err(invalid("related page must be 1..32"));
        }
        if let Some(cursor) = after {
            identifier(&cursor.memory_id)?;
            identifier(&cursor.change_id)?;
            identifier(&cursor.candidate_id)?;
            if i64::try_from(cursor.source_index).is_err() {
                return Err(invalid("invalid related source index"));
            }
        }
        self.database.try_with_connection(|tx| {
            super::store::check_host(self,tx,host,false)?;
            // Saved target identity authorizes reverse lookup after erasure as well as before it.
            let known:bool=tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal_entries WHERE subject_id=?1 AND journal_id=?2 AND id=?3)
                    OR EXISTS(SELECT 1 FROM memory_journal_links WHERE subject_id=?1 AND journal_id=?2 AND entry_id=?3)",
                params![host.subject_id,journal_id,entry_id],|r|r.get(0))?;
            if !known { return Err(JournalError::NotFound("Journal entry".into())); }
            let mut stmt=tx.prepare(
                "SELECT m.memory_id,m.change_id,l.candidate_id,l.source_index,
                    EXISTS(SELECT 1 FROM journal_corrections c WHERE c.subject_id=l.subject_id AND c.target_id=l.entry_id)
                 FROM memory_journal_links l JOIN memory_change_candidates m
                    ON m.subject_id=l.subject_id AND m.candidate_id=l.candidate_id
                 WHERE l.subject_id=?1 AND l.reason='linked'
                    AND ((l.journal_id=?2 AND l.entry_id=?3)
                        OR EXISTS(SELECT 1 FROM journal_corrections c WHERE c.subject_id=?1 AND c.entry_id=?3 AND c.target_id=l.entry_id))
                    AND (?4 IS NULL OR (m.memory_id,m.change_id,l.candidate_id,l.source_index)>(?4,?5,?6,?7))
                 ORDER BY m.memory_id,m.change_id,l.candidate_id,l.source_index LIMIT ?8")?;
            let mut items=stmt.query_map(params![host.subject_id,journal_id,entry_id,
                after.map(|c|c.memory_id.as_str()),after.map(|c|c.change_id.as_str()),
                after.map(|c|c.candidate_id.as_str()),after.map(|c|c.source_index),limit+1],
                |r|Ok(RelatedMemory {selector:MemoryJournalSelector {memory_id:r.get(0)?,change_id:r.get(1)?,candidate_id:r.get(2)?},
                    source_index:r.get(3)?,reconsideration:r.get(4)?}))?.collect::<Result<Vec<_>,_>>()?;
            let more=items.len()>limit;
            items.truncate(limit);
            let next=if more {items.last().map(|last|RelatedMemoryCursor {
                memory_id:last.selector.memory_id.clone(),change_id:last.selector.change_id.clone(),
                candidate_id:last.selector.candidate_id.clone(),source_index:last.source_index })} else {None};
            Ok(MemoryJournalResponse::Related(RelatedMemories {items,next}))
        })
    }
}
