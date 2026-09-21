use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use client::BackendApiClient;
use futures::{SinkExt, StreamExt};
use server_api::{ExternalWorkdirGrantCreateRequest, ExternalWorkdirGrantResponse};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};
use workdir::external::{
    ExternalProviderInstanceId, ExternalWorkdirGrantId, ExternalWorkdirOperationId,
    ExternalWorkdirOperationOutcome, ExternalWorkdirProviderFrame, ExternalWorkdirProviderMessage,
    ExternalWorkdirProviderRegistration, ExternalWorkdirServerFrame, ExternalWorkdirServerMessage,
};
use workdir::http::{WorkdirTransportError, dispatch_workdir_session_operation};
use workdir::{
    BoundedReadLimits, LocalWorkdirSession, Workdir, WorkdirSession, WorkdirSessionCapabilities,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkdirShareOptions {
    pub path: PathBuf,
    pub workspace_id: String,
    pub backend_url: String,
    pub display_name: String,
    pub ttl: Duration,
}

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
}

impl OperationTask {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for OperationTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(crate) async fn run(options: WorkdirShareOptions) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(&options.path)
        .map_err(|error| format!("selected path is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("selected External Workdir root must not be a symbolic link".to_string());
    }
    let canonical = options
        .path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize selected directory: {error}"))?;
    if !canonical.is_dir() {
        return Err("selected External Workdir path must be a directory".to_string());
    }

    let client = BackendApiClient::from_stored_token(&options.backend_url)
        .map_err(|error| error.to_string())?;
    let provider_instance_id = format!("cli-{}", uuid::Uuid::now_v7());
    let create_path = format!(
        "/api/w/{}/external-workdir-grants",
        encode_path_segment(&options.workspace_id)
    );
    let response = client
        .request(reqwest::Method::POST, &create_path)
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(10))
        .json(&ExternalWorkdirGrantCreateRequest {
            provider_instance_id: provider_instance_id.clone(),
            display_name: options.display_name.clone(),
            ttl_seconds: options.ttl.as_secs(),
            read_only: true,
        })
        .send()
        .await
        .map_err(|error| format!("External Workdir grant request failed: {error}"))?;
    let response = client
        .require_success(response)
        .await
        .map_err(|error| error.to_string())?;
    let grant = response
        .json::<ExternalWorkdirGrantResponse>()
        .await
        .map_err(|error| format!("Backend returned an invalid External Workdir grant: {error}"))?;

    let limits = BoundedReadLimits::EXTERNAL_DEFAULT;
    let session = LocalWorkdirSession::external_read_only(
        Workdir::new(&grant.working_directory_id),
        &canonical,
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
            &client,
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
                let refresh = fetch_grant(&client, &options.workspace_id, &grant.grant_id);
                let refreshed = tokio::select! {
                    signal = tokio::signal::ctrl_c() => {
                        signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
                        revoke(&client, &options.workspace_id, &grant.grant_id).await?;
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
                        if wait_to_reconnect(&client, &options.workspace_id, &grant.grant_id)
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
                if wait_to_reconnect(&client, &options.workspace_id, &grant.grant_id).await? {
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
                if wait_to_reconnect(&client, &options.workspace_id, &grant.grant_id).await? {
                    println!("External Workdir grant revoked.");
                    return Ok(());
                }
            }
        }
    }
}

async fn serve_provider_connection(
    client: &BackendApiClient,
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
    let request = client
        .websocket_request(&provider_path)
        .map_err(|error| error.to_string())?;
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(workdir::external::MAX_EXTERNAL_SERVER_FRAME_BYTES))
        .max_frame_size(Some(workdir::external::MAX_EXTERNAL_SERVER_FRAME_BYTES));
    let connection = connect_async_with_config(request, Some(websocket_config), false);
    let (mut socket, _) = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("failed to wait for Ctrl-C: {error}"))?;
            revoke(client, &options.workspace_id, &grant.grant_id).await?;
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
            revoke(client, &options.workspace_id, &grant.grant_id).await?;
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
                abort_operations(&mut operations);
                revoke(client, &options.workspace_id, &grant.grant_id).await?;
                return Ok(ProviderEnd::Interrupted);
            }
            completion = completions.recv() => {
                let Some(completion) = completion else {
                    abort_operations(&mut operations);
                    return Err("External Workdir operation executor closed".to_string());
                };
                let Some(operation) = operations.remove(completion.operation_id.as_str()) else {
                    continue;
                };
                if operation.is_cancelled() {
                    continue;
                }
                let result = ExternalWorkdirProviderFrame::current(
                    ExternalWorkdirProviderMessage::OperationResult {
                        generation,
                        operation_id: completion.operation_id,
                        outcome: completion.outcome,
                    },
                );
                if send_provider_frame(&mut socket, &result).await.is_err() {
                    abort_operations(&mut operations);
                    return Ok(ProviderEnd::Disconnected);
                }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else {
                    abort_operations(&mut operations);
                    return Ok(ProviderEnd::Disconnected);
                };
                let incoming = match incoming {
                    Ok(incoming) => incoming,
                    Err(_) => {
                        abort_operations(&mut operations);
                        return Ok(ProviderEnd::Disconnected);
                    }
                };
                match incoming {
                    Message::Text(text) => {
                        let frame = serde_json::from_str::<ExternalWorkdirServerFrame>(&text)
                            .map_err(|error| format!("Backend returned an invalid External Workdir frame: {error}"))?;
                        match frame.message {
                            ExternalWorkdirServerMessage::Operation {
                                generation: frame_generation,
                                operation_id,
                                operation,
                            } if frame_generation == generation => {
                                if operations.len() >= 16 || operations.contains_key(operation_id.as_str()) {
                                    abort_operations(&mut operations);
                                    return Err("Backend exceeded External Workdir operation bounds".to_string());
                                }
                                let key = operation_id.as_str().to_string();
                                let completion_sender = completion_sender.clone();
                                let session = session.clone();
                                let task_operation_id = operation_id.clone();
                                let cancelled = Arc::new(AtomicBool::new(false));
                                let operation = operation.into_inner();
                                std::thread::Builder::new()
                                    .name("external-workdir-operation".to_string())
                                    .spawn(move || {
                                        let outcome = futures::executor::block_on(execute_operation(&session, operation));
                                        let _ = completion_sender.blocking_send(OperationCompletion {
                                            operation_id: task_operation_id,
                                            outcome,
                                        });
                                    })
                                    .map_err(|error| format!("failed to start External Workdir operation: {error}"))?;
                                operations.insert(key, OperationTask { cancelled });
                            }
                            ExternalWorkdirServerMessage::Heartbeat {
                                generation: frame_generation,
                                sequence,
                            } if frame_generation == generation => {
                                let heartbeat = ExternalWorkdirProviderFrame::current(
                                    ExternalWorkdirProviderMessage::Heartbeat { generation, sequence },
                                );
                                send_provider_frame(&mut socket, &heartbeat).await?;
                            }
                            ExternalWorkdirServerMessage::Cancel {
                                generation: frame_generation,
                                operation_id,
                            } if frame_generation == generation => {
                                if let Some(operation) = operations.get(operation_id.as_str()) {
                                    operation.cancel();
                                    let cancelled = ExternalWorkdirProviderFrame::current(
                                        ExternalWorkdirProviderMessage::OperationResult {
                                            generation,
                                            operation_id,
                                            outcome: ExternalWorkdirOperationOutcome::Cancelled,
                                        },
                                    );
                                    send_provider_frame(&mut socket, &cancelled).await?;
                                }
                            }
                            ExternalWorkdirServerMessage::Revoke {
                                generation: frame_generation,
                                ..
                            } if frame_generation == generation => {
                                abort_operations(&mut operations);
                                let acknowledged = ExternalWorkdirProviderFrame::current(
                                    ExternalWorkdirProviderMessage::RevokeAcknowledged { generation },
                                );
                                let _ = send_provider_frame(&mut socket, &acknowledged).await;
                                return Ok(ProviderEnd::Revoked);
                            }
                            _ => {
                                abort_operations(&mut operations);
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
                            abort_operations(&mut operations);
                            return Ok(ProviderEnd::Disconnected);
                        }
                    }
                    Message::Close(_) => {
                        abort_operations(&mut operations);
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

fn abort_operations(operations: &mut HashMap<String, OperationTask>) {
    for (_, operation) in operations.drain() {
        operation.cancel();
    }
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
    client: &BackendApiClient,
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
    client: &BackendApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<ExternalWorkdirGrantResponse, String> {
    let path = format!(
        "/api/w/{}/external-workdir-grants/{}",
        encode_path_segment(workspace_id),
        encode_path_segment(grant_id)
    );
    let response = client
        .request(reqwest::Method::GET, &path)
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("External Workdir grant refresh failed: {error}"))?;
    client
        .require_success(response)
        .await
        .map_err(|error| error.to_string())?
        .json::<ExternalWorkdirGrantResponse>()
        .await
        .map_err(|error| format!("Backend returned an invalid External Workdir grant: {error}"))
}

async fn revoke(
    client: &BackendApiClient,
    workspace_id: &str,
    grant_id: &str,
) -> Result<(), String> {
    let path = format!(
        "/api/w/{}/external-workdir-grants/{}",
        encode_path_segment(workspace_id),
        encode_path_segment(grant_id)
    );
    let response = client
        .request(reqwest::Method::DELETE, &path)
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("External Workdir revoke request failed: {error}"))?;
    client.require_success(response).await.map_err(|error| {
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

    #[test]
    fn path_segments_are_encoded_without_exposing_structure() {
        assert_eq!(encode_path_segment("workspace/a b"), "workspace%2Fa%20b");
        assert_eq!(encode_path_segment("safe-_.~"), "safe-_.~");
    }
}
