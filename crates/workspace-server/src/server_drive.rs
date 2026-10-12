//! Authorized Workspace document API. Storage, CAS and receipts stay in workspace-drive.
//! HTTP upload consumes frames with a hard bound; DB publication occurs only after validation.
use super::*;
use futures::StreamExt;
use server_api::*;
use sha2::{Digest, Sha256};
use workspace_drive::{Drive, Mutation, NodeId};

fn mutation_actor(context: &ServerRequestContext) -> DriveResult<String> {
    if let Some(source) = &context.runtime_source {
        let worker = source.worker_id.as_ref().ok_or_else(|| failure(403))?;
        Ok(format!(
            "worker:{}",
            serde_json::to_string(&(&source.runtime_id, worker)).map_err(|_| failure(400))?
        ))
    } else {
        Ok(format!(
            "user:{}",
            context.actor.as_ref().ok_or_else(|| failure(403))?.user_id
        ))
    }
}

type DriveResult<T> = std::result::Result<T, DriveApiError>;
const TRANSFER_CHUNK: usize = 64 * 1024;

pub(super) fn failure(status: u16) -> DriveApiError {
    let code = match status {
        401 | 403 => DriveApiErrorCode::Denied,
        404 => DriveApiErrorCode::NotFound,
        409 => DriveApiErrorCode::Conflict,
        413 => DriveApiErrorCode::Limit,
        500..=599 => DriveApiErrorCode::StorageUnavailable,
        _ => DriveApiErrorCode::Invalid,
    };
    DriveApiError::new(code)
}
fn domain_error(error: workspace_drive::Error, mutation: bool) -> DriveApiError {
    use workspace_drive::Error;
    match error {
        Error::Denied => failure(403),
        Error::NotFound => failure(404),
        Error::Conflict | Error::Fenced => failure(409),
        Error::Invalid(_) => failure(400),
        Error::Blob(_) => failure(503),
        _ if mutation => DriveApiError::new(DriveApiErrorCode::OutcomeUnknown),
        _ => failure(503),
    }
}
async fn blocking<T: Send + 'static>(
    mutation: bool,
    action: impl FnOnce() -> workspace_drive::Result<T> + Send + 'static,
) -> DriveResult<T> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(|_| {
            if mutation {
                DriveApiError::new(DriveApiErrorCode::OutcomeUnknown)
            } else {
                failure(503)
            }
        })?
        .map_err(|error| domain_error(error, mutation))
}
fn node_id(value: &str) -> DriveResult<NodeId> {
    value.to_owned().try_into().map_err(|_| failure(400))
}
fn last_mutation_id(value: &str) -> DriveResult<String> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(failure(400));
    }
    Ok(value.to_owned())
}
fn entry(workspace: &str, reference: &DriveEntryRef) -> DriveResult<NodeId> {
    if reference.workspace_id != workspace {
        return Err(failure(403));
    }
    node_id(&reference.node_id)
}
fn scoped_id(workspace: &str, entry_workspace: &str, id: &str) -> DriveResult<NodeId> {
    if workspace != entry_workspace {
        return Err(failure(403));
    }
    node_id(id)
}
fn safe_content_type(value: &str) -> DriveResult<String> {
    // Deliberately accept a bare MIME type only, not attacker-controlled header parameters.
    if value.len() > 128
        || !value.contains('/')
        || value.matches('/').count() != 1
        || value.split('/').any(str::is_empty)
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-/".contains(&b))
    {
        return Err(failure(400));
    }
    Ok(value.to_ascii_lowercase())
}
fn disposition(name: &str) -> String {
    // ASCII fallback plus RFC 5987 UTF-8 filename. Never place raw names in headers.
    let encoded: String = name
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"download\"; filename*=UTF-8''{encoded}")
}
fn error_response(error: DriveApiError) -> Response {
    let status = match error.code {
        DriveApiErrorCode::Denied => 403,
        DriveApiErrorCode::NotFound => 404,
        DriveApiErrorCode::Conflict => 409,
        DriveApiErrorCode::Limit => 413,
        DriveApiErrorCode::Invalid => 400,
        DriveApiErrorCode::StorageUnavailable | DriveApiErrorCode::OutcomeUnknown => 503,
    };
    (StatusCode::from_u16(status).unwrap(), Json(error)).into_response()
}
// No raw storage diagnostic is ever placed in a public error or stream failure.
fn stream_error() -> std::io::Error {
    std::io::Error::other("Drive transfer unavailable")
}

/// Stream HTTP frames while enforcing both announced and observed limits. Storage currently
/// accepts a bounded whole-file Vec; this is binary buffering, never JSON/base64 expansion.
async fn receive(body: axum::body::Body, declared: u64, digest: &str) -> DriveResult<Vec<u8>> {
    if declared > workspace_drive::MAX_FILE_BYTES as u64 {
        return Err(failure(413));
    }
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure(400));
    }
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    let mut hasher = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| failure(400))?;
        let size = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| failure(413))?;
        if size > workspace_drive::MAX_FILE_BYTES {
            return Err(failure(413));
        }
        if size > declared as usize {
            return Err(failure(400));
        }
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != declared as usize
        || hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            != digest
    {
        return Err(failure(400));
    }
    Ok(bytes)
}

fn public_node(node: workspace_drive::Node) -> DriveEntry {
    let workspace_id = node.workspace_id.clone();
    let time = chrono::DateTime::from_timestamp_millis(node.updated_at_ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    DriveEntry {
        entry: DriveEntryRef {
            workspace_id: workspace_id.clone(),
            node_id: node.id.get().to_string(),
        },
        parent: node.parent_id.map(|id| DriveEntryRef {
            workspace_id,
            node_id: id.get().to_string(),
        }),
        name: node.name,
        kind: match node.kind {
            workspace_drive::Kind::File => DriveEntryKind::File,
            workspace_drive::Kind::Directory => DriveEntryKind::Folder,
        },
        last_mutation_id: node.last_mutation_id.clone(),
        size: (node.kind == workspace_drive::Kind::File).then_some(node.size as u32),
        content_type: node.content_type,
        updated_by: node.updated_by,
        updated_at: time,
        latest_url: format!(
            "/api/w/{}/drive/{}?entry_workspace_id={}&id={}",
            node.workspace_id,
            if node.kind == workspace_drive::Kind::File {
                "download"
            } else {
                "metadata"
            },
            node.workspace_id,
            node.id.get()
        ),
    }
}
fn cursor(workspace: &str, id: Option<NodeId>) -> Option<String> {
    id.map(|id| format!("{workspace}:{}", id.get()))
}
fn after(workspace: &str, value: Option<String>) -> DriveResult<Option<NodeId>> {
    value
        .map(|value| {
            let (scope, id) = value.rsplit_once(':').ok_or_else(|| failure(400))?;
            scoped_id(workspace, scope, id)
        })
        .transpose()
}
fn limit(value: Option<u32>, maximum: usize) -> DriveResult<usize> {
    let limit = value.unwrap_or(maximum as u32) as usize;
    if limit == 0 || limit > maximum {
        return Err(failure(413));
    }
    Ok(limit)
}
pub(super) async fn root(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
) -> DriveResult<DriveEntry> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    blocking(false, move || drive.root()).await.map(public_node)
}
pub(super) async fn metadata(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveEntryQuery,
) -> DriveResult<DriveEntry> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = scoped_id(workspace, &query.entry_workspace_id, &query.id)?;
    blocking(false, move || drive.metadata(id))
        .await
        .map(public_node)
}
pub(super) async fn list(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveListQuery,
) -> DriveResult<DriveListResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = scoped_id(workspace, &query.entry_workspace_id, &query.id)?;
    let after = after(workspace, query.after)?;
    let limit = limit(query.limit, workspace_drive::MAX_PAGE)?;
    let page = blocking(false, move || drive.list(id, after, limit)).await?;
    Ok(DriveListResponse {
        entries: page.nodes.into_iter().map(public_node).collect(),
        next_after: cursor(workspace, page.next_after),
    })
}
pub(super) async fn search(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveSearchQuery,
) -> DriveResult<DriveListResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let after = after(workspace, query.after)?;
    let limit = limit(query.limit, workspace_drive::MAX_SEARCH_NODES)?;
    let page = blocking(false, move || {
        drive.search(&query.query, query.include_text, after, limit)
    })
    .await?;
    Ok(DriveListResponse {
        entries: page.nodes.into_iter().map(public_node).collect(),
        next_after: cursor(workspace, page.next_after),
    })
}
pub(super) async fn read_text(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveReadTextQuery,
) -> DriveResult<DriveReadTextResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = scoped_id(workspace, &query.entry_workspace_id, &query.id)?;
    let max = query.max_bytes as usize;
    if max == 0 || max > DRIVE_TEXT_MAX_BYTES {
        return Err(failure(413));
    }
    let (node, chunk) = blocking(false, move || {
        let node = drive.metadata(id)?;
        let chunk = drive.read(id, &node.last_mutation_id, 0, max)?;
        Ok((node, chunk))
    })
    .await?;
    let text = match std::str::from_utf8(&chunk.bytes) {
        Ok(text) => text,
        Err(error) if !chunk.eof && error.error_len().is_none() => {
            std::str::from_utf8(&chunk.bytes[..error.valid_up_to()]).map_err(|_| failure(400))?
        }
        Err(_) => return Err(failure(400)),
    };
    Ok(DriveReadTextResponse {
        entry: public_node(node),
        text: text.into(),
        truncated: !chunk.eof,
    })
}
pub(super) async fn read_chunk(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveReadChunkQuery,
) -> DriveResult<BinaryBody> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = scoped_id(workspace, &query.entry_workspace_id, &query.id)?;
    let last_mutation_id = last_mutation_id(&query.expected_mutation_id)?;
    if query.length == 0 || query.length > DRIVE_CHUNK_MAX_BYTES {
        return Err(failure(413));
    }
    let chunk = blocking(false, move || {
        drive.read(
            id,
            &last_mutation_id,
            query.offset.into(),
            query.length as usize,
        )
    })
    .await?;
    Ok(chunk.bytes.into())
}
fn text_bytes(text: String) -> DriveResult<Vec<u8>> {
    if text.len() > DRIVE_TEXT_MAX_BYTES {
        return Err(failure(413));
    }
    Ok(text.into_bytes())
}
fn mutation(workspace: &str, mutation: DriveMutation) -> DriveResult<Mutation> {
    Ok(match mutation {
        DriveMutation::CreateFolder { parent, name } => Mutation::CreateFolder {
            parent: entry(workspace, &parent)?,
            name,
        },
        DriveMutation::CreateText {
            parent,
            name,
            text,
            content_type,
        } => Mutation::CreateFile {
            parent: entry(workspace, &parent)?,
            name,
            content_type: safe_content_type(&content_type)?,
            bytes: text_bytes(text)?,
        },
        DriveMutation::UpdateText {
            id,
            expected_mutation_id,
            text,
            content_type,
        } => Mutation::Update {
            id: entry(workspace, &id)?,
            expected_mutation_id: last_mutation_id(&expected_mutation_id)?,
            content_type: safe_content_type(&content_type)?,
            bytes: text_bytes(text)?,
        },
        DriveMutation::Relocate {
            id,
            expected_mutation_id,
            parent,
            name,
        } => Mutation::Relocate {
            id: entry(workspace, &id)?,
            expected_mutation_id: last_mutation_id(&expected_mutation_id)?,
            parent: entry(workspace, &parent)?,
            name,
        },
        DriveMutation::Delete {
            id,
            expected_mutation_id,
        } => Mutation::Delete {
            id: entry(workspace, &id)?,
            expected_mutation_id: last_mutation_id(&expected_mutation_id)?,
        },
    })
}
fn result(request_id: String, result: workspace_drive::MutationResult) -> DriveMutationResponse {
    DriveMutationResponse {
        request_id,
        entry: (!result.deleted).then(|| public_node(result.node)),
    }
}
pub(super) async fn mutate(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    request: DriveMutationRequest,
) -> DriveResult<DriveMutationResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, true).await?;
    let actor = mutation_actor(context)?;
    let operation = mutation(workspace, request.mutation)?;
    let request_id = request.request_id.clone();
    let mutated = blocking(true, move || {
        drive.mutate(&request.request_id, &actor, operation)
    })
    .await?;
    Ok(result(request_id, mutated))
}
fn upload_mutation(
    workspace: &str,
    query: &DriveUploadQuery,
    bytes: Vec<u8>,
) -> DriveResult<Mutation> {
    if query.entry_workspace_id != workspace {
        return Err(failure(403));
    }
    let content_type = safe_content_type(&query.content_type)?;
    match query.operation {
        DriveUploadOperation::Create
            if query.id.is_none() && query.expected_mutation_id.is_none() =>
        {
            Ok(Mutation::CreateFile {
                parent: node_id(query.parent_id.as_deref().ok_or_else(|| failure(400))?)?,
                name: query.name.clone().ok_or_else(|| failure(400))?,
                content_type,
                bytes,
            })
        }
        DriveUploadOperation::Update if query.parent_id.is_none() && query.name.is_none() => {
            Ok(Mutation::Update {
                id: node_id(query.id.as_deref().ok_or_else(|| failure(400))?)?,
                expected_mutation_id: last_mutation_id(
                    query
                        .expected_mutation_id
                        .as_deref()
                        .ok_or_else(|| failure(400))?,
                )?,
                content_type,
                bytes,
            })
        }
        _ => Err(failure(400)),
    }
}
pub(super) async fn upload(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveUploadQuery,
    body: BinaryBody,
) -> DriveResult<DriveMutationResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, true).await?;
    let actor = mutation_actor(context)?;
    let bytes = receive(
        axum::body::Body::from(body.into_bytes()),
        query.size.into(),
        &query.sha256,
    )
    .await?;
    publish_upload(drive, actor, workspace, query, bytes).await
}
async fn publish_upload(
    drive: Drive,
    actor: String,
    workspace: &str,
    query: DriveUploadQuery,
    bytes: Vec<u8>,
) -> DriveResult<DriveMutationResponse> {
    let mutation = upload_mutation(workspace, &query, bytes)?;
    let request_id = query.request_id.clone();
    let mutated = blocking(true, move || {
        drive.mutate(&query.request_id, &actor, mutation)
    })
    .await?;
    Ok(result(request_id, mutated))
}
pub(super) async fn request_status(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    request_id: String,
) -> DriveResult<DriveRequestStatusResponse> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = request_id.clone();
    let status = blocking(false, move || drive.request_status(&id)).await?;
    Ok(match status {
        workspace_drive::RequestStatus::Uncommitted => DriveRequestStatusResponse {
            request_id,
            state: DriveRequestState::Uncommitted,
            response: None,
        },
        workspace_drive::RequestStatus::Committed { result: mutation } => {
            DriveRequestStatusResponse {
                request_id: request_id.clone(),
                state: DriveRequestState::Committed,
                response: Some(result(request_id, mutation)),
            }
        }
    })
}

async fn select_download(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveDownloadQuery,
) -> DriveResult<(Drive, workspace_drive::Node)> {
    let drive = drive_grants::authorized_drive(api, context, workspace, false).await?;
    let id = scoped_id(workspace, &query.entry_workspace_id, &query.id)?;
    let for_node = drive.clone();
    let node = blocking(false, move || for_node.metadata(id)).await?;
    if node.kind != workspace_drive::Kind::File {
        return Err(failure(400));
    }
    if query
        .expected_mutation_id
        .as_deref()
        .is_some_and(|v| v != node.last_mutation_id.clone())
    {
        return Err(failure(409));
    }
    safe_content_type(node.content_type.as_deref().unwrap_or("")).map_err(|_| failure(503))?;
    Ok((drive, node))
}
pub(super) async fn download(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    query: DriveDownloadQuery,
) -> DriveResult<server_api::server_api_responses::DriveDownload> {
    let (drive, node) = select_download(api, context, workspace, query).await?;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let copy = drive.clone();
        let node = node.clone();
        let chunk = blocking(false, move || {
            copy.read(node.id, &node.last_mutation_id, offset, TRANSFER_CHUNK)
        })
        .await?;
        offset += chunk.bytes.len() as u64;
        bytes.extend_from_slice(&chunk.bytes);
        if chunk.eof {
            break;
        }
    }
    Ok(server_api::server_api_responses::DriveDownload::Status200 {
        body: bytes.into(),
        header_content_disposition: disposition(&node.name),
        header_cache_control: "private, no-store".into(),
        header_etag: validator(&node),
        header_x_content_type_options: "nosniff".into(),
        header_content_security_policy: "sandbox; default-src 'none'".into(),
    })
}
fn validator(node: &workspace_drive::Node) -> String {
    // Request IDs are caller-chosen UTF-8, never interpolate them into HTTP headers.
    let mut digest = Sha256::new();
    digest.update(node.workspace_id.as_bytes());
    digest.update([0]);
    digest.update(node.id.get().to_be_bytes());
    digest.update(node.last_mutation_id.as_bytes());
    let hex: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("\"drive-{hex}\"")
}

pub(super) fn router(service: Arc<ServerApiContractService>) -> Router {
    Router::new()
        .merge(streaming_router(service.clone()))
        .merge(server_api::server_api_axum::drive_root(service.clone()))
        .merge(server_api::server_api_axum::drive_metadata(service.clone()))
        .merge(server_api::server_api_axum::drive_list(service.clone()))
        .merge(server_api::server_api_axum::drive_search(service.clone()))
        .merge(server_api::server_api_axum::drive_read_text(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::drive_read_chunk(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::drive_mutate(service.clone()))
        .merge(server_api::server_api_axum::drive_request_status(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::drive_grant_create(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::drive_grant_revoke(
            service.clone(),
        ))
        .merge(server_api::server_api_axum::drive_grant_list(
            service.clone(),
        ))
        .layer(DefaultBodyLimit::max(512 * 1024))
        .layer(middleware::from_fn(normalize_rejections))
}
async fn normalize_rejections(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if matches!(response.status().as_u16(), 415 | 422) {
        error_response(failure(400))
    } else if matches!(response.status().as_u16(), 400 | 413)
        && response
            .headers()
            .get(CONTENT_TYPE)
            .is_none_or(|value| value != "application/json")
    {
        error_response(failure(response.status().as_u16()))
    } else {
        response
    }
}

fn streaming_router(service: Arc<ServerApiContractService>) -> Router {
    // The operation inventory is the route authority, not duplicated string literals.
    use api_macros::Operation;
    use axum::routing::put;
    use server_api::server_api_operations::{DriveDownload, DriveUpload};
    Router::new()
        .route(DriveUpload::METADATA.path, put(streaming_upload))
        .route(DriveDownload::METADATA.path, get(streaming_download))
        .with_state(service)
}
async fn streaming_upload(
    State(service): State<Arc<ServerApiContractService>>,
    AxumPath(workspace): AxumPath<String>,
    Extension(context): Extension<ServerRequestContext>,
    query: std::result::Result<Query<DriveUploadQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let result = async {
        let api = service.workspace_api().map_err(|_| failure(404))?;
        let drive = drive_grants::authorized_drive(api, &context, &workspace, true).await?;
        let actor = mutation_actor(&context)?;
        let Query(query) = query.map_err(|_| failure(400))?;
        // Reject malformed metadata, foreign refs and over-limit declarations before reading.
        upload_mutation(&workspace, &query, Vec::new())?;
        if query.size as usize > workspace_drive::MAX_FILE_BYTES {
            return Err(failure(413));
        }
        if let Some(length) = request.headers().get(axum::http::header::CONTENT_LENGTH) {
            let length: u64 = length
                .to_str()
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| failure(400))?;
            if length > workspace_drive::MAX_FILE_BYTES as u64 {
                return Err(failure(413));
            }
            if length != u64::from(query.size) {
                return Err(failure(400));
            }
        }
        let bytes = receive(request.into_body(), query.size.into(), &query.sha256).await?;
        // The bound Drive authorizer runs again under attached-authority IMMEDIATE,
        // so a grant revoked during reception cannot publish or replay a receipt.
        publish_upload(drive, actor, &workspace, query, bytes).await
    }
    .await;
    match result {
        Ok(result) => Json(result).into_response(),
        Err(error) => error_response(error),
    }
}
async fn streaming_download(
    State(service): State<Arc<ServerApiContractService>>,
    AxumPath(workspace): AxumPath<String>,
    Extension(context): Extension<ServerRequestContext>,
    query: std::result::Result<Query<DriveDownloadQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let result = async {
        let api = service.workspace_api().map_err(|_| failure(404))?;
        let Query(query) = query.map_err(|_| failure(400))?;
        let (drive, node) = select_download(api, &context, &workspace, query).await?;
        // Validate the referenced blob before HTTP success, including a zero-byte file.
        let first_drive = drive.clone();
        let selected = node.clone();
        let first = blocking(false, move || {
            first_drive.read(selected.id, &selected.last_mutation_id, 0, TRANSFER_CHUNK)
        })
        .await?;
        let id = node.id;
        let last_mutation_id = node.last_mutation_id.clone();
        let state = (drive, id, last_mutation_id, Some(first), 0_u64, false);
        let stream = futures::stream::try_unfold(
            state,
            |(drive, id, last_mutation_id, first, offset, done)| async move {
                if done {
                    return Ok(None);
                }
                let chunk = match first {
                    Some(chunk) => chunk,
                    None => {
                        let copy = drive.clone();
                        let expected = last_mutation_id.clone();
                        blocking(false, move || {
                            copy.read(id, &expected, offset, TRANSFER_CHUNK)
                        })
                        .await
                        .map_err(|_| stream_error())?
                    }
                };
                let next = offset + chunk.bytes.len() as u64;
                let eof = chunk.eof;
                Ok::<_, std::io::Error>(Some((
                    axum::body::Bytes::from(chunk.bytes),
                    (drive, id, last_mutation_id, None, next, eof),
                )))
            },
        );
        let mut response = axum::body::Body::from_stream(stream).into_response();
        let headers = response.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&safe_content_type(
                node.content_type.as_deref().unwrap_or(""),
            )?)
            .map_err(|_| failure(503))?,
        );
        headers.insert(
            axum::http::header::CONTENT_LENGTH,
            HeaderValue::from_str(&node.size.to_string()).map_err(|_| failure(503))?,
        );
        headers.insert(
            axum::http::header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition(&node.name)).map_err(|_| failure(503))?,
        );
        headers.insert(
            axum::http::header::ETAG,
            HeaderValue::from_str(&validator(&node)).map_err(|_| failure(503))?,
        );
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        headers.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
        Ok::<_, DriveApiError>(response)
    }
    .await;
    match result {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

#[cfg(test)]
mod validator_tests {
    use super::*;

    #[test]
    fn request_identity_is_hashed_before_becoming_an_http_validator() {
        let mut node = workspace_drive::Node {
            id: "2".to_string().try_into().unwrap(),
            workspace_id: "workspace".into(),
            parent_id: Some("1".to_string().try_into().unwrap()),
            name: "document".into(),
            kind: workspace_drive::Kind::File,
            last_mutation_id: "write/資料 ?\"#&=+".into(),
            size: 0,
            content_type: Some("text/plain".into()),
            updated_by: "actor".into(),
            updated_at_ms: 0,
        };
        let first = validator(&node);
        assert!(first.parse::<axum::http::HeaderValue>().is_ok());
        assert!(!first.contains(&node.last_mutation_id));
        assert_eq!(first, validator(&node));
        node.last_mutation_id = "another-request".into();
        assert_ne!(first, validator(&node));
    }
}
