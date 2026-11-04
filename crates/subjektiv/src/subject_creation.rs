//! Before sealing, new Subjects receive their self-scoped source in the creation transaction.
//! After sealing, creation retains enrollment evidence only; current reads never revive defaults.
//! This path is never called by migration, launch, restore or permission reads. Receipts retain
//! identity, input fingerprint and any post-seal enrollment across erasure, preventing replay
//! from recreating an erased Subject or restoring a revoked policy.
use super::*;
use server_api::authorization::{SubjectPermissions, subject_conversation_permissions};
use sha2::{Digest, Sha256};

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(
        "CREATE TABLE subject_creation_receipts (
            operation_id TEXT PRIMARY KEY CHECK(length(operation_id)>0),
            subject_id TEXT NOT NULL UNIQUE,
            request_fingerprint TEXT NOT NULL
        );",
    )?;
    Ok(())
}

impl SubjektivStore {
    /// Creation-only enrollment evidence for the canonical domain. Never a current permission
    /// source or a read-time default; the Host must not apply it to an existing/unset policy.
    pub fn subject_permission_enrollment(
        &self,
        subject_id: &str,
    ) -> Result<Option<SubjectPermissions>> {
        self.database.try_transaction(|tx| {
            require_subject(tx, subject_id)?;
            if !permissions::transfer::has_table(tx, "permission_authority_seal")? {
                return Ok(None);
            }
            let raw: Option<String> = tx.query_row(
                "SELECT permission_enrollment_json FROM subject_creation_receipts WHERE subject_id=?1",
                [subject_id], |row| row.get::<_, Option<String>>(0),
            ).optional()?.flatten();
            raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into)).transpose()
        })
    }

    pub(super) fn create_subject_request(
        &self,
        role: SubjectRole,
        behavior_md: String,
        operation_id: Option<&str>,
    ) -> Result<SubjectRecord> {
        validate_subject_behavior(&behavior_md)?;
        let fingerprint: String = Sha256::digest(serde_json::to_vec(&(&role, &behavior_md))?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.database.try_transaction(|tx| {
            if let Some(operation) = operation_id {
                let receipt: Option<(String, String)> = tx.query_row(
                    "SELECT subject_id,request_fingerprint FROM subject_creation_receipts WHERE operation_id=?1",
                    [operation], |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional()?;
                if let Some((subject, saved)) = receipt {
                    if saved != fingerprint {
                        return Err(SubjektivError::SubjectCreationConflict(operation.into()));
                    }
                    // Current identity/state, never a stale success that can recreate deleted data.
                    return require_active_subject(tx, &subject);
                }
            }
            let timestamp = now();
            let record = SubjectRecord {
                schema_version: SUBJEKTIV_SCHEMA_VERSION,
                id: issued_id("subject"), role, behavior_md,
                state: SubjectState::Active,
                created_at: timestamp.clone(), updated_at: timestamp,
            };
            tx.execute(
                "INSERT INTO subjects(subject_id,role,behavior_md,state,record_json,created_at,updated_at)
                 VALUES(?1,?2,?3,'active',?4,?5,?6)",
                params![record.id,record.role.as_str(),record.behavior_md,serde_json::to_string(&record)?,record.created_at,record.updated_at],
            )?;
            let source = SubjectPermissions {
                change_id: issued_id("permissions"),
                execution: subject_conversation_permissions(self.scope_id(), &record.id),
                delegation: vec![],
            };
            let sealed = permissions::transfer::is_sealed(tx)?;
            if !sealed {
                tx.execute("INSERT INTO subject_permission_changes(subject_id,change_id,record_json) VALUES(?1,?2,?3)",
                    params![record.id,source.change_id,serde_json::to_string(&source)?])?;
                tx.execute("INSERT INTO subject_permissions(subject_id,change_id) VALUES(?1,?2)",params![record.id,source.change_id])?;
            }
            if sealed {
                // Retain creation-only enrollment for the canonical domain, never a legacy
                // source write or a read-time default. Even non-idempotent creation has evidence.
                let enrollment_operation = operation_id.map(str::to_owned)
                    .unwrap_or_else(|| format!("subject-enrollment:{}", record.id));
                tx.execute("INSERT INTO subject_creation_receipts(operation_id,subject_id,request_fingerprint,permission_enrollment_json) VALUES(?1,?2,?3,?4)",
                    params![enrollment_operation,record.id,fingerprint,serde_json::to_string(&source)?])?;
            } else if let Some(operation) = operation_id {
                tx.execute("INSERT INTO subject_creation_receipts(operation_id,subject_id,request_fingerprint) VALUES(?1,?2,?3)",params![operation,record.id,fingerprint])?;
            }
            Ok(record)
        })
    }
}

#[cfg(test)]
mod tests;
