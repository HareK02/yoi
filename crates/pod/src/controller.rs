use std::path::Path;
use std::sync::Arc;

use llm_worker::hook::ToolCall;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::subscriber::WorkerSubscriber;
use llm_worker::timeline::event::{ErrorEvent, UsageEvent};
use llm_worker::timeline::{TextBlockEvent, ToolUseBlockEvent};
use llm_worker::WorkerError;
use llm_worker_persistence::Store;
use tokio::sync::{broadcast, mpsc};

use crate::pod::{Pod, PodRunResult, PodError};
use protocol::{ErrorCode, Event, Method, RunResult, TurnResult};
use crate::runtime_dir::RuntimeDir;
use crate::shared_state::{PodSharedState, PodStatus};
use crate::socket_server::SocketServer;

// ---------------------------------------------------------------------------
// PodHandle — client-facing, Clone-able
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PodHandle {
    method_tx: mpsc::Sender<Method>,
    event_tx: broadcast::Sender<Event>,
    pub shared_state: Arc<PodSharedState>,
    pub runtime_dir: Arc<RuntimeDir>,
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
}

// ---------------------------------------------------------------------------
// PodController — actor that owns a Pod
// ---------------------------------------------------------------------------

pub struct PodController;

impl PodController {
    pub async fn spawn<C, St>(
        mut pod: Pod<C, St>,
        runtime_base: &Path,
    ) -> Result<PodHandle, std::io::Error>
    where
        C: LlmClient + 'static,
        St: Store + 'static,
    {
        let (method_tx, mut method_rx) = mpsc::channel::<Method>(32);
        let (event_tx, _) = broadcast::channel::<Event>(256);

        let manifest_toml = toml::to_string_pretty(pod.manifest()).unwrap_or_default();
        let shared_state = Arc::new(PodSharedState::new(
            pod.manifest().pod.name.clone(),
            pod.session_id(),
            manifest_toml.clone(),
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
        };

        // Start socket server (lives as a background task, cleaned up on drop via RuntimeDir)
        let _socket_server = SocketServer::start(&handle).await?;
        // Keep the server alive by moving it into the controller task
        // (it will be dropped when the task ends)

        // Register the event bridge subscriber on the worker
        let bridge = EventBridgeSubscriber {
            event_tx: event_tx.clone(),
        };
        pod.session_mut().worker.subscribe(bridge);

        // Clone cancel sender before moving pod
        let cancel_tx = pod.session_mut().worker.cancel_sender();

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

                        let new_status = run_with_cancel_support(
                            pod.run(&input),
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                        )
                        .await;

                        let items = pod.session_mut().worker.history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;
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

                        let new_status = run_with_cancel_support(
                            pod.resume(),
                            &mut method_rx,
                            &event_tx,
                            &cancel_tx,
                            &shared_state,
                        )
                        .await;

                        let items = pod.session_mut().worker.history().to_vec();
                        shared_state.update_history(items);
                        shared_state.set_status(new_status);
                        let _ = runtime_dir.write_status(&shared_state).await;
                        let _ = runtime_dir.write_history(&shared_state).await;
                    }

                    Method::Cancel => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::NotRunning,
                            message: "Pod is not running".into(),
                        });
                    }
                }
            }
        });

        Ok(handle)
    }
}

/// Runs a Pod future while concurrently processing incoming methods.
/// Only `Cancel` is handled during execution; `Run` and `Resume` get errors.
async fn run_with_cancel_support<F>(
    pod_future: F,
    method_rx: &mut mpsc::Receiver<Method>,
    event_tx: &broadcast::Sender<Event>,
    cancel_tx: &mpsc::Sender<()>,
    shared_state: &Arc<PodSharedState>,
) -> PodStatus
where
    F: std::future::Future<Output = Result<PodRunResult, PodError>>,
{
    tokio::pin!(pod_future);

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
                        status
                    }
                    Err(e) => {
                        let code = worker_error_code(&e);
                        let _ = event_tx.send(Event::Error {
                            code,
                            message: e.to_string(),
                        });
                        PodStatus::Idle
                    }
                };
            }
            method = method_rx.recv() => {
                match method {
                    Some(Method::Cancel) => {
                        let _ = cancel_tx.try_send(());
                    }
                    Some(Method::Run { .. } | Method::Resume) => {
                        let _ = event_tx.send(Event::Error {
                            code: ErrorCode::AlreadyRunning,
                            message: "Pod is already executing a turn".into(),
                        });
                    }
                    None => {
                        let _ = cancel_tx.try_send(());
                        shared_state.set_status(PodStatus::Idle);
                        return PodStatus::Idle;
                    }
                }
            }
        }
    }
}

fn worker_error_code(e: &PodError) -> ErrorCode {
    match e {
        PodError::Session(se) => {
            use llm_worker_persistence::SessionError;
            match se {
                SessionError::Worker(we) => match we {
                    WorkerError::Tool(_) => ErrorCode::ToolError,
                    WorkerError::Client(_) => ErrorCode::ProviderError,
                    _ => ErrorCode::Internal,
                },
                _ => ErrorCode::Internal,
            }
        }
        PodError::Provider(_) => ErrorCode::ProviderError,
        _ => ErrorCode::Internal,
    }
}

// ---------------------------------------------------------------------------
// EventBridgeSubscriber — bridges Worker events to broadcast channel
// ---------------------------------------------------------------------------

struct EventBridgeSubscriber {
    event_tx: broadcast::Sender<Event>,
}

impl WorkerSubscriber for EventBridgeSubscriber {
    type TextBlockScope = ();
    type ToolUseBlockScope = ();

    fn on_turn_start(&mut self, turn: usize) {
        let _ = self.event_tx.send(Event::TurnStart { turn });
    }

    fn on_turn_end(&mut self, turn: usize) {
        let _ = self.event_tx.send(Event::TurnEnd {
            turn,
            result: TurnResult::Finished,
        });
    }

    fn on_text_block(&mut self, _scope: &mut (), event: &TextBlockEvent) {
        match event {
            TextBlockEvent::Delta(text) => {
                let _ = self.event_tx.send(Event::TextDelta {
                    text: text.clone(),
                });
            }
            TextBlockEvent::Start(_) | TextBlockEvent::Stop(_) => {}
        }
    }

    fn on_text_complete(&mut self, text: &str) {
        let _ = self.event_tx.send(Event::TextDone {
            text: text.to_owned(),
        });
    }

    fn on_tool_use_block(&mut self, _scope: &mut (), event: &ToolUseBlockEvent) {
        match event {
            ToolUseBlockEvent::Start(start) => {
                let _ = self.event_tx.send(Event::ToolCallStart {
                    id: start.id.clone(),
                    name: start.name.clone(),
                });
            }
            ToolUseBlockEvent::InputJsonDelta(json) => {
                let _ = self.event_tx.send(Event::ToolCallArgsDelta {
                    id: String::new(),
                    json: json.clone(),
                });
            }
            ToolUseBlockEvent::Stop(_) => {}
        }
    }

    fn on_tool_call_complete(&mut self, call: &ToolCall) {
        let _ = self.event_tx.send(Event::ToolCallDone {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.input.to_string(),
        });
    }

    fn on_usage(&mut self, event: &UsageEvent) {
        let _ = self.event_tx.send(Event::Usage {
            input_tokens: event.input_tokens,
            output_tokens: event.output_tokens,
        });
    }

    fn on_error(&mut self, event: &ErrorEvent) {
        let _ = self.event_tx.send(Event::Error {
            code: ErrorCode::ProviderError,
            message: event.message.clone(),
        });
    }
}
