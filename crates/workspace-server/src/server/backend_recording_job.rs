//! Finite recording input is executed by its owning Job. Bounded Worker Jobs
//! retain their immutable prefix/attempt contracts; completion never scans a Journal
//! or a removed producer to invent new work.
use super::*;
use crate::store::{
    BackendJobRecordingInput, BackendJobRecordingOutcome, BackendJobRecordingScope,
};
use session_store::capture::{CaptureBody, CaptureDto, RecordingPosition};
use subjektiv::journal::{JournalHostContext, recording::JournalInput};

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}
fn position(value: RecordingPosition) -> (u64, u64) {
    (value.canonical_order, value.byte_offset)
}
fn end(capture: &CaptureDto) -> Result<RecordingPosition> {
    let entry = capture
        .entries
        .last()
        .ok_or_else(|| Error::RepositoryConflict("Recording capture made no progress".into()))?;
    let CaptureBody::CanonicalPublic {
        descriptor,
        byte_offset,
        text,
        ..
    } = &entry.body
    else {
        return Err(Error::InvalidInput(
            "Recording input is not canonical original material".into(),
        ));
    };
    let offset = byte_offset
        .checked_add(text.len() as u64)
        .ok_or_else(|| Error::InvalidInput("Recording source length overflow".into()))?;
    Ok(if offset == descriptor.total_bytes {
        RecordingPosition {
            canonical_order: entry
                .order
                .checked_add(1)
                .ok_or_else(|| Error::InvalidInput("Recording source order overflow".into()))?,
            byte_offset: 0,
        }
    } else {
        RecordingPosition {
            canonical_order: entry.order,
            byte_offset: offset,
        }
    })
}

/// Capture a finite view before accepting work. Every saved prefix is body-bearing;
/// an admission error never releases originals or claims an accepted Job.
pub(super) fn capture_scope(
    mut start: RecordingPosition,
    fixed_end: Option<RecordingPosition>,
    mut capture: impl FnMut(u64, u64, usize) -> Result<CaptureDto>,
) -> Result<(RecordingPosition, Vec<CaptureDto>)> {
    let mut chunks = Vec::new();
    let mut bytes = 0usize;
    let mut stop = fixed_end;
    let mut segment = None;
    loop {
        if stop.is_some_and(|stop| position(start) >= position(stop)) {
            break;
        }
        let mut value = capture(
            start.canonical_order,
            start.byte_offset,
            JournalInput::capture_budget(),
        )?;
        let stop = *stop.get_or_insert(RecordingPosition {
            canonical_order: value.view_entry_count,
            byte_offset: 0,
        });
        if position(start) == position(stop) {
            break;
        }
        if stop.byte_offset != 0 {
            return Err(Error::InvalidInput(
                "Recording snapshots must end at a committed source boundary".into(),
            ));
        }
        value
            .entries
            .retain(|entry| entry.order < stop.canonical_order);
        value.view_entry_count = stop.canonical_order;
        if let Some(segment) = &segment {
            if &value.segment_id != segment {
                return Err(Error::RepositoryConflict(
                    "Source snapshot changed during recording admission".into(),
                ));
            }
        } else {
            segment = Some(value.segment_id.clone());
        }
        let next = end(&value)?;
        if position(next) <= position(start) || position(next) > position(stop) {
            return Err(Error::RepositoryConflict(
                "Recording capture escaped its finite scope".into(),
            ));
        }
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&value)
                    .map_err(|e| Error::Store(e.to_string()))?
                    .len(),
            )
            .ok_or_else(|| Error::InvalidInput("Recording input size overflow".into()))?;
        if bytes > crate::store::MAX_RECORDING_INPUT_BYTES
            || chunks.len() >= crate::store::MAX_RECORDING_CHUNKS
        {
            return Err(Error::InvalidInput(
                "Finite recording input exceeds the admission limit; originals remain retained"
                    .into(),
            ));
        }
        chunks.push(value);
        start = next;
    }
    Ok((stop.unwrap_or(start), chunks))
}

pub(super) fn admit(
    api: &WorkspaceApi,
    scope: BackendJobRecordingScope,
    chunks: &[CaptureDto],
) -> Result<String> {
    let input = scope.request_input(chunks)?;
    let digest = scope.recording_scope_digest(chunks)?;
    let id = format!("recording-scope:{digest}");
    let request = BackendJobRequest {
        job_id: id.clone(),
        purpose: "journal_recording".into(),
        input_ref: format!(
            "subjektiv://{}/session/{}/recording",
            scope.subject_id, scope.session_id
        ),
        input,
        instruction: "Host-owned finite recording input; not a Worker submission".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    api.config_store.reserve_backend_job_recording_input(
        api.workspace_id(),
        &request,
        &scope,
        chunks,
        &now(),
    )?;
    own(api, &id);
    Ok(id)
}

pub(super) fn recover(api: &WorkspaceApi) -> Result<()> {
    let mut after = None;
    loop {
        let ids = api.config_store.list_active_backend_job_recording_ids(
            api.workspace_id(),
            after.as_deref(),
            100,
        )?;
        for id in &ids {
            own(api, id);
        }
        if ids.len() < 100 {
            break;
        }
        after = ids.last().cloned();
    }
    Ok(())
}

fn own(api: &WorkspaceApi, id: &str) {
    let key = format!("recording-host:{id}");
    {
        let mut owners = api
            .backend_job_recovery
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !owners.insert(key.clone()) {
            return;
        }
    }
    let api = api.clone();
    let id = id.to_owned();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        api.backend_job_recovery
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        return;
    };
    runtime.spawn(async move {
        let mut failures=0usize;
        loop {
            if api.backend_job_recovery.stopping.load(Ordering::Acquire) { break; }
            let worker=api.clone();let job=id.clone();
            let result=match api.host_executors.host() {
                Ok(executor)=>executor.run(move || step(&worker,&job)).await,
                Err(error)=>Err(error),
            };
            match result {
                Ok(true)=>break,
                Ok(false)=>failures=0,
                Err(error)=>{
                    failures=failures.saturating_add(1);
                    tracing::warn!(job_id=%id,%error,"Recording Job retains its immutable input and unfinished effects");
                    // Storage faults are retried by the same owner. Other failures
                    // remain diagnosed without minting a replacement or dropping bytes.
                }
            }
            let delay=backend_job_recovery::SETTLEMENT_BACKOFF_MS[failures.min(4)];
            api.backend_job_recovery.pause(std::time::Duration::from_millis(delay)).await;
        }
        api.backend_job_recovery.running.lock().unwrap_or_else(|p|p.into_inner()).remove(&key);
        api.backend_job_recovery.changed.notify_waiters();
    });
}

fn domain_store(
    api: &WorkspaceApi,
    input: &BackendJobRecordingInput,
) -> Result<crate::subjektiv::SubjektivStore> {
    let db = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &input.scope.database)
        .map_err(Error::SubjektivStorage)?;
    crate::subjektiv::SubjektivStore::from_database(db).map_err(subjektiv_store_error)
}

fn target_erased(api: &WorkspaceApi, input: &BackendJobRecordingInput) -> Result<bool> {
    if subjektiv_deletion::pending(api, &input.scope.subject_id)? {
        return Ok(true);
    }
    let store = domain_store(api, input)?;
    if store
        .subject_deletion(&input.scope.subject_id)
        .map_err(subjektiv_store_error)?
        .is_some()
    {
        return Ok(true);
    }
    let attr = store
        .session_attribution(&input.scope.session_id)
        .map_err(subjektiv_store_error)?
        .filter(|a| {
            a.subject_id == input.scope.subject_id
                && a.runtime_id == input.scope.worker.runtime_id
                && a.worker_id == input.scope.worker.worker_id
        })
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Recording Job source attribution changed".into())
        })?;
    let host =
        JournalHostContext::validated_session(&store, attr).map_err(subjektiv_journal::domain)?;
    if host.writer_binding_id() != Some(input.scope.binding_id.as_str()) {
        return Err(Error::WorkspacePermissionDenied(
            "Recording Job source lifetime changed".into(),
        ));
    }
    store
        .journal_session_erased(&host)
        .map_err(subjektiv_journal::domain)
}

fn captured_prefix(
    api: &WorkspaceApi,
    input: &BackendJobRecordingInput,
    order: u64,
    offset: u64,
    _budget: usize,
) -> Result<CaptureDto> {
    let mut index = 0;
    loop {
        let page = api.config_store.get_backend_job_recording_chunks(
            api.workspace_id(),
            &input.job_id,
            index,
            1,
        )?;
        for chunk in page.chunks {
            let mut capture = chunk.capture;
            capture.entries.retain(|entry| {
                if entry.order < order {
                    return false;
                }
                if entry.order > order {
                    return true;
                }
                match &entry.body {
                    CaptureBody::CanonicalPublic {
                        byte_offset, text, ..
                    } => byte_offset.saturating_add(text.len() as u64) > offset,
                    _ => false,
                }
            });
            if let Some(first) = capture.entries.first_mut() {
                if first.order == order {
                    if let CaptureBody::CanonicalPublic {
                        byte_offset, text, ..
                    } = &mut first.body
                    {
                        if offset < *byte_offset {
                            return Err(Error::RepositoryConflict(
                                "Saved recording input has a cursor gap".into(),
                            ));
                        }
                        let skip = usize::try_from(offset - *byte_offset)
                            .map_err(|_| Error::InvalidInput("Recording offset overflow".into()))?;
                        if !text.is_char_boundary(skip) {
                            return Err(Error::RepositoryConflict(
                                "Recording cursor is not a source scalar boundary".into(),
                            ));
                        }
                        text.drain(..skip);
                        *byte_offset = offset;
                    }
                }
                return Ok(capture);
            }
        }
        let Some(next) = page.next_index else {
            return Err(Error::RepositoryConflict(
                "Saved recording input does not cover the unprocessed cursor".into(),
            ));
        };
        index = next;
    }
}

// Legacy executions may predate finite-input adoption. A checkpoint or Failed
// state cannot let their importer publish completion ahead of common cleanup.
fn session_executions_released(api: &WorkspaceApi, session: &str) -> Result<bool> {
    let mut after = None;
    loop {
        let jobs = api.store.list_backend_journal_jobs(
            api.workspace_id(),
            session,
            after.as_deref(),
            100,
        )?;
        for job in &jobs {
            if matches!(
                job.state,
                BackendJobState::Pending | BackendJobState::Finalizing
            ) {
                return Ok(false);
            }
            let attempts = api
                .store
                .list_backend_job_attempts(api.workspace_id(), &job.request.job_id)?;
            if attempts.is_empty() {
                return Err(Error::Store(
                    "Journal execution is missing its attempt history".into(),
                ));
            }
            if attempts.iter().any(|a| {
                !a.state.terminal()
                    || a.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed)
            }) {
                return Ok(false);
            }
        }
        if jobs.len() < 100 {
            return Ok(true);
        }
        after = jobs.last().map(|job| job.request.job_id.clone());
    }
}

pub(super) fn step(api: &WorkspaceApi, id: &str) -> Result<bool> {
    let Some(input) = api
        .config_store
        .get_backend_job_recording_input(api.workspace_id(), id)?
    else {
        return if subjektiv_deletion::job_erasure_confirmed(api, id)? {
            Ok(true)
        } else {
            Err(Error::Store(
                "Recording Job input is missing without retirement evidence".into(),
            ))
        };
    };
    let gate = subjektiv_journal::lifecycle_lock(api, &input.scope.subject_id)?;
    let _guard = gate
        .lock()
        .map_err(|_| Error::Store("Recording Job lifecycle lock poisoned".into()))?;
    let job = api
        .store
        .get_backend_job(api.workspace_id(), id)?
        .ok_or_else(|| Error::Store("Recording Host Job is missing".into()))?;
    if matches!(
        job.state,
        BackendJobState::Completed | BackendJobState::Failed
    ) {
        return Ok(true);
    }
    if let Some(child) = &input.current_child_job_id {
        let child = api
            .store
            .get_backend_job(api.workspace_id(), child)?
            .ok_or_else(|| {
                Error::Store("Recording child is missing without retirement evidence".into())
            })?;
        if matches!(
            child.state,
            BackendJobState::Pending | BackendJobState::Finalizing
        ) {
            return Ok(false);
        }
        let attempts = api
            .store
            .list_backend_job_attempts(api.workspace_id(), &child.request.job_id)?;
        if attempts.iter().any(|a| {
            !a.state.terminal()
                || a.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed)
        }) {
            return Ok(false);
        }
        if child.state == BackendJobState::Failed && !target_erased(api, &input)? {
            if subjektiv_journal::can_resume_partial(&child) {
                subject_authorization::authorize_job_input(api, &child.request)
                    .map_err(|e| e.error)?;
                let reservation = api.store.reserve_backend_job_retry(
                    api.workspace_id(),
                    &child.request.job_id,
                    &now(),
                )?;
                api.own_backend_job_recovery(&reservation.attempt);
                return Ok(false);
            }
            api.config_store.finish_backend_job_recording(api.workspace_id(),id,&BackendJobRecordingOutcome::Failed {
                category:child.failure_category.unwrap_or_else(||"recording_child_failed".into()),
                detail:"The bounded recording execution failed; its original input/checkpoints remain owned by this Job".into(),
            },&now())?;
            return Ok(true);
        }
    }
    if target_erased(api, &input)? {
        if !session_executions_released(api, &input.scope.session_id)? {
            return Ok(false);
        }
        api.config_store.finish_backend_job_recording(
            api.workspace_id(),
            id,
            &BackendJobRecordingOutcome::Completed {
                result: serde_json::json!({"application":"not_required"}),
            },
            &now(),
        )?;
        return Ok(true);
    }
    let store = domain_store(api, &input)?;
    let store = subject_authorization::recording_job_source_store(api, &store, &input)
        .map_err(|error| error.error)?;
    let attr = store
        .session_attribution(&input.scope.session_id)
        .map_err(subjektiv_store_error)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Recording source attribution missing".into())
        })?;
    let host = JournalHostContext::validated_recording_source(&store, attr.clone())
        .map_err(subjektiv_journal::domain)?;
    let journal = store
        .journal_for_session(&host)
        .map_err(subjektiv_journal::domain)?;
    let cursor = if let Some(id) = &journal {
        let j = store
            .read_journal(&host, id)
            .map_err(subjektiv_journal::domain)?;
        (j.process_order, j.process_byte)
    } else {
        position(input.scope.start)
    };
    if cursor >= position(input.scope.end) {
        if !session_executions_released(api, &input.scope.session_id)? {
            return Ok(false);
        }
        api.config_store.finish_backend_job_recording(
            api.workspace_id(),
            id,
            &BackendJobRecordingOutcome::Completed {
                result: serde_json::json!({"application":"applied"}),
            },
            &now(),
        )?;
        return Ok(true);
    }
    if cursor < position(input.scope.start) {
        return Err(Error::RepositoryConflict(
            "Recording Job cannot skip an unaccepted prefix".into(),
        ));
    }
    let mut child_count = 0usize;
    let mut after = None;
    loop {
        let children = api.config_store.list_backend_job_recording_child_ids(
            api.workspace_id(),
            id,
            after.as_deref(),
            100,
        )?;
        child_count += children.len();
        if children.len() < 100 {
            break;
        }
        after = children.last().cloned();
    }
    let child_id = format!("{id}:prefix:{}", child_count + 1);
    if let Some(saved) = api.store.get_backend_job(api.workspace_id(), &child_id)? {
        api.config_store.set_backend_job_recording_child(
            api.workspace_id(),
            id,
            input.current_child_job_id.as_deref(),
            Some(&saved.request.job_id),
        )?;
        return Ok(false);
    }
    if let Some(journal) = &journal {
        if api
            .store
            .find_active_backend_job_for_resource(
                api.workspace_id(),
                &crate::backend_job::journal_serialization_key(journal),
            )?
            .is_some()
        {
            return Ok(false);
        }
    }
    let capture = captured_prefix(
        api,
        &input,
        cursor.0,
        cursor.1,
        JournalInput::capture_budget(),
    )?;
    let ranges = subjektiv::journal::recording::capture_ranges(&capture)
        .map_err(subjektiv_journal::domain)?;
    let journal = store
        .accept_journal_sources(&host, "Session activity", &ranges)
        .map_err(subjektiv_journal::domain)?;
    let body = JournalInput::build(&store, &host, &journal, |order, offset, budget| {
        captured_prefix(api, &input, order, offset, budget)
            .map_err(|error| subjektiv::journal::JournalError::Conflict(error.to_string()))
    })
    .map_err(subjektiv_journal::domain)?;
    let mut request = subjektiv_journal::recording_request(&store, attr, &journal, body)?;
    request.job_id = child_id;
    let (_, reservation) = api.config_store.reserve_backend_job_recording_child_with(
        api.workspace_id(),
        id,
        input.current_child_job_id.as_deref(),
        |tx| {
            crate::store::reserve_backend_job_in_transaction(
                tx,
                api.workspace_id(),
                &request,
                &now(),
            )
        },
    )?;
    // The prefix intent and parent binding are saved before eager dispatch.
    api.own_backend_job_recovery(&reservation.attempt);
    Ok(false)
}
