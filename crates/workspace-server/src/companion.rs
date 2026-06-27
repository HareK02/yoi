use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use worker_runtime::catalog::{CapabilityRequest, ProfileSelector};

use crate::hosts::{
    DiagnosticSeverity, RuntimeDiagnostic, RuntimeRegistry, WorkerInputKind, WorkerInputRequest,
    WorkerOperationState, WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent, WorkerSpawnRequest,
    WorkerSummary,
};

const COMPANION_RUNTIME_ID: &str = "embedded-worker-runtime";
const MAX_MESSAGE_CHARS: usize = 8_000;
const PROVIDERLESS_RESPONSE: &str =
    include_str!("../../../resources/prompts/worker/web_companion_providerless.md");

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
    runtime: Arc<RuntimeRegistry>,
    worker: Mutex<CompanionWorkerState>,
    transcript: Mutex<CompanionTranscript>,
}

impl CompanionConsole {
    pub fn new(runtime: Arc<RuntimeRegistry>) -> Self {
        let initial = spawn_companion_worker(&runtime);
        Self {
            runtime,
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
                    transport: providerless_transport(),
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
            transport: providerless_transport(),
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

        let mut transcript = match self.transcript.try_lock() {
            Ok(transcript) => transcript,
            Err(std::sync::TryLockError::WouldBlock) => {
                return self.busy_message_response();
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return self.error_message_response(diagnostic(
                    "companion_transcript_unavailable",
                    DiagnosticSeverity::Error,
                    "Companion transcript is unavailable",
                ));
            }
        };

        let (worker, mut diagnostics) = match self.current_worker() {
            Ok((Some(worker), diagnostics)) => (worker, diagnostics),
            Ok((None, diagnostics)) => {
                return response_from_locked_transcript(
                    &transcript,
                    CompanionState::Error,
                    None,
                    None,
                    None,
                    0,
                    200,
                    diagnostics,
                );
            }
            Err(diagnostic) => {
                return response_from_locked_transcript(
                    &transcript,
                    CompanionState::Error,
                    None,
                    None,
                    None,
                    0,
                    200,
                    vec![diagnostic],
                );
            }
        };

        let user_item = transcript.push("user", content.clone(), "browser_request", "accepted");
        match self.runtime.send_input(
            &worker.runtime_id,
            &worker.worker_id,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content,
            },
        ) {
            Ok(result) if result.state == WorkerOperationState::Accepted => {
                diagnostics.extend(result.diagnostics);
            }
            Ok(result) => {
                diagnostics.extend(result.diagnostics);
                diagnostics.push(diagnostic(
                    "companion_runtime_input_rejected",
                    DiagnosticSeverity::Error,
                    "Embedded Companion Worker rejected the browser message",
                ));
                return response_from_locked_transcript(
                    &transcript,
                    CompanionState::Error,
                    Some(worker),
                    Some(user_item),
                    None,
                    0,
                    200,
                    diagnostics,
                );
            }
            Err(error) => {
                diagnostics.push(diagnostic(
                    "companion_runtime_input_failed",
                    DiagnosticSeverity::Error,
                    format!("Embedded Companion Worker input failed: {error:?}"),
                ));
                return response_from_locked_transcript(
                    &transcript,
                    CompanionState::Error,
                    Some(worker),
                    Some(user_item),
                    None,
                    0,
                    200,
                    diagnostics,
                );
            }
        }

        diagnostics.push(diagnostic(
            "companion_providerless_boundary",
            DiagnosticSeverity::Info,
            "Real LLM completion is not connected in this MVP; response is the backend provider-less boundary text",
        ));
        let assistant_item = transcript.push(
            "assistant",
            providerless_response_text(),
            "backend_providerless_boundary",
            "complete",
        );
        response_from_locked_transcript(
            &transcript,
            CompanionState::Accepted,
            Some(worker),
            Some(user_item),
            Some(assistant_item),
            0,
            200,
            diagnostics,
        )
    }

    pub fn cancel(&self, _request: CompanionCancelRequest) -> CompanionMessageResponse {
        let diagnostics = vec![diagnostic(
            "companion_cancel_no_active_run",
            DiagnosticSeverity::Info,
            "Provider-less Companion Console has no active generation to cancel",
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

    fn current_worker(
        &self,
    ) -> Result<(Option<WorkerSummary>, Vec<RuntimeDiagnostic>), RuntimeDiagnostic> {
        let worker = self.worker.lock().map_err(|_| {
            diagnostic(
                "companion_state_unavailable",
                DiagnosticSeverity::Error,
                "Companion state is unavailable",
            )
        })?;
        Ok((worker.worker.clone(), worker.diagnostics.clone()))
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

    fn busy_message_response(&self) -> CompanionMessageResponse {
        let diagnostic = diagnostic(
            "companion_busy",
            DiagnosticSeverity::Warning,
            "Companion Console is already processing a message",
        );
        match self.transcript.lock() {
            Ok(transcript) => response_from_locked_transcript(
                &transcript,
                CompanionState::Busy,
                self.status().worker,
                None,
                None,
                0,
                200,
                vec![diagnostic],
            ),
            Err(_) => CompanionMessageResponse {
                state: CompanionState::Busy,
                worker: self.status().worker,
                user_item: None,
                assistant_item: None,
                transcript: CompanionTranscriptProjection {
                    state: CompanionState::Busy,
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

    fn error_message_response(&self, diagnostic: RuntimeDiagnostic) -> CompanionMessageResponse {
        CompanionMessageResponse {
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
                diagnostics: vec![diagnostic.clone()],
            },
            diagnostics: vec![diagnostic],
        }
    }
}

impl CompanionTranscript {
    fn push(
        &mut self,
        role: impl Into<String>,
        content: impl Into<String>,
        source: impl Into<String>,
        status: impl Into<String>,
    ) -> CompanionTranscriptItem {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let item = CompanionTranscriptItem {
            sequence: self.next_sequence,
            role: role.into(),
            content: content.into(),
            created_at: Utc::now().to_rfc3339(),
            source: source.into(),
            status: status.into(),
        };
        self.items.push(item.clone());
        item
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

fn providerless_response_text() -> String {
    PROVIDERLESS_RESPONSE.trim().to_string()
}

fn providerless_transport() -> CompanionTransportSummary {
    CompanionTransportSummary {
        kind: "providerless_backend_internal".to_string(),
        completion: "synchronous_request_response".to_string(),
        limitation: "No provider-backed LLM generation is wired in this MVP; browser messages are recorded by a backend-internal tools-less Companion Worker and receive a resource-defined boundary response.".to_string(),
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
    fn companion_spawns_visible_worker_and_records_providerless_turn() {
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
        assert_eq!(response.state, CompanionState::Accepted);
        assert_eq!(response.transcript.items.len(), 2);
        assert_eq!(response.transcript.items[0].role, "user");
        assert_eq!(response.transcript.items[1].role, "assistant");
        assert!(
            response.transcript.items[1]
                .content
                .contains("provider-less")
        );

        let runtime_transcript = registry
            .transcript(COMPANION_RUNTIME_ID, &worker.worker_id, 0, 10)
            .unwrap();
        assert_eq!(runtime_transcript.items.len(), 1);
        assert_eq!(runtime_transcript.items[0].role, "user");

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
