//! Subject-owned current permission policy, owner operations and creation enrollment.
//!
//! The published `workspace_configuration_policies` table and its wrapper remain
//! the sole current-policy storage. Configuration supplies reusable Sets, never
//! individual Subject assignments. Neither mutations nor enrollment edit source,
//! its projection, editing history or configuration adoption receipts. A separate
//! tombstone records only removed-state identity, never a duplicate active policy;
//! CAS therefore cannot confuse a removed policy with a never-configured Subject.
//!
//! Callers authenticate and hold the native Subject lifecycle fence for owner
//! mutations. Enrollment acquires that fence itself, then the catalog lease, then
//! Server write ownership; both outer guards survive the Server commit. Creation
//! evidence is consumed once, including when an explicit owner decision won first.

use feature_permissions::PermissionHost;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use server_api::authorization::{
    CurrentSubjectPolicy, PermissionReferenceEncoding, SubjectPolicySource,
};
use server_api::subject_policy::{
    SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation,
    SubjectPermissionPolicyMutationResponse,
};

use crate::policy_config::{self, AdoptedSubjectPolicy, AssignmentSource};
use crate::{Error, Result, SqliteWorkspaceStore};

pub(crate) const SCHEMA: &str = include_str!("subject_policy.sql");
pub(crate) const MIGRATION_NAME: &str = "Dedicated Subject permission operations and enrollment";

fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::Store(error.to_string()))
}

fn decode<T: DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).map_err(|error| Error::RegistryInconsistency(error.to_string()))
}

fn conflict(message: &str) -> Error {
    Error::WorkspaceConfigConflict(message.into())
}

fn require_owner(connection: &Connection, workspace: &str, account: &str) -> Result<()> {
    let allowed: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND owner_account_id=?2 AND state='active')",
        params![workspace, account],
        |row| row.get(0),
    )?;
    if !allowed {
        return Err(Error::WorkspacePermissionDenied(
            "Subject permission policy requires the active Workspace owner".into(),
        ));
    }
    Ok(())
}

fn require_host(workspace: &str, host: &PermissionHost) -> Result<()> {
    if host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    Ok(())
}

fn current(
    connection: &Connection,
    workspace: &str,
    subject: &str,
) -> Result<Option<CurrentSubjectPolicy>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT policy_json FROM workspace_configuration_policies WHERE workspace_id=?1 AND subject_id=?2",
            params![workspace, subject],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let saved: AdoptedSubjectPolicy = decode(&raw)?;
        if saved.source.subject_id != subject {
            return Err(Error::RegistryInconsistency(
                "Subject policy identity mismatch".into(),
            ));
        }
        Ok(saved.current())
    })
    .transpose()
}

fn tombstone(connection: &Connection, workspace: &str, subject: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT change_id FROM subject_permission_tombstones WHERE workspace_id=?1 AND subject_id=?2",
            params![workspace, subject],
            |row| row.get(0),
        )
        .optional()?)
}

fn state(
    connection: &Connection,
    workspace: &str,
    subject: &str,
) -> Result<(Option<String>, Option<CurrentSubjectPolicy>)> {
    let policy = current(connection, workspace, subject)?;
    let change_id = match &policy {
        Some(policy) => Some(policy.change_id.clone()),
        None => tombstone(connection, workspace, subject)?,
    };
    Ok((change_id, policy))
}

fn save(
    connection: &Connection,
    workspace: &str,
    subject: &str,
    policy: &CurrentSubjectPolicy,
) -> Result<()> {
    connection.execute(
        "DELETE FROM subject_permission_tombstones WHERE workspace_id=?1 AND subject_id=?2",
        params![workspace, subject],
    )?;
    connection.execute(
        "INSERT INTO workspace_configuration_policies(workspace_id,subject_id,policy_json) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,subject_id) DO UPDATE SET policy_json=excluded.policy_json",
        params![workspace, subject, json(&AdoptedSubjectPolicy::from_current(subject, policy))?],
    )?;
    Ok(())
}

// Resolve only shared configuration in the same transaction as the Subject write.
// Calling the store-level configured_permission_host here would reenter its lock.
fn configured_host(
    connection: &Connection,
    workspace: &str,
    host: &PermissionHost,
) -> Result<PermissionHost> {
    require_host(workspace, host)?;
    let state = crate::config_source::load_state(connection, workspace)?
        .ok_or_else(|| Error::RegistryInconsistency("Workspace source not materialized".into()))?;
    let evaluation = config_source::SnapshotEnvironment::new(state.snapshot)
        .evaluate_contract(&state.contract)
        .map_err(|error| {
            Error::RegistryInconsistency(format!(
                "Workspace permission source unavailable: {error:?}"
            ))
        })?;
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::RegistryInconsistency(
            "Workspace permission projection identity mismatch".into(),
        ));
    }
    policy_config::configured_host(host, &policy_config::source(&evaluation)?)
}

fn validate_policy(host: &PermissionHost, policy: &CurrentSubjectPolicy) -> Result<()> {
    policy_config::validate_assignment(
        host,
        &AssignmentSource::from_assignment(&policy.execution),
    )?;
    policy_config::validate_assignment(host, &AssignmentSource::from_assignment(&policy.delegation))
}

impl SqliteWorkspaceStore {
    /// Read the sole current-policy row, without issuing or enrolling permissions.
    pub fn current_subject_permission_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<CurrentSubjectPolicy>> {
        self.with_conn(|connection| current(connection, workspace, subject))
    }

    /// Read policy and its active or removed state identity from one SQLite snapshot.
    /// Only a Subject with no configured policy or removal has no change identity.
    pub fn subject_permission_policy_state(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<(Option<String>, Option<CurrentSubjectPolicy>)> {
        self.with_conn(|connection| {
            let transaction = connection.unchecked_transaction()?;
            state(&transaction, workspace, subject)
        })
    }

    pub(crate) fn subject_permission_policy_view(
        &self,
        workspace: &str,
        subject: &str,
        host: &PermissionHost,
    ) -> Result<(Option<String>, Option<CurrentSubjectPolicy>, PermissionHost)> {
        self.with_conn(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let (change_id, policy) = state(&transaction, workspace, subject)?;
            let host = configured_host(&transaction, workspace, host)?;
            Ok((change_id, policy, host))
        })
    }

    pub fn subject_permission_policy_change_id(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<String>> {
        self.subject_permission_policy_state(workspace, subject)
            .map(|(change_id, _)| change_id)
    }

    /// The API caller holds native Subject existence/deletion and catalog guards.
    /// Replay returns the historical response, never reapplies its policy. Owner
    /// admission is repeated inside this transaction even for an exact replay.
    pub fn mutate_subject_permission_policy(
        &self,
        workspace: &str,
        account: &str,
        subject: &str,
        request: &SubjectPermissionPolicyMutation,
        host: &PermissionHost,
    ) -> Result<SubjectPermissionPolicyMutationResponse> {
        self.with_immediate_transaction(|transaction| {
            require_owner(transaction, workspace, account)?;
            server_api::authorization::validate_authorization_identifier(subject)
                .map_err(Error::InvalidInput)?;
            server_api::authorization::validate_authorization_identifier(&request.operation_id)
                .map_err(Error::InvalidInput)?;
            require_host(workspace, host)?;
            let request_json = json(request)?;
            let saved: Option<(String, String, String, String)> = transaction
                .query_row(
                    "SELECT subject_id,account_id,request_json,result_json FROM subject_permission_operations WHERE workspace_id=?1 AND operation_id=?2",
                    params![workspace, request.operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            if let Some((saved_subject, saved_account, saved_request, result)) = saved {
                if saved_subject != subject || saved_account != account || saved_request != request_json {
                    return Err(conflict("Subject permission operation ID was reused with different input"));
                }
                return decode(&result);
            }
            let (observed_change_id, previous) = state(transaction, workspace, subject)?;
            if observed_change_id != request.expected_change_id {
                return Err(conflict(
                    "Subject permission policy changed; reload before a new operation",
                ));
            }
            let change_id = uuid::Uuid::new_v4().to_string();
            let policy = match &request.action {
                SubjectPermissionPolicyAction::Replace {
                    execution,
                    delegation,
                } => {
                    let host = configured_host(transaction, workspace, host)?;
                    let policy = CurrentSubjectPolicy {
                        change_id: change_id.clone(),
                        execution: execution.clone(),
                        delegation: delegation.clone(),
                        legacy_constraints: previous.and_then(|policy| policy.legacy_constraints),
                        reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
                    };
                    validate_policy(&host, &policy)?;
                    save(transaction, workspace, subject, &policy)?;
                    Some(policy)
                }
                SubjectPermissionPolicyAction::Remove => {
                    transaction.execute(
                        "DELETE FROM workspace_configuration_policies WHERE workspace_id=?1 AND subject_id=?2",
                        params![workspace, subject],
                    )?;
                    transaction.execute(
                        "INSERT INTO subject_permission_tombstones(workspace_id,subject_id,change_id) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,subject_id) DO UPDATE SET change_id=excluded.change_id",
                        params![workspace, subject, change_id],
                    )?;
                    None
                }
            };
            let response = SubjectPermissionPolicyMutationResponse {
                change_id: Some(change_id),
                policy,
            };
            transaction.execute(
                "INSERT INTO subject_permission_operations(workspace_id,operation_id,subject_id,account_id,request_json,result_json) VALUES(?1,?2,?3,?4,?5,?6)",
                params![workspace, request.operation_id, subject, account, request_json, json(&response)?],
            )?;
            Ok(response)
        })
    }

    /// Consume only native creation evidence, never today's creation defaults.
    /// A failed Server commit leaves enrollment retryable; an explicit decision or
    /// either generation of consumption receipt prevents later default revival.
    pub fn enroll_created_subject_policy<L>(
        &self,
        workspace: &str,
        account: &str,
        native: &subjektiv::SubjektivStore,
        subject: &str,
        acquire_host: impl FnOnce() -> Result<(L, PermissionHost)>,
    ) -> Result<Option<CurrentSubjectPolicy>> {
        self.enroll_created_subject_policy_observed(
            workspace,
            account,
            native,
            subject,
            acquire_host,
            || {},
        )
    }

    pub(crate) fn enroll_created_subject_policy_observed<L>(
        &self,
        workspace: &str,
        account: &str,
        native: &subjektiv::SubjektivStore,
        subject: &str,
        acquire_host: impl FnOnce() -> Result<(L, PermissionHost)>,
        before_feature: impl FnOnce(),
    ) -> Result<Option<CurrentSubjectPolicy>> {
        self.with_conn(|connection| require_owner(connection, workspace, account))?;
        if native.scope_id() != workspace {
            return Err(Error::WorkspaceIdMismatch);
        }
        let identity = native
            .database_identity()
            .map_err(|error| Error::Store(format!("Subject enrollment identity: {error}")))?;
        before_feature();
        native
            .with_permission_authority_guard(&identity, |authority| {
                authority.require_subject(subject)?;
                // Retain the exact creation policy as immutable enrollment evidence.
                let enrollment = authority.subject_permission_enrollment(subject)?;
                Ok((|| {
                    let Some(enrollment) = enrollment else {
                        return Ok(None);
                    };
                    let (_lease, host) = acquire_host()?;
                    require_host(workspace, &host)?;
                    self.with_immediate_transaction(|transaction| {
                        require_owner(transaction, workspace, account)?;
                        enroll(transaction, workspace, subject, &enrollment, &host)
                    })
                })())
            })
            .map_err(|error| Error::Store(format!("Subject enrollment guard: {error}")))?
    }
}

fn enroll(
    connection: &Connection,
    workspace: &str,
    subject: &str,
    enrollment: &SubjectPolicySource,
    host: &PermissionHost,
) -> Result<Option<CurrentSubjectPolicy>> {
    let source_json = json(enrollment)?;
    let consumed: Option<String> = connection
        .query_row(
            "SELECT source_json FROM subject_permission_enrollments WHERE workspace_id=?1 AND subject_id=?2",
            params![workspace, subject],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(consumed) = consumed {
        if consumed != source_json {
            return Err(conflict(
                "Subject creation enrollment source changed after consumption",
            ));
        }
        return current(connection, workspace, subject);
    }
    let previous = current(connection, workspace, subject)?;
    let decided: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_permission_tombstones WHERE workspace_id=?1 AND subject_id=?2) OR EXISTS(SELECT 1 FROM subject_permission_operations WHERE workspace_id=?1 AND subject_id=?2)",
        params![workspace, subject],
        |row| row.get(0),
    )?;
    let policy = if previous.is_some() || decided {
        previous
    } else {
        let SubjectPolicySource::Feature(policy) = enrollment else {
            return Err(Error::InvalidInput(
                "Legacy creation enrollment requires explicit Subject permission replacement; no default was issued".into(),
            ));
        };
        let policy = AdoptedSubjectPolicy::from_current(subject, policy).current();
        let host = configured_host(connection, workspace, host)?;
        validate_policy(&host, &policy)?;
        save(connection, workspace, subject, &policy)?;
        Some(policy)
    };
    connection.execute(
        "INSERT INTO subject_permission_enrollments(workspace_id,subject_id,source_json) VALUES(?1,?2,?3)",
        params![workspace, subject, source_json],
    )?;
    Ok(policy)
}

#[cfg(test)]
#[path = "subject_policy_tests.rs"]
mod tests;
