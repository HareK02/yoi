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
    /// Canonical internal identity used for an authoritative content_digest reread.
    pub ticket_id: String,
    /// User-facing identity used in checker output and advisory notifications.
    pub ticket_resource_key: String,
    pub content_digest: String,
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

pub fn content_digest(ticket: &ticket::Ticket) -> String {
    ticket::ticket_content_digest(ticket)
}

pub fn request(
    ticket: &ticket::Ticket,
    edit: TicketItemCheckEdit,
    source_worker: RuntimeWorkerRef,
    instruction: String,
) -> Result<BackendJobRequest> {
    let content_digest = content_digest(ticket);
    let resource_key = ticket
        .meta
        .resource_key
        .clone()
        .unwrap_or_else(|| ticket.meta.id.clone());
    let input = TicketItemCheckInput {
        ticket_id: ticket.meta.id.clone(),
        ticket_resource_key: resource_key.clone(),
        content_digest: content_digest.clone(),
        title: ticket.meta.title.clone(),
        body: ticket.document.body.as_str().to_string(),
        edit,
    };
    let mut request = BackendJobRequest {
        job_id: format!("ticket-item-check:{}", ticket.meta.id),
        purpose: PURPOSE.to_string(),
        input_ref: format!("ticket://{resource_key}/contents/{content_digest}"),
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
    // The same item content can result from distinct edits or different source
    // Workers. Content identity fences advisory freshness, but does not identify
    // the complete Job intent. Hash the request with its stable Ticket prefix
    // before adding the digest so exact retries still reuse the same Job.
    request.job_id = format!("{}:{}", request.job_id, request.fingerprint()?);
    request.validate()?;
    Ok(request)
}

pub fn parse_input(value: &Value) -> Result<TicketItemCheckInput> {
    let input: TicketItemCheckInput = serde_json::from_value(value.clone()).map_err(|error| {
        Error::InvalidInput(format!("invalid Ticket item checker input: {error}"))
    })?;
    if input.ticket_id.trim().is_empty()
        || input.ticket_resource_key.trim().is_empty()
        || input.content_digest.trim().is_empty()
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
    if input.content_digest != content_digest(current_ticket)
        || input.ticket_id != current_ticket.meta.id
    {
        return Ok(TicketItemCheckNotification::Stale);
    }
    if result.findings.is_empty() {
        return Ok(TicketItemCheckNotification::NoFindings);
    }

    let mut content = format!(
        "[Ticket item checker advisory]\nTicket: {}\nContent digest: {}\n\nThis is a post-save advisory based only on the inspected Ticket text. It is not a new user request, approval gate, workflow blocker, or implementation instruction.\n",
        input.ticket_resource_key, input.content_digest
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
            "content_digest": "content_digest-7",
            "title": "Deploy safely",
            "body": "This turn only creates the Ticket; I will not implement it. Production deployment requires approval.",
            "edit": { "kind": "create" }
        })
    }

    #[test]
    fn checker_job_identity_reuses_exact_intent_but_separates_edits_and_sources() {
        use ticket::TicketBackend;

        let dir = tempfile::tempdir().unwrap();
        let backend =
            ticket::SqliteTicketBackend::open(dir.path().join("tickets.db"), "space").unwrap();
        let (_, ticket) = backend
            .create_with_snapshot(ticket::NewTicket::new("Check this content"))
            .unwrap();
        let source = RuntimeWorkerRef {
            runtime_id: "runtime".into(),
            worker_id: "source".into(),
        };
        let build = |edit, source, instruction: &str| {
            request(&ticket, edit, source, instruction.to_string()).unwrap()
        };
        let created = build(
            TicketItemCheckEdit::Create,
            source.clone(),
            "Check the item.",
        );
        let replay = build(
            TicketItemCheckEdit::Create,
            source.clone(),
            "Check the item.",
        );
        assert_eq!(created.job_id, replay.job_id);
        assert_eq!(
            created.fingerprint().unwrap(),
            replay.fingerprint().unwrap()
        );

        let edited = build(
            TicketItemCheckEdit::Edit {
                title_changed: true,
                body_changed: false,
                previous_title: Some("Different prior title".into()),
                previous_body: None,
                body_replacement: None,
            },
            source.clone(),
            "Check the item.",
        );
        let other_source = build(
            TicketItemCheckEdit::Create,
            RuntimeWorkerRef {
                worker_id: "other-source".into(),
                ..source.clone()
            },
            "Check the item.",
        );
        let other_instruction = build(
            TicketItemCheckEdit::Create,
            source,
            "Check with new policy.",
        );
        for different_intent in [edited, other_source, other_instruction] {
            assert_eq!(
                created.input["content_digest"],
                different_intent.input["content_digest"]
            );
            assert_ne!(created.job_id, different_intent.job_id);
        }
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
