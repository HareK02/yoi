use workspace_api::{
    CompanionLifecycleState, CompanionMessageDisposition, CompanionTransportSummary, Diagnostic,
    DiagnosticSeverity,
};

pub use workspace_api::{
    CompanionCancelRequest, CompanionMessageRequest, CompanionMessageResponse,
    CompanionStatusResponse, CompanionTranscriptProjection,
};

#[derive(Clone, Default)]
pub struct CompanionConsole;

impl CompanionConsole {
    pub fn disabled() -> Self {
        Self
    }

    pub fn status(&self) -> CompanionStatusResponse {
        CompanionStatusResponse {
            state: CompanionLifecycleState::Stopped,
            worker: None,
            transport: CompanionTransportSummary {
                mode: "disabled".to_string(),
                available: false,
            },
            diagnostics: vec![disabled_diagnostic()],
        }
    }

    pub fn transcript(&self, start: usize, limit: usize) -> CompanionTranscriptProjection {
        CompanionTranscriptProjection {
            state: CompanionLifecycleState::Stopped,
            start,
            limit,
            total: 0,
            next: None,
            items: Vec::new(),
        }
    }

    pub fn send_message(&self, _request: CompanionMessageRequest) -> CompanionMessageResponse {
        disabled_message_response()
    }

    pub fn cancel(&self, _request: CompanionCancelRequest) -> CompanionMessageResponse {
        disabled_message_response()
    }
}

fn disabled_message_response() -> CompanionMessageResponse {
    CompanionMessageResponse {
        state: CompanionMessageDisposition::Rejected,
        message: "Workspace Companion auto-start is disabled; create or select an explicit Worker instead."
            .to_string(),
    }
}

fn disabled_diagnostic() -> Diagnostic {
    Diagnostic {
        code: "companion_disabled".to_string(),
        severity: DiagnosticSeverity::Info,
        message:
            "Workspace Companion auto-start was removed; use the explicit Worker lifecycle instead."
                .to_string(),
    }
}
