//! Grant cleanup owns one frozen child request before any external release.
//! Retained failure attachments are diagnostics, not dispatch or settlement
//! authority. An admitted child without a confirmed receipt remains uncertain;
//! neither a matching attachment set nor an acknowledgement closes the attempt.
use protocol::background_failure::{BackgroundFailure, StoredFailure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use worker_runtime::{catalog::LogicalWorkdirAttachment, identity::RuntimeWorkerRef};

use crate::{Error, Result, hosts::RuntimeOperationStatus};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRuntimeTarget {
    pub worker: RuntimeWorkerRef,
    pub connection_id: String,
    pub alias: String,
    pub expected_generation: String,
    pub logical_workdir_attachments: Vec<LogicalWorkdirAttachment>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRequest {
    pub operation_id: String,
    pub workspace_id: String,
    pub grant_id: String,
    pub provider_instance_id: String,
    pub grant_generation: u64,
    pub working_directory_id: String,
    pub target: Option<CleanupRuntimeTarget>,
}

impl CleanupRequest {
    pub fn runtime_request(&self) -> Option<runtime_api::WorkerWorkdirAttachmentsOperationRequest> {
        self.target.as_ref().map(
            |target| runtime_api::WorkerWorkdirAttachmentsOperationRequest {
                request_id: self.operation_id.clone(),
                expected_generation: target.expected_generation.clone(),
                logical_workdir_attachments: target
                    .logical_workdir_attachments
                    .iter()
                    .map(|attachment| runtime_api::LogicalWorkdirAttachment {
                        alias: attachment.alias.clone(),
                        working_directory_id: attachment.working_directory_id.clone(),
                        capabilities: attachment.capabilities,
                    })
                    .collect(),
            },
        )
    }
}

impl std::fmt::Debug for CleanupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupRequest").finish_non_exhaustive()
    }
}
impl std::fmt::Debug for CleanupRuntimeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupRuntimeTarget")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupOperation {
    pub request: CleanupRequest,
    pub release_confirmed: bool,
    pub runtime_dispatched: bool,
    pub runtime_status: Option<RuntimeOperationStatus>,
    pub settled: bool,
    failures: Vec<String>,
}

impl CleanupOperation {
    pub fn failures(&self) -> Vec<StoredFailure> {
        self.failures
            .iter()
            .cloned()
            .map(StoredFailure::from_storage)
            .collect()
    }
}

/// The exact committed preimage, not a wall-clock revision or a retry token.
/// A caller cannot reconstruct it from displayed causes or cleanup state.
#[derive(Clone)]
pub struct LoadedCleanupOperation {
    operation: CleanupOperation,
    raw: String,
}

impl std::fmt::Debug for CleanupOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupOperation").finish_non_exhaustive()
    }
}
impl std::fmt::Debug for LoadedCleanupOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedCleanupOperation")
            .finish_non_exhaustive()
    }
}

impl LoadedCleanupOperation {
    pub fn operation(&self) -> &CleanupOperation {
        &self.operation
    }
}

/// A bound diagnostic preimage only; it cannot dispatch or settle cleanup.
#[derive(Clone)]
pub struct LoadedCleanupIntent {
    workspace_id: String,
    grant_id: String,
    provider_instance_id: String,
    grant_generation: u64,
    working_directory_id: String,
    failures: Vec<String>,
    raw: String,
}

impl std::fmt::Debug for LoadedCleanupIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedCleanupIntent")
            .finish_non_exhaustive()
    }
}

impl LoadedCleanupIntent {
    pub fn failures(&self) -> Vec<StoredFailure> {
        self.failures
            .iter()
            .cloned()
            .map(StoredFailure::from_storage)
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
enum CleanupDocument {
    ExternalWorkdirCleanupV1(CleanupOperation),
    ExternalWorkdirCleanupIntentV1 {
        workspace_id: String,
        grant_id: String,
        provider_instance_id: String,
        grant_generation: u64,
        working_directory_id: String,
        failures: Vec<String>,
    },
}

fn encode_intent(
    workspace: &str,
    grant: &str,
    provider: &str,
    generation: u64,
    workdir: &str,
) -> Result<String> {
    serde_json::to_string(&CleanupDocument::ExternalWorkdirCleanupIntentV1 {
        workspace_id: workspace.into(),
        grant_id: grant.into(),
        provider_instance_id: provider.into(),
        grant_generation: generation,
        working_directory_id: workdir.into(),
        failures: Vec::new(),
    })
    .map_err(|_| Error::RegistryInconsistency("External cleanup intent encoding failed".into()))
}

/// Only the transaction that changed nonterminal authority may install this
/// intent. Existing terminal rows never acquire never-dispatched authority.
pub(crate) fn install_fresh_fence_intent(
    conn: &Connection,
    workspace: &str,
    grant: &str,
    now: &str,
) -> Result<()> {
    let (provider, generation, workdir): (String, i64, String) = conn.query_row(
        "SELECT provider_instance_id,generation,workdir_id FROM external_workdir_grants WHERE workspace_id=?1 AND grant_id=?2 AND status IN ('revoked','expired')",
        params![workspace,grant], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    )?;
    let raw = encode_intent(
        workspace,
        grant,
        &provider,
        u64::try_from(generation).map_err(|_| conflict())?,
        &workdir,
    )?;
    if conn.execute("UPDATE external_workdir_grants SET cleanup_error=?3,cleanup_updated_at=?4 WHERE workspace_id=?1 AND grant_id=?2 AND cleanup_state='pending' AND cleanup_error IS NULL",
        params![workspace,grant,raw,now])? != 1 { return Err(conflict()); }
    Ok(())
}

pub(crate) fn has_intent(conn: &Connection, workspace: &str, grant: &str) -> Result<bool> {
    Ok(read_intent(conn, workspace, grant)?.is_some())
}

pub(crate) fn read_intent(
    conn: &Connection,
    workspace: &str,
    grant: &str,
) -> Result<Option<LoadedCleanupIntent>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT cleanup_error FROM external_workdir_grants WHERE workspace_id=?1 AND grant_id=?2 AND cleanup_state='pending'",
            params![workspace, grant],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(raw) = raw.filter(|raw| !legacy_observation_only(Some(raw))) else {
        return Ok(None);
    };
    let document: CleanupDocument = serde_json::from_str(&raw).map_err(|_| conflict())?;
    let CleanupDocument::ExternalWorkdirCleanupIntentV1 {
        workspace_id,
        grant_id,
        provider_instance_id,
        grant_generation,
        working_directory_id,
        failures,
    } = document
    else {
        return Ok(None);
    };
    if workspace_id != workspace || grant_id != grant {
        return Err(conflict());
    }
    ensure_grant_binding(
        conn,
        &workspace_id,
        &grant_id,
        &provider_instance_id,
        grant_generation,
        &working_directory_id,
    )?;
    Ok(Some(LoadedCleanupIntent {
        workspace_id,
        grant_id,
        provider_instance_id,
        grant_generation,
        working_directory_id,
        failures,
        raw,
    }))
}

/// Diagnostics may be retained before Runtime generation lookup succeeds.
/// Exact bytes fence concurrent appends and admission; no owner is created.
pub(crate) fn append_intent_failures(
    conn: &Connection,
    expected: &LoadedCleanupIntent,
    failures: &[BackgroundFailure],
    now: &str,
) -> Result<LoadedCleanupIntent> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let mut saved = expected.clone();
    saved.failures.extend(
        failures
            .iter()
            .map(|failure| failure.as_storage_str().to_owned()),
    );
    let raw = serde_json::to_string(&CleanupDocument::ExternalWorkdirCleanupIntentV1 {
        workspace_id: saved.workspace_id.clone(),
        grant_id: saved.grant_id.clone(),
        provider_instance_id: saved.provider_instance_id.clone(),
        grant_generation: saved.grant_generation,
        working_directory_id: saved.working_directory_id.clone(),
        failures: saved.failures.clone(),
    })
    .map_err(|_| Error::RegistryInconsistency("External cleanup intent encoding failed".into()))?;
    let changed = tx.execute(
        "UPDATE external_workdir_grants SET cleanup_error=?6,cleanup_updated_at=?7
         WHERE workspace_id=?1 AND grant_id=?2 AND provider_instance_id=?3
           AND generation=?4 AND workdir_id=?5 AND status IN ('revoked','expired')
           AND cleanup_state='pending' AND cleanup_error=?8",
        params![
            expected.workspace_id,
            expected.grant_id,
            expected.provider_instance_id,
            i64::try_from(expected.grant_generation).map_err(|_| conflict())?,
            expected.working_directory_id,
            raw,
            now,
            expected.raw
        ],
    )?;
    if changed != 1 {
        return Err(conflict());
    }
    tx.commit()?;
    saved.raw = raw;
    Ok(saved)
}

pub enum CleanupAdmission {
    Admitted(LoadedCleanupOperation),
    Existing(LoadedCleanupOperation),
}

fn conflict() -> Error {
    Error::WorkdirAttachmentConflict("External grant cleanup operation changed".into())
}

fn valid_request(request: &CleanupRequest) -> bool {
    !request.operation_id.is_empty()
        && request.grant_generation > 0
        && request.target.as_ref().is_none_or(|target| {
            !target.expected_generation.is_empty()
                && !target.connection_id.is_empty()
                && workdir::WorkdirAttachmentAlias::new(target.alias.clone()).is_ok()
                && target.logical_workdir_attachments.iter().all(|attachment| {
                    attachment.working_directory_id != request.working_directory_id
                        && attachment.alias.as_str() != target.alias
                })
        })
}

fn parse(raw: &str) -> Result<CleanupOperation> {
    let CleanupDocument::ExternalWorkdirCleanupV1(operation) =
        serde_json::from_str(raw).map_err(|_| {
            Error::RegistryInconsistency("External cleanup operation cannot be read".into())
        })?
    else {
        return Err(conflict());
    };
    if !valid_request(&operation.request)
        || (operation.runtime_status.is_some() && !operation.runtime_dispatched)
        || (operation.runtime_dispatched && !operation.release_confirmed)
        || (operation.runtime_dispatched && operation.request.target.is_none())
        || (operation.settled && !operation.release_confirmed)
        || (operation.settled
            && operation.request.target.is_some()
            && !operation.runtime_dispatched)
    {
        return Err(Error::RegistryInconsistency(
            "Invalid External cleanup operation".into(),
        ));
    }
    Ok(operation)
}

pub(crate) fn diagnostic_failures(raw: &str) -> Vec<StoredFailure> {
    if let Ok(operation) = parse(raw) {
        return operation.failures();
    }
    if let Ok(CleanupDocument::ExternalWorkdirCleanupIntentV1 { failures, .. }) =
        serde_json::from_str(raw)
    {
        return failures
            .into_iter()
            .map(StoredFailure::from_storage)
            .collect();
    }
    // Published category text and unsupported/corrupt optional evidence cannot
    // create an operation or its causes. The admission reader is stricter.
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(StoredFailure::from_storage)
        .collect()
}

pub(crate) fn read(
    conn: &Connection,
    workspace: &str,
    grant: &str,
) -> Result<Option<LoadedCleanupOperation>> {
    let raw: Option<String> = conn.query_row(
        "SELECT cleanup_error FROM external_workdir_grants WHERE workspace_id=?1 AND grant_id=?2",
        params![workspace, grant],
        |row| row.get(0),
    )?;
    if has_intent(conn, workspace, grant)? {
        return Ok(None);
    }
    raw.filter(|raw| !legacy_observation_only(Some(raw)))
        .map(|raw| {
            let operation = parse(&raw)?;
            if operation.request.workspace_id != workspace || operation.request.grant_id != grant {
                return Err(conflict());
            }
            ensure_binding(conn, &operation.request)?;
            Ok(LoadedCleanupOperation { operation, raw })
        })
        .transpose()
}

pub(crate) fn legacy_observation_only(raw: Option<&str>) -> bool {
    raw.is_none_or(|raw| {
        raw == "attachment_release_failed" || serde_json::from_str::<Vec<String>>(raw).is_ok()
    })
}

fn ensure_binding(conn: &Connection, request: &CleanupRequest) -> Result<()> {
    ensure_grant_binding(
        conn,
        &request.workspace_id,
        &request.grant_id,
        &request.provider_instance_id,
        request.grant_generation,
        &request.working_directory_id,
    )?;
    ensure_frozen_target_binding(conn, request)
}

fn ensure_grant_binding(
    conn: &Connection,
    workspace: &str,
    grant: &str,
    provider: &str,
    generation: u64,
    workdir: &str,
) -> Result<()> {
    let current: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM external_workdir_grants
         WHERE workspace_id=?1 AND grant_id=?2 AND provider_instance_id=?3
           AND generation=?4 AND workdir_id=?5 AND status IN ('revoked','expired'))",
        params![
            workspace,
            grant,
            provider,
            i64::try_from(generation).map_err(|_| conflict())?,
            workdir
        ],
        |row| row.get(0),
    )?;
    if !current {
        return Err(conflict());
    }
    Ok(())
}

fn ensure_frozen_target_binding(conn: &Connection, request: &CleanupRequest) -> Result<()> {
    let link = crate::store::latest_workdir_worker_link_on(
        conn,
        &request.workspace_id,
        &request.working_directory_id,
    )?;
    let target_matches = match (&request.target, &link) {
        (None, None) => true,
        (Some(target), Some(link)) => {
            target.worker == link.worker
                && target.connection_id == link.connection_id
                && target.alias == link.alias
        }
        _ => false,
    };
    if !target_matches {
        return Err(conflict());
    }
    Ok(())
}

/// The caller must hold the existing Worker lock through session close and
/// confirmation. Historical links are evidence, never authority to close a
/// session now occupying a reused alias. Confirm repeats this check atomically.
pub(crate) fn validate_release(conn: &Connection, expected: &LoadedCleanupOperation) -> Result<()> {
    if expected.operation.release_confirmed || expected.operation.runtime_dispatched {
        return Err(conflict());
    }
    let request = &expected.operation.request;
    ensure_binding(conn, request)?;
    let current: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM external_workdir_grants
         WHERE workspace_id=?1 AND grant_id=?2 AND cleanup_state='pending' AND cleanup_error=?3)",
        params![request.workspace_id, request.grant_id, expected.raw],
        |row| row.get(0),
    )?;
    let target_matches: bool = if let Some(target) = &request.target {
        conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM worker_workdir_links
             WHERE workspace_id=?1 AND workdir_id=?2 AND runtime_id=?3 AND worker_id=?4
               AND alias=?5 AND connection_id=?6 AND unlinked_at IS NULL)",
            params![
                request.workspace_id,
                request.working_directory_id,
                target.worker.runtime_id,
                target.worker.worker_id,
                target.alias,
                target.connection_id
            ],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM worker_workdir_links
             WHERE workspace_id=?1 AND workdir_id=?2 AND unlinked_at IS NULL)",
            params![request.workspace_id, request.working_directory_id],
            |row| row.get(0),
        )?
    };
    if !current || !target_matches {
        return Err(conflict());
    }
    Ok(())
}

pub(crate) fn begin(
    conn: &mut Connection,
    request: &CleanupRequest,
    failures: &[BackgroundFailure],
    now: &str,
) -> Result<CleanupAdmission> {
    if !valid_request(request) {
        return Err(Error::InvalidInput(
            "Invalid External cleanup request".into(),
        ));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_binding(&tx, request)?;
    let previous_raw: Option<String> = tx.query_row(
        "SELECT cleanup_error FROM external_workdir_grants WHERE workspace_id=?1 AND grant_id=?2",
        params![request.workspace_id, request.grant_id],
        |row| row.get(0),
    )?;
    let previous_raw = previous_raw.ok_or_else(conflict)?;
    let document: CleanupDocument = serde_json::from_str(&previous_raw).map_err(|_| conflict())?;
    let retained_failures = match document {
        CleanupDocument::ExternalWorkdirCleanupV1(_) => {
            let operation = parse(&previous_raw)?;
            // An exact replay observes the saved owner, never admits a second call.
            if operation.request != *request {
                return Err(conflict());
            }
            return Ok(CleanupAdmission::Existing(LoadedCleanupOperation {
                operation,
                raw: previous_raw,
            }));
        }
        CleanupDocument::ExternalWorkdirCleanupIntentV1 {
            workspace_id,
            grant_id,
            provider_instance_id,
            grant_generation,
            working_directory_id,
            failures,
        } => {
            if workspace_id != request.workspace_id
                || grant_id != request.grant_id
                || provider_instance_id != request.provider_instance_id
                || grant_generation != request.grant_generation
                || working_directory_id != request.working_directory_id
            {
                return Err(conflict());
            }
            failures
        }
    };
    let operation = CleanupOperation {
        request: request.clone(),
        release_confirmed: false,
        runtime_dispatched: false,
        runtime_status: None,
        settled: false,
        failures: retained_failures
            .into_iter()
            .chain(
                failures
                    .iter()
                    .map(|failure| failure.as_storage_str().to_owned()),
            )
            .collect(),
    };
    let raw = serde_json::to_string(&CleanupDocument::ExternalWorkdirCleanupV1(
        operation.clone(),
    ))
    .map_err(|_| {
        Error::RegistryInconsistency("External cleanup operation encoding failed".into())
    })?;
    let changed = tx.execute(
        "UPDATE external_workdir_grants SET cleanup_error=?6,cleanup_updated_at=?7
         WHERE workspace_id=?1 AND grant_id=?2 AND provider_instance_id=?3
           AND generation=?4 AND workdir_id=?5 AND status IN ('revoked','expired')
           AND cleanup_state='pending' AND cleanup_error=?8",
        params![
            request.workspace_id,
            request.grant_id,
            request.provider_instance_id,
            request.grant_generation as i64,
            request.working_directory_id,
            raw,
            now,
            previous_raw
        ],
    )?;
    if changed != 1 {
        return Err(conflict());
    }
    tx.commit()?;
    Ok(CleanupAdmission::Admitted(LoadedCleanupOperation {
        operation,
        raw,
    }))
}

fn save_in_transaction(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    operation: CleanupOperation,
    now: &str,
) -> Result<LoadedCleanupOperation> {
    if operation.request != expected.operation.request {
        return Err(conflict());
    }
    ensure_binding(conn, &operation.request)?;
    let raw = serde_json::to_string(&CleanupDocument::ExternalWorkdirCleanupV1(
        operation.clone(),
    ))
    .map_err(|_| {
        Error::RegistryInconsistency("External cleanup operation encoding failed".into())
    })?;
    let request = &operation.request;
    let changed = conn.execute(
        "UPDATE external_workdir_grants SET cleanup_error=?6,cleanup_updated_at=?7,
           cleanup_state=CASE WHEN ?9 THEN 'completed' ELSE cleanup_state END
         WHERE workspace_id=?1 AND grant_id=?2 AND provider_instance_id=?3
           AND generation=?4 AND workdir_id=?5 AND status IN ('revoked','expired')
           AND cleanup_error=?8",
        params![
            request.workspace_id,
            request.grant_id,
            request.provider_instance_id,
            request.grant_generation as i64,
            request.working_directory_id,
            raw,
            now,
            expected.raw,
            operation.settled
        ],
    )?;
    if changed != 1 {
        return Err(conflict());
    }
    Ok(LoadedCleanupOperation { operation, raw })
}

/// Called only after the owner confirmed provider/session close. The exact
/// Backend connection unlink and that confirmed fact commit in one transaction.
pub(crate) fn confirm_release(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    now: &str,
) -> Result<LoadedCleanupOperation> {
    if expected.operation.release_confirmed || expected.operation.runtime_dispatched {
        return Err(conflict());
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    validate_release(&tx, expected)?;
    let request = &expected.operation.request;
    if let Some(target) = &request.target {
        let unlinked = tx.execute(
            "UPDATE worker_workdir_links SET unlinked_at=?7
             WHERE workspace_id=?1 AND workdir_id=?2 AND runtime_id=?3 AND worker_id=?4
               AND alias=?5 AND connection_id=?6 AND unlinked_at IS NULL",
            params![
                request.workspace_id,
                request.working_directory_id,
                target.worker.runtime_id,
                target.worker.worker_id,
                target.alias,
                target.connection_id,
                now
            ],
        )?;
        if unlinked != 1 {
            return Err(conflict());
        }
    }
    let mut operation = expected.operation.clone();
    operation.release_confirmed = true;
    let saved = save_in_transaction(&tx, expected, operation, now)?;
    tx.commit()?;
    Ok(saved)
}

/// A committed dispatch checkpoint is required before the Runtime call. Reentry
/// observes this child; it does not dispatch it a second time.
pub(crate) fn mark_runtime_dispatched(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    now: &str,
) -> Result<LoadedCleanupOperation> {
    if !expected.operation.release_confirmed
        || expected.operation.runtime_dispatched
        || expected.operation.request.target.is_none()
    {
        return Err(conflict());
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let mut operation = expected.operation.clone();
    operation.runtime_dispatched = true;
    let saved = save_in_transaction(&tx, expected, operation, now)?;
    tx.commit()?;
    Ok(saved)
}

/// Only the original receipt plus its still-current Runtime generation can
/// settle this child. A same-set replacement, Worker reincarnation or superseded
/// receipt cannot be attributed to the frozen request. Unknown stays historical.
pub(crate) fn settle(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    observation: Option<&runtime_api::WorkerWorkdirAttachmentsOperationObservation>,
    now: &str,
) -> Result<LoadedCleanupOperation> {
    use runtime_api::WorkerWorkdirAttachmentsOperationOutcome as Outcome;
    let operation = &expected.operation;
    if operation.settled || !operation.release_confirmed {
        return Err(conflict());
    }
    match (&operation.request.target, observation) {
        (None, None) => {}
        (Some(target), Some(observed)) if operation.runtime_dispatched => {
            let Outcome::Receipt { receipt } = &observed.result else {
                return Err(conflict());
            };
            let request = operation.request.runtime_request().ok_or_else(conflict)?;
            if observed.worker_ref.worker_id.to_string() != target.worker.worker_id
                || observed.request != request
                || receipt.worker_ref != observed.worker_ref
                || receipt.request != request
                || receipt.resulting_generation.is_empty()
                || receipt.resulting_generation == target.expected_generation
                || observed.current_generation.as_deref()
                    != Some(receipt.resulting_generation.as_str())
            {
                return Err(conflict());
            }
        }
        _ => return Err(conflict()),
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    ensure_binding(&tx, &operation.request)?;
    if let Some(target) = &operation.request.target {
        let mut current = Vec::new();
        for link in crate::store::list_worker_workdir_links_on(
            &tx,
            &operation.request.workspace_id,
            &target.worker,
        )? {
            let kind: String = tx.query_row(
                "SELECT source_kind FROM workdir_registry WHERE workspace_id=?1 AND workdir_id=?2",
                params![operation.request.workspace_id, link.workdir_id],
                |row| row.get(0),
            )?;
            if kind == "workspace_config" {
                continue;
            }
            current.push(LogicalWorkdirAttachment {
                alias: workdir::WorkdirAttachmentAlias::new(link.alias).map_err(|_| conflict())?,
                working_directory_id: link.workdir_id,
                capabilities: link.capabilities,
            });
        }
        let mut requested = target.logical_workdir_attachments.clone();
        current.sort_by(|a, b| a.alias.as_str().cmp(b.alias.as_str()));
        requested.sort_by(|a, b| a.alias.as_str().cmp(b.alias.as_str()));
        if current != requested {
            return Err(conflict());
        }
    }
    let mut operation = operation.clone();
    operation.settled = true;
    let saved = save_in_transaction(&tx, expected, operation, now)?;
    tx.commit()?;
    Ok(saved)
}

/// Diagnostic append never changes dispatch status or grants continuation.
pub(crate) fn append_failures(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    failures: &[BackgroundFailure],
    now: &str,
) -> Result<LoadedCleanupOperation> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let mut operation = expected.operation.clone();
    operation.failures.extend(
        failures
            .iter()
            .map(|failure| failure.as_storage_str().to_owned()),
    );
    let saved = save_in_transaction(&tx, expected, operation, now)?;
    tx.commit()?;
    Ok(saved)
}

/// Record an immediate typed observation only. This does not mark cleanup
/// completed, prove the original operation fulfilled, or authorize a resend.
pub(crate) fn record_runtime_observation(
    conn: &Connection,
    expected: &LoadedCleanupOperation,
    status: RuntimeOperationStatus,
    failures: &[BackgroundFailure],
    now: &str,
) -> Result<LoadedCleanupOperation> {
    if !expected.operation.release_confirmed
        || !expected.operation.runtime_dispatched
        || expected.operation.runtime_status.is_some()
        || expected.operation.settled
    {
        return Err(conflict());
    }
    let mut operation = expected.operation.clone();
    operation.runtime_status = Some(status);
    operation.failures.extend(
        failures
            .iter()
            .map(|failure| failure.as_storage_str().to_owned()),
    );
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let saved = save_in_transaction(&tx, expected, operation, now)?;
    tx.commit()?;
    Ok(saved)
}
