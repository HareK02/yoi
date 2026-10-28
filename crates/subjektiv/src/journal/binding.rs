//! Durable Journal writer lifetime, issued only from the Host's body binding. Kernel leases
//! and Runtime liveness do not end ownership. The immutable binding ID fences retained contexts
//! even when the same Worker later reconnects. Hosts serialize release with body lifecycle and
//! reconcile final committed history before calling release; this store checks saved effects.

use super::*;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalWriterBinding {
    pub subject_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub binding_id: String,
}

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<(), FeatureStorageError> {
    tx.execute_batch("CREATE TABLE journal_writer_bindings (
        binding_id TEXT PRIMARY KEY,
        subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
        runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL,
        active INTEGER NOT NULL CHECK(active IN (0,1))
    );
    CREATE UNIQUE INDEX journal_one_body_binding ON journal_writer_bindings(subject_id) WHERE active=1;
    CREATE TABLE journal_session_bindings (
        session_id TEXT PRIMARY KEY REFERENCES subject_session_attributions(session_id) ON DELETE RESTRICT,
        binding_id TEXT NOT NULL REFERENCES journal_writer_bindings(binding_id) ON DELETE RESTRICT
    );
    CREATE INDEX journal_body_history ON journal_writer_bindings(subject_id,active);
    CREATE INDEX journal_body_sessions ON journal_session_bindings(binding_id);
    CREATE INDEX journal_recent_subject ON journals(subject_id,updated_at DESC,id DESC);")?;
    Ok(())
}

fn denied() -> JournalError {
    JournalError::PermissionDenied("Journal writer binding is absent, released or replaced".into())
}

pub(super) fn check_writer(tx: &Connection, host: &JournalHostContext) -> JournalResult<()> {
    crate::require_subject_in_connection(tx, &host.subject_id)?;
    let attr = host.attribution.as_ref().ok_or_else(denied)?;
    let binding = host.writer_binding_id.as_ref().ok_or_else(denied)?;
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings b JOIN journal_session_bindings s ON s.binding_id=b.binding_id WHERE b.binding_id=?1 AND b.subject_id=?2 AND b.runtime_id=?3 AND b.worker_id=?4 AND b.active=1 AND s.session_id=?5)", params![binding,host.subject_id,attr.runtime_id,attr.worker_id,attr.session_id], |r| r.get::<_,bool>(0))? {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn require_saved_position(tx: &Connection, generation: &str) -> JournalResult<()> {
    let saved: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_operations p WHERE p.generation_id=?1 AND json_extract(p.request_json,'$.action.action')='update_position' AND p.rowid > COALESCE((SELECT MAX(r.rowid) FROM journal_operations r WHERE r.generation_id=?1 AND json_extract(r.request_json,'$.action.action') IN ('create','rewrite','fix','confirm_unchanged')),0))",[generation],|r|r.get(0))?;
    if !saved {
        return Err(JournalError::Conflict("Journal current position must be saved after the generation's activity records before completion".into()));
    }
    Ok(())
}

impl SubjektivStore {
    /// The last released body's most recently updated Journal Session, independent of source
    /// retention. Session attachment order may differ from the order of recorded activity.
    pub fn journal_predecessor_session(
        &self,
        binding: &JournalWriterBinding,
    ) -> JournalResult<Option<String>> {
        self.database.try_with_connection(|tx| {
            crate::require_subject_in_connection(tx, &binding.subject_id)?;
            Ok(tx.query_row("SELECT j.source_session_id FROM journals j JOIN journal_session_bindings s ON s.session_id=j.source_session_id WHERE j.subject_id=?1 AND s.binding_id=(SELECT binding_id FROM journal_writer_bindings WHERE subject_id=?1 AND active=0 ORDER BY rowid DESC LIMIT 1) ORDER BY j.updated_at DESC,j.id DESC LIMIT 1", [&binding.subject_id], |r| r.get(0)).optional()?)
        })
    }

    /// Absence proof for failed body creation, including bindings never attached to a Session
    /// and released lifetimes. The Host must hold its lifecycle fence across this observation
    /// and recovery; absence alone does not fence a delayed Runtime create.
    pub fn journal_body_is_unpublished(
        &self,
        subject: &str,
        runtime: &str,
        worker: &str,
    ) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            crate::require_subject_in_connection(tx, subject)?;
            Ok(tx.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM journal_writer_bindings
                   WHERE subject_id=?1 AND runtime_id=?2 AND worker_id=?3)
                 AND NOT EXISTS(SELECT 1 FROM subject_session_attributions
                   WHERE subject_id=?1 AND runtime_id=?2 AND worker_id=?3)",
                params![subject, runtime, worker],
                |row| row.get(0),
            )?)
        })
    }

    pub fn journal_worker_was_released(
        &self,
        subject: &str,
        runtime: &str,
        worker: &str,
    ) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            crate::require_subject_in_connection(tx, subject)?;
            Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE subject_id=?1 AND runtime_id=?2 AND worker_id=?3 AND active=0)", params![subject,runtime,worker], |r| r.get(0))?)
        })
    }

    pub fn journal_writer_active(&self, binding: &JournalWriterBinding) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            crate::require_subject_in_connection(tx, &binding.subject_id)?;
            Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE binding_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4 AND active=1)", params![binding.binding_id,binding.subject_id,binding.runtime_id,binding.worker_id], |r| r.get(0))?)
        })
    }

    pub fn journal_session_writer_matches(
        &self,
        binding: &JournalWriterBinding,
        session: &str,
    ) -> JournalResult<bool> {
        self.database.try_with_connection(|tx| {
            crate::require_subject_in_connection(tx, &binding.subject_id)?;
            Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_session_bindings WHERE binding_id=?1 AND session_id=?2)", params![binding.binding_id,session], |r| r.get(0))?)
        })
    }

    /// Called under the existing Host Subject lifecycle authority. A stopped or unavailable
    /// owner remains the owner; only the exact same body may resume an unreleased binding.
    pub fn claim_journal_writer(
        &self,
        subject: &str,
        runtime: &str,
        worker: &str,
    ) -> JournalResult<JournalWriterBinding> {
        for identifier in [subject, runtime, worker] {
            if identifier.is_empty()
                || identifier.len() > 128
                || identifier.trim() != identifier
                || identifier.chars().any(char::is_control)
            {
                return Err(JournalError::InvalidInput(
                    "invalid Journal binding identity".into(),
                ));
            }
        }
        self.database.try_transaction(|tx| {
            crate::require_subject_in_connection(tx, subject)?;
            crate::deletion::require_live_worker(tx, runtime, worker)?;
            let state: Option<String> = tx.query_row("SELECT state FROM subjects WHERE subject_id=?1", [subject], |r| r.get(0)).optional()?;
            if state.as_deref() != Some("active") { return Err(JournalError::PermissionDenied("Subject is absent or retired".into())); }
            let current: Option<JournalWriterBinding> = tx.query_row("SELECT subject_id,runtime_id,worker_id,binding_id FROM journal_writer_bindings WHERE subject_id=?1 AND active=1", [subject], |r| Ok(JournalWriterBinding { subject_id:r.get(0)?,runtime_id:r.get(1)?,worker_id:r.get(2)?,binding_id:r.get(3)? })).optional()?;
            if let Some(current) = current {
                if current.runtime_id != runtime || current.worker_id != worker {
                    return Err(JournalError::Conflict("Subject is still occupied; explicit Idle and Journal-complete release required".into()));
                }
                return Ok(current);
            }
            // Upgraded T-734 data can contain a removed body's durable recording handoff.
            // Retention is not permission for a different body to occupy that Subject.
            let legacy_busy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM journals j JOIN subject_session_attributions a ON a.session_id=j.source_session_id WHERE j.subject_id=?1 AND (a.runtime_id<>?2 OR a.worker_id<>?3) AND NOT EXISTS(SELECT 1 FROM journal_session_bindings b WHERE b.session_id=j.source_session_id) AND (EXISTS(SELECT 1 FROM journal_generations g WHERE g.journal_id=j.id AND g.active=1) OR EXISTS(SELECT 1 FROM journal_sources s,json_each(s.accepted_json) r WHERE s.journal_id=j.id AND json_extract(r.value,'$.end')>s.reflected_bytes)))", params![subject,runtime,worker], |r| r.get(0))?;
            if legacy_busy { return Err(JournalError::Conflict("previous body's retained Journal handoff is not complete".into())); }
            let binding = JournalWriterBinding { subject_id:subject.into(),runtime_id:runtime.into(),worker_id:worker.into(),binding_id:crate::issued_id("journal-binding") };
            tx.execute("INSERT INTO journal_writer_bindings(binding_id,subject_id,runtime_id,worker_id,active) VALUES(?1,?2,?3,?4,1)", params![binding.binding_id,subject,runtime,worker])?;
            Ok(binding)
        })
    }

    /// Binds a new Session once. Reconnecting a Worker under a later binding cannot acquire
    /// mutation authority for its old Session. Attribution and binding are Host-only input.
    pub fn bind_journal_session(
        &self,
        binding: &JournalWriterBinding,
        attribution: SubjectSessionAttribution,
    ) -> JournalResult<JournalHostContext> {
        let mut host = JournalHostContext::validated_session(self, attribution)?;
        self.database.try_transaction(|tx| {
            let attr = host.attribution.as_ref().expect("validated Session");
            if binding.subject_id != attr.subject_id || binding.runtime_id != attr.runtime_id || binding.worker_id != attr.worker_id {
                return Err(denied());
            }
            tx.execute("INSERT OR IGNORE INTO journal_session_bindings(session_id,binding_id) SELECT ?1,binding_id FROM journal_writer_bindings WHERE binding_id=?2 AND subject_id=?3 AND runtime_id=?4 AND worker_id=?5 AND active=1",params![attr.session_id,binding.binding_id,attr.subject_id,attr.runtime_id,attr.worker_id])?;
            host.writer_binding_id = Some(binding.binding_id.clone());
            check_writer(tx,&host)?;
            Ok(host.clone())
        })
    }

    /// Destructive release is distinct from a continuity handoff: every attributed Session
    /// of this body must already have a permanent erasure tombstone. This also records a
    /// released lifetime for pre-binding bodies, without granting them new mutation authority.
    pub fn release_erased_journal_body(&self, host: &JournalHostContext) -> JournalResult<bool> {
        self.database.try_transaction(|tx| {
            super::store::check_session_host(self, tx, host)?;
            let attr = host.attribution.as_ref().expect("checked Session authority");
            let remaining: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_session_attributions a WHERE a.subject_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3 AND NOT EXISTS(SELECT 1 FROM journal_erased_sessions e WHERE e.session_id=a.session_id AND e.subject_id=a.subject_id))", params![host.subject_id,attr.runtime_id,attr.worker_id], |r| r.get(0))?;
            if remaining {
                return Ok(false);
            }
            let existing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE subject_id=?1 AND runtime_id=?2 AND worker_id=?3)", params![host.subject_id,attr.runtime_id,attr.worker_id], |r| r.get(0))?;
            tx.execute("UPDATE journal_writer_bindings SET active=0 WHERE subject_id=?1 AND runtime_id=?2 AND worker_id=?3",params![host.subject_id,attr.runtime_id,attr.worker_id])?;
            if !existing {
                tx.execute("INSERT INTO journal_writer_bindings(binding_id,subject_id,runtime_id,worker_id,active) VALUES(?1,?2,?3,?4,0)", params![crate::issued_id("journal-erased-body"),host.subject_id,attr.runtime_id,attr.worker_id])?;
            }
            Ok(true)
        })
    }

    /// The Host must hold its lifecycle fence, confirm Idle, reconcile all committed original
    /// Sessions (including the save/notification gap), and exclude queued/running/Unknown Jobs.
    /// This transaction rejects retained writers, pending source bytes and active generations.
    /// It never fixes an open entry or declares the represented work complete.
    pub fn release_journal_writer(&self, binding: &JournalWriterBinding) -> JournalResult<()> {
        self.database.try_transaction(|tx| {
            crate::require_subject_in_connection(tx, &binding.subject_id)?;
            let active: Option<bool> = tx.query_row("SELECT active FROM journal_writer_bindings WHERE binding_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4",params![binding.binding_id,binding.subject_id,binding.runtime_id,binding.worker_id],|r|r.get(0)).optional()?;
            let Some(active) = active else { return Err(denied()); };
            if !active { return Ok(()); }
            let unbound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_session_attributions a WHERE a.subject_id=?1 AND a.runtime_id=?2 AND a.worker_id=?3 AND NOT EXISTS(SELECT 1 FROM journal_session_bindings s WHERE s.session_id=a.session_id) AND NOT EXISTS(SELECT 1 FROM journal_erased_sessions e WHERE e.session_id=a.session_id AND e.subject_id=a.subject_id))",params![binding.subject_id,binding.runtime_id,binding.worker_id],|r|r.get(0))?;
            if unbound { return Err(JournalError::Conflict("body has legacy Sessions whose committed history has not been reconciled under its writer binding".into())); }
            let busy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM journal_generations g JOIN journals j ON j.id=g.journal_id JOIN journal_session_bindings s ON s.session_id=j.source_session_id WHERE s.binding_id=?1 AND g.active=1)",[&binding.binding_id],|r|r.get(0))?;
            if busy { return Err(JournalError::Conflict("Journal generation remains active or its result is unconfirmed".into())); }
            let mut stmt = tx.prepare("SELECT s.accepted_json,s.reflected_bytes,s.unavailable_json FROM journal_sources s JOIN journals j ON j.id=s.journal_id JOIN journal_session_bindings b ON b.session_id=j.source_session_id WHERE b.binding_id=?1")?;
            let mut rows = stmt.query([&binding.binding_id])?;
            while let Some(row) = rows.next()? {
                let accepted: Vec<JournalByteRange> = serde_json::from_str(&row.get::<_,String>(0)?)?;
                let reflected: u64 = row.get(1)?;
                if accepted.iter().any(|r| r.end > reflected) {
                    return Err(JournalError::Conflict("Journal activity/position/checkpoint has unreflected source bytes".into()));
                }
            }
            let mut checkpoints = tx.prepare("SELECT p.generation_id FROM journal_operations p JOIN journals j ON j.id=p.journal_id JOIN journal_session_bindings b ON b.session_id=j.source_session_id WHERE b.binding_id=?1 AND p.rowid=(SELECT MAX(c.rowid) FROM journal_operations c WHERE c.journal_id=j.id AND json_extract(c.request_json,'$.action.action')='checkpoint')")?;
            let generations = checkpoints.query_map([&binding.binding_id], |r| r.get::<_,String>(0))?;
            for generation in generations { require_saved_position(tx, &generation?)?; }
            tx.execute("UPDATE journal_writer_bindings SET active=0 WHERE binding_id=?1 AND active=1",[&binding.binding_id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpublished_body_rejects_orphan_and_released_bindings_and_attribution_only() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = crate::tests::open_store(temp.path(), "absence-tests");
        let subject = store
            .create_subject(crate::SubjectRole::new("companion").unwrap())
            .unwrap();
        let other_subject = store
            .create_subject(crate::SubjectRole::new("companion").unwrap())
            .unwrap();
        assert!(
            store
                .journal_body_is_unpublished(&subject.id, "runtime", "worker")
                .unwrap()
        );
        // No Session was attached: an orphan binding is still published body evidence.
        let binding = store
            .claim_journal_writer(&subject.id, "runtime", "worker")
            .unwrap();
        assert!(
            !store
                .journal_body_is_unpublished(&subject.id, "runtime", "worker")
                .unwrap()
        );
        for (subject_id, runtime, worker) in [
            (other_subject.id.as_str(), "runtime", "worker"),
            (subject.id.as_str(), "other-runtime", "worker"),
            (subject.id.as_str(), "runtime", "other-worker"),
        ] {
            assert!(
                store
                    .journal_body_is_unpublished(subject_id, runtime, worker)
                    .unwrap()
            );
        }
        store.release_journal_writer(&binding).unwrap();
        assert!(
            !store
                .journal_body_is_unpublished(&subject.id, "runtime", "worker")
                .unwrap()
        );
        store
            .record_session_attribution(
                SubjectSessionAttribution::new(
                    &subject.id,
                    "runtime",
                    "attribution-only",
                    "session",
                )
                .unwrap(),
            )
            .unwrap();
        assert!(
            !store
                .journal_body_is_unpublished(&subject.id, "runtime", "attribution-only")
                .unwrap()
        );
        assert!(
            store
                .journal_body_is_unpublished(&subject.id, "other-runtime", "attribution-only")
                .unwrap()
        );
        assert!(
            store
                .journal_body_is_unpublished(&other_subject.id, "runtime", "attribution-only")
                .unwrap()
        );
        assert!(
            store
                .journal_body_is_unpublished(&subject.id, "runtime", "never-published")
                .unwrap()
        );
    }
}
