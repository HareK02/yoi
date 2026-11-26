//! Runtime schema-22 conversion of saved attachment expectations, not grants.
//!
//! Only a confirmed attribution for this Worker's active Session can recover a
//! Subject body identity. Unknown identities (including old Job bindings, which
//! saved no Subject) remain explicit unresolved connections: the owner adapter
//! rejects their restoration rather than silently starting an ordinary Worker.
//! Fingerprints, settings, lifecycle ownership and Session history are retained.

use super::*;
use protocol::feature_initialization::{FeatureInitialization, FeatureInitializations};
use subjektiv_api::worker_initialization::{
    FEATURE_ID, ResolvedWorkerConnection, VERSION, WorkerConnectionKind,
};

// These are storage-owned request positions, not a recursive search through
// user input, plugin payloads, manifests or receipt bodies.
const REQUEST_PATHS: &[&str] = &[
    "/request",
    "/execution/request",
    "/execution_state/execution/request",
    "/execution_state/execution/execution/request",
    "/execution_state/execution/intent/request",
    "/execution_state/execution/intent/pending_restore/request",
    "/execution_state/execution/intent/restore_finalization/execution/request",
];

pub(super) fn migrate(
    document: &mut serde_json::Value,
    identity_path: &Path,
) -> Result<(), RuntimeError> {
    for pointer in REQUEST_PATHS {
        let Some(request) = document
            .pointer_mut(pointer)
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        let Some(attached) = request.get("subjektiv_attached") else {
            continue;
        };
        let attached = attached.as_bool().ok_or_else(|| {
            runtime_store_corrupt(
                identity_path,
                "invalid legacy Subjektiv attachment flag".into(),
            )
        })?;
        let initialization: FeatureInitializations = request
            .get("feature_initialization")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| runtime_store_corrupt(identity_path, error.to_string()))?
            .unwrap_or_default();
        if !initialization.is_empty() {
            return Err(runtime_store_corrupt(
                identity_path,
                "Worker request contains both legacy attachment and Feature initialization".into(),
            ));
        }
        let initialization = if attached {
            let workspace = request
                .pointer("/workspace_api/workspace_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    runtime_store_corrupt(
                        identity_path,
                        "attached Worker has no Workspace scope".into(),
                    )
                })?;
            let job = request.get("backend_job").filter(|job| !job.is_null());
            if let Some(job) = job
                && (job["subjektiv_consolidation"] != true || job["subjektiv_journal"] == true)
            {
                return Err(runtime_store_corrupt(
                    identity_path,
                    "legacy attached Job has no exclusive consolidation binding".into(),
                ));
            }
            let connection = ResolvedWorkerConnection {
                workspace_id: workspace.into(),
                subject_id: if job.is_some() {
                    None
                } else {
                    confirmed_subject(request, workspace, identity_path)?
                },
                kind: if job.is_some() {
                    WorkerConnectionKind::MemoryConsolidation
                } else {
                    WorkerConnectionKind::SubjectBody
                },
            };
            if connection.subject_id.is_some() {
                connection
                    .validate()
                    .map_err(|error| runtime_store_corrupt(identity_path, error))?;
            }
            let entry = FeatureInitialization::encode(FEATURE_ID, VERSION, &connection)
                .map_err(|error| runtime_store_corrupt(identity_path, error))?;
            FeatureInitializations::new(vec![entry])
                .map_err(|error| runtime_store_corrupt(identity_path, error))?
        } else {
            initialization
        };
        let request = request
            .as_object_mut()
            .expect("attachment field belongs to object");
        request.remove("subjektiv_attached");
        request.insert(
            "feature_initialization".into(),
            serde_json::to_value(initialization)
                .map_err(|error| runtime_store_corrupt(identity_path, error.to_string()))?,
        );
    }
    Ok(())
}

fn confirmed_subject(
    request: &serde_json::Value,
    workspace: &str,
    identity_path: &Path,
) -> Result<Option<String>, RuntimeError> {
    let metadata_path = identity_path.with_file_name(WORKER_METADATA_FILE);
    if !metadata_path
        .try_exists()
        .map_err(|error| runtime_io_error("inspect Worker metadata", &metadata_path, error))?
    {
        return Ok(None);
    }
    let metadata: serde_json::Value = read_json(&metadata_path, "read saved Subject attribution")?;
    let worker_id: WorkerId = serde_json::from_value(request["worker_id"].clone())
        .map_err(|error| runtime_store_corrupt(identity_path, error.to_string()))?;
    if metadata["worker_name"].as_str() != Some(runtime_worker_name(worker_id).as_str())
        || metadata["workspace_id"].as_str() != Some(workspace)
    {
        return Err(runtime_store_corrupt(
            &metadata_path,
            "saved Subject attribution has a different Worker or Workspace owner".into(),
        ));
    }
    let attribution = &metadata["subjektiv_session_attribution"];
    if attribution["state"] != "confirmed"
        || attribution["session_id"].is_null()
        || metadata.pointer("/active/session_id") != Some(&attribution["session_id"])
    {
        return Ok(None);
    }
    let subject = attribution["subject_id"].as_str().ok_or_else(|| {
        runtime_store_corrupt(
            &metadata_path,
            "confirmed attribution has no Subject identity".into(),
        )
    })?;
    Ok(Some(subject.into()))
}

pub(super) fn has_unresolved_connection(aggregate: &WorkerAggregateRecord) -> bool {
    let request = match &aggregate.execution_state {
        PersistedWorkerExecutionState::NotStarted(execution)
        | PersistedWorkerExecutionState::Stopped(execution)
        | PersistedWorkerExecutionState::Suspended(execution) => {
            execution.as_ref().map(|execution| &execution.request)
        }
        PersistedWorkerExecutionState::Runnable(execution) => Some(&execution.request),
        PersistedWorkerExecutionState::RestoreAdmitted(admission) => admission
            .execution
            .as_ref()
            .map(|execution| &execution.request),
        PersistedWorkerExecutionState::RestoreFinalization(finalization) => finalization
            .execution
            .as_ref()
            .map(|execution| &execution.request),
        PersistedWorkerExecutionState::ReconciliationRequired(operation) => match operation {
            PersistedWorkerLifecycleOperation::Create(create) => create.request.as_ref(),
            PersistedWorkerLifecycleOperation::Restore(restore) => Some(&restore.request),
        },
    };
    request
        .and_then(|request| request.feature_initialization.get(FEATURE_ID))
        .and_then(|entry| {
            entry
                .decode::<ResolvedWorkerConnection>(FEATURE_ID, VERSION)
                .ok()
        })
        .is_some_and(|connection| connection.subject_id.is_none())
}

pub(super) fn record_unavailable_workers(
    document: &mut serde_json::Value,
    workers: Vec<WorkerRef>,
    path: &Path,
) -> Result<(), RuntimeError> {
    for worker_ref in workers {
        let id = document["next_diagnostic_id"].as_u64().ok_or_else(|| {
            runtime_store_corrupt(path, "Runtime diagnostic identity is missing".into())
        })?;
        document["next_diagnostic_id"] = serde_json::json!(id.checked_add(1).ok_or_else(|| {
            runtime_store_corrupt(path, "Runtime diagnostic identity exhausted".into())
        })?);
        let diagnostic = RuntimeDiagnostic {
            id,
            worker_ref: Some(worker_ref),
            severity: DiagnosticSeverity::Warning,
            code: "worker_subject_identity_unresolved".into(),
            message: "Saved Subjektiv attachment retained without a provable Subject identity; restoration is unavailable and no replacement connection was issued".into(),
        };
        document["diagnostics"]
            .as_array_mut()
            .ok_or_else(|| {
                runtime_store_corrupt(path, "Runtime diagnostics must be an array".into())
            })?
            .push(
                serde_json::to_value(diagnostic)
                    .map_err(|error| runtime_store_corrupt(path, error.to_string()))?,
            );
    }
    Ok(())
}
