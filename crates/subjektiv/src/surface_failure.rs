//! Generation-bound failure receipts. An old Subject projection is not evidence that a
//! new generation failed. Legacy failed projections remain displayable but are never
//! backfilled with a guessed generation owner.
use super::*;

pub(super) const MIGRATION_NAME: &str = "retain generation-bound surface failure diagnostics";

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(
        "CREATE TABLE surface_generation_failures (
            subject_id TEXT NOT NULL,
            generation_id TEXT NOT NULL,
            reason_code TEXT NOT NULL CHECK(length(reason_code)>0),
            detail TEXT CHECK(detail IS NULL OR length(CAST(detail AS BLOB))<=8192),
            created_at TEXT NOT NULL,
            PRIMARY KEY(subject_id,generation_id),
            FOREIGN KEY(subject_id,generation_id)
                REFERENCES surface_generation_runs(subject_id,generation_id) ON DELETE RESTRICT
        );
        CREATE TRIGGER surface_generation_failures_no_update BEFORE UPDATE ON surface_generation_failures
            BEGIN SELECT RAISE(ABORT, 'surface failure receipts are immutable'); END;
        CREATE TRIGGER surface_generation_failures_no_delete BEFORE DELETE ON surface_generation_failures
            WHEN NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
            BEGIN SELECT RAISE(ABORT, 'surface failure receipts are retained'); END;
        CREATE TRIGGER surface_generation_failures_deletion_fence BEFORE INSERT ON surface_generation_failures
            WHEN EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=NEW.subject_id)
            BEGIN SELECT RAISE(ABORT, 'Subject deletion has begun'); END;",
    )?;
    Ok(())
}

pub(super) fn record(
    tx: &Transaction<'_>,
    subject_id: &str,
    generation_id: &str,
    reason_code: &str,
    detail: Option<&str>,
) -> Result<()> {
    if detail.is_some_and(|value| {
        value.len() > server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES || value.contains('\0')
    }) {
        return Err(SubjektivError::InvalidRecord(
            "invalid surface failure detail".into(),
        ));
    }
    let saved: Option<(String, Option<String>)> = tx.query_row(
        "SELECT reason_code,detail FROM surface_generation_failures WHERE subject_id=?1 AND generation_id=?2",
        params![subject_id,generation_id], |row| Ok((row.get(0)?,row.get(1)?))
    ).optional()?;
    if let Some((reason, saved_detail)) = saved {
        if reason != reason_code || saved_detail.as_deref() != detail {
            return Err(SubjektivError::SurfaceGenerationConflict(
                "generation already has a different failure receipt".into(),
            ));
        }
    } else {
        tx.execute(
            "INSERT INTO surface_generation_failures(subject_id,generation_id,reason_code,detail,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![subject_id,generation_id,reason_code,detail,now()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
