import type {
  TicketEvidenceSummary,
  TicketMergeRequestSummary,
} from "#lib/generated/ticket-api.ts";
import type {
  MergeRequestDetail,
  MergeRequestThreadEvent,
} from "./api/merge-requests.ts";

type ReviewEvent = Extract<MergeRequestThreadEvent, { kind: "review" }>;
type SourceEvidenceEvent = Extract<
  MergeRequestThreadEvent,
  { kind: "review" | "review_requested" }
>;

function isCurrentSourceEvent(
  event: MergeRequestThreadEvent,
  source: string,
): boolean {
  return "subject_ref" in event && event.subject_ref === source;
}

function requestHasTerminalOutcome(
  thread: MergeRequestThreadEvent[],
  requestEventId: unknown,
): boolean {
  return thread.some((event) =>
    (event.kind === "review" || event.kind === "review_cancelled") &&
    event.request_event_id === requestEventId
  );
}

export function sourceReviewFreshness(
  mergeRequest: MergeRequestDetail,
): string {
  if (mergeRequest.state === "merged") {
    // The Backend validates the full persisted result before paging the thread.
    // A merge event in a page alone is not evidence of a valid approval.
    if (mergeRequest.source.diagnostic) {
      return `Immutable integration evidence unavailable: ${mergeRequest.source.diagnostic.code}.`;
    }
    const source = mergeRequest.source.ref;
    if (mergeRequest.source.status !== "known" || !source) {
      return "Immutable integration evidence unavailable: no validated persisted source.";
    }
    const merge = [...mergeRequest.thread].reverse().find(
      (event): event is Extract<MergeRequestThreadEvent, { kind: "merge" }> =>
        event.kind === "merge",
    );
    return merge
      ? `Recorded integration approval ${merge.approval_event_id} for immutable merged source ${source}.`
      : `Recorded integration approval for immutable merged source ${source}.`;
  }
  const source = mergeRequest.source.ref;
  if (!source) return "Source review unavailable: selector_from is unresolved.";

  const effectiveReview = [...mergeRequest.thread].reverse().find(
    (event): event is ReviewEvent => {
      if (event.kind !== "review" || !isCurrentSourceEvent(event, source)) {
        return false;
      }
      return !mergeRequest.thread.some(
        (candidate) =>
          candidate.kind === "review_revoked" &&
          candidate.review_event_id === event.event_id,
      );
    },
  );
  if (effectiveReview) {
    return effectiveReview.decision === "approve"
      ? `Current source approved at exact ref ${source}.`
      : `Current source requests changes at exact ref ${source}.`;
  }

  const latestEvidence = [...mergeRequest.thread].reverse().find(
    (event): event is SourceEvidenceEvent =>
      event.kind === "review" || event.kind === "review_requested",
  );
  if (latestEvidence?.subject_ref && latestEvidence.subject_ref !== source) {
    return `Fresh source review required: selector_from moved from ${latestEvidence.subject_ref} to ${source}.`;
  }

  const pendingRequest = [...mergeRequest.thread].reverse().find((event) =>
    event.kind === "review_requested" &&
    isCurrentSourceEvent(event, source) &&
    !requestHasTerminalOutcome(mergeRequest.thread, event.event_id)
  );
  if (pendingRequest) {
    return `Current source review pending for exact ref ${source}.`;
  }

  return `Fresh source review required: no effective verdict exists for ${source}.`;
}

// Source/integration approval can survive rescope; only this exact revision
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
