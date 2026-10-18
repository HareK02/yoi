//! Read-only, bounded Journal context at body startup and compaction boundaries.
//! Selection, text projection, references and processing checkpoints share one storage
//! transaction. Reads neither consult source history/Job liveness nor mutate recording.
//! Startup prefers the Host-retained predecessor; compaction requires a live body writer
//! and prefers its current Session, without waiting for Idle or recording completion.
//! Both supplement only from the same Subject under the same independent budgets.
//! This is derived reference material, never a new observation or an execution capability.

use super::*;
use rusqlite::{Connection, OptionalExtension, params};

pub const STARTUP_MAX_BYTES: usize = 32 * 1024;
const JOURNALS: usize = 3;
const ENTRIES: usize = 3;
const TEXT_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalStartupSnapshot {
    pub subject_id: String,
    pub journals: Vec<StartupJournal>,
    pub omitted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupJournal {
    pub journal_id: String,
    pub source_session_id: String,
    pub state_token: String,
    pub process_order: u64,
    pub process_byte: u64,
    pub current_position: String,
    pub entries: Vec<StartupEntry>,
    pub omitted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupEntry {
    pub entry_id: String,
    pub position: u64,
    pub state: JournalEntryState,
    pub title: String,
    pub body: String,
    pub ongoing: String,
    pub source_end: Option<JournalSourcePointer>,
    pub correction_refs: Vec<String>,
    pub omitted: bool,
}

// Read a bounded UTF-8 prefix at SQLite, not a full megabyte field followed by truncation.
fn text(tx: &Connection, table: &str, id: &str, field: &str) -> JournalResult<(String, bool)> {
    let (mut bytes, length): (Vec<u8>, usize) = tx.query_row(
        &format!("SELECT COALESCE(substr(CAST({field} AS BLOB),1,?2),X''),length(CAST({field} AS BLOB)) FROM {table} WHERE id=?1"),
        params![id, TEXT_BYTES],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let length_read = bytes.len();
    match std::str::from_utf8(&bytes) {
        Ok(_) => {}
        Err(error) if error.error_len().is_none() => bytes.truncate(error.valid_up_to()),
        Err(_) => {
            return Err(JournalError::Conflict(
                "stored Journal text is not UTF-8".into(),
            ));
        }
    }
    let omitted = bytes.len() < length || length_read < length;
    Ok((String::from_utf8(bytes).expect("validated UTF-8"), omitted))
}

impl SubjektivStore {
    /// `previous_session` comes from the Host's retained predecessor binding, not model input.
    /// Recent Subject-local Journals supplement that predecessor under independent fixed budgets.
    pub fn journal_startup_snapshot(
        &self,
        host: &JournalHostContext,
        previous_session: Option<&str>,
    ) -> JournalResult<JournalStartupSnapshot> {
        self.context_snapshot(host, previous_session, false)
    }

    /// Read the saved current-Session Journal without reconciling or advancing it.
    /// The active writer check fences released body lifetimes, not non-Idle execution.
    pub fn journal_compaction_snapshot(
        &self,
        host: &JournalHostContext,
    ) -> JournalResult<JournalStartupSnapshot> {
        let session = host.attribution.as_ref().ok_or_else(|| {
            JournalError::PermissionDenied("compaction requires a Host-bound body Session".into())
        })?;
        self.context_snapshot(host, Some(&session.session_id), true)
    }

    fn context_snapshot(
        &self,
        host: &JournalHostContext,
        preferred_session: Option<&str>,
        require_current_writer: bool,
    ) -> JournalResult<JournalStartupSnapshot> {
        self.database.try_transaction(|tx| {
            super::store::check_host(self, tx, host, require_current_writer)?;
            let mut ids = Vec::new();
            if let Some(session) = preferred_session {
                let previous: Option<(String, String)> = tx.query_row(
                    "SELECT id,subject_id FROM journals WHERE source_session_id=?1",
                    [session], |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional()?;
                if let Some((id, subject)) = previous {
                    if subject != host.subject_id { return Err(JournalError::PermissionDenied("predecessor Journal belongs to another Subject".into())); }
                    ids.push(id);
                }
            }
            let mut query = tx.prepare("SELECT id FROM journals WHERE subject_id=?1 ORDER BY updated_at DESC,id DESC LIMIT ?2")?;
            let recent = query.query_map(params![host.subject_id, JOURNALS + 1], |r| r.get::<_, String>(0))?;
            for id in recent { let id = id?; if !ids.contains(&id) { ids.push(id); } }
            let omitted = ids.len() > JOURNALS;
            ids.truncate(JOURNALS);
            let mut snapshot = JournalStartupSnapshot { subject_id: host.subject_id.clone(), journals: Vec::new(), omitted };
            for id in ids {
                let (session, token, order, byte) = tx.query_row(
                    "SELECT source_session_id,state_token,process_order,process_byte FROM journals WHERE id=?1 AND subject_id=?2",
                    params![id, host.subject_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
                )?;
                let (current_position, mut omitted) = text(tx, "journals", &id, "current_position")?;
                let mut stmt = tx.prepare("SELECT id,position,state,source_end_json FROM journal_entries WHERE journal_id=?1 ORDER BY position DESC LIMIT ?2")?;
                let rows = stmt.query_map(params![id, ENTRIES + 1], |r| Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
                omitted |= rows.len() > ENTRIES;
                let mut entries = Vec::new();
                for (entry_id, position, state, source_end) in rows.into_iter().take(ENTRIES) {
                    let (title, title_cut) = text(tx, "journal_entries", &entry_id, "title")?;
                    let (body, body_cut) = text(tx, "journal_entries", &entry_id, "body")?;
                    let (ongoing, ongoing_cut) = text(tx, "journal_entries", &entry_id, "ongoing")?;
                    let mut refs = tx.prepare("SELECT target_id FROM journal_corrections WHERE entry_id=?1 ORDER BY target_id LIMIT 17")?;
                    let mut correction_refs = refs.query_map([&entry_id], |r| r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                    let omitted = title_cut || body_cut || ongoing_cut || correction_refs.len() > 16;
                    correction_refs.truncate(16);
                    entries.push(StartupEntry { entry_id, position, state: match state.as_str() { "open" => JournalEntryState::Open, "fixed" => JournalEntryState::Fixed, _ => return Err(JournalError::Conflict("invalid stored Journal entry state".into())) }, title, body, ongoing, source_end: source_end.map(|s| serde_json::from_str(&s)).transpose()?, correction_refs, omitted });
                }
                snapshot.journals.push(StartupJournal { journal_id: id, source_session_id: session, state_token: token, process_order: order, process_byte: byte, current_position, entries, omitted });
            }
            // JSON escaping also consumes the independent output budget. Drop least-priority
            // entries/Journals rather than returning an over-budget or silently partial document.
            while serde_json::to_vec(&snapshot)?.len() > STARTUP_MAX_BYTES {
                snapshot.omitted = true;
                let journal = snapshot.journals.last_mut().ok_or_else(|| JournalError::InvalidInput("startup metadata exceeds budget".into()))?;
                if journal.entries.pop().is_some() { journal.omitted = true; } else { snapshot.journals.pop(); }
            }
            Ok(snapshot)
        })
    }
}
