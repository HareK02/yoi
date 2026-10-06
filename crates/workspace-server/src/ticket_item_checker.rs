//! Post-save Ticket item checks executed through the Backend-owned Job runner.
//!
//! The immutable input contains only the persisted Ticket item and the edit that
//! produced it. Conversation history is deliberately outside this boundary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use worker_runtime::identity::RuntimeWorkerRef;

use crate::backend_job::{BackendJobLimits, BackendJobRequest, BackendJobResultAcceptance};
use crate::{Error, Result};

pub const PURPOSE: &str = "ticket_item_check";
const MAX_FINDINGS: usize = 8;
const MAX_QUOTE_BYTES: usize = 512;
const MAX_REASON_BYTES: usize = 512;
const MAX_SUGGESTION_BYTES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TicketItemCheckInput {
    /// Canonical internal identity used for an authoritative revision reread.
    pub ticket_id: String,
    /// User-facing identity used in checker output and advisory notifications.
    pub ticket_resource_key: String,
    pub revision: String,
    pub title: String,
    pub body: String,
    pub edit: TicketItemCheckEdit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TicketItemCheckEdit {
    Create,
    Edit {
        title_changed: bool,
        body_changed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_replacement: Option<TicketItemCheckBodyReplacement>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TicketItemCheckBodyReplacement {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TicketItemCheckResult {
    pub findings: Vec<TicketItemCheckFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct TicketItemCheckFinding {
    pub category: TicketItemCheckFindingCategory,
    pub quote: String,
    pub reason: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TicketItemCheckFindingCategory {
    WriterScope,
    InternalInconsistency,
    AmbiguousBoundary,
}

impl TicketItemCheckFindingCategory {
    fn label(self) -> &'static str {
        match self {
            Self::WriterScope => "writer scope mixed into requirements",
            Self::InternalInconsistency => "internal inconsistency",
            Self::AmbiguousBoundary => "ambiguous writer-scope/specification boundary",
        }
    }
}

pub fn item_revision(ticket: &ticket::Ticket) -> String {
    ticket
        .events
        .iter()
        .rev()
        .find(|event| matches!(event.kind.as_str(), "create" | "item_edit"))
        .and_then(|event| event.attributes.get("event_id").cloned())
        .or_else(|| ticket.meta.updated_at.clone())
        .unwrap_or_else(|| format!("{}:0", ticket.meta.id))
}

pub fn request(
    ticket: &ticket::Ticket,
    edit: TicketItemCheckEdit,
    source_worker: RuntimeWorkerRef,
    instruction: String,
) -> Result<BackendJobRequest> {
    let revision = item_revision(ticket);
    let resource_key = ticket
        .meta
        .resource_key
        .clone()
        .unwrap_or_else(|| ticket.meta.id.clone());
    let input = TicketItemCheckInput {
        ticket_id: ticket.meta.id.clone(),
        ticket_resource_key: resource_key.clone(),
        revision: revision.clone(),
        title: ticket.meta.title.clone(),
        body: ticket.document.body.as_str().to_string(),
        edit,
    };
    let request = BackendJobRequest {
        job_id: format!("ticket-item-check:{}:{revision}", ticket.meta.id),
        purpose: PURPOSE.to_string(),
        input_revision: revision.clone(),
        input_ref: format!("ticket://{resource_key}/revisions/{revision}"),
        input: serde_json::to_value(input).map_err(|error| {
            Error::InvalidInput(format!("serialize Ticket item checker input: {error}"))
        })?,
        instruction,
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: Some(source_worker.clone()),
        notification_target: Some(source_worker),
        limits: BackendJobLimits {
            max_concurrent_jobs: 4,
            timeout_seconds: 45,
            max_result_bytes: 8 * 1024,
            max_attempts: 1,
        },
    };
    request.validate()?;
    Ok(request)
}

pub fn parse_input(value: &Value) -> Result<TicketItemCheckInput> {
    let input: TicketItemCheckInput = serde_json::from_value(value.clone()).map_err(|error| {
        Error::InvalidInput(format!("invalid Ticket item checker input: {error}"))
    })?;
    if input.ticket_id.trim().is_empty()
        || input.ticket_resource_key.trim().is_empty()
        || input.revision.trim().is_empty()
    {
        return Err(Error::InvalidInput(
            "Ticket item checker input identity is incomplete".to_string(),
        ));
    }
    Ok(input)
}

pub fn validate_result(input: &Value, result: &Value) -> Result<TicketItemCheckResult> {
    let input = parse_input(input)?;
    let result: TicketItemCheckResult =
        serde_json::from_value(result.clone()).map_err(|error| {
            Error::InvalidInput(format!("invalid Ticket item checker result: {error}"))
        })?;
    if result.findings.len() > MAX_FINDINGS {
        return Err(Error::InvalidInput(format!(
            "Ticket item checker result exceeds {MAX_FINDINGS} findings"
        )));
    }
    let mut unique = BTreeSet::new();
    for finding in &result.findings {
        validate_finding_text("quote", &finding.quote, MAX_QUOTE_BYTES)?;
        validate_finding_text("reason", &finding.reason, MAX_REASON_BYTES)?;
        validate_finding_text("suggestion", &finding.suggestion, MAX_SUGGESTION_BYTES)?;
        if !input.title.contains(&finding.quote) && !input.body.contains(&finding.quote) {
            return Err(Error::InvalidInput(
                "Ticket item checker finding quote is not present in the inspected title or body"
                    .to_string(),
            ));
        }
        if !unique.insert((finding.category, finding.quote.as_str())) {
            return Err(Error::InvalidInput(
                "Ticket item checker result contains a duplicate finding".to_string(),
            ));
        }
    }
    Ok(result)
}

fn validate_finding_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(Error::InvalidInput(format!(
            "Ticket item checker finding {field} must be non-empty and trimmed"
        )));
    }
    if value.len() > max_bytes {
        return Err(Error::InvalidInput(format!(
            "Ticket item checker finding {field} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketItemCheckNotification {
    Deliver(String),
    NoFindings,
    Stale,
}

/// Decide whether a completed check should produce an advisory. Clean and stale
/// results remain distinct so delivery recovery can record why no input was sent.
pub fn notification(
    acceptance: &BackendJobResultAcceptance,
    current_ticket: &ticket::Ticket,
) -> Result<TicketItemCheckNotification> {
    let input = parse_input(&acceptance.job.request.input)?;
    let result = validate_result(
        &acceptance.job.request.input,
        acceptance.job.result.as_ref().ok_or_else(|| {
            Error::InvalidInput("completed Ticket item checker Job has no result".to_string())
        })?,
    )?;
    if input.revision != acceptance.job.request.input_revision
        || input.revision != item_revision(current_ticket)
        || input.ticket_id != current_ticket.meta.id
    {
        return Ok(TicketItemCheckNotification::Stale);
    }
    if result.findings.is_empty() {
        return Ok(TicketItemCheckNotification::NoFindings);
    }

    let mut content = format!(
        "[Ticket item checker advisory]\nTicket: {}\nRevision: {}\n\nThis is a post-save advisory based only on the inspected Ticket text. It is not a new user request, approval gate, workflow blocker, or implementation instruction.\n",
        input.ticket_resource_key, input.revision
    );
    for (index, finding) in result.findings.iter().enumerate() {
        let quoted = serde_json::to_string(&finding.quote).map_err(|error| {
            Error::InvalidInput(format!("encode Ticket checker quote: {error}"))
        })?;
        content.push_str(&format!(
            "\n{}. {}\n   Quote: {}\n   Reason: {}\n   Suggested correction or confirmation: {}\n",
            index + 1,
            finding.category.label(),
            quoted,
            finding.reason,
            finding.suggestion
        ));
    }
    Ok(TicketItemCheckNotification::Deliver(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        serde_json::json!({
            "ticket_id": "ticket-internal",
            "ticket_resource_key": "T-42",
            "revision": "revision-7",
            "title": "Deploy safely",
            "body": "This turn only creates the Ticket; I will not implement it. Production deployment requires approval.",
            "edit": { "kind": "create" }
        })
    }

    #[test]
    fn result_requires_bounded_findings_grounded_in_the_saved_item() {
        let valid = serde_json::json!({"findings": [{
            "category": "writer_scope",
            "quote": "I will not implement it",
            "reason": "The writer's current action boundary is phrased as a durable requirement.",
            "suggestion": "Remove the writer-specific sentence from the Ticket requirement."
        }]});
        assert_eq!(validate_result(&input(), &valid).unwrap().findings.len(), 1);

        let ungrounded = serde_json::json!({"findings": [{
            "category": "internal_inconsistency",
            "quote": "not in the item",
            "reason": "Invented external claim.",
            "suggestion": "Do something."
        }]});
        assert!(validate_result(&input(), &ungrounded).is_err());
        assert!(
            validate_result(
                &input(),
                &serde_json::json!({"findings": [], "extra": true})
            )
            .is_err()
        );
    }
}
