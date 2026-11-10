//! Reconstruct Worker creation selections from the materialized Workspace source.
//!
//! Shared selection is independent of Subject policy adoption. Legacy Subject
//! reads still require adoption before consulting transferred policy.
//!
//! These reads do not issue grants. Each caller enforces its own launch authority
//! and binds the selection before protected supply or Runtime dispatch. A retry
//! or restore uses the saved grant, never this selection routine's latest default.

use feature_permissions::{PermissionAssignment, PermissionHost};
use rusqlite::OptionalExtension;

use super::{canonical, policy_config};
use crate::{Error, Result, SqliteWorkspaceStore};

fn current_source(
    connection: &rusqlite::Connection,
    workspace: &str,
) -> Result<policy_config::PermissionSource> {
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
    policy_config::source(&evaluation)
}

fn require_workspace(workspace: &str, host: &PermissionHost) -> Result<()> {
    if host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    Ok(())
}

impl SqliteWorkspaceStore {
    // Native effect admission already owns a query-only Host transaction. Reuse
    // that reservation; ordinary callers still need one coherent read snapshot.
    fn with_permission_snapshot<T>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        self.with_conn(|connection| {
            if connection.is_autocommit() {
                let transaction = connection.unchecked_transaction()?;
                read(&transaction)
            } else {
                read(connection)
            }
        })
    }

    pub(crate) fn adopted_subject_legacy_permissions(
        &self,
        workspace: &str,
        subject: &str,
        feature_host: &PermissionHost,
    ) -> Result<Option<server_api::authorization::SubjectPermissions>> {
        require_workspace(workspace, feature_host)?;
        self.with_permission_snapshot(|transaction| {
            if !canonical(transaction, workspace)? {
                return Err(Error::InvalidInput(
                    "Workspace policy transfer is not adopted".into(),
                ));
            }
            let source = current_source(transaction, workspace)?;
            let host = policy_config::configured_host(feature_host, &source)?;
            let raw: Option<String> = transaction
                .query_row(
                    "SELECT policy_json FROM workspace_configuration_policies WHERE workspace_id=?1 AND subject_id=?2",
                    rusqlite::params![workspace, subject],
                    |row| row.get(0),
                )
                .optional()?;
            raw.map(|raw| {
                let policy: policy_config::AdoptedSubjectPolicy = super::decode(&raw)?;
                if policy.source.subject_id != subject {
                    return Err(Error::RegistryInconsistency("Subject policy identity mismatch".into()));
                }
                policy.legacy_read(&host)
            }).transpose()
        })
    }

    /// Rebuild the catalog overlay from the current materialized source, retaining the
    /// supplied Host's current Feature declarations and registration identities.
    /// Use this before current authorization, catalog reads and policy issuance;
    /// do not cache it across source/Feature changes or use an old receipt as input.
    pub fn configured_permission_host(
        &self,
        workspace: &str,
        feature_host: &PermissionHost,
    ) -> Result<PermissionHost> {
        require_workspace(workspace, feature_host)?;
        self.with_permission_snapshot(|transaction| {
            let source = current_source(transaction, workspace)?;
            policy_config::configured_host(feature_host, &source)
        })
    }

    /// Caller-neutral new Worker selection. `None` selects the configured
    /// default; `Some(empty)` stays empty; `Some(selection)` replaces the default.
    /// Return references, not their expansion, so later Set contributions remain
    /// dynamic. The caller owns creation authorization, grant persistence and the
    /// effect-admission fence; this read is neither a credential nor a reservation.
    pub fn worker_permissions(
        &self,
        workspace: &str,
        feature_host: &PermissionHost,
        explicit: Option<&PermissionAssignment>,
    ) -> Result<(PermissionHost, PermissionAssignment)> {
        require_workspace(workspace, feature_host)?;
        self.with_permission_snapshot(|transaction| {
            let source = current_source(transaction, workspace)?;
            let host = policy_config::configured_host(feature_host, &source)?;
            let assignment = explicit
                .cloned()
                .unwrap_or_else(|| source.worker_default.assignment());
            policy_config::validate_assignment(
                &host,
                &policy_config::AssignmentSource::from_assignment(&assignment),
            )?;
            Ok((host, assignment))
        })
    }

    pub fn worker_assignment(
        &self,
        workspace: &str,
        feature_host: &PermissionHost,
        explicit: Option<&PermissionAssignment>,
    ) -> Result<PermissionAssignment> {
        self.worker_permissions(workspace, feature_host, explicit)
            .map(|(_, assignment)| assignment)
    }
}
