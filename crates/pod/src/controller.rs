use std::path::Path;
use std::sync::Arc;

use llm_worker::WorkerError;
use llm_worker::llm_client::client::LlmClient;
use session_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::notification_buffer::NotificationBuffer;
use crate::notifier::Notifier;
use crate::pod::{Pod, PodError, PodRunResult};
use crate::runtime_dir::RuntimeDir;
use crate::shared_state::{PodSharedState, PodStatus};
use crate::socket_server::SocketServer;
use protocol::{ErrorCode, Event, Method, NotificationLevel, NotificationSource, RunResult, TurnResult};

// ---------------------------------------------------------------------------
// PodHandle — client-facing, Clone-able
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PodHandle {
    method_tx: mpsc::Sender<Method>,
    event_tx: broadcast::Sender<Event>,
    pub shared_state: Arc<PodSharedState>,
    pub runtime_dir: Arc<RuntimeDir>,
    pub notifier: Notifier,
}

impl PodHandle {
    pub async fn send(&self, method: Method) -> Result<(), mpsc::error::SendError<Method>> {
        self.method_tx.send(method).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    /// Broadcast an event to all listeners (including socket clients).
    pub fn send_event(&self, event: Event) -> Result<usize, broadcast::error::SendError<Event>> {
        self.event_tx.send(event)
    }

    /// Emit a user-facing notification. Thin wrapper over `Notifier::notify`.
    pub fn notify(&self, level: NotificationLevel, source: NotificationSource, message: String) {
        self.notifier.notify(level, source, message);
    }
}

// ---------------------------------------------------------------------------
// PodController — actor that owns a Pod
// ---------------------------------------------------------------------------

pub type ShutdownReceiver = oneshot::Receiver<()>;

pub struct PodController;

impl PodController {
    pub async fn spawn<C, St>(
        mut pod: Pod<C, St>,
        runtime_base: &Path,
    ) -> Result<(PodHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + 'static,
        St: Store + 'static,
    {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (method_tx, mut method_rx) = mpsc::channel::<Method>(32);
        let (event_tx, _) = broadcast::channel::<Event>(256);
        let notifier = Notifier::new(event_tx.clone());

        let manifest_toml = toml::to_string_pretty(pod.manifest()).unwrap_or_default();
        let greeting = build_greeting(&pod);
        let shared_state = Arc::new(PodSharedState::new(
            pod.manifest().pod.name.clone(),
            pod.session_id(),
            manifest_toml.clone(),
            greeting,
        ));

        // Create runtime directory and write initial files
        let runtime_dir = RuntimeDir::create(runtime_base, &pod.manifest().pod.name).await?;
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;
        runtime_dir.write_history(&shared_state).await?;
        let runtime_dir = Arc::new(runtime_dir);

        let handle = PodHandle {
            method_tx,
            event_tx: event_tx.clone(),
            shared_state: shared_state.clone(),
            runtime_dir: runtime_dir.clone(),
            notifier: notifier.clone(),
        };

        // Hand the notifier to the Pod so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        pod.attach_notifier(notifier.clone());

        // Start socket server (lives as a background task, cleaned up on drop via RuntimeDir)
        let _socket_server = SocketServer::start(&handle).await?;
        // Keep the server alive by moving it into the controller task
        // (it will be dropped when the task ends)

        // Grab the scope/pwd before the mutable borrow of the worker so we
        // can build a `ScopedFs` for the builtin tools.
        let scope_for_tools = pod.scope().clone();
        let pwd_for_tools = pod.pwd().to_path_buf();

        // Register event bridge callbacks on the worker
        {
            let worker = pod.worker_mut();

            let tx = event_tx.clone();
            worker.on_turn_start(move |turn| {
                let _ = tx.send(Event::TurnStart { turn });
            });

            let tx = event_tx.clone();
            worker.on_turn_end(move |turn| {
                let _ = tx.send(Event::TurnEnd {
                    turn,
                    result: TurnResult::Finished,
                });
            });

            let tx = event_tx.clone();
            worker.on_text_block(move |block| {
                let tx_d = tx.clone();
                block.on_delta(move |text| {
                    let _ = tx_d.send(Event::TextDelta {
                        text: text.to_owned(),
                    });
                });
                let tx_s = tx.clone();
                block.on_stop(move |text| {
                    let _ = tx_s.send(Event::TextDone {
                        text: text.to_owned(),
                    });
                });
            });

            let tx = event_tx.clone();
            worker.on_tool_use_block(move |start, block| {
                let _ = tx.send(Event::ToolCallStart {
                    id: start.id.clone(),
                    name: start.name.clone(),
                });
                let id_for_delta = start.id.clone();
                let tx_d = tx.clone();
                block.on_delta(move |json| {
                    let _ = tx_d.send(Event::ToolCallArgsDelta {
                        id: id_for_delta.clone(),
                        json: json.to_owned(),
                    });
                });
                let tx_s = tx.clone();
                block.on_stop(move |call| {
                    let _ = tx_s.send(Event::ToolCallDone {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.input.to_string(),
                    });
                });
            });

            let tx = event_tx.clone();
            worker.on_usage(move |event| {
                let _ = tx.send(Event::Usage {
                    input_tokens: event.input_tokens,
                    output_tokens: event.output_tokens,
                });
            });

            let tx = event_tx.clone();
            worker.on_error(move |event| {
                let _ = tx.send(Event::Error {
                    code: ErrorCode::ProviderError,
                    message: event.message.clone(),
                });
            });

            let notifier_for_worker = notifier.clone();
            worker.on_warning(move |message| {
                notifier_for_worker.notify(
                    NotificationLevel::Warn,
                    NotificationSource::Worker,
                    message.to_owned(),
                );
            });

            // Register the builtin file-manipulation tools (Read / Write /
            // Edit / Glob / Grep). `ScopedFs` carries the pod-lifetime
            // scope/pwd; `Tracker` is session-scoped — a fresh instance per
            // controller spawn ensures state from a previous process
            // lifetime cannot be reused after a resume. The tracker is
            // also handed to the Pod itself so Pod-level operations (e.g.
            // context compaction) can ask which files the agent has been
            // touching.
            let fs = tools::ScopedFs::new(scope_for_tools, pwd_for_tools);
            let tracker = tools::Tracker::new();
            worker.register_tools(tools::builtin_tools(fs, tracker.clone()));
            pod.attach_tracker(tracker);
        }

        // Clone cancel sender and notification buffer before moving pod
        // into the controller task so the main loop can route
        // `Method::Notify` into the buffer even while `pod` is held by
        // an in-flight `run_for_notification` / `run` future.
        let cancel_tx = pod.worker_mut().cancel_sender();
        let notification_buffer = pod.notification_buffer_handle();

        tokio::spawn(async move {
            // Hold socket server alive for the lifetime of the controller task
            let _socket_server = _socket_server;

            loop {
                let method = match method_rx.recv().await {
                    Some(m) => m,
                    None => break,
                };

                match method {
                    Method::Run { input } => {
                        if shared_state.get_status() != PodStatus::Idle {
                            let _ = event_tx.send(Event::Error {
                                code: ErrorCode::AlreadyRunning,
                                message: "Pod is already executing a turn".into(),
                            });
                            continue;
                        }
                        shared_state.set_status(PodStatus::Running);
                        let _ = runtime_dir.write_status(&shared_state).await;

                        let (new_status, shutdown) = run_with_cancel_support(
                            pod.run(&input),
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                            &notification_buffer,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                notifier.notify(
                                    NotificationLevel::Warn,
                                    NotificationSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;

                        if shutdown {
                            let _ = event_tx.send(Event::Shutdown);
                            break;
                        }
                    }

                    Method::Notify { source, message } => {
                        pod.push_notification(source, message);
                        if shared_state.get_status() != PodStatus::Idle {
                            // RUNNING / Paused: the buffer push is the
                            // entire operation; the in-flight turn (or
                            // next Resume) will drain the buffer at its
                            // next pre_llm_request.
                            continue;
                        }
                        // IDLE: auto-start a turn so the LLM sees the
                        // buffered notification(s) without a human Run.
                        shared_state.set_status(PodStatus::Running);
                        let _ = runtime_dir.write_status(&shared_state).await;

                        let (new_status, shutdown) = run_with_cancel_support(
                            pod.run_for_notification(),
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                            &notification_buffer,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                notifier.notify(
                                    NotificationLevel::Warn,
                                    NotificationSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;

                        if shutdown {
                            let _ = event_tx.send(Event::Shutdown);
                            break;
                        }
                    }

                    Method::Resume => {
                        if shared_state.get_status() != PodStatus::Paused {
                            let _ = event_tx.send(Event::Error {
                                code: ErrorCode::NotPaused,
                                message: "Pod is not paused".into(),
                            });
                            continue;
                        }
                        shared_state.set_status(PodStatus::Running);
                        let _ = runtime_dir.write_status(&shared_state).await;

                        let (new_status, shutdown) = run_with_cancel_support(
                            pod.resume(),
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                            &notification_buffer,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                notifier.notify(
                                    NotificationLevel::Warn,
                                    NotificationSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;

                        if shutdown {
                            let _ = event_tx.send(Event::Shutdown);
                            break;
                        }
                    }

                    Method::Cancel => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::NotRunning,
                            message: "Pod is not running".into(),
                        });
                    }

                    Method::Shutdown => {
                        let _ = event_tx.send(Event::Shutdown);
                        break;
                    }

                    // GetHistory is handled at the socket layer (direct response).
                    // If it somehow reaches the controller, ignore it.
                    Method::GetHistory => {}
                }
            }

            let _ = shutdown_tx.send(());
        });

        Ok((handle, shutdown_rx))
    }
}

/// Runs a Pod future while concurrently processing incoming methods.
///
/// Returns `(final_status, shutdown_requested)`.
async fn run_with_cancel_support<F>(
    pod_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    shared_state: &Arc<PodSharedState>,
    notification_buffer: &NotificationBuffer,
) -> (PodStatus, bool)
where
    F: std::future::Future<Output = Result<PodRunResult, PodError>>,
{
    tokio::pin!(pod_future);
    let mut shutdown_requested = false;

    loop {
        tokio::select! {
            result = &mut pod_future => {
                return match result {
                    Ok(r) => {
                        let (status, run_result) = match r {
                            PodRunResult::Finished => (PodStatus::Idle, RunResult::Finished),
                            PodRunResult::Paused => (PodStatus::Paused, RunResult::Paused),
                            PodRunResult::LimitReached => (PodStatus::Idle, RunResult::LimitReached),
                        };
                        let _ = event_tx.send(Event::RunEnd { result: run_result });
                        (status, shutdown_requested)
                    }
                    Err(e) => {
                        let code = worker_error_code(&e);
                        let _ = event_tx.send(Event::Error {
                            code,
                            message: e.to_string(),
                        });
                        (PodStatus::Idle, shutdown_requested)
                    }
                };
            }
            method = method_rx.recv() => {
                match method {
                    Some(Method::Cancel) => {
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Shutdown) => {
                        shutdown_requested = true;
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Run { .. } | Method::Resume) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Pod is already executing a turn".into(),
                        });
                    }
                    Some(Method::Notify { source, message }) => {
                        // Route into the buffer; the in-flight turn will
                        // drain it at its next pre_llm_request.
                        notification_buffer.push(source, message);
                    }
                    Some(Method::GetHistory) => {}
                    None => {
                        let _ = cancel_tx.try_send(());
                        shared_state.set_status(PodStatus::Idle);
                        return (PodStatus::Idle, false);
                    }
                }
            }
        }
    }
}

fn build_greeting<C, St>(pod: &Pod<C, St>) -> protocol::Greeting
where
    C: LlmClient,
    St: Store,
{
    let manifest = pod.manifest();
    let provider = match manifest.provider.kind {
        manifest::ProviderKind::Anthropic => "anthropic",
        manifest::ProviderKind::Openai => "openai",
        manifest::ProviderKind::Gemini => "gemini",
        manifest::ProviderKind::Ollama => "ollama",
    };
    // The tool list mirrors `builtin_tools`. A fresh `ScopedFs`/`Tracker`
    // is instantiated only to invoke the factories for name extraction;
    // the instances themselves are discarded.
    let fs = tools::ScopedFs::new(pod.scope().clone(), pod.pwd().to_path_buf());
    let tracker = tools::Tracker::new();
    let tool_names = tools::builtin_tools(fs, tracker)
        .iter()
        .map(|def| def().0.name)
        .collect();
    protocol::Greeting {
        pod_name: manifest.pod.name.clone(),
        cwd: pod.pwd().display().to_string(),
        provider: provider.into(),
        model: manifest.provider.model.clone(),
        scope_summary: pod.scope().summary(),
        tools: tool_names,
    }
}

fn worker_error_code(e: &PodError) -> ErrorCode {
    match e {
        PodError::Worker(we) => match we {
            WorkerError::Tool(_) => ErrorCode::ToolError,
            WorkerError::Client(_) => ErrorCode::ProviderError,
            _ => ErrorCode::Internal,
        },
        PodError::Provider(_) => ErrorCode::ProviderError,
        _ => ErrorCode::Internal,
    }
}
