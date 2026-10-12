//! Logical configuration operations share the durable attachment ledger and canonical CAS.
//! This module never opens a Runtime filesystem session or exports a host path.
//! Files support read/edit/write and non-entrypoint deletion; directories support
//! create, with atomic change sets (including non-entrypoint renames) on the root.
//! The canonical main.dcdl entrypoint cannot be deleted or renamed.
use super::*;
use server_api::{
    WorkspaceConfigAccess as Access, WorkspaceConfigApiError as Failure,
    WorkspaceConfigFailureClassification as Classification, WorkspaceConfigNodeKind as NodeKind,
};
use server_api::{
    WorkspaceConfigAttachRequest, WorkspaceConfigAttachment, WorkspaceConfigCommitRequest,
    WorkspaceConfigCommitResponse, WorkspaceConfigGrantCreateRequest, WorkspaceConfigGrantResponse,
    WorkspaceConfigNode, WorkspaceConfigObserveRequest, WorkspaceConfigObserveResponse,
    WorkspaceConfigReadRequest, WorkspaceConfigReadResponse,
};

const ALIAS: &str = "workspace-config";
const MAX_NODES: usize = 256;
const MAX_TEXT: usize = 256 * 1024;
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
type ConfigResult<T> = std::result::Result<T, Failure>;

fn fail(status: u16, code: &str) -> Failure {
    Failure {
        status,
        code: code.into(),
        message: format!("Workspace configuration: {code}"),
        classification: Classification::NotCommitted,
    }
}
pub(super) fn pre_error(error: impl Into<ApiError>) -> Failure {
    let error = error.into();
    let status = api_error_status(&error.error).as_u16();
    fail(
        status,
        match status {
            401 | 403 => "access_denied",
            404 => "not_found",
            409 => "conflict",
            413 => "limit_exceeded",
            500..=599 => "unavailable",
            _ => "invalid_request",
        },
    )
}
fn mutation_error(error: Error) -> Failure {
    // Storage/service failures at a mutation boundary may include a commit
    // acknowledgement failure. Never describe those as a guaranteed rollback.
    if matches!(&error, Error::Sqlite(_) | Error::Store(_)) {
        unknown()
    } else {
        pre_error(error)
    }
}
fn unknown() -> Failure {
    Failure {
        classification: Classification::Unknown,
        ..fail(500, "outcome_unknown")
    }
}
pub(super) fn identity(
    api: &WorkspaceApi,
    context: &server_api::ServerRequestContext,
    workspace_id: &str,
) -> ConfigResult<RuntimeWorkerRef> {
    validate_workspace_scope(api, workspace_id).map_err(pre_error)?;
    let headers =
        current_worker_contract_headers(context).map_err(|_| fail(403, "access_denied"))?;
    current_worker_identity(api, workspace_id, &headers).map_err(pre_error)
}
async fn lock(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ConfigResult<tokio::sync::OwnedMutexGuard<()>> {
    tokio::time::timeout(
        DEADLINE,
        current_worker_session_lock(api, worker).lock_owned(),
    )
    .await
    .map_err(|_| fail(503, "busy"))
}
fn grant_for_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ConfigResult<WorkspaceConfigGrantResponse> {
    api.store
        .current_workspace_config_grant(&api.config.workspace_id, worker)
        .map_err(pre_error)?
        .filter(|grant| !grant.revoked)
        .ok_or_else(|| fail(403, "access_denied"))
}
fn access_capabilities(access: Access) -> workdir::WorkdirSessionCapabilities {
    match access {
        Access::ReadOnly => workdir::WorkdirSessionCapabilities::READ_ONLY,
        Access::ReadWrite => workdir::WorkdirSessionCapabilities::READ_WRITE,
    }
}
pub(super) fn grant_capabilities(
    api: &WorkspaceApi,
    grant_id: &str,
) -> Result<workdir::WorkdirSessionCapabilities> {
    let grant = api
        .store
        .get_workspace_config_grant(&api.config.workspace_id, grant_id)?
        .filter(|grant| !grant.revoked)
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("Workspace config grant unavailable".into())
        })?;
    Ok(access_capabilities(grant.access))
}
pub(super) fn granted_to(api: &WorkspaceApi, worker: &RuntimeWorkerRef, grant_id: &str) -> bool {
    grant_for_worker(api, worker).is_ok_and(|grant| grant.grant_id == grant_id)
}
fn active_link(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    grant: &WorkspaceConfigGrantResponse,
    connection_id: Option<&str>,
) -> ConfigResult<Option<WorkerWorkdirLinkRecord>> {
    if connection_id.is_some_and(|id| id.is_empty() || id.len() > 128) {
        return Err(fail(400, "invalid_connection"));
    }
    let link = api
        .store
        // The T697 exclusive-active-Workdir ledger bounds this lookup to one
        // current lifetime, regardless of accumulated detached history.
        .list_workdir_worker_links(&api.config.workspace_id, &grant.working_directory_id)
        .map_err(pre_error)?
        .into_iter()
        .find(|link| link.unlinked_at.is_none() && link.worker == *worker && link.alias == ALIAS);
    if let Some(link) = &link {
        let record = api
            .store
            .get_workdir_registry(&api.config.workspace_id, &link.workdir_id)
            .map_err(pre_error)?
            .ok_or_else(|| fail(403, "access_denied"))?;
        if link.workdir_id != grant.working_directory_id
            || !matches!(record.source, WorkdirRegistrySource::WorkspaceConfig { ref grant_id } if grant_id == &grant.grant_id)
        {
            return Err(fail(403, "access_denied"));
        }
    }
    if let Some(id) = connection_id {
        if !link.as_ref().is_some_and(|link| link.connection_id == id) {
            return Err(fail(409, "connection_changed"));
        }
    }
    Ok(link)
}
fn effective_access(
    grant: &WorkspaceConfigGrantResponse,
    link: &WorkerWorkdirLinkRecord,
) -> Access {
    if grant.access == Access::ReadWrite
        && link
            .capabilities
            .supports(workdir::WorkdirSessionCapability::Write)
        && link
            .capabilities
            .supports(workdir::WorkdirSessionCapability::Edit)
    {
        Access::ReadWrite
    } else {
        Access::ReadOnly
    }
}
fn attachment(
    grant: &WorkspaceConfigGrantResponse,
    link: &WorkerWorkdirLinkRecord,
    already_attached: bool,
) -> WorkspaceConfigAttachment {
    WorkspaceConfigAttachment {
        workspace_id: grant.workspace_id.clone(),
        connection_id: link.connection_id.clone(),
        alias: ALIAS.into(),
        working_directory_id: link.workdir_id.clone(),
        access: effective_access(grant, link),
        name: "Workspace configuration".into(),
        purpose: "Explore and edit this Workspace's canonical configuration".into(),
        content_path: "/workspace-config".into(),
        already_attached,
    }
}
pub(super) async fn get(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ConfigResult<Option<WorkspaceConfigAttachment>> {
    let _guard = lock(api, worker).await?;
    let grant = grant_for_worker(api, worker)?;
    Ok(active_link(api, worker, &grant, None)?.map(|link| attachment(&grant, &link, true)))
}
pub(super) async fn attach(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: WorkspaceConfigAttachRequest,
) -> ConfigResult<WorkspaceConfigAttachment> {
    if request.alias.as_deref().is_some_and(|alias| alias != ALIAS) {
        return Err(fail(400, "invalid_alias"));
    }
    let _guard = lock(api, worker).await?;
    let grant = grant_for_worker(api, worker)?;
    let access = request.access.unwrap_or(grant.access);
    if access == Access::ReadWrite && grant.access != Access::ReadWrite {
        return Err(fail(403, "access_denied"));
    }
    if let Some(mut link) = active_link(api, worker, &grant, None)? {
        // Same active alias/identity is idempotent; explicit write never silently downgrades.
        if request.access == Some(Access::ReadWrite)
            && effective_access(&grant, &link) != Access::ReadWrite
        {
            return Err(fail(403, "access_denied"));
        }
        if access == Access::ReadOnly && effective_access(&grant, &link) != Access::ReadOnly {
            link.capabilities = access_capabilities(access);
            link = api
                .store
                .attach_worker_workdir(&link)
                .map_err(mutation_error)?;
        }
        return Ok(attachment(&grant, &link, true));
    }
    let link = api
        .store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: grant.workspace_id.clone(),
            worker: worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: ALIAS.into(),
            capabilities: access_capabilities(access),
            linked_at: now_registry_timestamp(),
            unlinked_at: None,
        })
        .map_err(mutation_error)?;
    // No Runtime filesystem/session synchronization for a logical config target.
    api.worker_projection
        .refresh(worker)
        .map_err(|_| unknown())?;
    Ok(attachment(&grant, &link, false))
}
fn resolve(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    connection_id: &str,
) -> ConfigResult<(WorkspaceConfigGrantResponse, WorkerWorkdirLinkRecord)> {
    let grant = grant_for_worker(api, worker)?;
    let link = active_link(api, worker, &grant, Some(connection_id))?
        .ok_or_else(|| fail(409, "connection_changed"))?;
    Ok((grant, link))
}
fn path_valid(path: &str, root: bool) -> ConfigResult<()> {
    if root && path.is_empty() {
        return Ok(());
    }
    config_source::VirtualPath::parse(path).map_err(|_| fail(400, "invalid_path"))?;
    if path.chars().any(char::is_control) {
        return Err(fail(400, "invalid_path"));
    }
    Ok(())
}
fn tree(api: &WorkspaceApi) -> ConfigResult<crate::config_source::WorkspaceConfigState> {
    api.config_store
        .load_workspace_config(&api.config.workspace_id)
        .map_err(pre_error)?
        .ok_or_else(|| fail(404, "not_found"))
}
fn validator(
    grant: &WorkspaceConfigGrantResponse,
    link: &WorkerWorkdirLinkRecord,
    state: &crate::config_source::WorkspaceConfigState,
    applied_schema_fingerprint: &str,
    path: &str,
) -> String {
    let bound = serde_json::to_vec(&(
        "workspace-config:v1",
        &grant.workspace_id,
        &grant.grant_id,
        &grant.runtime_id,
        &grant.worker_id,
        &link.connection_id,
        effective_access(grant, link),
        path,
        &state.snapshot.digest,
        &state.contract.fingerprint,
        &state.projection_digest,
        applied_schema_fingerprint,
    ))
    .expect("bounded configuration validator tuple");
    format!(
        "wc1:{}",
        Sha256::digest(bound)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}
fn check_validator(actual: &str, expected: &str) -> ConfigResult<()> {
    if actual != expected {
        Err(fail(409, "stale_validator"))
    } else {
        Ok(())
    }
}
pub(super) async fn observe(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: WorkspaceConfigObserveRequest,
) -> ConfigResult<WorkspaceConfigObserveResponse> {
    if request.paths.is_empty() || request.paths.len() > MAX_NODES || request.depth > 8 {
        return Err(fail(400, "limit_exceeded"));
    }
    for path in &request.paths {
        path_valid(path, true)?;
    }
    let _guard = lock(api, worker).await?;
    let (grant, link) = resolve(api, worker, &request.connection_id)?;
    let state = tree(api)?;
    let schema_fingerprint = api
        .config_schema_registry
        .compose()
        .map_err(pre_error)?
        .fingerprint;
    let snapshot = &state.snapshot;
    let writable = effective_access(&grant, &link) == Access::ReadWrite;
    // Build only bounded metadata (the canonical tree itself is bounded by config-source).
    let mut kinds = BTreeMap::from([(String::new(), NodeKind::Directory)]);
    for path in snapshot.entries.keys() {
        let text = path.as_str();
        kinds.insert(text.to_string(), NodeKind::File);
        let mut cursor = text;
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            kinds.entry(parent.into()).or_insert(NodeKind::Directory);
            cursor = parent;
        }
    }
    let mut selected = BTreeMap::new();
    for path in request.paths {
        selected.insert(
            path.clone(),
            *kinds.get(&path).unwrap_or(&NodeKind::Missing),
        );
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        for (child, kind) in kinds.range(prefix.clone()..) {
            if !child.starts_with(&prefix) {
                break;
            }
            if child == &path {
                continue;
            }
            let relative = &child[prefix.len()..];
            if relative.split('/').count() <= request.depth as usize {
                selected.insert(child.clone(), *kind);
            }
            if selected.len() > MAX_NODES {
                return Err(fail(413, "limit_exceeded"));
            }
        }
    }
    if selected.len() > MAX_NODES {
        return Err(fail(413, "limit_exceeded"));
    }
    let nodes = selected
        .into_iter()
        .map(|(path, kind)| {
            let entry = snapshot
                .entries
                .iter()
                .find(|(key, _)| key.as_str() == path)
                .map(|(_, entry)| entry);
            let mut operations = match kind {
                NodeKind::File => vec!["read".to_string()],
                NodeKind::Directory | NodeKind::Missing => vec![],
            };
            if writable {
                match kind {
                    NodeKind::File => {
                        operations.extend(["edit", "write"].map(str::to_string));
                        if path != crate::config_source::MAIN_CONFIG_ENTRYPOINT {
                            operations.push("delete".into());
                        }
                    }
                    NodeKind::Missing => operations.push("create".into()),
                    NodeKind::Directory => {
                        operations.push("create".into());
                        if path.is_empty() {
                            operations.push("apply_changes".into());
                        }
                    }
                }
            }
            WorkspaceConfigNode {
                validator: validator(&grant, &link, &state, &schema_fingerprint, &path),
                path,
                kind,
                digest: entry.map(|entry| entry.content_digest.clone()),
                content_type: entry.map(|entry| match entry.content_type {
                    config_source::ConfigContentType::Decodal => {
                        server_api::ConfigContentType::Decodal
                    }
                    config_source::ConfigContentType::Text => server_api::ConfigContentType::Text,
                }),
                operations,
            }
        })
        .collect();
    Ok(WorkspaceConfigObserveResponse {
        connection_id: link.connection_id.clone(),
        validator: validator(&grant, &link, &state, &schema_fingerprint, ""),
        digest: snapshot.digest.clone(),
        entrypoints: state
            .contract
            .entrypoints
            .iter()
            .map(ToString::to_string)
            .collect(),
        nodes,
    })
}
pub(super) async fn read(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: WorkspaceConfigReadRequest,
) -> ConfigResult<WorkspaceConfigReadResponse> {
    path_valid(&request.path, false)?;
    let _guard = lock(api, worker).await?;
    let (grant, link) = resolve(api, worker, &request.connection_id)?;
    let state = tree(api)?;
    let schema_fingerprint = api
        .config_schema_registry
        .compose()
        .map_err(pre_error)?
        .fingerprint;
    let expected = validator(&grant, &link, &state, &schema_fingerprint, &request.path);
    check_validator(&request.validator, &expected)?;
    let path =
        config_source::VirtualPath::parse(&request.path).map_err(|_| fail(400, "invalid_path"))?;
    let entry = state
        .snapshot
        .entries
        .get(&path)
        .ok_or_else(|| fail(404, "not_found"))?;
    if entry.content.len() > MAX_TEXT {
        return Err(fail(413, "limit_exceeded"));
    }
    Ok(WorkspaceConfigReadResponse {
        path: request.path,
        content: entry.content.clone(),
        content_type: match entry.content_type {
            config_source::ConfigContentType::Decodal => server_api::ConfigContentType::Decodal,
            config_source::ConfigContentType::Text => server_api::ConfigContentType::Text,
        },
        digest: entry.content_digest.clone(),
        validator: expected,
    })
}
pub(super) async fn commit(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: WorkspaceConfigCommitRequest,
) -> ConfigResult<WorkspaceConfigCommitResponse> {
    if request.request.changes.len() > MAX_NODES {
        return Err(fail(413, "limit_exceeded"));
    }
    for change in &request.request.changes {
        match change {
            server_api::ConfigTreeChange::Create { path, content, .. }
            | server_api::ConfigTreeChange::Update { path, content, .. } => {
                path_valid(path, false)?;
                if content.len() > MAX_TEXT {
                    return Err(fail(413, "limit_exceeded"));
                }
            }
            server_api::ConfigTreeChange::Rename { from, to, .. } => {
                path_valid(from, false)?;
                path_valid(to, false)?;
            }
            server_api::ConfigTreeChange::Delete { path, .. } => path_valid(path, false)?,
        }
    }
    let _guard = lock(api, worker).await?;
    let (grant, link) = resolve(api, worker, &request.connection_id)?;
    if effective_access(&grant, &link) != Access::ReadWrite {
        return Err(fail(403, "access_denied"));
    }
    let state = tree(api)?;
    let schema_fingerprint = api
        .config_schema_registry
        .compose()
        .map_err(pre_error)?
        .fingerprint;
    check_validator(
        &request.validator,
        &validator(&grant, &link, &state, &schema_fingerprint, ""),
    )?;
    let canonical = config_commit_request_from_api(request.request).map_err(pre_error)?;
    // Evaluation/semantic validation are pure preparation. Bound this stage
    // without ever dispatching persistence in a timed-out background task.
    let preparation_api = api.clone();
    let preparation = tokio::task::spawn_blocking(move || {
        prepare_workspace_config_tree(
            &preparation_api,
            &preparation_api.config.workspace_id,
            &canonical,
        )
    });
    let candidate = tokio::time::timeout(DEADLINE, preparation)
        .await
        .map_err(|_| fail(503, "evaluation_timeout"))?
        .map_err(|_| fail(500, "evaluation_unavailable"))?
        .map_err(pre_error)?;
    if candidate.base_digest != state.snapshot.digest
        || candidate.base_toolchain_fingerprint != state.contract.fingerprint
        || candidate.base_projection_digest != state.projection_digest
        || candidate.contract.schema_bundle.fingerprint != schema_fingerprint
    {
        return Err(fail(409, "stale_validator"));
    }
    // Recheck durable authority before the first auxiliary/canonical effects.
    let (current_grant, current_link) = resolve(api, worker, &request.connection_id)?;
    if current_grant != grant || current_link.capabilities != link.capabilities {
        return Err(fail(409, "authority_changed"));
    }
    // From this boundary repository credential projection may have side effects.
    // Never falsely claim rollback or automatically replay a failed/uncertain save.
    let state = persist_workspace_config_candidate(api, &api.config.workspace_id, &candidate)
        .map_err(|_| unknown())?;
    Ok(WorkspaceConfigCommitResponse {
        validator: validator(&grant, &link, &state, &schema_fingerprint, ""),
        digest: state.snapshot.digest,
    })
}
pub(super) async fn create_grant(
    api: &WorkspaceApi,
    actor: &RequestActor,
    workspace_id: &str,
    request: WorkspaceConfigGrantCreateRequest,
) -> ConfigResult<WorkspaceConfigGrantResponse> {
    require_workspace_owner(api, workspace_id, actor, "Workspace config grants")
        .await
        .map_err(pre_error)?;
    let worker = RuntimeWorkerRef::new(&request.runtime_id, &request.worker_id);
    let _guard = lock(api, &worker).await?;
    let summary = api
        .runtime
        .worker(&worker)
        .map_err(|_| fail(404, "worker_not_found"))?;
    if summary.workspace.workspace_id.as_deref() != Some(workspace_id) {
        return Err(fail(403, "access_denied"));
    }
    if let Some(grant) = api
        .store
        .current_workspace_config_grant(workspace_id, &worker)
        .map_err(pre_error)?
    {
        if grant.access != request.access {
            return Err(fail(409, "grant_exists_revoke_first"));
        }
        return Ok(grant);
    }
    let grant = WorkspaceConfigGrantResponse {
        grant_id: Uuid::now_v7().to_string(),
        workspace_id: workspace_id.into(),
        runtime_id: worker.runtime_id,
        worker_id: worker.worker_id,
        working_directory_id: Uuid::now_v7().to_string(),
        access: request.access,
        revoked: false,
    };
    api.store
        .create_workspace_config_grant(&grant, &actor.account_id)
        .map_err(mutation_error)?;
    Ok(grant)
}
pub(super) async fn revoke_grant(
    api: &WorkspaceApi,
    actor: &RequestActor,
    workspace_id: &str,
    grant_id: &str,
) -> ConfigResult<WorkspaceConfigGrantResponse> {
    require_workspace_owner(api, workspace_id, actor, "Workspace config grants")
        .await
        .map_err(pre_error)?;
    let mut grant = api
        .store
        .get_workspace_config_grant(workspace_id, grant_id)
        .map_err(pre_error)?
        .ok_or_else(|| fail(404, "not_found"))?;
    let worker = RuntimeWorkerRef::new(&grant.runtime_id, &grant.worker_id);
    let _guard = lock(api, &worker).await?;
    api.store
        .revoke_workspace_config_grant(workspace_id, grant_id)
        .map_err(mutation_error)?;
    grant.revoked = true;
    // Revocation is durable before unlink; a failed cleanup cannot restore access.
    if let Some(link) = api
        .store
        .list_workdir_worker_links(workspace_id, &grant.working_directory_id)
        .map_err(|_| unknown())?
        .into_iter()
        .find(|link| link.unlinked_at.is_none() && link.worker == worker)
    {
        api.store
            .detach_worker_workdir_connection(
                workspace_id,
                &worker,
                &link.alias,
                &link.connection_id,
                &now_registry_timestamp(),
            )
            .map_err(|_| unknown())?;
    }
    api.worker_projection
        .refresh(&worker)
        .map_err(|_| unknown())?;
    Ok(grant)
}

pub(super) fn summary(
    grant: &WorkspaceConfigGrantResponse,
    record: &WorkdirRegistryRecord,
) -> server_api::WorkingDirectorySummary {
    server_api::WorkingDirectorySummary {
        working_directory_id: record.workdir_id.clone(),
        display_name: Some("Workspace configuration".into()),
        source: server_api::WorkingDirectorySource::WorkspaceConfig {
            access: grant.access,
            content_path: "/workspace-config".into(),
            purpose: "Workspace configuration".into(),
        },
        creation_selector: None,
        creation_ref: None,
        creation_tree: None,
        current_selector: None,
        current_ref: None,
        current_tree: None,
        observed_at_epoch_seconds: None,
        materializer_kind: server_api::WorkingDirectoryMaterializerKind::LogicalWorkspaceConfig,
        cleanup_target: None,
        status: if grant.revoked {
            server_api::WorkingDirectoryStatusKind::NotFound
        } else {
            server_api::WorkingDirectoryStatusKind::Active
        },
        cleanliness: None,
        occupied_by: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_config_mutation_storage_failures_are_conservatively_unknown() {
        assert_eq!(
            mutation_error(Error::Sqlite(rusqlite::Error::InvalidQuery)).classification,
            Classification::Unknown
        );
        assert_eq!(
            mutation_error(Error::Store("private /host/storage acknowledgement".into()))
                .classification,
            Classification::Unknown
        );
        let denied = mutation_error(Error::WorkspacePermissionDenied(
            "secret permission context".into(),
        ));
        assert_eq!(denied.classification, Classification::NotCommitted);
        assert_eq!(denied.status, 403);
        assert!(!denied.message.contains("secret"));
    }
}
