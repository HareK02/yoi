/** Review status comes from the Backend's complete-evidence summary, never the
 * returned history page. Unknown source observations remain unknown; target-only
 * movement does not invalidate unchanged-source approval.
 */
import type {
  TicketEvidenceSummary,
  TicketMergeRequestSummary,
} from "#lib/generated/ticket-api.ts";
import type { MergeRequestDetail } from "./api/merge-requests.ts";

export function sourceReviewFreshness(
  mergeRequest: MergeRequestDetail,
): string {
  const source = mergeRequest.source.ref;
  if (mergeRequest.state === "merged") {
    if (mergeRequest.source.diagnostic) {
      return `Immutable integration evidence unavailable: ${mergeRequest.source.diagnostic.code}.`;
    }
    if (mergeRequest.source.status !== "known" || !source) {
      return "Immutable integration evidence unavailable: no validated persisted source.";
    }
    const approval = mergeRequest.summary?.review;
    return approval && mergeRequest.summary.review_status === "approved"
      ? `Recorded integration approval ${approval.event.event_id} for immutable merged source ${source}.`
      : `Integration approval unavailable for immutable merged source ${source}.`;
  }
  if (mergeRequest.source.status !== "known" || !source) {
    return mergeRequest.source.diagnostic
      ? `Source review unavailable: ${mergeRequest.source.diagnostic.code}.`
      : "Source review unavailable: selector_from is unresolved.";
  }

  const summary = mergeRequest.summary;
  // Do not fall back to partial-thread inference for an older/malformed response.
  if (!summary || summary.review_status === "unknown") {
    return "Source review unavailable: authoritative summary is unavailable.";
  }
  switch (summary.review_status) {
    case "approved":
      return `Current source approved at exact ref ${source}.`;
    case "request_changes":
      return `Current source requests changes at exact ref ${source}.`;
    case "pending":
      return `Current source review pending for exact ref ${source}.`;
  }
  const previous = summary.latest_review_subject_ref;
  if (previous && previous !== source) {
    return `Fresh source review required: selector_from moved from ${previous} to ${source}.`;
  }
  return `Fresh source review required: no effective verdict exists for ${source}.`;
}

// Source/integration approval can survive rescope; only this exact item snapshot
// and linked-result attestation approves the current Ticket requirements.
export function currentRequirementApprovalStatus(
  evidence: TicketEvidenceSummary,
): string {
  return evidence.review_after_rescope ? "approved" : "not established";
}

export function summarySourceReviewStatus(
  mergeRequest: TicketMergeRequestSummary,
): string {
  const source = mergeRequest.current_subject_ref;
  if (mergeRequest.state === "merged") {
    if (!source) return "Immutable merged source unavailable.";
    if (
      mergeRequest.integration_evidence_error !== null ||
      mergeRequest.review_status !== "approved" ||
      mergeRequest.review_subject_ref !== source
    ) {
      return `Integration approval unavailable for immutable merged source ${source}.`;
    }
    return `Integration approval recorded for immutable merged source ${source}.`;
  }

  const observation = mergeRequest.source_ref_observation;
  if (observation.status === "unavailable") {
    return `Live source observation unavailable: ${observation.code}.`;
  }
  if (observation.status === "not_required") {
    return "Live source observation not required.";
  }
  if (!source) {
    return "Live source review unavailable: no source ref was observed.";
  }
  if (mergeRequest.review_subject_ref === source) {
    return `Live source review: ${mergeRequest.review_status} for exact ref ${source}.`;
  }
  if (mergeRequest.review_subject_ref) {
    return `Fresh live source review required: selector_from moved from ${mergeRequest.review_subject_ref} to ${source}.`;
  }
  return `Fresh live source review required: no effective verdict exists for ${source}.`;
}

export function targetIntegrationStatus(
  mergeRequest: MergeRequestDetail,
): string {
  if (mergeRequest.state === "merged") {
    return "Target integration recorded by CompleteMergeRequest.";
  }
  if (!mergeRequest.target.ref) {
    return "Target integration unavailable: selector_to is unresolved.";
  }
  return `Target integration awaits Orchestrator action at ${mergeRequest.target.ref}. Target-only movement refreshes integration evidence; it does not invalidate approval for an unchanged source.`;
}
