//! Read-only notification evidence. Acceptance is not model processing or execution authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDiagnosticsQuery {
    pub ticket: Option<String>,
    pub merge_request_id: Option<String>,
    pub job_id: Option<String>,
    pub source_event_id: Option<String>,
    pub assignment_id: Option<String>,
    pub delivery_id: Option<String>,
    /// Exact source identity for a single-record lookup.
    pub source_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDiagnosticStage {
    Historical,
    AwaitingRouting,
    Suppressed,
    Deferred,
    Pending,
    Reserved,
    RequestPersisted,
    RuntimeRejected,
    AcceptanceUnknown,
    DeliveryFailed,
    RuntimeAccepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDiagnostic {
    pub source_id: String,
    pub source_event_id: String,
    pub event_kind: String,
    pub ticket: Option<String>,
    pub merge_request_id: Option<String>,
    pub job_id: Option<String>,
    pub recipient_role: String,
    pub assignment_id: Option<String>,
    /// Current assignment identity only; null for fixed-worker or absent recipients.
    pub recipient_current: Option<bool>,
    pub disposition: Option<String>,
    pub reason: Option<String>,
    pub purpose: Option<String>,
    pub delivery_id: Option<String>,
    pub stage: NotificationDiagnosticStage,
    pub target_runtime_id: Option<String>,
    pub target_worker_id: Option<String>,
    pub last_failure_code: Option<String>,
    pub last_failure_at: Option<String>,
    /// Saved delivery candidate, NOT permission to send. Live recipient and binding checks remain required.
    pub retry_candidate: bool,
    /// No model-processing evidence is currently collected by this read contract.
    pub model_processing_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSourceCheck {
    pub resource_kind: String,
    pub reference: String,
    /// Whether an exact durable origin is present; null means that reference form was not verified.
    pub origin_present: Option<bool>,
    /// Whether the origin has any captured source (independent of pagination/recipient filters).
    /// A present origin with false here is unconnected, not intentionally suppressed.
    pub captured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDiagnosticsPage {
    pub items: Vec<NotificationDiagnostic>,
    pub next_cursor: Option<String>,
    pub source_checks: Vec<NotificationSourceCheck>,
}
