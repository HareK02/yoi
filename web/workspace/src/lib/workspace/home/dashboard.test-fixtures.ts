// Deterministic read-only fixtures shared by Home component and browser tests.
export const fixturePage = (returned: number, more = false) => ({
  returned,
  limit: 5,
  sort: "priority",
  has_more: more,
  next_cursor: more ? "next" : null,
  source_limit: null,
  source_truncated: false,
});
const updated = (hour: number) =>
  `2026-09-28T${String(hour).padStart(2, "0")}:00:00Z`;
export function fixtureTicket(key: string, state: string, hour = 10) {
  return {
    id: `internal-${key}`,
    resource_key: key,
    title: key === "T-101"
      ? "Restore queued Worker submissions"
      : `${state} ticket ${key}`,
    state,
    priority: "normal",
    queued_at: null,
    queued_by: null,
    updated_at: updated(hour),
    record_source: "fixture",
    workspace_action_priority: state === "inprogress" || state === "queued"
      ? "active_work"
      : "background",
  };
}
export function fixtureDetail(key = "T-101") {
  const summary = fixtureTicket(key, key === "T-101" ? "inprogress" : "queued");
  const { workspace_action_priority: _, ...ticket } = summary;
  return {
    ...ticket,
    item_revision: "revision-1",
    created_at: updated(1),
    readiness: null,
    body: "Implementation in progress.",
    body_truncated: false,
    resolution: null,
    artifacts: [],
    artifact_count: 0,
    assignments: [],
    assignment_diagnostics: [],
    events: [],
    event_count: 0,
    event_page: fixturePage(0),
    implementation_reports: [],
    evidence: {
      has_merge_request: false,
      has_current_subject_ref: false,
      has_review_request: false,
      has_commit: false,
      review_status: null,
      approved_current_subject: false,
      review_after_rescope: false,
      unresolved_request_changes: false,
      complete_for_integration: false,
      missing: [],
    },
    current_coder: key === "T-101"
      ? {
        assignment_id: "a-1",
        runtime_id: "internal-runtime",
        worker_id: "internal-worker",
        worker_resource_key: "W-7",
      }
      : null,
    action_eligibility: {
      blockers: [],
      can_assign_orchestrator: false,
      can_queue: false,
      can_start_manual_coder: false,
      can_unassign_orchestrator: false,
      queue_tickets: [],
    },
    linked_objectives: [],
    merge_request: null,
    merge_requests: [],
    targets: [],
    risk_flags: [],
    relations: {
      outgoing: [],
      incoming: [],
      notices: [],
      blockers: key === "T-101"
        ? [{
          blocking_resource_key: "T-99",
          blocking_state: "ready",
          blocking_ticket: "internal-T-99",
          note: null,
          reason_kind: "dependency",
          relation_kind: "depends_on",
        }]
        : [],
    },
  };
}
export function dashboardFixture(
  path: string,
  search: URLSearchParams,
  workspaceId = "home-owner",
): unknown | null {
  const empty = workspaceId === "home-empty";
  const long = workspaceId === "home-long";
  if (path === "/tickets") {
    const state = search.get("states")!;
    const items = empty
      ? []
      : state === "inprogress,queued"
      ? [fixtureTicket("T-101", "inprogress"), fixtureTicket("T-102", "queued")]
      : state === "done"
      ? [fixtureTicket("T-103", "done", 12), fixtureTicket("T-104", "done", 7)]
      : [fixtureTicket("T-105", "closed", 14)];
    if (long && items[0]) {
      items[0].title = "長いTicket名 — ".repeat(8) +
        "preserve-the-full-title-without-horizontal-overflow";
    }
    return {
      workspace_id: workspaceId,
      limit: 5,
      items,
      page: fixturePage(items.length, !empty),
      invalid_records: [],
      record_authority: "fixture",
    };
  }
  if (/^\/tickets\/T-10[12]/.test(path)) {
    return fixtureDetail(path.includes("T-101") ? "T-101" : "T-102");
  }
  const objective = {
    id: "internal-objective",
    resource_key: "O-12",
    title: long ? "Objective ".repeat(18) : "Improve daily Workspace operation",
    state: "active",
    created_at: updated(1),
    updated_at: updated(13),
    summary: "",
    linked_tickets: [],
    record_source: "fixture",
  };
  if (path === "/objectives") {
    return {
      workspace_id: workspaceId,
      limit: 5,
      items: empty ? [] : [objective],
      invalid_records: [],
      record_authority: "fixture",
    };
  }
  if (path.startsWith("/objectives/O-12")) {
    const { summary: _, ...detail } = objective;
    return {
      ...detail,
      body: "# Objective detail\n\nHome dashboard navigation destination.",
      body_truncated: false,
      revision: "revision-1",
      linked_ticket_summaries: [],
      resources: [],
      events: [],
      event_page: fixturePage(0),
    };
  }
  if (path === "/merge-requests") {
    return {
      items: empty
        ? []
        : ["request_changes", "pending", "approved"].map((status, index) => ({
          summary: {
            merge_request_id: `internal-merge-${index}`,
            repository_key: "main",
            state: "open",
            selector_from: long
              ? "feature/" + "long-branch-".repeat(15)
              : ["fix/queue", "feat/home", "fix/sidebar"][index],
            selector_to: "develop",
            review_status: status,
            updated_at: updated(15 - index),
            review_excerpt: null,
            review_requested_at: updated(10),
            review_submitted_at: null,
            current_subject_ref: "abc123",
            source_ref_observation: { status: "observed" },
            integration_evidence_error: null,
            review_subject_ref: "abc123",
          },
          thread_event_count: 1,
          ticket_ids: [],
          ref_diagnostics: [],
        })),
      next_cursor: empty ? null : "older-reviews",
    };
  }
  return null;
}
