//! Drive authority is rechecked inside the metadata transaction against the
//! attached Server DB. A bound handle is not an authorization cache or lease.
use super::*;
use rusqlite::params;
use server_api::{
    DriveApiError, DriveApiErrorCode, DriveGrantCreateRequest, DriveGrantListQuery,
    DriveGrantListResponse, DriveGrantResponse, ServerRequestContext,
};

type DriveResult<T> = std::result::Result<T, DriveApiError>;
fn fail(code: DriveApiErrorCode) -> DriveApiError {
    DriveApiError::new(code)
}

pub(super) fn pre_error(error: impl Into<ApiError>) -> DriveApiError {
    let error = error.into();
    fail(match api_error_status(&error.error).as_u16() {
        401 | 403 => DriveApiErrorCode::Denied,
        404 => DriveApiErrorCode::NotFound,
        409 => DriveApiErrorCode::Conflict,
        413 => DriveApiErrorCode::Limit,
        500..=599 => DriveApiErrorCode::StorageUnavailable,
        _ => DriveApiErrorCode::Invalid,
    })
}
fn mutation_error(error: Error) -> DriveApiError {
    if matches!(error, Error::Sqlite(_) | Error::Store(_)) {
        fail(DriveApiErrorCode::OutcomeUnknown)
    } else {
        pre_error(error)
    }
}
fn worker_request(context: &ServerRequestContext) -> bool {
    context.runtime_source.is_some()
        || context.worker_source.is_some()
        || context.transport_headers.iter().any(|(name, _)| {
            name.eq_ignore_ascii_case("x-yoi-runtime-id")
                || name.eq_ignore_ascii_case("x-yoi-worker-id")
        })
}
fn browser_actor(context: &ServerRequestContext) -> DriveResult<&RequestActor> {
    if worker_request(context) {
        return Err(fail(DriveApiErrorCode::Denied));
    }
    context
        .actor
        .as_ref()
        .ok_or_else(|| fail(DriveApiErrorCode::Denied))
}

pub(super) async fn authorized_drive(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace_id: &str,
    write: bool,
) -> DriveResult<workspace_drive::Drive> {
    validate_workspace_scope(api, workspace_id).map_err(pre_error)?;
    let workspace = workspace_id.to_owned();
    let drive = if worker_request(context) {
        // Never fall back to an actor when a Worker/Runtime source is present,
        // incomplete, or inconsistent with its transport identity.
        let headers = current_worker_contract_headers(context)
            .map_err(|_| fail(DriveApiErrorCode::Denied))?;
        let worker = current_worker_identity(api, workspace_id, &headers)
            .map_err(|_| fail(DriveApiErrorCode::Denied))?;
        if context.worker_source.as_ref().is_some_and(|source| {
            source.runtime_id != worker.runtime_id || source.worker_id != worker.worker_id
        }) {
            return Err(fail(DriveApiErrorCode::Denied));
        }
        let drive = api.drive().await.map_err(pre_error)?;
        drive.with_authorizer(move |conn, write| {
            if !crate::store::drive_worker_is_live(conn, true, &workspace, &worker)? {
                return Err(workspace_drive::Error::Denied);
            }
            let granted: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM yoi_workspace_authority.workspace_drive_grants
                WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND revoked=0 AND (?4=0 OR access='read_write'))",
                params![workspace,worker.runtime_id,worker.worker_id,write],|row|row.get(0))?;
            if !granted { return Err(workspace_drive::Error::Denied); }
            Ok(())
        })
    } else {
        let actor = browser_actor(context)?;
        let account = actor.account_id.clone();
        let user = actor.user_id.clone();
        let drive = api.drive().await.map_err(pre_error)?;
        // Ordinary Workspace use is member-facing: the current Server policy
        // admits persisted authenticated users, not only the Workspace owner.
        drive.with_authorizer(move |conn, _write| {
            let member: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM yoi_workspace_authority.workspaces ws
                CROSS JOIN yoi_workspace_authority.users u
                WHERE ws.workspace_id=?1 AND ws.state='active' AND u.account_id=?2 AND u.user_id=?3)",
                params![workspace,account,user],|row|row.get(0))?;
            if !member { return Err(workspace_drive::Error::Denied); }
            Ok(())
        })
    };
    // Reject missing/read-only grants before upload body reception. This check
    // is admission only: every later operation rechecks under its own lock.
    tokio::task::spawn_blocking(move || {
        drive.check_authorized(write)?;
        Ok::<_, workspace_drive::Error>(drive)
    })
    .await
    .map_err(|_| fail(DriveApiErrorCode::StorageUnavailable))?
    .map_err(|error| {
        fail(match error {
            workspace_drive::Error::Denied => DriveApiErrorCode::Denied,
            workspace_drive::Error::Fenced | workspace_drive::Error::Conflict => {
                DriveApiErrorCode::Conflict
            }
            workspace_drive::Error::NotFound => DriveApiErrorCode::NotFound,
            workspace_drive::Error::Invalid(_) => DriveApiErrorCode::Invalid,
            _ => DriveApiErrorCode::StorageUnavailable,
        })
    })
}

pub(super) async fn create_grant(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace_id: &str,
    request: DriveGrantCreateRequest,
) -> DriveResult<DriveGrantResponse> {
    let actor = browser_actor(context)?;
    require_workspace_owner(api, workspace_id, actor, "Drive grants")
        .await
        .map_err(pre_error)?;
    for identity in [&request.runtime_id, &request.worker_id] {
        if identity.is_empty() || identity.len() > 256 || identity.chars().any(char::is_control) {
            return Err(fail(DriveApiErrorCode::Invalid));
        }
    }
    let worker = RuntimeWorkerRef::new(&request.runtime_id, &request.worker_id);
    let summary = api
        .runtime
        .worker(&worker)
        .map_err(|_| fail(DriveApiErrorCode::NotFound))?;
    if summary.workspace.workspace_id.as_deref() != Some(workspace_id) {
        return Err(fail(DriveApiErrorCode::Denied));
    }
    let grant = DriveGrantResponse {
        grant_id: String::new(),
        workspace_id: workspace_id.into(),
        runtime_id: request.runtime_id,
        worker_id: request.worker_id,
        access: request.access,
        revoked: false,
        created_by: actor.account_id.clone(),
        created_at: String::new(),
        revoked_by: None,
        revoked_at: None,
    };
    let store = api.store.clone();
    tokio::task::spawn_blocking(move || store.create_workspace_drive_grant(&grant))
        .await
        .map_err(|_| fail(DriveApiErrorCode::OutcomeUnknown))?
        .map_err(mutation_error)
}

pub(super) async fn revoke_grant(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace_id: &str,
    grant_id: &str,
) -> DriveResult<DriveGrantResponse> {
    let actor = browser_actor(context)?;
    require_workspace_owner(api, workspace_id, actor, "Drive grants")
        .await
        .map_err(pre_error)?;
    validate_grant_id(grant_id)?;
    let (workspace, grant, actor) = (
        workspace_id.to_owned(),
        grant_id.to_owned(),
        actor.account_id.clone(),
    );
    let store = api.store.clone();
    tokio::task::spawn_blocking(move || {
        store.revoke_workspace_drive_grant(&workspace, &grant, &actor)
    })
    .await
    .map_err(|_| fail(DriveApiErrorCode::OutcomeUnknown))?
    .map_err(mutation_error)?
    .ok_or_else(|| fail(DriveApiErrorCode::NotFound))
}

fn validate_grant_id(id: &str) -> DriveResult<()> {
    if !id
        .parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
    {
        return Err(fail(DriveApiErrorCode::Invalid));
    }
    Ok(())
}
pub(super) async fn list_grants(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace_id: &str,
    query: DriveGrantListQuery,
) -> DriveResult<DriveGrantListResponse> {
    let actor = browser_actor(context)?;
    require_workspace_owner(api, workspace_id, actor, "Drive grants")
        .await
        .map_err(pre_error)?;
    let limit = query.limit.unwrap_or(100) as usize;
    if limit == 0 || limit > 200 {
        return Err(fail(DriveApiErrorCode::Invalid));
    }
    if let Some(after) = &query.after {
        validate_grant_id(after)?;
    }
    let workspace = workspace_id.to_owned();
    let store = api.store.clone();
    let mut grants = tokio::task::spawn_blocking(move || {
        store.list_workspace_drive_grants(&workspace, limit + 1, query.after.as_deref())
    })
    .await
    .map_err(|_| fail(DriveApiErrorCode::StorageUnavailable))?
    .map_err(pre_error)?;
    let more = grants.len() > limit;
    grants.truncate(limit);
    let next_after = if more {
        grants.last().map(|g| g.grant_id.clone())
    } else {
        None
    };
    Ok(DriveGrantListResponse { grants, next_after })
}
