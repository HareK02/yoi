use std::path::{Path, PathBuf};
use std::sync::Arc;

use llm_worker::WorkerError;
use llm_worker::llm_client::client::LlmClient;
use session_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};

use llm_worker::Item;
use session_store::LogEntry;
use session_store::session_log;

use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::pod::{LogCommand, LogDrainHandle, Pod, PodError, PodRunResult};
use crate::runtime::dir::RuntimeDir;
use crate::session_log_sink::SessionLogSink;
use crate::shared_state::PodSharedState;
use crate::spawn::comm_tools::{
    list_pods_tool, read_pod_output_tool, send_to_pod_tool, stop_pod_tool,
};
use crate::spawn::registry::SpawnedPodRegistry;
use crate::spawn::tool::spawn_pod_tool;
use protocol::{
    AlertLevel, AlertSource, ErrorCode, Event, Method, PodStatus, RunResult, Segment, TurnResult,
};

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
    /// Session-log mirror + broadcast handle. The IPC server snapshots
    /// it on every new connection (Event::Snapshot) and forwards
    /// subsequent commits (Event::Entry) on the receiver.
    pub sink: SessionLogSink,
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

async fn set_controller_status(
    shared_state: &Arc<PodSharedState>,
    runtime_dir: &RuntimeDir,
    event_tx: &broadcast::Sender<Event>,
    status: PodStatus,
) {
    shared_state.set_status(status);
    let _ = runtime_dir.write_status(shared_state).await;
    let _ = event_tx.send(Event::Status { status });
}

async fn finish_controller_run<C, St>(
    pod: &mut Pod<C, St>,
    shared_state: &Arc<PodSharedState>,
    runtime_dir: &RuntimeDir,
    event_tx: &broadcast::Sender<Event>,
    new_status: PodStatus,
) where
    C: LlmClient + Clone + 'static,
    St: Store + Clone + 'static,
{
    // history / user_segments are no longer mirrored on PodSharedState —
    // clients reconstruct them from `Event::Snapshot` + live
    // `Event::Entry` deliveries driven by the session-log sink. We
    // only flip the status and kick post-run memory jobs here.
    set_controller_status(shared_state, runtime_dir, event_tx, new_status).await;
    pod.spawn_post_run_memory_jobs();
}

/// Pending turn launch staged by an event handler for the next outer-loop
/// iteration. Each variant carries the input needed by the corresponding
/// `Pod::*` entry point — `RunForNotification` carries none because
/// `pod.run_for_notification()` drains the NotifyBuffer on its own.
enum PendingRun {
    Run(Vec<Segment>),
    InterruptAndRun(Vec<Segment>),
    RunForNotification,
    Resume,
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
        C: LlmClient + Clone + 'static,
        St: Store + Clone + 'static,
    {
        // === 1. Initialization (channels / RuntimeDir / pod-immutable
        //         snapshots / SpawnedPodRegistry / alerter attach /
        //         bash-output scope) ===
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (method_tx, method_rx) = mpsc::channel::<Method>(32);
        let (event_tx, _) = broadcast::channel::<Event>(256);
        let alerter = Alerter::new(event_tx.clone());

        // Runtime directory is created before tool registration because
        // the spawn-tool factories need its socket path, and before the
        // initial status/history writes consume the greeting we build
        // after registration is complete.
        let runtime_dir =
            Arc::new(RuntimeDir::create(runtime_base, &pod.manifest().pod.name).await?);

        let spawner_name = pod.manifest().pod.name.clone();
        let self_parent_socket = pod.callback_socket().cloned();
        let spawned_registry = SpawnedPodRegistry::new(runtime_dir.clone());

        // Hand the alerter to the Pod so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        pod.attach_alerter(alerter.clone());
        // Also hand the raw broadcast sender so Pod-internal operations
        // can emit typed lifecycle `Event`s (currently: compact progress).
        pod.attach_event_tx(event_tx.clone());

        // Bash spills long outputs to a per-pod subdir under the runtime
        // dir. Push a recursive `allow(Read)` for that path into the
        // Pod's runtime scope so the agent can `Read` saved files
        // without polluting the workspace.
        let bash_output_dir = runtime_dir.path().join("bash-output");
        std::fs::create_dir_all(&bash_output_dir).map_err(|e| {
            std::io::Error::other(format!(
                "create bash output dir {}: {e}",
                bash_output_dir.display()
            ))
        })?;
        pod.add_scope_rules([manifest::ScopeRule {
            target: bash_output_dir.clone(),
            permission: manifest::Permission::Read,
            recursive: true,
        }])
        .map_err(std::io::Error::other)?;

        // === 1.5. Per-item history-commit drain task ===
        //
        // Worker callbacks fire `on_history_append` for each assistant
        // item / tool result / hook-injected item that lands in
        // history. The drain task picks them up off an unbounded mpsc
        // and commits each as a typed `LogEntry` through the sink,
        // serialised against the same `session_head` lock the Pod uses
        // for its own commits. This gives mid-turn snapshot visibility:
        // a late-attaching client sees in-flight tool calls + completed
        // assistant blocks without waiting for the turn-end persist.
        let (log_cmd_tx, log_cmd_rx) = mpsc::unbounded_channel::<LogCommand>();
        let drain_ctx = pod.log_drain_handle();
        let _drain_task = tokio::spawn(run_log_drain(log_cmd_rx, drain_ctx));
        pod.attach_log_cmd_tx(log_cmd_tx.clone());

        // === 2. Worker event bridge wiring ===
        wire_event_bridges_on_worker(&mut pod, &event_tx, &alerter, log_cmd_tx);

        // === 3. Tool registration (builtin / memory / spawn-orchestration) ===
        let fs_for_view = register_pod_tools(
            &mut pod,
            bash_output_dir,
            runtime_dir.socket_path(),
            runtime_base.to_path_buf(),
            spawned_registry.clone(),
        );

        // Materialise pending tool factories so the greeting reflects
        // the actual registered set instead of a hand-maintained mirror.
        pod.worker().tool_server_handle().flush_pending();

        // === 4. Initial runtime files + PodSharedState + PodHandle +
        //         SocketServer ===
        let manifest_toml = toml::to_string_pretty(pod.manifest()).unwrap_or_default();
        let greeting = build_greeting(&pod);
        let shared_state = Arc::new(PodSharedState::new(
            pod.manifest().pod.name.clone(),
            pod.session_id(),
            manifest_toml.clone(),
            greeting,
        ));
        shared_state.set_fs_view(crate::fs_view::PodFsView::new(fs_for_view));
        shared_state.set_workflows(
            pod.workflow_completions()
                .into_iter()
                .map(|slug| crate::shared_state::WorkflowCandidate { slug })
                .collect(),
        );
        shared_state.set_knowledge(
            pod.knowledge_completions()
                .into_iter()
                .map(|slug| crate::shared_state::KnowledgeCandidate { slug })
                .collect(),
        );
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;

        let handle = PodHandle {
            method_tx,
            event_tx: event_tx.clone(),
            shared_state: shared_state.clone(),
            runtime_dir: runtime_dir.clone(),
            alerter: alerter.clone(),
            sink: pod.sink(),
        };

        let socket_server = SocketServer::start(&handle).await?;

        // === 5. controller_loop ===
        // Clone cancel sender and notification buffer before moving pod
        // into the controller task so the in-flight turn can be reached
        // via these handles while pod itself is borrowed by drive_turn.
        let cancel_tx = pod.worker_mut().cancel_sender();
        let notify_buffer = pod.notify_buffer_handle();

        tokio::spawn(controller_loop(
            pod,
            method_rx,
            event_tx,
            shared_state,
            runtime_dir,
            cancel_tx,
            notify_buffer,
            self_parent_socket,
            spawner_name,
            spawned_registry,
            shutdown_tx,
            socket_server,
        ));

        Ok((handle, shutdown_rx))
    }
}

/// Wire the per-event broadcast bridges on the Pod's Worker. Each callback
/// re-publishes a worker-level signal as a `protocol::Event` on `event_tx`
/// so subscribers (TUI, socket clients) get a single typed stream.
///
/// Also wires `on_history_append` into the per-item drain channel so
/// every history append observed by the worker becomes a typed
/// `LogEntry` commit (via the drain task).
fn wire_event_bridges_on_worker<C, St>(
    pod: &mut Pod<C, St>,
    event_tx: &broadcast::Sender<Event>,
    alerter: &Alerter,
    log_cmd_tx: mpsc::UnboundedSender<LogCommand>,
) where
    C: LlmClient + Clone + 'static,
    St: Store + Clone + 'static,
{
    let worker = pod.worker_mut();

    // Per-history-append → drain channel. Sends are infallible-by-design
    // here (UnboundedSender never blocks); a closed receiver just means
    // the controller is shutting down, in which case dropping the item
    // is acceptable.
    let drain_tx = log_cmd_tx.clone();
    worker.on_history_append(move |item| {
        let _ = drain_tx.send(LogCommand::Item(item.clone()));
    });

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
        // Start fires unconditionally so the TUI can show "Thinking..."
        // even when the provider doesn't emit plaintext deltas.
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
            cache_read_input_tokens: event.cache_read_input_tokens,
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

    // History-append broadcasts (previously `Event::SystemMessage`)
    // have been removed: every persistent history item is now committed
    // through the session-log sink as a typed `LogEntry`, and clients
    // see it via `Event::Snapshot` + live `Event::Entry`. The
    // per-item commit channel is wired at the top of this function.
}

/// Drain task: consumes `LogCommand::Item` and `LogCommand::Flush`
/// off the channel and commits each item as a typed `LogEntry` through
/// the supplied store + sink. Lives as long as the controller; exits
/// when the sender is dropped (controller shutdown).
async fn run_log_drain<St>(mut rx: mpsc::UnboundedReceiver<LogCommand>, ctx: LogDrainHandle<St>)
where
    St: session_store::Store + Clone + Send + 'static,
{
    while let Some(cmd) = rx.recv().await {
        match cmd {
            LogCommand::Item(item) => {
                let Some(entry) = classify_history_item(item) else {
                    continue;
                };
                commit_via_drain(&ctx, entry).await;
            }
            LogCommand::SystemItems(items) => {
                if items.is_empty() {
                    continue;
                }
                let entry = LogEntry::SystemItems {
                    ts: session_log::now_millis(),
                    items,
                };
                commit_via_drain(&ctx, entry).await;
            }
            LogCommand::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

async fn commit_via_drain<St>(ctx: &LogDrainHandle<St>, entry: LogEntry)
where
    St: session_store::Store + Clone + Send + 'static,
{
    let mut head = ctx.session_head.lock().await;
    match session_store::append_entry_with_hash(
        &ctx.store,
        head.session_id,
        &mut head.head_hash,
        entry.clone(),
    )
    .await
    {
        Ok(_) => {
            // Publish under the same critical section view a
            // `subscribe_with_snapshot` would observe.
            ctx.sink.publish(entry);
        }
        Err(e) => {
            tracing::warn!(error = %e, "drain: append_entry failed; entry dropped");
        }
    }
}

/// Map one LLM-driven worker-history append to its `LogEntry` form.
///
/// `None` is the skip signal for items that the drain must not commit:
/// - `user_message` items are committed by `Pod::run` up-front as
///   `LogEntry::UserInput { segments }`.
/// - `system_message` items are committed by `PodInterceptor` as part
///   of a `LogEntry::SystemItems` batch (with typed kind metadata)
///   before they reach the worker's history.
fn classify_history_item(item: Item) -> Option<LogEntry> {
    let ts = session_log::now_millis();
    if item.is_user_message() {
        return None;
    }
    if matches!(
        item,
        Item::Message {
            role: llm_worker::Role::System,
            ..
        }
    ) {
        return None;
    }
    if item.is_tool_result() {
        return Some(LogEntry::ToolResults {
            ts,
            items: vec![session_store::LoggedItem::from(&item)],
        });
    }
    if item.is_assistant_message() || item.is_tool_call() || item.is_reasoning() {
        return Some(LogEntry::AssistantItems {
            ts,
            items: vec![session_store::LoggedItem::from(&item)],
        });
    }
    // Defensive: anything else (future Item kinds) routes through
    // AssistantItems rather than getting silently dropped.
    Some(LogEntry::AssistantItems {
        ts,
        items: vec![session_store::LoggedItem::from(&item)],
    })
}

/// Register the builtin file-manipulation tools, optional memory tools,
/// and the Pod-orchestration tools (SpawnPod + comm) on the Pod's
/// Worker. Returns the `ScopedFs` clone used to attach a `PodFsView` to
/// the shared state.
fn register_pod_tools<C, St>(
    pod: &mut Pod<C, St>,
    bash_output_dir: PathBuf,
    spawner_socket: PathBuf,
    runtime_base: PathBuf,
    spawned_registry: Arc<SpawnedPodRegistry>,
) -> tools::ScopedFs
where
    C: LlmClient + Clone + 'static,
    St: Store + Clone + 'static,
{
    // Pod-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `pod`.
    let scope_handle = pod.scope().clone();
    let pwd = pod.pwd().to_path_buf();
    let task_store = pod.task_store();
    let session_id_for_usage = pod.session_id().to_string();
    let scope_change_sink = pod.scope_change_sink();
    let memory_config = pod.manifest().memory.clone();
    let spawner_name = pod.manifest().pod.name.clone();
    let spawner_model = pod.manifest().model.clone();
    let self_parent_socket = pod.callback_socket().cloned();

    let worker = pod.worker_mut();

    // The Pod's SharedScope (already augmented with the bash-output
    // Read rule by the caller) is the single source of truth — every
    // ScopedFs (builtin tools, fs_view, compact worker) reads from it,
    // and any future scope mutation (SpawnPod-style revoke, future
    // GrantScope) propagates through it.
    let fs = tools::ScopedFs::with_shared_scope(scope_handle.clone(), pwd.clone());
    let tracker = tools::Tracker::new();
    // Same ScopedFs also powers the IPC `ListCompletions` query — keep
    // a clone for the FS view we attach below, since the tools consume
    // `fs` itself.
    let fs_for_view = fs.clone();
    worker.register_tools(tools::builtin_tools(
        fs,
        tracker.clone(),
        task_store,
        bash_output_dir,
    ));

    // Memory subsystem opt-in. When `[memory]` is present in the
    // manifest, register the memory-specific Read/Write/Edit tools that
    // target `<workspace>/memory/` and `<workspace>/knowledge/` with
    // their built-in linter. Companion deny rules on the generic CRUD
    // scope were already applied during `Pod::from_manifest`.
    if let Some(mem) = memory_config.as_ref() {
        let layout = memory::WorkspaceLayout::resolve(mem, &pwd);
        let query_cfg = memory::tool::QueryConfig::from(mem);
        worker.register_tool(memory::tool::read_tool_with_usage(
            layout.clone(),
            session_id_for_usage,
        ));
        worker.register_tool(memory::tool::write_tool(layout.clone()));
        worker.register_tool(memory::tool::edit_tool(layout.clone()));
        worker.register_tool(memory::tool::memory_query_tool(layout.clone(), query_cfg));
        worker.register_tool(memory::tool::knowledge_query_tool(layout, query_cfg));
    }

    // Pod-orchestration tools (SpawnPod + the four comm tools) share
    // the Pod-scoped `SpawnedPodRegistry` (also consumed by the main
    // loop's `PodEvent` handler).
    worker.register_tool(spawn_pod_tool(
        spawner_name,
        spawner_socket,
        runtime_base,
        pwd,
        spawned_registry.clone(),
        self_parent_socket,
        spawner_model,
        scope_handle,
        scope_change_sink,
    ));
    worker.register_tool(send_to_pod_tool(spawned_registry.clone()));
    worker.register_tool(read_pod_output_tool(spawned_registry.clone()));
    worker.register_tool(stop_pod_tool(spawned_registry.clone()));
    worker.register_tool(list_pods_tool(spawned_registry));
    pod.attach_tracker(tracker);
    fs_for_view
}

/// Idle/Paused event loop. Each iteration either fires a staged
/// `PendingRun` (delegating to [`drive_turn`] for the Running phase) or
/// waits for the next `Method`. Method handlers stop at "update state +
/// stage `pending`"; the loop's top-of-iteration block owns the
/// status-flip → run → finish sequence so it lives in exactly one
/// place.
#[allow(clippy::too_many_arguments)]
async fn controller_loop<C, St>(
    mut pod: Pod<C, St>,
    mut method_rx: mpsc::Receiver<Method>,
    event_tx: broadcast::Sender<Event>,
    shared_state: Arc<PodSharedState>,
    runtime_dir: Arc<RuntimeDir>,
    cancel_tx: mpsc::Sender<()>,
    notify_buffer: NotifyBuffer,
    self_parent_socket: Option<PathBuf>,
    spawner_name: String,
    spawned_registry: Arc<SpawnedPodRegistry>,
    shutdown_tx: oneshot::Sender<()>,
    socket_server: SocketServer,
) where
    C: LlmClient + Clone + 'static,
    St: Store + Clone + 'static,
{
    // Hold socket server alive for the lifetime of the controller task.
    let _socket_server = socket_server;

    let mut pending: Option<PendingRun> = None;

    loop {
        // Top-of-iteration: if an event handler staged a run, fire it
        // here so the status flip → drive_turn → finish sequence lives
        // in one place, regardless of which Method caused it.
        if let Some(run) = pending.take() {
            set_controller_status(&shared_state, &runtime_dir, &event_tx, PodStatus::Running).await;
            let (new_status, shutdown) = match run {
                PendingRun::Run(input) => {
                    drive_turn(
                        pod.run(input),
                        &mut method_rx,
                        &event_tx,
                        &cancel_tx,
                        &shared_state,
                        &notify_buffer,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                    )
                    .await
                }
                PendingRun::InterruptAndRun(input) => {
                    drive_turn(
                        pod.interrupt_and_run(input),
                        &mut method_rx,
                        &event_tx,
                        &cancel_tx,
                        &shared_state,
                        &notify_buffer,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                    )
                    .await
                }
                PendingRun::RunForNotification => {
                    drive_turn(
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
                    .await
                }
                PendingRun::Resume => {
                    drive_turn(
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
                    .await
                }
            };
            finish_controller_run(&mut pod, &shared_state, &runtime_dir, &event_tx, new_status)
                .await;
            if shutdown {
                let _ = event_tx.send(Event::Shutdown);
                break;
            }
            continue;
        }

        let method = match method_rx.recv().await {
            Some(m) => m,
            None => break,
        };

        match method {
            Method::Run { input } => {
                let status_before = shared_state.get_status();
                if status_before == PodStatus::Running {
                    // Defensive: the inner select! inside drive_turn
                    // already rejects `Run` while a turn is live, so
                    // this branch is only reachable across a race window
                    // around status flips.
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Pod is already executing a turn".into(),
                    });
                    continue;
                }
                if let Err(e) = pod.validate_workflow_invocations(&input) {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: e.to_string(),
                    });
                    continue;
                }
                // Broadcast the accepted user message so every
                // subscriber (including the submitter) can render the
                // turn header + user line from a single source of
                // truth. shared_state's `user_segments` is re-synced
                // from `pod` after the run completes, so we don't push
                // here.
                let _ = event_tx.send(Event::UserMessage {
                    segments: input.clone(),
                });
                pending = Some(if status_before == PodStatus::Paused {
                    PendingRun::InterruptAndRun(input)
                } else {
                    PendingRun::Run(input)
                });
            }

            Method::Notify { message } => {
                // Client-side live echo is delivered as `Event::SystemItem`
                // once the interceptor commits the corresponding
                // `LogEntry::SystemItems` entry — drained out of the
                // notify buffer + broadcast through the sink. No
                // separate echo here.
                pod.push_notify(message);
                // RUNNING / Paused: the buffer push is the entire
                // operation; an in-flight turn (or the next
                // Resume/Run) will drain it at its next
                // pre_llm_request. IDLE: auto-start a turn so the LLM
                // sees the buffered notification(s) without a human
                // Run.
                if shared_state.get_status() == PodStatus::Idle {
                    pending = Some(PendingRun::RunForNotification);
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
                pending = Some(PendingRun::Resume);
            }

            Method::Cancel => {
                let _ = event_tx.send(Event::Error {
                    code: ErrorCode::NotRunning,
                    message: "Pod is not running".into(),
                });
            }

            Method::Pause => {
                // Already paused → idempotent no-op. Otherwise the
                // Pod is Idle (Running turns go through `drive_turn`,
                // not this outer match), so there is nothing to pause.
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

            // ListCompletions is handled at the socket layer (direct
            // response). If it reaches the controller, ignore it.
            Method::ListCompletions { .. } => {}

            Method::PodEvent(event) => {
                // Live echo travels through the SystemItem lane: once
                // the interceptor drains the notify buffer, the
                // typed `SystemItem::PodEvent` lands as a
                // `LogEntry::SystemItems` entry and the sink fans it
                // out to clients as `Event::SystemItem`.
                //
                // (1) system side effects — idempotent and tolerant of
                // out-of-order delivery (e.g. `TurnEnded` arriving
                // after `ShutDown`).
                crate::ipc::event::apply_event_side_effects(
                    &event,
                    &spawned_registry,
                    &spawner_name,
                    &self_parent_socket,
                )
                .await;
                // (2) queue the typed event in the notification buffer;
                // the next LLM request will inject it as a typed
                // `SystemItem::PodEvent` via the interceptor drain.
                pod.push_pod_event_notify(event);
                // Auto-kick a turn if the Pod is idle so the
                // notification is not stranded. Matches the
                // `Method::Notify` idle path.
                if shared_state.get_status() == PodStatus::Idle {
                    pending = Some(PendingRun::RunForNotification);
                }
            }
        }
    }

    // Background memory jobs own extract/consolidate workers after a
    // turn completes. Join them before the controller task exits so
    // staging writes and consolidation cleanups are not abandoned.
    pod.wait_for_memory_jobs().await;

    // Report upward that this Pod is stopping before the controller
    // task exits. Awaited (not fire-and-forget): after `shutdown_tx.send`
    // the process may exit quickly, and a spawned task would be killed
    // mid-send. The `connect_and_send` helper enforces a 5 s timeout so
    // a stuck parent cannot block process exit indefinitely.
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
}

/// Drives a Pod future (one in-flight turn) while concurrently
/// processing incoming methods through an inner select! arm. Returns
/// `(final_status, shutdown_requested)`.
///
/// `parent_socket` / `self_name` drive upward `PodEvent` reports
/// (`TurnEnded` on a clean Finished, `Errored` on a worker failure).
/// `None` parent skips the send (top-level Pod). Transient method
/// rejections such as `AlreadyRunning` are intentionally NOT reported
/// as `Errored` — only the worker-execution `Err` branch below fires.
#[allow(clippy::too_many_arguments)]
async fn drive_turn<F>(
    pod_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    shared_state: &Arc<PodSharedState>,
    notify_buffer: &NotifyBuffer,
    parent_socket: Option<&PathBuf>,
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
                        // Live echo arrives via `Event::SystemItem` once
                        // the in-flight turn's next `pre_llm_request`
                        // drains this entry through the interceptor.
                        notify_buffer.push_notify(message);
                    }
                    Some(Method::ListCompletions { .. }) => {}
                    Some(Method::PodEvent(event)) => {
                        // mpsc is consume-once, so we cannot defer this
                        // to the next main-loop iteration — drop here
                        // would lose the event entirely (children fire
                        // and forget). Apply the side effects inline
                        // and stage the typed event on the notification
                        // buffer so the in-flight turn's next
                        // `pre_llm_request` surfaces it as a typed
                        // `SystemItem::PodEvent`.
                        let self_parent_socket = parent_socket.cloned();
                        crate::ipc::event::apply_event_side_effects(
                            &event,
                            spawned_registry,
                            self_name,
                            &self_parent_socket,
                        )
                        .await;
                        notify_buffer.push_pod_event(event);
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
        scope_summary: pod.scope_snapshot().summary(),
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
