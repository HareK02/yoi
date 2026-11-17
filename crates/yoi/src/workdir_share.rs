use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use client::BackendApiClient;
use error_contract::{FailureFacts, Tool, project_failure};
use futures::{SinkExt, StreamExt};
use server_api::client_support::{ClientError, ClientFailure};
use server_api::{
    ExternalWorkdirGrantCreateRequest, ExternalWorkdirGrantResponse, ServerApiClient,
};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};
use workdir::external::{
    ExternalProviderInstanceId, ExternalWorkdirGrantId, ExternalWorkdirOperationError,
    ExternalWorkdirOperationId, ExternalWorkdirOperationOutcome, ExternalWorkdirProviderFrame,
    ExternalWorkdirProviderMessage, ExternalWorkdirProviderRegistration,
    ExternalWorkdirServerFrame, ExternalWorkdirServerMessage,
};
use workdir::failure::{
    WorkdirFailure, WorkdirFailureDetails, WorkdirFailureOperation, WorkdirFailureVisibility,
};
use workdir::http::{WorkdirTransportError, dispatch_workdir_session_operation};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation, WorkdirManagementVisibility,
};
use workdir::{
    BoundedReadLimits, ExternalWorkdirRoot, LocalWorkdirSession, Workdir, WorkdirSession,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkdirShareOptions {
    pub path: PathBuf,
    pub workspace_name: Option<String>,
    pub backend_url: String,
    pub display_name: String,
    pub ttl: Option<Duration>,
    pub permissions: server_api::ExternalWorkdirPermissions,
    pub non_interactive: bool,
}

const PROVIDER_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// CLI-local carriers: received errors, settlements and secondary failures are
/// retained until the outer display boundary. None of these authorize replay.
#[derive(Debug)]
enum ShareCause {
    Local(String),
    Grant {
        operation: WorkdirManagementOperation,
        error: ClientError<server_api::WorkdirManagementApiError>,
    },
    Workdir(WorkdirFailure),
    ProviderEnded(ProviderEnd),
}

#[derive(Debug)]
struct ShareError {
    cause: ShareCause,
    secondary: Vec<ShareError>,
}

impl From<String> for ShareError {
    fn from(message: String) -> Self {
        Self {
            cause: ShareCause::Local(message),
            secondary: Vec::new(),
        }
    }
}

impl ShareError {
    fn grant(
        operation: WorkdirManagementOperation,
        error: ClientError<server_api::WorkdirManagementApiError>,
    ) -> Self {
        Self {
            cause: ShareCause::Grant { operation, error },
            secondary: Vec::new(),
        }
    }

    fn workdir(operation: WorkdirFailureOperation, error: workdir::WorkdirError) -> Self {
        Self {
            cause: ShareCause::Workdir(WorkdirFailure::new(operation, error)),
            secondary: Vec::new(),
        }
    }

    fn with_secondary(mut self, secondary: ShareError) -> Self {
        self.secondary.push(secondary);
        self
    }

    fn same_observation(&self, other: &Self) -> bool {
        let same_cause = match (&self.cause, &other.cause) {
            (ShareCause::ProviderEnded(a), ShareCause::ProviderEnded(b)) => a == b,
            (ShareCause::Local(a), ShareCause::Local(b)) => a == b,
            (ShareCause::Workdir(a), ShareCause::Workdir(b)) => a.details() == b.details(),
            (
                ShareCause::Grant {
                    operation: a_op,
                    error: a,
                },
                ShareCause::Grant {
                    operation: b_op,
                    error: b,
                },
            ) if a_op == b_op => match (a, b) {
                (ClientError::Failure(a), ClientError::Failure(b)) => a == b,
                (
                    ClientError::Public {
                        status: a_status,
                        error: a,
                    },
                    ClientError::Public {
                        status: b_status,
                        error: b,
                    },
                ) => {
                    a_status == b_status
                        && a.details() == b.details()
                        && a.legacy().failure == b.legacy().failure
                        && a.legacy().error == b.legacy().error
                        && a.legacy().message == b.legacy().message
                }
                _ => false,
            },
            _ => false,
        };
        same_cause
            && self.secondary.len() == other.secondary.len()
            && self
                .secondary
                .iter()
                .zip(&other.secondary)
                .all(|(a, b)| a.same_observation(b))
    }

    fn prevents_reconnect(&self) -> bool {
        matches!(
            &self.cause,
            ShareCause::Grant {
                operation: WorkdirManagementOperation::GrantCreate
                    | WorkdirManagementOperation::GrantRevoke,
                ..
            } | ShareCause::Workdir(_)
        ) || !self.secondary.is_empty()
    }

    fn render(&self) -> String {
        let mut message = match &self.cause {
            ShareCause::ProviderEnded(end) => {
                format!("External Workdir provider ended ({end:?}); supplementary shutdown failed")
            }
            ShareCause::Local(message) => message.clone(),
            ShareCause::Workdir(failure) => FailureFacts::new(*failure.details())
                .and_then(|facts| {
                    project_failure::<Tool, _>(
                        &facts,
                        &WorkdirFailureVisibility {
                            can_view_cause: true,
                        },
                    )
                })
                .map(|projected| projected.explanation().to_string())
                .unwrap_or_else(|_| "Workdir diagnostic projection unavailable".into()),
            ShareCause::Grant { operation, error } => {
                let context = match operation {
                    WorkdirManagementOperation::GrantCreate => {
                        "External Workdir grant request failed"
                    }
                    WorkdirManagementOperation::GrantInspect => {
                        "External Workdir grant refresh failed"
                    }
                    WorkdirManagementOperation::GrantRevoke => {
                        "External Workdir revoke was not confirmed by the Backend"
                    }
                    WorkdirManagementOperation::GrantConnect => {
                        "External Workdir provider connection failed"
                    }
                    _ => "External Workdir grant operation failed",
                };
                let explanation = match error {
                    ClientError::Public { status, error } => {
                        let projected = error.details().and_then(|details| {
                            FailureFacts::new(*details).ok().and_then(|facts| {
                                project_failure::<Tool, _>(
                                    &facts,
                                    &WorkdirManagementVisibility {
                                        can_view_cause: true,
                                    },
                                )
                                .ok()
                            })
                        });
                        projected.map(|value| value.explanation().to_string())
                            .or_else(|| error.legacy().failure.as_ref().and_then(management_attachment_explanation))
                            .unwrap_or_else(|| format!("Backend rejected the request (HTTP {}); typed cause unavailable", status.as_u16()))
                    }
                    ClientError::Failure(failure) => {
                        // ClientFailure is a closed, body/path-free transport observation.
                        // A failed mutation response is not evidence that it was not applied.
                        let mut text = failure.to_string();
                        if matches!(
                            operation,
                            WorkdirManagementOperation::GrantCreate
                                | WorkdirManagementOperation::GrantRevoke
                        ) {
                            text.push_str("; mutation result is unconfirmed; inspect existing state; do not automatically resend");
                        }
                        text
                    }
                };
                format!("{context}: {explanation}")
            }
        };
        for secondary in &self.secondary {
            message.push_str("; supplementary failure (does not replace the original result): ");
            message.push_str(&secondary.render());
        }
        message
    }
}

fn management_attachment_explanation(
    attachment: &protocol::startup_failure::StartupFailureAttachment,
) -> Option<String> {
    attachment
        .project_for::<Tool, WorkdirManagementFailure>(&WorkdirManagementVisibility {
            can_view_cause: true,
        })
        .ok()
        .map(|projected| projected.explanation().to_string())
}

fn cleanup_attachment_explanation(
    attachment: &protocol::startup_failure::StartupFailureAttachment,
) -> String {
    attachment.project_for::<Tool, WorkdirFailureDetails>(&WorkdirFailureVisibility { can_view_cause: true })
        .ok().map(|projected| projected.explanation().to_string())
        .or_else(|| management_attachment_explanation(attachment))
        .unwrap_or_else(|| "supplementary diagnostic unavailable or unsupported; original grant status is unchanged".into())
}

#[derive(Default)]
struct ShareEvidence {
    grants: Vec<ExternalWorkdirGrantResponse>,
    failures: Vec<(ShareError, u64)>,
}

impl ShareEvidence {
    fn retain_grant(&mut self, grant: ExternalWorkdirGrantResponse) {
        if self.grants.last() != Some(&grant) {
            self.grants.push(grant);
        }
    }

    fn retain_failure(&mut self, error: ShareError) {
        if let Some((_, count)) = self
            .failures
            .iter_mut()
            .find(|(existing, _)| existing.same_observation(&error))
        {
            *count = count.saturating_add(1);
        } else {
            self.failures.push((error, 1));
        }
    }

    fn render(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for grant in &self.grants {
            let status = match grant.status.as_str() {
                "pending" | "online" | "offline" | "revoked" | "expired" => grant.status.as_str(),
                _ => "unrecognized",
            };
            // Never infer grant authority from cleanup completion (or vice versa).
            lines.push(format!("External Workdir grant status: {status}"));
            let cleanup = match grant.cleanup_state.as_str() {
                "not_required" | "pending" | "retry_required" | "completed" | "unconfirmed" => {
                    grant.cleanup_state.as_str()
                }
                _ => "unrecognized",
            };
            lines.push(format!(
                "Supplementary cleanup state: {cleanup} (independent of grant status)"
            ));
            for failure in &grant.cleanup_failures {
                lines.push(format!(
                    "Supplementary cleanup/audit/persistence failure: {}",
                    cleanup_attachment_explanation(failure)
                ));
            }
        }
        lines.extend(self.failures.iter().map(|(error, count)| {
            let mut rendered = error.render();
            if *count > 1 {
                rendered.push_str(&format!(
                    " (observed {count} times; no mutation replay authority)"
                ));
            }
            rendered
        }));
        lines
    }
}

fn provider_connection_error(error: tokio_tungstenite::tungstenite::Error) -> ShareError {
    // The handshake can carry the same typed Problem Details as grant CRUD.
    // Decode it through the existing bounded API hook; never display the HTTP
    // body, headers, URL, or the tungstenite error's Display implementation.
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        let status = response.status();
        let mut content_types = response
            .headers()
            .get_all(reqwest::header::CONTENT_TYPE)
            .iter();
        let content_type = content_types.next().and_then(|value| value.to_str().ok());
        let decoded = if content_types.next().is_none() {
            server_api::WorkdirManagementApiError::decode_received(
                status.as_u16(),
                content_type.unwrap_or(""),
                response.body().as_deref().unwrap_or_default(),
            )
        } else {
            Err(ClientFailure::ErrorResponseDecode {
                status: status.as_u16(),
            })
        };
        let error = match decoded {
            Ok(error) => ClientError::public(status, error),
            Err(failure) => ClientError::Failure(failure),
        };
        return ShareError::grant(WorkdirManagementOperation::GrantConnect, error);
    }
    "External Workdir provider connection failed; inspect the current grant and connection"
        .to_string()
        .into()
}

fn retain_cleanup<T>(
    original: Result<T, ShareError>,
    cleanup: Result<(), ShareError>,
) -> Result<T, ShareError> {
    match (original, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(error.with_secondary(cleanup)),
    }
}

fn retain_provider_cleanup(
    original: Result<ProviderEnd, ShareError>,
    cleanup: Result<(), ShareError>,
) -> Result<ProviderEnd, ShareError> {
    match (original, cleanup) {
        (Ok(end), Err(cleanup)) => Err(ShareError {
            cause: ShareCause::ProviderEnded(end),
            secondary: vec![cleanup],
        }),
        (original, cleanup) => retain_cleanup(original, cleanup),
    }
}

fn validate_provider_registration(message: Message, generation: u64) -> Result<(), String> {
    let registered = match message {
        Message::Text(text) => text,
        Message::Close(_) => {
            // Close reasons are arbitrary server/provider text, not typed evidence.
            return Err("Backend rejected the External Workdir provider registration; inspect the current grant".to_string());
        }
        _ => {
            return Err(
                "Backend returned an invalid External Workdir registration frame".to_string(),
            );
        }
    };
    let frame = serde_json::from_str::<ExternalWorkdirServerFrame>(&registered)
        .map_err(|_| "Backend returned an invalid External Workdir frame".to_string())?;
    if matches!(
        frame.message,
        ExternalWorkdirServerMessage::Registered { generation: registered_generation, .. }
            if registered_generation == generation
    ) {
        Ok(())
    } else {
        Err("Backend rejected the External Workdir provider registration".to_string())
    }
}

fn provider_operation_timeout(operation: &workdir::http::WorkdirSessionOperation) -> Duration {
    if matches!(
        operation,
        workdir::http::WorkdirSessionOperation::CommandOutput(request) if request.wait
    ) {
        Duration::from_secs(workdir::external::MAX_EXTERNAL_COMMAND_TIMEOUT_SECS + 10)
    } else {
        PROVIDER_OPERATION_TIMEOUT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderEnd {
    Disconnected,
    Revoked,
    Interrupted,
}

struct OperationCompletion {
    operation_id: ExternalWorkdirOperationId,
    outcome: ExternalWorkdirOperationOutcome,
}

struct OperationTask {
    cancelled: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OperationTask {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn join(mut self) -> Result<bool, String> {
        let cancelled = self.is_cancelled();
        let thread = self
            .thread
            .take()
            .expect("operation task must retain its executor thread");
        thread
            .join()
            .map_err(|_| "External Workdir operation executor panicked".to_string())?;
        Ok(cancelled)
    }
}

impl Drop for OperationTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn pin_selected_root(path: &std::path::Path) -> Result<ExternalWorkdirRoot, ShareError> {
    let cwd = std::env::current_dir().map_err(|error| {
        ShareError::workdir(
            WorkdirFailureOperation::SessionOpen,
            workdir::WorkdirError::Io {
                path: std::path::PathBuf::from("<current-directory>"),
                source: error,
            },
        )
    })?;
    pin_selected_root_from(path, &cwd)
}

fn pin_selected_root_from(
    path: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<ExternalWorkdirRoot, ShareError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    ExternalWorkdirRoot::pin(absolute)
        .map_err(|error| ShareError::workdir(WorkdirFailureOperation::SessionOpen, error))
}

fn authenticated_server_api_client(
    backend_client: &BackendApiClient,
    backend_url: &str,
) -> Result<ServerApiClient, String> {
    // BackendApiClient remains the stored-credential authority used by the
    // manual provider WebSocket. Reuse that transport's Authorization header
    // for the generated JSON client instead of loading or interpreting the
    // credential a second time here.
    let authenticated_request = backend_client
        .websocket_request("/")
        .map_err(|error| error.to_string())?;
    let authorization = authenticated_request
        .headers()
        .get(reqwest::header::AUTHORIZATION)
        .cloned()
        .ok_or_else(|| "stored Backend credential did not provide Authorization".to_string())?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, authorization);
    let http_client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("failed to configure Backend client: {error}"))?;
    let base_url = backend_url
        .try_into()
        .map_err(|error| format!("invalid Backend URL: {error}"))?;
    Ok(ServerApiClient::with_client(base_url, http_client))
}

fn permissions_label(permissions: server_api::ExternalWorkdirPermissions) -> &'static str {
    match permissions.grant_level() {
        Some(server_api::ExternalWorkdirPermissionLevel::Read) => "READ",
        Some(server_api::ExternalWorkdirPermissionLevel::Write) => "WRITE (includes READ)",
        Some(server_api::ExternalWorkdirPermissionLevel::Command) => {
            "COMMAND (includes WRITE + READ)"
        }
        None => "INVALID",
    }
}

fn validate_permissions(permissions: server_api::ExternalWorkdirPermissions) -> Result<(), String> {
    if permissions.grant_level().is_none() {
        return Err(
            "select exactly one permission level: READ, WRITE (includes READ), or COMMAND (includes WRITE + READ)"
                .to_string(),
        );
    }
    Ok(())
}

fn workdir_capabilities(
    permissions: server_api::ExternalWorkdirPermissions,
) -> workdir::WorkdirSessionCapabilities {
    match permissions.grant_level() {
        Some(server_api::ExternalWorkdirPermissionLevel::Read) => {
            workdir::WorkdirSessionCapabilities::READ_ONLY
        }
        Some(server_api::ExternalWorkdirPermissionLevel::Write) => {
            workdir::WorkdirSessionCapabilities::READ_WRITE
        }
        Some(server_api::ExternalWorkdirPermissionLevel::Command) => {
            workdir::WorkdirSessionCapabilities::ALL
        }
        None => workdir::WorkdirSessionCapabilities::EMPTY,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedWorkdirShareOptions {
    path: PathBuf,
    workspace_id: String,
    backend_url: String,
    display_name: String,
    ttl: Option<Duration>,
    permissions: server_api::ExternalWorkdirPermissions,
}

pub fn parse_ttl(value: &str) -> Result<Option<Duration>, String> {
    if matches!(value, "none" | "unlimited") {
        return Ok(None);
    }
    let (number, multiplier) = if let Some(value) = value.strip_suffix('h') {
        (value, 60 * 60)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1)
    } else {
        return Err("TTL must use an h, m, or s suffix (for example 1h)".to_string());
    };
    let amount = number
        .parse::<u64>()
        .map_err(|_| "TTL must contain a positive integer and h, m, or s suffix".to_string())?;
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| "TTL is too large".to_string())?;
    if !(60..=86_400).contains(&seconds) {
        return Err("TTL must be between 60s and 24h".to_string());
    }
    Ok(Some(Duration::from_secs(seconds)))
}

fn ttl_label(ttl: Option<Duration>) -> String {
    match ttl {
        None => "unlimited (ends on stop, disconnect, or revoke)".to_string(),
        Some(ttl) => format!("{}s", ttl.as_secs()),
    }
}

fn read_prompt_line(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> Result<Option<String>, String> {
    write!(output, "{prompt}").map_err(|error| format!("failed to write prompt: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("failed to flush prompt: {error}"))?;
    let mut line = String::new();
    let read = input
        .read_line(&mut line)
        .map_err(|error| format!("failed to read operator input: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

fn configure_interactively(
    mut options: WorkdirShareOptions,
    workspaces: &[server_api::WorkspaceSummary],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<ResolvedWorkdirShareOptions>, String> {
    if workspaces.is_empty() {
        return Err("Backend returned no accessible Workspaces".to_string());
    }
    writeln!(output, "Select a Workspace:")
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    for (index, workspace) in workspaces.iter().enumerate() {
        writeln!(
            output,
            "  {}) {} ({})",
            index + 1,
            workspace.display_name,
            workspace.workspace_id
        )
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    }
    let initial_index = options
        .workspace_name
        .as_deref()
        .map(|name| workspace_index_by_name(workspaces, name))
        .transpose()?
        .unwrap_or(0);
    let Some(selection) = read_prompt_line(
        input,
        output,
        &format!("Workspace [{}; q to cancel]: ", initial_index + 1),
    )?
    else {
        return Ok(None);
    };
    if matches!(selection.as_str(), "q" | "quit" | "cancel") {
        return Ok(None);
    }
    let selected_index = if selection.is_empty() {
        initial_index
    } else {
        selection
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .filter(|index| *index < workspaces.len())
            .ok_or_else(|| "Workspace selection is invalid".to_string())?
    };
    let selected_workspace = &workspaces[selected_index];

    writeln!(
        output,
        "Permission levels: READ (Read/Glob/Grep), WRITE (includes READ + Write/Edit), or COMMAND (includes WRITE + READ + host command lifecycle). COMMAND runs with this CLI user's host authority and is not sandboxed."
    )
    .map_err(|error| format!("failed to write prompt: {error}"))?;
    let Some(permission_input) = read_prompt_line(
        input,
        output,
        &format!(
            "Permission level [READ/WRITE/COMMAND; current {}; q to cancel]: ",
            permissions_label(options.permissions)
        ),
    )?
    else {
        return Ok(None);
    };
    if matches!(permission_input.as_str(), "q" | "quit" | "cancel") {
        return Ok(None);
    }
    if !permission_input.is_empty() {
        options.permissions = match permission_input.to_ascii_lowercase().as_str() {
            "r" | "read" | "read-only" | "read_only" => {
                server_api::ExternalWorkdirPermissions::READ
            }
            "w" | "write" | "read-write" | "read_write" => {
                server_api::ExternalWorkdirPermissions::WRITE
            }
            "c" | "command" => server_api::ExternalWorkdirPermissions::COMMAND,
            _ => {
                return Err(format!(
                    "unknown permission level `{permission_input}`; use READ, WRITE, or COMMAND"
                ));
            }
        };
    }

    writeln!(output, "TTL: {}", ttl_label(options.ttl))
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    let Some(ttl) = read_prompt_line(
        input,
        output,
        "TTL [empty keeps current; `none` is unlimited; q to cancel]: ",
    )?
    else {
        return Ok(None);
    };
    options.ttl = match ttl.as_str() {
        "" => options.ttl,
        "q" | "quit" | "cancel" => return Ok(None),
        value => parse_ttl(value)?,
    };

    let workspace_id = selected_workspace.workspace_id.clone();
    writeln!(output, "Share configuration:")
        .and_then(|()| writeln!(output, "  Workspace: {}", selected_workspace.display_name))
        .and_then(|()| {
            writeln!(
                output,
                "  Permissions: {}",
                permissions_label(options.permissions)
            )
        })
        .and_then(|()| writeln!(output, "  TTL: {}", ttl_label(options.ttl)))
        .map_err(|error| format!("failed to write confirmation: {error}"))?;
    let Some(confirm) = read_prompt_line(input, output, "Connect and create grant? [y/N]: ")?
    else {
        return Ok(None);
    };
    if !matches!(confirm.as_str(), "y" | "yes") {
        return Ok(None);
    }
    Ok(Some(ResolvedWorkdirShareOptions {
        path: options.path,
        workspace_id,
        backend_url: options.backend_url,
        display_name: options.display_name,
        ttl: options.ttl,
        permissions: options.permissions,
    }))
}

fn ensure_interactive_tty(stdin_is_tty: bool, stderr_is_tty: bool) -> Result<(), String> {
    if stdin_is_tty && stderr_is_tty {
        Ok(())
    } else {
        Err(
            "interactive sharing requires a TTY; use --non-interactive with --workspace-name for automation"
                .to_string(),
        )
    }
}

const SHARE_WORKSPACE_LIMIT: u32 = 200;

fn workspace_index_by_name(
    workspaces: &[server_api::WorkspaceSummary],
    name: &str,
) -> Result<usize, String> {
    if name.trim().is_empty() {
        return Err("--workspace-name must not be blank".into());
    }
    // A full bounded catalog cannot establish uniqueness beyond the returned rows.
    if workspaces.len() >= SHARE_WORKSPACE_LIMIT as usize {
        return Err("Workspace catalog limit reached; name uniqueness could not be confirmed. Use interactive selection or narrow the accessible catalog.".into());
    }
    let mut matches = workspaces
        .iter()
        .enumerate()
        .filter(|(_, workspace)| workspace.display_name == name);
    let Some((index, _)) = matches.next() else {
        return Err(format!(
            "No accessible Workspace has the exact name {name:?} on this Backend"
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "Workspace name {name:?} is ambiguous on this Backend; use interactive selection or give the Workspaces distinct names"
        ));
    }
    Ok(index)
}

fn configure_non_interactively(
    options: WorkdirShareOptions,
    workspaces: &[server_api::WorkspaceSummary],
) -> Result<ResolvedWorkdirShareOptions, String> {
    let name = options.workspace_name.as_deref().ok_or_else(||
        "non-interactive sharing requires --workspace-name; no Workspace is inferred from cwd or path".to_string())?;
    let workspace_id = workspaces[workspace_index_by_name(workspaces, name)?]
        .workspace_id
        .clone();
    Ok(ResolvedWorkdirShareOptions {
        path: options.path,
        workspace_id,
        backend_url: options.backend_url,
        display_name: options.display_name,
        ttl: options.ttl,
        permissions: options.permissions,
    })
}

async fn load_share_workspaces(
    client: &ServerApiClient,
) -> Result<Vec<server_api::WorkspaceSummary>, String> {
    client
        .workspace_catalog_list(server_api::WorkspaceListQuery {
            limit: Some(SHARE_WORKSPACE_LIMIT),
        })
        .await
        .map(|response| response.0)
        .map_err(|error| format!("failed to query Backend Workspace catalog: {error}"))
}

async fn resolve_share_options(
    options: WorkdirShareOptions,
) -> Result<Option<ResolvedWorkdirShareOptions>, String> {
    validate_permissions(options.permissions)?;
    if options.non_interactive && options.workspace_name.is_none() {
        return Err("non-interactive sharing requires --workspace-name; no Workspace is inferred from cwd or path".into());
    }
    if !options.non_interactive {
        ensure_interactive_tty(
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal(),
        )?;
    }
    let backend_client = BackendApiClient::from_stored_token(&options.backend_url)
        .map_err(|error| error.to_string())?;
    let api = authenticated_server_api_client(&backend_client, &options.backend_url)?;
    let workspaces = load_share_workspaces(&api).await?;
    if options.non_interactive {
        return configure_non_interactively(options, &workspaces).map(Some);
    }
    let stdin = std::io::stdin();
    let stderr = std::io::stderr();
    configure_interactively(options, &workspaces, &mut stdin.lock(), &mut stderr.lock())
}

pub async fn run(options: WorkdirShareOptions) -> Result<(), String> {
    let mut evidence = ShareEvidence::default();
    let result = run_typed(options, &mut evidence).await;
    let observations = evidence.render();
    match result {
        Ok(()) => {
            for observation in observations {
                eprintln!("{observation}");
            }
            Ok(())
        }
        Err(error) => {
            let mut message = error.render();
            for observation in observations {
                message.push_str("\nRetained observation: ");
                message.push_str(&observation);
            }
            Err(message)
        }
    }
}

async fn run_typed(
    options: WorkdirShareOptions,
    evidence: &mut ShareEvidence,
) -> Result<(), ShareError> {
    let Some(options) = resolve_share_options(options).await? else {
        eprintln!("External Workdir share cancelled; no grant was created.");
        return Ok(());
    };
    // Pin before creating remote authority: the Backend grant can never outlive
    // a failed local approval/open race, and subsequent path replacement cannot
    // redirect the provider.
    let pinned_root = pin_selected_root(&options.path)?;

    let backend_client = BackendApiClient::from_stored_token(&options.backend_url)
        .map_err(|error| error.to_string())?;
    let server_api_client = authenticated_server_api_client(&backend_client, &options.backend_url)?;
    let provider_instance_id = format!("cli-{}", uuid::Uuid::now_v7());
    let grant = server_api_client
        .external_workdir_grant_create(
            options.workspace_id.clone(),
            ExternalWorkdirGrantCreateRequest {
                provider_instance_id: provider_instance_id.clone(),
                display_name: options.display_name.clone(),
                ttl_seconds: options.ttl.map(|ttl| ttl.as_secs()),
                permissions: options.permissions,
            },
        )
        .await
        .map_err(|error| ShareError::grant(WorkdirManagementOperation::GrantCreate, error))?;
    evidence.retain_grant(grant.clone());

    if grant.permissions != options.permissions {
        return Err(
            "Backend returned permissions that differ from the requested grant"
                .to_string()
                .into(),
        );
    }
    let capabilities = workdir_capabilities(grant.permissions);
    let limits = BoundedReadLimits::EXTERNAL_DEFAULT;

    println!("External Workdir grant: {}", grant.grant_id);
    println!("Logical Workdir: {}", grant.working_directory_id);
    println!("Display name: {}", grant.display_name);
    println!("Permissions: {}", permissions_label(grant.permissions));
    if grant.permissions.command {
        println!(
            "COMMAND executes with this sharing CLI user's host authority; it is not sandboxed."
        );
    }
    println!(
        "Expires at: {}",
        grant
            .expires_at
            .as_deref()
            .unwrap_or("never (until stopped or revoked)")
    );

    let expires_at = grant
        .expires_at
        .as_deref()
        .map(|expires_at| {
            chrono::DateTime::parse_from_rfc3339(expires_at)
                .map(|expires_at| expires_at.with_timezone(&Utc))
                .map_err(|_| "Backend returned an invalid External Workdir expiry".to_string())
        })
        .transpose()?;
    let mut generation = grant.generation;
    loop {
        // Command handles and results are generation-owned. A reconnect always
        // gets a fresh LocalWorkdirSession, and closing the old one kills and
        // reaps every process before the next generation can register.
        let session = LocalWorkdirSession::external_with_capabilities_pinned(
            Workdir::new(&grant.working_directory_id),
            pinned_root.clone(),
            limits,
            capabilities,
        )
        .map_err(|error| ShareError::workdir(WorkdirFailureOperation::SessionOpen, error))?;
        let outcome = serve_provider_connection(
            &backend_client,
            &server_api_client,
            &options,
            &grant,
            &provider_instance_id,
            &session,
            generation,
            evidence,
        )
        .await;
        let close_result = session
            .close()
            .await
            .map_err(|error| ShareError::workdir(WorkdirFailureOperation::SessionClose, error));
        // A revocation request whose response was lost must never be routed
        // through connection refresh/reconnect and sent again.
        let cleanup_failed = close_result.is_err();
        let outcome = retain_provider_cleanup(outcome, close_result);
        if cleanup_failed
            || outcome
                .as_ref()
                .is_err_and(|error| error.prevents_reconnect())
        {
            return outcome.map(|_| ());
        }
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(connection_error) => {
                evidence.retain_failure(connection_error);
                if expires_at.is_some_and(|expires_at| Utc::now() >= expires_at) {
                    return Err(
                        "External Workdir grant expired after provider connection failure"
                            .to_string()
                            .into(),
                    );
                }
                let refresh =
                    fetch_grant(&server_api_client, &options.workspace_id, &grant.grant_id);
                let refreshed = tokio::select! {
                    signal = tokio::signal::ctrl_c() => {
                        signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
                        evidence.retain_grant(revoke(&server_api_client, &options.workspace_id, &grant.grant_id).await?);
                        println!("External Workdir revocation response received; see retained grant status.");
                        return Ok(());
                    }
                    refreshed = refresh => refreshed,
                };
                let current = match refreshed {
                    Ok(current) => current,
                    Err(refresh_error) => {
                        evidence.retain_failure(refresh_error);
                        if wait_to_reconnect(
                            &server_api_client,
                            &options.workspace_id,
                            &grant.grant_id,
                            evidence,
                        )
                        .await?
                        {
                            println!(
                                "External Workdir revocation response received; see retained grant status."
                            );
                            return Ok(());
                        }
                        continue;
                    }
                };
                evidence.retain_grant(current.clone());
                match current.status.as_str() {
                    "pending" => generation = current.generation,
                    "offline" => {
                        generation = current.generation.checked_add(1).ok_or_else(|| {
                            "External Workdir provider generation overflow".to_string()
                        })?;
                    }
                    "online" => {
                        // The Backend may still be observing the old socket close.
                        // Wait for its durable offline fence before reconnecting.
                    }
                    "revoked" | "expired" => {
                        println!("External Workdir grant is no longer active.");
                        return Ok(());
                    }
                    _ => {
                        return Err("Backend returned an unknown External Workdir grant status"
                            .to_string()
                            .into());
                    }
                }
                println!("Connection: reconnecting");
                if wait_to_reconnect(
                    &server_api_client,
                    &options.workspace_id,
                    &grant.grant_id,
                    evidence,
                )
                .await?
                {
                    println!(
                        "External Workdir revocation response received; see retained grant status."
                    );
                    return Ok(());
                }
                continue;
            }
        };
        match outcome {
            ProviderEnd::Interrupted => {
                println!(
                    "External Workdir revocation response received; see retained grant status."
                );
                return Ok(());
            }
            ProviderEnd::Revoked => {
                println!("External Workdir grant is no longer active.");
                return Ok(());
            }
            ProviderEnd::Disconnected => {
                if expires_at.is_some_and(|expires_at| Utc::now() >= expires_at) {
                    return Err("External Workdir grant expired while disconnected"
                        .to_string()
                        .into());
                }
                generation = generation
                    .checked_add(1)
                    .ok_or_else(|| "External Workdir provider generation overflow".to_string())?;
                println!("Connection: reconnecting");
                if wait_to_reconnect(
                    &server_api_client,
                    &options.workspace_id,
                    &grant.grant_id,
                    evidence,
                )
                .await?
                {
                    println!(
                        "External Workdir revocation response received; see retained grant status."
                    );
                    return Ok(());
                }
            }
        }
    }
}

async fn serve_provider_connection(
    backend_client: &BackendApiClient,
    server_api_client: &ServerApiClient,
    options: &ResolvedWorkdirShareOptions,
    grant: &ExternalWorkdirGrantResponse,
    provider_instance_id: &str,
    session: &LocalWorkdirSession,
    generation: u64,
    evidence: &mut ShareEvidence,
) -> Result<ProviderEnd, ShareError> {
    let provider_path = format!(
        "/api/w/{}/external-workdir-grants/{}/provider",
        encode_path_segment(&options.workspace_id),
        encode_path_segment(&grant.grant_id)
    );
    let request = backend_client
        .websocket_request(&provider_path)
        .map_err(|error| error.to_string())?;
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(workdir::external::MAX_EXTERNAL_SERVER_FRAME_BYTES))
        .max_frame_size(Some(workdir::external::MAX_EXTERNAL_SERVER_FRAME_BYTES));
    let connection = connect_async_with_config(request, Some(websocket_config), false);
    let (socket, _) = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            evidence.retain_grant(revoke(server_api_client, &options.workspace_id, &grant.grant_id).await?);
            return Ok(ProviderEnd::Interrupted);
        }
        connection = connection => connection
            .map_err(provider_connection_error)?,
    };
    let registration =
        ExternalWorkdirProviderFrame::current(ExternalWorkdirProviderMessage::Register {
            registration: ExternalWorkdirProviderRegistration {
                provider_instance_id: ExternalProviderInstanceId::new(provider_instance_id)
                    .map_err(|_| "invalid External Workdir provider identity".to_string())?,
                grant_id: ExternalWorkdirGrantId::new(grant.grant_id.clone()).map_err(|_| {
                    "Backend returned an invalid External Workdir grant identity".to_string()
                })?,
                workdir_id: session.workdir().id().clone(),
                generation,
                capabilities: session.capabilities(),
                read_limits: BoundedReadLimits::EXTERNAL_DEFAULT,
            },
        });
    let outcome = serve_external_workdir_provider_socket_typed(
        socket,
        registration,
        session,
        generation,
        async {
            tokio::signal::ctrl_c()
                .await
                .map_err(|error| format!("failed to wait for Ctrl-C: {error}"))
        },
    )
    .await;
    let interrupted = matches!(&outcome, Ok(ProviderEnd::Interrupted))
        || matches!(
            &outcome,
            Err(ShareError {
                cause: ShareCause::ProviderEnded(ProviderEnd::Interrupted),
                ..
            })
        );
    if interrupted {
        // Even failed local shutdown must not skip the requested authority fence.
        // Send it once, then retain its response/error independently of shutdown.
        match revoke(server_api_client, &options.workspace_id, &grant.grant_id).await {
            Ok(grant) => evidence.retain_grant(grant),
            Err(revoke_error) => {
                return match outcome {
                    Err(error) => Err(error.with_secondary(revoke_error)),
                    Ok(_) => Err(revoke_error),
                };
            }
        }
    }
    outcome
}

/// Run the production External Workdir provider protocol over an established WebSocket.
///
/// The CLI uses this pump after opening its authenticated Backend connection. Keeping the
/// protocol pump separate lets composed tests exercise the same registration, operation,
/// cancellation, and result-forwarding implementation without spawning a second CLI process.
pub async fn serve_external_workdir_provider_socket<S, I>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    registration: ExternalWorkdirProviderFrame,
    session: &LocalWorkdirSession,
    generation: u64,
    interrupt: I,
) -> Result<ProviderEnd, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    I: std::future::Future<Output = Result<(), String>>,
{
    serve_external_workdir_provider_socket_typed(
        socket,
        registration,
        session,
        generation,
        interrupt,
    )
    .await
    .map_err(|error| error.render())
}

async fn serve_external_workdir_provider_socket_typed<S, I>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    registration: ExternalWorkdirProviderFrame,
    session: &LocalWorkdirSession,
    generation: u64,
    interrupt: I,
) -> Result<ProviderEnd, ShareError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    I: std::future::Future<Output = Result<(), String>>,
{
    tokio::pin!(interrupt);
    send_provider_frame(&mut socket, &registration).await?;
    let registered = tokio::select! {
        signal = &mut interrupt => {
            signal?;
            return Ok(ProviderEnd::Interrupted);
        }
        registered = tokio::time::timeout(Duration::from_secs(10), socket.next()) => {
            registered
                .map_err(|_| "Backend timed out External Workdir provider registration".to_string())?
                .ok_or_else(|| "Backend closed the External Workdir provider connection".to_string())?
                .map_err(|_| "External Workdir provider registration transport failed".to_string())?
        }
    };
    validate_provider_registration(registered, generation)?;
    println!("Connection: online (generation {generation})");

    let (completion_sender, mut completions) =
        tokio::sync::mpsc::channel::<OperationCompletion>(16);
    let mut operations = HashMap::<String, OperationTask>::new();
    let outcome = 'provider: loop {
        tokio::select! {
            signal = &mut interrupt => {
                if let Err(error) = signal {
                    break 'provider Err(ShareError::from(error));
                }
                break 'provider Ok(ProviderEnd::Interrupted);
            }
            completion = completions.recv() => {
                let Some(completion) = completion else {
                    cancel_operations(&operations);
                    break 'provider Err("External Workdir operation executor closed".to_string().into());
                };
                let Some(operation) = operations.remove(completion.operation_id.as_str()) else {
                    continue;
                };
                let cancelled = match operation.join() {
                    Ok(cancelled) => cancelled,
                    Err(error) => {
                        break 'provider Err(error.into());
                    }
                };
                let outcome = if cancelled {
                    cancel_completed_command(session, &completion.outcome).await;
                    ExternalWorkdirOperationOutcome::Cancelled
                } else {
                    completion.outcome
                };
                let result = ExternalWorkdirProviderFrame::current(
                    ExternalWorkdirProviderMessage::OperationResult {
                        generation,
                        operation_id: completion.operation_id,
                        outcome,
                    },
                );
                if send_provider_frame(&mut socket, &result).await.is_err() {
                    break 'provider Ok(ProviderEnd::Disconnected);
                }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else {
                    break 'provider Ok(ProviderEnd::Disconnected);
                };
                let incoming = match incoming {
                    Ok(incoming) => incoming,
                    Err(_) => {
                        break 'provider Ok(ProviderEnd::Disconnected);
                    }
                };
                match incoming {
                    Message::Text(text) => {
                        let frame = match serde_json::from_str::<ExternalWorkdirServerFrame>(&text) {
                            Ok(frame) => frame,
                            Err(_) => {
                                break 'provider Err("Backend returned an invalid External Workdir frame".to_string().into());
                            }
                        };
                        match frame.message {
                            ExternalWorkdirServerMessage::Operation {
                                generation: frame_generation,
                                operation_id,
                                operation,
                            } if frame_generation == generation => {
                                if operations.len() >= 16 || operations.contains_key(operation_id.as_str()) {
                                    break 'provider Err("Backend exceeded External Workdir operation bounds".to_string().into());
                                }
                                let key = operation_id.as_str().to_string();
                                let completion_sender = completion_sender.clone();
                                let task_operation_id = operation_id.clone();
                                let cancelled = Arc::new(AtomicBool::new(false));
                                let operation_timeout = provider_operation_timeout(operation.as_inner());
                                let guarded_session = session.with_operation_guard(
                                    cancelled.clone(),
                                    Instant::now() + operation_timeout,
                                );
                                let operation = operation.into_inner();
                                let runtime = tokio::runtime::Handle::current();
                                let spawned = std::thread::Builder::new()
                                    .name("external-workdir-operation".to_string())
                                    .spawn(move || {
                                        let outcome = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| {
                                                runtime.block_on(execute_operation(
                                                    &guarded_session,
                                                    operation,
                                                ))
                                            }),
                                        )
                                        .unwrap_or_else(|_| ExternalWorkdirOperationOutcome::Failed {
                                            error: ExternalWorkdirOperationError::from_transport_error(
                                                WorkdirTransportError {
                                                    failure_reason: None,
                                                    denial_reason: None,
                                                    code: workdir::http::WorkdirTransportErrorCode::Internal,
                                                    message: "External Workdir operation executor failed".to_string(),
                                                },
                                            ),
                                        });
                                        let _ = completion_sender.blocking_send(OperationCompletion {
                                            operation_id: task_operation_id,
                                            outcome,
                                        });
                                    });
                                let thread = match spawned {
                                    Ok(thread) => thread,
                                    Err(_) => {
                                        break 'provider Err("failed to start External Workdir operation executor".to_string().into());
                                    }
                                };
                                operations.insert(
                                    key,
                                    OperationTask {
                                        cancelled,
                                        thread: Some(thread),
                                    },
                                );
                            }
                            ExternalWorkdirServerMessage::Heartbeat {
                                generation: frame_generation,
                                sequence,
                            } if frame_generation == generation => {
                                let heartbeat = ExternalWorkdirProviderFrame::current(
                                    ExternalWorkdirProviderMessage::Heartbeat { generation, sequence },
                                );
                                if send_provider_frame(&mut socket, &heartbeat).await.is_err() {
                                    break 'provider Ok(ProviderEnd::Disconnected);
                                }
                            }
                            ExternalWorkdirServerMessage::Cancel {
                                generation: frame_generation,
                                operation_id,
                            } if frame_generation == generation => {
                                if let Some(operation) = operations.get(operation_id.as_str()) {
                                    // The terminal Cancelled result is emitted only after the
                                    // executor reports that filesystem work has actually stopped.
                                    operation.cancel();
                                }
                            }
                            ExternalWorkdirServerMessage::Revoke {
                                generation: frame_generation,
                                ..
                            } if frame_generation == generation => {
                                break 'provider Ok(ProviderEnd::Revoked);
                            }
                            _ => {
                                break 'provider Err("stale or unexpected External Workdir provider frame".to_string().into());
                            }
                        }
                    }
                    Message::Ping(bytes) => {
                        let pong = tokio::time::timeout(
                            Duration::from_secs(5),
                            socket.send(Message::Pong(bytes)),
                        )
                        .await;
                        if !matches!(pong, Ok(Ok(()))) {
                            break 'provider Ok(ProviderEnd::Disconnected);
                        }
                    }
                    Message::Close(_) => {
                        break 'provider Ok(ProviderEnd::Disconnected);
                    }
                    _ => {}
                }
            }
        }
    };
    let outcome = retain_provider_cleanup(
        outcome,
        stop_provider_operations(session, &mut operations, &mut completions).await,
    );
    if matches!(&outcome, Ok(ProviderEnd::Revoked)) {
        let acknowledged = ExternalWorkdirProviderFrame::current(
            ExternalWorkdirProviderMessage::RevokeAcknowledged { generation },
        );
        let _ = send_provider_frame(&mut socket, &acknowledged).await;
    }
    outcome
}

async fn cancel_completed_command(
    session: &LocalWorkdirSession,
    outcome: &ExternalWorkdirOperationOutcome,
) {
    let ExternalWorkdirOperationOutcome::Completed { result } = outcome else {
        return;
    };
    let workdir::http::WorkdirSessionOperationResult::CommandStart(handle) = result.as_inner()
    else {
        return;
    };
    let _ = session.cancel_command(handle.clone()).await;
    let _ = session
        .command_output(workdir::CommandOutputRequest {
            handle: handle.clone(),
            cursor: 0,
            limit: workdir::external::MAX_EXTERNAL_COMMAND_OUTPUT_BYTES,
            wait: true,
        })
        .await;
}

async fn execute_operation(
    session: &LocalWorkdirSession,
    operation: workdir::http::WorkdirSessionOperation,
) -> ExternalWorkdirOperationOutcome {
    match dispatch_workdir_session_operation(session, operation).await {
        Ok(result) => match result.try_into() {
            Ok(result) => ExternalWorkdirOperationOutcome::Completed { result },
            Err(message) => ExternalWorkdirOperationOutcome::Failed {
                error: ExternalWorkdirOperationError::from_transport_error(WorkdirTransportError {
                    failure_reason: None,
                    denial_reason: None,
                    code: workdir::http::WorkdirTransportErrorCode::InvalidRequest,
                    message,
                }),
            },
        },
        Err(error) => ExternalWorkdirOperationOutcome::Failed {
            error: ExternalWorkdirOperationError::from_transport_error(
                WorkdirTransportError::from_workdir_error(&error),
            ),
        },
    }
}

fn cancel_operations(operations: &HashMap<String, OperationTask>) {
    for operation in operations.values() {
        operation.cancel();
    }
}

async fn stop_provider_operations(
    session: &LocalWorkdirSession,
    operations: &mut HashMap<String, OperationTask>,
    completions: &mut tokio::sync::mpsc::Receiver<OperationCompletion>,
) -> Result<(), ShareError> {
    let close = session
        .close()
        .await
        .map_err(|error| ShareError::workdir(WorkdirFailureOperation::SessionClose, error));
    stop_operations_after_close(close, operations, completions).await
}

async fn stop_operations_after_close(
    close: Result<(), ShareError>,
    operations: &mut HashMap<String, OperationTask>,
    completions: &mut tokio::sync::mpsc::Receiver<OperationCompletion>,
) -> Result<(), ShareError> {
    // Closing a session may fail before cancelling/reaping its executors.
    // Always run the executor shutdown, retaining both independent failures.
    let stopped = stop_operations(operations, completions).await;
    retain_cleanup(close, stopped)
}

async fn stop_operations(
    operations: &mut HashMap<String, OperationTask>,
    completions: &mut tokio::sync::mpsc::Receiver<OperationCompletion>,
) -> Result<(), ShareError> {
    cancel_operations(operations);
    let mut result = Ok(());
    while !operations.is_empty() {
        let Some(completion) = completions.recv().await else {
            result = retain_cleanup(
                result,
                Err(
                    "External Workdir operation executor closed during cancellation"
                        .to_string()
                        .into(),
                ),
            );
            // No senders remain; join every remaining executor, even if an
            // earlier one panicked. Dropping JoinHandles would detach them.
            for (_, operation) in operations.drain() {
                result = retain_cleanup(
                    result,
                    operation.join().map(|_| ()).map_err(ShareError::from),
                );
            }
            break;
        };
        if let Some(operation) = operations.remove(completion.operation_id.as_str()) {
            result = retain_cleanup(
                result,
                operation.join().map(|_| ()).map_err(ShareError::from),
            );
        }
    }
    result
}

async fn send_provider_frame<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    frame: &ExternalWorkdirProviderFrame,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = Message::Text(
        serde_json::to_string(frame)
            .map_err(|error| error.to_string())?
            .into(),
    );
    tokio::time::timeout(Duration::from_secs(5), socket.send(message))
        .await
        .map_err(|_| "timed out sending External Workdir provider frame".to_string())?
        .map_err(|_| "failed to send External Workdir provider frame".to_string())
}

async fn wait_to_reconnect(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
    evidence: &mut ShareEvidence,
) -> Result<bool, ShareError> {
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            evidence.retain_grant(revoke(client, workspace_id, grant_id).await?);
            Ok(true)
        }
        _ = tokio::time::sleep(Duration::from_millis(250)) => Ok(false),
    }
}

async fn fetch_grant(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<ExternalWorkdirGrantResponse, ShareError> {
    client
        .external_workdir_grant_get(workspace_id.to_string(), grant_id.to_string())
        .await
        .map_err(|error| ShareError::grant(WorkdirManagementOperation::GrantInspect, error))
}

async fn revoke(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<ExternalWorkdirGrantResponse, ShareError> {
    // Exactly one mutation call. Transport/decoding failure is not a retry gate.
    client
        .external_workdir_grant_revoke(workspace_id.to_string(), grant_id.to_string())
        .await
        .map_err(|error| ShareError::grant(WorkdirManagementOperation::GrantRevoke, error))
}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    fn share_options() -> WorkdirShareOptions {
        WorkdirShareOptions {
            path: PathBuf::from("."),
            workspace_name: None,
            backend_url: "https://backend.example".to_string(),
            display_name: "External Workdir".to_string(),
            ttl: None,
            permissions: server_api::ExternalWorkdirPermissions::default(),
            non_interactive: false,
        }
    }

    fn workspace(workspace_id: &str, display_name: &str) -> server_api::WorkspaceSummary {
        server_api::WorkspaceSummary {
            workspace_id: workspace_id.to_string(),
            owner_account_id: "account-a".to_string(),
            display_name: display_name.to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn provider_registration_does_not_disclose_untyped_backend_close_reason() {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

        let error = validate_provider_registration(
            Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "provider-secret /private/provider; Backend requires internal token".into(),
            })),
            1,
        )
        .unwrap_err();
        assert!(error.contains("rejected"), "{error}");
        assert!(!error.contains("provider-secret"), "{error}");
        assert!(!error.contains("/private/provider"), "{error}");
    }

    #[test]
    fn workspace_name_selection_requires_a_unique_exact_accessible_name() {
        let workspaces = [
            workspace("id-a", "Alpha"),
            workspace("id-b", "開発 Workspace"),
        ];
        for (name, expected) in [("Alpha", "id-a"), ("開発 Workspace", "id-b")] {
            let mut options = share_options();
            options.workspace_name = Some(name.into());
            options.non_interactive = true;
            let resolved = configure_non_interactively(options, &workspaces).unwrap();
            assert_eq!(resolved.workspace_id, expected);
        }
        for name in ["alpha", "Al", "id-a", " Alpha", "missing"] {
            assert!(
                workspace_index_by_name(&workspaces, name)
                    .unwrap_err()
                    .contains("No accessible Workspace")
            );
        }
        assert!(workspace_index_by_name(&[], "Alpha").is_err());
        assert!(
            workspace_index_by_name(&workspaces, " ")
                .unwrap_err()
                .contains("blank")
        );
        let duplicates = [workspace("id-a", "Alpha"), workspace("id-b", "Alpha")];
        assert!(
            workspace_index_by_name(&duplicates, "Alpha")
                .unwrap_err()
                .contains("ambiguous")
        );
        let mut bounded = (0..SHARE_WORKSPACE_LIMIT)
            .map(|i| workspace(&format!("id-{i}"), &format!("Name {i}")))
            .collect::<Vec<_>>();
        assert!(
            workspace_index_by_name(&bounded, "Name 0")
                .unwrap_err()
                .contains("uniqueness could not be confirmed")
        );
        bounded.pop();
        assert_eq!(workspace_index_by_name(&bounded, "Name 0").unwrap(), 0);
    }

    #[test]
    fn interactive_workspace_name_preselects_without_skipping_confirmation() {
        let workspaces = [
            workspace("id-a", "Alpha"),
            workspace("id-b", "開発 Workspace"),
        ];
        let mut options = share_options();
        options.workspace_name = Some("開発 Workspace".into());
        let mut output = Vec::new();
        let selected = configure_interactively(
            options.clone(),
            &workspaces,
            &mut std::io::Cursor::new(b"\n\n\nyes\n"),
            &mut output,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.workspace_id, "id-b");
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Workspace [2;"));
        assert!(output.contains("Workspace: 開発 Workspace"));
        assert!(output.contains("Connect and create grant?"));
        assert!(
            configure_interactively(
                options,
                &workspaces,
                &mut std::io::Cursor::new(b"\n\n\nno\n"),
                &mut Vec::new()
            )
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn workspace_name_catalog_uses_authenticated_http_and_returns_resolved_id() {
        let body = serde_json::to_string(&vec![
            workspace("workspace-a", "Alpha"),
            workspace("workspace-b", "開発 Workspace"),
        ])
        .unwrap();
        let (base, requests, server) = spawn_json_server(body, 1);
        let client = test_server_api_client(&base);
        let catalog = load_share_workspaces(&client).await.unwrap();
        let mut options = share_options();
        options.backend_url = base;
        options.workspace_name = Some("開発 Workspace".into());
        options.non_interactive = true;
        let resolved = configure_non_interactively(options, &catalog).unwrap();
        assert_eq!(resolved.workspace_id, "workspace-b");
        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .to_ascii_lowercase();
        assert!(
            request.starts_with("get /api/workspaces?limit=200 "),
            "{request}"
        );
        assert!(request.contains("authorization: bearer test-token\r\n"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn workspace_name_catalog_rejection_or_malformed_response_is_not_an_empty_catalog() {
        for (status, body) in [("403 Forbidden", "{}"), ("200 OK", "not-json")] {
            let (base, requests, server) =
                spawn_http_server(body.into(), 1, Some(status), "application/json");
            let error = load_share_workspaces(&test_server_api_client(&base))
                .await
                .unwrap_err();
            assert!(error.contains("failed to query Backend Workspace catalog"));
            assert!(
                requests
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .starts_with("GET /api/workspaces?")
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn interactive_share_defaults_to_first_workspace_read_only_and_unlimited() {
        let workspaces = [workspace("workspace-a", "Alpha")];
        let mut input = std::io::Cursor::new(b"\n\n\nyes\n".to_vec());
        let mut output = Vec::new();

        let selected =
            configure_interactively(share_options(), &workspaces, &mut input, &mut output)
                .unwrap()
                .unwrap();

        assert_eq!(selected.workspace_id, "workspace-a");
        assert_eq!(
            selected.permissions,
            server_api::ExternalWorkdirPermissions::default()
        );
        assert_eq!(selected.ttl, None);
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Connect and create grant? [y/N]")
        );
    }

    #[test]
    fn interactive_share_selects_one_hierarchical_level_and_requires_confirmation() {
        let workspaces = [
            workspace("workspace-a", "Alpha"),
            workspace("workspace-b", "Beta"),
        ];
        let mut input = std::io::Cursor::new(b"2\nCOMMAND\n15m\ny\n".to_vec());
        let mut output = Vec::new();

        let selected =
            configure_interactively(share_options(), &workspaces, &mut input, &mut output)
                .unwrap()
                .unwrap();

        assert_eq!(selected.workspace_id, "workspace-b");
        assert_eq!(
            selected.permissions,
            server_api::ExternalWorkdirPermissions {
                read: true,
                write: true,
                command: true,
            }
        );
        assert_eq!(selected.ttl, Some(Duration::from_secs(900)));
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("not sandboxed"));
        assert!(output.contains("Permissions: COMMAND (includes WRITE + READ)"));
    }

    #[test]
    fn share_permission_levels_map_to_exact_existing_capability_bits() {
        let bits = |permissions| {
            serde_json::to_value(workdir_capabilities(permissions)).unwrap()["bits"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(bits(server_api::ExternalWorkdirPermissions::READ), 25);
        assert_eq!(bits(server_api::ExternalWorkdirPermissions::WRITE), 31);
        assert_eq!(bits(server_api::ExternalWorkdirPermissions::COMMAND), 63);

        for invalid in [
            server_api::ExternalWorkdirPermissions {
                read: false,
                write: false,
                command: true,
            },
            server_api::ExternalWorkdirPermissions {
                read: false,
                write: true,
                command: false,
            },
            server_api::ExternalWorkdirPermissions {
                read: true,
                write: false,
                command: true,
            },
        ] {
            assert!(validate_permissions(invalid).is_err());
        }
    }

    #[test]
    fn interactive_share_cancellation_yields_no_grant_configuration() {
        let workspaces = [workspace("workspace-a", "Alpha")];
        let mut declined = std::io::Cursor::new(b"\n\n\nno\n".to_vec());
        let mut output = Vec::new();
        assert!(
            configure_interactively(share_options(), &workspaces, &mut declined, &mut output,)
                .unwrap()
                .is_none()
        );

        let mut eof = std::io::Cursor::new(Vec::<u8>::new());
        assert!(
            configure_interactively(share_options(), &workspaces, &mut eof, &mut Vec::new())
                .unwrap()
                .is_none()
        );
    }

    // Both modes consume the same authenticated catalog; neither infers a Workspace from the path.
    #[tokio::test]
    async fn non_interactive_share_resolves_the_same_choices_without_prompting() {
        let workspaces = [workspace("workspace-a", "Alpha")];
        for level in ["read", "write", "command"] {
            for ttl in ["none", "unlimited", "15m"] {
                let mut options = share_options();
                options.backend_url = "not-a-backend-url".into();
                options.path = PathBuf::from("./project");
                options.display_name = "project".into();
                // Start bounded so explicit none/unlimited must actually clear it.
                options.ttl = Some(Duration::from_secs(3600));
                let mut input = std::io::Cursor::new(format!("1\n{level}\n{ttl}\nyes\n"));
                let interactive = configure_interactively(
                    options.clone(),
                    &workspaces,
                    &mut input,
                    &mut Vec::new(),
                )
                .unwrap()
                .unwrap();
                options.workspace_name = Some("Alpha".into());
                options.permissions = match level {
                    "read" => server_api::ExternalWorkdirPermissions::READ,
                    "write" => server_api::ExternalWorkdirPermissions::WRITE,
                    "command" => server_api::ExternalWorkdirPermissions::COMMAND,
                    _ => unreachable!(),
                };
                options.ttl = parse_ttl(ttl).unwrap();
                options.non_interactive = true;
                let automated = configure_non_interactively(options, &workspaces).unwrap();
                assert_eq!(automated.path, interactive.path);
                assert_eq!(automated.workspace_id, interactive.workspace_id);
                assert_eq!(automated.backend_url, interactive.backend_url);
                assert_eq!(automated.display_name, interactive.display_name);
                assert_eq!(automated.permissions, interactive.permissions);
                assert_eq!(automated.ttl, interactive.ttl);
                assert_eq!(
                    automated.ttl,
                    if ttl == "15m" {
                        Some(Duration::from_secs(900))
                    } else {
                        None
                    }
                );
            }
        }
        let mut missing_workspace = share_options();
        missing_workspace.non_interactive = true;
        assert!(
            resolve_share_options(missing_workspace)
                .await
                .unwrap_err()
                .contains("requires --workspace-name")
        );
    }

    #[test]
    fn interactive_share_fails_closed_without_input_and_prompt_ttys() {
        assert!(ensure_interactive_tty(true, true).is_ok());
        assert!(
            ensure_interactive_tty(false, true)
                .unwrap_err()
                .contains("--non-interactive")
        );
        assert!(
            ensure_interactive_tty(true, false)
                .unwrap_err()
                .contains("--non-interactive")
        );
    }

    fn test_server_api_client(base_url: &str) -> ServerApiClient {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_static("Bearer test-token"),
        );
        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        ServerApiClient::with_client(base_url.try_into().unwrap(), http_client)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0, "HTTP client closed before completing request");
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none()
                && let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                });
                expected_length = Some(header_end + 4 + content_length.unwrap_or(0));
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        String::from_utf8(request).unwrap()
    }

    fn spawn_json_server(
        response_body: String,
        response_count: usize,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        spawn_http_server(response_body, response_count, None, "application/json")
    }

    fn spawn_http_server(
        response_body: String,
        response_count: usize,
        status_override: Option<&'static str>,
        content_type: &'static str,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for _ in 0..response_count {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                let status = status_override.unwrap_or(if request.starts_with("POST ") {
                    "201 Created"
                } else {
                    "200 OK"
                });
                sender.send(request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), receiver, thread)
    }

    fn grant_response_json() -> String {
        serde_json::json!({
            "grant_id": "grant/a b",
            "workspace_id": "workspace/a b",
            "working_directory_id": "workdir-a",
            "provider_instance_id": "provider-a",
            "display_name": "Shared files",
            "permissions": {"read": true, "write": false, "command": false},
            "expires_at": "2026-09-23T03:00:00Z",
            "generation": 7,
            "status": "pending",
            "cleanup_state": "not_required",
            "cleanup_failures": []
        })
        .to_string()
    }

    fn management_problem(
        operation: WorkdirManagementOperation,
        reason: workdir::management_failure::WorkdirManagementReason,
    ) -> String {
        let facts = FailureFacts::new(WorkdirManagementFailure { operation, reason }).unwrap();
        let projected = project_failure::<error_contract::Browser, _>(
            &facts,
            &WorkdirManagementVisibility {
                can_view_cause: true,
            },
        )
        .unwrap();
        let api_error = server_api::WorkdirManagementApiError::projected(projected);
        let mut body = serde_json::to_value(api_error).unwrap();
        // Received explanation is untrusted, even when its typed cause is known.
        body["detail"] = serde_json::json!("provider-secret /private/provider HTTP-body");
        body["yoi_failure"]["explanation"] = body["detail"].clone();
        body.to_string()
    }

    #[tokio::test]
    async fn generated_grant_errors_reach_cli_with_fresh_specific_safe_projection() {
        use workdir::management_failure::WorkdirManagementReason as Reason;
        for (operation, reason, status, expected) in [
            (
                WorkdirManagementOperation::GrantCreate,
                Reason::GrantDisplayNameInvalid,
                "400 Bad Request",
                "1 to 100 bytes",
            ),
            (
                WorkdirManagementOperation::GrantInspect,
                Reason::GrantNotFound,
                "400 Bad Request",
                "not found in this Workspace",
            ),
            (
                WorkdirManagementOperation::GrantRevoke,
                Reason::GrantCreatorRequired,
                "403 Forbidden",
                "grant creator",
            ),
        ] {
            let (url, requests, server) = spawn_http_server(
                management_problem(operation, reason),
                1,
                Some(status),
                "application/problem+json",
            );
            let client = test_server_api_client(&url);
            let error = match operation {
                WorkdirManagementOperation::GrantCreate => client
                    .external_workdir_grant_create(
                        "workspace-a".into(),
                        ExternalWorkdirGrantCreateRequest {
                            provider_instance_id: "provider-a".into(),
                            display_name: "Shared files".into(),
                            ttl_seconds: None,
                            permissions: server_api::ExternalWorkdirPermissions::READ,
                        },
                    )
                    .await
                    .map_err(|error| ShareError::grant(operation, error))
                    .unwrap_err(),
                WorkdirManagementOperation::GrantInspect => {
                    fetch_grant(&client, "workspace-a", "grant-a")
                        .await
                        .unwrap_err()
                }
                WorkdirManagementOperation::GrantRevoke => {
                    revoke(&client, "workspace-a", "grant-a").await.unwrap_err()
                }
                _ => unreachable!(),
            };
            // Evidence remains typed until the display boundary.
            let ShareCause::Grant {
                error: ClientError::Public { error: api, .. },
                ..
            } = &error.cause
            else {
                panic!("typed management error was lost: {error:?}");
            };
            assert_eq!(
                api.details(),
                Some(&WorkdirManagementFailure { operation, reason })
            );
            let rendered = error.render();
            assert!(rendered.contains(expected), "{rendered}");
            for secret in ["provider-secret", "/private/provider", "HTTP-body"] {
                assert!(!rendered.contains(secret), "{rendered}");
            }
            requests.recv_timeout(Duration::from_secs(5)).unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn provider_handshake_decodes_typed_management_error_without_exposing_http_body() {
        use workdir::management_failure::WorkdirManagementReason as Reason;
        let body = management_problem(
            WorkdirManagementOperation::GrantConnect,
            Reason::GrantRevoked,
        );
        let response = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(400)
            .header("content-type", "application/problem+json")
            .body(Some(body.into_bytes()))
            .unwrap();
        let error = provider_connection_error(tokio_tungstenite::tungstenite::Error::Http(
            Box::new(response),
        ));
        let rendered = error.render();
        assert!(rendered.contains("provider connection"), "{rendered}");
        assert!(rendered.contains("has been revoked"), "{rendered}");
        assert!(!rendered.contains("provider-secret"), "{rendered}");
        assert!(!rendered.contains("HTTP-body"), "{rendered}");
    }

    #[test]
    fn connection_timeout_keeps_existing_reconnect_path_but_never_replays_grant_mutation() {
        let connection = provider_connection_error(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "/private/provider timeout"),
        ));
        assert!(!connection.prevents_reconnect());
        assert!(!connection.render().contains("/private/"));
        let revoke = ShareError::grant(
            WorkdirManagementOperation::GrantRevoke,
            ClientError::Failure(ClientFailure::Timeout),
        );
        assert!(revoke.prevents_reconnect());
        assert!(revoke.render().contains("do not automatically resend"));
        let cleanup = unknown_workdir_failure(WorkdirFailureOperation::SessionClose);
        assert!(connection.with_secondary(cleanup).prevents_reconnect());
    }

    #[tokio::test]
    async fn revoke_response_loss_retains_unconfirmed_mutation_without_resend() {
        // A 2xx mutation with an invalid result body is not a confirmed revoke,
        // nor evidence that the mutation was not applied.
        let (url, requests, server) =
            spawn_json_server("{invalid HTTP-body provider-secret".into(), 1);
        let error = revoke(&test_server_api_client(&url), "workspace-a", "grant-a")
            .await
            .unwrap_err();
        assert!(matches!(
            &error.cause,
            ShareCause::Grant {
                operation: WorkdirManagementOperation::GrantRevoke,
                error: ClientError::Failure(_),
            }
        ));
        assert!(error.prevents_reconnect());
        let rendered = error.render();
        assert!(
            rendered.contains("mutation result is unconfirmed"),
            "{rendered}"
        );
        assert!(
            rendered.contains("do not automatically resend"),
            "{rendered}"
        );
        assert!(!rendered.contains("provider-secret"), "{rendered}");
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.starts_with("DELETE "), "{request}");
        server.join().unwrap();
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn legacy_grant_error_does_not_disclose_provider_or_http_text() {
        let legacy = server_api::RepositoryApiError::new(
            403,
            "provider-code",
            "provider-secret /private/provider HTTP-body",
            Vec::new(),
        );
        let error = ShareError::grant(
            WorkdirManagementOperation::GrantRevoke,
            ClientError::public(reqwest::StatusCode::FORBIDDEN, legacy.into()),
        );
        let rendered = error.render();
        assert!(rendered.contains("HTTP 403"), "{rendered}");
        assert!(rendered.contains("typed cause unavailable"), "{rendered}");
        assert!(!rendered.contains("provider-secret"), "{rendered}");
        assert!(!rendered.contains("provider-code"), "{rendered}");
    }

    fn unknown_workdir_failure(operation: WorkdirFailureOperation) -> ShareError {
        ShareError {
            cause: ShareCause::Workdir(WorkdirFailure::from_transport_error(
                operation,
                WorkdirTransportError {
                    code: workdir::http::WorkdirTransportErrorCode::OutcomeUnknown,
                    failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
                    denial_reason: None,
                    message: "provider-secret /private/provider".into(),
                },
            )),
            secondary: Vec::new(),
        }
    }

    #[test]
    fn interruption_and_original_connection_failure_survive_cleanup_failure() {
        let interrupted = retain_provider_cleanup(
            Ok(ProviderEnd::Interrupted),
            Err(unknown_workdir_failure(
                WorkdirFailureOperation::SessionClose,
            )),
        )
        .unwrap_err();
        assert!(matches!(
            &interrupted.cause,
            ShareCause::ProviderEnded(ProviderEnd::Interrupted)
        ));
        assert!(interrupted.prevents_reconnect());
        let connection: ShareError = "original connection failure".to_string().into();
        let error = retain_provider_cleanup(
            Err(connection),
            Err(unknown_workdir_failure(
                WorkdirFailureOperation::SessionClose,
            )),
        )
        .unwrap_err();
        assert!(
            matches!(&error.cause, ShareCause::Local(message) if message == "original connection failure")
        );
        assert_eq!(error.secondary.len(), 1);
        assert!(error.prevents_reconnect());
    }

    #[test]
    fn original_unknown_is_not_replaced_by_cleanup_failure_or_its_explanation() {
        let cleanup = ShareError::workdir(
            WorkdirFailureOperation::SessionClose,
            workdir::WorkdirError::Io {
                path: std::path::PathBuf::from("/private/cleanup"),
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "provider-secret",
                ),
            },
        );
        let error = retain_cleanup::<()>(
            Err(unknown_workdir_failure(WorkdirFailureOperation::Write)),
            Err(cleanup),
        )
        .unwrap_err();
        let ShareCause::Workdir(primary) = &error.cause else {
            panic!("original failure lost")
        };
        assert_eq!(
            primary.details().classification,
            workdir::http::WorkdirTransportErrorCode::OutcomeUnknown
        );
        assert_eq!(
            primary.details().failure_reason,
            Some(workdir::WorkdirFailureReason::ProviderResponseLost)
        );
        assert_eq!(error.secondary.len(), 1);
        let rendered = error.render();
        assert!(
            rendered.contains("do not automatically resend"),
            "{rendered}"
        );
        assert!(rendered.contains("session closing"), "{rendered}");
        assert!(!rendered.contains("provider-secret"), "{rendered}");
        assert!(!rendered.contains("/private/cleanup"), "{rendered}");
    }

    #[test]
    fn refresh_observations_do_not_replace_original_failure_or_grow_for_identical_retries() {
        let mut evidence = ShareEvidence::default();
        evidence.retain_failure(unknown_workdir_failure(
            WorkdirFailureOperation::SessionOpen,
        ));
        evidence.retain_failure(unknown_workdir_failure(
            WorkdirFailureOperation::SessionOpen,
        ));
        evidence.retain_failure(ShareError::grant(
            WorkdirManagementOperation::GrantInspect,
            ClientError::Failure(ClientFailure::Timeout),
        ));
        assert_eq!(evidence.failures.len(), 2);
        assert_eq!(evidence.failures[0].1, 2);
        let ShareCause::Workdir(original) = &evidence.failures[0].0.cause else {
            panic!("primary lost")
        };
        assert_eq!(
            original.details().classification,
            workdir::http::WorkdirTransportErrorCode::OutcomeUnknown
        );
        let rendered = evidence.render().join("\n");
        assert!(
            rendered.contains("do not automatically resend"),
            "{rendered}"
        );
        assert!(rendered.contains("grant refresh failed"), "{rendered}");
        assert!(!rendered.contains("provider-secret"), "{rendered}");
    }

    #[test]
    fn grant_settlement_and_secondary_cleanup_diagnostics_remain_independent() {
        use protocol::startup_failure::StartupFailureAttachment;
        let ShareCause::Workdir(failure) =
            unknown_workdir_failure(WorkdirFailureOperation::SessionClose).cause
        else {
            unreachable!()
        };
        let projected = project_failure::<error_contract::Browser, _>(
            &FailureFacts::new(*failure.details()).unwrap(),
            &WorkdirFailureVisibility {
                can_view_cause: true,
            },
        )
        .unwrap();
        let attachment = StartupFailureAttachment::from_projected(&projected).unwrap();
        // Corrupt/future attachments must not hide the settled status or leak bytes.
        let corrupt: StartupFailureAttachment =
            serde_json::from_value(serde_json::json!("{provider-secret /private/cleanup")).unwrap();
        let mut grant: ExternalWorkdirGrantResponse =
            serde_json::from_str(&grant_response_json()).unwrap();
        grant.status = "revoked".into();
        grant.cleanup_state = "retry_required".into();
        grant.cleanup_failures = vec![attachment, corrupt];
        for reason in [
            workdir::management_failure::WorkdirManagementReason::GrantAuditFailed,
            workdir::management_failure::WorkdirManagementReason::GrantCleanupPersistenceFailed,
        ] {
            let facts = FailureFacts::new(WorkdirManagementFailure {
                operation: WorkdirManagementOperation::GrantCleanup,
                reason,
            })
            .unwrap();
            let projected = project_failure::<error_contract::Browser, _>(
                &facts,
                &WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
            grant
                .cleanup_failures
                .push(StartupFailureAttachment::from_projected(&projected).unwrap());
        }
        let evidence = ShareEvidence {
            grants: vec![grant.clone()],
            failures: Vec::new(),
        };
        let rendered = evidence.render().join("\n");
        assert_eq!(evidence.grants[0], grant);
        assert!(rendered.contains("grant status: revoked"), "{rendered}");
        assert!(
            rendered.contains("cleanup state: retry_required"),
            "{rendered}"
        );
        assert!(
            rendered.contains("audit record could not be confirmed"),
            "{rendered}"
        );
        assert!(
            rendered.contains("cleanup observation could not be saved"),
            "{rendered}"
        );
        assert!(
            rendered.contains("do not automatically resend"),
            "{rendered}"
        );
        assert!(
            rendered.contains("diagnostic unavailable or unsupported"),
            "{rendered}"
        );
        assert!(!rendered.contains("provider-secret"), "{rendered}");
        assert!(!rendered.contains("/private/cleanup"), "{rendered}");
    }

    #[tokio::test]
    async fn generated_grant_operations_preserve_auth_and_encode_contract_paths() {
        let (base_url, requests, server) = spawn_json_server(grant_response_json(), 3);
        let client = test_server_api_client(&base_url);

        let created = client
            .external_workdir_grant_create(
                "workspace/a b".to_string(),
                ExternalWorkdirGrantCreateRequest {
                    provider_instance_id: "provider-a".to_string(),
                    display_name: "Shared files".to_string(),
                    ttl_seconds: Some(600),
                    permissions: server_api::ExternalWorkdirPermissions::default(),
                },
            )
            .await
            .unwrap();
        let fetched = fetch_grant(&client, "workspace/a b", "grant/a b")
            .await
            .unwrap();
        let revoked = revoke(&client, "workspace/a b", "grant/a b").await.unwrap();

        assert_eq!(revoked.status, "pending"); // Preserve the actual DTO, not an assumed revoke result.
        assert_eq!(revoked.cleanup_state, "not_required");
        assert_eq!(created.grant_id, "grant/a b");
        assert_eq!(fetched.generation, 7);
        let requests = (0..3)
            .map(|_| requests.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        server.join().unwrap();
        assert!(
            requests[0]
                .starts_with("POST /api/w/workspace%2Fa%20b/external-workdir-grants HTTP/1.1\r\n")
        );
        assert!(requests[1].starts_with(
            "GET /api/w/workspace%2Fa%20b/external-workdir-grants/grant%2Fa%20b HTTP/1.1\r\n"
        ));
        assert!(requests[2].starts_with(
            "DELETE /api/w/workspace%2Fa%20b/external-workdir-grants/grant%2Fa%20b HTTP/1.1\r\n"
        ));
        for request in &requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("\r\nauthorization: bearer test-token\r\n")
            );
        }
        assert!(requests[0].contains("\"provider_instance_id\":\"provider-a\""));
        assert!(requests[0].contains("\"ttl_seconds\":600"));
    }

    #[tokio::test]
    async fn generated_grant_response_is_bounded() {
        let oversized_body = format!("{{\"padding\":\"{}\"}}", "x".repeat(1_048_576));
        let (base_url, requests, server) = spawn_json_server(oversized_body, 1);
        let client = test_server_api_client(&base_url);

        let error = fetch_grant(&client, "workspace-a", "grant-a")
            .await
            .unwrap_err();

        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
        assert!(
            request
                .starts_with("GET /api/w/workspace-a/external-workdir-grants/grant-a HTTP/1.1\r\n")
        );
        let error = error.render();
        assert!(error.contains("External Workdir grant refresh failed"));
        assert!(
            error.contains("response body exceeded 1048576 bytes"),
            "{error}"
        );
    }

    #[test]
    fn relative_share_path_is_resolved_and_pinned_before_backend_access() {
        let directory = tempfile::tempdir().unwrap();
        let pinned = pin_selected_root_from(std::path::Path::new("."), directory.path()).unwrap();
        assert_eq!(
            pinned.canonical_path(),
            directory.path().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn operation_shutdown_waits_until_executor_has_stopped() {
        assert_operation_shutdown(Ok(())).await.unwrap();
    }

    #[tokio::test]
    async fn session_close_failure_still_cancels_and_joins_executors() {
        let error = assert_operation_shutdown(Err(unknown_workdir_failure(
            WorkdirFailureOperation::SessionClose,
        )))
        .await
        .unwrap_err();
        let ShareCause::Workdir(failure) = &error.cause else {
            panic!("close failure was lost")
        };
        assert_eq!(
            failure.details().classification,
            workdir::http::WorkdirTransportErrorCode::OutcomeUnknown
        );
    }

    #[tokio::test]
    async fn executor_join_failure_does_not_skip_remaining_executor_shutdown() {
        let (sender, mut completions) = tokio::sync::mpsc::channel(2);
        let stopped = Arc::new(AtomicBool::new(false));
        let mut operations = HashMap::new();
        for (id, panic_after_completion) in [("op-panics", true), ("op-stops", false)] {
            let operation_id = ExternalWorkdirOperationId::new(id).unwrap();
            let cancelled = Arc::new(AtomicBool::new(false));
            let task_cancelled = cancelled.clone();
            let task_stopped = stopped.clone();
            let sender = sender.clone();
            let thread = std::thread::spawn(move || {
                while !task_cancelled.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                sender
                    .blocking_send(OperationCompletion {
                        operation_id,
                        outcome: ExternalWorkdirOperationOutcome::Cancelled,
                    })
                    .unwrap();
                if panic_after_completion {
                    panic!("injected executor join failure");
                }
                task_stopped.store(true, Ordering::Release);
            });
            operations.insert(
                id.to_string(),
                OperationTask {
                    cancelled,
                    thread: Some(thread),
                },
            );
        }
        drop(sender);
        let error = stop_operations(&mut operations, &mut completions)
            .await
            .unwrap_err();
        assert!(operations.is_empty());
        assert!(stopped.load(Ordering::Acquire));
        assert!(error.render().contains("executor panicked"));
    }

    async fn assert_operation_shutdown(close: Result<(), ShareError>) -> Result<(), ShareError> {
        let operation_id = ExternalWorkdirOperationId::new("op-cancel-test").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (sender, mut completions) = tokio::sync::mpsc::channel(1);
        let task_cancelled = cancelled.clone();
        let task_started = started.clone();
        let task_stopped = stopped.clone();
        let task_operation_id = operation_id.clone();
        let thread = std::thread::spawn(move || {
            task_started.store(true, Ordering::Release);
            while !task_cancelled.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(20));
            task_stopped.store(true, Ordering::Release);
            sender
                .blocking_send(OperationCompletion {
                    operation_id: task_operation_id,
                    outcome: ExternalWorkdirOperationOutcome::Cancelled,
                })
                .unwrap();
        });
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        let mut operations = HashMap::from([(
            operation_id.as_str().to_string(),
            OperationTask {
                cancelled,
                thread: Some(thread),
            },
        )]);

        let result = stop_operations_after_close(close, &mut operations, &mut completions).await;

        assert!(operations.is_empty());
        assert!(stopped.load(Ordering::Acquire));
        result
    }

    #[test]
    fn path_segments_are_encoded_without_exposing_structure() {
        assert_eq!(encode_path_segment("workspace/a b"), "workspace%2Fa%20b");
        assert_eq!(encode_path_segment("safe-_.~"), "safe-_.~");
    }
}
