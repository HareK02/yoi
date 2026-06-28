use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use worker_runtime::catalog::ProfileSelector;
use worker_runtime::config_bundle::{
    ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};

use crate::hosts::{
    DiagnosticSeverity, RuntimeDiagnostic, RuntimeRegistry, WorkerInputKind, WorkerInputRequest,
    WorkerOperationState, WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent, WorkerSpawnRequest,
    WorkerSummary, WorkerTranscriptItem as RuntimeTranscriptItem, WorkerTranscriptProjection,
};

const COMPANION_RUNTIME_ID: &str = "embedded-worker-runtime";
const COMPANION_PROFILE_ID: &str = "builtin:companion";
const COMPANION_CONFIG_BUNDLE_ID: &str = "workspace-companion-config";
const MAX_MESSAGE_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionState {
    Ready,
    Busy,
    Error,
    Timeout,
    Cancelled,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionStatusResponse {
    pub state: CompanionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub transport: CompanionTransportSummary,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTransportSummary {
    pub kind: String,
    pub completion: String,
    pub limitation: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CompanionMessageRequest {
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct CompanionCancelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionMessageResponse {
    pub state: CompanionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_item: Option<CompanionTranscriptItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_item: Option<CompanionTranscriptItem>,
    pub transcript: CompanionTranscriptProjection,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTranscriptProjection {
    pub state: CompanionState,
    pub start: usize,
    pub limit: usize,
    pub total_items: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start: Option<usize>,
    pub items: Vec<CompanionTranscriptItem>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTranscriptItem {
    pub sequence: u64,
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub source: String,
    pub status: String,
}

#[derive(Debug)]
struct CompanionWorkerState {
    state: CompanionState,
    worker: Option<WorkerSummary>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

pub struct CompanionConsole {
    runtime: Arc<RuntimeRegistry>,
    worker: Mutex<CompanionWorkerState>,
}

impl CompanionConsole {
    pub fn new(runtime: Arc<RuntimeRegistry>) -> Self {
        let initial = spawn_companion_worker(&runtime);
        Self {
            runtime,
            worker: Mutex::new(initial),
        }
    }

    pub fn status(&self) -> CompanionStatusResponse {
        match self.refresh_worker_state() {
            Ok(worker) => CompanionStatusResponse {
                state: worker.state,
                worker: worker.worker.clone(),
                transport: companion_transport(worker.worker.as_ref()),
                diagnostics: worker.diagnostics.clone(),
            },
            Err(diagnostic) => CompanionStatusResponse {
                state: CompanionState::Error,
                worker: None,
                transport: companion_transport(None),
                diagnostics: vec![diagnostic],
            },
        }
    }

    pub fn transcript(&self, start: usize, limit: usize) -> CompanionTranscriptProjection {
        match self.current_worker() {
            Ok(Some(worker)) => {
                match self
                    .runtime
                    .transcript(COMPANION_RUNTIME_ID, &worker.worker_id, start, limit)
                {
                    Ok(transcript) => project_runtime_transcript(
                        &transcript,
                        companion_state_for_worker(&worker),
                        Vec::new(),
                    ),
                    Err(error) => CompanionTranscriptProjection {
                        state: CompanionState::Error,
                        start,
                        limit,
                        total_items: 0,
                        next_start: None,
                        items: Vec::new(),
                        diagnostics: vec![diagnostic(
                            "companion_transcript_unavailable",
                            DiagnosticSeverity::Error,
                            format!("Companion Worker transcript is unavailable: {error:?}"),
                        )],
                    },
                }
            }
            Ok(None) => CompanionTranscriptProjection {
                state: CompanionState::Error,
                start,
                limit,
                total_items: 0,
                next_start: None,
                items: Vec::new(),
                diagnostics: vec![diagnostic(
                    "companion_worker_unavailable",
                    DiagnosticSeverity::Error,
                    "Workspace Companion Worker is unavailable",
                )],
            },
            Err(diagnostic) => CompanionTranscriptProjection {
                state: CompanionState::Error,
                start,
                limit,
                total_items: 0,
                next_start: None,
                items: Vec::new(),
                diagnostics: vec![diagnostic],
            },
        }
    }

    pub fn send_message(&self, request: CompanionMessageRequest) -> CompanionMessageResponse {
        let content = request.content.trim().to_string();
        if content.is_empty() {
            return self.rejected_message_response(diagnostic(
                "companion_message_empty",
                DiagnosticSeverity::Warning,
                "Companion message content is empty",
            ));
        }
        if content.chars().count() > MAX_MESSAGE_CHARS {
            return self.rejected_message_response(diagnostic(
                "companion_message_too_large",
                DiagnosticSeverity::Warning,
                format!("Companion message exceeds the {MAX_MESSAGE_CHARS} character limit"),
            ));
        }

        let worker = match self.current_worker() {
            Ok(Some(worker)) => worker,
            Ok(None) => {
                return self.rejected_message_response(diagnostic(
                    "companion_worker_unavailable",
                    DiagnosticSeverity::Error,
                    "Workspace Companion Worker is unavailable",
                ));
            }
            Err(diagnostic) => return self.rejected_message_response(diagnostic),
        };

        let response = self.runtime.send_input(
            COMPANION_RUNTIME_ID,
            &worker.worker_id,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: content.clone(),
            },
        );

        match response {
            Ok(result) => {
                let state = match result.state {
                    WorkerOperationState::Accepted => CompanionState::Accepted,
                    WorkerOperationState::Unsupported | WorkerOperationState::Rejected => {
                        CompanionState::Rejected
                    }
                };
                let diagnostics = if result.diagnostics.is_empty() {
                    Vec::new()
                } else {
                    result.diagnostics.clone()
                };
                let projection = self.transcript(0, 200);
                CompanionMessageResponse {
                    state,
                    worker: projection_worker(&self.status()),
                    user_item: projection
                        .items
                        .iter()
                        .rev()
                        .find(|item| item.role == "user" && item.content == content)
                        .cloned(),
                    assistant_item: projection
                        .items
                        .iter()
                        .rev()
                        .find(|item| item.role == "assistant")
                        .cloned(),
                    transcript: projection,
                    diagnostics,
                }
            }
            Err(error) => self.rejected_message_response(diagnostic(
                "companion_worker_input_failed",
                DiagnosticSeverity::Error,
                format!("Companion Worker input dispatch failed: {error:?}"),
            )),
        }
    }

    pub fn cancel(&self, _request: CompanionCancelRequest) -> CompanionMessageResponse {
        let diagnostics = vec![diagnostic(
            "companion_cancel_no_active_run",
            DiagnosticSeverity::Info,
            "Workspace Companion has no active generation to cancel",
        )];
        let status = self.status();
        let projection = self.transcript(0, 200);
        CompanionMessageResponse {
            state: CompanionState::Cancelled,
            worker: status.worker,
            user_item: None,
            assistant_item: projection
                .items
                .iter()
                .rev()
                .find(|item| item.role == "assistant")
                .cloned(),
            transcript: projection,
            diagnostics,
        }
    }

    fn rejected_message_response(&self, diagnostic: RuntimeDiagnostic) -> CompanionMessageResponse {
        let status = self.status();
        let projection = self.transcript(0, 200);
        CompanionMessageResponse {
            state: CompanionState::Rejected,
            worker: status.worker,
            user_item: None,
            assistant_item: projection
                .items
                .iter()
                .rev()
                .find(|item| item.role == "assistant")
                .cloned(),
            transcript: projection,
            diagnostics: vec![diagnostic],
        }
    }

    fn current_worker(&self) -> Result<Option<WorkerSummary>, RuntimeDiagnostic> {
        self.refresh_worker_state()
            .map(|state| state.worker.clone())
    }

    fn refresh_worker_state(&self) -> Result<CompanionWorkerState, RuntimeDiagnostic> {
        let mut state = self.worker.lock().map_err(|_| {
            diagnostic(
                "companion_state_unavailable",
                DiagnosticSeverity::Error,
                "Companion state is unavailable",
            )
        })?;
        let Some(worker_id) = state.worker.as_ref().map(|worker| worker.worker_id.clone()) else {
            return Ok(CompanionWorkerState {
                state: state.state,
                worker: None,
                diagnostics: state.diagnostics.clone(),
            });
        };

        match self.runtime.worker(COMPANION_RUNTIME_ID, &worker_id) {
            Ok(worker) => {
                let mut diagnostics = if worker.capabilities.can_accept_input {
                    Vec::new()
                } else {
                    state.diagnostics.clone()
                };
                if !worker.capabilities.can_accept_input
                    && !diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.code == "companion_worker_not_input_capable")
                {
                    diagnostics.push(companion_not_input_capable_diagnostic(&worker));
                }
                state.state = companion_state_for_worker(&worker);
                state.worker = Some(worker);
                state.diagnostics = diagnostics;
            }
            Err(error) => {
                state.state = CompanionState::Error;
                state.diagnostics = vec![diagnostic(
                    "companion_worker_lookup_failed",
                    DiagnosticSeverity::Error,
                    format!("Companion Worker lookup failed: {error:?}"),
                )];
            }
        }

        Ok(CompanionWorkerState {
            state: state.state,
            worker: state.worker.clone(),
            diagnostics: state.diagnostics.clone(),
        })
    }
}

fn projection_worker(status: &CompanionStatusResponse) -> Option<WorkerSummary> {
    status.worker.clone()
}

fn spawn_companion_worker(runtime: &RuntimeRegistry) -> CompanionWorkerState {
    let selector = companion_profile_selector();
    let mut diagnostics = Vec::new();
    let config_bundle = companion_config_bundle();

    match runtime.sync_config_bundle(COMPANION_RUNTIME_ID, config_bundle) {
        Ok(result) => diagnostics.extend(result.diagnostics),
        Err(error) => diagnostics.push(diagnostic(
            "companion_config_bundle_sync_failed",
            DiagnosticSeverity::Error,
            format!("Workspace Companion config bundle sync failed: {error:?}"),
        )),
    }

    let response = runtime.spawn_worker(
        COMPANION_RUNTIME_ID,
        WorkerSpawnRequest {
            intent: WorkerSpawnIntent::WorkspaceCompanion,
            requested_worker_name: Some("workspace-companion".to_string()),
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: Some(selector),
            initial_input: None,
        },
    );

    match response {
        Ok(response) => {
            diagnostics.extend(response.diagnostics);
            if let Some(worker) = response.worker {
                if !worker.capabilities.can_accept_input {
                    diagnostics.push(companion_not_input_capable_diagnostic(&worker));
                }
                CompanionWorkerState {
                    state: companion_state_for_worker(&worker),
                    worker: Some(worker),
                    diagnostics,
                }
            } else {
                diagnostics.push(diagnostic(
                    "companion_worker_missing",
                    DiagnosticSeverity::Error,
                    "Workspace Companion Worker spawn did not return a Worker projection",
                ));
                CompanionWorkerState {
                    state: CompanionState::Error,
                    worker: None,
                    diagnostics,
                }
            }
        }
        Err(error) => CompanionWorkerState {
            state: CompanionState::Error,
            worker: None,
            diagnostics: vec![diagnostic(
                "companion_worker_spawn_failed",
                DiagnosticSeverity::Error,
                format!("Workspace Companion Worker spawn failed: {error:?}"),
            )],
        },
    }
}

fn companion_profile_selector() -> ProfileSelector {
    ProfileSelector::Builtin(COMPANION_PROFILE_ID.to_string())
}

fn companion_config_bundle() -> ConfigBundle {
    ConfigBundle {
        metadata: ConfigBundleMetadata {
            id: COMPANION_CONFIG_BUNDLE_ID.to_string(),
            digest: String::new(),
            revision: "1".to_string(),
            workspace_id: "workspace-companion".to_string(),
            created_at: Utc::now().to_rfc3339(),
            provenance: ConfigBundleProvenance {
                source: "workspace-server".to_string(),
                detail: Some("workspace-companion".to_string()),
            },
        },
        profiles: vec![ConfigProfileDescriptor {
            selector: companion_profile_selector(),
            label: Some("Workspace Companion".to_string()),
        }],
        declarations: Vec::new(),
    }
    .with_computed_digest()
}

fn companion_state_for_worker(worker: &WorkerSummary) -> CompanionState {
    if !worker.capabilities.can_accept_input {
        return CompanionState::Error;
    }
    match worker.status.as_str() {
        "busy" | "running" | "stopping" => CompanionState::Busy,
        "errored" | "error" | "stopped" | "unavailable" => CompanionState::Error,
        _ => CompanionState::Ready,
    }
}

fn companion_not_input_capable_diagnostic(worker: &WorkerSummary) -> RuntimeDiagnostic {
    diagnostic(
        "companion_worker_not_input_capable",
        DiagnosticSeverity::Error,
        format!(
            "Workspace Companion Worker '{}' is not input-capable; check profile, provider, secret, and authority diagnostics",
            worker.worker_id
        ),
    )
}

fn project_runtime_transcript(
    transcript: &WorkerTranscriptProjection,
    state: CompanionState,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> CompanionTranscriptProjection {
    CompanionTranscriptProjection {
        state,
        start: transcript.start,
        limit: transcript.limit,
        total_items: transcript.total_items,
        next_start: transcript.next_start,
        items: transcript
            .items
            .iter()
            .map(project_runtime_transcript_item)
            .collect(),
        diagnostics,
    }
}

fn project_runtime_transcript_item(item: &RuntimeTranscriptItem) -> CompanionTranscriptItem {
    CompanionTranscriptItem {
        sequence: item.sequence,
        role: item.role.clone(),
        content: item.content.clone(),
        created_at: format!("runtime_sequence:{}", item.sequence),
        source: "worker_runtime".to_string(),
        status: "committed".to_string(),
    }
}

fn companion_transport(worker: Option<&WorkerSummary>) -> CompanionTransportSummary {
    if worker.is_some_and(|worker| worker.capabilities.can_accept_input) {
        CompanionTransportSummary {
            kind: "embedded_worker_runtime".to_string(),
            completion: "connected".to_string(),
            limitation:
                "Workspace Companion input is dispatched through the normal Worker runtime path."
                    .to_string(),
        }
    } else {
        CompanionTransportSummary {
            kind: "embedded_worker_runtime".to_string(),
            completion: "not_input_capable".to_string(),
            limitation:
                "Workspace Companion is a Worker but is not input-capable; inspect typed diagnostics for missing profile, provider, secret, or authority."
                    .to_string(),
        }
    }
}

fn diagnostic(
    code: impl Into<String>,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.into(),
        severity,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::{EmbeddedWorkerRuntime, RuntimeRegistry};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::{Duration, Instant};
    use worker_runtime::execution::{
        WorkerExecutionBackend, WorkerExecutionContext, WorkerExecutionHandle,
        WorkerExecutionOperation, WorkerExecutionResult, WorkerExecutionRunState,
        WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
    };
    use worker_runtime::identity::WorkerRef;
    use worker_runtime::interaction::WorkerInput;

    #[derive(Default)]
    struct DeterministicExecutionBackend {
        contexts: StdMutex<HashMap<WorkerRef, WorkerExecutionContext>>,
    }

    impl WorkerExecutionBackend for DeterministicExecutionBackend {
        fn backend_id(&self) -> &str {
            "deterministic-companion-test"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            self.contexts
                .lock()
                .unwrap()
                .insert(request.worker_ref.clone(), request.context);
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(
                    request.worker_ref.clone(),
                    "deterministic-companion-test",
                ),
                run_state: WorkerExecutionRunState::Idle,
            }
        }

        fn dispatch_input(
            &self,
            handle: &WorkerExecutionHandle,
            request: WorkerInput,
        ) -> WorkerExecutionResult {
            let worker = handle.worker_ref().clone();
            let context = self
                .contexts
                .lock()
                .unwrap()
                .get(&worker)
                .cloned()
                .expect("execution context");
            let content = request.content.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(25));
                let _ = context.publish_protocol_event(protocol::Event::TextDone {
                    text: format!("companion echoed: {content}"),
                });
            });
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Input,
                WorkerExecutionRunState::Idle,
            )
        }
    }

    #[test]
    fn companion_spawns_worker_with_companion_profile_through_runtime_backend() {
        let registry = RuntimeRegistry::for_workspace(
            EmbeddedWorkerRuntime::new_memory_with_execution_backend(
                "local:test",
                Arc::new(DeterministicExecutionBackend::default()),
            )
            .unwrap(),
        );
        let registry = Arc::new(registry);
        let companion = CompanionConsole::new(registry.clone());

        let status = companion.status();
        let worker = status.worker.clone().expect("companion worker");
        assert_eq!(worker.runtime_id, COMPANION_RUNTIME_ID);
        assert_eq!(worker.role.as_deref(), Some(COMPANION_PROFILE_ID));
        assert!(worker.capabilities.can_accept_input);
        assert_eq!(status.transport.completion, "connected");
        assert!(status.diagnostics.is_empty());

        let response = companion.send_message(CompanionMessageRequest {
            content: "hello".to_string(),
        });
        assert_eq!(response.state, CompanionState::Accepted);
        assert!(response.diagnostics.is_empty());
        assert!(
            response
                .transcript
                .items
                .iter()
                .any(|entry| entry.role == "user" && entry.content == "hello")
        );

        let worker_detail = registry
            .worker(COMPANION_RUNTIME_ID, &worker.worker_id)
            .expect("worker detail");
        assert_eq!(worker_detail.profile.as_deref(), Some(COMPANION_PROFILE_ID));

        let browser_payload = serde_json::to_string(&(status, response, worker_detail)).unwrap();
        for forbidden in [
            "/workspace/project",
            "metadata.json",
            ".jsonl",
            "/run/user/",
            "session",
            "manifest",
        ] {
            assert!(
                !browser_payload.contains(forbidden),
                "companion projection leaked forbidden term {forbidden}: {browser_payload}"
            );
        }
    }

    #[test]
    fn companion_dispatches_input_and_projects_assistant_output_from_worker_runtime() {
        let registry = RuntimeRegistry::for_workspace(
            EmbeddedWorkerRuntime::new_memory_with_execution_backend(
                "local:test",
                Arc::new(DeterministicExecutionBackend::default()),
            )
            .expect("embedded runtime"),
        );
        let registry = Arc::new(registry);
        let companion = CompanionConsole::new(registry.clone());
        let status = companion.status();
        let worker = status.worker.clone().expect("companion worker");
        assert_eq!(status.transport.completion, "connected");
        assert_eq!(worker.profile.as_deref(), Some(COMPANION_PROFILE_ID));
        assert!(worker.capabilities.can_accept_input);

        let source = registry
            .observation_source(COMPANION_RUNTIME_ID, &worker.worker_id)
            .expect("observation source");
        let crate::observation::RuntimeObservationSource::Embedded(source) = source else {
            panic!("expected embedded observation source");
        };
        let cursor = source
            .runtime
            .worker_observation_cursor_now(&source.worker_ref)
            .expect("observation cursor");

        let response = companion.send_message(CompanionMessageRequest {
            content: "hello runtime".to_string(),
        });
        assert_eq!(response.state, CompanionState::Accepted);
        assert!(
            response
                .user_item
                .as_ref()
                .is_some_and(|item| item.role == "user" && item.content == "hello runtime")
        );
        assert!(response.diagnostics.is_empty());

        let deadline = Instant::now() + Duration::from_secs(2);
        let observed = loop {
            let observed = source
                .runtime
                .read_worker_observation_events(&source.worker_ref, cursor)
                .expect("observation events");
            if observed.iter().any(|event| {
                serde_json::to_string(event)
                    .unwrap()
                    .contains("companion echoed: hello runtime")
            }) {
                break observed;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for observation event"
            );
            thread::sleep(Duration::from_millis(20));
        };
        let observed_json = serde_json::to_string(&observed).unwrap();
        assert!(observed_json.contains("companion echoed: hello runtime"));

        let deadline = Instant::now() + Duration::from_secs(2);
        let transcript = loop {
            let transcript = companion.transcript(0, 20);
            if transcript.items.iter().any(|item| {
                item.role == "assistant" && item.content == "companion echoed: hello runtime"
            }) {
                break transcript;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for companion assistant output: {transcript:?}"
            );
            thread::sleep(Duration::from_millis(20));
        };

        assert!(
            transcript
                .items
                .iter()
                .any(|item| item.role == "user" && item.content == "hello runtime")
        );
        assert!(transcript.items.iter().any(|item| {
            item.role == "assistant"
                && item.source == "worker_runtime"
                && item.status == "committed"
        }));

        let runtime_transcript = registry
            .transcript(COMPANION_RUNTIME_ID, &worker.worker_id, 0, 20)
            .expect("runtime transcript");
        assert!(runtime_transcript.items.iter().any(|item| {
            item.role == "assistant" && item.content == "companion echoed: hello runtime"
        }));
    }
}
