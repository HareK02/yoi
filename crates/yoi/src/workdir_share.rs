use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use client::BackendApiClient;
use futures::{SinkExt, StreamExt};
use server_api::{
    ExternalWorkdirGrantCreateRequest, ExternalWorkdirGrantResponse, ServerApiClient,
};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};
use workdir::external::{
    ExternalProviderInstanceId, ExternalWorkdirGrantId, ExternalWorkdirOperationId,
    ExternalWorkdirOperationOutcome, ExternalWorkdirProviderFrame, ExternalWorkdirProviderMessage,
    ExternalWorkdirProviderRegistration, ExternalWorkdirServerFrame, ExternalWorkdirServerMessage,
};
use workdir::http::{WorkdirTransportError, dispatch_workdir_session_operation};
use workdir::{
    BoundedReadLimits, ExternalWorkdirRoot, LocalWorkdirSession, Workdir, WorkdirSession,
    WorkdirSessionCapabilities,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkdirShareOptions {
    pub path: PathBuf,
    pub workspace_id: String,
    pub backend_url: String,
    pub display_name: String,
    pub ttl: Duration,
}

const PROVIDER_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderEnd {
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

fn pin_selected_root(path: &std::path::Path) -> Result<ExternalWorkdirRoot, String> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("failed to resolve current directory: {error}"))?;
    pin_selected_root_from(path, &cwd)
}

fn pin_selected_root_from(
    path: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<ExternalWorkdirRoot, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    ExternalWorkdirRoot::pin(absolute).map_err(|error| error.to_string())
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

pub(crate) async fn run(options: WorkdirShareOptions) -> Result<(), String> {
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
                ttl_seconds: options.ttl.as_secs(),
                read_only: true,
            },
        )
        .await
        .map_err(|error| format!("External Workdir grant request failed: {error}"))?;

    let limits = BoundedReadLimits::EXTERNAL_DEFAULT;
    let session = LocalWorkdirSession::external_read_only_pinned(
        Workdir::new(&grant.working_directory_id),
        pinned_root,
        limits,
    )
    .map_err(|error| error.to_string())?;

    println!("External Workdir grant: {}", grant.grant_id);
    println!("Logical Workdir: {}", grant.working_directory_id);
    println!("Display name: {}", grant.display_name);
    println!("Permission: {}", grant.permissions);
    println!("Expires at: {}", grant.expires_at);

    let expires_at = chrono::DateTime::parse_from_rfc3339(&grant.expires_at)
        .map_err(|_| "Backend returned an invalid External Workdir expiry".to_string())?
        .with_timezone(&Utc);
    let mut generation = grant.generation;
    loop {
        let outcome = serve_provider_connection(
            &backend_client,
            &server_api_client,
            &options,
            &grant,
            &provider_instance_id,
            &session,
            generation,
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(connection_error) => {
                if Utc::now() >= expires_at {
                    return Err(format!(
                        "External Workdir grant expired after provider connection failure: {connection_error}"
                    ));
                }
                let refresh =
                    fetch_grant(&server_api_client, &options.workspace_id, &grant.grant_id);
                let refreshed = tokio::select! {
                    signal = tokio::signal::ctrl_c() => {
                        signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
                        revoke(&server_api_client, &options.workspace_id, &grant.grant_id).await?;
                        println!("External Workdir grant revoked.");
                        return Ok(());
                    }
                    refreshed = refresh => refreshed,
                };
                let current = match refreshed {
                    Ok(current) => current,
                    Err(refresh_error) => {
                        eprintln!(
                            "Connection refresh failed; retrying: {connection_error}; {refresh_error}"
                        );
                        if wait_to_reconnect(
                            &server_api_client,
                            &options.workspace_id,
                            &grant.grant_id,
                        )
                        .await?
                        {
                            println!("External Workdir grant revoked.");
                            return Ok(());
                        }
                        continue;
                    }
                };
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
                    status => {
                        return Err(format!(
                            "Backend returned unknown External Workdir grant status `{status}`"
                        ));
                    }
                }
                println!("Connection: reconnecting");
                if wait_to_reconnect(&server_api_client, &options.workspace_id, &grant.grant_id)
                    .await?
                {
                    println!("External Workdir grant revoked.");
                    return Ok(());
                }
                continue;
            }
        };
        match outcome {
            ProviderEnd::Interrupted => {
                println!("External Workdir grant revoked.");
                return Ok(());
            }
            ProviderEnd::Revoked => {
                println!("External Workdir grant is no longer active.");
                return Ok(());
            }
            ProviderEnd::Disconnected => {
                if Utc::now() >= expires_at {
                    return Err("External Workdir grant expired while disconnected".to_string());
                }
                generation = generation
                    .checked_add(1)
                    .ok_or_else(|| "External Workdir provider generation overflow".to_string())?;
                println!("Connection: reconnecting");
                if wait_to_reconnect(&server_api_client, &options.workspace_id, &grant.grant_id)
                    .await?
                {
                    println!("External Workdir grant revoked.");
                    return Ok(());
                }
            }
        }
    }
}

async fn serve_provider_connection(
    backend_client: &BackendApiClient,
    server_api_client: &ServerApiClient,
    options: &WorkdirShareOptions,
    grant: &ExternalWorkdirGrantResponse,
    provider_instance_id: &str,
    session: &LocalWorkdirSession,
    generation: u64,
) -> Result<ProviderEnd, String> {
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
    let (mut socket, _) = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            revoke(server_api_client, &options.workspace_id, &grant.grant_id).await?;
            return Ok(ProviderEnd::Interrupted);
        }
        connection = connection => connection
            .map_err(|error| format!("External Workdir provider connection failed: {error}"))?,
    };
    let registration =
        ExternalWorkdirProviderFrame::current(ExternalWorkdirProviderMessage::Register {
            registration: ExternalWorkdirProviderRegistration {
                provider_instance_id: ExternalProviderInstanceId::new(provider_instance_id)
                    .map_err(|error| error.to_string())?,
                grant_id: ExternalWorkdirGrantId::new(grant.grant_id.clone())
                    .map_err(|error| error.to_string())?,
                workdir_id: session.workdir().id().clone(),
                generation,
                capabilities: WorkdirSessionCapabilities::READ_ONLY,
                read_limits: BoundedReadLimits::EXTERNAL_DEFAULT,
            },
        });
    send_provider_frame(&mut socket, &registration).await?;
    let registered = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            revoke(server_api_client, &options.workspace_id, &grant.grant_id).await?;
            return Ok(ProviderEnd::Interrupted);
        }
        registered = tokio::time::timeout(Duration::from_secs(10), socket.next()) => {
            registered
                .map_err(|_| "Backend timed out External Workdir provider registration".to_string())?
                .ok_or_else(|| "Backend closed the External Workdir provider connection".to_string())?
                .map_err(|error| format!("External Workdir provider registration failed: {error}"))?
        }
    };
    let Message::Text(registered) = registered else {
        return Err("Backend returned an invalid External Workdir registration frame".to_string());
    };
    let frame = serde_json::from_str::<ExternalWorkdirServerFrame>(&registered)
        .map_err(|error| format!("Backend returned an invalid External Workdir frame: {error}"))?;
    if !matches!(
        frame.message,
        ExternalWorkdirServerMessage::Registered { generation: registered_generation, .. }
            if registered_generation == generation
    ) {
        return Err("Backend rejected the External Workdir provider registration".to_string());
    }
    println!("Connection: online (generation {generation})");

    let (completion_sender, mut completions) =
        tokio::sync::mpsc::channel::<OperationCompletion>(16);
    let mut operations = HashMap::<String, OperationTask>::new();
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
                stop_operations(&mut operations, &mut completions).await?;
                revoke(server_api_client, &options.workspace_id, &grant.grant_id).await?;
                return Ok(ProviderEnd::Interrupted);
            }
            completion = completions.recv() => {
                let Some(completion) = completion else {
                    cancel_operations(&operations);
                    return Err("External Workdir operation executor closed".to_string());
                };
                let Some(operation) = operations.remove(completion.operation_id.as_str()) else {
                    continue;
                };
                let cancelled = match operation.join() {
                    Ok(cancelled) => cancelled,
                    Err(error) => {
                        stop_operations(&mut operations, &mut completions).await?;
                        return Err(error);
                    }
                };
                let outcome = if cancelled {
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
                    stop_operations(&mut operations, &mut completions).await?;
                    return Ok(ProviderEnd::Disconnected);
                }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else {
                    stop_operations(&mut operations, &mut completions).await?;
                    return Ok(ProviderEnd::Disconnected);
                };
                let incoming = match incoming {
                    Ok(incoming) => incoming,
                    Err(_) => {
                        stop_operations(&mut operations, &mut completions).await?;
                        return Ok(ProviderEnd::Disconnected);
                    }
                };
                match incoming {
                    Message::Text(text) => {
                        let frame = match serde_json::from_str::<ExternalWorkdirServerFrame>(&text) {
                            Ok(frame) => frame,
                            Err(error) => {
                                stop_operations(&mut operations, &mut completions).await?;
                                return Err(format!("Backend returned an invalid External Workdir frame: {error}"));
                            }
                        };
                        match frame.message {
                            ExternalWorkdirServerMessage::Operation {
                                generation: frame_generation,
                                operation_id,
                                operation,
                            } if frame_generation == generation => {
                                if operations.len() >= 16 || operations.contains_key(operation_id.as_str()) {
                                    stop_operations(&mut operations, &mut completions).await?;
                                    return Err("Backend exceeded External Workdir operation bounds".to_string());
                                }
                                let key = operation_id.as_str().to_string();
                                let completion_sender = completion_sender.clone();
                                let task_operation_id = operation_id.clone();
                                let cancelled = Arc::new(AtomicBool::new(false));
                                let guarded_session = session.with_operation_guard(
                                    cancelled.clone(),
                                    Instant::now() + PROVIDER_OPERATION_TIMEOUT,
                                );
                                let operation = operation.into_inner();
                                let spawned = std::thread::Builder::new()
                                    .name("external-workdir-operation".to_string())
                                    .spawn(move || {
                                        let outcome = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| {
                                                futures::executor::block_on(execute_operation(
                                                    &guarded_session,
                                                    operation,
                                                ))
                                            }),
                                        )
                                        .unwrap_or_else(|_| ExternalWorkdirOperationOutcome::Failed {
                                            error: WorkdirTransportError {
                                                code: workdir::http::WorkdirTransportErrorCode::Internal,
                                                message: "External Workdir operation executor failed".to_string(),
                                            },
                                        });
                                        let _ = completion_sender.blocking_send(OperationCompletion {
                                            operation_id: task_operation_id,
                                            outcome,
                                        });
                                    });
                                let thread = match spawned {
                                    Ok(thread) => thread,
                                    Err(error) => {
                                        stop_operations(&mut operations, &mut completions).await?;
                                        return Err(format!("failed to start External Workdir operation: {error}"));
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
                                    stop_operations(&mut operations, &mut completions).await?;
                                    return Ok(ProviderEnd::Disconnected);
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
                                stop_operations(&mut operations, &mut completions).await?;
                                let acknowledged = ExternalWorkdirProviderFrame::current(
                                    ExternalWorkdirProviderMessage::RevokeAcknowledged { generation },
                                );
                                let _ = send_provider_frame(&mut socket, &acknowledged).await;
                                return Ok(ProviderEnd::Revoked);
                            }
                            _ => {
                                stop_operations(&mut operations, &mut completions).await?;
                                return Err("stale or unexpected External Workdir provider frame".to_string());
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
                            stop_operations(&mut operations, &mut completions).await?;
                            return Ok(ProviderEnd::Disconnected);
                        }
                    }
                    Message::Close(_) => {
                        stop_operations(&mut operations, &mut completions).await?;
                        return Ok(ProviderEnd::Disconnected);
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn execute_operation(
    session: &LocalWorkdirSession,
    operation: workdir::http::WorkdirSessionOperation,
) -> ExternalWorkdirOperationOutcome {
    match dispatch_workdir_session_operation(session, operation).await {
        Ok(result) => match result.try_into() {
            Ok(result) => ExternalWorkdirOperationOutcome::Completed { result },
            Err(message) => ExternalWorkdirOperationOutcome::Failed {
                error: WorkdirTransportError {
                    code: workdir::http::WorkdirTransportErrorCode::Unsupported,
                    message,
                },
            },
        },
        Err(error) => ExternalWorkdirOperationOutcome::Failed {
            error: WorkdirTransportError::from_workdir_error(&error),
        },
    }
}

fn cancel_operations(operations: &HashMap<String, OperationTask>) {
    for operation in operations.values() {
        operation.cancel();
    }
}

async fn stop_operations(
    operations: &mut HashMap<String, OperationTask>,
    completions: &mut tokio::sync::mpsc::Receiver<OperationCompletion>,
) -> Result<(), String> {
    cancel_operations(operations);
    while !operations.is_empty() {
        let completion = completions.recv().await.ok_or_else(|| {
            "External Workdir operation executor closed during cancellation".to_string()
        })?;
        if let Some(operation) = operations.remove(completion.operation_id.as_str()) {
            operation.join()?;
        }
    }
    Ok(())
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
        .map_err(|error| format!("failed to send External Workdir provider frame: {error}"))
}

async fn wait_to_reconnect(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<bool, String> {
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            revoke(client, workspace_id, grant_id).await?;
            Ok(true)
        }
        _ = tokio::time::sleep(Duration::from_millis(250)) => Ok(false),
    }
}

async fn fetch_grant(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<ExternalWorkdirGrantResponse, String> {
    client
        .external_workdir_grant_get(workspace_id.to_string(), grant_id.to_string())
        .await
        .map_err(|error| format!("External Workdir grant refresh failed: {error}"))
}

async fn revoke(
    client: &ServerApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<(), String> {
    client
        .external_workdir_grant_revoke(workspace_id.to_string(), grant_id.to_string())
        .await
        .map_err(|error| {
            format!("External Workdir revoke was not confirmed by the Backend: {error}")
        })?;
    Ok(())
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for _ in 0..response_count {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                let status = if request.starts_with("POST ") {
                    "201 Created"
                } else {
                    "200 OK"
                };
                sender.send(request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
            "permissions": "read_only",
            "expires_at": "2026-09-23T03:00:00Z",
            "generation": 7,
            "status": "pending"
        })
        .to_string()
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
                    ttl_seconds: 600,
                    read_only: true,
                },
            )
            .await
            .unwrap();
        let fetched = fetch_grant(&client, "workspace/a b", "grant/a b")
            .await
            .unwrap();
        revoke(&client, "workspace/a b", "grant/a b").await.unwrap();

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

        stop_operations(&mut operations, &mut completions)
            .await
            .unwrap();

        assert!(operations.is_empty());
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn path_segments_are_encoded_without_exposing_structure() {
        assert_eq!(encode_path_segment("workspace/a b"), "workspace%2Fa%20b");
        assert_eq!(encode_path_segment("safe-_.~"), "safe-_.~");
    }
}
