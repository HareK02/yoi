use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use worker_runtime::catalog::{CapabilityRequest, ProfileSelector};

use crate::hosts::{
    DiagnosticSeverity, RuntimeDiagnostic, RuntimeRegistry, WorkerOperationState,
    WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent, WorkerSpawnRequest, WorkerSummary,
};

const COMPANION_RUNTIME_ID: &str = "embedded-worker-runtime";
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

#[derive(Debug, Default)]
struct CompanionTranscript {
    items: Vec<CompanionTranscriptItem>,
    next_sequence: u64,
}

#[derive(Debug)]
struct CompanionWorkerState {
    state: CompanionState,
    worker: Option<WorkerSummary>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

pub struct CompanionConsole {
    worker: Mutex<CompanionWorkerState>,
    transcript: Mutex<CompanionTranscript>,
}

impl CompanionConsole {
    pub fn new(runtime: Arc<RuntimeRegistry>) -> Self {
        let initial = spawn_companion_worker(&runtime);
        Self {
            worker: Mutex::new(initial),
            transcript: Mutex::new(CompanionTranscript::default()),
        }
    }

    pub fn status(&self) -> CompanionStatusResponse {
        let worker = match self.worker.lock() {
            Ok(worker) => worker,
            Err(_) => {
                return CompanionStatusResponse {
                    state: CompanionState::Error,
                    worker: None,
                    transport: companion_transport(),
                    diagnostics: vec![diagnostic(
                        "companion_state_unavailable",
                        DiagnosticSeverity::Error,
                        "Companion state is unavailable",
                    )],
                };
            }
        };
        CompanionStatusResponse {
            state: worker.state,
            worker: worker.worker.clone(),
            transport: companion_transport(),
            diagnostics: worker.diagnostics.clone(),
        }
    }

    pub fn transcript(&self, start: usize, limit: usize) -> CompanionTranscriptProjection {
        let transcript = match self.transcript.lock() {
            Ok(transcript) => transcript,
            Err(_) => {
                return CompanionTranscriptProjection {
                    state: CompanionState::Error,
                    start,
                    limit,
                    total_items: 0,
                    next_start: None,
                    items: Vec::new(),
                    diagnostics: vec![diagnostic(
                        "companion_transcript_unavailable",
                        DiagnosticSeverity::Error,
                        "Companion transcript is unavailable",
                    )],
                };
            }
        };
        project_transcript(&transcript, CompanionState::Ready, start, limit, Vec::new())
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

        self.rejected_message_response(diagnostic(
            "companion_llm_not_connected",
            DiagnosticSeverity::Error,
            "Workspace Companion input is disabled until it is connected to actual Worker/LLM execution",
        ))
    }

    pub fn cancel(&self, _request: CompanionCancelRequest) -> CompanionMessageResponse {
        let diagnostics = vec![diagnostic(
            "companion_cancel_no_active_run",
            DiagnosticSeverity::Info,
            "Workspace Companion has no active generation to cancel",
        )];
        match self.transcript.lock() {
            Ok(transcript) => response_from_locked_transcript(
                &transcript,
                CompanionState::Cancelled,
                self.status().worker,
                None,
                None,
                0,
                200,
                diagnostics,
            ),
            Err(_) => CompanionMessageResponse {
                state: CompanionState::Error,
                worker: self.status().worker,
                user_item: None,
                assistant_item: None,
                transcript: CompanionTranscriptProjection {
                    state: CompanionState::Error,
                    start: 0,
                    limit: 200,
                    total_items: 0,
                    next_start: None,
                    items: Vec::new(),
                    diagnostics: vec![diagnostic(
                        "companion_transcript_unavailable",
                        DiagnosticSeverity::Error,
                        "Companion transcript is unavailable",
                    )],
                },
                diagnostics,
            },
        }
    }

    fn rejected_message_response(&self, diagnostic: RuntimeDiagnostic) -> CompanionMessageResponse {
        match self.transcript.lock() {
            Ok(transcript) => response_from_locked_transcript(
                &transcript,
                CompanionState::Rejected,
                self.status().worker,
                None,
                None,
                0,
                200,
                vec![diagnostic],
            ),
            Err(_) => CompanionMessageResponse {
                state: CompanionState::Rejected,
                worker: self.status().worker,
                user_item: None,
                assistant_item: None,
                transcript: CompanionTranscriptProjection {
                    state: CompanionState::Error,
                    start: 0,
                    limit: 200,
                    total_items: 0,
                    next_start: None,
                    items: Vec::new(),
                    diagnostics: vec![diagnostic.clone()],
                },
                diagnostics: vec![diagnostic],
            },
        }
    }
}

fn spawn_companion_worker(runtime: &RuntimeRegistry) -> CompanionWorkerState {
    let request = WorkerSpawnRequest {
        intent: WorkerSpawnIntent::WorkspaceCompanion,
        requested_worker_name: Some("workspace-companion".to_string()),
        acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
            expected_segments: 0,
        },
        profile: Some(ProfileSelector::RuntimeDefault),
        config_bundle: None,
        requested_capabilities: vec![CapabilityRequest::named("conversation")],
    };
    match runtime.spawn_worker(COMPANION_RUNTIME_ID, request) {
        Ok(result) if result.state == WorkerOperationState::Accepted => CompanionWorkerState {
            state: CompanionState::Ready,
            worker: result.worker,
            diagnostics: result.diagnostics,
        },
        Ok(result) => CompanionWorkerState {
            state: CompanionState::Error,
            worker: result.worker,
            diagnostics: result.diagnostics,
        },
        Err(error) => CompanionWorkerState {
            state: CompanionState::Error,
            worker: None,
            diagnostics: vec![diagnostic(
                "companion_worker_spawn_failed",
                DiagnosticSeverity::Error,
                format!("Companion Worker spawn failed: {error:?}"),
            )],
        },
    }
}

fn response_from_locked_transcript(
    transcript: &CompanionTranscript,
    state: CompanionState,
    worker: Option<WorkerSummary>,
    user_item: Option<CompanionTranscriptItem>,
    assistant_item: Option<CompanionTranscriptItem>,
    start: usize,
    limit: usize,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> CompanionMessageResponse {
    CompanionMessageResponse {
        state,
        worker,
        user_item,
        assistant_item,
        transcript: project_transcript(transcript, state, start, limit, diagnostics.clone()),
        diagnostics,
    }
}

fn project_transcript(
    transcript: &CompanionTranscript,
    state: CompanionState,
    start: usize,
    limit: usize,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> CompanionTranscriptProjection {
    let limit = limit.min(200);
    let total_items = transcript.items.len();
    let end = start.saturating_add(limit).min(total_items);
    let items = if start < total_items {
        transcript.items[start..end].to_vec()
    } else {
        Vec::new()
    };
    CompanionTranscriptProjection {
        state,
        start,
        limit,
        total_items,
        next_start: (end < total_items).then_some(end),
        items,
        diagnostics,
    }
}

fn companion_transport() -> CompanionTransportSummary {
    CompanionTransportSummary {
        kind: "embedded_worker_runtime".to_string(),
        completion: "not_connected".to_string(),
        limitation: "Workspace Companion is visible as an embedded Worker, but browser input is disabled until actual Worker/LLM execution is connected.".to_string(),
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

    #[test]
    fn companion_spawns_visible_worker_without_fake_turn() {
        let registry =
            RuntimeRegistry::for_workspace(EmbeddedWorkerRuntime::new_memory("local:test"));
        let registry = Arc::new(registry);
        let companion = CompanionConsole::new(registry.clone());

        let status = companion.status();
        assert_eq!(status.state, CompanionState::Ready);
        let worker = status.worker.clone().expect("companion worker");
        assert_eq!(worker.runtime_id, COMPANION_RUNTIME_ID);
        assert_eq!(worker.role.as_deref(), Some("workspace_companion"));
        assert!(!worker.capabilities.can_stop);

        let workers = registry.list_workers(10);
        assert!(
            workers
                .items
                .iter()
                .any(|item| item.worker_id == worker.worker_id)
        );

        let response = companion.send_message(CompanionMessageRequest {
            content: "hello".to_string(),
        });
        assert_eq!(response.state, CompanionState::Rejected);
        assert!(response.transcript.items.is_empty());
        assert!(
            response
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "companion_llm_not_connected")
        );

        let runtime_transcript = registry
            .transcript(COMPANION_RUNTIME_ID, &worker.worker_id, 0, 10)
            .unwrap();
        assert!(runtime_transcript.items.is_empty());

        let browser_payload = serde_json::to_string(&(status, response)).unwrap();
        for forbidden in [
            "/workspace/project",
            "metadata.json",
            ".jsonl",
            "/run/user/",
        ] {
            assert!(
                !browser_payload.contains(forbidden),
                "companion projection leaked forbidden term {forbidden}: {browser_payload}"
            );
        }
    }
}
