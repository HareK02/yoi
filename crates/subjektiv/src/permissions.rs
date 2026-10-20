//! Host-administered permission sources and immutable child grants, separate from behavior.
//!
//! The Host authenticates administrators before source writes and binds verified Worker/Profile
//! identities before grant writes. These records are not a Worker registry. The Host alone
//! authenticates root execution ownership against its canonical Backend singleton and checks
//! every live lineage edge; Journal bindings and retained grants cannot establish ownership.
//! Replays return historical evidence without restoring a source or granting current authority.
use super::*;
use server_api::authorization::{
    PermissionResource, ScopedPermission, SubjectPermissions, SubjectPermissionsUpdate,
    WorkerExecutionGrant, validate_authorization_identifier, validate_scoped_permissions,
};
use sha2::{Digest, Sha256};

pub const MAX_SUBJECT_WORKER_EXECUTION_GRANTS: usize = 1_000;

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("permissions.sql"))?;
    Ok(())
}

impl SubjektivStore {
    /// Unset sources are `None` (deny), including migrated Subjects. This is an admin read,
    /// not an effective-authorization decision; retired Subjects retain their source evidence.
    pub fn subject_permissions(&self, subject_id: &str) -> Result<Option<SubjectPermissions>> {
        self.database.try_with_connection(|connection| {
            require_subject_in_connection(connection, subject_id)?;
            source_in_connection(connection, subject_id)
        })
    }

    /// Host-admin-only replacement. Every replacement issues a fresh change ID, including
    /// identical contents. Revocation is an explicit empty replacement. A scope-wide operation
    /// ID binds Subject, CAS input, and both permission vectors; result and receipt commit together.
    pub fn set_subject_permissions(
        &self,
        subject_id: &str,
        update: &SubjectPermissionsUpdate,
    ) -> Result<SubjectPermissions> {
        validate_authorization_identifier(subject_id).map_err(SubjektivError::InvalidRecord)?;
        validate_authorization_identifier(&update.operation_id)
            .map_err(SubjektivError::InvalidRecord)?;
        if let Some(change) = &update.expected_change_id {
            validate_authorization_identifier(change).map_err(SubjektivError::InvalidRecord)?;
        }
        validate_scope(self.scope_id(), &update.execution)?;
        validate_scope(self.scope_id(), &update.delegation)?;
        let fingerprint: String = Sha256::digest(serde_json::to_vec(&(subject_id, update))?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.database.try_transaction(|tx| {
            require_subject(tx, subject_id)?;
            let receipt: Option<(String, String)> = tx.query_row(
                "SELECT request_fingerprint, change_id FROM subject_permission_receipts WHERE operation_id=?1",
                [&update.operation_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((saved, change_id)) = receipt {
                if saved != fingerprint {
                    return Err(SubjektivError::PermissionOperationConflict(update.operation_id.clone()));
                }
                let raw: String = tx.query_row(
                    "SELECT record_json FROM subject_permission_changes WHERE subject_id=?1 AND change_id=?2",
                    params![subject_id, change_id], |row| row.get(0),
                )?;
                return Ok(serde_json::from_str(&raw)?);
            }
            require_active_subject(tx, subject_id)?;
            let current = source_in_connection(tx, subject_id)?;
            if current.as_ref().map(|source| &source.change_id) != update.expected_change_id.as_ref() {
                return Err(SubjektivError::SubjectPermissionsConflict(subject_id.into()));
            }
            let source = SubjectPermissions {
                change_id: issued_id("permissions"),
                execution: update.execution.clone(),
                delegation: update.delegation.clone(),
            };
            tx.execute("INSERT INTO subject_permission_changes(subject_id,change_id,record_json) VALUES(?1,?2,?3)",
                params![subject_id, source.change_id, serde_json::to_string(&source)?])?;
            tx.execute("INSERT INTO subject_permissions(subject_id,change_id) VALUES(?1,?2) ON CONFLICT(subject_id) DO UPDATE SET change_id=excluded.change_id",
                params![subject_id, source.change_id])?;
            tx.execute("INSERT INTO subject_permission_receipts(operation_id,subject_id,request_fingerprint,change_id) VALUES(?1,?2,?3,?4)",
                params![update.operation_id, subject_id, fingerprint, source.change_id])?;
            Ok(source)
        })
    }

    /// Binds child grants only. Exact retries recover an immutable binding even after revocation;
    /// that success is not authorization to execute or redelegate. New bindings require the
    /// current source, an active Subject and explicit delegation containment. The Host must
    /// authenticate root singleton ownership and every live lineage edge before calling;
    /// a missing retained parent is not ownership evidence. Profile resolution, provider scope,
    /// Worker creation and caller authentication also remain Host duties.
    pub fn bind_worker_execution_grant(&self, grant: &WorkerExecutionGrant) -> Result<()> {
        validate_grant(self.scope_id(), grant)?;
        self.database.try_transaction(|tx| {
            if let Some(existing) = grant_in_connection(tx, &grant.runtime_id, &grant.worker_id)? {
                return if existing == *grant { Ok(()) } else {
                    Err(SubjektivError::WorkerExecutionGrantConflict(format!("{}/{}", grant.runtime_id, grant.worker_id)))
                };
            }
            require_active_subject(tx, &grant.subject_id)?;
            let source = source_in_connection(tx, &grant.subject_id)?
                .ok_or_else(|| denied("Subject permissions are unset"))?;
            if source.change_id != grant.source_change_id {
                return Err(denied("Subject permission source is no longer current"));
            }
            require_covered(&source.delegation, &grant.permissions)?;
            require_covered(&source.delegation, &grant.redelegation)?;
            // A child grant cannot turn a body (including a released historical body) into a
            // delegated child or attach a Worker historically attributed to a different Subject.
            let invalid_target: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE runtime_id=?1 AND worker_id=?2) OR EXISTS(SELECT 1 FROM subject_session_attributions WHERE runtime_id=?1 AND worker_id=?2 AND subject_id<>?3)",
                params![grant.runtime_id, grant.worker_id, grant.subject_id], |row| row.get(0),
            )?;
            if invalid_target { return Err(denied("target is a body or belongs to another Subject")); }
            let origin = require_retained_origin(tx, grant)?;
            if let Some(origin) = origin {
                require_covered(&origin.redelegation, &grant.permissions)?;
                require_covered(&origin.redelegation, &grant.redelegation)?;
            }
            tx.execute("INSERT INTO worker_execution_grants(runtime_id,worker_id,subject_id,source_change_id,record_json) VALUES(?1,?2,?3,?4,?5)",
                params![grant.runtime_id, grant.worker_id, grant.subject_id, grant.source_change_id, serde_json::to_string(grant)?])?;
            Ok(())
        })
    }

    /// Returns this Subject's retained grants, including stale grants needed for cancellation
    /// and reporting. Never an ownership list or effective-authorization decision. Overflow is
    /// explicit rather than silently omitting Workers from a cancellation/reporting pass.
    pub fn list_subject_worker_execution_grants(
        &self,
        subject_id: &str,
    ) -> Result<Vec<WorkerExecutionGrant>> {
        validate_authorization_identifier(subject_id).map_err(SubjektivError::InvalidRecord)?;
        self.database.try_with_connection(|connection| {
            require_subject_in_connection(connection, subject_id)?;
            let mut statement = connection.prepare(
                "SELECT record_json FROM worker_execution_grants WHERE subject_id=?1 ORDER BY runtime_id,worker_id LIMIT ?2",
            )?;
            let mut rows = statement.query(params![subject_id, MAX_SUBJECT_WORKER_EXECUTION_GRANTS + 1])?;
            let mut grants = Vec::new();
            while let Some(row) = rows.next()? {
                if grants.len() == MAX_SUBJECT_WORKER_EXECUTION_GRANTS {
                    return Err(SubjektivError::WorkerExecutionGrantListOverflow {
                        subject_id: subject_id.into(),
                        limit: MAX_SUBJECT_WORKER_EXECUTION_GRANTS,
                    });
                }
                grants.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
            }
            Ok(grants)
        })
    }

    /// Returns retained binding evidence, not current authorization. The Host must compare the
    /// active Subject, source change, current origin ownership, Profile and provider constraints.
    pub fn worker_execution_grant(
        &self,
        runtime: &str,
        worker: &str,
    ) -> Result<Option<WorkerExecutionGrant>> {
        validate_authorization_identifier(runtime).map_err(SubjektivError::InvalidRecord)?;
        validate_authorization_identifier(worker).map_err(SubjektivError::InvalidRecord)?;
        self.database
            .try_with_connection(|connection| grant_in_connection(connection, runtime, worker))
    }
}

fn source_in_connection(
    connection: &Connection,
    subject: &str,
) -> Result<Option<SubjectPermissions>> {
    let raw: Option<String> = connection.query_row(
        "SELECT c.record_json FROM subject_permissions p JOIN subject_permission_changes c ON c.subject_id=p.subject_id AND c.change_id=p.change_id WHERE p.subject_id=?1",
        [subject], |row| row.get(0),
    ).optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

fn grant_in_connection(
    connection: &Connection,
    runtime: &str,
    worker: &str,
) -> Result<Option<WorkerExecutionGrant>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM worker_execution_grants WHERE runtime_id=?1 AND worker_id=?2",
            params![runtime, worker],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

fn validate_scope(scope: &str, permissions: &[ScopedPermission]) -> Result<()> {
    validate_scoped_permissions(permissions).map_err(SubjektivError::InvalidRecord)?;
    for permission in permissions {
        if let PermissionResource::Workspace { workspace_id } = &permission.resource
            && workspace_id != scope
        {
            return Err(denied("Workspace permission is outside this store scope"));
        }
    }
    Ok(())
}

fn validate_grant(scope: &str, grant: &WorkerExecutionGrant) -> Result<()> {
    for value in [
        &grant.subject_id,
        &grant.origin_runtime_id,
        &grant.origin_worker_id,
        &grant.source_change_id,
        &grant.runtime_id,
        &grant.worker_id,
        &grant.profile,
        &grant.profile_digest,
    ] {
        validate_authorization_identifier(value).map_err(SubjektivError::InvalidRecord)?;
    }
    if grant.origin_runtime_id == grant.runtime_id && grant.origin_worker_id == grant.worker_id {
        return Err(denied("a child grant cannot originate from itself"));
    }
    validate_scope(scope, &grant.permissions)?;
    validate_scope(scope, &grant.redelegation)
}

fn require_covered(ceiling: &[ScopedPermission], requested: &[ScopedPermission]) -> Result<()> {
    if requested
        .iter()
        .all(|permission| ceiling.iter().any(|allowed| allowed.covers(permission)))
    {
        Ok(())
    } else {
        Err(denied(
            "requested permission or redelegation exceeds its explicit ceiling",
        ))
    }
}

fn require_retained_origin(
    tx: &Transaction<'_>,
    grant: &WorkerExecutionGrant,
) -> Result<Option<WorkerExecutionGrant>> {
    let mut runtime = grant.origin_runtime_id.clone();
    let mut worker = grant.origin_worker_id.clone();
    let mut direct_origin = None;
    let mut visited = HashSet::from([(grant.runtime_id.clone(), grant.worker_id.clone())]);
    // Validate retained lineage only. Absence terminates at a Host-authenticated root;
    // neither Journal active state nor absence of a grant proves current singleton ownership.
    for _ in 0..64 {
        if !visited.insert((runtime.clone(), worker.clone())) {
            return Err(denied("cyclic grant origin"));
        }
        let foreign_attribution: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE runtime_id=?1 AND worker_id=?2 AND subject_id<>?3) OR EXISTS(SELECT 1 FROM subject_session_attributions WHERE runtime_id=?1 AND worker_id=?2 AND subject_id<>?3)",
            params![runtime, worker, grant.subject_id], |row| row.get(0),
        )?;
        if foreign_attribution {
            return Err(denied("grant origin is attributed to another Subject"));
        }
        let Some(origin) = grant_in_connection(tx, &runtime, &worker)? else {
            return Ok(direct_origin);
        };
        if origin.subject_id != grant.subject_id
            || origin.source_change_id != grant.source_change_id
        {
            return Err(denied(
                "grant origin belongs to another Subject or permission source",
            ));
        }
        if direct_origin.is_none() {
            direct_origin = Some(origin.clone());
        }
        runtime = origin.origin_runtime_id;
        worker = origin.origin_worker_id;
    }
    Err(denied("grant origin exceeds maximum delegation depth"))
}

fn denied(reason: &str) -> SubjektivError {
    SubjektivError::AuthorizationDenied(reason.into())
}

#[cfg(test)]
mod tests;
