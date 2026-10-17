//! Optional Host metadata observations for original material, independent of saved Journal reading.
//!
//! Presence checks must not fetch original history or restore a Worker. A failed/unauthorized
//! metadata lookup is unknown, not missing, and cannot prevent a saved Journal body from being read.

use super::memory_links::{
    MemoryJournalRequest, MemoryJournalResponse, OriginalSourceAvailability,
};
use super::{JournalHostContext, JournalResult, SubjektivStore};
use rusqlite::{OptionalExtension, params};

impl SubjektivStore {
    /// Executes the saved-reference operation first, then annotates original-source absence using
    /// Host-owned metadata only. `Some(false)` is confirmed absence; `None` is an unavailable lookup.
    /// Presence alone cannot attest that a particular historical entry is still readable.
    pub fn execute_memory_journal_with_source_presence(
        &self,
        host: &JournalHostContext,
        request: MemoryJournalRequest,
        mut presence: impl FnMut(&str) -> Option<bool>,
    ) -> JournalResult<MemoryJournalResponse> {
        let selector = match &request {
            MemoryJournalRequest::Reconcile { selector }
            | MemoryJournalRequest::Links { selector }
            | MemoryJournalRequest::ReconcilePage { selector, .. }
            | MemoryJournalRequest::LinksPage { selector, .. }
            | MemoryJournalRequest::Read { selector, .. } => Some(selector.clone()),
            MemoryJournalRequest::Related { .. } => None,
        };
        let mut response = self.execute_memory_journal(host, request)?;
        let Some(selector) = selector else {
            return Ok(response);
        };
        // Membership and scoped reader authority were checked by the operation. Candidate JSON is
        // immutable; never infer source ownership from its presence without checking attribution.
        let candidate = self.staging_candidate(&host.subject_id, &selector.candidate_id)?;
        let mut observations = std::collections::HashMap::new();
        let mut annotate =
            |link: &mut super::memory_links::MemoryJournalSourceLink| -> JournalResult<()> {
                let source = candidate
                    .as_ref()
                    .and_then(|c| c.source_refs.get(link.source_index));
                // Legacy anchors may lack a Session ID. A connected edge already resolved it
                // from immutable attribution and stable Segment/entry identity; reuse that saved
                // identity rather than fetching history or modifying the original candidate.
                let saved_session: Option<String> = self.database.try_with_connection(|tx| {
                    Ok::<_, super::JournalError>(tx.query_row(
                        "SELECT source_session_id FROM memory_journal_links
                         WHERE subject_id=?1 AND candidate_id=?2 AND source_index=?3 AND reason='linked'",
                        params![host.subject_id, selector.candidate_id, link.source_index],
                        |row| row.get(0),
                    ).optional()?)
                })?;
                let session = saved_session
                    .as_deref()
                    .or_else(|| source.and_then(|s| s.session_id.as_deref()));
                let observed = if let Some(session) = session {
                    if let Some(value) = observations.get(session) {
                        *value
                    } else {
                        let owned = self
                            .session_attribution(session)?
                            .is_some_and(|a| a.subject_id == host.subject_id);
                        let value = if owned { presence(session) } else { None };
                        observations.insert(session.to_owned(), value);
                        value
                    }
                } else {
                    None
                };
                link.original_source = match observed {
                    Some(false) => OriginalSourceAvailability::Missing,
                    // Preserve explicit saved missing-byte attestations. Directory/registry presence
                    // does not prove the referenced entry survived rewind or storage corruption.
                    _ if link.original_source == OriginalSourceAvailability::Missing => {
                        OriginalSourceAvailability::Missing
                    }
                    _ => OriginalSourceAvailability::Unknown,
                };
                Ok(())
            };
        match &mut response {
            MemoryJournalResponse::Links(page) => {
                for source in &mut page.sources {
                    annotate(source)?;
                }
            }
            MemoryJournalResponse::Read(read) => annotate(&mut read.source)?,
            MemoryJournalResponse::Related(_) => {}
        }
        Ok(response)
    }
}
