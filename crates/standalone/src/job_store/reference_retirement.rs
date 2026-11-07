//! Durable cross-DB migration work, never an alternate runtime Job format.
use super::*;
use feature_storage::DatabaseIdentity;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS job_reference_retirements (
    job_id TEXT PRIMARY KEY NOT NULL,
    domain_references INTEGER NOT NULL CHECK(domain_references IN (0,1))
);
CREATE TABLE IF NOT EXISTS job_reference_retirement_target (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    scope_id TEXT NOT NULL,
    feature_id TEXT NOT NULL,
    physical_id TEXT NOT NULL,
    subject_id TEXT NOT NULL
);
"#;

impl JobStore {
    /// Generic Hosts may discard work only when migration found no possible domain
    /// references. A Subject Host drains even these IDs, under its Subject lease.
    pub(crate) fn finish_domainless_reference_retirement(&mut self) -> Result<(), JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let blocked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM job_reference_retirements WHERE domain_references=1)
             OR (EXISTS(SELECT 1 FROM job_reference_retirements)
                 AND EXISTS(SELECT 1 FROM job_reference_retirement_target))",
            [],
            |r| r.get(0),
        )?;
        if blocked {
            return Err(JobStoreError::InvalidTransition(
                "pending Job reference retirement requires the owning Subject Host",
            ));
        }
        tx.execute("DELETE FROM job_reference_retirements", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Pin before the first Feature effect; retain the target after completion so
    /// later opens cannot reinterpret this Subject's catalog as a different DB.
    pub(crate) fn pin_reference_retirement(
        &mut self,
        subject: &str,
        identity: &DatabaseIdentity,
    ) -> Result<Vec<String>, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO job_reference_retirement_target VALUES(1,?1,?2,?3,?4)",
            params![identity.scope, identity.feature, identity.physical, subject],
        )?;
        let matches: bool = tx.query_row(
            "SELECT scope_id=?1 AND feature_id=?2 AND physical_id=?3 AND subject_id=?4
             FROM job_reference_retirement_target WHERE singleton=1",
            params![identity.scope, identity.feature, identity.physical, subject],
            |r| r.get(0),
        )?;
        if !matches {
            return Err(JobStoreError::InvalidTransition(
                "Job reference retirement target changed",
            ));
        }
        let ids = tx
            .prepare("SELECT job_id FROM job_reference_retirements ORDER BY job_id")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        tx.commit()?;
        Ok(ids)
    }

    /// Called only after the pinned Feature transaction committed. Failure retains
    /// the exact set; replaying the Feature operation is deliberately idempotent.
    pub(crate) fn complete_reference_retirement(
        &mut self,
        ids: &[String],
    ) -> Result<(), JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            tx.execute(
                "DELETE FROM job_reference_retirements WHERE job_id=?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
