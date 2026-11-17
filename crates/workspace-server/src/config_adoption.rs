//! Owner-authenticated shared configuration publication, independent of Subject policy.
//! Source and editing-head CAS and exact request receipts fence concurrent saves and retries.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{ConfigTreeSnapshot, WorkspaceConfigSchemaBundle};
use content_history::HistoryId;
use content_history_store::{Access, Authority, ScopeKey, ScopedStore};
use feature_permissions::PermissionHost;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::config_source::{ConfigCommitRequest, WorkspaceConfigState};
use crate::configuration_failure::{
    ConfigurationFailure, ConfigurationFailureOperation as FailureOperation,
    ConfigurationFailureReason as FailureReason,
};
use crate::policy_config::{self, AdoptedSubjectPolicy, PermissionSource};
use crate::{Error, Result, SqliteWorkspaceStore};

pub(crate) const SCHEMA: &str = include_str!("config_adoption.sql");
pub(crate) const MIGRATION_NAME: &str = "Atomic Workspace source and Subject policy adoption";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationAdoptionRequest {
    pub operation_id: String,
    /// Editing history identity, not a content digest. Null is only the initial head.
    pub expected_head: Option<String>,
    pub expected_catalog_digest: String,
    pub source: ConfigCommitRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePolicyApplication {
    NextCurrentPolicyCheck,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationAdoptionResult {
    pub workspace_id: String,
    pub operation_id: String,
    pub history_head: String,
    pub state: WorkspaceConfigState,
    pub source_stored: bool,
    pub policy_committed: bool,
    /// Consumers must reload current policy before their next protected operation.
    /// This is not confirmation that a running Worker has stopped or reloaded.
    pub runtime_application: RuntimePolicyApplication,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPreview {
    pub expected_head: Option<String>,
    pub state: WorkspaceConfigState,
    pub changed_paths: Vec<String>,
    pub changed_subjects: Vec<String>,
}

pub(crate) struct ConfigurationCurrent {
    pub state: WorkspaceConfigState,
    pub history_head: Option<String>,
    pub policies: BTreeMap<String, AdoptedSubjectPolicy>,
}

struct OwnerScope<'a>(&'a str);
impl Authority for OwnerScope<'_> {
    fn allows(&self, key: &ScopeKey, access: Access) -> bool {
        key.workspace == self.0
            && key.target == "configuration"
            && matches!(access, Access::Read | Access::Write)
    }
}

fn content_error(e: impl std::fmt::Display) -> Error {
    Error::Store(format!("configuration content: {e}"))
}
fn failure(operation: FailureOperation, reason: FailureReason) -> Error {
    ConfigurationFailure::new(operation, reason).into()
}
fn during(operation: FailureOperation, error: Error) -> Error {
    match error {
        Error::Configuration(cause) => cause.with_operation(operation).into(),
        other => other,
    }
}
fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Store(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| Error::RegistryInconsistency(e.to_string()))
}
fn scope(workspace: &str) -> ScopeKey {
    ScopeKey {
        workspace: workspace.into(),
        target: "configuration".into(),
    }
}
pub(crate) fn require_owner(
    conn: &Connection,
    workspace: &str,
    account: &str,
    operation: FailureOperation,
) -> Result<()> {
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND owner_account_id=?2 AND state='active')",
        params![workspace, account], |r| r.get(0),
    )?;
    if !allowed {
        return Err(failure(operation, FailureReason::OwnerRequired));
    }
    Ok(())
}

pub(crate) fn head(conn: &Connection, workspace: &str) -> Result<Option<HistoryId>> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT head FROM content_scopes WHERE workspace=?1 AND target='configuration'",
            [workspace],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    bytes
        .map(|bytes| {
            bytes.try_into().map(HistoryId::from_bytes).map_err(|_| {
                Error::RegistryInconsistency("invalid configuration editing head".into())
            })
        })
        .transpose()
}

pub(crate) fn request_fingerprint(
    workspace: &str,
    account: &str,
    request: &ConfigurationAdoptionRequest,
) -> Result<String> {
    server_api::authorization::validate_authorization_identifier(&request.operation_id)
        .map_err(Error::InvalidInput)?;
    Ok(config_source::digest_bytes(
        json(&(workspace, account, request))?.as_bytes(),
    ))
}

pub(crate) fn has_history(conn: &Connection, workspace: &str) -> Result<bool> {
    Ok(head(conn, workspace)?.is_some())
}

pub fn catalog_digest(host: &PermissionHost) -> Result<String> {
    Ok(config_source::digest_bytes(
        json(&host.catalog())?.as_bytes(),
    ))
}

fn policies(conn: &Connection, workspace: &str) -> Result<BTreeMap<String, AdoptedSubjectPolicy>> {
    let mut query = conn.prepare("SELECT subject_id,policy_json FROM workspace_configuration_policies WHERE workspace_id=?1 ORDER BY subject_id")?;
    let mut result = BTreeMap::new();
    for row in query.query_map([workspace], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (subject, raw) = row?;
        let policy: AdoptedSubjectPolicy = decode(&raw)?;
        if subject != policy.source.subject_id {
            return Err(Error::RegistryInconsistency(
                "Subject policy identity mismatch".into(),
            ));
        }
        result.insert(subject, policy);
    }
    Ok(result)
}

pub(crate) fn receipt(
    conn: &Connection,
    workspace: &str,
    operation: &str,
) -> Result<Option<(String, ConfigurationAdoptionResult)>> {
    let row: Option<(String,String)> = conn.query_row(
        "SELECT request_fingerprint,result_json FROM workspace_configuration_receipts WHERE workspace_id=?1 AND operation_id=?2",
        params![workspace,operation], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    row.map(|(fingerprint, result)| Ok((fingerprint, decode(&result)?)))
        .transpose()
}

fn changed_paths(before: &ConfigTreeSnapshot, after: &ConfigTreeSnapshot) -> Vec<String> {
    before
        .entries
        .keys()
        .chain(after.entries.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.entries.get(*path) != after.entries.get(*path))
        .map(|path| path.as_str().to_owned())
        .collect()
}

impl SqliteWorkspaceStore {
    pub(crate) fn configuration_current(
        &self,
        workspace: &str,
        account: &str,
    ) -> Result<ConfigurationCurrent> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            require_owner(&tx, workspace, account, FailureOperation::Source)?;
            Ok(ConfigurationCurrent {
                state: crate::config_source::load_state(&tx, workspace)?.ok_or_else(|| {
                    Error::InvalidInput("Workspace source not materialized".into())
                })?,
                history_head: head(&tx, workspace)?.map(|id| id.to_string()),
                policies: policies(&tx, workspace)?,
            })
        })
    }

    pub(crate) fn configuration_request_result(
        &self,
        workspace: &str,
        account: &str,
        request: &ConfigurationAdoptionRequest,
    ) -> Result<Option<ConfigurationAdoptionResult>> {
        let fingerprint = request_fingerprint(workspace, account, request)?;
        self.with_conn(|conn| {
            require_owner(conn, workspace, account, FailureOperation::Reconcile)?;
            match receipt(conn, workspace, &request.operation_id)? {
                Some((saved, result)) if saved == fingerprint => Ok(Some(result)),
                Some(_) => Err(failure(
                    FailureOperation::Reconcile,
                    FailureReason::RequestIdentityConflict,
                )),
                None => Ok(None),
            }
        })
    }

    /// Authentication supplies `account`; it is never accepted from an adoption DTO.
    pub fn configuration_head(&self, workspace: &str, account: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            require_owner(conn, workspace, account, FailureOperation::Source)?;
            Ok(head(conn, workspace)?.map(|id| id.to_string()))
        })
    }

    /// Result lookup is a read. A historical receipt never reactivates policy.
    pub fn configuration_adoption_result(
        &self,
        workspace: &str,
        account: &str,
        operation: &str,
    ) -> Result<Option<ConfigurationAdoptionResult>> {
        self.with_conn(|conn| {
            require_owner(conn, workspace, account, FailureOperation::Reconcile)?;
            Ok(receipt(conn, workspace, operation)?.map(|(_, result)| result))
        })
    }

    pub(crate) fn preview_configuration_adoption(
        &self,
        workspace: &str,
        account: &str,
        request: &ConfigurationAdoptionRequest,
        schema: WorkspaceConfigSchemaBundle,
        host: &PermissionHost,
    ) -> Result<ConfigurationPreview> {
        self.with_conn(|conn| {
            require_owner(conn, workspace, account, FailureOperation::Preview)?;
            let tx = conn.unchecked_transaction()?;
            let (current, state, _source) = prepare(
                &tx,
                workspace,
                request,
                schema,
                host,
                FailureOperation::Preview,
            )?;
            Ok(ConfigurationPreview {
                expected_head: request.expected_head.clone(),
                changed_paths: changed_paths(&current.snapshot, &state.snapshot),
                changed_subjects: vec![],
                state,
            })
        })
    }

    /// Publish shared configuration and its exact request receipt in one transaction.
    pub(crate) fn adopt_configuration(
        &self,
        workspace: &str,
        account: &str,
        request: &ConfigurationAdoptionRequest,
        schema: WorkspaceConfigSchemaBundle,
        host: &PermissionHost,
    ) -> Result<ConfigurationAdoptionResult> {
        let fingerprint = request_fingerprint(workspace, account, request)?;
        self.with_immediate_transaction(|tx| {
            require_owner(tx, workspace, account, FailureOperation::Adopt)?;
            if let Some((saved, result)) = receipt(tx, workspace, &request.operation_id)? {
                if saved != fingerprint {
                    return Err(failure(
                        FailureOperation::Adopt,
                        FailureReason::RequestIdentityConflict,
                    ));
                }
                return Ok(result);
            }
            let (_, state, _source) = prepare(
                tx,
                workspace,
                request,
                schema,
                host,
                FailureOperation::Adopt,
            )?;
            crate::config_source::sync_adopted_config_references(tx, workspace, &state)?;
            let authority = OwnerScope(workspace);
            let content = ScopedStore::new(tx, scope(workspace), &authority);
            let prepared = content_history_adapters::settings::prepare(&state.snapshot)
                .map_err(content_error)?;
            for (id, object) in &prepared.objects {
                content.put(*id, &object.encode()).map_err(content_error)?;
            }
            tx.execute(
                "DELETE FROM workspace_config_entries WHERE workspace_id=?1",
                [workspace],
            )?;
            crate::config_source::insert_materialized_state(
                tx,
                workspace,
                &state,
                &chrono::Utc::now().to_rfc3339(),
            )?;
            let new_head = content
                .adopt(
                    prepared.root,
                    head(tx, workspace)?,
                    account.into(),
                    chrono::Utc::now().timestamp_micros(),
                )
                .map_err(content_error)?;
            let result = ConfigurationAdoptionResult {
                workspace_id: workspace.into(),
                operation_id: request.operation_id.clone(),
                history_head: new_head.to_string(),
                state,
                source_stored: true,
                policy_committed: true,
                runtime_application: RuntimePolicyApplication::NextCurrentPolicyCheck,
            };
            tx.execute(
                "INSERT INTO workspace_configuration_receipts VALUES(?1,?2,?3,?4)",
                params![workspace, request.operation_id, fingerprint, json(&result)?],
            )?;
            Ok(result)
        })
    }

    pub fn adopted_subject_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<AdoptedSubjectPolicy>> {
        self.with_conn(|conn| {
            let raw: Option<String> = conn.query_row("SELECT policy_json FROM workspace_configuration_policies WHERE workspace_id=?1 AND subject_id=?2",params![workspace,subject],|r|r.get(0)).optional()?;
            raw.map(|raw|decode(&raw)).transpose()
        })
    }
}

fn prepare(
    tx: &Transaction<'_>,
    workspace: &str,
    request: &ConfigurationAdoptionRequest,
    schema: WorkspaceConfigSchemaBundle,
    host: &PermissionHost,
    operation: FailureOperation,
) -> Result<(WorkspaceConfigState, WorkspaceConfigState, PermissionSource)> {
    if host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspacePermissionDenied(
            "permission registry belongs to another Workspace".into(),
        ));
    }
    if catalog_digest(host)? != request.expected_catalog_digest {
        return Err(failure(operation, FailureReason::CatalogChanged));
    }
    if head(tx, workspace)?.map(|id| id.to_string()) != request.expected_head {
        return Err(failure(operation, FailureReason::EditingHeadChanged));
    }
    let current = crate::config_source::load_state(tx, workspace)?
        .ok_or_else(|| Error::InvalidInput("Workspace source not materialized".into()))?;
    let editing_base = &current;
    if editing_base.snapshot.digest != request.source.base_digest {
        return Err(failure(operation, FailureReason::SourceContentChanged));
    }
    crate::config_source::validate_entrypoint_request(&request.source.entrypoints)?;
    let candidate = crate::config_source::evaluate_candidate_in_transaction(
        tx,
        workspace,
        editing_base.clone(),
        crate::config_source::load_evaluation_identity(tx, workspace)?,
        &request.source.changes,
        schema,
    )
    .map_err(|error| during(operation, error))?;
    crate::config_source::validate_workspace_config_candidate_projections(workspace, &candidate)?;
    crate::repository_access::validate_repository_access_in_transaction(tx, workspace, &candidate)?;
    let source = policy_config::source(&candidate.evaluation)?;
    policy_config::validate_source(host, &source)?;
    let state = WorkspaceConfigState {
        snapshot: candidate.snapshot,
        contract: candidate.contract,
        projection_digest: candidate.evaluation.projection_digest,
    };
    Ok((current, state, source))
}

mod worker_permissions;

#[cfg(test)]
#[path = "config_adoption/worker_permissions_tests.rs"]
mod worker_permissions_tests;

#[cfg(test)]
#[path = "config_adoption_tests.rs"]
mod tests;
