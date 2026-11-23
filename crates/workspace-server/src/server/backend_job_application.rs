//! Result destinations are consumer-owned; their disappearance does not prove that
//! execution or Host effects ended. Only canonical deletion fences settle omission.
use super::*;
use crate::backend_job::{BackendJobRecord, JobResultApplicationDisposition};

/// Call under the consumer's lifecycle gate. Storage, authorization and identity
/// errors remain errors; registry absence and an ordinary removed producer are irrelevant.
pub(super) fn target_not_required(api: &WorkspaceApi, request: &BackendJobRequest) -> Result<bool> {
    let Some(subject) = subjektiv_deletion::job_subject(request) else {
        return Ok(false);
    };
    if subjektiv_deletion::pending(api, subject)? {
        return Ok(true);
    }
    let domain = open_subjektiv_store(api).map_err(|error| error.error)?;
    if domain
        .subject_deletion(subject)
        .map_err(subjektiv_store_error)?
        .is_some()
    {
        return Ok(true);
    }
    // An admitted target with neither its identity nor a deletion fence is an
    // inconsistency, not permission to discard a result or recreate the target.
    if domain
        .subject(subject)
        .map_err(subjektiv_store_error)?
        .is_none()
    {
        return Err(Error::SubjektivSubjectNotFound(subject.into()));
    }
    if request.grants.subjektiv_journal.is_some() {
        return subjektiv_journal::session_erased(api, request);
    }
    Ok(false)
}

/// Preserve omission separately from the execution outcome. A pending execution
/// still enters canonical termination/cleanup; accepted results are never replaced.
pub(super) fn settle_not_required(
    api: &WorkspaceApi,
    job: &BackendJobRecord,
    attempt: &BackendJobAttemptRecord,
) -> Result<()> {
    // Domain consumers validate their saved effect before accepting a result.
    // Later deletion cannot rewrite that historical application, including a
    // crash between acceptance and its application-fact projection.
    let disposition = if attempt.result.is_some() {
        JobResultApplicationDisposition::Applied
    } else {
        JobResultApplicationDisposition::NotRequired
    };
    api.store.record_backend_job_result_application(
        api.workspace_id(),
        attempt,
        &job.intent_fingerprint,
        disposition,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
    )?;
    if !attempt.state.terminal() {
        let cause = crate::backend_job::BackgroundJobCause::ExecutionFailed
            .storage()
            .map_err(Error::BackendJobCauseProjection)?;
        api.finish_backend_job_attempt_with_cause(
            &attempt.job_id,
            &attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "target_not_required",
            "The consumer's canonical deletion fence makes result application unnecessary; execution termination and resource release remain independent",
            Some(&cause),
        )?;
    } else {
        api.schedule_backend_job_worker_cleanup(attempt);
    }
    Ok(())
}
