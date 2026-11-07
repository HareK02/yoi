//! Authenticated Subject-body access to durable Memory/Journal links.
//!
//! The current singleton grants access; identifiers in the request do not. Saved
//! Journal reads deliberately use a reader context, not a source Session, Runtime
//! lookup or restore. Domain storage owns link matching, persistence and paging.
use super::*;
use subjektiv::journal::memory_links::{MemoryJournalRequest, MemoryJournalResponse};
use subjektiv::journal::{JournalError, JournalHostContext};

fn domain(error: JournalError) -> Error {
    match error {
        JournalError::PermissionDenied(message) => Error::WorkspacePermissionDenied(message),
        JournalError::Conflict(message) => Error::RepositoryConflict(message),
        JournalError::InvalidInput(message) => Error::InvalidInput(message),
        JournalError::NotFound(id) => {
            Error::InvalidInput(format!("Journal record not found: {id}"))
        }
        JournalError::Subject(error) => subjektiv_store_error(error),
        other => Error::Store(other.to_string()),
    }
}

pub(super) async fn route(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    source: Option<axum::Extension<crate::worker_source::VerifiedRuntimeRequestSource>>,
    Json(request): Json<MemoryJournalRequest>,
) -> ApiResult<Json<MemoryJournalResponse>> {
    let source = source
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "Memory Journal requires signed Runtime request identity".into(),
            )
        })?
        .0;
    let worker_id = source.worker_id.ok_or_else(|| {
        Error::WorkspacePermissionDenied("Memory Journal requires bound Worker identity".into())
    })?;
    let worker = RuntimeWorkerRef::new(source.runtime_id, worker_id);
    // This is entirely store-owned, including callbacks while a Host parent waits
    // on Runtime. Do not put it behind Host or consult live Runtime/config state.
    api.store
        .db_executor()
        .run(move || {
            validate_workspace_scope(&api, &path.workspace_id)?;
            let projection = api
                .store
                .worker_registry_projection(&path.workspace_id, &worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Memory Journal Worker registration missing".into(),
                    )
                })?;
            // No Job grant implicitly expands to all of a Subject's saved Journal.
            if projection.job.is_some() {
                return Err(Error::WorkspacePermissionDenied(
                    "Memory Journal is not authorized for Job Workers".into(),
                )
                .into());
            }
            let lease = api
                .store
                .require_current_worker_singleton_owner(&path.workspace_id, &worker)?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Memory Journal requires the current Subject Worker".into(),
                    )
                })?;
            let (subject_id, authority) = subjektiv_scope_from_singleton_key(&lease.key)?;
            require_subjektiv_worker_authority(
                authority,
                SubjektivWorkerAuthority::Subject,
                "Memory Journal access",
            )?;
            let operation = match &request {
                MemoryJournalRequest::Reconcile { .. }
                | MemoryJournalRequest::ReconcilePage { .. } => "journal.memory.links.reconcile",
                MemoryJournalRequest::Links { .. } | MemoryJournalRequest::LinksPage { .. } => {
                    "journal.memory.links.read"
                }
                MemoryJournalRequest::Read { .. } => "journal.memory.read",
                MemoryJournalRequest::Related { .. } => "journal.memory.related.read",
            };
            authorize_subjektiv_operation(
                &api,
                &worker,
                &path.workspace_id,
                subject_id,
                operation,
            )?;
            let store = open_subjektiv_store(&api)?;
            let host = JournalHostContext::validated_reader(&store, subject_id).map_err(domain)?;
            store
                .execute_memory_journal_with_source_presence(&host, request, |session| {
                    // Absence is known only when neither retained Worker registration nor an
                    // archive location remains. Runtime disconnection is not material deletion.
                    // These SQL lookups never fetch Session bytes or restore the old Worker.
                    let attribution = store.session_attribution(session).ok()??;
                    if attribution.subject_id != subject_id {
                        return None;
                    }
                    let target =
                        RuntimeWorkerRef::new(&attribution.runtime_id, &attribution.worker_id);
                    let archive = api
                        .config_store
                        .worker_session_archive(&path.workspace_id, &target, session)
                        .ok()?;
                    let registered = api
                        .store
                        .get_worker_registry(&path.workspace_id, &target)
                        .ok()?;
                    if archive.is_none() && registered.is_none() {
                        Some(false)
                    } else {
                        None
                    }
                })
                .map(Json)
                .map_err(domain)
                .map_err(ApiError::from)
        })
        .await
}
