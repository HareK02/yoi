declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: URL): Promise<string>;
};

import type { TicketMergeRequestSummary } from "#lib/generated/ticket-api.ts";
import { fixtureDetail } from "./home/dashboard.test-fixtures.ts";
import { parseTicketDetail } from "./api/ticket-browser.ts";
import {
  currentRequirementApprovalStatus,
  sourceReviewFreshness,
  summarySourceReviewStatus,
} from "./merge-request-status.ts";
import type { MergeRequestDetail } from "./api/merge-requests.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, received ${
        JSON.stringify(actual)
      }`,
    );
  }
}

const summary: TicketMergeRequestSummary = {
  merge_request_id: "mr-1",
  repository_key: "main",
  state: "open",
  selector_from: "work/T-1",
  selector_to: "develop",
  updated_at: "2026-10-08T00:00:00Z",
  current_subject_ref: "source-1",
  source_ref_observation: { status: "observed" },
  integration_evidence_error: null,
  review_status: "approved",
  review_subject_ref: "source-1",
};

Deno.test("unavailable live observation shows its error rather than claiming review is stale or pending", () => {
  assertEquals(
    summarySourceReviewStatus({
      ...summary,
      current_subject_ref: null,
      source_ref_observation: {
        status: "unavailable",
        code: "repository_unavailable",
      },
      review_status: "pending",
    }),
    "Live source observation unavailable: repository_unavailable.",
  );
});

Deno.test("merged summary labels approval as immutable persisted evidence without live observation", () => {
  assertEquals(
    summarySourceReviewStatus({
      ...summary,
      state: "merged",
      source_ref_observation: { status: "not_required" },
    }),
    "Integration approval recorded for immutable merged source source-1.",
  );
});

Deno.test("merged evidence error does not claim a valid integration approval", () => {
  assertEquals(
    summarySourceReviewStatus({
      ...summary,
      state: "merged",
      source_ref_observation: { status: "not_required" },
      integration_evidence_error: "approval_event_missing",
    }),
    "Integration approval unavailable for immutable merged source source-1.",
  );
});

Deno.test("observed live source movement requires fresh review but an unchanged ref retains approval", () => {
  assertEquals(
    summarySourceReviewStatus(summary),
    "Live source review: approved for exact ref source-1.",
  );
  assertEquals(
    summarySourceReviewStatus({ ...summary, current_subject_ref: "source-2" }),
    "Fresh live source review required: selector_from moved from source-1 to source-2.",
  );
});

Deno.test("done Ticket can retain immutable approval while current requirement evidence is incomplete", () => {
  const ticket = parseTicketDetail({
    ...fixtureDetail(),
    state: "done",
    evidence: {
      ...fixtureDetail().evidence,
      has_merge_request: true,
      has_current_subject_ref: true,
      has_commit: true,
      has_review_request: true,
      review_status: "pending",
      approved_current_subject: true,
      review_after_rescope: false,
      missing: ["review_after_rescope"],
    },
    merge_requests: [{
      ...summary,
      state: "merged",
      source_ref_observation: { status: "not_required" },
    }],
  });
  assertEquals(ticket.state, "done");
  assertEquals(ticket.evidence.complete_for_integration, false);
  assertEquals(
    currentRequirementApprovalStatus(ticket.evidence),
    "not established",
  );
  const recovered = parseTicketDetail({
    ...ticket,
    evidence: {
      ...ticket.evidence,
      review_after_rescope: true,
      review_status: "approved",
      complete_for_integration: true,
      missing: [],
    },
  });
  assertEquals(recovered.evidence.approved_current_subject, true);
  assertEquals(
    currentRequirementApprovalStatus(recovered.evidence),
    "approved",
  );
  assertEquals(recovered.state, "done");
  assertEquals(
    summarySourceReviewStatus(ticket.merge_requests[0]!),
    "Integration approval recorded for immutable merged source source-1.",
  );
});

Deno.test("Ticket route separates current requirement evidence from immutable integration approval without cancelling done", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
      import.meta.url,
    ),
  );
  for (
    const required of [
      "Current requirement evidence",
      "ticket.evidence.complete_for_integration",
      "Current requirement approval:</strong> {currentRequirementApprovalStatus(ticket.evidence)}",
      "ticket.evidence.missing",
      "summarySourceReviewStatus(mergeRequest)",
      "Immutable integration evidence",
      "Integration evidence error:",
      '{#if ticket.state === "done"}',
      "Incomplete current requirement evidence does not cancel recorded completion.",
    ]
  ) {
    if (!source.includes(required)) {
      throw new Error(`Ticket route must include ${required}`);
    }
  }
});

Deno.test("Merge Request routes distinguish recorded approval from live source observation and expose errors", async () => {
  const list = await Deno.readTextFile(
    new URL(
      "../../routes/w/[workspaceId]/merge-requests/+page.svelte",
      import.meta.url,
    ),
  );
  assertEquals(list.includes("summarySourceReviewStatus(mergeRequest)"), true);
  assertEquals(list.includes("Integration evidence error:"), true);
  const detail = await Deno.readTextFile(
    new URL(
      "../../routes/w/[workspaceId]/merge-requests/[mergeRequestId]/+page.svelte",
      import.meta.url,
    ),
  );
  assertEquals(detail.includes("Recorded integration approval"), true);
  assertEquals(detail.includes("Live source ref"), true);
  assertEquals(detail.includes("Source observation error"), true);
});

Deno.test("merged detail uses backend validated immutable evidence even with a partial thread", () => {
  const detail: MergeRequestDetail = {
    merge_request_id: "mr-1",
    workspace_id: "workspace-a",
    repository_key: "main",
    ticket_ids: [],
    linked_tickets: [],
    selector_from: "work/T-1",
    selector_to: "develop",
    state: "merged",
    created_at: summary.updated_at,
    updated_at: summary.updated_at,
    source: {
      status: "known",
      ref: "source-1",
      observed_at: summary.updated_at,
    },
    target: {
      status: "known",
      ref: "target-2",
      observed_at: summary.updated_at,
    },
    thread: [{
      kind: "merge",
      event_id: "merge-1",
      sequence: 3,
      created_at: summary.updated_at,
      operation_id: "operation-1",
      approval_event_id: "approval-1",
      approved_source_ref: "source-1",
      target_ref_before: "target-1",
      target_ref_after: "target-2",
      strategy: "fast_forward",
      resolution: "none",
      merged_by: { runtime_id: "runtime-a", worker_id: "worker-a" },
    }],
  };
  assertEquals(
    sourceReviewFreshness(detail),
    "Recorded integration approval approval-1 for immutable merged source source-1.",
  );
  assertEquals(
    sourceReviewFreshness({ ...detail, thread: [] }),
    "Recorded integration approval for immutable merged source source-1.",
  );
  assertEquals(
    sourceReviewFreshness({
      ...detail,
      source: {
        ...detail.source,
        status: "unknown",
        diagnostic: { code: "approval_revoked", message: "Revoked" },
      },
    }),
    "Immutable integration evidence unavailable: approval_revoked.",
  );
  assertEquals(
    sourceReviewFreshness({
      ...detail,
      source: { ...detail.source, status: "unknown", ref: null },
    }),
    "Immutable integration evidence unavailable: no validated persisted source.",
  );
});
