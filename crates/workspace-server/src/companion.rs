use serde::{Deserialize, Serialize};

use crate::hosts::{DiagnosticSeverity, RuntimeDiagnostic, WorkerSummary};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionState {
    Disabled,
    Rejected,
    Cancelled,
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

pub struct CompanionConsole;

impl CompanionConsole {
    pub fn disabled() -> Self {
        Self
    }

    pub fn status(&self) -> CompanionStatusResponse {
        CompanionStatusResponse {
            state: CompanionState::Disabled,
            worker: None,
            transport: disabled_transport(),
            diagnostics: vec![disabled_diagnostic()],
        }
    }

    pub fn transcript(&self, start: usize, limit: usize) -> CompanionTranscriptProjection {
        CompanionTranscriptProjection {
            state: CompanionState::Disabled,
            start,
            limit,
            total_items: 0,
            next_start: None,
            items: Vec::new(),
            diagnostics: vec![disabled_diagnostic()],
        }
    }

    pub fn send_message(&self, _request: CompanionMessageRequest) -> CompanionMessageResponse {
        disabled_message_response(CompanionState::Rejected)
    }

    pub fn cancel(&self, _request: CompanionCancelRequest) -> CompanionMessageResponse {
        disabled_message_response(CompanionState::Cancelled)
    }
}

fn disabled_message_response(state: CompanionState) -> CompanionMessageResponse {
    CompanionMessageResponse {
        state,
        worker: None,
        user_item: None,
        assistant_item: None,
        transcript: CompanionTranscriptProjection {
            state: CompanionState::Disabled,
            start: 0,
            limit: 200,
            total_items: 0,
            next_start: None,
            items: Vec::new(),
            diagnostics: vec![disabled_diagnostic()],
        },
        diagnostics: vec![disabled_diagnostic()],
    }
}

fn disabled_transport() -> CompanionTransportSummary {
    CompanionTransportSummary {
        kind: "none".to_string(),
        completion: "disabled".to_string(),
        limitation:
            "Workspace Companion auto-start has been removed; create an explicit Worker instead."
                .to_string(),
    }
}

fn disabled_diagnostic() -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: "companion_disabled".to_string(),
        severity: DiagnosticSeverity::Info,
        message: "Workspace Companion auto-start is disabled; create an explicit Worker instead."
            .to_string(),
    }
}
