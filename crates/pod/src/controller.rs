use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use llm_worker::WorkerError;
use llm_worker::llm_client::client::LlmClient;
use pod_store::PodMetadataStore;
use session_store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::discovery::{PodDiscovery, list_pods_tool, restore_pod_tool, send_to_peer_pod_tool};
use crate::feature::FeatureRegistryBuilder;
use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::pod::{Pod, PodError, PodRunResult, SystemItemCommitter};
use crate::runtime::dir::RuntimeDir;
use crate::segment_log_sink::SegmentLogSink;
use crate::shared_state::PodSharedState;
use crate::spawn::comm_tools::{read_pod_output_tool, send_to_pod_tool, stop_pod_tool};
use crate::spawn::registry::SpawnedPodRegistry;
use crate::spawn::tool::spawn_pod_tool;
use protocol::{
    AlertLevel, AlertSource, ErrorCode, Event, Method, PodStatus, RewindTargetId, RunResult,
    Segment, TurnResult,
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
    /// Segment-log mirror + broadcast handle. The IPC server snapshots
    /// it on every new connection (Event::Snapshot) and forwards
    /// subsequent commits (Event::Entry) on the receiver.
    pub sink: SegmentLogSink,
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
    St: Store + PodMetadataStore + Clone + 'static,
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
    /// Self-initiated turn kicked from the notify buffer. The carried
    /// `InvokeKind` is the trigger that flipped the Pod from IDLE
    /// (Notify or PodEvent) and is recorded by the Invoke marker
    /// committed at the start of `pod.run_for_notification`.
    RunForNotification(protocol::InvokeKind),
    Resume,
}

impl PendingRun {
    /// Whether this turn was kicked off by the parent (via `Method::Run`
    /// or `Method::Resume`). Used by [`drive_turn`] to gate upward
    /// `PodEvent::TurnEnded` / `PodEvent::Errored` reports so the parent
    /// only sees completion signals for work it actually delegated.
    /// `RunForNotification` covers self-initiated turns kicked from the
    /// notify buffer (Notify / inbound PodEvent) and stays silent.
    fn is_parent_originated(&self) -> bool {
        match self {
            PendingRun::Run(_) | PendingRun::Resume => true,
            PendingRun::RunForNotification(_) => false,
        }
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
        C: LlmClient + Clone + 'static,
        St: Store + PodMetadataStore + Clone + Send + Sync + 'static,
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
        let loaded_registry = SpawnedPodRegistry::load_from_pod_state_with_reclaim(
            runtime_dir.clone(),
            pod.store().clone(),
            spawner_name.clone(),
            Some(pod.scope().clone()),
        )
        .await?;
        let reclaimed_unreachable = loaded_registry.reclaimed_unreachable;
        let spawned_registry = loaded_registry.registry;
        if reclaimed_unreachable {
            pod.push_notify(
                "Restored Pod state contained unreachable delegated child Pods; their delegated write scopes were reclaimed before resume."
                    .to_string(),
            );
        }

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

        // === 1.5. Direct writer wiring ===
        //
        // Worker callbacks fire `on_history_append` for each assistant
        // item / tool result that lands in history. With the sync
        // writer in place, the callback commits each item directly
        // through a `LogWriterHandle` (no mpsc ferry, no drain task).
        // The same handle is type-erased into a `SystemItemCommitter`
        // and handed to the interceptor for `SystemItem` commits, so
        // assistant / tool / system items all share one commit path.
        let writer_for_system: Arc<dyn SystemItemCommitter> = Arc::new(pod.log_writer_handle());
        pod.attach_log_writer(writer_for_system);
        pod.wire_history_persistence();

        // === 2. Worker event bridge wiring ===
        wire_event_bridges_on_worker(&mut pod, &event_tx, &alerter);

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
            pod.segment_id(),
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
/// `Pod::wire_history_persistence` is called separately to wire the
/// per-item history commit callback so every assistant / tool item
/// landing in `worker.history` becomes a singular `LogEntry::AssistantItem`
/// / `ToolResult` commit through the sync writer.
fn wire_event_bridges_on_worker<C, St>(
    pod: &mut Pod<C, St>,
    event_tx: &broadcast::Sender<Event>,
    alerter: &Alerter,
) where
    C: LlmClient + Clone + 'static,
    St: Store + PodMetadataStore + Clone + 'static,
{
    let ai_activity = pod.ai_activity_counter();
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
    worker.on_llm_call_start(move |llm_call| {
        let _ = tx.send(Event::LlmCallStart { llm_call });
    });

    let tx = event_tx.clone();
    worker.on_llm_call_end(move |llm_call| {
        let _ = tx.send(Event::LlmCallEnd { llm_call });
    });

    let tx = event_tx.clone();
    worker.on_llm_retry(move |llm_call, notice| {
        let _ = tx.send(Event::LlmRetry {
            llm_call,
            failed_attempt: notice.failed_attempt,
            max_attempts: notice.max_attempts,
            wait_ms: notice.wait.as_millis() as u64,
            elapsed_ms: notice.elapsed.as_millis() as u64,
            status: notice.status,
            error: notice.error.clone(),
        });
    });

    let tx = event_tx.clone();
    worker.on_llm_continuation(move |llm_call, attempt, max_attempts, reason| {
        let _ = tx.send(Event::LlmContinuation {
            llm_call,
            attempt,
            max_attempts,
            reason: reason.to_owned(),
        });
    });

    let tx = event_tx.clone();
    let activity = ai_activity.clone();
    worker.on_text_block(move |block| {
        let tx_d = tx.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            let _ = tx_d.send(Event::TextDelta {
                text: text.to_owned(),
            });
        });
        let tx_s = tx.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            let _ = tx_s.send(Event::TextDone {
                text: text.to_owned(),
            });
        });
    });

    let tx = event_tx.clone();
    let activity = ai_activity.clone();
    worker.on_thinking_block(move |block| {
        // Start fires unconditionally so the TUI can show "Thinking..."
        // even when the provider doesn't emit plaintext deltas.
        activity.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(Event::ThinkingStart);
        let tx_d = tx.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            let _ = tx_d.send(Event::ThinkingDelta {
                text: text.to_owned(),
            });
        });
        let tx_s = tx.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            let _ = tx_s.send(Event::ThinkingDone {
                text: text.to_owned(),
            });
        });
    });

    let tx = event_tx.clone();
    let activity = ai_activity.clone();
    worker.on_tool_use_block(move |start, block| {
        activity.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(Event::ToolCallStart {
            id: start.id.clone(),
            name: start.name.clone(),
        });
        let id_for_delta = start.id.clone();
        let tx_d = tx.clone();
        let activity_d = activity.clone();
        block.on_delta(move |json| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            let _ = tx_d.send(Event::ToolCallArgsDelta {
                id: id_for_delta.clone(),
                json: json.to_owned(),
            });
        });
        let tx_s = tx.clone();
        let activity_s = activity.clone();
        block.on_stop(move |call| {
            activity_s.fetch_add(1, Ordering::SeqCst);
            let _ = tx_s.send(Event::ToolCallDone {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.input.to_string(),
            });
        });
    });

    let tx = event_tx.clone();
    let activity = ai_activity.clone();
    worker.on_tool_result(move |result| {
        activity.fetch_add(1, Ordering::SeqCst);
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
    St: Store + PodMetadataStore + Clone + 'static,
{
    // Pod-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `pod`.
    let scope_handle = pod.scope().clone();
    let pwd = pod.pwd().to_path_buf();
    let task_feature = pod.task_feature();
    let session_id_for_usage = pod.segment_id().to_string();
    let memory_config = pod.manifest().memory.clone();
    let web_config = pod.manifest().web.clone();
    let spawner_name = pod.manifest().pod.name.clone();
    let spawner_manifest = pod.manifest().clone();
    let prompts = pod.prompts().clone();
    let pod_store = pod.store().clone();
    let self_parent_socket = pod.callback_socket().cloned();

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
    pod.worker_mut().register_tools(tools::core_builtin_tools(
        fs,
        tracker.clone(),
        bash_output_dir,
        web_config,
    ));

    let mut feature_registry = FeatureRegistryBuilder::new();
    feature_registry.add_module(task_feature);
    feature_registry.add_module(crate::feature::builtin::ticket_tools_feature(&pwd));
    let _feature_install_report = pod.install_features(feature_registry);

    let worker = pod.worker_mut();

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
        worker.register_tool(memory::tool::delete_tool(layout.clone()));
        worker.register_tool(memory::tool::memory_query_tool(layout.clone(), query_cfg));
        worker.register_tool(memory::tool::knowledge_query_tool(layout, query_cfg));
    }

    // Pod-orchestration tools (SpawnPod + the four comm tools) share
    // the Pod-scoped `SpawnedPodRegistry` (also consumed by the main
    // loop's `PodEvent` handler).
    worker.register_tool(spawn_pod_tool(
        spawner_name.clone(),
        spawner_socket,
        runtime_base.clone(),
        pwd.clone(),
        spawned_registry.clone(),
        self_parent_socket,
        spawner_manifest,
        scope_handle,
        prompts,
    ));
    worker.register_tool(send_to_pod_tool(spawned_registry.clone()));
    worker.register_tool(read_pod_output_tool(spawned_registry.clone()));
    worker.register_tool(stop_pod_tool(spawned_registry.clone()));
    let discovery = PodDiscovery::new(pod_store, spawner_name, runtime_base, pwd, spawned_registry);
    worker.register_tool(list_pods_tool(discovery.clone()));
    worker.register_tool(restore_pod_tool(discovery.clone()));
    worker.register_tool(send_to_peer_pod_tool(discovery));
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
    St: Store + PodMetadataStore + Clone + 'static,
{
    // Hold socket server alive for the lifetime of the controller task.
    let _socket_server = socket_server;

    let discovery_runtime_base = runtime_dir
        .path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir.path().to_path_buf());
    let discovery = PodDiscovery::new(
        pod.store().clone(),
        spawner_name.clone(),
        discovery_runtime_base,
        pod.pwd().to_path_buf(),
        spawned_registry.clone(),
    );
    let mut pending: Option<PendingRun> = None;

    loop {
        // Top-of-iteration: if an event handler staged a run, fire it
        // here so the status flip → drive_turn → finish sequence lives
        // in one place, regardless of which Method caused it.
        if let Some(run) = pending.take() {
            // Cancellation is meaningful only for an accepted running turn. Clear
            // idle/stale signals before the status flip; any Cancel/Pause received
            // after this point is delivered to the turn and must not be discarded by
            // the Worker at run start.
            pod.worker_mut().clear_pending_cancel();
            set_controller_status(&shared_state, &runtime_dir, &event_tx, PodStatus::Running).await;
            let parent_originated = run.is_parent_originated();
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
                        parent_originated,
                    )
                    .await
                }
                PendingRun::RunForNotification(kind) => {
                    drive_turn(
                        pod.run_for_notification(kind),
                        &mut method_rx,
                        &event_tx,
                        &cancel_tx,
                        &shared_state,
                        &notify_buffer,
                        self_parent_socket.as_ref(),
                        &spawner_name,
                        &spawned_registry,
                        parent_originated,
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
                        parent_originated,
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
                if shared_state.get_status() == PodStatus::Running {
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
                // Stage the run without a speculative user-message echo.
                // `Pod::run` validates the input, commits
                // `LogEntry::UserInput`, and the session-log sink turns that
                // committed entry into the live `Event::UserMessage`. That
                // keeps every client ordered against `SegmentStart` replay and
                // makes persisted history the single source of visible user
                // input. Paused→Run cleanup (orphan tool_result closure +
                // interrupt system note) is applied inside `Pod::run` itself
                // when the worker's `last_run_interrupted` flag is set.
                pending = Some(PendingRun::Run(input));
            }

            Method::Notify { message } => {
                // Client-side live echo is delivered as `Event::SystemItem`
                // once the interceptor commits the corresponding
                // `LogEntry::SystemItem` entry — drained out of the
                // notify buffer + broadcast through the sink. No
                // separate echo here.
                pod.push_notify(message);
                // RUNNING / Paused: the buffer push is the entire
                // operation; an in-flight turn (or the next
                // Resume/Run) will drain it at its next
                // pending_history_appends. IDLE: auto-start a turn so the LLM
                // sees the buffered notification(s) without a human
                // Run.
                if shared_state.get_status() == PodStatus::Idle {
                    pending = Some(PendingRun::RunForNotification(protocol::InvokeKind::Notify));
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

            Method::Compact => match shared_state.get_status() {
                PodStatus::Idle => {
                    if let Err(error) = pod.manual_compact().await {
                        let _ = event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                }
                PodStatus::Paused => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot compact while the Pod is paused; resume or start a fresh turn first"
                            .into(),
                    });
                }
                PodStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Pod is already executing a turn; compact can only run while idle"
                            .into(),
                    });
                }
            },

            Method::ListRewindTargets => match shared_state.get_status() {
                PodStatus::Idle | PodStatus::Paused => emit_rewind_targets(&pod, &event_tx),
                PodStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Pod is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::RewindTo {
                target,
                expected_head_entries,
            } => match shared_state.get_status() {
                PodStatus::Idle => {
                    if apply_rewind(&mut pod, &event_tx, target, expected_head_entries) {
                        shared_state.set_status(PodStatus::Idle);
                        let _ = event_tx.send(Event::Status {
                            status: PodStatus::Idle,
                        });
                    }
                }
                PodStatus::Paused => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot apply rewind while the Pod is paused; resume or wait for idle first"
                            .into(),
                    });
                }
                PodStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Pod is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::Shutdown => {
                let _ = event_tx.send(Event::Shutdown);
                break;
            }

            Method::ListPods => match discovery.list_visible().await {
                Ok(pods) => match serde_json::to_value(pods) {
                    Ok(pods) => {
                        let _ = event_tx.send(Event::PodsListed { pods });
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize visible pods: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            Method::RestorePod { name } => match discovery.restore(&name).await {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(result) => {
                        let _ = event_tx.send(Event::PodRestored { result });
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize pod restore result: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            Method::RegisterPeer { name } => match discovery.register_peer(&name) {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(result) => {
                        let _ = event_tx.send(Event::PeerRegistered { result });
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize peer registration result: {error}"),
                        });
                    }
                },
                Err(error) => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    });
                }
            },

            // ListCompletions is handled at the socket layer (direct
            // response). If it reaches the controller, ignore it.
            Method::ListCompletions { .. } => {}

            Method::PodEvent(event) => {
                if handle_inbound_pod_event(
                    event,
                    &spawned_registry,
                    &spawner_name,
                    self_parent_socket.as_ref(),
                    &notify_buffer,
                )
                .await
                {
                    // Auto-kick a turn if the Pod is idle so the
                    // notification is not stranded. Matches the
                    // `Method::Notify` idle path.
                    if shared_state.get_status() == PodStatus::Idle {
                        pending = Some(PendingRun::RunForNotification(
                            protocol::InvokeKind::PodEvent,
                        ));
                    }
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

/// Apply an inbound child `PodEvent` exactly once.
///
/// Side effects are control-plane state updates and upward propagation; they
/// run for every event. Only agent-visible events are staged on the notify
/// buffer. The caller owns lifecycle-dependent follow-up such as idle
/// `RunForNotification` auto-kick.
async fn handle_inbound_pod_event(
    event: protocol::PodEvent,
    spawned_registry: &Arc<SpawnedPodRegistry>,
    self_name: &str,
    parent_socket: Option<&PathBuf>,
    notify_buffer: &NotifyBuffer,
) -> bool {
    let self_parent_socket = parent_socket.cloned();
    crate::ipc::event::apply_event_side_effects(
        &event,
        spawned_registry,
        self_name,
        &self_parent_socket,
    )
    .await;

    let notify_agent = event.should_notify_agent();
    if notify_agent {
        notify_buffer.push_pod_event(event);
    }
    notify_agent
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
///
/// `parent_originated` further restricts both upward reports to turns
/// the parent actually delegated (`Method::Run` / `Method::Resume`).
/// `Method::Notify` / inbound `PodEvent` auto-kicks complete silently
/// so the parent's history does not get flooded with child-internal
/// turn boundaries.
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
    parent_originated: bool,
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
                            PodRunResult::RolledBack => (PodStatus::Idle, RunResult::RolledBack),
                        };
                        let _ = event_tx.send(Event::RunEnd { result: run_result });
                        if parent_originated && matches!(run_result, RunResult::Finished) {
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
                        if parent_originated {
                            crate::ipc::event::fire_and_forget(
                                parent_socket.cloned(),
                                protocol::PodEvent::Errored {
                                    pod_name: self_name.to_string(),
                                    message,
                                },
                            );
                        }
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
                    Some(Method::Compact | Method::ListRewindTargets | Method::RewindTo { .. }) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Pod is already executing a turn; rewind/compact can only run while idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::Notify { message }) => {
                        // Live echo arrives via `Event::SystemItem` once
                        // the in-flight turn's next `pending_history_appends`
                        // drains this entry through the interceptor.
                        notify_buffer.push_notify(message);
                    }
                    Some(Method::ListCompletions { .. }) => {}
                    Some(Method::ListPods | Method::RestorePod { .. } | Method::RegisterPeer { .. }) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Pod discovery/control requests are only handled while the Pod is idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::PodEvent(event)) => {
                        // mpsc is consume-once, so we cannot defer this
                        // to the next main-loop iteration — drop here
                        // would lose the event entirely (children fire
                        // and forget). Auto-kick remains unnecessary here:
                        // the in-flight turn will drain agent-visible events
                        // from the notify buffer on its next history append.
                        handle_inbound_pod_event(
                            event,
                            spawned_registry,
                            self_name,
                            parent_socket,
                            notify_buffer,
                        )
                        .await;
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

fn emit_rewind_targets<C, St>(pod: &Pod<C, St>, event_tx: &broadcast::Sender<Event>)
where
    C: LlmClient,
    St: Store,
{
    match pod.list_rewind_targets() {
        Ok((head_entries, targets)) => {
            let _ = event_tx.send(Event::RewindTargets {
                head_entries,
                targets,
            });
        }
        Err(err) => {
            let _ = event_tx.send(Event::Error {
                code: ErrorCode::Internal,
                message: err.to_string(),
            });
        }
    }
}

fn apply_rewind<C, St>(
    pod: &mut Pod<C, St>,
    event_tx: &broadcast::Sender<Event>,
    target: RewindTargetId,
    expected_head_entries: usize,
) -> bool
where
    C: LlmClient,
    St: Store,
{
    match pod.rewind_to(target, expected_head_entries) {
        Ok(applied) => match applied
            .entries
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(entries) => {
                let _ = event_tx.send(Event::RewindApplied {
                    entries,
                    input: applied.input,
                    summary: applied.summary,
                });
                true
            }
            Err(error) => {
                let _ = event_tx.send(Event::Error {
                    code: ErrorCode::Internal,
                    message: format!("failed to encode rewind snapshot: {error}"),
                });
                false
            }
        },
        Err(err) => {
            let _ = event_tx.send(Event::Error {
                code: ErrorCode::InvalidRequest,
                message: err.to_string(),
            });
            false
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
    let context_window = resolved
        .as_ref()
        .map(|cfg| cfg.context_window)
        .unwrap_or(provider::catalog::DEFAULT_CONTEXT_WINDOW);
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
        context_window,
        context_tokens: pod.total_tokens().tokens,
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
        PodError::WorkflowResolve(_) => ErrorCode::InvalidRequest,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::dir::SpawnedPodRecord;
    use protocol::PodEvent;
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[test]
    fn pending_run_parent_origin_table() {
        assert!(PendingRun::Run(Vec::new()).is_parent_originated());
        assert!(PendingRun::Resume.is_parent_originated());
        assert!(
            !PendingRun::RunForNotification(protocol::InvokeKind::Notify).is_parent_originated()
        );
    }

    struct DriveTurnEnv {
        // Held to keep the channel alive; without this `method_rx.recv()`
        // would observe channel-closed and confuse the select! arm.
        _method_tx: mpsc::Sender<Method>,
        method_rx: mpsc::Receiver<Method>,
        event_tx: broadcast::Sender<Event>,
        cancel_tx: mpsc::Sender<()>,
        _cancel_rx: mpsc::Receiver<()>,
        shared_state: Arc<PodSharedState>,
        notify_buffer: NotifyBuffer,
        spawned_registry: Arc<SpawnedPodRegistry>,
        parent_socket_path: PathBuf,
        _runtime_dir: Arc<RuntimeDir>,
        _temp: TempDir,
    }

    async fn make_env() -> DriveTurnEnv {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = Arc::new(
            RuntimeDir::create(temp.path(), "child-pod")
                .await
                .expect("runtime dir create"),
        );
        let (method_tx, method_rx) = mpsc::channel::<Method>(16);
        let (event_tx, _) = broadcast::channel::<Event>(16);
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        let shared_state = Arc::new(PodSharedState::new(
            "child-pod".to_string(),
            session_store::new_segment_id(),
            String::new(),
            protocol::Greeting {
                pod_name: "child-pod".to_string(),
                cwd: String::new(),
                provider: String::new(),
                model: String::new(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 200_000,
                context_tokens: 0,
            },
        ));
        let notify_buffer = NotifyBuffer::new();
        let spawned_registry = SpawnedPodRegistry::new(runtime_dir.clone());
        let parent_socket_path = temp.path().join("parent.sock");

        DriveTurnEnv {
            _method_tx: method_tx,
            method_rx,
            event_tx,
            cancel_tx,
            _cancel_rx: cancel_rx,
            shared_state,
            notify_buffer,
            spawned_registry,
            parent_socket_path,
            _runtime_dir: runtime_dir,
            _temp: temp,
        }
    }

    /// Listen on a bound UnixListener for one inbound connection and
    /// return the first `Method::PodEvent` read from it. Returns `None`
    /// on timeout / EOF / non-PodEvent.
    async fn recv_pod_event(listener: UnixListener, timeout: Duration) -> Option<PodEvent> {
        let accept = async {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut writer = JsonLineWriter::new(w);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        pod_name: "parent".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 200_000,
                        context_tokens: 0,
                    },
                    status: PodStatus::Idle,
                })
                .await
                .ok()?;
            let mut reader = JsonLineReader::new(r);
            match reader.next::<Method>().await {
                Ok(Some(Method::PodEvent(e))) => Some(e),
                _ => None,
            }
        };
        tokio::time::timeout(timeout, accept).await.ok().flatten()
    }

    #[tokio::test]
    async fn parent_originated_finished_fires_turn_ended() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_pod_event(listener, Duration::from_secs(2)));

        let pod_future = async { Ok::<_, PodError>(PodRunResult::Finished) };
        let (status, shutdown) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-pod",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, PodStatus::Idle);
        assert!(!shutdown);

        let event = recv.await.expect("recv task").expect("PodEvent received");
        match event {
            PodEvent::TurnEnded { pod_name } => assert_eq!(pod_name, "child-pod"),
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_parent_originated_finished_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let pod_future = async { Ok::<_, PodError>(PodRunResult::Finished) };
        let (status, _) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-pod",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, PodStatus::Idle);

        // Wait long enough for any (incorrect) fire-and-forget send to
        // land; expect the accept to time out.
        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no PodEvent for non-parent-originated turn"
        );
    }

    #[tokio::test]
    async fn parent_originated_worker_error_fires_errored() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_pod_event(listener, Duration::from_secs(2)));

        let pod_future = async {
            Err::<PodRunResult, _>(PodError::Worker(WorkerError::Aborted(
                "boom from test".into(),
            )))
        };
        let (status, _) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-pod",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, PodStatus::Idle);

        let event = recv.await.expect("recv task").expect("PodEvent received");
        match event {
            PodEvent::Errored { pod_name, message } => {
                assert_eq!(pod_name, "child-pod");
                assert!(message.contains("boom from test"), "got message: {message}");
            }
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_parent_originated_worker_error_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let pod_future = async {
            Err::<PodRunResult, _>(PodError::Worker(WorkerError::Aborted(
                "boom from notify".into(),
            )))
        };
        let (status, _) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-pod",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, PodStatus::Idle);

        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no PodEvent for notification-originated worker error"
        );
    }

    #[tokio::test]
    async fn running_scope_sub_delegated_applies_side_effects_without_notify_buffer() {
        let mut env = make_env().await;
        env.spawned_registry
            .add(SpawnedPodRecord {
                pod_name: "child".into(),
                socket_path: "/tmp/child.sock".into(),
                scope_delegated: vec![],
                callback_address: "/tmp/parent.sock".into(),
            })
            .await
            .expect("seed child record");
        env._method_tx
            .send(Method::PodEvent(PodEvent::ScopeSubDelegated {
                parent_pod: "child".into(),
                sub_pod: "grandchild".into(),
                sub_socket: "/tmp/grandchild.sock".into(),
                scope: vec![],
            }))
            .await
            .expect("send pod event");

        let pod_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, PodError>(PodRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, PodStatus::Idle);
        assert!(!shutdown);
        assert!(
            env.spawned_registry.get("grandchild").await.is_some(),
            "ScopeSubDelegated side effects must still register the grandchild"
        );
        assert!(
            env.notify_buffer.is_empty(),
            "control-plane-only ScopeSubDelegated must not enter the agent-visible notify buffer"
        );
    }

    #[tokio::test]
    async fn running_visible_pod_event_enters_notify_buffer() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::PodEvent(PodEvent::TurnEnded {
                pod_name: "child".into(),
            }))
            .await
            .expect("send pod event");

        let pod_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, PodError>(PodRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "parent",
            &env.spawned_registry,
            false,
        )
        .await;

        assert_eq!(status, PodStatus::Idle);
        assert!(!shutdown);
        assert_eq!(env.notify_buffer.len(), 1);
    }

    #[tokio::test]
    async fn compact_method_is_rejected_while_running() {
        let mut env = make_env().await;
        let mut events = env.event_tx.subscribe();
        env._method_tx
            .send(Method::Compact)
            .await
            .expect("send compact");

        let pod_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, PodError>(PodRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            pod_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-pod",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, PodStatus::Idle);
        assert!(!shutdown);

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event timeout")
            .expect("event");
        match event {
            Event::Error { code, message } => {
                assert_eq!(code, ErrorCode::AlreadyRunning);
                assert!(message.contains("compact"), "got message: {message}");
            }
            other => panic!("expected compact rejection error, got {other:?}"),
        }
    }
}
