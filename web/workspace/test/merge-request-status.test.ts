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

function event(
  kind: string,
  fields: Record<string, unknown>,
): MergeRequestThreadEvent {
  return { kind, sequence: 1, at: "2026-09-01T00:00:00Z", ...fields };
}

function detail(thread: MergeRequestThreadEvent[]): MergeRequestDetail {
  return {
    merge_request_id: "MR-1",
    workspace_id: "W",
    repository_id: "main",
    ticket_ids: ["T-1"],
    selector_from: "work/ticket",
    selector_to: "develop",
    state: "open",
    opened_by: {
      runtime_id: "runtime",
      worker_id: "worker",
      assignment_id: "assignment",
    },
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-01T00:00:00Z",
    source: {
      status: "known",
      ref: "source-2",
      observed_at: "2026-09-01T00:00:00Z",
    },
    target: {
      status: "known",
      ref: "target-2",
      observed_at: "2026-09-01T00:00:00Z",
    },
    linked_tickets: [{ ticket_id: "T-1", key: "T-1" }],
    thread,
  };
}

Deno.test("revoked review requires a fresh review instead of appearing pending", () => {
  const mergeRequest = detail([
    event("review_requested", {
      event_id: "request-1",
      subject_ref: "source-2",
    }),
    event("review", {
      event_id: "review-1",
      request_event_id: "request-1",
      subject_ref: "source-2",
      decision: "approve",
    }),
    event("review_revoked", {
      event_id: "revoke-1",
      review_event_id: "review-1",
    }),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Fresh source review required: no effective verdict exists for source-2.",
  );
});

Deno.test("unresolved review request for the current source is pending", () => {
  const mergeRequest = detail([
    event("review_requested", {
      event_id: "request-2",
      subject_ref: "source-2",
    }),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Current source review pending for exact ref source-2.",
  );
});

Deno.test("completed or cancelled request is not projected as pending", () => {
  const approved = detail([
    event("review_requested", {
      event_id: "request-3",
      subject_ref: "source-2",
    }),
    event("review", {
      event_id: "review-3",
      request_event_id: "request-3",
      subject_ref: "source-2",
      decision: "approve",
    }),
  ]);
  assertEquals(
    sourceReviewFreshness(approved),
    "Current source approved at exact ref source-2.",
  );

  const cancelled = detail([
    event("review_requested", {
      event_id: "request-4",
      subject_ref: "source-2",
    }),
    event("review_cancelled", {
      event_id: "cancel-4",
      request_event_id: "request-4",
      subject_ref: "source-2",
    }),
  ]);
  assertEquals(
    sourceReviewFreshness(cancelled),
    "Fresh source review required: no effective verdict exists for source-2.",
  );
});

Deno.test("source movement explains the exact stale and current refs", () => {
  const mergeRequest = detail([
    event("review", {
      event_id: "review-old",
      request_event_id: "request-old",
      subject_ref: "source-1",
      decision: "approve",
    }),
  ]);

  assertEquals(
    sourceReviewFreshness(mergeRequest),
    "Fresh source review required: selector_from moved from source-1 to source-2.",
  );
});
