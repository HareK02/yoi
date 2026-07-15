use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use llm_engine::EngineError;
use llm_engine::llm_client::client::LlmClient;
use manifest::TicketFeatureAccessConfig;
use session_store::WorkerMetadataStore;
use session_store::{LogEntry, Store};
use ticket::LocalTicketBackend;
use ticket::config::TicketConfig;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, warn};

use crate::discovery::{
    WorkerDiscovery, list_workers_tool, restore_worker_tool, send_to_peer_worker_tool,
};
use crate::feature::FeatureRegistryBuilder;
use crate::in_flight::{InFlightEvents, snapshot_from_guard};
use crate::ipc::alerter::Alerter;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::ipc::server::SocketServer;
use crate::runtime::dir::RuntimeDir;
use crate::segment_log_sink::SegmentLogSink;
use crate::shared_state::WorkerSharedState;
use crate::shutdown_after_idle::{
    ShutdownAfterIdleRequest, TicketIntakeReadyShutdownHook, is_ticket_intake_role,
    take_shutdown_request_after_status,
};
use crate::spawn::comm_tools::{read_worker_output_tool, send_to_worker_tool, stop_worker_tool};
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::spawn::tool::spawn_worker_tool;
use crate::ticket_event_notify::{
    TicketEventCompanionNotifyHook, companion_worker_name_for_workspace,
};
use crate::worker::{SystemItemCommitter, Worker, WorkerError, WorkerRunResult, WorkspaceClient};
use protocol::{
    AlertLevel, AlertSource, ErrorCode, Event, Method, RewindTargetId, RunResult, Segment,
    TurnResult, WorkerStatus,
};

// ---------------------------------------------------------------------------
// WorkerHandle — client-facing, Clone-able
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct WorkerHandle {
    method_tx: mpsc::Sender<Method>,
    event_tx: broadcast::Sender<Event>,
    pub shared_state: Arc<WorkerSharedState>,
    pub runtime_dir: Arc<RuntimeDir>,
    pub alerter: Alerter,
    pub in_flight: InFlightEvents,
    /// Segment-log mirror + broadcast handle. The IPC server snapshots
    /// it on every new connection (Event::Snapshot) and forwards
    /// subsequent commits (Event::Entry) on the receiver.
    pub sink: SegmentLogSink,
}

impl WorkerHandle {
    pub async fn send(&self, method: Method) -> Result<(), mpsc::error::SendError<Method>> {
        self.method_tx.send(method).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    pub fn snapshot_event(&self) -> Event {
        self.snapshot_event_with_entry_subscription().0
    }

    pub(crate) fn snapshot_event_with_entry_subscription(
        &self,
    ) -> (Event, broadcast::Receiver<LogEntry>) {
        let (entries, entry_rx, in_flight) = {
            let in_flight_guard = self.in_flight.snapshot_guard();
            let (entries, entry_rx) = self.sink.subscribe_with_snapshot();
            let in_flight = snapshot_from_guard(&in_flight_guard);
            (entries, entry_rx, in_flight)
        };
        let event = Event::Snapshot {
            entries: entries
                .into_iter()
                .map(|entry| serde_json::to_value(entry).expect("log entry serializes"))
                .collect(),
            greeting: self.shared_state.greeting.clone(),
            status: self.shared_state.get_status(),
            in_flight,
        };
        (event, entry_rx)
    }

    pub fn completion_entries(
        &self,
        kind: protocol::CompletionKind,
        prefix: &str,
    ) -> Vec<protocol::CompletionEntry> {
        match kind {
            protocol::CompletionKind::File => self
                .shared_state
                .fs_view()
                .map(|view| view.list_file_completions(prefix))
                .unwrap_or_default()
                .into_iter()
                .map(|c| protocol::CompletionEntry {
                    value: c.path,
                    is_dir: c.is_dir,
                })
                .collect(),
            protocol::CompletionKind::Knowledge => self
                .shared_state
                .list_knowledge_completions(prefix)
                .into_iter()
                .map(|c| protocol::CompletionEntry {
                    value: c.slug,
                    is_dir: false,
                })
                .collect(),
            protocol::CompletionKind::Workflow => self
                .shared_state
                .list_workflow_completions(prefix)
                .into_iter()
                .map(|c| protocol::CompletionEntry {
                    value: c.slug,
                    is_dir: false,
                })
                .collect(),
        }
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
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    event_tx: &broadcast::Sender<Event>,
    status: WorkerStatus,
) {
    shared_state.set_status(status);
    let _ = runtime_dir.write_status(shared_state).await;
    let _ = event_tx.send(Event::Status { status });
}

async fn finish_controller_run<C, St>(
    worker: &mut Worker<C, St>,
    shared_state: &Arc<WorkerSharedState>,
    runtime_dir: &RuntimeDir,
    event_tx: &broadcast::Sender<Event>,
    new_status: WorkerStatus,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // history / user_segments are no longer mirrored on WorkerSharedState —
    // clients reconstruct them from `Event::Snapshot` + live
    // `Event::Entry` deliveries driven by the session-log sink. We
    // only flip the status and kick post-run memory jobs here.
    set_controller_status(shared_state, runtime_dir, event_tx, new_status).await;
    worker.spawn_post_run_memory_jobs();
}

/// Pending turn launch staged by an event handler for the next outer-loop
/// iteration. Each variant carries the input needed by the corresponding
/// `Worker::*` entry point — `RunForNotification` carries none because
/// `worker.run_for_notification()` drains the NotifyBuffer on its own.
enum PendingRun {
    Run(Vec<Segment>),
    /// Self-initiated turn kicked from the notify buffer. The carried
    /// `InvokeKind` is the trigger that flipped the Worker from IDLE
    /// (Notify or WorkerEvent) and is recorded by the Invoke marker
    /// committed at the start of `worker.run_for_notification`.
    RunForNotification(protocol::InvokeKind),
    Resume,
}

impl PendingRun {
    /// Whether this turn was kicked off by the parent (via `Method::Run`
    /// or `Method::Resume`). Used by [`drive_turn`] to gate upward
    /// `WorkerEvent::TurnEnded` / `WorkerEvent::Errored` reports so the parent
    /// only sees completion signals for work it actually delegated.
    /// `RunForNotification` covers self-initiated turns kicked from the
    /// notify buffer (Notify / inbound WorkerEvent) and stays silent.
    fn is_parent_originated(&self) -> bool {
        match self {
            PendingRun::Run(_) | PendingRun::Resume => true,
            PendingRun::RunForNotification(_) => false,
        }
    }
}

fn should_auto_run_notification(status: WorkerStatus, auto_run: bool) -> bool {
    auto_run && status == WorkerStatus::Idle
}

// ---------------------------------------------------------------------------
// WorkerController — actor that owns a Worker
// ---------------------------------------------------------------------------

pub type ShutdownReceiver = oneshot::Receiver<()>;

pub struct WorkerController;

impl WorkerController {
    pub async fn spawn<C, St>(
        mut worker: Worker<C, St>,
        runtime_base: &Path,
    ) -> Result<(WorkerHandle, ShutdownReceiver), std::io::Error>
    where
        C: LlmClient + Clone + 'static,
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        // === 1. Initialization (channels / RuntimeDir / worker-immutable
        //         snapshots / SpawnedWorkerRegistry / alerter attach /
        //         bash-output scope) ===
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (method_tx, method_rx) = mpsc::channel::<Method>(32);
        let (event_tx, _) = broadcast::channel::<Event>(256);
        let alerter = Alerter::new(event_tx.clone());
        let in_flight = InFlightEvents::new(event_tx.clone());
        worker.attach_in_flight_events(in_flight.clone());

        // Runtime directory is created before tool registration because
        // the spawn-tool factories need its socket path, and before the
        // initial status/history writes consume the greeting we build
        // after registration is complete.
        let runtime_dir =
            Arc::new(RuntimeDir::create(runtime_base, &worker.manifest().worker.name).await?);

        let spawner_name = worker.manifest().worker.name.clone();
        let self_parent_socket = worker.callback_socket().cloned();
        let loaded_registry = SpawnedWorkerRegistry::load_from_worker_state_with_reclaim(
            runtime_dir.clone(),
            worker.store().clone(),
            spawner_name.clone(),
            Some(worker.scope().clone()),
        )
        .await?;
        let reclaimed_unreachable = loaded_registry.reclaimed_unreachable;
        let spawned_registry = loaded_registry.registry;
        if reclaimed_unreachable {
            worker.push_notify(
                "Restored Worker state contained unreachable delegated child Workers; their delegated write scopes were reclaimed before resume."
                    .to_string(),
            );
        }

        // Hand the alerter to the Worker so internal operations (compaction,
        // AGENTS.md ingestion during the first turn) can emit user-facing
        // notifications on the same channel.
        worker.attach_alerter(alerter.clone());
        // Also hand the raw broadcast sender so Worker-internal operations
        // can emit typed lifecycle `Event`s (currently: compact progress).
        worker.attach_event_tx(event_tx.clone());

        // Bash spills long outputs to a per-worker subdir under the runtime
        // dir. Push a recursive `allow(Read)` for that path into the
        // Worker's runtime scope so the agent can `Read` saved files
        // without polluting the workspace.
        let bash_output_dir = runtime_dir.path().join("bash-output");
        std::fs::create_dir_all(&bash_output_dir).map_err(|e| {
            std::io::Error::other(format!(
                "create bash output dir {}: {e}",
                bash_output_dir.display()
            ))
        })?;
        worker
            .add_scope_rules([manifest::ScopeRule {
                target: bash_output_dir.clone(),
                permission: manifest::Permission::Read,
                recursive: true,
            }])
            .map_err(std::io::Error::other)?;

        // === 1.5. Direct writer wiring ===
        //
        // Engine callbacks fire `on_history_append` for each assistant
        // item / tool result that lands in history. With the sync
        // writer in place, the callback commits each item directly
        // through a `LogWriterHandle` (no mpsc ferry, no drain task).
        // The same handle is type-erased into a `SystemItemCommitter`
        // and handed to the interceptor for `SystemItem` commits, so
        // assistant / tool / system items all share one commit path.
        let writer_for_system: Arc<dyn SystemItemCommitter> = Arc::new(worker.log_writer_handle());
        worker.attach_log_writer(writer_for_system);
        worker.wire_history_persistence();

        // === 2. Engine event bridge wiring ===
        wire_event_bridges_on_engine(&mut worker, &event_tx, &alerter, &in_flight);

        // === 3. Tool registration (builtin / memory / spawn-orchestration) ===
        let fs_for_view = register_worker_tools(
            &mut worker,
            bash_output_dir,
            runtime_dir.socket_path(),
            runtime_base.to_path_buf(),
            spawned_registry.clone(),
        )
        .await?;

        install_ticket_event_companion_notify_hook(
            &mut worker,
            runtime_base.to_path_buf(),
            spawned_registry.clone(),
        );

        // Intake role Workers self-terminate only after a successful
        // TicketIntakeReady turn has fully settled back to Idle. The request
        // is transient controller state, not model-visible context or ticket
        // claim metadata.
        let shutdown_after_idle = ShutdownAfterIdleRequest::default();
        worker.add_post_tool_call_hook(TicketIntakeReadyShutdownHook::new(
            shutdown_after_idle.clone(),
            is_ticket_intake_role(worker.runtime_ticket_role()),
        ));

        // Materialise pending tool factories so the greeting reflects
        // the actual registered set instead of a hand-maintained mirror.
        worker.engine().tool_server_handle().flush_pending();

        // === 4. Initial runtime files + WorkerSharedState + WorkerHandle +
        //         SocketServer ===
        let manifest_toml = toml::to_string_pretty(worker.manifest()).unwrap_or_default();
        let greeting = build_greeting(&worker);
        let shared_state = Arc::new(WorkerSharedState::new(
            worker.manifest().worker.name.clone(),
            worker.segment_id(),
            manifest_toml.clone(),
            greeting,
        ));
        if let Some(fs_for_view) = fs_for_view {
            shared_state.set_fs_view(crate::fs_view::WorkerFsView::new(fs_for_view));
        }
        shared_state.set_workflows(
            worker
                .workflow_completions()
                .into_iter()
                .map(|slug| crate::shared_state::WorkflowCandidate { slug })
                .collect(),
        );
        shared_state.set_knowledge(
            worker
                .knowledge_completions()
                .into_iter()
                .map(|slug| crate::shared_state::KnowledgeCandidate { slug })
                .collect(),
        );
        runtime_dir.write_manifest(&manifest_toml).await?;
        runtime_dir.write_status(&shared_state).await?;

        let handle = WorkerHandle {
            method_tx,
            event_tx: event_tx.clone(),
            shared_state: shared_state.clone(),
            runtime_dir: runtime_dir.clone(),
            alerter: alerter.clone(),
            in_flight: in_flight.clone(),
            sink: worker.sink(),
        };

        let socket_server = SocketServer::start(&handle).await?;

        // === 5. controller_loop ===
        // Clone cancel sender and notification buffer before moving worker
        // into the controller task so the in-flight turn can be reached
        // via these handles while worker itself is borrowed by drive_turn.
        let cancel_tx = worker.engine_mut().cancel_sender();
        let notify_buffer = worker.notify_buffer_handle();

        tokio::spawn(controller_loop(
            worker,
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
            shutdown_after_idle,
        ));

        Ok((handle, shutdown_rx))
    }
}

/// Wire the per-event broadcast bridges on the Worker's Engine. Each callback
/// re-publishes a worker-level signal as a `protocol::Event` on `event_tx`
/// so subscribers (TUI, socket clients) get a single typed stream.
///
/// `Worker::wire_history_persistence` is called separately to wire the
/// per-item history commit callback so every assistant / tool item
/// landing in `worker.history` becomes a singular `LogEntry::AssistantItem`
/// / `ToolResult` commit through the sync writer.
fn wire_event_bridges_on_engine<C, St>(
    worker: &mut Worker<C, St>,
    event_tx: &broadcast::Sender<Event>,
    alerter: &Alerter,
    in_flight: &InFlightEvents,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let ai_activity = worker.ai_activity_counter();
    let worker = worker.engine_mut();

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

    let in_flight_text = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_text_block(move |block| {
        let block_id = in_flight_text.start_text_block();
        let in_flight_d = in_flight_text.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.text_delta(block_id, text.to_owned());
        });
        let in_flight_s = in_flight_text.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            in_flight_s.text_done(block_id, text.to_owned());
        });
    });

    let in_flight_thinking = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_thinking_block(move |block| {
        // Start fires unconditionally so the TUI can show "Thinking..."
        // even when the provider doesn't emit plaintext deltas.
        activity.fetch_add(1, Ordering::SeqCst);
        let block_id = in_flight_thinking.thinking_start();
        let in_flight_d = in_flight_thinking.clone();
        let activity_d = activity.clone();
        block.on_delta(move |text| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.thinking_delta(block_id, text.to_owned());
        });
        let in_flight_s = in_flight_thinking.clone();
        let activity_s = activity.clone();
        block.on_stop(move |text| {
            if !text.is_empty() {
                activity_s.fetch_add(1, Ordering::SeqCst);
            }
            in_flight_s.thinking_done(block_id, text.to_owned());
        });
    });

    let in_flight_tool = in_flight.clone();
    let activity = ai_activity.clone();
    worker.on_tool_use_block(move |start, block| {
        activity.fetch_add(1, Ordering::SeqCst);
        let block_id = in_flight_tool.tool_call_start(start.id.clone(), start.name.clone());
        let id_for_delta = start.id.clone();
        let in_flight_d = in_flight_tool.clone();
        let activity_d = activity.clone();
        block.on_delta(move |json| {
            activity_d.fetch_add(1, Ordering::SeqCst);
            in_flight_d.tool_call_args_delta(block_id, id_for_delta.clone(), json.to_owned());
        });
        let in_flight_s = in_flight_tool.clone();
        let activity_s = activity.clone();
        block.on_stop(move |call| {
            activity_s.fetch_add(1, Ordering::SeqCst);
            in_flight_s.tool_call_done(block_id, call.id.clone(), call.input.to_string());
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
        alerter_for_worker.alert(AlertLevel::Warn, AlertSource::Engine, message.to_owned());
    });

    // History-append broadcasts (previously `Event::SystemMessage`)
    // have been removed: every persistent history item is now committed
    // through the session-log sink as a typed `LogEntry`, and clients
    // see it via `Event::Snapshot` + live `Event::Entry`. The
    // per-item commit channel is wired at the top of this function.
}

fn install_ticket_event_companion_notify_hook<C, St>(
    worker: &mut Worker<C, St>,
    runtime_base: PathBuf,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    if !is_ticket_orchestrator_role(worker.runtime_ticket_role()) {
        return;
    }

    let ticket_feature = &worker.manifest().feature.ticket;
    if !ticket_feature.enabled
        || !matches!(ticket_feature.access, TicketFeatureAccessConfig::Lifecycle)
    {
        return;
    }

    let Some(local) = worker.local_working_directory() else {
        return;
    };
    let Some(companion_worker_name) = companion_worker_name_for_workspace(&local.root) else {
        return;
    };
    if companion_worker_name == worker.manifest().worker.name {
        return;
    }

    let Ok(ticket_config) = TicketConfig::load_workspace(&local.cwd) else {
        return;
    };
    let backend_root = ticket_config.backend_root().to_path_buf();
    if !backend_root.is_dir() {
        return;
    }

    let discovery = WorkerDiscovery::new(
        worker.worker_metadata_store(),
        worker.manifest().worker.name.clone(),
        runtime_base,
        Some(local.cwd.clone()),
        spawned_registry,
    );
    match discovery.ensure_existing_peer(&companion_worker_name) {
        Ok(Some(_)) => {
            debug!(
                companion = %companion_worker_name,
                orchestrator = %worker.manifest().worker.name,
                "ensured Companion peer relationship for Orchestrator Ticket event notifications"
            );
        }
        Ok(None) => {
            debug!(
                companion = %companion_worker_name,
                orchestrator = %worker.manifest().worker.name,
                "Companion metadata is missing; Ticket event notifications will skip until Companion exists"
            );
        }
        Err(error) => {
            warn!(
                companion = %companion_worker_name,
                orchestrator = %worker.manifest().worker.name,
                error = %error,
                "failed to ensure Companion peer relationship for Orchestrator Ticket event notifications"
            );
        }
    }
    worker.add_post_tool_call_hook(TicketEventCompanionNotifyHook::new(
        LocalTicketBackend::new(backend_root),
        discovery,
        companion_worker_name,
    ));
}

fn is_ticket_orchestrator_role(role: Option<&str>) -> bool {
    role.map(|role| role.eq_ignore_ascii_case("orchestrator"))
        .unwrap_or(false)
}

/// Register the builtin file-manipulation tools, optional memory tools,
/// and the Worker-orchestration tools (SpawnWorker + comm) on the Worker's
/// Engine. Returns the `ScopedFs` clone used to attach a `WorkerFsView` to
/// the shared state.
async fn register_worker_tools<C, St>(
    worker: &mut Worker<C, St>,
    bash_output_dir: PathBuf,
    spawner_socket: PathBuf,
    runtime_base: PathBuf,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
) -> std::io::Result<Option<tools::ScopedFs>>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Worker-immutable snapshots taken before the mutable worker borrow
    // below so the worker borrow doesn't conflict with reads on `worker`.
    let scope_handle = worker.scope().clone();
    let local_filesystem = worker.local_working_directory().cloned();
    let local_workspace_root = local_filesystem.as_ref().map(|local| local.root.clone());
    let task_feature = worker.task_feature();
    let session_id_for_usage = worker.segment_id().to_string();
    let memory_config = worker.manifest().memory.clone();
    let web_config = worker.manifest().web.clone();
    let mcp_config = worker.manifest().mcp.clone();
    let feature_config = worker.manifest().feature.clone();
    let spawner_name = worker.manifest().worker.name.clone();
    let spawner_manifest = worker.manifest().clone();
    let prompts = worker.prompts().clone();
    let worker_metadata_store = worker.store().clone();
    let self_parent_socket = worker.callback_socket().cloned();

    // The Worker's SharedScope is the single source of truth for every
    // ScopedFs when local filesystem authority exists. No-workdir Workers
    // deliberately skip constructing/registering filesystem and Bash tools.
    let (fs_for_view, tracker) = if let Some(local) = local_filesystem.as_ref() {
        let fs = tools::ScopedFs::with_shared_scope(scope_handle.clone(), local.cwd.clone());
        let tracker = tools::Tracker::new();
        let fs_for_view = fs.clone();
        worker
            .engine_mut()
            .register_tools(tools::core_builtin_tools(
                fs,
                tracker.clone(),
                bash_output_dir,
            ));
        (Some(fs_for_view), Some(tracker))
    } else {
        (None, None)
    };
    if feature_config.web.enabled {
        worker
            .engine_mut()
            .register_tools(tools::web_builtin_tools(web_config));
    }

    let mut feature_registry = FeatureRegistryBuilder::new();
    if feature_config.task.enabled {
        feature_registry.add_module(task_feature);
    }
    if feature_config.ticket.enabled || feature_config.ticket_orchestration.enabled {
        let ticket_access = match feature_config.ticket.access {
            TicketFeatureAccessConfig::ReadOnly => {
                crate::feature::builtin::ticket::TicketFeatureAccess::ReadOnly
            }
            TicketFeatureAccessConfig::Lifecycle => {
                crate::feature::builtin::ticket::TicketFeatureAccess::Lifecycle
            }
        };
        // Ticket tools are typed operations over the current workspace Ticket backend.
        // Runtime-hosted Workers prefer the workspace API URI carried by the
        // Worker context; legacy/local Workers fall back to the checked-out worktree.
        let ticket_backend = match worker.workspace_client() {
            WorkspaceClient::Http {
                workspace_id,
                base_url,
            } => crate::feature::builtin::ticket::TicketFeatureBackend::WorkspaceHttp {
                workspace_id: workspace_id.clone(),
                base_url: base_url.clone(),
            },
            _ => {
                let ticket_cwd = local_filesystem
                    .as_ref()
                    .map(|local| &local.cwd)
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "ticket tools require local Worker filesystem authority",
                        )
                    })?;
                crate::feature::builtin::ticket::TicketFeatureBackend::Local {
                    root: ticket_cwd.clone(),
                }
            }
        };
        feature_registry.add_module(
            crate::feature::builtin::ticket::ticket_tools_feature_with_options(
                ticket_backend,
                feature_config.ticket.enabled.then_some(ticket_access),
                feature_config.ticket_orchestration.enabled,
            ),
        );
    }
    for module in crate::feature::plugin::plugin_tool_features_if_enabled(
        feature_config.plugins.enabled,
        &worker.manifest().plugins,
    ) {
        feature_registry = feature_registry.with_module(module);
    }
    if let Some(workspace_root) = local_workspace_root.as_ref() {
        if let Some(module) =
            crate::feature::mcp::discover_stdio_tool_feature(&mcp_config, workspace_root).await
        {
            feature_registry = feature_registry.with_module(module);
        }
    }

    {
        let worker = worker.engine_mut();

        // Memory tools require both explicit feature exposure and memory storage
        // configuration. This keeps resident-memory config separate from the
        // model-visible Memory*/Knowledge* tool surface.
        if feature_config.memory.enabled {
            let mem = memory_config.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "[feature.memory].enabled = true requires a [memory] configuration section",
                )
            })?;
            let workspace_root = local_workspace_root.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "memory tools require local Worker filesystem authority",
                )
            })?;
            let layout = memory::WorkspaceLayout::resolve(mem, workspace_root);
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

        // Worker-orchestration tools (SpawnWorker + the four comm tools) share
        // the Worker-scoped `SpawnedWorkerRegistry` (also consumed by the main
        // loop's `WorkerEvent` handler). Expose them only behind the explicit
        // profile feature and require delegation authority up front so enabling
        // the surface cannot imply broad child scope by accident.
        if feature_config.workers.enabled {
            if spawner_manifest.delegation_scope.allow.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "[feature.workers].enabled = true requires non-empty [[delegation_scope.allow]]",
                ));
            }
            let spawner_cwd = local_filesystem
                .as_ref()
                .map(|local| local.cwd.clone())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "worker spawn tools require local Worker filesystem authority",
                    )
                })?;
            let spawner_workspace_root = local_workspace_root.clone().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "worker spawn tools require local Worker filesystem authority",
                )
            })?;
            worker.register_tool(spawn_worker_tool(
                spawner_name.clone(),
                spawner_socket,
                runtime_base.clone(),
                spawner_workspace_root,
                spawner_cwd.clone(),
                spawned_registry.clone(),
                self_parent_socket,
                spawner_manifest,
                scope_handle,
                prompts,
            ));
            worker.register_tool(send_to_worker_tool(spawned_registry.clone()));
            worker.register_tool(read_worker_output_tool(spawned_registry.clone()));
            worker.register_tool(stop_worker_tool(spawned_registry.clone()));
            let discovery = WorkerDiscovery::new(
                worker_metadata_store,
                spawner_name,
                runtime_base,
                Some(spawner_cwd),
                spawned_registry,
            );
            worker.register_tool(list_workers_tool(discovery.clone()));
            worker.register_tool(restore_worker_tool(discovery.clone()));
            worker.register_tool(send_to_peer_worker_tool(discovery));
        }
    }
    let _feature_install_report = worker.install_features(feature_registry);
    if let Some(tracker) = tracker {
        worker.attach_tracker(tracker);
    }
    Ok(fs_for_view)
}

/// Idle/Paused event loop. Each iteration either fires a staged
/// `PendingRun` (delegating to [`drive_turn`] for the Running phase) or
/// waits for the next `Method`. Method handlers stop at "update state +
/// stage `pending`"; the loop's top-of-iteration block owns the
/// status-flip → run → finish sequence so it lives in exactly one
/// place.
#[allow(clippy::too_many_arguments)]
async fn controller_loop<C, St>(
    mut worker: Worker<C, St>,
    mut method_rx: mpsc::Receiver<Method>,
    event_tx: broadcast::Sender<Event>,
    shared_state: Arc<WorkerSharedState>,
    runtime_dir: Arc<RuntimeDir>,
    cancel_tx: mpsc::Sender<()>,
    notify_buffer: NotifyBuffer,
    self_parent_socket: Option<PathBuf>,
    spawner_name: String,
    spawned_registry: Arc<SpawnedWorkerRegistry>,
    shutdown_tx: oneshot::Sender<()>,
    socket_server: SocketServer,
    shutdown_after_idle: ShutdownAfterIdleRequest,
) where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    // Hold socket server alive for the lifetime of the controller task.
    let _socket_server = socket_server;

    let discovery_runtime_base = runtime_dir
        .path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir.path().to_path_buf());
    let discovery_cwd = worker
        .local_working_directory()
        .map(|local| local.cwd.clone());
    let discovery = WorkerDiscovery::new(
        worker.store().clone(),
        spawner_name.clone(),
        discovery_runtime_base,
        discovery_cwd,
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
            // the Engine at run start.
            worker.engine_mut().clear_pending_cancel();
            set_controller_status(
                &shared_state,
                &runtime_dir,
                &event_tx,
                WorkerStatus::Running,
            )
            .await;
            let parent_originated = run.is_parent_originated();
            let (new_status, shutdown) = match run {
                PendingRun::Run(input) => {
                    drive_turn(
                        worker.run(input),
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
                        worker.run_for_notification(kind),
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
                        worker.resume(),
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
            finish_controller_run(
                &mut worker,
                &shared_state,
                &runtime_dir,
                &event_tx,
                new_status,
            )
            .await;
            if shutdown {
                let _ = event_tx.send(Event::Shutdown);
                break;
            }
            if take_shutdown_request_after_status(&shutdown_after_idle, new_status) {
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
                if shared_state.get_status() == WorkerStatus::Running {
                    // Defensive: the inner select! inside drive_turn
                    // already rejects `Run` while a turn is live, so
                    // this branch is only reachable across a race window
                    // around status flips.
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn".into(),
                    });
                    continue;
                }
                // Stage the run without a speculative user-message echo.
                // `Worker::run` validates the input, commits
                // `LogEntry::UserInput`, and the session-log sink turns that
                // committed entry into the live `Event::UserMessage`. That
                // keeps every client ordered against `SegmentStart` replay and
                // makes persisted history the single source of visible user
                // input. Paused→Run cleanup (orphan tool_result closure +
                // interrupt system note) is applied inside `Worker::run` itself
                // when the worker's `last_run_interrupted` flag is set.
                pending = Some(PendingRun::Run(input));
            }

            Method::Notify { message, auto_run } => {
                // Client-side live echo is delivered as `Event::SystemItem`
                // once the interceptor commits the corresponding
                // `LogEntry::SystemItem` entry — drained out of the
                // notify buffer + broadcast through the sink. No
                // separate echo here.
                worker.push_notify(message);
                // RUNNING / Paused: the buffer push is the entire
                // operation; an in-flight turn (or the next
                // Resume/Run) will drain it at its next
                // pending_history_appends. IDLE: only `auto_run`
                // notifications stage RunForNotification; weak progress
                // notices stay queued until an explicit run/resume.
                if should_auto_run_notification(shared_state.get_status(), auto_run) {
                    pending = Some(PendingRun::RunForNotification(protocol::InvokeKind::Notify));
                }
            }

            Method::Resume => {
                if shared_state.get_status() != WorkerStatus::Paused {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::NotPaused,
                        message: "Worker is not paused".into(),
                    });
                    continue;
                }
                pending = Some(PendingRun::Resume);
            }

            Method::Cancel => match shared_state.get_status() {
                WorkerStatus::Paused => match worker.cancel_paused_turn() {
                    Ok(()) => {
                        set_controller_status(
                            &shared_state,
                            &runtime_dir,
                            &event_tx,
                            WorkerStatus::Idle,
                        )
                        .await;
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                },
                WorkerStatus::Idle => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::NotRunning,
                        message: "Worker is not running".into(),
                    });
                }
                WorkerStatus::Running => {
                    // Running turns receive Cancel through drive_turn; this is
                    // only reachable across a defensive race window.
                    let _ = cancel_tx.try_send(());
                }
            },

            Method::Pause => {
                // Already paused → idempotent no-op. Otherwise the
                // Worker is Idle (Running turns go through `drive_turn`,
                // not this outer match), so there is nothing to pause.
                if shared_state.get_status() != WorkerStatus::Paused {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::NotRunning,
                        message: "Worker is not running".into(),
                    });
                }
            }

            Method::Compact => match shared_state.get_status() {
                WorkerStatus::Idle => {
                    if let Err(error) = worker.manual_compact().await {
                        let _ = event_tx.send(Event::Error {
                            code: worker_error_code(&error),
                            message: error.to_string(),
                        });
                    }
                }
                WorkerStatus::Paused => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot compact while the Worker is paused; resume or start a fresh turn first"
                            .into(),
                    });
                }
                WorkerStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message:
                            "Worker is already executing a turn; compact can only run while idle"
                                .into(),
                    });
                }
            },

            Method::ListRewindTargets => match shared_state.get_status() {
                WorkerStatus::Idle | WorkerStatus::Paused => {
                    emit_rewind_targets(&worker, &event_tx)
                }
                WorkerStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::RewindTo {
                target,
                expected_head_entries,
            } => match shared_state.get_status() {
                WorkerStatus::Idle => {
                    if apply_rewind(&mut worker, &event_tx, target, expected_head_entries) {
                        shared_state.set_status(WorkerStatus::Idle);
                        let _ = event_tx.send(Event::Status {
                            status: WorkerStatus::Idle,
                        });
                    }
                }
                WorkerStatus::Paused => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "Cannot apply rewind while the Worker is paused; resume or wait for idle first"
                            .into(),
                    });
                }
                WorkerStatus::Running => {
                    let _ = event_tx.send(Event::Error {
                        code: ErrorCode::AlreadyRunning,
                        message: "Worker is already executing a turn; rewind can only run while idle or paused"
                            .into(),
                    });
                }
            },

            Method::Shutdown => {
                let _ = event_tx.send(Event::Shutdown);
                break;
            }

            Method::ListWorkers => match discovery.list_visible().await {
                Ok(workers) => match serde_json::to_value(workers) {
                    Ok(workers) => {
                        let _ = event_tx.send(Event::WorkersListed { workers });
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize visible workers: {error}"),
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

            Method::RestoreWorker { name } => match discovery.restore(&name).await {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(result) => {
                        let _ = event_tx.send(Event::WorkerRestored { result });
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::Internal,
                            message: format!("serialize worker restore result: {error}"),
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

            Method::WorkerEvent(event) => {
                if handle_inbound_worker_event(
                    event,
                    &spawned_registry,
                    &spawner_name,
                    self_parent_socket.as_ref(),
                    &notify_buffer,
                )
                .await
                {
                    // Auto-kick a turn if the Worker is idle so the
                    // notification is not stranded. Matches the
                    // `Method::Notify` idle path.
                    if shared_state.get_status() == WorkerStatus::Idle {
                        pending = Some(PendingRun::RunForNotification(
                            protocol::InvokeKind::WorkerEvent,
                        ));
                    }
                }
            }
        }
    }

    // Background memory jobs own extract/consolidate workers after a
    // turn completes. Join them before the controller task exits so
    // staging writes and consolidation cleanups are not abandoned.
    worker.wait_for_memory_jobs().await;

    // Report upward that this Worker is stopping before the controller
    // task exits. Awaited (not fire-and-forget): after `shutdown_tx.send`
    // the process may exit quickly, and a spawned task would be killed
    // mid-send. The `connect_and_send` helper enforces a 5 s timeout so
    // a stuck parent cannot block process exit indefinitely.
    if let Some(parent) = self_parent_socket.as_ref() {
        if let Err(e) = crate::ipc::event::send_worker_event(
            parent,
            protocol::WorkerEvent::ShutDown {
                worker_name: spawner_name.clone(),
            },
        )
        .await
        {
            tracing::warn!(error = %e, "ShutDown WorkerEvent send failed");
        }
    }

    let _ = shutdown_tx.send(());
}

/// Apply an inbound child `WorkerEvent` exactly once.
///
/// Side effects are control-plane state updates and upward propagation; they
/// run for every event. Only agent-visible events are staged on the notify
/// buffer. The caller owns lifecycle-dependent follow-up such as idle
/// `RunForNotification` auto-kick.
async fn handle_inbound_worker_event(
    event: protocol::WorkerEvent,
    spawned_registry: &Arc<SpawnedWorkerRegistry>,
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
        notify_buffer.push_worker_event(event);
    }
    notify_agent
}

/// Drives a Worker future (one in-flight turn) while concurrently
/// processing incoming methods through an inner select! arm. Returns
/// `(final_status, shutdown_requested)`.
///
/// `parent_socket` / `self_name` drive upward `WorkerEvent` reports
/// (`TurnEnded` on a clean Finished, `Errored` on a worker failure).
/// `None` parent skips the send (top-level Worker). Transient method
/// rejections such as `AlreadyRunning` are intentionally NOT reported
/// as `Errored` — only the worker-execution `Err` branch below fires.
///
/// `parent_originated` further restricts both upward reports to turns
/// the parent actually delegated (`Method::Run` / `Method::Resume`).
/// `Method::Notify` / inbound `WorkerEvent` auto-kicks complete silently
/// so the parent's history does not get flooded with child-internal
/// turn boundaries.
#[allow(clippy::too_many_arguments)]
async fn drive_turn<F>(
    worker_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    shared_state: &Arc<WorkerSharedState>,
    notify_buffer: &NotifyBuffer,
    parent_socket: Option<&PathBuf>,
    self_name: &str,
    spawned_registry: &Arc<SpawnedWorkerRegistry>,
    parent_originated: bool,
) -> (WorkerStatus, bool)
where
    F: std::future::Future<Output = Result<WorkerRunResult, WorkerError>>,
{
    tokio::pin!(worker_future);
    let mut shutdown_requested = false;
    let mut pause_requested = false;

    loop {
        tokio::select! {
            result = &mut worker_future => {
                return match result {
                    Ok(r) => {
                        let (status, run_result) = match r {
                            WorkerRunResult::Finished => (WorkerStatus::Idle, RunResult::Finished),
                            WorkerRunResult::Paused => (WorkerStatus::Paused, RunResult::Paused),
                            WorkerRunResult::LimitReached => (WorkerStatus::Idle, RunResult::LimitReached),
                            WorkerRunResult::RolledBack => (WorkerStatus::Idle, RunResult::RolledBack),
                        };
                        let _ = event_tx.send(Event::RunEnd { result: run_result });
                        if parent_originated && matches!(run_result, RunResult::Finished) {
                            crate::ipc::event::fire_and_forget(
                                parent_socket.cloned(),
                                protocol::WorkerEvent::TurnEnded {
                                    worker_name: self_name.to_string(),
                                },
                            );
                        }
                        (status, shutdown_requested)
                    }
                    Err(WorkerError::Engine(EngineError::Cancelled)) if pause_requested => {
                        // User-initiated Pause. Report the transition to
                        // clients as a normal Paused run-end, and
                        // intentionally skip `WorkerEvent::Errored` upward:
                        // that channel is reserved for worker runtime
                        // failures, not deliberate interruptions.
                        let _ = event_tx.send(Event::RunEnd { result: RunResult::Paused });
                        (WorkerStatus::Paused, shutdown_requested)
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
                                protocol::WorkerEvent::Errored {
                                    worker_name: self_name.to_string(),
                                    message,
                                },
                            );
                        }
                        (WorkerStatus::Idle, shutdown_requested)
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
                            message: "Worker is already executing a turn".into(),
                        });
                    }
                    Some(Method::Compact | Method::ListRewindTargets | Method::RewindTo { .. }) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker is already executing a turn; rewind/compact can only run while idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::Notify { message, .. }) => {
                        // Live echo arrives via `Event::SystemItem` once
                        // the in-flight turn's next `pending_history_appends`
                        // drains this entry through the interceptor.
                        notify_buffer.push_notify(message);
                    }
                    Some(Method::ListCompletions { .. }) => {}
                    Some(Method::ListWorkers | Method::RestoreWorker { .. } | Method::RegisterPeer { .. }) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Worker discovery/control requests are only handled while the Worker is idle or paused"
                                .into(),
                        });
                    }
                    Some(Method::WorkerEvent(event)) => {
                        // mpsc is consume-once, so we cannot defer this
                        // to the next main-loop iteration — drop here
                        // would lose the event entirely (children fire
                        // and forget). Auto-kick remains unnecessary here:
                        // the in-flight turn will drain agent-visible events
                        // from the notify buffer on its next history append.
                        handle_inbound_worker_event(
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
                        shared_state.set_status(WorkerStatus::Idle);
                        return (WorkerStatus::Idle, false);
                    }
                }
            }
        }
    }
}

fn emit_rewind_targets<C, St>(worker: &Worker<C, St>, event_tx: &broadcast::Sender<Event>)
where
    C: LlmClient,
    St: Store,
{
    match worker.list_rewind_targets() {
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
    worker: &mut Worker<C, St>,
    event_tx: &broadcast::Sender<Event>,
    target: RewindTargetId,
    expected_head_entries: usize,
) -> bool
where
    C: LlmClient,
    St: Store,
{
    match worker.rewind_to(target, expected_head_entries) {
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

fn build_greeting<C, St>(worker: &Worker<C, St>) -> protocol::Greeting
where
    C: LlmClient,
    St: Store,
{
    let manifest = worker.manifest();
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
    // Engine. Caller must have flushed pending factories first; without
    // a flush the tool table is empty and this returns an empty vec.
    let tool_names: Vec<String> = worker
        .engine()
        .tool_server_handle()
        .tool_definitions_sorted()
        .into_iter()
        .map(|def| def.name)
        .collect();
    protocol::Greeting {
        worker_name: manifest.worker.name.clone(),
        cwd: worker
            .local_working_directory()
            .map(|local| local.cwd.display().to_string())
            .unwrap_or_default(),
        provider: provider_name,
        model: model_id,
        scope_summary: worker.scope_snapshot().summary(),
        tools: tool_names,
        context_window,
        context_tokens: worker.total_tokens().tokens,
    }
}

fn worker_error_code(e: &WorkerError) -> ErrorCode {
    match e {
        WorkerError::Engine(we) => match we {
            EngineError::Tool(_) => ErrorCode::ToolError,
            EngineError::Client(_) => ErrorCode::ProviderError,
            _ => ErrorCode::Internal,
        },
        WorkerError::Provider(_) => ErrorCode::ProviderError,
        WorkerError::WorkflowResolve(_) => ErrorCode::InvalidRequest,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::dir::SpawnedWorkerRecord;
    use protocol::WorkerEvent;
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

    #[test]
    fn notification_auto_run_gate_only_allows_idle_auto_run() {
        assert!(should_auto_run_notification(WorkerStatus::Idle, true));
        assert!(!should_auto_run_notification(WorkerStatus::Idle, false));
        assert!(!should_auto_run_notification(WorkerStatus::Running, true));
        assert!(!should_auto_run_notification(WorkerStatus::Paused, true));
    }

    struct DriveTurnEnv {
        // Held to keep the channel alive; without this `method_rx.recv()`
        // would observe channel-closed and confuse the select! arm.
        _method_tx: mpsc::Sender<Method>,
        method_rx: mpsc::Receiver<Method>,
        event_tx: broadcast::Sender<Event>,
        cancel_tx: mpsc::Sender<()>,
        _cancel_rx: mpsc::Receiver<()>,
        shared_state: Arc<WorkerSharedState>,
        notify_buffer: NotifyBuffer,
        spawned_registry: Arc<SpawnedWorkerRegistry>,
        parent_socket_path: PathBuf,
        _runtime_dir: Arc<RuntimeDir>,
        _temp: TempDir,
    }

    async fn make_env() -> DriveTurnEnv {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = Arc::new(
            RuntimeDir::create(temp.path(), "child-worker")
                .await
                .expect("runtime dir create"),
        );
        let (method_tx, method_rx) = mpsc::channel::<Method>(16);
        let (event_tx, _) = broadcast::channel::<Event>(16);
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        let shared_state = Arc::new(WorkerSharedState::new(
            "child-worker".to_string(),
            session_store::new_segment_id(),
            String::new(),
            protocol::Greeting {
                worker_name: "child-worker".to_string(),
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
        let spawned_registry = SpawnedWorkerRegistry::new(runtime_dir.clone());
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
    /// return the first `Method::WorkerEvent` read from it. Returns `None`
    /// on timeout / EOF / non-WorkerEvent.
    async fn recv_worker_event(listener: UnixListener, timeout: Duration) -> Option<WorkerEvent> {
        let accept = async {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut writer = JsonLineWriter::new(w);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "parent".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 200_000,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .ok()?;
            let mut reader = JsonLineReader::new(r);
            match reader.next::<Method>().await {
                Ok(Some(Method::WorkerEvent(e))) => Some(e),
                _ => None,
            }
        };
        tokio::time::timeout(timeout, accept).await.ok().flatten()
    }

    #[tokio::test]
    async fn parent_originated_finished_fires_turn_ended() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_worker_event(listener, Duration::from_secs(2)));

        let worker_future = async { Ok::<_, WorkerError>(WorkerRunResult::Finished) };
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);
        assert!(!shutdown);

        let event = recv
            .await
            .expect("recv task")
            .expect("WorkerEvent received");
        match event {
            WorkerEvent::TurnEnded { worker_name } => assert_eq!(worker_name, "child-worker"),
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_parent_originated_finished_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let worker_future = async { Ok::<_, WorkerError>(WorkerRunResult::Finished) };
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        // Wait long enough for any (incorrect) fire-and-forget send to
        // land; expect the accept to time out.
        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no WorkerEvent for non-parent-originated turn"
        );
    }

    #[tokio::test]
    async fn parent_originated_worker_error_fires_errored() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");
        let recv = tokio::spawn(recv_worker_event(listener, Duration::from_secs(2)));

        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::Engine(EngineError::Aborted(
                "boom from test".into(),
            )))
        };
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            true,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        let event = recv
            .await
            .expect("recv task")
            .expect("WorkerEvent received");
        match event {
            WorkerEvent::Errored {
                worker_name,
                message,
            } => {
                assert_eq!(worker_name, "child-worker");
                assert!(message.contains("boom from test"), "got message: {message}");
            }
            other => panic!("expected Errored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_parent_originated_worker_error_stays_silent() {
        let mut env = make_env().await;
        let listener = UnixListener::bind(&env.parent_socket_path).expect("bind listener");

        let worker_future = async {
            Err::<WorkerRunResult, _>(WorkerError::Engine(EngineError::Aborted(
                "boom from notify".into(),
            )))
        };
        let (status, _) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);

        let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accept.is_err(),
            "expected no WorkerEvent for notification-originated worker error"
        );
    }

    #[tokio::test]
    async fn running_scope_sub_delegated_applies_side_effects_without_notify_buffer() {
        let mut env = make_env().await;
        env.spawned_registry
            .add(SpawnedWorkerRecord {
                worker_name: "child".into(),
                socket_path: "/tmp/child.sock".into(),
                scope_delegated: vec![],
                callback_address: "/tmp/parent.sock".into(),
            })
            .await
            .expect("seed child record");
        env._method_tx
            .send(Method::WorkerEvent(WorkerEvent::ScopeSubDelegated {
                parent_worker: "child".into(),
                sub_worker: "grandchild".into(),
                sub_socket: "/tmp/grandchild.sock".into(),
                scope: vec![],
            }))
            .await
            .expect("send worker event");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            worker_future,
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

        assert_eq!(status, WorkerStatus::Idle);
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
    async fn running_visible_worker_event_enters_notify_buffer() {
        let mut env = make_env().await;
        env._method_tx
            .send(Method::WorkerEvent(WorkerEvent::TurnEnded {
                worker_name: "child".into(),
            }))
            .await
            .expect("send worker event");

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            worker_future,
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

        assert_eq!(status, WorkerStatus::Idle);
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

        let worker_future = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, WorkerError>(WorkerRunResult::Finished)
        };
        let (status, shutdown) = drive_turn(
            worker_future,
            &mut env.method_rx,
            &env.event_tx,
            &env.cancel_tx,
            &env.shared_state,
            &env.notify_buffer,
            Some(&env.parent_socket_path),
            "child-worker",
            &env.spawned_registry,
            false,
        )
        .await;
        assert_eq!(status, WorkerStatus::Idle);
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
