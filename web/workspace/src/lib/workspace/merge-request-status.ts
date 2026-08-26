import type {
  MergeRequestDetail,
  MergeRequestThreadEvent,
} from "./api/merge-requests.ts";

function isCurrentSourceEvent(
  event: MergeRequestThreadEvent,
  source: string,
): boolean {
  return event.subject_ref === source;
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
  const source = mergeRequest.source.ref;
  if (!source) return "Source review unavailable: selector_from is unresolved.";

  const effectiveReview = [...mergeRequest.thread].reverse().find((event) => {
    if (event.kind !== "review" || !isCurrentSourceEvent(event, source)) {
      return false;
    }
    return !mergeRequest.thread.some(
      (candidate) =>
        candidate.kind === "review_revoked" &&
        candidate.review_event_id === event.event_id,
    );
  });
  if (effectiveReview) {
    return effectiveReview.decision === "approve"
      ? `Current source approved at exact ref ${source}.`
      : `Current source requests changes at exact ref ${source}.`;
  }

  const latestEvidence = [...mergeRequest.thread].reverse().find(
    (event) =>
      (event.kind === "review" || event.kind === "review_requested") &&
      typeof event.subject_ref === "string",
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
