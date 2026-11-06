//! Backend creation boundary. Cause and independent owner observations are retained
//! before HTTP projection. Correlation fingerprints are lookup aids, not operation
//! identities, idempotency keys, execution grants, or permission to retry.
//!
//! # Result investigation
//!
//! Start with the Browser Problem's `yoi_startup.operation.correlation` (also its
//! `instance` suffix). For a known Runtime/Worker identity, `correlation` derives
//! the same path-free token. Find the single `yoi::worker_startup` event named
//! `worker_create_result`; do not use the token as a request or operation key.
//! Compare its typed domain/reason with `WorkspaceApi::worker_create_failure`'s
//! retained reservation evidence. Inspect acceptance/execution first, then finalize,
//! Backend compensation and residual resources independently. Unknown/corrupt or
//! absent history remains unknown/absent, not evidence of rejection or cleanup.
//!
//! The existing Session metadata retains the Runtime startup primary and its own
//! `persistence`; `backend_result_persistence` reports only this coordinator result
//! column's atomic write. A failed sink leaves the primary and observations intact.
//! Reopening the store reads evidence, never dispatches work. Current authorization
//! and a fresh domain projection are required before redisclosing loaded evidence.
//! Unsupported bodies, credentials, source errors and filesystem paths are not log
//! fields. Context wrapping, Remote decoding and persistence do not duplicate the
//! HTTP primary event; cleanup has a separate safe coordinator event. Non-HTTP
//! consumers receive the same typed result and own their eventual observation sink.
use super::*;
use runtime_api::startup::{
    CreationAcceptance, ExecutionOutcome, StartupCause, StartupOperationFacts, WorkerCreateFailure,
};

pub(super) fn local_rejection() -> WorkerCreateFailure {
    WorkerCreateFailure::from_cause(
        StartupCause::Runtime(protocol::startup_failure::RuntimeStartupCause {
            stage: protocol::startup_failure::StartupStage::Preflight,
            reason: protocol::startup_failure::RuntimeStartupReason::StartupFailed,
        }),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Rejected,
            execution: ExecutionOutcome::Unknown,
            finalize: None,
            correlation: None,
            backend_compensation: None,
        },
    )
    .expect("enum-only local cause fits the boundary budget")
}

pub(super) fn finalize_error(
    source: ApiError,
    stage: WorkerSpawnFinalizeStage,
    compensation: runtime_api::startup::CompensationOutcome,
) -> ApiError {
    let failure = finalize_failure(&source, stage, compensation);
    ApiError::with_diagnostics(
        Error::WorkerCreateFailed {
            failure: Box::new(failure),
            source: Some(Box::new(source.error)),
        },
        source.diagnostics,
    )
}

pub(super) fn finalize_failure(
    source: &ApiError,
    stage: WorkerSpawnFinalizeStage,
    compensation: runtime_api::startup::CompensationOutcome,
) -> WorkerCreateFailure {
    use runtime_api::startup::{
        FinalizeFacts, FinalizeStage, StartupBoundaryCause, StartupBoundaryReason,
    };
    let stage = match stage {
        WorkerSpawnFinalizeStage::WorkerIdentity => FinalizeStage::WorkerIdentity,
        WorkerSpawnFinalizeStage::WorkspaceApiBinding => FinalizeStage::WorkspaceApiBinding,
        WorkerSpawnFinalizeStage::CreateReservation => FinalizeStage::CreateReservation,
        WorkerSpawnFinalizeStage::WorkerRegistry => FinalizeStage::WorkerRegistry,
        WorkerSpawnFinalizeStage::TicketAssignmentBind => FinalizeStage::TicketAssignmentBind,
        WorkerSpawnFinalizeStage::TicketAssignmentCurrent => FinalizeStage::TicketAssignmentCurrent,
        WorkerSpawnFinalizeStage::TicketStateAccept => FinalizeStage::TicketStateAccept,
        WorkerSpawnFinalizeStage::WorkdirRegistry => FinalizeStage::WorkdirRegistry,
        WorkerSpawnFinalizeStage::WorkdirAttachment => FinalizeStage::WorkdirAttachment,
    };
    let operation = StartupOperationFacts {
        acceptance: CreationAcceptance::Accepted,
        execution: ExecutionOutcome::Accepted,
        finalize: Some(FinalizeFacts {
            stage,
            compensation,
        }),
        backend_compensation: Some(compensation),
        correlation: None,
    };
    match &source.error {
        Error::WorkerCreateFailed { failure, .. } => {
            let (startup, mut previous) = failure.clone().into_parts();
            // Adding finalization context must not rewrite the earlier owner result.
            previous.finalize = operation.finalize;
            previous.backend_compensation = Some(compensation);
            WorkerCreateFailure::new(startup, previous)
        }
        _ => match source.session_registration_cause {
            Some(cause) => WorkerCreateFailure::from_cause(StartupCause::Session(cause), operation),
            None => WorkerCreateFailure::from_cause(
                StartupCause::Boundary(StartupBoundaryCause {
                    reason: StartupBoundaryReason::FinalizeFailed,
                }),
                operation,
            ),
        },
    }
    .expect("locally constructed finalization facts fit the shared boundary budget")
}

pub(super) fn unknown_result() -> WorkerCreateFailure {
    WorkerCreateFailure::from_cause(
        StartupCause::Boundary(runtime_api::startup::StartupBoundaryCause {
            reason: runtime_api::startup::StartupBoundaryReason::InvalidResponse,
        }),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Unknown,
            execution: ExecutionOutcome::Unknown,
            finalize: None,
            correlation: None,
            backend_compensation: None,
        },
    )
    .expect("enum-only observation fits the boundary budget")
}

pub(super) fn correlate(
    failure: WorkerCreateFailure,
    worker: &RuntimeWorkerRef,
) -> WorkerCreateFailure {
    let reference = correlation(&worker.runtime_id, &worker.worker_id);
    let mut operation = failure.operation().clone();
    operation.correlation = Some(
        runtime_api::startup::StartupCorrelation::new(reference).expect("fixed digest is safe"),
    );
    failure.with_operation(operation)
}

pub(super) fn compensated(
    failure: WorkerCreateFailure,
    outcome: runtime_api::startup::CompensationOutcome,
) -> WorkerCreateFailure {
    let mut operation = failure.operation().clone();
    operation.backend_compensation = Some(outcome);
    failure.with_operation(operation)
}

/// Save only observations on the existing reservation, never its lifecycle state.
/// An unavailable diagnostic sink cannot replace the original primary or authorize replay.
/// The prospective persisted marker commits in the same transaction as these facts.
/// Remote-supplied Backend persistence is discarded at this responsibility boundary.
pub(super) fn persist(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    failure: WorkerCreateFailure,
) -> WorkerCreateFailure {
    use protocol::startup_failure::StartupPersistenceOutcome::{
        Conflict, NotSaved, OutcomeUnknown, Persisted,
    };
    use rusqlite::OptionalExtension;
    let failure = correlate(failure, worker).with_backend_result_persistence(None);
    let Some(worker_id) = WorkerId::parse(&worker.worker_id) else {
        return failure.with_backend_result_persistence(Some(NotSaved));
    };
    let observed = api.config_store.with_conn(|conn| {
        conn.query_row(
            "SELECT create_fingerprint, startup_failure_json FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
            rusqlite::params![api.workspace_id(), worker.runtime_id, worker.worker_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        ).optional().map_err(Error::from)
    });
    let (fingerprint, previous) = match observed {
        Ok(Some(observed)) => observed,
        Ok(None) => return failure.with_backend_result_persistence(Some(NotSaved)),
        Err(_) => return failure.with_backend_result_persistence(Some(OutcomeUnknown)),
    };
    let saved = failure
        .clone()
        .with_backend_result_persistence(Some(Persisted));
    let Ok(encoded) = saved.encode_storage() else {
        return failure.with_backend_result_persistence(Some(NotSaved));
    };
    match api.config_store.save_worker_create_failure(
        api.workspace_id(),
        &worker.runtime_id,
        worker_id,
        &fingerprint,
        previous.as_deref(),
        &encoded,
    ) {
        Ok(true) => saved,
        Ok(false) => failure.with_backend_result_persistence(Some(Conflict)),
        Err(_) => failure.with_backend_result_persistence(Some(OutcomeUnknown)),
    }
}

pub(super) fn correlation(runtime_id: &str, worker_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(runtime_id.as_bytes());
    hash.update([0]);
    hash.update(worker_id.as_bytes());
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("worker-{digest}")
}

/// Only the HTTP result boundary records the primary failure. Context wrappers,
/// encoders, and Remote adapters do not emit duplicate failure records. The
/// independent coordinator cleanup event contains no exception/source text.
pub(super) fn observe_http(failure: &WorkerCreateFailure, status: StatusCode) {
    let Ok(projected) = failure.telemetry() else {
        tracing::error!(target: "yoi::worker_startup", event = "startup_projection_unavailable", status = status.as_u16());
        return;
    };
    let fields = error_contract::observation_fields(&projected);
    let operation = failure.operation();
    tracing::warn!(
        target: "yoi::worker_startup",
        event = "worker_create_result",
        domain = fields.domain,
        reason = fields.reason,
        operation = fields.operation,
        stage = fields.stage,
        correlation = operation.correlation.as_ref().map(|value| value.as_str()),
        acceptance = ?operation.acceptance,
        execution = ?operation.execution,
        finalize = ?operation.finalize,
        backend_compensation = ?operation.backend_compensation,
        backend_result_persistence = ?failure.backend_result_persistence(),
        persistence = ?failure.startup().persistence,
        resources = ?failure.startup().resources,
        status = status.as_u16(),
        "Worker creation returned a typed failure"
    );
}
