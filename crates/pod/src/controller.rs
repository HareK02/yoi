use std::path::Path;
use std::sync::Arc;

use llm_worker::WorkerError;
use llm_worker::llm_client::client::LlmClient;
use session_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::pod::{Pod, PodError, PodRunResult};
use crate::runtime::dir::RuntimeDir;
use crate::shared_state::{PodSharedState, PodStatus};
use crate::spawn::comm_tools::{
    list_pods_tool, read_pod_output_tool, send_to_pod_tool, stop_pod_tool,
};
use crate::spawn::registry::SpawnedPodRegistry;
use crate::spawn::tool::spawn_pod_tool;
use protocol::{AlertLevel, AlertSource, ErrorCode, Event, Method, RunResult, TurnResult};

// ---------------------------------------------------------------------------
// PodHandle — client-facing, Clone-able
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PodHandle {
    method_tx: mpsc::Sender<Method>,
    event_tx: broadcast::Sender<Event>,
    pub shared_state: Arc<PodSharedState>,
    pub runtime_dir: Arc<RuntimeDir>,
    pub alerter: Alerter,
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

    /// Emit a user-facing alert. Thin wrapper over `Alerter::alert`.
    pub fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        self.alerter.alert(level, source, message);
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
        let alerter = Alerter::new(event_tx.clone());

        // Runtime directory is created before tool registration because
        // the spawn-tool factories need its socket path, and before the
        // initial status/history writes because those writes consume the
        // greeting we build after registration is complete.
        let runtime_dir =
            Arc::new(RuntimeDir::create(runtime_base, &pod.manifest().pod.name).await?);

        // Snapshot pod-immutable values needed for tool factories so the
        // mutable worker borrow below doesn't conflict with reads on `pod`.
        let scope_for_tools = pod.scope().clone();
        let pwd_for_tools = pod.pwd().to_path_buf();
        let spawner_name = pod.manifest().pod.name.clone();
        let spawner_model = pod.manifest().model.clone();
        let memory_config = pod.manifest().memory.clone();

        // Parent callback socket (this Pod's own parent, used for
        // `PodEvent` upward reports). `None` for top-level Pods.
        let self_parent_socket = pod.callback_socket().cloned();

        // `SpawnedPodRegistry` is shared between the Pod-orchestration
        // tools (registered below) and the main loop's `PodEvent`
        // handler (added later in this function), so hoist its creation
        // above the worker-borrow block.
        let spawner_socket = runtime_dir.socket_path();
        let spawned_registry = SpawnedPodRegistry::new(runtime_dir.clone());

        // Hand the alerter to the Pod so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        pod.attach_alerter(alerter.clone());
        // Also hand the raw broadcast sender so Pod-internal operations
        // can emit typed lifecycle `Event`s (currently: compact progress).
        pod.attach_event_tx(event_tx.clone());

        // Stashed during tool registration below so we can attach a
        // `PodFsView` to the shared state once the latter exists.
        let fs_for_view: tools::ScopedFs;

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
            worker.on_thinking_block(move |block| {
                // Start fires unconditionally so the TUI can show
                // "Thinking..." even when the provider doesn't emit
                // plaintext deltas.
                let _ = tx.send(Event::ThinkingStart);
                let tx_d = tx.clone();
                block.on_delta(move |text| {
                    let _ = tx_d.send(Event::ThinkingDelta {
                        text: text.to_owned(),
                    });
                });
                let tx_s = tx.clone();
                block.on_stop(move |text| {
                    let _ = tx_s.send(Event::ThinkingDone {
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
            worker.on_tool_result(move |result| {
                let _ = tx.send(Event::ToolResult {
                    id: result.tool_use_id.clone(),
                    summary: result.summary.clone(),
                    output: result.content.clone(),
                    is_error: result.is_error,
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

            let alerter_for_worker = alerter.clone();
            worker.on_warning(move |message| {
                alerter_for_worker.alert(AlertLevel::Warn, AlertSource::Worker, message.to_owned());
            });

            // Register the builtin file-manipulation tools (Read / Write /
            // Edit / Glob / Grep / Bash). `ScopedFs` carries the pod-
            // lifetime scope/pwd; `Tracker` is session-scoped — a fresh
            // instance per controller spawn ensures state from a previous
            // process lifetime cannot be reused after a resume. The tracker
            // is also handed to the Pod itself so Pod-level operations (e.g.
            // context compaction) can ask which files the agent has been
            // touching.
            //
            // Bash spills long outputs to a per-pod subdir under the
            // runtime dir. We layer a recursive `allow(Read)` rule for
            // that path on top of the user-facing scope so the agent can
            // `Read` the saved files without polluting the workspace.
            // Same approach memory takes for its deny rules: round-trip
            // through `ScopeConfig` and rebuild via `from_config`.
            let bash_output_dir = runtime_dir.path().join("bash-output");
            std::fs::create_dir_all(&bash_output_dir).map_err(|e| {
                std::io::Error::other(format!(
                    "create bash output dir {}: {e}",
                    bash_output_dir.display()
                ))
            })?;
            let mut scope_config = manifest::ScopeConfig {
                allow: scope_for_tools.allow_rules(),
                deny: scope_for_tools.deny_rules(),
            };
            scope_config.allow.push(manifest::ScopeRule {
                target: bash_output_dir.clone(),
                permission: manifest::Permission::Read,
                recursive: true,
            });
            let scope_with_bash = manifest::Scope::from_config(&scope_config)
                .map_err(std::io::Error::other)?;
            let fs = tools::ScopedFs::new(scope_with_bash, pwd_for_tools.clone());
            let tracker = tools::Tracker::new();
            // The same ScopedFs also powers the IPC `ListCompletions`
            // query — keep a clone for the FS view we attach below,
            // since the tools consume `fs` itself.
            fs_for_view = fs.clone();
            worker.register_tools(tools::builtin_tools(
                fs,
                tracker.clone(),
                bash_output_dir,
            ));

            // Memory subsystem opt-in. When `[memory]` is present in
            // the manifest, register the memory-specific Read/Write/Edit
            // tools that target `<workspace>/memory/` and
            // `<workspace>/knowledge/` with their built-in linter. The
            // companion deny rules on the generic CRUD scope were
            // already applied during `Pod::from_manifest`.
            if let Some(mem) = memory_config.as_ref() {
                let layout = memory::WorkspaceLayout::resolve(mem, &pwd_for_tools);
                let query_cfg = memory::tool::QueryConfig::from(mem);
                worker.register_tool(memory::tool::read_tool(layout.clone()));
                worker.register_tool(memory::tool::write_tool(layout.clone()));
                worker.register_tool(memory::tool::edit_tool(layout.clone()));
                worker.register_tool(memory::tool::memory_query_tool(layout.clone(), query_cfg));
                worker.register_tool(memory::tool::knowledge_query_tool(layout, query_cfg));
            }

            // Pod-orchestration tools (SpawnPod + the four comm tools)
            // share the Pod-scoped `SpawnedPodRegistry` hoisted above
            // (also consumed by the main loop's `PodEvent` handler).
            worker.register_tool(spawn_pod_tool(
                spawner_name.clone(),
                spawner_socket.clone(),
                runtime_base.to_path_buf(),
                pwd_for_tools,
                spawned_registry.clone(),
                self_parent_socket.clone(),
                spawner_model.clone(),
            ));
            worker.register_tool(send_to_pod_tool(spawned_registry.clone()));
            worker.register_tool(read_pod_output_tool(spawned_registry.clone()));
            worker.register_tool(stop_pod_tool(spawned_registry.clone()));
            worker.register_tool(list_pods_tool(spawned_registry.clone()));
            pod.attach_tracker(tracker);
        }

        // Materialise pending tool factories so the greeting reflects
        // the actual registered set instead of a hand-maintained mirror.
        pod.worker().tool_server_handle().flush_pending();

        // Greeting + initial runtime files now that the tool list is final.
        let manifest_toml = toml::to_string_pretty(pod.manifest()).unwrap_or_default();
        let greeting = build_greeting(&pod);
        let shared_state = Arc::new(PodSharedState::new(
            pod.manifest().pod.name.clone(),
            pod.session_id(),
            manifest_toml.clone(),
            greeting,
        ));
        shared_state.update_history(pod.worker().history().to_vec());
        shared_state.set_user_segments(pod.user_segments().to_vec());
        shared_state.set_fs_view(crate::fs_view::PodFsView::new(fs_for_view));
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;
        runtime_dir.write_history(&shared_state).await?;

        let handle = PodHandle {
            method_tx,
            event_tx: event_tx.clone(),
            shared_state: shared_state.clone(),
            runtime_dir: runtime_dir.clone(),
            alerter: alerter.clone(),
        };

        // Start socket server (lives as a background task, cleaned up on
        // drop via RuntimeDir). Kept alive by moving it into the
        // controller task so it drops when that task ends.
        let _socket_server = SocketServer::start(&handle).await?;

        // Clone cancel sender and notification buffer before moving pod
        // into the controller task so the main loop can route
        // `Method::Notify` into the buffer even while `pod` is held by
        // an in-flight `run_for_notification` / `run` future.
        let cancel_tx = pod.worker_mut().cancel_sender();
        let notify_buffer = pod.notify_buffer_handle();

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
                        let status_before = shared_state.get_status();
                        if status_before == PodStatus::Running {
                            let _ = event_tx.send(Event::Error {
                                code: ErrorCode::AlreadyRunning,
                                message: "Pod is already executing a turn".into(),
                            });
                            continue;
                        }
                        // Broadcast the accepted user message so every
                        // subscriber (including the submitter) can
                        // render the turn header + user line from a
                        // single source of truth. shared_state's
                        // `user_segments` is re-synced from `pod` after
                        // the run completes, so we don't push here.
                        let _ = event_tx.send(Event::UserMessage {
                            segments: input.clone(),
                        });
                        let was_paused = status_before == PodStatus::Paused;
                        shared_state.set_status(PodStatus::Running);
                        let _ = runtime_dir.write_status(&shared_state).await;

                        let run_future = async {
                            if was_paused {
                                pod.interrupt_and_run(input).await
                            } else {
                                pod.run(input).await
                            }
                        };
                        let (new_status, shutdown) = run_with_cancel_support(
                            run_future,
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                            &notify_buffer,
                            self_parent_socket.as_ref(),
                            &spawner_name,
                            &spawned_registry,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_extract().await {
                                tracing::warn!(error = %e, "Post-run memory extract error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory extract error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_consolidate().await {
                                tracing::warn!(error = %e, "Post-run memory consolidate error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory consolidate error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_user_segments(pod.user_segments().to_vec());
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;

                        if shutdown {
                            let _ = event_tx.send(Event::Shutdown);
                            break;
                        }
                    }

                    Method::Notify { message } => {
                        pod.push_notify(message);
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
                            &notify_buffer,
                            self_parent_socket.as_ref(),
                            &spawner_name,
                            &spawned_registry,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_extract().await {
                                tracing::warn!(error = %e, "Post-run memory extract error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory extract error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_consolidate().await {
                                tracing::warn!(error = %e, "Post-run memory consolidate error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory consolidate error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_user_segments(pod.user_segments().to_vec());
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
                            &notify_buffer,
                            self_parent_socket.as_ref(),
                            &spawner_name,
                            &spawned_registry,
                        )
                        .await;

                        if new_status == PodStatus::Idle {
                            if let Err(e) = pod.try_post_run_extract().await {
                                tracing::warn!(error = %e, "Post-run memory extract error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory extract error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_consolidate().await {
                                tracing::warn!(error = %e, "Post-run memory consolidate error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Pod,
                                    format!("post-run memory consolidate error: {e}"),
                                );
                            }
                            if let Err(e) = pod.try_post_run_compact().await {
                                tracing::warn!(error = %e, "Post-run compaction error");
                                alerter.alert(
                                    AlertLevel::Warn,
                                    AlertSource::Compactor,
                                    format!("post-run compaction error: {e}"),
                                );
                            }
                        }

                        let items = pod.worker().history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_user_segments(pod.user_segments().to_vec());
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

                    Method::Pause => {
                        // Already paused → idempotent no-op. Otherwise
                        // the Pod is Idle (Running turns go through
                        // `run_with_cancel_support`, not this outer
                        // match), so there is nothing to pause.
                        if shared_state.get_status() != PodStatus::Paused {
                            let _ = event_tx.send(Event::Error {
                                code: ErrorCode::NotRunning,
                                message: "Pod is not running".into(),
                            });
                        }
                    }

                    Method::Shutdown => {
                        let _ = event_tx.send(Event::Shutdown);
                        break;
                    }

                    // GetHistory / ListCompletions are handled at the socket
                    // layer (direct response). If they somehow reach the
                    // controller, ignore them.
                    Method::GetHistory | Method::ListCompletions { .. } => {}

                    Method::PodEvent(event) => {
                        // (1) system side effects — idempotent and
                        // tolerant of out-of-order delivery (e.g.
                        // `TurnEnded` arriving after `ShutDown`).
                        crate::ipc::event::apply_event_side_effects(
                            &event,
                            &spawned_registry,
                            &spawner_name,
                            &self_parent_socket,
                        )
                        .await;
                        // (2) render a one-line summary and push it
                        // into the notification buffer; the next LLM
                        // request will inject it as a system message
                        // via `PodInterceptor::pre_llm_request`.
                        let text = crate::ipc::event::render_event(&event);
                        pod.push_notify(text);
                        // Auto-kick a turn if the Pod is idle so the
                        // notification is not stranded. Matches the
                        // `Method::Notify` idle path.
                        if shared_state.get_status() == PodStatus::Idle {
                            shared_state.set_status(PodStatus::Running);
                            let _ = runtime_dir.write_status(&shared_state).await;

                            let (new_status, shutdown) = run_with_cancel_support(
                                pod.run_for_notification(),
                                &mut method_rx,
                                &event_tx,
                                &cancel_tx,
                                &shared_state,
                                &notify_buffer,
                                self_parent_socket.as_ref(),
                                &spawner_name,
                                &spawned_registry,
                            )
                            .await;

                            if new_status == PodStatus::Idle {
                                if let Err(e) = pod.try_post_run_extract().await {
                                    tracing::warn!(error = %e, "Post-run memory extract error");
                                    alerter.alert(
                                        AlertLevel::Warn,
                                        AlertSource::Pod,
                                        format!("post-run memory extract error: {e}"),
                                    );
                                }
                                if let Err(e) = pod.try_post_run_compact().await {
                                    tracing::warn!(error = %e, "Post-run compaction error");
                                    alerter.alert(
                                        AlertLevel::Warn,
                                        AlertSource::Compactor,
                                        format!("post-run compaction error: {e}"),
                                    );
                                }
                            }

                            let items = pod.worker().history().to_vec();
                            shared_state.update_history(items);
                            shared_state.set_user_segments(pod.user_segments().to_vec());
                            shared_state.set_status(new_status);
                            let _ = runtime_dir.write_status(&shared_state).await;
                            let _ = runtime_dir.write_history(&shared_state).await;

                            if shutdown {
                                let _ = event_tx.send(Event::Shutdown);
                                break;
                            }
                        }
                    }
                }
            }

            // Report upward that this Pod is stopping before the
            // controller task exits. Awaited (not fire-and-forget):
            // after `shutdown_tx.send` the process may exit quickly,
            // and a spawned task would be killed mid-send. The
            // `connect_and_send` helper enforces a 5 s timeout so a
            // stuck parent cannot block process exit indefinitely.
            if let Some(parent) = self_parent_socket.as_ref() {
                if let Err(e) = crate::ipc::event::send_pod_event(
                    parent,
                    protocol::PodEvent::ShutDown {
                        pod_name: spawner_name.clone(),
                    },
                )
                .await
                {
                    tracing::warn!(error = %e, "ShutDown PodEvent send failed");
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
///
/// `parent_socket` / `self_name` drive upward `PodEvent` reports
/// (`TurnEnded` on a clean Finished, `Errored` on a worker failure).
/// `None` parent skips the send (top-level Pod). Transient method
/// rejections such as `AlreadyRunning` are intentionally NOT reported
/// as `Errored` — only the worker-execution `Err` branch below fires.
async fn run_with_cancel_support<F>(
    pod_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    shared_state: &Arc<PodSharedState>,
    notify_buffer: &NotifyBuffer,
    parent_socket: Option<&std::path::PathBuf>,
    self_name: &str,
    spawned_registry: &Arc<SpawnedPodRegistry>,
) -> (PodStatus, bool)
where
    F: std::future::Future<Output = Result<PodRunResult, PodError>>,
{
    tokio::pin!(pod_future);
    let mut shutdown_requested = false;
    let mut pause_requested = false;

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
                        if matches!(run_result, RunResult::Finished) {
                            crate::ipc::event::fire_and_forget(
                                parent_socket.cloned(),
                                protocol::PodEvent::TurnEnded {
                                    pod_name: self_name.to_string(),
                                },
                            );
                        }
                        (status, shutdown_requested)
                    }
                    Err(PodError::Worker(WorkerError::Cancelled)) if pause_requested => {
                        // User-initiated Pause. Report the transition to
                        // clients as a normal Paused run-end, and
                        // intentionally skip `PodEvent::Errored` upward:
                        // that channel is reserved for worker runtime
                        // failures, not deliberate interruptions.
                        let _ = event_tx.send(Event::RunEnd { result: RunResult::Paused });
                        (PodStatus::Paused, shutdown_requested)
                    }
                    Err(e) => {
                        let code = worker_error_code(&e);
                        let message = e.to_string();
                        let _ = event_tx.send(Event::Error {
                            code,
                            message: message.clone(),
                        });
                        crate::ipc::event::fire_and_forget(
                            parent_socket.cloned(),
                            protocol::PodEvent::Errored {
                                pod_name: self_name.to_string(),
                                message,
                            },
                        );
                        (PodStatus::Idle, shutdown_requested)
                    }
                };
            }
            method = method_rx.recv() => {
                match method {
                    Some(Method::Cancel) => {
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Pause) => {
                        pause_requested = true;
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
                    Some(Method::Notify { message }) => {
                        // Route into the buffer; the in-flight turn will
                        // drain it at its next pre_llm_request.
                        notify_buffer.push(message);
                    }
                    Some(Method::GetHistory | Method::ListCompletions { .. }) => {}
                    Some(Method::PodEvent(event)) => {
                        // mpsc is consume-once, so we cannot defer this
                        // to the next main-loop iteration — drop here
                        // would lose the event entirely (children fire
                        // and forget). Apply the side effects inline
                        // and stage the rendered string on the
                        // notification buffer so the in-flight turn's
                        // next `pre_llm_request` surfaces it.
                        let self_parent_socket = parent_socket.cloned();
                        crate::ipc::event::apply_event_side_effects(
                            &event,
                            spawned_registry,
                            self_name,
                            &self_parent_socket,
                        )
                        .await;
                        notify_buffer.push(crate::ipc::event::render_event(&event));
                    }
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
    // `build_client` がここに到達する前に同じマニフェストで成功している
    // ため、カタログ解決も必ず通る。念のため失敗時は "unknown" に落とす。
    let resolved = provider::catalog::resolve_model_manifest(&manifest.model).ok();
    let (provider_name, model_id) = match resolved {
        Some(cfg) => {
            let name = match cfg.scheme {
                manifest::SchemeKind::Anthropic => "anthropic",
                manifest::SchemeKind::OpenaiChat => "openai_chat",
                manifest::SchemeKind::OpenaiResponses => "openai_responses",
                manifest::SchemeKind::Gemini => "gemini",
            };
            (name.to_string(), cfg.model_id)
        }
        None => (
            "unknown".to_string(),
            manifest
                .model
                .ref_
                .clone()
                .or_else(|| manifest.model.model_id.clone())
                .unwrap_or_default(),
        ),
    };
    // Tool list reflects whatever `spawn()` ended up registering on the
    // Worker. Caller must have flushed pending factories first; without
    // a flush the tool table is empty and this returns an empty vec.
    let tool_names: Vec<String> = pod
        .worker()
        .tool_server_handle()
        .tool_definitions_sorted()
        .into_iter()
        .map(|def| def.name)
        .collect();
    protocol::Greeting {
        pod_name: manifest.pod.name.clone(),
        cwd: pod.pwd().display().to_string(),
        provider: provider_name,
        model: model_id,
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
