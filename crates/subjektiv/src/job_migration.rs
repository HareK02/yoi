//! Host-owned migration of references to discarded Job execution records.
//! This is not a runtime repair, completion decision, or execution-stop proof.
use crate::{Result, SubjektivStore};
use rusqlite::params;

impl SubjektivStore {
    /// Retire references selected by the owning catalog migration, before publishing
    /// the store to runtime consumers. The caller must quiesce external writers,
    /// pin this physical database identity, and durably retain the exact Job set
    /// until this transaction commits. Repeating the same set is idempotent.
    /// Only execution ownership is retired: content, snapshots, operation receipts,
    /// source coverage, completion evidence and writer bindings remain unchanged.
    pub fn retire_migrated_job_references(&self, job_ids: &[String]) -> Result<()> {
        self.retire_job_references(None, job_ids)
    }

    /// Per-Subject counterpart for Hosts whose Job identifiers are scoped to one
    /// Subject rather than the entire physical Feature database.
    pub fn retire_migrated_subject_job_references(
        &self,
        subject_id: &str,
        job_ids: &[String],
    ) -> Result<()> {
        self.retire_job_references(Some(subject_id), job_ids)
    }

    fn retire_job_references(&self, subject_id: Option<&str>, job_ids: &[String]) -> Result<()> {
        self.database.try_transaction(|tx| {
            for job in job_ids {
                tx.execute(
                    "UPDATE journal_generations SET active=0 WHERE active=1 AND job_id=?1
                     AND (?2 IS NULL OR journal_id IN (SELECT id FROM journals WHERE subject_id=?2))",
                    params![job, subject_id],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SubjectRole;
    use feature_storage::FeatureStorage;

    fn fixture() -> (
        tempfile::TempDir,
        feature_storage::ScopedFeatureStorage,
        SubjektivStore,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let scope = FeatureStorage::new(dir.path())
            .scope("migration-scope")
            .unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        let subject = store
            .create_subject(SubjectRole::new("owner").unwrap())
            .unwrap();
        let other = store
            .create_subject(SubjectRole::new("other").unwrap())
            .unwrap();
        store.database.with_connection(|conn| {
            for (journal, subject, job) in [("old", &subject.id, "obsolete"), ("current", &subject.id, "retained"), ("other", &other.id, "obsolete")] {
                conn.execute("INSERT INTO journals(id,subject_id,source_session_id,title,current_position,state_token,created_at,updated_at) VALUES(?1,?2,?1,'saved title','saved position','saved token','1','2')", params![journal,subject])?;
                conn.execute("INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES(?1,?2,?1,0,'open','entry','saved body','ongoing','1','2')",params![journal,subject])?;
                conn.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) VALUES(?1,?1,?2,'attempt','{\"saved\":true}','{\"snapshot\":true}',1)",params![journal,job])?;
                conn.execute("INSERT INTO journal_operations(journal_id,request_id,generation_id,request_json,receipt_json) VALUES(?1,'operation',?1,'{}','{\"saved\":true}')",[journal])?;
            }
            Ok(())
        }).unwrap();
        (dir, scope, store, subject.id)
    }

    fn snapshots(
        store: &SubjektivStore,
    ) -> Vec<(
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
    )> {
        store.database.with_connection(|conn| {
            conn.prepare("SELECT g.id,g.request_json,g.snapshot_json,g.completion_json,e.body,o.request_json,o.receipt_json FROM journal_generations g JOIN journal_entries e ON e.journal_id=g.journal_id JOIN journal_operations o ON o.generation_id=g.id ORDER BY g.id")?
                .query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        }).unwrap()
    }

    fn active(store: &SubjektivStore) -> Vec<(String, bool)> {
        store
            .database
            .with_connection(|conn| {
                conn.prepare("SELECT id,active FROM journal_generations ORDER BY id")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(Into::into)
            })
            .unwrap()
    }

    #[test]
    fn subject_scoped_retirement_preserves_other_jobs_content_and_receipts_on_replay() {
        let (_dir, _scope, store, subject) = fixture();
        let before = snapshots(&store);
        for _ in 0..2 {
            store
                .retire_migrated_subject_job_references(&subject, &["obsolete".into()])
                .unwrap();
            assert_eq!(
                active(&store),
                vec![
                    ("current".into(), true),
                    ("old".into(), false),
                    ("other".into(), true)
                ]
            );
            assert_eq!(snapshots(&store), before);
        }
        store
            .retire_migrated_job_references(&["obsolete".into()])
            .unwrap();
        assert_eq!(
            active(&store),
            vec![
                ("current".into(), true),
                ("old".into(), false),
                ("other".into(), false)
            ]
        );
        assert_eq!(snapshots(&store), before);
    }

    #[test]
    fn failed_retirement_rolls_back_all_selected_generations() {
        let (_dir, _scope, store, _) = fixture();
        let before = active(&store);
        store.database.with_connection(|conn| conn.execute_batch("CREATE TRIGGER refuse_current BEFORE UPDATE ON journal_generations WHEN OLD.id='current' BEGIN SELECT RAISE(ABORT,'injected'); END;").map_err(Into::into)).unwrap();
        assert!(
            store
                .retire_migrated_job_references(&["obsolete".into(), "retained".into()])
                .is_err()
        );
        assert_eq!(active(&store), before);
    }
}
