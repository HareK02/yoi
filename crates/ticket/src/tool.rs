//! LLM tool implementations for typed Ticket backend operations.
//!
//! These tools are intentionally owned by the `ticket` crate so Pod features can
//! install Ticket behavior without reimplementing domain/backend logic or
//! granting generic filesystem write authority.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ExtensibleTicketStatus, LocalTicketBackend, MarkdownText, NewTicket, NewTicketEvent, Ticket,
    TicketBackend, TicketDoctorDiagnostic, TicketDoctorReport, TicketDoctorSeverity, TicketError,
    TicketEventKind, TicketIdOrSlug, TicketRef, TicketReview, TicketReviewResult, TicketStatus,
    TicketSummary,
};

const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 200;
const DEFAULT_EVENT_LIMIT: usize = 20;
const MAX_EVENT_LIMIT: usize = 100;
const DEFAULT_ARTIFACT_LIMIT: usize = 50;
const MAX_ARTIFACT_LIMIT: usize = 200;
const DEFAULT_BODY_MAX_BYTES: usize = 16 * 1024;
const MAX_BODY_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_DIAGNOSTIC_LIMIT: usize = 100;
const MAX_DIAGNOSTIC_LIMIT: usize = 500;

pub const TICKET_TOOL_NAMES: [&str; 8] = [
    "TicketCreate",
    "TicketList",
    "TicketShow",
    "TicketComment",
    "TicketReview",
    "TicketStatus",
    "TicketClose",
    "TicketDoctor",
];

const CREATE_DESCRIPTION: &str = "Create a Ticket through the configured typed Ticket backend. \
Inputs mirror the Ticket `item.md` fields; `title` is required, `body` is Markdown, and the \
backend assigns the id and writes the local Ticket file layout under the configured backend root.";
const LIST_DESCRIPTION: &str = "List Tickets from the configured typed Ticket backend. Filter by \
status (`open`, `pending`, `closed`, or `all`) and optionally kind/priority/label. Output is a \
bounded JSON summary list, not full ticket bodies.";
const SHOW_DESCRIPTION: &str = "Show one Ticket by id, slug, or exact query through the configured \
typed Ticket backend. Output includes bounded Markdown body, recent thread events, resolution, and \
artifact metadata.";
const COMMENT_DESCRIPTION: &str = "Append a typed Ticket thread event. `role` must be `comment`, \
`plan`, `decision`, or `implementation_report`; `body` is Markdown. Writes stay inside the \
configured Ticket backend root.";
const REVIEW_DESCRIPTION: &str = "Append a Ticket review event. `result` must be `approve` or \
`request_changes`; `body` is Markdown. Writes stay inside the configured Ticket backend root.";
const STATUS_DESCRIPTION: &str = "Move a Ticket between non-closed local statuses through the typed \
Ticket backend. Use `TicketClose` for closing because closed Tickets require a resolution accepted \
by `yoi ticket doctor`.";
const CLOSE_DESCRIPTION: &str = "Close a Ticket with a Markdown resolution through the typed Ticket \
backend. The backend moves the Ticket to closed/, writes resolution.md, updates item.md, and appends \
a close event.";
const DOCTOR_DESCRIPTION: &str = "Run typed Ticket backend consistency checks and return bounded \
diagnostics through the typed backend without shelling out to external commands.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCreateParams {
    /// Ticket title. Must not be empty.
    title: String,
    /// Optional slug seed. The local backend slugifies this value.
    #[serde(default)]
    slug: Option<String>,
    /// Ticket kind. Defaults to `task`.
    #[serde(default)]
    kind: Option<String>,
    /// Ticket priority. Defaults to `P2`.
    #[serde(default)]
    priority: Option<String>,
    /// Ticket labels.
    #[serde(default)]
    labels: Vec<String>,
    /// Markdown body for item.md. If omitted, a small default body is used.
    #[serde(default)]
    body: Option<String>,
    /// Optional thread author for the create event.
    #[serde(default)]
    author: Option<String>,
    /// Optional assignee frontmatter value.
    #[serde(default)]
    assignee: Option<String>,
    /// Optional legacy ticket reference frontmatter value.
    #[serde(default)]
    legacy_ticket: Option<String>,
    /// Optional readiness frontmatter value.
    #[serde(default)]
    readiness: Option<String>,
    /// Optional preflight flag frontmatter value.
    #[serde(default)]
    needs_preflight: Option<bool>,
    /// Optional risk flag frontmatter values.
    #[serde(default)]
    risk_flags: Vec<String>,
    /// Optional action-required frontmatter value.
    #[serde(default)]
    action_required: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketListStatusParam {
    Open,
    Pending,
    Closed,
    All,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketListParams {
    /// Status filter. Defaults to `open`; use `all` to include closed and pending Tickets.
    #[serde(default)]
    status: Option<TicketListStatusParam>,
    /// Maximum number of summaries to return. Defaults to 100, max 200.
    #[serde(default)]
    limit: Option<usize>,
    /// Optional exact kind filter.
    #[serde(default)]
    kind: Option<String>,
    /// Optional exact priority filter.
    #[serde(default)]
    priority: Option<String>,
    /// Optional label that must be present.
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketShowParams {
    /// Ticket id. Exactly one of `id`, `slug`, or `query` must be provided.
    #[serde(default)]
    id: Option<String>,
    /// Ticket slug. Exactly one of `id`, `slug`, or `query` must be provided.
    #[serde(default)]
    slug: Option<String>,
    /// Exact id-or-slug query. Exactly one of `id`, `slug`, or `query` must be provided.
    #[serde(default)]
    query: Option<String>,
    /// Maximum number of most-recent thread events to return. Defaults to 20, max 100.
    #[serde(default)]
    event_limit: Option<usize>,
    /// Maximum number of artifact metadata entries to return. Defaults to 50, max 200.
    #[serde(default)]
    artifact_limit: Option<usize>,
    /// Maximum bytes for each Markdown body field before adding a truncation marker. Defaults to 16 KiB, max 64 KiB.
    #[serde(default)]
    body_max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketCommentRoleParam {
    Comment,
    Plan,
    Decision,
    ImplementationReport,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCommentParams {
    /// Ticket id or slug.
    ticket: String,
    /// Thread event role: `comment`, `plan`, `decision`, or `implementation_report`.
    role: TicketCommentRoleParam,
    /// Markdown event body.
    body: String,
    /// Optional thread author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketReviewResultParam {
    Approve,
    RequestChanges,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketReviewParams {
    /// Ticket id or slug.
    ticket: String,
    /// Review result: `approve` or `request_changes`.
    result: TicketReviewResultParam,
    /// Markdown review body.
    body: String,
    /// Optional thread author.
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TicketStatusParam {
    Open,
    Pending,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketStatusParams {
    /// Ticket id or slug.
    ticket: String,
    /// New status. Use `TicketClose` for `closed`.
    status: TicketStatusParam,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketCloseParams {
    /// Ticket id or slug.
    ticket: String,
    /// Markdown resolution written to resolution.md and thread.md.
    resolution: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TicketDoctorParams {
    /// Maximum diagnostics to return. Defaults to 100, max 500.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TicketRefOutput {
    id: String,
    slug: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct TicketListOutput {
    status_filter: String,
    count: usize,
    returned: usize,
    truncated: bool,
    tickets: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct TicketDoctorOutput {
    ok: bool,
    error_count: usize,
    diagnostic_count: usize,
    returned: usize,
    truncated: bool,
    diagnostics: Vec<Value>,
}

#[derive(Clone)]
struct TicketCreateTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketListTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketShowTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketCommentTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketReviewTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketStatusTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketCloseTool {
    backend: LocalTicketBackend,
}

#[derive(Clone)]
struct TicketDoctorTool {
    backend: LocalTicketBackend,
}

#[async_trait]
impl Tool for TicketCreateTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketCreateParams = parse_input("TicketCreate", input_json)?;
        let mut input = NewTicket::new(params.title);
        input.slug = params.slug;
        if let Some(kind) = params.kind {
            input.kind = kind;
        }
        if let Some(priority) = params.priority {
            input.priority = priority;
        }
        input.labels = params.labels;
        if let Some(body) = params.body {
            input.body = MarkdownText::new(body);
        }
        input.author = params.author;
        input.assignee = params.assignee;
        input.legacy_ticket = params.legacy_ticket;
        input.readiness = params.readiness;
        input.needs_preflight = params.needs_preflight;
        input.risk_flags = params.risk_flags;
        input.action_required = params.action_required;

        let created = self
            .backend
            .create(input)
            .map_err(|error| backend_error("TicketCreate", error))?;
        Ok(json_output(
            format!(
                "Created ticket {} ({}) status {}",
                created.id,
                created.slug,
                created.status.as_str()
            ),
            ticket_ref_output(created),
        ))
    }
}

#[async_trait]
impl Tool for TicketListTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketListParams = parse_input("TicketList", input_json)?;
        let status = params.status.unwrap_or(TicketListStatusParam::Open);
        let filter = match status {
            TicketListStatusParam::Open => crate::TicketFilter::status(TicketStatus::Open),
            TicketListStatusParam::Pending => crate::TicketFilter::status(TicketStatus::Pending),
            TicketListStatusParam::Closed => crate::TicketFilter::status(TicketStatus::Closed),
            TicketListStatusParam::All => crate::TicketFilter::all(),
        };
        let status_filter = match status {
            TicketListStatusParam::Open => "open",
            TicketListStatusParam::Pending => "pending",
            TicketListStatusParam::Closed => "closed",
            TicketListStatusParam::All => "all",
        };
        let limit = bounded(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);
        let mut tickets = self
            .backend
            .list(filter)
            .map_err(|error| backend_error("TicketList", error))?;
        tickets.retain(|ticket| {
            params.kind.as_ref().is_none_or(|kind| ticket.kind == *kind)
                && params
                    .priority
                    .as_ref()
                    .is_none_or(|priority| ticket.priority == *priority)
                && params
                    .label
                    .as_ref()
                    .is_none_or(|label| ticket.labels.iter().any(|item| item == label))
        });
        let count = tickets.len();
        let returned_tickets: Vec<_> = tickets
            .into_iter()
            .take(limit)
            .map(ticket_summary_json)
            .collect();
        let output = TicketListOutput {
            status_filter: status_filter.to_string(),
            count,
            returned: returned_tickets.len(),
            truncated: count > returned_tickets.len(),
            tickets: returned_tickets,
        };
        Ok(json_output(
            format!(
                "Listed {} ticket(s) for status {status_filter}{}",
                output.returned,
                if output.truncated { " (truncated)" } else { "" }
            ),
            output,
        ))
    }
}

#[async_trait]
impl Tool for TicketShowTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketShowParams = parse_input("TicketShow", input_json)?;
        let query = id_or_slug(params.id, params.slug, params.query)?;
        let event_limit = bounded(params.event_limit, DEFAULT_EVENT_LIMIT, MAX_EVENT_LIMIT);
        let artifact_limit = bounded(
            params.artifact_limit,
            DEFAULT_ARTIFACT_LIMIT,
            MAX_ARTIFACT_LIMIT,
        );
        let body_max_bytes = bounded(
            params.body_max_bytes,
            DEFAULT_BODY_MAX_BYTES,
            MAX_BODY_MAX_BYTES,
        );
        let ticket = self
            .backend
            .show(query)
            .map_err(|error| backend_error("TicketShow", error))?;
        let summary = format!(
            "Ticket {} ({}) status {}",
            ticket.meta.id,
            ticket.meta.slug,
            status_as_str(&ticket.meta.status)
        );
        Ok(json_output(
            summary,
            ticket_json(&ticket, event_limit, artifact_limit, body_max_bytes),
        ))
    }
}

#[async_trait]
impl Tool for TicketCommentTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketCommentParams = parse_input("TicketComment", input_json)?;
        let kind = match params.role {
            TicketCommentRoleParam::Comment => TicketEventKind::Comment,
            TicketCommentRoleParam::Plan => TicketEventKind::Plan,
            TicketCommentRoleParam::Decision => TicketEventKind::Decision,
            TicketCommentRoleParam::ImplementationReport => TicketEventKind::ImplementationReport,
        };
        let role = kind.as_str().to_string();
        let mut event = NewTicketEvent::new(kind, params.body);
        event.author = params.author;
        self.backend
            .add_event(TicketIdOrSlug::Query(params.ticket.clone()), event)
            .map_err(|error| backend_error("TicketComment", error))?;
        Ok(json_output(
            format!("Appended {role} event to ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "event": role, "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketReviewTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketReviewParams = parse_input("TicketReview", input_json)?;
        let result = match params.result {
            TicketReviewResultParam::Approve => TicketReviewResult::Approve,
            TicketReviewResultParam::RequestChanges => TicketReviewResult::RequestChanges,
        };
        let result_str = result.as_str().to_string();
        let review = TicketReview {
            result,
            author: params.author,
            body: MarkdownText::new(params.body),
        };
        self.backend
            .review(TicketIdOrSlug::Query(params.ticket.clone()), review)
            .map_err(|error| backend_error("TicketReview", error))?;
        Ok(json_output(
            format!("Appended {result_str} review to ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "review": result_str, "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketStatusTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketStatusParams = parse_input("TicketStatus", input_json)?;
        let status = match params.status {
            TicketStatusParam::Open => TicketStatus::Open,
            TicketStatusParam::Pending => TicketStatus::Pending,
        };
        self.backend
            .set_status(TicketIdOrSlug::Query(params.ticket.clone()), status)
            .map_err(|error| backend_error("TicketStatus", error))?;
        Ok(json_output(
            format!("Moved ticket {} to {}", params.ticket, status.as_str()),
            json!({ "ticket": params.ticket, "status": status.as_str(), "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketCloseTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketCloseParams = parse_input("TicketClose", input_json)?;
        self.backend
            .close(
                TicketIdOrSlug::Query(params.ticket.clone()),
                MarkdownText::new(params.resolution),
            )
            .map_err(|error| backend_error("TicketClose", error))?;
        Ok(json_output(
            format!("Closed ticket {}", params.ticket),
            json!({ "ticket": params.ticket, "status": "closed", "ok": true }),
        ))
    }
}

#[async_trait]
impl Tool for TicketDoctorTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: TicketDoctorParams = parse_input("TicketDoctor", input_json)?;
        let limit = bounded(params.limit, DEFAULT_DIAGNOSTIC_LIMIT, MAX_DIAGNOSTIC_LIMIT);
        let report = self
            .backend
            .doctor()
            .map_err(|error| backend_error("TicketDoctor", error))?;
        let output = doctor_output(report, limit);
        Ok(json_output(
            format!(
                "Ticket doctor: {} error(s), {} diagnostic(s){}",
                output.error_count,
                output.diagnostic_count,
                if output.truncated { " (truncated)" } else { "" }
            ),
            output,
        ))
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(tool: &str, input_json: &str) -> Result<T, ToolError> {
    serde_json::from_str(input_json)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool} input: {error}")))
}

fn backend_error(tool: &str, error: TicketError) -> ToolError {
    ToolError::ExecutionFailed(format!("{tool} failed: {error}"))
}

fn bounded(value: Option<usize>, default: usize, max: usize) -> usize {
    value.unwrap_or(default).clamp(1, max)
}

fn id_or_slug(
    id: Option<String>,
    slug: Option<String>,
    query: Option<String>,
) -> Result<TicketIdOrSlug, ToolError> {
    let provided = id.iter().chain(slug.iter()).chain(query.iter()).count();
    if provided != 1 {
        return Err(ToolError::InvalidArgument(
            "exactly one of id, slug, or query must be provided".to_string(),
        ));
    }
    if let Some(id) = id {
        Ok(TicketIdOrSlug::Id(id))
    } else if let Some(slug) = slug {
        Ok(TicketIdOrSlug::Slug(slug))
    } else {
        Ok(TicketIdOrSlug::Query(
            query.expect("provided count checked"),
        ))
    }
}

fn status_as_str(status: &ExtensibleTicketStatus) -> &str {
    status.as_str()
}

fn ticket_ref_output(ticket: TicketRef) -> TicketRefOutput {
    TicketRefOutput {
        id: ticket.id,
        slug: ticket.slug,
        status: ticket.status.as_str().to_string(),
    }
}

fn ticket_summary_json(ticket: TicketSummary) -> Value {
    json!({
        "id": ticket.id,
        "slug": ticket.slug,
        "title": ticket.title,
        "status": status_as_str(&ticket.status),
        "kind": ticket.kind,
        "priority": ticket.priority,
        "labels": ticket.labels,
        "readiness": ticket.readiness,
        "needs_preflight": ticket.needs_preflight,
        "action_required": ticket.action_required,
        "updated_at": ticket.updated_at,
    })
}

fn ticket_json(
    ticket: &Ticket,
    event_limit: usize,
    artifact_limit: usize,
    body_max_bytes: usize,
) -> Value {
    let event_count = ticket.events.len();
    let events: Vec<_> = ticket
        .events
        .iter()
        .skip(event_count.saturating_sub(event_limit))
        .map(|event| {
            json!({
                "kind": event.kind.as_str(),
                "author": event.author,
                "at": event.at,
                "status": event.status,
                "from": event.from,
                "to": event.to,
                "reason": event.reason,
                "state_field": event.state_field,
                "attributes": event.attributes,
                "heading": event.heading,
                "body": truncate_text(event.body.as_str(), body_max_bytes),
            })
        })
        .collect();
    let artifact_count = ticket.artifacts.len();
    let artifacts: Vec<_> = ticket
        .artifacts
        .iter()
        .take(artifact_limit)
        .map(|artifact| artifact.relative_path.display().to_string())
        .collect();
    json!({
        "meta": {
            "id": ticket.meta.id,
            "slug": ticket.meta.slug,
            "title": ticket.meta.title,
            "status": status_as_str(&ticket.meta.status),
            "kind": ticket.meta.kind,
            "priority": ticket.meta.priority,
            "labels": ticket.meta.labels,
            "created_at": ticket.meta.created_at,
            "updated_at": ticket.meta.updated_at,
            "assignee": ticket.meta.assignee,
            "legacy_ticket": ticket.meta.legacy_ticket,
            "readiness": ticket.meta.readiness,
            "needs_preflight": ticket.meta.needs_preflight,
            "risk_flags": ticket.meta.risk_flags,
            "action_required": ticket.meta.action_required,
        },
        "body": truncate_text(ticket.document.body.as_str(), body_max_bytes),
        "events": {
            "count": event_count,
            "returned": events.len(),
            "truncated": event_count > events.len(),
            "items": events,
        },
        "artifacts": {
            "count": artifact_count,
            "returned": artifacts.len(),
            "truncated": artifact_count > artifacts.len(),
            "items": artifacts,
        },
        "resolution": ticket.resolution.as_ref().map(|resolution| truncate_text(resolution.as_str(), body_max_bytes)),
    })
}

fn doctor_output(report: TicketDoctorReport, limit: usize) -> TicketDoctorOutput {
    let diagnostic_count = report.diagnostics.len();
    let error_count = report.error_count();
    let diagnostics = report
        .diagnostics
        .into_iter()
        .take(limit)
        .map(diagnostic_json)
        .collect::<Vec<_>>();
    TicketDoctorOutput {
        ok: error_count == 0,
        error_count,
        diagnostic_count,
        returned: diagnostics.len(),
        truncated: diagnostic_count > diagnostics.len(),
        diagnostics,
    }
}

fn diagnostic_json(diagnostic: TicketDoctorDiagnostic) -> Value {
    let severity = match diagnostic.severity {
        TicketDoctorSeverity::Error => "error",
        TicketDoctorSeverity::Warning => "warning",
    };
    json!({
        "severity": severity,
        "message": diagnostic.message,
        "path": diagnostic.path.map(|path| path.display().to_string()),
    })
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let marker = format!("\n\n[truncated: {} bytes dropped]", text.len() - max_bytes);
    let mut cut = max_bytes.saturating_sub(marker.len());
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = text[..cut].to_string();
    out.push_str(&marker);
    out
}

fn json_output(summary: String, value: impl Serialize) -> ToolOutput {
    ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())),
    }
}

fn tool_definition<T>(
    name: &'static str,
    description: &'static str,
    backend: LocalTicketBackend,
) -> ToolDefinition
where
    T: Tool + From<LocalTicketBackend> + 'static,
{
    Arc::new(move || {
        let schema_value = input_schema(name);
        let meta = ToolMeta::new(name)
            .description(description)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(T::from(backend.clone()));
        (meta, tool)
    })
}

fn input_schema(name: &str) -> Value {
    match name {
        "TicketCreate" => serde_json::to_value(schemars::schema_for!(TicketCreateParams)),
        "TicketList" => serde_json::to_value(schemars::schema_for!(TicketListParams)),
        "TicketShow" => serde_json::to_value(schemars::schema_for!(TicketShowParams)),
        "TicketComment" => serde_json::to_value(schemars::schema_for!(TicketCommentParams)),
        "TicketReview" => serde_json::to_value(schemars::schema_for!(TicketReviewParams)),
        "TicketStatus" => serde_json::to_value(schemars::schema_for!(TicketStatusParams)),
        "TicketClose" => serde_json::to_value(schemars::schema_for!(TicketCloseParams)),
        "TicketDoctor" => serde_json::to_value(schemars::schema_for!(TicketDoctorParams)),
        _ => Ok(json!({})),
    }
    .unwrap_or_else(|_| json!({}))
}

macro_rules! impl_from_backend {
    ($tool:ident) => {
        impl From<LocalTicketBackend> for $tool {
            fn from(backend: LocalTicketBackend) -> Self {
                Self { backend }
            }
        }
    };
}

impl_from_backend!(TicketCreateTool);
impl_from_backend!(TicketListTool);
impl_from_backend!(TicketShowTool);
impl_from_backend!(TicketCommentTool);
impl_from_backend!(TicketReviewTool);
impl_from_backend!(TicketStatusTool);
impl_from_backend!(TicketCloseTool);
impl_from_backend!(TicketDoctorTool);

/// Build all MVP Ticket tool definitions over one local backend root.
pub fn ticket_tools(backend: LocalTicketBackend) -> Vec<ToolDefinition> {
    vec![
        tool_definition::<TicketCreateTool>("TicketCreate", CREATE_DESCRIPTION, backend.clone()),
        tool_definition::<TicketListTool>("TicketList", LIST_DESCRIPTION, backend.clone()),
        tool_definition::<TicketShowTool>("TicketShow", SHOW_DESCRIPTION, backend.clone()),
        tool_definition::<TicketCommentTool>("TicketComment", COMMENT_DESCRIPTION, backend.clone()),
        tool_definition::<TicketReviewTool>("TicketReview", REVIEW_DESCRIPTION, backend.clone()),
        tool_definition::<TicketStatusTool>("TicketStatus", STATUS_DESCRIPTION, backend.clone()),
        tool_definition::<TicketCloseTool>("TicketClose", CLOSE_DESCRIPTION, backend.clone()),
        tool_definition::<TicketDoctorTool>("TicketDoctor", DOCTOR_DESCRIPTION, backend),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn backend(temp: &TempDir) -> LocalTicketBackend {
        LocalTicketBackend::new(temp.path().join("tickets"))
    }

    fn tool(definition: ToolDefinition) -> Arc<dyn Tool> {
        let (_, tool) = definition();
        tool
    }

    fn tool_by_name(backend: LocalTicketBackend, name: &str) -> Arc<dyn Tool> {
        ticket_tools(backend)
            .into_iter()
            .find_map(|definition| {
                let (meta, tool) = definition();
                (meta.name == name).then_some(tool)
            })
            .expect("tool exists")
    }

    #[tokio::test]
    async fn ticket_tools_create_list_show_and_doctor() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let create = tool_by_name(backend.clone(), "TicketCreate");
        let list = tool_by_name(backend.clone(), "TicketList");
        let show = tool_by_name(backend.clone(), "TicketShow");
        let doctor = tool_by_name(backend.clone(), "TicketDoctor");

        let created = create
            .execute(
                &json!({
                    "title": "Tool Created",
                    "slug": "tool-created",
                    "labels": ["ticket", "tool"],
                    "body": "## Background\n\nCreated by tool.\n"
                })
                .to_string(),
            )
            .await
            .unwrap();
        assert!(created.summary.contains("Created ticket"));
        let created_json: Value = serde_json::from_str(&created.content.unwrap()).unwrap();
        let id = created_json["id"].as_str().unwrap().to_string();

        let listed = list
            .execute(&json!({ "status": "open", "label": "tool" }).to_string())
            .await
            .unwrap();
        assert!(listed.summary.contains("Listed 1 ticket"));
        assert!(listed.content.unwrap().contains("Tool Created"));

        let shown = show
            .execute(&json!({ "id": id, "event_limit": 10 }).to_string())
            .await
            .unwrap();
        assert!(shown.summary.contains("tool-created"));
        assert!(shown.content.unwrap().contains("Created by tool"));

        let report = doctor.execute(&json!({}).to_string()).await.unwrap();
        assert!(report.summary.contains("0 error(s)"));
    }

    #[tokio::test]
    async fn ticket_tools_comment_review_status_and_close_are_doctor_clean() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let created = backend.create(NewTicket::new("Flow Tool")).unwrap();
        let comment = tool_by_name(backend.clone(), "TicketComment");
        let review = tool_by_name(backend.clone(), "TicketReview");
        let status = tool_by_name(backend.clone(), "TicketStatus");
        let close = tool_by_name(backend.clone(), "TicketClose");
        let doctor = tool_by_name(backend.clone(), "TicketDoctor");

        comment
            .execute(
                &json!({
                    "ticket": created.slug,
                    "role": "implementation_report",
                    "body": "Implemented."
                })
                .to_string(),
            )
            .await
            .unwrap();
        review
            .execute(
                &json!({
                    "ticket": created.id,
                    "result": "approve",
                    "body": "Looks good."
                })
                .to_string(),
            )
            .await
            .unwrap();
        status
            .execute(&json!({ "ticket": created.slug, "status": "pending" }).to_string())
            .await
            .unwrap();
        close
            .execute(
                &json!({ "ticket": created.id, "resolution": "Done via TicketClose.\n" })
                    .to_string(),
            )
            .await
            .unwrap();

        let report = doctor.execute(&json!({}).to_string()).await.unwrap();
        assert!(report.summary.contains("0 error(s)"));
        let closed = backend.show(TicketIdOrSlug::Query(created.slug)).unwrap();
        assert!(closed.resolution.is_some());
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::ImplementationReport)
        );
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::Review)
        );
        assert!(
            closed
                .events
                .iter()
                .any(|event| event.kind == TicketEventKind::StatusChanged)
        );
    }

    #[tokio::test]
    async fn ticket_show_requires_exactly_one_identifier() {
        let temp = TempDir::new().unwrap();
        let show = tool_by_name(backend(&temp), "TicketShow");
        let error = show
            .execute(&json!({ "id": "a", "slug": "b" }).to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn ticket_create_slug_path_traversal_is_sanitized_under_backend_root() {
        let temp = TempDir::new().unwrap();
        let backend = backend(&temp);
        let create = tool_by_name(backend.clone(), "TicketCreate");
        create
            .execute(&json!({ "title": "Escape", "slug": "../escape" }).to_string())
            .await
            .unwrap();
        assert!(!temp.path().join("escape").exists());
        assert_eq!(backend.list(crate::TicketFilter::all()).unwrap().len(), 1);
    }

    #[test]
    fn ticket_tool_definitions_have_expected_names_and_schemas() {
        let temp = TempDir::new().unwrap();
        let names = ticket_tools(backend(&temp))
            .into_iter()
            .map(|definition| definition().0)
            .map(|meta| {
                assert_eq!(meta.input_schema["type"], "object");
                meta.name
            })
            .collect::<Vec<_>>();
        assert_eq!(names, TICKET_TOOL_NAMES);
    }

    #[test]
    fn individual_tool_definition_factory_is_callable() {
        let temp = TempDir::new().unwrap();
        let create = tool(tool_definition::<TicketCreateTool>(
            "TicketCreate",
            CREATE_DESCRIPTION,
            backend(&temp),
        ));
        let _ = create;
    }
}
