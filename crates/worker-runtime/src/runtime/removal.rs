use super::*;
use crate::retention::{WorkerRemovalFreeze, WorkerRemovalSessionMember};
use serde::{Deserialize, Serialize};
use session_store::capture::{RecordingCaptureSlice, RecordingPosition};
use session_store::worker_sources::{WorkerSessionOwner, WorkerSessionSources};
use session_store::{SegmentId, SessionId};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRemovalFreeze {
    request: WorkerRetentionExecutionRequest,
    create_fingerprint: Option<String>,
    freeze: WorkerRemovalFreeze,
}

fn rejected(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidRequest(message.into())
}

fn sources(
    root: &Path,
    workspace: &str,
    worker: WorkerId,
) -> Result<WorkerSessionSources, RuntimeError> {
    WorkerSessionSources::new(
        root,
        WorkerSessionOwner {
            workspace_id: workspace.into(),
            worker_id: worker.to_string(),
        },
    )
    .map_err(source_error)
}

fn read_saved(path: &Path) -> Result<Option<SavedRemovalFreeze>, RuntimeError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let saved: SavedRemovalFreeze = serde_json::from_slice(&bytes)
                .map_err(|_| ownership_unknown("Worker removal freeze is corrupt"))?;
            let freeze = &saved.freeze;
            let mut sessions = std::collections::HashSet::new();
            if saved
                .create_fingerprint
                .as_deref()
                .is_some_and(str::is_empty)
                || freeze.operation_id != saved.request.operation_id
                || freeze.input_fingerprint != saved.request.input_fingerprint
                || freeze.workspace_id != saved.request.workspace_id
                || freeze.source_runtime_id != saved.request.source_runtime_id
                || freeze.worker_id != saved.request.worker_id
                || freeze.members.is_empty()
                || freeze.members.iter().any(|member| {
                    !sessions.insert(member.session_id)
                        || member.end.byte_offset != 0
                        || member.end.canonical_order != member.descriptors.len() as u64
                        || member.materialized != member.active_segment_id.is_some()
                        || (!member.materialized && !member.descriptors.is_empty())
                        || member
                            .descriptors
                            .iter()
                            .enumerate()
                            .any(|(index, descriptor)| descriptor.canonical_order != index as u64)
                })
            {
                return Err(ownership_unknown(
                    "Worker removal freeze metadata is inconsistent",
                ));
            }
            Ok(Some(saved))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RuntimeError::StoreIo {
            operation: "read Worker removal freeze",
            path: path.to_path_buf(),
            source,
        }),
    }
}

impl RuntimeState {
    pub(super) fn ensure_worker_removal_not_frozen(
        &self,
        worker: WorkerId,
    ) -> Result<(), RuntimeError> {
        if let Some(store) = self.fs_store() {
            let path = store
                .runtime_dir()
                .join("retention/removal-freezes")
                .join(format!("{worker}.json"));
            match std::fs::metadata(&path) {
                Ok(_) => {
                    return Err(rejected(
                        "Worker removal has permanently frozen original ingress and restore",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(source) => {
                    return Err(RuntimeError::StoreIo {
                        operation: "check Worker removal fence",
                        path,
                        source,
                    });
                }
            }
        }
        Ok(())
    }
}

fn source_unavailable(message: impl Into<String>) -> RuntimeError {
    RuntimeError::RecordingSourceUnavailable {
        message: message.into(),
    }
}

fn ownership_unknown(message: impl Into<String>) -> RuntimeError {
    RuntimeError::RecordingOwnershipUnknown {
        message: message.into(),
    }
}

fn execution_unconfirmed(message: impl Into<String>) -> RuntimeError {
    RuntimeError::RecordingExecutionUnconfirmed {
        message: message.into(),
    }
}

fn source_error(error: session_store::StoreError) -> RuntimeError {
    source_unavailable(error.to_string())
}

// Both Remove and explicit Release enumerate the same producer-owned Session union.
// The caller owns the exclusive publication lock and, for live Release, an actual
// Controller reservation. Remove uses the settled lifecycle; optional termination
// history below reconciles accepted input, not whether execution has stopped.
fn recording_scope_members(
    root: &Path,
    owner: &WorkerSessionSources,
    workspace_id: &str,
    worker_id: WorkerId,
    termination: Option<&crate::execution::WorkerExecutionTermination>,
) -> Result<Vec<WorkerRemovalSessionMember>, RuntimeError> {
    let retention = crate::retention::runtime_session_retention(root);
    let worker_ref = WorkerRef::new(worker_id);
    let mut members = Vec::new();
    let metadata = session_store::WorkerAggregateStore::open_read_only(
        owner.worker_root(),
        crate::worker_backend::ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
            &worker_ref,
        ),
    )
    .and_then(|store| store.read_read_only())
    .map_err(|_| ownership_unknown("Worker Session ownership metadata unavailable"))?
    .ok_or_else(|| ownership_unknown("Worker Session ownership metadata missing"))?;
    if metadata.workspace_id.as_deref() != Some(workspace_id) {
        return Err(ownership_unknown("Worker Session ownership mismatch"));
    }
    let active = metadata
        .active
        .ok_or_else(|| ownership_unknown("Worker Session ownership has no active reference"))?;
    let _guard = retention
        .lock_live(active.session_id)
        .map_err(source_error)?;
    let member = match active.segment_id {
        Some(segment) => {
            let store = session_store::WorkerSessionStore::open_read_only(
                owner.worker_root().join("session"),
            )
            .map_err(source_error)?;
            let (entries, _) = store
                .read_all_read_only_bounded(active.session_id, segment, u64::MAX)
                .map_err(source_error)?;
            ensure_inputs_settled(&entries)?;
            let mut receipts = std::collections::HashSet::new();
            for source_segment in session_store::Store::list_segments(&store, active.session_id)
                .map_err(source_error)?
            {
                let (records, _) = store
                    .read_all_read_only_bounded(active.session_id, source_segment, u64::MAX)
                    .map_err(source_error)?;
                let replay = session_store::segment_log::collect_state(&records);
                for (_, payload) in replay
                    .extensions
                    .iter()
                    .filter(|(domain, _)| domain == "worker.pending_activations.v1")
                {
                    #[derive(Deserialize)]
                    struct Receipt {
                        submission_request_id: String,
                        submission_id: String,
                    }
                    #[derive(Deserialize)]
                    struct ReceiptCheckpoint {
                        receipts: Vec<Receipt>,
                    }
                    let checkpoint: ReceiptCheckpoint = serde_json::from_value(payload.clone())
                        .map_err(|_| {
                            execution_unconfirmed(
                                "Worker accepted-input receipt checkpoint is invalid",
                            )
                        })?;
                    receipts.extend(
                        checkpoint
                            .receipts
                            .into_iter()
                            .map(|receipt| (receipt.submission_request_id, receipt.submission_id)),
                    );
                }
            }
            if termination.is_some_and(|termination| {
                termination.submissions.iter().any(|ack| {
                    !receipts
                        .contains(&(ack.submission_request_id.clone(), ack.submission_id.clone()))
                })
            }) {
                return Err(execution_unconfirmed(
                    "Worker accepted pending input reconciliation is unknown: an owner-observed acknowledgement is absent from retained receipts",
                ));
            }

            let descriptors = session_store::capture::recording_union_catalog_worker_guarded(
                &owner.worker_root().join("session"),
                active.session_id,
                segment,
                Default::default(),
                &_guard,
            )
            .map_err(|error| {
                source_unavailable(format!("Worker recording union unavailable: {error}"))
            })?;
            WorkerRemovalSessionMember {
                session_id: active.session_id,
                active_segment_id: Some(segment),
                materialized: true,
                end: RecordingPosition {
                    canonical_order: descriptors.len() as u64,
                    byte_offset: 0,
                },
                descriptors,
            }
        }
        None if termination.is_none_or(|termination| termination.submissions.is_empty())
            && crate::retention::unmaterialized_session(&owner.worker_root(), workspace_id)?
                == Some(active.session_id) =>
        {
            WorkerRemovalSessionMember {
                session_id: active.session_id,
                active_segment_id: None,
                materialized: false,
                end: Default::default(),
                descriptors: Vec::new(),
            }
        }
        None => {
            return Err(source_unavailable(
                "Worker Session material has no active selector",
            ));
        }
    };
    members.push(member);
    drop(_guard);
    for binding in owner
        .retained_bindings()
        .map_err(|error| ownership_unknown(error.to_string()))?
    {
        if members
            .iter()
            .any(|member| member.session_id == binding.session_id)
        {
            return Err(ownership_unknown(
                "Session maps to multiple producer locations",
            ));
        }
        let guard = retention
            .lock_live(binding.session_id)
            .map_err(source_error)?;
        let entries = session_store::FsStore::read_retained_segment(
            &owner.retained_root(),
            binding.session_id,
            binding.active_segment_id,
            u64::MAX,
        )
        .map_err(source_error)?;
        ensure_inputs_settled(&entries)?;
        let descriptors = session_store::capture::recording_union_catalog_fs_guarded(
            &owner.retained_root(),
            binding.session_id,
            binding.active_segment_id,
            Default::default(),
            &guard,
        )
        .map_err(|error| {
            source_unavailable(format!("Retained recording union unavailable: {error}"))
        })?;
        members.push(WorkerRemovalSessionMember {
            session_id: binding.session_id,
            active_segment_id: Some(binding.active_segment_id),
            materialized: true,
            end: RecordingPosition {
                canonical_order: descriptors.len() as u64,
                byte_offset: 0,
            },
            descriptors,
        });
    }
    members.sort_by_key(|member| member.session_id);
    Ok(members)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedReleaseScope {
    request: runtime_api::SubjectReleaseRequest,
    recording_scope: Option<runtime_api::SubjectReleaseRecordingScope>,
}

fn same_release(
    a: &runtime_api::SubjectReleaseRequest,
    b: &runtime_api::SubjectReleaseRequest,
) -> bool {
    a.workspace_id == b.workspace_id
        && a.worker_id == b.worker_id
        && a.operation_id == b.operation_id
        && a.binding_id == b.binding_id
}

fn read_release_scope(
    owner: &WorkerSessionSources,
) -> Result<Option<SavedReleaseScope>, RuntimeError> {
    match std::fs::read(owner.release_fence_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ownership_unknown("Subject release source fence is corrupt")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(source_unavailable(error.to_string())),
    }
}

impl Runtime {
    pub(super) fn subject_release_with_sources(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: runtime_api::SubjectReleaseRequest,
    ) -> Result<runtime_api::SubjectReleaseResponse, RuntimeError> {
        use runtime_api::SubjectReleaseAction as Action;
        if request.workspace_id != scope.workspace_id
            || request.binding_id.trim().is_empty()
            || request.operation_id.trim().is_empty()
            || request.binding_id.len() > 512
            || request.operation_id.len() > 512
        {
            return Err(RuntimeError::InvalidRequest(
                "invalid Subject release identity".into(),
            ));
        }
        let worker_ref = WorkerRef::new(request.worker_id);
        let lease = self.worker_operation_lock(request.worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        self.ensure_worker_in_workspace(scope, &worker_ref)?;
        let backend = {
            let state = self.lock()?;
            state.ensure_running()?;
            let worker = state.worker(&worker_ref)?;
            if worker.has_pending_lifecycle_operation() {
                return Err(RuntimeError::InvalidRequest(
                    "Subject lifecycle is unresolved".into(),
                ));
            }
            if let Some((saved, _)) = state
                .subject_release_operations
                .get(&(request.worker_id, request.operation_id.clone()))
            {
                if saved.operation_id != request.operation_id
                    || saved.binding_id != request.binding_id
                    || saved.workspace_id != request.workspace_id
                {
                    return Err(RuntimeError::InvalidRequest(
                        "different Subject release is retained".into(),
                    ));
                }
            } else if request.action != Action::Reserve {
                return Err(RuntimeError::InvalidRequest(
                    "Subject reservation is absent or lost; retain occupancy".into(),
                ));
            } else {
                state.ensure_subject_not_reserved(&worker_ref, false)?;
            }
            state.execution_backend.clone().ok_or_else(|| {
                RuntimeError::RecordingExecutionUnconfirmed {
                    message: "actual Controller reservation is unavailable".into(),
                }
            })?
        };
        let key = (request.worker_id, request.operation_id.clone());
        let cancelled = {
            let mut state = self.lock()?;
            let cancelled = state
                .subject_release_operations
                .get(&key)
                .is_some_and(|(_, saved)| *saved == runtime_api::SubjectReleaseState::Cancelled);
            if cancelled && request.action != Action::Cancel {
                return Err(RuntimeError::InvalidRequest(
                    "cancelled release cannot be rearmed; use a new operation".into(),
                ));
            }
            if !state.subject_release_operations.contains_key(&key) {
                // Admission intent survives restart but is not Controller-held proof.
                state.subject_release_operations.insert(
                    key.clone(),
                    (request.clone(), runtime_api::SubjectReleaseState::Reserved),
                );
            }
            state.persist_runtime_snapshot()?;
            cancelled
        };
        // Cancellation must not let a resumed Controller race the publication fence.
        // Reserve acquires this lock only AFTER the Controller ACK, so its final
        // persistence can complete before the producer-exclusive inventory read.
        let mut owner = if request.action == Action::Cancel {
            self.subject_release_source_owner(&request)?
        } else {
            None
        };
        let mut publication = owner
            .as_ref()
            .map(|owner| owner.lock_exclusive())
            .transpose()
            .map_err(|error| RuntimeError::RecordingSourceUnavailable {
                message: error.to_string(),
            })?;
        if cancelled {
            if let Some(owner) = &owner {
                self.cancel_subject_release_publication(owner, &request)?;
            }
            return Ok(runtime_api::SubjectReleaseResponse {
                operation_id: request.operation_id,
                binding_id: request.binding_id,
                state: runtime_api::SubjectReleaseState::Cancelled,
                recording_scope: None,
            });
        }
        let mut response = backend
            .subject_release(&request)
            .map_err(|message| RuntimeError::RecordingExecutionUnconfirmed { message })?;
        if response.binding_id != request.binding_id
            || response.operation_id != request.operation_id
            || !matches!(
                (request.action, response.state),
                (
                    Action::Reserve,
                    runtime_api::SubjectReleaseState::Reserved
                        | runtime_api::SubjectReleaseState::Disconnected
                ) | (
                    Action::Disconnect,
                    runtime_api::SubjectReleaseState::Disconnected
                ) | (Action::Cancel, runtime_api::SubjectReleaseState::Cancelled)
            )
        {
            return Err(RuntimeError::RecordingExecutionUnconfirmed {
                message: "Subject release acknowledgment mismatch".into(),
            });
        }
        response.recording_scope = if request.action != Action::Cancel {
            owner = self.subject_release_source_owner(&request)?;
            let owner = owner
                .as_ref()
                .ok_or_else(|| RuntimeError::RecordingSourceUnavailable {
                    message:
                        "Subject release recording scope requires producer-owned filesystem sources"
                            .into(),
                })?;
            publication = Some(owner.lock_exclusive().map_err(|error| {
                RuntimeError::RecordingSourceUnavailable {
                    message: error.to_string(),
                }
            })?);
            Some(self.subject_release_recording_scope(owner, &request)?)
        } else {
            None
        };
        {
            let mut state = self.lock()?;
            let before = state
                .subject_release_operations
                .insert(key.clone(), (request.clone(), response.state));
            if let Err(error) = state.persist_runtime_snapshot() {
                if let Some(before) = before {
                    state.subject_release_operations.insert(key, before);
                }
                return Err(error);
            }
        }
        if response.state == runtime_api::SubjectReleaseState::Cancelled {
            if let Some(owner) = &owner {
                self.cancel_subject_release_publication(owner, &request)?;
            }
        }
        drop(publication);
        Ok(response)
    }

    fn subject_release_source_owner(
        &self,
        request: &runtime_api::SubjectReleaseRequest,
    ) -> Result<Option<WorkerSessionSources>, RuntimeError> {
        let state = self.lock()?;
        state
            .fs_store()
            .map(|store| {
                sources(
                    store.runtime_dir(),
                    &request.workspace_id,
                    request.worker_id,
                )
            })
            .transpose()
    }

    // Called only after Backend rechecks its actual HeldReservation, including on
    // replay/restart. A saved Reserved row is not evidence of Controller quiescence.
    // The caller holds the producer lock until the response is durably recorded.
    fn subject_release_recording_scope(
        &self,
        owner: &WorkerSessionSources,
        request: &runtime_api::SubjectReleaseRequest,
    ) -> Result<runtime_api::SubjectReleaseRecordingScope, RuntimeError> {
        let (root, runtime_id) = {
            let state = self.lock()?;
            (
                state
                    .fs_store()
                    .ok_or_else(|| {
                        source_unavailable("Subject release requires filesystem sources")
                    })?
                    .runtime_dir()
                    .to_path_buf(),
                state
                    .runtime_identity
                    .clone()
                    .ok_or_else(|| ownership_unknown("Runtime identity is unavailable"))?,
            )
        };
        let path = owner.release_fence_path();
        let mut saved = match read_release_scope(owner)? {
            Some(saved) if same_release(&saved.request, request) => saved,
            Some(_) => {
                return Err(ownership_unknown(
                    "Another Subject release owns source publication",
                ));
            }
            None => {
                let saved = SavedReleaseScope {
                    request: request.clone(),
                    recording_scope: None,
                };
                crate::retention::atomic_write_json(
                    &path,
                    &saved,
                    "fence Subject release sources",
                )?;
                saved
            }
        };
        if let Some(scope) = saved.recording_scope {
            if scope.workspace_id != request.workspace_id
                || scope.worker_id != request.worker_id
                || scope.source_runtime_id != runtime_id
                || scope.members.is_empty()
            {
                return Err(ownership_unknown("Subject release scope identity mismatch"));
            }
            return Ok(scope);
        }
        let scope = runtime_api::SubjectReleaseRecordingScope {
            workspace_id: request.workspace_id.clone(),
            source_runtime_id: runtime_id,
            worker_id: request.worker_id,
            members: recording_scope_members(
                &root,
                owner,
                &request.workspace_id,
                request.worker_id,
                None,
            )?,
        };
        saved.recording_scope = Some(scope.clone());
        crate::retention::atomic_write_json(&path, &saved, "save Subject release scope")?;
        Ok(scope)
    }

    // Only a confirmed exact Cancelled receipt may remove its transient fence.
    // A stale Cancel replay is harmless even if a newer operation now owns it.
    fn cancel_subject_release_publication(
        &self,
        owner: &WorkerSessionSources,
        request: &runtime_api::SubjectReleaseRequest,
    ) -> Result<(), RuntimeError> {
        if read_release_scope(owner)?.is_some_and(|saved| same_release(&saved.request, request)) {
            let path = owner.release_fence_path();
            std::fs::remove_file(&path).map_err(|error| source_unavailable(error.to_string()))?;
            std::fs::File::open(path.parent().expect("fence parent"))
                .and_then(|file| file.sync_all())
                .map_err(|error| source_unavailable(error.to_string()))?;
        }
        Ok(())
    }

    /// Freeze a canonically settled producer under its lifecycle lease. Optional
    /// Stop history preserves accepted-input checks, not a second shutdown gate.
    pub fn freeze_worker_removal_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRemovalFreeze, RuntimeError> {
        crate::retention::validate_request(request)?;
        if request.workspace_id != scope.workspace_id {
            return Err(rejected("Worker removal Workspace mismatch"));
        }
        let lease = self.worker_operation_lock(request.worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let worker_ref = WorkerRef::new(request.worker_id);
        let (root, create_fingerprint, termination, backend) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.ensure_workspace_owner(scope, false)?;
            if state.runtime_identity.as_deref() != Some(request.source_runtime_id.as_str()) {
                return Err(ownership_unknown("Worker removal Runtime mismatch"));
            }
            state.refresh_uncertain_worker_write(&worker_ref)?;
            let worker = state.worker(&worker_ref)?;
            if worker.workspace_id.as_deref() != Some(scope.workspace_id.as_str()) {
                return Err(ownership_unknown("Worker removal owner mismatch"));
            }
            if !worker.is_settled_inactive() || worker.has_pending_lifecycle_operation() {
                return Err(execution_unconfirmed(
                    "Worker removal requires stopped, reconciled execution",
                ));
            }
            let fingerprint = worker
                .request
                .as_ref()
                .map(|request| request.create_fingerprint.clone());
            let root = state
                .fs_store()
                .ok_or_else(|| source_unavailable("Worker removal requires filesystem storage"))?
                .runtime_dir()
                .to_path_buf();
            (
                root,
                fingerprint,
                worker.restore_guard.execution_termination.clone(),
                state.execution_backend.clone(),
            )
        };
        if let Some(backend) = backend {
            backend
                .preflight_worker_removal(&worker_ref)
                .map_err(execution_unconfirmed)?;
        }
        let owner = sources(&root, &scope.workspace_id, request.worker_id)?;
        let _publication = owner.lock_exclusive().map_err(source_error)?;
        let path = owner.freeze_path();
        if let Some(saved) = read_saved(&path)? {
            if saved.request != *request || saved.create_fingerprint != create_fingerprint {
                return Err(ownership_unknown(
                    "Worker removal operation or source identity conflicts with saved freeze",
                ));
            }
            return Ok(saved.freeze);
        }
        // The Worker lifecycle lock and producer lock exclude all ingress while
        // the sources are inspected. Publish exclusion and fixed ends atomically;
        // a queue requiring explicit recovery must not become irreversibly frozen.
        let members = recording_scope_members(
            &root,
            &owner,
            &scope.workspace_id,
            request.worker_id,
            termination.as_ref(),
        )?;
        let freeze = WorkerRemovalFreeze {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            workspace_id: request.workspace_id.clone(),
            source_runtime_id: request.source_runtime_id.clone(),
            worker_id: request.worker_id,
            members,
        };
        let saved = SavedRemovalFreeze {
            request: request.clone(),
            create_fingerprint,
            freeze: freeze.clone(),
        };
        crate::retention::atomic_write_json(&path, &saved, "save Worker removal sources")?;
        Ok(freeze)
    }

    /// Publish and verify every frozen original without removing its producer.
    /// The saved freeze is the replay identity, including after registry removal.
    pub fn publish_worker_removal_archive_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<crate::retention::WorkerSessionArchiveManifest, RuntimeError> {
        crate::retention::validate_request(request)?;
        if request.workspace_id != scope.workspace_id {
            return Err(ownership_unknown("Frozen archive Workspace mismatch"));
        }
        let lease = self.worker_operation_lock(request.worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let (root, fingerprint) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.ensure_workspace_owner(scope, false)?;
            if state.runtime_identity.as_deref() != Some(request.source_runtime_id.as_str()) {
                return Err(ownership_unknown("Frozen archive Runtime mismatch"));
            }
            let fingerprint = if state.workers.contains_key(&request.worker_id) {
                let worker_ref = WorkerRef::new(request.worker_id);
                state.refresh_uncertain_worker_write(&worker_ref)?;
                let worker = state.worker(&worker_ref)?;
                if worker.workspace_id.as_deref() != Some(scope.workspace_id.as_str())
                    || !worker.is_settled_inactive()
                    || worker.has_pending_lifecycle_operation()
                {
                    return Err(execution_unconfirmed(
                        "Frozen archive requires a settled producer",
                    ));
                }
                Some(
                    worker
                        .request
                        .as_ref()
                        .map(|request| request.create_fingerprint.clone()),
                )
            } else {
                None
            };
            let root = state
                .fs_store()
                .ok_or_else(|| source_unavailable("Frozen archive storage unavailable"))?
                .runtime_dir()
                .to_path_buf();
            (root, fingerprint)
        };
        let owner = sources(&root, &scope.workspace_id, request.worker_id)?;
        let _publication = owner.lock_exclusive().map_err(source_error)?;
        let saved = read_saved(&owner.freeze_path())?.ok_or_else(|| {
            execution_unconfirmed("Frozen archive requires a saved removal freeze")
        })?;
        if saved.request != *request
            || fingerprint
                .as_ref()
                .is_some_and(|value| value != &saved.create_fingerprint)
        {
            return Err(ownership_unknown(
                "Frozen archive operation or producer identity mismatch",
            ));
        }
        FsWorkerRetentionProvider::new(&root).publish_frozen_archive(request, &saved.freeze)
    }

    pub(super) fn additional_session_binding_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        source: &runtime_api::SessionPublicSource,
        session: SessionId,
    ) -> Result<Option<(std::path::PathBuf, SegmentId)>, RuntimeError> {
        let (root, runtime_id) = {
            let mut state = self.lock()?;
            state.ensure_workspace_owner(scope, false)?;
            (
                state
                    .fs_store()
                    .ok_or_else(|| rejected("Retained Session storage unavailable"))?
                    .runtime_dir()
                    .to_path_buf(),
                state.runtime_identity.clone(),
            )
        };
        match source {
            runtime_api::SessionPublicSource::Retained { worker_id } => {
                self.ensure_worker_in_workspace(scope, &WorkerRef::new(*worker_id))?;
                let owner = sources(&root, &scope.workspace_id, *worker_id)?;
                Ok(owner
                    .retained_bindings()
                    .map_err(source_error)?
                    .into_iter()
                    .find(|binding| binding.session_id == session)
                    .map(|binding| (owner.retained_root(), binding.active_segment_id)))
            }
            runtime_api::SessionPublicSource::Archived { archive_id } => {
                let (manifest, root) =
                    FsWorkerRetentionProvider::new(&root).session_archive_identity(archive_id)?;
                if manifest.workspace_id != scope.workspace_id
                    || runtime_id.as_deref() != Some(manifest.source_runtime_id.as_str())
                {
                    return Err(rejected("Archive scope mismatch"));
                }
                Ok(manifest
                    .retained_sessions
                    .iter()
                    .find(|binding| binding.session_id == session)
                    .map(|binding| (root.join("retained-sessions"), binding.active_segment_id)))
            }
        }
    }

    /// Preserve the existing explicit-erasure contract for producer-owned
    /// additional Sessions. The ownership registry survives source erasure.
    pub(super) fn delete_additional_session_source_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        source: &runtime_api::SessionPublicSource,
        expected_session_id: &str,
        intent: session_store::retention::SessionDeletionIntent,
    ) -> Result<bool, RuntimeError> {
        let session: SessionId = expected_session_id
            .parse()
            .map_err(|_| rejected("Invalid Session identity"))?;
        let (root, runtime_id) = {
            let mut state = self.lock()?;
            state.ensure_workspace_owner(scope, false)?;
            (
                state
                    .fs_store()
                    .ok_or_else(|| rejected("Session erasure requires storage"))?
                    .runtime_dir()
                    .to_path_buf(),
                state.runtime_identity.clone(),
            )
        };
        let worker = match source {
            runtime_api::SessionPublicSource::Retained { worker_id } => *worker_id,
            runtime_api::SessionPublicSource::Archived { archive_id } => {
                let (manifest, _) =
                    FsWorkerRetentionProvider::new(&root).session_archive_identity(archive_id)?;
                if manifest.workspace_id != scope.workspace_id
                    || runtime_id.as_deref() != Some(manifest.source_runtime_id.as_str())
                {
                    return Err(rejected("Archived Session scope mismatch"));
                }
                if !manifest
                    .retained_sessions
                    .iter()
                    .any(|binding| binding.session_id == session)
                {
                    return Ok(false);
                }
                manifest.source_worker_id
            }
        };
        let lease = self.worker_operation_lock(worker)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        {
            let state = self.lock()?;
            if let Some(current) = state.workers.get(&worker) {
                if current.workspace_id.as_deref() != Some(scope.workspace_id.as_str())
                    || !current.is_settled_inactive()
                    || current.has_pending_lifecycle_operation()
                {
                    return Err(rejected(
                        "Session deletion requires a stopped, settled source Worker",
                    ));
                }
            }
        }
        let owner = sources(&root, &scope.workspace_id, worker)?;
        let _publication = owner.lock_exclusive().map_err(source_error)?;
        let original = owner.retained_root().join(session.to_string());
        let selected = match source {
            runtime_api::SessionPublicSource::Retained { .. } => original.clone(),
            runtime_api::SessionPublicSource::Archived { archive_id } => {
                FsWorkerRetentionProvider::new(&root)
                    .session_archive_identity(archive_id)?
                    .1
                    .join("retained-sessions")
                    .join(session.to_string())
            }
        };
        if matches!(source, runtime_api::SessionPublicSource::Retained { .. })
            && !owner
                .retained_bindings()
                .map_err(source_error)?
                .iter()
                .any(|binding| binding.session_id == session)
        {
            return Ok(false);
        }
        let retention = crate::retention::runtime_session_retention(&root);
        retention
            .delete(session, intent, || {
                if intent == session_store::retention::SessionDeletionIntent::ErasePermanently {
                    crate::retention::remove_archived_session_copies(&root, session)?;
                }
                if intent == session_store::retention::SessionDeletionIntent::ErasePermanently
                    && original != selected
                {
                    match std::fs::remove_dir_all(&original) {
                        Ok(()) => (),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error.into()),
                    }
                }
                match std::fs::remove_dir_all(&selected) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                }
            })
            .map_err(source_error)?;
        Ok(true)
    }

    /// Read only the live-source union fixed by the exact saved release. No
    /// Controller restore or ordinary active-lineage/archive fallback is allowed.
    /// Reserved reads validate preservation; Backend still owns release completion
    /// and must not dispatch recording work before that completion is durable.
    pub fn capture_subject_release_session_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_id: WorkerId,
        request: &runtime_api::SubjectReleaseSessionCaptureRequest,
    ) -> Result<RecordingCaptureSlice, RuntimeError> {
        if request.workspace_id != scope.workspace_id
            || request.operation_id.trim().is_empty()
            || request.binding_id.trim().is_empty()
            || request.operation_id.len() > 512
            || request.binding_id.len() > 512
        {
            return Err(rejected("Invalid Subject release capture identity"));
        }
        if !(1..=4096).contains(&request.max_entries)
            || !(1024..=4 * 1024 * 1024).contains(&request.max_serialized_bytes)
        {
            return Err(rejected(
                "Subject release capture exceeds recording transport limits",
            ));
        }
        let lease = self.worker_operation_lock(worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let (root, runtime_id) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.ensure_workspace_owner(scope, false)?;
            let worker = state.worker(&WorkerRef::new(worker_id))?;
            if worker.workspace_id.as_deref() != Some(scope.workspace_id.as_str()) {
                return Err(ownership_unknown("Subject release capture owner mismatch"));
            }
            let (release, status) = state
                .subject_release_operations
                .get(&(worker_id, request.operation_id.clone()))
                .ok_or_else(|| ownership_unknown("Subject release operation missing"))?;
            if release.workspace_id != request.workspace_id
                || release.worker_id != worker_id
                || release.operation_id != request.operation_id
                || release.binding_id != request.binding_id
                || !matches!(
                    status,
                    runtime_api::SubjectReleaseState::Reserved
                        | runtime_api::SubjectReleaseState::Disconnected
                )
            {
                return Err(ownership_unknown(
                    "Subject release capture operation mismatch",
                ));
            }
            (
                state
                    .fs_store()
                    .ok_or_else(|| source_unavailable("Subject release storage unavailable"))?
                    .runtime_dir()
                    .to_path_buf(),
                state.runtime_identity.clone(),
            )
        };
        let owner = sources(&root, &scope.workspace_id, worker_id)?;
        let _publication = owner.lock_exclusive().map_err(source_error)?;
        let saved = read_release_scope(&owner)?
            .ok_or_else(|| ownership_unknown("Subject release source fence missing"))?;
        if saved.request.workspace_id != request.workspace_id
            || saved.request.worker_id != worker_id
            || saved.request.operation_id != request.operation_id
            || saved.request.binding_id != request.binding_id
            || saved.request.action == runtime_api::SubjectReleaseAction::Cancel
        {
            return Err(ownership_unknown("Subject release saved identity mismatch"));
        }
        let fixed = saved
            .recording_scope
            .ok_or_else(|| source_unavailable("Subject release recording scope missing"))?;
        let mut sessions = std::collections::HashSet::new();
        if fixed.workspace_id != request.workspace_id
            || fixed.worker_id != worker_id
            || runtime_id.as_deref() != Some(fixed.source_runtime_id.as_str())
            || fixed.members.is_empty()
            || fixed.members.iter().any(|member| {
                !sessions.insert(member.session_id)
                    || member.end.byte_offset != 0
                    || member.end.canonical_order != member.descriptors.len() as u64
                    || member.materialized != member.active_segment_id.is_some()
                    || (!member.materialized && !member.descriptors.is_empty())
                    || member
                        .descriptors
                        .iter()
                        .enumerate()
                        .any(|(index, descriptor)| descriptor.canonical_order != index as u64)
            })
        {
            return Err(ownership_unknown(
                "Subject release scope metadata is inconsistent",
            ));
        }
        let member = fixed
            .members
            .iter()
            .find(|member| {
                member.session_id == request.session_id
                    && member.active_segment_id == request.active_segment_id
            })
            .ok_or_else(|| ownership_unknown("Subject release Session is not a fixed member"))?;
        let active = member
            .active_segment_id
            .ok_or_else(|| rejected("Subject release Session is not materialized"))?;
        let retained = owner
            .retained_bindings()
            .map_err(|error| ownership_unknown(error.to_string()))?;
        let retention = crate::retention::runtime_session_retention(&root);
        let guard = retention
            .lock_live(member.session_id)
            .map_err(source_error)?;
        let result = if let Some(binding) = retained
            .iter()
            .find(|binding| binding.session_id == member.session_id)
        {
            if binding.active_segment_id != active {
                return Err(ownership_unknown("Retained Session selector changed"));
            }
            session_store::capture::capture_recording_union_fs_guarded(
                &owner.retained_root(),
                member.session_id,
                active,
                Default::default(),
                &member.descriptors,
                request.start,
                request.max_entries,
                request.max_serialized_bytes,
                &guard,
            )
        } else {
            // This is an explicitly verified primary source, not a fallback to
            // whichever Session happens to be active now.
            let metadata = session_store::WorkerAggregateStore::open_read_only(
                owner.worker_root(),
                crate::worker_backend::ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
                    &WorkerRef::new(worker_id),
                ),
            )
            .and_then(|store| store.read_read_only())
            .map_err(|_| ownership_unknown("Subject release primary ownership unavailable"))?
            .ok_or_else(|| ownership_unknown("Subject release primary ownership missing"))?;
            if metadata.workspace_id.as_deref() != Some(scope.workspace_id.as_str())
                || !metadata.active.is_some_and(|selected| {
                    selected.session_id == member.session_id && selected.segment_id == Some(active)
                })
            {
                return Err(ownership_unknown(
                    "Subject release primary selector changed",
                ));
            }
            session_store::capture::capture_recording_union_worker_guarded(
                &owner.worker_root().join("session"),
                member.session_id,
                active,
                Default::default(),
                &member.descriptors,
                request.start,
                request.max_entries,
                request.max_serialized_bytes,
                &guard,
            )
        };
        result.map_err(|error| {
            source_unavailable(format!(
                "Fixed Subject release capture unavailable: {error}"
            ))
        })
    }

    /// Capture from the immutable per-Session union fixed by this exact removal.
    pub fn capture_worker_removal_session_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        operation_id: &str,
        worker_id: WorkerId,
        session_id: SessionId,
        active_segment_id: Option<SegmentId>,
        start: RecordingPosition,
        max_entries: usize,
        max_serialized_bytes: usize,
    ) -> Result<RecordingCaptureSlice, RuntimeError> {
        if !(1..=4096).contains(&max_entries)
            || !(1024..=4 * 1024 * 1024).contains(&max_serialized_bytes)
        {
            return Err(rejected(
                "Worker removal capture exceeds recording transport limits",
            ));
        }
        let lease = self.worker_operation_lock(worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let (root, runtime_id, fingerprint) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.ensure_workspace_owner(scope, false)?;
            let fingerprint = match state.workers.get(&worker_id) {
                Some(worker) => {
                    if worker.workspace_id.as_deref() != Some(scope.workspace_id.as_str()) {
                        return Err(ownership_unknown("Worker removal capture owner mismatch"));
                    }
                    Some(
                        worker
                            .request
                            .as_ref()
                            .map(|request| request.create_fingerprint.clone()),
                    )
                }
                None => None,
            };
            (
                state
                    .fs_store()
                    .ok_or_else(|| source_unavailable("Worker removal storage unavailable"))?
                    .runtime_dir()
                    .to_path_buf(),
                state.runtime_identity.clone(),
                fingerprint,
            )
        };
        let owner = sources(&root, &scope.workspace_id, worker_id)?;
        let _publication = owner.lock_exclusive().map_err(source_error)?;
        let saved = read_saved(&owner.freeze_path())?
            .ok_or_else(|| execution_unconfirmed("Worker removal freeze missing"))?;
        if saved.request.operation_id != operation_id
            || saved.request.workspace_id != scope.workspace_id
            || runtime_id.as_deref() != Some(saved.request.source_runtime_id.as_str())
            || fingerprint
                .as_ref()
                .is_some_and(|value| value != &saved.create_fingerprint)
        {
            return Err(ownership_unknown(
                "Worker removal capture identity mismatch",
            ));
        }
        let freeze = saved.freeze;
        let member = freeze
            .members
            .iter()
            .find(|member| {
                member.session_id == session_id && member.active_segment_id == active_segment_id
            })
            .ok_or_else(|| ownership_unknown("Worker removal Session is not a fixed member"))?;
        let active = active_segment_id
            .ok_or_else(|| rejected("Worker removal Session is not materialized"))?;
        let (primary_root, retained_root, retained) = if fingerprint.is_some() {
            (
                owner.worker_root().join("session"),
                owner.retained_root(),
                owner
                    .retained_bindings()
                    .map_err(|error| ownership_unknown(error.to_string()))?,
            )
        } else {
            let archive_id = saved
                .request
                .archive_id
                .as_deref()
                .ok_or_else(|| source_unavailable("Removed Worker has no retained archive"))?;
            // A different Session may have been explicitly erased since publication.
            // Validate this member's frozen descriptors under its own erasure lock,
            // rather than requiring the entire archive's original checksum here.
            let (manifest, archive_root) =
                FsWorkerRetentionProvider::new(&root).session_archive_identity(archive_id)?;
            if manifest.workspace_id != scope.workspace_id
                || manifest.source_runtime_id != saved.request.source_runtime_id
                || manifest.source_worker_id != worker_id
                || manifest.operation_id != operation_id
                || manifest.input_fingerprint != saved.request.input_fingerprint
                || (manifest.source_session_id == session_id.to_string()
                    && manifest.source_active_segment_id != Some(active))
            {
                return Err(ownership_unknown(
                    "Removed Worker archive identity mismatch",
                ));
            }
            (
                archive_root.clone(),
                archive_root.join("retained-sessions"),
                manifest.retained_sessions,
            )
        };
        let retention = crate::retention::runtime_session_retention(&root);
        let guard = retention.lock_live(session_id).map_err(source_error)?;
        let result = if let Some(binding) = retained
            .iter()
            .find(|binding| binding.session_id == session_id)
        {
            if binding.active_segment_id != active {
                return Err(ownership_unknown("Retained Session selector changed"));
            }
            session_store::capture::capture_recording_union_fs_guarded(
                &retained_root,
                session_id,
                active,
                Default::default(),
                &member.descriptors,
                start,
                max_entries,
                max_serialized_bytes,
                &guard,
            )
        } else {
            session_store::capture::capture_recording_union_worker_guarded(
                &primary_root,
                session_id,
                active,
                Default::default(),
                &member.descriptors,
                start,
                max_entries,
                max_serialized_bytes,
                &guard,
            )
        };
        result.map_err(|error| {
            source_unavailable(format!(
                "Fixed Worker recording capture unavailable: {error}"
            ))
        })
    }
}

// The Runtime adapter inspects the existing Worker-owned queue checkpoint, not
// a public snapshot (which deliberately omits queues on retained reads). It never
// reconstructs inputs from previews or treats an unknown checkpoint as empty.
fn ensure_inputs_settled(entries: &[session_store::LogEntry]) -> Result<(), RuntimeError> {
    #[derive(Deserialize)]
    struct Queue {
        activating: Option<serde_json::Value>,
        #[serde(default)]
        activating_notification: serde_json::Value,
        pending: Vec<serde_json::Value>,
        pending_notifications: Vec<serde_json::Value>,
        #[serde(default)]
        invocation_recovery_blocked: bool,
    }
    let state = session_store::segment_log::collect_state(entries);
    if let Some((_, payload)) = state
        .extensions
        .iter()
        .rev()
        .find(|(domain, _)| domain == "worker.pending_activations.v1")
    {
        let queue: Queue = serde_json::from_value(payload.clone())
            .map_err(|_| execution_unconfirmed("Worker accepted-input checkpoint is invalid"))?;
        let notifications_settled = queue.activating_notification.is_null()
            || queue
                .activating_notification
                .as_array()
                .is_some_and(|items| items.is_empty());
        if queue.activating.is_some()
            || !notifications_settled
            || !queue.pending.is_empty()
            || !queue.pending_notifications.is_empty()
            || queue.invocation_recovery_blocked
        {
            return Err(execution_unconfirmed(
                "Worker recording scope holds accepted pending input or unresolved invocation recovery",
            ));
        }
    }
    Ok(())
}

pub(super) fn check_retention_freeze(
    root: &Path,
    request: &WorkerRetentionExecutionRequest,
    create_fingerprint: Option<&str>,
) -> Result<(), RuntimeError> {
    let owner = sources(root, &request.workspace_id, request.worker_id)?;
    if let Some(saved) = read_saved(&owner.freeze_path())? {
        if create_fingerprint
            .is_some_and(|fingerprint| Some(fingerprint) != saved.create_fingerprint.as_deref())
        {
            return Err(ownership_unknown("Worker retention creation owner changed"));
        }
        if saved.request != *request {
            // Explicit permanent erasure is a different intent, not a retry of
            // ordinary Remove. It may supersede an archive request only when
            // every frozen original is erased and no old receipt exists. Keep
            // the old freeze unchanged; never reparent an issued receipt.
            let exact_body = saved.request.workspace_id == request.workspace_id
                && saved.request.source_runtime_id == request.source_runtime_id
                && saved.request.worker_id == request.worker_id
                && saved.request.source_created_at == request.source_created_at;
            let erased_scope = !saved.freeze.members.is_empty()
                && saved.freeze.members.iter().all(|member| {
                    matches!(
                        super::super::retention::runtime_session_retention(root)
                            .ensure_live(member.session_id),
                        Err(session_store::StoreError::SessionErased(_))
                    )
                });
            if exact_body
                && request.session_disposition == crate::retention::SessionDisposition::Purge
                && request.diagnostics_disposition
                    == crate::retention::DiagnosticsDisposition::Purge
                && request.archive_id.is_none()
                && erased_scope
                && crate::retention::FsWorkerRetentionProvider::new(root)
                    .has_no_retention_receipt(&saved.request)?
            {
                return Ok(());
            }
            return Err(ownership_unknown(
                "Worker retention conflicts with the original removal freeze",
            ));
        }
    }
    Ok(())
}
