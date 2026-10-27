//! Asynchronous advisory checks of saved Ticket content using Backend Jobs.
//!
//! Authenticated Worker Create/EditItem paths freeze check input and reserve a Job after saving.
//! Failed preparation, execution, or delivery does not roll back the save or change case state
//! or assignments. Direct browser edits have no notification Worker and start no check.
//! Dispatch and timeout processing use bounded admission; overload retains the saved reservation
//! or deadline for diagnosis. An unknown DB outcome is never automatically retried.
//!
//! Input is limited to saved title/body, content digest, and the prior body/replacement context
//! needed for that edit, excluding conversation, user agreements, and external facts. Content
//! digests identify advisory freshness, not Job intent. Identical content with different edit
//! context or sources creates different Jobs; oversized input is not truncated and marked checked.
//!
//! Results are checked for unknown fields, duplicates, count/string limits, and quotes that are
//! exact substrings of the checked title/body. Invalid results end as invalid_result.
//! Before routing, the adapter rereads the Ticket and reports findings/content freshness.
//! Common routing records clean/stale suppression separately from Runtime acceptance. Once
//! a request is pinned, recovery reuses those bytes even if the Ticket later changes.
//! Advice is neither an approval gate nor implementation guidance;
//! no findings do not prove the entire specification correct.

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
pub enum TicketItemCheckFacts {
    Findings(String),
    NoFindings,
    Stale,
}

/// Domain facts about validated results and the current checked content.
/// The common routing policy owns whether these facts produce an advisory.
pub fn result_facts(
    acceptance: &BackendJobResultAcceptance,
    current_ticket: &ticket::Ticket,
) -> Result<TicketItemCheckFacts> {
    let input = parse_input(&acceptance.job.request.input)?;
    let result_value = acceptance.job.result.as_ref().ok_or_else(|| {
        Error::InvalidInput("completed Ticket item checker Job has no result".to_string())
    })?;
    let result = validate_result(&acceptance.job.request.input, result_value)?;
    if input.content_digest != content_digest(current_ticket)
        || input.ticket_id != current_ticket.meta.id
    {
        return Ok(TicketItemCheckFacts::Stale);
    }
    if result.findings.is_empty() {
        return Ok(TicketItemCheckFacts::NoFindings);
    }

    Ok(TicketItemCheckFacts::Findings(render_result(
        &acceptance.job.request.input,
        result_value,
    )?))
}

/// Stable rendering from the original checked snapshot. Used to pin pre-upgrade
/// unacknowledged requests without reinterpreting them against newer Ticket text.
pub(crate) fn render_result(input: &Value, result: &Value) -> Result<String> {
    let checked = parse_input(input)?;
    let result = validate_result(input, result)?;
    if result.findings.is_empty() {
        return Err(Error::InvalidInput(
            "clean checker result has no advisory body".into(),
        ));
    }
    let mut content = format!(
        "[Ticket item checker advisory]\nTicket: {}\nContent digest: {}\n\nThis is a post-save advisory based only on the inspected Ticket text. It is not a new user request, approval gate, workflow blocker, or implementation instruction.\n",
        checked.ticket_resource_key, checked.content_digest
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
    Ok(content)
}

// Supplied model outputs exercise host validation/rendering, not LLM judgment.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) struct FakeTicketCheckerModel;

    impl FakeTicketCheckerModel {
        pub(crate) fn respond(request: &BackendJobRequest) -> Value {
            let input = parse_input(&request.input).unwrap();
            assert!(request.instruction.contains("writer_scope"));
            assert!(request.instruction.contains("internal_inconsistency"));
            assert!(request.instruction.contains("ambiguous_boundary"));
            assert!(
                request
                    .instruction
                    .contains("Production deployment requires approval")
            );
            assert!(request.instruction.contains("background, quotations"));
            if input
                .body
                .contains("This time I only create the Ticket; I will not implement it.")
                && !input.body.starts_with("Background bug example:")
            {
                serde_json::json!({"findings": [{
                    "category": "writer_scope",
                    "quote": "I will not implement it",
                    "reason": "The active requirement records the writer's own implementation boundary.",
                    "suggestion": "Remove the writer-specific action boundary."
                }]})
            } else if input.body.contains("The rollout region is undecided.")
                && input.body.contains("The rollout region is us-east-1.")
            {
                serde_json::json!({"findings": [{
                    "category": "internal_inconsistency",
                    "quote": "The rollout region is undecided.",
                    "reason": "The same active text also settles the rollout region as us-east-1.",
                    "suggestion": "Choose either the unresolved or settled rollout-region statement."
                }]})
            } else if input.body == "Implementation stops after Ticket creation." {
                serde_json::json!({"findings": [{
                    "category": "ambiguous_boundary",
                    "quote": "Implementation stops after Ticket creation.",
                    "reason": "The text alone does not establish whether this is a delivery constraint or the writer's current action boundary.",
                    "suggestion": "Confirm whether this limits the product workflow or only the current writer."
                }]})
            } else {
                serde_json::json!({"findings": []})
            }
        }
    }

    pub(crate) struct Case {
        pub(crate) name: &'static str,
        pub(crate) body: &'static str,
        pub(crate) advisory_quote: Option<&'static str>,
    }
    pub(crate) fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "writer-scope-positive",
                body: "This time I only create the Ticket; I will not implement it.",
                advisory_quote: Some("I will not implement it"),
            },
            Case {
                name: "production-approval-negative",
                body: "Production deployment requires approval.",
                advisory_quote: None,
            },
            Case {
                name: "quoted-background-negative",
                body: "Background bug example: a generated Ticket once said “I will not implement it.” Do not reproduce that sentence in generated requirements.",
                advisory_quote: None,
            },
            Case {
                name: "unresolved-versus-settled-positive",
                body: "The rollout region is undecided. The rollout region is us-east-1.",
                advisory_quote: Some("The rollout region is undecided."),
            },
            Case {
                name: "ambiguous-boundary-confirmation",
                body: "Implementation stops after Ticket creation.",
                advisory_quote: Some("Implementation stops after Ticket creation."),
            },
        ]
    }
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

    fn ticket_fixture() -> ticket::Ticket {
        ticket::Ticket {
            meta: ticket::TicketMeta {
                id: "ticket-internal".into(),
                resource_key: Some("T-42".into()),
                slug: "check-this-content".into(),
                title: "Check this content".into(),
                status: ticket::ExtensibleTicketStatus::Open,
                kind: "task".into(),
                priority: "P2".into(),
                labels: vec![],
                created_at: None,
                created_by: None,
                updated_at: None,
                assignee: None,
                readiness: None,
                risk_flags: vec![],
                workflow_state: ticket::TicketState::Open,
                workflow_state_explicit: true,
                queued_by: None,
                queued_at: None,
                targets: vec![],
                raw: Default::default(),
            },
            document: ticket::TicketDocument {
                body: ticket::MarkdownText::new("Check the saved content."),
                raw_frontmatter: Default::default(),
            },
            tags: vec![],
            events: vec![],
            artifacts: vec![],
            relations: Default::default(),
            resolution: None,
            closure_operation_key: None,
            current_closure: None,
            case_history: vec![],
            legacy_workflow_snapshot: None,
        }
    }

    fn completed_acceptance(
        request: BackendJobRequest,
        result: Value,
    ) -> BackendJobResultAcceptance {
        use crate::backend_job::{
            BackendJobAttemptRecord, BackendJobAttemptState, BackendJobRecord, BackendJobState,
        };
        let digest = crate::backend_job::result_digest(&result, request.limits.max_result_bytes)
            .unwrap()
            .0;
        let attempt = BackendJobAttemptRecord {
            workspace_id: "checker-unit".into(),
            job_id: request.job_id.clone(),
            attempt_id: "checker-attempt".into(),
            attempt: 1,
            input_digest: request.input_digest().unwrap(),
            state: BackendJobAttemptState::Completed,
            worker: Some(RuntimeWorkerRef::new("runtime", "checker")),
            runtime_run_id: Some("run".into()),
            dispatched_at: Some("1".into()),
            deadline_at: "3".into(),
            result: Some(result.clone()),
            result_digest: Some(digest.clone()),
            failure_category: None,
            failure_detail: None,
            worker_cleanup_state: None,
            worker_cleanup_failure_category: None,
            worker_cleanup_failure_detail: None,
            worker_cleanup_updated_at: None,
            worker_cleanup_completed_at: None,
            created_at: "1".into(),
            updated_at: "2".into(),
            completed_at: Some("2".into()),
        };
        BackendJobResultAcceptance {
            job: BackendJobRecord {
                workspace_id: "checker-unit".into(),
                intent_fingerprint: request.fingerprint().unwrap(),
                input_digest: request.input_digest().unwrap(),
                input_disposition: crate::backend_job::JobInputDisposition::Available,
                request,
                state: BackendJobState::Completed,
                current_attempt: 1,
                result: Some(result),
                result_digest: Some(digest),
                failure_category: None,
                failure_detail: None,
                created_at: "1".into(),
                updated_at: "2".into(),
                completed_at: Some("2".into()),
            },
            attempt,
            replayed: false,
        }
    }

    #[test]
    fn supplied_model_classifications_validate_and_render_advisories_or_no_findings() {
        let instruction = worker::PromptCatalog::builtins_only()
            .unwrap()
            .ticket_item_checker()
            .unwrap();
        for case in test_support::cases() {
            let mut ticket = ticket_fixture();
            ticket.meta.title = format!("Boundary case: {}", case.name);
            ticket.document.body = ticket::MarkdownText::new(case.body);
            let request = request(
                &ticket,
                TicketItemCheckEdit::Create,
                RuntimeWorkerRef::new("runtime", "source"),
                instruction.clone(),
            )
            .unwrap();
            let result = test_support::FakeTicketCheckerModel::respond(&request);
            let checked = validate_result(&request.input, &result).unwrap();
            let acceptance = completed_acceptance(request, result);
            match (
                case.advisory_quote,
                result_facts(&acceptance, &ticket).unwrap(),
            ) {
                (Some(quote), TicketItemCheckFacts::Findings(advisory)) => {
                    assert_eq!(checked.findings.len(), 1, "{}", case.name);
                    assert!(
                        advisory.contains("[Ticket item checker advisory]"),
                        "{}",
                        case.name
                    );
                    assert!(advisory.contains(quote), "{}", case.name);
                }
                (None, TicketItemCheckFacts::NoFindings) => {
                    assert!(checked.findings.is_empty(), "{}", case.name);
                }
                (_, actual) => panic!("{}: unexpected classification {actual:?}", case.name),
            }
        }
    }

    #[test]
    fn checker_job_identity_reuses_exact_intent_but_separates_edits_and_sources() {
        let ticket = ticket_fixture();
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
