//! Journal-specific retention and explicit erasure. Hosts reconcile the final committed public
//! Session before consulting retention; absence from the source ledger is not proof of no work.
//! Ordinary retention never deletes Journal text. Explicit erasure leaves a content-free Session
//! tombstone in the same transaction as removal, fencing delayed admission and Job operations.

use super::*;
use crate::now;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<(), FeatureStorageError> {
    tx.execute_batch(
        "ALTER TABLE journal_generations ADD COLUMN completion_json TEXT;
        CREATE TABLE journal_generation_deliveries (
            generation_id TEXT NOT NULL REFERENCES journal_generations(id) ON DELETE CASCADE,
            source_order INTEGER NOT NULL,
            ranges_json TEXT NOT NULL,
            PRIMARY KEY(generation_id, source_order)
        );
        CREATE TABLE journal_erased_sessions (
            session_id TEXT PRIMARY KEY,
            subject_id TEXT NOT NULL,
            request_id TEXT NOT NULL UNIQUE,
            erased_at TEXT NOT NULL
        );
        DROP TRIGGER journal_operations_no_delete;
        CREATE TRIGGER journal_operations_no_delete BEFORE DELETE ON journal_operations
        WHEN NOT EXISTS (
            SELECT 1 FROM journals j JOIN journal_erased_sessions e
                ON e.session_id=j.source_session_id AND e.subject_id=j.subject_id
            WHERE j.id=OLD.journal_id
        )
        BEGIN SELECT RAISE(ABORT, 'journal operation receipts are retained'); END;",
    )?;
    Ok(())
}

pub(super) fn reject_erased(tx: &Connection, host: &JournalHostContext) -> JournalResult<()> {
    let session = host
        .attribution
        .as_ref()
        .ok_or_else(|| JournalError::PermissionDenied("Session authority required".into()))?;
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM journal_erased_sessions WHERE session_id=?1)",
        [&session.session_id],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(JournalError::PermissionDenied(
            "Session was explicitly erased".into(),
        ));
    }
    Ok(())
}

impl SubjektivStore {
    /// Reads the content-free erasure fence under the original Session attribution.
    /// Hosts use this before source reconciliation so ordinary removal of an erased Session
    /// neither fetches erased material nor forces a new recording generation.
    pub fn journal_session_erased(&self, host: &JournalHostContext) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            super::store::check_session_host(self, tx, host)?;
            let attr = host.attribution.as_ref().expect("checked Session authority");
            Ok(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal_erased_sessions WHERE session_id=?1 AND subject_id=?2)",
                params![attr.session_id, host.subject_id],
                |row| row.get(0),
            )?)
        })
    }

    /// Finds this Host's immutable source Session without scanning unrelated Journals.
    pub fn journal_for_session(&self, host: &JournalHostContext) -> JournalResult<Option<String>> {
        self.database.try_with_connection(|tx| {
            super::store::check_session_host(self, tx, host)?;
            reject_erased(tx, host)?;
            let attr = host
                .attribution
                .as_ref()
                .expect("checked Session authority");
            Ok(tx
                .query_row(
                    "SELECT id FROM journals WHERE source_session_id=?1 AND subject_id=?2",
                    params![attr.session_id, host.subject_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    /// True while any admitted bytes still need reflection, including unavailable bytes.
    /// A Host must first reconcile storage against admission under its deletion/append fence.
    pub fn journal_retention_required(&self, host: &JournalHostContext) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            super::store::check_session_host(self, tx, host)?;
            let attr = host.attribution.as_ref().expect("checked Session authority");
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_erased_sessions WHERE session_id=?1)", [&attr.session_id], |r| r.get::<_,bool>(0))? {
                return Ok(false);
            }
            let mut stmt = tx.prepare("SELECT s.accepted_json,s.reflected_bytes FROM journal_sources s JOIN journals j ON j.id=s.journal_id WHERE j.source_session_id=?1 AND j.subject_id=?2 ORDER BY s.source_order")?;
            let mut rows = stmt.query(params![attr.session_id,host.subject_id])?;
            while let Some(row) = rows.next()? {
                let accepted: Vec<JournalByteRange> = serde_json::from_str(&row.get::<_, String>(0)?)?;
                let reflected: u64 = row.get(1)?;
                if accepted.iter().any(|range| range.end > reflected) {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// Explicit Host-authorized complete erasure, not an ordinary Worker removal policy.
    /// No generation is forced. The tombstone remains even if no Journal was admitted yet.
    /// Exact retries converge; a request ID cannot be reused to erase a different Session.
    pub fn erase_journal_session(
        &self,
        host: &JournalHostContext,
        request_id: &str,
    ) -> JournalResult<()> {
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
        {
            return Err(JournalError::InvalidInput(
                "invalid erasure request identifier".into(),
            ));
        }
        self.database.try_transaction(|tx| {
            super::store::check_session_host(self, tx, host)?;
            let attr = host.attribution.as_ref().expect("checked Session authority");
            let previous: Option<(String,String)> = tx.query_row("SELECT session_id,subject_id FROM journal_erased_sessions WHERE request_id=?1", [request_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if previous.as_ref().is_some_and(|(session,subject)| session!=&attr.session_id || subject!=&host.subject_id) {
                return Err(JournalError::Conflict("erasure request belongs to a different Session".into()));
            }
            let erased_by: Option<String> = tx.query_row("SELECT request_id FROM journal_erased_sessions WHERE session_id=?1", [&attr.session_id], |row| row.get(0)).optional()?;
            if erased_by.as_deref().is_some_and(|key| key != request_id) {
                return Err(JournalError::Conflict("Session already erased by a different request".into()));
            }
            tx.execute("INSERT OR IGNORE INTO journal_erased_sessions(session_id,subject_id,request_id,erased_at) VALUES(?1,?2,?3,?4)", params![attr.session_id,host.subject_id,request_id,now()])?;
            let journal: Option<String> = tx.query_row("SELECT id FROM journals WHERE source_session_id=?1 AND subject_id=?2",params![attr.session_id,host.subject_id],|r|r.get(0)).optional()?;
            if let Some(id) = journal {
                // The erasure marker is the only condition permitting receipt deletion. Normal
                // record/checkpoint/retention paths retain their immutable receipts.
                tx.execute("DELETE FROM journal_operations WHERE journal_id=?1",[&id])?;
                tx.execute("DELETE FROM journal_generations WHERE journal_id=?1",[&id])?;
                tx.execute("DELETE FROM journal_corrections WHERE entry_id IN (SELECT id FROM journal_entries WHERE journal_id=?1) OR target_id IN (SELECT id FROM journal_entries WHERE journal_id=?1)",[&id])?;
                tx.execute("DELETE FROM journal_entries WHERE journal_id=?1",[&id])?;
                tx.execute("DELETE FROM journal_sources WHERE journal_id=?1",[&id])?;
                tx.execute("DELETE FROM journals WHERE id=?1",[&id])?;
            }
            Ok(())
        })
    }
}
