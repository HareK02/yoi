import type {
  MergeRequestDetailResponse,
  MergeRequestListItem,
} from "#lib/generated/ticket-api.ts";

export function mergeRequestDetailFixture(): MergeRequestDetailResponse {
  const worker = { runtime_id: "fixture-runtime", worker_id: "fixture-worker" };
  const time = "2026-11-17T10:00:00Z";
  return {
    merge_request_id: "MR-1",
    workspace_id: "tickets",
    repository_key: "main",
    selector_from: "work/integration-source",
    selector_to: "refs/heads/develop",
    state: { kind: "integration", started_by: "approval-1" },
    created_at: time,
    updated_at: time,
    ticket_ids: ["T-2"],
    linked_tickets: [{ ticket_id: "T-2", key: "T-2" }],
    source: { status: "known", ref: "a".repeat(40), observed_at: time },
    target: {
      status: "unknown",
      ref: null,
      observed_at: time,
      diagnostic: {
        code: "provider_unavailable",
        message: "Provider observation unavailable",
      },
    },
    view: "detail",
    summary: {
      review_status: "request_changes",
      review: null,
      latest_review_subject_ref: "a".repeat(40),
      latest_event: null,
      thread_event_count: 1,
      readiness: {
        ready: false,
        blockers: ["target_unknown"],
        subject_ref: "a".repeat(40),
      },
    },
    worker_resource_key: "W-2",
    worker_assignment: {
      event_id: "assignment-1",
      sequence: 1,
      operation_id: "assign-1",
      previous_operation_id: null,
      worker,
      assigned_by: worker,
      created_at: time,
    },
    event_page: {
      omitted: false,
      has_more: false,
      next_after: null,
      limit: 100 as MergeRequestDetailResponse["event_page"]["limit"],
    },
    thread: [{
      kind: "comment",
      event_id: "comment-1",
      sequence: 2,
      author: null,
      body:
        "Publication outcome is unknown. Inspect the current target before deciding the next action.",
      created_at: time,
      operation_id: null,
      owner_operation_id: null,
    }],
  };
}

export function mergeRequestFindingsFixture(): MergeRequestDetailResponse {
  const detail = mergeRequestDetailFixture();
  detail.thread = [{
    kind: "review",
    event_id: "findings-review",
    sequence: 50,
    request_event_id: "request-1",
    reviewer: { runtime_id: "fixture-runtime", worker_id: "reviewer" },
    created_at: detail.updated_at,
    subject_ref: detail.source.ref!,
    ticket_content_digest: "digest",
    ticket_merge_request_subjects: [],
    decision: "request_changes",
    body: "",
    findings: [
      {
        severity: "blocker",
        code: "publication_rejection",
        path: "crates/worker-runtime/src/working_directory/publication.rs",
        line: 350,
        body: "Preserve the explicit rejection in the MR thread.\n" +
          "Long finding details must remain readable. ".repeat(30),
      },
      {
        severity: "major",
        body: "Inspect the current target before deciding the next action.\n" +
          "A second long finding must retain its own body and location. "
            .repeat(20),
      },
      {
        severity: "minor",
        code: null,
        path: `crates/${"long-path-segment-".repeat(10)}/publication.rs`,
        line: null,
        body: "A path can be supplied without a line.",
      },
      {
        severity: "note",
        line: 12,
        body: "A line can be supplied without a path.",
      },
    ],
  }];
  detail.summary.thread_event_count = 51;
  detail.event_page = { ...detail.event_page, has_more: true, next_after: 50 };
  return detail;
}

export function mergeRequestListFixture(): MergeRequestListItem {
  const detail = mergeRequestDetailFixture();
  return {
    summary: {
      merge_request_id: detail.merge_request_id,
      repository_key: detail.repository_key,
      selector_from: detail.selector_from,
      selector_to: detail.selector_to,
      state: detail.state,
      updated_at: detail.updated_at,
      current_subject_ref: detail.source.ref,
      review_status: detail.summary.review_status,
      review_subject_ref: detail.source.ref,
      source_ref_observation: { status: "observed" },
      integration_evidence_error: null,
    },
    worker_assignment: detail.worker_assignment,
    worker_resource_key: "W-2",
    ticket_ids: detail.ticket_ids,
    thread_event_count: 1,
    ref_diagnostics: [],
  };
}
