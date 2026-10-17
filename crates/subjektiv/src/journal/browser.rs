//! Bounded saved-Journal browser projections. No history fetch, reconciliation or execution.
use super::store::{check_host, entry, fit_page, journal, summary};
use super::*;
use rusqlite::params;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryCursor {
    scope: String,
    subject: String,
    journal: Option<String>,
    query: Option<String>,
    correcting: Option<String>,
    created_at: String,
    id: String,
}

fn invalid(message: &str) -> JournalError {
    JournalError::InvalidInput(message.into())
}

impl SubjektivStore {
    pub fn journal_entry_summary(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        entry_id: &str,
    ) -> JournalResult<JournalEntrySummary> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            summary(tx, entry(tx, journal_id, entry_id)?)
        })
    }

    /// Correction targets are capped at sixteen by the writer contract, and may be in
    /// another Journal of this Subject. Include actual Journal IDs rather than guessing.
    pub fn journal_correction_targets(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
        entry_id: &str,
    ) -> JournalResult<Vec<JournalEntrySummary>> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            entry(tx, journal_id, entry_id)?;
            let mut stmt = tx.prepare(
                "SELECT e.journal_id,e.id FROM journal_corrections c
                JOIN journal_entries e ON e.subject_id=c.subject_id AND e.id=c.target_id
                WHERE c.subject_id=?1 AND c.entry_id=?2 ORDER BY e.created_at,e.id LIMIT 16",
            )?;
            let rows = stmt
                .query_map(params![host.subject_id, entry_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(journal_id, id)| summary(tx, entry(tx, &journal_id, &id)?))
                .collect()
        })
    }

    /// Recent entries use immutable creation time and ID as a stable descending keyset.
    /// Search is literal substring matching (not SQL LIKE wildcards), bounded to 256 bytes.
    /// A correcting selector filters saved correction edges, not inferred text or intervals.
    pub fn browse_journal_entries(
        &self,
        host: &JournalHostContext,
        journal_id: Option<&str>,
        query: Option<&str>,
        correcting: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> JournalResult<JournalPage<JournalEntrySummary>> {
        if !(1..=MAX_JOURNAL_PAGE).contains(&limit) {
            return Err(invalid("page limit must be 1..32"));
        }
        if query.is_some_and(|q| q.len() > 256 || q.chars().any(char::is_control)) {
            return Err(invalid(
                "search query must be at most 256 bytes without controls",
            ));
        }
        if correcting.is_some() && journal_id.is_none() {
            return Err(invalid("correction lookup requires a Journal"));
        }
        let cursor: Option<EntryCursor> = after
            .map(|raw| {
                if raw.len() > 4096 {
                    return Err(invalid("entry cursor exceeds byte limit"));
                }
                let cursor: EntryCursor =
                    serde_json::from_str(raw).map_err(|_| invalid("invalid entry cursor"))?;
                if cursor.scope != host.scope_id
                    || cursor.subject != host.subject_id
                    || cursor.journal.as_deref() != journal_id
                    || cursor.query.as_deref() != query
                    || cursor.correcting.as_deref() != correcting
                {
                    return Err(invalid(
                        "entry cursor belongs to another scope, Subject or filter",
                    ));
                }
                Ok(cursor)
            })
            .transpose()?;
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            if let Some(id) = journal_id {
                journal(tx, host, id, false)?;
            }
            if let Some(id) = correcting {
                entry(tx, journal_id.unwrap(), id)?;
            }
            let mut stmt = tx.prepare(
                "SELECT e.journal_id,e.id,e.created_at FROM journal_entries e
                 WHERE e.subject_id=?1 AND (?4 IS NOT NULL OR ?2 IS NULL OR e.journal_id=?2)
                 AND (?3 IS NULL OR instr(lower(e.title),lower(?3))>0
                     OR instr(lower(e.body),lower(?3))>0 OR instr(lower(e.ongoing),lower(?3))>0)
                 AND (?4 IS NULL OR EXISTS(SELECT 1 FROM journal_corrections c
                     WHERE c.subject_id=?1 AND c.entry_id=e.id AND c.target_id=?4))
                 AND (?5 IS NULL OR (e.created_at,e.id)<(?5,?6))
                 ORDER BY e.created_at DESC,e.id DESC LIMIT ?7",
            )?;
            let rows = stmt
                .query_map(
                    params![
                        host.subject_id,
                        journal_id,
                        query,
                        correcting,
                        cursor.as_ref().map(|c| &c.created_at),
                        cursor.as_ref().map(|c| &c.id),
                        limit + 1
                    ],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let more = rows.len() > limit;
            let mut items = Vec::new();
            for (journal_id_value, id, created_at) in rows.into_iter().take(limit) {
                let next = serde_json::to_string(&EntryCursor {
                    scope: host.scope_id.clone(),
                    subject: host.subject_id.clone(),
                    journal: journal_id.map(str::to_owned),
                    query: query.map(str::to_owned),
                    correcting: correcting.map(str::to_owned),
                    created_at,
                    id: id.clone(),
                })?;
                items.push((next, summary(tx, entry(tx, &journal_id_value, &id)?)?));
            }
            fit_page(items, more)
        })
    }

    /// True only for admitted source bytes which the durable checkpoint has not reflected.
    /// False does not claim that all original Session activity was admitted or recorded.
    pub fn journal_has_unreflected_sources(
        &self,
        host: &JournalHostContext,
        journal_id: &str,
    ) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            check_host(self, tx, host, false)?;
            journal(tx, host, journal_id, false)?;
            Ok(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal_sources s, json_each(s.accepted_json) b
                 WHERE s.journal_id=?1 AND json_extract(b.value,'$.end')>s.reflected_bytes)",
                [journal_id],
                |r| r.get(0),
            )?)
        })
    }
}
