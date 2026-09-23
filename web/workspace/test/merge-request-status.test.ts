/// <reference lib="deno.ns" />

import type {
  MergeRequestDetail,
  MergeRequestThreadEvent,
} from "../src/lib/workspace/api/merge-requests.ts";
import { sourceReviewFreshness } from "../src/lib/workspace/merge-request-status.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

const worker = { runtime_id: "runtime", worker_id: "worker" };
const createdAt = "2026-09-01T00:00:00Z";

type ReviewRequested = Extract<
  MergeRequestThreadEvent,
  { kind: "review_requested" }
>;
type Review = Extract<MergeRequestThreadEvent, { kind: "review" }>;
type ReviewRevoked = Extract<
  MergeRequestThreadEvent,
  { kind: "review_revoked" }
>;
type ReviewCancelled = Extract<
  MergeRequestThreadEvent,
  { kind: "review_cancelled" }
>;

function reviewRequested(
  eventId: string,
  subjectRef: string,
): ReviewRequested {
  return {
    kind: "review_requested",
    event_id: eventId,
    sequence: 1,
    subject_ref: subjectRef,
    requested_by: worker,
    reviewer: worker,
    created_at: createdAt,
  };
}

function review(
  eventId: string,
  requestEventId: string,
  subjectRef: string,
): Review {
  return {
    kind: "review",
    event_id: eventId,
    sequence: 1,
    request_event_id: requestEventId,
    subject_ref: subjectRef,
    decision: "approve",
    body: "approved",
    findings: [],
    reviewer: worker,
    created_at: createdAt,
  };
}

function reviewRevoked(
  eventId: string,
  reviewEventId: string,
  subjectRef: string,
): ReviewRevoked {
  return {
    kind: "review_revoked",
    event_id: eventId,
    sequence: 1,
    review_event_id: reviewEventId,
    subject_ref: subjectRef,
    reason: "superseded",
    revoked_by: worker,
    created_at: createdAt,
  };
}

function reviewCancelled(
  eventId: string,
  requestEventId: string,
  subjectRef: string,
): ReviewCancelled {
  return {
    kind: "review_cancelled",
    event_id: eventId,
    sequence: 1,
    request_event_id: requestEventId,
    subject_ref: subjectRef,
    reason: "cancelled",
    created_at: createdAt,
  };
}

function detail(thread: MergeRequestThreadEvent[]): MergeRequestDetail {
  return {
    merge_request_id: "MR-1",
    workspace_id: "W",
    repository_key: "main",
    ticket_ids: ["T-1"],
    selector_from: "work/ticket",
    selector_to: "develop",
    state: "open",
    created_at: createdAt,
    updated_at: createdAt,
    source: {
      status: "known",
      ref: "source-2",
      observed_at: createdAt,
    },
    target: {
      status: "known",
      ref: "target-2",
      observed_at: createdAt,
    },
    linked_tickets: [{ ticket_id: "T-1", key: "T-1" }],
    thread,
  };
}

Deno.test("revoked review requires a fresh review instead of appearing pending", () => {
  const mergeRequest = detail([
    reviewRequested("request-1", "source-2"),
    review("review-1", "request-1", "source-2"),
    reviewRevoked("revoke-1", "review-1", "source-2"),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Fresh source review required: no effective verdict exists for source-2.",
  );
});

Deno.test("unresolved review request for the current source is pending", () => {
  const mergeRequest = detail([
    reviewRequested("request-2", "source-2"),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Current source review pending for exact ref source-2.",
  );
});

Deno.test("completed or cancelled request is not projected as pending", () => {
  const approved = detail([
    reviewRequested("request-3", "source-2"),
    review("review-3", "request-3", "source-2"),
  ]);
  assertEquals(
    sourceReviewFreshness(approved),
    "Current source approved at exact ref source-2.",
  );

  const cancelled = detail([
    reviewRequested("request-4", "source-2"),
    reviewCancelled("cancel-4", "request-4", "source-2"),
  ]);
  assertEquals(
    sourceReviewFreshness(cancelled),
    "Fresh source review required: no effective verdict exists for source-2.",
  );
});

Deno.test("source movement explains the exact stale and current refs", () => {
  const mergeRequest = detail([
    review("review-old", "request-old", "source-1"),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Fresh source review required: selector_from moved from source-1 to source-2.",
  );
});
