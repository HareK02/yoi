//! Owner-only adoption of Workspace source and Subject policy in the Server transaction.
//!
//! The old Feature database is exported and permanently sealed before its immutable
//! transfer is adopted here. That preparation is not a distributed commit: failure
//! after seal leaves an explicit, resumable transfer, never a legacy fallback.
//! Normal saves never touch the old database. Object preparation, source,
//! projection, policy, editing head and receipt commit in one caller-owned transaction.
//!
//! # Editing and reconciliation
//!
//! Owner authentication precedes all source, preview, adoption and reconciliation
//! operations. The public transport contract belongs to
//! [`server_api::configuration_adoption`]. It extends the existing virtual tree;
//! imports resolve within one snapshot and no host filesystem path grants access.
//! Before cutover, retrieval returns an evaluated editable migration draft, not a
//! saved change. Preview is read-only and reports changed paths and Subjects.
//!
//! Content identity and editing history are distinct. Adoption compares the
//! observed history head as well as the source digest, so A -> B -> A cannot
//! authorize an old editor. The request fingerprint binds Workspace, authenticated
//! owner and the entire request (including catalog, export, head and changes).
//! Reuse of an operation ID with different input is a conflict. Exact replay
//! returns historical evidence before stale-head checks; it never reapplies policy.
//!
//! After a lost response, retain the full request and reconcile it with that same
//! ID. An adopted receipt proves that request's stored result, not today's head.
//! A pending transfer proves the immutable legacy seal, not Server adoption.
//! Absence of either is not evidence of failure or permission to generate a new
//! ID. Retry the exact request; read current source before a distinct edit.
//!
//! # Cutover boundary
//!
//! Initial cutover and legacy logical config writes acquire the guarded Feature
//! authority transaction before Server storage. Sealing commits independently;
//! the outer Feature connection guard remains held through the Server result.
//! After seal, both old policy writes and old logical source writes reject,
//! including when Server persistence rolls back. Reopen/retry uses the same
//! identity-bound export and request. No in-memory mutex is the durable fence.
//! Validation (including blocking migration diagnostics) precedes seal. Failure
//! there retains legacy authority and the previous source/policy/head/receipt.
//! A failure after seal retains the old Server source but fails closed for runtime
//! reads until the same transfer is adopted. This is not a distributed atomic
//! commit or a guarantee for arbitrary machine loss at every instruction.
//!
//! Existing explicit permissions are never automatically merged or replaced,
//! even when empty. Unsupported operations or child-expanding conversions block
//! adoption rather than dropping entries. Exact limited legacy constraints and
//! change identities remain in the domain, separate from editable permission keys.
//! History and child grants are retained, not regenerated as new authorizations.
//!
//! A receipt separately confirms source storage and policy commit. Its typed
//! runtime application means the next current-policy check, not interruption,
//! reload or termination of an already-running Worker. Legacy consumers cannot
//! turn a new unrepresented key into an all-target grant.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{ConfigTreeSnapshot, WorkspaceConfigSchemaBundle};
use content_history::HistoryId;
use content_history_store::{Access, Authority, ScopeKey, ScopedStore};
use feature_permissions::PermissionHost;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::config_source::{ConfigCommitRequest, WorkspaceConfigState};
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
    pub expected_legacy_export: Option<String>,
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

/// Only the trusted migration boundary constructs this from a verified Feature
/// export. The transfer identity binds Workspace, physical DB, export and the full
/// adoption request. It is not an HTTP request or an authority credential.
pub(crate) struct ConfigurationPolicyTransfer {
    pub transfer_identity: String,
    pub physical_identity: String,
    pub export_digest: String,
    pub policies: BTreeMap<String, AdoptedSubjectPolicy>,
}

pub(crate) struct CanonicalConfigurationSource {
    pub store: SqliteWorkspaceStore,
    pub host: PermissionHost,
}

impl subjektiv::CanonicalPermissionSource for CanonicalConfigurationSource {
    fn subject_permissions(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::SubjectPermissions>> {
        if self.host.catalog().workspace.as_str() != workspace {
            return Err(subjektiv::SubjektivError::PermissionTransferConflict);
        }
        self.store
            .adopted_subject_policy(workspace, subject)
            .and_then(|policy| {
                policy
                    .map(|policy| policy.legacy_read(&self.host))
                    .transpose()
            })
            .map_err(|error| {
                subjektiv::SubjektivError::InvalidRecord(format!(
                    "canonical configuration policy unavailable: {error}"
                ))
            })
    }
}

pub(crate) struct ConfigurationCurrent {
    pub state: WorkspaceConfigState,
    pub history_head: Option<String>,
    pub canonical: bool,
    pub policies: BTreeMap<String, AdoptedSubjectPolicy>,
}

pub(crate) struct ConfigurationMigrationBaseline<'a> {
    pub original: &'a WorkspaceConfigState,
    pub state: &'a WorkspaceConfigState,
    pub policies: &'a BTreeMap<String, AdoptedSubjectPolicy>,
    pub diagnostics: &'a [policy_config::MigrationDiagnostic],
    pub export_digest: &'a str,
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
fn conflict(message: &str) -> Error {
    Error::WorkspaceConfigConflict(message.into())
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
fn require_owner(conn: &Connection, workspace: &str, account: &str) -> Result<()> {
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND owner_account_id=?2 AND state='active')",
        params![workspace, account], |r| r.get(0),
    )?;
    if !allowed {
        return Err(Error::WorkspacePermissionDenied(
            "configuration management requires the Workspace owner".into(),
        ));
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

pub(crate) fn canonical(conn: &Connection, workspace: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_configuration_authorities WHERE workspace_id=?1)",
        [workspace],
        |row| row.get(0),
    )?)
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

fn receipt(
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
fn changed_subjects(
    before: &BTreeMap<String, AdoptedSubjectPolicy>,
    after: &BTreeMap<String, AdoptedSubjectPolicy>,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|subject| before.get(*subject) != after.get(*subject))
        .cloned()
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
            require_owner(&tx, workspace, account)?;
            Ok(ConfigurationCurrent {
                state: crate::config_source::load_state(&tx, workspace)?.ok_or_else(|| {
                    Error::InvalidInput("Workspace source not materialized".into())
                })?,
                history_head: head(&tx, workspace)?.map(|id| id.to_string()),
                canonical: canonical(&tx, workspace)?,
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
            require_owner(conn, workspace, account)?;
            match receipt(conn, workspace, &request.operation_id)? {
                Some((saved, result)) if saved == fingerprint => Ok(Some(result)),
                Some(_) => Err(conflict("operation ID reused with another request")),
                None => Ok(None),
            }
        })
    }

    /// Authentication supplies `account`; it is never accepted from an adoption DTO.
    pub fn configuration_head(&self, workspace: &str, account: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            require_owner(conn, workspace, account)?;
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
            require_owner(conn, workspace, account)?;
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
        baseline: Option<&ConfigurationMigrationBaseline<'_>>,
        mut require_subject: impl FnMut(&str) -> Result<()>,
    ) -> Result<ConfigurationPreview> {
        self.with_conn(|conn| {
            require_owner(conn, workspace, account)?;
            let tx = conn.unchecked_transaction()?;
            let (current, state, source) = prepare(
                &tx,
                workspace,
                request,
                schema,
                host,
                baseline,
                &mut require_subject,
            )?;
            let before = match baseline {
                Some(baseline) => baseline.policies.clone(),
                None => policies(&tx, workspace)?,
            };
            let mut after = before.clone();
            policy_config::apply(&mut after, &source, "preview");
            Ok(ConfigurationPreview {
                expected_head: request.expected_head.clone(),
                changed_paths: changed_paths(&current.snapshot, &state.snapshot),
                changed_subjects: changed_subjects(&before, &after),
                state,
            })
        })
    }

    /// The transfer callback may seal the old DB only after all validation and
    /// current-head checks. If subsequent Server persistence fails, that specific
    /// transfer remains pending; callers reconcile/retry the same request, not a new ID.
    pub(crate) fn adopt_configuration(
        &self,
        workspace: &str,
        account: &str,
        request: &ConfigurationAdoptionRequest,
        schema: WorkspaceConfigSchemaBundle,
        host: &PermissionHost,
        baseline: Option<&ConfigurationMigrationBaseline<'_>>,
        mut require_subject: impl FnMut(&str) -> Result<()>,
        mut prepare_transfer: impl FnMut(&str) -> Result<ConfigurationPolicyTransfer>,
    ) -> Result<ConfigurationAdoptionResult> {
        let fingerprint = request_fingerprint(workspace, account, request)?;
        self.with_immediate_transaction(|tx| {
            require_owner(tx,workspace,account)?;
            if let Some((saved,result)) = receipt(tx,workspace,&request.operation_id)? {
                if saved != fingerprint { return Err(conflict("operation ID reused with another request")); }
                return Ok(result);
            }
            let (_,state,source) = prepare(tx,workspace,request,schema,host,baseline,&mut require_subject)?;
            let authority_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_configuration_authorities WHERE workspace_id=?1)", [workspace], |r|r.get(0))?;
            if authority_exists && (baseline.is_some() || request.expected_legacy_export.is_some()) {
                return Err(conflict("legacy transfer is already adopted; reload current source"));
            }
            if !authority_exists && request.expected_legacy_export.is_none() {
                return Err(conflict("legacy export identity must be observed before transfer"));
            }
            let mut current_policies = if authority_exists { policies(tx,workspace)? } else {
                let transfer = prepare_transfer(&fingerprint)?;
                if transfer.transfer_identity != fingerprint || request.expected_legacy_export.as_deref() != Some(transfer.export_digest.as_str()) {
                    return Err(conflict("transfer does not bind the complete adoption request and export"));
                }
                tx.execute("INSERT INTO workspace_configuration_authorities VALUES(?1,?2,?3,?4)",params![workspace,transfer.transfer_identity,transfer.physical_identity,transfer.export_digest])?;
                transfer.policies
            };
            // Client-selected operation IDs are not policy change identities: reusing
            // an old legacy change ID must never make a stale child grant current.
            policy_config::apply(&mut current_policies,&source,&format!("configuration:{fingerprint}"));
            crate::config_source::sync_adopted_config_references(tx, workspace, &state)?;
            let authority = OwnerScope(workspace);
            let content = ScopedStore::new(tx,scope(workspace),&authority);
            let prepared = content_history_adapters::settings::prepare(&state.snapshot).map_err(content_error)?;
            for (id,object) in &prepared.objects { content.put(*id,&object.encode()).map_err(content_error)?; }
            tx.execute("DELETE FROM workspace_config_entries WHERE workspace_id=?1",[workspace])?;
            crate::config_source::insert_materialized_state(tx,workspace,&state,&chrono::Utc::now().to_rfc3339())?;
            tx.execute("DELETE FROM workspace_configuration_policies WHERE workspace_id=?1",[workspace])?;
            for (subject,policy) in &current_policies {
                tx.execute("INSERT INTO workspace_configuration_policies VALUES(?1,?2,?3)", params![workspace,subject,json(policy)?])?;
            }
            let new_head = content.adopt(prepared.root,head(tx,workspace)?,account.into(),chrono::Utc::now().timestamp_micros()).map_err(content_error)?;
            let result = ConfigurationAdoptionResult {
                workspace_id: workspace.into(), operation_id: request.operation_id.clone(),
                history_head: new_head.to_string(), state, source_stored: true, policy_committed: true,
                runtime_application: RuntimePolicyApplication::NextCurrentPolicyCheck,
            };
            tx.execute("INSERT INTO workspace_configuration_receipts VALUES(?1,?2,?3,?4)",params![workspace,request.operation_id,fingerprint,json(&result)?])?;
            Ok(result)
        })
    }

    pub fn adopted_subject_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> Result<Option<AdoptedSubjectPolicy>> {
        self.with_conn(|conn| {
            let canonical: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workspace_configuration_authorities WHERE workspace_id=?1)",[workspace],|r|r.get(0))?;
            if !canonical { return Err(Error::InvalidInput("Workspace policy transfer is not adopted".into())); }
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
    baseline: Option<&ConfigurationMigrationBaseline<'_>>,
    require_subject: &mut impl FnMut(&str) -> Result<()>,
) -> Result<(WorkspaceConfigState, WorkspaceConfigState, PermissionSource)> {
    if let Some(baseline) = baseline {
        if baseline.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                policy_config::MigrationDiagnostic::ExplicitConfigurationRequired { .. }
            )
        }) {
            return Err(Error::InvalidInput("legacy permissions require explicit configuration before adoption; legacy authority was not sealed".into()));
        }
    }
    if host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspacePermissionDenied(
            "permission registry belongs to another Workspace".into(),
        ));
    }
    if catalog_digest(host)? != request.expected_catalog_digest {
        return Err(conflict("Feature catalog changed"));
    }
    if head(tx, workspace)?.map(|id| id.to_string()) != request.expected_head {
        return Err(conflict("editing history head changed"));
    }
    let current = crate::config_source::load_state(tx, workspace)?
        .ok_or_else(|| Error::InvalidInput("Workspace source not materialized".into()))?;
    let editing_base = if let Some(baseline) = baseline {
        if *baseline.original != current
            || request.expected_legacy_export.as_deref() != Some(baseline.export_digest)
        {
            return Err(conflict("migration source or export changed"));
        }
        baseline.state
    } else {
        &current
    };
    if editing_base.snapshot.digest != request.source.base_digest {
        return Err(conflict("source content changed"));
    }
    crate::config_source::validate_entrypoint_request(&request.source.entrypoints)?;
    let candidate = crate::config_source::evaluate_candidate_in_transaction(
        tx,
        workspace,
        editing_base.clone(),
        crate::config_source::load_evaluation_identity(tx, workspace)?,
        &request.source.changes,
        schema,
    )?;
    crate::config_source::validate_workspace_config_candidate_projections(workspace, &candidate)?;
    let source = policy_config::source(&candidate.evaluation)?;
    policy_config::validate_source(host, &source, require_subject)?;
    let state = WorkspaceConfigState {
        snapshot: candidate.snapshot,
        contract: candidate.contract,
        projection_digest: candidate.evaluation.projection_digest,
    };
    Ok((current, state, source))
}

#[cfg(test)]
#[path = "config_adoption_tests.rs"]
mod tests;
